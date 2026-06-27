// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pure d3-force "by project" simulation, with no DOM or worker dependency, so it can run either
// in the layout Web Worker (the normal path) or synchronously on the main thread (a fallback when a
// worker can't be created). Keeping the force configuration in one place keeps the two paths in step.

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

interface SimNode extends SimulationNodeDatum {
  id: string;
  radius: number;
}
type SimLink = SimulationLinkDatum<SimNode>;

export interface LayoutRequest {
  nodes: { id: string; radius: number }[];
  links: { source: string; target: string }[];
  width: number;
  height: number;
}

export interface LayoutResponse {
  positions: { id: string; x: number; y: number }[];
}

/** Run the force layout to a converged set of positions. Heavy step; off-main-thread when possible. */
export function simulate(req: LayoutRequest): LayoutResponse {
  const { nodes: rawNodes, links: rawLinks, width, height } = req;

  // Fresh mutable copies — d3-force writes x/y onto these objects in place.
  const nodes: SimNode[] = rawNodes.map((n) => ({ id: n.id, radius: n.radius }));
  const links: SimLink[] = rawLinks.map((l) => ({ source: l.source, target: l.target }));

  const sim = forceSimulation(nodes)
    .force(
      "link",
      forceLink<SimNode, SimLink>(links)
        .id((n) => n.id)
        .distance(55)
        .strength(0.7),
    )
    .force("charge", forceManyBody().strength(-170))
    .force("center", forceCenter(width / 2, height / 2))
    .force(
      "collide",
      forceCollide<SimNode>().radius((n) => n.radius + 5),
    )
    .force("x", forceX(width / 2).strength(0.05))
    .force("y", forceY(height / 2).strength(0.05))
    .stop();

  // The layout is visually converged well before 400 ticks; fewer on big maps keeps the run quick.
  const ticks = nodes.length > 400 ? 250 : 400;
  for (let i = 0; i < ticks; i++) sim.tick();

  return { positions: nodes.map((n) => ({ id: n.id, x: n.x ?? 0, y: n.y ?? 0 })) };
}
