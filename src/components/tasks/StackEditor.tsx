// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Edit or delete one task stack (PA-1): its name, its kind and its note. The task editor opens it
// from its Stack row; any surface showing a stack can open it the same way. Deleting a stack keeps
// its tasks, out of any stack (the backend's FK sets their `stack_id` to null), and the backend
// fires `tasks://changed` after either write, so every list re-reads.
//
// Portalled to <body> for the same reason as the task editor: a host inside a transformed card
// would trap the fixed scrim. The portal root stops `pointerdown` from reaching a host's drag
// handler.

import { useId, useState } from "react";
import { createPortal } from "react-dom";
import { deleteTaskStack, updateTaskStack } from "../../lib/ipc";
import { runMutation } from "../../lib/runMutation";
import { STACK_KIND_LABEL, STACK_KINDS } from "../../lib/tasks";
import type { TaskStack, TaskStackKind } from "../../lib/types";
import { Button, Callout, ConfirmDialog, Dialog, Input, Select, Textarea } from "../ui";

/** The longest stack name and note the backend takes. */
const TITLE_MAX = 500;
const NOTE_MAX = 10_000;

export interface StackEditorProps {
  /** The stack to edit; null renders nothing. */
  stack: TaskStack | null;
  onClose: () => void;
  /** After a save, with the stack as saved. */
  onSaved?: (stack: TaskStack) => void;
  /** After the stack was deleted (its tasks stay, unstacked). */
  onDeleted?: (id: number) => void;
}

export function StackEditor(props: StackEditorProps) {
  if (!props.stack) return null;
  return createPortal(
    <div onPointerDown={(e) => e.stopPropagation()}>
      <StackEditorBody key={props.stack.id} {...props} stack={props.stack} />
    </div>,
    document.body,
  );
}

function StackEditorBody({
  stack,
  onClose,
  onSaved,
  onDeleted,
}: StackEditorProps & { stack: TaskStack }) {
  const [title, setTitle] = useState(stack.title);
  const [kind, setKind] = useState<TaskStackKind>(stack.kind);
  const [note, setNote] = useState(stack.note ?? "");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const ids = { title: useId(), kind: useId(), note: useId() };

  async function save() {
    const name = title.trim();
    if (!name) {
      setError("A stack needs a name.");
      return;
    }
    const cleanNote = note.trim() ? note.trim() : null;
    setBusy(true);
    const ok = await runMutation(() => updateTaskStack(stack.id, name, kind, cleanNote), setError);
    setBusy(false);
    if (ok) {
      onSaved?.({ ...stack, title: name, kind, note: cleanNote });
      onClose();
    }
  }

  async function remove() {
    setBusy(true);
    const ok = await runMutation(() => deleteTaskStack(stack.id), setError);
    setBusy(false);
    setConfirmDelete(false);
    if (ok) {
      onDeleted?.(stack.id);
      onClose();
    }
  }

  const fieldLabel = "mb-1 block text-xs text-ink3";
  const shownName = stack.title || STACK_KIND_LABEL[stack.kind];

  return (
    <>
      <Dialog
        open
        onClose={busy ? () => {} : onClose}
        title="Edit stack"
        widthClassName="max-w-md"
        footer={
          <>
            <Button
              variant="danger"
              onClick={() => setConfirmDelete(true)}
              disabled={busy}
              className="mr-auto"
            >
              Delete stack
            </Button>
            <Button variant="tertiary" onClick={onClose} disabled={busy}>
              Cancel
            </Button>
            <Button variant="primary" onClick={() => void save()} disabled={busy || !title.trim()}>
              {busy ? "Saving…" : "Save"}
            </Button>
          </>
        }
      >
        <div className="mt-4 flex flex-col gap-3">
          <div>
            <label htmlFor={ids.title} className={fieldLabel}>
              Stack name
            </label>
            <Input
              id={ids.title}
              value={title}
              maxLength={TITLE_MAX}
              required
              data-autofocus
              onChange={(e) => setTitle(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void save();
                }
              }}
              className="h-8 text-xs"
            />
          </div>
          <div>
            <label htmlFor={ids.kind} className={fieldLabel}>
              How its tasks relate
            </label>
            <Select
              id={ids.kind}
              compact
              value={kind}
              onChange={(e) => setKind(e.target.value as TaskStackKind)}
              className="w-full"
            >
              {STACK_KINDS.map((k) => (
                <option key={k} value={k}>
                  {STACK_KIND_LABEL[k]}
                </option>
              ))}
            </Select>
          </div>
          <div>
            <label htmlFor={ids.note} className={fieldLabel}>
              Note (optional)
            </label>
            <Textarea
              id={ids.note}
              value={note}
              rows={2}
              maxLength={NOTE_MAX}
              placeholder="e.g. waiting on the embassy's reply"
              onChange={(e) => setNote(e.target.value)}
            />
          </div>
          {error && <Callout as="p">{error}</Callout>}
        </div>
      </Dialog>
      <ConfirmDialog
        open={confirmDelete}
        title="Delete this stack?"
        confirmLabel="Delete"
        danger
        busy={busy}
        onConfirm={() => void remove()}
        onClose={() => setConfirmDelete(false)}
      >
        “{shownName}” goes. Its tasks stay where they are, just not in a stack.
      </ConfirmDialog>
    </>
  );
}
