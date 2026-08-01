// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Merge into" (board card #279) — the explicit replacement for the `parent` field #278 retired.
// `parent` was a standing half-status that suppressed a project's own status; this is the honest
// version of the one job it actually did: a project that turns out never to have been independent
// is folded into the one it belonged to, and stops existing.
//
// The ceremony is deliberately heavier than the Teach tab's entity merge, because this one is
// reached from Focus where the user is triaging rather than fixing name variants. A merge moves
// files, chats, milestones and the whole activity history, then HARD-DELETES the source project
// row — there is no in-app undo. So the counts are computed live from the same rows the merge
// will move (never estimated), and the user types the target's canonical name to proceed.

import { useEffect, useState } from "react";
import { mergeProjectPreview, mergeProjects } from "../lib/ipc";
import type { MergePreview } from "../lib/types";
import { Button, Callout, Dialog, Select } from "./ui";

/** The live counts as one plain clause, omitting zeros so a project with no milestones doesn't
 *  read "0 milestones". `null` when nothing moves at all — saying so is more honest than an
 *  empty list, and it tells the user this merge is only deleting an empty project. */
export function movesClause(preview: MergePreview): string | null {
  const parts: string[] = [];
  if (preview.files > 0) parts.push(`${preview.files} file${preview.files === 1 ? "" : "s"}`);
  if (preview.chats > 0) parts.push(`${preview.chats} chat${preview.chats === 1 ? "" : "s"}`);
  if (preview.milestones > 0) {
    parts.push(`${preview.milestones} milestone${preview.milestones === 1 ? "" : "s"}`);
  }
  if (parts.length === 0) return null;
  if (parts.length === 1) return parts[0];
  return `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

export function MergeProjectDialog({
  project,
  otherProjects,
  onClose,
  onMerged,
}: {
  /** The project being folded away — the source. It will not exist afterwards. */
  project: string;
  otherProjects: string[];
  onClose: () => void;
  onMerged: () => void;
}) {
  const [target, setTarget] = useState("");
  const [preview, setPreview] = useState<MergePreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-preview whenever the target changes. The backend applies the same guards the merge will,
  // so an impossible pair (merging a project into itself — including via one of its own aliases —
  // or out of Unsorted) surfaces HERE rather than after the user has typed a confirmation.
  // `cancelled` drops a stale in-flight reply so a fast re-pick can't show the wrong counts.
  useEffect(() => {
    if (!target) {
      setPreview(null);
      setPreviewError(null);
      return;
    }
    let cancelled = false;
    setPreview(null);
    setPreviewError(null);
    void mergeProjectPreview(project, target)
      .then((p) => {
        if (!cancelled) setPreview(p);
      })
      .catch((e: unknown) => {
        if (!cancelled) setPreviewError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [project, target]);

  // Confirm against the CANONICAL name, not the option label: if the picked name is an alias,
  // typing that alias would confirm a name the merged documents will never actually carry.
  const expected = preview?.into_canonical ?? "";
  const confirmed = expected.length > 0 && typed.trim() === expected;
  const moves = preview ? movesClause(preview) : null;

  async function run() {
    if (!confirmed || busy) return;
    setBusy(true);
    setError(null);
    try {
      await mergeProjects(project, target);
      onMerged();
      onClose();
    } catch (e: unknown) {
      setError(String(e));
      setBusy(false);
    }
  }

  return (
    <Dialog
      open
      onClose={() => (busy ? undefined : onClose())}
      widthClassName="max-w-md"
      title="Merge into another project"
      footer={
        <>
          <Button variant="tertiary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void run()} disabled={!confirmed || busy}>
            {busy ? "Merging…" : "Merge and delete"}
          </Button>
        </>
      }
    >
      <p className="mt-2 text-sm leading-relaxed text-ink3">
        Fold <span className="font-medium text-ink2">{project}</span> into a project it was always
        part of. Everything it holds moves across, and{" "}
        <span className="font-medium text-ink2">{project}</span> is permanently deleted.
      </p>

      <label className="mt-4 block text-xs text-ink3" htmlFor="merge-target">
        Keep this project
      </label>
      <Select
        id="merge-target"
        value={target}
        onChange={(e) => {
          setTarget(e.target.value);
          setTyped("");
        }}
        className="mt-1 w-full"
        disabled={busy}
      >
        <option value="">Choose a project…</option>
        {otherProjects.map((p) => (
          <option key={p} value={p}>
            {p}
          </option>
        ))}
      </Select>

      {previewError && (
        <Callout as="p" size="md" className="mt-3">
          {previewError}
        </Callout>
      )}

      {preview && (
        <>
          {/* The honest preview: counted from the rows the merge itself will move. */}
          <p className="mt-4 text-sm leading-relaxed text-ink3">
            {moves ? (
              <>
                <span className="font-medium text-ink2">{moves}</span> will move to{" "}
                <span className="font-medium text-ink2">{preview.into_canonical}</span>.
              </>
            ) : (
              <>
                <span className="font-medium text-ink2">{project}</span> is empty — nothing will
                move to <span className="font-medium text-ink2">{preview.into_canonical}</span>.
              </>
            )}{" "}
            Its deadlines, manual priority and activity history are deleted with it. This
            can&rsquo;t be undone from inside PM.
          </p>

          <label className="mt-4 block text-xs text-ink3" htmlFor="merge-confirm">
            Type <span className="font-medium text-ink2">{preview.into_canonical}</span> to confirm
          </label>
          <input
            id="merge-confirm"
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            autoComplete="off"
            spellCheck={false}
            disabled={busy}
            className="mt-1 w-full rounded-[var(--radius-sm)] border border-border2 bg-transparent px-2 py-1 text-sm text-ink outline-none focus:border-accent"
          />
        </>
      )}

      {error && (
        <Callout as="p" size="md" className="mt-3">
          {error}
        </Callout>
      )}
    </Dialog>
  );
}
