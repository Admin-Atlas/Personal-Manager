// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Preferences section of the Teach tab (structured preference model, spec §4.5 — PR 2). Lists
// the typed preference records that replaced the free-text "Learning You" blob, and lets the user
// state a new one (a structured form, or "in your own words" parsed by the model into the same
// fields), confirm a migrated/inferred one, edit, or delete. Every action calls the same backend the
// chat/migration use — no new write path — and reuses the Teach-tab list / modal / run-reload shape.

import { useEffect, useState } from "react";
import {
  addPreference,
  confirmPreference,
  deletePreference,
  listPreferences,
  parsePreferenceStatement,
  updatePreference,
} from "../lib/ipc";
import type { Entity, Preference } from "../lib/types";
import { useDevMode } from "../lib/capabilities";
import { useDepth } from "../theme";
import { DevRaw } from "./dev/DevRaw";
import { Button, Card, ConfirmDialog, Input, Modal, Select, Skeleton, Textarea } from "./ui";

const SCOPE_GLOBAL = "global";
const SCOPE_PROJECT = "project";
const SCOPE_CONTEXT = "context";

interface FormState {
  scope: string;
  entityId: number | null;
  condition: string;
  value: string;
}

const EMPTY_FORM: FormState = { scope: SCOPE_GLOBAL, entityId: null, condition: "", value: "" };

const dangerBox = {
  borderColor: "color-mix(in oklab, var(--st-due) 40%, transparent)",
  background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
} as const;

/** The Preferences section, rendered inside the Teach tab below the project list. `projects` is the
 *  Teach tab's already-loaded entity list, reused for the project picker (no extra query). */
export function TeachPreferences({ projects }: { projects: Entity[] }) {
  const { showPower } = useDepth();
  // null = first load (skeleton); [] = loaded-but-empty.
  const [prefs, setPrefs] = useState<Preference[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // The add/edit modal: null = closed; { id: null } = adding; { id } = editing that record.
  const [editing, setEditing] = useState<{ id: number | null; form: FormState } | null>(null);
  const [deleting, setDeleting] = useState<Preference | null>(null);

  async function load() {
    setError(null);
    try {
      setPrefs(await listPreferences());
    } catch (e) {
      setError(String(e));
      setPrefs((prev) => prev ?? []); // don't hang on the skeleton if the first load fails
    }
  }

  useEffect(() => {
    void load();
  }, []);

  // Run a mutation, then reload so the UI reflects the true stored state (mirrors TeachView).
  async function run(fn: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await fn();
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  function openAdd() {
    setEditing({ id: null, form: { ...EMPTY_FORM } });
  }

  function openEdit(p: Preference) {
    setEditing({
      id: p.id,
      form: { scope: p.scope, entityId: p.entity_id, condition: p.condition ?? "", value: p.value },
    });
  }

  async function save(form: FormState) {
    const value = form.value.trim();
    if (!value) return;
    const entityId = form.scope === SCOPE_PROJECT ? form.entityId : null;
    const condition = form.scope === SCOPE_CONTEXT ? form.condition.trim() || null : null;
    const id = editing?.id ?? null;
    setEditing(null);
    await run(() =>
      id == null
        ? addPreference(form.scope, entityId, condition, value)
        : updatePreference(id, form.scope, entityId, condition, value),
    );
  }

  if (prefs == null) {
    return (
      <section className="mt-8 border-t border-rule pt-5">
        <Skeleton className="h-5 w-28" />
        <Skeleton className="mt-3 h-14 w-full" />
      </section>
    );
  }

  return (
    <section className="mt-8 border-t border-rule pt-5" data-help="teach-preferences">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="font-head text-sm font-semibold text-ink">Preferences</h2>
          <p className="mt-0.5 text-xs text-ink3">
            How you like things done. PM brings the relevant ones to mind in chat, sorting, and your
            briefing — instead of one long note.
          </p>
        </div>
        <Button variant="secondary" onClick={openAdd} disabled={busy}>
          Add
        </Button>
      </div>

      {error && (
        <div
          className="mt-3 rounded-[var(--radius)] border px-3 py-2 text-sm text-st-due"
          style={dangerBox}
        >
          {error}
        </div>
      )}

      {prefs.length === 0 ? (
        <p className="mt-4 text-sm text-ink4">
          Nothing yet. Tell PM how you work — “file invoices under Finances”, “keep replies short
          during work hours” — and it'll apply each one where it fits.
        </p>
      ) : (
        <ul className="mt-4 flex flex-col gap-2">
          {prefs.map((p) => (
            <PreferenceRow
              key={p.id}
              pref={p}
              showPower={showPower}
              busy={busy}
              onConfirm={() => void run(() => confirmPreference(p.id))}
              onEdit={() => openEdit(p)}
              onDelete={() => setDeleting(p)}
            />
          ))}
        </ul>
      )}

      {editing && (
        <PreferenceModal
          initial={editing.form}
          isEdit={editing.id != null}
          projects={projects}
          busy={busy}
          onCancel={() => setEditing(null)}
          onSave={save}
        />
      )}

      <ConfirmDialog
        open={deleting != null}
        title="Delete this preference?"
        danger
        confirmLabel="Delete"
        busy={busy}
        onConfirm={() => {
          const id = deleting?.id;
          setDeleting(null);
          if (id != null) void run(() => deletePreference(id));
        }}
        onClose={() => setDeleting(null)}
      >
        {deleting && <p className="text-ink2">“{deleting.value}”</p>}
      </ConfirmDialog>
    </section>
  );
}

function PreferenceRow({
  pref,
  showPower,
  busy,
  onConfirm,
  onEdit,
  onDelete,
}: {
  pref: Preference;
  showPower: boolean;
  busy: boolean;
  onConfirm: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  // A suggestion the user hasn't vouched for yet — offer a one-click confirm. Both migrated/inferred
  // records and ones PM noticed you state in chat (source "chat", card 7F) surface this way; a chat
  // one gets its own origin label so it's clear where the suggestion came from.
  const fromChat = pref.source === "chat";
  const fromImported = pref.source === "imported";
  const unconfirmed =
    (pref.source === "inferred" || fromChat || fromImported) && !pref.user_confirmed;
  const { devMode } = useDevMode();

  return (
    <li>
      <Card className="flex items-start justify-between gap-3 p-3" data-help="teach-pref">
        <div className="min-w-0 flex-1">
          <p className="text-sm text-ink2">{pref.value}</p>
          <div className="mt-1 flex flex-wrap items-center gap-1.5 text-xs">
            <ScopeChip pref={pref} />
            {unconfirmed && (
              <span
                className="rounded-[var(--radius-sm)] border border-border px-1.5 py-0.5 text-ink4"
                title={
                  fromChat
                    ? "PM noticed you said this in chat — keep it if it's right."
                    : fromImported
                      ? "Imported from another AI's memory — keep it if it's right."
                      : "PM carried this over from your earlier profile — keep it if it's right."
                }
              >
                {fromChat ? "Suggested from chat" : fromImported ? "Imported" : "Suggested"}
              </span>
            )}
            {showPower && (
              <span
                className="font-mono text-ink4"
                title={
                  `Where it came from, how sure PM is of it, and its row id.\n\n` +
                  `${Math.round(pref.confidence * 100)}% is the confidence. A preference you typed ` +
                  `in — or one you've since Kept — sits at 100% and is used in prompts. One PM ` +
                  `distilled from a chat or an AI-memory import starts at 60% and is withheld from ` +
                  `prompts until you Keep it.`
                }
              >
                {pref.source} · {Math.round(pref.confidence * 100)}% · id {pref.id}
              </span>
            )}
          </div>
          {devMode && (
            <DevRaw
              label="preference"
              fields={[
                ["id", pref.id],
                ["scope", pref.scope],
                ["entity_id", pref.entity_id],
                ["source", pref.source],
                ["confidence", `${Math.round(pref.confidence * 100)}%`],
                ["user_confirmed", pref.user_confirmed ? "yes" : "no"],
              ]}
            />
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {unconfirmed && (
            <Button variant="secondary" onClick={onConfirm} disabled={busy}>
              ✓ Keep
            </Button>
          )}
          <Button variant="tertiary" onClick={onEdit} disabled={busy}>
            Edit
          </Button>
          <Button variant="tertiary" onClick={onDelete} disabled={busy}>
            Delete
          </Button>
        </div>
      </Card>
    </li>
  );
}

/** The scope chip: "Everywhere" / "Project: X" / "When …". */
function ScopeChip({ pref }: { pref: Preference }) {
  let label: string;
  if (pref.scope === SCOPE_PROJECT) {
    label = pref.project_name ? `Project: ${pref.project_name}` : "A project";
  } else if (pref.scope === SCOPE_CONTEXT) {
    label = pref.condition ? `When ${pref.condition}` : "Situational";
  } else {
    label = "Everywhere";
  }
  return (
    <span className="rounded-[var(--radius-sm)] bg-accent-soft px-1.5 py-0.5 text-accent-text">
      {label}
    </span>
  );
}

function PreferenceModal({
  initial,
  isEdit,
  projects,
  busy,
  onCancel,
  onSave,
}: {
  initial: FormState;
  isEdit: boolean;
  projects: Entity[];
  busy: boolean;
  onCancel: () => void;
  onSave: (form: FormState) => void;
}) {
  const [form, setForm] = useState<FormState>(initial);
  const [sentence, setSentence] = useState("");
  const [parsing, setParsing] = useState(false);
  const [parseError, setParseError] = useState<string | null>(null);

  const set = (patch: Partial<FormState>) => setForm((f) => ({ ...f, ...patch }));

  // The "in your own words" path: a model call turns one sentence into the fields below, which the
  // user reviews before saving (so a mis-parse is corrected, never silently stored).
  async function parse() {
    const text = sentence.trim();
    if (!text) return;
    setParsing(true);
    setParseError(null);
    try {
      const d = await parsePreferenceStatement(text);
      setForm({
        scope: d.scope,
        entityId: d.entity_id,
        condition: d.condition ?? "",
        value: d.value,
      });
    } catch (e) {
      setParseError(String(e));
    } finally {
      setParsing(false);
    }
  }

  const incomplete = !form.value.trim() || (form.scope === SCOPE_PROJECT && form.entityId == null);

  return (
    <Modal open onClose={busy ? () => {} : onCancel} widthClassName="max-w-lg">
      <div className="p-5">
        <h2 className="font-head text-base font-semibold text-ink">
          {isEdit ? "Edit preference" : "Add a preference"}
        </h2>

        {!isEdit && (
          <>
            <div className="mt-4" data-help="teach-pref-nl">
              <label className="block text-xs text-ink3">In your own words</label>
              <div className="mt-1 flex gap-2">
                <Input
                  value={sentence}
                  onChange={(e) => setSentence(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      void parse();
                    }
                  }}
                  placeholder="e.g. file invoices under Finances"
                  className="flex-1"
                  disabled={parsing}
                />
                <Button
                  variant="secondary"
                  onClick={() => void parse()}
                  disabled={parsing || !sentence.trim()}
                >
                  {parsing ? "Reading…" : "Fill in"}
                </Button>
              </div>
              {parseError && <p className="mt-1 text-xs text-st-due">{parseError}</p>}
              <p className="mt-1 text-xs text-faint">
                PM turns it into the fields below — check them before saving.
              </p>
            </div>
            <div className="my-4 border-t border-rule" />
          </>
        )}

        <div className="flex flex-col gap-3">
          <div>
            <label className="block text-xs text-ink3">Applies</label>
            <Select
              className="mt-1 w-full"
              value={form.scope}
              onChange={(e) => set({ scope: e.target.value })}
            >
              <option value={SCOPE_GLOBAL}>Everywhere</option>
              <option value={SCOPE_PROJECT}>To one project</option>
              <option value={SCOPE_CONTEXT}>In a situation</option>
            </Select>
          </div>

          {form.scope === SCOPE_PROJECT && (
            <div>
              <label className="block text-xs text-ink3">Project</label>
              <Select
                className="mt-1 w-full"
                value={form.entityId ?? ""}
                onChange={(e) => set({ entityId: e.target.value ? Number(e.target.value) : null })}
              >
                <option value="">Choose a project…</option>
                {projects.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.canonical_name}
                  </option>
                ))}
              </Select>
              {projects.length === 0 && (
                <p className="mt-1 text-xs text-faint">
                  No projects yet — file some documents first, or pick a different scope.
                </p>
              )}
            </div>
          )}

          {form.scope === SCOPE_CONTEXT && (
            <div>
              <label className="block text-xs text-ink3">When</label>
              <Input
                className="mt-1 w-full"
                value={form.condition}
                onChange={(e) => set({ condition: e.target.value })}
                placeholder="during work hours"
              />
            </div>
          )}

          <div>
            <label className="block text-xs text-ink3">Preference</label>
            <Textarea
              className="mt-1"
              rows={2}
              value={form.value}
              onChange={(e) => set({ value: e.target.value })}
              placeholder="keep replies short and to the point"
            />
          </div>
        </div>

        <div className="mt-5 flex justify-end gap-2">
          <Button variant="tertiary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => onSave(form)} disabled={busy || incomplete}>
            {busy ? "Saving…" : isEdit ? "Save" : "Add preference"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
