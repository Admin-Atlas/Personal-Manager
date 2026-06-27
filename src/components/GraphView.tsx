// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { quadtree, type Quadtree } from "d3-quadtree";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  installOptionalTsne,
  listDocuments,
  onLayoutProgress,
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
 * of nodes — index-only Drive files inflate the count — stay smooth. Hover a node for its details;
 * click one to open its project's focused view. Colour is applied in the draw call, so a theme toggle
 * is a redraw, not a relayout.
 */

type LayoutMode = "project" | "semantic";
const MODE_KEY = "pm.map.layoutMode";

function initialMode(): LayoutMode {
  return localStorage.getItem(MODE_KEY) === "semantic" ? "semantic" : "project";
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
  const [loading, setLoading] = useState(true);

  const [layoutMode, setLayoutMode] = useState<LayoutMode>(initialMode);
  const [semantic, setSemantic] = useState<SemanticLayout | null>(null);
  const [computing, setComputing] = useState(false);
  const [tsneInstalled, setTsneInstalled] = useState<boolean | null>(null);
  const [installingTsne, setInstallingTsne] = useState(false);

  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const transformRef = useRef<Transform | null>(null);

  useEffect(() => {
    listDocuments()
      .then(setDocuments)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    optionalTsneStatus()
      .then((s) => setTsneInstalled(s.installed))
      .catch(() => setTsneInstalled(false));
  }, []);

  // Persist the chosen arrangement (per-device, like the theme/depth prefs).
  useEffect(() => {
    localStorage.setItem(MODE_KEY, layoutMode);
  }, [layoutMode]);

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
  // the backend coords to the documents. A stale async result can't overwrite a newer one.
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
      setBase(coords.length > 0 ? buildSemanticLayout(documents, coords) : null);
    }
    return () => {
      cancelled = true;
    };
  }, [documents, layoutMode, semantic]);

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

    const t = fitTransform(base.bounds, cssW, cssH);
    transformRef.current = t;
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
  }, [base, colorFor, hovered]);

  useEffect(() => {
    const id = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(id);
  }, [draw]);

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

  const installTsne = useCallback(() => {
    setInstallingTsne(true);
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
        ? `showing the top ${placed} of ${documents.length} documents by meaning · click to open a project`
        : `${documents.length} document${documents.length === 1 ? "" : "s"} by meaning · click to open a project`
      : `${documents.length} document${documents.length === 1 ? "" : "s"} across ${legend.length} project${legend.length === 1 ? "" : "s"} · hover for details, click to open`;

  // The toggle's help differs by whether t-SNE is installed (explain what it is vs that it's running).
  const toggleHelp = tsneInstalled ? "map-layout-toggle-tsne" : "map-layout-toggle";

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between gap-4 border-b border-border px-6 py-3">
        <div className="flex items-center gap-4">
          <div data-help="nav-graph">
            <h1 className="text-sm font-semibold font-head text-ink">Map</h1>
            {loading ? (
              <Skeleton className="mt-1 h-3 w-56" />
            ) : (
              <p className="text-xs text-ink3">{subtitle}</p>
            )}
          </div>
          <div
            className="flex rounded-[var(--radius-sm)] border border-border p-0.5 text-xs"
            data-help={toggleHelp}
          >
            <ModeButton
              active={layoutMode === "semantic"}
              onClick={() => setLayoutMode("semantic")}
            >
              Semantic
            </ModeButton>
            <ModeButton active={layoutMode === "project"} onClick={() => setLayoutMode("project")}>
              By project
            </ModeButton>
          </div>
        </div>
        {base && (
          <div className="flex max-w-[50%] flex-wrap justify-end gap-x-3 gap-y-1">
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

      {layoutMode === "semantic" && (
        <SemanticBar
          computing={computing}
          tsneInstalled={tsneInstalled}
          installing={installingTsne}
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

/** The strip under the header in semantic mode: a recompute indicator, or the optional-t-SNE nudge. */
function SemanticBar({
  computing,
  tsneInstalled,
  installing,
  onInstall,
}: {
  computing: boolean;
  tsneInstalled: boolean | null;
  installing: boolean;
  onInstall: () => void;
}) {
  if (computing) {
    return (
      <div className="flex items-center gap-2 border-b border-border bg-panel px-6 py-1.5 text-xs text-ink3">
        <Skeleton className="h-2 w-2" style={{ borderRadius: "9999px" }} />
        Updating the map by meaning…
      </div>
    );
  }
  if (installing) {
    return (
      <div className="flex items-center gap-2 border-b border-border bg-panel px-6 py-1.5 text-xs text-ink3">
        <Skeleton className="h-2 w-2" style={{ borderRadius: "9999px" }} />
        Downloading the enhanced (t-SNE) layout…
      </div>
    );
  }
  if (tsneInstalled === false) {
    return (
      <div className="flex items-center gap-2 border-b border-border bg-panel px-6 py-1.5 text-xs text-ink3">
        <span>Using the basic layout.</span>
        <button
          type="button"
          onClick={onInstall}
          className="font-medium text-accent hover:underline"
        >
          Enable enhanced (t-SNE) →
        </button>
      </div>
    );
  }
  return null;
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
