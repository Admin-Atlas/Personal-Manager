// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The Local AI tab's sections, in the order the tab renders them: the anchor id, the name the
 * settings rail shows, and the help entry the section's wrapper points at.
 *
 * One list rather than three that have to agree. The rail (`settings/registry.ts`) is built from it,
 * the scroll-spy follows the rail's order, and copy that sends someone to a section reads the name
 * from here — so renaming a section renames every pointer to it, instead of leaving one sentence
 * naming a heading that no longer exists.
 */
export const LOCALAI_SECTIONS = [
  { id: "sec-localai-machine", label: "Your machine", help: "settings-localai-machine" },
  { id: "sec-localai-models", label: "Recommended models", help: "settings-localai-models" },
  {
    id: "sec-localai-downloaded",
    label: "Already downloaded",
    help: "settings-localai-downloaded",
  },
  { id: "sec-localai-endpoint", label: "Connect endpoint", help: "settings-localai-endpoint" },
  { id: "sec-localai-roles", label: "Assign roles", help: "settings-localai-roles" },
  { id: "sec-localai-power", label: "On battery", help: "settings-localai-power" },
  { id: "sec-localai-lifecycle", label: "Graphics card", help: "settings-localai-lifecycle" },
] as const;

export type SectionId = (typeof LOCALAI_SECTIONS)[number]["id"];

const LABELS = Object.fromEntries(LOCALAI_SECTIONS.map((s) => [s.id, s.label])) as Record<
  SectionId,
  string
>;

/** A section's name, exactly as the rail shows it. */
export function sectionLabel(id: SectionId): string {
  return LABELS[id];
}
