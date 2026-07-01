// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import {
  createConversation,
  deleteConversation as apiDeleteConversation,
  getMessages,
  listConversations,
} from "./ipc";
import { useChatStream } from "./useChatStream";
import { isNewChatTrigger } from "./chatSession";
import type { Conversation } from "./types";

/**
 * A project's scoped chat *session*, lifted above `ProjectView` so the left sidebar can list this
 * project's conversations (like the global chat) while the pane renders the active one — both read
 * one source, so a sidebar click and the on-screen thread can't drift (board card 7E).
 *
 * Owns the conversation list (filtered to this project, newest-first from the backend), the active
 * `convId`, the streaming state (its own `useChatStream` — independent of the global chat's, since
 * each streams only through its own `send`), and the idle-dismiss latch. Opening a project lands on
 * a fresh pane; past chats live in the sidebar to resume. Pass `null` when no project is open (the
 * session stays dormant).
 */
export function useProjectChat(project: string | null) {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [convId, setConvId] = useState<number | null>(null);
  // The conversation whose idle-prompt the user dismissed, so it doesn't nag again this session.
  const [dismissedIdleFor, setDismissedIdleFor] = useState<number | null>(null);
  // Mirror convId for the stream guard so switching projects (which nulls convId) abandons an
  // in-flight reply instead of letting it land in the new project.
  const convIdRef = useRef(convId);
  convIdRef.current = convId;
  const chat = useChatStream(() => convIdRef.current);
  // Latest open project, tracked synchronously so a slow `listConversations()` started for the
  // previous project can't resolve late and overwrite the list with the wrong project's chats.
  const projectRef = useRef(project);
  projectRef.current = project;

  const refreshConversations = useCallback(() => {
    const p = project;
    if (p == null) {
      setConversations([]);
      return;
    }
    listConversations()
      .then((all) => {
        if (projectRef.current !== p) return; // stale — the project changed mid-flight
        setConversations(all.filter((c) => c.project === p));
      })
      .catch(() => {});
  }, [project]);

  // Reset the pane when the open project changes (also abandons any in-flight reply): a fresh chat,
  // this project's history loaded. Past chats live in the sidebar to resume from.
  useEffect(() => {
    setConvId(null);
    setDismissedIdleFor(null);
    chat.clearTransient();
    chat.setMessages([]);
    refreshConversations();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- re-init only on project change
  }, [project]);

  /** Resume a past chat from the sidebar: swap its turns into the pane. Guarded so a fast project
   *  switch mid-load can't drop stale messages into the new project. */
  const openConversation = useCallback(
    async (id: number) => {
      if (id === convIdRef.current) return;
      // Adopt the id synchronously (not just via the next render) so the post-await guard is
      // reliable even if getMessages resolves before React commits setConvId.
      convIdRef.current = id;
      setConvId(id);
      setDismissedIdleFor(null);
      chat.clearTransient();
      try {
        const msgs = await getMessages(id);
        if (convIdRef.current === id) chat.setMessages(msgs);
      } catch (e) {
        chat.setError(String(e));
      }
    },
    [chat],
  );

  /** Start a fresh chat in the pane (the sidebar "+ New" button / the /new trigger). The just-left
   *  chat is already persisted and shows in the sidebar, so this is a clean swap — nothing to save. */
  const newChat = useCallback(() => {
    setConvId(null);
    chat.clearTransient();
    chat.setMessages([]);
  }, [chat]);

  /** Delete a past chat from the sidebar (card 7G). If it was the one open in the pane, reset to a
   *  fresh chat; then refresh this project's list. Irreversible — the caller confirms first. */
  const deleteConversation = useCallback(
    async (id: number) => {
      try {
        await apiDeleteConversation(id);
      } catch (e) {
        chat.setError(String(e));
        return;
      }
      if (convIdRef.current === id) newChat();
      refreshConversations();
    },
    [chat, newChat, refreshConversations],
  );

  const handleSend = useCallback(
    async (text: string) => {
      // /new · /done starts a fresh chat instead of sending — never reaches the model or the vault.
      if (isNewChatTrigger(text)) {
        newChat();
        return;
      }
      if (project == null) return;
      let id = convIdRef.current;
      if (id == null) {
        try {
          const created = await createConversation(project);
          id = created.id;
          convIdRef.current = id; // adopt synchronously so the post-send guard is reliable
          setConvId(id);
        } catch (e) {
          chat.setError(String(e));
          return;
        }
      }

      await chat.send(id, text);

      // Adopt persisted messages only if we're still on this project's chat, then refresh the
      // sidebar so a just-created chat (and any background title/order) shows there.
      try {
        if (convIdRef.current === id) chat.setMessages(await getMessages(id));
      } catch {
        /* keep optimistic state on reload failure */
      }
      refreshConversations();
    },
    [project, chat, newChat, refreshConversations],
  );

  return {
    conversations,
    convId,
    messages: chat.messages,
    streaming: chat.streaming,
    sending: chat.sending,
    error: chat.error,
    setError: chat.setError,
    dismissedIdleFor,
    setDismissedIdleFor,
    openConversation,
    newChat,
    deleteConversation,
    handleSend,
    refreshConversations,
  };
}

/** The project chat session shared by App (sidebar) and ProjectView (pane). */
export type ProjectChat = ReturnType<typeof useProjectChat>;
