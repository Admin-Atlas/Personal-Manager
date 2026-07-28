// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Dev-time generator for the curated local-model catalog (#296) shipped at
// `src-tauri/local_models.json`. It refreshes a small, hand-picked SEED of GGUF repos from the
// Hugging Face API — real per-quant file sizes, architecture, context window, and (for MoE models)
// the active-parameter count read out of the GGUF header — so the app can size each model against a
// user's hardware with `fit.rs`.
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

import { gguf } from "@huggingface/gguf";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";

const HF = "https://huggingface.co";
const UA = "pm-local-catalog-generator (Personal-Manager)";
const DEFAULT_QUANTS = ["Q3_K_M", "Q4_K_M", "Q5_K_M", "Q6_K", "Q8_0"];
const SCHEMA_VERSION = 1;
// Bounded retries for transient Hugging Face failures (network drop / HTTP 5xx) before we give up.
const MAX_ATTEMPTS = 4;

// Thrown when a curated seed can't be fetched/verified (transient outage or a permanent 4xx). It
// ABORTS the whole run rather than emit a smaller catalog — a dropped seed would delete a model AND
// bump `catalog_version`, nudging every user to rescan over a Hugging Face blip. Distinct from a
// model that fetched fine but doesn't qualify (embedding, no curated quant), which is a clean drop.
class AbortRun extends Error {}

// The curated SEED: verified-real GGUF repos spanning small→large, dense + MoE + multimodal, from
// reputable quantizers (bartowski / unsloth / ggml-org). `sort=downloads` discovery is a maintainer
// aid for finding new entries (see --discover) — never an auto-append, so diffs stay reviewable.
// `role`: which slot this suits ("background" = fast/cheap summaries, "chat" = quality replies).
const SEED = [
  { repo: "bartowski/Llama-3.2-1B-Instruct-GGUF", role: "background" },
  { repo: "ggml-org/gemma-3-1b-it-GGUF", role: "background" },
  { repo: "bartowski/gemma-2-2b-it-GGUF", role: "background" },
  { repo: "bartowski/Llama-3.2-3B-Instruct-GGUF", role: "background" },
  { repo: "bartowski/Phi-3.5-mini-instruct-GGUF", role: "background" },
  { repo: "unsloth/Qwen3.5-4B-GGUF", role: "background" },
  { repo: "ggml-org/gemma-3-4b-it-GGUF", role: "chat" },
  { repo: "bartowski/Qwen2.5-7B-Instruct-GGUF", role: "chat" },
  { repo: "bartowski/Meta-Llama-3.1-8B-Instruct-GGUF", role: "chat" },
  { repo: "unsloth/Qwen3.5-9B-GGUF", role: "chat" },
  { repo: "unsloth/gemma-4-12b-it-GGUF", role: "chat" },
  { repo: "bartowski/Qwen2.5-14B-Instruct-GGUF", role: "chat" },
  { repo: "ggml-org/gpt-oss-20b-GGUF", role: "chat" },
  { repo: "unsloth/Qwen3.6-27B-GGUF", role: "chat" },
  { repo: "unsloth/Qwen3.6-35B-A3B-GGUF", role: "chat" },
  { repo: "unsloth/gemma-4-26B-A4B-it-GGUF", role: "chat" },
  { repo: "bartowski/Qwen2.5-72B-Instruct-GGUF", role: "chat" },
  { repo: "bartowski/DeepSeek-V4-Flash-GGUF", role: "chat" },
  { repo: "ggml-org/SmolVLM-500M-Instruct-GGUF", role: "background" },
];

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const outPath = join(repoRoot, "src-tauri", "local_models.json");

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
  const infoRes = await hfFetch(`${HF}/api/models/${repo}?expand[]=gguf`);
  if (!infoRes.ok) {
    // 5xx already retried+threw in hfFetch; a 4xx here means a curated seed is gone/renamed — a
    // maintenance signal, not a silent drop. Abort so the SEED gets fixed rather than shipped short.
    throw new AbortRun(
      `curated seed ${repo}: model info HTTP ${infoRes.status} — fix SEED or retry`,
    );
  }
  const info = await infoRes.json();
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
    console.warn(`  skip ${repo}: incomplete GGUF metadata (params/arch/ctx)`);
    return null;
  }
  if (isEmbeddingOrReranker(repo, architecture)) {
    console.warn(`  skip ${repo}: embedding/reranker (not a chat model)`);
    return null;
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
  for (const label of seed.quants || DEFAULT_QUANTS) {
    const size = sumQuantShards(ggufFiles, label);
    if (size) quants.push({ quant: label, file_gb: gib(size.bytes), sharded: size.sharded });
  }
  if (quants.length === 0) {
    console.warn(`  skip ${repo}: none of the curated quants present in the tree`);
    return null;
  }

  // 3. Multimodal projector: an mmproj file makes the model multimodal. A model loads ONE projector
  //    at runtime, so pick a single precision (never sum the F16/F32/BF16 variants).
  const projectorBytes = pickProjector(ggufFiles.filter((f) => /mmproj/i.test(f.path)));
  const multimodal = projectorBytes != null;
  const projectorGb = multimodal ? gib(projectorBytes) : null;

  // 4. Active params: dense == total; MoE is read from the GGUF header (decision E — never
  //    total×used/count). A MoE we can't parse is EXCLUDED from the curated catalog.
  const looksMoe = isMoe(repo, architecture);
  let activeParams = totalParams;
  let fit = "computed";
  if (looksMoe) {
    const firstShard = ggufFiles
      .filter((f) => matchesQuant(f.path, quants[0].quant) && !/mmproj/i.test(f.path))
      .map((f) => f.path)
      .sort()[0];
    const active = await moeActiveParams(repo, firstShard, totalParams);
    if (active && active > 0 && active <= totalParams) {
      activeParams = active;
    } else {
      console.warn(`  EXCLUDE ${repo}: MoE active-params unreadable from GGUF (decision E)`);
      return null;
    }
  }
  // Unmodelled architectures we can't fit-score: keep the row but mark it honestly.
  if (isUnmodelledArch(architecture)) fit = "unknown";

  return {
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
    fit,
    quants,
    install: { ollama: null },
  };
}

// --- GGUF header parse for MoE active params ---------------------------------------------------

// Read a MoE model's active-parameter count out of the GGUF header (decision E — never derived from
// HF JSON). `general.parameter_count` isn't always present, so the total comes from the caller (the
// HF `gguf.total`); the header supplies the expert geometry to subtract the inactive experts.
async function moeActiveParams(repo, shardPath, totalParams) {
  if (!shardPath) return null;
  const url = `${HF}/${repo}/resolve/main/${shardPath}`;
  let metadata;
  for (let attempt = 1; ; attempt++) {
    try {
      ({ metadata } = await gguf(url));
      break;
    } catch (e) {
      // Retry a transient header-range read; only after it persists is this a real decision-E
      // exclusion (an unparseable MoE header → drop). A network blip must not masquerade as one.
      if (attempt < MAX_ATTEMPTS) {
        await sleep(backoffMs(attempt));
        continue;
      }
      console.warn(
        `    gguf parse failed for ${repo} after ${MAX_ATTEMPTS} attempts: ${e?.message || e}`,
      );
      return null;
    }
  }
  return activeFromHeader(metadata, totalParams);
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

// --- small pure helpers ------------------------------------------------------------------------

export function sumQuantShards(files, label) {
  const parts = files.filter((f) => matchesQuant(f.path, label) && !/mmproj/i.test(f.path));
  if (parts.length === 0) return null;
  const bytes = parts.reduce((n, f) => n + (Number(f.size) || 0), 0);
  return bytes > 0 ? { bytes, sharded: parts.length > 1 } : null;
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

  console.log(`generate-local-catalog: refreshing ${SEED.length} seed repos from Hugging Face …`);
  const entries = [];
  for (const seed of SEED) {
    process.stdout.write(`- ${seed.repo}\n`);
    let entry;
    try {
      entry = await buildEntry(seed);
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
    if (entry) entries.push(entry);
  }
  entries.sort((a, b) => a.parameters_b - b.parameters_b);

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
