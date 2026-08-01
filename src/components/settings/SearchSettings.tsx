// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

import {
  getSettings,
  languageOptions,
  setReranking,
  settingsDefaults,
  setVaultEmbedder,
} from "../../lib/ipc";
import type { LanguageOptions } from "../../lib/types";
import { RebuildProgress } from "../RebuildProgress";
import {
  Callout,
  ConfirmDialog,
  SectionInfo,
  SectionLabel,
  SegmentedControl,
  SettingRow,
  Toggle,
} from "../ui";
import { ResetLink } from "./ResetControls";

/** The Search Settings tab: the vault's search language (with the guided re-index on a populated
 *  vault) and the query-time reranking toggle. Self-contained: reranking persists immediately, and
 *  the language switch runs its own confirm + rebuild flow. Errors surface inline here, not in a
 *  shared footer. Onboarding has its own, simpler language picker — this is the non-onboarding one. */
export function SearchSettings() {
  const [reranking, setRerankingState] = useState(true);
  // The out-of-the-box default (from the backend's single defaults source) — drives the per-option
  // "Reset". Re-ranking defaults on.
  const [rerankingDefault, setRerankingDefault] = useState(true);
  const [langOpts, setLangOpts] = useState<LanguageOptions | null>(null);
  const [embedderId, setEmbedderId] = useState("");
  // The pending confirm target, the in-flight switch (drives the guided re-index modal: { to, from }),
  // and any error from the switch itself.
  const [switchTarget, setSwitchTarget] = useState<string | null>(null);
  const [switching, setSwitching] = useState<{ to: string; from: string } | null>(null);
  const [rebuildOpen, setRebuildOpen] = useState(false);
  const [switchError, setSwitchError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getSettings()
      .then((s) => {
        if (!cancelled) setRerankingState(s.reranking);
      })
      .catch(() => {});
    settingsDefaults()
      .then((d) => {
        if (!cancelled) setRerankingDefault(d.reranking);
      })
      .catch(() => {});
    languageOptions()
      .then((lo) => {
        if (!cancelled) {
          setLangOpts(lo);
          setEmbedderId(lo.selected);
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  // Re-sync the language picker with the backend's truth (after a switch lands, or reverts).
  async function reloadLang() {
    try {
      const lo = await languageOptions();
      setLangOpts(lo);
      setEmbedderId(lo.selected);
    } catch {
      /* ignore — the picker simply keeps its last state */
    }
  }

  async function toggleReranking(next: boolean) {
    setError(null);
    setRerankingState(next); // optimistic — revert if the write fails
    try {
      await setReranking(next);
    } catch (e) {
      setRerankingState(!next);
      setError(String(e));
    }
  }

  // A click on the *other* segment: stage the target and open the confirm. The picker's value stays
  // on the current selection until the switch actually lands, so a cancel snaps back.
  function requestLanguageSwitch(newId: string) {
    if (!langOpts || newId === embedderId) return;
    setSwitchError(null);
    setSwitchTarget(newId);
  }

  // Confirmed: record the new embedder. An empty vault is done immediately (the backend resized its
  // empty vector table); a populated vault launches the guided re-index, remembering the old id so a
  // download/offline failure can revert the selection (search keeps working on the old index).
  async function confirmLanguageSwitch() {
    if (!langOpts || !switchTarget) return;
    const to = switchTarget;
    const from = embedderId;
    setSwitchTarget(null);
    setSwitchError(null);
    try {
      await setVaultEmbedder(to);
    } catch (e) {
      setSwitchError(String(e));
      return;
    }
    if (langOpts.has_documents) {
      setSwitching({ to, from });
      setRebuildOpen(true);
    } else {
      setEmbedderId(to);
      await reloadLang();
    }
  }

  return (
    <>
      {error && <Callout className="mt-4">{error}</Callout>}

      <div className="mt-4 border-t border-border pt-4">
        <SectionLabel>Search</SectionLabel>
        {langOpts && langOpts.options.length > 1 && (
          <div className="mt-2">
            <div className="flex items-center justify-between gap-3">
              <p className="text-xs text-ink4">
                Language:{" "}
                <span className="text-ink2">
                  {langOpts.options.find((o) => o.id === embedderId)?.label ?? "English"}
                </span>
              </p>
              <SegmentedControl
                ariaLabel="Search language"
                value={embedderId}
                onChange={requestLanguageSwitch}
                options={langOpts.options.map((o) => ({ value: o.id, label: o.label }))}
              />
            </div>
            {switchError && <p className="mt-1 text-xs text-st-due">{switchError}</p>}
          </div>
        )}
        <SettingRow
          label="Re-rank search results"
          emphasis="strong"
          aside={
            reranking !== rerankingDefault && (
              <ResetLink onReset={() => void toggleReranking(rerankingDefault)} />
            )
          }
        >
          {(a11y) => (
            <Toggle {...a11y} checked={reranking} onChange={(v) => void toggleReranking(v)} />
          )}
        </SettingRow>
        {/* Both of the section's paragraphs in one disclosure at the foot. Folding the
            "switching re-indexes your library" note is safe *because* the confirm dialog
            restates it at the moment it bites — and nothing here is lost either way
            (your Markdown files are the source it re-indexes from). */}
        <SectionInfo title="About search language & re-ranking">
          {langOpts && langOpts.options.length > 1 && (
            <p>
              Switching language re-indexes your whole library from your Markdown files —
              Multilingual downloads a larger model the first time (about 1 GB, once). Your original
              files are never touched.
            </p>
          )}
          <p>
            Re-ranking runs a second pass that re-scores search hits for sharper relevance. First
            use downloads a small model; turn it off for fastest results.
          </p>
        </SectionInfo>
      </div>

      <ConfirmDialog
        open={switchTarget !== null}
        title="Switch search language?"
        confirmLabel="Switch & re-index"
        onConfirm={() => void confirmLanguageSwitch()}
        onClose={() => setSwitchTarget(null)}
      >
        This re-indexes your whole library from your Markdown files
        {langOpts?.options.find((o) => o.id === switchTarget)?.multilingual
          ? ", and downloads a larger language model the first time (about 1 GB, once)"
          : ""}
        . Your original files aren&apos;t changed, and it can take a while on a large library.
      </ConfirmDialog>

      {switching && (
        <RebuildProgress
          open={rebuildOpen}
          title="Switching search language"
          subtitle={`Re-indexing your library for ${
            langOpts?.options.find((o) => o.id === switching.to)?.label ?? "the new language"
          }.`}
          onError={() => {
            // The re-index failed (e.g. offline): revert the selection so search keeps working on
            // the existing index.
            void setVaultEmbedder(switching.from).catch(() => {});
          }}
          onClose={() => {
            setRebuildOpen(false);
            setSwitching(null);
            void reloadLang();
          }}
        />
      )}
    </>
  );
}
