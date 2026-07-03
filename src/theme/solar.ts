// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Sunrise/sunset for the "Auto" (real-time) Mode preference — pure, dependency-free astronomy so
// the app can flip light/dark by the sun without any network call or location permission (see
// resolveMode.ts, which feeds it timezone-derived coordinates). This is a direct port of the
// NOAA "Sunrise equation" (Wikipedia, § Complete calculation on Earth); it's accurate to about a
// minute, which is far more than a day/night switch needs. Everything here is a pure function of
// (instant, latitude, longitude) so it's trivially testable and never touches the DOM.

const J2000 = 2451545.0; // Julian date of 2000-01-01 12:00 TT
const UNIX_EPOCH_JD = 2440587.5; // Julian date of the Unix epoch
const DEG = Math.PI / 180;

function toJulian(date: Date): number {
  return date.getTime() / 86400000 + UNIX_EPOCH_JD;
}
function fromJulian(jd: number): Date {
  return new Date((jd - UNIX_EPOCH_JD) * 86400000);
}

export interface SunTimes {
  /** Sunrise/sunset for the civil day containing `date`, as absolute instants — or null when the
   *  sun neither rises nor sets that day (see the polar flags). */
  sunrise: Date | null;
  sunset: Date | null;
  /** Polar day: the sun stays above the horizon all day (high summer latitudes). */
  alwaysUp: boolean;
  /** Polar night: the sun never rises that day (deep winter latitudes). */
  alwaysDown: boolean;
}

/** Sunrise and sunset for the day containing `date`, at latitude `latDeg` and longitude
 *  `lonEastDeg` (east positive, e.g. London ≈ -0.13, Tokyo ≈ 139.7). */
export function sunTimes(date: Date, latDeg: number, lonEastDeg: number): SunTimes {
  const lw = -lonEastDeg; // the algorithm uses longitude *west* as positive
  const n = Math.ceil(toJulian(date) - J2000 + 0.0008); // current Julian day (+ leap-second term)
  const jStar = n - lw / 360; // mean solar time
  const M = 357.5291 + 0.98560028 * jStar; // solar mean anomaly (deg)
  const Mr = M * DEG;
  const C = 1.9148 * Math.sin(Mr) + 0.02 * Math.sin(2 * Mr) + 0.0003 * Math.sin(3 * Mr); // centre
  const lambda = (M + C + 180 + 102.9372) * DEG; // ecliptic longitude (rad)
  const jTransit = J2000 + jStar + 0.0053 * Math.sin(Mr) - 0.0069 * Math.sin(2 * lambda); // noon
  const sinDec = Math.sin(lambda) * Math.sin(23.4397 * DEG); // solar declination
  const cosDec = Math.cos(Math.asin(sinDec));
  const phi = latDeg * DEG;
  // -0.833° accounts for atmospheric refraction and the sun's angular radius.
  const cosOmega = (Math.sin(-0.833 * DEG) - Math.sin(phi) * sinDec) / (Math.cos(phi) * cosDec);
  if (cosOmega < -1) return { sunrise: null, sunset: null, alwaysUp: true, alwaysDown: false };
  if (cosOmega > 1) return { sunrise: null, sunset: null, alwaysUp: false, alwaysDown: true };
  const omega = Math.acos(cosOmega) / DEG / 360; // hour angle as a fraction of a day
  return {
    sunrise: fromJulian(jTransit - omega),
    sunset: fromJulian(jTransit + omega),
    alwaysUp: false,
    alwaysDown: false,
  };
}

/** Is it daytime at `now` for the given location? Polar day → true, polar night → false. */
export function isDaytime(now: Date, latDeg: number, lonEastDeg: number): boolean {
  const t = sunTimes(now, latDeg, lonEastDeg);
  if (t.alwaysUp) return true;
  if (t.alwaysDown) return false;
  return now >= (t.sunrise as Date) && now < (t.sunset as Date);
}

/** The next sunrise-or-sunset strictly after `now`, for scheduling the next light/dark flip.
 *  Returns null on a polar day/night (no transition that day — the caller should re-check daily). */
export function nextTransition(now: Date, latDeg: number, lonEastDeg: number): Date | null {
  const t = sunTimes(now, latDeg, lonEastDeg);
  if (t.alwaysUp || t.alwaysDown) return null;
  const upcoming = [t.sunrise as Date, t.sunset as Date]
    .filter((d) => d.getTime() > now.getTime())
    .sort((a, b) => a.getTime() - b.getTime());
  if (upcoming.length > 0) return upcoming[0];
  // Both of today's transitions have passed — the next one is tomorrow's sunrise.
  const tomorrow = sunTimes(new Date(now.getTime() + 86400000), latDeg, lonEastDeg);
  return tomorrow.sunrise;
}
