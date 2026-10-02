// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from "react";

import { formatBytes } from "../../lib/format";
import type { LocalRole } from "../../lib/localModelState";
import type { LocalBetterFit, LocalRecommendations } from "../../lib/types";
import { runnerGuides, type RunnerName } from "../../lib/workbenchGuide";
import { useDepth } from "../../theme";
import { IngestProgress } from "../IngestProgress";
import { withCode } from "../withCode";
import { CommandLine } from "./CommandLine";
import { FitBadge, TokenChip } from "./fitDisplay";
import { useLocate } from "./locate";
import {
  alsoHaveLine,
  diskLine,
  eyebrow,
  facts,
  inUseLine,
  nothingSentence,
  pickInUse,
  settingsLine,
  why,
  type ShownPick,
} from "./pickWords";
import {
  overall,
  primaryOf,
  rightNow,
  runnerOf,
  shownPick,
  standing,
  type AssignPlan,
  type ReadinessInput,
  type Step,
  type StepAction,
  type StepState,
} from "./readiness";
import { sectionHelp, sectionLabel } from "./sections";
import { speedLong } from "./speedWords";
import type { ModelPull } from "./usePull";
import {
  Button,
  Callout,
  Card,
  cn,
  SectionInfo,
  SectionLabel,
  SegmentedControl,
  SettingRow,
  type ButtonVariant,
} from "../ui";

/** What the start card's buttons do. Each is the same write a section control makes. */
export interface StartActions {
  connect: (url: string) => void;
  /** The address a Connect is running for, or null. */
  connecting: string | null;
  assign: (plan: AssignPlan) => void;
  assigning: boolean;
  test: (role: LocalRole) => void;
  detect: () => void;
  detecting: boolean;
  release: () => void;
  releasing: boolean;
  rescan: () => void;
  rescanning: boolean;
}

/**
 * "Your local model" — where the user stands, the model PM would run here, and the four steps to get
 * it going: a model server, the model, putting it to work, and checking it answers. For someone
 * already set up, what the server holds right now and what On battery does.
 *
 * Every word comes from `readiness.ts` and `pickWords.ts`, pure readings of the tab's state; this
 * renders them. Only the step to do next has a primary button, and every button makes a write some
 * section further down also makes — the card is a way through the tab, not a second copy of it.
 */
export function LocalAiStart({
  input,
  steps: list,
  betterFit,
  onDismissBetterFit,
  error,
  pull,
  actions,
}: {
  input: ReadinessInput;
  /** `steps(input)`, worked out once by the tab — All models reads step 2 too. */
  steps: Step[];
  betterFit: LocalBetterFit | null;
  onDismissBetterFit: () => void;
  /** Something the start card asked for went wrong, said here. */
  error?: string | null;
  pull: ModelPull;
  actions: StartActions;
}) {
  const locate = useLocate();
  const chip = overall(list, input);
  const pick = input.recs?.pick;
  const isPick = !!betterFit && !!pick && pick.kind !== "nothing" && betterFit.repo === pick.repo;
  const now = list[2].state === "done" ? rightNow(input) : null;
  const pickBlock = <PickCard input={input} steps={list} betterFit={betterFit} actions={actions} />;
  const hasPickBlock = input.recsLoading || !input.recs || !!input.recs.pick;

  return (
    <div
      id="sec-localai-start"
      data-settings-section
      data-help={sectionHelp("sec-localai-start")}
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel action={chip && <TokenChip token={chip.token}>{chip.label}</TokenChip>}>
        {sectionLabel("sec-localai-start")}
      </SectionLabel>
      {error && <Callout className="mt-2">{error}</Callout>}

      {/* Polite: it changes as the setup does (a server found, a model loaded), and someone who is
          working through the steps wants to hear that without being interrupted for it. */}
      <p aria-live="polite" className="mt-1.5 text-sm text-ink2">
        {standing(input)}
      </p>

      {/* A better-fitting model is available (#437) — the quiet counterpart to the dots on the
          sidebar and the settings nav, and the thing they lead to. Never a modal, never a gate:
          dismissing it is always enough. */}
      {betterFit && (
        <Callout tone="info" body="ink" live className="mt-3 flex flex-wrap items-center gap-2">
          <span className="min-w-0 flex-1 text-ink2">
            <span className="text-ink">{betterFit.display_name}</span>{" "}
            {betterFit.already_downloaded
              ? "is already on this device and fits your machine better than"
              : "would fit your machine better than"}{" "}
            {betterFit.replaces}.{isPick ? " It's PM's pick for this computer." : ""}
          </span>
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => locate?.(isPick ? "sec-localai-start" : `rec:${betterFit.repo}`)}
          >
            Show me
          </Button>
          <Button variant="tertiary" size="sm" onClick={onDismissBetterFit}>
            Dismiss
          </Button>
        </Callout>
      )}

      <Card className="mt-3 p-4">
        {pickBlock}
        {hasPickBlock && <div className="my-3 border-t border-border" />}
        <ol aria-label="Steps to get it running" className="space-y-3">
          {list.map((s) => (
            <StepRow key={s.id} step={s} input={input} pull={pull} actions={actions} />
          ))}
        </ol>
      </Card>

      {/* Never folded and never Depth-gated: what the server holds is the number someone who is
          already set up came here for. */}
      {now && (now.holding || now.battery) && (
        <div className="mt-3 space-y-1.5 text-xs text-ink3">
          {now.holding && (
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <span>
                {now.holding}
                {now.pmLoaded === true && " PM loaded it, so it can hand it back."}
                {now.pmLoaded === false && " PM didn't load it, so it leaves it alone."}
              </span>
              {now.pmLoaded === true && (
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={actions.release}
                  disabled={actions.releasing}
                >
                  {actions.releasing ? "Freeing…" : "Free it now"}
                </Button>
              )}
            </div>
          )}
          {now.battery && (
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <span>{now.battery}</span>
              <Button variant="tertiary" size="sm" onClick={() => locate?.("sec-localai-power")}>
                Battery options
              </Button>
            </div>
          )}
        </div>
      )}

      <SectionInfo title="How PM picks">
        <p>
          PM looks for the largest model in its list that runs entirely on your graphics card with
          the room PM keeps free, at the context PM sizes it for — 32k tokens, less for a model made
          for less, and more for one your server already runs with more — because a model that
          spills into system memory replies many times slower. On a computer without a separate
          graphics card, it only considers models its cautious estimate says are quick enough for
          PM's background work. If you already have a model that fits that way and nothing in the
          list is at least 15% larger, PM points at the one you have. It never downloads or switches
          anything for you.
        </p>
      </SectionInfo>
    </div>
  );
}

/** PM's pick, or why there isn't one. Nothing at all for a payload with no pick. */
function PickCard({
  input,
  steps: list,
  betterFit,
  actions,
}: {
  input: ReadinessInput;
  steps: Step[];
  betterFit: LocalBetterFit | null;
  actions: StartActions;
}) {
  const { showMeta, showPower } = useDepth();
  const { recs } = input;
  const checkAgain = (
    <div className="mt-1.5">
      <Button variant="tertiary" size="sm" onClick={actions.rescan} disabled={actions.rescanning}>
        {actions.rescanning ? "Checking…" : "Check again"}
      </Button>
    </div>
  );
  if (input.recsLoading) {
    return <p className="text-xs text-ink4">Sizing PM's models against your machine…</p>;
  }
  if (!recs) {
    return (
      <div>
        <p className="text-xs text-ink3">
          PM couldn't read this machine's hardware, so it can't pick a model yet.
        </p>
        {checkAgain}
      </div>
    );
  }
  const pick = recs.pick;
  if (!pick) return null;
  if (pick.kind === "nothing") {
    return (
      <div>
        <p className="text-xs text-ink3">
          {nothingSentence(pick, recs, input.status?.power?.any_cloud_key ?? false)}
        </p>
        {pick.reason === "too_little_memory" && checkAgain}
      </div>
    );
  }
  return (
    <ShownPickCard
      pick={pick}
      recs={recs}
      input={input}
      steps={list}
      betterFit={betterFit}
      showMeta={showMeta}
      showPower={showPower}
    />
  );
}

function ShownPickCard({
  pick,
  recs,
  input,
  steps: list,
  betterFit,
  showMeta,
  showPower,
}: {
  pick: ShownPick;
  recs: LocalRecommendations;
  input: ReadinessInput;
  steps: Step[];
  betterFit: LocalBetterFit | null;
  showMeta: boolean;
  showPower: boolean;
}) {
  const rec = recs.curated.find((r) => r.repo === pick.repo) ?? null;
  const servedIds = new Set(input.served.map((m) => m.id.toLowerCase()));
  const inUse = pickInUse(pick, input.config, input.status, servedIds);
  const { row, params, reserve } = facts(pick, recs);
  const configured = !!input.config?.base_url;
  // Before connecting, the server step 1 offers to connect is the one PM found running.
  const offered = list[0].action?.kind === "connect" ? list[0].action.url : null;
  const settings = settingsLine(pick.fit, {
    configured,
    runner: runnerOf(configured ? input.config?.base_url : offered),
    setup: list[0].setup,
    commandShown: list[1].command !== null,
  });
  const moe = !!rec && rec.active_parameters_b + 0.01 < rec.parameters_b;
  const qualifier = speedLong(pick.fit, recs.hardware, { moe });
  // The download only matters while there is one to make: once the server has the pick, "5.1 GB
  // download" is a figure about something already done.
  const disk =
    pick.kind === "catalogue" && !servedIds.has(pick.tag.toLowerCase())
      ? diskLine(pick, recs)
      : null;
  const also = pick.kind === "catalogue" ? alsoHaveLine(pick, recs) : null;
  // The job's local model, when one runs on a model other than the pick. The better-fit line already
  // says this when it names the pick's own repo, so it isn't said twice.
  const chatLocal = input.config?.chat_routing !== "cloud" && input.config?.chat_model?.trim();
  const bound =
    (chatLocal ? input.config?.chat_model : null) ??
    (input.config?.background_routing !== "cloud" ? input.config?.background_model : null) ??
    null;
  const differs = betterFit?.repo === pick.repo ? null : inUseLine(pick, bound || null, recs);

  return (
    <div>
      <p className="text-xs text-accent-text">{eyebrow(pick, inUse)}</p>
      <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1">
        <span className="text-sm font-medium text-ink">{pick.display_name}</span>
        <FitBadge verdict={pick.fit.verdict} />
        {rec && (
          <span className="text-[0.625rem] text-ink4">
            <a
              href={rec.licence.url}
              target="_blank"
              rel="noreferrer noopener"
              className="text-ink4 underline decoration-dotted underline-offset-2"
            >
              {rec.licence.name}
            </a>
            {!rec.licence.open && " · its own terms"}
          </span>
        )}
      </div>
      {pick.kind === "owned" && pick.id !== pick.display_name && (
        <p className="mt-0.5 text-xs text-ink4">
          as <span className="break-all font-mono">{pick.id}</span>
        </p>
      )}
      <p className="mt-2 text-xs text-ink3">{why(pick)}</p>
      {/* Every figure the pick is justified by, at every Depth. Only the parameter count is extra. */}
      <p className="mt-1.5 text-xs text-ink3">
        {[...row, ...(showMeta && params ? [params] : [])].join(" · ")}
      </p>
      {showPower && <p className="mt-1 text-xs text-ink4">{reserve}</p>}
      {/* The half of the speed figure that makes it honest — never folded, never Depth-gated. */}
      {qualifier && <p className="mt-1 text-xs text-ink4">{qualifier}</p>}
      {settings && <p className="mt-1 text-xs text-ink4">{settings}</p>}
      {disk && (
        <>
          <p className="mt-1 text-xs text-ink4">{disk.line}</p>
          {disk.over && <p className="mt-1 text-xs text-st-due">{disk.over}</p>}
        </>
      )}
      {also && <p className="mt-1 text-xs text-ink4">{also}</p>}
      {differs && <p className="mt-1 text-xs text-ink4">{differs}</p>}
    </div>
  );
}

const STATE_CHIP: Record<Exclude<StepState, "waiting">, { label: string; token: string }> = {
  done: { label: "Done", token: "--st-quick" },
  next: { label: "Do this next", token: "--accent" },
  attention: { label: "Needs attention", token: "--st-due" },
  optional: { label: "Optional", token: "--ink4" },
  checking: { label: "Checking…", token: "--ink4" },
};

function StepRow({
  step,
  input,
  pull,
  actions,
}: {
  step: Step;
  input: ReadinessInput;
  pull: ModelPull;
  actions: StartActions;
}) {
  const { showMeta, showPower } = useDepth();
  const locate = useLocate();
  const next = step.state === "next";
  const chip = step.state === "waiting" ? null : STATE_CHIP[step.state];
  const primary = primaryOf(step);
  const buttons = [...(step.action ? [step.action] : []), ...step.secondary];

  function run(a: StepAction) {
    switch (a.kind) {
      case "connect":
        return actions.connect(a.url);
      case "download": {
        const rec = input.recs?.curated.find((r) => r.repo === a.repo);
        if (rec) pull.requestPull(rec, a.tag, "start");
        return;
      }
      case "assign":
        return actions.assign(a.plan);
      case "test":
        return actions.test(a.role);
      case "detect":
        return actions.detect();
      case "release":
        return actions.release();
      case "locate":
        return locate?.(a.to);
    }
  }

  function busy(a: StepAction): { label: string; disabled: boolean } {
    switch (a.kind) {
      case "connect":
        return {
          label: actions.connecting === a.url ? "Connecting…" : a.label,
          disabled: actions.connecting !== null,
        };
      case "assign":
        return { label: actions.assigning ? "Saving…" : a.label, disabled: actions.assigning };
      case "detect":
        return { label: actions.detecting ? "Looking…" : a.label, disabled: actions.detecting };
      case "release":
        return { label: actions.releasing ? "Freeing…" : a.label, disabled: actions.releasing };
      case "download":
        return { label: a.label, disabled: pull.pulling !== null };
      case "test":
        return { label: a.label, disabled: input.tests.running !== null };
      default:
        return { label: a.label, disabled: false };
    }
  }

  function variantOf(a: StepAction): ButtonVariant {
    if (a === primary) return "primary";
    return a.kind === "detect" ? "tertiary" : "secondary";
  }

  const prog = pull.pullProg;
  const pct =
    prog && prog.total_bytes
      ? Math.min(100, Math.round((100 * (prog.completed_bytes ?? 0)) / prog.total_bytes))
      : null;
  const pickName = shownPick(input.recs)?.display_name ?? "PM's pick";

  return (
    <li
      aria-current={next ? "step" : undefined}
      className={cn(next && "border-l-2 pl-3")}
      style={next ? { borderColor: "var(--accent)" } : undefined}
    >
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
        <span className="font-mono text-xs text-ink4">{step.n}</span>
        <span className="text-sm text-ink2">{step.title}</span>
        {chip && <TokenChip token={chip.token}>{chip.label}</TokenChip>}
      </div>
      {step.line && <p className="mt-1 text-xs text-ink3">{step.line}</p>}
      {step.state !== "waiting" && (
        <>
          {step.setup && (
            <RunnerSetup
              key={step.setup}
              preselect={step.setup}
              pickContext={shownPick(input.recs)?.fit.context ?? null}
            />
          )}
          {step.progress && (
            <div className="mt-2">
              <IngestProgress
                processed={pct ?? 0}
                total={pct != null ? 100 : null}
                label={`Downloading ${pickName}`}
                mode="percent"
                startedAt={pull.startedAt ?? undefined}
              />
              <div className="mt-1 flex items-center justify-between gap-2">
                {/* The status word at every Depth; the byte counts are detail. */}
                <p className="min-w-0 truncate font-mono text-[0.625rem] text-ink4">
                  {prog?.status ?? "starting…"}
                  {showMeta && prog?.total_bytes
                    ? ` · ${formatBytes(prog.completed_bytes)} / ${formatBytes(prog.total_bytes)}`
                    : ""}
                </p>
                <Button variant="tertiary" size="sm" onClick={pull.cancel}>
                  Cancel
                </Button>
              </div>
            </div>
          )}
          {step.command && <CommandLine command={step.command} />}
          {showPower && step.tag && (
            <p className="mt-1 break-all font-mono text-[0.625rem] text-ink4">{step.tag}</p>
          )}
          {step.notes.map((note) => (
            <p key={note} className="mt-1 text-xs text-ink4">
              {note}
            </p>
          ))}
          {buttons.length > 0 && (
            <div className="mt-2 flex flex-wrap items-center gap-2">
              {buttons.map((a) => {
                const b = busy(a);
                return (
                  <Button
                    key={`${a.kind}:${a.label}`}
                    variant={variantOf(a)}
                    size="sm"
                    disabled={b.disabled}
                    onClick={() => run(a)}
                  >
                    {b.label}
                  </Button>
                );
              })}
            </div>
          )}
        </>
      )}
    </li>
  );
}

/**
 * The three servers, and how to get the one chosen — unfolded, because before anything is connected
 * it is the step. The only runner guide on the page while nothing is connected: Model server's
 * comparison appears once there is something to compare against.
 */
/** The install steps, with the context step set to what PM sized its pick for, so "the steps are in
 *  step 1" points at steps that set the number the pick card names. */
function RunnerSetup({
  preselect,
  pickContext,
}: {
  preselect: RunnerName;
  pickContext: number | null;
}) {
  const guides = runnerGuides(undefined, pickContext);
  const [chosen, setChosen] = useState<RunnerName>(preselect);
  const g = guides.find((x) => x.name === chosen) ?? guides[0];
  return (
    <div className="mt-2 text-xs text-ink4">
      <ul className="space-y-0.5">
        {guides.map((x) => (
          <li key={x.name}>
            <span className="text-ink2">{x.name}</span>: {x.bestFor}
          </li>
        ))}
      </ul>
      <SettingRow label="Server to install">
        {(a11y) => (
          <SegmentedControl
            {...a11y}
            value={chosen}
            onChange={setChosen}
            options={guides.map((x) => ({ value: x.name as RunnerName, label: x.name }))}
          />
        )}
      </SettingRow>
      <p className="mt-2">
        <a
          href={g.url}
          target="_blank"
          rel="noreferrer noopener"
          className="text-accent-text underline decoration-dotted underline-offset-2"
        >
          Get {g.name} at {g.url.replace(/^https:\/\//, "")} →
        </a>
      </p>
      {g.caveat && <p className="mt-1 text-ink3">Worth knowing: {withCode(g.caveat)}</p>}
      <p className="mt-1 text-ink3">Staying running: {withCode(g.lifecycle)}</p>
      <ol className="ml-4 mt-1.5 list-decimal space-y-1">
        {g.steps.map((s, k) => (
          <li key={k}>{withCode(s)}</li>
        ))}
      </ol>
    </div>
  );
}
