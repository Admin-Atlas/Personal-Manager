// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Accessibility settings tab. Opt-in axes that change how PM presents itself — text size,
// density / touch-target size, motion, and a legible font — persisted with the rest of the theme (so
// they travel with the vault) and applied instantly. Text/motion/font defaults equal PM's normal
// behaviour; density defaults to the compliant `standard` on fresh installs but is pinned to
// `compact` (today's look) for existing installs, so nothing shifts until you choose otherwise or
// Reset. Colour-blind-safe palettes and a high-contrast theme are planned as further axes (epic #502).

import { SectionInfo, SegmentedControl, Toggle } from "../ui";
import { useTheme, type FontScale, type Density } from "../../theme";
import { ResetLink, TabResetSection } from "./ResetControls";

// The Text-size steps. Kept here (and mirrored inline in GeneralSettings' Appearance section) rather
// than exported, so this component file exports only its component (react-refresh hygiene).
const FONT_SIZE_OPTIONS: ReadonlyArray<{ value: FontScale; label: string; title: string }> = [
  { value: "small", label: "Small", title: "90%" },
  { value: "default", label: "Default", title: "100%" },
  { value: "large", label: "Large", title: "115%" },
  { value: "xlarge", label: "XL", title: "130%" },
];

const DENSITY_OPTIONS: ReadonlyArray<{ value: Density; label: string; title: string }> = [
  { value: "compact", label: "Compact", title: "Today's tight spacing — smaller targets" },
  { value: "standard", label: "Standard", title: "Roomier, 24px touch targets (WCAG 2.5.8)" },
  {
    value: "comfortable",
    label: "Comfortable",
    title: "Largest, 44px targets for lower precision",
  },
];

const SECTION_HEAD = "block font-mono text-xs font-medium uppercase tracking-wide text-ink3";
const ROW = "mt-3 flex items-center justify-between gap-3";

export function AccessibilitySettings() {
  const {
    fontScale,
    setFontScale,
    density,
    setDensity,
    reduceMotion,
    setReduceMotion,
    legibleFont,
    setLegibleFont,
    accessibilityIsDefault,
    resetAccessibility,
  } = useTheme();

  return (
    <>
      <div id="sec-a11y-text" data-settings-section className="pt-1">
        <div className="flex items-center justify-between gap-2">
          <label className={SECTION_HEAD}>Text size</label>
          {fontScale !== "default" && <ResetLink onReset={() => setFontScale("default")} />}
        </div>
        <div className={ROW}>
          <span className="text-sm text-ink2">Size</span>
          <SegmentedControl value={fontScale} onChange={setFontScale} options={FONT_SIZE_OPTIONS} />
        </div>
        <SectionInfo helpId="settings-a11y-text">
          <p>
            Scales all of PM's text and spacing together, like your browser's zoom — it's the same
            control as “Text size” under Appearance. Very large sizes may reveal a few spots that
            don't reflow perfectly yet.
          </p>
        </SectionInfo>
      </div>

      <div id="sec-a11y-density" data-settings-section className="mt-5 border-t border-border pt-4">
        <div className="flex items-center justify-between gap-2">
          <label className={SECTION_HEAD}>Controls &amp; touch targets</label>
          {density !== "standard" && <ResetLink onReset={() => setDensity("standard")} />}
        </div>
        <div className={ROW}>
          <span className="text-sm text-ink2">Density</span>
          <SegmentedControl value={density} onChange={setDensity} options={DENSITY_OPTIONS} />
        </div>
        <SectionInfo helpId="settings-a11y-density">
          <p>
            Sets how large PM's controls and their tap/click targets are. “Standard” meets the
            recommended 24px minimum; “Comfortable” grows them to 44px, which helps when precise
            clicking is hard. “Compact” keeps PM's original, tighter spacing.
          </p>
        </SectionInfo>
      </div>

      <div id="sec-a11y-motion" data-settings-section className="mt-5 border-t border-border pt-4">
        <div className="flex items-center justify-between gap-2">
          <label className={SECTION_HEAD}>Motion</label>
          {reduceMotion && <ResetLink onReset={() => setReduceMotion(false)} />}
        </div>
        <div className={ROW}>
          <span className="text-sm text-ink2">Animations</span>
          <SegmentedControl
            value={reduceMotion ? "reduced" : "system"}
            onChange={(v) => setReduceMotion(v === "reduced")}
            options={[
              { value: "system", label: "System", title: "Follow your device's motion setting" },
              { value: "reduced", label: "Reduced", title: "Turn animations and transitions off" },
            ]}
          />
        </div>
        <SectionInfo helpId="settings-a11y-motion">
          <p>
            “System” follows your device's reduce-motion setting. “Reduced” turns PM's animations
            and transitions off regardless — useful if motion is distracting or causes discomfort.
          </p>
        </SectionInfo>
      </div>

      <div id="sec-a11y-font" data-settings-section className="mt-5 border-t border-border pt-4">
        <div className="flex items-center justify-between gap-2">
          <label className={SECTION_HEAD}>Legible font</label>
          {legibleFont && <ResetLink onReset={() => setLegibleFont(false)} />}
        </div>
        <div className={ROW}>
          <span className="text-sm text-ink2">Use Atkinson Hyperlegible</span>
          <Toggle
            checked={legibleFont}
            onChange={setLegibleFont}
            ariaLabel="Use the Atkinson Hyperlegible font"
          />
        </div>
        <SectionInfo helpId="settings-a11y-font">
          <p>
            Switches PM's interface and heading text to Atkinson Hyperlegible — a typeface designed
            for high legibility, with letterforms that are easy to tell apart. Numbers and code keep
            their monospaced font.
          </p>
        </SectionInfo>
      </div>

      <TabResetSection
        tabName="Accessibility"
        isDefault={accessibilityIsDefault}
        onReset={resetAccessibility}
        confirmBody={
          <p>
            This sets text size, density, motion, and the legible font back to their defaults. Your
            theme (system, mode, accent) isn't affected.
          </p>
        }
      />
    </>
  );
}
