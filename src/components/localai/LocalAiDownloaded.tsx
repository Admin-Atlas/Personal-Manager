// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { formatGib } from "../../lib/format";
import type { LocalDiskSource, LocalOnDiskModel, LocalRecommendations } from "../../lib/types";
import { downloadedState, type DownloadedState } from "./downloadedState";
import { ConfigRow, FitBadge } from "./fitDisplay";
import { Button, SectionInfo, SectionLabel } from "../ui";

/**
 * "Already downloaded" (#449) — the models this device has, whoever put them there.
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
}: {
  recs: LocalRecommendations | null;
  loading: boolean;
  configured: boolean;
  onPickFolder: () => void;
  onClearFolder: () => void;
}) {
  return (
    <div
      id="sec-localai-downloaded"
      data-settings-section
      data-help="settings-localai-downloaded"
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel
        align="baseline"
        action={
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
        Already downloaded
      </SectionLabel>
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
 *  state it describes. */
function emptyCopy(state: Exclude<DownloadedState, { kind: "list" }>): string {
  switch (state.kind) {
    case "endpointHasAll": {
      const one = state.count === 1;
      return `Your server has ${state.count} model${one ? "" : "s"} downloaded, and PM can see ${
        one ? "it" : "them all"
      } — ${one ? "it's" : "they're"} listed under Assign roles above.`;
    }
    case "allServed":
      return `Found ${listJoin(
        state.runners,
      )} on this device, with nothing downloaded that isn't already being served.`;
    case "folderEmpty":
      return `Found ${listJoin(state.runners)} on this device, but nothing downloaded into it yet.`;
    case "endpointEmpty":
      return "Your server is running, but nothing has been downloaded into it yet — pick one from Recommended models above.";
    case "blocked":
      // Never suggests changing the permissions. The store belongs to a service account, and telling
      // someone to loosen one so a settings panel can count files would be a bad trade PM has no
      // business proposing. Connecting the server gets the same answer and costs nothing.
      return state.root.source === "folder"
        ? `PM isn't allowed to read the folder you pointed it at (${state.root.path}), so it can't say what's in there.`
        : `${DISK_SOURCE_LABEL[state.root.source]} keeps its models at ${
            state.root.path
          }, and PM isn't allowed to read that folder — the packaged Linux server owns its store as its own user, which is normal and nothing is wrong. Connect it below and PM will ask the server what it has instead.`;
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
            {configured
              ? "None of these can be assigned yet — PM can only use a model your endpoint is actually serving. Load one in the app you downloaded it with and it shows up under Assign roles above within about half a minute."
              : "This is what's on your device, not what PM can use yet. Connect an endpoint above, then load the model in the app you downloaded it with, and it appears under Assign roles."}
          </p>
          <div className="max-h-72 space-y-2 overflow-y-auto pr-1">
            {recs.on_disk.map((m) => (
              <OnDiskCard key={`${m.source}:${m.path}:${m.name}`} model={m} />
            ))}
          </div>
          {found.length > 0 && (
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

function OnDiskCard({ model }: { model: LocalOnDiskModel }) {
  return (
    <div className="rounded-[var(--radius-sm)] border border-border px-3 py-2">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <span className="min-w-0 break-all text-sm text-ink2">{model.name}</span>
        <FitBadge verdict={model.fit.verdict} />
      </div>
      <p className="mt-0.5 text-xs text-ink4">
        {DISK_SOURCE_LABEL[model.source]} · {formatGib(model.size_gb)}
        {model.quant ? ` · ${model.quant}` : ""}
        {model.shards > 1 ? ` · ${model.shards} files` : ""}
      </p>
      <ConfigRow label="In system memory" fit={model.fit} />
      {model.fit.notes.map((n, i) => (
        <p key={i} className="mt-1 text-xs text-ink4">
          {n}
        </p>
      ))}
    </div>
  );
}

/** "a, b and c" — the Oxford-free list join the rest of PM's copy uses. */
function listJoin(items: string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

// ── Small pieces ──────────────────────────────────────────────────────────────────────────────
