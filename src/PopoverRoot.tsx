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
import { ThemeProvider, UserTimeProvider } from "./theme";

export function PopoverRoot() {
  return (
    <ThemeProvider>
      <UserTimeProvider>
        {/* autoRefresh off: the main window's provider already owns the once-a-day stale check, and
            `refresh_daily_briefing` has no backend single-flight — two providers both deciding the
            briefing was stale would fire two model calls and race on the stored timestamp. This
            window shows what is stored and refreshes only when the user asks. */}
        <BriefingProvider autoRefresh={false}>
          <div className="flex h-full flex-col bg-panel text-ink">
            {/* The window is frameless, so this strip is what drags it. `data-tauri-drag-region` is
                the one plugin-backed thing this root uses; capabilities/briefing.json grants it. */}
            <div
              data-tauri-drag-region
              className="flex shrink-0 cursor-grab items-center justify-between gap-2 border-b border-border px-3 py-1.5 active:cursor-grabbing"
            >
              <span
                data-tauri-drag-region
                className="font-mono text-[11px] uppercase tracking-wide text-faint"
              >
                Today
              </span>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto px-3 py-2">
              <Briefing variant="panel" />
            </div>
          </div>
        </BriefingProvider>
      </UserTimeProvider>
    </ThemeProvider>
  );
}
