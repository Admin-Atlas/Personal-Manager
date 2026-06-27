// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "by project" map layout, computed off the main thread and cached so the Map tab opens instantly.
//
// Two seams the app relies on:
//   - warmMapLayout()  — kicked off once after vault unlock (App.tsx) at idle priority, so the heavy
//     force simulation is already done (or running on the worker) by the time the user opens the Map.
//     The worker runs on its own thread, so it never stutters the UI even mid-launch.
//   - getProjectLayout(documents) — what GraphView calls. A cache hit on the same document set returns
//     the warmed layout immediately; otherwise it computes now (prioritised — no idle wait).
//
// The expensive force run lives in graphLayout.worker.ts (falling back to a synchronous run here if a
// worker can't be created). This module owns only the cheap parts: building the node/link graph from
// documents, and turning the returned positions into edges + a fit box for the canvas.

import { listDocuments } from "./ipc";
import { simulate, type LayoutRequest, type LayoutResponse } from "./forceLayout";
import type { Document, SemanticCoord } from "./types";

export const MAP_WIDTH = 960;
export const MAP_HEIGHT = 640;
const PAD = 50;
/** The semantic coords arrive in [0,1]²; spread them over a square this wide before the fit transform. */
const SEMANTIC_SPAN = 900;

/** A node with its final position, ready to colour + draw. Theme-independent (no colour here). */
export interface PositionedNode {
  id: string;
  kind: "project" | "doc";
  label: string;
  project: string;
  radius: number;
  x: number;
  y: number;
  doc?: Document;
  /** Semantic mode: this document has no real coordinate yet (new since the last compute, or beyond
   *  the node cap), so it's parked at its project's centroid and drawn faded until the recompute lands. */
  transient?: boolean;
}

export interface Edge {
  sx: number;
  sy: number;
  tx: number;
  ty: number;
}

/** Numeric fit box (canvas transform reads this); `viewBox` is the same as an SVG-style string. */
export interface Bounds {
  minX: number;
  minY: number;
  width: number;
  height: number;
}

/** Positions only — the force-sim output, independent of theme colour. */
export interface BaseLayout {
  nodes: PositionedNode[];
  edges: Edge[];
  bounds: Bounds;
  viewBox: string;
  projectNames: string[];
}

// ---- worker plumbing ------------------------------------------------------

let worker: Worker | null = null;
let workerBroken = false;

function getWorker(): Worker | null {
  if (workerBroken) return null;
  if (worker) return worker;
  try {
    worker = new Worker(new URL("./graphLayout.worker.ts", import.meta.url), { type: "module" });
    return worker;
  } catch {
    workerBroken = true;
    return null;
  }
}

// Requests are serialised on a single promise chain so one worker reply can never be mistaken for
// another's (we only ever need one map layout at a time).
let chain: Promise<unknown> = Promise.resolve();

function runOnce(req: LayoutRequest): Promise<LayoutResponse> {
  const w = getWorker();
  if (!w) return Promise.resolve(simulate(req)); // synchronous fallback
  return new Promise<LayoutResponse>((resolve) => {
    const onMessage = (e: MessageEvent<LayoutResponse>) => {
      cleanup();
      resolve(e.data);
    };
    const onError = () => {
      cleanup();
      workerBroken = true; // give up on the worker and fall back for this and future runs
      worker = null;
      resolve(simulate(req));
    };
    const cleanup = () => {
      w.removeEventListener("message", onMessage);
      w.removeEventListener("error", onError);
    };
    w.addEventListener("message", onMessage);
    w.addEventListener("error", onError);
    w.postMessage(req);
  });
}

function simulateLayout(req: LayoutRequest): Promise<LayoutResponse> {
  const result = chain.then(() => runOnce(req));
  chain = result.catch(() => undefined);
  return result;
}

// ---- graph construction ---------------------------------------------------

function docRadius(chunkCount: number): number {
  return Math.min(16, 4.5 + Math.sqrt(Math.max(1, chunkCount)) * 1.6);
}

/** Stable key over the inputs that affect the layout (ids, project, content volume). */
function docSetHash(documents: Document[]): string {
  // Sorted so order from the backend never changes the key; cheap to build and compare.
  const parts = documents.map((d) => `${d.id}:${d.project || "Unsorted"}:${d.chunk_count}`).sort();
  return `${parts.length}|${parts.join(",")}`;
}

async function compute(documents: Document[]): Promise<BaseLayout> {
  const projectNames = Array.from(new Set(documents.map((d) => d.project || "Unsorted")));

  const meta = new Map<string, Omit<PositionedNode, "x" | "y">>();
  for (const name of projectNames) {
    meta.set(`project:${name}`, {
      id: `project:${name}`,
      kind: "project",
      label: name,
      project: name,
      radius: 14,
    });
  }
  for (const d of documents) {
    const project = d.project || "Unsorted";
    meta.set(`doc:${d.id}`, {
      id: `doc:${d.id}`,
      kind: "doc",
      label: d.title,
      project,
      radius: docRadius(d.chunk_count),
      doc: d,
    });
  }

  const simNodes = Array.from(meta.values()).map((n) => ({ id: n.id, radius: n.radius }));
  const links = documents.map((d) => ({
    source: `doc:${d.id}`,
    target: `project:${d.project || "Unsorted"}`,
  }));

  const { positions } = await simulateLayout({
    nodes: simNodes,
    links,
    width: MAP_WIDTH,
    height: MAP_HEIGHT,
  });

  const pos = new Map(positions.map((p) => [p.id, p]));
  const nodes: PositionedNode[] = Array.from(meta.values()).map((n) => {
    const p = pos.get(n.id);
    return { ...n, x: p?.x ?? 0, y: p?.y ?? 0 };
  });

  const at = (id: string) => pos.get(id) ?? { x: 0, y: 0 };
  const edges: Edge[] = links.map((l) => {
    const s = at(l.source);
    const t = at(l.target);
    return { sx: s.x, sy: s.y, tx: t.x, ty: t.y };
  });

  // Fit everything in view (accounting for node radius + labels) — the same math the old SVG viewBox
  // used, kept in numeric form so the canvas transform can read it directly.
  const xs = nodes.map((n) => n.x);
  const ys = nodes.map((n) => n.y);
  const minX = Math.min(...xs) - PAD;
  const minY = Math.min(...ys) - PAD;
  const width = Math.max(...xs) - minX + PAD;
  const height = Math.max(...ys) - minY + PAD;
  const bounds: Bounds = { minX, minY, width, height };

  return { nodes, edges, bounds, viewBox: `${minX} ${minY} ${width} ${height}`, projectNames };
}

// ---- cache + public API ---------------------------------------------------

interface CacheEntry {
  hash: string;
  promise: Promise<BaseLayout>;
}
let cached: CacheEntry | null = null;

/** Layout for this document set — the warmed result if the set is unchanged, otherwise computed now. */
export function getProjectLayout(documents: Document[]): Promise<BaseLayout> | null {
  if (documents.length === 0) return null;
  const hash = docSetHash(documents);
  if (cached?.hash === hash) return cached.promise;
  const promise = compute(documents);
  cached = { hash, promise };
  return promise;
}

// ---- semantic layout (coords from the backend) ----------------------------

/**
 * Build a "by meaning" layout from the backend's 2-D coordinates: each document sits where its
 * embedding projects, so nearby = similar. No project hubs or edges (proximity itself is the signal),
 * but documents are still coloured by project for the legend. A document with no coordinate yet (added
 * since the last compute, or beyond the node cap) is parked at its project's centroid and marked
 * transient, so it still appears — grouped — and snaps into place when the next recompute lands.
 */
export function buildSemanticLayout(
  documents: Document[],
  coords: SemanticCoord[],
): BaseLayout | null {
  if (documents.length === 0) return null;

  const projectNames = Array.from(new Set(documents.map((d) => d.project || "Unsorted")));
  const coordById = new Map(coords.map((c) => [c.id, c]));

  // Project centroids (from the documents that do have coords) + an overall fallback.
  const sums = new Map<string, { x: number; y: number; n: number }>();
  let ox = 0;
  let oy = 0;
  for (const d of documents) {
    const c = coordById.get(d.id);
    if (!c) continue;
    const proj = d.project || "Unsorted";
    const s = sums.get(proj) ?? { x: 0, y: 0, n: 0 };
    s.x += c.x;
    s.y += c.y;
    s.n += 1;
    sums.set(proj, s);
    ox += c.x;
    oy += c.y;
  }
  const overall =
    coords.length > 0 ? { x: ox / coords.length, y: oy / coords.length } : { x: 0.5, y: 0.5 };
  const centroidFor = (proj: string) => {
    const s = sums.get(proj);
    return s && s.n > 0 ? { x: s.x / s.n, y: s.y / s.n } : overall;
  };

  const nodes: PositionedNode[] = documents.map((d) => {
    const proj = d.project || "Unsorted";
    const c = coordById.get(d.id);
    const p = c ?? centroidFor(proj);
    return {
      id: `doc:${d.id}`,
      kind: "doc",
      label: d.title,
      project: proj,
      radius: docRadius(d.chunk_count),
      x: p.x * SEMANTIC_SPAN,
      y: p.y * SEMANTIC_SPAN,
      doc: d,
      transient: !c,
    };
  });

  const xs = nodes.map((n) => n.x);
  const ys = nodes.map((n) => n.y);
  const minX = Math.min(...xs) - PAD;
  const minY = Math.min(...ys) - PAD;
  const width = Math.max(...xs) - minX + PAD;
  const height = Math.max(...ys) - minY + PAD;
  const bounds: Bounds = { minX, minY, width, height };

  return { nodes, edges: [], bounds, viewBox: `${minX} ${minY} ${width} ${height}`, projectNames };
}

let warming = false;

/**
 * Pre-compute the map layout in the background after unlock, at idle priority. Fire-and-forget and
 * idempotent: it fetches the document list itself and primes the cache, so opening the Map later is
 * instant. The simulation is on the worker thread, so this never blocks the UI; the idle wait just
 * keeps it from competing for the main thread during a busy launch.
 */
export function warmMapLayout(): void {
  if (warming) return;
  warming = true;
  const start = () => {
    void listDocuments()
      .then((docs) => getProjectLayout(docs))
      .catch(() => undefined)
      .finally(() => {
        warming = false;
      });
  };
  if (typeof requestIdleCallback === "function") {
    requestIdleCallback(start, { timeout: 4000 });
  } else {
    setTimeout(start, 1200);
  }
}
