// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";

import {
  acceptLocalModelTerms,
  activeLocalPull,
  cancelLocalPull,
  pullLocalModel,
} from "../../lib/ipc";
import type {
  LocalRecommendation,
  LocalRecommendations,
  PullProgress,
  PullSnapshot,
} from "../../lib/types";

/** Which section asked for a download, so what goes wrong with it is said there. */
export type PullOrigin = "start" | "models";

/** The one model download, as the tab sees it — what `usePull` hands down. */
export interface ModelPull {
  /** The tag downloading right now (`hf.co/<repo>:<QUANT>`), or null. */
  pulling: string | null;
  pullProg: PullProgress | null;
  /** When the download on screen began (epoch ms): the moment this view asked for it, until the
   *  backend's own stamp arrives — which is the one that survives the tab unmounting. */
  startedAt: number | null;
  /** The last tag this view saw finish successfully, or null. */
  lastPulledTag: string | null;
  /** The model whose licence terms are being shown, and the pull tag the user asked for, or null
   *  when no dialog is open. */
  termsFor: { rec: LocalRecommendation; tag: string; origin: PullOrigin } | null;
  /** Download `tag` (one of `rec`'s rungs), showing the licence first if it needs showing. */
  requestPull: (rec: LocalRecommendation, tag: string, origin?: PullOrigin) => void;
  acceptTermsAndPull: () => Promise<void>;
  closeTerms: () => void;
  cancel: () => void;
}

/**
 * The one-click model pull: which tag is downloading, how far it has got, the licence that has to be
 * answered before a restricted model is fetched, and the recovery when the backend refuses.
 *
 * The tab's, not one section's: the job itself is backend-owned (it survives the tab unmounting),
 * and everything here is the view of it — so it lives where every control that can start a download
 * can read it, and the licence dialog it drives renders once, at the tab.
 *
 * `onError` is told which section asked (`origin`). A download this view only adopted — one the
 * backend was already running when the tab mounted — has no asker, and arrives as `null`.
 */
export function usePull({
  recs,
  onRecs,
  onReload,
  onRefreshRecs,
  onError,
}: {
  recs: LocalRecommendations | null;
  /** Replace the tab's recommendations (a licence acceptance, a finished pull). */
  onRecs: (recs: LocalRecommendations) => void;
  /** Re-read the stored config and the served-model list. */
  onReload: () => Promise<void>;
  /** Re-read the recommendations from the backend. */
  onRefreshRecs: () => Promise<void>;
  /** Show an error, or clear it with `null`, in the section the download belongs to. */
  onError: (message: string | null, origin: PullOrigin | null) => void;
}): ModelPull {
  // `pulling` holds the pull TAG (`hf.co/<repo>:<QUANT>`), not the repo: the backend's job snapshot
  // is keyed on the tag, so a view that mounts mid-download can adopt it and mark the right card.
  const [pulling, setPulling] = useState<string | null>(null);
  const [pullProg, setPullProg] = useState<PullProgress | null>(null);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [lastPulledTag, setLastPulledTag] = useState<string | null>(null);
  /** The model whose licence terms are being shown, and the pull tag the user asked for, or null
   *  when no dialog is open. The TAG rides along because a card can offer more than one way to run
   *  the same model: resolving it again after the dialog would resolve the card's default, not the
   *  row the user actually clicked. */
  const [termsFor, setTermsFor] = useState<ModelPull["termsFor"]>(null);
  /** Mirrors `pulling` synchronously. `pull()` below is async, so the `pulling` it captured when it
   *  started is stale by the time its `finally` runs — and that `finally` must be able to tell
   *  whether the tag it started is still the one on screen. */
  const pullingRef = useRef<string | null>(null);
  /** Who asked for the download this view started, so an error that only surfaces later — on the
   *  snapshot poll — still lands in the asker's section. */
  const askedRef = useRef<{ tag: string; origin: PullOrigin } | null>(null);
  /** The tag a Cancel was pressed for. A cancelled pull resolves like a finished one (the backend
   *  treats a cancel as deliberate, not an error), and only this tells the two apart. */
  const cancelledRef = useRef<string | null>(null);

  /** The single writer for the pull marker. Keeps `pullingRef` in step with the state so the two
   *  can never disagree — a marker cleared in one and not the other silently kills the 1s snapshot
   *  poller, whose effect dependency is `pulling`. */
  const markPulling = useCallback((tag: string | null) => {
    pullingRef.current = tag;
    setPulling(tag);
    if (tag === null) setStartedAt(null);
  }, []);

  /** Mirror the backend's pull job into the view. The snapshot is the source of truth: it survives
   *  this view unmounting, and it is the only thing that knows about a download this component did
   *  not start. A snapshot with nothing running is deliberately NOT a reset — callers decide that. */
  const applyPullSnapshot = useCallback(
    (snap: PullSnapshot | null) => {
      if (!snap?.running) return false;
      markPulling(snap.model);
      setStartedAt(snap.started_at_ms);
      setPullProg({
        status: snap.status,
        completed_bytes: snap.completed_bytes,
        total_bytes: snap.total_bytes,
        done: false,
      });
      return true;
    },
    [markPulling],
  );

  // A download owned by the BACKEND may be running while this view mounts (the tab router unmounts
  // on every switch): adopt it, and while any pull is marked running keep re-reading the snapshot —
  // it is the source of truth that survives the unmount, and its terminal state carries the error
  // a channel nobody was listening to could not deliver.
  useEffect(() => {
    let cancelled = false;
    void activeLocalPull()
      .then((snap) => {
        if (cancelled) return;
        applyPullSnapshot(snap);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [applyPullSnapshot]);
  useEffect(() => {
    if (pulling === null) return;
    let cancelled = false;
    const id = setInterval(() => {
      void activeLocalPull()
        .then((snap) => {
          if (cancelled || !snap || snap.model !== pulling) return;
          if (snap.running) {
            setStartedAt(snap.started_at_ms);
            setPullProg({
              status: snap.status,
              completed_bytes: snap.completed_bytes,
              total_bytes: snap.total_bytes,
              done: false,
            });
            return;
          }
          // Terminal. The locally-started path also lands here if its invoke handler is gone.
          markPulling(null);
          setPullProg(null);
          if (snap.error) onError(snap.error, originOf(snap.model));
          else if (snap.status !== "cancelled") setLastPulledTag(snap.model);
          void onReload();
          void onRefreshRecs();
        })
        .catch(() => {});
    }, 1000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pulling]);

  /** The section that asked for `tag`, or null for a download this view only adopted. */
  function originOf(tag: string): PullOrigin | null {
    return askedRef.current?.tag === tag ? askedRef.current.origin : null;
  }

  /** Download, once the terms behind this model have been shown and accepted (if they need to be).
   *
   *  Restricted-licence models — Gemma, Llama, the largest Qwen — carry publisher terms rather than
   *  an open-source licence, so PM shows them first. Acceptance is remembered per LICENCE, so
   *  reading the Gemma Terms once covers every Gemma. Open-licence models are never interrupted.
   *
   *  This is disclosure, not enforcement: the download is the user's own Ollama fetching the weights
   *  from the publisher, and they could run `ollama pull` without PM at all. */
  function requestPull(rec: LocalRecommendation, tag: string, origin: PullOrigin = "models") {
    const needsTerms = !rec.licence.open && !(recs?.terms_accepted ?? []).includes(rec.licence.id);
    if (needsTerms) {
      setTermsFor({ rec, tag, origin });
      return;
    }
    void pull(tag, origin);
  }

  async function acceptTermsAndPull() {
    const pending = termsFor;
    if (!pending) return;
    const { rec, tag, origin } = pending;
    setTermsFor(null);
    try {
      const accepted = await acceptLocalModelTerms(rec.licence.id);
      if (recs) onRecs({ ...recs, terms_accepted: accepted });
    } catch (e) {
      // The acceptance failed to persist, so the next download of this licence asks again. That is
      // the safe direction: never start the download on the back of a record that wasn't written.
      onError(String(e), origin);
      return;
    }
    await pull(tag, origin);
  }

  async function pull(tag: string, origin: PullOrigin) {
    askedRef.current = { tag, origin };
    cancelledRef.current = null;
    markPulling(tag);
    setStartedAt(Date.now());
    setPullProg(null);
    onError(null, origin);
    try {
      // The job itself is backend-owned (it survives this view unmounting); the channel is just
      // the low-latency progress feed while we ARE mounted — the 1s snapshot poll is the fallback.
      await pullLocalModel(tag, setPullProg);
      if (cancelledRef.current !== tag) setLastPulledTag(tag);
      await onReload(); // the model now shows as served / installed
      await onRefreshRecs();
    } catch (e) {
      onError(String(e), origin);
      // The job is backend-owned and the backend refuses a second concurrent pull, so this is the
      // ordinary outcome of clicking a second Download while one runs. The optimistic mark above has
      // already displaced whatever was running; recover it from the snapshot rather than dropping to
      // null, because null tears down the 1s poller (its dependency is `pulling`) and leaves a live
      // download with no progress bar and no Cancel until the view happens to remount.
      applyPullSnapshot(await activeLocalPull().catch(() => null));
    } finally {
      // Per-tag, never unconditional: the re-adoption above may have just put ANOTHER pull on
      // screen, and this reset runs after it.
      if (pullingRef.current === tag) {
        markPulling(null);
        setPullProg(null);
      }
    }
  }

  function cancel() {
    cancelledRef.current = pullingRef.current;
    void cancelLocalPull().catch(() => {});
  }

  return {
    pulling,
    pullProg,
    startedAt,
    lastPulledTag,
    termsFor,
    requestPull,
    acceptTermsAndPull,
    closeTerms: () => setTermsFor(null),
    cancel,
  };
}
