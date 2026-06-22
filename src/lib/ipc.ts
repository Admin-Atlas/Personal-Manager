// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { invoke, Channel } from "@tauri-apps/api/core";
import type {
  AppLockStatus,
  CalendarEvent,
  CalendarStatus,
  ChatEvent,
  Conversation,
  CostSummary,
  DailyBriefing,
  Document,
  GoogleCalendar,
  IcsFeedInfo,
  Importance,
  IngestEvent,
  LearningProfile,
  Message,
  ModelInfo,
  ModelRecommendations,
  ProjectOverview,
  ProjectProposalEvent,
  ProjectSize,
  ReviewDecision,
  ReviewEvent,
  RetrievedChunk,
  Settings,
  SidecarStatus,
} from "./types";

export const hasOpenRouterKey = () => invoke<boolean>("has_openrouter_key");

export const setOpenRouterKey = (key: string) =>
  invoke<void>("set_openrouter_key", { key });

export const hasOpenRouterBackgroundKey = () =>
  invoke<boolean>("has_openrouter_background_key");

export const setOpenRouterBackgroundKey = (key: string) =>
  invoke<void>("set_openrouter_background_key", { key });

export const getSettings = () => invoke<Settings>("get_settings");

/** Persist the user's IANA time zone (e.g. "Europe/London"). An empty string clears
 *  it (the backend then reasons in UTC). Validated against the tz database in Rust. */
export const setTimeZone = (zone: string) =>
  invoke<void>("set_time_zone", { zone });

/** Read a UI preference blob (theme axes, pinboard layout) from the encrypted store
 *  so it travels with the data folder. `null` when nothing is stored yet. */
export const getPref = (key: string) =>
  invoke<string | null>("get_pref", { key });

/** Persist a UI preference blob. The backend only accepts a fixed allowlist of keys
 *  (`appearance`, `pinboard`), so this can't touch schema-critical settings. */
export const setPref = (key: string, value: string) =>
  invoke<void>("set_pref", { key, value });

/** Ordered preferred chat models (first = primary). */
export const setChatModels = (models: string[]) =>
  invoke<void>("set_chat_models", { models });

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
export const modelRecommendations = () =>
  invoke<ModelRecommendations>("model_recommendations");

/** Persist the optional recommender denylist (provider or model slugs). */
export const setRecommendDenylist = (denylist: string[]) =>
  invoke<void>("set_recommend_denylist", { denylist });

/** Toggle the UI help/explain mode (Step 4b). */
export const setHelpMode = (enabled: boolean) =>
  invoke<void>("set_help_mode", { enabled });

// --- Biometric app-lock (soft UI gate, opt-in — spec §16.2) ---

/** Whether the app-lock is on, and whether this device can perform an OS verification. */
export const appLockStatus = () => invoke<AppLockStatus>("app_lock_status");

/** Turn the app-lock on/off. Enabling is rejected by the backend when unavailable. */
export const setAppLock = (enabled: boolean) =>
  invoke<void>("set_app_lock", { enabled });

/** Run the OS verification (Windows Hello / Touch ID) to lift the launch lock.
 *  Resolves true on success, false when the user cancels/fails; rejects when the
 *  verifier can't run at all. */
export const unlockApp = () => invoke<boolean>("unlock_app");

// --- Learning You (Step 4b, spec §4.5) ---

/** The distilled profile of how the user organises, for display in Settings. */
export const getLearningProfile = () =>
  invoke<LearningProfile>("get_learning_profile");

/** Re-distil the profile from logged corrections; returns the refreshed profile. */
export const refreshLearningProfile = () =>
  invoke<LearningProfile>("refresh_learning_profile");

export const listConversations = () =>
  invoke<Conversation[]>("list_conversations");

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

/** Hybrid search over the store, returning the top-k matching chunks. */
export const searchDocuments = (query: string, k?: number) =>
  invoke<RetrievedChunk[]>("search_documents", { query, k });

/** Transcribe a recorded voice clip to text, fully on-device via the sidecar's
 *  Whisper model. `audioBase64` is the standard-base64 of the recording bytes. */
export const transcribeAudio = (audioBase64: string) =>
  invoke<string>("transcribe_audio", { audioBase64 });

/** Ingest files/folders, streaming progress for each item. */
export function ingestPaths(
  paths: string[],
  onEvent: (event: IngestEvent) => void,
): Promise<void> {
  const channel = new Channel<IngestEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("ingest_paths", { paths, onEvent: channel });
}

/** Drop the index and rebuild it from the Markdown vault. */
export function rebuildIndex(
  onEvent: (event: IngestEvent) => void,
): Promise<void> {
  const channel = new Channel<IngestEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("rebuild_index", { onEvent: channel });
}

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
) =>
  invoke<Document>("set_document_metadata", { documentId, project, tags, importance });

// --- Personal Assistant: focus view & projects (Step 5, spec §4) ---

/** Every active project with its triage metadata and derived status. */
export const listProjectOverviews = () =>
  invoke<ProjectOverview[]>("list_project_overviews");

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
export const removeIcsFeed = (id: string) =>
  invoke<void>("remove_ics_feed", { id });

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
export const listGoogleCalendars = () =>
  invoke<GoogleCalendar[]>("list_google_calendars");

/** Choose which calendars to sync. */
export const setGoogleCalendarIds = (ids: string[]) =>
  invoke<void>("set_google_calendar_ids", { ids });

/** Pull events from the selected calendars into the local mirror; returns the count. */
export const syncCalendar = () => invoke<number>("sync_calendar");

/** Upcoming events in the mirror, for the focus-view agenda. */
export const listCalendarEvents = () =>
  invoke<CalendarEvent[]>("list_calendar_events");

// --- Personal Assistant: Daily briefing (Step 7, spec §4 P1) ---

/** The stored "here's your picture today" briefing + whether it's due a refresh. */
export const getDailyBriefing = () => invoke<DailyBriefing>("get_daily_briefing");

/** Regenerate the briefing from the current focus-view state; returns the new one. */
export const refreshDailyBriefing = () =>
  invoke<DailyBriefing>("refresh_daily_briefing");
