// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The Pinboard model (spec §4 — a bounded planning board). The board is user content (notes,
 *  timelines), so it is persisted to the encrypted `settings` table over IPC — see usePinboard.
 *  All geometry is in **grid cells**, not pixels, so the board renders crisply at any cell size. */

export type WidgetKind = "note" | "timeline" | "folder";

/** The games a folder can play — see `game.ts` for the rules and `FolderGame.tsx` for the table.
 *  Lives here rather than beside the rules because it is part of the persisted board shape, and
 *  `types.ts` is where that shape is declared. **Only ever add to this union**: dropping a value
 *  would leave stored boards naming a game that no longer exists. */
export type GameKind = "roulette" | "straws" | "box" | "coin" | "rps";

/** A widget's position and size, in grid cells (top-left origin). */
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A single point on the board, in grid cells — e.g. the cell the mouse pointer is over. Distinct
 *  from a Rect: filing a drop into a folder is decided by the POINTER, not by the dragged rect. */
export interface CellPoint {
  x: number;
  y: number;
}

/** One dated entry on a timeline widget. `date` is an ISO date (YYYY-MM-DD); rendered DD-MM-YYYY. */
export interface TimelineItem {
  id: string;
  date: string;
  label: string;
}

export interface Widget {
  id: string;
  kind: WidgetKind;
  rect: Rect;
  /** A design-token name (`st-quick` etc.) used to tint the widget; undefined → neutral. */
  color?: string;
  // note
  text?: string;
  /** Set once a note has been ingested as a document (ISO time). The document is keyed on the
   *  widget id (`note:<id>`) and lives independently — deleting the note never removes it. */
  ingestedAt?: string;
  /** A cheap client-side hash of the note text at last ingest, so the UI can offer "re-ingest
   *  edits" when the text has since changed. Not the backend content hash. */
  ingestedHash?: string;
  /** A short user label shown (and edited) in the widget's top bar. Originally timeline-only;
   *  now the shared title for every kind (note / timeline / folder). Empty → the header shows a
   *  kind placeholder. Optional and additive, so old boards parse unchanged. */
  title?: string;
  // timeline
  items?: TimelineItem[];
  /** When set, a timeline widget is *bound* to this project: it shows and edits that project's
   *  real milestones (backend `project_milestones`) instead of the freeform `items` above, so
   *  changes flow to the daily brief and the project's Focus/sidebar. Unset → freeform (default).
   *  Optional and additive, so old boards parse unchanged and `BOARD_VERSION` stays 1. */
  project?: string;
  /** Freeform timelines only: whether this widget's dated entries also appear on the Calendar tab
   *  (the pinboard overlay). Unset → shown, so entries land on the calendar by default. A
   *  project-bound timeline ignores it — those entries are real milestones, already drawn by the
   *  milestone overlay. Optional and additive, so old boards parse unchanged and `BOARD_VERSION`
   *  stays 1. */
  showOnCalendar?: boolean;
  /** How a timeline widget lays its entries out: `"list"` (stacked rows) or `"row"` (a
   *  horizontal date-ordered track). Unset falls back to each kind's historical default —
   *  freeform → list, project-bound → row — so old boards look identical. Optional and
   *  additive, so `BOARD_VERSION` stays 1. */
  view?: "list" | "row";
  // folder (kind === "folder")
  /** The widgets contained by a folder. Notes and timelines only — folders never nest (enforced
   *  on drop and on load). A collapsed folder shows the child count; expanding reveals the cards.
   *  Additive, so old boards (which have none) parse unchanged and `BOARD_VERSION` stays 1. */
  children?: Widget[];
  /** How an opened folder is presented, persisted per-folder. Default `"inline"` (an in-place
   *  panel); `"overlay"` opens a centred modal. Whether the folder is *currently* expanded is
   *  transient UI state and is deliberately NOT stored here. */
  expandMode?: "inline" | "overlay";
  /** Which game a folder plays when you open it — see `game.ts`. Unset means it has never been
   *  given one. Choosing a game does NOT by itself change what the tile does; {@link gameOn} is
   *  the switch. Optional and additive, so old boards parse unchanged and `BOARD_VERSION` stays 1. */
  game?: GameKind;
  /** Whether this folder's game is turned on. Absent or `false` → the tile opens the cards, exactly
   *  as a folder always has; `true` → the tile plays the game and the cards are one click away
   *  inside it. Kept apart from {@link game} so turning a game off remembers which one it was.
   *  Optional and additive, so old boards parse unchanged and `BOARD_VERSION` stays 1. */
  gameOn?: boolean;
  /** The cards already drawn in the round now in progress, oldest first. They stay in the folder,
   *  greyed, and are not drawn again until the round loops all the way back — at which point this
   *  empties itself. Persisted deliberately: a round should survive closing the lid on the laptop
   *  and coming back to it. Optional and additive, so `BOARD_VERSION` stays 1. */
  spent?: string[];
  /** Whether a drawn card also leaves the folder for the board (the same move the ⤴ makes). Absent
   *  → it stays put and merely greys out. Optional and additive, so `BOARD_VERSION` stays 1. */
  autoPopOut?: boolean;
  /** Whether a card can be drawn again straight away — TRUE random, with no memory between plays.
   *  Absent (the default) → a drawn card greys out and waits for the round to loop, which is the
   *  behaviour {@link spent} describes. `true` → nothing is ever recorded as drawn, every card is
   *  in every play, and the wheel can hand you the same job twice running. A whole-folder choice
   *  rather than a per-play one, because it is a statement about how you want to be nagged.
   *  Optional and additive, so old boards parse unchanged and `BOARD_VERSION` stays 1. */
  repeat?: boolean;
  /** A folder CHILD's share of the draw, where the game has a visible proportion to give it — the
   *  roulette wheel, and only that one. `1` is an even share and is what every card is worth until
   *  it is changed, so absent means the same as 1 and an untouched folder draws uniformly. Read
   *  through `weightOf`, which is where the range is enforced. Optional and additive, so old boards
   *  parse unchanged and `BOARD_VERSION` stays 1. */
  weight?: number;
}

export interface Board {
  /** Schema version, so a future shape change can migrate or discard an old stored board. */
  version: number;
  widgets: Widget[];
}

export const BOARD_VERSION = 1;

/** The settings-table key the board persists under (must match the backend `set_pref` allowlist).
 *  Shared so read-only consumers (the calendar's pinboard overlay) can't drift from the writer. */
export const PINBOARD_PREF_KEY = "pinboard";

export const EMPTY_BOARD: Board = { version: BOARD_VERSION, widgets: [] };
