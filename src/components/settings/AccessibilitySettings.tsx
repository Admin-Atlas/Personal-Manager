// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Accessibility settings tab. Axes that change how PM presents itself — text size, contrast,
// density / touch-target size, motion, a legible font, and a colour-blind-safe palette — persisted
// with the rest of the theme (so they travel with the vault) and applied instantly. This tab covers
// the full accessibility epic (#502).
//
// Contrast and density each briefly offered a below-baseline level ("Legacy" / "Compact") holding
// PM's original ramps and sizing, so the epic wouldn't change an existing install's look under it.
// Both are withdrawn: they were a sub-AA default nobody was choosing on purpose, and keeping them on
// the picker only invited someone to pick one. Everyone lands on AA / Standard (see ThemeContext for
// how a stored value migrates), and the copy below no longer mentions the levels that are gone.

import { SectionInfo, SegmentedControl, Toggle } from "../ui";
import { useTheme, type FontScale, type Density, type Contrast } from "../../theme";
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
  { value: "standard", label: "Standard", title: "24px touch targets (WCAG 2.5.8)" },
  {
    value: "comfortable",
    label: "Comfortable",
    title: "Larger, 44px targets for lower precision",
  },
];

const CONTRAST_OPTIONS: ReadonlyArray<{ value: Contrast; label: string; title: string }> = [
  { value: "aa", label: "AA", title: "Meets WCAG 1.4.3 (4.5:1 body text)" },
  { value: "high", label: "High", title: "AAA — 7:1 body text, firmer borders" },
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
    colorblind,
    setColorblind,
    contrast,
    setContrast,
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

      <div
        id="sec-a11y-contrast"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
      >
        <div className="flex items-center justify-between gap-2">
          <label className={SECTION_HEAD}>Contrast</label>
          {contrast !== "aa" && <ResetLink onReset={() => setContrast("aa")} />}
        </div>
        <div className={ROW}>
          <span className="text-sm text-ink2">Text &amp; edge contrast</span>
          <SegmentedControl value={contrast} onChange={setContrast} options={CONTRAST_OPTIONS} />
        </div>
        <SectionInfo helpId="settings-a11y-contrast">
          <p>
            Sets how strongly PM's text and edges stand out from the background. “AA” is the default
            and meets the recommended 4.5:1 for body text. “High” goes further (AAA, 7:1) and firms
            up the faintest text and the borders — worth choosing in a bright room, or on a dim
            screen.
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
            Sets how large PM's controls and their tap/click targets are. “Standard” is the default
            and meets the recommended 24px minimum. “Comfortable” grows them to 44px, which helps a
            lot when precise clicking is hard, or on a touchscreen.
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

      <div id="sec-a11y-color" data-settings-section className="mt-5 border-t border-border pt-4">
        <div className="flex items-center justify-between gap-2">
          <label className={SECTION_HEAD}>Colour</label>
          {colorblind && <ResetLink onReset={() => setColorblind(false)} />}
        </div>
        <div className={ROW}>
          <span className="text-sm text-ink2">Colour-blind-safe palette</span>
          <Toggle
            checked={colorblind}
            onChange={setColorblind}
            ariaLabel="Use the colour-blind-safe palette"
          />
        </div>
        <SectionInfo helpId="settings-a11y-color">
          <p>
            Swaps the colours PM uses to tell things apart — status badges (Due soon, Blocked, Quick
            win…), Map nodes, and the per-calendar colours on the Calendar and Upcoming grids — for
            a colour-blind-safe (Okabe–Ito) set chosen to stay distinct under the common types of
            colour vision.
          </p>
          <p>
            It deliberately leaves your <em>theme</em> alone: System, Mode and Accent under
            Appearance are a matter of taste, not of telling two things apart, so PM keeps the look
            you chose and only re-colours the things that carry meaning. Text labels and icons are
            unaffected too — nothing here has ever relied on colour alone.
          </p>
        </SectionInfo>
      </div>

      <TabResetSection
        tabName="Accessibility"
        isDefault={accessibilityIsDefault}
        onReset={resetAccessibility}
        confirmBody={
          <p>
            This sets text size, contrast, density, motion, the legible font, and the colour-blind
            palette back to their defaults. Your theme (system, mode, accent) isn't affected.
          </p>
        }
      />
    </>
  );
}
