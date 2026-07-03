// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Public surface of the theme layer. Components import from "../theme" (or "../../theme").

export {
  SYSTEMS,
  MODES,
  MODE_PREFS,
  DEPTHS,
  ROLES,
  STATUS_KEYS,
  FONTS,
  RADII,
  PROFILES,
  ACCENTS,
  STATUS,
  MONO_ACCENT,
  MONO_RAMP,
  EIGENGRAU,
} from "./profiles";
export type { System, Mode, ModePref, Depth, Role, StatusKey, Fonts } from "./profiles";

export { oklabLCH, hexA } from "./oklab";
export type { OkLCH } from "./oklab";

export { themeVars, applyTheme } from "./tokens";
export type { ThemeVars } from "./tokens";

export { resolveMode, prefersDark } from "./resolveMode";
export type { ModeResolution, ModeSource } from "./resolveMode";

export { sunTimes, isDaytime, nextTransition } from "./solar";
export type { SunTimes } from "./solar";

export { coordsForTimezone, deviceCoords, parseCoords, formatCoords } from "./timezones";
export type { Coords } from "./timezones";

export { ThemeProvider, useTheme } from "./ThemeContext";
export type { ThemeState } from "./ThemeContext";

export { useDepth } from "./depth";
export type { DepthState } from "./depth";

export { graphColor } from "./graphPalette";

export { sourcePalette, sourceColors } from "./sourcePalette";
