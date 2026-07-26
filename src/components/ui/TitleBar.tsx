// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Custom window chrome (PR3). Native decorations are off (tauri.conf.json `decorations: false`), so
// this bar provides window dragging (data-tauri-drag-region), double-click-to-maximize, and the
// minimize / maximize / close controls. Controls follow OS convention: macOS = traffic-light dots on
// the left (the design's --border2 dots); Windows/Linux = caption buttons on the right. Per-System
// styling rides the active theme. Mounted ABOVE <App/> in main.tsx so it is present on every screen
// (incl. the loading + onboarding states) — a frameless window needs a way to drag/close throughout.
//
// Known limitations (flagged for OS testing): Windows 11 Snap Layouts (the maximize-button fly-out)
// is unavailable with custom decorations without native Rust work; on macOS this draws custom dots
// rather than the native traffic lights (so no native rounded corners/shadow). Switching macOS to
// the native titleBarStyle:"Overlay" path is a documented follow-up.

import { useEffect, useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { useTheme } from "../../theme";
import { cn } from "./cn";

const IS_MAC = typeof navigator !== "undefined" && /Mac|iPhone|iPad|iPod/.test(navigator.userAgent);

/** The running build, e.g. "3.88.0-alpha". Read once — it cannot change while the app is open.
 *  Rendered beside the stage word so "which version am I on?" is answerable without opening
 *  Settings, which is the whole reason the sidebar's duplicate badge could go. */
function useAppVersion(): string | null {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => {});
  }, []);
  return version;
}

/** The version with any `-alpha`/`-beta` suffix removed: the stage is already spelled out beside
 *  it, and "3.88.0-alpha alpha" reads as a mistake. */
function baseVersion(v: string): string {
  const dash = v.indexOf("-");
  return dash === -1 ? v : v.slice(0, dash);
}

function SystemLabel() {
  const { system } = useTheme();
  const version = useAppVersion();
  const num = version ? baseVersion(version) : null;
  if (system === "terminal") {
    return (
      <span className="pointer-events-none font-mono text-xs text-ink3">
        <span style={{ color: "var(--accent)" }}>●</span> pm{" "}
        <span className="text-ink4">[alpha{num ? ` ${num}` : ""}]</span>
      </span>
    );
  }
  return (
    <span className="pointer-events-none">
      <span className="font-head text-sm text-ink2">PM</span>{" "}
      <span className="font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
        alpha{num ? ` ${num}` : ""}
      </span>
    </span>
  );
}

function MacDot({ label, onClick }: { label: string; onClick: () => void }) {
  // The visible dot stays 12px, but the button is a 24px hit target (WCAG 2.5.8) with the dot
  // centred inside. 24px targets can't sit at the old ~20px centres without overlapping, so the
  // three dots read a touch more spread than before — a macOS-only change flagged for OS testing.
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="group flex h-6 w-6 items-center justify-center"
    >
      <span className="h-3 w-3 rounded-full bg-[var(--border2)] transition-colors group-hover:bg-[var(--ink4)]" />
    </button>
  );
}

function CaptionButton({
  label,
  onClick,
  danger,
  children,
}: {
  label: string;
  onClick: () => void;
  danger?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={cn(
        "flex w-11 items-center justify-center text-sm text-ink3 transition-colors hover:text-ink",
        danger
          ? "hover:bg-[color-mix(in_oklab,var(--st-due)_30%,transparent)] hover:text-st-due"
          : "hover:bg-surface",
      )}
    >
      {children}
    </button>
  );
}

/** Corner brackets: pointing outward = enter fullscreen; inward = exit. */
function FullscreenIcon({ active }: { active: boolean }) {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 12 12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.2"
      aria-hidden
    >
      {active ? (
        <path d="M4 1v3H1 M8 1v3h3 M4 11V8H1 M8 11V8h3" />
      ) : (
        <path d="M1 4V1h3 M11 4V1H8 M1 8v3h3 M11 8v3H8" />
      )}
    </svg>
  );
}

export function TitleBar() {
  const { system } = useTheme();
  const [maximized, setMaximized] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;
    // There is no fullscreen-changed event, and toggling fullscreen also fires a
    // resize, so re-read both window states from the one onResized handler.
    const sync = () => {
      win
        .isMaximized()
        .then(setMaximized)
        .catch(() => {});
      win
        .isFullscreen()
        .then(setFullscreen)
        .catch(() => {});
    };
    sync();
    win
      .onResized(sync)
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});

    // F11 toggles fullscreen; Esc leaves it — a frameless fullscreen must never trap
    // the user. Read the live state (not the closed-over value) before toggling.
    const onKey = (e: KeyboardEvent) => {
      if (e.repeat) return; // holding a key shouldn't interleave async fullscreen toggles
      // Let an open dialog own Escape — Modal has its own window listener, so don't also
      // exit fullscreen on the same keystroke (or yank the user out from under a busy
      // ConfirmDialog they can't dismiss).
      if (e.key === "Escape" && document.querySelector('[role="dialog"]')) return;
      if (e.key === "F11") {
        e.preventDefault();
        getCurrentWindow()
          .isFullscreen()
          .then((fs) => getCurrentWindow().setFullscreen(!fs))
          .catch(() => {});
      } else if (e.key === "Escape") {
        getCurrentWindow()
          .isFullscreen()
          .then((fs) => {
            if (fs) void getCurrentWindow().setFullscreen(false);
          })
          .catch(() => {});
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      unlisten?.();
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  const minimize = () =>
    void getCurrentWindow()
      .minimize()
      .catch(() => {});
  const toggleMaximize = () =>
    void getCurrentWindow()
      .toggleMaximize()
      .catch(() => {});
  const toggleFullscreen = () =>
    void getCurrentWindow()
      .isFullscreen()
      .then((fs) => getCurrentWindow().setFullscreen(!fs))
      .catch(() => {});
  const close = () =>
    void getCurrentWindow()
      .close()
      .catch(() => {});

  return (
    <div
      data-tauri-drag-region
      className="flex h-9 shrink-0 select-none items-stretch justify-between border-b border-border bg-panel"
    >
      <div data-tauri-drag-region className="flex items-center gap-2 px-3">
        {IS_MAC && (
          <div className="flex items-center">
            {/* macOS order: close, minimize, zoom (left→right) */}
            <MacDot label="Close" onClick={close} />
            <MacDot label="Minimize" onClick={minimize} />
            <MacDot label={maximized ? "Restore" : "Maximize"} onClick={toggleMaximize} />
          </div>
        )}
        <SystemLabel />
      </div>

      <div data-tauri-drag-region className="flex-1" />

      <div className={cn("flex items-stretch", system === "terminal" && "font-mono")}>
        {/* Fullscreen affordance on every OS (Mac's right side is otherwise empty);
            also bound to F11 / Esc. */}
        <CaptionButton
          label={fullscreen ? "Exit full screen" : "Full screen"}
          onClick={toggleFullscreen}
        >
          <FullscreenIcon active={fullscreen} />
        </CaptionButton>
        {!IS_MAC && (
          <>
            {/* Icons are CSS shapes (not glyphs) so minimize/maximize match the close weight
                and size consistently across fonts. */}
            <CaptionButton label="Minimize" onClick={minimize}>
              <span className="block h-px w-3 bg-current" />
            </CaptionButton>
            <CaptionButton label={maximized ? "Restore" : "Maximize"} onClick={toggleMaximize}>
              {maximized ? (
                <span className="relative block h-3 w-3">
                  <span className="absolute bottom-0 left-0 block h-2 w-2 border border-current" />
                  <span className="absolute right-0 top-0 block h-2 w-2 border border-current" />
                </span>
              ) : (
                <span className="block h-3 w-3 border border-current" />
              )}
            </CaptionButton>
            <CaptionButton label="Close" onClick={close} danger>
              <span className="text-base leading-none">✕</span>
            </CaptionButton>
          </>
        )}
      </div>
    </div>
  );
}
