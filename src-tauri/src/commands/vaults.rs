// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vault itself: status, passphrase and location changes, sharing/adopting a shared
//! vault, repair, and the per-device lock session.

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::blocking::spawn_blocking_result;
use crate::error::{Error, Result, VaultFault, VaultFaultCode};
use crate::{lock_session, pathguard, paths, secrets, vault, AppState, VaultRuntime};

// --- vault (shareable / portable) ---

/// Build the session's Markdown runtime for a freshly opened vault: the resolved
/// Markdown dir plus the policy-aware cipher (derived from the master that the DB key
/// hex carries). Shared by unlock and open-existing so both install an identical runtime.
fn vault_runtime_for(
    resolved: &vault::ResolvedVault,
    meta: &vault::VaultMeta,
    key_hex: &str,
) -> Result<VaultRuntime> {
    let master = vault::master_from_db_key_hex(key_hex)?;
    Ok(VaultRuntime::build(resolved, meta, &master))
}

/// What the frontend needs to decide whether to show the unlock screen and how to
/// label the vault. Non-secret: mode, whether the store is currently locked, whether
/// Markdown is encrypted at rest, the vault location, and the stable vault id.
#[derive(Serialize)]
pub struct VaultStatus {
    pub mode: vault::KeyMode,
    pub needs_unlock: bool,
    pub markdown_encrypted: bool,
    pub location: String,
    pub vault_id: Option<String>,
    /// Whether the stored index was produced by a different retrieval config than this build
    /// (a model, chunk-rule, or splitter change) — i.e. a one-time Rebuild is recommended. The
    /// Documents view surfaces this as a dismissible banner. False when the vault is locked or
    /// has no documents yet.
    pub retrieval_rebuild_needed: bool,
    /// Why the store is unavailable beyond needing an unlock — a classified fault (boot open
    /// failure, denied/gone pointed root, mid-session access loss), or `None` in the normal
    /// case. The UI branches on `fault.code`: Denied gets Repair access, NoVault/NotFound get
    /// the honest gone-folder story, everything else the Retry surface. Replaces the old
    /// string-only `open_error`.
    pub fault: Option<VaultFault>,
    /// The folder this profile's pointer names, when one is set (a moved or joined vault).
    /// Lets the UI offer "detach back to a local vault" when that folder stops answering.
    pub pointed_root: Option<String>,
    /// Whether a vault already sits at this profile's DEFAULT location while a pointer
    /// redirects elsewhere — i.e. a joiner's set-aside vault. Drives the detach confirm's
    /// copy: "switch back to the set-aside vault" vs "start a new, empty vault".
    pub has_set_aside_vault: bool,
    /// A shared folder this profile detached from whose vault still answers (or is merely
    /// access-denied — repairable), so Settings can offer "Rejoin …". `None` when never
    /// detached, or when the folder no longer holds a vault (the offer self-heals away).
    pub retired_root: Option<String>,
    /// Set when the shared vault this profile points at was DELETED by its owner (a tombstone
    /// marks the folder) — the folder, and when it was deleted. The UI shows a one-time notice
    /// and switches back to a local vault, instead of the generic "couldn't open" screen.
    pub deleted_notice: Option<DeletedVaultNotice>,
    /// Whether the CURRENT Windows account owns the active vault. True for a device vault or a legacy
    /// shared vault (no owner recorded); a shared vault stamped with an owner SID is owned only by its
    /// creator's account, so a joiner sees `false`. Lets the UI present connectors as owner-managed.
    ///
    /// Fails OPEN on purpose — see [`vault::is_vault_owner`]. For anything destructive read
    /// `ownership` instead, which keeps "nobody recorded an owner" apart from "this is ours".
    pub is_owner: bool,
    /// Who owns the active vault, with "unknown" told apart from "ours" — the distinction `is_owner`
    /// folds away and a delete button needs. Drives hiding "Delete shared vault" for a joiner, and
    /// warning on it when ownership can't be established.
    pub ownership: vault::VaultOwnership,
    /// The last CONFIRMED ownership takeover this vault records, if any — from/to SID and when. Read
    /// by the Vault card, which states it in one line. The field exists so the record is not
    /// write-only: `prepare_shareable` writes it under the meta MAC, and a stored field nothing
    /// displays is a record nobody can act on.
    pub ownership_transfer: Option<vault::OwnershipTransfer>,
    /// "This vault's settings file was altered outside PM", when the last open said so. The same
    /// sentence `vault://meta-warning` carries — repeated here because the boot open happens before
    /// any webview is listening, and because the condition now PERSISTS: a failed integrity check is
    /// no longer re-signed away on the next launch.
    pub meta_warning: Option<String>,
}

/// The joiner-facing record that a pointed shared vault was deleted by its owner (from the
/// discovery tombstone). Drives the one-time "switched you back to your own vault" notice.
#[derive(Serialize)]
pub struct DeletedVaultNotice {
    pub folder: String,
    /// RFC3339; the UI formats it (DD-MM-YYYY).
    pub deleted_at: Option<String>,
}

/// Non-fatal warnings from a vault operation (a folder-ACL or discovery-marker hiccup),
/// for the UI to surface without failing the operation — encryption still protects the
/// vault when these fire.
#[derive(Serialize)]
pub struct VaultOpOutcome {
    pub warnings: Vec<String>,
}

/// What `adopt_shared_vault` tells the joining UI: whether this instance came up as the
/// active writer (false ⇒ the other account holds the baton and the curtain shows), plus
/// any non-fatal warnings.
#[derive(Serialize)]
pub struct AdoptOutcome {
    pub active_writer: bool,
    pub warnings: Vec<String>,
}

/// Report the current vault's mode and whether it needs unlocking (a passphrase vault
/// whose key isn't cached in this profile yet).
#[tauri::command]
pub fn vault_status(app: AppHandle, state: State<'_, AppState>) -> Result<VaultStatus> {
    // Resolve tolerantly: a POINTED root that stopped answering (access revoked, folder
    // deleted) must still yield a status — with `open_error` carrying the boot detail and
    // `pointed_root` naming the folder — rather than an error that leaves the UI blind.
    let data_dir = paths::data_dir(&app)?;
    let pointer = vault::pointer::load(&data_dir).ok().flatten();
    let resolved = vault::resolve_layout(&data_dir, pointer.as_ref());
    let meta = vault::load_meta(&resolved.vault_root).ok().flatten();
    let (mode, meta_says_encrypted, vault_id) = match &meta {
        Some(m) => (
            m.key_mode,
            m.markdown.encryption != vault::MarkdownEncryption::None,
            Some(m.vault_id.clone()),
        ),
        None => (vault::KeyMode::Device, false, None),
    };
    // Report the cipher that is actually WRITING, not the policy the file claims. The two can now
    // disagree: `MarkdownCipher::from_meta` refuses to honour a metadata downgrade it can contradict,
    // and a failed integrity check is no longer repaired on disk — so reading the claim here would
    // tell the user their notes are in the clear at the exact moment PM is encrypting them.
    let markdown_encrypted = state
        .markdown_io()
        .map(|(_, cipher)| cipher.encryption_on())
        .unwrap_or(meta_says_encrypted);
    // A populated vault whose stored index was produced by a different retrieval config than
    // this build (a model/chunk/splitter change, or a pre-stamp vault) gets a one-time Rebuild
    // prompt. Only meaningful when the store is open and has documents.
    let retrieval_rebuild_needed = if state.is_unlocked() {
        let conn = state.conn()?;
        let has_docs: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM documents)", [], |r| r.get(0))?;
        // Compare against what this build would produce for *this vault's* embedder, so a
        // multilingual vault isn't wrongly flagged stale against the English default.
        let current = crate::retrieval_config::RetrievalConfig::current_for(
            &crate::db::selected_embedder(&conn)?,
        );
        has_docs && crate::db::get_retrieval_stamp(&conn)?.as_ref() != Some(&current)
    } else {
        false
    };
    // A set-aside vault = metadata already at the DEFAULT location while a pointer redirects
    // elsewhere (a joiner's own vault, parked by the adopt). Only meaningful when pointed.
    let has_set_aside_vault =
        pointer.is_some() && matches!(vault::load_meta(&data_dir), Ok(Some(_)));
    // The rejoin offer, probed so it self-heals: a retired folder that still answers with a
    // vault (or is merely denied — repairable) keeps the offer; one that no longer holds a
    // vault drops out. The record itself is kept — a drive that comes back re-offers.
    let retired_root = vault::pointer::load_retired(&data_dir)
        .ok()
        .flatten()
        .filter(|r| !matches!(vault::load_meta(&r.vault_root), Ok(None)))
        .map(|r| r.vault_root.to_string_lossy().into_owned());
    // A pointed folder that no longer holds a vault AND is tombstoned = the owner deleted it.
    // Only checked when we're pointed at a folder that isn't currently answering (no meta),
    // so a live shared vault never triggers the notice. Matched by PATH (the id is unreadable
    // once the folder is gone). Discovery is Windows-only, so this is `None` elsewhere.
    let deleted_notice = pointer.as_ref().filter(|_| meta.is_none()).and_then(|p| {
        let ads = vault::advert::ads_dir().map(|d| vault::advert::list(&d))?;
        vault::advert::deletion_tombstone_for(&ads, &p.vault_root).map(|ad| DeletedVaultNotice {
            folder: p.vault_root.to_string_lossy().into_owned(),
            deleted_at: ad.deleted_at.clone(),
        })
    });
    Ok(VaultStatus {
        mode,
        needs_unlock: !state.is_unlocked(),
        markdown_encrypted,
        location: resolved.vault_root.to_string_lossy().into_owned(),
        vault_id,
        retrieval_rebuild_needed,
        fault: state.vault_fault(),
        pointed_root: pointer.map(|p| p.vault_root.to_string_lossy().into_owned()),
        has_set_aside_vault,
        retired_root,
        deleted_notice,
        is_owner: meta.as_ref().map(vault::is_vault_owner).unwrap_or(true),
        ownership: meta
            .as_ref()
            .map(vault::vault_ownership)
            .unwrap_or(vault::VaultOwnership::Device),
        ownership_transfer: meta.as_ref().and_then(|m| m.ownership_transfer.clone()),
        meta_warning: state.meta_warning(),
    })
}

/// Reject a connector-setup action when the current account doesn't own the (shared) vault. Owner-only
/// connectors: OAuth tokens live in the per-Windows-account keychain, so a joiner literally cannot sync
/// an account they connect — gating the setup replaces the opaque "connection fails" with an honest
/// message. Fails OPEN on a device / legacy vault (no owner recorded) and if the meta can't be read, so
/// it never blocks the real owner. Windows-only ownership; a no-op everywhere else.
pub(super) fn require_vault_owner(app: &AppHandle) -> Result<()> {
    let is_owner = vault::resolve(app)
        .ok()
        .and_then(|r| vault::load_meta(&r.vault_root).ok().flatten())
        .map(|m| vault::is_vault_owner(&m))
        .unwrap_or(true);
    if is_owner {
        Ok(())
    } else {
        Err(Error::Other(
            "Connectors on a shared vault are set up by its owner on this PC. Ask the vault's owner to \
             connect this account — you'll still see everything they index."
                .into(),
        ))
    }
}

/// Retry opening the store after a transient boot-time open failure (B1-6). Re-runs the
/// boot open path; on success installs the session and clears the carried error, so the UI's
/// Retry surface unmounts and the app proceeds. A now-locked passphrase vault (key not
/// cached) clears the error too and falls through to the unlock prompt. A still-failing open
/// re-arms the error and returns it, so the surface shows the fresh message.
#[tauri::command]
pub fn retry_open_vault(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    // The user asked for this, so spend a fresh keychain read if the last one failed. Without it a
    // denied macOS consent prompt latched the secret cache `Untrusted` for the life of the process:
    // every Retry then failed identically — the boot open path never reached the keychain again —
    // while the error message told the user to unlock their keychain and choose Retry. Scoped to
    // this one command so the ~40 incidental boot accessors still get a single attempt between them.
    secrets::rearm_for_retry();
    let resolved = vault::resolve(&app)?;
    let meta = vault::load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
    match vault::open_at_boot(&resolved, &meta) {
        Ok(Some((conn, master, report))) => {
            // open_session clears the carried fault (the one healing choke point).
            state.open_session(conn, VaultRuntime::build(&resolved, &meta, &master))?;
            state.note_meta_report(&report);
            // Re-engage the cooperative writer lock now the store is open again.
            lock_session::engage(&app)?;
            Ok(())
        }
        Ok(None) => {
            // Now merely locked (key not cached) — the unlock prompt takes it from here.
            state.set_vault_fault(None);
            Ok(())
        }
        Err(e) => {
            // Re-arm with the fresh story so the surface shows the current failure.
            state.set_vault_fault(Some(VaultFault::from_error("open the vault", &e)));
            Err(e)
        }
    }
}

/// Convert this profile's device vault into a shareable, passphrase-protected one —
/// and, when `target_location` is given, move it to that (cross-account-reachable)
/// folder in the SAME crash-recoverable migration. The guided share flow always passes
/// a location: a shareable vault left inside the per-user profile folder is unreachable
/// by every other account, which is exactly the trap this closes. The device-only
/// default is untouched for users who never opt in; changing an existing passphrase is
/// `change_vault_passphrase`.
#[tauri::command]
pub async fn create_shareable_vault(
    app: AppHandle,
    passphrase: String,
    target_location: Option<String>,
) -> Result<VaultOpOutcome> {
    // I-03: hold the passphrase in a Zeroizing so its plaintext is wiped from memory on return
    // (every derived key is already Zeroizing; the raw passphrase was the gap).
    let passphrase = zeroize::Zeroizing::new(passphrase);
    if passphrase.trim().is_empty() {
        return Err(Error::Other("a passphrase is required".into()));
    }
    // M-4: enforce the strength floor here in the command layer — a shareable vault's Markdown is
    // reachable by other accounts, so a weak passphrase is a real exposure. Create/change only.
    vault::kdf::validate_passphrase_strength(&passphrase)?;
    // L-5: if the caller chose a destination, validate it (webview-supplied) before we move the
    // encrypted store there. `None` keeps the vault in place.
    if let Some(loc) = &target_location {
        pathguard::sanitize_destination(loc)?;
    }
    {
        let meta = vault::load_meta(&vault::resolve(&app)?.vault_root)?
            .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
        if meta.key_mode == vault::KeyMode::Passphrase {
            return Err(Error::Other("this vault is already shareable".into()));
        }
    }
    let plan = vault::migrate::MigrationPlan {
        target_key_mode: vault::KeyMode::Passphrase,
        new_passphrase: Some(passphrase),
        target_markdown: vault::MarkdownEncryption::XChaCha20Poly1305,
        target_location: target_location.map(std::path::PathBuf::from),
        // Device -> Passphrase. `Keep` is guarded on the SOURCE mode inside `prepare_shareable`, so a
        // device vault has nothing to keep and this account is stamped as the creator, as it always was.
        owner: vault::OwnerOnRekey::Keep,
    };
    let app2 = app.clone();
    let mut warnings = spawn_blocking_result("migration", move || {
        vault::migrate::migrate_vault(&app2, plan)
    })
    .await?;
    engage_or_warn(&app, &mut warnings);
    Ok(VaultOpOutcome { warnings })
}

/// Re-engage the writer lock after a migration, demoting a failure to a WARNING: by this
/// point the migration has already committed, so erroring here would misreport a successful
/// transition as failed (and the verify-then-commit relocate already probes the folder's
/// writability before committing, so `engage`'s real failure mode can't reach here anyway).
pub(super) fn engage_or_warn(app: &AppHandle, warnings: &mut Vec<String>) {
    if let Err(e) = lock_session::engage(app) {
        warnings.push(format!(
            "PM couldn't re-engage its shared-vault coordination ({e}) — restart PM before \
             using the vault from another account."
        ));
    }
}

/// Change a shareable vault's passphrase: re-derive the key (new salt + verifier),
/// re-key the store, and re-encrypt the Markdown under the new subkey — one atomic,
/// crash-recoverable migration. Only valid for an already-shareable vault.
///
/// **Refused for a vault another account on this PC created, unless the caller confirms a takeover.**
/// A re-key mints whole new metadata, so before [`vault::OwnerOnRekey`] existed it also re-stamped the
/// rotating account as owner — any joiner became the recorded owner with one button press and no
/// notice to anyone. The ownership decision is made HERE, before a key is derived and long before
/// `store_meta` commits it (`vault-acl-verify-then-commit`), and `confirm_ownership_transfer` is the
/// hatch that keeps the refusal recoverable: an owner whose SID changed, or a vault whose owner has
/// left, is one confirmed rotation away from working again. It is a webview-supplied boolean, so it is
/// a speed bump against ACCIDENT and not an authorization control — anyone who can unlock the vault
/// can send `true`. What it buys is that a takeover is deliberate, recorded and visible instead of
/// silent, and (via `Keep`) that no other path can claim ownership as a side effect at all.
#[tauri::command]
pub async fn change_vault_passphrase(
    app: AppHandle,
    new_passphrase: String,
    confirm_ownership_transfer: bool,
) -> Result<VaultOpOutcome> {
    // I-03: wipe the passphrase plaintext from memory on return.
    let new_passphrase = zeroize::Zeroizing::new(new_passphrase);
    if new_passphrase.trim().is_empty() {
        return Err(Error::Other("a passphrase is required".into()));
    }
    // M-4: strength floor on the new passphrase (create/change only — the unlock path is untouched).
    vault::kdf::validate_passphrase_strength(&new_passphrase)?;
    let owner = {
        let meta = vault::load_meta(&vault::resolve(&app)?.vault_root)?
            .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
        if meta.key_mode != vault::KeyMode::Passphrase {
            return Err(Error::Other(
                "this vault has no passphrase; make it shareable first".into(),
            ));
        }
        match vault::gate_for(vault::vault_ownership(&meta), confirm_ownership_transfer) {
            vault::RekeyGate::Refuse => {
                return Err(Error::Other(
                    "This shared vault was created by another account on this PC. Changing the \
                     passphrase locks every other account out until you give them the new one, and \
                     makes this account the vault's owner. Ask its owner to change it — or confirm \
                     you're taking the vault over."
                        .into(),
                ));
            }
            vault::RekeyGate::Allow(owner) => owner,
        }
    };
    let plan = vault::migrate::MigrationPlan {
        target_key_mode: vault::KeyMode::Passphrase,
        new_passphrase: Some(new_passphrase),
        target_markdown: vault::MarkdownEncryption::XChaCha20Poly1305,
        target_location: None,
        owner,
    };
    let app2 = app.clone();
    let mut warnings = spawn_blocking_result("migration", move || {
        vault::migrate::migrate_vault(&app2, plan)
    })
    .await?;
    engage_or_warn(&app, &mut warnings);
    Ok(VaultOpOutcome { warnings })
}

/// Whether making a vault private must first move it back into this profile's own folder
/// before decrypting — true when the vault currently lives OUTSIDE the data dir (a shared
/// location). Decrypting in place there would briefly write plaintext notes into a folder
/// other accounts can reach; moving home first keeps plaintext to the OS-isolated profile
/// dir. Pure, so the decision unit-tests.
fn needs_move_home(vault_root: &std::path::Path, data_dir: &std::path::Path) -> bool {
    !vault_root.starts_with(data_dir)
}

/// Make a shareable vault private again: re-key it to a random device key (held only in
/// this profile's keychain) and decrypt the Markdown back to plaintext. A vault that lives
/// in a shared folder is FIRST moved back into this profile's own (OS-isolated) folder —
/// still encrypted — so the decrypt never writes plaintext where another account could read
/// it. Also withdraws the discovery marker and linked-accounts sidecar (inside the
/// migration). Reverses `create_shareable_vault`; a no-op-style error if already device-only.
///
/// **Refused outright for a vault another account on this PC created, with no hatch** — the one place
/// in the vault surface where that is the right answer. This is a strictly worse takeover than a
/// passphrase change: it re-keys the vault to THIS account's device key (held in this keychain
/// alone), decrypts the Markdown, and moves the shared folder into this profile's directory. Where a
/// confirmed re-key leaves the displaced owner a passphrase that still opens their data, there is no
/// passphrase here — nothing gets them back in. So it mirrors `delete_shared_vault`'s refusal exactly
/// and points at leaving instead, which is non-destructive, reversible, and the very affordance the
/// neighbouring move-home error already names.
#[tauri::command]
pub async fn make_vault_private(app: AppHandle) -> Result<VaultOpOutcome> {
    let data_dir = paths::data_dir(&app)?;
    let resolved = vault::resolve(&app)?;
    let meta = vault::load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
    if meta.key_mode == vault::KeyMode::Device {
        return Err(Error::Other(
            "this vault is already private to this device".into(),
        ));
    }
    // Before anything is derived, moved or decrypted. `Unknown` falls open here for the same reason
    // it does on the re-key (see `vault::private_gate_for`): it is every shared vault off Windows and
    // every pre-ownership vault, and refusing there would strand a genuine owner.
    if vault::private_gate_for(vault::vault_ownership(&meta)) == vault::RekeyGate::Refuse {
        return Err(Error::Other(
            "This shared vault is recorded as belonging to another account on this PC, so it isn't \
             yours to make private — doing so would re-key it to this account alone and move it out \
             of the shared folder, with no way back in for anyone else. If you joined it, leave it \
             with \"Use a vault on this account instead\": it stays exactly where it is for everyone \
             still using it. If it IS yours and the recorded owner is stale — you moved domain, or \
             the account was recreated — change the passphrase first and confirm the ownership \
             transfer; that records you as the owner, and Make private then works."
                .into(),
        ));
    }
    let mut warnings = Vec::new();

    // Move home first if the vault is in a shared folder — two individually crash-safe
    // journaled migrations; a crash between them leaves a valid shareable-in-profile vault
    // that a re-run finishes. The home slot must be free: a joiner whose own vault is parked
    // there detaches instead of making someone else's shared vault private.
    if needs_move_home(&resolved.vault_root, &data_dir) {
        match vault::migrate::relocation_target_state(&vault::load_meta(&data_dir), &meta.vault_id)
        {
            vault::migrate::TargetState::ForeignVault | vault::migrate::TargetState::Unreadable => {
                return Err(Error::Other(
                    "this account already has its own vault here — leave the shared vault with \
                     \"Use a vault on this account instead\" rather than making it private"
                        .into(),
                ));
            }
            _ => {}
        }
        // A pure relocate to the profile root, keeping the passphrase key + encryption. The
        // move-home target IS inside the data dir, so the migration's lockdown/pre-flight
        // are correctly skipped (they gate on `!starts_with(data_dir)`) — no icacls touches
        // the profile folder.
        let move_plan = vault::migrate::MigrationPlan {
            target_key_mode: vault::KeyMode::Passphrase,
            new_passphrase: None,
            target_markdown: vault::MarkdownEncryption::XChaCha20Poly1305,
            target_location: Some(data_dir.clone()),
            owner: vault::OwnerOnRekey::Keep,
        };
        let app2 = app.clone();
        let move_warnings = spawn_blocking_result("migration", move || {
            vault::migrate::migrate_vault(&app2, move_plan)
        })
        .await?;
        warnings.extend(move_warnings);
    }

    // Decrypt in place at the (now-local) root.
    let plan = vault::migrate::MigrationPlan {
        target_key_mode: vault::KeyMode::Device,
        new_passphrase: None,
        target_markdown: vault::MarkdownEncryption::None,
        target_location: None,
        // A Device target never reaches `prepare_shareable`: `private_meta` builds the new meta and
        // clears the owner (and any transfer record) itself.
        owner: vault::OwnerOnRekey::Keep,
    };
    let app2 = app.clone();
    let decrypt_warnings = spawn_blocking_result("migration", move || {
        vault::migrate::migrate_vault(&app2, plan)
    })
    .await?;
    warnings.extend(decrypt_warnings);

    // The vault is now a device vault at the default location; clear the pointer so it's the
    // plain no-pointer default (the invariant `boot_meta_decision` branches on). Idempotent —
    // a no-op when the vault was already local.
    vault::pointer::clear(&data_dir)?;
    engage_or_warn(&app, &mut warnings);
    Ok(VaultOpOutcome { warnings })
}

/// Move the vault to a new folder (e.g. a shared location), keeping its key and Markdown
/// policy unchanged. Copy-verify-delete with the pointer flipped last, so an interrupted
/// move leaves the vault safely at its current location. Refuses a folder that already
/// holds a DIFFERENT vault (the collision guard in the migration) — join that one instead.
#[tauri::command]
pub async fn move_vault(app: AppHandle, folder: String) -> Result<VaultOpOutcome> {
    // L-5: `folder` is a webview-supplied destination — validate its shape and that its containing
    // folder exists before we relocate the whole encrypted store into it.
    pathguard::sanitize_destination(&folder)?;
    let target = std::path::PathBuf::from(folder);
    let plan = {
        let meta = vault::load_meta(&vault::resolve(&app)?.vault_root)?
            .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
        vault::migrate::MigrationPlan {
            target_key_mode: meta.key_mode,
            new_passphrase: None,
            target_markdown: meta.markdown.encryption,
            target_location: Some(target),
            // A pure move: with no new passphrase the plan clones the old meta wholesale, owner and
            // all, so this never reaches `prepare_shareable`.
            owner: vault::OwnerOnRekey::Keep,
        }
    };
    let app2 = app.clone();
    let mut warnings = spawn_blocking_result("migration", move || {
        vault::migrate::migrate_vault(&app2, plan)
    })
    .await?;
    engage_or_warn(&app, &mut warnings);
    Ok(VaultOpOutcome { warnings })
}

/// Surface a non-blocking warning to the UI when the vault meta was repaired on open (M-3): a
/// silently-downgraded Markdown-encryption policy that PM forced back on, or a failed integrity check.
///
/// Also parks it in [`AppState::meta_warning`], so the notice survives a dismissal-by-reload and so
/// the boot path — which has no webview listening yet — can report the same condition through
/// `vault_status` instead of stderr.
fn emit_vault_meta_warning(app: &AppHandle, state: &AppState, report: &vault::MetaAuthReport) {
    state.note_meta_report(report);
    if let Some(msg) = report.warning() {
        let _ = app.emit("vault://meta-warning", msg);
    }
}

/// Unlock the current (passphrase) vault: derive + verify, open the store, and cache
/// the derived key in this profile so the next launch is silent. The cache is best-effort —
/// nothing in this session reads it back (see below).
#[tauri::command]
pub fn unlock_vault(app: AppHandle, state: State<'_, AppState>, passphrase: String) -> Result<()> {
    // I-03: wipe the passphrase plaintext from memory on return.
    let passphrase = zeroize::Zeroizing::new(passphrase);
    let resolved = vault::resolve(&app)?;
    let meta = vault::load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata to unlock".into()))?;
    let (conn, key, meta_report) = vault::open_with_passphrase(&resolved, &meta, &passphrase)?;
    // Cache-first is deliberate, and matches `adopt_shared_vault` and every migration: a cache
    // failure costs one passphrase prompt next launch, never a failed unlock. It used to be a `?`
    // — so a broken OS credential store (Credential Manager disabled, no Secret Service) meant the
    // CORRECT passphrase opened the store and then threw the connection away, locking the user out
    // of all their data until they repaired an OS service the error didn't name. The store is
    // already open at this point; nothing below reads the cache back.
    //
    // NOTE for the next reader: the identical-looking `?` in `switch_to_vault` is CORRECT and must
    // stay — there the keychain write is load-bearing (the boot path reads it back) and it fails
    // safely, before the pointer commits.
    let mut cache_warning = None;
    if let Err(e) = secrets::set_cached_vault_key(&meta.vault_id, key.expose()) {
        cache_warning = Some(format!(
            "PM couldn't keep the key on this account ({e}) — you'll be asked for the \
             passphrase again next launch."
        ));
    }
    let runtime = vault_runtime_for(&resolved, &meta, key.expose())?;
    state.open_session(conn, runtime)?;
    // Now that the store is open, engage the cooperative writer lock for this vault.
    lock_session::engage(&app)?;
    // M-3: if the meta was repaired on open, tell the user (non-blocking).
    emit_vault_meta_warning(&app, &state, &meta_report);
    if let Some(msg) = cache_warning {
        let _ = app.emit("vault://meta-warning", msg);
    }
    Ok(())
}

/// Forget this profile's cached key for the current vault, so the passphrase is needed
/// again next launch. Does not lock the current session (the store stays open until exit).
#[tauri::command]
pub fn forget_vault_passphrase(app: AppHandle) -> Result<()> {
    let resolved = vault::resolve(&app)?;
    let meta = vault::load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
    // Only a passphrase vault has a passphrase to forget. Clearing the cache for a DEVICE
    // vault would be wrong: a restored/relocated device vault keeps its only key there, so
    // dropping it would leave the vault unopenable (it can't fall back to a passphrase).
    if meta.key_mode != vault::KeyMode::Passphrase {
        return Err(Error::Other(
            "this vault has no passphrase to forget".into(),
        ));
    }
    secrets::clear_cached_vault_key(&meta.vault_id)?;
    Ok(())
}

/// The strength of a candidate passphrase, for the create/change UI meter (M-4). Mirrors the backend
/// floor (`vault::kdf::validate_passphrase_strength`) so the hint the user sees and the gate that
/// actually blocks agree.
#[derive(serde::Serialize)]
pub struct PassphraseScore {
    /// zxcvbn strength, 0 (weakest) .. 4 (strongest).
    pub score: u8,
    /// True iff it clears the create/change floor (padding AND length AND score).
    pub acceptable: bool,
    /// Non-empty but below the length floor (so the UI can say "too short" specifically).
    pub too_short: bool,
    /// Starts or ends with whitespace, which create/change refuses (kdf.rs policy Rule 2) — so the
    /// meter can name the real problem instead of scoring bytes the backend will reject anyway.
    pub padded: bool,
    /// A short human warning when weak, else null.
    pub warning: Option<String>,
    /// Actionable suggestions to strengthen it.
    pub suggestions: Vec<String>,
}

/// Score a candidate passphrase for the UI strength meter, using the SAME zxcvbn model as the backend
/// floor (M-4). Never derives a key or unlocks anything — purely advisory; the command-layer floor is
/// the real check. The passphrase is zeroized on return and never logged.
#[tauri::command]
pub fn score_passphrase(passphrase: String) -> PassphraseScore {
    let passphrase = zeroize::Zeroizing::new(passphrase);
    let len = passphrase.chars().count();
    if len == 0 {
        return PassphraseScore {
            score: 0,
            acceptable: false,
            too_short: false,
            padded: false,
            warning: None,
            suggestions: Vec::new(),
        };
    }
    let estimate = zxcvbn::zxcvbn(&passphrase, &[]);
    let score = u8::from(estimate.score());
    let too_short = len < vault::kdf::MIN_PASSPHRASE_LEN;
    // Mirror validate_passphrase_strength's order and verdict exactly — this struct's whole purpose
    // is that the meter and the gate agree. A padded passphrase is unacceptable however strong it
    // scores, so the Save button it drives must not offer a submit the backend will refuse.
    let padded = passphrase.trim() != passphrase.as_str();
    let acceptable = !padded && !too_short && score >= vault::kdf::MIN_PASSPHRASE_SCORE;
    let (warning, suggestions) = match estimate.feedback() {
        Some(f) => (
            f.warning().map(|w| w.to_string()),
            f.suggestions().iter().map(|s| s.to_string()).collect(),
        ),
        None => (None, Vec::new()),
    };
    PassphraseScore {
        score,
        acceptable,
        too_short,
        padded,
        warning,
        suggestions,
    }
}

/// Grant another account on this machine access to the shared vault folder — the
/// Settings "link a second account" action. Takes an account name (e.g. `PC\alice`) or
/// a SID. Only a shareable vault that has actually MOVED out of this profile's private
/// folder can be linked — an ACE on a folder under the owner's profile is inert (other
/// accounts can't traverse the profile directories), which used to make this action
/// silently useless. The principal is persisted in the vault-access sidecar so a later
/// move re-applies it, and the discovery marker is refreshed. ACLs are defence in depth
/// (encryption is the real protection), so on platforms without support this surfaces
/// as a clear error the UI can show as a warning.
#[tauri::command]
pub fn link_vault_account(app: AppHandle, account: String) -> Result<VaultOpOutcome> {
    let resolved = vault::resolve(&app)?;
    let meta = vault::load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
    if meta.key_mode != vault::KeyMode::Passphrase {
        return Err(Error::Other(
            "only a shareable vault can be linked to another account; make it shareable first"
                .into(),
        ));
    }
    let data_dir = paths::data_dir(&app)?;
    if resolved.vault_root.starts_with(&data_dir) {
        return Err(Error::Other(
            "this vault still lives in your account's private folder, which other accounts \
             can never reach — move it to a shared location first (Share with other accounts)"
                .into(),
        ));
    }
    vault::acl::grant_access(&resolved.vault_root, &account)?;
    let mut warnings = Vec::new();
    // Read the grant back: a fail-loud `grant_access` already errored on a non-zero icacls,
    // but a readback catches the case where icacls reports success yet the ACE didn't land
    // (a resolvable-but-wrong principal). NotFound is a hard error (the link didn't take);
    // an inconclusive readback is only a warning — it must never fail a link that worked.
    match vault::acl::verify_grant(&resolved.vault_root, &account) {
        vault::acl::GrantCheck::Granted => {}
        vault::acl::GrantCheck::NotFound => {
            return Err(Error::Other(format!(
                "PM granted access to {account} but Windows didn't record it — check the \
                 account name or SID is exactly right and try again"
            )));
        }
        vault::acl::GrantCheck::Inconclusive(detail) => {
            // Names only actions that EXIST. This used to say "remove and re-add the account" —
            // but PM has no unlink, so the one instruction we handed the user at the one moment
            // they needed it pointed at a button nobody ever built. Adding again is idempotent,
            // and Repair access is the real tool when the folder itself stops answering.
            warnings.push(format!(
                "PM granted access to {account} but couldn't confirm it landed ({detail}). \
                 If they can't open the vault, add the account again — and if the folder \
                 itself stops opening, use Repair access."
            ));
        }
    }
    // Record the principal so a later move's owner-lockdown re-grants it (best-effort:
    // the ACE above is already applied either way).
    let mut access = vault::access::load(&resolved.vault_root, &meta.vault_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| vault::access::VaultAccess::new(&meta.vault_id));
    access.principals = vault::access::merge_principal(&access.principals, &account);
    if let Err(e) = vault::access::store(&resolved.vault_root, &access) {
        warnings.push(format!(
            "PM couldn't record the linked account ({e}) — if you move the vault later, \
             link this account again afterwards."
        ));
    }
    // Refresh the discovery marker so the linked account's fresh install gets the offer.
    if let Some(ads) = vault::advert::ads_dir() {
        let ad = vault::advert::SharedVaultAd::for_vault(&meta.vault_id, &resolved.vault_root);
        if let Err(e) = vault::advert::publish(&ads, &ad) {
            warnings.push(format!(
                "PM couldn't announce this vault to other accounts ({e}) — they can still \
                 join it by picking the folder by hand."
            ));
        }
    }
    Ok(VaultOpOutcome { warnings })
}

/// The shared vaults other accounts have advertised on this machine, filtered to ones
/// this profile could actually join (not its own vault; folders that still answer). An
/// unreadable folder is still offered — that's exactly the "owner hasn't linked this
/// account yet" case, and adopting surfaces the actionable error.
#[tauri::command]
pub fn list_shared_vaults(app: AppHandle) -> Result<Vec<vault::advert::SharedVaultAd>> {
    let Some(ads) = vault::advert::ads_dir() else {
        return Ok(Vec::new());
    };
    let data_dir = paths::data_dir(&app)?;
    let pointer = vault::pointer::load(&data_dir).ok().flatten();
    let resolved = vault::resolve_layout(&data_dir, pointer.as_ref());
    let current = vault::load_meta(&resolved.vault_root)
        .ok()
        .flatten()
        .map(|m| m.vault_id);
    Ok(vault::advert::filter_adoptable(
        vault::advert::list(&ads),
        current.as_deref(),
        // "Still standing" = anything except a readable folder with no vault in it; an
        // ACCESS-DENIED folder keeps its offer so the joiner gets the real error.
        |root| !matches!(vault::load_meta(root), Ok(None)),
    ))
}

/// Point this profile at `root` and install the freshly opened session: pointer first
/// (the commit — the next launch reads it), then the session swap, then the writer
/// lock. Shared by the backup-restore switch and the shared-vault adopt so the
/// attach sequence lives exactly once.
pub(super) fn attach_profile_here(
    app: &AppHandle,
    state: &AppState,
    root: std::path::PathBuf,
    conn: rusqlite::Connection,
    runtime: VaultRuntime,
) -> Result<()> {
    let data_dir = paths::data_dir(app)?;
    vault::pointer::store(&data_dir, &vault::pointer::VaultPointer::new(root))?;
    state.open_session(conn, runtime)?;
    lock_session::engage(app)?;
    Ok(())
}

/// Join an existing shared vault from THIS Windows account: validate the folder, unlock
/// it with the passphrase (verifier first, so a wrong passphrase errors cleanly), cache
/// the derived key so the next launch is silent, then point this profile at the folder.
/// The joiner's previous vault stays intact on disk — set aside, never deleted;
/// `detach_from_shared_vault` brings it back. No strength floor here: adopt is
/// unlock-family, and the passphrase already exists.
#[tauri::command]
pub fn adopt_shared_vault(
    app: AppHandle,
    state: State<'_, AppState>,
    folder: String,
    passphrase: String,
) -> Result<AdoptOutcome> {
    // I-03: wipe the passphrase plaintext from memory on return.
    let passphrase = zeroize::Zeroizing::new(passphrase);
    // L-5: `folder` is a webview string pointing at an existing shared-vault folder — require a
    // real, absolute, well-formed location before we read vault metadata from it.
    pathguard::sanitize_source(&folder)?;
    let root = std::path::PathBuf::from(&folder);
    let meta = match vault::load_meta(&root) {
        Ok(Some(m)) => m,
        Ok(None) => {
            return Err(Error::Vault(VaultFault {
                code: VaultFaultCode::NoVault,
                op: "join the shared vault".into(),
                path: Some(root.display().to_string()),
                message: "No PM vault was found in that folder — pick the folder that holds \
                          vault-meta.json and pm.sqlite."
                    .into(),
            }))
        }
        // A denied folder gets the joiner-persona story (the owner must add this account;
        // an owner locked out of their own folder repairs it instead) — and stays a
        // distinct code from wrong-passphrase, so "can't open the folder" is never read
        // as "passphrase not working" (the lockout incident's most damaging conflation).
        Err(e) => {
            let fault = VaultFault::from_error("join the shared vault", &e);
            let message = if fault.code == VaultFaultCode::Denied {
                format!(
                    "PM can't open that folder from this Windows account ({}). If someone \
                     shared it with you, they need to add this account first (their PM: \
                     Settings → Vault → Manage sharing). If it's yours, use Repair access.",
                    fault.message
                )
            } else {
                fault.message.clone()
            };
            return Err(Error::Vault(VaultFault { message, ..fault }));
        }
    };
    if meta.key_mode != vault::KeyMode::Passphrase {
        return Err(Error::Other(
            "that vault is private to its owner's account, so it can't be joined — they \
             can make it shareable first"
                .into(),
        ));
    }
    // The layout of a folder we are about to adopt is the same rule as our own, with no pointer to
    // consult — `resolve_layout` is pure (unlike `vault::resolve`, which creates directories).
    let resolved = vault::resolve_layout(&root, None);
    let (conn, key, meta_report) = vault::open_with_passphrase(&resolved, &meta, &passphrase)?;
    let mut warnings = Vec::new();
    // Cache-first is deliberate: a cache failure costs one passphrase prompt next
    // launch, never a failed adopt.
    if let Err(e) = secrets::set_cached_vault_key(&meta.vault_id, key.expose()) {
        warnings.push(format!(
            "PM couldn't keep the key on this account ({e}) — you'll be asked for the \
             passphrase again next launch."
        ));
    }
    let runtime = vault_runtime_for(&resolved, &meta, key.expose())?;
    // attach_profile_here → open_session clears any carried "vault unreachable" fault.
    attach_profile_here(&app, &state, root.clone(), conn, runtime)?;
    // A completed rejoin retires the breadcrumb: if this folder is the one the profile
    // once detached from, the "Rejoin …" offer has served its purpose. Best-effort.
    let data_dir = paths::data_dir(&app)?;
    if let Ok(Some(retired)) = vault::pointer::load_retired(&data_dir) {
        if retired.vault_root == root {
            let _ = vault::pointer::clear_retired(&data_dir);
        }
    }
    // M-3: if the meta was repaired on open, tell the user (non-blocking).
    emit_vault_meta_warning(&app, &state, &meta_report);
    Ok(AdoptOutcome {
        active_writer: lock_session::status(&app).active,
        warnings,
    })
}

/// Leave the shared vault: RETIRE this profile's pointer (keeping the folder on record
/// so Settings can offer a rejoin) and reopen the vault already at the default location
/// if one was set aside (a joiner's own vault) — otherwise a fresh, EMPTY one. That
/// empty case is real for an owner whose vault physically moved into the shared folder:
/// the shared copy is then the only copy, kept on disk untouched and rejoinable with the
/// passphrase. The UI confirms exactly which of the two the user is about to get before
/// calling this. This is the escape hatch when the shared vault stops answering (owner
/// revoked access, folder gone, vault made private).
#[tauri::command]
pub fn detach_from_shared_vault(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    let data_dir = paths::data_dir(&app)?;
    vault::pointer::retire(&data_dir)?;
    // Close whatever is open (possibly the shared store) before reopening locally.
    let _ = state.take_conn();
    let _ = state.clear_vault_runtime();
    // The unreachable-shared-vault story no longer applies — this profile walked away.
    state.set_vault_fault(None);
    let resolved = vault::resolve(&app)?;
    let meta = vault::ensure_device_meta(&resolved.vault_root)?;
    // A different vault from here on, so anything the old one was warning about no longer applies.
    state.clear_meta_warning();
    if let Some((conn, master, report)) = vault::open_at_boot(&resolved, &meta)? {
        state.open_session(conn, VaultRuntime::build(&resolved, &meta, &master))?;
        state.note_meta_report(&report);
    }
    lock_session::engage(&app)?;
    Ok(())
}

/// Switch this profile back to a vault on its own account after the shared vault it
/// pointed at was DELETED by its owner (the joiner-side acknowledgement of a tombstone).
/// Unlike detach, this does NOT retire the pointer for a later rejoin — the shared vault is
/// gone for good — and it drops this profile's cached key for it. Idempotent-ish: safe to
/// call even if the folder briefly reappears (the tombstone is the authority the UI acted
/// on). The set-aside local vault (a joiner's own) reopens, or a fresh empty one is minted.
#[tauri::command]
pub fn acknowledge_deleted_shared_vault(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    let data_dir = paths::data_dir(&app)?;
    // Drop this profile's cached key for the deleted vault before we forget which vault it
    // was (the pointer names the folder; the meta named the id — read it while we still can).
    if let Some(pointer) = vault::pointer::load(&data_dir)? {
        if let Ok(Some(meta)) = vault::load_meta(&pointer.vault_root) {
            let _ = secrets::clear_cached_vault_key(&meta.vault_id);
        }
    }
    vault::pointer::clear(&data_dir)?;
    vault::pointer::clear_retired(&data_dir)?;
    let _ = state.take_conn();
    let _ = state.clear_vault_runtime();
    state.set_vault_fault(None);
    let resolved = vault::resolve(&app)?;
    let meta = vault::ensure_device_meta(&resolved.vault_root)?;
    // A different vault from here on, so anything the old one was warning about no longer applies.
    state.clear_meta_warning();
    if let Some((conn, master, report)) = vault::open_at_boot(&resolved, &meta)? {
        state.open_session(conn, VaultRuntime::build(&resolved, &meta, &master))?;
        state.note_meta_report(&report);
    }
    lock_session::engage(&app)?;
    Ok(())
}

/// Owner-side deletion of a shared vault: remove the DB + artifacts from the shared folder,
/// leave a tombstone so every joined account learns it's gone at their next launch, and
/// switch THIS account back to a vault of its own. Distinct from make-private (which keeps
/// the data, just re-privatises it) and from detach (which leaves the shared copy intact) —
/// this is the deliberate "take the shared vault away from everyone" action.
///
/// **Refused when the vault records a different account as its owner.** The doc here used to say the
/// UI warns in that case; it never did, so any joined account got the same button with the same
/// confirmation and could take the vault away from everyone who used it. The gate cannot be airtight
/// — a joiner with write access to the folder can delete the files in Explorer regardless — but that
/// is not a reason for PM to hand them the button. A vault with no owner recorded is still allowed
/// through, so a genuine owner from before ownership existed, or one whose SID changed, is never
/// locked out of deleting their own vault; the UI warns there instead.
#[tauri::command]
pub fn delete_shared_vault(app: AppHandle, state: State<'_, AppState>) -> Result<VaultOpOutcome> {
    let data_dir = paths::data_dir(&app)?;
    let Some(pointer) = vault::pointer::load(&data_dir)? else {
        return Err(Error::Other(
            "this account isn't using a shared vault, so there's nothing to delete here".into(),
        ));
    };
    let root = pointer.vault_root;
    if root.starts_with(&data_dir) {
        return Err(Error::Other(
            "this vault lives in your own account's folder — use \"Make private\" or \"Remove \
             PM data\" instead of deleting a shared vault"
                .into(),
        ));
    }
    let meta = vault::load_meta(&root)?
        .ok_or_else(|| Error::Other("this folder no longer holds a PM vault".into()))?;
    if vault::vault_ownership(&meta) == vault::VaultOwnership::Joined {
        return Err(Error::Other(
            "This shared vault was created by another account on this machine, so it's theirs to \
             delete. You can leave it from here instead — the vault stays where it is for everyone \
             still using it."
                .into(),
        ));
    }
    let mut warnings = Vec::new();

    // Close our handle, then remove the vault from the shared folder. Reset any lockdown
    // first so the artifacts are deletable, then strip PM's files and drop the folder if it
    // was ours alone (leaving any unrelated files the user kept there).
    let _ = state.take_conn();
    let _ = state.clear_vault_runtime();
    // Release OUR writer lock before the sweep: `vault.lock` sits in the folder we are about to
    // empty, and delete_vault_artifacts deliberately spares it (it can't tell our lock from
    // another instance's). Held, it guaranteed the empty check below never passed — so the
    // "deleted" shared folder always survived holding blobs an ex-joiner could still read. The
    // tail re-engages on the local vault, and disengage is idempotent.
    lock_session::disengage(&app);
    let _ = vault::lock::release(&root, &state.instance_id);
    let _ = vault::acl::reset_inheritance(&root);
    vault::migrate::delete_vault_artifacts(&root);
    if let Ok(mut entries) = std::fs::read_dir(&root) {
        if entries.next().is_none() {
            let _ = std::fs::remove_dir(&root);
        }
    }

    // Leave the tombstone so joiners learn it was deleted (not merely unreachable), and drop
    // our own cached key for it. Both best-effort — the vault is already gone from disk.
    if let Some(ads) = vault::advert::ads_dir() {
        if let Err(e) = vault::advert::publish(
            &ads,
            &vault::advert::SharedVaultAd::tombstone(&meta.vault_id, &root),
        ) {
            warnings.push(format!(
                "PM removed the shared vault but couldn't leave a deletion marker ({e}); other \
                 accounts will see it as unreachable rather than deleted."
            ));
        }
    }
    let _ = secrets::clear_cached_vault_key(&meta.vault_id);

    // Switch this account back to a vault of its own (the detach tail).
    vault::pointer::clear(&data_dir)?;
    vault::pointer::clear_retired(&data_dir)?;
    state.set_vault_fault(None);
    let resolved = vault::resolve(&app)?;
    let local_meta = vault::ensure_device_meta(&resolved.vault_root)?;
    // A different vault from here on, so anything the old one was warning about no longer applies.
    state.clear_meta_warning();
    if let Some((conn, master, report)) = vault::open_at_boot(&resolved, &local_meta)? {
        state.open_session(conn, VaultRuntime::build(&resolved, &local_meta, &master))?;
        state.note_meta_report(&report);
    }
    engage_or_warn(&app, &mut warnings);
    Ok(VaultOpOutcome { warnings })
}

/// What `repair_vault_access` achieved: whether the folder answers again, whether the
/// store could be reopened right away (false + repaired ⇒ the passphrase prompt is
/// next), and any non-fatal warnings.
#[derive(Serialize)]
pub struct RepairOutcome {
    pub repaired: bool,
    pub reopened: bool,
    pub warnings: Vec<String>,
}

/// Owner-side repair for a vault folder the OS is refusing: re-grant this account,
/// verify the vault answers, best-effort restore the intended lockdown (owner + linked
/// accounts), and reopen the session. Works even against a hostile DACL because the
/// folder's OS owner retains implicit READ_CONTROL + WRITE_DAC on objects it created —
/// exactly the account that shared the vault. Never elevates: when even this fails, the
/// UI shows a copyable admin recipe instead. A joiner running it gets an honest denial
/// (they don't own the folder) plus guidance to ask the owner.
#[tauri::command]
pub fn repair_vault_access(app: AppHandle, state: State<'_, AppState>) -> Result<RepairOutcome> {
    let data_dir = paths::data_dir(&app)?;
    // resolve_layout, not resolve(): resolve's create_dir_all may itself be the thing
    // that's denied, and repair must reach the grant step regardless.
    let Some(pointer) = vault::pointer::load(&data_dir)? else {
        return Err(Error::Other(
            "this account isn't pointed at a shared vault — there's nothing to repair".into(),
        ));
    };
    let root = pointer.vault_root;
    if let Err(e) = std::fs::metadata(&root) {
        // Denied metadata still means the folder EXISTS (that's the repairable case);
        // only a genuinely absent folder ends the repair here.
        if e.kind() == std::io::ErrorKind::NotFound {
            return Err(Error::Vault(VaultFault {
                code: VaultFaultCode::NotFound,
                op: "repair the vault folder".into(),
                path: Some(root.display().to_string()),
                message: "That folder is gone — if it lives on a removable drive, plug it \
                          in and try again."
                    .into(),
            }));
        }
    }
    let mut warnings = Vec::new();
    // (1) Reset the folder's DACL to inherit again, THEN re-grant this account. The reset
    // clears a botched lockdown wholesale (a `/grant` alone can't repair an `/inheritance:r`
    // that dropped the owner's usable access on child items); both work against a hostile
    // DACL because the folder's OS owner keeps implicit WRITE_DAC. POSIX chmod-700 can't
    // strip the Unix owner, so this whole step is Windows-only.
    #[cfg(windows)]
    {
        // A reset failure isn't fatal on its own — the grant below may still fix access —
        // so it's a warning; the grant's failure is the real gate.
        if let Err(e) = vault::acl::reset_inheritance(&root) {
            warnings.push(format!(
                "PM couldn't reset the folder's inherited permissions ({e}); trying a direct \
                 grant instead."
            ));
        }
        let me = vault::acl::current_user_sid()?;
        vault::acl::grant_access(&root, &me).map_err(|e| {
            Error::Vault(VaultFault {
                code: VaultFaultCode::Denied,
                op: "repair the vault folder".into(),
                path: Some(root.display().to_string()),
                message: format!(
                    "Windows wouldn't let PM change the folder's permissions from this \
                     account ({e})."
                ),
            })
        })?;
    }
    // (2) The probe: the vault must actually answer now (ACLs are checked at handle-open,
    // so this read is the honest test of whether the grant took effect).
    let meta = vault::load_meta(&root)?.ok_or_else(|| {
        Error::Vault(VaultFault {
            code: VaultFaultCode::NoVault,
            op: "repair the vault folder".into(),
            path: Some(root.display().to_string()),
            message: "The folder answers again, but it doesn't hold a PM vault any more.".into(),
        })
    })?;
    // (3) Best-effort: restore the intended lockdown (owner + every linked account from
    // the sidecar). Failure leaves the vault reachable-but-unlocked-down; encryption
    // still protects the contents, so this is a warning, not a failed repair.
    let linked = vault::access::principals(&root, &meta.vault_id);
    if let Err(e) = vault::acl::restrict_to_owner(&root, &linked) {
        warnings.push(format!(
            "Access is restored, but PM couldn't re-apply the folder's protections ({e}) — \
             other accounts on this PC may see the encrypted files (they still can't read \
             their contents)."
        ));
    }
    // (4) Reopen if the store is closed; a repaired-but-uncached passphrase vault falls
    // through to the unlock prompt (repaired: true, reopened: false).
    let mut reopened = false;
    if state.is_unlocked() {
        // A watcher-raised fault on a still-open session: the folder answers again.
        state.set_vault_fault(None);
    } else {
        let resolved = vault::resolve_layout(&root, None);
        if let Some((conn, master, report)) = vault::open_at_boot(&resolved, &meta)? {
            // open_session clears the carried fault (the one healing choke point).
            state.open_session(conn, VaultRuntime::build(&resolved, &meta, &master))?;
            state.note_meta_report(&report);
            reopened = true;
        } else {
            state.set_vault_fault(None);
        }
    }
    if let Err(e) = lock_session::engage(&app) {
        warnings.push(format!(
            "PM couldn't re-engage its writer coordination ({e}) — restart PM before using \
             the vault from another account."
        ));
    }
    Ok(RepairOutcome {
        repaired: true,
        reopened,
        warnings,
    })
}

/// The suggested cross-account location for a shared vault, plus whether it looks
/// writable from here. Windows only (the suggestion lives under `%ProgramData%`, whose
/// default ACLs let any user create their own subfolder); elsewhere `path` is null and
/// the UI asks for a folder pick.
#[derive(Serialize)]
pub struct SuggestedLocation {
    pub path: Option<String>,
    pub writable: bool,
}

/// Suggest where a shared vault should live (see [`SuggestedLocation`]): the first
/// `Shared Vault` / `Shared Vault 2` / … folder under the shared base not already
/// occupied by a different vault. Re-suggesting this vault's own folder is fine (a
/// wizard re-run).
#[tauri::command]
pub fn suggest_shared_vault_location(app: AppHandle) -> Result<SuggestedLocation> {
    let Some(base) = vault::advert::shared_base_dir() else {
        return Ok(SuggestedLocation {
            path: None,
            writable: false,
        });
    };
    let own_id = vault::load_meta(&vault::resolve(&app)?.vault_root)?
        .map(|m| m.vault_id)
        .unwrap_or_default();
    // Occupied = a different vault sits there, or the folder can't be checked. This
    // vault's own folder (or an empty one) is free.
    let occupied = |p: &std::path::Path| {
        !matches!(
            vault::migrate::relocation_target_state(&vault::load_meta(p), &own_id),
            vault::migrate::TargetState::Vacant | vault::migrate::TargetState::SameVault
        )
    };
    let path = vault::advert::next_free_location(&base, "Shared Vault", occupied);
    // Probe writability of the BASE (creating the vault folder itself is the move's
    // job): stock ProgramData lets Users create subfolders, but GPO/AV can tighten it.
    let writable = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&base)?;
        let probe = base.join(".pm-write-probe");
        std::fs::write(&probe, b"probe")?;
        std::fs::remove_file(&probe)?;
        Ok(())
    })()
    .is_ok();
    Ok(SuggestedLocation {
        path: Some(path.to_string_lossy().into_owned()),
        writable,
    })
}

/// One local Windows account for the share wizard's picker.
#[derive(Serialize)]
pub struct LocalAccount {
    pub name: String,
    pub sid: String,
    pub is_current: bool,
}

/// Parse `Get-LocalUser` picker lines (`name|SID`, one per line), marking the caller's
/// own account. Pure; tolerant of blank/garbage lines.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_account_lines(output: &str, current_sid: &str) -> Vec<LocalAccount> {
    output
        .lines()
        .filter_map(|line| {
            let (name, sid) = line.trim().rsplit_once('|')?;
            (!name.is_empty() && sid.starts_with("S-1-")).then(|| LocalAccount {
                name: name.to_string(),
                sid: sid.to_string(),
                is_current: sid == current_sid,
            })
        })
        .collect()
}

/// The enabled local Windows accounts, for the share wizard's "who can open it" picker
/// (so nobody has to hand-copy a SID). Best-effort: on failure or off-Windows the UI
/// falls back to the manual name/SID field.
#[tauri::command]
pub fn list_local_accounts() -> Result<Vec<LocalAccount>> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Suppress the console window that would flash when a GUI app spawns a child.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-LocalUser | Where-Object { $_.Enabled } | ForEach-Object { $_.Name + '|' + $_.SID.Value }",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| Error::Other(format!("could not list local accounts: {e}")))?;
        if !out.status.success() {
            return Err(Error::Other("could not list local accounts".into()));
        }
        let current = vault::acl::current_user_sid().unwrap_or_default();
        Ok(parse_account_lines(
            &String::from_utf8_lossy(&out.stdout),
            &current,
        ))
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

/// The cooperative writer-lock status for a shared vault: whether this instance is the
/// active writer, whether another live profile holds it, and whether that holder looks
/// crashed (so the UI can offer a warned force-take). A device vault always reports active.
#[tauri::command]
pub fn vault_lock_status(app: AppHandle) -> Result<lock_session::VaultLockStatus> {
    Ok(lock_session::status(&app))
}

/// "Continue here" on the curtain: ask the other live profile to hand the vault over (the
/// watcher takes it once they release), or take it immediately if they've already gone.
#[tauri::command]
pub fn continue_here(app: AppHandle) -> Result<()> {
    lock_session::continue_here(&app)
}

/// Force-take a vault whose holder looks crashed (a stale heartbeat). The UI shows the
/// "the other instance may not have saved its last change" warning before calling this.
#[tauri::command]
pub fn force_take_vault(app: AppHandle) -> Result<()> {
    lock_session::force_take(&app)
}

#[cfg(test)]
mod vault_command_tests {
    use super::needs_move_home;
    use std::path::Path;

    #[test]
    fn needs_move_home_only_when_the_vault_is_outside_the_profile() {
        let data_dir = Path::new("/profile/data");
        // A shared-folder vault (outside the profile) must move home before decrypting.
        assert!(needs_move_home(
            Path::new("/ProgramData/Personal Manager/Shared Vault"),
            data_dir
        ));
        // A vault already at (or under) the profile data dir stays put.
        assert!(!needs_move_home(data_dir, data_dir));
        assert!(!needs_move_home(&data_dir.join("vault"), data_dir));
    }
}
