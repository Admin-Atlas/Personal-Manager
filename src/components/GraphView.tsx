// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { quadtree, type Quadtree } from "d3-quadtree";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  installOptionalTsne,
  listDocuments,
  onLayoutProgress,
  onTsneInstall,
  optionalTsneStatus,
  prioritiseSemanticLayout,
  semanticLayout,
} from "../lib/ipc";
import { formatDate } from "../lib/format";
import {
  buildSemanticLayout,
  getProjectLayout,
  type BaseLayout,
  type Bounds,
  type PositionedNode,
} from "../lib/mapLayout";
import { IngestProgress } from "./IngestProgress";
import { Skeleton } from "./ui";
import type { Document, SemanticLayout } from "../lib/types";
import { graphColor, useTheme, useDepth } from "../theme";

/**
 * A map of the store, in one of two arrangements the user switches between:
 *   - **By project** — every project is a hub, each document a node linked to it (the d3-force layout).
 *   - **Semantic proximity** — documents laid out by meaning (nearby = similar), from a UMAP/t-SNE-style
 *     projection of their embeddings computed in the background (see `src/lib/layout.rs`); no edges.
 *
 * Rendering is on a single `<canvas>` (one draw call per frame, d3-quadtree hit-testing) so thousands
 * of nodes — index-only Drive files inflate the count — stay smooth. The view is navigable: scroll to
 * zoom, drag to pan, double-click (or the Fit button) to reset. Hover a node for its details; click one
 * (without dragging) to open its project's focused view. Colour is applied in the draw call, so a theme
 * toggle is a redraw, not a relayout.
 */

type LayoutMode = "project" | "semantic";
const MODE_KEY = "pm.map.layoutMode";
const COHESION_KEY = "pm.map.cohesion";
const LABELS_KEY = "pm.map.labels";

function initialMode(): LayoutMode {
  return localStorage.getItem(MODE_KEY) === "semantic" ? "semantic" : "project";
}

/** Project-cohesion weight (0 = pure meaning, the default; ≤0.5). Per-device, like the mode pref. */
function initialCohesion(): number {
  const raw = Number(localStorage.getItem(COHESION_KEY));
  return Number.isFinite(raw) ? Math.max(0, Math.min(0.5, raw)) : 0;
}

/** Show each document's file name on its node? Off by default (keeps a dense map clean); per-device. */
function initialShowLabels(): boolean {
  return localStorage.getItem(LABELS_KEY) === "1";
}

/** The widest prefix of `text` (with a trailing … if shortened) that fits `maxWidth` px in the ctx's
 *  current font — "" if not even one character fits. A proportional first guess keeps the trim short. */
function fitLabel(ctx: CanvasRenderingContext2D, text: string, maxWidth: number): string {
  if (maxWidth <= 0 || !text) return "";
  const full = ctx.measureText(text).width;
  if (full <= maxWidth) return text;
  let keep = Math.min(
    text.length - 1,
    Math.max(0, Math.floor((text.length * maxWidth) / full) - 1),
  );
  while (keep > 0 && ctx.measureText(text.slice(0, keep) + "…").width > maxWidth) keep--;
  return keep > 0 ? text.slice(0, keep) + "…" : "";
}

/** Themed values the canvas needs as concrete strings (canvas can't read CSS `var(--…)`). */
interface ThemeColors {
  border: string;
  bg: string;
  ink: string;
  ink2: string;
  stLook: string;
  uiFont: string;
}

// Neutral fallback when a theme custom property reads empty (e.g. mid theme-swap) — a legible mid-grey
// on either canvas (I-02, was an inline hex). The graph draws on a <canvas>, which can't resolve
// var(--…), so `readThemeColors` snapshots the tokens and this guards a momentarily-missing one.
const THEME_FALLBACK = "#888";

function readThemeColors(): ThemeColors {
  const s = getComputedStyle(document.documentElement);
  const v = (name: string) => s.getPropertyValue(name).trim() || THEME_FALLBACK;
  return {
    border: v("--border"),
    bg: v("--bg"),
    ink: v("--ink"),
    ink2: v("--ink2"),
    stLook: v("--st-look"),
    uiFont: s.getPropertyValue("--ui").trim() || "system-ui, sans-serif",
  };
}

interface Transform {
  scale: number;
  offsetX: number;
  offsetY: number;
}

/** Fit the world `bounds` into a `w`×`h` CSS-pixel canvas, centred (xMidYMid meet). */
function fitTransform(bounds: Bounds, w: number, h: number): Transform {
  const scale = Math.min(w / bounds.width, h / bounds.height);
  return {
    scale,
    offsetX: (w - bounds.width * scale) / 2 - bounds.minX * scale,
    offsetY: (h - bounds.height * scale) / 2 - bounds.minY * scale,
  };
}

const isDashed = (doc?: Document) => !!doc && (!doc.reviewed || doc.source_type === "index_only");

export function GraphView({ onOpenProject }: { onOpenProject?: (project: string) => void }) {
  const { mode } = useTheme();
  const [documents, setDocuments] = useState<Document[]>([]);
  const [base, setBase] = useState<BaseLayout | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [hovered, setHovered] = useState<Document | null>(null);
  const [loading, setLoading] = useState(true);

  const [layoutMode, setLayoutMode] = useState<LayoutMode>(initialMode);
  const [cohesion, setCohesion] = useState<number>(initialCohesion);
  const [showLabels, setShowLabels] = useState<boolean>(initialShowLabels);
  const [semantic, setSemantic] = useState<SemanticLayout | null>(null);
  const [computing, setComputing] = useState(false);
  const [tsneInstalled, setTsneInstalled] = useState<boolean | null>(null);
  const [installingTsne, setInstallingTsne] = useState(false);
  const [installFrac, setInstallFrac] = useState(0);
  const [grabbing, setGrabbing] = useState(false);

  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  // The live world→screen transform (pan/zoom mutate it in place; draw + hit-testing read it).
  const viewRef = useRef<Transform | null>(null);
  // The most recent fit transform, for clamping zoom and resetting the view.
  const fitRef = useRef<Transform | null>(null);
  // Recompute the fit on the next draw (a fresh layout, or a reset).
  const needFitRef = useRef(true);
  // The user has panned/zoomed, so a resize keeps their view instead of refitting.
  const interactedRef = useRef(false);
  const dragRef = useRef<{
    startX: number;
    startY: number;
    lastX: number;
    lastY: number;
    moved: boolean;
  } | null>(null);

  useEffect(() => {
    listDocuments()
      // Archived documents are deliberately shelved — keep them off the Map (both arrangements),
      // while they stay fully searchable and listed elsewhere. Untriaged docs still appear.
      .then((docs) => setDocuments(docs.filter((d) => d.importance !== "archive")))
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    optionalTsneStatus()
      .then((s) => setTsneInstalled(s.installed))
      .catch(() => setTsneInstalled(false));
  }, []);

  // Persist the chosen arrangement + cohesion (per-device, like the theme/depth prefs).
  useEffect(() => {
    localStorage.setItem(MODE_KEY, layoutMode);
  }, [layoutMode]);
  useEffect(() => {
    localStorage.setItem(COHESION_KEY, String(cohesion));
  }, [cohesion]);
  useEffect(() => {
    localStorage.setItem(LABELS_KEY, showLabels ? "1" : "0");
  }, [showLabels]);

  // Follow the optional-t-SNE download's progress (it can be triggered here or from Settings).
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    void onTsneInstall((e) => {
      if (!cancelled) setInstallFrac(e.fraction);
    }).then((u) => {
      unlisten = u;
      if (cancelled) u();
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Semantic mode: fetch the cached coords, ask the backend to prioritise a recompute if stale, and
  // follow the global progress event so the map refreshes when a new layout lands.
  useEffect(() => {
    if (layoutMode !== "semantic") return;
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    const load = () =>
      semanticLayout()
        .then((d) => {
          if (!cancelled) {
            setSemantic(d);
            setComputing(d.computing);
          }
        })
        .catch(() => {});
    void load();
    void prioritiseSemanticLayout().catch(() => {});
    void onLayoutProgress((e) => {
      if (cancelled) return;
      if (e.state === "done") {
        setComputing(false);
        void load();
      } else if (e.state === "preparing" || e.state === "reducing") {
        setComputing(true);
      } else {
        setComputing(false); // deferred / error
      }
    }).then((u) => {
      unlisten = u;
      if (cancelled) u();
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [layoutMode]);

  // The render layout. Project mode runs the (worker-backed, cached) force layout; semantic mode joins
  // the backend coords to the documents (optionally blended toward project centroids by `cohesion`).
  // A stale async result can't overwrite a newer one.
  useEffect(() => {
    let cancelled = false;
    if (layoutMode === "project") {
      const job = getProjectLayout(documents);
      if (!job) {
        setBase(null);
        return;
      }
      job.then((b) => !cancelled && setBase(b)).catch(() => !cancelled && setBase(null));
    } else {
      // Don't render a pile of centroids before any coords exist — show the preparing state instead.
      const coords = semantic?.coords ?? [];
      setBase(coords.length > 0 ? buildSemanticLayout(documents, coords, cohesion) : null);
    }
    return () => {
      cancelled = true;
    };
  }, [documents, layoutMode, semantic, cohesion]);

  // A new layout (or mode/cohesion change) refits the view; the user's pan/zoom only applies within a
  // given layout.
  useEffect(() => {
    needFitRef.current = true;
    interactedRef.current = false;
  }, [base]);

  const projectIndex = useMemo(() => {
    const m = new Map<string, number>();
    base?.projectNames.forEach((name, i) => m.set(name, i));
    return m;
  }, [base]);
  const colorFor = useCallback(
    (project: string) => graphColor(projectIndex.get(project) ?? 0, mode),
    [projectIndex, mode],
  );
  const legend = useMemo(
    () => base?.projectNames.map((name) => ({ name, color: colorFor(name) })) ?? [],
    [base, colorFor],
  );

  const docQuadtree = useMemo<Quadtree<PositionedNode> | null>(() => {
    if (!base) return null;
    const docs = base.nodes.filter((n) => n.kind === "doc");
    return quadtree<PositionedNode>()
      .x((n) => n.x)
      .y((n) => n.y)
      .addAll(docs);
  }, [base]);

  // ---- drawing ----
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const wrap = wrapRef.current;
    if (!canvas || !wrap || !base) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const cssW = wrap.clientWidth;
    const cssH = wrap.clientHeight;
    if (cssW === 0 || cssH === 0) return;

    if (canvas.width !== Math.round(cssW * dpr) || canvas.height !== Math.round(cssH * dpr)) {
      canvas.width = Math.round(cssW * dpr);
      canvas.height = Math.round(cssH * dpr);
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, cssH);

    // Fit on a fresh layout / reset; otherwise keep the user's pan & zoom.
    if (needFitRef.current || !viewRef.current) {
      const fit = fitTransform(base.bounds, cssW, cssH);
      fitRef.current = fit;
      viewRef.current = fit;
      needFitRef.current = false;
    }
    const t = viewRef.current;
    const sx = (x: number) => x * t.scale + t.offsetX;
    const sy = (y: number) => y * t.scale + t.offsetY;
    const colors = readThemeColors();

    // Edges (project mode only; one path, single stroke). Node radii scale with the fit (size =
    // content volume, a real signal); stroke/edge widths stay fixed in screen pixels as crisp chrome.
    if (base.edges.length > 0) {
      ctx.strokeStyle = colors.border;
      ctx.lineWidth = 0.75;
      ctx.beginPath();
      for (const e of base.edges) {
        ctx.moveTo(sx(e.sx), sy(e.sy));
        ctx.lineTo(sx(e.tx), sy(e.ty));
      }
      ctx.stroke();
      ctx.setLineDash([]);
    }

    for (const n of base.nodes) {
      const color = colorFor(n.project);
      const r = n.radius * t.scale;
      if (n.kind === "project") {
        ctx.globalAlpha = 0.25;
        ctx.fillStyle = color;
        ctx.beginPath();
        ctx.arc(sx(n.x), sy(n.y), r, 0, Math.PI * 2);
        ctx.fill();
        ctx.globalAlpha = 1;
        ctx.strokeStyle = color;
        ctx.lineWidth = 1.5;
        ctx.stroke();
        ctx.fillStyle = colors.ink2;
        ctx.font = `600 ${11 * t.scale}px ${colors.uiFont}`;
        ctx.textAlign = "center";
        ctx.textBaseline = "alphabetic";
        ctx.fillText(n.label, sx(n.x), sy(n.y) - r - 4);
      } else {
        const dashed = isDashed(n.doc);
        // Transient (not-yet-projected) semantic nodes sit at a project centroid; draw them faded.
        ctx.globalAlpha = n.transient ? 0.32 : 0.85;
        ctx.fillStyle = color;
        ctx.beginPath();
        ctx.arc(sx(n.x), sy(n.y), r, 0, Math.PI * 2);
        ctx.fill();
        ctx.globalAlpha = 1;
        ctx.strokeStyle = dashed ? colors.stLook : colors.bg;
        ctx.lineWidth = dashed ? 1.5 : 1;
        ctx.setLineDash(dashed ? [2, 2] : []);
        ctx.stroke();
        ctx.setLineDash([]);
      }
    }

    if (hovered) {
      const n = base.nodes.find((x) => x.doc?.id === hovered.id);
      if (n) {
        ctx.globalAlpha = 1;
        ctx.fillStyle = colorFor(n.project);
        ctx.beginPath();
        ctx.arc(sx(n.x), sy(n.y), n.radius * t.scale, 0, Math.PI * 2);
        ctx.fill();
        ctx.strokeStyle = colors.ink;
        ctx.lineWidth = 2;
        ctx.setLineDash([]);
        ctx.stroke();
      }
    }

    // File-name labels centred inside each document node (opt-in). Sized relative to the node — so a
    // readable ~10–15 characters fit and the text always sits within the circle (it scales with the
    // node as you zoom, never a fixed screen size) — and truncated to the node's width, so a long name
    // shows a prefix; zoom into a bigger node to read more. A contrasting halo keeps it legible on any
    // node colour; nodes that are off-screen or too small to read are skipped, bounding the per-frame
    // cost on a large library.
    if (showLabels) {
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.lineJoin = "round";
      ctx.setLineDash([]);
      for (const n of base.nodes) {
        if (n.kind !== "doc") continue;
        const r = n.radius * t.scale;
        if (r < 12) continue; // too small to read — skip (also bounds how many labels we draw)
        const cx = sx(n.x);
        const cy = sy(n.y);
        if (cx < -r || cx > cssW + r || cy < -r || cy > cssH + r) continue;
        const fontPx = r * 0.25;
        ctx.font = `500 ${fontPx}px ${colors.uiFont}`;
        const text = fitLabel(ctx, n.label, r * 1.8);
        if (!text) continue;
        ctx.lineWidth = Math.max(1.5, fontPx * 0.14);
        ctx.strokeStyle = colors.bg;
        ctx.strokeText(text, cx, cy);
        ctx.fillStyle = colors.ink;
        ctx.fillText(text, cx, cy);
      }
    }
  }, [base, colorFor, hovered, showLabels]);

  useEffect(() => {
    const id = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(id);
  }, [draw]);

  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const ro = new ResizeObserver(() => {
      // Keep the user's framing if they've navigated; otherwise refit to the new size.
      if (!interactedRef.current) needFitRef.current = true;
      requestAnimationFrame(draw);
    });
    ro.observe(wrap);
    return () => ro.disconnect();
  }, [draw]);

  // ---- pointer interaction (hit-test in world coords) ----
  const nodeAt = useCallback(
    (clientX: number, clientY: number): PositionedNode | null => {
      const canvas = canvasRef.current;
      const t = viewRef.current;
      if (!canvas || !t || !docQuadtree) return null;
      const rect = canvas.getBoundingClientRect();
      const wx = (clientX - rect.left - t.offsetX) / t.scale;
      const wy = (clientY - rect.top - t.offsetY) / t.scale;
      // Allow a constant ~6 screen-px slop on top of the node's world radius, so nodes stay easy to
      // hit when zoomed out (where a node covers few pixels).
      const tol = 6 / t.scale;
      const hit = docQuadtree.find(wx, wy, 16 + tol + 2);
      if (!hit) return null;
      const d = Math.hypot(hit.x - wx, hit.y - wy);
      return d <= hit.radius + tol ? hit : null;
    },
    [docQuadtree],
  );

  // Keep the latest callbacks reachable from the once-attached native/window listeners.
  const drawRef = useRef(draw);
  const nodeAtRef = useRef(nodeAt);
  const onOpenRef = useRef(onOpenProject);
  useEffect(() => {
    drawRef.current = draw;
    nodeAtRef.current = nodeAt;
    onOpenRef.current = onOpenProject;
  });

  /** Zoom about a point (screen px), clamped so the whole map is never smaller than half the fit. */
  const zoomAt = useCallback((px: number, py: number, factor: number) => {
    const t = viewRef.current;
    if (!t) return;
    const fitScale = fitRef.current?.scale ?? t.scale;
    const wx = (px - t.offsetX) / t.scale;
    const wy = (py - t.offsetY) / t.scale;
    const scale = Math.min(fitScale * 16, Math.max(fitScale * 0.5, t.scale * factor));
    viewRef.current = { scale, offsetX: px - wx * scale, offsetY: py - wy * scale };
    interactedRef.current = true;
    setHovered(null);
    requestAnimationFrame(() => drawRef.current());
  }, []);

  const zoomButton = useCallback(
    (factor: number) => {
      const wrap = wrapRef.current;
      if (!wrap) return;
      zoomAt(wrap.clientWidth / 2, wrap.clientHeight / 2, factor);
    },
    [zoomAt],
  );

  const resetView = useCallback(() => {
    needFitRef.current = true;
    interactedRef.current = false;
    requestAnimationFrame(() => drawRef.current());
  }, []);

  // Native, non-passive wheel listener (React's onWheel is passive, so it can't preventDefault).
  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const onWheel = (e: WheelEvent) => {
      const canvas = canvasRef.current;
      const t = viewRef.current;
      if (!canvas || !t) return;
      e.preventDefault();
      // A horizontal-dominant wheel (trackpad side-swipe or Shift+wheel) pans left/right; a vertical
      // wheel zooms toward the cursor. Scrolling right reveals content to the right.
      if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        viewRef.current = { ...t, offsetX: t.offsetX - e.deltaX };
        interactedRef.current = true;
        setHovered(null);
        requestAnimationFrame(() => drawRef.current());
        return;
      }
      const rect = canvas.getBoundingClientRect();
      const factor = Math.exp(-e.deltaY * 0.0015);
      zoomAt(e.clientX - rect.left, e.clientY - rect.top, factor);
    };
    wrap.addEventListener("wheel", onWheel, { passive: false });
    return () => wrap.removeEventListener("wheel", onWheel);
  }, [zoomAt]);

  // Pan with a window-level drag (so it keeps tracking outside the canvas), and treat a press that
  // didn't move as a click that opens the project under it.
  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      const drag = dragRef.current;
      const t = viewRef.current;
      if (!drag || !t) return;
      const dx = e.clientX - drag.lastX;
      const dy = e.clientY - drag.lastY;
      drag.lastX = e.clientX;
      drag.lastY = e.clientY;
      if (Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) > 3) drag.moved = true;
      viewRef.current = { ...t, offsetX: t.offsetX + dx, offsetY: t.offsetY + dy };
      interactedRef.current = true;
      setHovered(null);
      requestAnimationFrame(() => drawRef.current());
    };
    const onUp = (e: MouseEvent) => {
      const drag = dragRef.current;
      if (!drag) return;
      dragRef.current = null;
      setGrabbing(false);
      if (!drag.moved) {
        const hit = nodeAtRef.current(e.clientX, e.clientY);
        if (hit?.doc && onOpenRef.current) onOpenRef.current(hit.doc.project);
      }
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, []);

  const onCanvasMouseDown = useCallback((e: React.MouseEvent) => {
    if (e.button !== 0) return;
    dragRef.current = {
      startX: e.clientX,
      startY: e.clientY,
      lastX: e.clientX,
      lastY: e.clientY,
      moved: false,
    };
    setGrabbing(true);
  }, []);

  const onMouseMove = useCallback(
    (e: React.MouseEvent) => {
      if (dragRef.current) return; // panning — no hover
      const hit = nodeAt(e.clientX, e.clientY);
      const doc = hit?.doc ?? null;
      setHovered((cur) => (cur?.id === doc?.id ? cur : doc));
    },
    [nodeAt],
  );
  const onMouseLeave = useCallback(() => setHovered(null), []);

  const installTsne = useCallback(() => {
    setInstallingTsne(true);
    setInstallFrac(0);
    installOptionalTsne()
      .then(() => optionalTsneStatus())
      .then((s) => setTsneInstalled(s.installed))
      .catch((e) => setError(String(e)))
      .finally(() => setInstallingTsne(false));
  }, []);

  const placed = semantic?.coords.length ?? 0;
  const subtitle =
    layoutMode === "semantic"
      ? placed > 0 && placed < documents.length
        ? `showing the top ${placed} of ${documents.length} documents by meaning · scroll to zoom, drag to pan`
        : `${documents.length} document${documents.length === 1 ? "" : "s"} by meaning · scroll to zoom, drag to pan`
      : `${documents.length} document${documents.length === 1 ? "" : "s"} across ${legend.length} project${legend.length === 1 ? "" : "s"} · scroll to zoom, drag to pan, click to open`;

  // The toggle's help differs by whether t-SNE is installed (explain what it is vs that it's running).
  const toggleHelp = tsneInstalled ? "map-layout-toggle-tsne" : "map-layout-toggle";
  const cursor = grabbing ? "grabbing" : hovered ? "pointer" : "grab";

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-4 border-b border-border px-6 py-3">
        <div data-help="nav-graph" className="shrink-0">
          <h1 className="text-sm font-semibold font-head text-ink">Map</h1>
          {loading ? (
            <Skeleton className="mt-1 h-3 w-56" />
          ) : (
            <p className="text-xs text-ink3">{subtitle}</p>
          )}
        </div>
        <div className="flex min-w-0 flex-1 items-center justify-end gap-4">
          {base && (
            <div className="flex min-w-0 flex-wrap justify-end gap-x-3 gap-y-1 overflow-hidden">
              {legend.map((p) => (
                <span key={p.name} className="inline-flex items-center gap-1.5 text-xs text-ink2">
                  <span
                    className="inline-block h-2.5 w-2.5 rounded-full"
                    style={{ background: p.color }}
                  />
                  {p.name}
                </span>
              ))}
            </div>
          )}
          <div className="flex shrink-0 items-center gap-3">
            {layoutMode === "semantic" && (
              <label
                className="flex items-center gap-1.5 text-xs text-ink3"
                data-help="map-cohesion"
              >
                <span>Cohesion</span>
                <select
                  value={String(cohesion)}
                  onChange={(e) => setCohesion(Number(e.target.value))}
                  className="rounded-[var(--radius-sm)] border border-border2 bg-surface px-1.5 py-0.5 text-xs text-ink2 outline-none transition focus:border-accent"
                >
                  <option value="0">Off</option>
                  <option value="0.15">Low</option>
                  <option value="0.3">Medium</option>
                  <option value="0.5">High</option>
                </select>
              </label>
            )}
            <button
              type="button"
              onClick={() => setShowLabels((v) => !v)}
              aria-pressed={showLabels}
              title={showLabels ? "Hide file names" : "Show file names"}
              data-help="map-labels"
              className={`rounded-[var(--radius-sm)] border px-2.5 py-1.5 text-xs transition-colors ${
                showLabels
                  ? "border-accent bg-accent text-accent-ink"
                  : "border-border text-ink3 hover:text-ink"
              }`}
            >
              Names
            </button>
            <div
              className="flex rounded-[var(--radius-sm)] border border-border p-0.5 text-xs"
              data-help={toggleHelp}
            >
              <ModeButton
                active={layoutMode === "project"}
                onClick={() => setLayoutMode("project")}
              >
                By project
              </ModeButton>
              <ModeButton
                active={layoutMode === "semantic"}
                onClick={() => setLayoutMode("semantic")}
              >
                Semantic
              </ModeButton>
            </div>
          </div>
        </div>
      </header>

      {layoutMode === "semantic" && (
        <SemanticBar
          computing={computing}
          tsneInstalled={tsneInstalled}
          installing={installingTsne}
          installFrac={installFrac}
          onInstall={installTsne}
        />
      )}

      <div className="relative flex-1 overflow-hidden" data-help="graph-canvas" ref={wrapRef}>
        {error && (
          <div
            className="absolute left-4 top-4 z-10 rounded-[var(--radius-sm)] border px-3 py-2 text-sm"
            style={{
              borderColor: "color-mix(in oklab, var(--st-due) 45%, transparent)",
              background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
              color: "var(--st-due)",
            }}
          >
            {error}
          </div>
        )}

        {loading ? (
          <MapSkeleton />
        ) : documents.length === 0 ? (
          <div className="flex h-full items-center justify-center text-sm text-ink4">
            No documents yet. Ingest some in the Documents view and they'll appear here.
          </div>
        ) : !base ? (
          // Semantic mode with no coords yet: the background job is preparing the first layout.
          <div className="flex h-full flex-col items-center justify-center gap-5 text-sm text-ink4">
            <div className="flex items-end gap-8" aria-hidden>
              <Skeleton className="h-8 w-8" style={{ borderRadius: "9999px" }} />
              <Skeleton className="h-12 w-12" style={{ borderRadius: "9999px" }} />
              <Skeleton className="h-9 w-9" style={{ borderRadius: "9999px" }} />
            </div>
            <span>Preparing the semantic map…</span>
          </div>
        ) : (
          <>
            <canvas
              ref={canvasRef}
              className="h-full w-full"
              style={{ cursor }}
              onMouseDown={onCanvasMouseDown}
              onMouseMove={onMouseMove}
              onMouseLeave={onMouseLeave}
              onDoubleClick={resetView}
            />
            <NavControls
              onZoomIn={() => zoomButton(1.3)}
              onZoomOut={() => zoomButton(1 / 1.3)}
              onReset={resetView}
            />
          </>
        )}

        {hovered && <DetailCard doc={hovered} />}
      </div>
    </div>
  );
}

function ModeButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`rounded-[calc(var(--radius-sm)-2px)] px-2.5 py-1 transition-colors ${
        active ? "bg-accent text-accent-ink" : "text-ink3 hover:text-ink"
      }`}
    >
      {children}
    </button>
  );
}

/** Zoom + fit controls, bottom-right over the canvas. */
function NavControls({
  onZoomIn,
  onZoomOut,
  onReset,
}: {
  onZoomIn: () => void;
  onZoomOut: () => void;
  onReset: () => void;
}) {
  const btn =
    "flex h-7 w-7 items-center justify-center rounded-[var(--radius-sm)] border border-border bg-surface/85 text-ink2 backdrop-blur transition hover:text-ink hover:border-border2";
  return (
    <div className="absolute bottom-4 right-4 z-10 flex flex-col gap-1.5" data-help="map-navigate">
      <button type="button" className={btn} onClick={onZoomIn} aria-label="Zoom in" title="Zoom in">
        +
      </button>
      <button
        type="button"
        className={btn}
        onClick={onZoomOut}
        aria-label="Zoom out"
        title="Zoom out"
      >
        −
      </button>
      <button
        type="button"
        className={btn}
        onClick={onReset}
        aria-label="Fit to view"
        title="Fit to view"
      >
        ⤢
      </button>
    </div>
  );
}

/** The strip under the header in semantic mode — only shown when there's status to report (a download
 *  in progress, a recompute, or the not-installed nudge); otherwise it renders nothing. */
function SemanticBar({
  computing,
  tsneInstalled,
  installing,
  installFrac,
  onInstall,
}: {
  computing: boolean;
  tsneInstalled: boolean | null;
  installing: boolean;
  installFrac: number;
  onInstall: () => void;
}) {
  if (!installing && !computing && tsneInstalled !== false) return null;
  return (
    <div className="flex min-h-[1.75rem] items-center gap-2 border-b border-border bg-panel px-6 py-1.5 text-xs text-ink3">
      {installing ? (
        <IngestProgress
          mode="percent"
          processed={Math.round(installFrac * 100)}
          total={100}
          label="Downloading the enhanced (t-SNE) layout"
          className="w-full max-w-xs"
        />
      ) : computing ? (
        <>
          <Skeleton className="h-2 w-2" style={{ borderRadius: "9999px" }} />
          Updating the map by meaning…
        </>
      ) : (
        <>
          <span>Using the basic layout.</span>
          <button
            type="button"
            onClick={onInstall}
            className="font-medium text-accent hover:underline"
          >
            Enable enhanced (t-SNE) →
          </button>
        </>
      )}
    </div>
  );
}

/** A node-cluster shimmer that mirrors the map's shape, so a load reads as "your map is coming". */
function MapSkeleton() {
  return (
    <div className="flex h-full items-center justify-center">
      <div className="flex flex-col items-center gap-8" aria-hidden>
        <div className="flex items-end gap-10">
          <Skeleton className="h-9 w-9" style={{ borderRadius: "9999px" }} />
          <Skeleton className="h-16 w-16" style={{ borderRadius: "9999px" }} />
          <Skeleton className="h-11 w-11" style={{ borderRadius: "9999px" }} />
        </div>
        <div className="flex items-center gap-12">
          <Skeleton className="h-7 w-7" style={{ borderRadius: "9999px" }} />
          <Skeleton className="h-14 w-14" style={{ borderRadius: "9999px" }} />
          <Skeleton className="h-8 w-8" style={{ borderRadius: "9999px" }} />
        </div>
      </div>
    </div>
  );
}

function DetailCard({ doc }: { doc: Document }) {
  const { showPower } = useDepth();
  return (
    <div className="absolute right-4 top-4 z-10 w-72 rounded-[var(--radius)] border border-border2 bg-surface p-4 shadow-2xl backdrop-blur">
      <div className="text-sm font-semibold text-ink" title={doc.title}>
        {doc.title}
      </div>
      <dl className="mt-2 space-y-1.5 text-xs">
        <Row label="Project" value={doc.project} />
        <Row label="Importance" value={doc.importance ?? "—"} capitalize />
        <Row label="Chunks" value={String(doc.chunk_count)} />
        <Row label="Reviewed" value={doc.reviewed ? "yes" : "awaiting review"} />
        {showPower && <Row label="Ingested" value={formatDate(doc.ingested_at)} />}
      </dl>
      {doc.tags.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1">
          {doc.tags.map((t) => (
            <span
              key={t}
              className="rounded-[var(--radius-sm)] bg-bg px-2 py-0.5 text-xs text-ink2"
            >
              {t}
            </span>
          ))}
        </div>
      )}
      {showPower && doc.source_path && (
        <div className="mt-2 truncate text-xs text-ink4" title={doc.source_path}>
          {doc.source_path}
        </div>
      )}
    </div>
  );
}

function Row({ label, value, capitalize }: { label: string; value: string; capitalize?: boolean }) {
  return (
    <div className="flex justify-between gap-3">
      <dt className="text-ink3">{label}</dt>
      <dd className={`text-ink2 ${capitalize ? "capitalize" : ""}`}>{value}</dd>
    </div>
  );
}
