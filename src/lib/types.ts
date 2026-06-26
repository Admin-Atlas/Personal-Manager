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

/** The distilled Learning-You profile shown in Settings (Step 4b, spec §4.5). */
export interface LearningProfile {
  profile: string;
  updated_at: string | null;
  correction_count: number;
}

/** A document an answer cited — the provenance shown under it. */
export interface Citation {
  document_id: number;
  title: string;
  source_path: string | null;
  vault_path: string;
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

/** Ranked importance of a document (or unset). */
export type Importance = "high" | "medium" | "low" | null;

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
   * pointer we index but don't hold (body fetched live, only a summary readable offline).
   */
  source_type: "vault" | "index_only";
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

/** One focus-view row: a project, its derived status, and the signals behind it. */
export interface ProjectOverview {
  name: string;
  status: ProjectStatus;
  doc_count: number;
  last_activity: string | null;
  deadline: string | null;
  size: ProjectSize;
  blocked_by: string | null;
  parent: string | null;
  importance: Importance;
  /** The upcoming calendar event whose title names this project (Step 6), if any —
   *  it can drive "Due soon" and is shown on the card to explain the status. */
  calendar_event: CalendarMatch | null;
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

// --- Personal Assistant: Google Calendar connector (Step 6, spec §8.6) ---

/** State of the calendar connector, covering both paths (iCal feeds + Google OAuth). */
export interface CalendarStatus {
  /** How many .ics feeds are subscribed (the no-OAuth path). */
  ics_feeds: number;
  /** The user has saved a Google client ID + secret. */
  oauth_client_configured: boolean;
  /** A Google OAuth token is stored (sign-in completed). */
  oauth_connected: boolean;
  /** How many Google calendars are selected to sync. */
  calendars_selected: number;
  /** ISO timestamp of the last successful sync, if any. */
  last_sync: string | null;
  /** How far ahead PM mirrors events (and the agenda horizon), in days. */
  window_days: number;
}

/** A subscribed .ics feed, without its secret URL (for display). */
export interface IcsFeedInfo {
  id: string;
  label: string;
}

/** One of the user's calendars (for the Settings picker). */
export interface GoogleCalendar {
  id: string;
  summary: string;
  primary: boolean;
  selected: boolean;
}

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
