// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The Local AI tab's sections, in the order the tab renders them: the anchor id, the name the
 * settings rail shows (which is also the section's heading, word for word), and the help entry the
 * section's wrapper points at.
 *
 * The order is the reader's: where they stand and what to do next first, then the server, the jobs
 * it does, the two things that change when the battery or the memory is short — On battery directly
 * after Assign roles because it only moves what that set, and Model memory after it because it holds
 * the release settings On battery shares — and the reference material last.
 *
 * One list rather than three that have to agree. The rail (`settings/registry.ts`) is built from it,
 * the scroll-spy follows the rail's order, and copy that sends someone to a section reads the name
 * from here — so renaming a section renames every pointer to it, instead of leaving one sentence
 * naming a heading that no longer exists.
 */
export const LOCALAI_SECTIONS = [
  { id: "sec-localai-start", label: "Your local model", help: "settings-localai-start" },
  { id: "sec-localai-endpoint", label: "Model server", help: "settings-localai-endpoint" },
  { id: "sec-localai-roles", label: "Assign roles", help: "settings-localai-roles" },
  { id: "sec-localai-power", label: "On battery", help: "settings-localai-power" },
  { id: "sec-localai-lifecycle", label: "Model memory", help: "settings-localai-lifecycle" },
  { id: "sec-localai-models", label: "All models", help: "settings-localai-models" },
  {
    id: "sec-localai-downloaded",
    label: "Already on this device",
    help: "settings-localai-downloaded",
  },
  { id: "sec-localai-machine", label: "Your machine", help: "settings-localai-machine" },
] as const;

export type SectionId = (typeof LOCALAI_SECTIONS)[number]["id"];

const LABELS = Object.fromEntries(LOCALAI_SECTIONS.map((s) => [s.id, s.label])) as Record<
  SectionId,
  string
>;

const HELPS = Object.fromEntries(LOCALAI_SECTIONS.map((s) => [s.id, s.help])) as Record<
  SectionId,
  string
>;

/** A section's name, exactly as the rail shows it — and as its own heading reads. */
export function sectionLabel(id: SectionId): string {
  return LABELS[id];
}

/** The help entry a section's wrapper points at (`data-help`), from the same row as its name. */
export function sectionHelp(id: SectionId): string {
  return HELPS[id];
}
