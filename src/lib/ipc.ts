// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { invoke as tauriInvoke, Channel } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { RepairOutcome, VaultFault } from "./types";
import type {
  AppLockStatus,
  BackupEntry,
  BackupEvent,
  AgendaEvent,
  BackupSchedule,
  BackupState,
  CalendarAccount,
  CalendarEvent,
  CalendarOverview,
  ChatEvent,
  ChunkSpan,
  CompressResult,
  CompressSnapshot,
  ContextStatus,
  Conversation,
  PassphraseScore,
  CostSummary,
  DailyBriefing,
  DevRetrievalExplain,
  DevSystemInfo,
  DevTableCount,
  DevTablePage,
  Document,
  DraftPreference,
  DriveAccount,
  DriveFolder,
  DriveScope,
  DriveStatus,
  DriveSyncState,
  Entity,
  Flag,
  FocusRoute,
  IcsFeedInfo,
  ImageData,
  Importance,
  IndexOnlyFetch,
  IngestEvent,
  InstallProgressEvent,
  LanguageOptions,
  LayoutProgressEvent,
  LocalFolder,
  LocalFolderSyncState,
  LocalSubfolder,
  Message,
  ModelInfo,
  OcrStatus,
  OneDriveAccount,
  OneDriveFolder,
  OneDriveScope,
  OneDriveStatus,
  OneDriveSyncState,
  Preference,
  Milestone,
  ProjectOverview,
  ProjectProposalEvent,
  ProjectSize,
  GdriveBackupStatus,
  ProtonCliStatus,
  ProtonConnStatus,
  ReviewDecision,
  ReviewEvent,
  RestoreSummary,
  SemanticLayout,
  Settings,
  SharedDrive,
  SidecarStatus,
  StorageReport,
  SyncEvent,
  TsneStatus,
  AdoptOutcome,
  LocalAccount,
  SharedVaultAd,
  SuggestedLocation,
  VaultLockStatus,
  VaultOpOutcome,
  VaultStatus,
  SmartAppControlState,
  WipeReport,
  WipeSelection,
} from "./types";

/** A rejected vault command, carrying the backend's classified `VaultFault` so the five
 *  vault recovery surfaces can branch on `fault.code` instead of string-matching. Its
 *  `message`/`toString()` stay the ready-to-show sentence, so the ~200 existing
 *  `String(e)` call sites keep rendering cleanly. */
export class VaultError extends Error {
  constructor(public readonly fault: VaultFault) {
    super(fault.message);
    this.name = "VaultError";
  }
  toString() {
    return this.message;
  }
}

/** The classified fault behind a caught error, or null for any other rejection. */
export const vaultFaultOf = (e: unknown): VaultFault | null =>
  e instanceof VaultError ? e.fault : null;

/** Whether a rejection is the backend's one structured error shape (Rust `Error::Vault`
 *  serializes `{code, op, path, message}`; every other variant stays a bare string). */
function isVaultFaultShaped(e: unknown): e is VaultFault {
  if (typeof e !== "object" || e === null) return false;
  const f = e as Record<string, unknown>;
  return typeof f.code === "string" && typeof f.op === "string" && typeof f.message === "string";
}

/** The one invoke used by every wrapper below: passes results and string rejections
 *  through untouched, and normalizes the single structured rejection shape into a
 *  `VaultError` — so no caller can ever see `[object Object]`, and no vault-path command
 *  can be missed by a per-wrapper list. */
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await tauriInvoke<T>(cmd, args);
  } catch (e) {
    throw isVaultFaultShaped(e) ? new VaultError(e) : e;
  }
}

export const hasOpenRouterKey = () => invoke<boolean>("has_openrouter_key");

export const setOpenRouterKey = (key: string) => invoke<void>("set_openrouter_key", { key });

export const hasOpenRouterBackgroundKey = () => invoke<boolean>("has_openrouter_background_key");

export const setOpenRouterBackgroundKey = (key: string) =>
  invoke<void>("set_openrouter_background_key", { key });

export const getSettings = () => invoke<Settings>("get_settings");

/** Current Windows Smart App Control state, so the updater can warn before a restart that SAC
 *  would silently block. Off-Windows / when SAC is absent this resolves to "unknown". */
export const smartAppControlState = () => invoke<SmartAppControlState>("smart_app_control_state");

/** Turn query-time reranking on/off (a cross-encoder re-scores search hits). Stateless — never
 *  triggers a Rebuild; the effect lands on the next query. */
export const setReranking = (enabled: boolean) => invoke<void>("set_reranking", { enabled });

/** Set the indexing-speed preference: "fast" (max throughput) or "gentle" (paced for low-end
 *  machines). Applies to the next Drive sync / file import. */
export const setIndexingSpeed = (speed: "fast" | "gentle") =>
  invoke<void>("set_indexing_speed", { speed });

/** The vault's search-language choices + current selection for the onboarding picker. */
export const languageOptions = () => invoke<LanguageOptions>("language_options");

/** Choose the vault's embedder ("search language"). Only valid while the vault is empty —
 *  changing it on a populated vault needs the Re-index flow (a later update). */
export const setVaultEmbedder = (embedderId: string) =>
  invoke<void>("set_vault_embedder", { embedderId });

/** Persist the user's IANA time zone (e.g. "Europe/London"). An empty string clears
 *  it (the backend then reasons in UTC). Validated against the tz database in Rust. */
export const setTimeZone = (zone: string) => invoke<void>("set_time_zone", { zone });

/** Read a UI preference blob (theme axes, pinboard layout) from the encrypted store
 *  so it travels with the data folder. `null` when nothing is stored yet. */
export const getPref = (key: string) => invoke<string | null>("get_pref", { key });

/** Persist a UI preference blob. The backend only accepts a fixed allowlist of keys
 *  (`appearance`, `pinboard`), so this can't touch schema-critical settings. */
export const setPref = (key: string, value: string) => invoke<void>("set_pref", { key, value });

/** Ordered preferred chat models (first = primary). */
export const setChatModels = (models: string[]) => invoke<void>("set_chat_models", { models });

/** Ordered preferred background models (sorting proposals + Learning You). */
export const setBackgroundModels = (models: string[]) =>
  invoke<void>("set_background_models", { models });

/** Toggle auto-switch (fallback to the next model on a rate-limit) for chat. */
export const setChatAutoSwitch = (enabled: boolean) =>
  invoke<void>("set_chat_auto_switch", { enabled });

/** Toggle auto-switch for background work. */
export const setBackgroundAutoSwitch = (enabled: boolean) =>
  invoke<void>("set_background_auto_switch", { enabled });

/** The OpenRouter model catalogue (id, name, pricing) for the Settings picker. */
export const listModels = () => invoke<ModelInfo[]>("list_models");

// --- Cost logger (spec §11.2 / §17.1) ---

/** Per-model token spend + totals; refreshes the daily price cache on read. */
export const costSummary = () => invoke<CostSummary>("cost_summary");

/** Force a re-pull of OpenRouter pricing; returns the refreshed summary. */
export const refreshPricing = () => invoke<CostSummary>("refresh_pricing");

/** Toggle the UI help/explain mode (Step 4b). */
export const setHelpMode = (enabled: boolean) => invoke<void>("set_help_mode", { enabled });

// --- Biometric app-lock (soft UI gate, opt-in — spec §16.2) ---

/** Whether the app-lock is on, and whether this device can perform an OS verification. */
export const appLockStatus = () => invoke<AppLockStatus>("app_lock_status");

/** Turn the app-lock on/off. Enabling is rejected by the backend when unavailable. */
export const setAppLock = (enabled: boolean) => invoke<void>("set_app_lock", { enabled });

/** Run the OS verification (Windows Hello / Touch ID) to lift the launch lock.
 *  Resolves true on success, false when the user cancels/fails; rejects when the
 *  verifier can't run at all. */
export const unlockApp = () => invoke<boolean>("unlock_app");

// --- Shared & portable vaults (spec §2–6) ---

/** The vault's mode, whether it needs unlocking on this profile, encryption + location. */
export const vaultStatus = () => invoke<VaultStatus>("vault_status");

/** Retry opening the store after a transient boot-time open failure (an AV / search-indexer
 *  file lock, disk I/O). Resolves once the store opens (or falls through to the unlock prompt);
 *  rejects with the fresh error if it still can't open. */
export const retryOpenVault = () => invoke<void>("retry_open_vault");

/** Convert this device vault into a shareable, passphrase-protected one (re-keys the
 *  store and encrypts the Markdown via the one migration routine). Pass `targetLocation`
 *  to also move it to a cross-account-reachable folder in the SAME crash-recoverable
 *  migration — the guided share flow always does, since a shareable vault left in the
 *  per-user profile folder is unreachable by every other account. */
export const createShareableVault = (passphrase: string, targetLocation?: string) =>
  invoke<VaultOpOutcome>("create_shareable_vault", {
    passphrase,
    targetLocation: targetLocation ?? null,
  });

/** Change a shareable vault's passphrase: re-derive the key and re-encrypt the Markdown. */
export const changeVaultPassphrase = (newPassphrase: string) =>
  invoke<VaultOpOutcome>("change_vault_passphrase", { newPassphrase });

/** Make a shareable vault private again: re-key to a device key and decrypt the Markdown
 *  (also withdraws the discovery marker other accounts see). */
export const makeVaultPrivate = () => invoke<VaultOpOutcome>("make_vault_private");

/** Move the vault to another folder (e.g. a shared location), keeping key + policy.
 *  Refuses a folder that already holds a different vault — join that one instead. */
export const moveVault = (folder: string) => invoke<VaultOpOutcome>("move_vault", { folder });

/** Unlock the current passphrase vault for this session and cache the key on this
 *  profile, so the next launch is silent. */
export const unlockVault = (passphrase: string) => invoke<void>("unlock_vault", { passphrase });

/** Score a candidate passphrase for the create/change strength meter (M-4). Advisory — the backend
 *  `validate_passphrase_strength` floor is the real gate; this just mirrors it so the meter agrees. */
export const scorePassphrase = (passphrase: string) =>
  invoke<PassphraseScore>("score_passphrase", { passphrase });

/** Subscribe to the non-blocking warning emitted when a vault's metadata was repaired on open (M-3):
 *  a silently-downgraded encryption policy that PM forced back on, or a failed integrity check. */
export const onVaultMetaWarning = (handler: (message: string) => void): Promise<UnlistenFn> =>
  listen<string>("vault://meta-warning", (e) => handler(e.payload));

/** Forget this profile's cached passphrase key, so it's asked for again next launch.
 *  Does not lock the current session. */
export const forgetVaultPassphrase = () => invoke<void>("forget_vault_passphrase");

/** Grant another account on this machine access to the shared vault folder (a name or
 *  SID). Requires a shareable vault that has moved OUT of this profile's private folder
 *  (an ACE inside the profile is unreachable for other accounts); a clear error otherwise. */
export const linkVaultAccount = (account: string) =>
  invoke<VaultOpOutcome>("link_vault_account", { account });

/** The shared vaults other accounts have advertised on this machine, filtered to ones this
 *  profile could join. Empty off-Windows (no machine-wide discovery folder there). */
export const listSharedVaults = () => invoke<SharedVaultAd[]>("list_shared_vaults");

/** Join an existing shared vault: unlock `folder` with the passphrase, cache the key for
 *  silent launches, and point this profile at it. The previous vault is set aside on disk,
 *  never deleted — `detachFromSharedVault` brings it back. */
export const adoptSharedVault = (folder: string, passphrase: string) =>
  invoke<AdoptOutcome>("adopt_shared_vault", { folder, passphrase });

/** Leave the shared vault: retire this profile's pointer (kept on record so Settings can
 *  offer a rejoin) and reopen the vault set aside at join time — or a fresh, EMPTY one for
 *  an owner whose vault physically moved into the shared folder. The shared folder itself
 *  is untouched. Callers confirm via DetachConfirm first — this is not a silent switch. */
export const detachFromSharedVault = () => invoke<void>("detach_from_shared_vault");

/** Owner-side repair for a vault folder the OS is refusing (`fault.code === "denied"`):
 *  re-grant this account, restore the intended lockdown, and reopen the session. Works
 *  even against a hostile DACL (the folder's OS owner keeps implicit permission-editing
 *  rights); never elevates — on failure the UI shows a copyable admin recipe instead. */
export const repairVaultAccess = () => invoke<RepairOutcome>("repair_vault_access");

/** Owner-side deletion of the shared vault: remove it from the shared folder, leave a
 *  tombstone so every joined account is told it's gone at next launch, and switch this
 *  account back to a vault of its own. Distinct from make-private (keeps the data) and
 *  detach (leaves the shared copy). Returns non-fatal warnings. */
export const deleteSharedVault = () => invoke<VaultOpOutcome>("delete_shared_vault");

/** Joiner-side acknowledgement that the shared vault was deleted by its owner: switch back
 *  to a vault on this account (no rejoin breadcrumb — it's gone for good) and drop the
 *  cached key. Called from the one-time deletion notice. */
export const acknowledgeDeletedSharedVault = () => invoke<void>("acknowledge_deleted_shared_vault");

/** Subscribe to `vault://fault` — emitted when PM loses access to the shared vault folder
 *  mid-session (the store closed, or the writer-lock heartbeat started failing), so the
 *  app can raise a banner naming the real problem instead of a generic "vault is locked". */
export const onVaultFault = (handler: (fault: VaultFault) => void): Promise<UnlistenFn> =>
  listen<VaultFault>("vault://fault", (e) => handler(e.payload));

/** Where a shared vault should live so every account can reach it (null path ⇒ ask the
 *  user to pick a folder), plus whether that base looks writable from here. */
export const suggestSharedVaultLocation = () =>
  invoke<SuggestedLocation>("suggest_shared_vault_location");

/** The enabled local Windows accounts for the share wizard's picker; empty on failure or
 *  off-Windows (the UI falls back to the manual name/SID field). */
export const listLocalAccounts = () => invoke<LocalAccount[]>("list_local_accounts");

/** Export the Markdown vault as plaintext `.md` into `destDir` — the "never locked in"
 *  escape hatch (decrypts with the in-session key). Returns the number of files written. */
export const exportPlaintextMarkdown = (destDir: string) =>
  invoke<number>("export_plaintext_markdown", { destDir });

// Single-writer lock for a shared vault (spec §5).

/** Whether this instance is the active writer, or another profile holds the vault. */
export const vaultLockStatus = () => invoke<VaultLockStatus>("vault_lock_status");

/** "Continue here": ask the other live profile to hand the vault over (the backend takes
 *  it once they release), or take it immediately if they've already gone. */
export const continueHere = () => invoke<void>("continue_here");

/** Force-take a vault whose holder looks crashed (stale heartbeat). Show the
 *  "may not have saved its last change" warning before calling. */
export const forceTakeVault = () => invoke<void>("force_take_vault");

/** The reason this instance was curtained: it found another writer on open
 *  ("other-active"), or it handed the baton over on request ("handed-off"). */
export interface VaultCurtainEvent {
  reason: "other-active" | "handed-off";
  other_profile: string | null;
}

/** Subscribe to the curtain event (this instance stepped back from being the writer). */
export const onVaultCurtain = (handler: (e: VaultCurtainEvent) => void): Promise<UnlistenFn> =>
  listen<VaultCurtainEvent>("vault://curtain", (e) => handler(e.payload));

/** Subscribe to the acquired event (this instance became the active writer; lift curtain). */
export const onVaultAcquired = (handler: () => void): Promise<UnlistenFn> =>
  listen("vault://acquired", () => handler());

// --- Structured preferences (spec §4.5 — the typed model that replaces the Learning-You blob) ---

/** Every structured preference record (the Teach tab's list). */
export const listPreferences = () => invoke<Preference[]>("list_preferences");

/** Add a preference the user has explicitly stated (structured form or confirmed parse). Stored as
 *  user-stated + confirmed. `entityId` is required for a project-scoped preference. */
export const addPreference = (
  scope: string,
  entityId: number | null,
  condition: string | null,
  value: string,
) => invoke<number>("add_preference", { scope, entityId, condition, value });

/** Edit a preference's scope/target/condition/value (also marks it confirmed). */
export const updatePreference = (
  id: number,
  scope: string,
  entityId: number | null,
  condition: string | null,
  value: string,
) => invoke<void>("update_preference", { id, scope, entityId, condition, value });

/** Mark an inferred preference as user-confirmed (the Teach-tab "✓ Confirm"). */
export const confirmPreference = (id: number) => invoke<void>("confirm_preference", { id });

/** Delete a preference. */
export const deletePreference = (id: number) => invoke<void>("delete_preference", { id });

/** Parse a free-text sentence into a draft preference (the "in your own words" path); the form
 *  prefills with the result for the user to confirm before it is stored. */
export const parsePreferenceStatement = (text: string) =>
  invoke<DraftPreference>("parse_preference_statement", { text });

export const listConversations = () => invoke<Conversation[]>("list_conversations");

/** Start a conversation, optionally scoped to a project (Step 5) so its chat
 *  retrieval is confined to that project's documents. */
export const createConversation = (project?: string | null) =>
  invoke<Conversation>("create_conversation", { project: project ?? null });

export const getMessages = (conversationId: number) =>
  invoke<Message[]>("get_messages", { conversationId });

/** Rename a conversation (board card 7E). Latches the title as user-chosen so the background title pass
 *  never overwrites it. Returns the saved (trimmed/clamped) title. */
export const renameConversation = (conversationId: number, title: string) =>
  invoke<string>("rename_conversation", { conversationId, title });

/** Move a conversation into a project, or back to global with `null` (board card B). The scope follows
 *  the new home on the next send — retrieval re-narrows and future activity re-keys automatically. */
export const setConversationProject = (conversationId: number, project: string | null) =>
  invoke<void>("set_conversation_project", { conversationId, project });

/** Delete a conversation and everything it produced (board card 7G): its messages, its session, and —
 *  if the chat was indexed — its document, chunks/vectors/FTS rows and vault file. Preferences it
 *  produced are kept. Irreversible. */
export const deleteConversation = (conversationId: number) =>
  invoke<void>("delete_conversation", { conversationId });

/**
 * Send a message and stream the assistant's reply. `onEvent` fires for every
 * token, then once on completion (or error). The returned promise resolves when
 * the whole exchange is persisted.
 */
export function sendMessage(
  conversationId: number,
  content: string,
  onEvent: (event: ChatEvent) => void,
): Promise<void> {
  const channel = new Channel<ChatEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("send_message", { conversationId, content, onEvent: channel });
}

/** How full the selected model's context window is for a conversation, plus the meter/alert state
 *  (board card 7D). Cheap; the chat calls it after each reply and after a compress/undo. */
export const chatContextStatus = (conversationId: number) =>
  invoke<ContextStatus>("chat_context_status", { conversationId });

/** Compress the conversation now: fold the older turns into the rolling summary to reclaim context.
 *  Returns the condensed bullets + an Undo snapshot, or null when there is nothing to fold. */
export const compressChat = (conversationId: number) =>
  invoke<CompressResult | null>("compress_chat", { conversationId });

/** Undo a compression by restoring the snapshot the UI held from `compressChat`. */
export const revertCompress = (conversationId: number, snapshot: CompressSnapshot) =>
  invoke<void>("revert_compress", { conversationId, snapshot });

// --- Archivist: documents ---

export const sidecarStatus = () => invoke<SidecarStatus>("sidecar_status");

export const ensureSidecar = () => invoke<void>("ensure_sidecar");

export const listDocuments = () => invoke<Document[]>("list_documents");
/** Fetch one document by id (F-48) — resolves a citation id without refetching the whole list. */
export const getDocument = (id: number) => invoke<Document>("get_document", { id });

/** What a pinboard note became after ingest (source id `note:<widgetId>`). */
export interface NoteIngest {
  source_id: string;
  document_id: number;
  reviewed: boolean;
  project: string;
}

/** Ingest a pinboard note's text as an index-only document — it flows through review → proposal →
 *  project-importance like any doc. Idempotent on the widget id: an unchanged re-ingest is a no-op,
 *  an edited note re-embeds in place keeping its filing. The document persists if the note is
 *  deleted. */
export const ingestNote = (widgetId: string, title: string, text: string) =>
  invoke<NoteIngest>("ingest_note", { widgetId, title, text });

/**
 * Dev-only (debug builds): drive the index-only substrate (board card 3) through its reducer without
 * a real connector. `kind` is "add" (ingest `body` as a new index-only item titled `title`), "update"
 * (re-embed from `body`), "delete", "rename" (to `externalRef`), or "source_failure". The backend
 * command is compiled out of release builds, so only call this behind `isDevBuild`
 * (the central build-time signal in `lib/capabilities`) — a TEST HARNESS, never the runtime
 * Developer mode (issue #78).
 */
export const devApplyChangeEvent = (
  kind: "add" | "update" | "delete" | "rename" | "source_failure",
  sourceId: string,
  title: string | null,
  body: string | null,
  externalRef: string | null,
) => invoke<void>("dev_apply_change_event", { kind, sourceId, title, body, externalRef });

/** Transcribe a recorded voice clip to text, fully on-device via the sidecar's
 *  Whisper model. `audioBase64` is the standard-base64 of the recording bytes. */
export const transcribeAudio = (audioBase64: string) =>
  invoke<string>("transcribe_audio", { audioBase64 });

/** Ingest files/folders, streaming progress for each item. `copyPhotosToVault` is the drag-drop
 *  opt-in to save dropped originals into the vault's `photos/` folder (photos only; default off). */
export function ingestPaths(
  paths: string[],
  onEvent: (event: IngestEvent) => void,
  copyPhotosToVault = false,
): Promise<void> {
  const channel = new Channel<IngestEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("ingest_paths", { paths, copyPhotosToVault, onEvent: channel });
}

/** Drop the index and rebuild it from the Markdown vault. */
export function rebuildIndex(onEvent: (event: IngestEvent) => void): Promise<void> {
  const channel = new Channel<IngestEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("rebuild_index", { onEvent: channel });
}

// --- Developer mode (issue #78): read-only inspection surfaces ---
// All are harmless reads (always registered in the backend); the UI that calls them is gated by
// the runtime `devMode` (see lib/capabilities). The backend redacts before returning, so these
// payloads are already safe to display.

/** Running vault's index-time + runtime facts for the Dev tab's System panel. */
export const devSystemInfo = () => invoke<DevSystemInfo>("dev_system_info");

/** Row counts for every inspected table (incl. the derived indexes + `settings`). */
export const devTableCounts = () => invoke<DevTableCount[]>("dev_table_counts");

/** The browsable table names for the Dev tab's table picker. */
export const devTableList = () => invoke<string[]>("dev_table_list");

/** A redacted page of one allow-listed table (newest rows first). `limit` is capped at 200. */
export const devTableRows = (table: string, limit: number, offset: number) =>
  invoke<DevTablePage>("dev_table_rows", { table, limit, offset });

/** The chunk breakdown for one document — the in-context Documents inspector. Content is
 *  length-only (the `chunks` projection), ordered by ordinal, capped at 500. */
export const devDocumentChunks = (documentId: number) =>
  invoke<DevTablePage>("dev_document_chunks", { documentId });

/** Run a query through the live hybrid retriever and return each candidate's per-stage scores
 *  (issue #81). Read-only; chunk bodies come back as truncated previews. `k` defaults to 6,
 *  clamped 1–50. Embeds via the sidecar, so it needs the document engine ready. */
export const devRetrievalExplain = (query: string, project?: string, k?: number) =>
  invoke<DevRetrievalExplain>("dev_retrieval_explain", { query, project, k });

/** In-chat "Retrieval explain" (card 7H): the same instrumented read as the dev panel, for
 *  graduated users. When `k` is omitted it defaults to the user's saved retrieval depth (what a
 *  real chat turn uses); pass an explicit `k` to preview a different candidate pool without
 *  committing it. Read-only; needs the document engine ready. */
export const retrievalExplain = (query: string, project?: string, k?: number) =>
  invoke<DevRetrievalExplain>("retrieval_explain", { query, project, k });

/** Commit the retrieval depth `k` — the candidate pool that reaches the reranker — as the value
 *  every future chat turn retrieves at (card 7H). Clamped 1–50 in the backend; stateless. */
export const setRetrievalK = (k: number) => invoke<void>("set_retrieval_k", { k });

/** Ask the background model to diagnose a retrieval symptom from the user's own explain state
 *  (card 7H). RECOMMEND-only: it returns plain-text advice and never changes any setting — the
 *  user makes any change themselves via the depth slider. */
export const retrievalDiagnose = (symptom: string, query: string, explain: DevRetrievalExplain) =>
  invoke<string>("retrieval_diagnose", { symptom, query, explain });

// --- Archivist: sorting review & organisation (Step 4) ---

/** Distinct project labels across all documents. */
export const listProjects = () => invoke<string[]>("list_projects");

/** Documents still awaiting the sorting review (`reviewed = false`). */
export const reviewQueue = () => invoke<Document[]>("review_queue");
/** Just the review-queue length for the sidebar badge (F-47) — avoids materialising the whole queue. */
export const reviewQueueCount = () => invoke<number>("review_queue_count");

/** Ask the AI to propose project/tags/importance for the unreviewed documents,
 *  streaming each proposal back as it's ready. Pass `documentIds` to scope it. */
export function proposeMetadata(
  onEvent: (event: ReviewEvent) => void,
  documentIds?: number[],
): Promise<void> {
  const channel = new Channel<ReviewEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("propose_metadata", { documentIds: documentIds ?? null, onEvent: channel });
}

/** Confirm a review pass — writes the metadata and logs every correction. */
export const commitReview = (decisions: ReviewDecision[]) =>
  invoke<void>("commit_review", { decisions });

/** Edit one already-reviewed document's metadata (an after-the-fact correction). */
export const setDocumentMetadata = (
  documentId: number,
  project: string,
  tags: string[],
  importance: Importance,
) => invoke<Document>("set_document_metadata", { documentId, project, tags, importance });

// --- Canonical entities (the Teach tab; entity-resolution foundation) ---

/** Project entities with their aliases (the Teach tab's list). */
export const listEntities = (kind?: string) =>
  invoke<Entity[]>("list_entities", { kind: kind ?? null });

/** Record a forward-going alias for a project entity. Rejected (not folded) if the alias
 *  already belongs to another project — that's a merge. */
export const addEntityAlias = (entityId: number, alias: string) =>
  invoke<void>("add_entity_alias", { entityId, alias });

/** Rename a canonical project — a one-row identity update that also rewrites its documents'
 *  vault frontmatter + cache to the new name. */
export const renameEntity = (entityId: number, newName: string) =>
  invoke<void>("rename_entity", { entityId, newName });

/** Merge `fromId` into `intoId`: fold aliases, repoint every document, delete the source.
 *  The headline action — folds a name variant into its canonical so it never recurs. */
export const mergeEntities = (fromId: number, intoId: number) =>
  invoke<void>("merge_entities", { fromId, intoId });

// --- Personal Assistant: focus view & projects (Step 5, spec §4) ---

/** Every active project with its triage metadata and derived status. */
export const listProjectOverviews = () => invoke<ProjectOverview[]>("list_project_overviews");

/** Set/clear a project's triage metadata (the confirm half of the AI loop, or a
 *  hand edit). Blank/omitted fields clear that attribute. */
export const setProjectMetadata = (
  name: string,
  meta: {
    deadline?: string | null;
    size?: ProjectSize;
    blockedBy?: string | null;
    parent?: string | null;
    /** Manual priority ("high"/"medium"/"low"); null = Auto (no tag). */
    importance?: Importance;
  },
) =>
  invoke<void>("set_project_metadata", {
    name,
    deadline: meta.deadline ?? null,
    size: meta.size ?? null,
    blockedBy: meta.blockedBy ?? null,
    parent: meta.parent ?? null,
    importance: meta.importance ?? null,
  });

/** Ask the AI to propose triage metadata for projects, streaming each proposal.
 *  Pass `names` to scope it to specific projects (default: all). */
export function proposeProjectMetadata(
  onEvent: (event: ProjectProposalEvent) => void,
  names?: string[],
): Promise<void> {
  const channel = new Channel<ProjectProposalEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("propose_project_metadata", { names: names ?? null, onEvent: channel });
}

// --- Personal Assistant: project milestones (multi-deadline — card 7) ---

/** A project's milestones, resolved (calendar-linked dates synced) and date-ordered. */
export const listMilestones = (project: string) =>
  invoke<Milestone[]>("list_milestones", { project });

/** Every project's milestones, resolved — read-only, for the calendar overlay (each carries its
 *  `project_name` for click-to-open). */
export const listAllMilestones = () => invoke<Milestone[]>("list_all_milestones");

/** Add a milestone to a project. A non-null `eventUid` makes it calendar-linked. */
export const addMilestone = (
  project: string,
  label: string,
  dueDate: string | null,
  eventUid: string | null,
) => invoke<number>("add_milestone", { project, label, dueDate, eventUid });

/** Edit a milestone's label and (for a PM-native milestone) its date. */
export const updateMilestone = (id: number, label: string, dueDate: string | null) =>
  invoke<void>("update_milestone", { id, label, dueDate });

/** Link a milestone to a calendar event (uid + cached date) or unlink it (null uid). */
export const setMilestoneEvent = (id: number, eventUid: string | null, cachedDate: string | null) =>
  invoke<void>("set_milestone_event", { id, eventUid, cachedDate });

/** Mark a milestone met or unmet. */
export const setMilestoneState = (id: number, met: boolean) =>
  invoke<void>("set_milestone_state", { id, met });

/** Delete a milestone. */
export const deleteMilestone = (id: number) => invoke<void>("delete_milestone", { id });

/** Persist a new ordering of a project's milestones (ids in display order). */
export const reorderMilestones = (project: string, orderedIds: number[]) =>
  invoke<void>("reorder_milestones", { project, orderedIds });

// --- Personal Assistant: Calendar connectors (multi-provider, read-only — cards 6A/6B) ---

/** The whole calendar surface in one read: provider clients, connected accounts/subscriptions, and
 *  every registered calendar (with its selection). Migrates a legacy single-account Google connection
 *  on first call. */
export const calendarOverview = () => invoke<CalendarOverview>("calendar_overview");

/** Tick/untick one calendar (by its `calendars.id`) for syncing. */
export const setCalendarSelected = (calendarId: string, selected: boolean) =>
  invoke<void>("set_calendar_selected", { calendarId, selected });

/** Mark a calendar "quiet": keep it on the Calendar tab but exclude its events from the assistant
 *  (briefing, flags/reminders, chat agenda, focus upcoming). No re-sync — events stay mirrored. */
export const setCalendarQuiet = (calendarId: string, quiet: boolean) =>
  invoke<void>("set_calendar_quiet", { calendarId, quiet });

/** Connect a Google Calendar account (multi-account) — opens the browser; resolves on sign-in.
 *  Pass an account's own project `clientId`/`clientSecret` to sign in with it (Advanced-Protection
 *  path); omit both to use the shared group client. */
export const connectGoogleCalendarAccount = (clientId?: string, clientSecret?: string) =>
  invoke<CalendarAccount>("connect_google_calendar_account", { clientId, clientSecret });

/** Disconnect one Google Calendar account (by email). */
export const disconnectGoogleCalendarAccount = (email: string) =>
  invoke<void>("disconnect_google_calendar_account", { email });

/** Connect an Outlook / Microsoft 365 calendar account (Graph OAuth) — opens the browser. */
export const connectOutlookCalendar = () => invoke<CalendarAccount>("connect_outlook_calendar");

/** Disconnect one Outlook calendar account (by email). */
export const disconnectOutlookCalendar = (email: string) =>
  invoke<void>("disconnect_outlook_calendar", { email });

// iCal subscriptions — the no-OAuth path (works under Advanced Protection).

/** Subscribed feeds (without their secret URLs). */
export const listIcsFeeds = () => invoke<IcsFeedInfo[]>("list_ics_feeds");

/** Add an iCal subscription and sync it. `provider` tags it (apple/outlook/google/other). */
export const addIcsFeed = (label: string, url: string, provider?: string) =>
  invoke<void>("add_ics_feed", { label, url, provider });

/** Remove a feed and its synced events. */
export const removeIcsFeed = (id: string) => invoke<void>("remove_ics_feed", { id });

/** Save the user's BYO Google "Desktop app" client credentials (keychain only). */
export const setGoogleClient = (clientId: string, clientSecret: string) =>
  invoke<void>("set_google_client", { clientId, clientSecret });

/** Forget the Google client credentials (also disconnects every Google service + clears the mirror). */
export const clearGoogleClient = () => invoke<void>("clear_google_client");

/** Pull events from every selected calendar (all providers) into the local mirror; returns the count. */
export const syncCalendar = () => invoke<number>("sync_calendar");

/** Upcoming events in the mirror, for the focus-view agenda. Each carries `ended`: the agenda widens
 *  the strict gate to also list events that finished earlier today so the view can grey them. */
export const listCalendarEvents = () => invoke<AgendaEvent[]>("list_calendar_events");

/** Every mirrored event across the widened band (previous month → ~a year ahead), for the unified
 *  calendar view (card 8). The client filters to the visible range + locally-hidden calendars. */
export const listAllCalendarEvents = () => invoke<CalendarEvent[]>("list_all_calendar_events");

// --- Google Drive (index-only connector, board card 4A) ---

/** The Drive connector's state: whether the shared Google client is set up + connected accounts. */
export const driveStatus = () => invoke<DriveStatus>("drive_status");

/** Connect a Google Drive account — opens the browser; resolves with the connected account. Pass an
 *  account's own project `clientId`/`clientSecret` to sign in with it (Advanced-Protection path);
 *  omit both to use the shared group client. */
export const connectDrive = (clientId?: string, clientSecret?: string) =>
  invoke<DriveAccount>("connect_drive", { clientId, clientSecret });

/** Disconnect one account: forget its token + flag its items unreachable (kept findable). */
export const disconnectDrive = (email: string) => invoke<void>("disconnect_drive", { email });

/** Start syncing one account (or all when `email` is null). Runs **detached** in the backend, so it
 *  keeps going if the user leaves Settings — progress arrives via the global `drive://sync` event
 *  (subscribe with {@link onDriveSync}). The returned promise resolves with the items-touched count
 *  when this call's sync finishes; it's fine to ignore it (the events + status drive the UI). */
export const syncDrive = (email: string | null) => invoke<number>("sync_drive", { account: email });

/** The current background-sync snapshot — used to restore the progress UI on returning to Settings,
 *  and to show the last finished sync's report. */
export const driveSyncStatus = () => invoke<DriveSyncState>("drive_sync_status");

/** Ask the running sync to stop after the current file. Already-indexed files are kept. */
export const stopDriveSync = () => invoke<void>("stop_drive_sync");

/** Resume a sync interrupted by a previous app close/crash mid-index. Called once on launch; resolves
 *  true if a resume was started. Already-indexed files survive, so it only does the outstanding work. */
export const resumeDriveSync = () => invoke<boolean>("resume_drive_sync");

/** Subscribe to global Drive sync progress (fires regardless of which view started the sync). */
export const onDriveSync = (handler: (e: SyncEvent) => void): Promise<UnlistenFn> =>
  listen<SyncEvent>("drive://sync", (e) => handler(e.payload));

/** The shared drives one account can see (for the "add shared drives" picker). */
export const listDriveSharedDrives = (email: string) =>
  invoke<SharedDrive[]>("list_drive_shared_drives", { email });

/** Shared drives already indexed by a DIFFERENT connected account → `{ [driveId]: ownerEmail }`. The
 *  scope picker greys those out (shared drives are de-duplicated — only the owner indexes a drive). */
export const driveSharedOwners = (email: string) =>
  invoke<Record<string, string>>("drive_shared_owners", { email });

/** Immediate subfolders of a folder in a shared drive (one lazy picker level). Pass the shared
 *  drive's id as `parentId` for the top level. */
export const listDriveFolders = (email: string, driveId: string, parentId: string) =>
  invoke<DriveFolder[]>("list_drive_folders", { email, driveId, parentId });

/** Read one account's indexing scope (My Drive on/off + opted-in shared drives and folders). */
export const getDriveScope = (email: string) => invoke<DriveScope>("get_drive_scope", { email });

/** Persist one account's indexing scope; follow with `syncDrive(email)` to apply it. */
export const setDriveScope = (email: string, scope: DriveScope) =>
  invoke<void>("set_drive_scope", { email, scope });

// --- Microsoft OneDrive (index-only connector, board card 4B) ---

/** Save the user's BYO Microsoft client id (public client — no secret; keychain only). */
export const setMicrosoftClient = (clientId: string) =>
  invoke<void>("set_microsoft_client", { clientId });

/** Clear the Microsoft client id and sign out every OneDrive account (kept findable). */
export const clearMicrosoftClient = () => invoke<void>("clear_microsoft_client");

/** The OneDrive connector's state: whether the Microsoft client is set up + connected accounts. */
export const oneDriveStatus = () => invoke<OneDriveStatus>("onedrive_status");

/** Connect a Microsoft OneDrive account — opens the browser; resolves with the connected account. */
export const connectOneDrive = () => invoke<OneDriveAccount>("connect_onedrive");

/** Disconnect one account: forget its token + flag its items unreachable (kept findable). */
export const disconnectOneDrive = (email: string) => invoke<void>("disconnect_onedrive", { email });

/** Start syncing one account (or all when `email` is null). Detached in the backend — progress
 *  arrives via the global `onedrive://sync` event (subscribe with {@link onOneDriveSync}). */
export const syncOneDrive = (email: string | null) =>
  invoke<number>("sync_onedrive", { account: email });

/** The current background-sync snapshot — used to restore the progress UI on returning to Settings. */
export const oneDriveSyncStatus = () => invoke<OneDriveSyncState>("onedrive_sync_status");

/** Ask the running sync to stop after the current file. Already-indexed files are kept. */
export const stopOneDriveSync = () => invoke<void>("stop_onedrive_sync");

/** Resume a sync interrupted by a previous app close/crash mid-index. Called once on launch. */
export const resumeOneDriveSync = () => invoke<boolean>("resume_onedrive_sync");

/** Subscribe to global OneDrive sync progress (fires regardless of which view started the sync). */
export const onOneDriveSync = (handler: (e: SyncEvent) => void): Promise<UnlistenFn> =>
  listen<SyncEvent>("onedrive://sync", (e) => handler(e.payload));

/** Immediate subfolders of a folder (one lazy picker level); pass `null` for the drive root. */
export const listOneDriveFolders = (email: string, parentId: string | null) =>
  invoke<OneDriveFolder[]>("list_onedrive_folders", { email, parentId });

/** Read one account's indexing scope (whole drive, or the chosen folders). */
export const getOneDriveScope = (email: string) =>
  invoke<OneDriveScope>("get_onedrive_scope", { email });

/** Persist one account's indexing scope; follow with `syncOneDrive(email)` to apply it. */
export const setOneDriveScope = (email: string, scope: OneDriveScope) =>
  invoke<void>("set_onedrive_scope", { email, scope });

// --- Local folders (index-only connector, board card 6) ---

/** Every tracked local folder, with its live document count, state, and whether its path is present. */
export const listLocalFolders = () => invoke<LocalFolder[]>("list_local_folders");

/** Track a folder (by absolute path) — registers it and returns its stable key. Index it with
 *  {@link syncLocalFolder}; the live watcher then keeps it current. Re-adding a folder reuses its row. */
export const addLocalFolder = (path: string) => invoke<string>("add_local_folder", { path });

/** Stop tracking a folder: drop its registration and flag its items `unreachable` — they stay findable
 *  (summaries searchable offline), never hard-deleted, just like a cloud disconnect. Pass the key. */
export const removeLocalFolder = (key: string) => invoke<void>("remove_local_folder", { key });

/** The immediate child subfolders of `rel` (root-relative, `/`-joined; null/empty = the folder root)
 *  inside a tracked folder — one lazy level of the local folder picker. */
export const listLocalSubfolders = (key: string, rel: string | null) =>
  invoke<LocalSubfolder[]>("list_local_subfolders", { key, rel });

/** Persist a tracked folder's excluded subfolders (root-relative paths). Apply it with a
 *  {@link syncLocalFolder} — newly-excluded files soft-remove, un-excluded ones re-index. */
export const setLocalExcludes = (key: string, exclude: string[]) =>
  invoke<void>("set_local_excludes", { key, exclude });

/** Start syncing one folder (or all when `folder` is null). Detached in the backend — progress arrives
 *  via the global `local://sync` event (subscribe with {@link onLocalSync}). */
export const syncLocalFolder = (folder: string | null) =>
  invoke<number>("sync_local_folder", { folder });

/** The current background local-sync snapshot — used to restore the progress UI on returning to Settings. */
export const localFolderSyncStatus = () => invoke<LocalFolderSyncState>("local_folder_sync_status");

/** Ask the running local sync to stop after the current file. Already-indexed files are kept. */
export const stopLocalFolderSync = () => invoke<void>("stop_local_folder_sync");

/** Resume a local sync interrupted by a previous app close/crash mid-index. Called once on launch. */
export const resumeLocalFolderSync = () => invoke<boolean>("resume_local_folder_sync");

/** Subscribe to global local-sync progress (fires regardless of which view started the sync). */
export const onLocalSync = (handler: (e: SyncEvent) => void): Promise<UnlistenFn> =>
  listen<SyncEvent>("local://sync", (e) => handler(e.payload));

/** Fire when the live filesystem watcher applied a batch of changes (a folder's items changed on disk
 *  outside a manual sync) — the UI refetches its folder list so counts/states stay current. */
export const onLocalChanged = (handler: () => void): Promise<UnlistenFn> =>
  listen("local://changed", () => handler());

/** Fetch an index-only document's full body live from its source (the body is never stored), plus
 *  whether the stored chunk offsets still index it exactly (so the reader can draw the overlay or
 *  offer a Re-index). */
export const fetchIndexOnlyBody = (docId: number) =>
  invoke<IndexOnlyFetch>("fetch_index_only_body", { docId });

/** Rebuild one index-only item's stored chunk map + summary against its current live body (the
 *  reader's "Re-index this item" — fixes a stale overlay, e.g. one left indexing the offline summary).
 *  Returns the exact body it embedded (+ aligned=true), so the reader redraws with no second fetch. */
export const reindexIndexOnly = (docId: number) =>
  invoke<IndexOnlyFetch>("reindex_index_only", { docId });

/** Promote an index-only Google Sheet to a full local spreadsheet import ("import fully"): pull the
 *  whole grid, index it locally, and flip the document off index-only. Returns the updated document. */
export const promoteIndexOnly = (docId: number) =>
  invoke<Document>("promote_index_only", { docId });

// ---- Semantic memory map (UMAP/t-SNE) ----

/** The cached semantic layout — coordinates by meaning, plus whether a recompute is in flight. Always
 *  returns immediately (stale-but-cached); the background job does the recompute. */
export const semanticLayout = () => invoke<SemanticLayout>("semantic_layout");

/** Kick off the background layout precompute after unlock (idle priority; defers to a Drive sync). */
export const startSemanticLayout = () => invoke<boolean>("start_semantic_layout");

/** Recompute the layout now if stale, jumping ahead of a Drive sync (called when the Map opens). */
export const prioritiseSemanticLayout = () => invoke<void>("prioritise_semantic_layout");

/** Whether the optional t-SNE reducer (an on-demand download) is installed. */
export const optionalTsneStatus = () => invoke<TsneStatus>("optional_tsne_status");

/** Install the optional t-SNE reducer into the managed venv, then recompute the layout with it. */
export const installOptionalTsne = () => invoke<void>("install_optional_tsne");

/** Whether the optional photo-OCR component (rapidocr + pillow-heif) is installed. */
export const optionalOcrStatus = () => invoke<OcrStatus>("optional_ocr_status");

/** Install the optional photo-OCR component into the managed venv (a one-time download). Progress
 *  rides the `ocr://install` event (see `onOcrInstall`); removal is via `removeStorageComponent`. */
export const installOptionalOcr = () => invoke<void>("install_optional_ocr");

/** Inventory the large on-device components (venv, t-SNE libraries, OCR stack, speech, search model). */
export const listStorageComponents = () => invoke<StorageReport>("list_storage_components");

/** Remove an on-device component, enforced by the dependency cascade server-side. */
export const removeStorageComponent = (id: string) =>
  invoke<void>("remove_storage_component", { id });

/** Subscribe to global semantic-layout progress (fires regardless of which view started the job). */
export const onLayoutProgress = (handler: (e: LayoutProgressEvent) => void): Promise<UnlistenFn> =>
  listen<LayoutProgressEvent>("layout://progress", (e) => handler(e.payload));

/** Subscribe to an optional component's install-download progress on `<component>://install`. One
 *  channel shape for the three optional downloads (t-SNE, OCR, and the macOS Python fetch); the thin
 *  `on*Install` aliases below name each channel for their callers. */
export const onInstallProgress = (
  component: "python" | "tsne" | "ocr",
  handler: (e: InstallProgressEvent) => void,
): Promise<UnlistenFn> =>
  listen<InstallProgressEvent>(`${component}://install`, (e) => handler(e.payload));

/** Subscribe to the optional t-SNE download's progress (fires from whichever view triggered it). */
export const onTsneInstall = (handler: (e: InstallProgressEvent) => void): Promise<UnlistenFn> =>
  onInstallProgress("tsne", handler);

/** Subscribe to the optional OCR download's progress (fires from whichever view triggered it). */
export const onOcrInstall = (handler: (e: InstallProgressEvent) => void): Promise<UnlistenFn> =>
  onInstallProgress("ocr", handler);

/** Subscribe to the macOS interpreter download's progress (fires during first-run setup when PM has
 *  to download Python because none was found on the machine). */
export const onPythonInstall = (handler: (e: InstallProgressEvent) => void): Promise<UnlistenFn> =>
  onInstallProgress("python", handler);

/** Open a document's source: an index-only web link (Drive/OneDrive) opens in the system browser; a
 *  local-folder file path is revealed-and-selected in the OS file manager. Supersedes `open_external_ref`. */
export const openSource = (docId: number) => invoke<void>("open_source", { docId });

/** The reader's body text: a local document's on-disk Markdown body (front-matter stripped, byte-exact to
 *  what the splitter chunked), or an index-only pointer's offline summary. */
export const readDocumentBody = (docId: number) => invoke<string>("read_document_body", { docId });

/** The document's chunk spans (leaves + parents, ordered) — the chunk-boundary overlay's data. */
export const documentChunkSpans = (docId: number) =>
  invoke<ChunkSpan[]>("document_chunk_spans", { docId });

/** The decrypted original image for a photo saved into the vault, as base64 + mime; `null` when none was
 *  saved (the reader then falls back to the OCR body). */
export const readDocumentImage = (docId: number) =>
  invoke<ImageData | null>("read_document_image", { docId });

/** Open an arbitrary http(s) URL in the system browser. Used by the app-wide external-link handler
 *  (the webview can't open `target="_blank"` itself); the backend guards the scheme to http/https. */
export const openUrl = (url: string) => invoke<void>("open_url", { url });

/** Bump the backend user-activity clock (F-08). Called throttled from App's interaction listener so
 *  idle-gated background jobs back off during real use (reading/triaging/editing), not only on chat
 *  sends + ingest. Fire-and-forget: no payload, no return. */
export const markActivity = () => invoke<void>("mark_activity");

// --- Personal Assistant: Daily briefing (Step 7, spec §4 P1) ---

/** The stored "here's your picture today" briefing + whether it's due a refresh. */
export const getDailyBriefing = () => invoke<DailyBriefing>("get_daily_briefing");

/** Regenerate the briefing from the current focus-view state; returns the new one. */
export const refreshDailyBriefing = () => invoke<DailyBriefing>("refresh_daily_briefing");

/** Mark a structured flag done (board card 9) — a deliberate user assertion that outranks detection,
 *  removing it from the rendered set. Optionally names the satisfying artifact by its
 *  `documents.source_id` so a happening-today on the same anchor can show "you're prepared, file's
 *  here". Returns the resolved flag. */
export const resolveFlag = (flagId: number, artifactSourceId?: string) =>
  invoke<Flag>("resolve_flag", { flagId, artifactSourceId });

/** Classify one line typed in the focus box (board card 9) and route it — mark a visible flag done,
 *  capture a durable preference, ask a flag-grounded question, or edit a project. One background
 *  classification call over the visible flags; the frontend acts on the returned route. */
export const routeFocusInput = (text: string) => invoke<FocusRoute>("route_focus_input", { text });

// --- Data folder: reveal + export ---

/** Reveal the data folder (encrypted store + Markdown vault) in the OS file manager. */
export const openDataFolder = () => invoke<void>("open_data_folder");

/** Bundle the data folder into a single .zip at `destPath` (store snapshot + vault;
 *  the regenerable runtime/ is excluded). The store stays encrypted in the archive. */
export const exportAllData = (destPath: string) => invoke<void>("export_all_data", { destPath });

/** "Remove PM data": erase the selected classes of on-machine data (regenerable runtime, vault +
 *  database, OS keychain). Selecting the keychain also revokes Google grants and reports connected
 *  Microsoft accounts (which have no programmatic revoke). Returns a summary; when `quitRequired` is
 *  set the app can no longer run and must be closed. Gated in the UI behind the full confirmation
 *  ladder — never call this without it. */
export const wipePmData = (selection: WipeSelection) =>
  invoke<WipeReport>("wipe_pm_data", { selection });

/** Run an OS user-presence check (Windows Hello / Touch ID) as the penultimate gate of the wipe
 *  ladder. `true` on success, `false` on cancel/failure, and it rejects when the verifier can't run
 *  at all (the caller treats that like a cancel). No session side effect. */
export const confirmWipeIdentity = () => invoke<boolean>("confirm_wipe_identity");

/** Recover from a bricked boot (the store is present but its key was lost, so it fails to open with
 *  "wrong key or corrupt file"): delete the unreadable store + metadata so the next launch starts a
 *  clean, empty vault. Only permitted while a boot open-error is actually carried; keychain secrets
 *  (API keys, etc.) are left intact. The caller relaunches the app afterwards. */
export const resetAfterOpenError = () => invoke<void>("reset_after_open_error");

/** After a full "remove PM completely" wipe, launch the Windows uninstaller (which also removes the
 *  leftover data + WebView2 folders) so nothing of PM remains, then the caller exits. Rejects on
 *  non-Windows or a dev build with no installed uninstaller — the data is already gone regardless. */
export const launchUninstaller = () => invoke<void>("launch_uninstaller");

// --- Encrypted backup (Proton Drive / user cloud) — PR1 local `.pmbackup` archive + restore ---

/** Create an encrypted, portable `.pmbackup` at `destPath`, protected by `passphrase`. Runs detached;
 *  progress arrives via the global `backup://progress` event (subscribe with {@link onBackupProgress}).
 *  Unlike the .zip export, this embeds the DB key inside the encrypted layer, so it restores anywhere. */
export const createLocalBackup = (destPath: string, passphrase: string) =>
  invoke<void>("create_local_backup", { destPath, passphrase });

/** Restore a `.pmbackup` into a fresh folder (validated before anything is promoted). Resolves with a
 *  summary; the live vault is untouched until you {@link switchToVault} to the restored one. */
export const restoreLocalBackup = (srcPath: string, passphrase: string) =>
  invoke<RestoreSummary>("restore_local_backup", { srcPath, passphrase });

/** Point this profile at a restored vault folder and open it (uses the key seeded during restore). */
export const switchToVault = (folder: string) => invoke<void>("switch_to_vault", { folder });

/** The current backup/restore snapshot — restores the progress UI on return + shows the last result. */
export const backupStatus = () => invoke<BackupState>("backup_status");

/** Ask the running backup/restore to stop. A backup's partial output is discarded; a restore leaves the
 *  live vault untouched. */
export const stopBackup = () => invoke<void>("stop_backup");

/** Subscribe to global backup/restore progress (fires regardless of which view started it). */
export const onBackupProgress = (handler: (e: BackupEvent) => void): Promise<UnlistenFn> =>
  listen<BackupEvent>("backup://progress", (e) => handler(e.payload));

/** Whether the official `proton-drive` CLI is installed (for backing up to Proton Drive). When it
 *  isn't, the Backup UI links `install_url` — PM does not download the CLI itself. Probes a manual
 *  override first, then PATH + well-known install/download dirs, so it can be re-called after the
 *  user installs the CLI (no restart). */
export const protonCliStatus = () => invoke<ProtonCliStatus>("proton_cli_status");

/** Remember (or clear, with "") a manual path to the `proton-drive` binary, for when it lives
 *  somewhere auto-detection doesn't look. Rejects a non-empty path that isn't an existing file. */
export const setProtonCliPath = (path: string) => invoke<void>("set_proton_cli_path", { path });

/** Whether the CLI has an active Proton session (+ the account email if available). */
export const protonStatus = () => invoke<ProtonConnStatus>("proton_status");

/** Sign in to Proton Drive — opens the browser and resolves once the flow completes. */
export const protonConnect = () => invoke<void>("proton_connect");

/** Sign out of Proton Drive. */
export const protonDisconnect = () => invoke<void>("proton_disconnect");

/** List PM's encrypted archives already on Proton Drive (newest first). */
export const listProtonBackups = () => invoke<BackupEntry[]>("list_proton_backups");

/** Pack an encrypted archive and push it to Proton Drive. Detached; progress on
 *  `backup://progress` ({@link onBackupProgress}). */
export const backupToProton = (passphrase: string) =>
  invoke<void>("backup_to_proton", { passphrase });

/** Download an archive from Proton Drive (by name) and restore it into a fresh folder; resolves
 *  with a summary. The live vault is untouched until you {@link switchToVault} to it. */
export const restoreFromProton = (name: string, passphrase: string) =>
  invoke<RestoreSummary>("restore_from_proton", { name, passphrase });

/** The current automatic-backup schedule (cadence + retention + keychain opt-in + last run). */
export const getBackupSchedule = () => invoke<BackupSchedule>("get_backup_schedule");

/** Set the automatic-backup cadence + keep-last-N retention. A non-`off` cadence requires a
 *  stored passphrase first ({@link setBackupPassphrase}) or the command rejects. */
export const setBackupSchedule = (frequency: string, retentionN: number) =>
  invoke<void>("set_backup_schedule", { frequency, retentionN });

/** Store the backup passphrase in the OS keychain for unattended (scheduled) backups (opt-in). */
export const setBackupPassphrase = (passphrase: string) =>
  invoke<void>("set_backup_passphrase", { passphrase });

/** Forget the stored backup passphrase and turn automatic backups off. */
export const forgetBackupPassphrase = () => invoke<void>("forget_backup_passphrase");

/** Enable/disable each backup destination for scheduled runs. Enabling Google Drive requires a
 *  granted account (or the command rejects), mirroring the passphrase guard on the schedule. */
export const setBackupDestinations = (protonEnabled: boolean, gdriveEnabled: boolean) =>
  invoke<void>("set_backup_destinations", { protonEnabled, gdriveEnabled });

/** The Google Drive backup status: which account is set up, whether it has the drive.file write
 *  grant, whether it's enabled, and the connected accounts (for the first-grant picker). */
export const backupGdriveStatus = () => invoke<GdriveBackupStatus>("backup_gdrive_status");

/** Grant Google Drive backup access — runs an OAuth re-consent for the drive.file WRITE scope
 *  (connector scopes are read-only). Opens the browser; resolves with the updated status. Pass
 *  `email` to require signing in as a specific already-connected account. */
export const backupGdriveConnect = (email?: string) =>
  invoke<GdriveBackupStatus>("backup_gdrive_connect", { email: email ?? null });

/** Stop backing up to Google Drive (disable + forget the account; the token is kept if the account
 *  is also a read connector). */
export const backupGdriveDisconnect = () => invoke<void>("backup_gdrive_disconnect");

/** List PM's encrypted archives already on Google Drive (newest first). */
export const listGdriveBackups = () => invoke<BackupEntry[]>("list_gdrive_backups");

/** Pack an encrypted archive and push it to Google Drive. Detached; progress on
 *  `backup://progress` ({@link onBackupProgress}). */
export const backupToGdrive = (passphrase: string) =>
  invoke<void>("backup_to_gdrive", { passphrase });

/** Download an archive from Google Drive (by name) and restore it into a fresh folder; resolves
 *  with a summary. The live vault is untouched until you {@link switchToVault} to it. */
export const restoreFromGdrive = (name: string, passphrase: string) =>
  invoke<RestoreSummary>("restore_from_gdrive", { name, passphrase });
