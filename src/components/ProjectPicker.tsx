// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The multi-project field (#275): a document's projects as removable pills plus one input that
// either reuses an existing project or creates a new one — the shape people already know from an
// email To: field.
//
// It is deliberately SEPARATE from `TagEditor` even though both render pills, because the two
// fields do genuinely different jobs and copying one into the other would break both:
//
//   - Projects keep the user's casing. `TagEditor` lowercases, which is right for a free-form
//     label and wrong here: `projects.name` is a primary key and `entities.canonical_name` is the
//     alias key, so downcasing would stop a name resolving to its own project.
//   - Commas are allowed. `TagEditor` strips them; "Atlas, Inc." is a real project name and the
//     vault's list encoding quotes each entry, so it round-trips.
//   - There is no 5-item cap. That cap is on the model's proposed tags, not on how many projects a
//     document may legitimately belong to.
//   - The FIRST pill is the primary project. It can be changed but never emptied: a document with
//     no project is not a state the store has, so the LAST remaining pill is the one that cannot go.
//
// Order carries meaning: `value[0]` is the primary, and both call sites write it back as
// `[project, ...also_projects]`, so reordering this list IS re-homing the document.
//
// Changing the primary is a first-class act here, not something you achieve sideways. It was
// originally left out — the reasoning being that re-homing should be deliberate — but "deliberate"
// had become "impossible": the primary pill carried no control at all, and the only instruction on
// offer pointed back at this very component. A document filed somewhere you didn't intend is a
// thing you must be able to correct where you notice it.

import { useId, useState } from "react";

export function ProjectPicker({
  value,
  onChange,
  suggestions,
  disabled,
  listId,
  inputClassName,
  hidePrimary,
}: {
  /** The document's projects, primary first. Never empty. */
  value: string[];
  onChange: (projects: string[]) => void;
  /** Existing project names, offered as autocomplete. */
  suggestions: string[];
  disabled?: boolean;
  /** Shared `<datalist>` id, so one list can back several pickers on a page. */
  listId?: string;
  inputClassName?: string;
  /**
   * Omit the primary pill when it names this project — for a list rendered INSIDE a project, where
   * "Primary <this project>" on every row only repeats the heading above them (Bobby, 2026-07-27).
   *
   * Only ever the primary, and only ever this one name. A pill for some OTHER project is the whole
   * point of the field, and a pill for a project this document is merely LINKED into carries a
   * remove control, so hiding either would take away information or a control rather than noise.
   */
  hidePrimary?: string;
}) {
  const [draft, setDraft] = useState("");
  const fallbackId = useId();
  const ownListId = listId ?? `project-picker-${fallbackId}`;

  // Match case-insensitively but keep what the user typed: a project is the same project however it
  // is cased, and the backend resolves the canonical name anyway.
  function add() {
    const name = draft.trim();
    setDraft("");
    if (!name) return;
    if (value.some((p) => p.toLowerCase() === name.toLowerCase())) return;
    onChange([...value, name]);
  }

  function remove(project: string) {
    // The guard is on COUNT, not on position: a document always belongs somewhere, so the last pill
    // can't go — but the primary can, and dropping it promotes the next in line rather than
    // refusing. Filtering preserves order, so `value[1]` becomes the new `value[0]` on its own.
    if (value.length <= 1) return;
    onChange(value.filter((p) => p !== project));
  }

  function makePrimary(project: string) {
    if (project === value[0]) return;
    onChange([project, ...value.filter((p) => p !== project)]);
  }

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {value.map((project, i) => {
        const primary = i === 0;
        // Index, not position: `value[0]` is still the primary whether or not its pill is drawn, so
        // hiding it never promotes the next pill into a Primary badge it hasn't earned.
        if (primary && hidePrimary && project.toLowerCase() === hidePrimary.toLowerCase()) {
          return null;
        }
        return (
          <span
            key={project}
            title={
              primary
                ? `${project} — the primary project. Its filing activity and Map position follow this one.`
                : `${project} — linked. Click the name to make it primary, × to unlink.`
            }
            className={`inline-flex items-center gap-1 rounded-[var(--radius-sm)] px-2 py-0.5 text-xs ${
              primary
                ? "bg-accent-soft text-accent-text"
                : "border border-border2 bg-surface text-ink3"
            }`}
          >
            {primary && <span className="text-[0.625rem] uppercase tracking-wide">Primary</span>}
            {primary ? (
              project
            ) : (
              <button
                type="button"
                onClick={() => makePrimary(project)}
                disabled={disabled}
                // The name IS the control. A separate "make primary" glyph on every linked pill
                // would crowd a field that is one pill wide for almost every document, and the pill
                // body is the largest target the row has to offer.
                className="rounded-[var(--radius-sm)] underline-offset-2 hover:underline disabled:opacity-50"
                aria-label={`Make ${project} the primary project`}
                title={`Make ${project} the primary project`}
              >
                {project}
              </button>
            )}
            {value.length > 1 && (
              <button
                type="button"
                onClick={() => remove(project)}
                disabled={disabled}
                // Persistently visible with hit padding, not a hover-revealed glyph: TeachView's
                // alias chips learned that the hard way — an under-sized target had people
                // clicking the chip body and reporting that removal did nothing.
                className="-mr-0.5 px-1 text-ink4 transition hover:text-ink disabled:opacity-50"
                aria-label={
                  primary
                    ? `Remove ${project}; ${value[1]} becomes the primary project`
                    : `Unlink from ${project}`
                }
                title={primary ? `Remove — ${value[1]} becomes primary` : `Unlink from ${project}`}
              >
                ×
              </button>
            )}
          </span>
        );
      })}
      <input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            add();
          } else if (e.key === "Escape") {
            e.preventDefault();
            setDraft("");
          }
        }}
        // Commit on blur so a typed name isn't lost by clicking Save — the same forgiving
        // behaviour TagEditor has. NOT on comma: a comma is a legal character in a project name.
        onBlur={add}
        disabled={disabled}
        list={ownListId}
        aria-label="Add a project"
        placeholder={value.length ? "link another project…" : "project…"}
        className={
          inputClassName ??
          "w-36 bg-transparent px-1 py-0.5 text-xs text-ink2 outline-none placeholder:text-ink4"
        }
      />
      {!listId && (
        <datalist id={ownListId}>
          {suggestions.map((p) => (
            <option key={p} value={p} />
          ))}
        </datalist>
      )}
    </div>
  );
}

/** A document's projects as one ordered list, primary first — what `ProjectPicker` edits. */
export function projectsOf(doc: { project: string; linked_projects: string[] }): string[] {
  return [doc.project, ...doc.linked_projects];
}

/**
 * The read-only counterpart: how a document's membership reads in a list.
 *
 * Bobby's requirement is that a document in several projects says so where you meet it, and that
 * changing which project is primary is a visible, reachable act rather than something you have to
 * already know about. A single-project document renders exactly as it always did — no new noise
 * for the overwhelmingly common case.
 */
export function ProjectSummary({
  doc,
  className,
}: {
  doc: { project: string; linked_projects: string[] };
  className?: string;
}) {
  const extra = doc.linked_projects.length;
  return (
    <span className={className} title={extra ? projectsOf(doc).join(", ") : undefined}>
      {doc.project}
      {extra > 0 && (
        <span className="ml-1 text-ink4">
          +{extra} more project{extra === 1 ? "" : "s"}
        </span>
      )}
    </span>
  );
}

/**
 * The badge a project's own file list shows against a document whose PRIMARY project is elsewhere.
 *
 * Without it, a linked file is indistinguishable from a filed one, and the user has no way to
 * notice — let alone correct — that a document they think of as belonging here is really homed
 * somewhere else.
 */
export function LinkedBadge({ home }: { home: string }) {
  return (
    <span
      className="shrink-0 rounded-[var(--radius-sm)] border border-border2 px-1.5 py-0.5 text-[0.625rem] uppercase tracking-wide text-ink4"
      title={`Linked here. Its primary project is ${home}.`}
    >
      Linked
    </span>
  );
}
