// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Best-effort hardware scan for the local-AI Workbench (#296): how much memory, what CPU, how much
//! free disk, and — where we can read it — the GPU and its VRAM.
//!
//! `sysinfo` covers RAM / CPU / disk on every OS (it already calls the right native API under the
//! hood). It has no GPU/VRAM/battery, so those are hand-rolled per-OS: on Windows a CIM query for the
//! video controller plus `nvidia-smi`; on Apple Silicon the unified-memory fraction; on Linux
//! `nvidia-smi` plus the AMD sysfs node. **No battery/AC here** — that's the deferred power-aware
//! routing card (#432).
//!
//! The contract every probe honours: **a failure nulls its field, it never errors.** A machine with
//! no GPU, no `nvidia-smi`, or a driver that lies about VRAM still gets a complete, honest scan — the
//! missing pieces come back `None` with a plain note, never a hard error the UI has to handle.
//!
//! Units: everything is **GiB** (bytes / 2³⁰), labelled "GB" — which is how people read their RAM and
//! GPU specs (a "32 GB" stick is 32 GiB). The catalog's model sizes use the same base, so fit-scoring
//! compares like with like.

use serde::Serialize;

/// Bytes per GiB — the single unit conversion, so RAM/VRAM/disk all read in the base people expect.
const GIB: f64 = 1_073_741_824.0;

/// The GPU's `AdapterRAM` (a WMI `uint32`) saturates around 4 GiB, so any value at/above this ceiling
/// is a lie for a modern card — we fall back to a RAM-only score rather than trust it. (Only the
/// Windows probe reads `AdapterRAM`, so it's gated with its helper — see the pure-parse section note.)
#[cfg(any(windows, test))]
const ADAPTER_RAM_CEILING: u64 = 4_000_000_000;

/// The fraction of unified memory Apple Silicon lets the GPU use (`recommendedMaxWorkingSetSize` is
/// ~75% of total on current macOS). A rough but honest ceiling for the fit estimate. (Only compiled
/// where it's used — the macOS probe — plus test builds.)
#[cfg(any(target_os = "macos", test))]
const APPLE_VRAM_FRACTION: f64 = 0.75;

/// The machine, as far as local-model fit cares. Nullable fields are "couldn't read it", never zero.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Hardware {
    /// `windows` | `macos` | `linux` | other (`std::env::consts::OS`).
    pub platform: String,
    pub total_ram_gb: f64,
    pub available_ram_gb: f64,
    pub cpu_brand: Option<String>,
    pub cpu_cores: Option<u32>,
    pub cpu_threads: Option<u32>,
    /// Free space on the volume holding PM's data dir (where models download), when known.
    pub disk_free_gb: Option<f64>,
    pub gpu_name: Option<String>,
    pub gpu_vendor: Option<String>,
    pub vram_gb: Option<f64>,
    /// How VRAM was read: `nvidia-smi` | `adapter_ram` | `apple_unified` | `amd_sysfs`.
    pub vram_source: Option<String>,
    /// Apple-Silicon-style shared CPU/GPU memory (VRAM is a slice of system RAM, not separate).
    pub unified_memory: bool,
    pub is_wsl: bool,
    /// Honest, user-facing caveats gathered during the scan.
    pub notes: Vec<String>,
}

/// What a per-OS GPU probe found. Empty is a valid answer (no GPU, or nothing readable).
#[derive(Debug, Clone, Default)]
struct GpuProbe {
    name: Option<String>,
    vendor: Option<String>,
    vram_gb: Option<f64>,
    source: Option<String>,
    unified: bool,
    notes: Vec<String>,
}

/// Scan this machine. `data_dir` (PM's data folder) picks the disk whose free space matters for model
/// downloads; `None` falls back to the roomiest mounted volume. Blocking — call it off the async
/// runtime (the command wraps it in `spawn_blocking`).
pub fn scan(data_dir: Option<&std::path::Path>) -> Hardware {
    let mut hw = Hardware {
        platform: std::env::consts::OS.to_string(),
        ..Default::default()
    };

    fill_ram_cpu_disk(&mut hw, data_dir);

    let gpu = probe_gpu();
    hw.gpu_name = gpu.name;
    hw.gpu_vendor = gpu.vendor;
    hw.vram_gb = gpu.vram_gb;
    hw.vram_source = gpu.source;
    hw.unified_memory = gpu.unified;
    hw.notes.extend(gpu.notes);

    hw
}

/// RAM / CPU / disk via `sysinfo` — one API, every OS. Each field is best-effort.
fn fill_ram_cpu_disk(hw: &mut Hardware, data_dir: Option<&std::path::Path>) {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();

    hw.total_ram_gb = round1(sys.total_memory() as f64 / GIB);
    hw.available_ram_gb = round1(sys.available_memory() as f64 / GIB);

    if let Some(cpu) = sys.cpus().first() {
        let brand = cpu.brand().trim();
        if !brand.is_empty() {
            hw.cpu_brand = Some(brand.to_string());
        }
    }
    hw.cpu_cores = System::physical_core_count().and_then(|c| u32::try_from(c).ok());
    let threads = sys.cpus().len();
    if threads > 0 {
        hw.cpu_threads = u32::try_from(threads).ok();
    }

    hw.disk_free_gb = disk_free_gb(data_dir);
}

/// Free GB on the volume holding `data_dir` (longest matching mount point), else the roomiest volume.
fn disk_free_gb(data_dir: Option<&std::path::Path>) -> Option<f64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let list = disks.list();
    if list.is_empty() {
        return None;
    }

    if let Some(dir) = data_dir {
        let target = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        let best = list
            .iter()
            .filter(|d| target.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len());
        if let Some(d) = best {
            return Some(round1(d.available_space() as f64 / GIB));
        }
    }

    list.iter()
        .map(|d| d.available_space())
        .max()
        .map(|b| round1(b as f64 / GIB))
}

// --- GPU / VRAM: hand-rolled per OS ------------------------------------------------------------

#[cfg(windows)]
fn probe_gpu() -> GpuProbe {
    let mut probe = GpuProbe::default();

    // Video controller name + AdapterRAM via CIM (works without any vendor tool).
    if let Some(out) = run_capture(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM,DriverVersion | ConvertTo-Json -Compress",
        ],
    ) {
        if let Some(gpu) = pick_gpu(&parse_video_controller_json(&out)) {
            probe.vendor = vendor_from_name(&gpu.name);
            probe.name = Some(gpu.name);
            if let Some(bytes) = gpu.adapter_ram {
                if adapter_ram_reliable(bytes) {
                    probe.vram_gb = Some(round1(bytes as f64 / GIB));
                    probe.source = Some("adapter_ram".to_string());
                }
            }
        }
    }

    // nvidia-smi is authoritative for NVIDIA VRAM — prefer it over the AdapterRAM guess.
    if let Some(mib) =
        run_capture("nvidia-smi", &NVIDIA_SMI_ARGS).and_then(|s| parse_nvidia_smi_csv(&s))
    {
        probe.vram_gb = Some(round1(mib / 1024.0));
        probe.source = Some("nvidia-smi".to_string());
    }

    // Shared-memory (integrated) GPU → no distinct faster pool, so no GPU Split. nvidia-smi VRAM is
    // always a discrete NVIDIA card (even on a hybrid laptop), so it wins; otherwise classify by the
    // controller name (Intel UHD/Iris, AMD "Radeon Graphics" / Vega APU).
    probe.unified = probe.source.as_deref() != Some("nvidia-smi")
        && probe.name.as_deref().is_some_and(integrated_gpu_from_name);

    // A named GPU we couldn't size reliably: say so plainly, don't invent a number.
    if probe.name.is_some() && probe.vram_gb.is_none() {
        probe
            .notes
            .push("GPU VRAM couldn't be read reliably — sized on system RAM instead.".to_string());
    }
    probe
}

#[cfg(target_os = "macos")]
fn probe_gpu() -> GpuProbe {
    let mut probe = GpuProbe::default();
    if cfg!(target_arch = "aarch64") {
        // Apple Silicon: the GPU shares system RAM. Size VRAM as the working-set fraction of total.
        let total_gb = {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            sys.total_memory() as f64 / GIB
        };
        probe.name = Some("Apple Silicon GPU".to_string());
        probe.vendor = Some("Apple".to_string());
        probe.vram_gb = Some(round1(apple_vram_gb(total_gb)));
        probe.source = Some("apple_unified".to_string());
        probe.unified = true;
    } else {
        probe
            .notes
            .push("GPU VRAM isn't read on Intel Macs — sized on system RAM instead.".to_string());
    }
    probe
}

#[cfg(target_os = "linux")]
fn probe_gpu() -> GpuProbe {
    let mut probe = GpuProbe::default();

    if is_wsl_now() {
        probe
            .notes
            .push("Running under WSL — GPU passthrough may vary.".to_string());
    }

    // NVIDIA first (authoritative), then the AMD sysfs node.
    if let Some(mib) =
        run_capture("nvidia-smi", &NVIDIA_SMI_ARGS).and_then(|s| parse_nvidia_smi_csv(&s))
    {
        probe.name = Some("NVIDIA GPU".to_string());
        probe.vendor = Some("NVIDIA".to_string());
        probe.vram_gb = Some(round1(mib / 1024.0));
        probe.source = Some("nvidia-smi".to_string());
    } else if let Some((bytes, card)) = read_amd_sysfs_vram() {
        probe.name = Some("AMD GPU".to_string());
        probe.vendor = Some("AMD".to_string());
        probe.vram_gb = Some(round1(bytes as f64 / GIB));
        probe.source = Some("amd_sysfs".to_string());
        // Integrated (APU) → the "VRAM" is a shared-RAM carve-out, not a distinct faster pool: no Split.
        probe.unified = amd_is_integrated(card);
    }
    probe
}

// Fallback for any other target: no GPU probe.
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn probe_gpu() -> GpuProbe {
    GpuProbe::default()
}

// nvidia-smi is queried by the Windows and Linux probes; macOS never shells out to it.
#[cfg(any(windows, target_os = "linux"))]
const NVIDIA_SMI_ARGS: [&str; 2] = ["--query-gpu=memory.total", "--format=csv,noheader,nounits"];

/// Run a command and capture stdout as a string, or `None` if it can't be run or fails. On Windows a
/// no-op-elsewhere `no_window` flag keeps a console from flashing. Only the Windows/Linux probes shell
/// out (macOS reads memory directly), so this is gated to them.
#[cfg(any(windows, target_os = "linux"))]
fn run_capture(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    no_window(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Hide the console window a spawned CLI would otherwise flash on Windows. No-op elsewhere. (Mirrors
/// the one-liner in `sidecar.rs` — kept local to avoid a shared-module dependency for one constant.)
#[cfg(windows)]
fn no_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}
// The no-op arm only needs to exist where `run_capture` is compiled but isn't Windows — i.e. Linux.
#[cfg(target_os = "linux")]
fn no_window(_cmd: &mut std::process::Command) {}

#[cfg(target_os = "linux")]
fn read_amd_sysfs_vram() -> Option<(u64, u32)> {
    // The first card's total VRAM in bytes AND its index, if the amdgpu driver exposes it. The index
    // lets `amd_is_integrated` check the SAME card's PCI bus (APU-vs-discrete).
    for n in 0..8 {
        let path = format!("/sys/class/drm/card{n}/device/mem_info_vram_total");
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Ok(bytes) = s.trim().parse::<u64>() {
                if bytes > 0 {
                    return Some((bytes, n));
                }
            }
        }
    }
    None
}

/// Whether amdgpu `card{n}` is integrated (an APU): its PCI device sits on bus 00 (the CPU root
/// complex), whereas a discrete card is behind a PCIe port on a higher bus. Best-effort — if the
/// `device` symlink can't be read it falls back to `false` (discrete), but since we only ask about a
/// card whose `.../device/mem_info_vram_total` we just read, that link is present in practice.
#[cfg(target_os = "linux")]
fn amd_is_integrated(card: u32) -> bool {
    std::fs::read_link(format!("/sys/class/drm/card{card}/device"))
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .is_some_and(|target| pci_bus_is_integrated(&target))
}

#[cfg(target_os = "linux")]
fn is_wsl_now() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| detect_wsl(&v))
        .unwrap_or(false)
}

// --- pure parse helpers (unit-tested; the live probes above are not) -----------------------------
//
// Each is `#[cfg]`-gated to the OS whose probe calls it, plus `test` so every platform's test run
// still exercises the parser. Without the gate, clippy's `-D warnings` dead-code lint fails the build
// on the platforms that don't use a given helper — a Windows `AdapterRAM` parser is dead on Linux.

#[cfg(any(windows, test))]
#[derive(Debug, Clone)]
struct GpuLine {
    name: String,
    adapter_ram: Option<u64>,
}

/// Parse `Get-CimInstance Win32_VideoController | ConvertTo-Json` — which is a single object for one
/// GPU and an array for several. Missing/zero `AdapterRAM` becomes `None`.
#[cfg(any(windows, test))]
fn parse_video_controller_json(json: &str) -> Vec<GpuLine> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json.trim()) else {
        return Vec::new();
    };
    let objects: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(a) => a.iter().collect(),
        serde_json::Value::Object(_) => vec![&value],
        _ => return Vec::new(),
    };
    objects
        .into_iter()
        .filter_map(|o| {
            let name = o.get("Name")?.as_str()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            // AdapterRAM can arrive as a JSON number or a stringified number; 0/negative → None.
            let adapter_ram = o.get("AdapterRAM").and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
            });
            let adapter_ram = adapter_ram.filter(|&b| b > 0);
            Some(GpuLine { name, adapter_ram })
        })
        .collect()
}

/// Choose the most relevant controller: skip Microsoft's basic/remote display shims, then take the
/// one reporting the most memory (usually the discrete card), else the first real one.
#[cfg(any(windows, test))]
fn pick_gpu(lines: &[GpuLine]) -> Option<GpuLine> {
    let real: Vec<&GpuLine> = lines
        .iter()
        .filter(|l| {
            let n = l.name.to_ascii_lowercase();
            !n.contains("basic display") && !n.contains("remote display")
        })
        .collect();
    let pool = if real.is_empty() {
        lines.iter().collect::<Vec<_>>()
    } else {
        real
    };
    pool.into_iter()
        .max_by_key(|l| l.adapter_ram.unwrap_or(0))
        .cloned()
}

/// `AdapterRAM` is a `uint32` that saturates near 4 GiB, so only sub-ceiling values are trustworthy.
#[cfg(any(windows, test))]
fn adapter_ram_reliable(bytes: u64) -> bool {
    bytes > 0 && bytes < ADAPTER_RAM_CEILING
}

/// The largest `memory.total` (MiB) across `nvidia-smi --query-gpu` lines, or `None`.
#[cfg(any(windows, target_os = "linux", test))]
fn parse_nvidia_smi_csv(csv: &str) -> Option<f64> {
    csv.lines()
        .filter_map(|l| l.trim().parse::<f64>().ok())
        .filter(|&m| m > 0.0)
        .fold(None, |acc: Option<f64>, m| {
            Some(acc.map_or(m, |a| a.max(m)))
        })
}

/// A GPU vendor guessed from the controller name, or `None` if unrecognized.
#[cfg(any(windows, test))]
fn vendor_from_name(name: &str) -> Option<String> {
    let n = name.to_ascii_lowercase();
    if n.contains("nvidia")
        || n.contains("geforce")
        || n.contains("quadro")
        || n.contains("rtx")
        || n.contains("gtx")
    {
        Some("NVIDIA".to_string())
    } else if n.contains("amd") || n.contains("radeon") || n.contains("ati ") {
        Some("AMD".to_string())
    } else if n.contains("intel") || n.contains("arc ") || n.contains("uhd") || n.contains("iris") {
        Some("Intel".to_string())
    } else if n.contains("apple") {
        Some("Apple".to_string())
    } else {
        None
    }
}

/// Whether a video-controller name is an integrated GPU (shares system memory, so no faster "GPU"
/// config to offer). Intel iGPUs are UHD / Iris / HD Graphics, and the integrated "Arc Graphics" on
/// Core Ultra (Meteor/Arrow Lake) chips — only *discrete* Arc cards carry an A-/B-series model token
/// (Arc A770, Arc B580). AMD APUs market as a bare "Radeon(TM) Graphics" or "Radeon Vega N Graphics",
/// while discrete AMD carries an explicit tier (RX / Pro / FirePro / Instinct). NVIDIA has no
/// integrated desktop/laptop part. Conservative: an unrecognized name → `false` (discrete), and AMD is
/// positive-matched on APU markers so a discrete card is never wrongly flagged (a missed newer APU
/// like a 780M usually reports no reliable VRAM on Windows anyway → no Split regardless).
#[cfg(any(windows, test))]
fn integrated_gpu_from_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.contains("nvidia") || n.contains("geforce") || n.contains("rtx") || n.contains("gtx") {
        return false; // NVIDIA is always discrete.
    }
    if n.contains("intel") || n.contains("uhd") || n.contains("iris") {
        // "Arc Graphics" (no model token) is the Core-Ultra iGPU; "Arc A770"/"Arc B580" are discrete.
        if n.contains("arc") {
            return !has_arc_discrete_model(&n);
        }
        return true;
    }
    if n.contains("radeon") || n.contains("amd") {
        let discrete = n.contains(" rx ")
            || n.contains("radeon rx")
            || n.contains("radeon pro")
            || n.contains("firepro")
            || n.contains("instinct");
        return !discrete && (n.contains("graphics") || n.contains("vega"));
    }
    false
}

/// True if an already-lowercased Intel-Arc name carries a discrete A-/B-series model token — an `a`/`b`
/// followed by 2+ digits (`a60`, `a770`, `b580`) — as opposed to the integrated "Arc Graphics" that
/// carries none. Covers the desktop (A3xx–A7xx, B5xx) and workstation Arc Pro (A40/A50/A60) lines.
#[cfg(any(windows, test))]
fn has_arc_discrete_model(name_lower: &str) -> bool {
    name_lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| {
            let b = tok.as_bytes();
            b.len() >= 3 && (b[0] == b'a' || b[0] == b'b') && b[1..].iter().all(u8::is_ascii_digit)
        })
}

/// VRAM available to an Apple-Silicon GPU: the working-set fraction of unified memory.
#[cfg(any(target_os = "macos", test))]
fn apple_vram_gb(total_gb: f64) -> f64 {
    total_gb * APPLE_VRAM_FRACTION
}

/// True when `/proc/version` marks a WSL kernel.
#[cfg(any(target_os = "linux", test))]
fn detect_wsl(proc_version: &str) -> bool {
    let v = proc_version.to_ascii_lowercase();
    v.contains("microsoft") || v.contains("wsl")
}

/// Is a `/sys/class/drm/cardN/device` symlink target an integrated GPU? Integrated GPUs sit on PCI
/// bus 00 (the CPU root complex); a discrete card is behind a PCIe port on a higher bus. The target
/// looks like `../../../0000:03:00.0` — the bus is the field between the two colons of the last path
/// segment. Pure; an unparseable target → `false` (treated as discrete).
#[cfg(any(target_os = "linux", test))]
fn pci_bus_is_integrated(link_target: &str) -> bool {
    link_target
        .rsplit('/')
        .next()
        .and_then(|addr| addr.split(':').nth(1)) // "0000:BB:DD.F" → "BB"
        .is_some_and(|bus| bus == "00")
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_object_and_array_video_controllers() {
        let single =
            r#"{"Name":"NVIDIA GeForce RTX 4070","AdapterRAM":2147483648,"DriverVersion":"1.2"}"#;
        let got = parse_video_controller_json(single);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "NVIDIA GeForce RTX 4070");
        assert_eq!(got[0].adapter_ram, Some(2_147_483_648));

        let array = r#"[{"Name":"Intel UHD","AdapterRAM":1073741824},{"Name":"NVIDIA RTX","AdapterRAM":4294967295}]"#;
        let got = parse_video_controller_json(array);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].adapter_ram, Some(4_294_967_295));
    }

    #[test]
    fn video_controller_handles_stringified_and_missing_adapter_ram() {
        let mixed =
            r#"[{"Name":"A","AdapterRAM":"2147483648"},{"Name":"B"},{"Name":"C","AdapterRAM":0}]"#;
        let got = parse_video_controller_json(mixed);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].adapter_ram, Some(2_147_483_648));
        assert_eq!(got[1].adapter_ram, None);
        assert_eq!(got[2].adapter_ram, None); // 0 filtered out
    }

    #[test]
    fn video_controller_rejects_garbage() {
        assert!(parse_video_controller_json("not json").is_empty());
        assert!(parse_video_controller_json("42").is_empty());
        assert!(parse_video_controller_json("").is_empty());
    }

    #[test]
    fn pick_gpu_prefers_the_discrete_card_over_basic_display() {
        let lines = vec![
            GpuLine {
                name: "Microsoft Basic Display Adapter".into(),
                adapter_ram: None,
            },
            GpuLine {
                name: "NVIDIA RTX".into(),
                adapter_ram: Some(2_000_000_000),
            },
        ];
        assert_eq!(pick_gpu(&lines).unwrap().name, "NVIDIA RTX");
    }

    #[test]
    fn pick_gpu_falls_back_to_basic_when_thats_all_there_is() {
        let lines = vec![GpuLine {
            name: "Microsoft Basic Display Adapter".into(),
            adapter_ram: None,
        }];
        assert_eq!(
            pick_gpu(&lines).unwrap().name,
            "Microsoft Basic Display Adapter"
        );
        assert!(pick_gpu(&[]).is_none());
    }

    #[test]
    fn adapter_ram_trusts_small_values_and_distrusts_the_uint32_ceiling() {
        assert!(adapter_ram_reliable(2_147_483_648)); // 2 GiB — trustworthy
        assert!(!adapter_ram_reliable(4_294_967_295)); // saturated uint32 — a lie
        assert!(!adapter_ram_reliable(0));
    }

    #[test]
    fn nvidia_smi_takes_the_largest_gpu() {
        assert_eq!(parse_nvidia_smi_csv("8192\n24576\n"), Some(24576.0));
        assert_eq!(parse_nvidia_smi_csv("12288"), Some(12288.0));
        assert_eq!(parse_nvidia_smi_csv(""), None);
        assert_eq!(parse_nvidia_smi_csv("no gpu found"), None);
    }

    #[test]
    fn vendor_is_guessed_from_the_name() {
        assert_eq!(
            vendor_from_name("NVIDIA GeForce RTX 4070").as_deref(),
            Some("NVIDIA")
        );
        assert_eq!(
            vendor_from_name("AMD Radeon RX 7900").as_deref(),
            Some("AMD")
        );
        assert_eq!(vendor_from_name("Intel Arc A770").as_deref(), Some("Intel"));
        assert_eq!(vendor_from_name("Apple M3 Pro").as_deref(), Some("Apple"));
        assert_eq!(vendor_from_name("Some Unknown Adapter"), None);
    }

    #[test]
    fn apple_vram_is_a_fraction_of_unified_memory() {
        assert!((apple_vram_gb(16.0) - 12.0).abs() < 1e-9);
    }

    #[test]
    fn wsl_is_detected_from_proc_version() {
        assert!(detect_wsl("Linux version 5.15.0-microsoft-standard-WSL2"));
        assert!(detect_wsl("... Microsoft ..."));
        assert!(!detect_wsl("Linux version 6.1.0-generic (gcc ...)"));
    }

    #[test]
    fn integrated_gpu_is_classified_from_the_name() {
        // Intel iGPUs are integrated, incl. the Core-Ultra "Arc Graphics"; discrete Arc has a model.
        assert!(integrated_gpu_from_name("Intel(R) UHD Graphics 770"));
        assert!(integrated_gpu_from_name("Intel(R) Iris(R) Xe Graphics"));
        assert!(integrated_gpu_from_name("Intel(R) Arc(TM) Graphics")); // Core-Ultra iGPU
        assert!(!integrated_gpu_from_name("Intel(R) Arc(TM) A770 Graphics")); // discrete
        assert!(!integrated_gpu_from_name("Intel Arc B580")); // discrete
        assert!(!integrated_gpu_from_name("Intel Arc Pro A60")); // discrete workstation (2-digit model)
                                                                 // AMD APUs read as bare "... Graphics" / "Vega N"; discrete AMD carries a tier.
        assert!(integrated_gpu_from_name("AMD Radeon(TM) Graphics"));
        assert!(integrated_gpu_from_name("AMD Radeon(TM) Vega 8 Graphics"));
        assert!(!integrated_gpu_from_name("AMD Radeon RX 7900 XT"));
        assert!(!integrated_gpu_from_name("AMD Radeon Pro W6800"));
        // NVIDIA has no integrated part; unknown names are conservatively discrete.
        assert!(!integrated_gpu_from_name(
            "NVIDIA GeForce RTX 5070 Laptop GPU"
        ));
        assert!(!integrated_gpu_from_name("Some Unknown Adapter"));
    }

    #[test]
    fn pci_bus_00_is_integrated_higher_buses_are_discrete() {
        assert!(pci_bus_is_integrated("../../../0000:00:02.0")); // iGPU on the root complex
        assert!(pci_bus_is_integrated("0000:00:08.1")); // bare last-segment form
        assert!(!pci_bus_is_integrated("../../../0000:03:00.0")); // discrete behind a PCIe port
        assert!(!pci_bus_is_integrated("../../../0000:01:00.0"));
        assert!(!pci_bus_is_integrated("")); // unparseable → discrete
        assert!(!pci_bus_is_integrated("garbage"));
    }
}
