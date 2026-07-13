// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Where the "Auto" (real-time) Mode preference gets a location — with no geolocation permission
// and no network. We read the device's IANA timezone (`Intl…timeZone`, e.g. "Europe/London") and
// look up a representative latitude/longitude for it. City-level accuracy is plenty for a
// sunrise/sunset light/dark switch (a few minutes' error at most). The coordinates are a small
// static table of public city locations (from the tz database's zone1970.tab) — generic reference
// data, never the user's own position; nothing about the device leaves it. A user who wants exact
// timings can override with their own coordinates in Settings (parseCoords below).

/** [latitude (north +), longitude (east +)] — the sign convention solar.ts expects. */
export type Coords = readonly [number, number];

// Representative coordinates per IANA timezone. Not exhaustive: an unlisted zone resolves to null
// and the caller falls back to the OS light/dark setting (and can prompt for a manual location).
const TZ_COORDS: Record<string, Coords> = {
  UTC: [0, 0],
  "Africa/Abidjan": [5.32, -4.03],
  "Africa/Accra": [5.55, -0.22],
  "Africa/Addis_Ababa": [9.03, 38.74],
  "Africa/Algiers": [36.78, 3.06],
  "Africa/Cairo": [30.05, 31.25],
  "Africa/Casablanca": [33.65, -7.58],
  "Africa/Johannesburg": [-26.25, 28.04],
  "Africa/Lagos": [6.45, 3.4],
  "Africa/Nairobi": [-1.28, 36.82],
  "Africa/Tripoli": [32.9, 13.18],
  "Africa/Tunis": [36.8, 10.18],
  "America/Anchorage": [61.22, -149.9],
  "America/Argentina/Buenos_Aires": [-34.6, -58.45],
  "America/Bogota": [4.6, -74.08],
  "America/Chicago": [41.85, -87.65],
  "America/Denver": [39.74, -104.98],
  "America/Halifax": [44.65, -63.6],
  "America/Havana": [23.13, -82.38],
  "America/Lima": [-12.05, -77.05],
  "America/Los_Angeles": [34.05, -118.24],
  "America/Mexico_City": [19.43, -99.13],
  "America/New_York": [40.71, -74.01],
  "America/Phoenix": [33.45, -112.07],
  "America/Santiago": [-33.45, -70.67],
  "America/Sao_Paulo": [-23.55, -46.63],
  "America/St_Johns": [47.56, -52.71],
  "America/Toronto": [43.65, -79.38],
  "America/Vancouver": [49.28, -123.12],
  "Asia/Almaty": [43.24, 76.9],
  "Asia/Baghdad": [33.34, 44.4],
  "Asia/Baku": [40.4, 49.87],
  "Asia/Bangkok": [13.75, 100.5],
  "Asia/Dhaka": [23.72, 90.41],
  "Asia/Dubai": [25.2, 55.27],
  "Asia/Ho_Chi_Minh": [10.82, 106.63],
  "Asia/Hong_Kong": [22.28, 114.15],
  "Asia/Jakarta": [-6.21, 106.85],
  "Asia/Jerusalem": [31.78, 35.22],
  "Asia/Kabul": [34.53, 69.17],
  "Asia/Karachi": [24.86, 67.01],
  "Asia/Kathmandu": [27.72, 85.32],
  "Asia/Kolkata": [22.57, 88.36],
  "Asia/Kuala_Lumpur": [3.14, 101.69],
  "Asia/Manila": [14.6, 120.98],
  "Asia/Novosibirsk": [55.03, 82.92],
  "Asia/Riyadh": [24.63, 46.72],
  "Asia/Seoul": [37.57, 126.98],
  "Asia/Shanghai": [31.23, 121.47],
  "Asia/Singapore": [1.29, 103.85],
  "Asia/Taipei": [25.03, 121.57],
  "Asia/Tashkent": [41.31, 69.28],
  "Asia/Tehran": [35.7, 51.42],
  "Asia/Tokyo": [35.68, 139.69],
  "Asia/Yangon": [16.8, 96.15],
  "Asia/Yekaterinburg": [56.84, 60.6],
  "Atlantic/Reykjavik": [64.15, -21.95],
  "Australia/Adelaide": [-34.93, 138.6],
  "Australia/Brisbane": [-27.47, 153.03],
  "Australia/Melbourne": [-37.81, 144.96],
  "Australia/Perth": [-31.95, 115.86],
  "Australia/Sydney": [-33.87, 151.21],
  "Europe/Amsterdam": [52.37, 4.89],
  "Europe/Athens": [37.98, 23.73],
  "Europe/Belgrade": [44.82, 20.46],
  "Europe/Berlin": [52.52, 13.4],
  "Europe/Brussels": [50.85, 4.35],
  "Europe/Bucharest": [44.43, 26.1],
  "Europe/Budapest": [47.5, 19.04],
  "Europe/Copenhagen": [55.68, 12.57],
  "Europe/Dublin": [53.33, -6.25],
  "Europe/Helsinki": [60.17, 24.94],
  "Europe/Istanbul": [41.01, 28.98],
  "Europe/Kyiv": [50.45, 30.52],
  "Europe/Lisbon": [38.72, -9.13],
  "Europe/London": [51.51, -0.13],
  "Europe/Madrid": [40.42, -3.7],
  "Europe/Moscow": [55.75, 37.62],
  "Europe/Oslo": [59.91, 10.75],
  "Europe/Paris": [48.85, 2.35],
  "Europe/Prague": [50.08, 14.44],
  "Europe/Rome": [41.9, 12.5],
  "Europe/Stockholm": [59.33, 18.07],
  "Europe/Vienna": [48.21, 16.37],
  "Europe/Warsaw": [52.23, 21.01],
  "Europe/Zurich": [47.37, 8.55],
  "Pacific/Auckland": [-36.85, 174.76],
  "Pacific/Fiji": [-18.14, 178.44],
  "Pacific/Honolulu": [21.31, -157.86],
};

// Deprecated/alias zone names some platforms still report, mapped to a canonical key above.
const TZ_ALIASES: Record<string, string> = {
  "Asia/Calcutta": "Asia/Kolkata",
  "Asia/Saigon": "Asia/Ho_Chi_Minh",
  "Europe/Kiev": "Europe/Kyiv",
  "America/Buenos_Aires": "America/Argentina/Buenos_Aires",
  "US/Pacific": "America/Los_Angeles",
  "US/Eastern": "America/New_York",
  "US/Central": "America/Chicago",
  "US/Mountain": "America/Denver",
  GB: "Europe/London",
  "Etc/UTC": "UTC",
  "Etc/GMT": "UTC",
};

/** Coordinates for an IANA timezone name, or null if we don't have one for it. */
export function coordsForTimezone(tz: string | undefined | null): Coords | null {
  if (!tz) return null;
  const canonical = TZ_ALIASES[tz] ?? tz;
  return TZ_COORDS[canonical] ?? null;
}

/** The device's coordinates, inferred from its IANA timezone. Null if the timezone is unknown to
 *  our table or unavailable. Never prompts, never hits the network. */
export function deviceCoords(): Coords | null {
  try {
    return coordsForTimezone(Intl.DateTimeFormat().resolvedOptions().timeZone);
  } catch {
    return null;
  }
}

/** Parse a user-entered "lat, lon" override (either order of sign, comma- or space-separated).
 *  Returns null on anything out of range or unparseable, so a bad value silently falls back. */
export function parseCoords(raw: string | null | undefined): Coords | null {
  if (!raw) return null;
  const parts = raw
    .split(/[,\s]+/)
    .map((s) => s.trim())
    .filter(Boolean);
  if (parts.length !== 2) return null;
  const lat = Number(parts[0]);
  const lon = Number(parts[1]);
  if (!Number.isFinite(lat) || !Number.isFinite(lon)) return null;
  if (lat < -90 || lat > 90 || lon < -180 || lon > 180) return null;
  return [lat, lon];
}

/** Compact "lat, lon" for display, rounded to 2 dp. */
export function formatCoords(coords: Coords): string {
  return `${coords[0].toFixed(2)}, ${coords[1].toFixed(2)}`;
}

/** The device's IANA time zone (e.g. "Europe/London"), or "UTC" if the runtime can't report one. */
export function deviceTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

/** The effective coordinates for the solar features: a valid user "lat, lon" override, else the
 *  device-timezone's representative coordinates (or null if unknown). This is the ONE derivation the
 *  theme's auto light/dark mode and the calendar's sunrise/sunset both read, so they can't drift. */
export function coordsFor(override: string | null | undefined): Coords | null {
  return parseCoords(override) ?? deviceCoords();
}

/** Every IANA zone the runtime knows (for a manual picker); just the device zone on a runtime
 *  without `Intl.supportedValuesOf`. */
export function allTimeZones(): string[] {
  const intl = Intl as typeof Intl & { supportedValuesOf?: (key: string) => string[] };
  try {
    return typeof intl.supportedValuesOf === "function"
      ? intl.supportedValuesOf("timeZone")
      : [deviceTimeZone()];
  } catch {
    return [deviceTimeZone()];
  }
}

/** Whether `tz` is an IANA zone this runtime accepts — guards persisted/extra zones before use. */
export function isValidTimeZone(tz: string): boolean {
  if (!tz) return false;
  try {
    new Intl.DateTimeFormat("en", { timeZone: tz });
    return true;
  } catch {
    return false;
  }
}
