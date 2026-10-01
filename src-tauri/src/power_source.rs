// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reading the machine's power source for the On battery policy (#432): the I/O edge only. Every
//! decision about what a reading MEANS lives in [`crate::power`]; this file answers "AC, battery, or
//! can't tell — and what charge" and nothing else.
//!
//! The contract is the hardware scan's: **a failure is Unknown, never an error and never a panic.**
//! Unknown is the safe answer because the policy treats it as mains — PM never sends work off the
//! machine on the strength of a reading it could not take.
//!
//! Per OS: Linux reads sysfs, Windows asks `GetSystemPowerStatus`, macOS asks IOKit's power-source
//! API. Each platform's pure mapper is compiled on that platform AND under `test`, so every
//! platform's test run checks every mapper (the arrangement `hardware.rs` uses), while clippy's
//! `-D warnings` on the other two CI jobs sees no dead code.
//!
//! The reads are blocking — a Linux battery read is an ACPI `_BST` evaluation, about 65 ms cold —
//! so the watcher runs them under `spawn_blocking` with a timeout.

use crate::power::PowerReading;
#[cfg(any(target_os = "linux", windows, target_os = "macos", test))]
use crate::power::PowerSource;

/// Read the machine now. Blocking. Returns [`PowerReading::default`] (Unknown) on any failure.
pub fn read() -> PowerReading {
    #[cfg(target_os = "linux")]
    {
        linux_read(std::path::Path::new("/sys/class/power_supply"))
    }
    #[cfg(windows)]
    {
        read_windows()
    }
    #[cfg(target_os = "macos")]
    {
        read_macos()
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        PowerReading::default()
    }
}

// --- Linux: /sys/class/power_supply -----------------------------------------------------------

/// One `/sys/class/power_supply/<name>` entry, as raw trimmed attribute text. `None` is a file that
/// is absent or unreadable — which the rules below treat as "says nothing", never as zero.
///
/// There is no `uevent` here, on purpose: it carries the battery's serial number, model and
/// manufacturer, none of which PM needs and none of which should ever reach a log or a fixture in a
/// public repo.
#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, Default)]
struct Supply {
    kind: Option<String>,
    scope: Option<String>,
    online: Option<String>,
    present: Option<String>,
    status: Option<String>,
    capacity: Option<String>,
    energy_now: Option<String>,
    energy_full: Option<String>,
    charge_now: Option<String>,
    charge_full: Option<String>,
}

/// The supply types that put power IN. `online` is read from these and only these.
#[cfg(any(target_os = "linux", test))]
const MAINS_LIKE: &[&str] = &[
    "Mains",
    "USB",
    "USB_C",
    "USB_PD",
    "USB_PD_DRP",
    "USB_DCP",
    "USB_CDP",
    "USB_ACA",
    "BrickID",
    "Wireless",
];

#[cfg(any(target_os = "linux", test))]
fn num(s: &Option<String>) -> Option<u64> {
    s.as_deref()?.trim().parse().ok()
}

/// A peripheral's supply — a mouse's battery, a USB-C port's own source descriptor — rather than
/// the machine's. The kernel marks these `scope=Device`, and they must never steer the policy: a
/// laptop's UCSI port supplies report `Discharging` while the laptop is on AC.
#[cfg(any(target_os = "linux", test))]
fn is_device_scope(s: &Supply) -> bool {
    s.scope
        .as_deref()
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("device"))
}

/// Read every supply under `root`. `None` when the directory itself cannot be read (a container
/// with no sysfs), which becomes Unknown.
///
/// The cheap discriminators (`type`, `scope`) are read first, and only a system battery pays for
/// the attribute reads that wake the embedded controller.
#[cfg(any(target_os = "linux", test))]
fn collect(root: &std::path::Path) -> Option<Vec<Supply>> {
    let entries = std::fs::read_dir(root).ok()?;
    let read = |dir: &std::path::Path, file: &str| {
        std::fs::read_to_string(dir.join(file))
            .ok()
            .map(|s| s.trim().to_string())
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let mut s = Supply {
            kind: read(&dir, "type"),
            scope: read(&dir, "scope"),
            ..Default::default()
        };
        if is_device_scope(&s) {
            out.push(s);
            continue;
        }
        if s.kind.as_deref() == Some("Battery") {
            s.present = read(&dir, "present");
            s.status = read(&dir, "status");
            s.capacity = read(&dir, "capacity");
            s.energy_now = read(&dir, "energy_now");
            s.energy_full = read(&dir, "energy_full");
            s.charge_now = read(&dir, "charge_now");
            s.charge_full = read(&dir, "charge_full");
        } else {
            s.online = read(&dir, "online");
        }
        out.push(s);
    }
    Some(out)
}

/// What a set of supplies says. The order of the source rules is the safety argument: power coming
/// in wins; no battery at all is a desktop; only a system battery that says `Discharging` is
/// "battery"; anything else — mains offline while the battery reads Full or Not charging — is
/// Unknown rather than a guess. `status` is never read from anything but a battery.
#[cfg(any(target_os = "linux", test))]
fn classify(supplies: &[Supply]) -> PowerReading {
    let system: Vec<&Supply> = supplies.iter().filter(|s| !is_device_scope(s)).collect();
    // `online` is 0 offline, 1 online, 2 "online, programmable" (USB PD) — any non-zero is power in.
    let ac_online = system.iter().any(|s| {
        s.kind.as_deref().is_some_and(|k| MAINS_LIKE.contains(&k))
            && num(&s.online).is_some_and(|v| v > 0)
    });
    // An absent `present` counts as present: plenty of batteries don't expose it.
    let batteries: Vec<&Supply> = system
        .iter()
        .copied()
        .filter(|s| s.kind.as_deref() == Some("Battery") && num(&s.present) != Some(0))
        .collect();
    let has_battery = !batteries.is_empty();
    let discharging = batteries
        .iter()
        .any(|b| b.status.as_deref() == Some("Discharging"));
    let source = if ac_online || !has_battery {
        PowerSource::Ac
    } else if discharging {
        PowerSource::Battery
    } else {
        PowerSource::Unknown
    };
    PowerReading {
        source,
        percent: battery_percent(&batteries),
        has_battery,
    }
}

/// The system's charge across its batteries. Per battery: the firmware's own `capacity`, else the
/// energy ratio, else the charge ratio. Never `*_full_design`: a battery whose design capacity is
/// below its measured full capacity — a real one, on the dev laptop — would read 102%. Several
/// batteries are weighted by `energy_full` when every one reports it, otherwise averaged plainly.
#[cfg(any(target_os = "linux", test))]
fn battery_percent(batteries: &[&Supply]) -> Option<u8> {
    let ratio = |now: &Option<String>, full: &Option<String>| match (num(now), num(full)) {
        (Some(n), Some(f)) if f > 0 => Some(n as f64 * 100.0 / f as f64),
        _ => None,
    };
    let per: Vec<(f64, Option<f64>)> = batteries
        .iter()
        .filter_map(|b| {
            let pct = num(&b.capacity)
                .map(|c| c as f64)
                .or_else(|| ratio(&b.energy_now, &b.energy_full))
                .or_else(|| ratio(&b.charge_now, &b.charge_full))?;
            let weight = num(&b.energy_full).filter(|f| *f > 0).map(|f| f as f64);
            Some((pct, weight))
        })
        .collect();
    if per.is_empty() {
        return None;
    }
    let weights: Option<Vec<f64>> = per.iter().map(|(_, w)| *w).collect();
    let pct = match weights {
        Some(w) if per.len() > 1 => {
            let total: f64 = w.iter().sum();
            per.iter().zip(&w).map(|((p, _), w)| p * w).sum::<f64>() / total
        }
        _ => per.iter().map(|(p, _)| p).sum::<f64>() / per.len() as f64,
    };
    Some(pct.round().clamp(0.0, 100.0) as u8)
}

#[cfg(any(target_os = "linux", test))]
fn linux_read(root: &std::path::Path) -> PowerReading {
    collect(root).map(|v| classify(&v)).unwrap_or_default()
}

// --- Windows: GetSystemPowerStatus -------------------------------------------------------------

/// Map `SYSTEM_POWER_STATUS`. `BatteryFlag` 128 is "no system battery" — a desktop — and is checked
/// first, because such a machine can report any line status. 255 in any field is "unknown".
#[cfg(any(windows, test))]
fn from_system_power_status(ac_line: u8, battery_flag: u8, life: u8) -> PowerReading {
    let percent = (life <= 100).then_some(life);
    if battery_flag != 255 && battery_flag & 128 != 0 {
        return PowerReading {
            source: PowerSource::Ac,
            percent,
            has_battery: false,
        };
    }
    let source = match ac_line {
        1 => PowerSource::Ac,
        0 => PowerSource::Battery,
        _ => PowerSource::Unknown,
    };
    PowerReading {
        source,
        percent,
        has_battery: true,
    }
}

#[cfg(windows)]
fn read_windows() -> PowerReading {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s = SYSTEM_POWER_STATUS::default();
    // SAFETY: `s` is a valid, writable SYSTEM_POWER_STATUS for the duration of the call; the API
    // only writes it.
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return PowerReading::default();
    }
    from_system_power_status(s.ACLineStatus, s.BatteryFlag, s.BatteryLifePercent)
}

// --- macOS: IOKit power sources ----------------------------------------------------------------

/// Map what IOKit said. `providing` is the providing-power-source type; `internal` is
/// `(current, max)` capacity for each present internal battery. A UPS, or an answer PM does not
/// recognise, is Unknown.
#[cfg(any(target_os = "macos", test))]
fn from_iops(providing: Option<&str>, internal: &[(Option<i64>, Option<i64>)]) -> PowerReading {
    let source = match providing {
        Some("AC Power") => PowerSource::Ac,
        Some("Battery Power") => PowerSource::Battery,
        _ => PowerSource::Unknown,
    };
    let ratios: Vec<f64> = internal
        .iter()
        .filter_map(|(cur, max)| match (cur, max) {
            (Some(c), Some(m)) if *m > 0 => Some(*c as f64 * 100.0 / *m as f64),
            _ => None,
        })
        .collect();
    let percent = (!ratios.is_empty()).then(|| {
        let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
        mean.round().clamp(0.0, 100.0) as u8
    });
    PowerReading {
        source,
        percent,
        has_battery: !internal.is_empty(),
    }
}

/// Look up one key in a power source's description. The value is borrowed from `dict` (the Get
/// rule) and lives exactly as long as it does.
#[cfg(target_os = "macos")]
fn iops_value<'a>(
    dict: &'a objc2_core_foundation::CFDictionary,
    key: &std::ffi::CStr,
) -> Option<&'a objc2_core_foundation::CFType> {
    use objc2_core_foundation::{CFString, CFType};
    let key = CFString::from_str(key.to_str().unwrap_or_default());
    let key_ptr: *const CFString = &*key;
    // SAFETY: `dict` is a valid CFDictionary and `key_ptr` a valid CFString for the call. The
    // returned pointer is borrowed from `dict` (Get rule): never released, never outliving it.
    let value = unsafe { dict.value(key_ptr.cast()) };
    if value.is_null() {
        return None;
    }
    // SAFETY: a non-null value of a CF dictionary is a CFTypeRef, borrowed as above.
    Some(unsafe { &*value.cast::<CFType>() })
}

#[cfg(target_os = "macos")]
fn read_macos() -> PowerReading {
    use objc2_core_foundation::{CFBoolean, CFNumber, CFString, CFType};
    use objc2_io_kit::{
        kIOPSCurrentCapacityKey, kIOPSInternalBatteryType, kIOPSIsPresentKey, kIOPSMaxCapacityKey,
        kIOPSTypeKey, IOPSCopyPowerSourcesInfo, IOPSCopyPowerSourcesList,
        IOPSGetPowerSourceDescription, IOPSGetProvidingPowerSourceType,
    };

    // Create rule: `blob` is owned (a CFRetained) and released when it drops, after every borrow
    // below has ended.
    let Some(blob) = IOPSCopyPowerSourcesInfo() else {
        return PowerReading::default();
    };
    let blob: &CFType = &blob;
    // SAFETY: `blob` is the CFTypeRef IOPSCopyPowerSourcesInfo returned, which is what this takes.
    // Get rule: the wrapper retains the returned string, so it is owned here.
    let providing = unsafe { IOPSGetProvidingPowerSourceType(Some(blob)) }.map(|s| s.to_string());

    let battery_type = kIOPSInternalBatteryType.to_str().unwrap_or_default();
    let mut internal: Vec<(Option<i64>, Option<i64>)> = Vec::new();
    // SAFETY: as above. Create rule: the list is owned (a CFRetained) and released on drop.
    if let Some(list) = unsafe { IOPSCopyPowerSourcesList(Some(blob)) } {
        for i in 0..list.count() {
            // SAFETY: `i` is inside 0..count. The element is a borrowed CFTypeRef owned by `list`
            // (Get rule) — never released here, and `list` outlives every use of it.
            let ptr = unsafe { list.value_at_index(i) };
            if ptr.is_null() {
                continue;
            }
            // SAFETY: a non-null element of the power-sources list is a CFTypeRef, borrowed as above.
            let ps: &CFType = unsafe { &*ptr.cast::<CFType>() };
            // SAFETY: `blob` and `ps` are the snapshot and one of its list members, as required.
            // Get rule: the wrapper retains the description, so it is owned here.
            let Some(desc) = (unsafe { IOPSGetPowerSourceDescription(Some(blob), Some(ps)) })
            else {
                continue;
            };
            let is_internal = iops_value(&desc, kIOPSTypeKey)
                .and_then(|v| v.downcast_ref::<CFString>())
                .is_some_and(|s| s.to_string() == battery_type);
            let present = iops_value(&desc, kIOPSIsPresentKey)
                .and_then(|v| v.downcast_ref::<CFBoolean>())
                .is_some_and(|b| b.as_bool());
            if !(is_internal && present) {
                continue;
            }
            let number = |key: &std::ffi::CStr| {
                iops_value(&desc, key)
                    .and_then(|v| v.downcast_ref::<CFNumber>())
                    .and_then(|n| n.as_i64())
            };
            internal.push((number(kIOPSCurrentCapacityKey), number(kIOPSMaxCapacityKey)));
        }
    }
    from_iops(providing.as_deref(), &internal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Write one invented supply into a sysfs-shaped tree: `attrs` are (file, contents).
    fn supply(root: &Path, name: &str, attrs: &[(&str, &str)]) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (file, value) in attrs {
            std::fs::write(dir.join(file), format!("{value}\n")).unwrap();
        }
    }

    fn mains(root: &Path, name: &str, online: &str) {
        supply(root, name, &[("type", "Mains"), ("online", online)]);
    }

    fn battery(root: &Path, name: &str, status: &str, capacity: &str) {
        supply(
            root,
            name,
            &[
                ("type", "Battery"),
                ("present", "1"),
                ("status", status),
                ("capacity", capacity),
            ],
        );
    }

    /// A USB-C port's own source descriptor, the shape UCSI exposes: offline, saying
    /// `Discharging`, with or without a `scope` file.
    fn port(root: &Path, name: &str, scope: Option<&str>, online: &str) {
        let mut attrs = vec![
            ("type", "USB"),
            ("online", online),
            ("status", "Discharging"),
        ];
        if let Some(scope) = scope {
            attrs.push(("scope", scope));
        }
        supply(root, name, &attrs);
    }

    /// The dev laptop's shape with invented names: a mains adapter, one battery held at a charge
    /// limit, and two port supplies that say `Discharging` on AC.
    fn laptop(root: &Path, online: &str, status: &str, capacity: &str) {
        mains(root, "ADP0", online);
        supply(
            root,
            "BAT0",
            &[
                ("type", "Battery"),
                ("present", "1"),
                ("status", status),
                ("capacity", capacity),
                ("energy_now", "87030000"),
                ("energy_full", "87030000"),
            ],
        );
        port(root, "port-psy-0", Some("Device"), "0");
        port(root, "port-psy-1", Some("Device"), "0");
    }

    fn read_tree(build: impl FnOnce(&Path)) -> PowerReading {
        let dir = tempfile::tempdir().unwrap();
        build(dir.path());
        linux_read(dir.path())
    }

    fn reading(source: PowerSource, percent: Option<u8>, has_battery: bool) -> PowerReading {
        PowerReading {
            source,
            percent,
            has_battery,
        }
    }

    #[test]
    fn l1_the_laptop_on_ac_at_a_charge_limit_is_ac() {
        let r = read_tree(|root| laptop(root, "1", "Not charging", "100"));
        assert_eq!(r, reading(PowerSource::Ac, Some(100), true));
    }

    #[test]
    fn l2_the_laptop_unplugged_is_battery() {
        let r = read_tree(|root| laptop(root, "0", "Discharging", "58"));
        assert_eq!(r, reading(PowerSource::Battery, Some(58), true));
    }

    #[test]
    fn l3_a_port_supply_with_no_scope_never_reads_as_battery() {
        let r = read_tree(|root| port(root, "port-psy-0", None, "0"));
        assert_eq!(r.source, PowerSource::Ac);
        assert!(!r.has_battery);
    }

    #[test]
    fn l4_usb_c_charging_is_ac() {
        let r = read_tree(|root| {
            port(root, "port-psy-0", Some("System"), "1");
            battery(root, "BAT0", "Charging", "40");
        });
        assert_eq!(r.source, PowerSource::Ac);
    }

    #[test]
    fn l5_online_two_counts_as_online() {
        let r = read_tree(|root| {
            port(root, "port-psy-0", Some("System"), "2");
            battery(root, "BAT0", "Discharging", "40");
        });
        assert_eq!(r.source, PowerSource::Ac);
    }

    #[test]
    fn l6_a_mouse_battery_on_a_desktop_is_ignored() {
        let r = read_tree(|root| {
            supply(
                root,
                "mouse_battery",
                &[
                    ("type", "Battery"),
                    ("scope", "Device"),
                    ("status", "Discharging"),
                    ("capacity", "12"),
                ],
            );
        });
        assert_eq!(r, reading(PowerSource::Ac, None, false));
    }

    #[test]
    fn l7_a_mouse_battery_does_not_move_the_laptops_percent() {
        let r = read_tree(|root| {
            laptop(root, "0", "Discharging", "70");
            supply(
                root,
                "mouse_battery",
                &[
                    ("type", "Battery"),
                    ("scope", "Device"),
                    ("status", "Discharging"),
                    ("capacity", "5"),
                ],
            );
        });
        assert_eq!(r.percent, Some(70));
    }

    #[test]
    fn l8_an_empty_root_is_a_desktop() {
        let r = read_tree(|_| {});
        assert_eq!(r, reading(PowerSource::Ac, None, false));
    }

    #[test]
    fn l9_a_missing_root_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let r = linux_read(&dir.path().join("not-here"));
        assert_eq!(r, PowerReading::default());
        assert_eq!(r.source, PowerSource::Unknown);
    }

    #[test]
    fn l10_an_empty_bay_is_not_a_battery() {
        let r = read_tree(|root| {
            mains(root, "AC", "0");
            supply(
                root,
                "BAT1",
                &[("type", "Battery"), ("present", "0"), ("status", "Unknown")],
            );
        });
        assert_eq!(r.source, PowerSource::Ac);
        assert!(!r.has_battery);
    }

    #[test]
    fn l11_two_batteries_without_energy_full_are_averaged() {
        let r = read_tree(|root| {
            mains(root, "AC", "0");
            battery(root, "BAT0", "Not charging", "90");
            battery(root, "BAT1", "Discharging", "30");
        });
        assert_eq!(r, reading(PowerSource::Battery, Some(60), true));
    }

    #[test]
    fn l12_two_batteries_are_weighted_by_energy_full() {
        let r = read_tree(|root| {
            for (name, cap, full) in [("BAT0", "100", "20000000"), ("BAT1", "0", "60000000")] {
                supply(
                    root,
                    name,
                    &[
                        ("type", "Battery"),
                        ("present", "1"),
                        ("status", "Discharging"),
                        ("capacity", cap),
                        ("energy_full", full),
                    ],
                );
            }
        });
        assert_eq!(r.percent, Some(25));
    }

    #[test]
    fn l13_charge_units_are_the_last_fallback() {
        let r = read_tree(|root| {
            supply(
                root,
                "BAT0",
                &[
                    ("type", "Battery"),
                    ("status", "Discharging"),
                    ("charge_now", "2000000"),
                    ("charge_full", "4000000"),
                    // Never read: a design capacity below the full one would push this past 100.
                    ("charge_full_design", "1000000"),
                ],
            );
        });
        assert_eq!(r.percent, Some(50));
    }

    #[test]
    fn l14_mains_offline_with_a_full_battery_is_unknown() {
        let r = read_tree(|root| {
            mains(root, "AC", "0");
            battery(root, "BAT0", "Full", "100");
        });
        assert_eq!(r.source, PowerSource::Unknown);
    }

    #[test]
    fn l15_a_weak_charger_under_load_is_ac() {
        let r = read_tree(|root| {
            mains(root, "AC", "1");
            battery(root, "BAT0", "Discharging", "50");
        });
        assert_eq!(r.source, PowerSource::Ac);
    }

    #[test]
    fn l16_an_unreadable_online_is_not_online() {
        let r = read_tree(|root| {
            supply(root, "AC", &[("type", "Mains")]);
            battery(root, "BAT0", "Discharging", "50");
        });
        assert_eq!(r.source, PowerSource::Battery);
    }

    #[test]
    fn l17_a_ratio_above_full_is_clamped() {
        let r = read_tree(|root| {
            supply(
                root,
                "BAT0",
                &[
                    ("type", "Battery"),
                    ("status", "Discharging"),
                    ("energy_now", "90"),
                    ("energy_full", "80"),
                ],
            );
        });
        assert_eq!(r.percent, Some(100));
    }

    #[test]
    fn windows_status_maps_line_flag_and_life() {
        assert_eq!(
            from_system_power_status(1, 8, 80),
            reading(PowerSource::Ac, Some(80), true)
        );
        assert_eq!(
            from_system_power_status(0, 1, 55),
            reading(PowerSource::Battery, Some(55), true)
        );
        assert_eq!(
            from_system_power_status(255, 1, 50).source,
            PowerSource::Unknown
        );
        // "No system battery" is checked before the line status.
        assert_eq!(
            from_system_power_status(0, 128, 255),
            reading(PowerSource::Ac, None, false)
        );
        assert_eq!(
            from_system_power_status(1, 255, 255),
            reading(PowerSource::Ac, None, true)
        );
    }

    #[test]
    fn macos_power_sources_map_type_and_capacity() {
        assert_eq!(
            from_iops(Some("AC Power"), &[(Some(50), Some(100))]),
            reading(PowerSource::Ac, Some(50), true)
        );
        assert_eq!(
            from_iops(Some("Battery Power"), &[(Some(30), Some(100))]),
            reading(PowerSource::Battery, Some(30), true)
        );
        assert_eq!(
            from_iops(Some("UPS Power"), &[(Some(30), Some(100))]).source,
            PowerSource::Unknown
        );
        assert_eq!(
            from_iops(None, &[(Some(30), Some(100))]).source,
            PowerSource::Unknown
        );
        assert!(!from_iops(Some("AC Power"), &[]).has_battery);
        assert_eq!(
            from_iops(Some("Battery Power"), &[(Some(30), Some(0))]).percent,
            None
        );
        assert_eq!(
            from_iops(
                Some("Battery Power"),
                &[(Some(40), Some(100)), (Some(80), Some(100))]
            )
            .percent,
            Some(60)
        );
    }

    #[test]
    fn live_read_never_panics() {
        // Asserts nothing about the result: CI has no battery, and a developer may be unplugged.
        let _ = read();
    }
}
