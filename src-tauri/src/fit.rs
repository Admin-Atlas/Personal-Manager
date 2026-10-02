// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pure model-fit scoring (#296): given a machine's memory and a model's size, decide the highest
//! quality (quant, context) that fits and roughly how fast it will run.
//!
//! The math follows the standard GGUF local-inference budget used by the public VRAM/RAM
//! calculators (weights + KV cache + a runtime overhead, scored against available memory minus a
//! reserve): this is PM's own implementation of that well-known approach, not a port of any one
//! tool. Three deliberate choices keep it honest:
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
//!   * The **KV** size comes from the model's own attention geometry ([`KvGeometry`]: its layers,
//!     KV heads and head size, its sliding-window layers, a hybrid's recurrent state), read from
//!     the GGUF header — not from its parameter count, which cannot see grouped-query attention and
//!     was off 13x for a model without it.
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

/// System-RAM read bandwidth used for the CPU/mmap throughput estimate.
///
/// 40 GB/s is not a neutral middle: it is arithmetically dual-channel DDR4-2400 (2 x 8 B x 2400 MT/s
/// = 38.4), so a dual-channel DDR5-5600 laptop — 89.6 GB/s peak — has its RAM-resident speed
/// understated by roughly 1.5x. It stays anyway, deliberately, for two reasons. Detecting the real
/// figure is not cheap or portable: Windows can read `Win32_PhysicalMemory` and Apple Silicon is a
/// per-chip constant, but on Linux the DMI tables are root-only and there is no unprivileged node
/// for memory generation or channel population — so the honest options are a per-OS seam or a
/// constant, and a constant that is wrong on two platforms is not better than one that is low on
/// three. And the direction matters: a RAM-resident config is the one that trips
/// `BACKGROUND_TOTAL_TIMEOUT`, where three slow calls in a row cool the endpoint down. Under-
/// promising costs a missed recommendation; over-promising costs a dead endpoint. CALIBRATE against
/// a real rig before raising it — nobody has yet.
const SYSTEM_BANDWIDTH_GBPS: f64 = 40.0;

/// Fallback dedicated-GPU read bandwidth, used only when the footprint fits in VRAM *and* the card
/// wasn't recognised by the per-model bandwidth table (`hardware::gpu_bandwidth_gbps`). CALIBRATE:
/// mid-range discrete GPUs land ~300-500 GB/s; 400 is a deliberately mid, non-flattering pick for the
/// unknown case. A recognised card overrides this with its real spec via `FitHardware`.
const GPU_BANDWIDTH_FALLBACK_GBPS: f64 = 400.0;

/// f16 KV-cache proxy: GB of cache per (billion active params × token), used ONLY for a spec that
/// carries no [`KvGeometry`] — every catalogue entry carries one, so in practice that is a test or
/// an entry whose GGUF header the generator could not read. It cannot see `n_kv_heads`/`head_dim`,
/// and it is wrong in both directions: calibrated on Qwen2.5 7B, it under-counts a model without
/// grouped-query attention (Phi 3.5 mini) about 13x and the Llama 3.x family 2-4.5x, and
/// over-counts sliding-window and hybrid models. The q8_0 rung scales this by
/// [`KvCache::size_ratio`].
const KV_GB_PER_BPARAM_TOKEN: f64 = 8e-6;

/// Bytes in the GB every size in this module is counted in. `file_gb` is written in GiB by the
/// catalogue generator, and the hardware probe reports VRAM and free RAM in GiB, so the KV term has
/// to be too.
const GIB: f64 = 1_073_741_824.0;

/// Tokens a sliding-window layer holds beyond its window: one batch, so a batch can be appended
/// before the oldest tokens fall out. Both runners PM sets up size it this way — llama.cpp's
/// sliding-window cache at `n_swa + n_ubatch` (src/llama-kv-cache-iswa.cpp, with `--swa-full` off,
/// llama-server's default) and Ollama's at the window plus its 512-token batch. Measured on Ollama
/// 0.33: gemma 3 4b's 29 sliding layers did not grow at all between an 8192 and a 32768 context. A
/// runtime that allocates sliding layers at the full context (`swa_full`) would exceed this
/// estimate.
const SWA_BATCH_TOKENS: u32 = 512;

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
    F16,
    Q8_0,
    Q6_K,
    Q5_1,
    Q5_K_M,
    Q5_K_S,
    Q5_0,
    Q4_1,
    Q4_K_M,
    Q4_K_S,
    Q4_0,
    IQ4_NL,
    Q3_K_L,
    IQ4_XS,
    Q3_K_M,
    IQ3_M,
    Q3_K_S,
    IQ3_XS,
    Q2_K,
    IQ2_M,
    IQ2_XS,
}

impl Quant {
    /// Approximate bytes per weight for this scheme (bits-per-weight / 8). CALIBRATE.
    pub fn bytes_per_param(self) -> f64 {
        match self {
            Quant::F16 => 2.00,
            Quant::Q8_0 => 1.06,
            Quant::Q6_K => 0.82,
            Quant::Q5_1 => 0.75,
            Quant::Q5_K_M => 0.71,
            Quant::Q5_K_S => 0.69,
            Quant::Q5_0 => 0.685,
            Quant::Q4_1 => 0.625,
            Quant::Q4_K_M => 0.61,
            Quant::Q4_K_S => 0.58,
            Quant::Q4_0 => 0.57,
            Quant::IQ4_NL => 0.56,
            Quant::Q3_K_L => 0.534,
            Quant::IQ4_XS => 0.53,
            Quant::Q3_K_M => 0.49,
            Quant::IQ3_M => 0.44,
            Quant::Q3_K_S => 0.43,
            Quant::IQ3_XS => 0.41,
            Quant::Q2_K => 0.36,
            Quant::IQ2_M => 0.33,
            Quant::IQ2_XS => 0.30,
        }
    }

    /// Parse a GGUF quant label (e.g. `"Q4_K_M"`) into a known scheme, case-insensitively. Unknown
    /// labels return `None` — the caller drops that candidate rather than guessing a size.
    ///
    /// The legacy (`Q4_0`, `Q5_1`, …) and full-precision labels matter as much as the K-quants here:
    /// on the on-disk path the weight is MEASURED from the file, so a label this table doesn't know
    /// throws away a fit PM could otherwise have worked out. `BF16`/`FP16` fold into `F16` because
    /// all three are two bytes a weight, which is the only thing this enum models about them.
    pub fn from_label(label: &str) -> Option<Quant> {
        match label.trim().to_ascii_uppercase().as_str() {
            "F16" | "FP16" | "BF16" => Some(Quant::F16),
            "Q8_0" => Some(Quant::Q8_0),
            "Q6_K" => Some(Quant::Q6_K),
            "Q5_1" => Some(Quant::Q5_1),
            "Q5_K_M" => Some(Quant::Q5_K_M),
            "Q5_K_S" => Some(Quant::Q5_K_S),
            "Q5_0" => Some(Quant::Q5_0),
            "Q4_1" => Some(Quant::Q4_1),
            "Q4_K_M" => Some(Quant::Q4_K_M),
            "Q4_K_S" => Some(Quant::Q4_K_S),
            "Q4_0" => Some(Quant::Q4_0),
            "IQ4_NL" => Some(Quant::IQ4_NL),
            "Q3_K_L" => Some(Quant::Q3_K_L),
            "IQ4_XS" => Some(Quant::IQ4_XS),
            "Q3_K_M" => Some(Quant::Q3_K_M),
            "IQ3_M" => Some(Quant::IQ3_M),
            "Q3_K_S" => Some(Quant::Q3_K_S),
            "IQ3_XS" => Some(Quant::IQ3_XS),
            "Q2_K" => Some(Quant::Q2_K),
            "IQ2_M" => Some(Quant::IQ2_M),
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
    /// Cache size relative to f16. q8_0 stores each block of 32 values as 32 bytes plus one fp16
    /// scale — 34 bytes against f16's 64 — so it is exactly 34/64 of f16. It was 0.53, a hair under
    /// that, and the memory contract is never to come in under the real figure.
    fn size_ratio(self) -> f64 {
        match self {
            KvCache::F16 => 1.0,
            KvCache::Q8_0 => 34.0 / 64.0,
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
    /// Measured file size in GiB (2^30 bytes, the unit of every size here) — the weight-memory term.
    pub weight_gb: f64,
}

/// What a model's KV cache costs, from its own attention geometry rather than its parameter count.
///
/// Read out of each catalogue entry's GGUF header by `scripts/generate-local-catalog.mjs`
/// (`kvFromHeader`, which says where each rule comes from): the layers whose cache spans the whole
/// context, the sliding-window layers that hold only their window, and the fixed recurrent state a
/// hybrid linear-attention model keeps on most layers instead of a cache. Bytes, at f16.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KvGeometry {
    /// What one token adds across the layers that keep the whole context.
    pub bytes_per_token: f64,
    /// What one token adds across the sliding-window layers, which hold at most
    /// `window + SWA_BATCH_TOKENS` tokens however long the context.
    pub window_bytes_per_token: f64,
    /// The sliding window, in tokens. Irrelevant when `window_bytes_per_token` is zero.
    pub window: u32,
    /// The recurrent state of a hybrid model's linear-attention layers: f32, and the same at every
    /// context and every cache precision.
    pub state_bytes: f64,
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
    /// The multimodal projector's size in GB, or `None` when the source didn't say. Charged into
    /// [`footprint_gb`] when known: an Ollama tag pulls the projector layer with the weights and
    /// the server holds it resident whether or not anything ever sends an image, and the on-disk
    /// scan measures a projector that is genuinely already there.
    pub projector_gb: Option<f64>,
    pub candidates: Vec<QuantCandidate>,
    /// The model's attention geometry, when known. Every catalogue entry carries one, and a served or
    /// on-disk model matched to an entry inherits that entry's. `None` falls back to the
    /// [`KV_GB_PER_BPARAM_TOKEN`] proxy.
    pub kv_geometry: Option<KvGeometry>,
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
    /// We can't compute a trustworthy fit — an unmodelled architecture, or (from the installed
    /// scan) a model that isn't in the catalog. Never guessed.
    Unknown,
}

/// Where a speed estimate's bandwidth figure came from, so the UI can say how far to trust it. The
/// arithmetic is the same on every path — bandwidth over the active weight bytes read per token — but
/// the bandwidth is a published spec on one path, a typical figure on two, and on shared memory a
/// number PM does not stand behind at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedBasis {
    /// Resident on a discrete GPU PM recognised, at that card's published memory bandwidth.
    GpuPublished,
    /// Resident on a discrete GPU PM did not recognise, at [`GPU_BANDWIDTH_FALLBACK_GBPS`].
    GpuTypical,
    /// Resident in memory shared with the processor (Apple Silicon, an APU, an iGPU). Shared-memory
    /// bandwidth varies too much from chip to chip for PM to put a number on it yet.
    Shared,
    /// Larger than the GPU (or there is none), so it runs from system RAM at
    /// [`SYSTEM_BANDWIDTH_GBPS`].
    System,
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
    /// Which bandwidth [`Self::est_tokens_per_sec`] was worked out from, so the UI can word the
    /// number as a ceiling, a typical figure or no figure at all. `None` when there is no estimate.
    pub speed_basis: Option<SpeedBasis>,
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

/// KV-cache footprint, in GiB, for `ctx` tokens of `spec` at the given cache precision (`f16` is the
/// conservative default; `q8_0` is 34/64 of it).
///
/// From the model's [`KvGeometry`] when the spec carries one: the full-context layers pay for every
/// token, the sliding-window layers for no more than their window plus a batch, and a hybrid's
/// recurrent state is added once, uncompressed. Measured against a live Ollama 0.33 (q8_0 cache,
/// flash attention, an RTX 5060 Laptop GPU, 02-10-2026), the footprint this feeds came out +9.6%
/// over Qwen2.5 7B Q5_K_M's real load at 32768 tokens and +11.4% at 8192, and +2.0% / +1.6% over
/// gemma 3 4b Q4_K_M's: inside the ±15% contract, and never under it. Without a geometry, the
/// parameter-count proxy ([`KV_GB_PER_BPARAM_TOKEN`]).
pub fn kv_cache_gb(spec: &ModelSpec, ctx: u32, kv: KvCache) -> f64 {
    let Some(g) = spec.kv_geometry else {
        return KV_GB_PER_BPARAM_TOKEN * spec.active_params_b * f64::from(ctx) * kv.size_ratio();
    };
    let window_tokens = ctx.min(g.window.saturating_add(SWA_BATCH_TOKENS));
    let cache =
        g.bytes_per_token * f64::from(ctx) + g.window_bytes_per_token * f64::from(window_tokens);
    (cache * kv.size_ratio() + g.state_bytes) / GIB
}

/// Total resident footprint for one (candidate, context, KV-precision) triple: measured weights + KV
/// + a flat overhead + the multimodal projector (0 when there is none).
fn footprint_gb(spec: &ModelSpec, cand: &QuantCandidate, ctx: u32, kv: KvCache) -> f64 {
    cand.weight_gb + kv_cache_gb(spec, ctx, kv) + OVERHEAD_GB + spec.projector_gb.unwrap_or(0.0)
}

/// Rough decode throughput: read bandwidth divided by the *active* weight bytes touched per token
/// (MoE only reads its active experts). Uses GPU bandwidth when the footprint fits in VRAM, else
/// system RAM, and says which. Returns `None` if the model has no active weight bytes (nonsensical
/// input).
///
/// Speed is an upper bound, not a forecast. On a recognised discrete GPU with the config resident on
/// the GPU, it must never be below real decode speed, and has measured +11% (Qwen2.5 7B Q5_K_M: est
/// 71, ~64 real) to +55% (gemma 3 4b Q4_K_M: est 162, ~105 real) above it on an RTX 5060 Laptop GPU
/// at 384 GB/s (n=2). PM claims no tolerance for an unrecognised card, shared memory or system RAM,
/// and no ranking may order by its magnitude: the one comparison PM makes is the background floor
/// (better_fit), computed at the deliberately low SYSTEM_BANDWIDTH_GBPS.
fn tokens_per_sec(
    spec: &ModelSpec,
    cand: &QuantCandidate,
    footprint_gb: f64,
    hw: &FitHardware,
) -> Option<(f64, SpeedBasis)> {
    let active_weight_gb = spec.active_params_b * cand.quant.bytes_per_param();
    if active_weight_gb <= 0.0 {
        return None;
    }
    let on_gpu = hw.vram_gb.is_some_and(|v| v >= footprint_gb);
    if !on_gpu {
        return Some((SYSTEM_BANDWIDTH_GBPS / active_weight_gb, SpeedBasis::System));
    }
    // A recognised card's real spec, else the flat fallback for an unlisted GPU. The basis changes
    // only what PM is willing to claim about the number, never the number: shared memory keeps the
    // figure it always had, and the UI declines to show it.
    let bandwidth = hw.gpu_bandwidth_gbps.unwrap_or(GPU_BANDWIDTH_FALLBACK_GBPS);
    let basis = if hw.unified_memory {
        SpeedBasis::Shared
    } else if hw.gpu_bandwidth_gbps.is_some() {
        SpeedBasis::GpuPublished
    } else {
        SpeedBasis::GpuTypical
    };
    Some((bandwidth / active_weight_gb, basis))
}

/// Decode speed from system RAM at [`SYSTEM_BANDWIDTH_GBPS`], whatever the machine has. The one speed
/// figure PM compares against anything: the background floor in `better_fit`, which needs the
/// pessimistic number on purpose — a model that clears it from system RAM clears it anywhere.
pub fn system_tokens_per_sec(active_params_b: f64, q: Quant) -> f64 {
    SYSTEM_BANDWIDTH_GBPS / (active_params_b * q.bytes_per_param())
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
    fit_within(spec, ram_budget_gb(hw), hw)
}

/// The system-RAM a model is scored against: free RAM less the reserve, floored at zero. One
/// definition, so a caller weighing two models against this budget cannot drift from the one a
/// single model's verdict was computed with.
pub fn ram_budget_gb(hw: &FitHardware) -> f64 {
    (hw.available_ram_gb - PM_RESERVE_GB).max(0.0)
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
                if on_gpu && hw.unified_memory {
                    // Shared memory is not a faster pool than the RAM it is carved from, so the
                    // discrete-card promise below would be a speed claim PM cannot make here.
                    notes.push("Fits the memory this computer's graphics can use.".to_string());
                } else if on_gpu {
                    notes.push("Fits your GPU's memory — expect GPU-class speed.".to_string());
                } else if hw.vram_gb.is_some() {
                    notes.push(
                        "Larger than your GPU's memory — runs in system RAM (slower).".to_string(),
                    );
                }
                // The notes are derived from the FACTS, not from the verdict. `Verdict` holds one
                // value, and `halved` claims it first — so a pick that had to halve its context AND
                // landed on the budget floor used to report the halving and stay silent about the
                // headroom. The configs scraping the floor were exactly the ones carrying no
                // warning: the suite's own `halves_context_when_even_a_q8_0_cache_does_not_fit_full_context`
                // sits 39 MB under its budget and says nothing about it. Both are true at once, they
                // answer different questions — what was given up, and what is left over — so both
                // are said. No verdict VALUE changes, so nothing that ranks or gates on one moves.
                if halved {
                    notes.push(format!(
                        "Context reduced to {ctx} tokens (from {}) to fit your memory.",
                        spec.target_context
                    ));
                }
                if headroom < COMFORT_MARGIN_GB {
                    notes.push("Fits, but with little memory headroom.".to_string());
                }

                let speed = tokens_per_sec(spec, cand, mem, hw);
                return FitResult {
                    verdict,
                    quant: Some(cand.quant),
                    context: Some(ctx),
                    kv,
                    est_memory_gb: Some(round2(mem)),
                    est_tokens_per_sec: speed.map(|(tps, _)| round1(tps)),
                    speed_basis: speed.map(|(_, basis)| basis),
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
        speed_basis: None,
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
    // (#459: Windows by controller name, Linux AMD by PCI bus). It does NOT change bandwidth realism
    // for the *single* config that fits a shared carve-out: that stays the flat GPU-bandwidth fallback.
    // Per-GPU bandwidth calibration shipped (#467) but deliberately skips unified memory — an
    // Apple/APU/iGPU probe name is generic, matches no entry, and resolves to the fallback anyway.
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

/// The best config that fits the card with its reserve AND free RAM.
///
/// What PM's pick (`better_fit::judge`) sizes against on a discrete card: a config that lives
/// entirely on the GPU, with the [`GPU_RESERVE_GB`] PM keeps free there, that the machine can also
/// hold in RAM right now. `None` on unified memory or with no card figure (the same two guards
/// [`gpu_fit`] opens with), when the RAM verdict already refused (`Unknown` / `StayOnCloud`), or
/// when nothing fits the card at all.
///
/// For every [`GpuFit::Split`] this is the very rung `gpu_fit` returned — the same call at the same
/// budget, because a Split's RAM config is larger than the card, so the RAM budget never binds — and
/// for [`GpuFit::NoGpuResident`] it is `None`. The one case it adds is the reserve band: a RAM config
/// in `(vram − GPU_RESERVE_GB, vram]`, which `gpu_fit` calls `Single` because it already reports GPU
/// speed, while it does not keep the reserve. There this returns the config that does.
pub fn resident_fit(spec: &ModelSpec, hw: &FitHardware, ram_fit: &FitResult) -> Option<FitResult> {
    if hw.unified_memory {
        return None;
    }
    let vram = hw.vram_gb?;
    if matches!(ram_fit.verdict, Verdict::Unknown | Verdict::StayOnCloud) {
        return None;
    }
    let budget = (vram - GPU_RESERVE_GB).max(0.0).min(ram_budget_gb(hw));
    let g = fit_within(spec, budget, hw);
    (!matches!(g.verdict, Verdict::Unknown | Verdict::StayOnCloud)).then_some(g)
}

/// A fit result for a model we deliberately won't score — an unmodelled architecture, or (from the
/// installed scan) a model not in the catalog. The verdict is `Unknown`; `reason` is the single
/// user-facing note.
pub fn unknown(reason: String) -> FitResult {
    FitResult {
        verdict: Verdict::Unknown,
        quant: None,
        context: None,
        kv: KvCache::F16,
        est_memory_gb: None,
        est_tokens_per_sec: None,
        speed_basis: None,
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

// --- two models on one machine (#786 item 6) ----------------------------------------------------

/// The memory estimate's own stated tolerance (DECISIONS.md ±15%), measured at +11.3% against a real
/// load on real hardware. A combined figure that overshoots a budget by less than this is inside PM's
/// own error bar, and PM has to say so rather than pick a side it cannot defend.
///
/// The asymmetry is deliberate and it is the whole reason this band exists. The estimate runs HIGH,
/// so "these fit" is the safe verdict — if the over-estimate fits, the real thing fits. "These will
/// not both stay loaded" is the one that can be wrong about a setup that works, and a confident wrong
/// warning is worse than the vague prose it replaces.
const ESTIMATE_TOLERANCE: f64 = 0.15;

/// How two models bound to the two roles behave when they share one server.
///
/// The question is NOT whether the machine will fail. Ollama's own FAQ is explicit that it does not:
/// when a new model will not fit beside a loaded one, "all new requests will be queued until the new
/// model can be loaded. As prior models become idle, one or more will be unloaded to make room". So
/// the outcome of exceeding the budget is eviction and reloading, not a crash — every alternation
/// between the chat role and the background role paying an unload plus a cold load (measured at
/// 2.6-4.2 s on a laptop GPU, 30-08-2026). That compounds against the flat 180 s background timeout
/// whose third strike cools the endpoint down for chat as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoResidency {
    /// Both stay loaded together, with the estimate on the safe side of the budget.
    Fits,
    /// The sum lands inside the estimate's own error band. PM cannot honestly call it either way.
    TooClose,
    /// They will not both stay loaded, so the server will swap between them.
    Exceeds,
    /// At least one of them could not be sized, so there is no sum to take. Never guessed.
    Unknown,
}

/// Two models weighed against one machine, on both budgets that can bind.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoResidencyFit {
    /// Against system RAM.
    pub ram: CoResidency,
    /// Against dedicated video memory. `None` when memory is unified or there is no discrete card —
    /// there is then no separate graphics-memory question, mirroring [`gpu_fit`]'s own guards.
    pub vram: Option<CoResidency>,
    /// The two footprints summed, in GB. `None` when either could not be sized.
    pub combined_gb: Option<f64>,
    pub ram_budget_gb: f64,
    pub vram_budget_gb: Option<f64>,
}

/// Weigh two models against one machine.
///
/// The sum is of `est_memory_gb` — the very numbers the two cards displayed — so this can never
/// contradict what the user was already shown. It composes exactly, which is not an accident of this
/// function but a property of [`footprint_gb`]: `OVERHEAD_GB` is per-model runtime cost (compute
/// buffers, the graph), so charging it twice for two models is correct rather than a double count,
/// while both reserves are subtracted from the BUDGET and never added to a footprint — so summing two
/// footprints against one budget charges each reserve exactly once.
///
/// A model PM could not size, or one it already told you to keep on the cloud, yields `Unknown`
/// rather than a sum with a hole in it.
pub fn co_residency(a: &FitResult, b: &FitResult, hw: &FitHardware) -> CoResidencyFit {
    let ram_budget = ram_budget_gb(hw);
    // No discrete-GPU question when the card shares the RAM pool, or when there is no card figure —
    // the same two guards `gpu_fit` opens with, for the same reasons.
    let vram_budget = (!hw.unified_memory)
        .then_some(hw.vram_gb)
        .flatten()
        .map(|v| (v - GPU_RESERVE_GB).max(0.0));

    let (Some(fa), Some(fb)) = (sizable_footprint(a), sizable_footprint(b)) else {
        return CoResidencyFit {
            ram: CoResidency::Unknown,
            vram: vram_budget.map(|_| CoResidency::Unknown),
            combined_gb: None,
            ram_budget_gb: ram_budget,
            vram_budget_gb: vram_budget,
        };
    };
    let combined = fa + fb;
    CoResidencyFit {
        ram: classify_combined(combined, ram_budget),
        vram: vram_budget.map(|b| classify_combined(combined, b)),
        combined_gb: Some(combined),
        ram_budget_gb: ram_budget,
        vram_budget_gb: vram_budget,
    }
}

/// The footprint to charge for one model, or `None` when there is no honest number.
///
/// `StayOnCloud` is excluded as well as `Unknown`: PM has already said not to run that one locally, so
/// adding it into a co-residency sum would produce a second warning about a decision the user has
/// been told not to make.
fn sizable_footprint(f: &FitResult) -> Option<f64> {
    match f.verdict {
        Verdict::Unknown | Verdict::StayOnCloud => None,
        _ => f.est_memory_gb,
    }
}

fn classify_combined(combined_gb: f64, budget_gb: f64) -> CoResidency {
    if combined_gb <= budget_gb {
        CoResidency::Fits
    } else if combined_gb <= budget_gb * (1.0 + ESTIMATE_TOLERANCE) {
        CoResidency::TooClose
    } else {
        CoResidency::Exceeds
    }
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
            projector_gb: None,
            candidates,
            kv_geometry: None,
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

    // --- two models on one machine (#786 item 6) -------------------------------------------------

    /// A fit with a known footprint, for the co-residency sums. The verdict only has to be one the
    /// sum is allowed to use.
    fn sized(gb: f64) -> FitResult {
        FitResult {
            verdict: Verdict::Comfortable,
            quant: Some(Quant::Q4_K_M),
            context: Some(32768),
            kv: KvCache::F16,
            est_memory_gb: Some(gb),
            est_tokens_per_sec: Some(30.0),
            speed_basis: None,
            notes: vec![],
        }
    }

    #[test]
    fn two_models_are_weighed_against_one_budget_with_the_reserve_charged_once() {
        // The composition this whole feature rests on. 16 GB free, PM_RESERVE_GB=2 → a 14 GB budget.
        // Two 6 GB models sum to 12 and fit; the reserve must not be subtracted twice, which would
        // leave 12 and make the same pair look impossible.
        let hw = ram(16.0);
        let both = co_residency(&sized(6.0), &sized(6.0), &hw);
        assert!(
            (both.ram_budget_gb - 14.0).abs() < EPS,
            "reserve charged once"
        );
        assert_eq!(both.combined_gb, Some(12.0));
        assert_eq!(both.ram, CoResidency::Fits);
    }

    #[test]
    fn a_pair_that_overshoots_by_more_than_the_estimates_own_error_is_called() {
        // 14 GB budget. 16.2 GB is 15.7% over — outside the +-15% the memory estimate is allowed.
        let hw = ram(16.0);
        assert_eq!(
            co_residency(&sized(8.1), &sized(8.1), &hw).ram,
            CoResidency::Exceeds
        );
    }

    #[test]
    fn a_pair_inside_the_estimates_error_bar_is_not_called_either_way() {
        // 14 GB budget, 15 GB combined: over, but only by 7%. PM's own memory estimate ran +11.3%
        // against a real load, so a confident "these will not both stay loaded" here would be a
        // claim the estimator cannot support. The band exists so PM says so instead of picking.
        let hw = ram(16.0);
        assert_eq!(
            co_residency(&sized(7.5), &sized(7.5), &hw).ram,
            CoResidency::TooClose
        );
    }

    #[test]
    fn a_model_that_could_not_be_sized_yields_no_sum_at_all() {
        // Never a sum with a hole in it. `StayOnCloud` is excluded alongside `Unknown`: PM has
        // already told the user not to run that one locally, and a second warning about a decision
        // they were told not to make is noise.
        let hw = ram(16.0);
        for verdict in [Verdict::Unknown, Verdict::StayOnCloud] {
            let mut bad = sized(6.0);
            bad.verdict = verdict;
            let out = co_residency(&sized(6.0), &bad, &hw);
            assert_eq!(out.ram, CoResidency::Unknown, "{verdict:?}");
            assert_eq!(out.combined_gb, None, "{verdict:?}");
        }
    }

    #[test]
    fn the_graphics_memory_question_is_asked_only_where_there_is_one() {
        // Mirrors `gpu_fit`'s own two guards. Unified memory means VRAM is a slice of the same pool
        // already counted in the RAM budget, and no card figure means nothing to size against —
        // asking either way would invent a second budget that does not exist.
        let mut unified = gpu(16.0, 8.0);
        unified.unified_memory = true;
        assert_eq!(co_residency(&sized(3.0), &sized(3.0), &unified).vram, None);
        assert_eq!(
            co_residency(&sized(3.0), &sized(3.0), &ram(16.0)).vram,
            None
        );

        // 8 GB card, GPU_RESERVE_GB=1 → a 7 GB budget. Two 3 GB models fit on the card; two 5 GB
        // ones do not, while both pairs still fit in system RAM. The two budgets must be able to
        // disagree, because that disagreement IS the finding: they stay loaded, just not on the GPU.
        let hw = gpu(32.0, 8.0);
        let small = co_residency(&sized(3.0), &sized(3.0), &hw);
        assert!((small.vram_budget_gb.unwrap() - 7.0).abs() < EPS);
        assert_eq!(small.vram, Some(CoResidency::Fits));
        let big = co_residency(&sized(5.0), &sized(5.0), &hw);
        assert_eq!(big.ram, CoResidency::Fits);
        assert_eq!(big.vram, Some(CoResidency::Exceeds));
    }

    #[test]
    fn a_machine_with_less_memory_than_the_reserve_gets_a_zero_budget_not_a_negative_one() {
        let out = co_residency(&sized(1.0), &sized(1.0), &ram(1.0));
        assert_eq!(out.ram_budget_gb, 0.0);
        assert_eq!(out.ram, CoResidency::Exceeds);
    }

    #[test]
    fn bytes_per_param_is_monotone_by_quality() {
        // Every variant, best to worst. The scorer sorts candidates by `bytes_per_param`, so this
        // ordering is the real quality ladder — a new variant slotted in at the wrong weight would
        // silently reorder which quant the fit prefers.
        let ladder = [
            Quant::F16,
            Quant::Q8_0,
            Quant::Q6_K,
            Quant::Q5_1,
            Quant::Q5_K_M,
            Quant::Q5_K_S,
            Quant::Q5_0,
            Quant::Q4_1,
            Quant::Q4_K_M,
            Quant::Q4_K_S,
            Quant::Q4_0,
            Quant::IQ4_NL,
            Quant::Q3_K_L,
            Quant::IQ4_XS,
            Quant::Q3_K_M,
            Quant::IQ3_M,
            Quant::Q3_K_S,
            Quant::IQ3_XS,
            Quant::Q2_K,
            Quant::IQ2_M,
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
        // The legacy and full-precision labels are known now — a file carrying one used to be
        // discarded as unscoreable even when its size on disk was measured exactly.
        assert_eq!(Quant::from_label("Q4_0"), Some(Quant::Q4_0));
        assert_eq!(Quant::from_label("q3_k_l"), Some(Quant::Q3_K_L));
        assert_eq!(Quant::from_label("IQ4_NL"), Some(Quant::IQ4_NL));
        // BF16 and FP16 weigh exactly what F16 weighs, which is all this enum claims to model.
        assert_eq!(Quant::from_label("BF16"), Some(Quant::F16));
        assert_eq!(Quant::from_label("fp16"), Some(Quant::F16));
        // Still refused rather than guessed: a real llama.cpp scheme PM has no weight for.
        assert_eq!(Quant::from_label("TQ1_0"), None);
        assert_eq!(Quant::from_label("garbage"), None);
    }

    #[test]
    fn without_a_geometry_the_kv_proxy_scales_with_active_params_and_context() {
        let spec = dense(7.0, 4096, vec![]);
        assert!((kv_cache_gb(&spec, 4096, KvCache::F16) - 8e-6 * 7.0 * 4096.0).abs() < EPS);
        // Doubling context doubles KV.
        assert!(
            (kv_cache_gb(&spec, 8192, KvCache::F16) - 2.0 * kv_cache_gb(&spec, 4096, KvCache::F16))
                .abs()
                < EPS
        );
        // q8_0 is exactly 34/64 of f16: 32 one-byte values and one two-byte scale per block of 32.
        let f16 = kv_cache_gb(&spec, 8192, KvCache::F16);
        let q8 = kv_cache_gb(&spec, 8192, KvCache::Q8_0);
        assert!((q8 - f16 * 34.0 / 64.0).abs() < EPS, "{q8} vs {f16}");
    }

    /// The geometry a catalogue entry carries, in bytes per token at f16.
    fn geometry(full: f64, windowed: f64, window: u32, state: f64) -> KvGeometry {
        KvGeometry {
            bytes_per_token: full,
            window_bytes_per_token: windowed,
            window,
            state_bytes: state,
        }
    }

    #[test]
    fn a_model_without_grouped_query_attention_is_sized_from_its_own_heads() {
        // Phi 3.5 mini: 32 layers × 32 KV heads × 96 × K and V × 2 bytes = 393216 bytes a token,
        // so 12 GiB at 32768 tokens. The proxy said about 1 GB — the 13x under-count that made PM
        // pick a 128k-context Phi for a 6 GB card that would need about 27 GB of q8_0 cache.
        let proxy = dense(3.82, 131072, vec![q(Quant::Q4_K_M, 2.23)]);
        let phi = ModelSpec {
            kv_geometry: Some(geometry(393_216.0, 0.0, 0, 0.0)),
            ..proxy.clone()
        };
        assert!((kv_cache_gb(&phi, 32768, KvCache::F16) - 12.0).abs() < EPS);
        assert!((kv_cache_gb(&phi, 32768, KvCache::Q8_0) - 6.375).abs() < EPS);
        assert!(
            kv_cache_gb(&phi, 32768, KvCache::F16)
                > 10.0 * kv_cache_gb(&proxy, 32768, KvCache::F16)
        );
        // And so it no longer fits a 6 GB card at its trained context: the resident budget is 5.
        let card = FitHardware {
            available_ram_gb: 12.0,
            vram_gb: Some(6.0),
            gpu_bandwidth_gbps: Some(288.0),
            unified_memory: false,
        };
        let rf = fit(&phi, &card);
        let resident = resident_fit(&phi, &card, &rf).expect("something fits the card");
        assert_eq!(resident.verdict, Verdict::HalvedContext, "{resident:?}");
    }

    #[test]
    fn a_sliding_window_layer_stops_growing_at_its_window() {
        // gemma 3 4b: five full layers, 29 that keep only the last 1024 tokens (+ one 512 batch).
        let spec = ModelSpec {
            kv_geometry: Some(geometry(20_480.0, 118_784.0, 1024, 0.0)),
            ..dense(3.88, 131072, vec![q(Quant::Q4_K_M, 2.32)])
        };
        let windowed = 118_784.0 * 1536.0 / GIB;
        let at = |ctx: u32| kv_cache_gb(&spec, ctx, KvCache::F16);
        assert!((at(32768) - (20_480.0 * 32768.0 / GIB + windowed)).abs() < EPS);
        assert!((at(131072) - (20_480.0 * 131072.0 / GIB + windowed)).abs() < EPS);
        // Below the window the sliding layers hold the whole (short) context, like any other layer.
        assert!((at(1024) - (20_480.0 + 118_784.0) * 1024.0 / GIB).abs() < EPS);
    }

    #[test]
    fn a_hybrid_models_recurrent_state_is_paid_once_and_never_compressed() {
        // Qwen3.5 4B: 8 attention layers keep a cache; the 24 linear-attention layers a fixed f32
        // state of 52690944 bytes between them, which q8_0 does not shrink and context does not grow.
        let spec = ModelSpec {
            kv_geometry: Some(geometry(32_768.0, 0.0, 0, 52_690_944.0)),
            ..dense(4.21, 262144, vec![q(Quant::Q4_K_M, 2.55)])
        };
        let state = 52_690_944.0 / GIB;
        let cache = |ctx: u32| 32_768.0 * f64::from(ctx) / GIB;
        assert!((kv_cache_gb(&spec, 32768, KvCache::F16) - (cache(32768) + state)).abs() < EPS);
        assert!(
            (kv_cache_gb(&spec, 32768, KvCache::Q8_0) - (cache(32768) * 34.0 / 64.0 + state)).abs()
                < EPS
        );
    }

    #[test]
    fn the_estimate_brackets_the_loads_measured_on_a_real_card() {
        // Ollama 0.33, q8_0 cache, flash attention, RTX 5060 Laptop GPU, 02-10-2026: what the card
        // held for the model (nvidia-smi, less the 79 MiB it held idle), at two contexts each. The
        // geometries are the catalogue's own for these two entries (pinned in local_catalog). The
        // memory contract is ±15%, and never under.
        let qwen = ModelSpec {
            kv_geometry: Some(geometry(57_344.0, 0.0, 0, 0.0)),
            ..dense(7.62, 32768, vec![q(Quant::Q5_K_M, 5.07)])
        };
        let gemma = ModelSpec {
            projector_gb: Some(0.79),
            kv_geometry: Some(geometry(20_480.0, 118_784.0, 1024, 0.0)),
            ..dense(3.88, 131072, vec![q(Quant::Q4_K_M, 2.32)])
        };
        for (label, spec, ctx, measured_mib) in [
            ("qwen 8k", &qwen, 8192, 5333.0),
            ("qwen 32k", &qwen, 32768, 6071.0),
            ("gemma 8k", &gemma, 8192, 3809.0),
            ("gemma 32k", &gemma, 32768, 4045.0),
        ] {
            let measured = measured_mib / 1024.0;
            let est = footprint_gb(spec, &spec.candidates[0], ctx, KvCache::Q8_0);
            assert!(
                est >= measured,
                "{label}: {est:.2} under the real {measured:.2}"
            );
            assert!(
                est <= measured * 1.15,
                "{label}: {est:.2} over the real {measured:.2} by >15%"
            );
        }
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
    fn multimodal_projector_adds_memory_and_an_unsized_one_is_still_scored() {
        let base = ModelSpec {
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

        // A projector whose size the source didn't report used to make the WHOLE model unscoreable,
        // even though the quant, the context and the weights were all known. PM can't send an image
        // to any model, so refusing to size the rest over that one term was never a fit question.
        let missing = fit(
            &ModelSpec {
                projector_gb: None,
                ..base
            },
            &ram(32.0),
        );
        assert_eq!(missing.verdict, Verdict::Comfortable);
        assert_eq!(missing.quant, Some(Quant::Q4_K_M));
        assert!((missing.est_memory_gb.unwrap() - no_proj).abs() < 0.01);
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
        // An unsized projector no longer sinks the GPU config either: the same Split is offered,
        // one projector lighter, instead of the model being refused a score altogether.
        let missing = ModelSpec {
            projector_gb: None,
            ..base
        };
        let rfm = fit(&missing, &hw);
        assert_eq!(rfm.verdict, Verdict::Comfortable);
        match gpu_fit(&missing, &hw, &rfm) {
            GpuFit::Split { fit } => assert_eq!(fit.quant, Some(Quant::Q4_K_M)),
            other => panic!("expected Split, got {other:?}"),
        }
    }

    #[test]
    fn a_halved_context_still_warns_when_the_headroom_is_thin() {
        // The pair that used to be mutually exclusive. `Verdict` holds one value and `halved` takes
        // it, so a pick that both compromised its context AND landed on the budget floor reported
        // only the compromise. Both notes are facts about the same result; both are now said.
        // KV-dominated on purpose, and one quant only: with a small model the q8_0 cache rescues the
        // full context at rung 0 (which is the ladder working correctly) and it never halves at all.
        let spec = dense(30.0, 8192, vec![q(Quant::Q4_K_M, 2.0)]);
        let budget = footprint_gb(&spec, &spec.candidates[0], 4096, KvCache::F16) + 0.01;
        let r = fit_within(&spec, budget, &ram(64.0));
        assert_eq!(r.verdict, Verdict::HalvedContext);
        assert_eq!(r.context, Some(4096));
        assert!(
            r.notes.iter().any(|n| n.contains("Context reduced")),
            "notes: {:?}",
            r.notes
        );
        assert!(
            r.notes.iter().any(|n| n.contains("little memory headroom")),
            "notes: {:?}",
            r.notes
        );
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

    // --- speed honesty and the resident config (the Local AI tab redesign) ---------------------

    #[test]
    fn every_speed_says_which_bandwidth_it_came_from() {
        // One small model, four machines — one per path through `tokens_per_sec`. The number is
        // the same arithmetic on all four; what differs is how far PM can stand behind it, and the
        // UI words each one differently ("up to", "about", or no figure at all).
        let spec = dense(7.0, 4096, vec![q(Quant::Q4_K_M, 4.3)]);
        let published = FitHardware {
            gpu_bandwidth_gbps: Some(384.0),
            ..gpu(32.0, 24.0)
        };
        let shared = FitHardware {
            unified_memory: true,
            ..gpu(32.0, 24.0)
        };
        for (hw, want) in [
            (published, SpeedBasis::GpuPublished),
            (gpu(32.0, 24.0), SpeedBasis::GpuTypical),
            (shared, SpeedBasis::Shared),
            (ram(32.0), SpeedBasis::System),
            // A card too small for the config runs it from system RAM, at system speed.
            (gpu(32.0, 2.0), SpeedBasis::System),
        ] {
            let r = fit(&spec, &hw);
            assert_eq!(r.speed_basis, Some(want), "{hw:?}");
            assert!(r.est_tokens_per_sec.is_some(), "{hw:?}");
        }

        // The basis never moves the number: shared memory keeps the figure it always had.
        let tps = |hw: &FitHardware| fit(&spec, hw).est_tokens_per_sec.unwrap();
        assert_eq!(tps(&shared), tps(&gpu(32.0, 24.0)));

        // No estimate, no basis.
        let huge = dense(405.0, 8192, vec![q(Quant::IQ2_XS, 146.0)]);
        let cloud = fit(&huge, &ram(16.0));
        assert_eq!(cloud.verdict, Verdict::StayOnCloud);
        assert_eq!(cloud.speed_basis, None);
        assert_eq!(unknown("x".to_string()).speed_basis, None);
    }

    #[test]
    fn a_shared_memory_fit_promises_no_gpu_class_speed() {
        // Shared memory is carved out of the same RAM, so "expect GPU-class speed" would be a speed
        // claim PM cannot make there. The discrete wording is untouched.
        let spec = dense(7.0, 4096, vec![q(Quant::Q4_K_M, 4.3)]);
        let shared = FitHardware {
            unified_memory: true,
            ..gpu(32.0, 24.0)
        };
        let r = fit(&spec, &shared);
        assert!(
            r.notes.iter().all(|n| !n.contains("GPU-class speed")),
            "{:?}",
            r.notes
        );
        assert!(r
            .notes
            .iter()
            .any(|n| n == "Fits the memory this computer's graphics can use."));
        assert!(fit(&spec, &gpu(32.0, 24.0))
            .notes
            .iter()
            .any(|n| n.contains("GPU-class speed")));
    }

    #[test]
    fn system_speed_is_the_pessimistic_ram_figure() {
        // 40 GB/s over the active weight bytes: Qwen2.5 7B at Q8_0 is the figure §3 of the redesign
        // spec quotes as failing the background floor (4.95 against 8.53).
        assert!((system_tokens_per_sec(7.62, Quant::Q8_0) - 40.0 / (7.62 * 1.06)).abs() < EPS);
        assert!((system_tokens_per_sec(7.62, Quant::Q8_0) - 4.95).abs() < 0.01);
        // And it is the same number `fit` reports for a config that runs from RAM.
        let spec = dense(7.62, 4096, vec![q(Quant::Q8_0, 7.54)]);
        let r = fit(&spec, &ram(32.0));
        assert_eq!(r.speed_basis, Some(SpeedBasis::System));
        assert_eq!(
            r.est_tokens_per_sec,
            Some(round1(system_tokens_per_sec(7.62, Quant::Q8_0)))
        );
    }

    #[test]
    fn the_resident_config_is_the_split_rung_wherever_there_is_one() {
        // Swept over the committed catalogue on a grid of cards and free RAM, so the claim in the
        // doc comment — "byte-identical to gpu_fit's rung for every Split, None for NoGpuResident" —
        // is a measurement rather than an argument.
        let mut splits = 0usize;
        let mut band = 0usize;
        for e in &crate::local_catalog::catalog().entries {
            let spec = crate::local_catalog::entry_to_spec(e);
            for vram in [2.0, 4.0, 6.0, 7.96, 8.0, 10.0, 12.0, 16.0, 24.0] {
                for free in [3.0, 6.0, 10.0, 13.4, 20.0, 24.0, 32.0, 48.0, 64.0] {
                    let hw = FitHardware {
                        gpu_bandwidth_gbps: Some(384.0),
                        ..gpu(free, vram)
                    };
                    let rf = fit(&spec, &hw);
                    let resident = resident_fit(&spec, &hw, &rf);
                    match gpu_fit(&spec, &hw, &rf) {
                        GpuFit::Split { fit: g } => {
                            splits += 1;
                            assert_eq!(resident.as_ref(), Some(&g), "{} {vram}/{free}", e.repo);
                        }
                        GpuFit::NoGpuResident => {
                            assert_eq!(resident, None, "{} {vram}/{free}", e.repo);
                        }
                        GpuFit::Single => {}
                    }
                    // Wherever there is one, it keeps the reserve on the card and fits free RAM.
                    if let Some(g) = &resident {
                        let mem = g.est_memory_gb.unwrap();
                        assert!(mem <= vram - gpu_reserve_gb() + 1e-6, "{} {vram}", e.repo);
                        assert!(mem <= ram_budget_gb(&hw) + 1e-6, "{} {free}", e.repo);
                        // The reserve band: `gpu_fit` says Single because the RAM config already
                        // fits raw VRAM, but that config does not keep the reserve.
                        if rf
                            .est_memory_gb
                            .is_some_and(|m| m > vram - gpu_reserve_gb() && m <= vram)
                        {
                            band += 1;
                            assert_ne!(g, &rf, "{} {vram}/{free}", e.repo);
                        }
                    }
                }
            }
        }
        assert!(
            splits > 50,
            "the grid must actually exercise Split ({splits})"
        );
        assert!(band > 0, "the grid must reach the reserve band");
    }

    #[test]
    fn the_resident_config_keeps_the_reserve_in_the_band_gpu_fit_calls_single() {
        // The dev-laptop case from the redesign spec: 7.96 GB card, 10 GB free. The RAM config is
        // Qwen2.5 7B Q6_K at 7.25 GB — under raw VRAM, so `gpu_fit` reports one config, but 0.29 GB
        // short of the reserve. The resident config is the Q5_K_M that keeps it.
        let e = crate::local_catalog::catalog()
            .entries
            .iter()
            .find(|e| e.repo == "bartowski/Qwen2.5-7B-Instruct-GGUF")
            .expect("catalogue entry");
        let spec = crate::local_catalog::entry_to_spec(e);
        let hw = FitHardware {
            gpu_bandwidth_gbps: Some(384.0),
            ..gpu(10.0, 7.96)
        };
        let rf = fit(&spec, &hw);
        assert_eq!(rf.quant, Some(Quant::Q6_K));
        assert_eq!(rf.est_memory_gb, Some(7.25));
        assert_eq!(gpu_fit(&spec, &hw, &rf), GpuFit::Single);

        let g = resident_fit(&spec, &hw, &rf).expect("a config that keeps the reserve");
        assert_eq!(g.quant, Some(Quant::Q5_K_M));
        assert_eq!(g.kv, KvCache::Q8_0);
        assert_eq!(g.context, Some(32768));
        // 6.63 under the parameter-count proxy; 6.50 from its own geometry, which is still +9.6%
        // over the 5.93 GiB this very config was measured holding on that card.
        assert_eq!(g.est_memory_gb, Some(6.5));
        assert_eq!(g.speed_basis, Some(SpeedBasis::GpuPublished));
        // The estimate Bobby's measurement (about 64 real) was taken against.
        assert_eq!(g.est_tokens_per_sec, Some(71.0));

        // The two guards `gpu_fit` opens with, and a RAM verdict that already refused.
        let shared = FitHardware {
            unified_memory: true,
            ..hw
        };
        assert_eq!(resident_fit(&spec, &shared, &fit(&spec, &shared)), None);
        assert_eq!(
            resident_fit(&spec, &ram(10.0), &fit(&spec, &ram(10.0))),
            None
        );
        let cloud = fit(&spec, &gpu(2.0, 7.96));
        assert_eq!(cloud.verdict, Verdict::StayOnCloud);
        assert_eq!(resident_fit(&spec, &gpu(2.0, 7.96), &cloud), None);
    }
}
