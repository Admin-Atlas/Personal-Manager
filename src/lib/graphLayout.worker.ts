// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "by project" force layout, run OFF the main thread. d3-force needs no DOM, so the simulation —
// the one genuinely heavy step when a vault has thousands of nodes (a full Drive sync) — runs here
// while the UI thread stays free to paint and respond. The main thread builds the node/link graph,
// posts it in, and gets back final positions to render on the canvas (see src/lib/mapLayout.ts).

import { simulate, type LayoutRequest, type LayoutResponse } from "./forceLayout";

// Typed locally so the file compiles under the DOM lib without pulling in the WebWorker lib (which
// would clash on `self`). Only the two members we use are declared.
interface WorkerScope {
  onmessage: ((e: MessageEvent<LayoutRequest>) => void) | null;
  postMessage(message: LayoutResponse): void;
}
const ctx = self as unknown as WorkerScope;

ctx.onmessage = (e: MessageEvent<LayoutRequest>) => {
  ctx.postMessage(simulate(e.data));
};
