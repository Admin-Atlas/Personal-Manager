// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  forceCenter,
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type SimulationLinkDatum,
  type SimulationNodeDatum,
} from "d3-force";
import { listDocuments } from "../lib/ipc";
import { formatDate } from "../lib/format";
import { Skeleton } from "./ui";
import type { Document } from "../lib/types";
import { graphColor, useTheme, useDepth } from "../theme";
import type { Mode } from "../theme";

/**
 * A force-directed map of the store: every project is a hub node, and each
 * document is a small node linked to its project's hub, so the grouping is
 * visible at a glance (Step 4b add-on). Hover a document node for its details.
 *
 * The layout is computed once per document set (a force simulation run synchronously to
 * completion), then rendered as static SVG scaled to fit via a `viewBox` — no animation loop,
 * no resize plumbing. Two efficiency seams matter at scale (a Drive sync can push this past a
 * thousand nodes):
 *   1. The expensive part (the force simulation) is keyed on the *document set only*, so a theme
 *      change re-colours without re-running the layout.
 *   2. The edges + nodes are a memoised SVG fragment keyed on the coloured layout, so hovering a
 *      node only re-renders a single overlay highlight — not all N nodes.
 * Beyond a few thousand nodes the next step is a canvas/WebGL renderer and/or server-side
 * coordinates (see the `buildPositions` note).
 */

const WIDTH = 960;
const HEIGHT = 640;

interface GNode extends SimulationNodeDatum {
  id: string;
  kind: "project" | "doc";
  label: string;
  project: string;
  radius: number;
  /** Filled in by `colorize`; empty in the position-only base layout. */
  color: string;
  doc?: Document;
}

type GLink = SimulationLinkDatum<GNode>;

interface Edge {
  sx: number;
  sy: number;
  tx: number;
  ty: number;
}

/** Positions only — the costly force-sim output, independent of theme colour. */
interface BaseLayout {
  nodes: GNode[];
  edges: Edge[];
  viewBox: string;
  projectNames: string[];
}

/** A `BaseLayout` with theme colours applied — what the SVG renders. */
interface Layout {
  nodes: GNode[];
  edges: Edge[];
  viewBox: string;
  projects: { name: string; color: string }[];
}

export function GraphView() {
  const { mode } = useTheme();
  const [documents, setDocuments] = useState<Document[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [hovered, setHovered] = useState<Document | null>(null);
  // Distinguish "still loading" from "genuinely empty": without this the view would
  // flash "No documents yet" on every open (documents starts []), before the fetch lands.
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    listDocuments()
      .then(setDocuments)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  // The expensive force layout — recomputed only when the document set changes, NOT on a theme
  // (mode) toggle, which would otherwise re-run the whole simulation just to swap colours.
  const base = useMemo(() => buildPositions(documents), [documents]);
  // Cheap: apply theme colours + build the legend from the already-computed positions.
  const layout = useMemo<Layout | null>(() => (base ? colorize(base, mode) : null), [base, mode]);

  // Stable so the memoised map body below isn't invalidated by a hover.
  const onHover = useCallback((doc: Document) => setHovered(doc), []);
  const onLeave = useCallback(
    (doc: Document) => setHovered((cur) => (cur?.id === doc.id ? null : cur)),
    [],
  );

  // Edges + nodes as one memoised SVG fragment keyed on the layout alone. Because the element
  // reference is stable across a hover, React skips reconciling all ~1000 nodes when `hovered`
  // changes — only the overlay highlight + detail card re-render. This is the main win at scale.
  const mapBody = useMemo(() => {
    if (!layout) return null;
    return (
      <>
        {layout.edges.map((e, i) => (
          <line
            key={i}
            x1={e.sx}
            y1={e.sy}
            x2={e.tx}
            y2={e.ty}
            stroke="var(--border)"
            strokeWidth={0.6}
          />
        ))}
        {layout.nodes.map((n) =>
          n.kind === "project" ? (
            <g key={n.id} className="pointer-events-none">
              <circle
                cx={n.x}
                cy={n.y}
                r={n.radius}
                fill={n.color}
                fillOpacity={0.25}
                stroke={n.color}
                strokeWidth={1.5}
              />
              <text
                x={n.x}
                y={(n.y ?? 0) - n.radius - 4}
                textAnchor="middle"
                style={{ fontSize: 11, fontWeight: 600, fill: "var(--ink2)" }}
              >
                {n.label}
              </text>
            </g>
          ) : (
            <circle
              key={n.id}
              cx={n.x}
              cy={n.y}
              r={n.radius}
              fill={n.color}
              fillOpacity={0.85}
              stroke={n.doc && !n.doc.reviewed ? "var(--st-look)" : "var(--bg)"}
              strokeWidth={n.doc && !n.doc.reviewed ? 1.5 : 1}
              strokeDasharray={n.doc && !n.doc.reviewed ? "2 2" : undefined}
              style={{ cursor: "pointer" }}
              onMouseEnter={() => n.doc && onHover(n.doc)}
              onMouseLeave={() => n.doc && onLeave(n.doc)}
            />
          ),
        )}
      </>
    );
  }, [layout, onHover, onLeave]);

  // The hovered node's position, looked up once per hover (a single pass — far cheaper than
  // re-rendering every node) so the highlight can be drawn as one overlay element.
  const hoveredNode = useMemo(
    () => (hovered && layout ? (layout.nodes.find((n) => n.doc?.id === hovered.id) ?? null) : null),
    [hovered, layout],
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
              {documents.length} document{documents.length === 1 ? "" : "s"} across{" "}
              {layout?.projects.length ?? 0} project
              {layout && layout.projects.length === 1 ? "" : "s"} · hover a node for details
            </p>
          )}
        </div>
        {layout && (
          <div className="flex max-w-[60%] flex-wrap justify-end gap-x-3 gap-y-1">
            {layout.projects.map((p) => (
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

      <div className="relative flex-1 overflow-hidden" data-help="graph-canvas">
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
        ) : !layout ? (
          <div className="flex h-full items-center justify-center text-sm text-ink4">
            No documents yet. Ingest some in the Documents view and they'll appear here.
          </div>
        ) : (
          <svg
            viewBox={layout.viewBox}
            className="h-full w-full"
            preserveAspectRatio="xMidYMid meet"
          >
            {mapBody}
            {hoveredNode && (
              <circle
                cx={hoveredNode.x}
                cy={hoveredNode.y}
                r={hoveredNode.radius}
                fill={hoveredNode.color}
                fillOpacity={1}
                stroke="var(--ink)"
                strokeWidth={2}
                className="pointer-events-none"
              />
            )}
          </svg>
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

/**
 * Run the force simulation to completion and project it into render-ready *positions* (no colour).
 * This is the heavy step; keep it keyed on the document set alone.
 *
 * Future: for much larger maps, compute coordinates server-side once and cache them — e.g. a
 * UMAP/t-SNE projection of the chunk embeddings — and stream just `{id, x, y}` here. That replaces
 * this synchronous sim with a one-time backend job and makes the map *semantic* (nearby = similar),
 * but the render path below is what governs draw cost, so it stays as-is.
 */
function buildPositions(documents: Document[]): BaseLayout | null {
  if (documents.length === 0) return null;

  const projectNames = Array.from(new Set(documents.map((d) => d.project || "Unsorted")));

  const projectNodes: GNode[] = projectNames.map((name) => ({
    id: `project:${name}`,
    kind: "project",
    label: name,
    project: name,
    radius: 14,
    color: "",
  }));
  const docNodes: GNode[] = documents.map((d) => ({
    id: `doc:${d.id}`,
    kind: "doc",
    label: d.title,
    project: d.project || "Unsorted",
    radius: Math.min(16, 4.5 + Math.sqrt(Math.max(1, d.chunk_count)) * 1.6),
    color: "",
    doc: d,
  }));

  const nodes = [...projectNodes, ...docNodes];
  const links: GLink[] = docNodes.map((n) => ({ source: n.id, target: `project:${n.project}` }));

  const sim = forceSimulation(nodes)
    .force(
      "link",
      forceLink<GNode, GLink>(links)
        .id((n) => n.id)
        .distance(55)
        .strength(0.7),
    )
    .force("charge", forceManyBody().strength(-170))
    .force("center", forceCenter(WIDTH / 2, HEIGHT / 2))
    .force(
      "collide",
      forceCollide<GNode>().radius((n) => n.radius + 5),
    )
    .force("x", forceX(WIDTH / 2).strength(0.05))
    .force("y", forceY(HEIGHT / 2).strength(0.05))
    .stop();
  // The sim runs synchronously on the main thread, so cap the work on big maps: the layout is
  // visually converged well before 400 ticks, and fewer ticks keeps opening a ~1000-node map (a
  // full Drive sync) responsive instead of freezing the UI.
  const ticks = nodes.length > 400 ? 250 : 400;
  for (let i = 0; i < ticks; i++) sim.tick();

  const edges: Edge[] = links.map((l) => {
    const s = l.source as GNode;
    const t = l.target as GNode;
    return { sx: s.x ?? 0, sy: s.y ?? 0, tx: t.x ?? 0, ty: t.y ?? 0 };
  });

  // Fit everything in view via the viewBox (accounting for node radius + labels).
  const pad = 50;
  const xs = nodes.map((n) => n.x ?? 0);
  const ys = nodes.map((n) => n.y ?? 0);
  const minX = Math.min(...xs) - pad;
  const minY = Math.min(...ys) - pad;
  const w = Math.max(...xs) - minX + pad;
  const h = Math.max(...ys) - minY + pad;

  return { nodes, edges, viewBox: `${minX} ${minY} ${w} ${h}`, projectNames };
}

/** Apply theme colours to a position-only layout + build the project legend. Cheap (no simulation). */
function colorize(base: BaseLayout, mode: Mode): Layout {
  const colorFor = (project: string) => graphColor(base.projectNames.indexOf(project), mode);
  return {
    nodes: base.nodes.map((n) => ({ ...n, color: colorFor(n.project) })),
    edges: base.edges,
    viewBox: base.viewBox,
    projects: base.projectNames.map((name) => ({ name, color: colorFor(name) })),
  };
}
