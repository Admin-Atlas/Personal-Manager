// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Dev-time generator for the curated local-model catalog (#296) shipped at
// `src-tauri/local_models.json`. It refreshes a small, hand-picked SEED of GGUF repos from the
// Hugging Face API — real per-quant file sizes, architecture, context window, and, read out of the
// GGUF header, every model's KV-cache geometry, (for MoE models) the active-parameter count, and
// per quant the bytes one decode step reads from its tensor table — so the app can size each model
// against a user's hardware with `fit.rs`.
//
// Run it by hand (`just generate-local-catalog`); it is NOT part of the PR check gate (network, rate
// limits, non-determinism). A scheduled Action that runs it and opens a PR is a fast-follow.
//
// Dependency note (a CONSCIOUS exception, not drift): `scripts/` is zero-dependency by habit, but this
// one script imports `@huggingface/gguf` (a dev-only devDependency, never shipped, never in the CI
// gate). Reading MoE expert counts out of a binary GGUF header is exactly the "maintained format
// library prevents a correctness bug we can't cheaply verify by hand" case that clears the bar. The
// bar still stands for the next dep. Transitive tree is two first-party MIT packages, no onward deps.
//
// Idempotent: it re-hashes the entries and rewrites `local_models.json` ONLY when the content changed
// (so a scheduled run opens a PR only on a real diff). `generated_at`/`catalog_version` advance only
// on a real change. Never reads HF_TOKEN — an accidental CI secret must not authenticate the catalog.
//
// All-or-nothing on fetch failure: a transient Hugging Face outage (network drop / HTTP 5xx) is
// retried, and if it persists the run ABORTS without writing — it never emits a shorter catalog. A
// dropped seed would delete a model AND bump `catalog_version`, so a blip must not masquerade as a
// real update. Only a model that fetched fine but doesn't qualify (embedding, no curated quant) drops.

import { gguf, GGMLQuantizationType as GGML } from "@huggingface/gguf";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";

const HF = "https://huggingface.co";
const UA = "pm-local-catalog-generator (Personal-Manager)";
const DEFAULT_QUANTS = ["Q3_K_M", "Q4_K_M", "Q5_K_M", "Q6_K", "Q8_0"];
const SCHEMA_VERSION = 4;
// Bounded retries for transient Hugging Face failures (network drop / HTTP 5xx) before we give up.
const MAX_ATTEMPTS = 4;

// Thrown when a curated seed can't be fetched/verified (transient outage or a permanent 4xx). It
// ABORTS the whole run rather than emit a smaller catalog — a dropped seed would delete a model AND
// bump `catalog_version`, nudging every user to rescan over a Hugging Face blip. Distinct from a
// model that fetched fine but doesn't qualify (embedding, no curated quant), which is a clean drop.
class AbortRun extends Error {}

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const outPath = join(repoRoot, "src-tauri", "local_models.json");
const ledgerPath = join(repoRoot, "src-tauri", "model_licences.json");

// The curated SEED — verified-real GGUF repos spanning small→large, dense + MoE + multimodal, from
// reputable quantizers (bartowski / unsloth / ggml-org) — now lives in `src-tauri/model_licences.json`
// as the keys of its `models` map. `sort=downloads` discovery is still a maintainer aid for finding
// new entries (see --discover), never an auto-append, so diffs stay reviewable.
//
// It moved there so a model CANNOT be catalogued without a licence row: one list, no second list to
// drift from it. The other half of the reason is `just model-licences`, the offline gate — it has to
// read the seed, and it cannot import this file, because the `@huggingface/gguf` import above
// resolves at module load and pr.yml's `hygiene` job runs with no `npm ci` (INVARIANTS.md I-18). A
// gate that imported this would pass on a dev box and die only in CI.
function readLedger() {
  const ledger = JSON.parse(readFileSync(ledgerPath, "utf8"));
  const seed = Object.entries(ledger.models).map(([repo, row]) => ({ repo, role: row.role }));
  return { ledger, seed };
}

/**
 * The upstream fields worth watching, in a stable shape so the stamp only moves when the licence
 * story does. Descriptions, download counts and file lists are deliberately not in here.
 */
export function evidenceOf(info) {
  const card = info?.cardData ?? {};
  return {
    license: card.license ?? null,
    license_name: card.license_name ?? null,
    license_link: card.license_link ?? null,
    gated: info?.gated ?? null,
    tags: (info?.tags ?? []).filter((t) => t.startsWith("license:")).sort(),
  };
}

export function stampOf(evidence) {
  return createHash("sha256").update(JSON.stringify(evidence), "utf8").digest("hex").slice(0, 32);
}

/**
 * Fold this run's evidence into the ledger and decide whether a human still stands behind each
 * licence. A hand-written row that has never been stamped is adopted once — whoever wrote it wrote
 * it against the upstream of the day. After that the stamp governs: when it moves, the licence is
 * BLANKED and the old answer is kept under `previous` rather than carried forward silently.
 */
export function reconcileLedger(ledger, evidenceByRepo) {
  const models = {};
  const review = [];
  const changed = [];
  for (const [repo, row] of Object.entries(ledger.models)) {
    const evidence = evidenceByRepo.get(repo);
    if (!evidence) {
      // No evidence this run (the fetch never got far enough). Leave the row exactly as it was —
      // silence is not a reason to withdraw a licence someone already decided.
      models[repo] = row;
      if (!row.licence) review.push(repo);
      continue;
    }
    const stamp = stampOf(evidence);
    const firstStamping = row.licence != null && row.stamp == null;
    const keep = firstStamping || row.stamp === stamp;
    const next = { role: row.role, licence: keep ? row.licence : null, stamp, evidence };
    if (keep && row.note) next.note = row.note;
    if (!keep) {
      next.previous = { licence: row.licence, stamp: row.stamp ?? null };
      changed.push(repo);
    }
    if (!next.licence) review.push(repo);
    models[repo] = next;
  }
  return { ledger: { ...ledger, models }, review, changed };
}

/** The block the catalogue carries per entry: everything the app needs without a second lookup. */
export function licenceFor(ledger, repo) {
  const id = ledger.models[repo]?.licence;
  const term = id ? ledger.terms[id] : null;
  if (!term) return null;
  return { id, name: term.name, url: term.url, open: term.open, summary: term.summary };
}

// --- HTTP with the mandatory rate-limit clear-error --------------------------------------------

// Fetch with bounded retries on transient failures. A network drop or an HTTP 5xx is retried a few
// times with backoff, then — if it still fails — throws `AbortRun` so the run stops WITHOUT writing a
// degraded catalog. A 429 is never retried (never spin a rate limit): one clear error, exit. A 2xx/3xx
// or a 4xx is returned for the caller to judge (a 4xx on a curated seed is fatal there).
async function hfFetch(url, extraHeaders = {}) {
  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
    let res;
    try {
      res = await fetch(url, { headers: { "User-Agent": UA, ...extraHeaders } });
    } catch (e) {
      const reason = e?.message || String(e);
      if (attempt < MAX_ATTEMPTS) {
        console.warn(`    ${url} → ${reason}, retry ${attempt}/${MAX_ATTEMPTS - 1} …`);
        await sleep(backoffMs(attempt));
        continue;
      }
      throw new AbortRun(`${url} failed after ${MAX_ATTEMPTS} attempts (${reason})`);
    }
    if (res.status === 429) {
      const retry = parseRateLimit(res.headers);
      console.error(
        `generate-local-catalog: Hugging Face rate-limited this IP (HTTP 429).\n` +
          `Retry in ${retry}s. This is a dev/CI tool — do NOT set HF_TOKEN to work around it.`,
      );
      process.exit(2);
    }
    if (res.status >= 500) {
      if (attempt < MAX_ATTEMPTS) {
        console.warn(`    ${url} → HTTP ${res.status}, retry ${attempt}/${MAX_ATTEMPTS - 1} …`);
        await sleep(backoffMs(attempt));
        continue;
      }
      throw new AbortRun(`${url}: HTTP ${res.status} after ${MAX_ATTEMPTS} attempts`);
    }
    return res;
  }
  // Unreachable — the loop either returns a response or throws — but keeps the type checker honest.
  throw new AbortRun(`${url}: exhausted retries`);
}

// Exponential backoff: 500ms, 1s, 2s. A dev tool, so a few seconds of waiting out a blip is fine.
export function backoffMs(attempt) {
  return 500 * 2 ** (attempt - 1);
}
function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// The `RateLimit` header carries `t=<seconds>`; fall back to `Retry-After`, then a plain default.
export function parseRateLimit(headers) {
  const rl = headers.get("ratelimit") || headers.get("RateLimit") || "";
  const m = /(?:^|[;,\s])t=(\d+)/i.exec(rl);
  if (m) return Number(m[1]);
  const ra = headers.get("retry-after");
  if (ra && /^\d+$/.test(ra.trim())) return Number(ra.trim());
  return 300;
}

// --- per-repo assembly -------------------------------------------------------------------------

async function buildEntry(seed) {
  const { repo, role } = seed;

  // 1. Inline GGUF metadata: total params, architecture, context window.
  // `cardData` + `gated` ride along with the gguf metadata — one request, not two. They are
  // EVIDENCE for the licence ledger, never the decision: see readLedger() above.
  const infoRes = await hfFetch(
    `${HF}/api/models/${repo}?expand[]=gguf&expand[]=cardData&expand[]=gated&expand[]=tags`,
  );
  if (!infoRes.ok) {
    // 5xx already retried+threw in hfFetch; a 4xx here means a curated seed is gone/renamed — a
    // maintenance signal, not a silent drop. Abort so the SEED gets fixed rather than shipped short.
    throw new AbortRun(
      `curated seed ${repo}: model info HTTP ${infoRes.status} — fix SEED or retry`,
    );
  }
  const info = await infoRes.json();
  // Captured BEFORE the qualification checks below: a repo can fail to qualify as a catalogue entry
  // (embedding model, no curated quant, unreadable MoE header) and still need its licence reviewed,
  // because it stays in the seed and can start qualifying at any time.
  const evidence = evidenceOf(info);
  const drop = (why) => {
    console.warn(`  skip ${repo}: ${why}`);
    return { entry: null, evidence };
  };
  const g = info.gguf || {};
  const totalParams = Number(g.total);
  const architecture = g.architecture ? String(g.architecture) : null;
  const contextLength = Number(g.context_length);
  if (
    !Number.isFinite(totalParams) ||
    totalParams <= 0 ||
    !architecture ||
    !Number.isFinite(contextLength)
  ) {
    return drop("incomplete GGUF metadata (params/arch/ctx)");
  }
  if (isEmbeddingOrReranker(repo, architecture)) {
    return drop("embedding/reranker (not a chat model)");
  }

  // 2. File tree: per-quant sizes (shards summed) + the mmproj (projector) if any.
  const treeRes = await hfFetch(`${HF}/api/models/${repo}/tree/main?recursive=true`);
  if (!treeRes.ok) {
    // This is the exact case that dropped Meta-Llama-3.1-8B on a 503: abort, never silently shrink.
    throw new AbortRun(`curated seed ${repo}: tree HTTP ${treeRes.status} — fix SEED or retry`);
  }
  const tree = await treeRes.json();
  const ggufFiles = tree.filter((f) => f.type === "file" && /\.gguf$/i.test(f.path));

  const quants = [];
  // The first quant's first shard, read for its decode bytes and reused as step 4's header.
  let header = null;
  for (const label of seed.quants || DEFAULT_QUANTS) {
    const size = sumQuantShards(ggufFiles, label);
    if (!size) continue;
    // `size.bytes` is the raw pre-gib() figure, which is what makes the comparison exact.
    const manifest = size.sharded ? null : await fetchOllamaManifest(repo, label);
    const read = await readQuantDecode(repo, label, quantShardPaths(ggufFiles, label));
    if (quants.length === 0) header = read.metadata;
    quants.push({
      quant: label,
      file_gb: gib(size.bytes),
      sharded: size.sharded,
      ollama: ollamaTagFor({
        repo,
        quant: label,
        sharded: size.sharded,
        bytes: size.bytes,
        manifest,
      }),
      decode_bytes: read.decode.decode_bytes,
      decode_slow_bytes: read.decode.decode_slow_bytes,
    });
  }
  if (quants.length === 0) {
    return drop("none of the curated quants present in the tree");
  }

  // 3. Multimodal projector: an mmproj file makes the model multimodal. A model loads ONE projector
  //    at runtime, so pick a single precision (never sum the F16/F32/BF16 variants).
  const projectorBytes = pickProjector(ggufFiles.filter((f) => /mmproj/i.test(f.path)));
  const multimodal = projectorBytes != null;
  const projectorGb = multimodal ? gib(projectorBytes) : null;

  // 4. The GGUF header, for two things the HF JSON does not carry: a MoE's expert geometry and every
  //    model's attention geometry. Any quant's header will do — both describe the architecture, not
  //    the quantization — so it is the first quant's first shard, already read above for its decode
  //    bytes (`header`).

  // 5. Active params: dense == total; MoE is read from the GGUF header (decision E — never
  //    total×used/count). A MoE we can't parse is EXCLUDED from the curated catalog.
  const looksMoe = isMoe(repo, architecture);
  let activeParams = totalParams;
  let fit = "computed";
  if (looksMoe) {
    const active = header ? activeFromHeader(header, totalParams) : null;
    if (active && active > 0 && active <= totalParams) {
      activeParams = active;
    } else {
      return drop("MoE active-params unreadable from GGUF (decision E)");
    }
  }

  // 6. What a token costs in the KV cache, from the attention geometry. Without it fit.rs falls back
  //    to its params × context proxy, which cannot see grouped-query attention and under-counts a
  //    model without it by an order of magnitude — so a missing figure is said out loud, never quiet.
  const kvCache = header ? kvFromHeader(header) : null;
  if (!kvCache) {
    console.warn(`    no KV geometry for ${repo}: fit.rs keeps the params × context proxy for it`);
  }
  // Unmodelled architectures we can't fit-score: keep the row but mark it honestly.
  if (isUnmodelledArch(architecture)) fit = "unknown";

  const entry = {
    repo,
    display_name: prettyName(repo),
    architecture,
    role_hint: role || null,
    parameters_b: round2(totalParams / 1e9),
    active_parameters_b: round2(activeParams / 1e9),
    context_length: contextLength,
    multimodal,
    reasoning: null,
    projector_gb: projectorGb,
    kv_cache: kvCache,
    fit,
    quants,
  };
  return { entry, evidence };
}

// --- GGUF header parse: MoE active params and attention geometry -------------------------------

// A header range read answered 4xx: the file is gated or gone. Permanent, so never retried.
class HeaderUnavailable extends Error {}

// Read one file's GGUF header — its metadata and its tensor table, never the tensors themselves —
// over HTTP range requests, as `{ metadata, tensorInfos }`, or `null`.
//
// Every range read goes through `hfFetch`, so a network drop or a 5xx is retried there and, if it
// persists, ABORTS the run like every other fetch in this file. A parse failure is retried here, in
// case a read came back short. Only what persists past both is `null` — a 4xx (a gated or vanished
// file) or a header `gguf()` cannot parse — and the caller decides what that costs: a MoE is dropped
// (decision E), a dense model keeps the KV proxy, and a quant's decode bytes are written as null. A
// network blip must not masquerade as any of them.
async function readGguf(repo, shardPath) {
  if (!shardPath) return null;
  const url = `${HF}/${repo}/resolve/main/${shardPath}`;
  const viaHf = async (u, init) => {
    const res = await hfFetch(u, init?.headers ?? {});
    if (!res.ok) throw new HeaderUnavailable(`HTTP ${res.status}`);
    return res;
  };
  for (let attempt = 1; ; attempt++) {
    try {
      const { metadata, tensorInfos } = await gguf(url, { fetch: viaHf });
      return { metadata, tensorInfos };
    } catch (e) {
      if (e instanceof AbortRun) throw e;
      if (!(e instanceof HeaderUnavailable) && attempt < MAX_ATTEMPTS) {
        await sleep(backoffMs(attempt));
        continue;
      }
      console.warn(`    GGUF header unreadable for ${repo}: ${e?.message || e}`);
      return null;
    }
  }
}

/** Read every shard of one quant and work out its decode bytes (`decodeBytes`), returning them
 *  with the first shard's metadata — the header step 4 reads the model's geometry from.
 *
 *  A shard PM could not read, or a tensor table `decodeBytes` refuses, writes `null` for both fields
 *  and says so: fit.rs then falls back to the parameter count for that quant. The row itself is never
 *  dropped — its size and its tag were measured from the tree, not the header — and a network blip
 *  still aborts the run inside `readGguf`, as everywhere else. */
async function readQuantDecode(repo, label, paths) {
  const unread = { decode_bytes: null, decode_slow_bytes: null };
  const shards = [];
  for (const path of paths) {
    const shard = await readGguf(repo, path);
    if (!shard) {
      console.warn(`    no decode bytes for ${repo} ${label}: ${path} unreadable`);
      return { metadata: shards[0]?.metadata ?? null, decode: unread };
    }
    shards.push(shard);
  }
  const metadata = shards[0]?.metadata ?? null;
  if (!metadata) return { metadata, decode: unread };
  try {
    return {
      metadata,
      decode: decodeBytes(
        shards.flatMap((s) => s.tensorInfos),
        metadata,
      ),
    };
  } catch (e) {
    console.warn(`    no decode bytes for ${repo} ${label}: ${e?.message || e}`);
    return { metadata, decode: unread };
  }
}

/** GGML tensor type id → `[elements per block, bytes per block]`: ggml's own `blck_size` and
 *  `type_size` for every type a catalogue file can carry (ggml/src/ggml.c `type_traits`). A tensor
 *  occupies its element count over the first, times the second. */
export const GGML_BLOCK = {
  [GGML.F32]: [1, 4],
  [GGML.F16]: [1, 2],
  [GGML.Q4_0]: [32, 18],
  [GGML.Q4_1]: [32, 20],
  [GGML.Q5_0]: [32, 22],
  [GGML.Q5_1]: [32, 24],
  [GGML.Q8_0]: [32, 34],
  [GGML.Q8_1]: [32, 36],
  [GGML.Q2_K]: [256, 84],
  [GGML.Q3_K]: [256, 110],
  [GGML.Q4_K]: [256, 144],
  [GGML.Q5_K]: [256, 176],
  [GGML.Q6_K]: [256, 210],
  [GGML.Q8_K]: [256, 292],
  [GGML.IQ2_XXS]: [256, 66],
  [GGML.IQ2_XS]: [256, 74],
  [GGML.IQ3_XXS]: [256, 98],
  [GGML.IQ1_S]: [256, 50],
  [GGML.IQ4_NL]: [32, 18],
  [GGML.IQ3_S]: [256, 110],
  [GGML.IQ2_S]: [256, 82],
  [GGML.IQ4_XS]: [256, 136],
  [GGML.I8]: [1, 1],
  [GGML.I16]: [1, 2],
  [GGML.I32]: [1, 4],
  [GGML.I64]: [1, 8],
  [GGML.F64]: [1, 8],
  [GGML.IQ1_M]: [256, 56],
  [GGML.BF16]: [1, 2],
  [GGML.MXFP4]: [32, 17],
};

/** The tensor types fit.rs charges as slow to unpack (`decode_slow_bytes`, `GPU_SLOW_BYTE_COST`):
 *  Q2_K, Q3_K and the i-quants. Only Q3_K is measured — the three Q3_K_M models timed on the dev
 *  laptop, whose Q3_K bytes streamed at about 38% of the card's bandwidth against 56% for the rest.
 *  The others are assumed by kinship with it, the other k-quant below Q4_K and the i-quants, and
 *  none of them has been timed. */
export const SLOW_TENSOR_TYPES = new Set([
  GGML.Q2_K,
  GGML.Q3_K,
  GGML.IQ2_XXS,
  GGML.IQ2_XS,
  GGML.IQ3_XXS,
  GGML.IQ1_S,
  GGML.IQ4_NL,
  GGML.IQ3_S,
  GGML.IQ2_S,
  GGML.IQ4_XS,
  GGML.IQ1_M,
]);

/** The bytes one decode step reads, from a quant's tensor table (every shard's, concatenated) and
 *  its first shard's metadata: `{ decode_bytes, decode_slow_bytes }`, integers, the second the part
 *  of the first in `SLOW_TENSOR_TYPES`. What fit.rs divides a card's bandwidth by, in place of
 *  active params × bytes per param, which read 1.25-1.63x too few bytes for both catalogue MoEs.
 *
 *  Every tensor streams in full, at its element count over its block size times its block bytes,
 *  with three exceptions, each the way llama.cpp's decode reads it:
 *    * `token_embd.weight` is one row looked up per token, so it counts only when there is no
 *      `output.weight` — a tied embedding IS the output head, read whole every token;
 *    * a `per_layer_token_embd` table (gemma 3n, gemma 4) is looked up a row at a time, so it
 *      counts nothing;
 *    * a routed-expert tensor (`*_exps.*`) counts at `expert_used_count / expert_count`, the share
 *      of experts a token reaches. Shared experts (`*_shexp.*`) run on every token and count in full.
 *  A type `GGML_BLOCK` does not know, or routed experts without their counts, THROWS rather than
 *  guess: the caller writes null for that quant. */
export function decodeBytes(tensorInfos, metadata) {
  const arch = String(metadata["general.architecture"] || "");
  const tied = !tensorInfos.some((t) => t.name === "output.weight");
  let expertShare;
  const routedShare = () => {
    if (expertShare === undefined) {
      const count = Number(metadata[`${arch}.expert_count`]);
      const used = Number(metadata[`${arch}.expert_used_count`]);
      if (!(count > 0 && used > 0 && used <= count)) {
        throw new Error(`routed experts without usable ${arch}.expert_* counts`);
      }
      expertShare = used / count;
    }
    return expertShare;
  };
  let total = 0;
  let slow = 0;
  for (const t of tensorInfos) {
    const block = GGML_BLOCK[t.dtype];
    if (!block) throw new Error(`${t.name}: unknown GGML tensor type ${t.dtype}`);
    const [elements, bytes] = block;
    const n = t.shape.reduce((a, d) => a * Number(d), 1);
    let streamed = (n / elements) * bytes;
    if (t.name === "token_embd.weight") {
      if (!tied) streamed = 0;
    } else if (/per_layer_token_embd/.test(t.name)) {
      streamed = 0;
    } else if (/_exps\./.test(t.name)) {
      streamed *= routedShare();
    }
    total += streamed;
    if (SLOW_TENSOR_TYPES.has(t.dtype)) slow += streamed;
  }
  return { decode_bytes: Math.round(total), decode_slow_bytes: Math.round(slow) };
}

/** The MoE active-parameter arithmetic, split out of the fetch so it can be tested without a network
 *  call. Returns null whenever the header does not carry a usable, complete set of counts — decision
 *  E: an unreadable MoE header EXCLUDES the model rather than guessing at its active size. */
export function activeFromHeader(metadata, totalParams) {
  const arch = String(metadata["general.architecture"] || "");
  const key = (k) => Number(metadata[`${arch}.${k}`] ?? metadata[k]);
  const nExpert = key("expert_count");
  const nUsed = key("expert_used_count");
  const nBlock = key("block_count");
  const dModel = key("embedding_length");
  const dFfn = key("expert_feed_forward_length");
  if (
    ![nExpert, nUsed, nBlock, dModel, dFfn].every(Number.isFinite) ||
    nExpert <= 0 ||
    nUsed <= 0
  ) {
    return null;
  }
  if (!Number.isFinite(totalParams) || totalParams <= 0) return null;
  // Params living in the INACTIVE experts (3 matrices — gate/up/down — per block), which the decoder
  // never reads for a given token: subtract them from the total to get the active count.
  const inactive = (nExpert - nUsed) * nBlock * 3 * dModel * dFfn;
  const active = totalParams - inactive;
  return active > 0 ? active : null;
}

/** Architectures whose sliding-window layers llama.cpp places by a fixed rule instead of a header
 *  key — `set_swa_pattern(n)` in its loader (src/llama-model.cpp, src/llama-hparams.cpp): layer `i`
 *  slides when `i % n < n - 1`. So gemma2 alternates, and gemma3 slides five layers in every six. */
const SWA_EVERY = { gemma2: 2, gemma3: 6 };

/** What one token costs this model's KV cache, read from its GGUF header: the attention geometry the
 *  `params × context` proxy in fit.rs cannot see. `null` when the header lacks a usable layer count
 *  or head geometry, and fit.rs then keeps the proxy for that entry.
 *
 *  Every layer that keeps a cache stores `(key_length + value_length) × head_count_kv` values a
 *  token, two bytes each at f16 — llama.cpp's `n_embd_k_gqa + n_embd_v_gqa`. The fallbacks are
 *  llama.cpp's own: `key_length`/`value_length` default to `embedding_length / head_count`, and
 *  `head_count_kv` to `head_count`, which is the no-grouped-query-attention case (Phi 3.5 mini). Any
 *  of them may be one number or one per layer (gemma4's `head_count_kv`).
 *
 *  Which layers pay for the whole context comes from the header where the header says it, and from
 *  llama.cpp's loader where it does not:
 *    * Sliding-window layers hold only the last `attention.sliding_window` tokens, so they are
 *      counted apart (`window_bytes_per_token`) and fit.rs caps them at the window. gemma4 names
 *      them in `attention.sliding_window_pattern` (true = sliding), with their own
 *      `key_length_swa`/`value_length_swa`. gemma2 and gemma3 carry the window but not the pattern,
 *      so `SWA_EVERY` supplies it. phi3 carries a window as well, but llama.cpp switches SWA off
 *      for phi3, so its layers count in full — as do every other architecture's, the direction that
 *      can only over-count.
 *    * Hybrid linear-attention models (qwen35, qwen35moe) keep a cache only on every
 *      `full_attention_interval`-th layer: llama.cpp marks layer `i` recurrent when
 *      `(i + 1) % interval != 0`. A recurrent layer holds a fixed state instead, sized as llama.cpp
 *      sizes it from the `ssm.*` keys — `n_embd_r + n_embd_s` values, f32 — which no context or
 *      cache precision changes (`state_bytes`).
 *    * The last `attention.shared_kv_layers` layers (gemma3n, gemma4) reuse an earlier layer's cache
 *      and add none of their own.
 *
 *  Measured against a live Ollama 0.33 (q8_0 cache, flash attention, RTX 5060 Laptop GPU): raising
 *  num_ctx from 8192 to 32768 grew the card's use by 31.5 KB a token for Qwen2.5 7B, against the
 *  30.5 KB this geometry gives at q8_0, and by 10.1 KB for gemma 3 4b against 10.9 KB for its five
 *  full layers — its 29 sliding layers did not grow at all. */
export function kvFromHeader(metadata) {
  const arch = String(metadata["general.architecture"] || "");
  const raw = (k) => metadata[`${arch}.${k}`] ?? metadata[k];
  const num = (v) => (v === undefined || v === null ? undefined : Number(v));
  // One value for every layer, or that layer's entry in a per-layer array.
  const at = (k, i) => {
    const v = raw(k);
    return num(Array.isArray(v) ? v[i] : v);
  };
  const usable = (x) => Number.isFinite(x) && x >= 0;

  const nLayer = num(raw("block_count"));
  if (!Number.isInteger(nLayer) || nLayer <= 0) return null;
  const nEmbd = num(raw("embedding_length"));

  const window = num(raw("attention.sliding_window"));
  const pattern = raw("attention.sliding_window_pattern");
  const every = SWA_EVERY[arch];
  const slides = (i) => {
    if (!Number.isInteger(window) || window <= 0) return false;
    if (Array.isArray(pattern)) return Boolean(pattern[i]);
    return every ? i % every < every - 1 : false;
  };
  const interval = num(raw("full_attention_interval"));
  const attends = (i) => !(Number.isInteger(interval) && interval > 0) || (i + 1) % interval === 0;
  const shared = num(raw("attention.shared_kv_layers"));
  const ownsCache = (i) => !(Number.isInteger(shared) && shared > 0 && i >= nLayer - shared);

  let full = 0;
  let windowed = 0;
  let recurrent = 0;
  for (let i = 0; i < nLayer; i++) {
    if (!ownsCache(i)) continue;
    if (!attends(i)) {
      recurrent += 1;
      continue;
    }
    const nHead = at("attention.head_count", i);
    const nHeadKv = at("attention.head_count_kv", i) ?? nHead;
    const sliding = slides(i);
    const dim = (key) =>
      (sliding ? at(`attention.${key}_swa`, i) : undefined) ??
      at(`attention.${key}`, i) ??
      nEmbd / nHead;
    const kLen = dim("key_length");
    const vLen = dim("value_length");
    if (![nHeadKv, kLen, vLen].every(usable)) return null;
    const bytes = Math.ceil((kLen + vLen) * nHeadKv * 2);
    if (sliding) windowed += bytes;
    else full += bytes;
  }
  if (full + windowed <= 0) return null;

  let state = 0;
  if (recurrent > 0) {
    const conv = num(raw("ssm.conv_kernel"));
    const inner = num(raw("ssm.inner_size"));
    const dState = num(raw("ssm.state_size"));
    const groups = num(raw("ssm.group_count")) ?? 0;
    if (![conv, inner, dState, groups].every(usable)) return null;
    // llama.cpp's n_embd_r (the convolution state) and n_embd_s (the recurrent state proper).
    const r = Math.max(conv - 1, 0) * (inner + 2 * groups * dState);
    const s = dState * inner;
    state = recurrent * (r + s) * 4;
  }

  return {
    bytes_per_token: full,
    window_bytes_per_token: windowed,
    window: windowed > 0 ? window : null,
    state_bytes: state,
  };
}

// --- small pure helpers ------------------------------------------------------------------------

/** The Ollama pull target for one quant, or `null` when PM must not offer a download for it.
 *
 *  Ollama parses the HOST out of a model name and uses it as the registry, and Hugging Face serves
 *  Ollama-format manifests at `/v2/{repo}/manifests/{QUANT}`. So `hf.co/<repo>:<QUANT>` is a pure
 *  function of two fields the catalogue already carries, and it pulls THE FILE THIS ROW MEASURED —
 *  no curated name list, nothing to drift. Ollama's own library is deliberately not used: its build
 *  of a family is frequently a different conversion (`gemma3:4b-it-q4_k_m` folds the vision tower
 *  into the model layer and runs +34% over this repo's measurement), so a card's fit verdict would
 *  describe a different file from the one the button downloads.
 *
 *  The size check is EXACT, not a tolerance. The manifest's `image.model` layer size is the repo
 *  tree's file size for the same artefact, so anything other than equality means we are looking at
 *  a different file and must not offer it. `null` here means "checked, and not offerable" — never
 *  "nobody looked"; the caller warns rather than aborting, so an unreachable registry degrades the
 *  download button without shrinking the catalogue. */
export function ollamaTagFor({ repo, quant, sharded, bytes, manifest }) {
  // Hugging Face's shim 400s on split GGUF by design ("Ollama does not yet support pulling sharded
  // GGUF via the registry"), so never spend a request — and never render a button that cannot work.
  if (sharded) return null;
  if (!manifest) return null;
  const model = (manifest.layers ?? [])
    .filter((l) => l.mediaType === "application/vnd.ollama.image.model")
    .reduce((n, l) => n + (Number(l.size) || 0), 0);
  if (model === 0 || model !== bytes) return null;
  return `hf.co/${repo}:${quant}`;
}

/** Fetch the Ollama-format manifest Hugging Face serves for one quant, or `null`.
 *
 *  Degrades rather than aborts: a manifest we cannot read costs one Download button, while an
 *  `AbortRun` would cost the whole catalogue. That is the opposite trade from the file tree, which
 *  aborts because a missing tree silently shrinks the catalogue itself. */
async function fetchOllamaManifest(repo, quant) {
  const url = `${HF}/v2/${repo}/manifests/${quant}`;
  let res;
  try {
    res = await hfFetch(url, { Accept: "application/vnd.docker.distribution.manifest.v2+json" });
  } catch (e) {
    console.warn(`    ${url} → ${e?.message || e}; no Ollama tag for this quant`);
    return null;
  }
  if (!res.ok) {
    console.warn(`    ${url} → HTTP ${res.status}; no Ollama tag for this quant`);
    return null;
  }
  try {
    return await res.json();
  } catch {
    console.warn(`    ${url} → unparseable manifest; no Ollama tag for this quant`);
    return null;
  }
}

/** The files that make up one quant's weights, sorted: every shard of it, never its projector or a
 *  draft head. One list for the size `sumQuantShards` sums and the headers `readQuantDecode` reads,
 *  so the decode bytes always describe the file the size does. */
export function quantShardPaths(files, label) {
  return files
    .filter((f) => matchesQuant(f.path, label) && !/mmproj/i.test(f.path) && !isDraftHead(f.path))
    .map((f) => f.path)
    .sort();
}

export function sumQuantShards(files, label) {
  const paths = new Set(quantShardPaths(files, label));
  const parts = files.filter((f) => paths.has(f.path));
  if (parts.length === 0) return null;
  const bytes = parts.reduce((n, f) => n + (Number(f.size) || 0), 0);
  return bytes > 0 ? { bytes, sharded: parts.length > 1 } : null;
}

// A multi-token-prediction draft head — `MTP/mtp-<model>-Q8_0.gguf` in unsloth's Gemma 4 repos — is
// an OPTIONAL second model for speculative decoding, not a shard of the weights. It carries the same
// quant token as the model it accelerates, so it matched `matchesQuant` and summed straight in:
// gemma-4-12b-it Q8_0 shipped as 12.23 GiB against a real 11.80, and gemma-4-26B-A4B-it Q8_0 as
// 25.45 against 25.02 — both also wrongly flagged `sharded`, since two files matched where one
// exists. Same class of mistake as counting an mmproj, and excluded the same way.
export function isDraftHead(path) {
  const segs = path.split("/");
  return (
    /^mtp[._-]/i.test(segs[segs.length - 1]) || segs.slice(0, -1).some((s) => /^mtp$/i.test(s))
  );
}

// A file belongs to `label` only when the label is the EXACT quant token right before `.gguf` (or a
// `-NNNNN-of-NNNNN.gguf` shard suffix), preceded by a separator — so "Q6_K" never also matches
// "Q6_K_L"/"Q6_K_XL", the bug that inflated sizes. Also accepts a per-quant subfolder.
export function matchesQuant(path, label) {
  const l = label.toLowerCase();
  const segs = path.toLowerCase().split("/");
  const name = segs[segs.length - 1];
  const re = new RegExp(`[._-]${escapeRe(l)}(?:-\\d{5}-of-\\d{5})?\\.gguf$`, "i");
  if (re.test(name)) return true;
  return segs.slice(0, -1).includes(l);
}

export function escapeRe(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// A multimodal model needs ONE projector at load time — prefer the F16 mmproj (the common default),
// else the smallest available. Never sum precisions. Returns bytes, or null if none/zero-sized.
export function pickProjector(files) {
  if (files.length === 0) return null;
  const f16 = files.find((f) => /mmproj[-._]?f16/i.test(f.path));
  const chosen =
    f16 || files.reduce((a, b) => ((Number(a.size) || 0) <= (Number(b.size) || 0) ? a : b));
  const bytes = Number(chosen.size) || 0;
  return bytes > 0 ? bytes : null;
}

export function isEmbeddingOrReranker(repo, arch) {
  const s = `${repo} ${arch}`.toLowerCase();
  return /embed|embedding|rerank|reranker|sentence-transformers|bge-|cross-encoder/.test(s);
}

export function isMoe(repo, arch) {
  return /moe/i.test(arch) || /\b[aA]\d+(\.\d+)?[bB]\b/.test(repo) || /gpt-oss/i.test(arch);
}

// Architectures whose fit math we don't trust (state-space / Mamba: the KV proxy is wrong).
export function isUnmodelledArch(arch) {
  return /mamba|ssm|rwkv|jamba/i.test(arch);
}

export function prettyName(repo) {
  return repo
    .split("/")
    .pop()
    .replace(/-GGUF$/i, "")
    .replace(/^[a-z0-9]+_/i, "") // strip a leading "author_" prefix some repos carry
    .replace(/[-_]+/g, " ")
    .trim();
}

export function gib(bytes) {
  return round2(bytes / 1_073_741_824);
}
export function round2(x) {
  return Math.round(x * 100) / 100;
}

// Hash over the entries only (not the timestamp), so re-runs are churn-free.
export function contentHash(entries) {
  return "sha256:" + createHash("sha256").update(JSON.stringify(entries)).digest("hex");
}

// --- main --------------------------------------------------------------------------------------

async function discover() {
  const res = await hfFetch(
    `${HF}/api/models?library=gguf&full=true&expand[]=gguf&sort=downloads&limit=100`,
  );
  const list = res.ok ? await res.json() : [];
  console.log("Top GGUF repos by downloads (maintainer aid — add good ones to SEED):");
  for (const m of list) {
    const g = m.gguf || {};
    if (!g.total) continue;
    console.log(
      `  ${m.id} | ${g.architecture} | ${round2(g.total / 1e9)}B | ctx ${g.context_length}`,
    );
  }
}

async function main() {
  if (process.argv.includes("--discover")) {
    await discover();
    return;
  }

  const { ledger: ledgerBefore, seed: SEED } = readLedger();
  console.log(`generate-local-catalog: refreshing ${SEED.length} seed repos from Hugging Face …`);
  const entries = [];
  const evidenceByRepo = new Map();
  for (const seed of SEED) {
    process.stdout.write(`- ${seed.repo}\n`);
    let built;
    try {
      built = await buildEntry(seed);
    } catch (e) {
      if (e instanceof AbortRun) {
        console.error(
          `\ngenerate-local-catalog: ABORTING without writing — ${e.message}\n` +
            `A transient Hugging Face failure must not silently drop a curated model or bump ` +
            `catalog_version. Re-run when Hugging Face is healthy.`,
        );
        process.exit(1);
      }
      throw e;
    }
    evidenceByRepo.set(seed.repo, built.evidence);
    if (built.entry) entries.push(built.entry);
  }
  entries.sort((a, b) => a.parameters_b - b.parameters_b);

  // The ledger is written FIRST and unconditionally: whatever happens to the catalogue, the fresh
  // evidence is on disk for whoever has to make the call. Writing it is not the same as approving it.
  const { ledger, review, changed } = reconcileLedger(ledgerBefore, evidenceByRepo);
  writeFileSync(ledgerPath, JSON.stringify(ledger, null, 2) + "\n");
  for (const repo of changed) {
    console.warn(
      `  LICENCE CHANGED upstream: ${repo} — was ${ledger.models[repo].previous.licence ?? "(none)"}; ` +
        `the recorded answer has been withdrawn`,
    );
  }

  // Requirement (b): refuse to emit a catalogue containing a model whose terms nobody has read.
  // The catalogue is compiled into the binary, so an unreviewed row would ship — and the UI would
  // have nothing to show a user before telling their machine to fetch the weights.
  const unreviewed = review.filter((repo) => entries.some((e) => e.repo === repo));
  if (unreviewed.length > 0) {
    console.error(
      `\ngenerate-local-catalog: NOT writing local_models.json — ${unreviewed.length} model(s) have ` +
        `no reviewed licence:\n` +
        unreviewed.map((r) => `  - ${r}\n`).join("") +
        `\nsrc-tauri/model_licences.json has been refreshed with what Hugging Face says. Read each ` +
        `row's \`evidence\`, check the publisher's own repo where it is ambiguous (a GGUF conversion ` +
        `copies the tag and can be stale), then fill in \`licence\` and re-run.`,
    );
    process.exit(1);
  }
  if (review.length > 0) {
    console.warn(
      `  ${review.length} seeded repo(s) await a licence but are not in the catalogue — not blocking: ` +
        review.join(", "),
    );
  }
  for (const entry of entries) {
    entry.licence = licenceFor(ledger, entry.repo);
    // A licence id that names no row in `terms` resolves to null, which would ship a catalogue the
    // Rust side cannot even parse (`licence` is required, and every struct is deny_unknown_fields).
    // Caught here rather than by a panic at first use.
    if (!entry.licence) {
      console.error(
        `\ngenerate-local-catalog: NOT writing local_models.json — ${entry.repo} is recorded as ` +
          `\`${ledger.models[entry.repo]?.licence}\`, which is not a row in the ledger's \`terms\` map.`,
      );
      process.exit(1);
    }
  }

  const hash = contentHash(entries);
  const prev = existsSync(outPath) ? JSON.parse(readFileSync(outPath, "utf8")) : null;
  if (prev && prev.content_hash === hash) {
    console.log(
      `generate-local-catalog: no content change (${entries.length} entries) — leaving the file untouched.`,
    );
    return;
  }

  const doc = {
    schema_version: SCHEMA_VERSION,
    catalog_version: (prev?.catalog_version || 0) + 1,
    content_hash: hash,
    generated_at: todayUtc(),
    source: "huggingface-gguf",
    entries,
  };
  writeFileSync(outPath, JSON.stringify(doc, null, 2) + "\n");
  console.log(
    `generate-local-catalog: wrote ${entries.length} entries → local_models.json ` +
      `(catalog_version ${doc.catalog_version}).`,
  );
}

// UTC date as YYYY-MM-DD, no time component (freshness is day-granular).
function todayUtc() {
  return new Date().toISOString().slice(0, 10);
}

// Run only when invoked as a script. Without this guard, importing the module for its pure helpers
// (the unit tests do) would fire 19 Hugging Face requests and could rewrite the tracked
// src-tauri/local_models.json mid-`just check`. pathToFileURL, not import.meta.filename: the latter
// needs Node >= 20.11 and pr.yml pins a floating `node-version: 20`.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
