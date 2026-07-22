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
//!   * The **KV** term assumes an **f16** cache (2 bytes/element) — the conservative default. The UI
//!     states this out loud rather than presenting a silently-pessimistic number (every result's
//!     `notes` carries the f16 line).
//!
//! No I/O, no DB, no tauri — every function here is a pure projection of its inputs, unit-tested
//! below. The numeric constants are first-pass estimates that need calibration against a real
//! low-RAM DDR4 box (see each `CALIBRATE` note); the *shape* of the decision is what matters here.

use serde::Serialize;

// --- constants (CALIBRATE against the real low-RAM DDR4 rig before trusting the numbers) --------

/// Memory PM + the OS want to keep free so inference doesn't push the machine into swap. Subtracted
/// from `available_ram` to get the usable budget. CALIBRATE: 2 GB is a guess for a low-RAM box.
const PM_RESERVE_GB: f64 = 2.0;

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

/// Dedicated-GPU read bandwidth, used only when the chosen footprint fits in VRAM. CALIBRATE:
/// mid-range discrete GPUs land ~300-500 GB/s; 400 is a deliberately mid, non-flattering pick.
const GPU_BANDWIDTH_GBPS: f64 = 400.0;

/// f16 KV-cache proxy: GB of cache per (billion active params × token). Deliberately a compact
/// heuristic, not a per-architecture derivation — it can't see `n_kv_heads`/`head_dim`, so it
/// trends conservative for wide models. CALIBRATE.
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
    /// Dedicated GPU VRAM in GB, if a reliable figure was read. Only refines the speed estimate —
    /// the fit verdict is scored against RAM, so we never over-promise a fit we can't run.
    pub vram_gb: Option<f64>,
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
#[derive(Debug, Clone, Serialize)]
pub struct FitResult {
    pub verdict: Verdict,
    /// The chosen quant (the best that fits), if any.
    pub quant: Option<Quant>,
    /// The context it fits at (== target, or a halved value ≥ 4096), if any.
    pub context: Option<u32>,
    pub est_memory_gb: Option<f64>,
    pub est_tokens_per_sec: Option<f64>,
    /// Honest, user-facing caveats — always includes the f16-KV line.
    pub notes: Vec<String>,
}

const F16_KV_NOTE: &str = "Estimate assumes an f16 KV cache (the conservative default).";

// --- the pure functions ------------------------------------------------------------------------

/// The system memory PM keeps free when scoring, so inference doesn't push the machine into swap.
/// Surfaced so the UI can state the reserve honestly rather than hide it in a pessimistic number.
pub fn reserve_gb() -> f64 {
    PM_RESERVE_GB
}

/// f16 KV-cache footprint for `ctx` tokens at `active_params_b` billion active params.
pub fn kv_cache_gb(active_params_b: f64, ctx: u32) -> f64 {
    KV_GB_PER_BPARAM_TOKEN * active_params_b * f64::from(ctx)
}

/// Total resident footprint for one (candidate, context) pair: measured weights + f16 KV + a flat
/// overhead + the multimodal projector (0 when there is none).
fn footprint_gb(spec: &ModelSpec, cand: &QuantCandidate, ctx: u32) -> f64 {
    cand.weight_gb
        + kv_cache_gb(spec.active_params_b, ctx)
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
        GPU_BANDWIDTH_GBPS
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

/// Score one model against one machine. Pure.
///
/// Order of degradation (locked decision): keep the full context and step the quant down the ladder
/// first; only halve the context — the more alarming, visible compromise — when no quant fits at
/// full context.
pub fn fit(spec: &ModelSpec, hw: &FitHardware) -> FitResult {
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

    let budget = (hw.available_ram_gb - PM_RESERVE_GB).max(0.0);

    // Best quant first (highest bytes-per-param = highest quality that we might afford).
    let mut candidates = spec.candidates.clone();
    candidates.sort_by(|a, b| {
        b.quant
            .bytes_per_param()
            .partial_cmp(&a.quant.bytes_per_param())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for (rung, &ctx) in context_ladder(spec.target_context).iter().enumerate() {
        for cand in &candidates {
            let mem = footprint_gb(spec, cand, ctx);
            if mem <= budget {
                let halved = rung > 0;
                let headroom = budget - mem;
                let verdict = if halved {
                    Verdict::HalvedContext
                } else if headroom >= COMFORT_MARGIN_GB {
                    Verdict::Comfortable
                } else {
                    Verdict::Tight
                };

                let mut notes = vec![F16_KV_NOTE.to_string()];
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
                    est_memory_gb: Some(round2(mem)),
                    est_tokens_per_sec: tokens_per_sec(spec, cand, mem, hw).map(round1),
                    notes,
                };
            }
        }
    }

    // Nothing fit, even the smallest quant at the floor context.
    FitResult {
        verdict: Verdict::StayOnCloud,
        quant: None,
        context: None,
        est_memory_gb: None,
        est_tokens_per_sec: None,
        notes: vec![
            F16_KV_NOTE.to_string(),
            "Too large for this machine's memory — better run in the cloud.".to_string(),
        ],
    }
}

/// A fit result for a model we deliberately won't score — an unmodelled architecture, a multimodal
/// model with no known projector, or (from the installed scan) a model not in the catalog. The
/// verdict is `Unknown`; `reason` is the single user-facing note.
pub fn unknown(reason: String) -> FitResult {
    FitResult {
        verdict: Verdict::Unknown,
        quant: None,
        context: None,
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
        assert!((kv_cache_gb(7.0, 4096) - 8e-6 * 7.0 * 4096.0).abs() < EPS);
        // Doubling context doubles KV.
        assert!((kv_cache_gb(7.0, 8192) - 2.0 * kv_cache_gb(7.0, 4096)).abs() < EPS);
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
        assert!(r.notes.iter().any(|n| n.contains("f16 KV")));
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
        let mem = footprint_gb(&spec, &q(Quant::Q4_K_M, 4.0), 4096);
        let avail = PM_RESERVE_GB + mem + 0.2; // headroom 0.2 < 1.5
        let r = fit(&spec, &ram(avail));
        assert_eq!(r.verdict, Verdict::Tight);
        assert!(r.notes.iter().any(|n| n.contains("little memory headroom")));
    }

    #[test]
    fn halves_context_when_full_context_does_not_fit() {
        // A big KV: only a halved context brings the footprint under budget.
        // active 40B → kv(8192) huge; kv(4096) half. Weight small so KV dominates.
        let spec = dense(40.0, 8192, vec![q(Quant::Q4_K_M, 1.0)]);
        let full = footprint_gb(&spec, &q(Quant::Q4_K_M, 1.0), 8192);
        let half = footprint_gb(&spec, &q(Quant::Q4_K_M, 1.0), 4096);
        // Budget between half and full.
        let avail = PM_RESERVE_GB + (full + half) / 2.0;
        let r = fit(&spec, &ram(avail));
        assert_eq!(r.verdict, Verdict::HalvedContext);
        assert_eq!(r.context, Some(4096));
        assert!(r.notes.iter().any(|n| n.contains("Context reduced")));
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
        let mem = footprint_gb(&spec, &q(Quant::Q4_K_M, 5.0), 4096); // ~5.5
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
            },
        );
        let gpu = fit(
            &spec,
            &FitHardware {
                available_ram_gb: 32.0,
                vram_gb: Some(24.0),
            },
        );
        assert!(gpu.est_tokens_per_sec.unwrap() > cpu.est_tokens_per_sec.unwrap());
        assert!(gpu.notes.iter().any(|n| n.contains("GPU-class speed")));
    }
}
