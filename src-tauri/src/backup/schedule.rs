// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automatic (scheduled) encrypted backups to Proton Drive — the last layer of the backup
//! feature. A background scheduler (modelled on [`crate::chat_summary::spawn_summary_scheduler`])
//! runs a backup when one is *due* per the user's cadence, then trims old archives to the
//! keep-last-N retention setting.
//!
//! Everything here is gated so it never surprises the user or fights foreground work:
//! it runs only when the vault is **unlocked**, the user is **idle**, no **sync** is active, no
//! backup/restore is already **busy**, a backup is **due**, the Proton CLI is **installed +
//! connected**, and the user has **opted in** by storing a backup passphrase (unattended runs
//! can't prompt — see [`crate::secrets::set_backup_passphrase`]). Any gate failing just skips
//! this pass; the next tick tries again.
//!
//! The two schedule knobs live in the encrypted `settings` table (non-secret): the cadence and
//! the retention count. `last_backup_at` is stamped after each success so `backup_due` is honest
//! across restarts. The passphrase is the only secret, and only in the OS keychain.

use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use tauri::{AppHandle, Manager};

use crate::{db, secrets, AppState};

/// Cadence setting key (`off` | `daily` | `weekly` | `monthly`).
pub const BACKUP_FREQUENCY_KEY: &str = "backup_frequency";
/// Keep-last-N retention setting key (a positive integer, as text).
pub const BACKUP_RETENTION_KEY: &str = "backup_retention_n";
/// When the last successful (manual or scheduled) Proton backup completed (RFC3339).
pub const LAST_BACKUP_AT_KEY: &str = "last_backup_at";
/// Default archives to keep if the user never set a number.
pub const DEFAULT_RETENTION_N: u32 = 5;

/// Launch catch-up: wait up to `LAUNCH_WAIT_TICKS × LAUNCH_WAIT_SECS` (~5 min) for the vault to
/// unlock, then run one due-check.
const LAUNCH_WAIT_TICKS: u32 = 60;
const LAUNCH_WAIT_SECS: u64 = 5;
/// Idle backstop: check every `TICK_SECS`, and only back up once the user has been idle past
/// `IDLE_THRESHOLD_SECS` — packing + uploading shouldn't compete with active use.
const TICK_SECS: u64 = 300;
const IDLE_THRESHOLD_SECS: u64 = 300;

/// How often the vault is backed up automatically. Serialized to/from the `off|daily|weekly|
/// monthly` setting; an unknown value degrades to `Off` (safe default — no automation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frequency {
    Off,
    Daily,
    Weekly,
    Monthly,
}

impl Frequency {
    pub fn from_setting(s: &str) -> Self {
        match s {
            "daily" => Self::Daily,
            "weekly" => Self::Weekly,
            "monthly" => Self::Monthly,
            _ => Self::Off,
        }
    }

    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }

    /// The minimum time between backups, or `None` for `Off`. `Monthly` is a flat 30 days (a
    /// backup cadence doesn't need calendar-month precision).
    fn interval(self) -> Option<ChronoDuration> {
        match self {
            Self::Off => None,
            Self::Daily => Some(ChronoDuration::days(1)),
            Self::Weekly => Some(ChronoDuration::days(7)),
            Self::Monthly => Some(ChronoDuration::days(30)),
        }
    }
}

/// Whether a backup is due now: `Off` never; otherwise due if there's no prior backup, or the
/// cadence interval has elapsed since the last one. Pure, so the cadence logic is unit-tested
/// without a clock or a vault.
pub fn backup_due(last_at: Option<DateTime<Utc>>, freq: Frequency, now: DateTime<Utc>) -> bool {
    let Some(interval) = freq.interval() else {
        return false;
    };
    match last_at {
        None => true,
        Some(last) => now.signed_duration_since(last) >= interval,
    }
}

/// The vault session is open (so we can read the DB / snapshot). Cheap.
fn unlocked(app: &AppHandle) -> bool {
    app.state::<AppState>().conn().is_ok()
}

/// Whether all "run a backup now" gates are met: vault open, user idle past `threshold`, no sync
/// running, and nothing already backing up/restoring. Shared by BOTH the launch catch-up and the
/// idle loop, so an overdue backup never packs + uploads while a sync is active or the user is
/// mid-task. (`backup_busy` is also re-checked by the `BusyGuard` inside `run_proton_backup`.)
fn ready_to_backup(app: &AppHandle, threshold: Duration) -> bool {
    let state = app.state::<AppState>();
    // Bind `open` first: `state.conn()` returns a guard borrowing the block-local `state`.
    let open = state.conn().is_ok();
    open && state.idle_for() >= threshold
        && !state.sync_active()
        && !state.backup_busy.load(Ordering::SeqCst)
}

/// Launch catch-up + idle backstop scheduler. Spawned once from `setup`; mirrors the summary
/// scheduler's shape. Fully detached — a scheduled backup reuses the same detached progress
/// events as a manual one, so the Backup tab shows it if the user happens to be looking.
pub fn spawn_backup_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let threshold = Duration::from_secs(IDLE_THRESHOLD_SECS);

        // Launch catch-up: bounded wait for the vault to unlock, then a *gated* due-check — an
        // overdue backup fires soon after launch, but never while a sync runs or the user is
        // mid-task (the same gate the idle backstop uses, since a backup is expensive).
        for _ in 0..LAUNCH_WAIT_TICKS {
            if unlocked(&app) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(LAUNCH_WAIT_SECS)).await;
        }
        if ready_to_backup(&app, threshold) {
            maybe_run_scheduled_backup(&app).await;
        }

        // Idle backstop.
        loop {
            tokio::time::sleep(Duration::from_secs(TICK_SECS)).await;
            if ready_to_backup(&app, threshold) {
                maybe_run_scheduled_backup(&app).await;
            }
        }
    });
}

/// One scheduled-backup attempt. Reads the schedule config under the lock (then drops it before
/// any `.await`/subprocess — repo rule #4), bails on any unmet precondition, runs the shared
/// backup routine, and on success stamps `last_backup_at` and applies retention. Best-effort:
/// failures are logged, and because `last_backup_at` is only stamped on success, a failed pass
/// stays due and retries next tick (bounded by `TICK_SECS`, so no hammering).
async fn maybe_run_scheduled_backup(app: &AppHandle) {
    // 1) Read cadence + retention + last-run under the DB lock, then release it.
    let (freq, retention_n, last_at) = {
        let state = app.state::<AppState>();
        let Ok(conn) = state.conn() else {
            return; // vault not open
        };
        let freq = Frequency::from_setting(
            &db::get_setting(&conn, BACKUP_FREQUENCY_KEY)
                .ok()
                .flatten()
                .unwrap_or_default(),
        );
        let retention_n = db::get_setting(&conn, BACKUP_RETENTION_KEY)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(DEFAULT_RETENTION_N);
        let last_at = db::get_setting(&conn, LAST_BACKUP_AT_KEY)
            .ok()
            .flatten()
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc));
        (freq, retention_n, last_at)
    };

    // 2) Preconditions (cheap, no network).
    if freq == Frequency::Off {
        return;
    }
    if !backup_due(last_at, freq, Utc::now()) {
        return;
    }
    let Some(passphrase) = secrets::get_backup_passphrase().ok().flatten() else {
        return; // not opted in — unattended backup can't prompt
    };
    let Some(cli) = crate::backup::proton::locate_proton_cli() else {
        return; // CLI uninstalled
    };

    // 3) Confirm the session is live BEFORE the expensive pack, so an offline machine doesn't
    //    pack a full archive just to fail at upload.
    let cli_probe = cli.clone();
    let connected = tokio::task::spawn_blocking(move || {
        crate::backup::proton::connection(&cli_probe).connected
    })
    .await
    .unwrap_or(false);
    if !connected {
        return;
    }

    // 4) Run the shared backup routine (snapshot + pack + upload + progress events). It stamps
    //    `last_backup_at` itself on success and returns this vault's id (for scoped retention).
    let vault_id =
        match crate::commands::run_proton_backup(app, passphrase.expose().to_string(), cli.clone())
            .await
        {
            Ok(id) => id,
            Err(e) => {
                eprintln!("scheduled backup skipped: {e}");
                return;
            }
        };

    // 5) Trim old archives to keep-last-N (best-effort) — scoped to THIS vault's prefix, so it
    //    never touches another device/vault backing up to the same Proton folder.
    let prefix = crate::backup::proton::archive_prefix(&vault_id);
    let cli_ret = cli;
    match tokio::task::spawn_blocking(move || {
        crate::backup::proton::apply_retention(&cli_ret, retention_n as usize, &prefix)
    })
    .await
    {
        Ok(Ok(trashed)) if trashed > 0 => {
            eprintln!("scheduled backup: trimmed {trashed} old archive(s) to Proton Trash");
        }
        Ok(Err(e)) => eprintln!("scheduled backup: retention failed: {e}"),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 2, 12, 0, 0).unwrap()
    }

    #[test]
    fn frequency_setting_round_trips_and_defaults_to_off() {
        for f in [
            Frequency::Off,
            Frequency::Daily,
            Frequency::Weekly,
            Frequency::Monthly,
        ] {
            assert_eq!(Frequency::from_setting(f.as_setting()), f);
        }
        assert_eq!(Frequency::from_setting("garbage"), Frequency::Off);
        assert_eq!(Frequency::from_setting(""), Frequency::Off);
    }

    #[test]
    fn off_is_never_due() {
        assert!(!backup_due(None, Frequency::Off, t()));
        assert!(!backup_due(
            Some(t() - ChronoDuration::days(365)),
            Frequency::Off,
            t()
        ));
    }

    #[test]
    fn no_prior_backup_is_always_due() {
        assert!(backup_due(None, Frequency::Daily, t()));
        assert!(backup_due(None, Frequency::Weekly, t()));
        assert!(backup_due(None, Frequency::Monthly, t()));
    }

    #[test]
    fn due_only_after_the_interval_elapses() {
        let now = t();
        // Daily: not at 23h, yes at 25h.
        assert!(!backup_due(
            Some(now - ChronoDuration::hours(23)),
            Frequency::Daily,
            now
        ));
        assert!(backup_due(
            Some(now - ChronoDuration::hours(25)),
            Frequency::Daily,
            now
        ));
        // Weekly: not at 6d, yes at 8d.
        assert!(!backup_due(
            Some(now - ChronoDuration::days(6)),
            Frequency::Weekly,
            now
        ));
        assert!(backup_due(
            Some(now - ChronoDuration::days(8)),
            Frequency::Weekly,
            now
        ));
        // Monthly (30d): not at 29d, yes at 31d.
        assert!(!backup_due(
            Some(now - ChronoDuration::days(29)),
            Frequency::Monthly,
            now
        ));
        assert!(backup_due(
            Some(now - ChronoDuration::days(31)),
            Frequency::Monthly,
            now
        ));
    }
}
