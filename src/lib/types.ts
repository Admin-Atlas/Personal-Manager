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

/** A chunk returned by hybrid search (the `search_documents` command). */
export interface RetrievedChunk {
  chunk_id: number;
  document_id: number;
  title: string;
  source_path: string | null;
  vault_path: string;
  heading: string | null;
  content: string;
  ordinal: number;
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
  regime: "summary" | "window";
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
   * conversation, born as a document on first index and backed by a Markdown vault file (epic 7).
   */
  source_type: "vault" | "index_only" | "chat";
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
  | "due_soon"
  | "blocked"
  | "quick_win"
  | "take_a_look"
  | "part_of"
  | "on_track";

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
  /** How far ahead PM mirrors events (and the agenda horizon), in days. */
  window_days: number;
}

/** A subscribed iCal feed, without its secret URL (for display). `provider` tags it for grouping. */
export interface IcsFeedInfo {
  id: string;
  label: string;
  /** `apple` | `outlook` | `google` | `other`. */
  provider: string;
}

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

/** A file a Drive sync tried to index but couldn't (e.g. an unsupported type, or a fetch error),
 *  surfaced in the post-sync report so the user knows what was left out. */
export interface DriveSyncIssue {
  name: string;
  reason: string;
}

/** The outcome of a Drive sync pass: how many items were indexed/updated/removed, the not-indexed
 *  list, and whether the user stopped it early. */
export interface DriveSyncReport {
  indexed: number;
  updated: number;
  removed: number;
  skipped: number;
  failed: number;
  /** The user pressed Stop — already-indexed files are kept; the rest were left for next time. */
  cancelled: boolean;
  /** Files attempted but not indexed (capped; see `issues_truncated`). */
  issues: DriveSyncIssue[];
  /** True when more files couldn't be indexed than `issues` lists. */
  issues_truncated: boolean;
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
  last_report: DriveSyncReport | null;
}

/** Streamed progress while a Drive sync runs (mapped onto the shared IngestProgress bar). */
export type DriveSyncEvent =
  | { type: "counted"; total: number }
  | { type: "item"; processed: number; total: number; name: string }
  | { type: "finished"; report: DriveSyncReport };

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

/** A file a OneDrive sync tried to index but couldn't, surfaced in the post-sync report. */
export interface OneDriveSyncIssue {
  name: string;
  reason: string;
}

/** The outcome of a OneDrive sync pass. */
export interface OneDriveSyncReport {
  indexed: number;
  updated: number;
  removed: number;
  skipped: number;
  failed: number;
  cancelled: boolean;
  issues: OneDriveSyncIssue[];
  issues_truncated: boolean;
}

/** Snapshot of an in-flight OneDrive sync, so the UI can resume showing progress after navigating
 *  away and back. */
export interface OneDriveSyncState {
  running: boolean;
  processed: number;
  total: number | null;
  account: string | null;
  last_report: OneDriveSyncReport | null;
}

/** Streamed progress while a OneDrive sync runs (mapped onto the shared IngestProgress bar). */
export type OneDriveSyncEvent =
  | { type: "counted"; total: number }
  | { type: "item"; processed: number; total: number; name: string }
  | { type: "finished"; report: OneDriveSyncReport };

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

/** Progress for the optional t-SNE component download (0..1, monotonic). Rendered as a percentage —
 *  a download has no file count — tiered by Depth (bar at minimal, bar + % at standard and power). */
export interface TsneInstallEvent {
  fraction: number;
}

/** Whether the optional photo-OCR component (rapidocr + pillow-heif) is installed. */
export interface OcrStatus {
  installed: boolean;
}

/** Progress for the optional OCR component download (0..1, monotonic). Rendered as a percentage, like
 *  the t-SNE download — there is no file count. */
export interface OcrInstallEvent {
  fraction: number;
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

/** Machine-readable cause of a setup failure, so the UI can show a tailored
 *  troubleshooting guide. Mirrors `SidecarErrorKind` in src-tauri/src/sidecar.rs. */
export type SidecarErrorKind =
  | "python_too_old"
  | "python_missing"
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
