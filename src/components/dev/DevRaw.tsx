// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A compact in-context developer reveal (issue #78, PR 2): a labelled mono line of key=value
// pairs that sits INSIDE an existing row (an entity, a preference, …) when devMode is on. Lighter
// than DevPanel (no Card) so it reads as an annotation on the row it diagnoses, not a new section.
// Read-only — it only displays values already in hand.

export interface DevRawProps {
  /** Small uppercase tag shown before the fields (defaults to "dev"). */
  label?: string;
  /** Ordered key/value pairs; a null/undefined value renders as "null". */
  fields: Array<[string, string | number | boolean | null | undefined]>;
}

export function DevRaw({ label = "dev", fields }: DevRawProps) {
  return (
    <div className="mt-2 border-t border-rule pt-1.5 font-mono text-[11px] leading-5 text-ink4">
      <span className="uppercase tracking-wide text-ink3">{label}</span>{" "}
      {fields.map(([k, v], i) => (
        <span key={k}>
          {i > 0 && <span className="text-faint"> · </span>}
          {k}=
          <span className="text-ink3">{v === null || v === undefined ? "null" : String(v)}</span>
        </span>
      ))}
    </div>
  );
}
