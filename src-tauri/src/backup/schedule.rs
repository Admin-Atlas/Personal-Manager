// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automatic (scheduled) encrypted backups — the last layer of the backup feature. A background
//! scheduler (modelled on [`crate::chat_summary::spawn_summary_scheduler`]) runs a backup when one
//! is *due* per the user's cadence, fanning the one archive out to every **enabled + ready**
//! destination (Proton Drive and/or Google Drive), then trims each to the keep-last-N setting.
//!
//! Everything here is gated so it never surprises the user or fights foreground work:
//! it runs only when the vault is **unlocked**, the user is **idle**, no **sync** is active, no
//! backup/restore is already **busy**, a backup is **due**, the user has **opted in** by storing a
//! backup passphrase (unattended runs can't prompt — see [`crate::secrets::set_backup_passphrase`]),
//! and at least one destination is **enabled + ready** (Proton installed + connected, or a Google
//! account with the `drive.file` grant). Any gate failing just skips this pass; the next tick tries
//! again.
//!
//! The two schedule knobs live in the encrypted `settings` table (non-secret): the cadence and
//! the retention count. `last_backup_at` is stamped after each success so `backup_due` is honest
//! across restarts. The passphrase is the only secret, and only in the OS keychain.

use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use tauri::{AppHandle, Manager};

use crate::backup::destination::BackupDestination;
use crate::{db, secrets, AppState};

/// Cadence setting key (`off` | `daily` | `weekly` | `monthly`).
pub const BACKUP_FREQUENCY_KEY: &str = "backup_frequency";
/// Keep-last-N retention setting key (a positive integer, as text).
pub const BACKUP_RETENTION_KEY: &str = "backup_retention_n";
/// When the last successful (manual or scheduled) backup completed (RFC3339). Shared across
/// destinations — it records "the last time at least one destination succeeded" (the cadence clock).
pub const LAST_BACKUP_AT_KEY: &str = "last_backup_at";

/// The PER-DESTINATION last-success stamp key (F-22). The shared [`LAST_BACKUP_AT_KEY`] advances when
/// *any* destination succeeds, which masks a second destination that persistently fails — so each also
/// records its own last success under `last_backup_at:<kind>` (see [`BackupDestination::kind`]). Reading
/// these back lets Settings surface a destination that has gone silently stale.
pub fn last_backup_at_key(kind: &str) -> String {
    format!("{LAST_BACKUP_AT_KEY}:{kind}")
}

/// Default archives to keep if the user never set a number.
pub const DEFAULT_RETENTION_N: u32 = 5;

/// Per-destination enable flags for scheduled runs (`"true"`/`"false"`). Proton defaults ON (absent
/// ⇒ true) so existing scheduled behavior is unchanged on upgrade; Google Drive is opt-in (absent ⇒
/// false). `BACKUP_GDRIVE_ACCOUNT_KEY` holds the email of the Google account chosen for backup.
pub const BACKUP_PROTON_ENABLED_KEY: &str = "backup_proton_enabled";
pub const BACKUP_GDRIVE_ENABLED_KEY: &str = "backup_gdrive_enabled";
pub const BACKUP_GDRIVE_ACCOUNT_KEY: &str = "backup_gdrive_account";

/// Read a boolean setting: `"true"` → true, any other present value → false, absent (or a read
/// error) → `default`.
///
/// The second reader of the `"true"`/`"false"` encoding [`db::set_bool`] writes, and it is NOT
/// equivalent to that encoding's documented reader [`db::get_bool`]: on a present-but-non-canonical
/// value `get_bool` returns `default` while this returns false, and this swallows a DB read error
/// into `default` where `get_bool` propagates it. That matters most for
/// [`BACKUP_PROTON_ENABLED_KEY`], whose default is `true` — a corrupt value reads "Proton backups
/// off" here and "on" there. Unreachable today (every writer of these keys goes through
/// `db::set_bool`, so only a canonical literal can be stored), so the divergence is latent, and
/// collapsing the two readers is a semantics decision rather than a cleanup — deliberately left
/// alone here.
pub fn setting_bool(conn: &rusqlite::Connection, key: &str, default: bool) -> bool {
    match db::get_setting(conn, key) {
        Ok(Some(v)) => v == "true",
        _ => default,
    }
}

/// Disable the Google-Drive backup destination and forget its account (F-38). Called when the shared
/// Google client is torn down ([`crate::commands::clear_google_client`]): otherwise the schedule keeps
/// `gdrive_enabled` pointed at a now-tokenless account and every scheduled run fails on it. Additive to
/// the existing disconnect path; leaves Proton untouched.
pub fn clear_gdrive_destination(conn: &rusqlite::Connection) -> crate::error::Result<()> {
    db::set_bool(conn, BACKUP_GDRIVE_ENABLED_KEY, false)?;
    db::set_setting(conn, BACKUP_GDRIVE_ACCOUNT_KEY, "")?;
    Ok(())
}

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
/// mid-task. (`backup_busy` is also re-checked by the `BusyGuard` inside `run_backup`.)
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
        let last_at = db::get_setting_time(&conn, LAST_BACKUP_AT_KEY);
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

    // 3) Read the per-destination enable flags + the chosen Google account + any manual Proton CLI
    //    path under the lock.
    let (proton_enabled, gdrive_enabled, gdrive_account, proton_cli_override) = {
        let state = app.state::<crate::AppState>();
        let Ok(conn) = state.conn() else {
            return;
        };
        (
            setting_bool(&conn, BACKUP_PROTON_ENABLED_KEY, true),
            setting_bool(&conn, BACKUP_GDRIVE_ENABLED_KEY, false),
            db::get_setting(&conn, BACKUP_GDRIVE_ACCOUNT_KEY)
                .ok()
                .flatten()
                .filter(|s| !s.is_empty()),
            db::get_setting(&conn, crate::backup::proton::CLI_PATH_SETTING)
                .ok()
                .flatten()
                .map(std::path::PathBuf::from)
                .filter(|p| p.is_file()),
        )
    };

    // 4) Build the enabled + READY destination set. Ready = reachable without the expensive pack:
    //    Proton must be installed + a live session; Google must have a chosen account whose token
    //    carries the drive.file grant (a local keychain check — the upload fails cleanly and is
    //    simply not stamped if the grant was revoked). Skipping the pack when nothing is ready
    //    keeps an offline/unconfigured machine from packing a full archive just to fail.
    let mut targets: Vec<BackupDestination> = Vec::new();
    if proton_enabled {
        if let Some(cli) = crate::backup::proton::locate_proton_cli(proton_cli_override.as_deref())
        {
            let cli_probe = cli.clone();
            let connected = tokio::task::spawn_blocking(move || {
                crate::backup::proton::connection(&cli_probe).connected
            })
            .await
            .unwrap_or(false);
            if connected {
                targets.push(BackupDestination::Proton { cli });
            }
        }
    }
    if gdrive_enabled {
        if let Some(email) = gdrive_account {
            let token_key = crate::drive::account_token_key(&email);
            if crate::google::token_has_scope(&token_key, crate::google::DRIVE_FILE_SCOPE)
                .unwrap_or(false)
            {
                targets.push(BackupDestination::GoogleDrive { token_key });
            }
        }
    }
    if targets.is_empty() {
        return; // nothing ready this pass — try again next tick
    }

    // 5) Run the shared backup routine (snapshot + pack ONCE + fan out + per-destination retention +
    //    progress events). It stamps `last_backup_at` itself when at least one destination succeeds.
    if let Err(e) = crate::commands::run_backup(
        app,
        passphrase.expose().to_string(),
        targets,
        Some(retention_n),
    )
    .await
    {
        eprintln!("scheduled backup skipped: {e}");
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

    #[test]
    fn last_backup_at_key_is_per_destination_and_distinct_from_the_shared_stamp() {
        // F-22: each destination stamps its own success, so a persistently-failing sibling is not
        // masked by the shared cadence clock.
        assert_eq!(last_backup_at_key("proton"), "last_backup_at:proton");
        assert_eq!(last_backup_at_key("gdrive"), "last_backup_at:gdrive");
        assert_ne!(last_backup_at_key("gdrive"), LAST_BACKUP_AT_KEY);
    }

    #[test]
    fn clearing_the_gdrive_destination_disables_it_and_forgets_the_account() {
        // F-38: tearing down the shared Google client must also disable the Drive BACKUP destination,
        // or the schedule keeps firing at a now-tokenless account. Proton is left untouched.
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        db::set_setting(&conn, BACKUP_GDRIVE_ENABLED_KEY, "true").unwrap();
        db::set_setting(&conn, BACKUP_GDRIVE_ACCOUNT_KEY, "me@example.com").unwrap();
        db::set_setting(&conn, BACKUP_PROTON_ENABLED_KEY, "true").unwrap();

        clear_gdrive_destination(&conn).unwrap();

        assert!(
            !setting_bool(&conn, BACKUP_GDRIVE_ENABLED_KEY, false),
            "the Drive backup destination is disabled"
        );
        assert_eq!(
            db::get_setting(&conn, BACKUP_GDRIVE_ACCOUNT_KEY)
                .unwrap()
                .as_deref(),
            Some(""),
            "the tokenless account is forgotten"
        );
        assert!(
            setting_bool(&conn, BACKUP_PROTON_ENABLED_KEY, false),
            "Proton is untouched"
        );
    }
}
