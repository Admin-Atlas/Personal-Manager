// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pure model-fit scoring (#296): given a machine's memory and a model's size, decide the highest
//! quality (quant, context) that fits and roughly how fast it will run.
//!
//! The math follows the standard GGUF local-inference budget used by the public VRAM/RAM
//! calculators (weights + KV cache + a runtime overhead, scored against available memory minus a
//! reserve): this is PM's own implementation of that well-known approach, not a port of any one
//! tool. Two deliberate choices keep it honest:
//!   * The **weight** term uses the catalog's *measured* per-quant `file_gb` (real bytes on disk),
//!     which is more accurate than reconstructing size from `params × bytes_per_param` — especially
//!     for K-quants, IQ-quants, and sharded/MoE files. `bytes_per_param` survives only for the
//!     throughput term (active weight bytes read per token) and to order quants by quality.
//!   * The **KV** term is sized at **f16** (2 bytes/element) by default — the conservative choice —
//!     but the ladder will compress it to **q8_0** (~half the size, near-lossless) *before* halving
//!     the context, so a KV-dominated model keeps its window and quant instead of degrading harder.
//!     Each result records the precision it was sized at in its `kv` field, surfaced per-config in the
//!     UI. f16 is always tried first, so this only ever *rescues* a config — never changes one that
//!     already fit.
//!
//! No I/O, no DB, no tauri — every function here is a pure projection of its inputs, unit-tested
//! below. The numeric constants are first-pass estimates that need calibration against a real
//! low-RAM DDR4 box (see each `CALIBRATE` note); the *shape* of the decision is what matters here.

use serde::Serialize;

// --- constants (CALIBRATE against the real low-RAM DDR4 rig before trusting the numbers) --------

/// Memory PM + the OS want to keep free so inference doesn't push the machine into swap. Subtracted
/// from `available_ram` to get the usable budget. CALIBRATE: 2 GB is a guess for a low-RAM box.
const PM_RESERVE_GB: f64 = 2.0;

/// VRAM PM keeps free when sizing the *GPU-resident* config, for the display framebuffer plus the
/// runtime's compute/context buffers that live outside the flat `OVERHEAD_GB`. Smaller than
/// `PM_RESERVE_GB` because VRAM holds only those, not the whole OS + PM. Subtracted from VRAM to get
/// the GPU budget; never added to the footprint. CALIBRATE: 1 GB is a first-pass guess.
const GPU_RESERVE_GB: f64 = 1.0;

/// Flat runtime overhead beyond weights + KV (compute buffers, allocator slack, the graph itself).
/// CALIBRATE.
const OVERHEAD_GB: f64 = 0.5;

/// Headroom above the fit at which we call it `Comfortable` rather than `Tight`. CALIBRATE.
const COMFORT_MARGIN_GB: f64 = 1.5;

/// The honest context floor: Ollama silently truncates at 4096, so halving never goes below it.
/// Below this we'd rather say `StayOnCloud` than promise a window we can't honour.
const CONTEXT_FLOOR: u32 = 4096;

/// System-RAM read bandwidth used for the CPU/mmap throughput estimate. CALIBRATE: ~40 GB/s is a
/// dual-channel DDR4 ballpark; real numbers vary widely by kit and channel population.
const SYSTEM_BANDWIDTH_GBPS: f64 = 40.0;

/// Fallback dedicated-GPU read bandwidth, used only when the footprint fits in VRAM *and* the card
/// wasn't recognised by the per-model bandwidth table (`hardware::gpu_bandwidth_gbps`). CALIBRATE:
/// mid-range discrete GPUs land ~300-500 GB/s; 400 is a deliberately mid, non-flattering pick for the
/// unknown case. A recognised card overrides this with its real spec via `FitHardware`.
const GPU_BANDWIDTH_FALLBACK_GBPS: f64 = 400.0;

/// f16 KV-cache proxy: GB of cache per (billion active params × token). Deliberately a compact
/// heuristic, not a per-architecture derivation — it can't see `n_kv_heads`/`head_dim`, so it
/// trends conservative for wide models. The q8_0 rung scales this by [`KvCache::size_ratio`].
/// CALIBRATE.
const KV_GB_PER_BPARAM_TOKEN: f64 = 8e-6;

// --- input / output model ----------------------------------------------------------------------

/// A model's architecture family, only to the resolution fit-scoring cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    /// A standard dense transformer: active params == total params.
    Dense,
    /// Mixture-of-experts: weights count all experts, but KV + throughput scale with *active*
    /// params only — which is why `active_params_b` is a distinct input.
    Moe,
    /// State-space / Mamba: the `params × ctx` KV proxy is simply wrong here, so we refuse to score.
    /// Also the home for any architecture we can't otherwise classify — refuse rather than guess.
    Ssm,
}

/// A GGUF quantization, ordered here best (largest, highest quality) to worst. The `bytes_per_param`
/// values approximate bits-per-weight / 8 for each scheme; they order quants by quality and feed the
/// throughput estimate, but the *memory* footprint always uses the catalog's measured `file_gb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[allow(non_camel_case_types)] // GGUF quant labels are the canonical names (Q4_K_M, IQ4_XS, …); serde emits them verbatim.
pub enum Quant {
    Q8_0,
    Q6_K,
    Q5_K_M,
    Q5_K_S,
    Q4_K_M,
    Q4_K_S,
    IQ4_XS,
    Q3_K_M,
    IQ3_M,
    Q2_K,
    IQ2_XS,
}

impl Quant {
    /// Approximate bytes per weight for this scheme (bits-per-weight / 8). CALIBRATE.
    pub fn bytes_per_param(self) -> f64 {
        match self {
            Quant::Q8_0 => 1.06,
            Quant::Q6_K => 0.82,
            Quant::Q5_K_M => 0.71,
            Quant::Q5_K_S => 0.69,
            Quant::Q4_K_M => 0.61,
            Quant::Q4_K_S => 0.58,
            Quant::IQ4_XS => 0.53,
            Quant::Q3_K_M => 0.49,
            Quant::IQ3_M => 0.44,
            Quant::Q2_K => 0.36,
            Quant::IQ2_XS => 0.30,
        }
    }

    /// Parse a GGUF quant label (e.g. `"Q4_K_M"`) into a known scheme, case-insensitively. Unknown
    /// labels return `None` — the caller drops that candidate rather than guessing a size.
    pub fn from_label(label: &str) -> Option<Quant> {
        match label.trim().to_ascii_uppercase().as_str() {
            "Q8_0" => Some(Quant::Q8_0),
            "Q6_K" => Some(Quant::Q6_K),
            "Q5_K_M" => Some(Quant::Q5_K_M),
            "Q5_K_S" => Some(Quant::Q5_K_S),
            "Q4_K_M" => Some(Quant::Q4_K_M),
            "Q4_K_S" => Some(Quant::Q4_K_S),
            "IQ4_XS" => Some(Quant::IQ4_XS),
            "Q3_K_M" => Some(Quant::Q3_K_M),
            "IQ3_M" => Some(Quant::IQ3_M),
            "Q2_K" => Some(Quant::Q2_K),
            "IQ2_XS" => Some(Quant::IQ2_XS),
            _ => None,
        }
    }
}

/// The KV-cache precision a fit was sized at. `F16` (2 bytes/element) is the conservative default;
/// `Q8_0` is the gentler lever the ladder tries *before* halving the context — roughly half the cache
/// size and near-lossless in practice (llama.cpp's `--cache-type-k q8_0`), so it can hold a larger
/// context or a higher weight quant than f16 could afford.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum KvCache {
    #[serde(rename = "f16")]
    F16,
    #[serde(rename = "q8_0")]
    Q8_0,
}

impl KvCache {
    /// Cache size relative to f16. q8_0 stores 8-bit values plus a per-block fp16 scale, so it lands a
    /// little above half of f16 — mirroring the 1.06-vs-2.0 bytes-per-weight ratio of Q8_0 vs f16
    /// weights. CALIBRATE.
    fn size_ratio(self) -> f64 {
        match self {
            KvCache::F16 => 1.0,
            KvCache::Q8_0 => 0.53,
        }
    }
}

/// KV precisions to try at each context rung, gentlest first. f16 is tried before q8_0 so any config
/// that fits at the conservative default is chosen unchanged; q8_0 only rescues a config that would
/// otherwise drop a weight quant, halve the context, or go to the cloud.
const KV_LADDER: [KvCache; 2] = [KvCache::F16, KvCache::Q8_0];

/// One downloadable quant of a model, paired with its measured on-disk size (all experts, all
/// shards summed — exactly what the catalog stores).
#[derive(Debug, Clone, Copy)]
pub struct QuantCandidate {
    pub quant: Quant,
    /// Measured file size in GB (billions of bytes) — the weight-memory term.
    pub weight_gb: f64,
}

/// The machine's memory, projected to just what fit-scoring needs.
#[derive(Debug, Clone, Copy)]
pub struct FitHardware {
    /// Free system RAM in GB. On Apple Silicon this is unified memory; on a discrete-GPU box it is
    /// system RAM (the always-available pool a model can run from, even if slowly).
    pub available_ram_gb: f64,
    /// Dedicated GPU VRAM in GB, if a reliable figure was read. The *quality* verdict is scored
    /// against RAM (so we never over-promise a fit we can't run); VRAM refines the speed estimate and
    /// drives the separate GPU-resident config (`gpu_fit`).
    pub vram_gb: Option<f64>,
    /// The GPU's real peak memory bandwidth (GB/s) when its model was recognised
    /// (`hardware::gpu_bandwidth_gbps`), else `None` → the flat [`GPU_BANDWIDTH_FALLBACK_GBPS`]. Only
    /// sharpens the on-GPU tok/s estimate; never affects which config fits.
    pub gpu_bandwidth_gbps: Option<f64>,
    /// Shared-memory GPU — Apple Silicon, OR a non-Apple integrated GPU / APU: VRAM is a slice of
    /// system RAM at the same bandwidth, so there is no distinct faster "GPU" config to offer
    /// (`gpu_fit` returns `Single`). Set by the hardware probe's integrated-GPU detection (#459).
    pub unified_memory: bool,
}

/// A model to score. `candidates` are best-quant-first; `active_params_b` drives the KV + throughput
/// terms (== total for dense, the smaller active count for MoE).
#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub arch: Architecture,
    pub active_params_b: f64,
    pub target_context: u32,
    pub multimodal: bool,
    /// The multimodal projector's size in GB. `Some(0.0)`/`None` differ: for a multimodal model a
    /// missing projector means we can't size it, so the fit is `Unknown`.
    pub projector_gb: Option<f64>,
    pub candidates: Vec<QuantCandidate>,
}

/// How well a model fits, coarsely — the vocabulary the UI speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Fits at full context with comfortable headroom.
    Comfortable,
    /// Fits at full context, but with little room to spare.
    Tight,
    /// Only fits with a reduced (halved, floored at 4096) context.
    HalvedContext,
    /// Doesn't fit even at the smallest quant and floor context — use the cloud.
    StayOnCloud,
    /// We can't compute a trustworthy fit (unmodelled architecture, or a multimodal model whose
    /// projector size is unknown). Never guessed.
    Unknown,
}

/// The full result of scoring one model against one machine.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FitResult {
    pub verdict: Verdict,
    /// The chosen quant (the best that fits), if any.
    pub quant: Option<Quant>,
    /// The context it fits at (== target, or a halved value ≥ 4096), if any.
    pub context: Option<u32>,
    /// The KV-cache precision this config was sized at: `f16` (the conservative default) or `q8_0`
    /// when the cache was compressed to keep a larger context or quant. The UI shows it per-config.
    pub kv: KvCache,
    pub est_memory_gb: Option<f64>,
    pub est_tokens_per_sec: Option<f64>,
    /// Honest, user-facing caveats (GPU-vs-RAM speed, halved context, thin headroom). The KV precision
    /// is carried structurally in `kv`, not here.
    pub notes: Vec<String>,
}

/// The relationship between a model's highest-quality (system-RAM) config and a faster GPU-resident
/// config, decided in Rust so the UI never has to infer the trade-off. The highest-quality config is
/// always the top-level `FitResult`; this only ever *adds* a faster alternative.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GpuFit {
    /// One config is the whole story: no discrete GPU, unified memory, a model we won't score, or the
    /// highest-quality config already fits VRAM (so it already runs at GPU speed).
    Single,
    /// A genuinely faster GPU-resident config exists beside the highest-quality one. Invariant: `fit`
    /// fits VRAM and its throughput beats the RAM config's.
    Split { fit: FitResult },
    /// A discrete GPU exists but nothing fits its VRAM even at the floor context (e.g. an MoE whose
    /// full weights exceed VRAM — still usable in system RAM at its active-parameter speed).
    NoGpuResident,
}

// --- the pure functions ------------------------------------------------------------------------

/// The system memory PM keeps free when scoring, so inference doesn't push the machine into swap.
/// Surfaced so the UI can state the reserve honestly rather than hide it in a pessimistic number.
pub fn reserve_gb() -> f64 {
    PM_RESERVE_GB
}

/// The VRAM PM keeps free when sizing the GPU-resident config. Surfaced so the UI can state it.
pub fn gpu_reserve_gb() -> f64 {
    GPU_RESERVE_GB
}

/// KV-cache footprint for `ctx` tokens at `active_params_b` billion active params, at the given cache
/// precision (`f16` is the conservative default; `q8_0` is roughly half the size).
pub fn kv_cache_gb(active_params_b: f64, ctx: u32, kv: KvCache) -> f64 {
    KV_GB_PER_BPARAM_TOKEN * active_params_b * f64::from(ctx) * kv.size_ratio()
}

/// Total resident footprint for one (candidate, context, KV-precision) triple: measured weights + KV
/// + a flat overhead + the multimodal projector (0 when there is none).
fn footprint_gb(spec: &ModelSpec, cand: &QuantCandidate, ctx: u32, kv: KvCache) -> f64 {
    cand.weight_gb
        + kv_cache_gb(spec.active_params_b, ctx, kv)
        + OVERHEAD_GB
        + spec.projector_gb.unwrap_or(0.0)
}

/// Rough decode throughput: read bandwidth divided by the *active* weight bytes touched per token
/// (MoE only reads its active experts). Uses GPU bandwidth when the footprint fits in VRAM, else
/// system RAM. Returns `None` if the model has no active weight bytes (nonsensical input).
fn tokens_per_sec(
    spec: &ModelSpec,
    cand: &QuantCandidate,
    footprint_gb: f64,
    hw: &FitHardware,
) -> Option<f64> {
    let active_weight_gb = spec.active_params_b * cand.quant.bytes_per_param();
    if active_weight_gb <= 0.0 {
        return None;
    }
    let on_gpu = hw.vram_gb.is_some_and(|v| v >= footprint_gb);
    let bandwidth = if on_gpu {
        // A recognised card's real spec, else the flat fallback for an unlisted GPU.
        hw.gpu_bandwidth_gbps.unwrap_or(GPU_BANDWIDTH_FALLBACK_GBPS)
    } else {
        SYSTEM_BANDWIDTH_GBPS
    };
    Some(bandwidth / active_weight_gb)
}

/// The context ladder: the target, then repeated halving, never below the floor. Always includes at
/// least the target (or the floor if the target is somehow below it).
fn context_ladder(target: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut ctx = target.max(CONTEXT_FLOOR);
    loop {
        out.push(ctx);
        let next = ctx / 2;
        if next < CONTEXT_FLOOR {
            break;
        }
        ctx = next;
    }
    out
}

/// Score one model against one machine's *system RAM* — the highest-quality config that fits. Pure.
/// A thin wrapper over [`fit_within`] with the RAM budget; behaviour is unchanged from before the
/// two-budget split (pinned by `fit_within_reproduces_fit_for_the_ram_budget`).
pub fn fit(spec: &ModelSpec, hw: &FitHardware) -> FitResult {
    let budget = (hw.available_ram_gb - PM_RESERVE_GB).max(0.0);
    fit_within(spec, budget, hw)
}

/// Score one model against an explicit memory `budget_gb`, reusing one degradation ladder. `fit()`
/// passes the system-RAM budget; [`gpu_fit`] passes the VRAM budget for the GPU-resident config.
///
/// Order of degradation (locked decision): keep the full context and step the quant down the ladder
/// first; only halve the context — the more alarming, visible compromise — when no quant fits at
/// full context. The refuse-to-guess guards live here so every budget refuses identically. `hw` is
/// used only to word the GPU/system-RAM note and pick the throughput bandwidth (both compare the
/// chosen footprint against raw `vram_gb`); the *budget* is the sole fit gate.
fn fit_within(spec: &ModelSpec, budget_gb: f64, hw: &FitHardware) -> FitResult {
    // Refuse-to-guess guards run first, before any arithmetic.
    if matches!(spec.arch, Architecture::Ssm) {
        return unknown(format!(
            "Fit can't be estimated for this architecture ({}).",
            arch_label(spec.arch)
        ));
    }
    if spec.multimodal && spec.projector_gb.is_none() {
        return unknown(
            "Fit can't be estimated: the vision projector's size is unknown.".to_string(),
        );
    }
    if spec.candidates.is_empty() {
        return unknown(
            "Fit can't be estimated: no known quantizations for this model.".to_string(),
        );
    }

    // Best quant first (highest bytes-per-param = highest quality that we might afford).
    let mut candidates = spec.candidates.clone();
    candidates.sort_by(|a, b| {
        b.quant
            .bytes_per_param()
            .partial_cmp(&a.quant.bytes_per_param())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // At each context rung, take the best weight quant that fits; within a quant, try f16 first and
    // only fall back to a q8_0 cache. This preserves the highest weight quant (compressing the
    // near-lossless cache to afford it) before dropping a quant, and keeps context (compressing the
    // cache before halving). f16-first at every step makes the whole thing non-regressing.
    for (rung, &ctx) in context_ladder(spec.target_context).iter().enumerate() {
        for cand in &candidates {
            for &kv in &KV_LADDER {
                let mem = footprint_gb(spec, cand, ctx, kv);
                if mem > budget_gb {
                    continue;
                }
                let halved = rung > 0;
                let headroom = budget_gb - mem;
                let verdict = if halved {
                    Verdict::HalvedContext
                } else if headroom >= COMFORT_MARGIN_GB {
                    Verdict::Comfortable
                } else {
                    Verdict::Tight
                };

                let mut notes: Vec<String> = Vec::new();
                let on_gpu = hw.vram_gb.is_some_and(|v| v >= mem);
                if on_gpu {
                    notes.push("Fits your GPU's memory — expect GPU-class speed.".to_string());
                } else if hw.vram_gb.is_some() {
                    notes.push(
                        "Larger than your GPU's memory — runs in system RAM (slower).".to_string(),
                    );
                }
                match verdict {
                    Verdict::HalvedContext => notes.push(format!(
                        "Context reduced to {ctx} tokens (from {}) to fit your memory.",
                        spec.target_context
                    )),
                    Verdict::Tight => {
                        notes.push("Fits, but with little memory headroom.".to_string());
                    }
                    _ => {}
                }

                return FitResult {
                    verdict,
                    quant: Some(cand.quant),
                    context: Some(ctx),
                    kv,
                    est_memory_gb: Some(round2(mem)),
                    est_tokens_per_sec: tokens_per_sec(spec, cand, mem, hw).map(round1),
                    notes,
                };
            }
        }
    }

    // Nothing fit, even the smallest quant with a q8_0 cache at the floor context.
    FitResult {
        verdict: Verdict::StayOnCloud,
        quant: None,
        context: None,
        kv: KvCache::F16,
        est_memory_gb: None,
        est_tokens_per_sec: None,
        notes: vec!["Too large for this machine's memory — better run in the cloud.".to_string()],
    }
}

/// Decide whether a faster GPU-resident config is worth showing beside the highest-quality
/// (`ram_fit`) one. Pure; reuses [`fit_within`] against the VRAM budget (`vram − GPU_RESERVE_GB`).
///
/// The "already fits the GPU" gate uses *raw* VRAM (not `vram − reserve`) on purpose: it must match
/// the note/speed predicate inside `fit_within` (`vram >= footprint`). If the quality config already
/// clears that bar it already reports GPU-class speed, so there is nothing faster to offer — and
/// gating on the reserve-shrunk budget here would let a config the same code labels "runs in system
/// RAM" sit beside a "fastest on GPU" row in the reserve band, contradicting itself.
pub fn gpu_fit(spec: &ModelSpec, hw: &FitHardware, ram_fit: &FitResult) -> GpuFit {
    // VRAM is a slice of the same RAM pool → no distinct faster config. `unified_memory` covers Apple
    // Silicon AND non-Apple integrated GPUs (AMD APU / Intel iGPU), flagged by the hardware probe
    // (#459: Windows by controller name, Linux AMD by PCI bus). What it deliberately does NOT change
    // is bandwidth realism for the *single* config that fits a shared carve-out (still scored at GPU
    // bandwidth) — that stays the v2 per-GPU calibration item.
    if hw.unified_memory {
        return GpuFit::Single;
    }
    let Some(vram) = hw.vram_gb else {
        return GpuFit::Single; // No discrete-GPU figure to size against.
    };
    // Never guess past the RAM verdict: an unscoreable model, or one already bound for the cloud.
    if matches!(ram_fit.verdict, Verdict::Unknown | Verdict::StayOnCloud) {
        return GpuFit::Single;
    }
    // The quality config already runs on the GPU, so it already reports GPU speed — nothing faster to
    // offer. Uses raw VRAM (not the reserve budget) to stay coherent with fit_within's own on-GPU
    // predicate; compared against the rounded `est_memory_gb`, so a sub-0.01 GB sliver at the exact
    // boundary can defer a Split (conservative — it only ever hides one, never fabricates a bad one).
    if ram_fit.est_memory_gb.is_some_and(|m| m <= vram) {
        return GpuFit::Single;
    }

    let gpu = fit_within(spec, (vram - GPU_RESERVE_GB).max(0.0), hw);
    // A GPU config only counts if it actually fits VRAM and differs from the RAM pick.
    if matches!(gpu.verdict, Verdict::Unknown | Verdict::StayOnCloud) {
        return GpuFit::NoGpuResident;
    }
    if gpu.quant == ram_fit.quant && gpu.context == ram_fit.context && gpu.kv == ram_fit.kv {
        return GpuFit::Single; // Defensive: identical pick — nothing distinct to show.
    }
    GpuFit::Split { fit: gpu }
}

/// A fit result for a model we deliberately won't score — an unmodelled architecture, a multimodal
/// model with no known projector, or (from the installed scan) a model not in the catalog. The
/// verdict is `Unknown`; `reason` is the single user-facing note.
pub fn unknown(reason: String) -> FitResult {
    FitResult {
        verdict: Verdict::Unknown,
        quant: None,
        context: None,
        kv: KvCache::F16,
        est_memory_gb: None,
        est_tokens_per_sec: None,
        notes: vec![reason],
    }
}

fn arch_label(arch: Architecture) -> &'static str {
    match arch {
        Architecture::Dense => "dense",
        Architecture::Moe => "mixture-of-experts",
        Architecture::Ssm => "state-space or unrecognized",
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-9;

    fn dense(active_b: f64, ctx: u32, candidates: Vec<QuantCandidate>) -> ModelSpec {
        ModelSpec {
            arch: Architecture::Dense,
            active_params_b: active_b,
            target_context: ctx,
            multimodal: false,
            projector_gb: None,
            candidates,
        }
    }

    fn q(quant: Quant, weight_gb: f64) -> QuantCandidate {
        QuantCandidate { quant, weight_gb }
    }

    fn ram(gb: f64) -> FitHardware {
        FitHardware {
            available_ram_gb: gb,
            vram_gb: None,
            gpu_bandwidth_gbps: None,
            unified_memory: false,
        }
    }

    /// A discrete-GPU machine: `ram` GB free system RAM, `vram` GB dedicated VRAM.
    fn gpu(ram: f64, vram: f64) -> FitHardware {
        FitHardware {
            available_ram_gb: ram,
            vram_gb: Some(vram),
            gpu_bandwidth_gbps: None,
            unified_memory: false,
        }
    }

    #[test]
    fn bytes_per_param_is_monotone_by_quality() {
        let ladder = [
            Quant::Q8_0,
            Quant::Q6_K,
            Quant::Q5_K_M,
            Quant::Q5_K_S,
            Quant::Q4_K_M,
            Quant::Q4_K_S,
            Quant::IQ4_XS,
            Quant::Q3_K_M,
            Quant::IQ3_M,
            Quant::Q2_K,
            Quant::IQ2_XS,
        ];
        for pair in ladder.windows(2) {
            assert!(
                pair[0].bytes_per_param() > pair[1].bytes_per_param(),
                "{:?} should weigh more than {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn from_label_is_case_insensitive_and_rejects_unknown() {
        assert_eq!(Quant::from_label("q4_k_m"), Some(Quant::Q4_K_M));
        assert_eq!(Quant::from_label(" IQ4_XS "), Some(Quant::IQ4_XS));
        assert_eq!(Quant::from_label("Q4_0"), None);
        assert_eq!(Quant::from_label("garbage"), None);
    }

    #[test]
    fn kv_cache_scales_with_active_params_and_context() {
        assert!((kv_cache_gb(7.0, 4096, KvCache::F16) - 8e-6 * 7.0 * 4096.0).abs() < EPS);
        // Doubling context doubles KV.
        assert!(
            (kv_cache_gb(7.0, 8192, KvCache::F16) - 2.0 * kv_cache_gb(7.0, 4096, KvCache::F16))
                .abs()
                < EPS
        );
        // q8_0 is a little above half of f16 (near-lossless, roughly halved footprint).
        let f16 = kv_cache_gb(7.0, 8192, KvCache::F16);
        let q8 = kv_cache_gb(7.0, 8192, KvCache::Q8_0);
        assert!(q8 < f16 && q8 > 0.4 * f16, "q8_0 KV should be ~half of f16");
    }

    #[test]
    fn comfortable_when_it_fits_with_headroom() {
        // 7B Q4 ~4.3 GB weights + tiny KV + 0.5 overhead ≈ 5 GB, on a 32 GB box (budget 30).
        let spec = dense(7.0, 8192, vec![q(Quant::Q4_K_M, 4.3), q(Quant::Q8_0, 8.0)]);
        let r = fit(&spec, &ram(32.0));
        assert_eq!(r.verdict, Verdict::Comfortable);
        // Best quant that fits at full context is chosen (Q8_0 fits comfortably here).
        assert_eq!(r.quant, Some(Quant::Q8_0));
        assert_eq!(r.context, Some(8192));
        // Comfortable fit uses the conservative f16 cache (no compression needed).
        assert_eq!(r.kv, KvCache::F16);
    }

    #[test]
    fn best_affordable_quant_is_picked_at_full_context() {
        // Budget only fits the Q4, not the Q8, at full context.
        let spec = dense(7.0, 4096, vec![q(Quant::Q4_K_M, 4.3), q(Quant::Q8_0, 8.0)]);
        // available 8 → budget 6. Q8 (8.0+..) doesn't fit; Q4 (4.3+..) does.
        let r = fit(&spec, &ram(8.0));
        assert_eq!(r.quant, Some(Quant::Q4_K_M));
        assert_eq!(r.context, Some(4096));
        assert!(matches!(r.verdict, Verdict::Comfortable | Verdict::Tight));
    }

    #[test]
    fn tight_when_headroom_is_thin() {
        // Footprint just under budget → Tight (headroom < COMFORT_MARGIN).
        let spec = dense(1.0, 4096, vec![q(Quant::Q4_K_M, 4.0)]);
        // budget = 6 - 2 = 4 ... need mem just below 4 but above 4 - 1.5. mem = 4.0 + kv + 0.5.
        let mem = footprint_gb(&spec, &q(Quant::Q4_K_M, 4.0), 4096, KvCache::F16);
        let avail = PM_RESERVE_GB + mem + 0.2; // headroom 0.2 < 1.5
        let r = fit(&spec, &ram(avail));
        assert_eq!(r.verdict, Verdict::Tight);
        assert!(r.notes.iter().any(|n| n.contains("little memory headroom")));
    }

    #[test]
    fn halves_context_when_even_a_q8_0_cache_does_not_fit_full_context() {
        // A big KV: even the compressed q8_0 cache at full context overflows, so the context halves.
        // active 40B → kv(8192) huge; kv(4096) half. Weight small so KV dominates.
        let spec = dense(40.0, 8192, vec![q(Quant::Q4_K_M, 1.0)]);
        let q8_full = footprint_gb(&spec, &q(Quant::Q4_K_M, 1.0), 8192, KvCache::Q8_0);
        let f16_half = footprint_gb(&spec, &q(Quant::Q4_K_M, 1.0), 4096, KvCache::F16);
        assert!(q8_full > f16_half); // the halved f16 config really is the smaller of the two
                                     // Budget below the gentlest full-context option (q8_0) but at/above the f16 half-context one.
        let avail = PM_RESERVE_GB + (q8_full + f16_half) / 2.0;
        let r = fit(&spec, &ram(avail));
        assert_eq!(r.verdict, Verdict::HalvedContext);
        assert_eq!(r.context, Some(4096));
        assert!(r.notes.iter().any(|n| n.contains("Context reduced")));
    }

    #[test]
    fn q8_0_kv_keeps_full_context_where_f16_alone_would_halve() {
        // KV-dominated: f16 at full context spills, but a q8_0 cache fits — so the window is kept,
        // sized on the compressed cache, instead of halved.
        let spec = dense(40.0, 8192, vec![q(Quant::Q4_K_M, 1.0)]);
        let f16_full = footprint_gb(&spec, &q(Quant::Q4_K_M, 1.0), 8192, KvCache::F16);
        let q8_full = footprint_gb(&spec, &q(Quant::Q4_K_M, 1.0), 8192, KvCache::Q8_0);
        assert!(q8_full < f16_full);
        // Budget between the two full-context footprints: f16 spills, q8_0 fits.
        let avail = PM_RESERVE_GB + (f16_full + q8_full) / 2.0;
        let r = fit(&spec, &ram(avail));
        assert_eq!(r.context, Some(8192)); // full context kept ...
        assert_eq!(r.kv, KvCache::Q8_0); // ... by compressing the cache
        assert_ne!(r.verdict, Verdict::HalvedContext);
    }

    #[test]
    fn q8_0_kv_holds_a_higher_weight_quant_than_f16_would_allow() {
        // A KV-heavy model with two quants. At this budget Q6_K only fits with a q8_0 cache; f16 would
        // force the lower Q4_K_M. The ladder keeps the higher weight quant on a compressed cache.
        let spec = dense(30.0, 8192, vec![q(Quant::Q6_K, 6.0), q(Quant::Q4_K_M, 4.0)]);
        // f16 KV(8192) ≈ 1.97: Q6_K f16 ≈ 8.47 (spills 8.0), Q6_K q8_0 ≈ 7.54 (fits), Q4_K_M f16 ≈ 6.47.
        let r = fit(&spec, &ram(PM_RESERVE_GB + 8.0));
        assert_eq!(r.quant, Some(Quant::Q6_K));
        assert_eq!(r.kv, KvCache::Q8_0);
        assert_eq!(r.context, Some(8192));
    }

    #[test]
    fn stays_on_cloud_when_nothing_fits_even_at_floor() {
        // 405B at Q2 is ~146 GB — no 16 GB box runs it.
        let spec = dense(
            405.0,
            8192,
            vec![q(Quant::IQ2_XS, 146.0), q(Quant::Q8_0, 430.0)],
        );
        let r = fit(&spec, &ram(16.0));
        assert_eq!(r.verdict, Verdict::StayOnCloud);
        assert!(r.quant.is_none() && r.context.is_none());
    }

    #[test]
    fn context_never_drops_below_floor() {
        for &c in &context_ladder(65536) {
            assert!(c >= CONTEXT_FLOOR);
        }
        // A target already below the floor still yields exactly the floor.
        assert_eq!(context_ladder(2048), vec![CONTEXT_FLOOR]);
    }

    #[test]
    fn reserve_is_applied() {
        // A model that fits `available` but not `available - reserve` must not be Comfortable.
        let spec = dense(1.0, 4096, vec![q(Quant::Q4_K_M, 5.0)]);
        let mem = footprint_gb(&spec, &q(Quant::Q4_K_M, 5.0), 4096, KvCache::F16); // ~5.5
                                                                                   // available = mem + reserve - 0.1 → budget = mem - 0.1 → does NOT fit.
        let r = fit(&spec, &ram(mem + PM_RESERVE_GB - 0.1));
        assert_eq!(r.verdict, Verdict::StayOnCloud);
    }

    #[test]
    fn moe_weights_all_experts_but_kv_uses_active() {
        // Two MoE models: same measured weight file, different active params → different KV/tok-s.
        let small_active = ModelSpec {
            arch: Architecture::Moe,
            active_params_b: 3.0,
            ..dense(3.0, 8192, vec![q(Quant::Q4_K_M, 18.0)])
        };
        let big_active = ModelSpec {
            active_params_b: 12.0,
            ..small_active.clone()
        };
        let hw = ram(64.0);
        let rs = fit(&small_active, &hw);
        let rb = fit(&big_active, &hw);
        // Same weights, bigger active → bigger KV → bigger memory, and slower tok/s.
        assert!(rb.est_memory_gb.unwrap() > rs.est_memory_gb.unwrap());
        assert!(rb.est_tokens_per_sec.unwrap() < rs.est_tokens_per_sec.unwrap());
    }

    #[test]
    fn multimodal_projector_adds_memory_and_missing_projector_is_unknown() {
        let base = ModelSpec {
            multimodal: true,
            projector_gb: Some(1.5),
            ..dense(7.0, 4096, vec![q(Quant::Q4_K_M, 4.3)])
        };
        let with_proj = fit(&base, &ram(32.0)).est_memory_gb.unwrap();
        let no_proj = fit(
            &ModelSpec {
                projector_gb: Some(0.0),
                ..base.clone()
            },
            &ram(32.0),
        )
        .est_memory_gb
        .unwrap();
        assert!((with_proj - no_proj - 1.5).abs() < 0.01);

        let missing = fit(
            &ModelSpec {
                projector_gb: None,
                ..base
            },
            &ram(32.0),
        );
        assert_eq!(missing.verdict, Verdict::Unknown);
    }

    #[test]
    fn ssm_architecture_is_unknown() {
        let ssm = ModelSpec {
            arch: Architecture::Ssm,
            ..dense(7.0, 4096, vec![q(Quant::Q4_K_M, 4.0)])
        };
        assert_eq!(fit(&ssm, &ram(64.0)).verdict, Verdict::Unknown);
    }

    #[test]
    fn empty_candidates_is_unknown_not_stay_on_cloud() {
        let spec = dense(7.0, 4096, vec![]);
        assert_eq!(fit(&spec, &ram(64.0)).verdict, Verdict::Unknown);
    }

    #[test]
    fn gpu_fit_reports_gpu_speed_and_is_faster() {
        let spec = dense(7.0, 4096, vec![q(Quant::Q4_K_M, 4.3)]);
        let cpu = fit(
            &spec,
            &FitHardware {
                available_ram_gb: 32.0,
                vram_gb: None,
                gpu_bandwidth_gbps: None,
                unified_memory: false,
            },
        );
        let on_gpu = fit(
            &spec,
            &FitHardware {
                available_ram_gb: 32.0,
                vram_gb: Some(24.0),
                gpu_bandwidth_gbps: None,
                unified_memory: false,
            },
        );
        assert!(on_gpu.est_tokens_per_sec.unwrap() > cpu.est_tokens_per_sec.unwrap());
        assert!(on_gpu.notes.iter().any(|n| n.contains("GPU-class speed")));
    }

    #[test]
    fn a_recognised_gpu_bandwidth_calibrates_the_tok_s_estimate() {
        // Same model, same VRAM fit — only the GPU's known bandwidth differs. A faster card must
        // report a proportionally faster tok/s; an unknown card falls back to the flat default.
        let spec = dense(7.0, 4096, vec![q(Quant::Q4_K_M, 4.3)]);
        let base = FitHardware {
            available_ram_gb: 32.0,
            vram_gb: Some(24.0),
            gpu_bandwidth_gbps: None, // → GPU_BANDWIDTH_FALLBACK_GBPS
            unified_memory: false,
        };
        let fast = FitHardware {
            gpu_bandwidth_gbps: Some(1008.0), // e.g. an RTX 4090
            ..base
        };
        let slow = FitHardware {
            gpu_bandwidth_gbps: Some(186.0), // e.g. an Arc A380
            ..base
        };
        let tps = |hw: &FitHardware| fit(&spec, hw).est_tokens_per_sec.unwrap();
        assert!(tps(&fast) > tps(&base)); // 1008 beats the 400 fallback
        assert!(tps(&base) > tps(&slow)); // 400 fallback beats a genuinely slow 186 card
                                          // Throughput scales linearly with bandwidth: 1008/186 ≈ the tok/s ratio.
        assert!((tps(&fast) / tps(&slow) - 1008.0 / 186.0).abs() < 0.2);
    }

    // --- two-budget GPU-resident scoring (#457) ------------------------------------------------

    #[test]
    fn fit_within_reproduces_fit_for_the_ram_budget() {
        // The extraction is behaviour-preserving: fit() is exactly fit_within() at the RAM budget.
        let spec = dense(7.0, 8192, vec![q(Quant::Q4_K_M, 4.3), q(Quant::Q8_0, 8.0)]);
        for hw in [ram(32.0), ram(8.0), gpu(22.0, 8.0), ram(3.0)] {
            let budget = (hw.available_ram_gb - PM_RESERVE_GB).max(0.0);
            assert_eq!(fit(&spec, &hw), fit_within(&spec, budget, &hw));
        }
    }

    #[test]
    fn gpu_fit_single_without_a_discrete_gpu() {
        let spec = dense(7.0, 8192, vec![q(Quant::Q8_0, 8.0)]);
        let hw = ram(32.0); // vram_gb == None
        let rf = fit(&spec, &hw);
        assert_eq!(gpu_fit(&spec, &hw, &rf), GpuFit::Single);
    }

    #[test]
    fn gpu_fit_single_on_unified_memory() {
        // A shared pool (Apple Silicon): VRAM is that same RAM, so there's no distinct faster config.
        let spec = dense(7.0, 8192, vec![q(Quant::Q8_0, 8.0)]);
        let hw = FitHardware {
            available_ram_gb: 24.0,
            vram_gb: Some(18.0),
            gpu_bandwidth_gbps: None,
            unified_memory: true,
        };
        let rf = fit(&spec, &hw);
        assert_eq!(gpu_fit(&spec, &hw, &rf), GpuFit::Single);
    }

    #[test]
    fn gpu_fit_splits_when_quality_spills_to_system_ram() {
        // The motivating case: big free RAM + small VRAM. fit() maxes fidelity (Q8_0, spills to RAM);
        // gpu_fit finds a smaller quant that fits VRAM and runs much faster.
        let spec = dense(7.0, 32768, vec![q(Quant::Q8_0, 7.5), q(Quant::Q4_K_M, 4.4)]);
        let hw = gpu(22.0, 8.0);
        let rf = fit(&spec, &hw);
        assert_eq!(rf.quant, Some(Quant::Q8_0));
        assert!(rf.est_memory_gb.unwrap() > 8.0); // the quality pick spilled past VRAM
        match gpu_fit(&spec, &hw, &rf) {
            GpuFit::Split { fit } => {
                assert_eq!(fit.quant, Some(Quant::Q4_K_M));
                // honours the GPU reserve (fits vram − GPU_RESERVE_GB, not just raw vram)
                assert!(fit.est_memory_gb.unwrap() <= 8.0 - gpu_reserve_gb() + 1e-6);
                assert!(fit.est_tokens_per_sec.unwrap() > rf.est_tokens_per_sec.unwrap());
                assert!(fit.notes.iter().any(|n| n.contains("GPU-class speed")));
            }
            other => panic!("expected Split, got {other:?}"),
        }
    }

    #[test]
    fn gpu_fit_single_when_quality_already_fits_the_gpu() {
        // A small model whose highest-quality config already fits VRAM → one config, no lossier split.
        let spec = dense(3.0, 8192, vec![q(Quant::Q8_0, 3.0)]);
        let hw = gpu(22.0, 8.0);
        let rf = fit(&spec, &hw);
        assert!(rf.est_memory_gb.unwrap() <= 8.0);
        assert_eq!(gpu_fit(&spec, &hw, &rf), GpuFit::Single);
    }

    #[test]
    fn gpu_fit_no_gpu_resident_when_nothing_fits_vram() {
        // Weights alone exceed VRAM (an MoE-shaped case): usable in RAM, but no GPU-resident config.
        let spec = dense(30.0, 8192, vec![q(Quant::Q4_K_M, 18.0)]);
        let hw = gpu(40.0, 8.0);
        let rf = fit(&spec, &hw);
        assert!(matches!(rf.verdict, Verdict::Comfortable | Verdict::Tight));
        assert_eq!(gpu_fit(&spec, &hw, &rf), GpuFit::NoGpuResident);
    }

    #[test]
    fn gpu_fit_reserve_excludes_a_config_that_only_fits_raw_vram() {
        // Q6_K's footprint (~8.0 GB) fits raw 8 GB VRAM but not the reserve-shrunk 7 GB budget, so no
        // GPU-resident config is offered — the reserve is honoured, not raw VRAM.
        let spec = dense(3.0, 4096, vec![q(Quant::Q8_0, 9.0), q(Quant::Q6_K, 7.4)]);
        let hw = gpu(22.0, 8.0);
        let rf = fit(&spec, &hw);
        assert!(rf.est_memory_gb.unwrap() > 8.0); // the quality pick (Q8_0) spilled past VRAM
        assert_eq!(gpu_fit(&spec, &hw, &rf), GpuFit::NoGpuResident);
    }

    #[test]
    fn gpu_fit_single_for_unknown_or_cloud_ram_fit() {
        let hw = gpu(22.0, 8.0);
        // Unscoreable architecture → RAM verdict Unknown → never invent a GPU config.
        let ssm = ModelSpec {
            arch: Architecture::Ssm,
            ..dense(7.0, 4096, vec![q(Quant::Q4_K_M, 4.0)])
        };
        let rf = fit(&ssm, &hw);
        assert_eq!(rf.verdict, Verdict::Unknown);
        assert_eq!(gpu_fit(&ssm, &hw, &rf), GpuFit::Single);

        // Too big even for RAM → StayOnCloud stands; a GPU sub-story would be noise.
        let huge = dense(405.0, 8192, vec![q(Quant::IQ2_XS, 146.0)]);
        let small = gpu(16.0, 8.0);
        let rf2 = fit(&huge, &small);
        assert_eq!(rf2.verdict, Verdict::StayOnCloud);
        assert_eq!(gpu_fit(&huge, &small, &rf2), GpuFit::Single);
    }

    #[test]
    fn gpu_fit_multimodal_projector_counts_against_vram() {
        let base = ModelSpec {
            multimodal: true,
            projector_gb: Some(1.0),
            ..dense(7.0, 32768, vec![q(Quant::Q8_0, 7.5), q(Quant::Q4_K_M, 4.4)])
        };
        let hw = gpu(22.0, 8.0);
        let rf = fit(&base, &hw);
        match gpu_fit(&base, &hw, &rf) {
            GpuFit::Split { fit } => {
                assert_eq!(fit.quant, Some(Quant::Q4_K_M));
                // the projector's 1 GB is inside the VRAM budget too
                assert!(fit.est_memory_gb.unwrap() <= 8.0 - gpu_reserve_gb() + 1e-6);
            }
            other => panic!("expected Split, got {other:?}"),
        }
        // No projector size for a multimodal model → unscoreable → no invented GPU config.
        let missing = ModelSpec {
            projector_gb: None,
            ..base
        };
        let rfm = fit(&missing, &hw);
        assert_eq!(rfm.verdict, Verdict::Unknown);
        assert_eq!(gpu_fit(&missing, &hw, &rfm), GpuFit::Single);
    }

    #[test]
    fn gpu_fit_uses_a_q8_0_cache_to_fit_a_config_into_vram() {
        // KV-dominated with small VRAM: the quality pick spills to RAM at f16; the GPU-resident config
        // fits VRAM only by compressing the cache to q8_0 (same quant + context) — so it Splits on the
        // KV difference alone and streams at GPU speed.
        let spec = dense(30.0, 16384, vec![q(Quant::Q5_K_M, 4.0)]);
        let hw = gpu(32.0, 8.0);
        let rf = fit(&spec, &hw);
        assert_eq!(rf.kv, KvCache::F16);
        assert!(rf.est_memory_gb.unwrap() > 8.0); // quality pick spilled past VRAM at f16
        match gpu_fit(&spec, &hw, &rf) {
            GpuFit::Split { fit } => {
                assert_eq!(fit.kv, KvCache::Q8_0);
                assert_eq!(fit.quant, rf.quant); // same quant + context ...
                assert_eq!(fit.context, rf.context);
                // ... it fits only because the cache is smaller
                assert!(fit.est_memory_gb.unwrap() <= 8.0 - gpu_reserve_gb() + 1e-6);
                assert!(fit.est_tokens_per_sec.unwrap() > rf.est_tokens_per_sec.unwrap());
                assert!(fit.notes.iter().any(|n| n.contains("GPU-class speed")));
            }
            other => panic!("expected a q8_0-KV Split, got {other:?}"),
        }
    }

    #[test]
    fn gpu_reserve_is_smaller_than_the_system_reserve() {
        // VRAM holds only the display + compute buffers, not the whole OS + PM.
        assert!(gpu_reserve_gb() < reserve_gb());
    }
}
