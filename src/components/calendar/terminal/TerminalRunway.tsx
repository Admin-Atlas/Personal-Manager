// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Terminal system's Runway (PA-2): the same columns, lanes and cards as `RunwayBoard`, drawn as
// mono text blocks, never a pixel board. Each column is a `── Today ──` rule with its days beside
// it, then `fg` and `bg` sub-heads over `[ ]` / `[~]` / `[x]` rows, each deadline in its tone.
// Green (the accent) is on today alone. Any column's rule folds it. Under each rule from today on,
// the column's events, read-only. There's no pointer drag here (the Terminal rule: nothing is
// dragged in this look); a task moves, and moves up or down its lane, from its Move… menu, the same
// one the board's cards have.

import { useState } from "react";
import {
  readRunwayPastOpen,
  writeRunwayPastOpen,
  type RunwayWindow,
} from "../../../lib/calendarPrefs";
import {
  linkedEvent,
  type RunwayCard,
  type RunwayColumn,
  type RunwayTaskCard,
  type TaskPlan,
} from "../../../lib/runway";
import { deadlineInfo, STACK_KIND_LABEL, TONE_CLASS } from "../../../lib/tasks";
import type { CalendarEvent, Task, TaskInput, TaskLane } from "../../../lib/types";
import { useDepth } from "../../../theme";
import { DeadlineChip, TaskStatusButton } from "../../tasks/TaskBits";
import { TaskEditor } from "../../tasks/TaskEditor";
import { Button, Callout, cn } from "../../ui";
import {
  eventWhen,
  linkedDue,
  memberPlace,
  newTaskDefaults,
  rolledWords,
  useRunwayActions,
  useRunwayModel,
  type RunwayProps,
} from "../views/runwayLogic";
import { EventsStrip, MoveMenu, RunwayStrip, WindowDialog } from "../views/runwayParts";

type Editing = { task: Task } | { defaults: Partial<TaskInput> } | null;

interface RowCtx {
  today: Date;
  columns: RunwayColumn[];
  events: CalendarEvent[];
  cycle: (task: Task, next: Task["status"]) => void;
  replan: (tasks: readonly Task[], planFor: (t: Task) => TaskPlan | null, where: string) => void;
  reorder: (order: number[], said: string) => void;
  edit: (task: Task) => void;
  onOpenProject?: (project: string) => void;
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
}

const LANES: { lane: TaskLane; label: string }[] = [
  { lane: "foreground", label: "fg" },
  { lane: "background", label: "bg" },
];

export function TerminalRunway(props: RunwayProps) {
  const {
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
  const { today, columns, model, strips, showEvents, toggleEvents } = useRunwayModel(props);
  const { error, cycle, replan, reorder } = useRunwayActions(announce);
  const [editing, setEditing] = useState<Editing>(null);
  const [windowEdit, setWindowEdit] = useState<{ window: RunwayWindow | null } | null>(null);
  const [pastOpen, setPastOpen] = useState(readRunwayPastOpen);
  // Any column folds, as on the board: Before today remembered, the rest for this visit.
  const [folded, setFolded] = useState<Set<string>>(() => new Set());
  const isOpenCol = (c: RunwayColumn) => (c.kind === "past" ? pastOpen : !folded.has(c.id));
  const toggle = (c: RunwayColumn) => {
    if (c.kind === "past") {
      writeRunwayPastOpen(!pastOpen);
      setPastOpen(!pastOpen);
      return;
    }
    setFolded((f) => {
      const next = new Set(f);
      if (next.has(c.id)) next.delete(c.id);
      else next.add(c.id);
      return next;
    });
  };

  const ctx: RowCtx = {
    today,
    columns,
    events,
    cycle,
    replan: (ts, planFor, where) => replan(ts, planFor, where),
    reorder,
    edit: (task) => setEditing({ task }),
    onOpenProject,
    onEventClick,
  };

  return (
    <div data-help="calendar-runway" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <RunwayStrip
        model={model}
        terminal
        onAddTask={() => setEditing({ defaults: newTaskDefaults(today) })}
        onAddWindow={() => setWindowEdit({ window: null })}
        showEvents={showEvents}
        onToggleEvents={toggleEvents}
      />
      {error && (
        <div className="px-4 pt-2">
          <Callout as="p">{error}</Callout>
        </div>
      )}
      <div className="flex flex-col gap-3 px-4 py-2 font-mono text-xs">
        {columns.map((c) => {
          const lanes = model.lanes[c.id] ?? { foreground: [], background: [] };
          const open = isOpenCol(c);
          const count = lanes.foreground.length + lanes.background.length;
          return (
            <section key={c.id} aria-label={c.label}>
              <div className="flex items-baseline gap-2">
                <button
                  type="button"
                  aria-expanded={open}
                  title={open ? "Fold this column" : "Show this column"}
                  onClick={() => toggle(c)}
                  className={cn(
                    "hover:text-ink",
                    c.today ? "text-accent-text" : c.kind === "past" ? "text-ink3" : "text-ink2",
                  )}
                >
                  ── {c.label} ({count}) {open ? "▾" : "▸"} ──
                </button>
                <span className={c.today ? "text-accent-text" : "text-ink4"}>{c.sub}</span>
                {c.windowId && (
                  <Button
                    variant="tertiary"
                    size="xs"
                    className="font-mono"
                    aria-label={`Edit the window ${c.label}`}
                    onClick={() => {
                      const w = windows.find((x) => x.id === c.windowId);
                      if (w) setWindowEdit({ window: w });
                    }}
                  >
                    edit
                  </Button>
                )}
              </div>
              {open && (
                <EventsStrip
                  strip={strips[c.id]}
                  column={c}
                  now={now}
                  colorOf={colorOf}
                  onEventClick={onEventClick}
                  onShowList={onShowList}
                  terminal
                />
              )}
              {open &&
                LANES.map(({ lane, label }) => (
                  <div key={lane} className="pl-2">
                    <p className="text-ink4">{label}</p>
                    {lanes[lane].length === 0 ? (
                      <p className="pl-3 text-ink4">empty</p>
                    ) : (
                      <ul className="flex flex-col">
                        {lanes[lane].map((card) => (
                          <Row
                            key={rowKey(card)}
                            card={card}
                            column={c}
                            laneCards={lanes[lane]}
                            ctx={ctx}
                          />
                        ))}
                      </ul>
                    )}
                  </div>
                ))}
            </section>
          );
        })}
        <section aria-label="Parked">
          <p className="text-ink2">── Parked ──</p>
          {model.parked.length === 0 ? (
            <p className="pl-3 text-ink4">nothing parked</p>
          ) : (
            <ul className="flex flex-col pl-2">
              {model.parked.map((card) => (
                <TaskRow
                  key={card.task.id}
                  card={card}
                  column={null}
                  laneCards={model.parked}
                  ctx={ctx}
                />
              ))}
            </ul>
          )}
        </section>
      </div>
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

function rowKey(card: RunwayCard): string {
  if (card.kind === "task") return `task:${card.task.id}`;
  if (card.kind === "milestone") return `milestone:${card.milestone.id}`;
  return `stack:${card.stack.id}`;
}

function Row({
  card,
  column,
  laneCards,
  ctx,
}: {
  card: RunwayCard;
  column: RunwayColumn | null;
  laneCards: readonly RunwayCard[];
  ctx: RowCtx;
}) {
  if (card.kind === "task")
    return <TaskRow card={card} column={column} laneCards={laneCards} ctx={ctx} />;
  if (card.kind === "milestone") {
    const m = card.milestone;
    const met = m.state === "met" || m.status === "done";
    // Its deadline in its tone wherever it sits (plain once met), as a task's is.
    const info = m.due_date ? deadlineInfo(m.due_date.slice(0, 10), "hard", ctx.today, met) : null;
    return (
      <li className="flex items-baseline gap-2 pl-3">
        <button
          type="button"
          onClick={() => ctx.onOpenProject?.(m.project_name)}
          title={`Open ${m.project_name}`}
          className={cn("min-w-0 text-left text-ink2 hover:text-ink", met && "line-through")}
        >
          ◆ {m.label} · {m.project_name}
        </button>
        {info && <span className={TONE_CLASS[info.tone]}>{info.text}</span>}
      </li>
    );
  }
  return <StackRow card={card} column={column} laneCards={laneCards} ctx={ctx} />;
}

function StackRow({
  card,
  column,
  laneCards,
  ctx,
}: {
  card: Extract<RunwayCard, { kind: "stack" }>;
  column: RunwayColumn | null;
  laneCards: readonly RunwayCard[];
  ctx: RowCtx;
}) {
  const { showMeta } = useDepth();
  const [open, setOpen] = useState(false);
  const title = card.stack.title || STACK_KIND_LABEL[card.stack.kind];
  return (
    <li className="flex flex-col pl-3">
      <div className="flex items-baseline gap-2">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="min-w-0 text-left text-ink2 hover:text-ink"
        >
          ≡ {title} ({card.left} left) {open ? "▾" : "▸"}
        </button>
        {card.next && showMeta && <span className="text-ink3">next: {card.next.title}</span>}
        <MoveMenu
          name={`the stack ${title}`}
          tasks={card.tasks}
          columns={ctx.columns}
          here={column?.id ?? null}
          onReplan={(planFor, where) => ctx.replan(card.tasks, planFor, where)}
          laneCards={laneCards}
          onReorder={ctx.reorder}
          terminal
        />
      </div>
      {open && (
        <div className="flex flex-col pl-3">
          {card.stack.note && <p className="whitespace-pre-wrap text-ink3">{card.stack.note}</p>}
          {/* The whole stack, each task saying where it is planned. */}
          <ul className="flex flex-col" aria-label={`The stack ${title}`}>
            {card.chain.map((m) => (
              <TaskRow
                key={m.card.task.id}
                card={m.card}
                column={
                  m.columnId === null
                    ? null
                    : (ctx.columns.find((c) => c.id === m.columnId) ?? null)
                }
                place={memberPlace(ctx.columns, m.columnId)}
                ctx={ctx}
              />
            ))}
          </ul>
        </div>
      )}
    </li>
  );
}

function TaskRow({
  card,
  column,
  laneCards,
  place,
  ctx,
}: {
  card: RunwayTaskCard;
  column: RunwayColumn | null;
  /** Its lane's rows, as drawn: its Move… menu's up and down. */
  laneCards?: readonly RunwayCard[];
  /** In an opened stack: where it is planned. */
  place?: string;
  ctx: RowCtx;
}) {
  const { showMeta } = useDepth();
  const [whyOpen, setWhyOpen] = useState(false);
  const t = card.task;
  const ev = linkedEvent(
    ctx.events,
    t.event_uid,
    ctx.today,
    t.deadline_from === "event" ? t.effective_deadline : null,
  );
  const due = linkedDue(t);
  const rolled = rolledWords(card);
  const titleCls = cn(
    "min-w-0 text-left",
    t.status === "done" ? "text-ink3 line-through" : "text-ink",
  );
  return (
    <li className="flex flex-col">
      <div className="flex flex-wrap items-baseline gap-x-2">
        <TaskStatusButton status={t.status} onCycle={(next) => ctx.cycle(t, next)} />
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
        {place !== undefined && <span className="text-ink3">{place}</span>}
        <DeadlineChip task={t} today={ctx.today} />
        {rolled && <span className="text-st-look">{rolled}</span>}
        {t.project_name && (
          <button
            type="button"
            className="text-ink3 hover:text-ink"
            title={`Open ${t.project_name}`}
            onClick={() => ctx.onOpenProject?.(t.project_name!)}
          >
            {t.project_name}
          </button>
        )}
        {ev && (
          <button
            type="button"
            className="text-ink3 hover:text-ink"
            title={ev.summary}
            aria-label={`Open the event ${ev.summary}`}
            onClick={(e) => ctx.onEventClick?.(ev, e.currentTarget.getBoundingClientRect())}
          >
            @ {eventWhen(ev)}
          </button>
        )}
        <Button
          variant="tertiary"
          size="xs"
          className="font-mono"
          onClick={() => ctx.edit(t)}
          aria-label={`Edit ${t.title}`}
          title="Edit task"
        >
          edit
        </Button>
        <MoveMenu
          name={t.title}
          tasks={[t]}
          columns={ctx.columns}
          here={t.parked ? null : (column?.id ?? null)}
          onReplan={(planFor, where) => ctx.replan([t], planFor, where)}
          laneCards={laneCards}
          onReorder={ctx.reorder}
          terminal
        />
      </div>
      {whyOpen && t.why && <p className="whitespace-pre-wrap pl-8 text-ink3">{t.why}</p>}
      {due && showMeta && <p className="pl-8 text-ink4">{due}</p>}
    </li>
  );
}
