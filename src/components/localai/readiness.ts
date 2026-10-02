// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The start card's single source of words: where the user stands, the four steps to a working local
// model, and the readout for someone already set up — plus the pure readings of the stored endpoint
// that every section words from (which server it is, and whether it is on this computer).
//
// Everything here is a pure reading of what the tab already holds — the stored config, the live
// status, the served list, the hardware and pick, the jobs. It writes nothing and decides nothing
// the backend decides: where a role's requests really go comes from `status.power.*.effective`, never
// from the routing preference alone, because "set to local" and "runs locally" are different facts
// and the start card is the one place that must never confuse them.
//
// The rules the card's copy keeps, each pinned in readiness.test.ts:
//   * at most one step is "next", and it is the first one still to do — so the card has one primary;
//   * no string repeats a phrase a section already says (the collision list), so a test looking for
//     one section's sentence never finds two;
//   * no string points "above" or "below" except inside this same section;
//   * every action makes exactly the write a section control makes (`ACTION_MIRRORS`).

import { formatGib } from "../../lib/format";
import type { LocalRole } from "../../lib/localModelState";
import { powerGate, powerOf, powerSummary, whoPhrase } from "../../lib/powerRoute";
import type {
  DetectedEndpoint,
  EffectiveRoute,
  LocalDiskSource,
  LocalFitResult,
  LocalGpuResidency,
  LocalLlmConfig,
  LocalLlmStatus,
  LocalPick,
  LocalRecommendations,
  LocalServedModel,
} from "../../lib/types";
import type { RunnerName } from "../../lib/workbenchGuide";
import type { LocalAiTarget } from "./locate";
import { sectionLabel } from "./sections";
import type { RoleTest } from "./useRoleTests";

/** The served context below which PM says so, under Assign roles. One filing batch is ~3.5k tokens
 *  of prompt before the reply reserve, so 8192 is the point at which a batch stops being comfortable
 *  rather than the point at which it breaks — a user is better told early than told by the work
 *  quietly getting worse. */
export const COMFORTABLE_WINDOW = 8192;

/** The runner each auto-detected port belongs to — the ports `local_ai.rs` probes. */
const RUNNER_BY_PORT: Record<string, RunnerName> = {
  "11434": "Ollama",
  "1234": "LM Studio",
  "8080": "llama-server",
};

/**
 * Which server an endpoint address is, by its port, or null when the port is not one PM probes.
 *
 * Parsed, never a substring test: ":114341", or "11434" anywhere in a path, must not count. A
 * heuristic all the same — an Ollama on a custom port reads as null and degrades to the steps that
 * suit any server, and anything else on 11434 reads as Ollama and gets a download whose pull fails
 * with a clear error.
 */
export function runnerOf(url: string | null | undefined): RunnerName | null {
  if (!url) return null;
  try {
    return RUNNER_BY_PORT[new URL(url).port] ?? null;
  } catch {
    return null;
  }
}

/**
 * Whether an endpoint address is this computer: `localhost`, `127.*` or `::1`.
 *
 * What lets copy say "on this computer" — anything else is "your model server", because a LAN or
 * remote server receives what PM sends it, and saying otherwise would be the one claim about privacy
 * this tab must never get wrong.
 */
export function isLoopback(url: string | null | undefined): boolean {
  if (!url) return false;
  try {
    const host = new URL(url).hostname.toLowerCase();
    // An address, not a name that happens to start "127." — `127.example.com` is somewhere else.
    // The URL parser has already normalised shorthand like `127.1` to four parts.
    return host === "localhost" || /^127\.\d+\.\d+\.\d+$/.test(host) || host === "[::1]";
  } catch {
    return false;
  }
}

/** "on this computer" for a loopback address, else "on your model server". */
export function whereOf(url: string | null | undefined): string {
  return isLoopback(url) ? "on this computer" : "on your model server";
}

// ── The start card ─────────────────────────────────────────────────────────────────────────────

/** Everything the start card reads. Every field is the tab's own state, untouched. */
export interface ReadinessInput {
  config: LocalLlmConfig | null;
  status: LocalLlmStatus | null;
  served: LocalServedModel[];
  /** `served` is an answer rather than a starting value (the tab's own note on it). */
  servedLoaded: boolean;
  recs: LocalRecommendations | null;
  recsLoading: boolean;
  /** What the port probe found while nothing is configured; null until it has answered. */
  detected: DetectedEndpoint[] | null;
  detecting: boolean;
  /** The one download: its tag, and how far it has got (0–100), or nulls. */
  pull: { tag: string | null; pct: number | null };
  /** The last tag this view saw finish downloading. */
  lastPulledTag: string | null;
  tests: { running: LocalRole | null; chat?: RoleTest; background?: RoleTest };
  /** A role was written in this mount (and the endpoint hasn't changed since). */
  justAssigned: boolean;
  /** undefined until first read, null when PM couldn't ask. */
  residency: LocalGpuResidency | null | undefined;
}

export type StepState = "done" | "next" | "waiting" | "attention" | "optional" | "checking";

/** How a role's routing is stored. */
export type LocalRouting = "cloud" | "local" | "local-then-cloud";

/** One "Use … for both" write: the model each role gets (null = left as it is) and the routing each
 *  role is switched to (null = left as it is), with the sentence that says so before the click. */
export interface AssignPlan {
  models: Record<LocalRole, string | null>;
  routing: Record<LocalRole, LocalRouting | null>;
  sentence: string;
}

export type StepAction =
  | { kind: "connect"; url: string; label: string }
  | { kind: "download"; repo: string; tag: string; label: string }
  | { kind: "assign"; plan: AssignPlan; label: string }
  | { kind: "test"; role: LocalRole; label: string }
  | { kind: "detect"; label: string }
  | { kind: "locate"; to: LocalAiTarget; label: string }
  | { kind: "release"; label: string };

export interface Step {
  id: "server" | "model" | "roles" | "check";
  n: 1 | 2 | 3 | 4;
  title: string;
  state: StepState;
  line: string | null;
  /** The step's own action. It is the card's primary only while the step is next (`primaryOf`). */
  action: StepAction | null;
  secondary: StepAction[];
  /** A command to copy, shown under the line. */
  command: string | null;
  /** Further lines, in order, after the line (and the command). */
  notes: string[];
  /** Step 1, not connected: the server guide to show, preselected on this runner. */
  setup: RunnerName | null;
  /** Step 2: the pick's own download is running, so its progress is shown here and not on its card. */
  progress: boolean;
  /** Step 2: the pick's Ollama tag, shown at Power depth beside its Download. */
  tag: string | null;
}

/**
 * Every start-card action makes the same write as a section control, so pressing one is never a
 * different thing from doing it by hand further down. The ipc wrappers each kind calls, and the
 * control that already makes that call — readiness.test.ts pins the table, and the start card's own
 * test presses both and compares the calls.
 */
export const ACTION_MIRRORS: Record<
  StepAction["kind"],
  { ipc: readonly string[]; control: string | null }
> = {
  connect: { ipc: ["setLocalLlmEndpoint"], control: "Model server › Connect" },
  download: { ipc: ["pullLocalModel"], control: "All models › Download" },
  assign: {
    ipc: ["setLocalLlmRoleModel", "setLocalLlmRouting"],
    control: "Assign roles › the model and “Where … runs” selects",
  },
  test: { ipc: ["testLocalLlm"], control: "Assign roles › Test it" },
  release: { ipc: ["releaseLocalGpu"], control: "Model memory › Release now" },
  detect: { ipc: ["probeLocalLlmPorts"], control: "Model server › Auto-detect a local server" },
  // Moves the reader, writes nothing.
  locate: { ipc: [], control: null },
};

/** The kinds that can be the card's one primary button — the ones that do the step. */
const PRIMARY_KINDS: ReadonlySet<StepAction["kind"]> = new Set([
  "connect",
  "download",
  "assign",
  "test",
]);

/** The step's primary button, or null: its own action, and only while it is the step to do next. */
export function primaryOf(step: Step): StepAction | null {
  return step.state === "next" && step.action && PRIMARY_KINDS.has(step.action.kind)
    ? step.action
    : null;
}

const ROLES: readonly LocalRole[] = ["chat", "background"];
const ROLE_NAME: Record<LocalRole, string> = { chat: "Chat", background: "Background work" };
const ROUTING_LABEL: Record<string, string> = {
  cloud: "Cloud",
  local: "Local only",
  "local-then-cloud": "Local, fall back to cloud",
};

/** "Chat" | "Background work" | "Chat and background work". */
function whoCap(roles: readonly LocalRole[]): string {
  const w = whoPhrase(roles.includes("chat"), roles.includes("background"));
  return w.charAt(0).toUpperCase() + w.slice(1);
}

function who(roles: readonly LocalRole[]): string {
  return whoPhrase(roles.includes("chat"), roles.includes("background"));
}

function isAre(roles: readonly LocalRole[]): string {
  return roles.length > 1 ? "are" : "is";
}

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

/** "a, b and c". */
function listJoin(items: readonly string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** The facts about one role, read once. */
interface RoleFacts {
  role: LocalRole;
  model: string;
  routing: string;
  /** Where its requests really go, or null while there is no status. */
  effective: EffectiveRoute | null;
  /** It runs on the local model — said from `effective`, including the battery's temporary move and
   *  a local role whose keys PM can't read (the fallback is what's unknown there, not the model). */
  atWork: boolean;
  /** It is answering on the local model right now: Local only, or Local, fall back to cloud. */
  local: boolean;
}

function roleFacts(i: ReadinessInput, role: LocalRole): RoleFacts {
  const model = (role === "chat" ? i.config?.chat_model : i.config?.background_model)?.trim() ?? "";
  const routing =
    (role === "chat" ? i.config?.chat_routing : i.config?.background_routing) ?? "cloud";
  const effective = i.status ? (powerOf(i.status)[role].effective ?? null) : null;
  const unknownLocal = effective === "unknown" && routing !== "cloud" && model !== "";
  const local = effective === "local_only" || effective === "local_then_cloud" || unknownLocal;
  return {
    role,
    model,
    routing,
    effective,
    local,
    atWork: local || effective === "cloud_for_power",
  };
}

function cloudKeyOf(i: ReadinessInput, role: LocalRole) {
  return i.status ? (powerOf(i.status)[role].cloud_key ?? "absent") : "absent";
}

/** The runner an auto-detected server is. */
function detectedName(d: DetectedEndpoint): string {
  return runnerOf(d.url) ?? d.label;
}

/** The order servers are named in: Ollama first — the one PM can download into — then the order
 *  PM probes them in. */
const RUNNER_RANK: Record<string, number> = { Ollama: 0, "LM Studio": 1, "llama-server": 2 };

/** The detected servers, in `RUNNER_RANK` order (anything else after them, as found). */
function sortedDetected(found: readonly DetectedEndpoint[]): DetectedEndpoint[] {
  const rank = (d: DetectedEndpoint) => RUNNER_RANK[detectedName(d)] ?? 3;
  return [...found].sort((a, b) => rank(a) - rank(b));
}

/** A runner PM can tell is installed but isn't answering: its model folder is here (readable or
 *  not). Ollama first. */
function installedButIdle(recs: LocalRecommendations | null): RunnerName | null {
  if (!recs) return null;
  const present = recs.disk_sources_present ?? [];
  const blocked = recs.disk_blocked ?? [];
  if (present.includes("ollama") || blocked.some((b) => b.source === "ollama")) return "Ollama";
  if (present.includes("lm_studio")) return "LM Studio";
  return null;
}

/** The served models that can hold a conversation. */
function chatCapable(i: ReadinessInput): LocalServedModel[] {
  return i.served.filter((m) => !m.embedding);
}

/** The served id equal to `id` ignoring case, or null. */
function servedId(i: ReadinessInput, id: string): string | null {
  const lower = id.toLowerCase();
  return i.served.find((m) => m.id.toLowerCase() === lower)?.id ?? null;
}

/** The catalogue name for a pull tag, or the tag itself. */
function nameForTag(recs: LocalRecommendations | null, tag: string): string {
  const rec = recs?.curated.find((r) => r.ollama_pull === tag || r.gpu_pull?.tag === tag);
  return rec?.display_name ?? tag;
}

/** Whether a fit's machine is one with a separate graphics card. */
export function hasCard(recs: LocalRecommendations | null): boolean {
  return !!recs && recs.hardware.vram_gb != null && !recs.hardware.unified_memory;
}

/** The flags that start llama-server the way PM sized a fit: its context, and the compressed cache
 *  when PM sized it on one. */
function serveFlags(fit: Pick<LocalFitResult, "context" | "kv">): string {
  return `${fit.context != null ? ` --ctx-size ${fit.context}` : ""}${
    fit.kv === "q8_0" ? " -fa on -ctk q8_0 -ctv q8_0" : ""
  }`;
}

/** llama-server fetching `repo` at the fit's quant from Hugging Face, sized the way PM sized it. */
export function hfServeCommand(
  repo: string,
  fit: Pick<LocalFitResult, "quant" | "context" | "kv">,
) {
  return `llama-server -hf ${repo}${fit.quant ? `:${fit.quant}` : ""}${serveFlags(fit)}`;
}

/**
 * How to get a model already on this computer served — the same words for PM's pick (step 2) and for
 * each model under Already on this device. The command is llama-server serving the file as it is,
 * for a single file whose path can be quoted; a split model gets no one-liner, and a path with a
 * quote in it gets none rather than one that breaks.
 */
export function onDiskHow(
  source: LocalDiskSource,
  shards: number,
  path: string | null,
  fit: Pick<LocalFitResult, "context" | "kv">,
): { line: string; command: string | null } {
  const command =
    shards <= 1 && path && !path.includes('"')
      ? `llama-server -m "${path}"${serveFlags(fit)}`
      : null;
  switch (source) {
    case "ollama":
      return {
        line: "It's in Ollama's folder — once Ollama is connected, it shows up by itself.",
        command: null,
      };
    case "lm_studio":
      return {
        line: "It's in LM Studio. Load it there and switch LM Studio's server on — PM sees it within about half a minute.",
        command,
      };
    default:
      return shards > 1
        ? {
            line: `It's split into ${shards} files, so PM can't give you a one-line command to serve it.`,
            command: null,
          }
        : { line: "It's a file on this computer. llama-server can serve it as it is:", command };
  }
}

/**
 * Where the user stands, in a sentence or a few — first match wins. Said from what PM can see:
 * a server it found, a runner's folder, the routes the backend worked out.
 */
export function standing(i: ReadinessInput): string {
  const { config, status } = i;
  if (!config) return "Reading your local AI setup…";
  const chat = roleFacts(i, "chat");
  const background = roleFacts(i, "background");

  if (!config.base_url) {
    const found = sortedDetected(i.detected ?? []).map(detectedName);
    if (found.length === 1)
      return `${found[0]} is running on this computer, but PM isn't connected to it yet.`;
    if (found.length === 2)
      return `${found[0]} and ${found[1]} are running on this computer, but PM isn't connected to either yet.`;
    if (found.length > 2)
      return `${listJoin(found)} are running on this computer, but PM isn't connected to any of them yet.`;
    const idle = installedButIdle(i.recs);
    if (idle) return `${idle} looks installed here, but it isn't running, so PM can't use it yet.`;
    let tail = ".";
    if (chat.effective === "cloud" && background.effective === "cloud") {
      tail = ", so PM uses your cloud model for everything.";
    } else if (
      chat.effective === "nothing" &&
      background.effective === "nothing" &&
      cloudKeyOf(i, "chat") === "absent" &&
      cloudKeyOf(i, "background") === "absent"
    ) {
      tail = ", and there's no cloud key either, so PM has no AI model to use yet.";
    }
    return `No local model is set up on this computer yet${tail}`;
  }

  if (!status) return "Checking your model server…";
  if (status.in_cooldown)
    return `PM has paused calls to your model server for a moment, after several failed in a row.${consequence(chat, background)}`;
  if (!status.reachable)
    return `PM can't reach your model server right now.${consequence(chat, background)}`;

  if (i.servedLoaded && chatCapable(i).length === 0) {
    return i.served.length > 0
      ? "Your model server is running, but it only has embedding models, which can't answer chat or do background work."
      : "Your model server is running, and it's empty.";
  }

  if (!chat.atWork && !background.atWork)
    return "Your model server is ready, but PM isn't using it for anything yet.";

  const where = whereOf(config.base_url);
  const fallsBack = (f: RoleFacts) =>
    f.effective === "local_then_cloud" || f.effective === "cloud_for_power";
  let text: string;
  if (chat.atWork && background.atWork && chat.model === background.model) {
    if (chat.effective === "local_only" && background.effective === "local_only") {
      text = `Chat and background work run on ${chat.model}, ${where}, and never use the cloud.`;
    } else if (fallsBack(chat) && fallsBack(background)) {
      text = `Chat and background work run on ${chat.model}, ${where}, with your cloud model as a fallback.`;
    } else {
      text = sentences(chat, background, where);
    }
  } else {
    text = sentences(chat, background, where);
  }

  const moved = ROLES.filter((r) => roleFacts(i, r).effective === "cloud_for_power");
  if (moved.length > 0)
    text += ` On battery, ${who(moved)} ${isAre(moved)} on your cloud model for now.`;
  const missing = new Set<string>();
  for (const f of [chat, background]) {
    if (
      i.servedLoaded &&
      f.routing !== "cloud" &&
      f.model &&
      !i.served.some((m) => m.id === f.model)
    )
      missing.add(f.model);
  }
  for (const m of missing) text += ` Your server isn't serving ${m} right now.`;
  if (hasCard(i.recs)) {
    const slow = new Set<string>();
    for (const f of [chat, background]) {
      if (!f.atWork || !f.model) continue;
      const row = i.recs?.installed.find((m) => m.id === f.model);
      if (row?.fit.speed_basis === "system") slow.add(f.model);
    }
    for (const m of slow)
      text += ` ${m} is larger than your graphics card's memory, so it runs from system memory — expect slow replies.`;
  }
  const unknown = ROLES.filter((r) => roleFacts(i, r).effective === "unknown");
  if (unknown.length > 0)
    text += ` PM can't read your saved keys right now, so it can't say whether ${who(unknown)} would fall back to the cloud.`;
  return text;
}

/** What an unreachable or paused server means for each job, by where its requests really go. */
function consequence(chat: RoleFacts, background: RoleFacts): string {
  const roles = [chat, background];
  const falling = roles.filter((f) => f.effective === "local_then_cloud").map((f) => f.role);
  const stuck = roles.filter((f) => f.effective === "local_only").map((f) => f.role);
  let text = "";
  if (falling.length > 0)
    text += ` ${whoCap(falling)} ${isAre(falling)} using your cloud model until it's back.`;
  if (stuck.length > 0) text += ` ${whoCap(stuck)} can't answer until it's back.`;
  return text;
}

function sentences(chat: RoleFacts, background: RoleFacts, where: string): string {
  return [chat, background]
    .map((f) => roleSentence(f, where))
    .filter(Boolean)
    .join(" ");
}

/** One role's sentence for `standing`. */
function roleSentence(f: RoleFacts, where: string): string {
  const runs = f.role === "chat" ? "Chat runs" : "Background work runs";
  switch (f.effective) {
    case "local_only":
      return `${runs} on ${f.model}, ${where}, and never uses the cloud.`;
    case "local_then_cloud":
    case "cloud_for_power":
      return `${runs} on ${f.model}, ${where}, with your cloud model as a fallback.`;
    case "cloud":
      return `${ROLE_NAME[f.role]} uses your cloud model.`;
    case "nothing":
      return `${ROLE_NAME[f.role]} has nothing to answer with${
        f.routing === "cloud" ? " — there's no cloud key" : " — no local model is chosen"
      }.`;
    case "unknown":
      // Its local model is known; whether it falls back is what PM can't read, said once after.
      return f.local ? `${runs} on ${f.model}, ${where}.` : "";
    default:
      return "";
  }
}

/** A step with nothing to show yet but what it is waiting on. */
function waitingOn(step: Step, n: number): Step {
  return {
    ...step,
    state: "waiting",
    line: `After step ${n}.`,
    action: null,
    secondary: [],
    command: null,
    notes: [],
    setup: null,
    progress: false,
    tag: null,
  };
}

function blank(id: Step["id"], n: Step["n"], title: string): Step {
  return {
    id,
    n,
    title,
    state: "waiting",
    line: null,
    action: null,
    secondary: [],
    command: null,
    notes: [],
    setup: null,
    progress: false,
    tag: null,
  };
}

/**
 * The four steps, in order. Each is worked out on its own and then put in order: while step 1 isn't
 * done the rest wait on it, and only the first step still to do may be "next" — a later one that
 * would be is shown waiting on the nearest open step before it, so the card never has two
 * primaries.
 */
export function steps(i: ReadinessInput): Step[] {
  const all = [serverStep(i), modelStep(i), rolesStep(i), checkStep(i)];
  if (all[0].state !== "done") {
    return [all[0], ...all.slice(1).map((s) => waitingOn(s, 1))];
  }
  let open: number | null = null;
  return all.map((s) => {
    const shown = open !== null && s.state === "next" ? waitingOn(s, open) : s;
    if (s.state !== "done" && s.state !== "optional" && s.state !== "checking") open = s.n;
    return shown;
  });
}

const LOOK_NOW: StepAction = { kind: "detect", label: "Look now" };
const LOOKING_NOTE = "PM looks for a running server every half minute while this tab is open.";

function serverStep(i: ReadinessInput): Step {
  const step = blank("server", 1, "Get a model server");
  const { config, status } = i;
  if (!config) return { ...step, state: "checking" };
  const url = config.base_url;
  if (url) {
    const toEndpoint: StepAction = {
      kind: "locate",
      to: "sec-localai-endpoint",
      label: "What to check",
    };
    if (!status) return { ...step, state: "checking", line: `Checking your server at ${url}…` };
    if (status.in_cooldown)
      return {
        ...step,
        state: "attention",
        line: "PM paused calls to your server after several failed in a row, and tries again by itself.",
        action: toEndpoint,
      };
    if (!status.reachable)
      return {
        ...step,
        state: "attention",
        line: `PM can't reach your server at ${url} — usually that means it isn't running.`,
        action: toEndpoint,
      };
    return { ...step, state: "done", line: `Your server is answering at ${url}.` };
  }

  // Not connected. Once connected, this step reads only the status — never the probe.
  if (i.detecting && i.detected === null)
    return { ...step, state: "checking", line: "Looking for a model server on this computer…" };
  const found = sortedDetected(i.detected ?? []);
  if (found.length > 0) {
    const [first, ...others] = found;
    const name = detectedName(first);
    return {
      ...step,
      state: "next",
      line: `${name} is running on this computer.`,
      action: { kind: "connect", url: first.url, label: `Connect to ${name}` },
      secondary: others.map((d) => ({
        kind: "connect" as const,
        url: d.url,
        label: `Connect to ${detectedName(d)} instead`,
      })),
      notes:
        found.length > 1 && name === "Ollama"
          ? ["Ollama is the only one PM can download models into for you."]
          : [],
    };
  }
  const idle = installedButIdle(i.recs);
  if (idle)
    return {
      ...step,
      state: "next",
      line: `${idle} looks installed — PM found its model folder — but it isn't answering. Start it, and PM notices within about half a minute.`,
      setup: idle,
      notes: [LOOKING_NOTE],
      secondary: [LOOK_NOW],
    };
  return {
    ...step,
    state: "next",
    line: "PM doesn't come with a model server: you install one once, and it runs on this computer. Ollama is the easiest, and the only one PM can download models into for you.",
    setup: "Ollama",
    notes: [LOOKING_NOTE],
    secondary: [LOOK_NOW],
  };
}

function toAllModels(): StepAction {
  return { kind: "locate", to: "catalog", label: `Go to ${sectionLabel("sec-localai-models")}` };
}

function toRoles(): StepAction {
  return {
    kind: "locate",
    to: "sec-localai-roles",
    label: `Go to ${sectionLabel("sec-localai-roles")}`,
  };
}

function modelStep(i: ReadinessInput): Step {
  const pick = i.recs?.pick;
  const shown = pick && pick.kind !== "nothing" ? pick : null;
  const step = blank("model", 2, shown ? "Get the model" : "Choose a model");
  if (!i.config?.base_url) return waitingOn(step, 1);
  if (!i.servedLoaded)
    return { ...step, state: "checking", line: "Checking what your server has…" };
  const capable = chatCapable(i);
  const allModels = sectionLabel("sec-localai-models");

  if (shown?.kind === "catalogue") {
    if (servedId(i, shown.tag)) return { ...step, state: "done", line: "Your server has it." };
    if (i.pull.tag === shown.tag) {
      return {
        ...step,
        state: "next",
        progress: true,
        notes: ["You can leave this tab — the download carries on."],
      };
    }
    // A job already runs on a model the server has: a setup that works is set up. The pick stays on
    // its card as an option (with the card's own "you're using…" line), never as the step left to
    // do — otherwise someone who chose a different model is told "Setting up" for as long as they
    // keep it, with a primary Download pointed at a model they didn't pick.
    const atWork = roleFacts(i, "chat").atWork || roleFacts(i, "background").atWork;
    if (atWork) {
      return {
        ...step,
        state: "done",
        line: `Your jobs already run on a model your server has. ${shown.display_name} is PM's pick for this computer, if you'd like to try it — it's under ${allModels} too.`,
      };
    }
    // Offered as an alternative to the download.
    const others =
      capable.length > 0
        ? [
            `Your server already has ${plural(capable.length, "other model")} — you can give one a job under ${sectionLabel("sec-localai-roles")} instead.`,
          ]
        : [];
    if (i.pull.tag) {
      return {
        ...step,
        state: "next",
        line: `Downloading ${nameForTag(i.recs, i.pull.tag)} under ${allModels} — PM can fetch its pick when that finishes.`,
        notes: others,
      };
    }
    const runner = runnerOf(i.config.base_url);
    const size = formatGib(shown.download_gb);
    const quant = shown.fit.quant ?? "";
    if (runner === "Ollama") {
      const rec = i.recs?.curated.find((r) => r.repo === shown.repo);
      const asks =
        rec != null &&
        !rec.licence.open &&
        !(i.recs?.terms_accepted ?? []).includes(rec.licence.id);
      return {
        ...step,
        state: "next",
        line: `A ${size} download. Your Ollama fetches it from Hugging Face — PM doesn't download anything itself.${
          asks
            ? " Its licence has its own terms, which PM shows you before the download starts."
            : ""
        }`,
        action: { kind: "download", repo: shown.repo, tag: shown.tag, label: `Download (${size})` },
        tag: shown.tag,
        notes: others,
      };
    }
    if (runner === "LM Studio") {
      return {
        ...step,
        state: "next",
        line: `PM can't download into LM Studio. In LM Studio's Discover tab, search for the name below, download its ${quant} file, then load it.`,
        command: shown.repo,
        notes: others,
      };
    }
    const command = hfServeCommand(shown.repo, shown.fit);
    if (runner === "llama-server") {
      return {
        ...step,
        state: "next",
        line: "llama-server serves one model at a time. Stop it, then start it again with:",
        command,
        notes: others,
      };
    }
    return {
      ...step,
      state: "next",
      line: `PM can only download into an Ollama on its usual port (11434). Get ${shown.repo} at ${quant} into your server — with llama-server, that's:`,
      command,
      notes: others,
    };
  }

  if (shown?.kind === "owned") {
    if (shown.served || servedId(i, shown.id))
      return { ...step, state: "done", line: "Your server has it." };
    const how = onDiskHow(shown.source ?? "folder", shown.shards, shown.path, shown.fit);
    // LM Studio's own route is the one to give for a model in LM Studio: no llama-server line.
    return {
      ...step,
      state: "next",
      line: how.line,
      command: shown.source === "lm_studio" ? null : how.command,
    };
  }

  // No pick: PM isn't choosing (it says why on the card), or this payload has none at all.
  if (capable.length > 0)
    return {
      ...step,
      state: "done",
      line: `Your server has ${plural(capable.length, "model")} you can use.`,
    };
  if (i.served.length > 0)
    return {
      ...step,
      state: "next",
      line: "Your server only has embedding models, which can't hold a conversation. It needs a chat model too.",
      action: toAllModels(),
    };
  return {
    ...step,
    state: "next",
    line: pick
      ? `PM isn't picking a model for this computer — the reason is just above. You can still choose one under ${allModels}; each says how it would run here.`
      : `Choose one under ${allModels}; each says how it would run here.`,
    action: toAllModels(),
  };
}

/** The model step 3 offers to put on both jobs, by name: PM's pick when the server has it, the
 *  model this view just downloaded, or the server's only one. Never an embedder. */
function candidate(i: ReadinessInput): { id: string; name: string } | null {
  if (!i.servedLoaded) return null;
  const pick = i.recs?.pick;
  if (pick?.kind === "owned") {
    const id = servedId(i, pick.id);
    if (id && !i.served.find((m) => m.id === id)?.embedding) return { id, name: pick.display_name };
  }
  if (i.lastPulledTag) {
    const id = servedId(i, i.lastPulledTag);
    if (id && !i.served.find((m) => m.id === id)?.embedding)
      return { id, name: nameForTag(i.recs, i.lastPulledTag) };
  }
  const capable = chatCapable(i);
  return capable.length === 1 ? { id: capable[0].id, name: capable[0].id } : null;
}

/**
 * Put `model` on both jobs: one model, so the server only ever holds one. A job routed to the cloud
 * is switched to run locally — with the cloud as its fallback when it has a key to fall back on,
 * Local only when it hasn't (or PM can't read it) — and a job already routed locally keeps its
 * routing, said by name.
 */
export function assignPlan(
  config: LocalLlmConfig,
  power: { chat: { cloud_key: string }; background: { cloud_key: string } },
  model: string,
  loopback: boolean,
): AssignPlan {
  const routing: Record<LocalRole, LocalRouting | null> = { chat: null, background: null };
  const unreadable: LocalRole[] = [];
  for (const role of ROLES) {
    const stored = role === "chat" ? config.chat_routing : config.background_routing;
    if (stored !== "cloud") continue;
    const key = power[role].cloud_key;
    routing[role] = key === "present" ? "local-then-cloud" : "local";
    if (key === "unreadable") unreadable.push(role);
  }
  const set = ROLES.filter((r) => routing[r] !== null);
  let clause = "";
  if (set.length > 0 && unreadable.length === set.length) {
    clause = `PM can't read your saved keys right now, so it sets ${who(set)} to Local only.`;
  } else if (set.length === 2 && set.every((r) => routing[r] === "local-then-cloud")) {
    clause = `PM sets both to Local, fall back to cloud: your cloud model answers only if your server stops. Choose Local only under ${sectionLabel("sec-localai-roles")} to keep everything ${loopback ? "on this computer" : "on your server"}.`;
  } else if (
    set.length === 2 &&
    set.every((r) => routing[r] === "local") &&
    unreadable.length === 0
  ) {
    clause = "PM sets both to Local only — there's no cloud key to fall back to.";
  } else if (set.length > 0) {
    clause = `PM sets ${set
      .map((r) => `${who([r])} to ${ROUTING_LABEL[routing[r] ?? "local"]}`)
      .join(" and ")}.`;
  }
  const kept = ROLES.filter((r) => routing[r] === null).map((r) => {
    const stored = r === "chat" ? config.chat_routing : config.background_routing;
    return ` ${ROLE_NAME[r]} keeps its current setting, ${ROUTING_LABEL[stored] ?? stored}.`;
  });
  return {
    models: { chat: model, background: model },
    routing,
    sentence:
      `One model for both jobs — chat and background work — so your server only ever holds one. ${clause}${kept.join("")}`.trim(),
  };
}

function rolesStep(i: ReadinessInput): Step {
  const step = blank("roles", 3, "Put it to work");
  if (!i.config?.base_url) return waitingOn(step, 1);
  const chat = roleFacts(i, "chat");
  const background = roleFacts(i, "background");
  const facts = [chat, background];
  const roles = toRoles();

  // The traps first: a job that looks set up and can't do what it says.
  for (const f of facts) {
    if (f.routing !== "cloud" && !f.model) {
      return {
        ...step,
        state: "attention",
        line: `${ROLE_NAME[f.role]} is set to run on your server but has no model chosen, so ${
          f.effective === "cloud" ? "it uses your cloud model" : "it has nothing to answer with"
        }.`,
        action: roles,
      };
    }
  }
  for (const f of facts) {
    if (
      i.servedLoaded &&
      f.routing !== "cloud" &&
      f.model &&
      !i.served.some((m) => m.id === f.model)
    ) {
      return {
        ...step,
        state: "attention",
        line: `${ROLE_NAME[f.role]} is set to ${f.model}, which your server isn't serving right now.`,
        action: roles,
      };
    }
  }
  const co = i.recs?.co_residency;
  if (
    co &&
    (co.vram ?? co.ram) === "exceeds" &&
    chat.routing !== "cloud" &&
    background.routing !== "cloud" &&
    chat.model &&
    background.model &&
    chat.model !== background.model
  ) {
    return {
      ...step,
      state: "attention",
      line: `${chat.model} and ${background.model} are too big to keep loaded together, so your server swaps between them, a few seconds each time.`,
      action: roles,
      secondary: [
        {
          kind: "assign",
          // Only background work's model changes: chat — the one someone is looking at — stays.
          plan: {
            models: { chat: null, background: chat.model },
            routing: { chat: null, background: null },
            sentence: "",
          },
          // "for both jobs", not Assign roles' own "Use … for both": two buttons with one name would
          // be two controls a screen reader can't tell apart.
          label: `Use ${chat.model} for both jobs`,
        },
      ],
    };
  }

  if (!chat.atWork && !background.atWork) {
    const cand = candidate(i);
    if (cand && i.config) {
      const plan = assignPlan(
        i.config,
        i.status
          ? powerOf(i.status)
          : { chat: { cloud_key: "absent" }, background: { cloud_key: "absent" } },
        cand.id,
        isLoopback(i.config.base_url),
      );
      return {
        ...step,
        state: "next",
        line: plan.sentence,
        action: { kind: "assign", plan, label: `Use ${cand.name} for both` },
      };
    }
    return {
      ...step,
      state: "next",
      line: `Choose which model answers chat and which does background work under ${sectionLabel("sec-localai-roles")}.`,
      action: roles,
    };
  }

  const c = chat.atWork ? chat.model : null;
  const b = background.atWork ? background.model : null;
  /** The other job's half of the sentence, from where its requests really go. */
  const elsewhere = (f: RoleFacts) => {
    const name = who([f.role]);
    if (f.effective === "nothing") return `${name} has nothing to answer with`;
    if (f.effective === "unknown")
      return `PM can't say where ${name} goes while it can't read your saved keys`;
    return `${name} uses your cloud model`;
  };
  let line: string;
  if (c && b) {
    line =
      c === b
        ? `Chat and background work both use ${c}.`
        : `Chat uses ${c}; background work uses ${b}.`;
  } else if (c) {
    line = `Chat uses ${c}; ${elsewhere(background)}.`;
  } else {
    line = `Background work uses ${b}; ${elsewhere(chat)}.`;
  }
  return { ...step, state: "done", line };
}

function checkStep(i: ReadinessInput): Step {
  const step = blank("check", 4, "Check it works");
  if (!i.config?.base_url) return waitingOn(step, 1);
  const chat = roleFacts(i, "chat");
  const background = roleFacts(i, "background");
  const role: LocalRole | null = chat.local ? "chat" : background.local ? "background" : null;
  if (!role) return waitingOn(step, 3);
  if (i.tests.running) {
    return {
      ...step,
      state: "checking",
      line: "Waiting for the reply. The first one includes loading the model, which can take a while.",
    };
  }
  const status = i.status;
  const win = status?.served_window ?? null;
  const proven = !!status?.served_window_proven && status?.window_source !== "models_meta";
  if (win != null && proven && win < COMFORTABLE_WINDOW) {
    return {
      ...step,
      state: "attention",
      line: `Your server gives the model room for ${win.toLocaleString()} tokens at a time, so PM sends background work in smaller pieces. Giving it more room makes that work better.`,
      action: {
        kind: "locate",
        // The steps live in Model server's tuning fold, which exists only for a server PM can name.
        to: runnerOf(i.config.base_url) ? "tuning" : "sec-localai-endpoint",
        label: "How to raise it",
      },
    };
  }
  const model = role === "chat" ? chat.model : background.model;
  const test = role === "chat" ? i.tests.chat : i.tests.background;
  const tryAgain: StepAction = { kind: "test", role, label: "Try again" };
  const failed = {
    ...step,
    state: "attention" as const,
    line: `The last test didn't get a usable reply — what happened is under ${sectionLabel("sec-localai-roles")}.`,
    secondary: [tryAgain],
  };
  if (test?.error) return failed;
  if (test?.result && test.result.model === model) {
    if (!test.result.ok) return failed;
    return {
      ...step,
      state: "done",
      line: `The last test replied in ${(test.result.elapsed_ms / 1000).toFixed(1)} s.${
        win != null && proven && win >= COMFORTABLE_WINDOW
          ? ` Your server gives it room for ${win.toLocaleString()} tokens at a time.`
          : ""
      }`,
    };
  }
  const send: StepAction = { kind: "test", role, label: "Send a test message" };
  if (i.justAssigned) {
    return {
      ...step,
      state: "next",
      line: "Send one short message and see what comes back — the only check that proves the model really answers.",
      action: send,
    };
  }
  // A restart forgets test results (they live in memory only), so this never nags a returning user.
  return {
    ...step,
    state: "optional",
    line: "No test since PM started, or since your server changed.",
    secondary: [send],
  };
}

/** The chip beside the section's heading. null while PM is still reading the setup, rather than a
 *  "Not set up" said about a server that may be answering perfectly well. */
export function overall(
  list: readonly Step[],
  i: Pick<ReadinessInput, "config" | "status">,
): { label: string; token: string } | null {
  if (!i.config) return null;
  if (i.config.base_url && !i.status) return null;
  if (list.some((s) => s.state === "attention"))
    return { label: "Needs attention", token: "--st-due" };
  if (list[0]?.state !== "done") return { label: "Not set up", token: "--ink4" };
  if (list.some((s) => s.state === "next")) return { label: "Setting up", token: "--st-look" };
  return { label: "Set up", token: "--st-quick" };
}

/** The returning user's readout: what the server is holding right now, and what On battery does. */
export interface RightNow {
  /** What the server holds, or null before PM has asked. */
  holding: string | null;
  /** Whether PM may hand it back: true = PM loaded it, false = it didn't, null = nothing to say. */
  pmLoaded: boolean | null;
  /** On battery in one sentence, or null where there is nothing to summarise. */
  battery: string | null;
}

export function rightNow(i: ReadinessInput): RightNow {
  const res = i.residency;
  let holding: string | null = null;
  let pmLoaded: boolean | null = null;
  if (res === null || res?.resident === null) {
    holding = "Right now PM can't see what your server has loaded.";
  } else if (res) {
    const held = res.resident;
    if (held.length === 0) {
      holding =
        "Right now nothing is loaded. The next request loads the model, which takes a few seconds.";
    } else {
      const card = hasCard(i.recs) && res.vram_gb != null;
      const atLeast = held.reduce((n, m) => n + m.size_vram_gb, 0);
      holding = `Right now your server is holding ${held.map((m) => m.model).join(", ")}${
        card
          ? ` — at least ${formatGib(atLeast)} of your ${formatGib(res.vram_gb)} card`
          : " in memory"
      }.`;
      pmLoaded = held.some((m) => m.pm_loaded);
    }
  }
  const configured = !!i.config?.base_url;
  const anyLocalRoleWithModel =
    (i.config?.chat_routing !== "cloud" && !!i.config?.chat_model?.trim()) ||
    (i.config?.background_routing !== "cloud" && !!i.config?.background_model?.trim());
  const gate = powerGate(configured, anyLocalRoleWithModel, i.status);
  const battery = powerSummary(gate, i.status ? powerOf(i.status) : null);
  return { holding, pmLoaded, battery };
}

/** PM's pick, when it names a model. */
export function shownPick(
  recs: LocalRecommendations | null,
): Exclude<LocalPick, { kind: "nothing" }> | null {
  const pick = recs?.pick;
  return pick && pick.kind !== "nothing" ? pick : null;
}
