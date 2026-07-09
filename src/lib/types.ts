// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

export interface Conversation {
  id: number;
  title: string;
  created_at: string;
  updated_at: string;
  /** The project this chat is scoped to (Step 5), or null for a global chat. */
  project?: string | null;
}

export interface Message {
  id: number;
  conversation_id: number;
  role: "user" | "assistant" | "system";
  content: string;
  model: string | null;
  created_at: string;
  /** Source documents this answer drew from (assistant turns only). */
  citations?: Citation[] | null;
}

export interface Settings {
  /** Ordered preferred models (first = primary) for chat and for background work.
   *  The rest are auto-switch fallbacks when the matching toggle is on. */
  chat_models: string[];
  background_models: string[];
  chat_auto_switch: boolean;
  background_auto_switch: boolean;
  /** When on, hovering a section shows an explanation panel (Step 4b help mode). */
  help_mode: boolean;
  /** The user's IANA time zone (e.g. "Europe/London"), or "" if not yet set — the UI
   *  auto-detects + persists one on first load. Drives the focus-view day boundaries
   *  and the briefing/agenda "now". */
  time_zone: string;
  /** Whether query-time reranking is on (a cross-encoder re-scores search hits for sharper
   *  relevance). Default on; stateless, so toggling it never triggers a Rebuild. */
  reranking: boolean;
  /** Indexing speed: "fast" (default, max throughput) or "gentle" (paced so a low-end machine
   *  stays usable while indexing runs in the background). */
  indexing_speed: string;
  /** Retrieval depth `k` — how many fused candidates reach the reranker (card 7H). The lever the
   *  in-chat Retrieval-explain panel tunes; default 6, stateless. */
  retrieval_k: number;
}

/** One search-language / embedder choice offered at vault creation. */
export interface LanguageOption {
  id: string;
  label: string;
  multilingual: boolean;
}

/** The vault's search-language choices (onboarding picker + Settings switcher): the selectable
 *  embedders, the current selection, and whether the vault already has documents. `has_documents`
 *  is true when switching the language means a re-index (the UI confirms + runs the guided
 *  Re-index) rather than a free choice on an empty vault. */
export interface LanguageOptions {
  options: LanguageOption[];
  selected: string;
  has_documents: boolean;
}

/** State of the optional biometric app-lock (a soft UI gate; the store is encrypted at
 *  rest regardless). `available` is whether the OS can verify (Windows Hello enrolled /
 *  Touch ID) — the Settings toggle is disabled when it's false. */
export interface AppLockStatus {
  enabled: boolean;
  available: boolean;
  /** The launch gate: enabled and not yet verified this session. Computed backend-side. */
  locked: boolean;
}

// --- "Remove PM data" teardown (Settings → Data & Security) ---

/** Which classes of PM's on-machine data to remove. Browser local storage is cleared in the
 *  webview (not part of the backend command), so it isn't represented here. */
export interface WipeSelection {
  /** The regenerable runtime (Python engine, t-SNE/OCR, Whisper model) — re-downloads on next use. */
  regenerable: boolean;
  /** The Markdown vault + encrypted database (the real user data). Irreversible. */
  vaultAndDb: boolean;
  /** Every OS-keychain secret; implies revoking Google grants + reporting Microsoft accounts. */
  keychain: boolean;
}

/** What a wipe actually did, for the "done" summary. All counts are best-effort. */
export interface WipeReport {
  /** Human-readable labels of the classes removed. */
  removed: string[];
  /** Approx bytes freed on disk. */
  freedBytes: number;
  /** Google grants revoked at Google's end. */
  googleRevoked: number;
  /** Google tokens that couldn't be revoked (offline / already invalid); local copy gone regardless. */
  googleRevokeFailures: number;
  /** Connected Microsoft account emails — no programmatic revoke, so the user finishes at
   *  account.microsoft.com (the UI links there). */
  microsoftAccounts: string[];
  /** Keychain entries deleted. */
  keychainDeleted: number;
  /** True when the store or keychain was touched, so the app can't keep running and must close. */
  quitRequired: boolean;
}

// --- Shared & portable vaults (spec §2–6) ---

/** How the vault's SQLCipher key is held: a random key in this device's keychain (the
 *  default, bound to one OS profile) or one derived from a passphrase (openable from any
 *  profile/machine that knows it). */
export type VaultMode = "device" | "passphrase";

/** The vault's current state for the UI: its key mode, whether it needs unlocking on
 *  this profile (a passphrase vault whose key isn't cached here yet), whether the
 *  Markdown is encrypted at rest, where it lives on disk, and its stable id. */
export interface VaultStatus {
  mode: VaultMode;
  needs_unlock: boolean;
  markdown_encrypted: boolean;
  location: string;
  vault_id: string | null;
  /** Whether the stored search index was produced by a different retrieval config than this
   *  build (a model, chunking, or splitter change) — i.e. a one-time Rebuild is recommended.
   *  The Documents view shows this as a dismissible banner. False when locked or empty. */
  retrieval_rebuild_needed: boolean;
  /** A friendly, retryable message when the store *failed to open* at boot (a transient AV /
   *  search-indexer file lock, disk I/O) — distinct from a locked passphrase vault, which sets
   *  `needs_unlock` instead. The app shows a Retry surface; null in the normal case. */
  open_error: string | null;
}

/** Cooperative single-writer state for a shared vault. `active` = this instance is the
 *  writer (always true for a device vault); `contended` = another live profile holds it
 *  (the curtain shows); `stale` = that holder looks crashed, so a warned force-take is
 *  offered; `other_profile` names it for the UI. */
export interface VaultLockStatus {
  active: boolean;
  contended: boolean;
  stale: boolean;
  other_profile: string | null;
}

/** A model from the OpenRouter catalogue, for the Settings model picker.
 *  Prices are USD per token (the picker renders them per-million). */
export interface ModelInfo {
  id: string;
  name: string;
  description: string;
  context_length: number | null;
  prompt_price: number | null;
  completion_price: number | null;
  input_modalities: string[];
}

/** Spend for one model over a window (the cost logger). `cost_usd` is null when the
 *  model isn't in the price cache yet — shown as "—", not an understated $0. */
export interface ModelSpend {
  model: string;
  prompt_tokens: number;
  completion_tokens: number;
  request_count: number;
  cost_usd: number | null;
}

/** The Settings "Usage & cost" payload (token spend priced from OpenRouter). */
export interface CostSummary {
  last_30d: ModelSpend[];
  all_time: ModelSpend[];
  total_30d_usd: number | null;
  total_all_time_usd: number | null;
  pricing_updated_at: string | null;
}

/** One recommended model (spec §6) — proposed, not applied; the user assigns it to a
 *  role and Saves. `effective_cost_per_mtok` is cache-weighted USD per million tokens. */
export interface ModelRecommendation {
  model: string;
  name: string;
  why: string;
  effective_cost_per_mtok: number | null;
  context_length: number | null;
  /** The live Artificial-Analysis capability index, when the catalogue had one (Advanced). */
  intelligence_index: number | null;
  /** True when the model is also on PM's curated faithfulness list. */
  curated: boolean;
}

/** PM's two model recommendations for the Settings cards. A pick is null when the cache
 *  can't yet produce one (offline before any fetch). `zdr_enforced` is always true — PM
 *  sends Zero-Data-Retention on every request; `stale` flags a cache older than the daily
 *  refresh window. */
export interface ModelRecommendations {
  day_to_day: ModelRecommendation | null;
  advanced: ModelRecommendation | null;
  denylist: string[];
  zdr_enforced: boolean;
  stale: boolean;
}

/** A document an answer cited — the provenance shown under it. */
export interface Citation {
  document_id: number;
  title: string;
  source_path: string | null;
  vault_path: string;
  /** Chat-source provenance (board card 7E PR3): when the citation is a past chat, these point back
   *  to the exact turn so clicking it reopens that conversation there. Absent/false for a document
   *  (and for citations persisted before this shipped). */
  is_chat?: boolean;
  conversation_id?: number | null;
  turn_id?: number | null;
  dated?: string | null;
}

export type ChatEvent =
  | { type: "token"; text: string }
  | { type: "done"; message_id: number; content: string; citations: Citation[] }
  | { type: "error"; message: string };

/** A larger-context model the Upgrade option can switch to (board card 7D). */
export interface ModelOption {
  id: string;
  name: string;
  context_length: number;
}

/** Whether Compress can reclaim context, and how much would fold (board card 7D). */
export interface CompressDecision {
  available: boolean;
  foldable: number;
  reason: string | null;
}

/** How full the selected model's context window is for a conversation, plus what the user can do
 *  about it (board card 7D, `chat_context_status`). `context_window`/`used_tokens`/`percent` are null
 *  when unknown (a custom model with no catalogued window, or no reply measured yet) ⇒ the meter shows
 *  "unknown" and never alerts. `percent` is a 0–1 fraction (the bar caps it at 1). */
export interface ContextStatus {
  model: string;
  context_window: number | null;
  used_tokens: number | null;
  percent: number | null;
  alerting: boolean;
  compress: CompressDecision;
  upgrade: ModelOption[];
}

/** The pre-compress state to Undo with — held by the UI and echoed back to `revert_compress`. */
export interface CompressSnapshot {
  prev_summary: string | null;
  prev_cursor: number | null;
  prev_prompt_tokens: number | null;
}

/** Result of a Compress (board card 7D, `compress_chat`): the bullets just folded in (the HITL "what was
 *  condensed" the user verifies), a rough token reclaim, and the snapshot to Undo with. */
export interface CompressResult {
  condensed_bullets: string;
  reclaimed_est: number;
  snapshot: CompressSnapshot;
}

/** Ranked importance of a document. `archive` is an explicit "shelved" level (hidden from the Map,
 *  sunk to the bottom of lists, still searchable); `null` is untriaged / unset — a distinct state. */
export type Importance = "high" | "medium" | "low" | "archive" | null;

export interface Document {
  id: number;
  title: string;
  source_path: string | null;
  ext: string | null;
  byte_size: number | null;
  chunk_count: number;
  created_at: string | null;
  ingested_at: string;
  /** Organisation metadata (Step 4). */
  project: string;
  tags: string[];
  importance: Importance;
  reviewed: boolean;
  last_activity: string | null;
  /**
   * "vault" — a fully-stored document (its body lives in the Markdown vault); "index_only" — a
   * pointer we index but don't hold (body fetched live, only a summary readable offline); "chat" — a
   * conversation, born as a document on first index and backed by a Markdown vault file (epic 7);
   * "photo" — an image (OCR + EXIF, synthetic Markdown body); "spreadsheet" — a workbook rendered to a
   * synthetic Markdown body. All non-index_only types resolve to an on-disk Markdown body for the reader.
   */
  source_type: "vault" | "index_only" | "chat" | "photo" | "spreadsheet";
  /**
   * Reachability of an index-only item's source. "ok" normally; "source_missing" (deleted at the
   * source, kept findable) and "unreachable" (expired auth / offline drive) are set by the
   * observe-and-react layer. Always "ok" for a vault document.
   */
  source_state: "ok" | "source_missing" | "unreachable";
  /** Source URL / id shown for an index-only item in place of a local `source_path`. */
  external_ref: string | null;
  /** The stable source id for an index-only item (`null` for a vault document). */
  source_id: string | null;
}

/**
 * One chunk's span in a document, for the reader's chunk-boundary overlay. `start_offset`/`end_offset`
 * are BYTE offsets into the document body (see `readDocumentBody`); they are `null` for chunk kinds that
 * predate the offset columns (chat turns). Leaves (`kind === "leaf"`) are the embedded units; `parent_id`
 * groups sibling leaves under a structural parent (parents are never embedded).
 */
export interface ChunkSpan {
  id: number;
  ordinal: number;
  parent_id: number | null;
  kind: string;
  start_offset: number | null;
  end_offset: number | null;
}

/** A decrypted image served to the reader as base64 + mime, for a `data:` URL (the asset protocol is off
 *  and a vault-saved original may be ciphertext). */
export interface ImageData {
  base64: string;
  mime: string;
}

/** The AI's proposed organisation for a document, shown in the Review view. */
export interface MetadataProposal {
  project: string;
  tags: string[];
  importance: Importance;
  reasoning: string;
}

/** A confirmed review decision sent to `commit_review`. Carries the AI proposal
 *  so the backend logs only the fields the user actually changed. */
export interface ReviewDecision {
  document_id: number;
  project: string;
  tags: string[];
  importance: Importance;
  proposed_project: string;
  proposed_tags: string[];
  proposed_importance: Importance;
}

/** Streamed as proposals come back from `propose_metadata`. */
export type ReviewEvent =
  | { type: "proposed"; document_id: number; proposal: MetadataProposal }
  | { type: "finished"; proposed: number };

// --- Canonical entities (the Teach tab; entity-resolution foundation) ---

/** A canonical entity with its known aliases — the unit the Teach tab manages. `type` is
 *  "project" today (the schema reserves "person"/"thing"). The canonical name is itself one of
 *  `aliases` (stored as a self-alias), so every name a document was filed under is listed. */
export interface Entity {
  id: number;
  type: string;
  canonical_name: string;
  aliases: string[];
  /** DB-only derived confidence in [0,1] (1.0 under today's exact-match resolution). Surfaced for
   *  future use; not yet shown in the Teach tab. */
  confidence: number;
  /** Whether the user has deliberately vouched for this entity (rename / merge / add-alias /
   *  explicit review correction). Portable truth, carried in the encrypted rules file. */
  user_confirmed: boolean;
}

// --- Structured preferences (spec §4.5 — the typed model that replaces the Learning-You blob) ---

/** One structured preference record — PM's memory of the user as a typed, queryable rule, retrieved
 *  by scope+condition at the decision point instead of one prose blob injected whole. The Teach tab
 *  manages these. */
export interface Preference {
  id: number;
  /** "global" (always) | "project" (one entity) | "context" (a stated situation). */
  scope: string;
  /** Set iff `scope === "project"` — the canonical entity this is about. */
  entity_id: number | null;
  /** Joined canonical name for `entity_id` (display-only). */
  project_name: string | null;
  /** When it applies — the predicate text for a context preference (null for global/project). */
  condition: string | null;
  value: string;
  /** "user" (explicitly stated) | "inferred" (distilled from the legacy blob / behaviour). */
  source: string;
  /** Revisable confidence in [0,1] (1.0 for user-stated; lower for inferred). */
  confidence: number;
  /** Whether the user has vouched for this record (stated, edited, or confirmed in Teach). */
  user_confirmed: boolean;
  created_at: string;
  updated_at: string;
}

/** A preference without an id yet — what the natural-language parse returns (for the form to
 *  prefill) and the shape the structured add path builds. `project_name` is display-only; the
 *  backend has resolved it to `entity_id` for a project-scoped record. */
export interface DraftPreference {
  scope: string;
  entity_id: number | null;
  project_name: string | null;
  condition: string | null;
  value: string;
}

// --- Personal Assistant: focus view & projects (Step 5, spec §4.1) ---

/** The one status a project shows in the focus view. Exactly one applies. */
export type ProjectStatus =
  "due_soon" | "blocked" | "quick_win" | "take_a_look" | "part_of" | "on_track";

/** A rough effort estimate for a project ("quick" → Quick win), or unset. */
export type ProjectSize = "quick" | "standard" | "large" | null;

/** One project milestone (card 7). A milestone is either PM-native (a user-set,
 *  editable `due_date`) or calendar-linked (`calendar_linked` / `event_uid` set), in
 *  which case `due_date` is the read-only date synced from the calendar event. */
export interface Milestone {
  id: number;
  project_name: string;
  label: string;
  /** The effective date — user-set for PM-native, synced from the calendar for linked. */
  due_date: string | null;
  event_uid: string | null;
  /** True when the date comes from a linked calendar event (read-only). */
  calendar_linked: boolean;
  /** True for a linked milestone whose event isn't in the current mirror (gone/unsynced). */
  event_missing: boolean;
  state: "met" | "unmet" | null;
  sort_order: number;
}

/** The milestone driving a project's status + card line (nearest unmet). */
export interface GoverningMilestone {
  id: number;
  label: string;
  due_date: string | null;
}

/** One focus-view row: a project, its derived status, and the signals behind it. */
export interface ProjectOverview {
  name: string;
  status: ProjectStatus;
  doc_count: number;
  last_activity: string | null;
  /** Legacy single deadline — superseded by `milestones` (card 7); kept for compat. */
  deadline: string | null;
  size: ProjectSize;
  blocked_by: string | null;
  parent: string | null;
  importance: Importance;
  /** Computed structural auto-importance — the value "Auto" resolves to (independent of
   *  the manual `importance` override). `null` when nothing depends on this project. */
  auto_importance: Importance;
  /** The upcoming calendar event whose title names this project (Step 6) — the
   *  zero-milestone fallback that drives "Due soon" only when the project has none. */
  calendar_event: CalendarMatch | null;
  /** All milestones, resolved (calendar-linked dates synced) and date-ordered. */
  milestones: Milestone[];
  /** The governing milestone (nearest unmet) driving the status + card line, if any. */
  governing_milestone: GoverningMilestone | null;
}

/** The AI's proposed triage metadata for a project (AI-proposes-you-confirm). */
export interface ProjectProposal {
  size: ProjectSize;
  parent: string | null;
  blocked_by: string | null;
  deadline: string | null;
  reasoning: string;
}

/** Streamed as project proposals come back from `propose_project_metadata`. */
export type ProjectProposalEvent =
  | { type: "proposed"; project: string; proposal: ProjectProposal }
  | { type: "finished"; proposed: number };

// --- Personal Assistant: Calendar connectors (multi-provider, read-only — cards 6A/6B) ---

/** A connected calendar account (Google/Outlook) or iCal subscription — one connector source. */
export interface CalendarAccount {
  /** Stable source id: `gcal:<email>` | `outlook:<email>` | `ics:<hex>`. */
  id: string;
  /** `google` | `microsoft` | `apple` | `other`. */
  provider: string;
  email: string | null;
  label: string;
  state: "ok" | "unreachable" | "error";
  last_synced_at: string | null;
}

/** One calendar within an account/subscription — the picker + unified-view unit. */
export interface Calendar {
  /** Stable mirror id: `<source>:<remoteId>` for OAuth, the feed id for a subscription. */
  id: string;
  source_id: string;
  provider: string;
  remote_id: string | null;
  name: string;
  color: string | null;
  selected: boolean;
  is_primary: boolean;
}

/** The whole calendar surface in one read (every provider). */
export interface CalendarOverview {
  google_client_configured: boolean;
  microsoft_client_configured: boolean;
  accounts: CalendarAccount[];
  calendars: Calendar[];
  /** ISO timestamp of the last successful sync, if any. */
  last_sync: string | null;
  /** How far ahead the focus-view agenda looks (the narrow horizon), in days. */
  window_days: number;
  /** The mirrored band start (RFC3339): the unified view shows an "outside synced range" hint
   *  when the user pages before this. */
  mirror_start: string;
  /** The mirrored band end (RFC3339): the unified view shows an "outside synced range" hint when
   *  the user pages after this. */
  mirror_end: string;
}

/** A subscribed iCal feed, without its secret URL (for display). `provider` tags it for grouping. */
export interface IcsFeedInfo {
  id: string;
  label: string;
  /** `apple` | `outlook` | `google` | `other`. */
  provider: string;
}

// --- Shared detached-sync types (index-only connectors: Drive / OneDrive / local folders) ---
// The Drive, OneDrive, and local-folder connectors all run the same detached, single-flighted,
// stop-able index-only sync, so they share one report/issue/event shape (mirrors the Rust structs).
// Each connector's in-flight *snapshot* keeps its own target field (`account` vs `folder`).

/** A file a sync tried to index but couldn't (an unsupported/empty type, or a fetch/read error),
 *  surfaced in the post-sync report so the user knows exactly what was left out. */
export interface SyncIssue {
  name: string;
  reason: string;
}

/** The outcome of one index-only sync pass: indexed/updated/removed counts, the not-indexed list,
 *  and whether the user stopped it early. Shared by every index-only connector. */
export interface SyncReport {
  indexed: number;
  updated: number;
  removed: number;
  skipped: number;
  failed: number;
  /** The user pressed Stop — already-indexed files are kept; the rest were left for next time. */
  cancelled: boolean;
  /** Files attempted but not indexed (capped; see `issues_truncated`). */
  issues: SyncIssue[];
  /** True when more files couldn't be indexed than `issues` lists. */
  issues_truncated: boolean;
}

/** Streamed progress while a sync runs (mapped onto the shared IngestProgress bar). */
export type SyncEvent =
  | { type: "counted"; total: number }
  | { type: "item"; processed: number; total: number; name: string }
  | { type: "finished"; report: SyncReport };

/** A connected Google Drive account (Connectors → Drive). Each is independent — its own token,
 *  sync cursor, and indexed items. */
export interface DriveAccount {
  id: string;
  email: string;
  label: string;
  last_synced_at: string | null;
  state: "ok" | "unreachable" | "error";
  /** How many index-only documents this account currently has. */
  indexed: number;
  /** Whether this account granted the read-only Sheets scope. `false` for accounts connected before
   *  Sheets support — their Google Sheets index by name only until the user reconnects. */
  has_sheets_scope: boolean;
}

/** The Drive connector's state for Settings. */
export interface DriveStatus {
  /** The shared BYO Google client is configured (provider-level). */
  oauth_client_configured: boolean;
  accounts: DriveAccount[];
}

/** A shared drive (Team Drive) an account can see — from `drives.list`, for the "add" picker. */
export interface SharedDrive {
  id: string;
  name: string;
}

/** A folder inside a (shared) drive — one node of the folder picker's lazy tree. */
export interface DriveFolder {
  id: string;
  name: string;
}

/** One shared drive an account opted into, and how much of it to index. */
export interface SharedSelection {
  drive_id: string;
  name: string;
  /** `null` = the entire shared drive; otherwise index only these folders (recursively). */
  folders: string[] | null;
}

/** What one account indexes: the personal My Drive plus any opted-in shared drives. Default scope is
 *  My Drive on, no shared drives — so a freshly-connected account behaves exactly as before. */
export interface DriveScope {
  my_drive: boolean;
  /** `null` = the entire My Drive (delta-cursor sync); otherwise index only these folders
   *  (recursively, re-enumerated each sync). */
  my_drive_folders: string[] | null;
  shared: SharedSelection[];
}

/** Snapshot of an in-flight Drive sync, so the UI can resume showing progress after navigating away
 *  and back. `running` is false when nothing is syncing; `last_report` holds the most recent result
 *  (so a user returning after it finished still sees the summary). */
export interface DriveSyncState {
  running: boolean;
  processed: number;
  total: number | null;
  /** The account (email) being synced, or null for an all-accounts pass. */
  account: string | null;
  last_report: SyncReport | null;
}

// --- Encrypted backup (Proton Drive / user cloud) — PR1 local `.pmbackup` archive + restore ---

/** Strength of a candidate passphrase, from the `score_passphrase` command (M-4). Mirrors the
 *  backend floor (`validate_passphrase_strength`) so the meter and the gate agree. Advisory — the
 *  command layer is the real check. */
export interface PassphraseScore {
  /** zxcvbn strength, 0 (weakest) .. 4 (strongest). */
  score: number;
  /** True iff it clears the create/change floor (length AND score). */
  acceptable: boolean;
  /** Non-empty but below the length floor. */
  too_short: boolean;
  /** A short human warning when weak, else null. */
  warning: string | null;
  /** Actionable suggestions to strengthen it. */
  suggestions: string[];
}

/** Which stage a backup/restore is in (mirrors the Rust `BackupPhase`). */
export type BackupPhase = "snapshot" | "pack" | "upload" | "download" | "restore" | "validate";

/** The outcome of a finished backup/restore, kept in the snapshot so the UI still shows it on return. */
export interface BackupReport {
  kind: "backup" | "restore";
  vault_id: string | null;
  /** Where a restore materialized the vault (absolute path) — for a follow-up "switch". Null for a backup. */
  target_dir: string | null;
  /** The archive's creation timestamp (RFC3339), surfaced on restore. */
  created_at: string | null;
  /** Destinations that failed this run while at least one other succeeded (F-22), as
   *  `"<label>: <error>"` strings — for a non-blocking "backed up, but X failed" banner. Empty on a
   *  clean run and always empty for a restore. */
  failed_destinations: string[];
}

/** Snapshot of an in-flight backup/restore, so the UI resumes progress after navigating away. */
export interface BackupState {
  running: boolean;
  phase: BackupPhase | null;
  fraction: number;
  last_report: BackupReport | null;
  last_error: string | null;
  /** A restored vault still waiting to be switched to, so the Backup panel can re-offer the
   *  "switch to it" button after being closed and reopened (null when nothing is staged). */
  pending_restore: RestoreSummary | null;
}

/** Streamed backup/restore progress (mapped onto the shared IngestProgress bar in percent mode). */
export type BackupEvent =
  | { type: "phase"; phase: BackupPhase; fraction: number }
  | { type: "finished"; report: BackupReport }
  | { type: "failed"; message: string };

/** A restore's frontend-safe summary (no embedded key) — lets the UI offer "switch to it now". */
export interface RestoreSummary {
  vault_id: string;
  key_mode: string;
  markdown_encryption: string;
  app_version: string;
  created_at: string;
  target_dir: string;
}

/** Whether the official `proton-drive` CLI is installed (for backing up to Proton Drive). PM
 *  does not bundle it; when `installed` is false the UI links `install_url` to install it. */
export interface ProtonCliStatus {
  installed: boolean;
  /** Resolved executable path when found, else null. */
  path: string | null;
  install_url: string;
}

/** Whether the installed CLI has an active Proton session. A clean "not signed in" is
 *  `connected: false` with no `error`; `error` is a real failure (network / CLI crash). */
export interface ProtonConnStatus {
  connected: boolean;
  /** The signed-in account email, when the CLI surfaced it. */
  account: string | null;
  error: string | null;
}

/** One of PM's encrypted archives already at a backup destination (Proton or Google Drive), for
 *  the restore picker. Both destinations use the same archive naming, so this shape is shared. */
export interface BackupEntry {
  name: string;
  /** Cleartext size in bytes, when reported. */
  size: number | null;
}

/** Automatic-backup schedule: one cadence + keep-last-N retention + the keychain opt-in state,
 *  fanned out to every enabled destination. A non-`off` cadence requires `passphrase_stored`
 *  (unattended runs can't prompt); Google backups additionally require a granted `gdrive_account`. */
export interface BackupSchedule {
  frequency: "off" | "daily" | "weekly" | "monthly";
  retention_n: number;
  passphrase_stored: boolean;
  /** When the last automatic backup completed (RFC3339), or null. */
  last_backup_at: string | null;
  /** Whether scheduled runs push to Proton Drive (defaults on). */
  proton_enabled: boolean;
  /** Whether scheduled runs push to Google Drive (opt-in). */
  gdrive_enabled: boolean;
  /** The Google account (email) chosen for backup, or null if none is set up. */
  gdrive_account: string | null;
  /** Per-destination last-success stamps (F-22, RFC3339 or null) — distinct from `last_backup_at`
   *  (the shared cadence clock), these reveal a destination that has gone stale while a sibling keeps
   *  succeeding. */
  proton_last_backup_at: string | null;
  gdrive_last_backup_at: string | null;
}

/** A connected Google Drive account (mirrors the Rust `DriveAccount`) — for the backup account
 *  picker on first grant. */
export interface DriveBackupAccount {
  id: string;
  email: string;
  label: string;
  last_synced_at: string | null;
  state: string;
  indexed: number;
}

/** The Google Drive backup destination's status: which account is set up, whether it has the
 *  `drive.file` write grant yet (a re-consent is required — connector scopes are read-only),
 *  whether it's enabled, and the connected accounts to choose from on first grant. */
export interface GdriveBackupStatus {
  account: string | null;
  has_write_scope: boolean;
  enabled: boolean;
  accounts: DriveBackupAccount[];
}

// --- Personal Assistant: Microsoft OneDrive connector (board card 4B, spec §8.1) ---
// A near-mirror of the Google Drive types above. One personal drive per account (no shared drives),
// indexed whole (delta cursor) or folder-scoped.

/** A connected Microsoft OneDrive account (Connectors → Drive). Each is independent — its own token,
 *  sync cursor, and indexed items. */
export interface OneDriveAccount {
  id: string;
  email: string;
  label: string;
  last_synced_at: string | null;
  state: "ok" | "unreachable" | "error";
  /** How many index-only documents this account currently has. */
  indexed: number;
}

/** The OneDrive connector's state for Settings. */
export interface OneDriveStatus {
  /** The BYO Microsoft client id is configured (provider-level). */
  oauth_client_configured: boolean;
  accounts: OneDriveAccount[];
}

/** A folder inside the drive — one node of the OneDrive folder picker's lazy tree. */
export interface OneDriveFolder {
  id: string;
  name: string;
}

/** What one account indexes: the whole personal OneDrive, or the chosen folders. Default scope is the
 *  whole drive — so a freshly-connected account indexes everything. */
export interface OneDriveScope {
  /** `null` = the entire OneDrive (delta-cursor sync); otherwise index only these folders
   *  (recursively, re-enumerated each sync). */
  folders: string[] | null;
}

/** Snapshot of an in-flight OneDrive sync, so the UI can resume showing progress after navigating
 *  away and back. */
export interface OneDriveSyncState {
  running: boolean;
  processed: number;
  total: number | null;
  account: string | null;
  last_report: SyncReport | null;
}

// --- Local folders (index-only connector, board card 6) ---

/** A tracked local folder as the Connectors UI lists it (mirrors the Rust `LocalFolder`). Every file
 *  inside is index-only — a searchable pointer + summary; the bytes stay on disk and are read on
 *  demand. `present` is whether the path is a readable directory right now (an unmounted/removed root
 *  reads `false` even while `state` is still `ok`, so the row can nudge before the next sync flags it). */
export interface LocalFolder {
  /** The stable folder key (the connector source id is `local:<key>`). */
  key: string;
  /** The absolute path being tracked. */
  path: string;
  /** The folder's own name, shown as the label. */
  label: string;
  state: "ok" | "unreachable" | "error";
  last_synced_at: string | null;
  /** How many index-only documents this folder currently has. */
  indexed: number;
  /** Whether the path is a readable directory right now (false = unmounted/removed). */
  present: boolean;
}

/** Snapshot of an in-flight local-folder sync, so the UI can restore progress after navigating away.
 *  `running` is false when idle; `last_report` holds the most recent result. */
export interface LocalFolderSyncState {
  running: boolean;
  processed: number;
  total: number | null;
  /** The folder key being synced, or null for an all-folders pass. */
  folder: string | null;
  last_report: SyncReport | null;
}

/** One document's 2-D position on the semantic memory map (coords are in [0,1]²). */
export interface SemanticCoord {
  id: number;
  x: number;
  y: number;
}

/** The cached semantic layout: which reducer produced it (`pca`|`tsne`|`none`), the coordinates, and
 *  whether a recompute is currently in flight. */
export interface SemanticLayout {
  method: string;
  coords: SemanticCoord[];
  computing: boolean;
}

/** Whether the optional t-SNE reducer (an on-demand download) is installed. */
export interface TsneStatus {
  installed: boolean;
}

/** Progress for an optional component's install-download — t-SNE, photo-OCR, or the macOS Python fetch
 *  (0..1, monotonic). Rendered as a percentage — a download has no file count — tiered by Depth (bar at
 *  minimal, bar + % at standard and power). One shape for all three channels (X-D6). */
export interface InstallProgressEvent {
  fraction: number;
}

/** Whether the optional photo-OCR component (rapidocr + pillow-heif) is installed. */
export interface OcrStatus {
  installed: boolean;
}

/** One on-device component in the Storage manager (the venv, the t-SNE libraries, the photo-OCR
 *  stack, the speech model, the active search model). `status` drives the action: removable now,
 *  blocked behind a dependent, required, in-use (managed elsewhere), or installable (an optional
 *  component not yet downloaded — the photo-OCR stack, whose `size_bytes` is then an estimate). */
export type StorageStatus = "required" | "in_use" | "removable" | "blocked" | "installable";
export interface StorageBlocker {
  label: string;
  /** The component id to scroll to (remove that one first). */
  anchor: string;
}
export interface StorageManage {
  label: string;
  /** The Settings tab to jump to (e.g. "search" for the embedder, "general" for the map toggle). */
  tab: string;
}
export interface StorageComponent {
  id: string;
  label: string;
  detail: string;
  size_bytes: number;
  /** The size is an estimate, not a real on-disk measurement (the embedder's shared cache). */
  approximate: boolean;
  /** Indented under the component above it (the t-SNE libraries under the enhanced layout). */
  child: boolean;
  status: StorageStatus;
  blockers: StorageBlocker[];
  manage: StorageManage | null;
  note: string | null;
}
export interface StorageReport {
  total_bytes: number;
  components: StorageComponent[];
}

/** Global progress for the semantic-layout precompute (fires regardless of which view started it). */
export type LayoutProgressEvent =
  | { state: "preparing" }
  | { state: "reducing"; count: number; method: string }
  | { state: "deferred" }
  | { state: "done"; method: string }
  | { state: "error"; message: string };

/** A mirrored calendar event (the agenda list). `start` is an ISO datetime, or a
 *  plain date for all-day events. */
export interface CalendarEvent {
  id: string;
  calendar_id: string;
  summary: string;
  description: string | null;
  location: string | null;
  start: string;
  end: string | null;
  all_day: boolean;
  html_link: string | null;
  /** The event's stable iCal UID — the anchor a milestone links to (card 7). */
  uid: string | null;
}

/** A focus-agenda row: a mirrored event plus whether it has already ended. The focus agenda widens
 *  the strict "not yet ended" gate to also list events that finished earlier today (in the user's
 *  zone); `ended` (`end < now`) is true for exactly those, so the view can show them de-emphasised
 *  until the user's local midnight. Every other consumer keeps the strict gate and never sees them. */
export interface AgendaEvent extends CalendarEvent {
  ended: boolean;
}

/** The upcoming event that named a project (shown on its focus card). */
export interface CalendarMatch {
  summary: string;
  start: string;
}

/** The daily briefing shown on the focus view (Step 7, spec §4 P1). `stale` is true
 *  when it's empty or older than the freshness window, so the focus view can kick off
 *  a background refresh. */
export interface DailyBriefing {
  briefing: string;
  updated_at: string | null;
  stale: boolean;
}

/** A structured proactive flag (board card 9): a decision the briefing and chat render, anchored on a
 *  stable milestone id (`anchor_kind: "milestone"`) or calendar iCal UID (`"calendar"`). Resolving a
 *  flag filters it out of the rendered set rather than editing prose — the sentence is volatile, the
 *  flag underneath is stable. Mirrors `Flag` in src-tauri/src/flags.rs (`r#type` serialises as `type`). */
export interface Flag {
  id: number;
  anchor_kind: string;
  anchor: string;
  type: string;
  /** How far ahead of the anchored time the flag fires; null = the type's default. */
  threshold: string | null;
  /** "active" | "resolved". */
  state: string;
  /** Which path closed it — "detection" | "assertion"; null while active. */
  source: string | null;
  confidence: number;
  /** A deliberate user vouch — true iff closed by assertion. */
  user_confirmed: boolean;
  /** `documents.source_id` of the satisfying artifact (rename-survives identity), if any. */
  artifact_ptr: string | null;
  /** The artifact's open URL — display-only (moves on rename). */
  artifact_url: string | null;
  created_at: string;
  updated_at: string;
  resolved_at: string | null;
  /** The occurrence this flag is about (a timed event's start); null for a milestone or pre-v33 flag. */
  instance_at: string | null;
}

/** Where the polymorphic focus box routes one line the user typed (board card 9, decisions 6–7): a
 *  background classifier places it, then the frontend acts (resolve/prefer, on the user's confirm — both
 *  are writes) or navigates (ask/edit). Mirrors `FocusRoute` in src-tauri/src/flags.rs, tagged by `kind`. */
export type FocusRoute =
  | { kind: "resolve"; flag_id: number; label: string }
  | { kind: "prefer"; draft: DraftPreference }
  | { kind: "ask"; text: string }
  | { kind: "edit"; project: string | null }
  | { kind: "unclear" };

/** Machine-readable cause of a setup failure, so the UI can show a tailored
 *  troubleshooting guide. Mirrors `SidecarErrorKind` in src-tauri/src/sidecar.rs. */
export type SidecarErrorKind =
  | "python_too_old"
  | "python_missing"
  | "python_download_failed"
  | "pip_failed"
  | "requirements_missing"
  | "packaging_bug"
  | "unknown";

/** Lifecycle of the Python document engine (sidecar). */
export type SidecarStatus =
  | { state: "not_installed" }
  | { state: "installing" }
  | { state: "ready" }
  | { state: "error"; message: string; kind: SidecarErrorKind };

export type IngestEvent =
  | { type: "preparing"; message: string }
  | { type: "counted"; total: number }
  | { type: "started"; path: string; name: string }
  | { type: "skipped"; path: string; reason: string }
  | { type: "done"; document: Document }
  | { type: "failed"; path: string; error: string }
  | { type: "finished"; ingested: number; skipped: number; failed: number };

// --- Developer mode (issue #78): read-only inspection surfaces ---
// Mirrors the structs in src-tauri/src/commands_dev.rs. Every value is already redacted by the
// backend, so these are exactly what the Dev tab may display.

/** The vault's stored retrieval-config stamp (index-time config); mirrors `RetrievalConfig`. */
export interface RetrievalStamp {
  version: number;
  chunk_target_tokens: number;
  chunk_overlap_tokens: number;
  prepend_headings: boolean;
  boundary_strategy: string;
  splitter_version: number;
  embedder_id: string;
  dimension: number;
  index_params: string;
  /** chunks_fts tokenisation mode — cjk-bigram-v1 on a multilingual vault, or none otherwise (F-33). */
  fts_segmentation: string;
}

/** Index-time + runtime facts for the Dev tab's System panel. */
export interface DevSystemInfo {
  /** The store's `PRAGMA user_version` (the applied migration level). */
  migration_version: number;
  embedder_id: string;
  embedder_label: string;
  /** The live physical vector width of `chunk_vec`. */
  vector_dim: number;
  reranking_enabled: boolean;
  retrieval_stamp: RetrievalStamp | null;
}

/** One row of the counts dashboard. */
export interface DevTableCount {
  table: string;
  rows: number;
}

/** A redacted page of one inspected table: projected column names + rendered cell strings. */
export interface DevTablePage {
  table: string;
  columns: string[];
  rows: string[][];
  total: number;
  limit: number;
  offset: number;
}

/** One ranked candidate from a "Retrieval explain" run, with every per-stage score (issue #81).
 *  `preview` is the chunk's body text truncated to a char cap — never the full body. */
export interface DevRetrievalRow {
  final_rank: number;
  chunk_id: number;
  document_id: number;
  title: string;
  heading: string | null;
  preview: string;
  /** 0-based rank in the vector KNN branch + the raw `vec0` distance; null if keyword-only. */
  vector_rank: number | null;
  vector_distance: number | null;
  /** 0-based rank in the keyword/FTS branch; null if vector-only. */
  keyword_rank: number | null;
  fused_score: number;
  decay_factor: number;
  decayed_score: number;
  reranker_score: number | null;
}

/** Result of a "Retrieval explain" run: the ranked rows + the engine context to read them by. */
export interface DevRetrievalExplain {
  embedder_id: string;
  embedder_label: string;
  reranking_enabled: boolean;
  /** Whether the reranker actually ran and reordered (vs. left the fused order). */
  reranked: boolean;
  rrf_k: number;
  half_life_days: number;
  k: number;
  rows: DevRetrievalRow[];
}
