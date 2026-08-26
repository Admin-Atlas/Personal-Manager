// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The catalog generator's correctness rules (#296). These decide what `src-tauri/local_models.json`
// says about every model PM recommends, and the whole fit calculator reads those numbers as
// measured truth — so a regression here does not fail loudly, it quietly mis-sizes models against
// real hardware.
//
// The generator is network-bound and deliberately outside `just check`, so it never runs in CI and
// nothing exercised these rules at all until this file. Everything tested below is pure; importing
// the module does NOT run it (see the entry-point guard at the bottom of the generator), so no
// Hugging Face request is made and the committed catalog is never touched.

import { describe, expect, it } from "vitest";

import {
  activeFromHeader,
  contentHash,
  gib,
  isDraftHead,
  ollamaTagFor,
  isEmbeddingOrReranker,
  isMoe,
  isUnmodelledArch,
  matchesQuant,
  pickProjector,
  prettyName,
  round2,
  sumQuantShards,
} from "./generate-local-catalog.mjs";

describe("matchesQuant", () => {
  // The highest-value rule in the file. Its own comment names the shipped bug it fixes: a loose
  // match let "Q6_K" also claim "Q6_K_L"/"Q6_K_XL", summing several quants into one size.
  it("matches a quant exactly, never a longer label that starts the same way", () => {
    expect(matchesQuant("gemma-3-4b-it-Q6_K.gguf", "Q6_K")).toBe(true);
    expect(matchesQuant("gemma-3-4b-it-Q6_K_L.gguf", "Q6_K")).toBe(false);
    expect(matchesQuant("gemma-3-4b-it-Q6_K_XL.gguf", "Q6_K")).toBe(false);
    expect(matchesQuant("gemma-3-4b-it-Q4_K_M.gguf", "Q4_K_M")).toBe(true);
  });

  it("accepts every separator publishers actually use, case-insensitively", () => {
    expect(matchesQuant("model.Q8_0.gguf", "Q8_0")).toBe(true);
    expect(matchesQuant("model_Q8_0.gguf", "Q8_0")).toBe(true);
    expect(matchesQuant("model-q8_0.gguf", "Q8_0")).toBe(true);
  });

  it("matches a shard member by its -NNNNN-of-NNNNN suffix", () => {
    expect(matchesQuant("Qwen2.5-72B-Q6_K-00001-of-00002.gguf", "Q6_K")).toBe(true);
    expect(matchesQuant("Qwen2.5-72B-Q6_K-00002-of-00002.gguf", "Q6_K")).toBe(true);
  });

  it("matches a per-quant subfolder, which is how some repos lay shards out", () => {
    expect(matchesQuant("Q6_K/Qwen2.5-72B-00001-of-00002.gguf", "Q6_K")).toBe(true);
    // ...but the folder must be the quant, not merely contain it.
    expect(matchesQuant("Q6_K_L/model-00001-of-00002.gguf", "Q6_K")).toBe(false);
  });

  it("does not match a non-gguf file that happens to carry the label", () => {
    expect(matchesQuant("model-Q6_K.gguf.incomplete", "Q6_K")).toBe(false);
    expect(matchesQuant("README-Q6_K.md", "Q6_K")).toBe(false);
  });
});

describe("sumQuantShards", () => {
  const f = (path, size) => ({ path, size });

  it("sums every shard of one quant and flags the set as sharded", () => {
    const files = [f("m-Q6_K-00001-of-00002.gguf", 1000), f("m-Q6_K-00002-of-00002.gguf", 2000)];
    expect(sumQuantShards(files, "Q6_K")).toEqual({ bytes: 3000, sharded: true });
  });

  it("reports a single file as not sharded", () => {
    expect(sumQuantShards([f("m-Q4_K_M.gguf", 500)], "Q4_K_M")).toEqual({
      bytes: 500,
      sharded: false,
    });
  });

  it("EXCLUDES the projector, so a quant size is weights only", () => {
    // This is what makes the catalog's `file_gb` comparable with an on-disk weights measurement,
    // and what stops the projector being counted in both the weight and projector terms.
    const files = [f("m-Q4_K_M.gguf", 500), f("mmproj-model-f16.gguf", 400)];
    expect(sumQuantShards(files, "Q4_K_M")).toEqual({ bytes: 500, sharded: false });
  });

  it("EXCLUDES a multi-token-prediction draft head, wherever it sits in the tree", () => {
    // Real paths from unsloth/gemma-4-12b-it-GGUF, which is what caught this: the draft head carries
    // the SAME quant token as the model, so it summed in and flipped `sharded` to true. Sizes are
    // the live ones — 11.80 GiB of weights beside a 0.433 GiB head that shipped as 12.23 GiB.
    const files = [
      f("gemma-4-12b-it-Q8_0.gguf", 12_670_000_000),
      f("MTP/mtp-gemma-4-12b-it-Q8_0.gguf", 465_000_000),
      f("mtp-gemma-4-12b-it.gguf", 465_000_000),
    ];
    expect(sumQuantShards(files, "Q8_0")).toEqual({ bytes: 12_670_000_000, sharded: false });
  });

  it("returns null when nothing matches or the sizes are all zero", () => {
    expect(sumQuantShards([f("m-Q8_0.gguf", 500)], "Q4_K_M")).toBeNull();
    expect(sumQuantShards([f("m-Q4_K_M.gguf", 0)], "Q4_K_M")).toBeNull();
  });
});

describe("isDraftHead", () => {
  it("spots a draft head by filename prefix or by its MTP folder", () => {
    expect(isDraftHead("MTP/mtp-gemma-4-12b-it-Q8_0.gguf")).toBe(true);
    expect(isDraftHead("mtp-gemma-4-26B-A4B-it.gguf")).toBe(true);
    expect(isDraftHead("MTP/anything.gguf")).toBe(true);
  });

  it("leaves the model's own weights alone", () => {
    // The prefix is anchored and separator-terminated on purpose: a real model whose name merely
    // begins with those three letters is not a draft head.
    expect(isDraftHead("gemma-4-12b-it-Q8_0.gguf")).toBe(false);
    expect(isDraftHead("mtpmodel-Q8_0.gguf")).toBe(false);
    expect(isDraftHead("Q8_0/gemma-4-12b-it-00001-of-00002.gguf")).toBe(false);
  });
});

describe("pickProjector", () => {
  const f = (path, size) => ({ path, size });

  it("takes ONE projector and never sums precisions", () => {
    // A model loads exactly one projector. Summing F16 + F32 was the on-disk crawl's bug.
    expect(pickProjector([f("mmproj-F16.gguf", 1000), f("mmproj-F32.gguf", 2000)])).toBe(1000);
  });

  it("prefers an f16 projector even when it is not the smallest", () => {
    expect(pickProjector([f("mmproj-Q8_0.gguf", 500), f("mmproj-f16.gguf", 900)])).toBe(900);
  });

  it("falls back to the smallest when no name matches the strict f16 test", () => {
    // `mmproj-model-f16.gguf` has `-model-` in between, so it misses the f16 regex — the fallback
    // still lands on the f16 whenever an f32 is its rival. The Rust side mirrors this exactly.
    expect(pickProjector([f("mmproj-model-f16.gguf", 800), f("mmproj-model-f32.gguf", 1600)])).toBe(
      800,
    );
  });

  it("returns null for no candidates or a zero-sized one", () => {
    expect(pickProjector([])).toBeNull();
    expect(pickProjector([f("mmproj-F16.gguf", 0)])).toBeNull();
  });
});

describe("isEmbeddingOrReranker", () => {
  it("drops embedders and rerankers before they can reach the catalog", () => {
    expect(isEmbeddingOrReranker("nomic-ai/nomic-embed-text-v1.5-GGUF", "nomic-bert")).toBe(true);
    expect(isEmbeddingOrReranker("BAAI/bge-reranker-v2-m3", "xlm-roberta")).toBe(true);
    expect(isEmbeddingOrReranker("sentence-transformers/all-MiniLM-L6-v2", "bert")).toBe(true);
    expect(isEmbeddingOrReranker("BAAI/bge-small-en-v1.5", "bert")).toBe(true);
  });

  it("lets ordinary chat models through", () => {
    expect(isEmbeddingOrReranker("bartowski/Qwen2.5-7B-Instruct-GGUF", "qwen2")).toBe(false);
    expect(isEmbeddingOrReranker("unsloth/gemma-4-26B-A4B-it-GGUF", "gemma4")).toBe(false);
  });
});

describe("architecture classification", () => {
  it("flags MoE by architecture name, by an aNNb repo tag, or by gpt-oss", () => {
    expect(isMoe("some/repo-GGUF", "qwen35moe")).toBe(true);
    expect(isMoe("unsloth/Qwen3.6-35B-A3B-GGUF", "qwen35")).toBe(true);
    expect(isMoe("unsloth/gemma-4-26B-A4B-it-GGUF", "gemma4")).toBe(true);
    expect(isMoe("ggml-org/gpt-oss-20b-GGUF", "gpt-oss")).toBe(true);
    expect(isMoe("bartowski/Qwen2.5-7B-Instruct-GGUF", "qwen2")).toBe(false);
  });

  it("marks state-space architectures unmodelled, because the KV term is wrong for them", () => {
    for (const arch of ["mamba", "mamba2", "ssm", "rwkv6", "jamba"]) {
      expect(isUnmodelledArch(arch)).toBe(true);
    }
    for (const arch of ["llama", "qwen2", "gemma3", "phi3"]) {
      expect(isUnmodelledArch(arch)).toBe(false);
    }
  });
});

describe("activeFromHeader", () => {
  // Decision E: active params come from the GGUF header, never from `total x used/count`, and an
  // unreadable header EXCLUDES the model rather than guessing.
  const header = (over = {}) => ({
    "general.architecture": "qwen35moe",
    "qwen35moe.expert_count": 128,
    "qwen35moe.expert_used_count": 8,
    "qwen35moe.block_count": 48,
    "qwen35moe.embedding_length": 2048,
    "qwen35moe.expert_feed_forward_length": 768,
    ...over,
  });

  it("subtracts the inactive experts from the total", () => {
    const total = 35_000_000_000;
    const inactive = (128 - 8) * 48 * 3 * 2048 * 768;
    expect(activeFromHeader(header(), total)).toBe(total - inactive);
  });

  it("reads the counts under the architecture prefix or bare", () => {
    const bare = {
      "general.architecture": "qwen35moe",
      expert_count: 128,
      expert_used_count: 8,
      block_count: 48,
      embedding_length: 2048,
      expert_feed_forward_length: 768,
    };
    expect(activeFromHeader(bare, 35_000_000_000)).toBe(activeFromHeader(header(), 35_000_000_000));
  });

  it("returns null on an incomplete or nonsensical header rather than guessing", () => {
    expect(activeFromHeader(header({ "qwen35moe.block_count": undefined }), 35e9)).toBeNull();
    expect(activeFromHeader(header({ "qwen35moe.expert_count": 0 }), 35e9)).toBeNull();
    expect(activeFromHeader(header({ "qwen35moe.expert_used_count": 0 }), 35e9)).toBeNull();
    expect(activeFromHeader(header(), 0)).toBeNull();
    expect(activeFromHeader(header(), Number.NaN)).toBeNull();
  });

  it("returns null when the inactive experts would exceed the whole model", () => {
    // A wrong header must not produce a negative or zero active count that then sails into fit.rs.
    expect(activeFromHeader(header(), 1_000_000)).toBeNull();
  });
});

describe("ollamaTagFor", () => {
  // Ollama routes by the HOST in a model name and Hugging Face serves Ollama manifests at
  // /v2/{repo}/manifests/{QUANT}, so `hf.co/<repo>:<QUANT>` pulls the file this row measured — no
  // curated name list to drift. The size check is EXACT because both sides are the same artefact:
  // the manifest's image.model layer size IS the repo tree's file size. All numbers below were
  // measured against the live registry on 27-08-2026.
  const manifest = (...layers) => ({
    layers: layers.map(([mediaType, size]) => ({ mediaType, size })),
  });
  const MODEL = "application/vnd.ollama.image.model";

  it("offers the tag when the manifest layer is the byte count this row measured", () => {
    expect(
      ollamaTagFor({
        repo: "bartowski/Qwen2.5-7B-Instruct-GGUF",
        quant: "Q4_K_M",
        sharded: false,
        bytes: 4_683_074_240,
        manifest: manifest([MODEL, 4_683_074_240], ["application/vnd.ollama.image.template", 1478]),
      }),
    ).toBe("hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q4_K_M");
  });

  it("compares bytes, not the rounded file_gb the catalogue displays", () => {
    // SmolVLM's rounded 0.41 GiB is 0.78% off its real size. Rounding is a display concern; if this
    // compared gib() values it would need a tolerance band, and a tolerance is where drift hides.
    expect(
      ollamaTagFor({
        repo: "ggml-org/SmolVLM-500M-Instruct-GGUF",
        quant: "Q8_0",
        sharded: false,
        bytes: 436_806_912,
        manifest: manifest(
          [MODEL, 436_806_912],
          ["application/vnd.ollama.image.projector", 108_783_360],
        ),
      }),
    ).toBe("hf.co/ggml-org/SmolVLM-500M-Instruct-GGUF:Q8_0");
  });

  it("refuses a single byte in either direction", () => {
    // Symmetric on purpose: the requirement is that the file IS the file, not that it fits the
    // budget. A smaller file is just as wrong as a larger one — it is a different artefact.
    for (const size of [4_683_074_239, 4_683_074_241]) {
      expect(
        ollamaTagFor({
          repo: "r/x",
          quant: "Q4_K_M",
          sharded: false,
          bytes: 4_683_074_240,
          manifest: manifest([MODEL, size]),
        }),
      ).toBeNull();
    }
  });

  it("refuses Ollama's own conversion when it is a different file", () => {
    // gemma3:4b-it-q4_k_m folds the vision tower into the model layer: 3_338_801_664 against this
    // repo's 2_490_720_384, +34%. Offering it would download something the card never sized — and
    // it is why the library route is not used at all.
    expect(
      ollamaTagFor({
        repo: "ggml-org/gemma-3-4b-it-GGUF",
        quant: "Q4_K_M",
        sharded: false,
        bytes: 2_490_720_384,
        manifest: manifest([MODEL, 3_338_801_664]),
      }),
    ).toBeNull();
  });

  it("refuses a sharded row without consulting any manifest", () => {
    // Hugging Face's shim 400s on split GGUF by design, so the generator must not spend the request
    // and the UI must not render a button that cannot work.
    expect(
      ollamaTagFor({
        repo: "bartowski/Qwen2.5-72B-Instruct-GGUF",
        quant: "Q8_0",
        sharded: true,
        bytes: 77_264_000_000,
        manifest: manifest([MODEL, 77_264_000_000]),
      }),
    ).toBeNull();
  });

  it("refuses when the manifest is missing or carries no model layer", () => {
    const args = { repo: "r/x", quant: "Q4_K_M", sharded: false, bytes: 100 };
    expect(ollamaTagFor({ ...args, manifest: null })).toBeNull();
    expect(
      ollamaTagFor({
        ...args,
        manifest: manifest(["application/vnd.ollama.image.projector", 100]),
      }),
    ).toBeNull();
  });
});

describe("stamping helpers", () => {
  it("hashes the entries only, so a re-run with no content change is a no-op", () => {
    const a = [{ repo: "x", parameters_b: 1 }];
    const b = [{ repo: "x", parameters_b: 1 }];
    expect(contentHash(a)).toBe(contentHash(b));
    expect(contentHash(a)).not.toBe(contentHash([{ repo: "y", parameters_b: 1 }]));
    expect(contentHash(a)).toMatch(/^sha256:[0-9a-f]{64}$/);
  });

  it("converts bytes in the same GiB base the fit calculator uses", () => {
    expect(gib(1_073_741_824)).toBe(1);
    expect(round2(2.345)).toBe(2.35);
  });

  it("turns a repo id into a readable display name", () => {
    expect(prettyName("bartowski/Qwen2.5-7B-Instruct-GGUF")).toBe("Qwen2.5 7B Instruct");
    expect(prettyName("ggml-org/gemma-3-4b-it-GGUF")).toBe("gemma 3 4b it");
  });
});
