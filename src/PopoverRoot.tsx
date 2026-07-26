// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The whole UI of the always-on-top briefing window (the `briefing` webview, opened from the tray
// or from Settings → "Always on top").
//
// Deliberately tiny. It is NOT `<App/>`: booting the real app in a second webview would repeat the
// whole startup batch — vault + lock status, the shared-vault list, the 15-minute calendar poll, the
// resume effects, the activity throttle — in a window showing one card. `main.tsx` branches on the
// `?window=briefing` query and renders this instead, with `App` behind a lazy import so this root
// never even downloads the main bundle.
//
// THE INVARIANT: everything here calls PM's own `#[tauri::command]`s and nothing else. Those are not
// ACL-gated (PM ships no app ACL manifest), which is why this window needs no capability entry for
// its data. `plugin:`-prefixed calls — `getCurrentWindow().hide()`, `listen()`, `emit()`, dialog,
// process — ARE gated and would fail at runtime with nothing in `just check` catching it. The Rust
// side (tray.rs) owns showing, hiding and closing this window; the one plugin permission it holds is
// window dragging, granted narrowly in capabilities/briefing.json.
//
// Both providers below were checked against that rule: ThemeProvider uses only getPref/setPref and
// UserTimeProvider only getSettings — app commands, both.

import { BriefingProvider } from "./lib/briefing";
import { Briefing } from "./components/Briefing";
import { closeBriefingWindow, showMainWindow } from "./lib/ipc";
import { ThemeProvider, UserTimeProvider } from "./theme";

export function PopoverRoot() {
  return (
    <ThemeProvider>
      <UserTimeProvider>
        {/* autoRefresh off: the main window's provider owns the launch check. Since #540 the
            backend is single-flighted, so a second check here would fold rather than race — but a
            display-only window still shouldn't be the one deciding when the model runs. It shows
            what is stored, follows `briefing://updated` for regenerations it didn't start, and
            calls the model only when the user clicks its own Refresh. */}
        <BriefingProvider autoRefresh={false}>
          <div className="flex h-full flex-col bg-panel text-ink">
            {/* The window is frameless, so this strip is what drags it. `data-tauri-drag-region` is
                the one plugin-backed thing this root uses; capabilities/briefing.json grants it. The
                title matches the in-app floating panel's word-for-word — same content, same name,
                wherever it's shown. */}
            <div
              data-tauri-drag-region
              className="flex shrink-0 cursor-grab items-center justify-between gap-2 border-b border-border px-3 py-1.5 active:cursor-grabbing"
            >
              <span
                data-tauri-drag-region
                className="font-mono text-[0.6875rem] uppercase tracking-wide text-faint"
              >
                Briefing — Today
              </span>
              {/* Both buttons live INSIDE the drag strip and deliberately carry no
                  `data-tauri-drag-region` — the ✕ already proves a real <button> in here still gets
                  its clicks. "Open PM" leaves this window where it is: it is a persistent
                  always-on-top panel, so dismissing it on every use would fight what it is for. */}
              <div className="flex items-center gap-1">
                <button
                  type="button"
                  onClick={() => void showMainWindow().catch(() => {})}
                  title="Bring the Personal Manager window to the front"
                  aria-label="Open Personal Manager"
                  className="rounded-[var(--radius-sm)] px-1.5 text-xs text-ink4 hover:bg-surface hover:text-ink"
                >
                  Open PM
                </button>
                {/* Closing hides the window AND switches the setting off, so it doesn't come back on
                    the next launch claiming to be on top. Rust owns both halves: this root can't hide
                    its own window (no capability) and the main window can't see this click. */}
                <button
                  type="button"
                  onClick={() => void closeBriefingWindow().catch(() => {})}
                  title="Close the briefing window"
                  aria-label="Close the briefing window"
                  className="rounded-[var(--radius-sm)] px-1.5 text-xs text-ink4 hover:bg-surface hover:text-ink"
                >
                  <span aria-hidden="true">✕</span>
                </button>
              </div>
            </div>
            {/* overflow-hidden, not -y-auto: with `fill` the Briefing owns its own scroller, and
                nesting two would give this frameless window a second scrollbar. */}
            <div className="min-h-0 flex-1 overflow-hidden px-3 py-2">
              <Briefing variant="panel" fill />
            </div>
          </div>
        </BriefingProvider>
      </UserTimeProvider>
    </ThemeProvider>
  );
}
