// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Public surface of the theme layer. Components import from "../theme" (or "../../theme").

export {
  SYSTEMS,
  MODES,
  DEPTHS,
  ROLES,
  STATUS_KEYS,
  FONTS,
  RADII,
  PROFILES,
  ACCENTS,
  STATUS,
} from "./profiles";
export type { System, Mode, Depth, Role, StatusKey, Fonts } from "./profiles";

export { oklabLCH, hexA } from "./oklab";
export type { OkLCH } from "./oklab";

export { themeVars, applyTheme } from "./tokens";
export type { ThemeVars } from "./tokens";

export { ThemeProvider, useTheme } from "./ThemeContext";
export type { ThemeState } from "./ThemeContext";

export { useDepth } from "./depth";
export type { DepthState } from "./depth";

export { graphColor } from "./graphPalette";
