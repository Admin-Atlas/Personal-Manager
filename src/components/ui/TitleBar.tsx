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
import { useTheme } from "../../theme";
import { cn } from "./cn";

const IS_MAC =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad|iPod/.test(navigator.userAgent);

function SystemLabel() {
  const { system } = useTheme();
  if (system === "terminal") {
    return (
      <span className="pointer-events-none font-mono text-xs text-ink3">
        <span style={{ color: "var(--accent)" }}>●</span> pm <span className="text-ink4">[alpha]</span>
      </span>
    );
  }
  return (
    <span className="pointer-events-none">
      <span className="font-head text-sm text-ink2">PM</span>{" "}
      <span className="font-mono text-[10px] uppercase tracking-wide text-ink4">alpha</span>
    </span>
  );
}

function MacDot({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="h-3 w-3 rounded-full bg-[var(--border2)] transition-colors hover:bg-[var(--ink4)]"
    />
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

export function TitleBar() {
  const { system } = useTheme();
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;
    win.isMaximized().then(setMaximized).catch(() => {});
    win
      .onResized(() => {
        win.isMaximized().then(setMaximized).catch(() => {});
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => unlisten?.();
  }, []);

  const minimize = () => void getCurrentWindow().minimize().catch(() => {});
  const toggleMaximize = () => void getCurrentWindow().toggleMaximize().catch(() => {});
  const close = () => void getCurrentWindow().close().catch(() => {});

  return (
    <div
      data-tauri-drag-region
      className="flex h-9 shrink-0 select-none items-stretch justify-between border-b border-border bg-panel"
    >
      <div data-tauri-drag-region className="flex items-center gap-2 px-3">
        {IS_MAC && (
          <div className="flex items-center gap-2">
            {/* macOS order: close, minimize, zoom (left→right) */}
            <MacDot label="Close" onClick={close} />
            <MacDot label="Minimize" onClick={minimize} />
            <MacDot label={maximized ? "Restore" : "Maximize"} onClick={toggleMaximize} />
          </div>
        )}
        <SystemLabel />
      </div>

      <div data-tauri-drag-region className="flex-1" />

      {!IS_MAC && (
        <div className={cn("flex items-stretch", system === "terminal" && "font-mono")}>
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
        </div>
      )}
    </div>
  );
}
