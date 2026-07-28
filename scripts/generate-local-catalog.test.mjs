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

  it("returns null when nothing matches or the sizes are all zero", () => {
    expect(sumQuantShards([f("m-Q8_0.gguf", 500)], "Q4_K_M")).toBeNull();
    expect(sumQuantShards([f("m-Q4_K_M.gguf", 0)], "Q4_K_M")).toBeNull();
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
