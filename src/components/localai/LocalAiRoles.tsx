// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { setLocalLlmRoleModel, setLocalLlmRouting } from "../../lib/ipc";
import type { LocalRole } from "../../lib/localModelState";
import type {
  EffectiveRoute,
  LocalCoResidency,
  LocalLlmConfig,
  LocalLlmStatus,
  LocalServedModel,
  LocalTestResult,
} from "../../lib/types";
import { formatGib } from "../../lib/format";
import { powerOf } from "../../lib/powerRoute";
import { COMFORTABLE_WINDOW, whereOf } from "./readiness";
import { SectionLink } from "./SectionLink";
import { sectionLabel } from "./sections";
import type { RoleTest, RoleTests } from "./useRoleTests";
import { Button, Callout, SectionInfo, SectionLabel, Select } from "../ui";

/**
 * "Assign roles" — which local model answers chat, which does the unattended work, and proof that
 * the pair actually works.
 *
 * The proof — the test state — is the tab's (`useRoleTests`), handed down as `roleTests`. A test
 * result belongs to a row: it is about one role's model on one server, and it stops being true the
 * moment either changes. This section clears it when you change a row; the hook clears everything
 * when the endpoint moves, which is why nothing here has to remember to invalidate itself from the
 * outside.
 */
export function LocalAiRoles({
  config,
  status,
  served,
  servedLoaded,
  configured,
  coResidency,
  anyLocalRoleWithModel,
  roleTests,
  onConfigPatch,
  onError,
  error,
}: {
  config: LocalLlmConfig | null;
  status: LocalLlmStatus | null;
  served: LocalServedModel[];
  /** `served` is an ANSWER rather than a starting value — see the tab's own note on it. */
  servedLoaded: boolean;
  configured: boolean;
  /** The two-models-at-once arithmetic, or null when there is only one model in play. */
  coResidency: LocalCoResidency | null;
  anyLocalRoleWithModel: boolean;
  /** Which test is running and the last outcome per role, from the tab's `useRoleTests`. */
  roleTests: RoleTests;
  /** Write a role change back into the tab's copy of the config, optimistically. */
  onConfigPatch: (patch: Partial<LocalLlmConfig>) => void;
  onError: (message: string | null) => void;
  /** Something in this section went wrong, said here rather than at the top of the tab. */
  error?: string | null;
}) {
  const { testing, tests, runTest, clearTest } = roleTests;
  // "on this computer" only for a loopback address: a LAN or remote server receives what PM sends.
  const where = whereOf(config?.base_url);
  /** A role's bound model, when its routing reaches the server and the listing has answered that
   *  the server isn't serving it — the trap where a job is set up and silently can't run. */
  const notServed = (model: string, routing: string) =>
    servedLoaded && routing !== "cloud" && model !== "" && !served.some((m) => m.id === model);

  // Each write clears this section's last error first: it was about a write this one supersedes.
  function changeRoleModel(role: "chat" | "background", model: string) {
    onError(null);
    onConfigPatch({ [`${role}_model`]: model || null });
    clearTest(role);
    void setLocalLlmRoleModel(role, model).catch((e) => onError(String(e)));
  }

  function changeRouting(role: "chat" | "background", pref: string) {
    onError(null);
    onConfigPatch({ [`${role}_routing`]: pref });
    clearTest(role);
    void setLocalLlmRouting(role, pref as "cloud" | "local" | "local-then-cloud").catch((e) =>
      onError(String(e)),
    );
  }

  return (
    <div
      id="sec-localai-roles"
      data-settings-section
      data-help="settings-localai-roles"
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel>{sectionLabel("sec-localai-roles")}</SectionLabel>
      {error && <Callout className="mt-2">{error}</Callout>}
      {!configured ? (
        <p className="mt-2 text-xs text-ink4">
          Connect your model server first, under <SectionLink to="sec-localai-endpoint" />. Then
          choose which model answers chat and which does background work.
        </p>
      ) : (
        <div className="mt-3 space-y-4">
          <RoleRow
            role="chat"
            label="Chat"
            hint="Answers your chats."
            model={config?.chat_model ?? ""}
            routing={config?.chat_routing ?? "cloud"}
            effective={status ? (powerOf(status).chat.effective ?? null) : null}
            where={where}
            notServed={notServed(config?.chat_model ?? "", config?.chat_routing ?? "cloud")}
            served={served}
            onModel={(m) => changeRoleModel("chat", m)}
            onRouting={(p) => changeRouting("chat", p)}
            onTest={() => void runTest("chat")}
            testing={testing === "chat"}
            testsBlocked={testing !== null}
            busy={status?.chat_answering ?? false}
            test={tests.chat}
          />
          <RoleRow
            role="background"
            label="Background work"
            hint="Sorting proposals, titles, summaries, and learning."
            model={config?.background_model ?? ""}
            routing={config?.background_routing ?? "cloud"}
            effective={status ? (powerOf(status).background.effective ?? null) : null}
            where={where}
            notServed={notServed(
              config?.background_model ?? "",
              config?.background_routing ?? "cloud",
            )}
            served={served}
            onModel={(m) => changeRoleModel("background", m)}
            onRouting={(p) => changeRouting("background", p)}
            onTest={() => void runTest("background")}
            testing={testing === "background"}
            testsBlocked={testing !== null}
            busy={status?.background_answering ?? false}
            test={tests.background}
          />
          {servedLoaded && served.length === 0 && (
            // Unfolded, for the same reason as the two hints below: the settings doctrine folds
            // prose but never a gating hint, and "both dropdowns are empty, and here is why" is
            // exactly one. This state only became something a user can sit in once the endpoint
            // check learned to accept a server with an empty model list (#790) — before that a
            // fresh runner failed to connect at all, so nothing here had to speak for it.
            <p className="text-xs text-ink4">
              This server isn't serving any models yet, so there is nothing to assign and both roles
              stay on cloud. Download a model into it and it will appear here within about half a
              minute.
            </p>
          )}
          {/* Arithmetic, not a hedge. This used to be one unconditional paragraph fired at every
              pair of distinct local models — noise on a setup with room to spare, which is how you
              train someone to ignore the line that matters. The backend now sums the two
              footprints the cards already showed against one budget, and `co_residency` is null
              unless there really are two different models both being served.

              It also used to describe a failure that does not happen. Ollama's FAQ is explicit:
              when a model will not fit beside a loaded one, "all new requests will be queued until
              the new model can be loaded. As prior models become idle, one or more will be
              unloaded to make room". So the outcome is swapping, not breaking — and the cost is
              seconds per switch, which is a thing to say plainly rather than a hazard to imply. */}
          {coResidency && (
            <CoResidencyLine
              fit={coResidency}
              chatModel={config?.chat_model ?? ""}
              onUseForBoth={(m) => changeRoleModel("background", m)}
            />
          )}
          {/* Unfolded: a loss warning, not prose. This is the number that explains the symptom
              people blame on model size — a server serving a small window cannot hold one filing
              batch, so PM sends fewer documents per call, and past a point stops rather than let
              the server cut the instructions off the front of the prompt and answer anyway.

              Three branches, because this whole block used to be gated on `served_window != null`
              and so rendered NOTHING on a fresh install — the one moment the warning exists for.
              The number can only be read while a model is resident (`/api/ps` on Ollama, `/slots`
              on llama-server) and nothing loads one until the first local call, so "not measured
              yet" was the permanent state of every new setup and it said so nowhere. */}
          {anyLocalRoleWithModel && status != null && status.served_window == null && (
            <p className="text-xs text-ink4">
              PM hasn't read your server's context window yet — that number is only visible while a
              model is loaded. Load one, or send a local message, and PM picks it up within about
              half a minute. It's worth knowing before you rely on it: a small window makes PM's
              background work send less per call.
            </p>
          )}
          {status?.served_window != null && status.window_source === "models_meta" && (
            // The two unproven rungs are NOT the same kind of uncertainty, and calling both "an
            // estimate" hid that. This one is the server's claim about the MODEL — its trained
            // capacity, an UPPER bound on this load — and it is the exact number that read 32768
            // while a server served 4096 (#792). PM sizes to its own floor regardless, so showing
            // it alone would have the panel contradict PM's behaviour without saying so.
            <p className="text-xs text-ink4">
              Your server says this model can go up to{" "}
              <span className="text-ink2">{status.served_window.toLocaleString()} tokens</span> —
              but that is the model's own limit, not what your server actually loaded it with, and
              the two are often far apart. Until PM can confirm the real one it sizes its work
              cautiously, so background work may be doing less than your server could take. One
              local reply is enough for PM to read the real number.
            </p>
          )}
          {status?.served_window != null &&
            status.window_source !== "models_meta" &&
            status.served_window < COMFORTABLE_WINDOW && (
              <p className="text-xs text-ink4">
                {status.served_window_proven ? (
                  <>
                    Your server is serving{" "}
                    <span className="text-ink2">
                      {status.served_window.toLocaleString()} tokens
                    </span>{" "}
                    of context.
                  </>
                ) : (
                  // The SUBJECT matters. An unproven floor is PM's own number, and the old
                  // sentence put it in the server's mouth — "your server is serving 4,096 (PM's
                  // floor)" reads as a reading of the user's machine when it is an admission
                  // about PM.
                  <>
                    PM is sizing its work for{" "}
                    <span className="text-ink2">
                      {status.served_window.toLocaleString()} tokens
                    </span>{" "}
                    because it hasn't been able to read your server's real window.
                  </>
                )}{" "}
                PM's background work — sorting proposals, summaries, learning — sends more than that
                in one go, so it will send smaller batches to fit. Raising it makes that work
                better: Ollama uses <span className="text-ink2">OLLAMA_CONTEXT_LENGTH</span>,
                llama-server uses <span className="text-ink2">--ctx-size</span>, and LM Studio has a
                context-length slider on the model — the steps for your server are under{" "}
                <SectionLink to="sec-localai-endpoint" />, in “Settings PM's numbers assume”. Ollama
                picks its default from your graphics card's memory, so{" "}
                <span className="text-ink2">ollama ps</span> is the way to see what it chose.
              </p>
            )}
          {served.some((m) => m.embedding) && (
            // Unfolded on purpose. The settings doctrine folds prose but never gating hints, and
            // "this one is listed but you can't pick it" is exactly a gating hint (same call as
            // the "already downloaded" list above).
            <p className="text-xs text-ink4">
              Embedding and reranking models are listed but can't be chosen — they turn text into
              numbers for search, and can't hold a conversation. PM uses its own for search.
            </p>
          )}
        </div>
      )}
      <SectionInfo title="How routing & fallback work">
        <p>
          <span className="text-ink2">Cloud</span> keeps using your OpenRouter model.{" "}
          <span className="text-ink2">Local only</span> uses the model you picked and fails if it's
          unreachable. <span className="text-ink2">Local, fall back to cloud</span> tries local
          first and quietly hands off to your cloud model only on a hard failure (an unreachable or
          broken server) — never to chase quality. The On battery section is separate from fallback:
          it can move a role set to Local, fall back to cloud onto your cloud model while your
          battery is low, and it asks you before it first does. Local only is never moved.
        </p>
      </SectionInfo>
    </div>
  );
}

const ROUTING_OPTIONS = [
  { value: "cloud", label: "Cloud" },
  { value: "local", label: "Local only" },
  { value: "local-then-cloud", label: "Local, fall back to cloud" },
];

/** Who each routing suits — said for the one chosen, so the choice is made against a reason. */
const GOOD_IF: Record<string, string> = {
  cloud:
    "Good if this computer is slow, or you'd rather not keep a model loaded. What you send goes to OpenRouter.",
  local:
    "Good if nothing should ever go to the cloud. If your server is down, this job doesn't answer, and On battery never moves it.",
  "local-then-cloud":
    "Good for most setups with a cloud key: your own model when it can, your cloud model when your server is down. It's the only setting On battery can move.",
};

/** The routing Select's and the model Select's names, per role. */
const ROLE_NAMES: Record<LocalRole, { routing: string; model: string }> = {
  chat: { routing: "Where chat runs", model: "Chat model" },
  background: { routing: "Where background work runs", model: "Background work model" },
};

/**
 * Where this job's requests really go, as the backend worked it out from the preference, the server,
 * the model and the key together (`effective`). Never guessed from the routing alone: a role set to
 * Local with no model doesn't quietly use the cloud, and a Cloud role with no key answers nothing.
 */
function effectiveLine(
  effective: EffectiveRoute,
  routing: string,
  model: string,
  where: string,
): string | null {
  // The three that name the model read the row's own (optimistic) choice. Just after it is cleared
  // the status can still say the old route for a moment; a sentence naming no model would be worse
  // than none until the next snapshot.
  if (!model && (effective === "local_only" || effective === "local_then_cloud")) return null;
  if (!model && effective === "cloud_for_power") return null;
  switch (effective) {
    case "cloud":
      return "Uses your cloud model.";
    case "local_only":
      return `Runs on ${model} ${where}, and never uses the cloud.`;
    case "local_then_cloud":
      return `Runs on ${model} ${where}; your cloud model answers only if your server fails.`;
    case "cloud_for_power":
      return `On battery, so this is on your cloud model for now; ${model} is waiting.`;
    case "nothing":
      return routing === "cloud"
        ? "There's no cloud key, so this job has nothing to answer with."
        : "No local model chosen, so this job has nothing to answer with.";
    case "unknown":
      return "PM can't read your saved keys right now, so it can't say where this goes.";
  }
}

function RoleRow({
  role,
  label,
  hint,
  model,
  routing,
  effective,
  where,
  notServed,
  served,
  onModel,
  onRouting,
  onTest,
  testing,
  testsBlocked,
  busy,
  test,
}: {
  role: LocalRole;
  label: string;
  hint: string;
  model: string;
  routing: string;
  /** Where this role's requests really go, or null while the status hasn't been read. */
  effective: EffectiveRoute | null;
  /** "on this computer" | "on your model server". */
  where: string;
  /** The bound model isn't one the server is serving right now. */
  notServed: boolean;
  served: LocalServedModel[];
  onModel: (m: string) => void;
  onRouting: (p: string) => void;
  onTest: () => void;
  /** This row's test is the one running. */
  testing: boolean;
  /** Some test is running — this row's or the other's. */
  testsBlocked: boolean;
  /** This role's model is answering something right now. */
  busy: boolean;
  test: RoleTest | undefined;
}) {
  // Keep the currently-saved model selectable even if the endpoint isn't serving it right now.
  // The embedder gate is applied AFTER this line, not by filtering `served` before it — otherwise
  // an embedder a user had already saved would slip back in through this branch.
  // A saved model the endpoint isn't serving right now stays enabled: it is already the current
  // value, disabling it would render the Select's own selection greyed out, and the gate's job is
  // to stop a NEW bad assignment. That also keeps the embedder predicate in exactly one place
  // (Rust), rather than a second copy here that could drift.
  const saved = served.some((m) => m.id === model);
  const routeLine = effective != null ? effectiveLine(effective, routing, model, where) : null;
  const options: LocalServedModel[] =
    model && !saved ? [{ id: model, embedding: false }, ...served] : served;
  return (
    <div>
      <div className="flex items-baseline justify-between gap-3">
        <span className="text-sm font-medium text-ink2">{label}</span>
        <span className="text-[0.6875rem] text-ink4">{hint}</span>
      </div>
      <div className="mt-1.5 flex flex-wrap items-center gap-2">
        {/* Routing first: WHERE the job runs is the decision, and which local model is a detail of
            one answer to it. */}
        <Select
          aria-label={ROLE_NAMES[role].routing}
          value={routing}
          onChange={(e) => onRouting(e.target.value)}
        >
          {ROUTING_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </Select>
        <Select
          aria-label={ROLE_NAMES[role].model}
          value={model}
          onChange={(e) => onModel(e.target.value)}
          className="min-w-[10rem] flex-1"
        >
          {/* Not "use cloud": with no model the role goes wherever its routing and keys send it,
              which can be nowhere at all. The line under the row says which. */}
          <option value="">— no local model —</option>
          {options.map((m) => (
            // Shown, not hidden: a model you can see in Ollama but not in PM reads as a PM bug,
            // whereas one shown with its reason reads as an explanation — and it makes a
            // mis-classification visible instead of a model that silently vanished.
            <option key={m.id} value={m.id} disabled={m.embedding}>
              {m.embedding ? `${m.id} — embedding model` : m.id}
            </option>
          ))}
        </Select>
        {/* Only where there is something to test. A role set to cloud has no local pair, and the
            backend refuses that case anyway — offering the button there would be a control whose
            only outcome is an error message. */}
        {routing !== "cloud" && model.trim() !== "" && (
          <Button
            variant="secondary"
            size="sm"
            onClick={onTest}
            disabled={testsBlocked}
            title="Send one short message to this model and show you what comes back"
          >
            {testing ? "Testing…" : "Test it"}
          </Button>
        )}
      </div>
      {/* Unfolded: a status readout, from the backend's own route — the sentence that says what
          actually happens with these settings, keys and server as they are. */}
      {(routeLine != null || notServed) && (
        <p className="mt-1 text-xs text-ink3">
          {routeLine}
          {routeLine != null && notServed && " "}
          {notServed && `Your server isn't serving ${model} right now.`}
        </p>
      )}
      {GOOD_IF[routing] && <p className="mt-1 text-[0.6875rem] text-ink4">{GOOD_IF[routing]}</p>}
      {/* Said before the click, not after it: a test arriving while the model is mid-answer waits
          for the lane, which can be the length of a whole reply. The button still works — this is
          an explanation of the wait, not a refusal. */}
      {busy && !testing && (
        <p className="mt-1 text-[0.6875rem] text-ink4">
          This model is answering something right now, so a test would wait its turn.
        </p>
      )}
      {testing && (
        <p className="mt-1 text-[0.6875rem] text-ink4">
          Waiting for a reply. If the model isn&rsquo;t loaded yet this includes loading it, which
          can take a while the first time.
        </p>
      )}
      {test?.error && <p className="mt-1 text-[0.6875rem] text-st-due">{test.error}</p>}
      {/* Only against the model it actually asked. The model picker stays live during a test (there
          is no reason to freeze the settings you may be testing), and the backend job outlives this
          view — so without this a result can land under a model that was never asked anything. */}
      {test?.result && test.result.model === model && <TestOutcome result={test.result} />}
    </div>
  );
}

/** What the test found — shown as the model's own words rather than a tick, because the reply IS
 *  the evidence and a green tick with nothing behind it is the reassurance this feature exists to
 *  stop giving. */
function TestOutcome({ result }: { result: LocalTestResult }) {
  const seconds = (result.elapsed_ms / 1000).toFixed(1);
  if (!result.ok) {
    return (
      <p className="mt-1 text-[0.6875rem] text-st-due">
        {result.message ?? "The test didn't get a usable reply."}
      </p>
    );
  }
  return (
    <>
      <p className="mt-1 text-[0.6875rem] text-st-quick">
        Answered in {seconds}s
        {result.reply ? (
          <>
            {" — "}
            <span className="text-ink3">&ldquo;{result.reply}&rdquo;</span>
          </>
        ) : null}
      </p>
      {result.loaded_for_test === true && (
        <p className="mt-1 text-[0.6875rem] text-ink4">
          PM loaded the model for this, so your release setting applies to it.
          {result.was_holding.length > 0 && (
            // PM cannot stop a server making room, and pretending otherwise would be worse than
            // saying what was there: someone reading a pass deserves to know why their next message
            // might take a few seconds.
            <>
              {" "}
              Your server was already holding{" "}
              <span className="text-ink3">{result.was_holding.join(", ")}</span> — if it needed the
              room, it may have unloaded that to fit this in.
            </>
          )}
        </p>
      )}
      {result.loaded_for_test === null && (
        // The honest consequence of not being able to ask. PM only unloads what it can prove it
        // loaded, so a load it could not observe is one it will never hand back.
        <p className="mt-1 text-[0.6875rem] text-ink4">
          PM couldn&rsquo;t check what your server already had loaded. If the test loaded this
          model, PM won&rsquo;t hand that memory back on its own — your server decides.
        </p>
      )}
    </>
  );
}

/**
 * What two different models on one server actually mean for this machine.
 *
 * Silent when they fit, because absence is the pass — the same call the served-window line makes and
 * for the same reason. Never says the machine will fail: it will not. It swaps, and swapping costs
 * time, so time is what this talks about.
 */
function CoResidencyLine({
  fit,
  chatModel,
  onUseForBoth,
}: {
  fit: LocalCoResidency;
  /** Chat's model, offered for both jobs when the pair won't stay loaded together. */
  chatModel: string;
  /** Put `model` on background work too. */
  onUseForBoth: (model: string) => void;
}) {
  // The graphics card is the tighter constraint whenever there is one, and it is the one people mean
  // by "it takes up all of my GPU twice" — so it decides the verdict, with system RAM as the fallback
  // on a machine with no discrete card.
  const verdict = fit.vram ?? fit.ram;
  if (verdict === "fits") return null;

  const budget = fit.vram ? fit.vram_budget_gb : fit.ram_budget_gb;
  const where = fit.vram ? "on your graphics card" : "in memory";

  if (verdict === "unknown") {
    return (
      <p className="text-xs text-ink4">
        Chat and background work use different models, and PM couldn't size one of them — so it
        can't say whether your server will keep both loaded or swap between them.
      </p>
    );
  }
  // The one fix that always works, offered where swapping is a real prospect. It writes background
  // work's model only, so chat — the one someone is looking at — never changes under them.
  const useForBoth = chatModel && (
    <div className="flex flex-wrap items-center gap-2">
      <Button variant="tertiary" size="sm" onClick={() => onUseForBoth(chatModel)}>
        Use {chatModel} for both
      </Button>
      <span className="text-xs text-ink4">
        Same model for both jobs: it loads once and never has to swap.
      </span>
    </div>
  );

  if (verdict === "too_close") {
    return (
      <>
        <p className="text-xs text-ink4">
          Chat and background work use different models. Together they come to about{" "}
          <span className="text-ink2">{formatGib(fit.combined_gb)}</span> against roughly{" "}
          <span className="text-ink2">{formatGib(budget)}</span> {where} — close enough that PM
          can't call it, since its memory estimate is only good to about 15%. If your server does
          start swapping between them you'll see replies pause for a few seconds now and then.
        </p>
        {useForBoth}
      </>
    );
  }
  return (
    <>
      {/* `text-st-due` is the attention token. This is the one state worth pulling a user's eye
          to, which is exactly why the other three must not. */}
      <p className="text-xs text-st-due">
        Chat and background work use different models, and they won't both stay loaded — together
        they need about <span className="font-medium">{formatGib(fit.combined_gb)}</span>, and there
        is about <span className="font-medium">{formatGib(budget)}</span> {where}. Nothing breaks:
        your server unloads one to make room for the other. But every switch between chatting and
        background work then costs a few seconds while a model reloads, and background work runs
        often. Putting the same model on both roles avoids it entirely.
      </p>
      {useForBoth}
    </>
  );
}
