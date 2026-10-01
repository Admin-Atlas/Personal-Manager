// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! When PM should hand the graphics card back (#786 item 8).
//!
//! A local model sits in video memory for as long as the server keeps it there, which on a machine
//! whose owner raised `OLLAMA_KEEP_ALIVE` is forever. That is the right default for a server — and
//! the wrong one for a laptop where PM is one of several things wanting the card. This module is the
//! pure decision: everything that talks to a server lives elsewhere, so every rule below is testable
//! against an injected clock.
//!
//! **The one mechanism PM is allowed to use is `keep_alive: 0`.** Measured against Ollama 0.33.0:
//! a single request carrying a POSITIVE `keep_alive` reprograms that runner for the rest of its life
//! — a later request omitting the field inherits the expiry and slides it forward, and so does one
//! arriving on `/v1/chat/completions`, which has no such field at all. So the obvious implementation
//! of "release after five minutes" (put a five-minute TTL on each request) would silently demote the
//! user's own server configuration, for every client of that server, permanently. PM instead runs its
//! OWN timer and sends an explicit unload, which leaves nothing behind.
//!
//! Two rules make the rest safe:
//!
//! 1. **PM only ever releases a model PM caused to be loaded.** A model someone loaded from a
//!    terminal is theirs, and a settings pane that quietly unloads it is a scheduler acting on
//!    something nobody consented to.
//! 2. **Releasing is never evidence about the endpoint.** It records no health outcome in either
//!    direction: scoring a success would let housekeeping clear a failing host's strike streak, and
//!    scoring a failure would eject a healthy one.
//!
//! **On battery** (#432) there is a second clock, set separately and off by default: hand the
//! memory back once nothing has used the model for a while, whatever the policy says. A graphics
//! card holding a model keeps drawing power with nothing asking it anything, which on a laptop is
//! battery spent on nothing. It is fed by the On battery latch — the SETTLED state, never a raw
//! reading — and it counts from no earlier than the unplug, so carrying a laptop to the sofa keeps a
//! model that was in use a moment ago. It sits behind the same ownership, activity and intent gates
//! as everything else here.

use std::time::Duration;

/// Settings key for the release policy. Absent → [`ReleasePolicy::Server`], so an install that never
/// touches this setting behaves exactly as it did before the feature existed.
pub const RELEASE_POLICY_KEY: &str = "local_llm_release_policy";
/// Settings key for the quiet period, in whole minutes. Absent → [`DEFAULT_IDLE_MINUTES`].
pub const RELEASE_IDLE_MINUTES_KEY: &str = "local_llm_release_idle_minutes";
/// Settings key for the on-battery quiet period (#432), in whole minutes. Absent or `"0"` = off.
pub const BATTERY_IDLE_MINUTES_KEY: &str = "local_llm_battery_idle_minutes";

/// The default quiet period. Five minutes because a chat exchange has gaps of a minute or two while
/// you read a reply and type the next thing, and paying a three-second reload inside one conversation
/// would be worse than holding the memory. Long enough to sit inside a conversation, short enough to
/// have given the card back by the time you have moved on.
pub const DEFAULT_IDLE_MINUTES: u64 = 5;
/// Guard rails on the stored value. A zero would release between two turns of the same conversation;
/// anything past a couple of hours is indistinguishable from "never" and should be said that way.
pub const MIN_IDLE_MINUTES: u64 = 1;
pub const MAX_IDLE_MINUTES: u64 = 120;

/// What PM does with a model it loaded, once nothing needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleasePolicy {
    /// PM never unloads anything. The default, and deliberately so: whatever the server was going to
    /// do with its own memory is what happens, and a machine that installs PM notices no change —
    /// except on battery, when you have set an on-battery release time.
    Server,
    /// Release when PM's process ends. Not when the window closes — with the tray icon on, a closed
    /// window means PM is still working for you.
    OnExit,
    /// Release after a quiet period, and on exit.
    Idle,
}

impl ReleasePolicy {
    /// Parse a stored value. Anything absent or unrecognised is [`Self::Server`] — the option that
    /// changes nothing, which is the only safe reading of a value PM does not understand.
    pub fn from_setting(s: Option<&str>) -> Self {
        match s {
            Some("on-exit") => Self::OnExit,
            Some("idle") => Self::Idle,
            _ => Self::Server,
        }
    }

    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::OnExit => "on-exit",
            Self::Idle => "idle",
        }
    }
}

/// A stored quiet period, clamped into range. Absent, unparseable or out of range all resolve to a
/// usable number rather than disabling the policy the user chose.
pub fn idle_after(stored: Option<&str>) -> Duration {
    let minutes = stored
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_MINUTES)
        .clamp(MIN_IDLE_MINUTES, MAX_IDLE_MINUTES);
    Duration::from_secs(minutes * 60)
}

/// A stored on-battery quiet period (#432). Absent, empty, `"0"`, negative or unparseable is `None`
/// — off — and anything else is clamped into the same range as the policy's own quiet period.
///
/// Deliberately not [`idle_after`]: that one turns `"0"` into a minute, because for the policy a
/// zero is a mistake to be corrected into something usable. Here zero is the off switch, and
/// reading it as "release after a minute on battery" would turn the feature on for everyone.
pub fn battery_idle_after(stored: Option<&str>) -> Option<Duration> {
    let minutes = stored?.trim().parse::<u64>().ok().filter(|m| *m > 0)?;
    Some(Duration::from_secs(
        minutes.clamp(MIN_IDLE_MINUTES, MAX_IDLE_MINUTES) * 60,
    ))
}

/// The value to store for an on-battery quiet period: zero stays zero (off), anything else is
/// clamped into range.
pub fn clamp_battery_idle(m: u64) -> u64 {
    if m == 0 {
        0
    } else {
        m.clamp(MIN_IDLE_MINUTES, MAX_IDLE_MINUTES)
    }
}

/// Everything the release scheduler reads from settings, cached together so a locked vault keeps
/// honouring all of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReleaseConfig {
    pub policy: ReleasePolicy,
    pub idle_after: Duration,
    pub battery_idle_after: Option<Duration>,
}

/// Everything the decision depends on. Assembled by the caller from live state so this stays pure.
#[derive(Debug, Clone, Copy)]
pub struct ReleaseInputs {
    pub policy: ReleasePolicy,
    /// PM caused this model to be loaded. When false, nothing below matters — it is not PM's to free.
    pub pm_loaded: bool,
    /// Calls occupying the local slot right now.
    pub in_flight: usize,
    /// Background jobs PM has already spawned that have not reached the slot yet.
    ///
    /// Without this the release races the work. `send_message` spawns its follow-up jobs *after* the
    /// reply returns, so the slot is genuinely quiet in the gap between them — and a release taken in
    /// that gap makes every one of them cold-load. Each cold load is charged whole against the flat
    /// 180 s background timeout with no "model loading" response to trigger the retry budget, and
    /// three timeouts are three strikes: a 60-300 s cooldown that `run_local_stream` applies to CHAT
    /// as well. A hold is minted when PM spawns the work, so it cannot be a bad prediction — PM
    /// created the thing it is predicting.
    pub holds: usize,
    /// How long the slot has been quiet, with no hold outstanding.
    pub quiet_for: Duration,
    /// The user's chosen quiet period.
    pub idle_after: Duration,
    /// How long PM has been on battery by the SETTLED On battery latch (#432). `None` on mains, and
    /// `None` while the latch is stale. Never a raw reading: a cable wiggle must not release a model.
    pub on_battery_for: Option<Duration>,
    /// The on-battery quiet period, or `None` when it is off.
    pub battery_idle_after: Option<Duration>,
}

/// Whether to release now, on the idle path.
///
/// Written as one flat ladder because the order is the safety argument: ownership first (it may not
/// be PM's model at all), then activity (something is using it), then intent (something is about to),
/// and only then the policy and the clock.
pub fn should_release(i: &ReleaseInputs) -> bool {
    if !i.pm_loaded || i.in_flight > 0 || i.holds > 0 {
        return false;
    }
    // `OnExit` is not "never" — it is "not on a timer". Its release happens in the exit hook.
    let policy_due = matches!(i.policy, ReleasePolicy::Idle) && i.quiet_for >= i.idle_after;
    // Counted from no earlier than the unplug, so carrying the laptop to the sofa keeps a model just used.
    let battery_due = matches!(
        (i.on_battery_for, i.battery_idle_after),
        (Some(b), Some(d)) if i.quiet_for.min(b) >= d
    );
    policy_due || battery_due
}

/// Which endpoints a release pass should visit.
///
/// `fresh` is what the open store says the endpoint is: `Some(Some(url))` a configured one,
/// `Some(None)` none at all. With the store CLOSED (`None`) PM cannot read the endpoint — and a model
/// PM loaded is still sitting on the card, which is the case the cached policy exists for. So a
/// closed store visits every endpoint PM itself loaded something on: the same unfiltered set the
/// exit hook releases, sorted so the order is stable.
pub fn release_endpoints(
    fresh: Option<Option<&str>>,
    pm_loaded: &[(String, String)],
) -> Vec<String> {
    match fresh {
        Some(Some(base)) if !base.trim().is_empty() => vec![base.to_string()],
        Some(_) => Vec::new(),
        None => {
            let mut bases: Vec<String> = pm_loaded.iter().map(|(b, _)| b.clone()).collect();
            bases.sort();
            bases.dedup();
            bases
        }
    }
}

/// The bearer token to send to one endpoint a release pass visits.
///
/// The stored token belongs to the endpoint the user configured, and to no other. A release pass can
/// visit more than one — the exit hook releases everywhere PM loaded something, and with the store
/// closed so does the timer — and someone who moved from a tokened remote server to another, or back
/// to loopback, still has PM's models sitting on the old one. Sending the current token there would
/// hand one server's credential to a different host. So an endpoint that is not the configured one
/// gets no token: an unload it then refuses costs a model left loaded, which is recoverable, and a
/// leaked credential is not. `configured` of `None` means PM does not know which endpoint the token
/// is for, and the answer is then always no token.
pub fn token_for<'a>(
    base_url: &str,
    configured: Option<&str>,
    token: Option<&'a str>,
) -> Option<&'a str> {
    token.filter(|_| configured.is_some_and(|c| c == base_url))
}

/// Whether to release as PM shuts down.
///
/// Deliberately ignores `in_flight` and `holds`: the process is going away, so work in flight is
/// going away with it, and the alternative is leaving gigabytes held by a process that no longer
/// exists. It still respects ownership and the policy.
pub fn should_release_on_exit(policy: ReleasePolicy, pm_loaded: bool) -> bool {
    pm_loaded && matches!(policy, ReleasePolicy::OnExit | ReleasePolicy::Idle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle(quiet_secs: u64) -> ReleaseInputs {
        ReleaseInputs {
            policy: ReleasePolicy::Idle,
            pm_loaded: true,
            in_flight: 0,
            holds: 0,
            quiet_for: Duration::from_secs(quiet_secs),
            idle_after: Duration::from_secs(300),
            on_battery_for: None,
            battery_idle_after: None,
        }
    }

    /// The policy that changes nothing, on battery with an on-battery quiet period set.
    fn on_battery(quiet_secs: u64, on_battery_secs: u64, battery_idle_secs: u64) -> ReleaseInputs {
        ReleaseInputs {
            policy: ReleasePolicy::Server,
            on_battery_for: Some(Duration::from_secs(on_battery_secs)),
            battery_idle_after: Some(Duration::from_secs(battery_idle_secs)),
            ..idle(quiet_secs)
        }
    }

    #[test]
    fn an_unrecognised_or_absent_policy_changes_nothing() {
        // The default has to be the option that does nothing. A machine that installs PM and never
        // opens this setting must behave exactly as it did before the feature existed.
        for stored in [
            None,
            Some("server"),
            Some(""),
            Some("release-everything-now"),
        ] {
            assert_eq!(ReleasePolicy::from_setting(stored), ReleasePolicy::Server);
        }
        assert_eq!(
            ReleasePolicy::from_setting(Some("idle")),
            ReleasePolicy::Idle
        );
        assert_eq!(
            ReleasePolicy::from_setting(Some("on-exit")),
            ReleasePolicy::OnExit
        );
        // Round-trips, so a written value parses back to itself.
        for p in [
            ReleasePolicy::Server,
            ReleasePolicy::OnExit,
            ReleasePolicy::Idle,
        ] {
            assert_eq!(ReleasePolicy::from_setting(Some(p.as_setting())), p);
        }
    }

    #[test]
    fn a_model_pm_did_not_load_is_never_pms_to_free() {
        // Someone's own `ollama run` in a terminal is theirs. This is the first gate for a reason:
        // no policy, no timer and no quiet period may reach past it.
        let mut i = idle(9999);
        i.pm_loaded = false;
        assert!(!should_release(&i));
        assert!(!should_release_on_exit(ReleasePolicy::Idle, false));
        assert!(!should_release_on_exit(ReleasePolicy::OnExit, false));
    }

    #[test]
    fn work_in_flight_or_merely_scheduled_both_hold_the_model() {
        // `in_flight` is the obvious one. `holds` is the one that bites: `send_message` spawns its
        // follow-up jobs AFTER the reply returns, so the slot is genuinely quiet in between — and a
        // release taken in that gap makes all of them cold-load, three timeouts being three strikes
        // and a cooldown that takes chat down too.
        let mut busy = idle(9999);
        busy.in_flight = 1;
        assert!(!should_release(&busy));

        let mut held = idle(9999);
        held.holds = 1;
        assert!(!should_release(&held));
    }

    #[test]
    fn the_quiet_period_is_a_floor_not_a_target() {
        assert!(
            !should_release(&idle(299)),
            "one second short is still busy"
        );
        assert!(should_release(&idle(300)), "exactly the period is enough");
        assert!(should_release(&idle(301)));
    }

    #[test]
    fn only_the_idle_policy_releases_on_a_timer() {
        for policy in [ReleasePolicy::Server, ReleasePolicy::OnExit] {
            let mut i = idle(9999);
            i.policy = policy;
            assert!(
                !should_release(&i),
                "{policy:?} must not release on a timer"
            );
        }
    }

    #[test]
    fn exit_releases_for_both_the_policies_that_asked_for_it_and_neither_other_condition() {
        // The process is going away, so work in flight is going with it — holding gigabytes for a
        // process that no longer exists is the worse outcome. Ownership and policy still apply.
        assert!(should_release_on_exit(ReleasePolicy::OnExit, true));
        assert!(should_release_on_exit(ReleasePolicy::Idle, true));
        assert!(!should_release_on_exit(ReleasePolicy::Server, true));
    }

    #[test]
    fn a_stored_quiet_period_is_clamped_rather_than_ignored() {
        assert_eq!(idle_after(None), Duration::from_secs(300));
        assert_eq!(idle_after(Some("10")), Duration::from_secs(600));
        assert_eq!(idle_after(Some("  10  ")), Duration::from_secs(600));
        // Nonsense resolves to the default rather than to zero, which would release between two
        // turns of one conversation.
        assert_eq!(idle_after(Some("banana")), Duration::from_secs(300));
        assert_eq!(idle_after(Some("0")), Duration::from_secs(60));
        assert_eq!(idle_after(Some("99999")), Duration::from_secs(120 * 60));
    }

    #[test]
    fn b1_on_battery_the_server_policy_releases_after_the_battery_quiet_period() {
        assert!(
            !should_release(&on_battery(299, 600, 300)),
            "4m59 is still busy"
        );
        assert!(should_release(&on_battery(300, 600, 300)));
    }

    #[test]
    fn b2_the_battery_clock_starts_no_earlier_than_the_unplug() {
        // Half an hour quiet on mains, then carried to the sofa: the model was just used, as far as
        // the battery is concerned, so it is kept for the battery period from the unplug.
        assert!(!should_release(&on_battery(1800, 299, 300)));
        assert!(should_release(&on_battery(1800, 300, 300)));
    }

    #[test]
    fn b3_on_mains_the_battery_period_never_applies() {
        let mut i = on_battery(10_000, 0, 60);
        i.on_battery_for = None;
        assert!(!should_release(&i));
    }

    #[test]
    fn b4_with_the_battery_period_off_server_still_changes_nothing() {
        for b in [0, 60, 600, 100_000] {
            let mut i = on_battery(100_000, b, 60);
            i.battery_idle_after = None;
            assert!(!should_release(&i), "on battery for {b}s");
        }
    }

    #[test]
    fn b5_the_battery_period_sits_behind_every_gate() {
        let mut not_ours = on_battery(600, 600, 300);
        not_ours.pm_loaded = false;
        assert!(!should_release(&not_ours));
        let mut busy = on_battery(600, 600, 300);
        busy.in_flight = 1;
        assert!(!should_release(&busy));
        let mut held = on_battery(600, 600, 300);
        held.holds = 1;
        assert!(!should_release(&held));
    }

    #[test]
    fn b6_b7_the_shorter_clock_wins() {
        // Idle at 5 minutes, battery at 15: the policy fires first.
        let mut a = on_battery(300, 1200, 900);
        a.policy = ReleasePolicy::Idle;
        a.idle_after = Duration::from_secs(300);
        assert!(should_release(&a));
        // Idle at 15 minutes, battery at 5: the battery period fires first.
        let mut b = on_battery(300, 1200, 300);
        b.policy = ReleasePolicy::Idle;
        b.idle_after = Duration::from_secs(900);
        assert!(should_release(&b));
    }

    #[test]
    fn b8_on_exit_gets_the_battery_timer_too() {
        let mut i = on_battery(300, 600, 300);
        i.policy = ReleasePolicy::OnExit;
        assert!(should_release(&i));
        i.on_battery_for = None;
        assert!(
            !should_release(&i),
            "off battery, OnExit is still not a timer"
        );
    }

    #[test]
    fn b9_a_stored_battery_period_is_off_unless_it_says_otherwise() {
        for stored in [None, Some(""), Some("0"), Some("-3"), Some("abc")] {
            assert_eq!(battery_idle_after(stored), None, "{stored:?}");
        }
        assert_eq!(battery_idle_after(Some("1")), Some(Duration::from_secs(60)));
        assert_eq!(
            battery_idle_after(Some(" 5 ")),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            battery_idle_after(Some("500")),
            Some(Duration::from_secs(7200))
        );
        assert_eq!(clamp_battery_idle(0), 0);
        assert_eq!(clamp_battery_idle(500), 120);
    }

    #[test]
    fn b10_a_closed_store_releases_on_every_endpoint_pm_loaded_on() {
        assert_eq!(
            release_endpoints(Some(Some("http://a")), &[]),
            vec!["http://a".to_string()]
        );
        assert!(release_endpoints(Some(None), &[]).is_empty());
        assert!(release_endpoints(Some(Some("  ")), &[]).is_empty());
        let pairs = [
            ("http://b".to_string(), "m1".to_string()),
            ("http://a".to_string(), "m2".to_string()),
            ("http://b".to_string(), "m3".to_string()),
        ];
        assert_eq!(
            release_endpoints(None, &pairs),
            vec!["http://a".to_string(), "http://b".to_string()]
        );
    }

    #[test]
    fn a_token_only_ever_goes_to_the_endpoint_it_was_saved_for() {
        // The exit hook and a closed-store pass both visit every endpoint PM loaded on. One of them
        // may be a server the user has since moved away from, and the current token is not theirs.
        let tok = Some("secret");
        assert_eq!(token_for("http://a", Some("http://a"), tok), tok);
        assert_eq!(token_for("http://b", Some("http://a"), tok), None);
        // Not knowing which endpoint the token is for is never a reason to send it anywhere.
        assert_eq!(token_for("http://a", None, tok), None);
        assert_eq!(token_for("http://a", Some("http://a"), None), None);
    }
}
