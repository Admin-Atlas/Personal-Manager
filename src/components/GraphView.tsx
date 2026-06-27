// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { quadtree, type Quadtree } from "d3-quadtree";
import { listDocuments } from "../lib/ipc";
import { formatDate } from "../lib/format";
import {
  getProjectLayout,
  type BaseLayout,
  type Bounds,
  type PositionedNode,
} from "../lib/mapLayout";
import { Skeleton } from "./ui";
import type { Document } from "../lib/types";
import { graphColor, useTheme, useDepth } from "../theme";

/**
 * A map of the store: every project is a hub node, and each document is a small node linked to its
 * project's hub, so the grouping is visible at a glance. Hover a document node for its details; click
 * one to open its project's focused view.
 *
 * Rendering is on a single `<canvas>` rather than per-node SVG, so a vault with thousands of nodes (a
 * full Drive sync inflates the count with index-only files) stays smooth: one draw call per frame, and
 * hover/click hit-testing via a d3-quadtree (O(log n)) instead of N DOM listeners. The heavy force
 * layout runs off the main thread on a worker and is pre-warmed at launch (see `src/lib/mapLayout.ts`),
 * so opening the Map doesn't stutter the app. Two efficiency seams carry over: the layout is keyed on
 * the document set only (a theme change re-colours without re-running it), and colour is applied in the
 * draw call (a theme toggle is a redraw, not a recompute).
 */

/**
 * Themed values the canvas needs as concrete strings, resolved from the document root. Canvas can't
 * read CSS `var(--…)` directly (unlike the old SVG), so we resolve the colours and the UI font here.
 */
interface ThemeColors {
  border: string;
  bg: string;
  ink: string;
  ink2: string;
  stLook: string;
  uiFont: string;
}

function readThemeColors(): ThemeColors {
  const s = getComputedStyle(document.documentElement);
  const v = (name: string) => s.getPropertyValue(name).trim() || "#888";
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
  // Distinguish "still loading" from "genuinely empty": without this the view would flash
  // "No documents yet" on every open (documents starts []), before the fetch lands.
  const [loading, setLoading] = useState(true);

  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const transformRef = useRef<Transform | null>(null);

  useEffect(() => {
    listDocuments()
      .then(setDocuments)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  // The layout is async (the force sim runs on a worker, pre-warmed at launch). A cache hit on the
  // same document set resolves immediately. Guard against a stale set landing after a newer one.
  useEffect(() => {
    let cancelled = false;
    const job = getProjectLayout(documents);
    if (!job) {
      setBase(null);
      return;
    }
    job.then((b) => !cancelled && setBase(b)).catch(() => !cancelled && setBase(null));
    return () => {
      cancelled = true;
    };
  }, [documents]);

  // Project → palette index, for colouring nodes + the legend. Keyed on the layout only.
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

  // Hit-testing index over the document nodes (project hubs aren't interactive), in world coords so
  // it survives resize/zoom. O(log n) lookup is the scale win that keeps thousands of nodes smooth.
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

    // Size the backing store for crisp nodes on HiDPI; draw in CSS pixels.
    if (canvas.width !== Math.round(cssW * dpr) || canvas.height !== Math.round(cssH * dpr)) {
      canvas.width = Math.round(cssW * dpr);
      canvas.height = Math.round(cssH * dpr);
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, cssH);

    const t = fitTransform(base.bounds, cssW, cssH);
    transformRef.current = t;
    const sx = (x: number) => x * t.scale + t.offsetX;
    const sy = (y: number) => y * t.scale + t.offsetY;
    const colors = readThemeColors();

    // Edges (one path, single stroke). Node radii scale with the fit (size = content volume, a real
    // signal); stroke/edge widths stay fixed in screen pixels so they read as crisp chrome at any zoom.
    ctx.strokeStyle = colors.border;
    ctx.lineWidth = 0.75;
    ctx.beginPath();
    for (const e of base.edges) {
      ctx.moveTo(sx(e.sx), sy(e.sy));
      ctx.lineTo(sx(e.tx), sy(e.ty));
    }
    ctx.stroke();
    ctx.setLineDash([]);

    // Nodes.
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
        ctx.globalAlpha = 0.85;
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

    // Hovered node redrawn last, fully opaque with an --ink halo (the old SVG overlay, now a draw call).
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
  }, [base, colorFor, hovered]);

  // Redraw on layout / theme / hover change, batched into a frame.
  useEffect(() => {
    const id = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(id);
  }, [draw]);

  // Redraw on resize (the canvas, unlike the old SVG viewBox, doesn't rescale itself).
  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const ro = new ResizeObserver(() => requestAnimationFrame(draw));
    ro.observe(wrap);
    return () => ro.disconnect();
  }, [draw]);

  // ---- pointer interaction (hit-test in world coords) ----
  const nodeAt = useCallback(
    (clientX: number, clientY: number): PositionedNode | null => {
      const canvas = canvasRef.current;
      const t = transformRef.current;
      if (!canvas || !t || !docQuadtree) return null;
      const rect = canvas.getBoundingClientRect();
      const wx = (clientX - rect.left - t.offsetX) / t.scale;
      const wy = (clientY - rect.top - t.offsetY) / t.scale;
      const hit = docQuadtree.find(wx, wy, 18);
      if (!hit) return null;
      const d = Math.hypot(hit.x - wx, hit.y - wy);
      return d <= hit.radius + 2 ? hit : null;
    },
    [docQuadtree],
  );

  const onMouseMove = useCallback(
    (e: React.MouseEvent) => {
      const hit = nodeAt(e.clientX, e.clientY);
      const doc = hit?.doc ?? null;
      setHovered((cur) => (cur?.id === doc?.id ? cur : doc));
    },
    [nodeAt],
  );
  const onMouseLeave = useCallback(() => setHovered(null), []);
  const onClick = useCallback(
    (e: React.MouseEvent) => {
      const hit = nodeAt(e.clientX, e.clientY);
      if (hit?.doc && onOpenProject) onOpenProject(hit.doc.project);
    },
    [nodeAt, onOpenProject],
  );

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b border-border px-6 py-3">
        <div data-help="nav-graph">
          <h1 className="text-sm font-semibold font-head text-ink">Map</h1>
          {loading ? (
            <Skeleton className="mt-1 h-3 w-56" />
          ) : (
            <p className="text-xs text-ink3">
              {documents.length} document{documents.length === 1 ? "" : "s"} across {legend.length}{" "}
              project{legend.length === 1 ? "" : "s"} · hover a node for details, click to open its
              project
            </p>
          )}
        </div>
        {base && (
          <div className="flex max-w-[60%] flex-wrap justify-end gap-x-3 gap-y-1">
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
      </header>

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
          // A node-cluster shimmer that mirrors the map's shape, so the load reads as
          // "your map is coming" rather than "you have nothing".
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
        ) : !base ? (
          <div className="flex h-full items-center justify-center text-sm text-ink4">
            No documents yet. Ingest some in the Documents view and they'll appear here.
          </div>
        ) : (
          <canvas
            ref={canvasRef}
            className="h-full w-full"
            style={{ cursor: hovered ? "pointer" : "default" }}
            onMouseMove={onMouseMove}
            onMouseLeave={onMouseLeave}
            onClick={onClick}
          />
        )}

        {hovered && <DetailCard doc={hovered} />}
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
