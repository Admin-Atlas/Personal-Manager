// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

import { useHelp } from "../../lib/help";
import {
  getPref,
  getSettings,
  installOptionalTsne,
  onTsneInstall,
  optionalTsneStatus,
  setPref,
  setTimeZone,
  startSemanticLayout,
} from "../../lib/ipc";
import {
  MAP_COHESION_KEY,
  MAP_MODE_KEY,
  readMapCohesion,
  readMapMode,
  type MapLayoutMode,
} from "../../lib/mapPrefs";
import { readConfirmDelete, writeConfirmDelete } from "../../lib/pinboard/prefs";
import {
  ACCENTS,
  accentName,
  allTimeZones,
  coordsForTimezone,
  deviceCoords,
  deviceTimeZone,
  EIGENGRAU,
  formatCoords,
  MONO_ACCENT,
  useTheme,
} from "../../theme";
import { IngestProgress } from "../IngestProgress";
import { Button, Input, SectionInfo, SegmentedControl, Select, Toggle } from "../ui";

/** The General Settings tab: appearance (system/mode/accent/depth/location), the memory-map defaults,
 *  time zone, and help mode. Everything here already persists immediately — theme axes through the
 *  theme context, the map prefs through setPref/localStorage, the time zone on change — so there is
 *  nothing to batch. Errors surface inline. */
export function GeneralSettings() {
  const {
    system,
    setSystem,
    mode,
    modePref,
    setModePref,
    modeSource,
    modeCoords,
    modeNextChange,
    autoLocation,
    setAutoLocation,
    depth,
    setDepth,
    accent,
    setAccent,
    teachVisible,
    setTeachVisible,
  } = useTheme();
  const help = useHelp();
  // Seeded from localStorage rather than watched: the toggle is the only writer here, and the
  // Pinboard reads the pref fresh at the moment you click delete (see pinboard/prefs.ts).
  const [confirmDelete, setConfirmDelete] = useState(readConfirmDelete);
  // Memory map (the Map tab): the default grouping, cohesion blend, node cap, and the optional t-SNE
  // component's install/enable state.
  const [mapGrouping, setMapGrouping] = useState<MapLayoutMode>(readMapMode);
  const [mapNodeCap, setMapNodeCap] = useState(1000);
  const [mapCohesion, setMapCohesion] = useState<number>(readMapCohesion);
  const [tsneInstalled, setTsneInstalled] = useState<boolean | null>(null);
  const [mapTsneEnabled, setMapTsneEnabled] = useState(true);
  const [installingTsne, setInstallingTsne] = useState(false);
  const [tsneInstallFrac, setTsneInstallFrac] = useState(0);
  const [timeZone, setTimeZoneState] = useState("");
  const [tzAuto, setTzAuto] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Load the persisted map prefs + the optional-t-SNE install state.
  useEffect(() => {
    getPref("map")
      .then((v) => {
        if (!v) return;
        try {
          const pref = JSON.parse(v);
          if (typeof pref?.nodeCap === "number") setMapNodeCap(pref.nodeCap);
          if (typeof pref?.tsneEnabled === "boolean") setMapTsneEnabled(pref.tsneEnabled);
        } catch {
          /* ignore a malformed pref */
        }
      })
      .catch(() => {});
    optionalTsneStatus()
      .then((s) => setTsneInstalled(s.installed))
      .catch(() => setTsneInstalled(false));
  }, []);

  // The stored time zone (always set by the time you reach this tab; onboarding records it first-run).
  useEffect(() => {
    let cancelled = false;
    getSettings()
      .then((s) => {
        if (cancelled || !s.time_zone) return;
        setTimeZoneState(s.time_zone);
        setTzAuto(s.time_zone === deviceTimeZone());
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  // Follow the optional-t-SNE download's progress so the row shows a real percentage bar.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void onTsneInstall((e) => {
      if (!cancelled) setTsneInstallFrac(e.fraction);
    }).then((u) => {
      unlisten = u;
      if (cancelled) u();
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  function changeMapGrouping(next: MapLayoutMode) {
    setMapGrouping(next);
    localStorage.setItem(MAP_MODE_KEY, next); // shared with the Map header toggle
  }

  function changeMapCohesion(next: number) {
    setMapCohesion(next);
    localStorage.setItem(MAP_COHESION_KEY, String(next)); // shared with the Map's cohesion control
  }

  // The `map` pref is one blob (`{ nodeCap, tsneEnabled }`), both part of the layout fingerprint —
  // write it whole (so neither key is lost) and recompute in the background so the change takes hold.
  function persistMapPref(nodeCap: number, tsneEnabled: boolean) {
    return setPref("map", JSON.stringify({ nodeCap, tsneEnabled }))
      .then(() => startSemanticLayout())
      .catch(() => {});
  }

  function changeMapNodeCap(next: number) {
    setMapNodeCap(next);
    void persistMapPref(next, mapTsneEnabled);
  }

  function changeTsneEnabled(next: boolean) {
    setMapTsneEnabled(next);
    void persistMapPref(mapNodeCap, next);
  }

  function downloadTsne() {
    setInstallingTsne(true);
    setTsneInstallFrac(0);
    installOptionalTsne()
      .then(() => optionalTsneStatus())
      .then((s) => {
        setTsneInstalled(s.installed);
        if (s.installed) {
          setMapTsneEnabled(true); // a fresh install starts enabled
          void persistMapPref(mapNodeCap, true);
        }
      })
      .catch((e) => setError(String(e)))
      .finally(() => setInstallingTsne(false));
  }

  // Time zone saves immediately. Auto persists the device zone; Manual persists the selected zone.
  // The shared time/location context re-reads on the event.
  function changeTz(auto: boolean, zone: string) {
    setTzAuto(auto);
    setTimeZoneState(zone);
    void setTimeZone(auto ? deviceTimeZone() : zone)
      .then(() => window.dispatchEvent(new Event("pm:settings-changed")))
      .catch((e) => setError(String(e)));
  }

  return (
    <>
      {error && (
        <div
          className="mt-4 rounded-[var(--radius-sm)] border px-3 py-2 text-xs"
          style={{
            borderColor: "color-mix(in oklab, var(--st-due) 45%, transparent)",
            background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
            color: "var(--st-due)",
          }}
        >
          {error}
        </div>
      )}

      <div
        id="sec-general-appearance"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
        data-help="settings-appearance"
      >
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Appearance
        </label>
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">System</span>
          <SegmentedControl
            value={system}
            onChange={setSystem}
            options={[
              {
                value: "editorial",
                label: "Editorial",
                title: "Editorial — serif headings, warm paper tones",
              },
              {
                value: "slate",
                label: "Slate",
                title: "Slate — clean sans, cool neutrals (default)",
              },
              {
                value: "terminal",
                label: "Terminal",
                title: "Terminal — monospace, high contrast",
              },
            ]}
          />
        </div>
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">Mode</span>
          <SegmentedControl
            value={modePref}
            onChange={setModePref}
            options={[
              { value: "light", label: "Light" },
              { value: "dark", label: "Dark" },
              {
                value: "system",
                label: "System",
                title: "Follow your device's light/dark setting",
              },
              {
                value: "auto",
                label: "Auto",
                title: "Follow sunrise and sunset at your location",
              },
            ]}
          />
        </div>
        {modePref === "system" && (
          <p className="mt-1.5 text-xs text-ink4">
            Following your device's light/dark setting — currently{" "}
            {mode === "dark" ? "dark" : "light"}.
          </p>
        )}
        {modePref === "auto" && (
          <>
            <p className="mt-1.5 text-xs text-ink4">
              {modeSource === "auto" ? (
                <>
                  Follows sunrise &amp; sunset — currently {mode === "dark" ? "dark" : "light"}
                  {modeCoords ? ` · ${formatCoords(modeCoords)}` : ""}
                  {modeNextChange
                    ? ` · switches to ${mode === "dark" ? "light" : "dark"} at ${modeNextChange.toLocaleTimeString(
                        [],
                        { hour: "2-digit", minute: "2-digit" },
                      )}`
                    : ""}
                  .
                </>
              ) : (
                <>
                  Couldn't determine your location, so it's following your device's light/dark
                  setting for now. Enter a location below for sunrise &amp; sunset.
                </>
              )}
            </p>
            <div className="mt-2 flex items-center justify-between gap-3">
              <span className="text-sm text-ink2">Location</span>
              <div className="flex items-center gap-2">
                <Input
                  value={autoLocation}
                  onChange={(e) => setAutoLocation(e.target.value)}
                  placeholder={
                    deviceCoords()
                      ? `${formatCoords(deviceCoords()!)} (detected)`
                      : "e.g. 51.51, -0.13"
                  }
                  aria-label="Location for sunrise and sunset, as latitude, longitude"
                  className="w-44"
                />
                {autoLocation && (
                  <button
                    type="button"
                    className="text-xs text-ink4 transition hover:text-ink"
                    onClick={() => setAutoLocation("")}
                  >
                    Reset
                  </button>
                )}
              </div>
            </div>
          </>
        )}
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">Depth</span>
          <SegmentedControl
            value={depth}
            onChange={setDepth}
            options={[
              { value: "min", label: "Min" },
              { value: "standard", label: "Standard" },
              { value: "power", label: "Power" },
            ]}
          />
        </div>
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">Accent</span>
          <div className="flex items-center gap-1.5">
            {ACCENTS[system].map((hex) => {
              const isMono = hex === MONO_ACCENT;
              const name = accentName(hex);
              return (
                <button
                  key={hex}
                  type="button"
                  aria-label={isMono ? "Monochrome (Eigengrau)" : `Accent: ${name}`}
                  title={isMono ? "Monochrome — Eigengrau base, white text & accents" : name}
                  onClick={() => setAccent(hex)}
                  style={{
                    background: isMono ? EIGENGRAU : hex,
                    // The Eigengrau swatch is near-black; a white rim makes it legible and
                    // signals the "white accents" treatment.
                    border: isMono ? "1px solid rgba(255,255,255,0.55)" : undefined,
                  }}
                  className={`h-5 w-5 rounded-full transition ${
                    accent === hex
                      ? "ring-2 ring-ink ring-offset-2 ring-offset-[var(--surface)]"
                      : ""
                  }`}
                />
              );
            })}
          </div>
        </div>
        <div
          className="mt-3 flex items-center justify-between gap-3"
          data-help="settings-pinboard-confirm-delete"
        >
          <span className="text-sm text-ink2">Confirm before deleting a pinboard card</span>
          <Toggle
            checked={confirmDelete}
            onChange={(on) => {
              setConfirmDelete(on);
              writeConfirmDelete(on);
            }}
            ariaLabel="Ask before deleting a note or timeline"
          />
        </div>
        <div
          className="mt-3 flex items-center justify-between gap-3"
          data-help="settings-teach-tab"
        >
          <span className="text-sm text-ink2">Review &amp; Teach tabs</span>
          <button
            type="button"
            role="switch"
            aria-checked={teachVisible}
            aria-label="Show the Review and Teach tabs"
            onClick={() => setTeachVisible(!teachVisible)}
            className={`inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
              teachVisible ? "bg-accent" : "bg-surface"
            }`}
          >
            <span
              className={`inline-block h-4 w-4 transform rounded-full bg-accent-ink transition-transform ${
                teachVisible ? "translate-x-4" : "translate-x-0.5"
              }`}
            />
          </button>
        </div>
        {/* Both of Appearance's paragraphs — the "it saves itself" reassurance and the
            Location field's format + privacy note — fold into this one disclosure at the
            foot. The auto-mode status line and its "couldn't find your location" fallback
            stay inline above: they're a readout and a gating hint, not explanation. */}
        <SectionInfo title="What these settings do">
          <p>Applies instantly and is remembered on this device.</p>
          {modePref === "auto" && (
            <p>
              Location is a latitude, longitude pair. Blank uses your device's timezone
              {(() => {
                try {
                  const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
                  return tz && coordsForTimezone(tz) ? ` (${tz})` : "";
                } catch {
                  return "";
                }
              })()}
              . Nothing about your location leaves this device.
            </p>
          )}
        </SectionInfo>
      </div>

      <div
        id="sec-general-map"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
        data-help="settings-memory-map"
      >
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Memory map
        </label>
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">Default grouping</span>
          <SegmentedControl
            value={mapGrouping}
            onChange={changeMapGrouping}
            options={[
              { value: "project", label: "By project" },
              { value: "semantic", label: "Semantic" },
            ]}
          />
        </div>
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">Project cohesion</span>
          <Select
            value={String(mapCohesion)}
            onChange={(e) => changeMapCohesion(Number(e.target.value))}
          >
            <option value="0">Off</option>
            <option value="0.15">Low</option>
            <option value="0.3">Medium</option>
            <option value="0.5">High</option>
          </Select>
        </div>
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">Maximum nodes</span>
          <Select
            value={String(mapNodeCap)}
            onChange={(e) => changeMapNodeCap(Number(e.target.value))}
          >
            {[200, 500, 1000, 2000, 3500, 5000].map((n) => (
              <option key={n} value={n}>
                {n.toLocaleString()}
              </option>
            ))}
          </Select>
        </div>
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">Enhanced layout (t-SNE)</span>
          {tsneInstalled === null ? (
            <span className="text-xs text-ink4">…</span>
          ) : tsneInstalled ? (
            <button
              type="button"
              role="switch"
              aria-checked={mapTsneEnabled}
              aria-label="Use the enhanced t-SNE layout"
              onClick={() => changeTsneEnabled(!mapTsneEnabled)}
              className={`inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
                mapTsneEnabled ? "bg-accent" : "bg-surface"
              }`}
            >
              <span
                className={`inline-block h-4 w-4 transform rounded-full bg-accent-ink transition-transform ${
                  mapTsneEnabled ? "translate-x-4" : "translate-x-0.5"
                }`}
              />
            </button>
          ) : (
            <Button variant="secondary" onClick={downloadTsne} disabled={installingTsne}>
              {installingTsne ? "Downloading…" : "Download"}
            </Button>
          )}
        </div>
        {installingTsne && (
          <IngestProgress
            mode="percent"
            processed={Math.round(tsneInstallFrac * 100)}
            total={100}
            label="Downloading the enhanced (t-SNE) layout"
            className="mt-2"
          />
        )}
        {/* The section's opening blurb and its trailing rationale said one thing between
            them — how the map is laid out — so they're one disclosure now. */}
        <SectionInfo title="How the map is arranged">
          <p>The Map tab — how documents are arranged and how many are plotted.</p>
          <p>
            Semantic proximity uses a basic on-device layout by default. Project cohesion gently
            pulls same-project documents together (Off keeps the layout purely by meaning). The
            optional t-SNE component (a one-time download) produces tighter clusters of related
            documents — turn it on or off here, or remove it to free space under Settings → Storage.
          </p>
        </SectionInfo>
      </div>

      <div
        id="sec-general-timezone"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
        data-help="settings-timezone"
      >
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Time zone
        </label>
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">Detection</span>
          <SegmentedControl
            value={tzAuto ? "auto" : "manual"}
            onChange={(v) => changeTz(v === "auto", timeZone)}
            options={[
              { value: "auto", label: "Auto" },
              { value: "manual", label: "Manual" },
            ]}
          />
        </div>
        {!tzAuto && (
          <div className="mt-3 flex items-center justify-between gap-3">
            <span className="text-sm text-ink2">Zone</span>
            <Select
              value={timeZone}
              onChange={(e) => changeTz(false, e.target.value)}
              className="max-w-[14rem]"
            >
              {allTimeZones().map((z) => (
                <option key={z} value={z}>
                  {z}
                </option>
              ))}
            </Select>
          </div>
        )}
        {/* The zone in force is a readout — it stays visible; only what the zone *means* folds away. */}
        <p className="mt-2 text-xs text-faint">
          {tzAuto ? `Following this device: ${deviceTimeZone()}` : `Selected: ${timeZone || "—"}`}
        </p>
        <SectionInfo title="What the time zone affects">
          <p>Sets “today”, “due soon”, and your calendar agenda. Auto follows this device.</p>
        </SectionInfo>
      </div>

      <div
        id="sec-general-help"
        data-settings-section
        className="mt-4 border-t border-border pt-4"
        data-help="settings-help-mode"
      >
        <div className="flex items-start justify-between gap-3">
          <label className="block text-sm font-medium text-ink2">Help mode</label>
          <button
            role="switch"
            aria-checked={help.enabled}
            onClick={() => help.setEnabled(!help.enabled)}
            className={`mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
              help.enabled ? "bg-accent" : "bg-surface"
            }`}
          >
            <span
              className={`inline-block h-4 w-4 transform rounded-full bg-accent-ink transition-transform ${
                help.enabled ? "translate-x-4" : "translate-x-0.5"
              }`}
            />
          </button>
        </div>
        <SectionInfo title="What is help mode?">
          <p>
            When on, hovering any highlighted section shows a short explanation of what it does.
          </p>
        </SectionInfo>
      </div>
    </>
  );
}
