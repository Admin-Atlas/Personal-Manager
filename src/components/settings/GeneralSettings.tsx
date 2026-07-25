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
import {
  clampFocusUpcomingDays,
  FOCUS_UPCOMING_MAX_DAYS,
  FOCUS_UPCOMING_MIN_DAYS,
  readFocusLayout,
  readFocusUpcomingDays,
  readFocusUpcomingMode,
  readFocusUpcomingRange,
  writeFocusLayout,
  writeFocusUpcomingDays,
  writeFocusUpcomingMode,
  writeFocusUpcomingRange,
  type FocusLayout,
  type FocusUpcomingMode,
} from "../../lib/focusPrefs";
import type { CalendarRange } from "../../lib/calendarPrefs";
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
import { ResetLink, TabResetSection } from "./ResetControls";

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
    fontScale,
    setFontScale,
    teachVisible,
    setTeachVisible,
    mapVisible,
    setMapVisible,
    appearanceIsDefault,
    resetAppearance,
  } = useTheme();
  const help = useHelp();
  // Seeded from localStorage rather than watched: the toggle is the only writer here, and the
  // Pinboard reads the pref fresh at the moment you click delete (see pinboard/prefs.ts).
  const [confirmDelete, setConfirmDelete] = useState(readConfirmDelete);
  // The Focus tab's default layout (split | stacked). Shared with the Focus header toggle; this is the
  // only other writer, seeded from localStorage like confirmDelete above.
  const [focusLayout, setFocusLayout] = useState<FocusLayout>(readFocusLayout);
  // The Focus "Upcoming" section: agenda list vs the few-day grid, plus the grid's hour window and how
  // many days it shows. Shared with the Upcoming header controls (same keys), seeded from localStorage.
  const [focusUpcomingMode, setFocusUpcomingMode] =
    useState<FocusUpcomingMode>(readFocusUpcomingMode);
  const [focusUpcomingRange, setFocusUpcomingRange] =
    useState<CalendarRange>(readFocusUpcomingRange);
  const [focusUpcomingDays, setFocusUpcomingDays] = useState<number>(() =>
    clampFocusUpcomingDays(readFocusUpcomingDays()),
  );
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

  function changeFocusLayout(next: FocusLayout) {
    setFocusLayout(next);
    writeFocusLayout(next); // shared with the Focus header toggle
  }
  function changeFocusUpcomingMode(next: FocusUpcomingMode) {
    setFocusUpcomingMode(next);
    writeFocusUpcomingMode(next); // shared with the Upcoming header toggle
  }
  function changeFocusUpcomingRange(next: CalendarRange) {
    setFocusUpcomingRange(next);
    writeFocusUpcomingRange(next);
  }
  function changeFocusUpcomingDays(next: number) {
    const clamped = clampFocusUpcomingDays(next);
    setFocusUpcomingDays(clamped);
    writeFocusUpcomingDays(clamped);
  }

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

  // --- Reset-to-default (#445) ---
  // Map view defaults: by-project layout, no cohesion, the 1,000-node cap, enhanced (t-SNE) layout on.
  const mapIsDefault =
    mapGrouping === "project" && mapCohesion === 0 && mapNodeCap === 1000 && mapTsneEnabled;
  const confirmDeleteIsDefault = confirmDelete;
  const focusLayoutIsDefault = focusLayout === "split";
  const focusUpcomingIsDefault =
    focusUpcomingMode === "list" && focusUpcomingRange === "day" && focusUpcomingDays === 3;
  const focusIsDefault = focusLayoutIsDefault && focusUpcomingIsDefault;
  const helpIsDefault = !help.enabled;
  // The whole tab, minus the deliberately-excluded time zone (device-derived, not a preference).
  const generalIsDefault =
    appearanceIsDefault &&
    mapIsDefault &&
    confirmDeleteIsDefault &&
    focusIsDefault &&
    helpIsDefault;

  function resetMap() {
    changeMapGrouping("project"); // state + shared localStorage key
    changeMapCohesion(0); // state + shared localStorage key
    setMapNodeCap(1000);
    setMapTsneEnabled(true);
    void persistMapPref(1000, true); // the vault-travelling `map` blob (nodeCap + t-SNE) + relayout
  }
  function resetConfirmDelete() {
    setConfirmDelete(true);
    writeConfirmDelete(true);
  }
  function resetFocus() {
    changeFocusLayout("split");
    changeFocusUpcomingMode("list");
    changeFocusUpcomingRange("day");
    changeFocusUpcomingDays(3);
  }
  function resetGeneral() {
    resetAppearance();
    resetMap();
    resetConfirmDelete();
    resetFocus();
    help.setEnabled(false);
    // Time zone is intentionally left alone — it's derived from the device, not a taste preference.
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
        <div className="flex items-center justify-between gap-2">
          <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
            Appearance
          </label>
          {!appearanceIsDefault && <ResetLink onReset={resetAppearance} label="Reset appearance" />}
        </div>
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
        {/* Text size — mirrored from the Accessibility tab (one setter). A mainstream comfort control,
            not only an accessibility need, so it's surfaced here too. */}
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">Text size</span>
          <SegmentedControl
            value={fontScale}
            onChange={setFontScale}
            options={[
              { value: "small", label: "Small", title: "90%" },
              { value: "default", label: "Default", title: "100%" },
              { value: "large", label: "Large", title: "115%" },
              { value: "xlarge", label: "XL", title: "130%" },
            ]}
          />
        </div>
        <div
          className="mt-3 flex items-center justify-between gap-3"
          data-help="settings-pinboard-confirm-delete"
        >
          <span className="text-sm text-ink2">Confirm before deleting a pinboard card</span>
          <div className="flex items-center gap-2">
            {!confirmDeleteIsDefault && <ResetLink onReset={resetConfirmDelete} />}
            <Toggle
              checked={confirmDelete}
              onChange={(on) => {
                setConfirmDelete(on);
                writeConfirmDelete(on);
              }}
              ariaLabel="Ask before deleting a note or timeline"
            />
          </div>
        </div>
        <div
          className="mt-3 flex items-center justify-between gap-3"
          data-help="settings-teach-tab"
        >
          <span className="text-sm text-ink2">Review &amp; Teach tabs</span>
          <Toggle
            checked={teachVisible}
            onChange={setTeachVisible}
            ariaLabel="Show the Review and Teach tabs"
          />
        </div>
        <div className="mt-3 flex items-center justify-between gap-3" data-help="settings-map-tab">
          <span className="text-sm text-ink2">Map tab</span>
          <Toggle checked={mapVisible} onChange={setMapVisible} ariaLabel="Show the Map tab" />
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
        id="sec-general-focus"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
      >
        <div className="flex items-center justify-between gap-2">
          <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
            Focus
          </label>
          {!focusIsDefault && <ResetLink onReset={resetFocus} label="Reset Focus" />}
        </div>
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">Layout</span>
          <SegmentedControl
            value={focusLayout}
            onChange={changeFocusLayout}
            options={[
              { value: "split", label: "Split" },
              { value: "vertical", label: "Stacked" },
            ]}
          />
        </div>
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-sm text-ink2">Upcoming</span>
          <SegmentedControl
            value={focusUpcomingMode}
            onChange={changeFocusUpcomingMode}
            options={[
              { value: "list", label: "List" },
              { value: "week", label: "Days" },
            ]}
          />
        </div>
        {focusUpcomingMode === "week" && (
          <>
            <div className="mt-3 flex items-center justify-between gap-3">
              <span className="text-sm text-ink2">Upcoming hours</span>
              <SegmentedControl
                value={focusUpcomingRange}
                onChange={changeFocusUpcomingRange}
                options={[
                  { value: "work", label: "Work" },
                  { value: "day", label: "Day" },
                  { value: "full", label: "24h" },
                ]}
              />
            </div>
            <div className="mt-3 flex items-center justify-between gap-3">
              <span className="text-sm text-ink2">Days shown</span>
              <SegmentedControl
                value={String(focusUpcomingDays)}
                onChange={(v) => changeFocusUpcomingDays(Number(v))}
                options={Array.from(
                  { length: FOCUS_UPCOMING_MAX_DAYS - FOCUS_UPCOMING_MIN_DAYS + 1 },
                  (_, i) => {
                    const n = FOCUS_UPCOMING_MIN_DAYS + i;
                    return { value: String(n), label: String(n) };
                  },
                )}
              />
            </div>
          </>
        )}
      </div>

      <div
        id="sec-general-map"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
        data-help="settings-memory-map"
      >
        <div className="flex items-center justify-between gap-2">
          <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
            Memory map
          </label>
          {!mapIsDefault && <ResetLink onReset={resetMap} label="Reset map" />}
        </div>
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
            <Toggle
              checked={mapTsneEnabled}
              onChange={changeTsneEnabled}
              ariaLabel="Use the enhanced t-SNE layout"
            />
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
          <div className="flex items-center gap-2">
            {!helpIsDefault && <ResetLink onReset={() => help.setEnabled(false)} />}
            <Toggle
              checked={help.enabled}
              onChange={help.setEnabled}
              ariaLabel="Help mode"
              className="mt-0.5"
            />
          </div>
        </div>
        <SectionInfo title="What is help mode?">
          <p>
            When on, hovering any highlighted section shows a short explanation of what it does.
          </p>
        </SectionInfo>
      </div>

      <TabResetSection
        tabName="General"
        isDefault={generalIsDefault}
        onReset={resetGeneral}
        confirmBody={
          <>
            Restores appearance (System, Mode, Accent, Depth, Location), the memory-map view, the
            pinboard delete confirmation, and help mode to their defaults. Your time zone is left
            as-is.
          </>
        }
      />
    </>
  );
}
