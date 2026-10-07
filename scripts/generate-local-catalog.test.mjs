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
  decodeBytes,
  GGML_BLOCK,
  gib,
  isDraftHead,
  ollamaTagFor,
  isEmbeddingOrReranker,
  isMoe,
  isUnmodelledArch,
  kvFromHeader,
  matchesQuant,
  pickProjector,
  prettyName,
  quantShardPaths,
  round2,
  SLOW_TENSOR_TYPES,
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

describe("quantShardPaths", () => {
  const f = (path, size = 1) => ({ path, size });

  it("lists one quant's shards in order, and nothing else that carries its label", () => {
    // The headers PM reads for a quant's decode bytes must be the very files its size sums: the
    // shards in order (the first carries the metadata), never a projector, a draft head or a
    // longer label that starts the same way.
    const files = [
      f("Q6_K/m-00002-of-00002.gguf"),
      f("mmproj-m-Q6_K.gguf"),
      f("MTP/mtp-m-Q6_K.gguf"),
      f("m-Q6_K_L.gguf"),
      f("Q6_K/m-00001-of-00002.gguf"),
      f("m-Q8_0.gguf"),
    ];
    expect(quantShardPaths(files, "Q6_K")).toEqual([
      "Q6_K/m-00001-of-00002.gguf",
      "Q6_K/m-00002-of-00002.gguf",
    ]);
    expect(sumQuantShards(files, "Q6_K")).toEqual({ bytes: 2, sharded: true });
    expect(quantShardPaths(files, "Q4_K_M")).toEqual([]);
  });
});

describe("decodeBytes", () => {
  // What a decode step reads, from the tensor table alone. Shapes are bigints, as @huggingface/gguf
  // returns them; the dtype is the numeric GGML type id the table stores.
  const F32 = 0;
  const Q8_0 = 8;
  const Q3_K = 11;
  const Q4_K = 12;
  const Q6_K = 14;
  const IQ4_XS = 23;
  const t = (name, dtype, ...shape) => ({ name, dtype, shape: shape.map(BigInt) });
  const llama = { "general.architecture": "llama" };
  // 256 × 1000 at Q8_0: 8000 blocks of 34 bytes.
  const embd = t("token_embd.weight", Q8_0, 256, 1000);
  const body = [t("blk.0.attn_q.weight", Q4_K, 256, 256), t("blk.0.attn_norm.weight", F32, 256)];
  const bodyBytes = 256 * 144 + 256 * 4;

  it("counts a tied embedding, because it is the output head read whole every token", () => {
    expect(decodeBytes([embd, ...body], llama)).toEqual({
      decode_bytes: 8000 * 34 + bodyBytes,
      decode_slow_bytes: 0,
    });
  });

  it("leaves out an untied embedding, which a token reads one row of", () => {
    const output = t("output.weight", Q6_K, 256, 1000);
    expect(decodeBytes([embd, ...body, output], llama).decode_bytes).toBe(bodyBytes + 1000 * 210);
  });

  it("leaves out a per-layer embedding table, which is looked up a row at a time", () => {
    const gemma4 = { "general.architecture": "gemma4" };
    const perLayer = t("per_layer_token_embd.weight", Q8_0, 8192, 1000);
    expect(decodeBytes([embd, ...body, perLayer], gemma4).decode_bytes).toBe(8000 * 34 + bodyBytes);
  });

  it("charges routed experts at the share a token reaches, and a shared expert in full", () => {
    const moe = {
      "general.architecture": "qwen35moe",
      "qwen35moe.expert_count": 128,
      "qwen35moe.expert_used_count": 8,
    };
    // 128 experts of 256 × 64 at Q4_K: 1179648 bytes, of which a token reads 8/128.
    const experts = t("blk.0.ffn_gate_exps.weight", Q4_K, 256, 64, 128);
    const shared = t("blk.0.ffn_up_shexp.weight", Q4_K, 256, 64);
    expect(decodeBytes([...body, experts, shared], moe).decode_bytes).toBe(
      bodyBytes + (1_179_648 * 8) / 128 + 64 * 144,
    );
    // Routed experts with no counts to scale them by are not guessed at.
    expect(() => decodeBytes([...body, experts], llama)).toThrow(/expert_\* counts/);
  });

  it("splits out the bytes in a slow-to-unpack type, scaled the same way", () => {
    const moe = {
      "general.architecture": "qwen35moe",
      "qwen35moe.expert_count": 4,
      "qwen35moe.expert_used_count": 1,
    };
    const tensors = [
      t("blk.0.attn_q.weight", Q3_K, 256, 256), // 256 blocks of 110: slow
      t("blk.0.attn_k.weight", IQ4_XS, 256, 256), // 256 of 136: slow
      t("blk.0.attn_v.weight", Q6_K, 256, 256), // 256 of 210
      t("blk.0.ffn_down_exps.weight", Q3_K, 256, 4, 4), // 16 of 110, a quarter read: slow
    ];
    const slow = 256 * 110 + 256 * 136 + (16 * 110) / 4;
    expect(decodeBytes(tensors, moe)).toEqual({
      decode_bytes: slow + 256 * 210,
      decode_slow_bytes: slow,
    });
  });

  it("rounds each sum once, at the end", () => {
    // Three routed tensors of 40 bytes, a third of each read: 40 in all. Rounding each tensor's
    // 13.33 first would write 39.
    const moe = {
      "general.architecture": "qwen35moe",
      "qwen35moe.expert_count": 3,
      "qwen35moe.expert_used_count": 1,
    };
    const tensors = ["gate", "up", "down"].map((m) => t(`blk.0.ffn_${m}_exps.weight`, F32, 10));
    expect(decodeBytes(tensors, moe)).toEqual({ decode_bytes: 40, decode_slow_bytes: 0 });
  });

  it("throws on a tensor type it has no block size for, rather than guessing one", () => {
    // TQ1_0 (34): a real ggml type no catalogue file carries yet.
    expect(() => decodeBytes([...body, t("blk.0.attn_out.weight", 34, 256, 256)], llama)).toThrow(
      /unknown GGML tensor type 34/,
    );
  });

  it("knows the block of every type id it claims, and only Q2_K, Q3_K and the i-quants are slow", () => {
    // The numeric ids a tensor table stores, pinned against the library enum the table is keyed by.
    const ids = Object.keys(GGML_BLOCK).map(Number);
    expect(ids.sort((a, b) => a - b)).toEqual([
      0, 1, 2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
      27, 28, 29, 30, 39,
    ]);
    expect([...SLOW_TENSOR_TYPES].sort((a, b) => a - b)).toEqual([
      10, 11, 16, 17, 18, 19, 20, 21, 22, 23, 29,
    ]);
    expect(GGML_BLOCK[Q3_K]).toEqual([256, 110]);
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

describe("kvFromHeader", () => {
  // The KV term fit.rs sizes a model with. Its old params × context proxy could not see attention
  // geometry and under-counted Phi 3.5 mini 13x, which made PM pick a 128k-context config for a 6 GB
  // card that needs about 27 GB of cache. Every header below is the real one, read off Hugging Face
  // on 02-10-2026, cut down to the keys this function reads.
  const llama31 = {
    "general.architecture": "llama",
    "llama.block_count": 32,
    "llama.embedding_length": 4096,
    "llama.attention.head_count": 32,
    "llama.attention.head_count_kv": 8,
  };

  it("sizes a model without grouped-query attention from all of its heads", () => {
    // Phi 3.5 mini: 32 layers × 32 KV heads × head_dim 96 (3072 / 32) × K and V × 2 bytes.
    const phi = {
      "general.architecture": "phi3",
      "phi3.block_count": 32,
      "phi3.embedding_length": 3072,
      "phi3.attention.head_count": 32,
      "phi3.attention.head_count_kv": 32,
      // llama.cpp switches SWA off for phi3, so this window must not shrink anything.
      "phi3.attention.sliding_window": 262144,
    };
    expect(kvFromHeader(phi)).toEqual({
      bytes_per_token: 393_216,
      window_bytes_per_token: 0,
      window: null,
      state_bytes: 0,
    });
  });

  it("counts only the KV heads of a grouped-query model, at llama.cpp's head-size default", () => {
    // 32 layers × 8 KV heads × 128 × 2 × 2. No key_length in the header: 4096 / 32.
    expect(kvFromHeader(llama31).bytes_per_token).toBe(131_072);
    // Explicit key/value lengths win over the default.
    const explicit = {
      ...llama31,
      "llama.attention.key_length": 64,
      "llama.attention.value_length": 64,
    };
    expect(kvFromHeader(explicit).bytes_per_token).toBe(65_536);
    // No head_count_kv at all is llama.cpp's "every head", not a guess.
    const noGqa = { ...llama31, "llama.attention.head_count_kv": undefined };
    expect(kvFromHeader(noGqa).bytes_per_token).toBe(524_288);
  });

  it("puts gemma 3's sliding layers apart: five in every six, capped at the header's window", () => {
    const gemma3 = {
      "general.architecture": "gemma3",
      "gemma3.block_count": 34,
      "gemma3.embedding_length": 2560,
      "gemma3.attention.head_count": 8,
      "gemma3.attention.head_count_kv": 4,
      "gemma3.attention.key_length": 256,
      "gemma3.attention.value_length": 256,
      "gemma3.attention.sliding_window": 1024,
    };
    // Layers 5, 11, 17, 23 and 29 are global: 5 × 4096 bytes. The other 29 slide.
    expect(kvFromHeader(gemma3)).toEqual({
      bytes_per_token: 5 * 4096,
      window_bytes_per_token: 29 * 4096,
      window: 1024,
      state_bytes: 0,
    });
    // Without its window there is nothing to cap at, so every layer counts in full.
    const noWindow = { ...gemma3, "gemma3.attention.sliding_window": undefined };
    expect(kvFromHeader(noWindow).bytes_per_token).toBe(34 * 4096);
  });

  it("alternates gemma 2's layers", () => {
    const gemma2 = {
      "general.architecture": "gemma2",
      "gemma2.block_count": 26,
      "gemma2.embedding_length": 2304,
      "gemma2.attention.head_count": 8,
      "gemma2.attention.head_count_kv": 4,
      "gemma2.attention.key_length": 256,
      "gemma2.attention.value_length": 256,
      "gemma2.attention.sliding_window": 4096,
    };
    const kv = kvFromHeader(gemma2);
    expect(kv.bytes_per_token).toBe(13 * 4096);
    expect(kv.window_bytes_per_token).toBe(13 * 4096);
    expect(kv.window).toBe(4096);
  });

  it("reads gemma 4's own pattern, per-layer KV heads and sliding head sizes", () => {
    // gemma-4-12b-it: global layers have ONE KV head of 512; sliding ones eight of 256.
    const pattern = Array.from({ length: 48 }, (_, i) => i % 6 !== 5);
    const gemma4 = {
      "general.architecture": "gemma4",
      "gemma4.block_count": 48,
      "gemma4.embedding_length": 3840,
      "gemma4.attention.head_count": 16,
      "gemma4.attention.head_count_kv": pattern.map((s) => (s ? 8 : 1)),
      "gemma4.attention.key_length": 512,
      "gemma4.attention.value_length": 512,
      "gemma4.attention.key_length_swa": 256,
      "gemma4.attention.value_length_swa": 256,
      "gemma4.attention.sliding_window": 1024,
      "gemma4.attention.sliding_window_pattern": pattern,
      "gemma4.attention.shared_kv_layers": 0,
    };
    expect(kvFromHeader(gemma4)).toEqual({
      bytes_per_token: 8 * (1024 * 1 * 2),
      window_bytes_per_token: 40 * (512 * 8 * 2),
      window: 1024,
      state_bytes: 0,
    });
    // Layers that reuse an earlier layer's cache add none of their own: the last six here.
    const sharing = { ...gemma4, "gemma4.attention.shared_kv_layers": 6 };
    expect(kvFromHeader(sharing)).toEqual({
      bytes_per_token: 7 * 2048,
      window_bytes_per_token: 35 * 8192,
      window: 1024,
      state_bytes: 0,
    });
  });

  it("gives a hybrid model a cache on its attention layers only, and a fixed state on the rest", () => {
    // Qwen3.5 4B: every fourth of 32 layers attends; the other 24 are linear attention.
    const qwen35 = {
      "general.architecture": "qwen35",
      "qwen35.block_count": 32,
      "qwen35.embedding_length": 2560,
      "qwen35.attention.head_count": 16,
      "qwen35.attention.head_count_kv": 4,
      "qwen35.attention.key_length": 256,
      "qwen35.attention.value_length": 256,
      "qwen35.full_attention_interval": 4,
      "qwen35.ssm.conv_kernel": 4,
      "qwen35.ssm.inner_size": 4096,
      "qwen35.ssm.state_size": 128,
      "qwen35.ssm.group_count": 16,
    };
    // n_embd_r = 3 × (4096 + 2 × 16 × 128) = 24576; n_embd_s = 128 × 4096 = 524288; f32.
    expect(kvFromHeader(qwen35)).toEqual({
      bytes_per_token: 8 * 4096,
      window_bytes_per_token: 0,
      window: null,
      state_bytes: 24 * (24_576 + 524_288) * 4,
    });
    // A hybrid whose state cannot be sized is not sized at all.
    expect(kvFromHeader({ ...qwen35, "qwen35.ssm.inner_size": undefined })).toBeNull();
  });

  it("returns null rather than guessing when the header lacks the geometry", () => {
    expect(kvFromHeader({ ...llama31, "llama.block_count": undefined })).toBeNull();
    expect(kvFromHeader({ ...llama31, "llama.attention.head_count": undefined })).toBeNull();
    expect(kvFromHeader({ ...llama31, "llama.attention.head_count_kv": "eight" })).toBeNull();
    expect(kvFromHeader({ ...llama31, "llama.attention.head_count_kv": 0 })).toBeNull();
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
