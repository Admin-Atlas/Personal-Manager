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

/** Lifecycle of the Python document engine (sidecar). */
export type SidecarStatus =
  | { state: "not_installed" }
  | { state: "installing" }
  | { state: "ready" }
  | { state: "error"; message: string };

export type IngestEvent =
  | { type: "preparing"; message: string }
  | { type: "started"; path: string; name: string }
  | { type: "skipped"; path: string; reason: string }
  | { type: "done"; document: Document }
  | { type: "failed"; path: string; error: string }
  | { type: "finished"; ingested: number; skipped: number; failed: number };
