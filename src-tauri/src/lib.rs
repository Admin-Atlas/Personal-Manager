// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

mod applock;
mod backup;
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
mod entities;
mod error;
// The structured flag layer (board card 9): detection (a pure reducer over milestones + calendar)
// populates first-class flag records, the briefing renders the active set, and a backstop
// scheduler keeps them current. Assertion/resolution and chat grounding arrive in the following PRs.
mod flags;
mod fts_segment;
mod google;
mod ics;
mod index_only;
mod ingest;
mod layout;
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
mod openrouter;
mod outlook_calendar;
mod paths;
mod photos;
mod preferences;
mod project_activity;
mod projects;
mod python_fetch;
mod recommend;
mod registry;
mod retrieval;
mod retrieval_config;
mod retrieval_diag;
mod review;
mod secret;
mod secrets;
mod sidecar;
mod splitter;
mod spreadsheets;
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
    /// The account being synced (email), or `None` for an all-accounts pass.
    pub account: Option<String>,
    /// Internal single-flight flag: a sync was requested while one was already running (e.g. the user
    /// connected another account mid-index). The running pass drains it with one more all-accounts
    /// sweep so the new account is still picked up. Not exposed to the UI.
    #[serde(skip)]
    pub rerun: bool,
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
    /// The folder key being synced, or `None` for an all-folders pass.
    pub folder: Option<String>,
    /// Internal single-flight flag (a sync requested while one was running). Not exposed to the UI.
    #[serde(skip)]
    pub rerun: bool,
    /// The most recent finished sync's report, so a user returning after a sync still sees the result.
    pub last_report: Option<localfolder::LocalSyncReport>,
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
    fn rerun(&self) -> bool {
        self.rerun
    }
    fn set_rerun(&mut self, rerun: bool) {
        self.rerun = rerun;
    }
    fn reset_for_rerun(&mut self) {
        self.processed = 0;
        self.total = None;
        self.account = None;
    }
    fn begin_pass(&mut self, target: Option<String>) {
        *self = CloudSyncState {
            running: true,
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
    fn rerun(&self) -> bool {
        self.rerun
    }
    fn set_rerun(&mut self, rerun: bool) {
        self.rerun = rerun;
    }
    fn reset_for_rerun(&mut self) {
        self.processed = 0;
        self.total = None;
        self.folder = None;
    }
    fn begin_pass(&mut self, target: Option<String>) {
        *self = LocalFolderSyncState {
            running: true,
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
    /// user navigates away and back). Empty/`running:false` when no sync is in flight.
    pub drive_sync: Mutex<CloudSyncState>,
    /// Cooperative stop flag for the running Drive sync. `stop_drive_sync` sets it; the sync loop
    /// checks it between files and halts, keeping everything indexed so far. Reset at each sync start.
    pub drive_sync_cancel: AtomicBool,
    /// Snapshot of the currently-running OneDrive sync (the Microsoft sibling of `drive_sync`).
    pub onedrive_sync: Mutex<CloudSyncState>,
    /// Cooperative stop flag for the running OneDrive sync (the sibling of `drive_sync_cancel`).
    pub onedrive_sync_cancel: AtomicBool,
    /// Snapshot of the currently-running local-folder sync (the filesystem sibling of `drive_sync`).
    pub local_sync: Mutex<LocalFolderSyncState>,
    /// Cooperative stop flag for the running local-folder sync (the sibling of `drive_sync_cancel`).
    pub local_sync_cancel: AtomicBool,
    /// Snapshot of the semantic-map layout precompute (single-flight; running/method/last-error), so
    /// the Map can show progress and a second request folds into the running one. See `layout`.
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
    /// Snapshot of the currently-running encrypted backup / restore, so the Backup settings UI can
    /// resume showing progress after the user navigates away and back (the sibling of `drive_sync`).
    pub backup_state: Mutex<BackupState>,
    /// Cooperative stop flag for the running backup/restore. `stop_backup` sets it; the pack/restore
    /// loop checks it between reads and aborts. Reset at the start of each op.
    pub backup_cancel: AtomicBool,
    /// Single-flight guard so a manual backup, a manual restore, and (later) a scheduled backup can
    /// never overlap and contend for the DB snapshot / archive file.
    pub backup_busy: AtomicBool,
    /// Keys recovered by a restore, held in memory (never the keychain) keyed by the restored
    /// folder, until the user explicitly switches to that vault. Deferring the keychain write to the
    /// switch keeps a restore-you-never-switch-to from clobbering the LIVE vault's cached key. Dropped
    /// (and zeroized) on exit; a restore not switched-to in this session is simply re-done.
    pub pending_restore_keys: Mutex<std::collections::HashMap<String, secret::Secret>>,
    /// A friendly, retryable message when the store *failed to open* at boot (a transient
    /// Windows file lock from antivirus/the search indexer, or disk I/O) — as opposed to a
    /// locked passphrase vault, which is `None` here and `needs_unlock` instead. Set once in
    /// `setup` so the app degrades to a locked state with a Retry rather than panicking out of
    /// setup (B1-6); cleared by `retry_open_vault` on a successful reopen.
    pub boot_open_error: Mutex<Option<String>>,
}

/// Single-flight guard with RAII release. [`BusyGuard::acquire`] returns `Some` if it flipped the flag
/// `false → true` (this caller won the single-flight), or `None` if another pass already holds it. The flag
/// resets to `false` when the guard drops — crucially including an unwinding panic, unlike a trailing
/// `store(false)`. A background task (title / summary / prefs / index sweep) that panics mid-op therefore
/// can't leave its subsystem's flag stuck `true`, which would otherwise silently wedge that feature (its
/// eager nudge AND its scheduler both short-circuit on the flag) until the app restarts.
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

    /// Whether a Drive or OneDrive sync is currently running — the idle indexer defers to either so they
    /// don't contend for the engine (mirrors the layout precompute's idle-priority).
    pub fn sync_active(&self) -> bool {
        let drive = self.drive_sync.lock().map(|s| s.running).unwrap_or(false);
        let onedrive = self
            .onedrive_sync
            .lock()
            .map(|s| s.running)
            .unwrap_or(false);
        drive || onedrive
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
            return Err(error::Error::Other("the vault is locked".into()));
        }
        Ok(DbGuard(guard))
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
        // Drop the guards before reconciling — it re-locks both to (re)build the rules file or
        // restore the mirror from it. Best-effort: a hiccup here never blocks the open. The
        // index-only manifest reconcile runs AFTER the rules one (it resolves item projects through
        // the rebuilt aliases).
        self.reconcile_entity_rules();
        self.reconcile_index_only();
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
            None => Err(error::Error::Other("the vault is locked".into())),
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
            None => Err(error::Error::Other("the vault is locked".into())),
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
            None => Err(error::Error::Other("the vault is locked".into())),
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
        if let Err(e) = index_only::reconcile_on_open(&conn, &vault_root, &cipher) {
            eprintln!("index_only: manifest reconcile skipped ({e})");
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
            eprintln!("index_only: manifest sync skipped ({e})");
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
    tauri::Builder::default()
        // Must be registered first. One instance only: a second launch (e.g. a
        // double-click or an updater relaunch overlap) focuses the running window
        // and exits, so two processes can't race to create the encrypted store
        // with different keys and orphan one of them (rule #2).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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
            let resolved = vault::resolve(handle)?;

            // GC abandoned restore staging before opening: each `restored-vaults/restore-*` is a full,
            // decryptable vault copy left by a restore-and-inspect. A copy the user didn't switch to
            // has an in-memory-only key that died with the last process, so it can never be reopened —
            // sweep every staged copy except the one this profile is actually pointed at. Best-effort;
            // a locked file just waits for the next boot. (F-25)
            if let Ok(data_dir) = paths::data_dir(handle) {
                wipe::sweep_restore_staging(&data_dir, &resolved.vault_root);
            }

            // Metadata exists from creation (device-mode on a fresh install; spec §6).
            // A device vault opens now with the keychain key; a passphrase/shareable
            // vault opens only if this profile cached its key, otherwise the store stays
            // locked (None) and the UI prompts to unlock before any DB command runs.
            let meta = vault::ensure_device_meta(&resolved.vault_root)?;
            // On a successful open we also get the Markdown cipher; pair it with the
            // resolved Markdown dir as the session's vault runtime. A locked
            // (passphrase, uncached) vault leaves both `None` until an unlock command.
            let (conn, vault_runtime, boot_open_error) = match vault::open_at_boot(&resolved, &meta)
            {
                Ok(Some((conn, master))) => (
                    Some(conn),
                    Some(VaultRuntime::build(&resolved, &meta, &master)),
                    None,
                ),
                Ok(None) => (None, None, None),
                // A boot-time open failure (a transient AV/search-indexer file lock, disk I/O)
                // must NOT abort the whole app — `db::open` already maps these to friendly,
                // retryable messages. Degrade to a not-opened store and carry the message so the
                // UI can offer Retry, exactly like a locked vault (B1-6). Only a genuinely
                // corrupt store then needs a manual fix, instead of every transient hiccup.
                Err(e) => {
                    let msg = e.to_string();
                    eprintln!("vault: boot open failed, starting locked with Retry: {msg}");
                    (None, None, Some(msg))
                }
            };

            // The sidecar source folder is optional at boot — chat works without
            // it; ingestion surfaces a clear error if it (or Python) is missing.
            let source_dir =
                paths::sidecar_source_dir(handle).unwrap_or_else(|_| PathBuf::from("sidecar"));
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
                layout_job: Mutex::new(layout::LayoutJobState::default()),
                last_user_activity: Mutex::new(Instant::now()),
                chat_index_busy: AtomicBool::new(false),
                summary_busy: AtomicBool::new(false),
                title_busy: AtomicBool::new(false),
                prefs_busy: AtomicBool::new(false),
                backup_state: Mutex::new(BackupState::default()),
                backup_cancel: AtomicBool::new(false),
                backup_busy: AtomicBool::new(false),
                pending_restore_keys: Mutex::new(std::collections::HashMap::new()),
                boot_open_error: Mutex::new(boot_open_error),
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::has_openrouter_key,
            commands::set_openrouter_key,
            commands::has_openrouter_background_key,
            commands::set_openrouter_background_key,
            commands::get_settings,
            commands::set_indexing_speed,
            commands::set_chat_models,
            commands::set_background_models,
            commands::set_chat_auto_switch,
            commands::set_background_auto_switch,
            commands::set_help_mode,
            commands::set_reranking,
            commands::set_retrieval_k,
            commands::retrieval_explain,
            commands::retrieval_diagnose,
            commands::language_options,
            commands::set_vault_embedder,
            commands::get_time_zone,
            commands::set_time_zone,
            commands::get_pref,
            commands::set_pref,
            commands::app_lock_status,
            commands::set_app_lock,
            commands::unlock_app,
            commands::vault_status,
            commands::retry_open_vault,
            commands::create_shareable_vault,
            commands::change_vault_passphrase,
            commands::make_vault_private,
            commands::move_vault,
            commands::unlock_vault,
            commands::open_existing_vault,
            commands::forget_vault_passphrase,
            commands::score_passphrase,
            commands::link_vault_account,
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
            commands::search_documents,
            commands::transcribe_audio,
            commands::list_projects,
            commands::review_queue,
            commands::review_queue_count,
            commands::propose_metadata,
            commands::commit_review,
            commands::set_document_metadata,
            commands::list_entities,
            commands::add_entity_alias,
            commands::rename_entity,
            commands::merge_entities,
            commands::reassign_document,
            commands::list_project_overviews,
            commands::set_project_metadata,
            commands::propose_project_metadata,
            commands::list_milestones,
            commands::add_milestone,
            commands::update_milestone,
            commands::set_milestone_event,
            commands::set_milestone_state,
            commands::delete_milestone,
            commands::reorder_milestones,
            commands::calendar_overview,
            commands::set_calendar_selected,
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
            commands::connect_drive,
            commands::disconnect_drive,
            commands::list_drive_accounts,
            commands::drive_status,
            commands::sync_drive,
            commands::drive_sync_status,
            commands::stop_drive_sync,
            commands::resume_drive_sync,
            commands::list_drive_shared_drives,
            commands::drive_shared_owners,
            commands::list_drive_folders,
            commands::get_drive_scope,
            commands::set_drive_scope,
            commands::onedrive_status,
            commands::list_onedrive_accounts,
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
            commands::sync_local_folder,
            commands::local_folder_sync_status,
            commands::stop_local_folder_sync,
            commands::resume_local_folder_sync,
            commands::fetch_index_only_body,
            commands::promote_index_only,
            layout::semantic_layout,
            layout::start_semantic_layout,
            layout::prioritise_semantic_layout,
            layout::optional_tsne_status,
            layout::install_optional_tsne,
            layout::uninstall_optional_tsne,
            photos::optional_ocr_status,
            photos::install_optional_ocr,
            components::list_storage_components,
            components::remove_storage_component,
            commands::open_source,
            commands::read_document_body,
            commands::document_chunk_spans,
            commands::read_document_image,
            commands::open_url,
            commands::get_daily_briefing,
            commands::refresh_daily_briefing,
            commands::resolve_flag,
            commands::route_focus_input,
            commands::cost_summary,
            commands::refresh_pricing,
            commands::model_recommendations,
            commands::set_recommend_denylist,
            commands::open_data_folder,
            commands::export_all_data,
            commands::export_plaintext_markdown,
            // "Remove PM data" teardown (Settings → Data & Security) — the à-la-carte counterpart to
            // the Windows uninstaller's automatic `runtime/` cleanup.
            wipe::wipe_pm_data,
            wipe::confirm_wipe_identity,
            // Encrypted portable backup: local `.pmbackup` archive/restore, plus two off-machine
            // destinations that share the compress→encrypt core and the one schedule — Proton Drive
            // (via its CLI) and Google Drive (via the Drive v3 REST API).
            commands::create_local_backup,
            commands::restore_local_backup,
            commands::switch_to_vault,
            commands::backup_status,
            commands::stop_backup,
            commands::proton_cli_status,
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
            // Developer mode (issue #78) — read-only inspection. Always registered (the
            // commands are harmless reads); only the UI is gated by the runtime `devMode`.
            commands_dev::dev_system_info,
            commands_dev::dev_table_counts,
            commands_dev::dev_table_list,
            commands_dev::dev_table_rows,
            commands_dev::dev_document_chunks,
            commands_dev::dev_retrieval_explain,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
