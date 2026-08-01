// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Delete a project (board card #573). The sibling of MergeProjectDialog, and deliberately heavier:
// a merge MOVES everything to a place you chose, while this can destroy files, chats and the whole
// activity history with no in-app undo.
//
// So the user chooses what happens to each kind of content rather than getting one fixed policy,
// the counts are computed live from the rows the delete will actually touch, and the confirmation
// is type-the-project-name. Milestones are the one thing with no choice — there is nowhere sensible
// to move a dated milestone whose project is gone — so the dialog warns about them explicitly
// instead of pretending it's a decision.

import { useEffect, useState } from "react";
import { deleteProject, deleteProjectPreview } from "../lib/ipc";
import type {
  ChatDisposition,
  DeletePreview,
  FileDisposition,
  NameDisposition,
} from "../lib/types";
import { Button, Callout, Dialog } from "./ui";

/** One radio row. Plain radios rather than a segmented control: these are consequential, mutually
 *  exclusive choices that each need a sentence of explanation, which a compact toggle can't carry. */
function Choice<T extends string>({
  name,
  value,
  current,
  onSelect,
  label,
  detail,
  disabled,
}: {
  name: string;
  value: T;
  current: T;
  onSelect: (v: T) => void;
  label: string;
  detail: string;
  disabled: boolean;
}) {
  const id = `${name}-${value}`;
  return (
    <label
      htmlFor={id}
      className="flex cursor-pointer items-start gap-2 rounded-[var(--radius-sm)] px-2 py-1.5 hover:bg-surface"
    >
      <input
        id={id}
        type="radio"
        name={name}
        checked={current === value}
        onChange={() => onSelect(value)}
        disabled={disabled}
        className="mt-0.5 shrink-0"
      />
      <span className="text-sm leading-snug">
        <span className="text-ink2">{label}</span>
        <span className="block text-xs text-ink4">{detail}</span>
      </span>
    </label>
  );
}

export function DeleteProjectDialog({
  project,
  onClose,
  onDeleted,
}: {
  project: string;
  onClose: () => void;
  onDeleted: () => void;
}) {
  const [preview, setPreview] = useState<DeletePreview | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [files, setFiles] = useState<FileDisposition>("unsorted");
  const [chats, setChats] = useState<ChatDisposition>("global");
  const [name, setName] = useState<NameDisposition>("unsorted");
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    deleteProjectPreview(project)
      .then((p) => {
        if (!cancelled) setPreview(p);
      })
      .catch((e: unknown) => {
        if (!cancelled) setLoadError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [project]);

  // Confirm against the CANONICAL name for the same reason the merge does: the name the user
  // clicked may be an alias, and the canonical is what actually identifies the project.
  const expected = preview?.canonical ?? "";
  const confirmed = expected.length > 0 && typed.trim() === expected;

  async function run() {
    if (!confirmed || busy) return;
    setBusy(true);
    setError(null);
    try {
      await deleteProject(project, { files, chats, name });
      onDeleted();
      onClose();
    } catch (e: unknown) {
      setError(String(e));
      setBusy(false);
    }
  }

  const destructive = files === "delete" || chats === "delete";

  return (
    <Dialog
      open
      onClose={() => (busy ? undefined : onClose())}
      widthClassName="max-w-lg"
      title={`Delete “${project}”`}
      footer={
        <>
          <Button variant="tertiary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void run()} disabled={!confirmed || busy}>
            {busy ? "Deleting…" : "Delete project"}
          </Button>
        </>
      }
    >
      {loadError && (
        <p role="alert" className="mt-3 text-sm text-st-due">
          {loadError}
        </p>
      )}

      {preview && (
        <>
          <p className="mt-2 text-sm leading-relaxed text-ink3">
            This project holds{" "}
            <span className="font-medium text-ink2">
              {preview.files} file{preview.files === 1 ? "" : "s"}
            </span>
            ,{" "}
            <span className="font-medium text-ink2">
              {preview.chats} chat{preview.chats === 1 ? "" : "s"}
            </span>{" "}
            and{" "}
            <span className="font-medium text-ink2">
              {preview.milestones} milestone{preview.milestones === 1 ? "" : "s"}
            </span>
            . Choose what happens to each.
          </p>

          <fieldset className="mt-4">
            <legend className="text-xs font-medium text-ink3">Its files</legend>
            <Choice
              name="files"
              value="unsorted"
              current={files}
              onSelect={setFiles}
              label="Move to Unsorted"
              detail="Kept and still searchable; they return to the review queue's inbox."
              disabled={busy}
            />
            <Choice
              name="files"
              value="delete"
              current={files}
              onSelect={setFiles}
              label="Delete them"
              detail="Removes the files from your vault and from search. Files indexed from Google Drive or OneDrive are only unlinked from PM — the originals there are never touched."
              disabled={busy}
            />
          </fieldset>

          <fieldset className="mt-3">
            <legend className="text-xs font-medium text-ink3">Its chats</legend>
            <Choice
              name="chats"
              value="global"
              current={chats}
              onSelect={setChats}
              label="Keep as general chats"
              detail="They stay in your history, just no longer tied to a project."
              disabled={busy}
            />
            <Choice
              name="chats"
              value="delete"
              current={chats}
              onSelect={setChats}
              label="Delete them"
              detail="The conversations and their saved transcripts are removed."
              disabled={busy}
            />
          </fieldset>

          <fieldset className="mt-3">
            <legend className="text-xs font-medium text-ink3">Its name</legend>
            <Choice
              name="pname"
              value="unsorted"
              current={name}
              onSelect={setName}
              label="Send anything naming it to Unsorted"
              detail="If this name turns up in a future document, it files to your inbox instead of quietly recreating the project."
              disabled={busy}
            />
            <Choice
              name="pname"
              value="free"
              current={name}
              onSelect={setName}
              label="Free the name"
              detail="The name is available again; a future document naming it starts a brand-new project."
              disabled={busy}
            />
          </fieldset>

          {/* Not a choice — so it is stated, not offered. `live={false}`: this is consequence
                prose that is present at mount, not a failure that appeared. */}
          <Callout as="p" size="md" body="ink" live={false} className="mt-4 text-ink2">
            {preview.milestones > 0 ? (
              <>
                Its {preview.milestones} milestone{preview.milestones === 1 ? "" : "s"} will be
                deleted
              </>
            ) : (
              <>Its milestones will be deleted</>
            )}
            , along with its deadlines, manual priority and the record of when you worked on it.
            There is no way to undo this from inside PM
            {destructive ? ", and deleted files and chats cannot be recovered" : ""}.
          </Callout>

          <label className="mt-4 block text-xs text-ink3" htmlFor="delete-confirm">
            Type <span className="font-medium text-ink2">{preview.canonical}</span> to confirm
          </label>
          <input
            id="delete-confirm"
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
