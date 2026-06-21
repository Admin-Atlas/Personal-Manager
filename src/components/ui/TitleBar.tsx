// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactNode } from "react";

// Custom window chrome lands in PR3 (Tauri `decorations: false` + per-OS controls + the
// per-System treatment: editorial/slate traffic-light dots, terminal `● pm [alpha] ~/pm/<view>`
// status bar). Until then the native OS titlebar stays and this renders nothing — the interface
// is fixed now so PR3 is a drop-in, not a refactor of every consumer.
export interface TitleBarProps {
  /** Active view label — shown in the terminal status bar (PR3). */
  view?: string;
  /** Trailing controls slot, e.g. the ⌘K affordance (PR3). */
  trailing?: ReactNode;
}

export function TitleBar(_props: TitleBarProps): ReactNode {
  return null;
}
