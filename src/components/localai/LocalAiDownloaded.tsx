// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactNode } from "react";

import { formatGib } from "../../lib/format";
import type {
  LocalDiskSource,
  LocalFitResult,
  LocalOnDiskModel,
  LocalRecommendations,
} from "../../lib/types";
import { useDepth } from "../../theme";
import { CommandLine } from "./CommandLine";
import { downloadedState, type DownloadedState } from "./downloadedState";
import { ConfigRow, FitBadge } from "./fitDisplay";
import { onDiskHow } from "./readiness";
import { SectionLink } from "./SectionLink";
import { sectionHelp, sectionLabel } from "./sections";
import { Button, Callout, Collapsible, SectionInfo, SectionLabel } from "../ui";

/** How many models the list shows before the rest fold away. */
const FIRST = 4;

/**
 * "Already on this device" (#449) — the models this device has, whoever put them there, and for each
 * one how to get it served.
 *
 * The whole section, because its copy and its empty states are one argument: an empty list means
 * four different things (nothing downloaded, a runner installed but empty, a folder PM is not
 * allowed to read, no folder at all) and saying the wrong one is how a cosmetic gap becomes a lie
 * about the machine. `downloadedState` is the pure ladder that decides which; this renders it.
 */
export function LocalAiDownloaded({
  recs,
  loading,
  configured,
  onPickFolder,
  onClearFolder,
  error,
}: {
  recs: LocalRecommendations | null;
  loading: boolean;
  configured: boolean;
  onPickFolder: () => void;
  onClearFolder: () => void;
  /** Something in this section went wrong, said here rather than at the top of the tab. */
  error?: string | null;
}) {
  const { showMeta } = useDepth();
  return (
    <div
      id="sec-localai-downloaded"
      data-settings-section
      data-help={sectionHelp("sec-localai-downloaded")}
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel
        align="baseline"
        action={
          showMeta &&
          !loading &&
          recs &&
          recs.installed.length + recs.on_disk.length > 0 && (
            // Both halves. `on_disk` is only the models NOTHING is serving, so counting it alone
            // read "0 downloaded" to anyone whose server holds everything they have — which is
            // every Ollama user, since `/v1/models` lists what has been pulled, not what is
            // loaded. The two sets are disjoint by construction (`already_served` filters one out
            // of the other), so this is a sum and not a union. No "on this device": the endpoint
            // may not be one.
            <span className="shrink-0 text-[0.6875rem] text-ink4">
              {recs.installed.length + recs.on_disk.length} downloaded
            </span>
          )
        }
      >
        {sectionLabel("sec-localai-downloaded")}
      </SectionLabel>
      {error && <Callout className="mt-2">{error}</Callout>}
      {loading ? (
        <p className="mt-2 text-xs text-ink4">Looking for downloaded models…</p>
      ) : recs ? (
        <DownloadedModels
          recs={recs}
          configured={configured}
          onPickFolder={onPickFolder}
          onClearFolder={onClearFolder}
        />
      ) : (
        <p className="mt-2 text-xs text-ink4">Couldn't check for downloaded models.</p>
      )}
      <SectionInfo title="Where PM looks, and what it reads">
        <p>
          PM checks the folders {SUPPORTED_RUNTIMES} keep their models in, so a model you've
          downloaded but aren't currently running still gets sized against your machine — and it
          asks your connected server what it holds, which on Linux is the only way to see a store
          the server owns as its own user.
        </p>
        <p>
          A folder PM finds but isn't allowed to read is said so plainly, rather than reported as a
          folder that isn't there. The two look identical to the operating system and they are not
          the same thing.
        </p>
        <p>
          It reads <span className="text-ink2">file names and sizes only</span> — never the contents
          of a model file — it writes nothing, and none of it leaves this device. Models it doesn't
          recognise are listed with an honest “can't estimate this” rather than a guess.
        </p>
      </SectionInfo>
    </div>
  );
}

/** The runners PM can find models for, named in one place so the copy can't drift from the crawl. */
const SUPPORTED_RUNTIMES = "Ollama, LM Studio and Hugging Face";

const DISK_SOURCE_LABEL: Record<LocalDiskSource, string> = {
  ollama: "Ollama",
  hugging_face: "Hugging Face",
  lm_studio: "LM Studio",
  folder: "Your folder",
};

/** The one sentence for each state that isn't a list. Split out so the copy sits beside the ladder's
 *  reasoning instead of inside a nested ternary, and so each branch can be read against the machine
 *  state it describes. Sections are named, never pointed at "above" or "below". */
function emptyCopy(state: Exclude<DownloadedState, { kind: "list" }>): ReactNode {
  switch (state.kind) {
    case "endpointHasAll": {
      const one = state.count === 1;
      return (
        <>
          Your server has {state.count} model{one ? "" : "s"} downloaded, and PM can see{" "}
          {one ? "it" : "them all"} — you can give {one ? "it" : "them"} a job under{" "}
          <SectionLink to="sec-localai-roles" />.
        </>
      );
    }
    case "allServed":
      return `Found ${listJoin(
        state.runners,
      )} on this device, with nothing downloaded that isn't already being served.`;
    case "folderEmpty":
      return `Found ${listJoin(state.runners)} on this device, but nothing downloaded into it yet.`;
    case "endpointEmpty":
      return (
        <>
          Your server is running, but nothing has been downloaded into it yet —{" "}
          <SectionLink to="sec-localai-start" /> suggests one for this computer.
        </>
      );
    case "blocked":
      // Never suggests changing the permissions. The store belongs to a service account, and telling
      // someone to loosen one so a settings panel can count files would be a bad trade PM has no
      // business proposing. Connecting the server gets the same answer and costs nothing.
      return state.root.source === "folder" ? (
        `PM isn't allowed to read the folder you pointed it at (${state.root.path}), so it can't say what's in there.`
      ) : (
        <>
          {DISK_SOURCE_LABEL[state.root.source]} keeps its models at {state.root.path}, and PM isn't
          allowed to read that folder — the packaged Linux server owns its store as its own user,
          which is normal and nothing is wrong. Connect it under{" "}
          <SectionLink to="sec-localai-endpoint" /> and PM will ask the server what it has instead.
        </>
      );
    case "noFolder":
      return `No model folder found for ${SUPPORTED_RUNTIMES}. If your models live somewhere else, point PM at that folder below.`;
  }
}

/** Models found on disk that no endpoint is serving. Distinguishes "we looked and this runner has
 *  nothing" from "this runner isn't on this machine" — an empty list means different things.
 *
 *  Exported for its test: the "you can't pick these yet" line is a GATING hint, not prose, so the
 *  settings doctrine keeps it unfolded — a test pins it rather than trusting it not to drift. */
export function DownloadedModels({
  recs,
  configured,
  onPickFolder,
  onClearFolder,
}: {
  recs: LocalRecommendations;
  /** An endpoint is saved. Decides which half of the gating hint applies. */
  configured: boolean;
  onPickFolder: () => void;
  onClearFolder: () => void;
}) {
  const { showMeta } = useDepth();
  const found = recs.disk_sources_present
    .filter((s) => s !== "folder")
    .map((s) => DISK_SOURCE_LABEL[s]);
  // Seven states, resolved in a pure module. The branch a stock Linux install lands in cannot be
  // reached from a render test without a service account, so the ladder is tested on its own.
  const state = downloadedState({
    unservedCount: recs.on_disk.length,
    endpointInventory: recs.endpoint_inventory,
    foundRunners: found,
    diskFound: recs.disk_found,
    blocked: recs.disk_blocked,
  });

  return (
    <div className="mt-2">
      {state.kind !== "list" ? (
        <p className="text-xs text-ink4">{emptyCopy(state)}</p>
      ) : (
        <>
          <p className="mb-2 text-xs text-ink4">
            {configured ? (
              <>
                None of these can be assigned yet — PM can only use a model your server is actually
                serving. Each one says how to get it served; it then shows up under{" "}
                <SectionLink to="sec-localai-roles" /> within about half a minute.
              </>
            ) : (
              <>
                This is what's on your device, not what PM can use yet. Connect your server under{" "}
                <SectionLink to="sec-localai-endpoint" /> first; each model says how to get it
                served, and it then appears under <SectionLink to="sec-localai-roles" />.
              </>
            )}
          </p>
          {/* The first few, then the rest folded — no inner scroller, so the wheel moves the tab. */}
          <div className="space-y-2">
            {recs.on_disk.slice(0, FIRST).map((m) => (
              <OnDiskCard key={`${m.source}:${m.path}:${m.name}`} model={m} />
            ))}
          </div>
          {recs.on_disk.length > FIRST && (
            <Collapsible
              title={`Show the other ${recs.on_disk.length - FIRST}`}
              defaultOpen={false}
              className="mt-2"
            >
              <div className="mt-2 space-y-2">
                {recs.on_disk.slice(FIRST).map((m) => (
                  <OnDiskCard key={`${m.source}:${m.path}:${m.name}`} model={m} />
                ))}
              </div>
            </Collapsible>
          )}
          {showMeta && found.length > 0 && (
            <p className="mt-2 text-xs text-ink4">Found via {listJoin(found)}.</p>
          )}
        </>
      )}

      {recs.disk_truncated && (
        <p className="mt-2 text-xs text-ink4">
          PM stopped after the first few hundred models, so this list isn't everything on your
          device.
        </p>
      )}

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Button variant="tertiary" size="sm" onClick={onPickFolder}>
          {recs.scan_dir ? "Change folder…" : "Also look in a folder…"}
        </Button>
        {recs.scan_dir && (
          <>
            <span className="min-w-0 break-all text-xs text-ink4">{recs.scan_dir}</span>
            <Button variant="tertiary" size="sm" onClick={onClearFolder}>
              Stop looking there
            </Button>
          </>
        )}
      </div>
    </div>
  );
}

/** Where a fit runs, from what its speed was worked out from — the card's config row says it rather
 *  than assuming system memory for a model that sits on the graphics card. */
function runsIn(fit: LocalFitResult): string {
  switch (fit.speed_basis) {
    case "gpu_published":
    case "gpu_typical":
      return "On your graphics card";
    case "shared":
      return "In shared memory";
    default:
      return "In system memory";
  }
}

function OnDiskCard({ model }: { model: LocalOnDiskModel }) {
  const { showMeta } = useDepth();
  // The same words the start card uses for PM's pick when it is a file on this computer.
  const how = onDiskHow(model.source, model.shards, model.path, model.fit);
  return (
    <div className="rounded-[var(--radius-sm)] border border-border px-3 py-2">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <span className="min-w-0 break-all text-sm text-ink2">{model.name}</span>
        <FitBadge verdict={model.fit.verdict} />
      </div>
      <p className="mt-0.5 text-xs text-ink4">
        {DISK_SOURCE_LABEL[model.source]} · {formatGib(model.size_gb)}
        {showMeta && model.quant ? ` · ${model.quant}` : ""}
        {showMeta && model.shards > 1 ? ` · ${model.shards} files` : ""}
      </p>
      <ConfigRow label={runsIn(model.fit)} fit={model.fit} />
      {model.fit.notes.map((n, i) => (
        <p key={i} className="mt-1 text-xs text-ink4">
          {n}
        </p>
      ))}
      <p className="mt-1 text-xs text-ink3">To use it here: {how.line}</p>
      {how.command && <CommandLine command={how.command} />}
    </div>
  );
}

/** "a, b and c" — the Oxford-free list join the rest of PM's copy uses. */
function listJoin(items: string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

// ── Small pieces ──────────────────────────────────────────────────────────────────────────────
