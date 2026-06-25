// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

mod applock;
mod briefing;
mod calendar;
mod clock;
mod commands;
mod cost;
mod db;
mod entities;
mod error;
mod google;
mod ics;
mod ingest;
mod learning;
mod lock_session;
mod model_gateway;
mod openrouter;
mod paths;
mod projects;
mod recommend;
mod registry;
mod retrieval;
mod retrieval_config;
mod review;
mod secret;
mod secrets;
mod sidecar;
mod splitter;
mod vault;

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard};

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
        }
    }
}

/// Shared app state. The SQLite connection is guarded by a mutex; commands lock
/// it only for short synchronous work, never across an `.await`. The sidecar
/// manages its own interior locking.
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
        // restore the mirror from it. Best-effort: a hiccup here never blocks the open.
        self.reconcile_entity_rules();
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
        // A policy/key transition swaps the rules cipher too; reconcile heals the file under the
        // new key (the mirror survived the SQLCipher rekey intact). Best-effort.
        self.reconcile_entity_rules();
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
            // If a vault migration was interrupted, repair it before anything opens the
            // store: roll the in-place phase back to its backup, or finish/discard a
            // partial move. A no-op when no migration journal is present.
            vault::migrate::recover(handle)?;
            // Resolve where this profile's vault lives — pointer-aware, but defaulting
            // to the per-profile data dir when no pointer is set (today's behaviour).
            let resolved = vault::resolve(handle)?;

            // Metadata exists from creation (device-mode on a fresh install; spec §6).
            // A device vault opens now with the keychain key; a passphrase/shareable
            // vault opens only if this profile cached its key, otherwise the store stays
            // locked (None) and the UI prompts to unlock before any DB command runs.
            let meta = vault::ensure_device_meta(&resolved.vault_root)?;
            // On a successful open we also get the Markdown cipher; pair it with the
            // resolved Markdown dir as the session's vault runtime. A locked
            // (passphrase, uncached) vault leaves both `None` until an unlock command.
            let (conn, vault_runtime) = match vault::open_at_boot(&resolved, &meta)? {
                Some((conn, master)) => (
                    Some(conn),
                    Some(VaultRuntime::build(&resolved, &meta, &master)),
                ),
                None => (None, None),
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
            });

            // Engage the cooperative writer lock for a shared vault (acquire it, or step
            // back behind another live profile), then run the heartbeat + hand-off watcher
            // for the life of the app. A no-op for a device-only vault.
            lock_session::engage(handle)?;
            lock_session::spawn_watcher(handle.clone());

            // After the writer lock is settled (so a stepped-back profile doesn't write), reconcile
            // the encrypted entity-rules file with the DB mirror: first run writes it from the v10
            // backfill; later runs rebuild the mirror from it. No-op when the vault is locked.
            app.state::<AppState>().reconcile_entity_rules();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::has_openrouter_key,
            commands::set_openrouter_key,
            commands::has_openrouter_background_key,
            commands::set_openrouter_background_key,
            commands::get_settings,
            commands::set_chat_models,
            commands::set_background_models,
            commands::set_chat_auto_switch,
            commands::set_background_auto_switch,
            commands::set_help_mode,
            commands::set_reranking,
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
            commands::create_shareable_vault,
            commands::change_vault_passphrase,
            commands::make_vault_private,
            commands::move_vault,
            commands::unlock_vault,
            commands::open_existing_vault,
            commands::forget_vault_passphrase,
            commands::link_vault_account,
            commands::vault_lock_status,
            commands::continue_here,
            commands::force_take_vault,
            commands::list_models,
            commands::get_learning_profile,
            commands::refresh_learning_profile,
            commands::list_conversations,
            commands::create_conversation,
            commands::get_messages,
            commands::send_message,
            commands::sidecar_status,
            commands::ensure_sidecar,
            commands::ingest_paths,
            commands::rebuild_index,
            commands::list_documents,
            commands::search_documents,
            commands::transcribe_audio,
            commands::list_projects,
            commands::review_queue,
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
            commands::calendar_status,
            commands::list_ics_feeds,
            commands::add_ics_feed,
            commands::remove_ics_feed,
            commands::set_google_client,
            commands::clear_google_client,
            commands::connect_google,
            commands::disconnect_google,
            commands::list_google_calendars,
            commands::set_google_calendar_ids,
            commands::sync_calendar,
            commands::list_calendar_events,
            commands::get_daily_briefing,
            commands::refresh_daily_briefing,
            commands::cost_summary,
            commands::refresh_pricing,
            commands::model_recommendations,
            commands::set_recommend_denylist,
            commands::open_data_folder,
            commands::export_all_data,
            commands::export_plaintext_markdown,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
