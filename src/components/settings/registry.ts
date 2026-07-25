// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The single source of truth for the Settings surface: every tab, the group it sits in, its rail
// icon, and its in-rail sub-nav sections. `SettingsView` renders the rail straight from this — add a
// tab or a section here and the rail, grouping, icons, and scroll-spy sub-nav all follow.
//
// Adding a TAB:      append a `SettingsTabDef` to the right group, add an icon in `tabIcons.tsx`, and
//                    render its component behind `{tab === "<id>" && …}` in `SettingsView`.
// Adding a SECTION:  add a `{ id, label }` here AND put that same `id` on the section's element in the
//                    tab component as `id="<id>" data-settings-section`. The two must match — that id
//                    is the scroll-spy anchor and the click-to-scroll target. Sections whose content
//                    is a whole self-contained component are wrapped in a bare
//                    `<div id data-settings-section>` (a transparent anchor — no box, no spacing).
// A tab with `sections: []` never expands in the rail (its content is one self-contained surface).

import type { ComponentType, SVGProps } from "react";

import {
  AccessibilityIcon,
  AiIcon,
  BackupIcon,
  ConnectorsIcon,
  DataIcon,
  DeveloperIcon,
  GeneralIcon,
  LocalAiIcon,
  SearchIcon,
  StorageIcon,
} from "./tabIcons";

export type SettingsTab =
  | "general"
  | "accessibility"
  | "ai"
  | "localai"
  | "search"
  | "connectors"
  | "data"
  | "backup"
  | "storage"
  | "developer";

export interface SettingsSectionDef {
  /** Must equal an `id=` on a `[data-settings-section]` element inside the tab's component. */
  id: string;
  label: string;
}

export interface SettingsTabDef {
  id: SettingsTab;
  label: string;
  Icon: ComponentType<SVGProps<SVGSVGElement>>;
  /** In-rail sub-nav. Empty = the tab doesn't expand (single self-contained surface). */
  sections: readonly SettingsSectionDef[];
}

export interface SettingsGroupDef {
  /** Small uppercase rail heading; `null` for the top, header-less group. */
  header: string | null;
  tabs: readonly SettingsTabDef[];
}

export const SETTINGS_GROUPS: readonly SettingsGroupDef[] = [
  {
    header: null,
    tabs: [
      {
        id: "general",
        label: "General",
        Icon: GeneralIcon,
        sections: [
          { id: "sec-general-appearance", label: "Appearance" },
          { id: "sec-general-focus", label: "Focus" },
          { id: "sec-general-map", label: "Memory map" },
          { id: "sec-general-timezone", label: "Time zone" },
          { id: "sec-general-help", label: "Help mode" },
        ],
      },
      {
        id: "accessibility",
        label: "Accessibility",
        Icon: AccessibilityIcon,
        sections: [
          { id: "sec-a11y-text", label: "Text size" },
          { id: "sec-a11y-density", label: "Density" },
          { id: "sec-a11y-motion", label: "Motion" },
          { id: "sec-a11y-font", label: "Legible font" },
        ],
      },
    ],
  },
  {
    header: "AI",
    tabs: [
      {
        id: "ai",
        label: "AI & Models",
        Icon: AiIcon,
        sections: [
          { id: "sec-ai-keys", label: "API keys" },
          { id: "sec-ai-models", label: "Models" },
          { id: "sec-ai-review", label: "Filing suggestions" },
          { id: "sec-ai-memory", label: "Import AI memory" },
          { id: "sec-ai-usage", label: "Usage & cost" },
        ],
      },
      {
        id: "localai",
        label: "Local AI",
        Icon: LocalAiIcon,
        sections: [
          { id: "sec-localai-machine", label: "Your machine" },
          { id: "sec-localai-models", label: "Recommended models" },
          { id: "sec-localai-endpoint", label: "Connect endpoint" },
          { id: "sec-localai-roles", label: "Assign roles" },
        ],
      },
      { id: "search", label: "Search", Icon: SearchIcon, sections: [] },
    ],
  },
  {
    header: "Data",
    tabs: [
      { id: "connectors", label: "Connectors", Icon: ConnectorsIcon, sections: [] },
      {
        id: "data",
        label: "Data & Security",
        Icon: DataIcon,
        sections: [
          { id: "sec-data-applock", label: "App lock" },
          { id: "sec-data-data", label: "Data" },
          { id: "sec-data-vault", label: "Vault" },
          { id: "sec-data-remove", label: "Remove data" },
          { id: "sec-data-license", label: "License" },
        ],
      },
      { id: "backup", label: "Backup", Icon: BackupIcon, sections: [] },
      { id: "storage", label: "Storage", Icon: StorageIcon, sections: [] },
    ],
  },
  {
    header: "Advanced",
    tabs: [{ id: "developer", label: "Developer", Icon: DeveloperIcon, sections: [] }],
  },
];

/** Every tab, flattened out of the groups — the order the rail renders them in. */
export const SETTINGS_TABS: readonly SettingsTabDef[] = SETTINGS_GROUPS.flatMap((g) => g.tabs);

/** The sub-nav sections for a tab (empty if it doesn't expand). */
export function sectionsFor(id: SettingsTab): readonly SettingsSectionDef[] {
  return SETTINGS_TABS.find((t) => t.id === id)?.sections ?? [];
}
