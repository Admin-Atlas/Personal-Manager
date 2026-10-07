// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The On battery policy (#432): when a laptop running a local model is on battery and the charge
//! has fallen to the level the user chose, PM may send new requests to the cloud instead. This
//! module is the pure half — the reading types, the settings parsers and the latch that turns a
//! stream of raw readings into the one settled state routing acts on. No I/O: the machine is read in
//! [`crate::power_source`], and the latch is fed by the watcher in [`crate::local_ai`]. The precedent
//! for the shape is [`crate::local_slot::HealthState::observe`] — a reducer with an injected
//! `Instant`, so every timing rule below is a unit test rather than an argument.
//!
//! The rules the latch keeps:
//!
//! * **Unknown never routes to cloud.** A machine PM cannot read — a container, a desktop with an
//!   odd supply, a read that hung — folds into `Mains`. Sending someone's chats off the machine and
//!   billing their key needs positive evidence of a battery, never the absence of evidence of mains.
//! * **Two guards, both needed.** A level guard (to cloud at the threshold or below; back to local
//!   on battery only at threshold + [`RETURN_GAP`]) stops a charge hovering on the line from
//!   flapping, and a time guard ([`SETTLE`]) stops unplugging to carry the laptop to another room
//!   from moving anything.
//! * **AC clears the level band.** Plugging in returns PM to local after the settle time whatever
//!   the charge reads. A laptop held at a charge limit can sit below the threshold for its whole
//!   life on mains, and the band must not strand it on paid cloud.
//! * **A verdict PM stopped watching is not trusted.** A gap of more than [`MAX_SAMPLE_GAP`] between
//!   samples (a suspended laptop, a wedged read) drops the latch to `Mains`, and the next sample has
//!   to settle again before anything goes to the cloud. The gap is measured on both clocks, because
//!   `Instant` stands still while Linux and macOS are suspended — see [`PowerTracker::observe_wall`].
//! * **Consent is per role.** The one-time question names the roles that would move when it is
//!   asked, and its answer covers exactly those. A role that becomes movable later is asked about
//!   once, the first time it would move — see [`Consent`].
//!
//! The lock rule: the tracker lives behind a LEAF mutex on [`crate::local_slot::LocalRuntime`].
//! Nothing — not `state.conn()`, not the slot lane, not the secrets cache — is acquired while it is
//! held, which is why [`crate::llm_gateway::resolve`] snapshots it BEFORE taking the DB guard.

use std::time::{Duration, Instant, SystemTime};

use crate::llm_gateway::Role;

/// Settings key: the charge level, in whole percent, at or below which PM moves to the cloud.
/// `"0"` means never. Absent means [`DEFAULT_THRESHOLD`] — on, and made safe by the consent ask.
pub const THRESHOLD_KEY: &str = "local_llm_power_threshold";
/// Settings key: which roles the policy may move — `"chat"` | `"background"` | `"both"`.
pub const SCOPE_KEY: &str = "local_llm_power_roles";
/// Settings key: the roles the user has agreed may go to the cloud on battery — `"chat"` |
/// `"background"` | `"both"`. Absent means not asked yet, and withdrawing deletes the row.
pub const CONSENT_KEY: &str = "local_llm_power_cloud_consent";

pub const DEFAULT_THRESHOLD: u8 = 60;
/// The highest stored threshold, so [`return_at`] stays at or below 95 and every offered value
/// round-trips through [`threshold_from`] unchanged.
pub const MAX_THRESHOLD: u8 = 80;
/// How far the charge has to climb back, on battery, before PM returns to local.
pub const RETURN_GAP: u8 = 15;
/// How long a change has to persist before PM acts on it.
pub const SETTLE: Duration = Duration::from_secs(60);
/// How often the watcher reads the machine.
pub const POLL: Duration = Duration::from_secs(30);
/// Three polls. A longer gap means the latch has not been watched and is no longer trusted.
pub const MAX_SAMPLE_GAP: Duration = Duration::from_secs(90);
/// How long one blocking read may take before the watcher treats it as Unknown.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the machine is drawing power from, as the OS reports it — before any settling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerSource {
    Ac,
    Battery,
    #[default]
    Unknown,
}

/// One reading of the machine. Default is Unknown: an unreadable machine can never read as "on
/// battery". There is deliberately no `Option<bool>` on_ac — `!on_ac.unwrap_or(false)` is the bug
/// this shape prevents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerReading {
    pub source: PowerSource,
    /// The system battery's charge, 0..=100. `None` when there is no battery or it won't say.
    pub percent: Option<u8>,
    /// A system battery exists. `false` is a desktop, which PM treats as always plugged in.
    pub has_battery: bool,
}

/// What PM acts on, after both guards. Unknown folds into `Mains`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerState {
    #[default]
    Mains,
    /// On battery, above the threshold (or the policy is off): nothing moves.
    Battery,
    /// On battery at or below the threshold: the roles in scope may move.
    BatteryLow,
}

/// Which roles the policy may move.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerScope {
    Chat,
    Background,
    #[default]
    Both,
}

impl PowerScope {
    /// Reading: absent or unknown is `Both`, the value the section shows pre-selected.
    pub fn from_setting(s: Option<&str>) -> Self {
        s.and_then(Self::parse_strict).unwrap_or_default()
    }

    /// Writing: strict — an unknown value is refused, never silently stored as `Both`.
    pub fn parse_strict(s: &str) -> Option<Self> {
        match s.trim() {
            "chat" => Some(Self::Chat),
            "background" => Some(Self::Background),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Background => "background",
            Self::Both => "both",
        }
    }

    pub fn covers(self, role: Role) -> bool {
        match self {
            Self::Both => true,
            Self::Chat => role == Role::Chat,
            Self::Background => role == Role::Background,
        }
    }
}

/// Which roles the user has said may go to the cloud on battery.
///
/// Per role, not one yes for everything, because the question names the roles that would move at
/// the moment it is asked — and an answer about background work must not quietly authorise chat
/// later, when chat becomes movable (a key added, a role switched to Local, fall back to cloud, the
/// scope widened). Data leaving the machine needs a yes that was about that data. A role outside the
/// consent is asked about once, the first time it would move; in the common case both roles can move
/// from the start and there is only ever the one question.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Consent {
    pub chat: bool,
    pub background: bool,
}

impl Consent {
    pub const NONE: Self = Self {
        chat: false,
        background: false,
    };
    #[cfg(test)]
    pub const ALL: Self = Self {
        chat: true,
        background: true,
    };

    /// Reading: absent or anything unrecognised is no consent at all. Only a yes that was written
    /// down counts.
    pub fn from_setting(s: Option<&str>) -> Self {
        match s.and_then(PowerScope::parse_strict) {
            Some(scope) => Self::NONE.with(scope),
            None => Self::NONE,
        }
    }

    pub fn covers(self, role: Role) -> bool {
        match role {
            Role::Chat => self.chat,
            Role::Background => self.background,
        }
    }

    /// This consent plus the roles `scope` names. Never removes a role.
    pub fn with(self, scope: PowerScope) -> Self {
        Self {
            chat: self.chat || scope.covers(Role::Chat),
            background: self.background || scope.covers(Role::Background),
        }
    }

    /// As stored and as sent to the webview; `None` is no consent.
    pub fn as_scope(self) -> Option<PowerScope> {
        match (self.chat, self.background) {
            (true, true) => Some(PowerScope::Both),
            (true, false) => Some(PowerScope::Chat),
            (false, true) => Some(PowerScope::Background),
            (false, false) => None,
        }
    }
}

impl serde::Serialize for Consent {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.as_scope().serialize(s)
    }
}

/// A stored threshold. Absent or unparseable is [`DEFAULT_THRESHOLD`]; anything above
/// [`MAX_THRESHOLD`] is clamped to it; `"0"` is "never".
pub fn threshold_from(stored: Option<&str>) -> u8 {
    stored
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|t| t.min(u64::from(MAX_THRESHOLD)) as u8)
        .unwrap_or(DEFAULT_THRESHOLD)
}

/// The charge at which PM returns to local while still on battery.
pub fn return_at(thr: u8) -> u8 {
    thr.saturating_add(RETURN_GAP)
}

/// The open store's policy settings, read on a borrowed connection (never locks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerSettings {
    pub threshold: u8,
    pub scope: PowerScope,
    pub consent: Consent,
}

impl PowerSettings {
    /// An unreadable policy is NO policy: any read error is this, never the default 60.
    pub const OFF: Self = Self {
        threshold: 0,
        scope: PowerScope::Both,
        consent: Consent::NONE,
    };

    pub fn read(conn: &rusqlite::Connection) -> Self {
        let read = || -> crate::error::Result<Self> {
            Ok(Self {
                threshold: threshold_from(crate::db::get_setting(conn, THRESHOLD_KEY)?.as_deref()),
                scope: PowerScope::from_setting(
                    crate::db::get_setting(conn, SCOPE_KEY)?.as_deref(),
                ),
                consent: Consent::from_setting(
                    crate::db::get_setting(conn, CONSENT_KEY)?.as_deref(),
                ),
            })
        };
        read().unwrap_or(Self::OFF)
    }
}

/// Where one reading points, given the state PM is in now. The level band only exists on battery:
/// any non-battery reading (AC, and Unknown) targets `Mains`, and that is what clears the band.
pub fn settle_target(current: PowerState, r: &PowerReading, thr: u8) -> PowerState {
    // Ac AND Unknown: Unknown never spends money.
    if r.source != PowerSource::Battery {
        return PowerState::Mains;
    }
    // Cloud only on positive evidence: a battery that won't say its charge is never "low".
    let Some(p) = r.percent else {
        return PowerState::Battery;
    };
    if thr == 0 {
        return PowerState::Battery;
    }
    let low = if current == PowerState::BatteryLow {
        p < return_at(thr)
    } else {
        p <= thr
    };
    if low {
        PowerState::BatteryLow
    } else {
        PowerState::Battery
    }
}

/// What a reader sees of the latch at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerSnapshot {
    /// The latest raw reading, before settling.
    pub reading: PowerReading,
    /// What PM acts on — `Mains` when the latch is stale.
    pub state: PowerState,
    /// The threshold the latch was computed on. `None` until a store has been read once.
    pub threshold: Option<u8>,
}

/// The latch. Fed by the watcher, read by routing and the status.
#[derive(Debug, Default)]
pub struct PowerTracker {
    state: PowerState,
    /// The first readable sample has been taken. Before it there is nothing to settle against.
    baselined: bool,
    /// A candidate state and when it was first seen; it latches once it has held for [`SETTLE`].
    pending: Option<(PowerState, Instant)>,
    threshold: Option<u8>,
    last: PowerReading,
    last_sample_at: Option<Instant>,
    /// The wall clock at the last sample, for [`Self::observe_wall`].
    last_wall: Option<SystemTime>,
    /// When the latch left `Mains`. `Some` only while the state is not `Mains`.
    battery_since: Option<Instant>,
}

impl PowerTracker {
    /// Feed one watcher sample. `threshold` `None` means the store was unreadable this tick, so the
    /// last known one is kept. Returns whether the SETTLED state changed — the only ping trigger.
    pub fn observe(&mut self, reading: PowerReading, threshold: Option<u8>, now: Instant) -> bool {
        let mut changed = false;
        // A verdict PM stopped watching is not trusted: drop to Mains and make the next one settle
        // again.
        if self.stale(now) {
            self.pending = None;
            changed |= self.set_state(PowerState::Mains, now);
        }
        self.last_sample_at = Some(now);
        // An Unknown sample says PM couldn't read the machine this time — not that its battery has
        // gone. Keeping what was known stops a single wedged read from turning a laptop into a
        // "desktop" in the section, which would disable the very override someone on battery reaches
        // for while the latch is still settling.
        self.last = if reading.source == PowerSource::Unknown {
            PowerReading {
                has_battery: reading.has_battery || self.last.has_battery,
                ..reading
            }
        } else {
            reading
        };
        if let Some(t) = threshold {
            changed |= self.apply_threshold(t, now);
        }
        // A threshold PM has never known behaves as "never".
        let thr = self.threshold.unwrap_or(0);
        if !self.baselined {
            if reading.source == PowerSource::Unknown {
                return changed;
            }
            // The launch baseline: there is nothing to wait out. A cold boot on battery must not
            // load weights for a minute before the policy engages.
            self.baselined = true;
            self.pending = None;
            changed |= self.set_state(settle_target(PowerState::Mains, &reading, thr), now);
            return changed;
        }
        let target = settle_target(self.state, &reading, thr);
        if target == self.state {
            self.pending = None;
            return changed;
        }
        match self.pending {
            Some((p, since)) if p == target => {
                if now.saturating_duration_since(since) >= SETTLE {
                    self.pending = None;
                    changed |= self.set_state(target, now);
                }
            }
            // A new or different candidate restarts the clock.
            _ => self.pending = Some((target, now)),
        }
        changed
    }

    /// A threshold PM has just learned (the first store read) or the user has just set. Never moves
    /// PM TOWARDS the cloud except on first knowledge; always may move it towards local at once.
    pub fn apply_threshold(&mut self, t: u8, now: Instant) -> bool {
        if self.threshold == Some(t) {
            return false;
        }
        let first = self.threshold.is_none();
        self.threshold = Some(t);
        if !self.baselined {
            return false;
        }
        // The plain entry rule, with no band memory: what this charge means under the new number.
        let entry = settle_target(PowerState::Battery, &self.last, t);
        if first {
            self.pending = None;
            let target = if self.state == PowerState::Mains {
                PowerState::Mains
            } else {
                entry
            };
            return self.set_state(target, now);
        }
        if self.state == PowerState::BatteryLow && entry != PowerState::BatteryLow {
            self.pending = None;
            return self.set_state(PowerState::Battery, now);
        }
        false
    }

    /// Feed the wall clock alongside each sample; call it BEFORE [`Self::observe`].
    ///
    /// `Instant` stands still while Linux and macOS are suspended, so to the monotonic clock a laptop
    /// that slept on battery for an hour looks like one that paused for a moment — and the stale rule
    /// would never fire across exactly the gap it was written for. Someone who closes the lid on
    /// battery and opens it plugged in would then keep going to the cloud for the full settle. The
    /// wall clock does see the sleep, so a wall gap past [`MAX_SAMPLE_GAP`] is treated like a
    /// monotonic one: the latch drops to `Mains` and the next reading settles again. A wall clock that
    /// jumps forward (an NTP correction) can only cause a spurious drop to `Mains`, the side that
    /// never spends money; one that jumps backwards is ignored. Returns whether the state changed.
    pub fn observe_wall(&mut self, wall: SystemTime, now: Instant) -> bool {
        let gap = self.last_wall.and_then(|w| wall.duration_since(w).ok());
        self.last_wall = Some(wall);
        if gap.is_some_and(|g| g > MAX_SAMPLE_GAP) {
            self.pending = None;
            return self.set_state(PowerState::Mains, now);
        }
        false
    }

    fn set_state(&mut self, new: PowerState, now: Instant) -> bool {
        if new == self.state {
            return false;
        }
        if new == PowerState::Mains {
            self.battery_since = None;
        } else if self.state == PowerState::Mains {
            self.battery_since = Some(now);
        }
        self.state = new;
        true
    }

    fn stale(&self, now: Instant) -> bool {
        self.last_sample_at
            .is_some_and(|t| now.saturating_duration_since(t) > MAX_SAMPLE_GAP)
    }

    /// Stale by the wall clock: what a read straight after waking sees, before the watcher's next
    /// tick has had the chance to run [`Self::observe_wall`] — `Instant` didn't move during the
    /// suspend, so only this can tell the verdict is from before it.
    fn wall_stale(&self, wall: Option<SystemTime>) -> bool {
        match (self.last_wall, wall) {
            (Some(last), Some(now)) => now.duration_since(last).is_ok_and(|g| g > MAX_SAMPLE_GAP),
            _ => false,
        }
    }

    /// The monotonic-only reading, for the reducer's own tests.
    #[cfg(test)]
    pub fn snapshot(&self, now: Instant) -> PowerSnapshot {
        self.snapshot_at(now, None)
    }

    /// [`Self::snapshot`], also distrusting a verdict the wall clock says is older than
    /// [`MAX_SAMPLE_GAP`]. What routing and the status read; `None` skips the wall check.
    pub fn snapshot_at(&self, now: Instant, wall: Option<SystemTime>) -> PowerSnapshot {
        PowerSnapshot {
            reading: self.last,
            state: if self.stale(now) || self.wall_stale(wall) {
                PowerState::Mains
            } else {
                self.state
            },
            threshold: self.threshold,
        }
    }

    /// How long PM has been settled off mains, or `None` on mains or while the latch is stale.
    #[cfg(test)]
    pub fn on_battery_for(&self, now: Instant) -> Option<Duration> {
        self.on_battery_for_at(now, None)
    }

    /// [`Self::on_battery_for`] with the wall-clock check of [`Self::snapshot_at`].
    pub fn on_battery_for_at(&self, now: Instant, wall: Option<SystemTime>) -> Option<Duration> {
        if self.stale(now) || self.wall_stale(wall) {
            return None;
        }
        self.battery_since.map(|s| now.saturating_duration_since(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ac(p: u8) -> PowerReading {
        PowerReading {
            source: PowerSource::Ac,
            percent: Some(p),
            has_battery: true,
        }
    }
    fn bat(p: u8) -> PowerReading {
        PowerReading {
            source: PowerSource::Battery,
            percent: Some(p),
            has_battery: true,
        }
    }
    fn bat_none() -> PowerReading {
        PowerReading {
            source: PowerSource::Battery,
            percent: None,
            has_battery: true,
        }
    }
    fn unk() -> PowerReading {
        PowerReading::default()
    }
    fn ac_desktop() -> PowerReading {
        PowerReading {
            source: PowerSource::Ac,
            percent: None,
            has_battery: false,
        }
    }

    /// A tracker with a fixed origin, which also checks T30 on every single call: a `true` return
    /// is exactly a settled-state change, so the ping fires once per change and never per poll.
    struct Rig {
        t: PowerTracker,
        t0: Instant,
        trues: usize,
        changes: usize,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                t: PowerTracker::default(),
                t0: Instant::now(),
                trues: 0,
                changes: 0,
            }
        }
        fn at(&self, s: f64) -> Instant {
            self.t0 + Duration::from_secs_f64(s)
        }
        fn count(&mut self, before: PowerState, returned: bool) -> bool {
            let changed = self.t.state != before;
            assert_eq!(
                returned, changed,
                "T30: a true return must be exactly a settled-state change"
            );
            self.trues += usize::from(returned);
            self.changes += usize::from(changed);
            returned
        }
        fn feed_thr(&mut self, r: PowerReading, thr: Option<u8>, s: f64) -> bool {
            let before = self.t.state;
            let now = self.at(s);
            let returned = self.t.observe(r, thr, now);
            self.count(before, returned)
        }
        fn feed(&mut self, r: PowerReading, s: f64) -> bool {
            self.feed_thr(r, Some(60), s)
        }
        fn apply(&mut self, thr: u8, s: f64) -> bool {
            let before = self.t.state;
            let now = self.at(s);
            let returned = self.t.apply_threshold(thr, now);
            self.count(before, returned)
        }
        fn state(&self, s: f64) -> PowerState {
            self.t.snapshot(self.at(s)).state
        }
        /// Feed `r` every 30 s from `from` to `to` inclusive.
        fn hold(&mut self, r: PowerReading, from: f64, to: f64) {
            let mut s = from;
            while s <= to + 1e-9 {
                self.feed(r, s);
                s += 30.0;
            }
        }
        /// A latch already settled in BatteryLow at `p`, last sampled at 0.
        fn battery_low(p: u8) -> Self {
            let mut rig = Self::new();
            rig.feed(bat(p), 0.0);
            assert_eq!(rig.state(0.0), PowerState::BatteryLow);
            rig
        }
    }

    #[test]
    fn t1_a_fresh_tracker_is_on_mains_and_knows_no_threshold() {
        let rig = Rig::new();
        let snap = rig.t.snapshot(rig.at(0.0));
        assert_eq!(snap.state, PowerState::Mains);
        assert_eq!(snap.threshold, None);
        assert_eq!(rig.t.on_battery_for(rig.at(0.0)), None);
    }

    #[test]
    fn t2_a_launch_on_a_low_battery_latches_at_once() {
        let mut rig = Rig::new();
        assert!(rig.feed(bat(40), 0.0));
        assert_eq!(rig.state(0.0), PowerState::BatteryLow);
        assert_eq!(
            rig.t.on_battery_for(rig.at(10.0)),
            Some(Duration::from_secs(10))
        );
    }

    #[test]
    fn t3_a_launch_on_ac_is_mains() {
        let mut rig = Rig::new();
        assert!(!rig.feed(ac(100), 0.0));
        assert_eq!(rig.state(0.0), PowerState::Mains);
    }

    #[test]
    fn t4_an_unknown_first_sample_is_not_a_baseline() {
        let mut rig = Rig::new();
        rig.feed(unk(), 0.0);
        assert_eq!(rig.state(0.0), PowerState::Mains);
        rig.feed(bat(40), 30.0);
        assert_eq!(
            rig.state(30.0),
            PowerState::BatteryLow,
            "still the first readable sample"
        );
    }

    #[test]
    fn t5_first_knowledge_of_the_threshold_is_a_baseline_too() {
        let mut rig = Rig::new();
        rig.feed_thr(bat(40), None, 0.0);
        assert_eq!(
            rig.state(0.0),
            PowerState::Battery,
            "an unknown threshold never goes low"
        );
        assert!(rig.feed_thr(bat(40), Some(60), 30.0));
        assert_eq!(rig.state(30.0), PowerState::BatteryLow);
    }

    #[test]
    fn t6_unplugging_settles_after_a_minute_and_pings_once() {
        let mut rig = Rig::new();
        rig.feed(ac(100), 0.0);
        rig.feed(bat(50), 30.0);
        assert_eq!(rig.state(30.0), PowerState::Mains);
        rig.feed(bat(50), 60.0);
        assert_eq!(rig.state(60.0), PowerState::Mains);
        rig.feed(bat(50), 90.0);
        assert_eq!(rig.state(90.0), PowerState::BatteryLow);
        assert_eq!(rig.trues, 1);
    }

    #[test]
    fn t7_the_settle_time_is_inclusive() {
        let mut a = Rig::new();
        a.feed(ac(100), 0.0);
        a.feed(bat(50), 30.0);
        a.feed(bat(50), 89.999);
        assert_eq!(a.state(89.999), PowerState::Mains);
        let mut b = Rig::new();
        b.feed(ac(100), 0.0);
        b.feed(bat(50), 30.0);
        b.feed(bat(50), 90.0);
        assert_eq!(b.state(90.0), PowerState::BatteryLow);
    }

    #[test]
    fn t8_the_threshold_itself_is_low() {
        let mut rig = Rig::new();
        rig.feed(bat(65), 0.0);
        assert_eq!(rig.state(0.0), PowerState::Battery);
        rig.hold(bat(60), 30.0, 90.0);
        assert_eq!(rig.state(90.0), PowerState::BatteryLow);
    }

    #[test]
    fn t9_one_point_above_the_threshold_never_goes_low() {
        let mut rig = Rig::new();
        rig.feed(ac(100), 0.0);
        // Read at +60 itself, not after the whole hold: `snapshot(now)` only uses `now` for
        // staleness, so asking about an earlier moment after later samples reads the LATER state.
        rig.hold(bat(61), 30.0, 60.0);
        assert_eq!(rig.state(60.0), PowerState::Mains, "pending, not latched");
        rig.feed(bat(61), 90.0);
        assert_eq!(rig.state(90.0), PowerState::Battery, "latched at +60");
        assert_eq!(rig.trues, 1);
        rig.hold(bat(61), 120.0, 150.0);
        assert_eq!(rig.state(150.0), PowerState::Battery);
        assert_eq!(rig.trues, 1, "and never BatteryLow");
    }

    #[test]
    fn t10_inside_the_band_it_stays_low() {
        let mut rig = Rig::battery_low(50);
        let mut s = 30.0;
        for p in [61, 70, 74] {
            for _ in 0..7 {
                rig.feed(bat(p), s);
                assert_eq!(rig.state(s), PowerState::BatteryLow, "{p}% at {s}");
                s += 30.0;
            }
        }
    }

    #[test]
    fn t11_the_return_point_is_inclusive() {
        let mut rig = Rig::battery_low(50);
        rig.feed(bat(75), 30.0);
        rig.feed(bat(75), 90.0);
        assert_eq!(rig.state(90.0), PowerState::Battery);

        let mut held = Rig::battery_low(50);
        held.hold(bat(74), 30.0, 300.0);
        assert_eq!(held.state(300.0), PowerState::BatteryLow);
    }

    #[test]
    fn t12_plugging_in_returns_to_local_whatever_the_charge() {
        let mut rig = Rig::battery_low(30);
        rig.feed(ac(30), 30.0);
        rig.feed(ac(30), 60.0);
        assert_eq!(rig.state(60.0), PowerState::BatteryLow);
        rig.feed(ac(30), 89.0);
        assert_eq!(rig.state(89.0), PowerState::BatteryLow);
        rig.feed(ac(30), 90.0);
        assert_eq!(rig.state(90.0), PowerState::Mains);
        assert_eq!(rig.t.on_battery_for(rig.at(90.0)), None);
    }

    #[test]
    fn t13_each_flip_restarts_the_guard() {
        let mut rig = Rig::battery_low(50);
        rig.feed(ac(50), 30.0);
        rig.feed(bat(50), 60.0);
        rig.feed(ac(50), 90.0);
        rig.feed(ac(50), 120.0);
        assert_eq!(rig.state(120.0), PowerState::BatteryLow);
        rig.feed(ac(50), 150.0);
        assert_eq!(rig.state(150.0), PowerState::Mains);
    }

    #[test]
    fn t14_unplugging_to_move_changes_nothing() {
        let mut rig = Rig::new();
        rig.feed(ac(50), 0.0);
        rig.feed(bat(50), 30.0);
        rig.feed(ac(50), 60.0);
        assert_eq!(rig.state(60.0), PowerState::Mains);
        rig.feed(bat(50), 80.0);
        rig.feed(bat(50), 110.0);
        assert_eq!(rig.state(110.0), PowerState::Mains);
        rig.feed(bat(50), 140.0);
        assert_eq!(rig.state(140.0), PowerState::BatteryLow);
    }

    #[test]
    fn t15_ac_clears_the_band() {
        let mut rig = Rig::battery_low(50);
        rig.hold(ac(62), 30.0, 90.0);
        assert_eq!(rig.state(90.0), PowerState::Mains);
        rig.hold(bat(62), 120.0, 180.0);
        assert_eq!(
            rig.state(180.0),
            PowerState::Battery,
            "re-entry is the plain <= T rule, not the band"
        );
    }

    #[test]
    fn t16_threshold_zero_never_goes_low() {
        let mut rig = Rig::new();
        let mut s = 0.0;
        while s <= 600.0 {
            rig.feed_thr(bat(10), Some(0), s);
            assert_eq!(rig.state(s), PowerState::Battery);
            s += 30.0;
        }
        assert!(rig.t.on_battery_for(rig.at(600.0)).is_some());
    }

    #[test]
    fn t17_a_lowered_or_disabled_threshold_returns_to_local_at_once() {
        let mut never = Rig::battery_low(50);
        assert!(never.apply(0, 10.0));
        assert_eq!(never.state(10.0), PowerState::Battery);

        let mut lower = Rig::battery_low(50);
        assert!(lower.apply(45, 10.0));
        assert_eq!(lower.state(10.0), PowerState::Battery);

        let mut higher = Rig::battery_low(50);
        assert!(!higher.apply(55, 10.0));
        assert_eq!(higher.state(10.0), PowerState::BatteryLow);
    }

    #[test]
    fn t18_a_raised_threshold_reaches_the_cloud_only_through_the_settle() {
        let mut rig = Rig::new();
        rig.feed(bat(65), 0.0);
        assert!(!rig.apply(70, 10.0));
        assert_eq!(rig.state(10.0), PowerState::Battery);
        rig.feed_thr(bat(65), Some(70), 40.0);
        assert_eq!(rig.state(40.0), PowerState::Battery);
        rig.feed_thr(bat(65), Some(70), 100.0);
        assert_eq!(rig.state(100.0), PowerState::BatteryLow);
    }

    #[test]
    fn t19_a_battery_that_wont_say_its_charge_is_never_low() {
        let mut rig = Rig::new();
        rig.feed(ac(100), 0.0);
        rig.hold(bat_none(), 30.0, 90.0);
        assert_eq!(rig.state(90.0), PowerState::Battery);

        let mut low = Rig::battery_low(40);
        low.hold(bat_none(), 30.0, 90.0);
        assert_eq!(low.state(90.0), PowerState::Battery);
    }

    #[test]
    fn t20_an_unreadable_store_keeps_the_last_threshold() {
        let mut rig = Rig::new();
        rig.feed(bat(65), 0.0);
        rig.feed_thr(bat(65), None, 30.0);
        assert_eq!(rig.t.snapshot(rig.at(30.0)).threshold, Some(60));
    }

    #[test]
    fn t21_a_clock_that_runs_backwards_latches_nothing() {
        let mut rig = Rig::new();
        rig.feed(ac(100), 0.0);
        rig.feed(bat(50), 10.0);
        rig.feed(bat(50), 5.0);
        assert_eq!(rig.state(5.0), PowerState::Mains);
    }

    #[test]
    fn t22_a_different_candidate_restarts_the_clock() {
        let mut rig = Rig::new();
        rig.feed(ac(100), 0.0);
        rig.feed(bat(61), 30.0);
        rig.feed(bat(59), 60.0);
        rig.feed(bat(59), 90.0);
        assert_eq!(rig.state(90.0), PowerState::Mains, "never Battery here");
        rig.feed(bat(59), 120.0);
        assert_eq!(rig.state(120.0), PowerState::BatteryLow);
    }

    #[test]
    fn t23_a_charge_wobbling_on_ac_never_pings() {
        let mut rig = Rig::new();
        let mut s = 0.0;
        let mut flip = false;
        while s <= 600.0 {
            rig.feed(ac(if flip { 59 } else { 61 }), s);
            flip = !flip;
            s += 30.0;
        }
        assert_eq!(rig.trues, 0);
    }

    #[test]
    fn t24_an_unknown_blip_is_not_an_unplug() {
        let mut blip = Rig::battery_low(50);
        blip.feed(unk(), 30.0);
        blip.feed(bat(50), 60.0);
        assert_eq!(blip.state(60.0), PowerState::BatteryLow);

        let mut gone = Rig::battery_low(50);
        gone.hold(unk(), 30.0, 90.0);
        assert_eq!(gone.state(90.0), PowerState::Mains);
    }

    #[test]
    fn t25_a_long_gap_drops_the_latch_and_the_next_sample_settles_again() {
        let mut rig = Rig::battery_low(50);
        assert!(rig.feed(bat(50), 91.0), "demoted to Mains");
        assert_eq!(rig.state(91.0), PowerState::Mains);
        rig.feed(bat(50), 121.0);
        assert_eq!(rig.state(121.0), PowerState::Mains);
        rig.feed(bat(50), 151.0);
        assert_eq!(rig.state(151.0), PowerState::BatteryLow);
    }

    #[test]
    fn t26_exactly_the_gap_is_not_a_gap() {
        let mut rig = Rig::battery_low(50);
        assert!(!rig.feed(bat(50), 90.0));
        assert_eq!(rig.state(90.0), PowerState::BatteryLow);
    }

    #[test]
    fn t27_a_stale_latch_reads_as_mains() {
        let rig = Rig::battery_low(50);
        assert_eq!(rig.state(90.0), PowerState::BatteryLow);
        assert_eq!(rig.state(90.001), PowerState::Mains);
        assert_eq!(rig.t.on_battery_for(rig.at(91.0)), None);
    }

    #[test]
    fn t28_a_gap_restarts_a_pending_change() {
        let mut rig = Rig::new();
        rig.feed(ac(100), 0.0);
        rig.feed(bat(50), 10.0);
        rig.feed(bat(50), 110.0);
        assert_eq!(rig.state(110.0), PowerState::Mains);
        rig.feed(bat(50), 140.0);
        assert_eq!(
            rig.state(140.0),
            PowerState::Mains,
            "the pending restarted at 110"
        );
        rig.feed(bat(50), 170.0);
        assert_eq!(rig.state(170.0), PowerState::BatteryLow);
    }

    #[test]
    fn t29_a_desktop_is_mains_forever() {
        let mut rig = Rig::new();
        let desktop = PowerReading {
            source: PowerSource::Ac,
            percent: None,
            has_battery: false,
        };
        for i in 0..100 {
            rig.feed(desktop, f64::from(i) * 30.0);
        }
        assert_eq!(rig.state(2970.0), PowerState::Mains);
        assert_eq!(rig.trues, 0);
    }

    #[test]
    fn t32_a_suspend_the_monotonic_clock_never_saw_still_distrusts_the_latch() {
        // Lid closed on battery for an hour; opened plugged in. `Instant` didn't move, so only the
        // wall clock can tell PM the latch is an hour old.
        let mut rig = Rig::battery_low(50);
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert!(
            !rig.t.observe_wall(wall, rig.at(0.0)),
            "the first call has nothing to compare"
        );
        assert!(!rig
            .t
            .observe_wall(wall + Duration::from_secs(30), rig.at(30.0)));
        assert_eq!(rig.state(30.0), PowerState::BatteryLow);
        // Monotonic: 30 s later. Wall: an hour later.
        assert!(rig
            .t
            .observe_wall(wall + Duration::from_secs(3630), rig.at(60.0)));
        assert_eq!(rig.state(60.0), PowerState::Mains);
        assert_eq!(rig.t.on_battery_for(rig.at(60.0)), None);
        // And the next reading has to settle again before anything goes back to the cloud.
        rig.feed(bat(50), 60.0);
        assert_eq!(rig.state(60.0), PowerState::Mains);
        rig.feed(bat(50), 120.0);
        assert_eq!(rig.state(120.0), PowerState::BatteryLow);
    }

    #[test]
    fn t32b_a_read_straight_after_waking_distrusts_the_latch_before_the_next_tick() {
        // The watcher only sees the suspend at its next tick, up to a poll later; a chat sent in
        // that window must not route on the pre-suspend verdict either.
        let mut rig = Rig::battery_low(50);
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        rig.t.observe_wall(wall, rig.at(0.0));
        let woke = Some(wall + Duration::from_secs(3600));
        assert_eq!(
            rig.t.snapshot_at(rig.at(5.0), woke).state,
            PowerState::Mains
        );
        assert_eq!(rig.t.on_battery_for_at(rig.at(5.0), woke), None);
        // Within the gap it is trusted as before.
        let soon = Some(wall + Duration::from_secs(20));
        assert_eq!(
            rig.t.snapshot_at(rig.at(5.0), soon).state,
            PowerState::BatteryLow
        );
    }

    #[test]
    fn t33_a_wall_clock_that_steps_back_or_within_the_gap_changes_nothing() {
        let mut rig = Rig::battery_low(50);
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        rig.t.observe_wall(wall, rig.at(0.0));
        assert!(
            !rig.t
                .observe_wall(wall + Duration::from_secs(90), rig.at(30.0)),
            "90 s is not more than 90 s"
        );
        // A clock corrected backwards by an hour: no demotion, and the next tick measures from the
        // corrected time rather than looking like an hour's jump forward.
        let back = wall - Duration::from_secs(3600);
        assert!(!rig.t.observe_wall(back, rig.at(60.0)));
        assert!(!rig
            .t
            .observe_wall(back + Duration::from_secs(30), rig.at(90.0)));
        assert_eq!(rig.state(90.0), PowerState::BatteryLow);
    }

    #[test]
    fn t34_an_unknown_sample_does_not_forget_the_machine_has_a_battery() {
        // A wedged read is an Unknown sample. It must not turn a laptop into a desktop in the
        // section — that would disable the override while the latch is still settling.
        let mut rig = Rig::battery_low(50);
        rig.feed(unk(), 30.0);
        let snap = rig.t.snapshot(rig.at(30.0));
        assert_eq!(snap.reading.source, PowerSource::Unknown);
        assert!(snap.reading.has_battery);
        assert_eq!(snap.reading.percent, None, "a stale charge is not quoted");
        // A desktop stays a desktop.
        let mut desk = Rig::new();
        desk.feed(ac_desktop(), 0.0);
        desk.feed(unk(), 30.0);
        assert!(!desk.t.snapshot(desk.at(30.0)).reading.has_battery);
    }

    #[test]
    fn consent_is_per_role_and_only_ever_grows() {
        assert_eq!(Consent::from_setting(None), Consent::NONE);
        assert_eq!(Consent::from_setting(Some("junk")), Consent::NONE);
        assert_eq!(Consent::from_setting(Some("both")), Consent::ALL);
        let bg = Consent::from_setting(Some("background"));
        assert!(bg.covers(Role::Background) && !bg.covers(Role::Chat));
        assert_eq!(bg.as_scope(), Some(PowerScope::Background));
        assert_eq!(bg.with(PowerScope::Chat), Consent::ALL);
        assert_eq!(Consent::ALL.with(PowerScope::Chat), Consent::ALL);
        assert_eq!(Consent::NONE.as_scope(), None);
        // The wire shape the TypeScript mirror reads: a scope string, or null.
        assert_eq!(serde_json::to_value(bg).unwrap(), "background");
        assert_eq!(
            serde_json::to_value(Consent::NONE).unwrap(),
            serde_json::Value::Null
        );
    }

    #[test]
    fn t30_true_returns_count_settled_changes() {
        // Every row above runs through `Rig::count`, which asserts this per call. One long mixed
        // sequence here as well, so the tally is checked over transitions in every direction.
        let mut rig = Rig::new();
        rig.feed(ac(100), 0.0);
        rig.hold(bat(70), 30.0, 90.0); // → Battery
        rig.hold(bat(55), 120.0, 180.0); // → BatteryLow
        rig.apply(0, 190.0); // → Battery at once
        rig.hold(ac(55), 210.0, 270.0); // → Mains
        rig.feed(bat(50), 400.0); // a gap, then a pending restart
        assert_eq!(rig.trues, rig.changes);
        assert_eq!(rig.trues, 4);
    }

    #[test]
    fn t31_settle_target_table() {
        use PowerState::*;
        for (current, reading, thr, expected) in [
            (Mains, ac(50), 60, Mains),
            (Mains, unk(), 60, Mains),
            (Battery, bat(60), 60, BatteryLow),
            (BatteryLow, bat(74), 60, BatteryLow),
            (BatteryLow, bat(75), 60, Battery),
            (Mains, bat(10), 0, Battery),
            (Mains, bat_none(), 60, Battery),
        ] {
            assert_eq!(
                settle_target(current, &reading, thr),
                expected,
                "({current:?}, {reading:?}, {thr})"
            );
        }
    }

    #[test]
    fn threshold_parses_clamps_and_defaults() {
        assert_eq!(threshold_from(None), 60);
        assert_eq!(threshold_from(Some("0")), 0);
        assert_eq!(threshold_from(Some(" 40 ")), 40);
        assert_eq!(threshold_from(Some("61")), 61);
        assert_eq!(threshold_from(Some("200")), 80);
        assert_eq!(threshold_from(Some("x")), 60);
        assert_eq!(threshold_from(Some("-5")), 60);
        assert_eq!(return_at(60), 75);
        assert_eq!(return_at(80), 95);
    }

    #[test]
    fn scope_reads_leniently_and_writes_strictly() {
        assert_eq!(PowerScope::from_setting(None), PowerScope::Both);
        assert_eq!(PowerScope::from_setting(Some("junk")), PowerScope::Both);
        for s in [PowerScope::Chat, PowerScope::Background, PowerScope::Both] {
            assert_eq!(PowerScope::from_setting(Some(s.as_setting())), s);
            assert_eq!(PowerScope::parse_strict(s.as_setting()), Some(s));
        }
        assert_eq!(PowerScope::parse_strict("junk"), None);

        assert!(PowerScope::Both.covers(Role::Chat));
        assert!(PowerScope::Both.covers(Role::Background));
        assert!(PowerScope::Chat.covers(Role::Chat));
        assert!(!PowerScope::Chat.covers(Role::Background));
        assert!(!PowerScope::Background.covers(Role::Chat));
        assert!(PowerScope::Background.covers(Role::Background));
    }

    /// The wire spelling the TypeScript mirror (`types.ts`) is written against.
    #[test]
    fn the_enums_serialize_as_the_frontend_mirrors_them() {
        let json = |v: serde_json::Value| v.as_str().unwrap().to_string();
        let source = |s: PowerSource| json(serde_json::to_value(s).unwrap());
        assert_eq!(source(PowerSource::Ac), "ac");
        assert_eq!(source(PowerSource::Battery), "battery");
        assert_eq!(source(PowerSource::Unknown), "unknown");
        let state = |s: PowerState| json(serde_json::to_value(s).unwrap());
        assert_eq!(state(PowerState::Mains), "mains");
        assert_eq!(state(PowerState::Battery), "battery");
        assert_eq!(state(PowerState::BatteryLow), "battery_low");
        for scope in [PowerScope::Chat, PowerScope::Background, PowerScope::Both] {
            // The stored spelling and the wire spelling are one spelling.
            assert_eq!(
                json(serde_json::to_value(scope).unwrap()),
                scope.as_setting()
            );
        }
    }

    /// A throwaway encrypted store, mirroring `commands`'s test fixture.
    fn temp_db() -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = crate::db::open(&path, key).unwrap();
        (dir, conn)
    }

    #[test]
    fn settings_read_back_and_an_unreadable_store_is_off() {
        let (_dir, conn) = temp_db();
        assert_eq!(
            PowerSettings::read(&conn),
            PowerSettings {
                threshold: 60,
                scope: PowerScope::Both,
                consent: Consent::NONE
            }
        );
        crate::db::set_setting(&conn, THRESHOLD_KEY, "40").unwrap();
        crate::db::set_setting(&conn, SCOPE_KEY, "chat").unwrap();
        crate::db::set_setting(&conn, CONSENT_KEY, "background").unwrap();
        assert_eq!(
            PowerSettings::read(&conn),
            PowerSettings {
                threshold: 40,
                scope: PowerScope::Chat,
                consent: Consent {
                    chat: false,
                    background: true
                }
            }
        );
        // A value PM doesn't recognise is no consent — a bare "true" included.
        crate::db::set_setting(&conn, CONSENT_KEY, "true").unwrap();
        assert_eq!(PowerSettings::read(&conn).consent, Consent::NONE);

        // No settings table at all: an unreadable policy is no policy, never the default 60.
        let bare = rusqlite::Connection::open_in_memory().unwrap();
        assert_eq!(PowerSettings::read(&bare), PowerSettings::OFF);
    }
}
