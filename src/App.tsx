// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { CalendarView } from "./components/calendar/CalendarView";
import { ChatView } from "./components/ChatView";
import { ConversationTitle } from "./components/ConversationTitle";
import { CommandPalette } from "./components/CommandPalette";
import { Composer } from "./components/Composer";
import { ContextMeter } from "./components/ContextMeter";
import { RetrievalExplainPanel } from "./components/RetrievalExplainPanel";
import { DevView } from "./components/DevView";
import { DocumentsView } from "./components/DocumentsView";
import { FocusView } from "./components/FocusView";
import { GraphView } from "./components/GraphView";
import { warmMapLayout } from "./lib/mapLayout";
import { HelpOverlay } from "./components/HelpOverlay";
import { LockScreen } from "./components/LockScreen";
import { PinboardView } from "./components/PinboardView";
import { ProjectView } from "./components/ProjectView";
import { ReviewView } from "./components/ReviewView";
import { TeachView } from "./components/TeachView";
import { Sidebar, type View } from "./components/Sidebar";
import { SettingsView } from "./components/SettingsView";
import { UpdateBanner } from "./components/UpdateBanner";
import { VaultCurtain } from "./components/VaultCurtain";
import { VaultUnlock } from "./components/VaultUnlock";
import { WhatsNew } from "./components/WhatsNew";
import { Skeleton } from "./components/ui";
import { HelpContext } from "./lib/help";
import { useChatStream } from "./lib/useChatStream";
import { useProjectChat } from "./lib/useProjectChat";
import { isNewChatTrigger } from "./lib/chatSession";
import { useUpdater } from "./lib/useUpdater";
import { useDevMode } from "./lib/capabilities";
import { useTheme } from "./theme";

const LAST_SEEN_VERSION_KEY = "pm:lastSeenVersion";
import {
  appLockStatus,
  createConversation,
  deleteConversation,
  getMessages,
  getSettings,
  hasOpenRouterKey,
  listConversations,
  onVaultAcquired,
  onVaultCurtain,
  openUrl,
  resumeDriveSync,
  resumeOneDriveSync,
  reviewQueue,
  setChatModels,
  setHelpMode,
  startSemanticLayout,
  syncCalendar,
  vaultLockStatus,
  vaultStatus,
} from "./lib/ipc";
import type { Conversation, Settings, VaultLockStatus, VaultStatus } from "./lib/types";

export default function App() {
  const [loading, setLoading] = useState(true);
  const [keySet, setKeySet] = useState(false);
  // The optional biometric app-lock (soft UI gate). Locked at launch when the user has
  // turned it on; lifted once the OS verifies them (see LockScreen). The store is already
  // decrypted regardless — this only withholds the window.
  const [locked, setLocked] = useState(false);
  // The vault unlock gate: a passphrase vault that booted without a cached key on this
  // profile. This gates *real* decryption (the DB is closed until unlocked), so it sits
  // ahead of everything that touches the store.
  const [vault, setVault] = useState<VaultStatus | null>(null);
  const [vaultNeedsUnlock, setVaultNeedsUnlock] = useState(false);
  // Cooperative single-writer state for a shared vault: when another profile is the
  // active writer, `vaultLock.active` is false and the curtain shows over everything.
  const [vaultLock, setVaultLock] = useState<VaultLockStatus | null>(null);
  const [curtainReason, setCurtainReason] = useState<"other-active" | "handed-off">("other-active");
  const [showSettings, setShowSettings] = useState(false);
  const [view, setView] = useState<View>("focus");
  const [selectedProject, setSelectedProject] = useState<string | null>(null);
  /** A file to highlight when the project opens (set by the command palette). */
  const [selectedDocId, setSelectedDocId] = useState<number | null>(null);
  /** A chat turn to scroll to and flash after a chat citation navigates here (card 7E PR3). The
   *  `nonce` bumps on every click so re-clicking the same citation re-fires the jump. */
  const [focusTurn, setFocusTurn] = useState<{ id: number; nonce: number } | null>(null);
  const [reviewCount, setReviewCount] = useState(0);
  const [showPalette, setShowPalette] = useState(false);

  function openProject(project: string, focusDocId?: number) {
    setSelectedProject(project);
    setSelectedDocId(focusDocId ?? null);
    setView("project");
  }

  // The open project's scoped chat session. Lifted here (not inside ProjectView) so the left
  // sidebar can list this project's conversations like the global chat, while ProjectView renders
  // the active thread — both read one source (board card 7E). Dormant when no project is open.
  const projectChat = useProjectChat(selectedProject);
  const inProject = view === "project" && selectedProject != null;

  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  // Live mirror of activeId for async callbacks that outlive a render: a streaming
  // send must not write its result into a conversation the user has since left.
  const activeIdRef = useRef(activeId);
  activeIdRef.current = activeId;
  // The on-screen conversation's row (for the editable title header); null on a fresh, unsent chat.
  const activeConv = conversations.find((c) => c.id === activeId) ?? null;
  // Chat send/stream state lives in a shared hook so the global chat and the
  // per-project chat can't drift apart (see useChatStream). The guard key is the
  // conversation currently on screen.
  const chat = useChatStream(() => activeIdRef.current);
  const update = useUpdater();
  const { teachVisible } = useTheme();
  const { devMode } = useDevMode();
  // If the Review/Teach (learning tools) or Dev tab gets hidden — a preset change, or the toggle
  // turned off in Settings — while it's open, fall back to Focus so the user is never stranded on a
  // view with no nav entry.
  useEffect(() => {
    if ((view === "teach" || view === "review") && !teachVisible) setView("focus");
    if (view === "dev" && !devMode) setView("focus");
  }, [view, teachVisible, devMode]);
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

  // External links open in the system browser. The webview can't honour an `<a target="_blank">`
  // on its own (no shell/opener plugin), so intercept clicks on any such link app-wide and hand the
  // URL to the OS browser via the backend (which guards to http/https). One handler covers every
  // link — existing and future — so individual `<a>`s need no special wiring.
  useEffect(() => {
    function onClick(e: MouseEvent) {
      if (e.defaultPrevented || e.button !== 0) return;
      const anchor = (e.target as HTMLElement | null)?.closest?.("a");
      const href = anchor?.getAttribute("href");
      if (anchor?.target === "_blank" && href && /^https?:\/\//i.test(href)) {
        e.preventDefault();
        void openUrl(href).catch(() => {});
      }
    }
    document.addEventListener("click", onClick);
    return () => document.removeEventListener("click", onClick);
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
        const appLocked = await appLockStatus().catch(() => null);
        if (appLocked?.locked) setLocked(true);
        // A passphrase vault with no cached key on this profile boots locked (the store
        // can't open until unlocked), so defer the store-backed load until it is.
        const vs = await vaultStatus().catch(() => null);
        setVault(vs);
        // Another profile may already be the active writer of a shared vault — then this
        // instance is curtained (its store closed). That takes priority over the unlock
        // prompt (the key is cached; the vault just isn't ours to write right now).
        const writerLock = await vaultLockStatus().catch(() => null);
        setVaultLock(writerLock);
        if (writerLock && !writerLock.active) return;
        if (vs?.needs_unlock) {
          setVaultNeedsUnlock(true);
          return;
        }
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

  // Once the vault is unlocked, load what the locked boot deferred.
  async function completeUnlock() {
    setVaultNeedsUnlock(false);
    setLoading(true);
    try {
      setVault(await vaultStatus().catch(() => null));
      const has = await hasOpenRouterKey();
      setKeySet(has);
      if (has) await refreshConversations(true);
    } catch (e) {
      chat.setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  // Became the active writer (baton acquired): lift the curtain and load the store-backed
  // state it had withheld (the backend reopened the store on acquisition).
  async function becomeActiveWriter() {
    setLoading(true);
    try {
      setVaultLock(await vaultLockStatus().catch(() => null));
      const has = await hasOpenRouterKey();
      setKeySet(has);
      if (has) await refreshConversations(true);
    } catch (e) {
      chat.setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  // Writer-lock events: the curtain drops when another profile takes over, and lifts when
  // this instance (re)acquires the baton.
  useEffect(() => {
    let offCurtain: (() => void) | undefined;
    let offAcquired: (() => void) | undefined;
    void onVaultCurtain((e) => {
      setCurtainReason(e.reason);
      vaultLockStatus()
        .then(setVaultLock)
        .catch(() => {});
    }).then((off) => (offCurtain = off));
    void onVaultAcquired(() => void becomeActiveWriter()).then((off) => (offAcquired = off));
    return () => {
      offCurtain?.();
      offAcquired?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- subscribe once for the app's life
  }, []);

  // While curtained, poll the lock so the holder going stale surfaces the force-take option.
  useEffect(() => {
    if (vaultLock?.active !== false) return;
    const id = setInterval(() => {
      vaultLockStatus()
        .then(setVaultLock)
        .catch(() => {});
    }, 2500);
    return () => clearInterval(id);
  }, [vaultLock?.active]);

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

  // Resume a Drive sync interrupted by a previous close/crash mid-index. Runs once the vault is open
  // (keySet implies an unlocked store), detached in the backend — already-indexed files survive, so
  // it just finishes the outstanding work. A no-op when there's nothing pending.
  useEffect(() => {
    if (keySet) void resumeDriveSync().catch(() => {});
  }, [keySet]);

  // Same for an interrupted OneDrive sync (independent connector, its own resume marker).
  useEffect(() => {
    if (keySet) void resumeOneDriveSync().catch(() => {});
  }, [keySet]);

  // Keep the read-only calendar mirror fresh in the background: one poll shortly after unlock, then
  // every 15 minutes. The mirror feeds the calendar view, the focus agenda, the daily briefing, and
  // chat's "what's on" answer, so it belongs at app scope (not the tab, which unmounts). Best-effort
  // and guarded against overlap; a manual "Refresh now" in the calendar header re-polls on demand.
  useEffect(() => {
    if (!keySet) return;
    let syncing = false;
    const poll = async () => {
      if (syncing) return;
      syncing = true;
      try {
        await syncCalendar();
      } catch {
        // A provider being unreachable is surfaced in the calendar header, not here.
      } finally {
        syncing = false;
      }
    };
    void poll();
    const id = setInterval(() => void poll(), 15 * 60 * 1000);
    return () => clearInterval(id);
  }, [keySet]);

  // Pre-compute the Map's layouts in the background after unlock, at idle priority: the by-project
  // force layout off the main thread (a worker), and the semantic layout in the backend (which defers
  // to an active Drive sync). Both prime caches so opening the Map is instant and never stutters launch.
  useEffect(() => {
    if (keySet) {
      warmMapLayout();
      void startSemanticLayout().catch(() => {});
    }
  }, [keySet]);

  async function selectConversation(id: number) {
    setActiveId(id);
    chat.clearTransient(); // drop any in-flight stream's UI from the conversation we're leaving
    chat.setMessages(await getMessages(id));
  }

  // A chat citation clicked anywhere opens that archived conversation in the global chat view,
  // scrolled to the cited turn (board card 7E PR3). The cited chat may be any conversation (general
  // or another project's), so it always lands in the global chat, not the current project pane. The
  // turn id is set only after its messages load, so ChatView finds the target turn to flash.
  function openChatCitation(conversationId: number, turnId: number | null) {
    setView("chat");
    void selectConversation(conversationId).then(() => {
      if (turnId == null) return; // a chat cited without a specific turn — just open it
      setFocusTurn((prev) => ({ id: turnId, nonce: (prev?.nonce ?? 0) + 1 }));
    });
  }

  function newConversation() {
    setActiveId(null);
    chat.clearTransient();
    chat.setMessages([]);
  }

  // Delete a global conversation for good (card 7G). If it's the one on screen, drop back to a blank
  // new chat; then refresh the sidebar list. The sidebar confirms before calling this.
  async function handleDeleteConversation(id: number) {
    try {
      await deleteConversation(id);
    } catch (e) {
      chat.setError(String(e));
      return;
    }
    if (activeIdRef.current === id) newConversation();
    await refreshConversations();
  }

  async function handleSend(text: string) {
    // Power-user parity with "+ New": /new · /done starts a fresh chat instead of sending, so the
    // trigger never reaches the model or the vault (board card 7E).
    if (isNewChatTrigger(text)) {
      newConversation();
      return;
    }
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
      const [msgs, convs] = await Promise.all([getMessages(convId), listConversations()]);
      setConversations(convs);
      if (activeIdRef.current === convId) {
        chat.setMessages(msgs);
      }
    } catch {
      /* keep optimistic state on reload failure */
    }
  }

  // Switch the chat to a larger-context model (the context meter's Upgrade option, card 7D). Promote the
  // chosen model to primary, keep the others as auto-switch fallbacks, then re-read settings so the picker
  // and sidebar tag reflect it.
  function handleUpgrade(modelId: string) {
    const rest = (settings?.chat_models ?? []).filter((m) => m !== modelId);
    setChatModels([modelId, ...rest])
      .then(refreshSettings)
      .catch(() => {});
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

  // The curtain comes first: another profile is the active writer of this shared vault,
  // so the store is closed here until we take the baton.
  if (vaultLock && !vaultLock.active) {
    return (
      <VaultCurtain
        status={vaultLock}
        reason={curtainReason}
        onChange={() =>
          vaultLockStatus()
            .then(setVaultLock)
            .catch(() => {})
        }
      />
    );
  }

  // The vault unlock gate comes next — it gates real decryption (the store is closed),
  // so it sits ahead of the soft biometric lock and the rest of the app.
  if (vaultNeedsUnlock) {
    return <VaultUnlock status={vault} onUnlocked={completeUnlock} />;
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
            conversations={inProject ? projectChat.conversations : conversations}
            activeId={inProject ? projectChat.convId : activeId}
            reviewCount={reviewCount}
            onSelect={
              inProject
                ? projectChat.openConversation
                : (id) => {
                    setView("chat");
                    selectConversation(id);
                  }
            }
            onDelete={inProject ? projectChat.deleteConversation : handleDeleteConversation}
            onNew={
              inProject
                ? projectChat.newChat
                : () => {
                    setView("chat");
                    newConversation();
                  }
            }
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
                chat={projectChat}
                focusDocId={selectedDocId}
                onOpenChatCitation={openChatCitation}
                onBack={() => setView("focus")}
              />
            </main>
          ) : view === "calendar" ? (
            <main className="flex h-full flex-1 flex-col">
              <CalendarView />
            </main>
          ) : view === "documents" ? (
            <main className="flex h-full flex-1 flex-col">
              {/* No "to review" jump when the learning tools are hidden — there's nowhere to land. */}
              <DocumentsView onReviewClick={teachVisible ? () => setView("review") : undefined} />
            </main>
          ) : view === "review" ? (
            <main className="flex h-full flex-1 flex-col">
              <ReviewView onChanged={refreshReviewCount} />
            </main>
          ) : view === "teach" ? (
            <main className="flex h-full flex-1 flex-col">
              <TeachView />
            </main>
          ) : view === "graph" ? (
            <main className="flex h-full flex-1 flex-col">
              <GraphView onOpenProject={openProject} />
            </main>
          ) : view === "pinboard" ? (
            <main className="flex h-full flex-1 flex-col">
              <PinboardView />
            </main>
          ) : view === "dev" ? (
            <main className="flex h-full flex-1 flex-col">
              <DevView />
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
              {activeConv && (
                <div className="flex items-center border-b border-rule px-4 py-2">
                  <ConversationTitle
                    conversationId={activeConv.id}
                    title={activeConv.title}
                    onRenamed={(title) =>
                      setConversations((prev) =>
                        prev.map((c) => (c.id === activeConv.id ? { ...c, title } : c)),
                      )
                    }
                  />
                </div>
              )}
              <ChatView
                messages={chat.messages}
                streaming={chat.streaming}
                onOpenChatCitation={openChatCitation}
                focusTurn={focusTurn}
              />
              <ContextMeter
                conversationId={activeId}
                refreshKey={chat.messages.length}
                onUpgrade={handleUpgrade}
              />
              <RetrievalExplainPanel messages={chat.messages} />
              <Composer disabled={chat.sending} onSend={handleSend} />
            </main>
          )}

          {showSettings && (
            <div className="absolute inset-0 z-50" style={{ background: "rgba(8,6,4,0.5)" }}>
              <SettingsView
                onboarding={false}
                onClose={() => {
                  setShowSettings(false);
                  refreshSettings();
                }}
                onOpenDev={() => {
                  setShowSettings(false);
                  setView("dev");
                }}
              />
            </div>
          )}

          {showWhatsNew && (
            <div className="absolute inset-0 z-50" style={{ background: "rgba(8,6,4,0.5)" }}>
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
