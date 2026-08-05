// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Dash points: a second kind of bullet, for a note that wants two.
//
// A pinboard note has two bullet markers — "." and "-" — and until now they rendered identically,
// because `toRenderMarkdown` turned both into the same GFM `- ` item and Markdown keeps no record of
// which character an author used. mdast has no marker field, and CSS cannot tell two `<ul>`s apart.
//
// So the note dialect now emits the two as DIFFERENT GFM bullet characters — "." as `* `, "-" as
// `+ ` — and this plugin puts the distinction back into the tree that the character carries through
// the parse. It reads the source character back out of the node's own position and tags the list, so
// `index.css` can give one a disc and the other an en dash. **Both stay real list items**: nesting
// and hanging indent are the whole reason not to render a dash point as prose with a dash typed in
// front of it.
//
// Two properties this leans on, both worth knowing before touching it:
//
//   * **Changing the bullet character STARTS A NEW LIST in CommonMark.** `* a` followed by `+ b` is
//     two `<ul>`s, not one list with two markers — which is exactly what makes a per-list class the
//     right granularity. A list is therefore homogeneous by construction and its first item's marker
//     speaks for all of them. It is also why the note dialect emits its CHECKBOXES on the bullet
//     marker: on any other one, a checklist under a bullet would become a second list with the
//     larger inter-list margin opening up between them.
//   * **This runs on mdast, i.e. as a REMARK plugin, so it is upstream of the whole rehype chain.**
//     `rehypeSanitize` still runs last and still has the final word on the class it emits — the
//     security invariant `markdown.tsx` documents is untouched, and `REHYPE_PLUGINS` keeps its
//     pinned length. The sanitizer's allow-list admits exactly the one literal class name below,
//     the same way it admits `task-list-item` and nothing else on an `li`.
//
// Note this is OPT-IN per caller (`<Markdown dashLists>`), used by the pinboard note alone. An
// ingested document that happens to use `+` for its bullets must keep rendering the way it always
// has; a silent restyle of other people's Markdown is not a thing to trade for a note feature.

/** The class the plugin tags a dash list with. Allowed on `ul` by `SCHEMA` as a pinned literal, and
 *  styled in the `.pm-markdown` block of `src/index.css`. */
export const DASH_LIST_CLASS = "pm-dash-list";

/** The GFM bullet character `toRenderMarkdown` emits for a "-" dash point.
 *
 *  It has to be one of GFM's three bullet characters and it cannot be "-", which is the dash point's
 *  own INPUT marker — emitting that would make the transform read its own output back as more dash
 *  points. That leaves `*` and `+`; `*` went to the round bullet, since it is the one people already
 *  read as an ordinary bullet in plain text, and this got the other. */
export const DASH_MARKER = "+";

/** The shape this walks — structural rather than imported, because `mdast`/`unist` are transitive
 *  dependencies of react-markdown rather than declared ones, so a type imported from either would
 *  ride on hoisting (the same reasoning `markdown.tsx` records for `unified`). */
interface MdNode {
  type: string;
  children?: MdNode[];
  position?: { start?: { offset?: number } };
  data?: { hProperties?: Record<string, unknown> };
}

/**
 * The marker character a list item begins with, read out of the source at `offset`.
 *
 * Scans past leading whitespace rather than trusting the offset to land on the marker: a nested
 * list's position starts at its indentation, so reading the character AT the offset would report a
 * space for every nested list and quietly leave them all unmarked.
 */
function markerCharAt(source: string, offset: number): string | null {
  for (let i = offset; i < source.length; i++) {
    const ch = source[i];
    if (ch === " " || ch === "\t") continue;
    return ch;
  }
  return null;
}

/** Tag every list in `tree` whose source marker is {@link DASH_MARKER}. Exported for its unit test —
 *  the plugin below is the three-line unified wrapper around it. */
export function markDashLists(tree: MdNode, source: string): void {
  const walk = (node: MdNode): void => {
    if (node.type === "list") {
      const first = node.children?.[0];
      const offset = first?.position?.start?.offset;
      if (typeof offset === "number" && markerCharAt(source, offset) === DASH_MARKER) {
        node.data = {
          ...node.data,
          hProperties: { ...node.data?.hProperties, className: DASH_LIST_CLASS },
        };
      }
    }
    for (const child of node.children ?? []) walk(child);
  };
  walk(tree);
}

/** The remark plugin. Runs on mdast, so it cannot reach past the sanitizer. */
export function remarkDashLists() {
  return (tree: MdNode, file: { value?: unknown }): void => {
    // A VFile's value is a string for every path PM uses (react-markdown hands it the `children`
    // string); anything else means there is no source to read a marker out of, so leave the tree be.
    if (typeof file?.value === "string") markDashLists(tree, file.value);
  };
}
