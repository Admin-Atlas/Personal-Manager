// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { ChatView } from "./components/ChatView";
import { CommandPalette } from "./components/CommandPalette";
import { Composer } from "./components/Composer";
import { DocumentsView } from "./components/DocumentsView";
import { FocusView } from "./components/FocusView";
import { GraphView } from "./components/GraphView";
import { HelpOverlay } from "./components/HelpOverlay";
import { LockScreen } from "./components/LockScreen";
import { PinboardView } from "./components/PinboardView";
import { ProjectView } from "./components/ProjectView";
import { ReviewView } from "./components/ReviewView";
import { Sidebar, type View } from "./components/Sidebar";
import { SettingsView } from "./components/SettingsView";
import { UpdateBanner } from "./components/UpdateBanner";
import { WhatsNew } from "./components/WhatsNew";
import { Skeleton } from "./components/ui";
import { HelpContext } from "./lib/help";
import { useChatStream } from "./lib/useChatStream";
import { useUpdater } from "./lib/useUpdater";

const LAST_SEEN_VERSION_KEY = "pm:lastSeenVersion";
import {
  appLockStatus,
  createConversation,
  getMessages,
  getSettings,
  hasOpenRouterKey,
  listConversations,
  reviewQueue,
  setHelpMode,
} from "./lib/ipc";
import type { Conversation, Settings } from "./lib/types";

export default function App() {
  const [loading, setLoading] = useState(true);
  const [keySet, setKeySet] = useState(false);
  // The optional biometric app-lock (soft UI gate). Locked at launch when the user has
  // turned it on; lifted once the OS verifies them (see LockScreen). The store is already
  // decrypted regardless — this only withholds the window.
  const [locked, setLocked] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [view, setView] = useState<View>("focus");
  const [selectedProject, setSelectedProject] = useState<string | null>(null);
  /** A file to highlight when the project opens (set by the command palette). */
  const [selectedDocId, setSelectedDocId] = useState<number | null>(null);
  const [reviewCount, setReviewCount] = useState(0);
  const [showPalette, setShowPalette] = useState(false);

  function openProject(project: string, focusDocId?: number) {
    setSelectedProject(project);
    setSelectedDocId(focusDocId ?? null);
    setView("project");
  }

  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  // Live mirror of activeId for async callbacks that outlive a render: a streaming
  // send must not write its result into a conversation the user has since left.
  const activeIdRef = useRef(activeId);
  activeIdRef.current = activeId;
  // Chat send/stream state lives in a shared hook so the global chat and the
  // per-project chat can't drift apart (see useChatStream). The guard key is the
  // conversation currently on screen.
  const chat = useChatStream(() => activeIdRef.current);
  const update = useUpdater();
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [showWhatsNew, setShowWhatsNew] = useState(false);
  const [helpMode, setHelpModeState] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);

  // Load settings (help-mode preference + the active models for the sidebar tag).
  // Toggling help mode writes through immediately; `refreshSettings` re-reads after
  // the Settings dialog closes so the sidebar tag reflects any model change.
  const refreshSettings = useCallback(() => {
    getSettings()
      .then((s) => {
        setHelpModeState(s.help_mode);
        setSettings(s);
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    refreshSettings();
  }, [refreshSettings]);

  const updateHelpMode = useCallback((enabled: boolean) => {
    setHelpModeState(enabled);
    void setHelpMode(enabled).catch(() => {});
  }, []);

  // Ctrl/Cmd+K toggles the command palette from anywhere (spec §4 — jump
  // anywhere in a couple of keystrokes).
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setShowPalette((open) => !open);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Auto-open "What's New" once after the app updates to a version the user
  // hasn't seen yet. We persist the last-seen version so it shows exactly once
  // per upgrade; opening it (or closing the auto-shown one) marks it seen.
  useEffect(() => {
    (async () => {
      try {
        const version = await getVersion();
        setAppVersion(version);
        if (localStorage.getItem(LAST_SEEN_VERSION_KEY) !== version) {
          setShowWhatsNew(true);
        }
      } catch {
        /* non-Tauri context or version unavailable — skip */
      }
    })();
  }, []);

  function closeWhatsNew() {
    setShowWhatsNew(false);
    if (appVersion) localStorage.setItem(LAST_SEEN_VERSION_KEY, appVersion);
  }

  useEffect(() => {
    (async () => {
      try {
        // Resolve the launch lock before the first paint so locked content never flashes.
        const lock = await appLockStatus().catch(() => null);
        if (lock?.locked) setLocked(true);
        const has = await hasOpenRouterKey();
        setKeySet(has);
        if (has) await refreshConversations(true);
      } catch (e) {
        chat.setError(String(e));
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  async function refreshConversations(selectFirst = false) {
    const list = await listConversations();
    setConversations(list);
    if (selectFirst && list.length > 0) {
      await selectConversation(list[0].id);
    }
  }

  const refreshReviewCount = useCallback(async () => {
    try {
      setReviewCount((await reviewQueue()).length);
    } catch {
      /* count is a hint; ignore failures */
    }
  }, []);

  // Keep the sidebar's review badge current as the user moves around the app.
  useEffect(() => {
    if (keySet) void refreshReviewCount();
  }, [keySet, view, refreshReviewCount]);

  async function selectConversation(id: number) {
    setActiveId(id);
    chat.clearTransient(); // drop any in-flight stream's UI from the conversation we're leaving
    chat.setMessages(await getMessages(id));
  }

  function newConversation() {
    setActiveId(null);
    chat.clearTransient();
    chat.setMessages([]);
  }

  async function handleSend(text: string) {
    let convId = activeId;
    if (convId == null) {
      try {
        const created = await createConversation();
        convId = created.id;
        setActiveId(created.id);
        setConversations((prev) => [created, ...prev]);
      } catch (e) {
        chat.setError(String(e));
        return;
      }
    }

    await chat.send(convId, text);

    // Reload persisted state. The conversation list (titles/order) always
    // refreshes; the messages are adopted only if the user is still here.
    try {
      const [msgs, convs] = await Promise.all([
        getMessages(convId),
        listConversations(),
      ]);
      setConversations(convs);
      if (activeIdRef.current === convId) {
        chat.setMessages(msgs);
      }
    } catch {
      /* keep optimistic state on reload failure */
    }
  }

  if (loading) {
    // A wireframe of the app shell (sidebar + content) rather than a bare spinner, so
    // the first paint already has PM's shape.
    return (
      <div className="flex h-full bg-bg">
        <div className="flex w-60 shrink-0 flex-col gap-2 border-r border-border p-3">
          <Skeleton className="h-7 w-28" />
          <div className="mt-3 flex flex-col gap-1.5">
            {Array.from({ length: 5 }).map((_, i) => (
              <Skeleton key={i} className="h-7 w-full" />
            ))}
          </div>
        </div>
        <div className="flex flex-1 flex-col gap-3 p-6">
          <Skeleton className="h-8 w-48" />
          <Skeleton className="h-24 w-full" />
          <Skeleton className="h-5 w-3/4" />
          <Skeleton className="h-5 w-2/3" />
        </div>
      </div>
    );
  }

  // The launch lock sits in front of everything (but below the title bar, which lives in
  // main.tsx) so the window stays draggable/closable while locked.
  if (locked) {
    return <LockScreen onUnlocked={() => setLocked(false)} />;
  }

  if (!keySet) {
    return (
      <SettingsView
        onboarding
        onClose={async () => {
          setKeySet(true);
          await refreshConversations(true);
        }}
      />
    );
  }

  return (
    <HelpContext.Provider value={{ enabled: helpMode, setEnabled: updateHelpMode }}>
      <div className={`flex h-full flex-col bg-bg text-ink ${helpMode ? "help-mode" : ""}`}>
        <UpdateBanner update={update} />
        <div className="relative flex flex-1 overflow-hidden">
          <Sidebar
            view={view}
            onNavigate={setView}
            conversations={conversations}
            activeId={activeId}
            reviewCount={reviewCount}
            onSelect={(id) => {
              setView("chat");
              selectConversation(id);
            }}
            onNew={() => {
              setView("chat");
              newConversation();
            }}
            onOpenSettings={() => setShowSettings(true)}
            onOpenWhatsNew={() => setShowWhatsNew(true)}
            onOpenPalette={() => setShowPalette(true)}
            chatModel={settings?.chat_models[0] ?? null}
            backgroundModel={settings?.background_models[0] ?? null}
            chatFallbacks={
              settings?.chat_auto_switch ? Math.max(0, settings.chat_models.length - 1) : 0
            }
            backgroundFallbacks={
              settings?.background_auto_switch
                ? Math.max(0, settings.background_models.length - 1)
                : 0
            }
          />

          {view === "focus" ? (
            <main className="flex h-full flex-1 flex-col">
              <FocusView onOpenProject={openProject} />
            </main>
          ) : view === "project" && selectedProject ? (
            <main className="flex h-full flex-1 flex-col">
              <ProjectView
                project={selectedProject}
                focusDocId={selectedDocId}
                onBack={() => setView("focus")}
              />
            </main>
          ) : view === "documents" ? (
            <main className="flex h-full flex-1 flex-col">
              <DocumentsView onReviewClick={() => setView("review")} />
            </main>
          ) : view === "review" ? (
            <main className="flex h-full flex-1 flex-col">
              <ReviewView onChanged={refreshReviewCount} />
            </main>
          ) : view === "graph" ? (
            <main className="flex h-full flex-1 flex-col">
              <GraphView />
            </main>
          ) : view === "pinboard" ? (
            <main className="flex h-full flex-1 flex-col">
              <PinboardView />
            </main>
          ) : (
            <main className="flex h-full flex-1 flex-col">
              {chat.error && (
                <div
                  className="border-b border-rule px-4 py-2 font-ui text-sm text-[var(--st-due)]"
                  style={{
                    background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
                  }}
                >
                  {chat.error}
                </div>
              )}
              <ChatView messages={chat.messages} streaming={chat.streaming} />
              <Composer disabled={chat.sending} onSend={handleSend} />
            </main>
          )}

          {showSettings && (
            <div className="absolute inset-0 z-10" style={{ background: "rgba(8,6,4,0.5)" }}>
              <SettingsView
                onboarding={false}
                onClose={() => {
                  setShowSettings(false);
                  refreshSettings();
                }}
              />
            </div>
          )}

          {showWhatsNew && (
            <div className="absolute inset-0 z-20" style={{ background: "rgba(8,6,4,0.5)" }}>
              <WhatsNew onClose={closeWhatsNew} currentVersion={appVersion} />
            </div>
          )}

          {showPalette && (
            <CommandPalette
              onClose={() => setShowPalette(false)}
              onOpenProject={openProject}
              onOpenConversation={(id) => {
                setView("chat");
                selectConversation(id);
              }}
              onNavigate={setView}
              onOpenSettings={() => setShowSettings(true)}
            />
          )}
        </div>
        <HelpOverlay />
      </div>
    </HelpContext.Provider>
  );
}
