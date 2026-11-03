// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Runway (PA-2): the Agenda's board layout in Slate and Editorial. Tasks and milestones sit in
// columns running forward from today (Before today, Today, Tomorrow, the rest of this week, next
// week, Later, and any named window the user adds), each with a foreground and a background lane,
// over a shelf of parked tasks. A task's card ticks, opens its why, opens its project and its
// linked event, opens the shared task editor, and moves: by dragging it (`useRunwayDrag`), or from
// its Move… menu, which is the same move from the keyboard. A drop or a move writes the task's
// plan (`planTask`) and then the lane's order (`reorderTasks`), and says where it went.
//
// What sits where is the pure `lib/runway.ts`; this only draws it. It holds no copy of a task: the
// task IPC fires `tasks://changed` after every write and CalendarView reads the list again.
// Milestones aren't dragged (their dates are the project's). Each column from today on heads its
// lanes with a read-only strip of the calendar's events and pinboard entries on its days; an
// event only opens its details.

import { useCallback, useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import {
  readRunwayPastOpen,
  writeRunwayPastOpen,
  type RunwayWindow,
} from "../../../lib/calendarPrefs";
import { deadlineInfo, STACK_KIND_LABEL, TONE_CLASS } from "../../../lib/tasks";
import {
  dropPlan,
  isDropColumn,
  linkedEvent,
  orderAfterDrop,
  type RunwayCard,
  type RunwayColumn,
  type RunwayEventStrip,
  type RunwayMilestoneCard,
  type RunwayStackCard,
  type RunwayTaskCard,
  type TaskPlan,
} from "../../../lib/runway";
import type {
  CalendarEvent,
  Milestone,
  Task,
  TaskInput,
  TaskLane,
  TaskStack,
} from "../../../lib/types";
import { useDepth } from "../../../theme";
import { DeadlineChip, ProjectChip, StackChip, TaskStatusButton } from "../../tasks/TaskBits";
import { TaskEditor } from "../../tasks/TaskEditor";
import { Button, Callout, SectionLabel, cn } from "../../ui";
import {
  eventWhen,
  linkedDue,
  memberPlace,
  newTaskDefaults,
  placeName,
  rolledWords,
  useRunwayActions,
  useRunwayModel,
  type RunwayProps,
} from "./runwayLogic";
import { EventsStrip, MoveMenu, RunwayStrip, WindowDialog } from "./runwayParts";
import {
  useRunwayDrag,
  type RunwayDrag,
  type RunwayDragItem,
  type RunwayDropAt,
} from "./useRunwayDrag";

/** Below this width the columns stack into one. */
const NARROW_PX = 720;

const LANES: { lane: TaskLane; label: string }[] = [
  { lane: "foreground", label: "Foreground" },
  { lane: "background", label: "Background" },
];

type Editing = { task: Task } | { defaults: Partial<TaskInput> } | null;

/** What every card needs from the board. */
interface CardCtx {
  today: Date;
  now: Date;
  columns: RunwayColumn[];
  events: CalendarEvent[];
  strips: Record<string, RunwayEventStrip>;
  stackById: Map<number, TaskStack>;
  drag: RunwayDrag;
  cycle: (task: Task, next: Task["status"]) => void;
  replan: (tasks: readonly Task[], planFor: (t: Task) => TaskPlan | null, where: string) => void;
  reorder: (order: number[], said: string) => void;
  edit: (task: Task) => void;
  colorOf?: (calendarId: string) => string;
  onOpenProject?: (project: string) => void;
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
  onShowList?: () => void;
}

export function RunwayBoard(props: RunwayProps) {
  const {
    tasks,
    stacks,
    events,
    windows,
    now,
    colorOf,
    onWindowsChange,
    onOpenProject,
    onEventClick,
    onShowList,
    announce,
  } = props;
  const { today, columns, model, strips } = useRunwayModel(props);
  const { error, cycle, replan, reorder } = useRunwayActions(announce);
  const [editing, setEditing] = useState<Editing>(null);
  const [windowEdit, setWindowEdit] = useState<{ window: RunwayWindow | null } | null>(null);
  const [pastOpen, setPastOpen] = useState(readRunwayPastOpen);
  const [folded, setFolded] = useState<Set<string>>(() => new Set());
  const rootRef = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const [narrow, setNarrow] = useState(false);

  // Measured, not a media query: the board's width is the window less the sidebar.
  useEffect(() => {
    const el = gridRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width ?? el.clientWidth;
      if (w > 0) setNarrow(w < NARROW_PX);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const taskById = useMemo(() => new Map(tasks.map((t) => [t.id, t])), [tasks]);
  const stackById = useMemo(() => new Map(stacks.map((s) => [s.id, s])), [stacks]);

  const onDrop = useCallback(
    (item: RunwayDragItem, at: RunwayDropAt) => {
      const moved = item.taskIds
        .map((id) => taskById.get(id))
        .filter((t): t is Task => t !== undefined);
      if (moved.length === 0) return;
      if (at.target.kind === "parked") {
        replan(
          moved,
          (t) => dropPlan({ kind: "parked" }, t),
          "Parked",
          orderAfterDrop(model.parked, item.taskIds, at.index),
        );
        return;
      }
      const { columnId, lane } = at.target;
      const column = columns.find((c) => c.id === columnId);
      if (!column || !isDropColumn(column)) return;
      // Where each came from: back into its own column it keeps its days (a reorder).
      const fromOf = (t: Task) => columns.find((c) => c.id === model.columnOf.get(t.id)) ?? null;
      replan(
        moved,
        (t) => dropPlan({ kind: "column", column, lane }, t, fromOf(t)),
        placeName(column, lane),
        orderAfterDrop(model.lanes[column.id]?.[lane] ?? [], item.taskIds, at.index),
      );
    },
    [taskById, columns, model, replan],
  );
  const drag = useRunwayDrag({
    rootRef,
    onDrop,
    onStart: (item) => {
      const first = taskById.get(item.taskIds[0]);
      announce(
        item.taskIds.length > 1
          ? `Moving ${item.taskIds.length} tasks`
          : `Moving ${first?.title ?? "the task"}`,
      );
    },
    onAbandon: () => announce("Put back."),
  });

  const ctx: CardCtx = {
    today,
    now,
    columns,
    events,
    strips,
    stackById,
    drag,
    cycle,
    replan: (ts, planFor, where) => replan(ts, planFor, where),
    reorder,
    edit: (task) => setEditing({ task }),
    colorOf,
    onOpenProject,
    onEventClick,
    onShowList,
  };

  const isFolded = (c: RunwayColumn) => (c.kind === "past" ? !pastOpen : folded.has(c.id));
  const toggleFold = (c: RunwayColumn) => {
    if (c.kind === "past") {
      setPastOpen((o) => {
        writeRunwayPastOpen(!o);
        return !o;
      });
      return;
    }
    setFolded((f) => {
      const next = new Set(f);
      if (next.has(c.id)) next.delete(c.id);
      else next.add(c.id);
      return next;
    });
  };

  return (
    <div
      ref={rootRef}
      data-help="calendar-runway"
      className={cn("flex min-h-0 flex-1 flex-col overflow-y-auto", drag.dragging && "select-none")}
    >
      <RunwayStrip
        model={model}
        onAddTask={() => setEditing({ defaults: newTaskDefaults(today) })}
        onAddWindow={() => setWindowEdit({ window: null })}
      />
      {error && (
        <div className="px-4 pt-2">
          <Callout as="p">{error}</Callout>
        </div>
      )}
      <div
        ref={gridRef}
        className={cn("flex gap-3 p-3", narrow ? "flex-col" : "flex-wrap items-start")}
      >
        {columns.map((c) => (
          <ColumnView
            key={c.id}
            column={c}
            lanes={model.lanes[c.id] ?? { foreground: [], background: [] }}
            folded={isFolded(c)}
            narrow={narrow}
            onToggle={() => toggleFold(c)}
            onEditWindow={
              c.windowId
                ? () => {
                    const w = windows.find((x) => x.id === c.windowId);
                    if (w) setWindowEdit({ window: w });
                  }
                : undefined
            }
            ctx={ctx}
          />
        ))}
      </div>
      <ParkedShelf cards={model.parked} ctx={ctx} />
      <TaskEditor
        open={editing !== null}
        task={editing && "task" in editing ? editing.task : null}
        defaults={editing && "defaults" in editing ? editing.defaults : undefined}
        onClose={() => setEditing(null)}
      />
      <WindowDialog
        open={windowEdit !== null}
        window={windowEdit?.window}
        today={today}
        windows={windows}
        onWindowsChange={onWindowsChange}
        onClose={() => setWindowEdit(null)}
      />
    </div>
  );
}

/** How many cards a lane pair holds, counting a stack's tasks. */
function cardCount(lanes: { foreground: RunwayCard[]; background: RunwayCard[] }): number {
  let n = 0;
  for (const c of [...lanes.foreground, ...lanes.background])
    n += c.kind === "stack" ? c.tasks.length : 1;
  return n;
}

function ColumnView({
  column: c,
  lanes,
  folded,
  narrow,
  onToggle,
  onEditWindow,
  ctx,
}: {
  column: RunwayColumn;
  lanes: { foreground: RunwayCard[]; background: RunwayCard[] };
  folded: boolean;
  narrow: boolean;
  onToggle: () => void;
  onEditWindow?: () => void;
  ctx: CardCtx;
}) {
  const headId = useId();
  const drop = isDropColumn(c);
  const count = cardCount(lanes);
  if (folded) {
    // A folded column is a rail (a bar when the columns are stacked); it still takes a drop, into
    // its foreground lane.
    return (
      <section
        aria-label={c.label}
        data-runway-zone={drop ? `${c.id}:foreground` : undefined}
        data-runway-col={drop ? c.id : undefined}
        data-runway-lane={drop ? "foreground" : undefined}
        className={cn(
          "flex rounded-[var(--radius-sm)] border bg-panel",
          narrow ? "w-full" : "w-9 flex-none self-stretch",
          ctx.drag.over?.zone === `${c.id}:foreground` ? "border-accent" : "border-rule",
        )}
      >
        <button
          type="button"
          aria-expanded={false}
          aria-label={`Show ${c.label}${count > 0 ? `, ${count}` : ""}`}
          title={`${c.label} · ${c.sub}`}
          onClick={onToggle}
          className={cn(
            "flex flex-1 items-center gap-2 px-2 py-2 text-xs hover:bg-surface",
            c.today ? "text-accent-text" : "text-ink3",
          )}
          style={narrow ? undefined : { writingMode: "vertical-rl" }}
        >
          <span className="font-head">{c.label}</span>
          {count > 0 && <span className="font-mono text-ink4">{count}</span>}
        </button>
      </section>
    );
  }
  return (
    <section
      aria-labelledby={headId}
      className={cn(
        "flex min-w-0 flex-col rounded-[var(--radius-sm)] border bg-panel",
        narrow ? "w-full" : "min-w-[15rem] flex-1 basis-60",
        c.today ? "border-accent" : "border-rule",
      )}
    >
      <div className="flex items-start gap-1 border-b border-rule px-2 py-1.5">
        <div className="min-w-0 flex-1">
          <h3
            id={headId}
            className={cn("truncate font-head text-sm", c.today ? "text-accent-text" : "text-ink")}
          >
            {c.label}
          </h3>
          <p className="font-mono text-[0.6875rem] text-ink4">{c.sub}</p>
        </div>
        {onEditWindow && (
          <Button
            variant="tertiary"
            size="xs"
            onClick={onEditWindow}
            aria-label={`Edit the window ${c.label}`}
            title="Rename, change its days or delete it"
          >
            ✎
          </Button>
        )}
        <Button
          variant="tertiary"
          size="xs"
          aria-expanded
          aria-label={`Fold ${c.label}`}
          title="Fold this column"
          onClick={onToggle}
        >
          ‹
        </Button>
      </div>
      <EventsStrip
        strip={ctx.strips[c.id]}
        column={c}
        now={ctx.now}
        colorOf={ctx.colorOf}
        onEventClick={ctx.onEventClick}
        onShowList={ctx.onShowList}
      />
      {LANES.map(({ lane, label }) => (
        <LaneView
          key={lane}
          column={c}
          lane={lane}
          label={label}
          cards={lanes[lane]}
          drop={drop}
          ctx={ctx}
        />
      ))}
    </section>
  );
}

function LaneView({
  column: c,
  lane,
  label,
  cards,
  drop,
  ctx,
}: {
  column: RunwayColumn;
  lane: TaskLane;
  label: string;
  cards: RunwayCard[];
  drop: boolean;
  ctx: CardCtx;
}) {
  const zone = `${c.id}:${lane}`;
  const over = ctx.drag.over?.zone === zone ? ctx.drag.over.index : -1;
  return (
    <div
      data-runway-zone={drop ? zone : undefined}
      data-runway-col={drop ? c.id : undefined}
      data-runway-lane={drop ? lane : undefined}
      className={cn(
        "flex flex-col gap-1 px-2 pb-2 pt-1",
        lane === "background" && "border-t border-dashed border-rule",
        over >= 0 && "bg-surface",
      )}
    >
      <p className="font-mono text-[0.625rem] uppercase tracking-wide text-ink4">{label}</p>
      <CardList cards={cards} zone={zone} column={c} over={over} ctx={ctx} />
    </div>
  );
}

/** The insertion line shown where a drag would land. */
function GhostLine() {
  return <li aria-hidden className="h-0.5 rounded-full bg-accent" data-runway-ghost="" />;
}

/** In a grid, where a drop lands beside a card: on its leading or trailing edge. */
type GhostMark = "before" | "after" | null;

/** A grid card's edge mark: a shadow, so it moves nothing. */
const MARK_CLASS: Record<"before" | "after", string> = {
  before: "shadow-[-4px_0_0_0_var(--accent)]",
  after: "shadow-[4px_0_0_0_var(--accent)]",
};

/** A zone's cards, with the insertion line drawn before the card at `over` (counted among the
 *  cards that aren't being dragged). In a grid (the parked shelf) a line in the flow would take a
 *  cell of its own and push the cards after it along, under the pointer that measures them, so
 *  there the card the drop lands beside is marked on its edge instead. */
function CardList({
  cards,
  zone,
  column,
  over,
  ctx,
  grid = false,
}: {
  cards: RunwayCard[];
  zone: string;
  column: RunwayColumn | null;
  over: number;
  ctx: CardCtx;
  grid?: boolean;
}) {
  const dragKey = ctx.drag.dragging?.key ?? null;
  const out: ReactNode[] = [];
  const staying = cards.filter((c) => cardKey(c, zone) !== dragKey).length;
  let i = 0;
  for (const card of cards) {
    const key = cardKey(card, zone);
    const moving = key === dragKey;
    let mark: GhostMark = null;
    if (!moving) {
      if (i === over) {
        if (grid) mark = "before";
        else out.push(<GhostLine key="ghost" />);
      } else if (grid && over >= staying && i === staying - 1) {
        mark = "after";
      }
      i += 1;
    }
    out.push(
      <CardView
        key={key}
        card={card}
        zone={zone}
        column={column}
        laneCards={cards}
        mark={mark}
        ctx={ctx}
      />,
    );
  }
  if (over >= i && over >= 0 && (!grid || staying === 0)) out.push(<GhostLine key="ghost" />);
  if (cards.length === 0 && over < 0) {
    return <p className="text-xs text-ink4">empty</p>;
  }
  return (
    <ul
      className={cn(
        grid ? "grid grid-cols-[repeat(auto-fill,minmax(14rem,1fr))] gap-2" : "flex flex-col gap-1",
      )}
    >
      {out}
    </ul>
  );
}

function cardKey(card: RunwayCard, zone: string): string {
  if (card.kind === "task") return `task:${card.task.id}`;
  if (card.kind === "milestone") return `milestone:${card.milestone.id}`;
  return `stack:${card.stack.id}:${zone}`;
}

function CardView({
  card,
  zone,
  column,
  laneCards,
  mark,
  ctx,
}: {
  card: RunwayCard;
  zone: string;
  column: RunwayColumn | null;
  laneCards: readonly RunwayCard[];
  mark: GhostMark;
  ctx: CardCtx;
}) {
  if (card.kind === "milestone") return <MilestoneCardView card={card} zone={zone} ctx={ctx} />;
  if (card.kind === "stack")
    return (
      <StackCardView card={card} zone={zone} column={column} laneCards={laneCards} ctx={ctx} />
    );
  return (
    <TaskCardView
      card={card}
      zone={zone}
      column={column}
      laneCards={laneCards}
      mark={mark}
      ctx={ctx}
    />
  );
}

/** A task's card. `zone` null: a stack's member, drawn inside the stack's card, with `place`
 *  saying where it is planned. */
function TaskCardView({
  card,
  zone,
  column,
  laneCards,
  mark = null,
  place,
  ctx,
}: {
  card: RunwayTaskCard;
  zone: string | null;
  column: RunwayColumn | null;
  /** The cards of its lane, as drawn: its Move… menu's up and down. */
  laneCards?: readonly RunwayCard[];
  mark?: GhostMark;
  place?: string;
  ctx: CardCtx;
}) {
  const { showMeta } = useDepth();
  const [whyOpen, setWhyOpen] = useState(false);
  const t = card.task;
  const key = `task:${t.id}`;
  const done = t.status === "done";
  const stack = t.stack_id !== null && zone !== null ? ctx.stackById.get(t.stack_id) : undefined;
  const ev = linkedEvent(ctx.events, t.event_uid, ctx.today);
  const due = linkedDue(t);
  const rolled = rolledWords(card);
  const titleCls = cn(
    "min-w-0 break-words text-left text-xs text-ink",
    done && "line-through opacity-60",
  );
  return (
    <li
      data-runway-card={zone ?? undefined}
      data-runway-key={key}
      data-runway-mark={mark ?? undefined}
      onPointerDown={(e) => ctx.drag.press(e, { key, taskIds: [t.id] })}
      className={cn(
        "flex cursor-grab touch-none items-start gap-1.5 rounded-[var(--radius-sm)] border border-border bg-surface px-1.5 py-1",
        ctx.drag.dragging?.key === key && "opacity-40",
        mark && MARK_CLASS[mark],
      )}
    >
      <TaskStatusButton status={t.status} onCycle={(next) => ctx.cycle(t, next)} />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5 py-0.5">
        {t.why && showMeta ? (
          <button
            type="button"
            aria-expanded={whyOpen}
            title={whyOpen ? "Hide the note" : "Show why"}
            onClick={() => setWhyOpen(!whyOpen)}
            className={cn(titleCls, "hover:text-accent-text")}
          >
            {t.title}
          </button>
        ) : (
          <span className={titleCls}>{t.title}</span>
        )}
        {whyOpen && t.why && (
          <p className="whitespace-pre-wrap break-words text-xs text-ink3">{t.why}</p>
        )}
        <div className="flex min-w-0 flex-wrap items-center gap-1">
          {place !== undefined && (
            <span className="shrink-0 font-mono text-[0.6875rem] text-ink3">{place}</span>
          )}
          <DeadlineChip task={t} today={ctx.today} />
          {rolled && (
            <span className="shrink-0 font-mono text-[0.6875rem] text-st-look">{rolled}</span>
          )}
          {t.status === "doing" && (
            <span className="shrink-0 font-mono text-[0.6875rem] text-accent-text">doing</span>
          )}
          {t.project_name && <ProjectChip project={t.project_name} onOpen={ctx.onOpenProject} />}
          {stack && <StackChip stack={stack} />}
          {ev && (
            <button
              type="button"
              className="shrink-0 truncate font-mono text-[0.6875rem] text-ink3 hover:text-ink"
              title={ev.summary}
              aria-label={`Open the event ${ev.summary}`}
              onClick={(e) => ctx.onEventClick?.(ev, e.currentTarget.getBoundingClientRect())}
            >
              📅 {eventWhen(ev)}
            </button>
          )}
        </div>
        {due && showMeta && <p className="truncate text-[0.6875rem] text-ink4">{due}</p>}
      </div>
      <div className="flex shrink-0 items-center">
        <Button
          variant="tertiary"
          size="xs"
          onClick={() => ctx.edit(t)}
          title="Edit task"
          aria-label={`Edit ${t.title}`}
        >
          ✎
        </Button>
        <MoveMenu
          name={t.title}
          tasks={[t]}
          columns={ctx.columns}
          here={t.parked ? null : (column?.id ?? null)}
          onReplan={(planFor, where) => ctx.replan([t], planFor, where)}
          laneCards={laneCards}
          onReorder={ctx.reorder}
        />
      </div>
    </li>
  );
}

function MilestoneCardView({
  card,
  zone,
  ctx,
}: {
  card: RunwayMilestoneCard;
  zone: string;
  ctx: CardCtx;
}) {
  const m = card.milestone;
  const met = m.state === "met" || m.status === "done";
  // Its deadline, toned like a task's, wherever it sits (a met one reads plain).
  const info = milestoneDue(m, ctx.today);
  const words = `◆ ${m.label} · ${m.project_name}`;
  return (
    <li
      data-runway-card={zone}
      data-runway-key={`milestone:${m.id}`}
      className="flex items-center gap-1.5 rounded-[var(--radius-sm)] border border-dashed border-border px-1.5 py-1"
    >
      <button
        type="button"
        onClick={() => ctx.onOpenProject?.(m.project_name)}
        title={`Open ${m.project_name}`}
        className={cn(
          "min-w-0 flex-1 break-words text-left text-xs text-ink2 hover:text-ink",
          met && "line-through opacity-60",
        )}
      >
        {words}
      </button>
      {info && (
        <span className={cn("shrink-0 font-mono text-[0.6875rem]", TONE_CLASS[info.tone])}>
          {info.text}
        </span>
      )}
    </li>
  );
}

function StackCardView({
  card,
  zone,
  column,
  laneCards,
  ctx,
}: {
  card: RunwayStackCard;
  zone: string;
  column: RunwayColumn | null;
  laneCards: readonly RunwayCard[];
  ctx: CardCtx;
}) {
  const { showMeta } = useDepth();
  const [open, setOpen] = useState(false);
  const key = `stack:${card.stack.id}:${zone}`;
  const title = card.stack.title || STACK_KIND_LABEL[card.stack.kind];
  const nextTask = card.next;
  // The whole stack's earliest open deadline, shown on its chip.
  const soonest = stackSoonest(card);
  const item: RunwayDragItem = { key, taskIds: card.tasks.map((t) => t.id) };
  return (
    <li
      data-runway-card={zone}
      data-runway-key={key}
      onPointerDown={open ? undefined : (e) => ctx.drag.press(e, item)}
      className={cn(
        "rounded-[var(--radius-sm)] border border-border bg-surface shadow-[3px_3px_0_0_var(--border)]",
        !open && "cursor-grab touch-none",
        ctx.drag.dragging?.key === key && "opacity-40",
      )}
    >
      <div className="flex items-start gap-1.5 px-1.5 py-1">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          title={open ? "Close the stack" : "Open the stack"}
          className="flex min-w-0 flex-1 flex-col items-start gap-0.5 py-0.5 text-left"
        >
          <span className="break-words text-xs text-ink">
            ≡ {title}
            <span className="font-mono text-[0.6875rem] text-ink4"> · {card.left} left</span>
          </span>
          {nextTask && showMeta && (
            <span className="w-full truncate text-[0.6875rem] text-ink3">
              Next: {nextTask.title}
            </span>
          )}
        </button>
        {soonest && <DeadlineChip task={soonest} today={ctx.today} className="pt-1" />}
        <MoveMenu
          name={`the stack ${title}`}
          tasks={card.tasks}
          columns={ctx.columns}
          here={column?.id ?? null}
          onReplan={(planFor, where) => ctx.replan(card.tasks, planFor, where)}
          laneCards={laneCards}
          onReorder={ctx.reorder}
        />
      </div>
      {open && (
        <div className="flex flex-col gap-1 border-t border-rule p-1.5">
          {card.stack.note && (
            <p className="whitespace-pre-wrap break-words text-xs text-ink3">{card.stack.note}</p>
          )}
          {/* The whole stack, each task saying where it is planned. */}
          <ul className="flex flex-col gap-1" aria-label={`The stack ${title}`}>
            {card.chain.map((m) => {
              const col =
                m.columnId === null ? null : (ctx.columns.find((c) => c.id === m.columnId) ?? null);
              return (
                <TaskCardView
                  key={m.card.task.id}
                  card={m.card}
                  zone={null}
                  column={col}
                  place={memberPlace(ctx.columns, m.columnId)}
                  ctx={ctx}
                />
              );
            })}
          </ul>
        </div>
      )}
    </li>
  );
}

/** A milestone's deadline as its card shows it: toned late or soon, plain once it's met. */
function milestoneDue(m: Milestone, today: Date) {
  const met = m.state === "met" || m.status === "done";
  return m.due_date ? deadlineInfo(m.due_date.slice(0, 10), "hard", today, met) : null;
}

/** A stack's earliest open deadline, over the whole stack. */
function stackSoonest(card: RunwayStackCard): Task | undefined {
  return card.chain
    .map((m) => m.card.task)
    .filter((t) => t.status !== "done" && t.effective_deadline)
    .sort((a, b) => (a.effective_deadline ?? "").localeCompare(b.effective_deadline ?? ""))[0];
}

function ParkedShelf({ cards, ctx }: { cards: RunwayTaskCard[]; ctx: CardCtx }) {
  const over = ctx.drag.over?.zone === "parked" ? ctx.drag.over.index : -1;
  return (
    <section
      aria-label="Parked"
      data-runway-zone="parked"
      data-runway-parked=""
      data-runway-grid=""
      className={cn(
        "mx-3 mb-3 flex flex-col gap-2 rounded-[var(--radius-sm)] border border-dashed border-rule p-2",
        over >= 0 && "bg-surface",
      )}
    >
      <SectionLabel as="h3">Parked</SectionLabel>
      {cards.length === 0 && over < 0 ? (
        <p className="text-xs text-ink4">Nothing parked. Drop a task here to set it aside.</p>
      ) : (
        <CardList cards={cards} zone="parked" column={null} over={over} ctx={ctx} grid />
      )}
    </section>
  );
}
