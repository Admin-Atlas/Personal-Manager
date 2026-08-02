// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

mod applock;
mod backup;
// "A model that fits your machine better is available" (#437): the pure, conservative decision about
// whether any curated model is worth interrupting the user about, given what they already run.
mod better_fit;
// One home for "run this blocking, fallible closure off the async runtime", so ~40 call sites stop
// hand-rolling the JoinError→Error conversion. It takes a label because the copies were the only
// thing keeping their ~30 distinct panic messages distinct.
mod blocking;
mod briefing;
mod calendar;
mod chat;
mod chat_index;
mod chat_prefs;
mod chat_summary;
mod chat_title;
mod clock;
// The Google Drive + OneDrive sync engines, lifted out of `commands` so the IPC layer keeps only the
// thin command wrappers. Two engines still (unifying them behind one driver is the next step); the
// shared single-flight lifecycle lives in `connector_sync`.
mod cloud_sync;
mod commands;
mod commands_dev;
mod components;
mod connector_sync;
mod context_budget;
mod cost;
mod db;
mod drive;
mod duplicates;
mod entities;
mod error;
// Pure model-fit scoring (#296): sizes a model against a machine's memory and picks the best
// (quant, context) that fits. No I/O — a pure projection of hardware + catalog inputs.
mod fit;
// The structured flag layer (board card 9): detection (a pure reducer over milestones + calendar)
// populates first-class flag records, the briefing renders the active set, and a backstop
// scheduler keeps them current. Assertion/resolution and chat grounding arrive in the following PRs.
mod flags;
mod fts_segment;
mod google;
// Best-effort hardware scan (#296): RAM/CPU/disk via sysinfo, GPU/VRAM hand-rolled per-OS. Every
// probe nulls its field on failure rather than erroring.
mod hardware;
mod ics;
mod index_only;
mod ingest;
mod layout;
mod llm_gateway;
mod local_ai;
// The curated local-model catalog (#296): in-repo GGUF table (include_str!), bridged into fit-scoring
// and the endpoint window ladder's catalog rung.
mod local_catalog;
// On-disk installed-model crawl (#449): find GGUF models a runner has downloaded but isn't serving,
// so the Workbench can size them without you loading them first. Read-only, metadata only.
mod local_disk;
mod local_slot;
// Local-folder indexing (board card 6): a third index-only source on the shared foundation, reading
// from the filesystem. This first PR reconciles a tracked folder on demand (a filtered walk +
// mtime→hash diff); the live `notify` watcher is the next card.
mod localfolder;
mod lock_session;
mod microsoft;
mod milestones;
mod model_gateway;
mod oauth_loopback;
mod onedrive;
mod openai_compat;
mod openrouter;
mod outlook_calendar;
mod pathguard;
mod paths;
mod photos;
mod preferences;
mod project_activity;
mod projects;
mod python_fetch;
mod registry;
mod retag;
mod retrieval;
mod retrieval_config;
mod retrieval_diag;
mod retrieval_feedback;
mod review;
mod secret;
mod secrets;
mod settings;
mod sidecar;
// The confinement's allow-set (which directories the worker may read / execute / write) is pure and
// platform-agnostic, so it is unit-tested everywhere — including the Windows dev box, where nothing
// else in `sidecar_sandbox_linux` compiles. Only the Linux sandbox consumes it, so it's dead code
// elsewhere; same arrangement as `sidecar_seccomp` below.
mod sidecar_allowset;
#[cfg(windows)]
mod sidecar_sandbox;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod sidecar_sandbox_linux;
#[cfg(target_os = "macos")]
mod sidecar_sandbox_macos;
// The seccomp filter builder is pure and platform-agnostic (its cBPF-interpreter tests run on every
// platform); only the Linux sandbox installs what it produces, so it's dead code off Linux.
mod sidecar_seccomp;
mod sidecar_stage;
mod smart_app_control;
mod splitter;
mod spreadsheets;
/// ONE-TIME cleanup, removed in the release after this one — see the module docs and card #651.
mod sweep;
mod tags;
mod tray;
mod update_delivery;
mod vault;
mod wipe;

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use rusqlite::Connection;
use tauri::Manager;

use sidecar::{SidecarManager, SidecarPaths};

/// The active vault's runtime context, set whenever the store opens (boot, unlock,
/// open-existing, make-shareable). Holds where the Markdown lives and the policy-aware
/// cipher used to read/write it, so ingest never has to re-resolve paths or re-derive
/// keys. `None` exactly when `db` is `None` (the vault is locked).
pub struct VaultRuntime {
    /// The Markdown vault folder (source of truth) for the active vault.
    pub markdown_dir: PathBuf,
    /// The vault root (one level up from `markdown_dir`), where `pm.sqlite` and the encrypted
    /// entity-rules file live.
    pub vault_root: PathBuf,
    /// Policy-aware reader/writer for this vault's Markdown files.
    pub cipher: vault::MarkdownCipher,
    /// Always-on cipher for the encrypted entity-rules file (the canonical-entity source of
    /// truth). Distinct from `cipher`: it encrypts even for device vaults whose Markdown is
    /// plaintext, since an alias map of projects is more revealing than any one document.
    pub rules_cipher: entities::RulesCipher,
    /// Always-on cipher for the encrypted index-only manifest (the portable classification of
    /// sources we index but don't fully import). Same primitive as `rules_cipher`, a distinct AAD
    /// stem apart; sits next to the rules file at the vault root.
    pub manifest_cipher: index_only::ManifestCipher,
}

impl VaultRuntime {
    /// Build the session runtime from the resolved layout, vault metadata, and the resolved
    /// 32-byte master: the policy-aware Markdown cipher and the always-on rules cipher are both
    /// subkeys of the same master, so every open path produces an identical runtime.
    pub fn build(
        resolved: &vault::ResolvedVault,
        meta: &vault::VaultMeta,
        master: &[u8; 32],
    ) -> Self {
        Self {
            markdown_dir: resolved.markdown_dir.clone(),
            vault_root: resolved.vault_root.clone(),
            cipher: vault::MarkdownCipher::from_meta(meta, master),
            rules_cipher: entities::RulesCipher::from_master(&meta.vault_id, master),
            manifest_cipher: index_only::ManifestCipher::from_master(&meta.vault_id, master),
        }
    }
}

/// Wall-clock milliseconds since the Unix epoch — what the detached-job snapshots below stamp
/// themselves with so a progress bar can show elapsed time from the TRUE start rather than from
/// whenever its component last mounted.
///
/// Deliberately `SystemTime`, not `ingest::iso_now(&AppState)`: the stamps are taken while a sync
/// slot's mutex is held, and `iso_now` reaches for the DB guard — an easy way to hit the
/// non-reentrant-mutex freeze. Deliberately millis-since-epoch, not an ISO string: the frontend
/// subtracts it from `Date.now()`, and a bare number needs no parsing or timezone handling.
///
/// Wall clock is skew-prone (an NTP step or a laptop sleep can move it), which a monotonic `Instant`
/// would avoid — but an `Instant` can't be serialised into something the webview can subtract from.
/// `formatElapsed` floors negatives at 0:00, so skew degrades to a stalled timer, never garbage.
pub fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Shared app state. The SQLite connection is guarded by a mutex; commands lock
/// it only for short synchronous work, never across an `.await`. The sidecar
/// manages its own interior locking.
/// A snapshot of a cloud sync (Google Drive or OneDrive) that's currently running (if any), shared so
/// the Settings UI can reflect an in-flight sync no matter which view is mounted. The sync runs
/// detached from the component that started it — leaving Settings (or starting it, then navigating
/// away) doesn't stop it; the UI re-reads this on return and follows the `drive://sync` /
/// `onedrive://sync` events live. One struct for both providers: `AppState` keeps a separate
/// `drive_sync` / `onedrive_sync` field so the two connectors run and report independently, but the
/// snapshot shape is identical (audit X-D1).
#[derive(Default, Clone, serde::Serialize)]
pub struct CloudSyncState {
    pub running: bool,
    pub processed: usize,
    pub total: Option<usize>,
    /// When this sync run actually began ([`epoch_ms`]), so a bar that mounts mid-run shows elapsed
    /// time from the true start instead of restarting at 0:00. `None` when idle. Set once in
    /// `begin_pass` and deliberately KEPT across `reset_for_rerun`: the follow-up sweep is the same
    /// run as far as the user is concerned, so the timer should keep counting, not restart.
    pub started_at_ms: Option<i64>,
    /// The account being synced (email), or `None` for an all-accounts pass.
    pub account: Option<String>,
    /// Internal single-flight queue: the sweeps this run still owes because a request arrived while a
    /// pass was running (the user pressed "Sync now" for another account mid-index, or connected
    /// one). Each queued account gets its own follow-up pass, so `account` above moves to it in turn
    /// and its row reads "Queued" and then "Syncing…"; a request that named no account collapses them
    /// into one all-accounts sweep. Not exposed to the UI.
    #[serde(skip)]
    pub queue: connector_sync::SyncQueue,
    /// The most recent finished sync's report (counts + the not-indexed list), so a user returning to
    /// Settings after a sync has completed still sees the result. Cleared when a new sync starts.
    pub last_report: Option<cloud_sync::CloudSyncReport>,
}

/// A snapshot of the local-folder sync that's currently running (if any) — the filesystem sibling of
/// [`CloudSyncState`]. Its own field/channel so the connectors run and report independently.
#[derive(Default, Clone, serde::Serialize)]
pub struct LocalFolderSyncState {
    pub running: bool,
    pub processed: usize,
    pub total: Option<usize>,
    /// When this sync run began ([`epoch_ms`]) — see [`CloudSyncState::started_at_ms`].
    pub started_at_ms: Option<i64>,
    /// The folder key being synced, or `None` for an all-folders pass.
    pub folder: Option<String>,
    /// Internal single-flight queue (the sweeps still owed to requests that arrived while a pass was
    /// running) — see [`CloudSyncState::queue`]. Not exposed to the UI.
    #[serde(skip)]
    pub queue: connector_sync::SyncQueue,
    /// The most recent finished sync's report, so a user returning after a sync still sees the result.
    pub last_report: Option<localfolder::LocalSyncReport>,
}

/// A snapshot of the index rebuild that's currently running (if any) — the ingest sibling of
/// [`CloudSyncState`], and for the same reason: the rebuild runs detached from the component that
/// started it, so leaving the Documents tab (or starting it, then navigating away) doesn't stop it.
/// Before this existed the backend kept working while the UI, having lost its per-call `Channel` on
/// unmount, showed the interface of an idle machine — the user reasonably concluded it had died.
/// The UI re-reads this on mount and follows the `ingest://progress` event live.
#[derive(Default, Clone, serde::Serialize)]
pub struct IngestJobState {
    pub running: bool,
    pub processed: usize,
    pub total: Option<usize>,
    /// When this rebuild began ([`epoch_ms`]) — see [`CloudSyncState::started_at_ms`]. A rebuild
    /// RESUMED after a relaunch is stamped afresh, so the timer reads "since resume" rather than
    /// since the original interrupted run; carrying the original would mean persisting a timestamp
    /// in the resume marker, and "since resume" is the honest reading of what is happening now.
    pub started_at_ms: Option<i64>,
    /// The latest `Preparing` message (engine install / model download), so a UI mounting mid-setup
    /// shows the same indeterminate label as one that watched it start. Cleared once counting begins.
    pub prep: Option<String>,
    /// The most recent finished rebuild's counts, so a user returning after it completed still sees
    /// the result — the live event only reaches a listener that was mounted. Cleared on a new run.
    pub last_report: Option<IngestReport>,
    /// The tail of the per-file Activity list, capped at [`RECENT_ITEMS_CAP`]. Without it a tab
    /// switch emptied the list outright and the card had nothing to render until the next file
    /// finished — seconds of apparent death mid-rebuild, while the elapsed timer (restored from
    /// `started_at_ms`) kept counting, which is what made the gap look like a bug rather than a load.
    /// Capped-and-flagged like [`crate::cloud_sync::CloudSyncReport::issues`]; the tail is what a
    /// returning user is looking for, so the front is dropped first.
    pub recent: Vec<IngestItem>,
    /// True once `recent` has dropped older rows, so the UI can say the list is partial.
    pub recent_truncated: bool,
}

/// How many per-file rows the snapshot keeps. Enough to look continuous on return, small enough that
/// a 10,000-file rebuild costs nothing to hold.
pub const RECENT_ITEMS_CAP: usize = 50;

/// One row of the rebuild's Activity list, mirrored into [`IngestJobState`].
#[derive(Clone, serde::Serialize)]
pub struct IngestItem {
    pub name: String,
    /// "working" | "done" | "skipped" | "failed" — the same vocabulary the UI's own rows use.
    pub status: String,
    /// The skip reason or error text; absent for working/done rows.
    pub detail: Option<String>,
}

/// The counts a finished rebuild reports, mirrored into [`IngestJobState`] so they survive the
/// unmount of whichever view started the run.
#[derive(Clone, serde::Serialize)]
pub struct IngestReport {
    pub ingested: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Entries the vault walk could not read (`IngestEvent::Finished::unreadable`). Carried here too,
    /// not just on the live event, because the returning-tab snapshot is the ONLY thing a user who
    /// switched away sees — and "the run was partial" is precisely the part they must not miss.
    pub unreadable: usize,
}

// The three detached-sync snapshots share their single-flight lifecycle through
// [`connector_sync::SyncRunGuard`]; each exposes its `running`/`rerun` fields (and how to reset its
// own counters + target) via [`connector_sync::SyncSlot`] so the guard can own that lifecycle
// generically. The guard clears `running` on drop — including on a panicked pass — so a sync that
// crashes can't wedge the connector with `running = true` for the session (audit F-43).
impl connector_sync::SyncSlot for CloudSyncState {
    fn running(&self) -> bool {
        self.running
    }
    fn set_running(&mut self, running: bool) {
        self.running = running;
    }
    fn queue(&mut self) -> &mut connector_sync::SyncQueue {
        &mut self.queue
    }
    fn reset_for_rerun(&mut self, target: Option<String>) {
        self.processed = 0;
        self.total = None;
        // The sweep's OWN account, not `None`: the follow-up now answers one queued request at a
        // time, so the row that showed "Queued" is the one that shows "Syncing…" next.
        self.account = target;
        // `started_at_ms` is deliberately NOT reset: the follow-up sweep is a continuation of the
        // same run the user started, so the elapsed timer keeps counting instead of restarting.
    }
    fn begin_pass(&mut self, target: Option<String>) {
        *self = CloudSyncState {
            running: true,
            started_at_ms: Some(epoch_ms()),
            account: target,
            ..Default::default()
        };
    }
}

impl connector_sync::SyncSlot for LocalFolderSyncState {
    fn running(&self) -> bool {
        self.running
    }
    fn set_running(&mut self, running: bool) {
        self.running = running;
    }
    fn queue(&mut self) -> &mut connector_sync::SyncQueue {
        &mut self.queue
    }
    fn reset_for_rerun(&mut self, target: Option<String>) {
        self.processed = 0;
        self.total = None;
        self.folder = target;
        // `started_at_ms` deliberately kept — see the cloud slot above.
    }
    fn begin_pass(&mut self, target: Option<String>) {
        *self = LocalFolderSyncState {
            running: true,
            started_at_ms: Some(epoch_ms()),
            folder: target,
            ..Default::default()
        };
    }
}

/// A snapshot of the currently-running backup or restore (if any), shared so the Backup
/// settings UI can reflect progress no matter which view is mounted — the same detached
/// model as the Drive sync. Empty / `running:false` when nothing is in flight.
#[derive(Default, Clone, serde::Serialize)]
pub struct BackupState {
    pub running: bool,
    /// When this backup/restore began ([`epoch_ms`]) — see [`CloudSyncState::started_at_ms`]. Stamped
    /// EDGE-TRIGGERED on idle→running: every phase transition emits a `Phase` event, so stamping it
    /// unconditionally there would restart the timer at each phase — worse than the bug it fixes.
    pub started_at_ms: Option<i64>,
    /// The current phase (snapshot/pack/upload/download/restore/validate), or `None` when idle.
    pub phase: Option<backup::BackupPhase>,
    /// Monotonic `0.0..=1.0` progress within the current phase.
    pub fraction: f32,
    /// The most recent finished op's report, so a user returning to Settings still sees the outcome.
    pub last_report: Option<backup::BackupReport>,
    /// The most recent failure message (cleared when a new op starts).
    pub last_error: Option<String>,
    /// The still-switchable restored vault, if any — the display companion to
    /// `pending_restore_keys` (which holds the actual key). Set when a restore stages a vault,
    /// cleared when the user switches to it. Carries NO key, only the summary the UI already got
    /// back from the restore command, so the Backup panel can re-offer "switch to it" after being
    /// closed and reopened. In-memory like the key map: it survives a UI remount but not an app
    /// restart (matching that a restore not switched-to in a session is simply re-done).
    pub pending_restore: Option<commands::RestoreSummary>,
}

pub struct AppState {
    /// The open store, or `None` when the vault is locked — a passphrase/shareable
    /// vault on a profile that hasn't unlocked it yet. Reach it via [`AppState::conn`],
    /// never by locking the mutex directly, so the locked case is handled uniformly.
    pub db: Mutex<Option<Connection>>,
    /// The active vault's Markdown dir + cipher, kept in lockstep with `db`: set
    /// together when the store opens, both `None` while locked. Reach it via
    /// [`AppState::markdown_io`].
    pub vault: Mutex<Option<VaultRuntime>>,
    pub sidecar: SidecarManager,
    /// Whether the optional app-lock has been satisfied this process. Starts false;
    /// `unlock_app` sets it on a successful OS verification. A soft UI gate only — the
    /// store is already decrypted (see `applock`). Backend-owned so the launch decision
    /// can't be flipped from the webview.
    pub app_unlocked: AtomicBool,
    /// This process's random id, written into the vault lockfile so a shared vault's
    /// other instance can tell our heartbeat from its own (see `lock_session`).
    pub instance_id: String,
    /// Cooperative single-writer lock state for the engaged shared vault, if any.
    pub lock_session: Mutex<lock_session::LockSession>,
    /// Snapshot of the currently-running Drive sync (so the UI can resume showing progress after the
    /// user navigates away and back). Empty/`running:false` when no sync is in flight. Listed in
    /// [`AppState::connector_slots`].
    pub drive_sync: Mutex<CloudSyncState>,
    /// Cooperative stop flag for the running Drive sync. `stop_drive_sync` sets it; the sync loop
    /// checks it between files and halts, keeping everything indexed so far. Reset at each sync start.
    pub drive_sync_cancel: AtomicBool,
    /// Snapshot of the currently-running OneDrive sync (the Microsoft sibling of `drive_sync`). Listed
    /// in [`AppState::connector_slots`].
    pub onedrive_sync: Mutex<CloudSyncState>,
    /// Cooperative stop flag for the running OneDrive sync (the sibling of `drive_sync_cancel`).
    pub onedrive_sync_cancel: AtomicBool,
    /// Snapshot of the currently-running local-folder sync (the filesystem sibling of `drive_sync`).
    /// Listed in [`AppState::connector_slots`].
    pub local_sync: Mutex<LocalFolderSyncState>,
    /// Cooperative stop flag for the running local-folder sync (the sibling of `drive_sync_cancel`).
    pub local_sync_cancel: AtomicBool,
    /// Snapshot of the currently-running index rebuild, so the Documents tab (and the Settings
    /// rebuild modal) can resume showing progress after the user navigates away and back — the
    /// ingest sibling of `drive_sync`.
    pub ingest_job: Mutex<IngestJobState>,
    /// Single-flight guard for the rebuild, and the flag every other indexing writer defers to while one
    /// runs (see [`AppState::rebuild_running`]). Two overlapping rebuilds are not merely wasteful — they
    /// fight over the same rows, and on the vector-width arm (which still clears the store) the second's
    /// `DELETE FROM documents` destroys the first's in-progress work. The UI's own guard is
    /// component-local and resets on remount, which made that reachable by simply switching tabs and
    /// clicking again.
    pub ingest_busy: AtomicBool,
    /// Snapshot of the semantic-map layout precompute (the reducer that ran + the last error), so the
    /// Map can show what produced the cached coordinates and surface a failure. The single-flight
    /// claim itself is NOT here — it is [`AppState::layout_busy`], so a panicked or dropped pass
    /// releases it. See `layout`.
    pub layout_job: Mutex<layout::LayoutJobState>,
    /// When the user was last active (a chat send / an ingest). The idle chat-indexer (`chat_index`)
    /// reads this so it only runs during a lull and never competes with active use.
    pub last_user_activity: Mutex<Instant>,
    /// Single-flight guard shared by the chat-index launch sweep and idle loop, so the two never overlap.
    pub chat_index_busy: AtomicBool,
    /// Single-flight guard shared by the rolling-summary eager nudge and the launch/idle scheduler
    /// (`chat_summary`), so a per-conversation extend and a full reconcile never overlap.
    pub summary_busy: AtomicBool,
    /// Single-flight guard shared by the chat-title eager nudge and the launch catch-up (`chat_title`),
    /// so a per-conversation title generation and a full reconcile never overlap.
    pub title_busy: AtomicBool,
    /// Single-flight guard shared by the chat-preference eager nudge and the launch catch-up
    /// (`chat_prefs`), so a per-conversation extraction and a full reconcile never overlap.
    pub prefs_busy: AtomicBool,
    /// Single-flight guard for the semantic-layout precompute, shared by the idle launch pass, the
    /// Map-open prioritise, and the two component-change recomputes (t-SNE installed / a storage
    /// component removed), so a second request folds into the running one instead of stacking a
    /// second reducer run. It is also what `semantic_layout` reports as `computing`.
    ///
    /// An `AtomicBool` reached through [`BusyGuard`] rather than a `running` field on
    /// [`layout::LayoutJobState`]: the flag has to survive a panicking or dropped pass (F-43), and a
    /// bool behind a `Mutex` additionally wedges on poison — reading idle forever while refusing
    /// forever.
    pub layout_busy: AtomicBool,
    /// Snapshot of the currently-running encrypted backup / restore, so the Backup settings UI can
    /// resume showing progress after the user navigates away and back (the sibling of `drive_sync`).
    pub backup_state: Mutex<BackupState>,
    /// Cooperative stop flag for the running backup/restore. `stop_backup` sets it; the pack/restore
    /// loop checks it between reads and aborts. Reset at the start of each op.
    pub backup_cancel: AtomicBool,
    /// Single-flight guard so a manual backup, a manual restore, and a scheduled backup can never
    /// overlap and contend for the DB snapshot / archive file.
    ///
    /// It carries a DATA-SAFETY property as well as that UX one: `wipe::wipe_pm_data` takes this
    /// guard for the whole erase, so while it is set no backup or restore can start, and an erase
    /// requested while one IS in flight is refused outright. Without that, an upload finishing after
    /// "Remove PM data" reported success would leave a fully decryptable `.pmbackup` of the erased
    /// vault on a remote (it embeds the DB key, so it restores anywhere with the passphrase the user
    /// still knows), and a restore finishing after it would strand one in `restored-vaults/`.
    /// Anything that widens the set of doors taking this guard must keep that sentence true.
    ///
    /// Reach it through [`BusyGuard`], never a bare `load` — a check-then-act read loses the race by
    /// construction, because the scheduler does a DB read and an async Proton probe between
    /// `ready_to_backup` and the acquire inside `run_backup`. Note also that `BackupState.running`
    /// (the display snapshot) is NOT this: `emit_backup_progress` clears it on `Finished`/`Failed`
    /// while the guard is still held and the run is still winding down, so it is a UI hint only.
    pub backup_busy: AtomicBool,
    /// Keys recovered by a restore, held in memory (never the keychain) keyed by the restored
    /// folder, until the user explicitly switches to that vault. Deferring the keychain write to the
    /// switch keeps a restore-you-never-switch-to from clobbering the LIVE vault's cached key. Dropped
    /// (and zeroized) on exit; a restore not switched-to in this session is simply re-done.
    pub pending_restore_keys: Mutex<std::collections::HashMap<String, secret::Secret>>,
    /// Why the store is unavailable beyond "locked, needs the passphrase" — a classified
    /// [`error::VaultFault`]: a boot-time open failure (transient AV/search-indexer file
    /// lock, disk I/O), an unreachable pointed root (access denied / folder gone), or a
    /// mid-session loss detected by the lock watcher. `None` for the normal cases (open,
    /// or merely `needs_unlock`). Set in `setup`/`engage`/the watcher; cleared by
    /// [`AppState::open_session`] on every successful open path, and re-armed by
    /// `retry_open_vault` on a failed retry. Drives `vault_status.fault` and the honest
    /// [`session_unavailable_message`] — "access denied" must never masquerade as
    /// "the vault is locked" (the ACL-lockout incident).
    pub vault_fault: Mutex<Option<error::VaultFault>>,
    /// The standing "this vault's settings file was altered outside PM" notice, if any — the
    /// user-facing half of [`vault::MetaAuthReport`]. Held rather than emitted because boot has no
    /// listener yet: `unlock_vault` and `adopt_shared_vault` can fire `vault://meta-warning` at a
    /// webview that is already up, but a vault opened from a cached key comes up before the frontend
    /// subscribes, so the boot report had nowhere to go but stderr. Surfaced through
    /// `vault_status.meta_warning`, which the app reads on load, and it STAYS set: a tampered meta is
    /// no longer re-signed on open, so the condition is real until the file is legitimately rewritten.
    pub meta_warning: Mutex<Option<String>>,
    /// Single-flight guard for the daily-briefing regeneration. A `tokio::Mutex` rather than the
    /// [`BusyGuard`] pattern above because the wanted semantics differ: a second caller must WAIT
    /// for the running generation and take its result, not give up. The briefing is shown in up to
    /// three places at once (Focus card, sidebar panel, always-on-top window) plus three background
    /// triggers, and each webview has its own frontend guard that cannot see the others — so
    /// "collapse concurrent refreshes" has to be enforced here. Without it, two overlapping model
    /// calls race on the stored trio and can pair an OLDER body with a NEWER timestamp.
    pub briefing_refresh: tokio::sync::Mutex<()>,
    /// Set when something that feeds the briefing changes (a calendar sync landing events, a
    /// milestone edited, a flag resolved). The briefing scheduler consumes it on its next tick, so
    /// a burst of edits coalesces into ONE check — and that check still only spends a model call
    /// if the facts actually differ. See [`briefing::nudge`].
    pub briefing_dirty: AtomicBool,
    /// Local-endpoint runtime (#297): the single-inference slot (chat preempts background), the
    /// dead-host circuit breaker, and the per-endpoint context-window cache. In-memory — a restart
    /// re-probes, since the loaded model / window can change across relaunches.
    pub local_ai: local_slot::LocalRuntime,
}

/// The sentence a command gets when it needs the store but the session is closed. Pure
/// so it unit-tests: `None` keeps today's "the vault is locked" (a passphrase vault
/// awaiting unlock); a Denied fault names the real problem and the way out; anything
/// else surfaces its own message with a pointer to the Vault settings.
pub(crate) fn session_unavailable_message(fault: Option<&error::VaultFault>) -> String {
    match fault {
        None => "the vault is locked".to_string(),
        Some(f) if f.code == error::VaultFaultCode::Denied => format!(
            "PM lost access to the vault folder — {} — use Repair access in Settings → Vault",
            f.message
        ),
        Some(f) => format!("{} — check Settings → Vault", f.message),
    }
}

/// Single-flight guard with RAII release. [`BusyGuard::acquire`] returns `Some` if it flipped the flag
/// `false → true` (this caller won the single-flight), or `None` if another pass already holds it. The flag
/// resets to `false` when the guard drops — crucially including an unwinding panic, unlike a trailing
/// `store(false)`. A background task (title / summary / prefs / index sweep / layout precompute) that panics
/// mid-op therefore can't leave its subsystem's flag stuck `true`, which would otherwise silently wedge that
/// feature (its eager nudge AND its scheduler both short-circuit on the flag) until the app restarts.
pub(crate) struct BusyGuard<'a>(&'a AtomicBool);

impl<'a> BusyGuard<'a> {
    pub(crate) fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        use std::sync::atomic::Ordering::SeqCst;
        flag.compare_exchange(false, true, SeqCst, SeqCst)
            .ok()
            .map(|_| BusyGuard(flag))
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl AppState {
    /// Mark the user as active right now. Bumped on a chat send, on ingest, and — via the
    /// `mark_activity` command (F-08) — on any real webview interaction (reading, scrolling,
    /// triaging, editing), so every idle-gated background job backs off during active use, not only
    /// during chat and imports.
    pub fn mark_user_activity(&self) {
        if let Ok(mut t) = self.last_user_activity.lock() {
            *t = Instant::now();
        }
    }

    /// How long since the last marked user activity (poisoned lock → treat as just-active).
    pub fn idle_for(&self) -> std::time::Duration {
        self.last_user_activity
            .lock()
            .map(|t| t.elapsed())
            .unwrap_or_default()
    }

    /// Whether an index rebuild is running right now. Every other writer of `documents`/`chunks` checks
    /// this and stands down (#371): a rebuild re-reads the whole vault and then sweeps away the documents
    /// it never saw, so a concurrent writer racing that pass could have its brand-new document swept, or —
    /// on the vector-width arm, the one that still clears the store first — written straight into the
    /// wiped window and lost.
    ///
    /// The sweep does not RELY on this (it re-checks each candidate's file on disk before deleting, so a
    /// racing writer's document is kept regardless). This is the cheaper, earlier guard: don't start the
    /// contending work at all.
    pub fn rebuild_running(&self) -> bool {
        self.ingest_busy.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Every detached connector-sync slot, in one place. **A new connector MUST be added here.**
    ///
    /// This register is the stand-down gate seven background jobs share through
    /// [`AppState::sync_active`]: the backup schedule, the chat index, the chat rolling summary, the
    /// briefing, flag detection, the activity rollup, and the orphan sweep's delete refusal. Omitting a
    /// slot fails silently — all seven simply read "nothing is syncing" — which is what shipped for the
    /// local-folder slot (audit B1-7).
    pub fn connector_slots(&self) -> [&dyn connector_sync::SlotStatus; 3] {
        [&self.drive_sync, &self.onedrive_sync, &self.local_sync]
    }

    /// Whether ANY connector sync (Drive, OneDrive, local folder) is running right now — background
    /// jobs defer to all of them so they neither contend for the engine nor act on a half-synced index.
    pub fn sync_active(&self) -> bool {
        connector_sync::any_running(&self.connector_slots())
    }
}

/// A borrow of the open connection. Derefs to [`Connection`], so call sites read just
/// like the old `state.db.lock().unwrap()` did — only the acquisition line changes to
/// `state.conn()?`. Holding it keeps the store locked, exactly as before.
pub struct DbGuard<'a>(MutexGuard<'a, Option<Connection>>);

impl Deref for DbGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.0
            .as_ref()
            .expect("DbGuard is only constructed when the connection is present")
    }
}

impl DerefMut for DbGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.0
            .as_mut()
            .expect("DbGuard is only constructed when the connection is present")
    }
}

impl AppState {
    /// Borrow the open store, or a friendly error if the vault is locked (not unlocked
    /// this session) or the lock was poisoned. The single way commands reach the DB.
    pub fn conn(&self) -> error::Result<DbGuard<'_>> {
        let guard = self
            .db
            .lock()
            .map_err(|_| error::Error::Other("database lock poisoned".into()))?;
        if guard.is_none() {
            return Err(error::Error::Other(session_unavailable_message(
                self.vault_fault().as_ref(),
            )));
        }
        Ok(DbGuard(guard))
    }

    /// The current vault fault, if any (poison-tolerant — a poisoned slot reads as none).
    pub fn vault_fault(&self) -> Option<error::VaultFault> {
        self.vault_fault.lock().ok().and_then(|g| g.clone())
    }

    /// Record (or clear) why the store is unavailable. Poison-tolerant: fault state is
    /// advisory for the UI, never worth failing a command over.
    pub fn set_vault_fault(&self, fault: Option<error::VaultFault>) {
        if let Ok(mut guard) = self.vault_fault.lock() {
            *guard = fault;
        }
    }

    /// The standing meta-tamper notice, if any (poison-tolerant — a poisoned slot reads as none).
    pub fn meta_warning(&self) -> Option<String> {
        self.meta_warning.lock().ok().and_then(|g| g.clone())
    }

    /// Record the outcome of a meta authentication. Only a warning REPLACES what is stored: every
    /// open path calls this, and several run against a vault that isn't the one that raised the
    /// notice, so a clean report must not be able to quietly clear a real one. `clear_meta_warning`
    /// is the deliberate way to drop it.
    pub fn note_meta_report(&self, report: &vault::MetaAuthReport) {
        if let (Some(w), Ok(mut guard)) = (report.warning(), self.meta_warning.lock()) {
            *guard = Some(w);
        }
    }

    /// Drop the meta-tamper notice — for switching to a different vault, where the old vault's
    /// story no longer applies.
    pub fn clear_meta_warning(&self) {
        if let Ok(mut guard) = self.meta_warning.lock() {
            *guard = None;
        }
    }

    /// Open the session after an unlock / open-existing succeeds: install the
    /// connection and its Markdown runtime together, so `db` and `vault` never drift.
    pub fn open_session(&self, conn: Connection, runtime: VaultRuntime) -> error::Result<()> {
        {
            let mut db = self
                .db
                .lock()
                .map_err(|_| error::Error::Other("database lock poisoned".into()))?;
            let mut vault = self
                .vault
                .lock()
                .map_err(|_| error::Error::Other("vault lock poisoned".into()))?;
            *db = Some(conn);
            *vault = Some(runtime);
        }
        // The one choke point that heals fault state: every successful open path (retry,
        // unlock, adopt, repair, curtain take-over) lands here, so none can leave a stale
        // "vault unreachable" story behind.
        self.set_vault_fault(None);
        // Drop the guards before reconciling — it re-locks both to (re)build the rules file or
        // restore the mirror from it. Best-effort: a hiccup here never blocks the open. The
        // index-only manifest reconcile runs AFTER the rules one (it resolves item projects through
        // the rebuilt aliases).
        self.reconcile_entity_rules();
        self.reconcile_index_only();
        // Repair chat vault files stripped by the pre-3.81.2 organisation-write bug. Runs on every
        // open path (retry, unlock, adopt, repair, curtain take-over) so an affected store is healed
        // during ordinary use, well before anyone reaches for Rebuild — which is also independently
        // guarded (`rebuild_core` calls this first).
        self.reconcile_chat_identity();
        Ok(())
    }

    /// Take the open connection out of the session, closing the store (the `Drop` of the
    /// returned `Connection` releases SQLite's file lock). Used by a vault relocation,
    /// which must unlock the DB file before it can be copied; the caller reopens at the
    /// new location afterwards. Leaves the vault locked (`None`) in the meantime.
    pub fn take_conn(&self) -> error::Result<Option<Connection>> {
        let mut guard = self
            .db
            .lock()
            .map_err(|_| error::Error::Other("database lock poisoned".into()))?;
        Ok(guard.take())
    }

    /// Replace the active vault's Markdown runtime in place (the connection stays open).
    /// Used when a transition changes the Markdown policy — e.g. making a vault
    /// shareable flips encryption on without reopening the store.
    pub fn set_vault_runtime(&self, runtime: VaultRuntime) -> error::Result<()> {
        {
            let mut guard = self
                .vault
                .lock()
                .map_err(|_| error::Error::Other("vault lock poisoned".into()))?;
            *guard = Some(runtime);
        }
        // A policy/key transition swaps the rules + manifest ciphers too; reconcile heals each file
        // under the new key (the mirror survived the SQLCipher rekey intact). Best-effort.
        self.reconcile_entity_rules();
        self.reconcile_index_only();
        Ok(())
    }

    /// Clear the active vault's Markdown runtime (used alongside `take_conn` when the lock
    /// session closes the store because another profile became the active writer).
    pub fn clear_vault_runtime(&self) -> error::Result<()> {
        let mut guard = self
            .vault
            .lock()
            .map_err(|_| error::Error::Other("vault lock poisoned".into()))?;
        *guard = None;
        Ok(())
    }

    /// Snapshot the active vault's Markdown dir + cipher, or a friendly error if the
    /// vault is locked. Cloned so the caller can do file IO without holding the lock —
    /// the single way ingest reaches the Markdown layer.
    pub fn markdown_io(&self) -> error::Result<(PathBuf, vault::MarkdownCipher)> {
        let guard = self
            .vault
            .lock()
            .map_err(|_| error::Error::Other("vault lock poisoned".into()))?;
        match guard.as_ref() {
            Some(rt) => Ok((rt.markdown_dir.clone(), rt.cipher.clone())),
            None => Err(error::Error::Other(session_unavailable_message(
                self.vault_fault().as_ref(),
            ))),
        }
    }

    /// Snapshot the active vault's root + always-on rules cipher, or a friendly error if the vault
    /// is locked. Cloned so the caller can do file IO off the lock — the single way the entity
    /// layer reaches the encrypted rules file.
    pub fn rules_io(&self) -> error::Result<(PathBuf, entities::RulesCipher)> {
        let guard = self
            .vault
            .lock()
            .map_err(|_| error::Error::Other("vault lock poisoned".into()))?;
        match guard.as_ref() {
            Some(rt) => Ok((rt.vault_root.clone(), rt.rules_cipher.clone())),
            None => Err(error::Error::Other(session_unavailable_message(
                self.vault_fault().as_ref(),
            ))),
        }
    }

    /// Snapshot the active vault's root + always-on manifest cipher, or a friendly error if the
    /// vault is locked. The single way the index-only layer reaches its encrypted manifest, off the
    /// lock — parity with [`AppState::rules_io`].
    pub fn manifest_io(&self) -> error::Result<(PathBuf, index_only::ManifestCipher)> {
        let guard = self
            .vault
            .lock()
            .map_err(|_| error::Error::Other("vault lock poisoned".into()))?;
        match guard.as_ref() {
            Some(rt) => Ok((rt.vault_root.clone(), rt.manifest_cipher.clone())),
            None => Err(error::Error::Other(session_unavailable_message(
                self.vault_fault().as_ref(),
            ))),
        }
    }

    /// Reconcile the encrypted rules file with the DB mirror for the active session: write it on
    /// first run, rebuild the mirror from it on later opens, or heal it after a key rotation. A
    /// no-op when the vault is locked. Best-effort — the DB mirror is the live truth, so a failure
    /// is logged, never propagated to block opening the vault.
    pub fn reconcile_entity_rules(&self) {
        let (vault_root, rules_cipher) = match self.rules_io() {
            Ok(v) => v,
            Err(_) => return,
        };
        let conn = match self.conn() {
            Ok(c) => c,
            Err(_) => return,
        };
        if let Err(e) = entities::reconcile_on_open(&conn, &vault_root, &rules_cipher) {
            eprintln!("entities: rules reconcile skipped ({e})");
        }
    }

    /// The setting key holding the most recent [`chat::ChatIdentityHeal`] as JSON, so the Developer
    /// readout can answer "did the repair actually run, and what did it find?" without re-scanning.
    pub const CHAT_HEAL_KEY: &'static str = "chat_identity_heal";

    /// Repair chat vault files stripped of their identity by the pre-3.81.2 organisation-write bug,
    /// and any damage a Rebuild has since done on top (see [`chat::reconcile_vault_identity`]).
    ///
    /// Called on vault open AND as a precondition of a Rebuild. Both, deliberately: the open-time run
    /// repairs a store during ordinary use, and the Rebuild-time run closes the one window that would
    /// otherwise turn recoverable damage into permanent damage — a user who updates and immediately
    /// hits Rebuild. It is idempotent and writes only where something is wrong, so the second call is
    /// free on a healthy store.
    ///
    /// A no-op when the vault is locked. Best-effort: a failure is logged, never propagated, because
    /// blocking vault open (or a Rebuild) over a repair would be worse than the defect it repairs.
    pub fn reconcile_chat_identity(&self) -> chat::ChatIdentityHeal {
        let (vault, cipher) = match self.markdown_io() {
            Ok(v) => v,
            Err(_) => return chat::ChatIdentityHeal::default(),
        };
        let mut conn = match self.conn() {
            Ok(c) => c,
            Err(_) => return chat::ChatIdentityHeal::default(),
        };
        match chat::reconcile_vault_identity(&mut conn, &vault, &cipher) {
            Ok(report) => {
                if report.touched_anything() {
                    eprintln!(
                        "chat: repaired vault identity — {} file(s) restamped, {} row(s) restored, \
                         {} re-link(s), {} queued for re-index (of {} scanned)",
                        report.restamped,
                        report.rows_restored,
                        report.relinked,
                        report.reindex_queued,
                        report.scanned,
                    );
                }
                // Persisted even on a clean pass, so the readout can distinguish "checked, all well"
                // from "never ran" — the distinction that makes this verifiable rather than assumed.
                if let Ok(json) = serde_json::to_string(&report) {
                    let _ = db::set_setting(&conn, Self::CHAT_HEAL_KEY, &json);
                }
                report
            }
            Err(e) => {
                eprintln!("chat: vault identity reconcile skipped ({e})");
                chat::ChatIdentityHeal::default()
            }
        }
    }

    /// Push the DB mirror out to the encrypted rules file (the one-way mirror→file direction).
    /// Called after ingest/rebuild, which only ever resolve an existing entity in practice but may
    /// create one for a never-seen project — keeping the portable source of truth current. A no-op
    /// when locked; best-effort (the mirror remains the live truth if the file write hiccups).
    pub fn sync_entity_rules(&self) {
        let (vault_root, rules_cipher) = match self.rules_io() {
            Ok(v) => v,
            Err(_) => return,
        };
        let conn = match self.conn() {
            Ok(c) => c,
            Err(_) => return,
        };
        let synced = entities::rules_from_mirror(&conn)
            .and_then(|rules| entities::write_rules_file(&vault_root, &rules_cipher, &rules))
            .map(|_| ());
        if let Err(e) = synced {
            eprintln!("entities: rules sync skipped ({e})");
        }
    }

    /// Reconcile the encrypted index-only manifest with the DB mirror for the active session. Runs
    /// AFTER [`AppState::reconcile_entity_rules`] (it resolves each item's project through the
    /// rebuilt aliases). A no-op when the vault is locked; best-effort, like the rules reconcile —
    /// the DB mirror is the live truth, so a hiccup never blocks the open.
    pub fn reconcile_index_only(&self) {
        let (vault_root, cipher) = match self.manifest_io() {
            Ok(v) => v,
            Err(_) => return,
        };
        let conn = match self.conn() {
            Ok(c) => c,
            Err(_) => return,
        };
        // v19 follow-up: retire identical shared-drive twins (harmless duplicates); a divergent twin is
        // left in place for a user-facing merge (deferred). Best-effort, cheap after the first pass.
        match crate::drive::resolve_shared_drive_twins(&conn) {
            Ok(s) if s.retired + s.divergent + s.adopted > 0 => eprintln!(
                "drive: shared-drive twins — retired {}, {} divergent left for review, adopted {}",
                s.retired, s.divergent, s.adopted
            ),
            Ok(_) => {}
            Err(e) => eprintln!("drive: shared-drive twin sweep skipped ({e})"),
        }
        match index_only::reconcile_on_open(&conn, &vault_root, &cipher) {
            // F-04: the reconcile resolves each item's project with `create_if_new`, so a manifest
            // naming a project the mirror lacks MINTS an entity here. Push it to the portable rules
            // file or the next boot's file-is-truth pass rolls it straight back — the mint repeats,
            // and round it goes forever. Dropping the connection first: `sync_entity_rules` takes
            // its own, and re-entering `conn()` under the guard self-deadlocks the whole app.
            Ok(minted) => {
                drop(conn);
                if minted {
                    self.sync_entity_rules();
                }
            }
            Err(e) => eprintln!("index_only: manifest reconcile skipped ({e})"),
        }
    }

    /// Push the DB mirror out to the encrypted index-only manifest (mirror→file). Called after a
    /// pointer is registered / re-embedded so the portable classification stays current. A no-op
    /// when locked; best-effort.
    pub fn sync_index_only(&self) {
        let (vault_root, cipher) = match self.manifest_io() {
            Ok(v) => v,
            Err(_) => return,
        };
        let conn = match self.conn() {
            Ok(c) => c,
            Err(_) => return,
        };
        if let Err(e) = index_only::write_synced(&conn, &vault_root, &cipher).map(|_| ()) {
            // The DB has already moved on, so the file is now behind it. Record that, or the next
            // boot would read the stale file as portable truth and quietly revert the change.
            eprintln!("index_only: manifest sync skipped ({e})");
            index_only::mark_manifest_stale(&conn);
        }
    }

    /// Whether the store is currently open (the vault is unlocked this session).
    pub fn is_unlocked(&self) -> bool {
        self.db.lock().map(|guard| guard.is_some()).unwrap_or(false)
    }

    /// Build the model gateway for an operation, resolving this vault's embedder (and the reranker
    /// paired with it) from an already-open connection. Takes a `&Connection` rather than locking
    /// `db` itself, so a caller holding the guard doesn't deadlock — and, because the gateway
    /// captures owned model entries, the caller can drop the guard and still rerank off the lock.
    pub fn gateway(&self, conn: &Connection) -> error::Result<model_gateway::ModelGateway<'_>> {
        let embedder = db::selected_embedder(conn)?;
        let reranker = registry::reranker_for(&embedder);
        Ok(model_gateway::ModelGateway::new(
            &self.sidecar,
            embedder,
            reranker,
        ))
    }

    /// Like [`AppState::gateway`], but first refuses an embedder whose vector width the vault's
    /// **live** `chunk_vec` can no longer hold (via [`ingest::guard_dimension`]). This is the seam
    /// for every embed-**write** path — note ingest, spreadsheet promote, and the connector
    /// executors that run unattended each sync cycle. Without it, after the user switches search
    /// language but before re-indexing, those writes pass `check_embeddings` (which compares against
    /// the embedder's own width, not the table's) and then fail deep inside the insert with a raw
    /// sqlite-vec error — on a background connector sync, every cycle, with no guidance. The guard
    /// converts that into the same "re-index the vault" message `ingest::run` already gives (F-46).
    /// Read/query paths keep using the unchecked [`AppState::gateway`]: a query against a stale
    /// index is a separate concern, and guarding it would add a `sqlite_master` read to the
    /// chat-retrieval hot path.
    pub fn gateway_for_write(
        &self,
        conn: &Connection,
    ) -> error::Result<model_gateway::ModelGateway<'_>> {
        let embedder = db::selected_embedder(conn)?;
        ingest::guard_dimension(conn, &embedder)?;
        let reranker = registry::reranker_for(&embedder);
        Ok(model_gateway::ModelGateway::new(
            &self.sidecar,
            embedder,
            reranker,
        ))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Linux/WebKitGTK stability: the WebKitWebProcess renderer SIGABRTs on close when a
    // Skia GPU painting thread tears down its thread-local SkiaGLContext
    // (eglDestroyContext) while the process is already running exit handlers — an
    // upstream teardown race in WebKitGTK's Skia backend. Forcing Skia CPU rasterization
    // means no per-painting-thread GL context is created, so there is nothing to race at
    // exit. Accelerated compositing stays on; only layer rasterization moves to the CPU
    // (negligible for PM's DOM-heavy UI + 2D-canvas map, which uses no WebGL). The var is
    // value-sensitive ("1" = CPU); respect an explicit user override (any value). Must be
    // the FIRST statement: set_var is only sound while single-threaded, and no threads
    // exist before Builder starts (single-instance plugin, tokio runtime, and GTK/WebKit
    // all init inside/after it). Remove once the fleet is on a fixed system WebKitGTK.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_SKIA_ENABLE_CPU_RENDERING").is_none() {
        std::env::set_var("WEBKIT_SKIA_ENABLE_CPU_RENDERING", "1");
    }

    tauri::Builder::default()
        // Must be registered first. One instance only: a second launch (e.g. a
        // double-click or an updater relaunch overlap) focuses the running window
        // and exits, so two processes can't race to create the encrypted store
        // with different keys and orphan one of them (rule #2).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Close policy for both windows: the briefing panel always just hides, and the main
        // window hides instead of quitting ONLY when the tray is on (see tray.rs).
        .on_window_event(tray::on_window_event)
        .setup(|app| {
            let handle = app.handle();
            // One-time: move the Google Calendar token from the legacy shared keychain key to its
            // per-service key (the move to per-service connectors). Keychain-only, no vault needed;
            // idempotent and best-effort, so a keychain hiccup never blocks startup.
            let _ = secrets::migrate_legacy_google_token();
            // If a vault migration was interrupted, repair it before anything opens the
            // store: roll the in-place phase back to its backup, or finish/discard a
            // partial move. A no-op when no migration journal is present.
            vault::migrate::recover(handle)?;
            // Resolve where this profile's vault lives — pointer-aware, but defaulting
            // to the per-profile data dir when no pointer is set (today's behaviour).
            // Resolved by hand (not `vault::resolve`) because a POINTED root that stopped
            // answering must degrade to a locked boot below, not abort setup here.
            let data_dir = paths::data_dir(handle)?;
            // A pointer FILE that exists but won't load (denied/corrupt) also degrades to a
            // locked boot carrying the fault — never an aborted setup, and never a fresh
            // default-location vault silently minted while the real one sits behind the
            // unreadable pointer. `Ok(None)` (no file at all) stays the plain default case.
            let (boot_pointer, pointer_fault) = match vault::pointer::load(&data_dir) {
                Ok(p) => (p, None),
                Err(e) => {
                    let fault = error::VaultFault::from_error("read PM's vault pointer", &e);
                    eprintln!(
                        "vault: pointer unreadable, booting locked: {}",
                        fault.message
                    );
                    (None, Some(fault))
                }
            };
            let pointer_present = boot_pointer.is_some() || pointer_fault.is_some();
            let resolved = vault::resolve_layout(&data_dir, boot_pointer.as_ref());
            let root_dirs_ready = std::fs::create_dir_all(&resolved.vault_root)
                .map_err(error::io_at(
                    "prepare the vault folder",
                    &resolved.vault_root,
                ))
                .and_then(|()| {
                    std::fs::create_dir_all(&resolved.markdown_dir).map_err(error::io_at(
                        "prepare the vault folder",
                        &resolved.markdown_dir,
                    ))
                })
                .map_err(|e| error::VaultFault::from_error("prepare the vault folder", &e));
            if !pointer_present {
                // The default location must be creatable — that failure is fatal, as before.
                if let Err(f) = &root_dirs_ready {
                    return Err(error::Error::Vault(f.clone()).into());
                }
            }

            // GC abandoned restore staging before opening: each `restored-vaults/restore-*` is a full,
            // decryptable vault copy left by a restore-and-inspect. A copy the user didn't switch to
            // has an in-memory-only key that died with the last process, so it can never be reopened —
            // sweep every staged copy except the one this profile is actually pointed at. Best-effort;
            // a locked file just waits for the next boot. (F-25)
            if let Ok(data_dir) = paths::data_dir(handle) {
                wipe::sweep_restore_staging(&data_dir, &resolved.vault_root);
            }

            // Same idea, one directory over: PM's staging files in the OS temp dir. A connector
            // download is staged there in the CLEAR for the converter to read, and its auto-delete
            // is deliberately disarmed so the converter can open it — so an interrupted sync leaves
            // the user's actual document lying in %TEMP%, and nothing has ever collected it. Boot is
            // the safe moment: no sync, backup or restore can be in flight yet.
            let swept = wipe::sweep_temp_staging(&handle.package_info().name);
            if swept > 0 {
                eprintln!(
                    "startup: cleared {swept} abandoned staging file(s) from the temp folder"
                );
            }

            // If a prior full "remove PM completely" wipe armed the uninstaller's purge marker but the
            // uninstall didn't happen (cancelled), we're booting normally — the user kept/reinstalled
            // PM — so clear the stale marker or a later *ordinary* uninstall would wrongly purge data.
            paths::clear_stale_uninstall_purge_marker(handle);

            // Decide what to do about metadata (spec §6). At the DEFAULT location a
            // fresh profile still gets its zero-friction device vault; behind a POINTER,
            // missing or unreachable metadata boots LOCKED with the fault carried —
            // never a fresh empty vault silently minted where the shared one should be
            // (the failure that made a broken join look like "all my data vanished").
            let meta_load = match (&pointer_fault, &root_dirs_ready) {
                (Some(f), _) => Err(f.clone()),
                (None, Ok(())) => vault::load_meta(&resolved.vault_root)
                    .map_err(|e| error::VaultFault::from_error("read the vault's settings", &e)),
                (None, Err(f)) => Err(f.clone()),
            };
            let boot_meta = vault::boot_meta_decision(pointer_present, meta_load)
                .map_err(error::Error::Vault)?;
            // Has the store been removed from under us? `db::open` would CREATE a missing
            // pm.sqlite and migrate it, presenting an empty library as a healthy one — so
            // this is decided BEFORE any open, and before `ensure_device_meta` writes the
            // metadata that would itself become evidence of use. A pointed vault that has
            // gone missing keeps its own richer fault below.
            let store_gone = vault::inspect_store_presence(&resolved, &data_dir)
                == vault::StorePresence::Missing;
            let open_attempt = match boot_meta {
                vault::BootMeta::UseExisting(_) | vault::BootMeta::CreateDeviceDefault
                    if store_gone =>
                {
                    Err(vault::store_missing_fault(&resolved.db_path))
                }
                vault::BootMeta::UseExisting(meta) => Ok(*meta),
                vault::BootMeta::CreateDeviceDefault => {
                    Ok(vault::ensure_device_meta(&resolved.vault_root)?)
                }
                // Compose the user-facing story around the fault's classification, keeping
                // code/path intact so the recovery screen can pick its actions (Repair for
                // denied, Try again / detach for a gone folder). An unreadable pointer FILE
                // keeps its own message — the pointed folder isn't even known in that case.
                vault::BootMeta::PointedVaultMissing(fault) => {
                    let root = resolved.vault_root.display();
                    let message = if pointer_fault.is_some() {
                        fault.message.clone()
                    } else {
                        match fault.code {
                            error::VaultFaultCode::Denied => format!(
                                "Windows is refusing this account access to your vault at \
                                 {root} — your documents are still there, nothing has been \
                                 deleted. Repair access below, or the vault's owner may need \
                                 to re-add this account"
                            ),
                            error::VaultFaultCode::NoVault | error::VaultFaultCode::NotFound => {
                                format!(
                                    "your shared vault at {root} isn't there any more: {} — \
                                     it may have been moved or deleted, or live on a drive \
                                     that isn't connected",
                                    fault.message
                                )
                            }
                            _ => format!(
                                "your shared vault at {root} isn't reachable: {} — its owner \
                                 may need to re-add this account, or you can go back to a \
                                 vault on this account from Settings",
                                fault.message
                            ),
                        }
                    };
                    Err(error::VaultFault { message, ..fault })
                }
            };
            // On a successful open we also get the Markdown cipher; pair it with the
            // resolved Markdown dir as the session's vault runtime. A locked
            // (passphrase, uncached) vault leaves both `None` until an unlock command.
            // A device vault opens now with the keychain key; a passphrase/shareable
            // vault opens only if this profile cached its key.
            let (conn, vault_runtime, boot_fault, boot_meta_warning) = match &open_attempt {
                Ok(meta) => match vault::open_at_boot(&resolved, meta) {
                    Ok(Some((conn, master, report))) => {
                        // Self-heal the discovery marker for an open shared vault living
                        // outside the profile (best-effort; a wiped ProgramData grows it
                        // back, and identical content skips the write).
                        if meta.key_mode == vault::KeyMode::Passphrase
                            && !resolved.vault_root.starts_with(&data_dir)
                        {
                            if let Some(ads) = vault::advert::ads_dir() {
                                let ad = vault::advert::SharedVaultAd::for_vault(
                                    &meta.vault_id,
                                    &resolved.vault_root,
                                );
                                if let Err(e) = vault::advert::publish(&ads, &ad) {
                                    eprintln!(
                                        "vault: could not refresh the shared-vault marker: {e}"
                                    );
                                }
                            }
                        }
                        (
                            Some(conn),
                            Some(VaultRuntime::build(&resolved, meta, &master)),
                            None,
                            report.warning(),
                        )
                    }
                    Ok(None) => (None, None, None, None),
                    // A boot-time open failure (a transient AV/search-indexer file lock, disk I/O)
                    // must NOT abort the whole app — `db::open` already maps these to friendly,
                    // retryable messages. Degrade to a not-opened store and carry the fault so the
                    // UI can offer Retry, exactly like a locked vault (B1-6). Only a genuinely
                    // corrupt store then needs a manual fix, instead of every transient hiccup.
                    Err(e) => {
                        let fault = error::VaultFault::from_error("open the vault", &e);
                        eprintln!(
                            "vault: boot open failed, starting locked with Retry: {}",
                            fault.message
                        );
                        (None, None, Some(fault), None)
                    }
                },
                // The pointed vault is missing/unreachable: boot locked, carrying the
                // classified story so the UI can offer Repair, Retry, or a detach.
                Err(fault) => {
                    eprintln!("vault: {}", fault.message);
                    (None, None, Some(fault.clone()), None)
                }
            };

            // The sidecar source folder is optional at boot — chat works without
            // it; ingestion surfaces a clear error if it (or Python) is missing.
            // M-5: `sidecar_source_dir` hard-errors in release when the bundled resource is missing.
            // Fall back to the ABSOLUTE resource path, never a CWD-relative "sidecar" an unprivileged
            // process could plant — if `pm_sidecar.py` isn't there, ingestion fails cleanly and chat
            // still works.
            let source_dir = paths::sidecar_source_dir(handle).unwrap_or_else(|_| {
                handle
                    .path()
                    .resource_dir()
                    .map(|r| r.join("sidecar"))
                    .unwrap_or_else(|_| PathBuf::from("sidecar"))
            });
            let venv_dir = paths::venv_dir(handle)?;
            let sidecar = SidecarManager::new(SidecarPaths {
                source_dir,
                venv_dir,
            });

            app.manage(AppState {
                db: Mutex::new(conn),
                vault: Mutex::new(vault_runtime),
                sidecar,
                app_unlocked: AtomicBool::new(false),
                instance_id: vault::lock::new_instance_id(),
                lock_session: Mutex::new(lock_session::LockSession::default()),
                drive_sync: Mutex::new(CloudSyncState::default()),
                drive_sync_cancel: AtomicBool::new(false),
                onedrive_sync: Mutex::new(CloudSyncState::default()),
                onedrive_sync_cancel: AtomicBool::new(false),
                local_sync: Mutex::new(LocalFolderSyncState::default()),
                local_sync_cancel: AtomicBool::new(false),
                ingest_job: Mutex::new(IngestJobState::default()),
                ingest_busy: AtomicBool::new(false),
                layout_job: Mutex::new(layout::LayoutJobState::default()),
                last_user_activity: Mutex::new(Instant::now()),
                chat_index_busy: AtomicBool::new(false),
                summary_busy: AtomicBool::new(false),
                title_busy: AtomicBool::new(false),
                prefs_busy: AtomicBool::new(false),
                layout_busy: AtomicBool::new(false),
                backup_state: Mutex::new(BackupState::default()),
                backup_cancel: AtomicBool::new(false),
                backup_busy: AtomicBool::new(false),
                pending_restore_keys: Mutex::new(std::collections::HashMap::new()),
                vault_fault: Mutex::new(boot_fault),
                meta_warning: Mutex::new(boot_meta_warning),
                briefing_refresh: tokio::sync::Mutex::new(()),
                briefing_dirty: AtomicBool::new(false),
                local_ai: local_slot::LocalRuntime::default(),
            });

            // Engage the cooperative writer lock for a shared vault (acquire it, or step
            // back behind another live profile), then run the heartbeat + hand-off watcher
            // for the life of the app. A no-op for a device-only vault.
            lock_session::engage(handle)?;
            lock_session::spawn_watcher(handle.clone());

            // After the writer lock is settled (so a stepped-back profile doesn't write), reconcile
            // the encrypted entity-rules file with the DB mirror: first run writes it from the v10
            // backfill; later runs rebuild the mirror from it. No-op when the vault is locked. The
            // index-only manifest reconcile runs AFTER it (it resolves item projects through the
            // rebuilt aliases).
            app.state::<AppState>().reconcile_entity_rules();
            app.state::<AppState>().reconcile_index_only();
            // Repair chat vault identity stripped by the pre-3.81.2 organisation-write bug, before
            // the user can reach any surface that would make the damage permanent.
            app.state::<AppState>().reconcile_chat_identity();

            // One-time: distil the legacy "Learning You" blob into structured preference records
            // (§4.5) so nothing accumulated is lost. Background, idempotent (a settings flag guards
            // it), best-effort; a no-op when the vault is locked or it has already run. Also retried
            // after a review commit, so a vault locked at startup still migrates once unlocked.
            commands::spawn_preferences_migration(handle.clone());

            // Catch up chat indexing for anything whose turn-pairs ran ahead of the index while the app
            // was closed (board card 7B). Background, best-effort; waits for the vault to unlock + the
            // engine to be provisioned, and never triggers a first-run engine build itself. The idle
            // indexer then keeps a live session progressively indexed during lulls (never competing with
            // active use), so nothing waits for the next launch.
            // F-54: the four chat launch passes below (index / summary / title / prefs) all wait for the
            // vault to unlock and would otherwise fire at once (an embed sweep + three model calls) —
            // a thundering herd at t≈5s, worsened by wake-from-sleep making every backstop due together.
            // Stagger them: the index sweep runs first, the other three at +30 / +60 / +90s after unlock.
            chat_index::spawn_launch_sweep(handle.clone());
            chat_index::spawn_idle_indexer(handle.clone());

            // Keep each long chat's rolling summary (board card 7C) caught up in the background: a launch
            // pass folds any backlog that grew while the app was closed, then an idle backstop reconciles
            // during lulls. The eager per-conversation nudge fires from `send_message`; this scheduler is
            // the catch-up net for sessions whose nudge never ran (no key at the time, app closed first).
            chat_summary::spawn_summary_scheduler(
                handle.clone(),
                std::time::Duration::from_secs(30),
            );

            // Automatic encrypted backups to Proton Drive (backup epic PR3): a launch catch-up +
            // idle backstop that backs up when due per the user's cadence and trims old archives.
            // Gated on unlocked + idle + Proton connected + an opted-in stored passphrase; a no-op
            // for everyone who hasn't turned it on.
            backup::schedule::spawn_backup_scheduler(handle.clone());

            // Compact the project activity log daily and prune its raw window (Stage-3 heat log):
            // once a day, when unlocked + idle, roll raw events older than the recent window into
            // per-day counts and delete them. A no-op until there are old events; nothing reads it yet.
            project_activity::spawn_rollup_scheduler(handle.clone());

            // Keep the structured flag layer current (board card 9): a backstop that re-evaluates
            // the proactive flags (deadline-approaching → overdue, today's events, prepare-ahead)
            // when the app is left open past a day boundary without a briefing refresh. Detection
            // also runs synchronously on every briefing refresh; gated on unlocked + idle + not
            // mid-sync, and a no-op until there are milestones/events in the near window.
            flags::spawn_flag_detection_scheduler(handle.clone());

            // Keep the daily briefing current without the user clicking Refresh (#540): picks up an
            // inputs-changed nudge (calendar sync, milestone edit, flag resolved) within a minute
            // and otherwise checks hourly. A check that finds the facts unmoved costs one DB pass
            // and no model call, so a quiet day is free. The launch check is the frontend's, since
            // it fires after unlock, when the store is actually open.
            briefing::spawn_briefing_scheduler(handle.clone());

            // Give each conversation a real 5-7 word title (board card 7E) once it has a few turns: a launch
            // pass titles any session that crossed the threshold while the app was closed (the eager
            // per-conversation nudge fires from `send_message`). Background, best-effort, single model call.
            chat_title::spawn_title_scheduler(handle.clone(), std::time::Duration::from_secs(60));

            // Capture preferences a user STATED in chat (board card 7F) as suggested Teach records: a
            // launch pass scans turns added while the app was closed (the eager per-conversation nudge
            // fires from `send_message`). Background, best-effort, explicit-only — never inferred.
            chat_prefs::spawn_prefs_scheduler(handle.clone(), std::time::Duration::from_secs(90));

            // Watch every tracked local folder (board card 6, PR2) for live changes: a debounced
            // filesystem watcher that re-embeds a saved file within seconds and keeps deletes/renames
            // reconciled, without a full walk. Reuses the on-demand sync's per-file semantics; runs a
            // catch-up reconcile on each unlock (self-healing anything changed while closed), and is a
            // no-op until a folder is tracked. Observer-only — takes no vault lock.
            localfolder::spawn_local_watcher(handle.clone());

            // The tray icon and the always-on-top briefing window. Both start hidden; the tray is
            // shown only if the user has switched it on. Best-effort: a desktop with no
            // StatusNotifierItem host (or no appindicator library at all) must not block startup.
            //
            // The window MUST be built here and nowhere else — `WebviewWindowBuilder::build()`
            // deadlocks Windows when called from a synchronous command or an event handler, which is
            // what the Settings toggle and the tray menu are (see tray.rs). Quitting properly is
            // handled by `tray::on_window_event`, which exits explicitly, not by withholding this.
            let _ = tray::build_briefing_window(handle);
            tray::init(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::has_openrouter_key,
            commands::set_openrouter_key,
            commands::has_openrouter_background_key,
            commands::set_openrouter_background_key,
            // Local AI endpoint (#297): detection, the posture-checked endpoint check, config
            // get/set, model listing, and the live status snapshot.
            local_ai::probe_local_llm_ports,
            local_ai::check_local_llm_endpoint,
            local_ai::get_local_llm_config,
            local_ai::set_local_llm_endpoint,
            local_ai::clear_local_llm_endpoint,
            local_ai::set_local_llm_role_model,
            local_ai::set_local_llm_routing,
            local_ai::set_local_llm_token,
            local_ai::clear_local_llm_token,
            local_ai::list_local_llm_models,
            local_ai::pull_local_model,
            local_ai::accept_local_model_terms,
            local_ai::local_llm_status,
            // Workbench (#296): hardware scan + per-machine model recommendations.
            local_ai::local_hardware_scan,
            local_ai::local_model_recommendations,
            local_ai::set_local_model_scan_dir,
            local_ai::local_better_fit_notice,
            local_ai::dismiss_local_better_fit,
            local_ai::set_local_model_rescan_cadence,
            settings::get_settings,
            settings::settings_defaults,
            settings::set_indexing_speed,
            settings::set_chat_models,
            settings::set_background_models,
            settings::set_chat_auto_switch,
            settings::set_background_auto_switch,
            settings::set_help_mode,
            settings::set_reranking,
            settings::set_duplicate_check,
            duplicates::scan_duplicates,
            // ONE-TIME cleanup — delete these three with the `sweep` module in the release after
            // this one (card #651's follow-up).
            sweep::scan_orphan_files,
            sweep::delete_orphan_files,
            sweep::dismiss_orphan_sweep,
            settings::set_retrieval_k,
            settings::set_retrieval_confidence_threshold,
            settings::ai_provider_status,
            settings::set_onboarding_done,
            commands::retrieval_explain,
            commands::retrieval_diagnose,
            settings::language_options,
            settings::set_vault_embedder,
            settings::set_time_zone,
            settings::get_pref,
            settings::set_pref,
            settings::app_lock_status,
            settings::set_app_lock,
            commands::unlock_app,
            commands::vault_status,
            commands::retry_open_vault,
            commands::create_replacement_store,
            commands::create_shareable_vault,
            commands::change_vault_passphrase,
            commands::make_vault_private,
            commands::move_vault,
            commands::unlock_vault,
            commands::forget_vault_passphrase,
            commands::score_passphrase,
            commands::link_vault_account,
            commands::list_shared_vaults,
            commands::adopt_shared_vault,
            commands::detach_from_shared_vault,
            commands::repair_vault_access,
            commands::delete_shared_vault,
            commands::acknowledge_deleted_shared_vault,
            commands::suggest_shared_vault_location,
            commands::list_local_accounts,
            commands::vault_lock_status,
            commands::continue_here,
            commands::force_take_vault,
            commands::list_models,
            commands::list_preferences,
            commands::add_preference,
            commands::update_preference,
            commands::confirm_preference,
            commands::delete_preference,
            commands::parse_preference_statement,
            commands::import_ai_memory,
            commands::list_conversations,
            commands::create_conversation,
            commands::get_messages,
            commands::rename_conversation,
            commands::set_conversation_project,
            commands::delete_conversation,
            commands::send_message,
            commands::mark_activity,
            commands::chat_context_status,
            commands::compress_chat,
            commands::revert_compress,
            commands::sidecar_status,
            commands::ensure_sidecar,
            commands::ingest_paths,
            commands::rebuild_index,
            commands::ingest_note,
            #[cfg(debug_assertions)]
            commands::dev_apply_change_event,
            commands::list_documents,
            commands::get_document,
            commands::transcribe_audio,
            commands::list_projects,
            commands::retag_scope,
            commands::propose_retag_vocabulary,
            commands::apply_retag_vocabulary,
            commands::delete_tag,
            commands::rename_tag,
            commands::list_tag_proposals,
            commands::discard_tag_proposals,
            commands::commit_retag,
            commands::list_tags,
            commands::review_queue,
            commands::review_queue_count,
            commands::cached_proposals,
            commands::propose_metadata,
            commands::commit_review,
            commands::set_document_metadata,
            commands::list_entities,
            commands::add_entity_alias,
            commands::remove_entity_alias,
            commands::rename_entity,
            commands::merge_entities,
            commands::merge_project_preview,
            commands::merge_projects,
            commands::delete_project_preview,
            commands::delete_project,
            commands::delete_document,
            commands::list_project_overviews,
            commands::set_project_metadata,
            commands::propose_project_metadata,
            commands::list_milestones,
            commands::list_all_milestones,
            commands::add_milestone,
            commands::update_milestone,
            commands::set_milestone_event,
            commands::set_milestone_state,
            commands::set_milestone_status,
            commands::rate_answer,
            commands::record_citation_click,
            commands::answer_feedback,
            commands::delete_milestone,
            commands::reorder_milestones,
            commands::calendar_overview,
            commands::set_calendar_selected,
            commands::set_calendar_quiet,
            commands::set_calendar_kind,
            commands::connect_google_calendar_account,
            commands::disconnect_google_calendar_account,
            commands::connect_outlook_calendar,
            commands::disconnect_outlook_calendar,
            commands::list_ics_feeds,
            commands::add_ics_feed,
            commands::remove_ics_feed,
            commands::set_google_client,
            commands::clear_google_client,
            commands::sync_calendar,
            commands::list_calendar_events,
            commands::list_all_calendar_events,
            commands::event_flags,
            commands::connect_drive,
            commands::disconnect_drive,
            commands::drive_status,
            commands::sync_drive,
            commands::drive_sync_status,
            commands::stop_drive_sync,
            commands::resume_drive_sync,
            commands::rebuild_status,
            commands::chat_identity_report,
            commands::resume_rebuild,
            commands::list_drive_shared_drives,
            commands::drive_shared_owners,
            commands::list_drive_folders,
            commands::list_drive_shared_with_me_roots,
            commands::drive_swm_root_owners,
            commands::get_drive_scope,
            commands::set_drive_scope,
            commands::onedrive_status,
            commands::set_microsoft_client,
            commands::clear_microsoft_client,
            commands::connect_onedrive,
            commands::disconnect_onedrive,
            commands::sync_onedrive,
            commands::onedrive_sync_status,
            commands::stop_onedrive_sync,
            commands::resume_onedrive_sync,
            commands::list_onedrive_folders,
            commands::get_onedrive_scope,
            commands::set_onedrive_scope,
            commands::add_local_folder,
            commands::remove_local_folder,
            commands::list_local_folders,
            commands::list_local_subfolders,
            commands::set_local_excludes,
            commands::sync_local_folder,
            commands::local_folder_sync_status,
            commands::stop_local_folder_sync,
            commands::resume_local_folder_sync,
            commands::fetch_index_only_body,
            commands::reindex_index_only,
            commands::promote_index_only,
            layout::semantic_layout,
            layout::start_semantic_layout,
            layout::prioritise_semantic_layout,
            layout::project_layout,
            layout::set_project_layout,
            layout::optional_tsne_status,
            layout::install_optional_tsne,
            photos::optional_ocr_status,
            photos::install_optional_ocr,
            components::list_storage_components,
            components::remove_storage_component,
            commands::open_source,
            commands::read_document_body,
            commands::document_chunk_spans,
            commands::read_document_image,
            commands::open_url,
            commands::get_tray_enabled,
            commands::set_tray_enabled,
            commands::set_briefing_window_visible,
            commands::close_briefing_window,
            commands::destroy_briefing_window,
            commands::show_main_window,
            commands::get_daily_briefing,
            commands::refresh_daily_briefing,
            commands::sync_daily_briefing,
            commands::resolve_flag,
            commands::route_focus_input,
            commands::cost_summary,
            commands::refresh_pricing,
            commands::open_data_folder,
            commands::export_all_data,
            commands::export_plaintext_markdown,
            // "Remove PM data" teardown (Settings → Data & Security) — the à-la-carte counterpart to
            // the Windows uninstaller's automatic `runtime/` cleanup.
            wipe::wipe_pm_data,
            wipe::confirm_wipe_identity,
            // Recover a bricked boot (store present, key lost) and finish a full self-uninstall.
            wipe::reset_after_open_error,
            wipe::launch_uninstaller,
            wipe::reveal_app_in_finder,
            // Encrypted portable backup: local `.pmbackup` archive/restore, plus two off-machine
            // destinations that share the compress→encrypt core and the one schedule — Proton Drive
            // (via its CLI) and Google Drive (via the Drive v3 REST API).
            commands::create_local_backup,
            commands::restore_local_backup,
            commands::switch_to_vault,
            commands::backup_status,
            commands::clear_backup_report,
            commands::stop_backup,
            commands::proton_cli_status,
            commands::set_proton_cli_path,
            commands::proton_connect,
            commands::proton_disconnect,
            commands::proton_status,
            commands::list_proton_backups,
            commands::backup_to_proton,
            commands::restore_from_proton,
            commands::get_backup_schedule,
            commands::set_backup_schedule,
            commands::set_backup_passphrase,
            commands::forget_backup_passphrase,
            commands::set_backup_destinations,
            commands::backup_gdrive_status,
            commands::backup_gdrive_connect,
            commands::backup_gdrive_disconnect,
            commands::list_gdrive_backups,
            commands::backup_to_gdrive,
            commands::restore_from_gdrive,
            commands::backup_now,
            commands::backup_archive_prefix,
            commands::prune_own_backups,
            commands::smart_app_control_state,
            commands::package_managed_linux,
            // Developer mode (issue #78) — read-only inspection. Always registered (the
            // commands are harmless reads); only the UI is gated by the runtime `devMode`.
            commands_dev::dev_system_info,
            commands_dev::dev_table_counts,
            commands_dev::dev_table_list,
            commands_dev::dev_table_rows,
            commands_dev::dev_document_chunks,
            commands_dev::dev_retrieval_explain,
            commands_dev::dev_sidecar_sandbox_report,
            #[cfg(debug_assertions)]
            commands_dev::dev_sidecar_net_selftest,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            // The last thing PM runs, and only after a full purge — on macOS and Linux the app
            // stays open on the "what to do next" screen instead of handing off to an uninstaller,
            // so this catches anything written back between the erase and the quit.
            //
            // It is a PRE-teardown hook: `RunEvent::Exit` fires while the window and webview are
            // still alive (see `wipe::final_sweep_after_purge` for the trace through tao). It
            // narrows the window rather than closing it, and it is deliberately the belt to the
            // braces of clearing the webview's own storage before the directory behind it goes.
            if matches!(event, tauri::RunEvent::Exit) && paths::data_dir_is_purged() {
                wipe::final_sweep_after_purge(app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_unavailable_message_tells_denied_apart_from_locked() {
        // No fault = the plain locked-vault story, byte-for-byte as before.
        assert_eq!(session_unavailable_message(None), "the vault is locked");
        // A Denied fault names the real problem and the way out — never "locked".
        let denied = error::VaultFault {
            code: error::VaultFaultCode::Denied,
            op: "read the vault's settings".into(),
            path: Some("C:/shared".into()),
            message: "PM couldn't read the vault's settings at C:/shared: the system refused \
                      this account access"
                .into(),
        };
        let msg = session_unavailable_message(Some(&denied));
        assert!(msg.contains("Repair access"));
        assert!(msg.contains("C:/shared"));
        assert!(!msg.contains("the vault is locked"));
        // Any other fault surfaces its own message with a Settings pointer.
        let other = error::VaultFault {
            code: error::VaultFaultCode::Other,
            op: "open the vault".into(),
            path: None,
            message: "disk I/O error".into(),
        };
        let msg = session_unavailable_message(Some(&other));
        assert!(msg.contains("disk I/O error"));
        assert!(msg.contains("Settings"));
    }

    #[test]
    fn busy_guard_admits_one_holder_and_reopens_the_slot_on_drop() {
        // Eight callers across seven flags stake single-flight on this type and, since the erase
        // joined them, one stakes a DATA-SAFETY property on it: `wipe_pm_data` refuses while a backup or
        // restore holds `backup_busy`, and holds it itself so none can start mid-erase. Both halves
        // are exactly the three assertions below, and neither was pinned anywhere before.
        let flag = AtomicBool::new(false);

        let first = BusyGuard::acquire(&flag).expect("an idle flag admits the first caller");
        assert!(
            flag.load(std::sync::atomic::Ordering::SeqCst),
            "winning the guard must publish the claim, so a later `load` sees it"
        );
        assert!(
            BusyGuard::acquire(&flag).is_none(),
            "a second caller must lose while the first holds it — this is the whole gate"
        );

        drop(first);
        assert!(
            !flag.load(std::sync::atomic::Ordering::SeqCst),
            "dropping the guard must clear the flag, not merely release the guard"
        );
        let second = BusyGuard::acquire(&flag);
        assert!(
            second.is_some(),
            "the slot must be reusable after a clean release, or one backup would wedge every later one"
        );
        drop(second);

        // Guards on different flags are independent — a held `backup_busy` must not stand down the
        // rebuild or the chat sweeps, which is only true because the guard borrows ONE flag.
        let other = AtomicBool::new(false);
        let _held = BusyGuard::acquire(&flag).unwrap();
        assert!(BusyGuard::acquire(&other).is_some());
    }

    #[test]
    fn busy_guard_releases_when_the_holder_panics() {
        // The F-43 regression for this type. A hand-rolled `store(false)` at the tail of the run is
        // skipped by an unwinding panic, which pins the flag `true` for the rest of the session:
        // every later backup, restore and — now — every "Remove PM data" would refuse until the app
        // restarts, with no way for the user to clear it. RAII release is what makes that
        // unreachable, so it is pinned here rather than left to the type's doc comment.
        let flag = AtomicBool::new(false);

        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // the deliberate panic below isn't a test failure
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = BusyGuard::acquire(&flag).expect("the flag starts idle");
            panic!("the pass dies mid-run");
        }));
        std::panic::set_hook(hook);

        assert!(unwound.is_err(), "the panic really unwound past the guard");
        assert!(
            !flag.load(std::sync::atomic::Ordering::SeqCst),
            "an unwinding panic must still clear the flag"
        );
        assert!(
            BusyGuard::acquire(&flag).is_some(),
            "and the subsystem must be usable again without an app restart"
        );
    }
}
