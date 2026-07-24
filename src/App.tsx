// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { CalendarView } from "./components/calendar/CalendarView";
import { ChatView } from "./components/ChatView";
import { ProviderChip } from "./components/ProviderChip";
import { FallbackStrip } from "./components/FallbackStrip";
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
import { VaultOpenError } from "./components/VaultOpenError";
import { DeletedVaultNotice } from "./components/VaultRecovery";
import { VaultUnlock } from "./components/VaultUnlock";
import { WhatsNew } from "./components/WhatsNew";
import { Skeleton } from "./components/ui";
import { HelpContext } from "./lib/help";
import { ReaderProvider } from "./lib/reader";
import { installAxisScrollNormalizer } from "./lib/scrollAxis";
import { useResizable } from "./lib/useResizable";
import { CollapseTab } from "./components/CollapseTab";
import { useChatStream } from "./lib/useChatStream";
import { useProjectChat } from "./lib/useProjectChat";
import { useLocalLlmStatus } from "./lib/useLocalLlmStatus";
import { aiReady as deriveAiReady } from "./lib/aiGate";
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
  aiProviderStatus,
  listConversations,
  listSharedVaults,
  markActivity,
  onVaultAcquired,
  onVaultCurtain,
  onVaultFault,
  onVaultMetaWarning,
  openUrl,
  resumeDriveSync,
  resumeRebuild,
  resumeLocalFolderSync,
  resumeOneDriveSync,
  reviewQueueCount,
  setChatModels,
  setConversationProject,
  setHelpMode,
  startSemanticLayout,
  syncCalendar,
  vaultLockStatus,
  vaultStatus,
} from "./lib/ipc";
import type {
  Conversation,
  Settings,
  SharedVaultAd,
  VaultLockStatus,
  VaultStatus,
} from "./lib/types";
import { VaultJoin } from "./components/VaultJoin";
import { markJustJoinedVault } from "./lib/joinedVault";

export default function App() {
  const [loading, setLoading] = useState(true);
  const [aiReady, setAiReady] = useState(false);
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
  // Shared vaults other accounts advertised on this PC (issue #337). A fresh profile
  // (no key yet, still on its own device vault) gets the join offer ahead of
  // onboarding; skipping falls through for this launch only, so a one-tap join is
  // never permanently buried.
  const [sharedOffers, setSharedOffers] = useState<SharedVaultAd[]>([]);
  const [joinSkipped, setJoinSkipped] = useState(false);
  const [curtainReason, setCurtainReason] = useState<"other-active" | "handed-off">("other-active");
  // A non-blocking notice when a vault's metadata was repaired on open (M-3): a downgraded
  // encryption policy PM forced back on, or a failed integrity check. Dismissible.
  const [metaWarning, setMetaWarning] = useState<string | null>(null);
  // PM lost access to the shared vault folder MID-SESSION (`vault://fault` — the store
  // closed, or the writer-lock heartbeat started failing while open handles still work).
  // A dismissible banner naming the real problem, so it never masquerades as "locked"
  // until the next relaunch surprises the user (issue #343). Fixed from Settings → Vault.
  const [vaultFaultNotice, setVaultFaultNotice] = useState<string | null>(null);
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
  // Cold start primes the most-recent conversation as active but defers fetching its messages until
  // the first chat-view open (F-09) — the landing view is Focus, so they'd be off-screen anyway.
  const primedConvId = useRef<number | null>(null);
  const chatPrimeLoaded = useRef(false);
  // The on-screen conversation's row (for the editable title header); null on a fresh, unsent chat.
  const activeConv = conversations.find((c) => c.id === activeId) ?? null;
  // Chat send/stream state lives in a shared hook so the global chat and the
  // per-project chat can't drift apart (see useChatStream). The guard key is the
  // conversation currently on screen.
  const chat = useChatStream(() => activeIdRef.current);
  // Live local-endpoint status for the chat provider surfaces (sidebar Local line, composer chip,
  // per-message provenance). One instance, shared down — the "subscribe once" rule (#297 PR6).
  const localAi = useLocalLlmStatus();
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

  // Idle-gate seam (F-08): treat ANY real interaction as active use, so idle-gated background jobs
  // (chat indexer, summary/title/prefs reconcile, backup, activity rollup, flag scan) back off while
  // the user reads/triages/edits — not only on chat sends + ingest, which was the whole starved
  // signal. Leading-edge throttle: at most one tiny IPC every 30s of continuous interaction, well
  // under the smallest 60s idle gate. Discrete intent events only (pointerdown / keydown / wheel) —
  // deliberately NOT pointermove, so idle mouse-drift doesn't read as active; `wheel` covers
  // scroll-while-reading. Passive + capture so it observes even when a child stops propagation, and
  // never blocks or preventDefaults. Mounted unconditionally: marking activity while locked is
  // harmless because every scheduler is independently gated on the vault being open + ready.
  useEffect(() => {
    let last = 0;
    const ACTIVITY_THROTTLE_MS = 30_000;
    const bump = () => {
      const now = Date.now();
      if (now - last < ACTIVITY_THROTTLE_MS) return;
      last = now;
      void markActivity().catch(() => {});
    };
    const opts = { capture: true, passive: true } as const;
    window.addEventListener("pointerdown", bump, opts);
    window.addEventListener("keydown", bump, opts);
    window.addEventListener("wheel", bump, opts);
    return () => {
      window.removeEventListener("pointerdown", bump, opts);
      window.removeEventListener("keydown", bump, opts);
      window.removeEventListener("wheel", bump, opts);
    };
  }, []);

  // Auto-open "What's New" once after the app updates to a version the user
  // hasn't seen yet. We persist the last-seen version so it shows exactly once
  // per upgrade; opening it (or closing the auto-shown one) marks it seen.
  useEffect(() => {
    void (async () => {
      try {
        const version = await getVersion();
        setAppVersion(version);
        const lastSeen = localStorage.getItem(LAST_SEEN_VERSION_KEY);
        if (lastSeen === null) {
          // A brand-new profile has no "what's new" — everything is new. Record the
          // version silently so only genuine upgrades open the modal (it used to fire
          // on every fresh profile, papering over an empty first launch — issue #337).
          localStorage.setItem(LAST_SEEN_VERSION_KEY, version);
        } else if (lastSeen !== version) {
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
    void (async () => {
      try {
        // These boot reads are independent, so fetch them in ONE parallel batch rather than serial
        // round-trips behind each other (F-09). AI readiness is NOT in this batch: `ai_provider_status`
        // reads the encrypted store (onboarding flag + local-endpoint settings), which isn't open
        // until past the locked/curtained/fault/needs-unlock gates below — so it's read after them.
        // The gate ORDER is preserved exactly.
        const [appLocked, vs, writerLock, offers] = await Promise.all([
          appLockStatus().catch(() => null),
          vaultStatus().catch(() => null),
          vaultLockStatus().catch(() => null),
          listSharedVaults().catch(() => []),
        ]);
        setSharedOffers(offers);
        // Resolve the launch lock before the first paint so locked content never flashes.
        if (appLocked?.locked) setLocked(true);
        // A passphrase vault with no cached key on this profile boots locked (the store can't open
        // until unlocked), so defer the store-backed load until it is.
        setVault(vs);
        // Another profile may already be the active writer of a shared vault — then this instance is
        // curtained (its store closed). That takes priority over the unlock prompt (the key is
        // cached; the vault just isn't ours to write right now).
        setVaultLock(writerLock);
        if (writerLock && !writerLock.active) return;
        // The store failed to open at boot (a transient file lock, a denied/gone pointed root,
        // disk I/O) — the open-error gate renders from `vault.fault` and offers the matching way
        // out (Retry / Repair access / detach). Checked before `needs_unlock` because a device
        // vault has no passphrase to prompt for. The store is closed, so skip the load below.
        if (vs?.fault) return;
        if (vs?.needs_unlock) {
          setVaultNeedsUnlock(true);
          return;
        }
        // Store is open past every gate — read AI readiness: a cloud key, a configured local
        // endpoint, or a prior "set up AI later" (#295). Existing keyed users pass on the key.
        const ready = deriveAiReady(await aiProviderStatus());
        setAiReady(ready);
        // Defer the primed conversation's getMessages off the first-paint path (the landing view is
        // Focus, so those messages aren't shown yet); loaded lazily on first chat open (F-09).
        if (ready) await refreshConversations(true, true);
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
      // The store is now open (just unlocked), so both reads are safe in one parallel batch: a
      // vault-status failure is tolerated (null), an ai-provider-status failure surfaces via catch.
      const [vs, status] = await Promise.all([vaultStatus().catch(() => null), aiProviderStatus()]);
      setVault(vs);
      const ready = deriveAiReady(status);
      setAiReady(ready);
      // Passphrase-vault cold start also lands on Focus, so defer messages identically (F-09).
      if (ready) await refreshConversations(true, true);
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
      // Same parallel-batch shape as the boot effect (the two reads are independent; the store is
      // open again on acquisition, so ai_provider_status is safe here).
      const [writerLock, status] = await Promise.all([
        vaultLockStatus().catch(() => null),
        aiProviderStatus(),
      ]);
      setVaultLock(writerLock);
      const ready = deriveAiReady(status);
      setAiReady(ready);
      if (ready) await refreshConversations(true);
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

  // M-3: surface the backend's non-blocking warning when a vault's metadata was repaired on open.
  useEffect(() => {
    let off: (() => void) | undefined;
    void onVaultMetaWarning((message) => setMetaWarning(message)).then((o) => (off = o));
    return () => off?.();
  }, []);

  // Issue #343: surface a mid-session loss of access to the vault folder the moment the
  // backend notices it, instead of a wall of "the vault is locked" errors.
  useEffect(() => {
    let off: (() => void) | undefined;
    void onVaultFault((fault) => setVaultFaultNotice(fault.message)).then((o) => (off = o));
    return () => off?.();
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

  async function refreshConversations(selectFirst = false, deferMessages = false) {
    const list = await listConversations();
    setConversations(list);
    if (selectFirst && list.length > 0) {
      if (deferMessages) {
        // Prime the selection (so the title header renders) but skip the getMessages round-trip; the
        // one-shot effect below loads it when chat is first opened (F-09). Every other caller passes
        // deferMessages=false and loads eagerly, exactly as before.
        setActiveId(list[0].id);
        primedConvId.current = list[0].id;
      } else {
        await selectConversation(list[0].id);
      }
    }
  }

  const refreshReviewCount = useCallback(async () => {
    try {
      setReviewCount(await reviewQueueCount());
    } catch {
      /* count is a hint; ignore failures */
    }
  }, []);

  // Keep the sidebar's review badge current as the user moves around the app.
  useEffect(() => {
    if (aiReady) void refreshReviewCount();
  }, [aiReady, view, refreshReviewCount]);

  // Resume a Drive sync interrupted by a previous close/crash mid-index. Runs once the vault is open
  // (aiReady implies an unlocked store), detached in the backend — already-indexed files survive, so
  // it just finishes the outstanding work. A no-op when there's nothing pending.
  useEffect(() => {
    if (aiReady) void resumeDriveSync().catch(() => {});
  }, [aiReady]);

  // Same for an interrupted OneDrive sync (independent connector, its own resume marker).
  useEffect(() => {
    if (aiReady) void resumeOneDriveSync().catch(() => {});
  }, [aiReady]);

  // Resume a rebuild interrupted by a previous close/crash. Like the connector resumes above, this one
  // genuinely CONTINUES the interrupted pass rather than restarting it (#371): each document the pass
  // committed carries its id, so the resumed run skips them and does only what was left — unless PM
  // updated in between and would now chunk differently, in which case it honestly rebuilds everything.
  // A no-op when there's nothing pending.
  useEffect(() => {
    if (aiReady) void resumeRebuild().catch(() => {});
  }, [aiReady]);

  // And an interrupted local-folder sync (board card 6, its own resume marker). The live filesystem
  // watcher is spawned separately at app setup; this just finishes any walk left mid-index by a close.
  useEffect(() => {
    if (aiReady) void resumeLocalFolderSync().catch(() => {});
  }, [aiReady]);

  // Keep the read-only calendar mirror fresh in the background: one poll shortly after unlock, then
  // every 15 minutes. The mirror feeds the calendar view, the focus agenda, the daily briefing, and
  // chat's "what's on" answer, so it belongs at app scope (not the tab, which unmounts). Best-effort
  // and guarded against overlap; a manual "Refresh now" in the calendar header re-polls on demand.
  useEffect(() => {
    if (!aiReady) return;
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
  }, [aiReady]);

  // Load the boot-primed conversation's messages the first time the user opens chat (F-09): cold
  // start primes the selection but defers this fetch off the first-paint path. Fires at most once.
  // The post-await `activeIdRef.current === target` guard is the correctness anchor — if a "+ New"
  // chat, an explicit sidebar/citation pick, or a focus-box "ask" (which spins up a new conversation
  // in the same tick) has since moved the active conversation, the primed messages are dropped
  // rather than overwriting what's on screen. `messages.length > 0` short-circuits the common case
  // where an explicit selectConversation already loaded a thread.
  useEffect(() => {
    if (view !== "chat" || chatPrimeLoaded.current) return;
    chatPrimeLoaded.current = true;
    const target = primedConvId.current;
    if (target == null || activeId !== target || chat.messages.length > 0) return;
    void getMessages(target)
      .then((msgs) => {
        if (activeIdRef.current === target) chat.setMessages(msgs);
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps -- fire once on first chat-view entry
  }, [view]);

  // Normalise wheel-scroll direction app-wide (installed once): a vertical wheel always scrolls
  // vertically, never getting translated into sideways motion over a horizontally-scrollable table.
  useEffect(() => installAxisScrollNormalizer(), []);

  // The left navigation sidebar's width (fraction of the window, so it stays proportional). It's
  // drag-resizable and snap-collapsible: drag the grip to the window edge and it hides behind a slim
  // reopen tab (see useResizable + CollapseTab).
  const leftBar = useResizable({
    storageKey: "pm.sidebar.frac",
    defaultFrac: 0.17,
    // Half the old 0.14 floor, so the bar can be dragged noticeably thinner before it bottoms out;
    // the snap-collapse still fires only once pinned at this minimum and dragged into the left edge.
    minFrac: 0.07,
    maxFrac: 0.32,
    edge: "right",
    collapsible: true,
  });

  // Pre-compute the Map's layouts in the background after unlock, at idle priority: the by-project
  // force layout off the main thread (a worker), and the semantic layout in the backend (which defers
  // to an active Drive sync). Both prime caches so opening the Map is instant and never stutters launch.
  useEffect(() => {
    if (aiReady) {
      warmMapLayout();
      void startSemanticLayout().catch(() => {});
    }
  }, [aiReady]);

  async function selectConversation(id: number) {
    activeIdRef.current = id; // adopt synchronously so a racing later selection wins the post-await guard
    setActiveId(id);
    chat.clearTransient(); // drop any in-flight stream's UI from the conversation we're leaving
    const msgs = await getMessages(id);
    if (activeIdRef.current !== id) return; // a newer selectConversation landed mid-fetch — don't clobber it
    chat.setMessages(msgs);
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

  // Move a global-list conversation into a project (or back to global). The global list shows every
  // chat regardless of scope, so the moved row simply re-labels and stays put — a refresh is enough,
  // and the on-screen chat (if it's the one moved) re-scopes its retrieval on its next send. Card B.
  async function handleMoveConversation(id: number, project: string | null) {
    try {
      await setConversationProject(id, project);
    } catch (e) {
      chat.setError(String(e));
      return;
    }
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

  // Route a question typed in the focus box (board card 9) to a fresh general chat and send it — that
  // chat is grounded in the same structured flags, so "am I ready for tomorrow?" answers from them.
  // Creates the conversation explicitly (not via handleSend) so it can never append to a stale active
  // conversation still in this closure's state.
  async function askInChat(text: string) {
    setView("chat");
    try {
      const created = await createConversation();
      setActiveId(created.id);
      setConversations((prev) => [created, ...prev]);
      chat.clearTransient();
      chat.setMessages([]);
      await chat.send(created.id, text);
      const [msgs, convs] = await Promise.all([getMessages(created.id), listConversations()]);
      setConversations(convs);
      if (activeIdRef.current === created.id) chat.setMessages(msgs);
    } catch (e) {
      chat.setError(String(e));
    }
  }

  // Switch the chat to a larger-context model (the context meter's Upgrade option, card 7D). Promote the
  // chosen model to primary, keep the others as auto-switch fallbacks, then re-read settings so the picker
  // and sidebar tag reflect it.
  function handleUpgrade(modelId: string) {
    const rest = (settings?.chat_models ?? []).filter((m) => m !== modelId);
    // Return the promise so the context meter can re-read AFTER the switch has landed.
    return setChatModels([modelId, ...rest])
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

  // The pointed shared vault was DELETED by its owner (a discovery tombstone marks it) — a
  // positive, honest notice + switch-to-local, ahead of the generic "couldn't open" gate
  // below (which would otherwise show the unreachable-folder story for a folder that's gone
  // for good). Only set when the folder is genuinely absent, so a live vault never hits this.
  if (vault?.deleted_notice) {
    return <DeletedVaultNotice notice={vault.deleted_notice} onAcknowledged={completeUnlock} />;
  }

  // A boot-time open failure (an AV / search-indexer file lock, a denied or gone pointed
  // root, disk I/O) degrades to this recovery gate instead of aborting the app (B1-6). It
  // sits before the unlock prompt: a vault that failed to OPEN has no passphrase to enter —
  // it needs Retry, Repair access, or a step back to a vault on this account.
  if (vault?.fault) {
    return <VaultOpenError status={vault} onResolved={completeUnlock} />;
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

  // The join offer (issue #337): a fresh profile (no key yet, still on its own device
  // vault) is offered any shared vault another account advertised on this PC, before
  // onboarding. Joining swaps the whole store, so it reloads the webview — the same
  // pattern as the backup-restore switch; the just-joined flag lets onboarding and the
  // Connectors tab explain what's theirs alone. Skipping falls through to onboarding
  // for this launch; the offer returns next launch.
  if (!aiReady && !joinSkipped && vault?.mode === "device" && sharedOffers.length > 0) {
    return (
      <VaultJoin
        vaults={sharedOffers}
        onJoined={() => {
          markJustJoinedVault();
          window.location.reload();
        }}
        onSkip={() => setJoinSkipped(true)}
      />
    );
  }

  if (!aiReady) {
    return (
      <SettingsView
        onboarding
        onClose={async () => {
          setAiReady(true);
          await refreshConversations(true);
        }}
      />
    );
  }

  return (
    <HelpContext.Provider value={{ enabled: helpMode, setEnabled: updateHelpMode }}>
      {/* The document reader mounts once here so any surface — Documents, a project's file list, a
          chat citation — can open it via useReader(). Closes itself when the top-level view changes. */}
      <ReaderProvider view={view} onOpenProject={openProject}>
        <div className={`flex h-full flex-col bg-bg text-ink ${helpMode ? "help-mode" : ""}`}>
          <UpdateBanner update={update} />
          {metaWarning && (
            <div
              role="alert"
              className="flex items-start justify-between gap-3 border-b border-border bg-accent-soft px-4 py-2 text-sm text-ink2"
            >
              <span>{metaWarning}</span>
              <button
                className="shrink-0 font-medium text-ink3 underline underline-offset-2 hover:text-ink"
                onClick={() => setMetaWarning(null)}
              >
                Dismiss
              </button>
            </div>
          )}
          {vaultFaultNotice && (
            <div
              role="alert"
              className="flex items-start justify-between gap-3 border-b border-border px-4 py-2 text-sm text-st-due"
              style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
            >
              <span>{vaultFaultNotice} You can fix this from Settings → Vault.</span>
              <button
                className="shrink-0 font-medium underline underline-offset-2"
                onClick={() => setVaultFaultNotice(null)}
              >
                Dismiss
              </button>
            </div>
          )}
          <div
            className={`relative flex flex-1 overflow-hidden ${leftBar.resizing ? "select-none" : ""}`}
          >
            {leftBar.collapsed && <CollapseTab side="left" onExpand={leftBar.expand} />}
            {!leftBar.collapsed && (
              <Sidebar
                width={leftBar.width}
                onStartResize={leftBar.startResize}
                resizing={leftBar.resizing}
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
                        void selectConversation(id);
                      }
                }
                onDelete={inProject ? projectChat.deleteConversation : handleDeleteConversation}
                onMove={inProject ? projectChat.moveConversation : handleMoveConversation}
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
                localAi={localAi}
              />
            )}

            {view === "focus" ? (
              <main className="flex h-full flex-1 flex-col">
                <FocusView onOpenProject={openProject} onAsk={askInChat} />
              </main>
            ) : view === "project" && selectedProject ? (
              <main className="flex h-full flex-1 flex-col">
                <ProjectView
                  project={selectedProject}
                  chat={projectChat}
                  localAi={localAi}
                  focusDocId={selectedDocId}
                  onOpenChatCitation={openChatCitation}
                  onUpgrade={handleUpgrade}
                  onBack={() => setView("focus")}
                />
              </main>
            ) : view === "calendar" ? (
              <main className="flex h-full flex-1 flex-col">
                <CalendarView
                  onOpenProject={openProject}
                  onOpenPinboard={() => setView("pinboard")}
                />
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
                <GraphView />
              </main>
            ) : view === "pinboard" ? (
              // min-w-0 lets the oversized pinboard board stay contained in its own
              // overflow-auto scroller instead of inflating <main> and shoving the board
              // header's right-aligned buttons off-screen.
              <main className="flex h-full min-w-0 flex-1 flex-col">
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
                {chat.fallback && (
                  <FallbackStrip fallback={chat.fallback} onDismiss={chat.dismissFallback} />
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
                  prompts={chat.prompts}
                  confidences={chat.confidences}
                  providers={chat.providers}
                  showProvenance={!!localAi?.configured}
                  streaming={chat.streaming}
                  onOpenChatCitation={openChatCitation}
                  focusTurn={focusTurn}
                />
                <Composer
                  disabled={chat.sending}
                  onSend={handleSend}
                  leftTools={
                    <div className="flex items-center gap-2">
                      <ContextMeter
                        conversationId={activeId}
                        refreshKey={chat.messages.length}
                        onUpgrade={handleUpgrade}
                      />
                      <ProviderChip status={localAi} />
                    </div>
                  }
                  rightTools={<RetrievalExplainPanel messages={chat.messages} />}
                />
              </main>
            )}

            {showSettings && (
              <div className="absolute inset-0 z-50" style={{ background: "var(--scrim)" }}>
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

            {/* WhatsNew renders its own Modal (scrim + centering, kept below the title bar), so it
                needs no extra wrapper scrim here — that only double-darkened the content. */}
            {showWhatsNew && <WhatsNew onClose={closeWhatsNew} currentVersion={appVersion} />}

            {showPalette && (
              <CommandPalette
                onClose={() => setShowPalette(false)}
                onOpenProject={openProject}
                onOpenConversation={(id) => {
                  setView("chat");
                  void selectConversation(id);
                }}
                onNavigate={setView}
                onOpenSettings={() => setShowSettings(true)}
              />
            )}
          </div>
          <HelpOverlay />
        </div>
      </ReaderProvider>
    </HelpContext.Provider>
  );
}
