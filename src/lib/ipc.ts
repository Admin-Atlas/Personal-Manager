// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { invoke, Channel } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppLockStatus,
  CalendarEvent,
  CalendarStatus,
  ChatEvent,
  Conversation,
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
  DriveSyncEvent,
  DriveSyncState,
  Entity,
  GoogleCalendar,
  IcsFeedInfo,
  Importance,
  IngestEvent,
  LanguageOptions,
  LayoutProgressEvent,
  Message,
  ModelInfo,
  ModelRecommendations,
  Preference,
  ProjectOverview,
  ProjectProposalEvent,
  ProjectSize,
  ReviewDecision,
  ReviewEvent,
  RetrievedChunk,
  SemanticLayout,
  Settings,
  SharedDrive,
  SidecarStatus,
  TsneInstallEvent,
  TsneStatus,
  VaultLockStatus,
  VaultStatus,
} from "./types";

export const hasOpenRouterKey = () => invoke<boolean>("has_openrouter_key");

export const setOpenRouterKey = (key: string) => invoke<void>("set_openrouter_key", { key });

export const hasOpenRouterBackgroundKey = () => invoke<boolean>("has_openrouter_background_key");

export const setOpenRouterBackgroundKey = (key: string) =>
  invoke<void>("set_openrouter_background_key", { key });

export const getSettings = () => invoke<Settings>("get_settings");

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

// --- Model recommender (spec §6) ---

/** PM's two live model recommendations (Day-to-day / Advanced) for the Settings cards.
 *  Reads the cached catalogue (refreshed on the cost logger's daily cadence). */
export const modelRecommendations = () => invoke<ModelRecommendations>("model_recommendations");

/** Persist the optional recommender denylist (provider or model slugs). */
export const setRecommendDenylist = (denylist: string[]) =>
  invoke<void>("set_recommend_denylist", { denylist });

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

/** Convert this device vault into a shareable, passphrase-protected one (re-keys the
 *  store and encrypts the Markdown via the one migration routine). */
export const createShareableVault = (passphrase: string) =>
  invoke<void>("create_shareable_vault", { passphrase });

/** Change a shareable vault's passphrase: re-derive the key and re-encrypt the Markdown. */
export const changeVaultPassphrase = (newPassphrase: string) =>
  invoke<void>("change_vault_passphrase", { newPassphrase });

/** Make a shareable vault private again: re-key to a device key and decrypt the Markdown. */
export const makeVaultPrivate = () => invoke<void>("make_vault_private");

/** Move the vault to another folder (e.g. a shared location), keeping key + policy. */
export const moveVault = (folder: string) => invoke<void>("move_vault", { folder });

/** Unlock the current passphrase vault for this session and cache the key on this
 *  profile, so the next launch is silent. */
export const unlockVault = (passphrase: string) => invoke<void>("unlock_vault", { passphrase });

/** Point this profile at an existing vault folder (a shared one) and open it. */
export const openExistingVault = (folder: string, passphrase?: string | null) =>
  invoke<void>("open_existing_vault", { folder, passphrase: passphrase ?? null });

/** Forget this profile's cached passphrase key, so it's asked for again next launch.
 *  Does not lock the current session. */
export const forgetVaultPassphrase = () => invoke<void>("forget_vault_passphrase");

/** Grant another account on this machine access to the shared vault folder (a name or
 *  SID). Only meaningful for a shareable vault; a clear error otherwise. */
export const linkVaultAccount = (account: string) =>
  invoke<void>("link_vault_account", { account });

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

// --- Archivist: documents ---

export const sidecarStatus = () => invoke<SidecarStatus>("sidecar_status");

export const ensureSidecar = () => invoke<void>("ensure_sidecar");

export const listDocuments = () => invoke<Document[]>("list_documents");

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

/** Hybrid search over the store, returning the top-k matching chunks. */
export const searchDocuments = (query: string, k?: number) =>
  invoke<RetrievedChunk[]>("search_documents", { query, k });

/** Transcribe a recorded voice clip to text, fully on-device via the sidecar's
 *  Whisper model. `audioBase64` is the standard-base64 of the recording bytes. */
export const transcribeAudio = (audioBase64: string) =>
  invoke<string>("transcribe_audio", { audioBase64 });

/** Ingest files/folders, streaming progress for each item. */
export function ingestPaths(paths: string[], onEvent: (event: IngestEvent) => void): Promise<void> {
  const channel = new Channel<IngestEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("ingest_paths", { paths, onEvent: channel });
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

// --- Archivist: sorting review & organisation (Step 4) ---

/** Distinct project labels across all documents. */
export const listProjects = () => invoke<string[]>("list_projects");

/** Documents still awaiting the sorting review (`reviewed = false`). */
export const reviewQueue = () => invoke<Document[]>("review_queue");

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

/** Point one document at a different existing entity — the misfile/reassignment case. */
export const reassignDocument = (documentId: number, entityId: number) =>
  invoke<void>("reassign_document", { documentId, entityId });

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
  },
) =>
  invoke<void>("set_project_metadata", {
    name,
    deadline: meta.deadline ?? null,
    size: meta.size ?? null,
    blockedBy: meta.blockedBy ?? null,
    parent: meta.parent ?? null,
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

// --- Personal Assistant: Calendar (Step 6, spec §8.6) ---

/** State of the calendar connector (both .ics feeds and Google OAuth). */
export const calendarStatus = () => invoke<CalendarStatus>("calendar_status");

// .ics feeds — the simple no-OAuth path (works under Advanced Protection).

/** Subscribed feeds (without their secret URLs). */
export const listIcsFeeds = () => invoke<IcsFeedInfo[]>("list_ics_feeds");

/** Add an .ics feed (e.g. a Google "secret address in iCal format") and sync it. */
export const addIcsFeed = (label: string, url: string) =>
  invoke<void>("add_ics_feed", { label, url });

/** Remove a feed and its synced events. */
export const removeIcsFeed = (id: string) => invoke<void>("remove_ics_feed", { id });

/** Save the user's BYO Google "Desktop app" client credentials (keychain only). */
export const setGoogleClient = (clientId: string, clientSecret: string) =>
  invoke<void>("set_google_client", { clientId, clientSecret });

/** Forget the client credentials (also disconnects + clears the mirror). */
export const clearGoogleClient = () => invoke<void>("clear_google_client");

/** Run the OAuth consent flow — opens the system browser; resolves on sign-in. */
export const connectGoogle = () => invoke<void>("connect_google");

/** Sign out: forget the token and clear mirrored events (creds kept). */
export const disconnectGoogle = () => invoke<void>("disconnect_google");

/** The user's calendars, with PM's selection applied (for the picker). */
export const listGoogleCalendars = () => invoke<GoogleCalendar[]>("list_google_calendars");

/** Choose which calendars to sync. */
export const setGoogleCalendarIds = (ids: string[]) =>
  invoke<void>("set_google_calendar_ids", { ids });

/** Pull events from the selected calendars into the local mirror; returns the count. */
export const syncCalendar = () => invoke<number>("sync_calendar");

/** Upcoming events in the mirror, for the focus-view agenda. */
export const listCalendarEvents = () => invoke<CalendarEvent[]>("list_calendar_events");

// --- Google Drive (index-only connector, board card 4A) ---

/** The Drive connector's state: whether the shared Google client is set up + connected accounts. */
export const driveStatus = () => invoke<DriveStatus>("drive_status");

/** The connected Drive accounts (each independent). */
export const listDriveAccounts = () => invoke<DriveAccount[]>("list_drive_accounts");

/** Connect a Google Drive account — opens the browser; resolves with the connected account. */
export const connectDrive = () => invoke<DriveAccount>("connect_drive");

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
export const onDriveSync = (handler: (e: DriveSyncEvent) => void): Promise<UnlistenFn> =>
  listen<DriveSyncEvent>("drive://sync", (e) => handler(e.payload));

/** The shared drives one account can see (for the "add shared drives" picker). */
export const listDriveSharedDrives = (email: string) =>
  invoke<SharedDrive[]>("list_drive_shared_drives", { email });

/** Immediate subfolders of a folder in a shared drive (one lazy picker level). Pass the shared
 *  drive's id as `parentId` for the top level. */
export const listDriveFolders = (email: string, driveId: string, parentId: string) =>
  invoke<DriveFolder[]>("list_drive_folders", { email, driveId, parentId });

/** Read one account's indexing scope (My Drive on/off + opted-in shared drives and folders). */
export const getDriveScope = (email: string) => invoke<DriveScope>("get_drive_scope", { email });

/** Persist one account's indexing scope; follow with `syncDrive(email)` to apply it. */
export const setDriveScope = (email: string, scope: DriveScope) =>
  invoke<void>("set_drive_scope", { email, scope });

/** Fetch an index-only document's full body live from its source (the body is never stored). */
export const fetchIndexOnlyBody = (docId: number) =>
  invoke<string>("fetch_index_only_body", { docId });

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

/** Remove the optional t-SNE reducer from the venv (the "delete" action), then recompute with PCA. */
export const uninstallOptionalTsne = () => invoke<void>("uninstall_optional_tsne");

/** Subscribe to global semantic-layout progress (fires regardless of which view started the job). */
export const onLayoutProgress = (handler: (e: LayoutProgressEvent) => void): Promise<UnlistenFn> =>
  listen<LayoutProgressEvent>("layout://progress", (e) => handler(e.payload));

/** Subscribe to the optional t-SNE download's progress (fires from whichever view triggered it). */
export const onTsneInstall = (handler: (e: TsneInstallEvent) => void): Promise<UnlistenFn> =>
  listen<TsneInstallEvent>("tsne://install", (e) => handler(e.payload));

/** Open an index-only document's source in the system browser (its Drive link). */
export const openExternalRef = (docId: number) => invoke<void>("open_external_ref", { docId });

/** Open an arbitrary http(s) URL in the system browser. Used by the app-wide external-link handler
 *  (the webview can't open `target="_blank"` itself); the backend guards the scheme to http/https. */
export const openUrl = (url: string) => invoke<void>("open_url", { url });

// --- Personal Assistant: Daily briefing (Step 7, spec §4 P1) ---

/** The stored "here's your picture today" briefing + whether it's due a refresh. */
export const getDailyBriefing = () => invoke<DailyBriefing>("get_daily_briefing");

/** Regenerate the briefing from the current focus-view state; returns the new one. */
export const refreshDailyBriefing = () => invoke<DailyBriefing>("refresh_daily_briefing");

// --- Data folder: reveal + export ---

/** Reveal the data folder (encrypted store + Markdown vault) in the OS file manager. */
export const openDataFolder = () => invoke<void>("open_data_folder");

/** Bundle the data folder into a single .zip at `destPath` (store snapshot + vault;
 *  the regenerable runtime/ is excluded). The store stays encrypted in the archive. */
export const exportAllData = (destPath: string) => invoke<void>("export_all_data", { destPath });
