// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "A model that fits your machine better is available" (#437) — the pure decision.
//!
//! The Workbench already scores every curated model against this machine. This decides whether any
//! of them is worth *interrupting* the user about, given what they already run. It is deliberately
//! conservative: a nag that fires on a marginal difference trains people to ignore it.
//!
//! The rules, in order:
//!
//! 1. **There must be something to improve on.** With no local model assigned to any role there is
//!    no "better than what you run", so this stays silent — pitching local AI at someone who hasn't
//!    set it up is a different feature, and not this one.
//! 2. **The baseline is the *best* model you already run**, not the first one found. Someone running
//!    a large chat model and a small background one should not be nagged about something that only
//!    beats the small one.
//! 3. **A candidate must be runnable and meaningfully bigger** — judged as the pick judges it, at
//!    the context PM runs it at and where it would run, and [`MIN_IMPROVEMENT`] more parameters.
//!    Comfortable and Tight count alike, as they do for the pick: a notice that also asked for no
//!    less headroom than the model being replaced turned away the very model the pick chose.
//! 4. **A model already on disk wins where the pick's would.** "You already have this downloaded"
//!    is a far better suggestion than "download this", and costs the user nothing — but only for a
//!    copy within [`MIN_IMPROVEMENT`] of the pick's download, the one the pick itself would name.
//! 5. **Only PM's pick.** A candidate the pick below would exclude — one that fits system RAM but
//!    not the graphics card, one too slow for background work, or one too slow for chat on the
//!    card — is never volunteered here
//!    either. The one download the notice will name is the pick's own: the largest eligible model,
//!    and none at all when the user already has a copy of something within [`MIN_IMPROVEMENT`] of
//!    it, which the pick would point at instead. When that download does not qualify, the notice
//!    stays silent rather than naming a smaller one, so the notice and the pick cannot contradict
//!    each other.
//!
//! Flag, never gate: the caller surfaces this passively and the user can always ignore it. Whether
//! it is time to look at all is the *cadence*'s decision ([`crate::local_catalog::rescan_due`]),
//! which this module knows nothing about.
//!
//! ## PM's pick for this computer
//!
//! The second question this module answers, for the top of the Local AI tab: of the curated list
//! and the models the user already has, which ONE would PM run here ([`pick`])? Pure, like the
//! notice, so every rule is pinned below without a machine.
//!
//! The pick writes nothing, never filters or reorders the curated list, never changes a badge, and
//! ranks on parameter count only. Speed is a gate, never a key: on a discrete graphics card a build
//! is eligible only if PM's estimate says it replies at [`chat_floor_tps`] or more. SUPERSEDES
//! DECISIONS 2026-10-02 (#851) 'never on tok/s', by Bobby's decision. The pick itself supersedes
//! DECISIONS 2026-07-24 'Auto that decides for you' for this one item only, also by his decision.
//!
//! The rules, in order:
//!
//! 1. **Where it would run decides how it is judged** ([`basis_for`]). With a discrete graphics card
//!    a model must fit entirely on it, with the reserve PM keeps free there ([`fit::resident_fit`]):
//!    one that spills into system RAM replies many times slower. There it must also be quick enough
//!    for chat: a build PM estimates at [`chat_floor_tps`] or more on the card, compared on the whole
//!    number the UI shows ([`chat_config`], [`quick_enough_for_chat`]). Without one — or on memory
//!    shared with the processor — it must fit free memory at a quant a cautious estimate says is
//!    quick enough for background work ([`system_config`], [`background_floor_tps`]). Either way PM
//!    steps down through the quants to find one, and the speed estimate depends on the quant and the
//!    card only, so more free memory, VRAM or bandwidth can only ever widen the choice.
//! 2. **At the context PM will run it at** ([`pick_context`]), not the one it was trained to: the
//!    32768 PM's own setup steps give a model at most, or less for a model trained on less.
//! 3. **The largest eligible model wins**, ties broken by repo so the answer is stable. Eligible is a
//!    runnable config (Comfortable or Tight at that context, never a halved one) that Ollama can
//!    fetch — judged among the fetchable quants only, so a larger file with no tag never displaces a
//!    smaller one that has one. A larger model that fits the card but is too slow for chat on it is
//!    named beside the pick ([`PassedOver`]), with how fast PM expects it to be.
//! 4. **A model the user already has wins** when it passes the same tests and nothing eligible is
//!    at least [`MIN_IMPROVEMENT`] larger — the same 15% the notice uses.
//! 5. **When nothing qualifies, say why** ([`NoPick`]): nothing fits the card
//!    ([`NoPick::NothingOnGpu`]), nothing that fits it is quick enough for chat
//!    ([`NoPick::TooSlowForChat`]), nothing off it is quick enough for background work
//!    ([`NoPick::TooSlow`]), or nothing fits the memory that is free. Never the least-bad option.

use serde::Serialize;

use crate::local_slot::tunables;
use crate::{fit, local_disk};

/// How much larger a candidate must be before it counts as an improvement rather than noise. 15% is
/// comfortably past the gap between neighbouring sizes of the same family (a 7B vs an 8B is not worth
/// a notice) while still catching a real step up (7B → 14B).
const MIN_IMPROVEMENT: f64 = 1.15;

/// One model this machine could run, as the comparison sees it.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub repo: String,
    pub display_name: String,
    pub parameters_b: f64,
    /// The verdict of the config the pick would run it at ([`Judged::config`]), or `Unknown` when
    /// there is none.
    pub verdict: fit::Verdict,
    /// Already on this machine (#449) as a copy PM's pick could itself choose — one the connected
    /// server can serve, with a runnable config of its own. The strongest kind of suggestion, since
    /// acting on it costs nothing.
    pub on_disk: bool,
    /// What it would occupy, from the same config as `verdict`. `None` when there is no config —
    /// which [`is_runnable`] already excludes, so in practice this is `Some` for anything that
    /// survives to the joint check.
    pub footprint_gb: Option<f64>,
    /// The pick could choose it: [`judge`] found a config that runs where it should and Ollama can
    /// fetch it. `false` keeps it out of [`suggest`] — rule 5 above.
    pub pick_eligible: bool,
}

/// What a suggestion has to share the machine with.
///
/// [`baseline`] picks the LARGER of the two assigned models and drops the other on the floor, so
/// without this a candidate is scored against the whole budget as though it were alone — and PM's own
/// nudge could talk someone into exactly the model-swapping the co-residency warning was added to
/// tell them about. That is the bug this type exists to close, not a readout.
#[derive(Debug, Clone, Copy)]
pub struct Beside {
    /// The footprint of the model on the role `baseline` did not pick.
    pub footprint_gb: f64,
    /// The budget the pair has to fit inside — [`fit::ram_budget_gb`], the one every config the pick
    /// judges has to fit as well, wherever it runs.
    pub budget_gb: f64,
}

/// The suggestion to surface, if there is one worth making.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Suggestion {
    pub repo: String,
    pub display_name: String,
    /// The model it improves on, so the copy can name both.
    pub replaces: String,
    /// It's already on this machine, so the suggestion is "use it", not "download it".
    pub already_downloaded: bool,
}

/// The best model currently assigned to any role, as the baseline to beat. `None` when nothing local
/// is assigned, or when nothing assigned could be matched to the catalog (an unknown model can't be
/// compared against, and guessing would be worse than staying quiet).
pub fn baseline<'a>(assigned: impl IntoIterator<Item = &'a Candidate>) -> Option<&'a Candidate> {
    assigned
        .into_iter()
        .max_by(|a, b| a.parameters_b.total_cmp(&b.parameters_b))
}

/// The model worth suggesting over `current`, if any.
///
/// `candidates` is every scored curated model, judged as the pick judges it; the caller marks the
/// ones already on disk. It names what the pick names (rule 5): the pick's download, or the copy the
/// user already has within [`MIN_IMPROVEMENT`] of it, the larger of those and then by repo so the
/// choice is stable across calls (a suggestion that flickers between two equals is its own kind of
/// noise) — and only when that is worth interrupting for.
pub fn suggest(
    current: Option<&Candidate>,
    candidates: &[Candidate],
    beside: Option<Beside>,
) -> Option<Suggestion> {
    // The baseline need not be one PM would run here: the comparison reads only its size and what it
    // shares the machine with. A model that does not fit the card at the context PM sizes for (Phi-3.5
    // mini's full-width cache on the dev laptop) is the user's strongest reason to hear about one that
    // does, and silencing the notice for it left the pick card offering a 12B with nothing above it.
    let current = current?;
    // Everything the pick could choose. A bigger model that only fits at a halved context is not an
    // upgrade, and `is_runnable` already says so.
    let eligible = || {
        candidates
            .iter()
            .filter(|c| c.pick_eligible && is_runnable(c.verdict))
    };
    let larger = |a: &&Candidate, b: &&Candidate| {
        a.parameters_b
            .total_cmp(&b.parameters_b)
            .then_with(|| b.repo.cmp(&a.repo))
    };
    let worth = |c: &&Candidate| {
        c.repo != current.repo
            && c.parameters_b >= current.parameters_b * MIN_IMPROVEMENT
            && fits_beside(c, beside)
    };
    // What the pick names: the largest eligible model, ties to the lower repo, exactly as [`pick`]
    // orders them — unless the user has a copy within 15% of it, which the pick would point at
    // instead (its rule 4). Only then is a copy named. A copy that is larger than the user's model
    // but well short of the pick's download used to win here, so the notice said "Qwen3.5 9B is
    // already on this device" directly above a pick card offering gemma 4 12b.
    let named = eligible().max_by(larger).map(|top| {
        candidates
            .iter()
            .filter(|c| c.on_disk && c.parameters_b * MIN_IMPROVEMENT > top.parameters_b)
            .max_by(larger)
            .unwrap_or(top)
    });
    named.filter(worth).map(|best| Suggestion {
        repo: best.repo.clone(),
        display_name: best.display_name.clone(),
        replaces: current.display_name.clone(),
        already_downloaded: best.on_disk,
    })
}

/// Whether a candidate still fits once the OTHER role's model is on the machine too.
///
/// `None` disables the check — one role on cloud, or both roles on the same model, so there is no
/// second footprint to make room for. A candidate PM could not size is REJECTED rather than waved
/// through: it cannot be shown to fit, and a suggestion is something PM volunteers unprompted, so
/// silence is the safe default. Strict, with no tolerance band, for the same reason — the band in
/// `fit::co_residency` exists so a WARNING is not over-confident, while here the conservative move is
/// to keep quiet.
fn fits_beside(c: &Candidate, beside: Option<Beside>) -> bool {
    let Some(beside) = beside else {
        return true;
    };
    c.footprint_gb
        .is_some_and(|f| f + beside.footprint_gb <= beside.budget_gb)
}

/// Whether a verdict describes a model this machine can actually run well. `HalvedContext` is
/// deliberately excluded as a *suggestion*: recommending a model that only fits by cutting the
/// context in half is not an improvement to volunteer, even though PM will happily run it if asked.
pub(crate) fn is_runnable(v: fit::Verdict) -> bool {
    matches!(v, fit::Verdict::Comfortable | fit::Verdict::Tight)
}

// --- PM's pick for this computer ---------------------------------------------------------------

/// Where PM's pick would run, which decides how a model is judged ([`judge`]) and how the copy words
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PickBasis {
    /// A discrete graphics card: the model must fit on it.
    Gpu,
    /// Memory shared between the processor and graphics (Apple Silicon, an APU, an iGPU).
    Shared,
    /// No graphics card figure at all: system RAM.
    System,
}

/// The basis for this machine. Unified memory is `Shared` whatever its VRAM figure says, because
/// that figure is a slice of the same RAM — the same reading [`fit::gpu_fit`] makes of it.
pub fn basis_for(hw: &fit::FitHardware) -> PickBasis {
    if hw.unified_memory {
        PickBasis::Shared
    } else if hw.vram_gb.is_some() {
        PickBasis::Gpu
    } else {
        PickBasis::System
    }
}

/// How the judged config relates to the highest-quality config that fits free memory at the same
/// context: it is that config, or a smaller one PM stepped down to for where the pick runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rung {
    Quality,
    /// Smaller, so it stays entirely on the graphics card with the reserve kept.
    Gpu,
    /// Smaller than the build that stays on the card ([`Judged::on_card`]): a build stepped down to
    /// be quick enough for chat on the graphics card ([`chat_floor_tps`]).
    Chat,
    /// Smaller, so it clears [`background_floor_tps`] from shared or system memory.
    Speed,
}

/// Why PM is not picking a model for this computer — each one its own sentence in the UI, because
/// "nothing fits" means three different things and only one of them is about memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoPick {
    /// There is a graphics card, nothing fits on it, and something would run from system RAM.
    NothingOnGpu,
    /// There is a graphics card and something fits on it, but no build of anything that does is
    /// quick enough for chat there ([`chat_floor_tps`]). With today's catalogue, only on a card
    /// slower than about 24 GB/s, which no listed card is.
    TooSlowForChat,
    /// Something fits, but only from system or shared memory, too slowly for background work.
    TooSlow,
    /// Nothing in the list fits the memory that is free.
    TooLittleMemory,
}

/// The slowest decode speed at which PM's largest background reply still finishes inside the
/// background budget, after the cold load that budget already allows for: [`REPLY_RESERVE_TOKENS`]
/// over (180 s − 60 s), about 8.53 tokens a second.
///
/// Decode only — prompt processing is not counted — so it is a cautious screen, not a guarantee: a
/// model that passes it may still be slow on a long prompt, and the copy says "by PM's cautious
/// estimate" for that reason. Compared only against [`fit::system_tokens_per_sec`], the deliberately
/// low system-RAM figure, never against a GPU estimate.
///
/// [`REPLY_RESERVE_TOKENS`]: crate::context_budget::REPLY_RESERVE_TOKENS
pub fn background_floor_tps() -> f64 {
    crate::context_budget::REPLY_RESERVE_TOKENS as f64
        / (tunables::BACKGROUND_TOTAL_TIMEOUT - tunables::COLD_LOAD_ALLOWANCE).as_secs_f64()
}

/// A chat answer of a few paragraphs — about 225 words.
pub const CHAT_REPLY_TOKENS: u32 = 300;

/// How long that answer may take to stream once it starts.
pub const CHAT_REPLY_SECS: f64 = 10.0;

/// The slowest decode speed PM's pick may have on a discrete graphics card: [`CHAT_REPLY_TOKENS`] in
/// [`CHAT_REPLY_SECS`], 30 tokens a second.
///
/// Silent reading runs at about 5 tokens a second, so this streams about six times faster than
/// anyone reads, and 300 tokens in 10 seconds is the usual attention limit. On the dev laptop's card
/// the measured models split cleanly around it: gemma 4 12b at 28.3 falls below; Qwen3.5 9B (42.9),
/// Qwen2.5 7B (45.1) and Llama 3.1 8B (47.0) are above. Of the floors weighed (25, 30, 35 and 40) it
/// is the highest that keeps the pick at 7B or larger on cards of 8 GB and up from 240 GB/s; 35
/// drops the 8 GB RTX 4060 Laptop (256 GB/s) to a 4B. Slower cards do drop: on the 8 GB RTX 3050 and
/// Radeon RX 6600 (224 GB/s) no build of a 7B or larger clears it — Qwen2.5 7B Q4_K_M, the quickest,
/// is 28.7 there — so they get Qwen3.5 4B.
///
/// A gate on PM's estimate ([`fit::gpu_tokens_per_sec`]), never a sort key, and never a measured
/// speed: the pick must be the same before and after the download.
pub fn chat_floor_tps() -> f64 {
    f64::from(CHAT_REPLY_TOKENS) / CHAT_REPLY_SECS
}

/// Whether an estimate clears [`chat_floor_tps`]. Compared on the whole number the UI prints
/// ([`fit::shown_tps`]), so a card can never say "about 30" beside "under 30".
pub fn quick_enough_for_chat(est_tps: f64) -> bool {
    fit::shown_tps(est_tps) >= chat_floor_tps()
}

/// The chat floor as the UI states it: the speed, and the answer it stands for.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ChatSpeed {
    pub floor_tps: f64,
    pub reply_tokens: u32,
    pub reply_secs: f64,
}

/// [`chat_floor_tps`] and what it is made of, for the payload.
pub fn chat_speed() -> ChatSpeed {
    ChatSpeed {
        floor_tps: chat_floor_tps(),
        reply_tokens: CHAT_REPLY_TOKENS,
        reply_secs: CHAT_REPLY_SECS,
    }
}

/// The longest context the pick judges a model at.
///
/// 32768 is the most PM's own setup steps ever give a model — the Ollama step sets
/// `OLLAMA_CONTEXT_LENGTH` to the pick's context, 32768 when there is none (workbenchGuide.ts) —
/// and PM never asks for more per request, so it is the context the pick's config will really run
/// at. Judged at its TRAINED context instead (131072 for Llama 3.x and Phi 3.5, 262144 for Qwen3.5
/// and gemma 4), a long-context model was sized for a window Ollama would never give it, and every
/// one that fits the card at 32k was turned away as a halved context while the pick called a
/// smaller model "the largest that fits". The cards in All models keep judging at the trained
/// context: this caps the pick, and the notice that judges as the pick does, nothing else.
pub const PICK_CONTEXT: u32 = 32_768;

/// The context the pick judges one model at: [`PICK_CONTEXT`], or the model's trained context when
/// that is shorter. A server PROVEN to have loaded it with a longer window raises it to that window,
/// because that is what is really resident, and judging less would flatter it.
pub fn pick_context(trained: u32, served: Option<u32>) -> u32 {
    trained.min(PICK_CONTEXT).max(served.unwrap_or(0))
}

/// One model as the pick judges it.
#[derive(Debug, Clone)]
pub struct Judged {
    /// The config PM would run it at here, or `None` when there is no acceptable one: nothing
    /// runnable fits the card quickly enough for chat on a GPU basis, or nothing runnable clears the
    /// background floor on Shared/System.
    pub config: Option<fit::FitResult>,
    /// Gpu basis only: the best runnable config that fits the card with its reserve, before the chat
    /// floor — the build [`PassedOver`] quotes when the floor turns the model away. `None` off the
    /// card, and when nothing runnable fits it.
    pub on_card: Option<fit::FitResult>,
    /// Which rung `config` is. `Quality` when there is no config.
    pub rung: Rung,
    /// Some config fits free memory and is runnable, at the pick's context — what `NothingOnGpu`
    /// may point at. The caller judges a catalogue model among the quants Ollama can fetch, so for
    /// those this already means a fetchable one.
    pub ram_runnable: bool,
    /// Shared/System only: something is runnable, but no runnable config clears
    /// [`background_floor_tps`].
    pub too_slow: bool,
    /// Gpu basis only: something fits the card ([`Self::on_card`]), but no build that does clears
    /// [`chat_floor_tps`].
    pub too_slow_for_chat: bool,
}

/// Judge one model for the pick at `context` (normally [`pick_context`]). Pure.
///
/// On a GPU basis the config is [`chat_config`]: the best runnable config that fits the card with
/// its reserve among the builds quick enough for chat there, with [`Judged::on_card`] keeping the
/// best one before that floor ([`fit::resident_fit`]). The background floor does not apply there —
/// a config resident on a discrete card is far above it. On a Shared or System basis it is
/// [`system_config`]: the best runnable config among the quants that clear the background floor.
/// That floor applies on Shared too: shared memory is not a faster pool than the RAM it comes from.
/// Both step down through the quants, so neither can turn a model away for want of memory it then
/// gets.
pub fn judge(spec: &fit::ModelSpec, hw: &fit::FitHardware, context: u32) -> Judged {
    let spec = &at_context(spec, context);
    let ram_fit = fit::fit(spec, hw);
    let ram_runnable = is_runnable(ram_fit.verdict);
    let basis = basis_for(hw);
    let (config, on_card) = match basis {
        PickBasis::Gpu => {
            let on_card = fit::resident_fit(spec, hw, &ram_fit).filter(|g| is_runnable(g.verdict));
            let config = on_card
                .as_ref()
                .and_then(|_| chat_config(spec, hw, &ram_fit));
            (config, on_card)
        }
        PickBasis::Shared | PickBasis::System => (system_config(spec, hw, context), None),
    };
    let same = |a: &fit::FitResult, b: &fit::FitResult| {
        (a.quant, a.context, a.kv) == (b.quant, b.context, b.kv)
    };
    let rung = match &config {
        None => Rung::Quality,
        Some(c) if same(c, &ram_fit) => Rung::Quality,
        Some(c) if basis == PickBasis::Gpu => {
            if on_card.as_ref().is_some_and(|o| !same(c, o)) {
                Rung::Chat
            } else {
                Rung::Gpu
            }
        }
        Some(_) => Rung::Speed,
    };
    let too_slow = basis != PickBasis::Gpu && ram_runnable && config.is_none();
    let too_slow_for_chat = on_card.is_some() && config.is_none();
    Judged {
        config,
        on_card,
        rung,
        ram_runnable,
        too_slow,
        too_slow_for_chat,
    }
}

/// The best config that fits the card with its reserve among the builds PM estimates quick enough
/// for chat there ([`quick_enough_for_chat`] of [`fit::gpu_tokens_per_sec`]), kept when runnable.
///
/// Built from the quick builds alone, the way [`system_config`] is off the card, so it steps down
/// to a smaller build of the same model before turning the model away. Stepping down a quant is not
/// always quicker under the estimate — gemma 4 12b is 29.4 at Q3_K_M and 30.2 at Q4_K_M on the dev
/// laptop — so this takes the best build that clears the floor, wherever it sits. The estimate
/// depends on the quant and the card only, never on memory, so more free memory, VRAM or bandwidth
/// can only widen what is left.
fn chat_config(
    spec: &fit::ModelSpec,
    hw: &fit::FitHardware,
    ram_fit: &fit::FitResult,
) -> Option<fit::FitResult> {
    let quick = fit::ModelSpec {
        candidates: spec
            .candidates
            .iter()
            .copied()
            .filter(|c| fit::gpu_tokens_per_sec(spec, c, hw).is_some_and(quick_enough_for_chat))
            .collect(),
        ..spec.clone()
    };
    // No quick build at all leaves no candidates, which the fit answers `Unknown`: no config.
    fit::resident_fit(&quick, hw, ram_fit).filter(|g| is_runnable(g.verdict))
}

/// The best config that runs from shared or system memory quickly enough for background work, at
/// `context`: the RAM fit built only from the quants whose [`fit::system_tokens_per_sec`] clears
/// [`background_floor_tps`], kept when runnable.
///
/// It steps down through the quants the way the GPU basis steps down to stay on the card. Checking
/// the floor only at the highest-quality quant that fits turned a larger model away whenever that
/// one quant was too slow, even with a smaller quant of it quick enough — so freeing memory could
/// SHRINK the pick: 8 GB free picked Qwen2.5 7B at Q4_K_M, and 9 GB free, where its best quant became
/// a too-slow Q5_K_M, picked a 3.9B. Built from the quick quants alone, the answer only grows as free
/// memory does.
pub fn system_config(
    spec: &fit::ModelSpec,
    hw: &fit::FitHardware,
    context: u32,
) -> Option<fit::FitResult> {
    let floor = background_floor_tps();
    let quick = fit::ModelSpec {
        candidates: spec
            .candidates
            .iter()
            .copied()
            .filter(|c| fit::system_tokens_per_sec(spec, c) >= floor)
            .collect(),
        ..at_context(spec, context)
    };
    // No quick quant at all leaves no candidates, which `fit` answers `Unknown`: not runnable.
    Some(fit::fit(&quick, hw)).filter(|f| is_runnable(f.verdict))
}

fn at_context(spec: &fit::ModelSpec, context: u32) -> fit::ModelSpec {
    fit::ModelSpec {
        target_context: context,
        ..spec.clone()
    }
}

/// One curated model, as the pick weighs it.
#[derive(Debug, Clone)]
pub struct CatalogueOption {
    pub repo: String,
    pub display_name: String,
    pub parameters_b: f64,
    pub judged: Judged,
    /// The Ollama tag for the judged config's quant, or `None` when there is no config or that quant
    /// cannot be fetched.
    pub tag: Option<String>,
    /// The download for the judged config's quant — weights plus any projector, which Ollama pulls
    /// with them — read from the same catalogue row as `tag`. `None` only when there is no config.
    pub download_gb: Option<f64>,
    /// It would be fine from system memory: [`system_config`] finds a runnable, fetchable config over
    /// the floor. What `NothingOnGpu`'s `system_fallback` reports.
    pub system_ok: bool,
}

/// One model the user already has — served by their endpoint, or found on disk.
#[derive(Debug, Clone)]
pub struct OwnedOption {
    /// The runner's own name for it: the served id, or the on-disk name.
    pub id: String,
    pub repo: String,
    pub display_name: String,
    pub parameters_b: f64,
    /// The endpoint serves it, as opposed to it only being on disk.
    pub served: bool,
    pub source: Option<local_disk::DiskSource>,
    pub path: Option<String>,
    pub shards: u32,
    /// The figures describe the user's own file (its real size and quant), not the catalogue's
    /// guess at it.
    pub measured: bool,
    /// The judged config, as [`Judged::config`]. No rung beside it: the owned pick shows the one
    /// config, and has no card whose rungs it would point between.
    pub config: Option<fit::FitResult>,
    /// As [`Judged::on_card`], from the same judgement as `config`: what fits the card before the
    /// chat floor, so a copy too slow for chat can be named rather than silently skipped
    /// ([`PassedOver::have`]).
    pub on_card: Option<fit::FitResult>,
    /// A role is already set to it.
    pub bound: bool,
}

impl OwnedOption {
    /// It fits the card ([`Self::on_card`]), but no build of it is quick enough for chat there: not
    /// a copy the pick can use, and one the card has to explain ([`PassedOver::have`]).
    pub fn too_slow_for_chat(&self) -> bool {
        self.config.is_none() && self.on_card.is_some()
    }
}

/// The owned model a catalogue pick is weighed against, so the copy can name it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnedRef {
    pub id: String,
    pub display_name: String,
    pub served: bool,
}

/// A larger model PM passed over because it is too slow for chat on this graphics card, so the pick
/// card can say which one and how fast PM expects it to be. Gpu basis only.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PassedOver {
    pub repo: String,
    pub display_name: String,
    /// Its best build on the card ([`Judged::on_card`]), the one PM estimates under the floor.
    pub quant: fit::Quant,
    /// That build's estimate, one decimal: its [`fit::FitResult::est_tokens_per_sec`].
    pub est_tokens_per_sec: f64,
    /// The user's own copy of it, when they have one — then `quant` and the estimate are that
    /// copy's. `None` when PM's list is the only source.
    pub have: Option<OwnedRef>,
}

/// PM's pick for this computer. The serialized shape is a contract with the TypeScript mirror: the
/// variant is named in a `kind` field (`catalogue` | `owned` | `nothing`) beside its own fields.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Pick {
    /// A model from the curated list, to download.
    Catalogue {
        repo: String,
        display_name: String,
        rung: Rung,
        tag: String,
        fit: fit::FitResult,
        download_gb: f64,
        basis: PickBasis,
        /// The best model the user already has that fits this machine the same way, which this one
        /// is at least 15% larger than — so the copy can say why PM still points at the download.
        also_have: Option<OwnedRef>,
        /// The largest model larger than this one that fits the card but not the chat floor.
        passed_over: Option<PassedOver>,
    },
    /// A model the user already has.
    Owned {
        id: String,
        repo: String,
        display_name: String,
        served: bool,
        source: Option<local_disk::DiskSource>,
        path: Option<String>,
        shards: u32,
        measured: bool,
        fit: fit::FitResult,
        basis: PickBasis,
        /// The largest model larger than this one that fits the card but not the chat floor.
        passed_over: Option<PassedOver>,
    },
    /// Nothing suits this computer, and why.
    Nothing {
        reason: NoPick,
        basis: PickBasis,
        /// `NothingOnGpu` only: some model would be fine from system memory, so the copy can say
        /// "some would run, several times slower" rather than "none would".
        system_fallback: bool,
    },
}

/// PM's pick. Pure: the caller has already judged every option.
///
/// Ranks on parameter count only; speed is a gate in [`judge`], never a key here — ranking by GPU
/// throughput would pick the smallest model in the list every time.
pub fn pick(basis: PickBasis, catalogue: &[CatalogueOption], owned: &[OwnedOption]) -> Pick {
    // C*: the largest eligible curated model, ties to the lower repo so the answer never depends on
    // input order.
    let best = catalogue
        .iter()
        .filter_map(|o| match (&o.judged.config, &o.tag, o.download_gb) {
            (Some(config), Some(tag), Some(download_gb)) if is_runnable(config.verdict) => {
                Some((o, config, tag, download_gb))
            }
            _ => None,
        })
        .max_by(|a, b| {
            a.0.parameters_b
                .total_cmp(&b.0.parameters_b)
                .then_with(|| b.0.repo.cmp(&a.0.repo))
        });

    let mut runnable: Vec<(&OwnedOption, &fit::FitResult)> = owned
        .iter()
        .filter_map(|o| {
            o.config
                .as_ref()
                .filter(|c| is_runnable(c.verdict))
                .map(|c| (o, c))
        })
        .collect();
    runnable.sort_by(|(a, _), (b, _)| owned_order(a, b));

    // An owned model wins unless something eligible is at least 15% larger than it.
    let qualifies = |o: &OwnedOption| {
        best.is_none_or(|(c, ..)| o.parameters_b * MIN_IMPROVEMENT > c.parameters_b)
    };
    if let Some((o, config)) = runnable.iter().find(|(o, _)| qualifies(o)) {
        return Pick::Owned {
            id: o.id.clone(),
            repo: o.repo.clone(),
            display_name: o.display_name.clone(),
            served: o.served,
            source: o.source,
            path: o.path.clone(),
            shards: o.shards,
            measured: o.measured,
            fit: (*config).clone(),
            basis,
            passed_over: passed_over(basis, catalogue, owned, o.parameters_b),
        };
    }

    if let Some((c, config, tag, download_gb)) = best {
        // Every runnable owned model failed only the 15% test, so the first of them is the one to
        // name beside the pick.
        let also_have = runnable.first().map(|(o, _)| OwnedRef {
            id: o.id.clone(),
            display_name: o.display_name.clone(),
            served: o.served,
        });
        return Pick::Catalogue {
            repo: c.repo.clone(),
            display_name: c.display_name.clone(),
            rung: c.judged.rung,
            tag: tag.clone(),
            fit: config.clone(),
            download_gb,
            basis,
            also_have,
            passed_over: passed_over(basis, catalogue, owned, c.parameters_b),
        };
    }

    let reason = match basis {
        PickBasis::Gpu if catalogue.iter().any(|o| o.judged.too_slow_for_chat) => {
            NoPick::TooSlowForChat
        }
        PickBasis::Gpu if catalogue.iter().any(|o| o.judged.ram_runnable) => NoPick::NothingOnGpu,
        PickBasis::Shared | PickBasis::System if catalogue.iter().any(|o| o.judged.too_slow) => {
            NoPick::TooSlow
        }
        _ => NoPick::TooLittleMemory,
    };
    Pick::Nothing {
        reason,
        basis,
        system_fallback: reason == NoPick::NothingOnGpu && catalogue.iter().any(|o| o.system_ok),
    }
}

/// The order the pick weighs the models the user has in: one a role already uses first, then one
/// the server already has, then the larger, then by id so the order is total.
fn owned_order(a: &OwnedOption, b: &OwnedOption) -> std::cmp::Ordering {
    b.bound
        .cmp(&a.bound)
        .then(b.served.cmp(&a.served))
        .then(b.parameters_b.total_cmp(&a.parameters_b))
        .then_with(|| a.id.cmp(&b.id))
}

/// The model to name as passed over for speed beside a pick of `above` billion parameters: the
/// largest model strictly larger than it, ties to the lower repo, that fits the card but is too slow
/// for chat on it — from PM's list, or a copy the user has. Gpu basis only: off the card the chat
/// floor never applies, so nothing there is passed over for it.
///
/// When the user has a copy of that model too slow for chat, the first of them in the pick's own
/// order ([`owned_order`]) is `have`, and its build on the card is the one quoted; otherwise the
/// list's.
///
/// One model only, and only a larger one, so a copy too slow for chat that is not this model — a
/// heavier build of the pick's own model, most often, or a larger model smaller than this one — is
/// never named here. A served copy's row says so instead (`local_ai::InstalledModel`'s
/// `under_chat_floor_tps`), which is what the card needs when that copy is the one a job runs on.
fn passed_over(
    basis: PickBasis,
    catalogue: &[CatalogueOption],
    owned: &[OwnedOption],
    above: f64,
) -> Option<PassedOver> {
    if basis != PickBasis::Gpu {
        return None;
    }
    let too_slow = |o: &&OwnedOption| o.too_slow_for_chat();
    let (repo, display_name, _) = catalogue
        .iter()
        .filter(|o| o.judged.too_slow_for_chat)
        .map(|o| (&o.repo, &o.display_name, o.parameters_b))
        .chain(
            owned
                .iter()
                .filter(too_slow)
                .map(|o| (&o.repo, &o.display_name, o.parameters_b)),
        )
        .filter(|(.., params)| *params > above)
        .max_by(|a, b| a.2.total_cmp(&b.2).then_with(|| b.0.cmp(a.0)))?;
    let mut copies: Vec<&OwnedOption> = owned
        .iter()
        .filter(too_slow)
        .filter(|o| &o.repo == repo)
        .collect();
    copies.sort_by(|a, b| owned_order(a, b));
    let (on_card, have) = match copies.first() {
        Some(o) => (
            o.on_card.as_ref()?,
            Some(OwnedRef {
                id: o.id.clone(),
                display_name: o.display_name.clone(),
                served: o.served,
            }),
        ),
        None => (
            catalogue
                .iter()
                .find(|o| &o.repo == repo && o.judged.too_slow_for_chat)?
                .judged
                .on_card
                .as_ref()?,
            None,
        ),
    };
    Some(PassedOver {
        repo: repo.clone(),
        display_name: display_name.clone(),
        quant: on_card.quant?,
        est_tokens_per_sec: on_card.est_tokens_per_sec?,
        have,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(repo: &str, params: f64, verdict: fit::Verdict, on_disk: bool) -> Candidate {
        Candidate {
            repo: repo.to_string(),
            display_name: repo.to_string(),
            parameters_b: params,
            verdict,
            on_disk,
            // Roughly a byte per param at Q8, which is only ever compared against a budget the test
            // chooses — the joint check is what it exists for, and every other test disables that
            // check by passing `None`.
            footprint_gb: Some(params),
            pick_eligible: true,
        }
    }

    #[test]
    fn nothing_is_suggested_without_a_model_to_improve_on() {
        let pool = vec![cand("big", 70.0, fit::Verdict::Comfortable, false)];
        // Pitching local AI at someone who hasn't set it up is a different feature.
        assert_eq!(suggest(None, &pool, None), None);
    }

    #[test]
    fn a_meaningfully_bigger_model_that_still_fits_is_suggested() {
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let pool = vec![
            cand("small", 7.0, fit::Verdict::Comfortable, false),
            cand("mid", 14.0, fit::Verdict::Comfortable, false),
        ];
        let s = suggest(Some(&current), &pool, None).unwrap();
        assert_eq!(s.repo, "mid");
        assert_eq!(s.replaces, "small");
        assert!(!s.already_downloaded);
    }

    #[test]
    fn a_marginal_size_difference_is_not_worth_a_notice() {
        // 7B → 8B is within the noise of one family's sizes; nagging about it trains people to
        // ignore the notice entirely.
        let current = cand("seven", 7.0, fit::Verdict::Comfortable, false);
        let pool = vec![cand("eight", 8.0, fit::Verdict::Comfortable, false)];
        assert_eq!(suggest(Some(&current), &pool, None), None);
        // 7B → 14B is a real step up.
        let pool = vec![cand("fourteen", 14.0, fit::Verdict::Comfortable, false)];
        assert!(suggest(Some(&current), &pool, None).is_some());
    }

    #[test]
    fn a_bigger_model_that_only_fits_at_a_halved_context_is_not_an_upgrade() {
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        // Bigger, but only at a halved context, or not at all — neither is worth volunteering.
        let pool = vec![
            cand("halved", 32.0, fit::Verdict::HalvedContext, false),
            cand("cloud", 70.0, fit::Verdict::StayOnCloud, false),
            cand("unknown", 70.0, fit::Verdict::Unknown, false),
        ];
        assert_eq!(suggest(Some(&current), &pool, None), None);

        // A tight fit is a fit, whatever headroom the current model has: the pick ranks the two
        // alike. Asking for no less headroom than the model being replaced turned away the pick
        // itself — a Tight gemma 4 26B on a machine where a 2B ran comfortably — and the notice
        // named a smaller model directly above the pick card.
        let pool = vec![cand("bigger", 14.0, fit::Verdict::Tight, false)];
        assert_eq!(suggest(Some(&current), &pool, None).unwrap().repo, "bigger");
        let tight_current = cand("small", 7.0, fit::Verdict::Tight, false);
        assert!(suggest(Some(&tight_current), &pool, None).is_some());
    }

    #[test]
    fn the_only_download_the_notice_names_is_the_picks_own() {
        // The pick names the largest eligible model. When that one does not fit beside the other
        // role's model, naming the next one down would put a second download above the pick card,
        // so the notice keeps quiet instead.
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let pool = vec![
            cand("mid", 14.0, fit::Verdict::Comfortable, false),
            cand("big", 32.0, fit::Verdict::Tight, false),
        ];
        assert_eq!(suggest(Some(&current), &pool, None).unwrap().repo, "big");
        let tight = Beside {
            footprint_gb: 6.0,
            budget_gb: 30.0,
        };
        assert_eq!(suggest(Some(&current), &pool, Some(tight)), None);
        let roomy = Beside {
            footprint_gb: 6.0,
            budget_gb: 40.0,
        };
        assert_eq!(
            suggest(Some(&current), &pool, Some(roomy)).unwrap().repo,
            "big"
        );
    }

    #[test]
    fn a_copy_within_fifteen_percent_of_the_download_is_what_the_pick_would_use() {
        // The user has an 11B, too close to the 10B they run to be worth a notice, and within 15%
        // of the 12B download — so PM's pick is the 11B they have, not the download. Naming the
        // download here would contradict the pick card.
        let current = cand("ten", 10.0, fit::Verdict::Comfortable, false);
        let download = cand("twelve", 12.0, fit::Verdict::Comfortable, false);
        let have = cand("eleven", 11.0, fit::Verdict::Comfortable, true);
        assert_eq!(
            suggest(Some(&current), &[download.clone(), have], None),
            None
        );
        let p = pick(
            PickBasis::Gpu,
            &[option("twelve", 12.0, true)],
            &[owned("eleven", 11.0, true, false)],
        );
        assert!(matches!(p, Pick::Owned { .. }), "{p:?}");

        // Without that copy, the download is the pick and the notice names it.
        assert_eq!(
            suggest(Some(&current), &[download], None).unwrap().repo,
            "twelve"
        );
    }

    #[test]
    fn a_copy_on_disk_wins_only_where_the_pick_would_name_it() {
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        // Within 15% of the download, the copy is the pick, and it costs nothing to act on.
        let pool = vec![
            cand("downloaded", 14.5, fit::Verdict::Comfortable, true),
            cand("download", 16.0, fit::Verdict::Comfortable, false),
        ];
        let s = suggest(Some(&current), &pool, None).unwrap();
        assert_eq!(s.repo, "downloaded");
        assert!(s.already_downloaded, "costs the user nothing to act on");
        let p = pick(
            PickBasis::Gpu,
            &[option("download", 16.0, true)],
            &[owned("downloaded", 14.5, true, false)],
        );
        assert!(matches!(p, Pick::Owned { .. }), "{p:?}");

        // Well short of it, the pick is the download, so the notice names the download too. Naming
        // the copy put "already on this device" above a pick card offering a bigger model.
        let pool = vec![
            cand("downloaded", 14.0, fit::Verdict::Comfortable, true),
            cand("download", 32.0, fit::Verdict::Comfortable, false),
        ];
        let s = suggest(Some(&current), &pool, None).unwrap();
        assert_eq!(s.repo, "download");
        assert!(!s.already_downloaded);
        let p = pick(
            PickBasis::Gpu,
            &[option("download", 32.0, true)],
            &[owned("downloaded", 14.0, true, false)],
        );
        assert!(matches!(p, Pick::Catalogue { .. }), "{p:?}");
    }

    #[test]
    fn the_current_model_is_never_suggested_back_to_itself() {
        let current = cand("same", 14.0, fit::Verdict::Comfortable, false);
        let pool = vec![cand("same", 14.0, fit::Verdict::Comfortable, true)];
        assert_eq!(suggest(Some(&current), &pool, None), None);
    }

    #[test]
    fn a_model_pm_would_not_run_here_still_hears_about_the_pick() {
        // Phi-3.5 mini's full-width cache does not fit the dev laptop's card at 32k, so it has no
        // config the pick would run. The comparison needs only its size, and a model that does fit
        // is the news its user most needs: staying silent left the pick card alone offering a 12B.
        let current = cand("phi", 3.82, fit::Verdict::Unknown, false);
        let pool = vec![cand("big", 11.91, fit::Verdict::Tight, false)];
        assert_eq!(suggest(Some(&current), &pool, None).unwrap().repo, "big");
    }

    #[test]
    fn the_baseline_is_the_best_model_already_running() {
        let chat = cand("chat", 14.0, fit::Verdict::Comfortable, false);
        let background = cand("background", 3.0, fit::Verdict::Comfortable, false);
        let base = baseline([&chat, &background]).unwrap();
        assert_eq!(base.repo, "chat", "the largest assigned model sets the bar");

        // So a model that only beats the small background one is not suggested.
        let pool = vec![cand("mid", 8.0, fit::Verdict::Comfortable, false)];
        assert_eq!(suggest(Some(base), &pool, None), None);

        assert!(baseline(std::iter::empty()).is_none());
    }

    #[test]
    fn a_model_that_fits_alone_but_not_beside_the_other_role_is_not_suggested() {
        // The live half of #786 item 6. `baseline` picks the LARGER of the two assigned models and
        // drops the other, so a suggestion was scored against the whole budget as though the machine
        // held nothing else — letting PM talk someone into exactly the model-swapping the warning
        // beside it was added to describe.
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let pool = vec![
            current.clone(),
            cand("mid", 14.0, fit::Verdict::Comfortable, false),
        ];

        // Alone, the 14 is a clear upgrade and is offered.
        assert!(suggest(Some(&current), &pool, None).is_some());

        // Beside a 6 GB model in a 16 GB budget it no longer fits (14 + 6 > 16), so it must not be.
        let beside = Beside {
            footprint_gb: 6.0,
            budget_gb: 16.0,
        };
        assert_eq!(suggest(Some(&current), &pool, Some(beside)), None);

        // Give the pair room and the same suggestion comes back — the filter has to be about the
        // arithmetic, not about the presence of a second model.
        let roomy = Beside {
            footprint_gb: 6.0,
            budget_gb: 32.0,
        };
        assert_eq!(
            suggest(Some(&current), &pool, Some(roomy)).unwrap().repo,
            "mid"
        );
    }

    #[test]
    fn a_candidate_with_no_footprint_is_kept_quiet_rather_than_waved_through() {
        // A suggestion is volunteered, not asked for, so "cannot be shown to fit" must resolve to
        // silence. Waving it through would make the joint check decorative.
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let mut unsized_big = cand("mid", 14.0, fit::Verdict::Comfortable, false);
        unsized_big.footprint_gb = None;
        let pool = vec![current.clone(), unsized_big];

        assert!(
            suggest(Some(&current), &pool, None).is_some(),
            "no check, no rejection"
        );
        let beside = Beside {
            footprint_gb: 1.0,
            budget_gb: 999.0,
        };
        assert_eq!(suggest(Some(&current), &pool, Some(beside)), None);
    }

    #[test]
    fn the_choice_is_stable_across_calls() {
        // Two equally good candidates must always resolve the same way — a suggestion that flickers
        // between them on every refresh is its own kind of noise.
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let pool = vec![
            cand("alpha", 14.0, fit::Verdict::Comfortable, false),
            cand("beta", 14.0, fit::Verdict::Comfortable, false),
        ];
        let first = suggest(Some(&current), &pool, None).unwrap();
        let reversed: Vec<Candidate> = pool.into_iter().rev().collect();
        assert_eq!(suggest(Some(&current), &reversed, None).unwrap(), first);
    }

    #[test]
    fn the_notice_never_suggests_a_model_the_pick_would_exclude() {
        // Fits system RAM comfortably and is twice the size — but not on the graphics card, so the
        // pick would never choose it. The notice used to; that is the contradiction rule 5 closes.
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let mut spills = cand("spills-to-ram", 14.0, fit::Verdict::Comfortable, false);
        spills.pick_eligible = false;
        assert_eq!(suggest(Some(&current), &[spills.clone()], None), None);
        spills.pick_eligible = true;
        assert!(suggest(Some(&current), &[spills], None).is_some());
    }

    // ---- PM's pick ----

    fn config(verdict: fit::Verdict) -> fit::FitResult {
        fit::FitResult {
            verdict,
            quant: Some(fit::Quant::Q4_K_M),
            context: Some(32768),
            kv: fit::KvCache::F16,
            est_memory_gb: Some(5.0),
            est_tokens_per_sec: Some(50.0),
            speed_basis: Some(fit::SpeedBasis::GpuPublished),
            notes: vec![],
        }
    }

    /// A curated option. `eligible` decides whether it has a runnable, fetchable config.
    fn option(repo: &str, params: f64, eligible: bool) -> CatalogueOption {
        let config = eligible.then(|| config(fit::Verdict::Tight));
        CatalogueOption {
            repo: repo.to_string(),
            display_name: repo.to_string(),
            parameters_b: params,
            judged: Judged {
                config: config.clone(),
                on_card: config,
                rung: Rung::Gpu,
                ram_runnable: true,
                too_slow: false,
                too_slow_for_chat: false,
            },
            tag: eligible.then(|| format!("hf.co/{repo}:Q4_K_M")),
            download_gb: eligible.then_some(params * 0.6),
            system_ok: false,
        }
    }

    fn owned(id: &str, params: f64, served: bool, bound: bool) -> OwnedOption {
        OwnedOption {
            id: id.to_string(),
            repo: format!("example/{id}"),
            display_name: id.to_string(),
            parameters_b: params,
            served,
            source: (!served).then_some(local_disk::DiskSource::Ollama),
            path: None,
            shards: 1,
            measured: true,
            config: Some(config(fit::Verdict::Comfortable)),
            on_card: Some(config(fit::Verdict::Comfortable)),
            bound,
        }
    }

    fn repo_of(p: &Pick) -> &str {
        match p {
            Pick::Catalogue { repo, .. } | Pick::Owned { repo, .. } => repo,
            Pick::Nothing { .. } => "",
        }
    }

    #[test]
    fn the_largest_eligible_model_is_the_pick_whatever_its_speed() {
        // The faster small model loses on size, which is the only key: `pick` itself never sorts on
        // speed, and the chat floor lives in `judge`, before it. Ranking by GPU throughput would
        // pick the smallest model in the list every time.
        let mut fast = option("tiny", 0.41, true);
        fast.judged.config.as_mut().unwrap().est_tokens_per_sec = Some(884.0);
        let pool = vec![
            fast,
            option("seven", 7.62, true),
            // Larger, but with no config that fits where it should.
            option("fourteen", 14.77, false),
        ];
        let p = pick(PickBasis::Gpu, &pool, &[]);
        assert!(matches!(p, Pick::Catalogue { .. }), "{p:?}");
        assert_eq!(repo_of(&p), "seven");

        // An eligible config with no Ollama tag is not eligible: the pick names a download.
        let mut untagged = option("big", 72.0, true);
        untagged.tag = None;
        let p = pick(
            PickBasis::Gpu,
            &[untagged, option("seven", 7.62, true)],
            &[],
        );
        assert_eq!(repo_of(&p), "seven");
    }

    #[test]
    fn a_tie_resolves_by_repo_whatever_the_input_order() {
        let pool = vec![option("beta", 7.0, true), option("alpha", 7.0, true)];
        let first = pick(PickBasis::Gpu, &pool, &[]);
        assert_eq!(repo_of(&first), "alpha");
        let reversed: Vec<CatalogueOption> = pool.into_iter().rev().collect();
        assert_eq!(pick(PickBasis::Gpu, &reversed, &[]), first);
    }

    #[test]
    fn a_model_you_already_have_wins_unless_the_pick_is_fifteen_percent_larger() {
        let pool = vec![option("qwen-7b", 7.62, true)];

        // The same size: PM points at the one you have.
        let p = pick(
            PickBasis::Gpu,
            &pool,
            &[owned("qwen2.5:7b", 7.62, true, false)],
        );
        match &p {
            Pick::Owned {
                id, served, basis, ..
            } => {
                assert_eq!(id, "qwen2.5:7b");
                assert!(served);
                assert_eq!(*basis, PickBasis::Gpu);
            }
            other => panic!("expected Owned, got {other:?}"),
        }

        // 3.88 × 1.15 < 7.62: the pick stands, and names the model you have beside it.
        let p = pick(
            PickBasis::Gpu,
            &pool,
            &[owned("gemma3:4b", 3.88, true, false)],
        );
        match &p {
            Pick::Catalogue {
                repo, also_have, ..
            } => {
                assert_eq!(repo, "qwen-7b");
                assert_eq!(
                    also_have.as_ref(),
                    Some(&OwnedRef {
                        id: "gemma3:4b".into(),
                        display_name: "gemma3:4b".into(),
                        served: true,
                    })
                );
            }
            other => panic!("expected Catalogue, got {other:?}"),
        }

        // An owned model with no acceptable config is neither picked nor named.
        let mut halved = owned("qwen2.5:7b", 7.62, true, false);
        halved.config = Some(config(fit::Verdict::HalvedContext));
        match pick(PickBasis::Gpu, &pool, &[halved]) {
            Pick::Catalogue { also_have, .. } => assert_eq!(also_have, None),
            other => panic!("expected Catalogue, got {other:?}"),
        }

        // With nothing eligible in the list, any runnable owned model qualifies.
        let p = pick(
            PickBasis::Gpu,
            &[option("big", 14.0, false)],
            &[owned("gemma3:4b", 3.88, true, false)],
        );
        assert!(matches!(p, Pick::Owned { .. }), "{p:?}");
    }

    #[test]
    fn among_owned_models_the_one_in_use_comes_first() {
        let pool = vec![option("qwen-7b", 7.62, true)];
        let have = vec![
            owned("larger-served", 8.0, true, false),
            owned("on-disk", 9.0, false, false),
            owned("in-use", 7.62, true, true),
        ];
        let id = |p: Pick| match p {
            Pick::Owned { id, .. } => id,
            other => panic!("expected Owned, got {other:?}"),
        };
        assert_eq!(id(pick(PickBasis::Gpu, &pool, &have)), "in-use");
        // Then served before on disk, even when the disk copy is larger.
        assert_eq!(id(pick(PickBasis::Gpu, &pool, &have[..2])), "larger-served");
        // Then the larger, then by id.
        let disk_only = vec![
            owned("b-disk", 8.0, false, false),
            owned("a-disk", 8.0, false, false),
            owned("small-disk", 7.7, false, false),
        ];
        assert_eq!(id(pick(PickBasis::Gpu, &pool, &disk_only)), "a-disk");
    }

    #[test]
    fn when_nothing_suits_the_pick_says_why() {
        // A graphics card, nothing on it, and something that would run from system RAM.
        let mut spills = option("spills", 7.62, false);
        spills.judged.ram_runnable = true;
        let p = pick(PickBasis::Gpu, &[spills.clone()], &[]);
        assert_eq!(
            p,
            Pick::Nothing {
                reason: NoPick::NothingOnGpu,
                basis: PickBasis::Gpu,
                system_fallback: false,
            }
        );
        // ... and one of those would be fine from system memory.
        spills.system_ok = true;
        assert_eq!(
            pick(PickBasis::Gpu, &[spills], &[]),
            Pick::Nothing {
                reason: NoPick::NothingOnGpu,
                basis: PickBasis::Gpu,
                system_fallback: true,
            }
        );

        // No card: something fits, but too slowly.
        let mut slow = option("slow", 72.0, false);
        slow.judged.too_slow = true;
        for basis in [PickBasis::System, PickBasis::Shared] {
            assert_eq!(
                pick(basis, &[slow.clone()], &[]),
                Pick::Nothing {
                    reason: NoPick::TooSlow,
                    basis,
                    system_fallback: false,
                }
            );
        }

        // Nothing fits at all.
        let mut none = option("none", 0.41, false);
        none.judged.ram_runnable = false;
        for basis in [PickBasis::Gpu, PickBasis::System, PickBasis::Shared] {
            assert_eq!(
                pick(basis, &[none.clone()], &[]),
                Pick::Nothing {
                    reason: NoPick::TooLittleMemory,
                    basis,
                    system_fallback: false,
                }
            );
        }
    }

    #[test]
    fn the_background_floor_is_the_largest_reply_over_what_the_budget_leaves() {
        // 1024 tokens in (180 s − 60 s cold load).
        assert_eq!(background_floor_tps(), 1024.0 / 120.0);
    }

    #[test]
    fn the_speed_floor_applies_off_the_graphics_card_only() {
        // Qwen2.5 7B at Q8_0: 4.95 tok/s from system RAM, under the 8.53 floor.
        let spec = fit::ModelSpec {
            arch: fit::Architecture::Dense,
            active_params_b: 7.62,
            target_context: 32768,
            projector_gb: None,
            candidates: vec![fit::QuantCandidate {
                quant: fit::Quant::Q8_0,
                weight_gb: 7.54,
                decode: None,
            }],
            kv_geometry: None,
        };
        let system = fit::FitHardware {
            available_ram_gb: 32.0,
            vram_gb: None,
            gpu_bandwidth_gbps: None,
            unified_memory: false,
        };
        let shared = fit::FitHardware {
            vram_gb: Some(24.0),
            unified_memory: true,
            ..system
        };
        for hw in [system, shared] {
            let rf = fit::fit(&spec, &hw);
            assert!(is_runnable(rf.verdict));
            let j = judge(&spec, &hw, PICK_CONTEXT);
            assert!(j.config.is_none(), "{hw:?}");
            assert!(j.too_slow && j.ram_runnable, "{hw:?}");
        }

        // On a discrete card it is resident, and the chat floor, not the background floor, applies:
        // 1008 GB/s puts it well over 30.
        let card = fit::FitHardware {
            vram_gb: Some(24.0),
            gpu_bandwidth_gbps: Some(1008.0),
            ..system
        };
        let rf = fit::fit(&spec, &card);
        let j = judge(&spec, &card, PICK_CONTEXT);
        assert!(!j.too_slow);
        let config = j.config.expect("resident on a 24 GB card");
        assert_eq!(
            config, rf,
            "the quality config already fits with the reserve"
        );
        assert_eq!(j.rung, Rung::Quality);

        // A model that clears the floor from system RAM is judged on its RAM fit as it stands.
        let small = fit::ModelSpec {
            active_params_b: 3.82,
            candidates: vec![fit::QuantCandidate {
                quant: fit::Quant::Q8_0,
                weight_gb: 3.78,
                decode: None,
            }],
            ..spec
        };
        let rf = fit::fit(&small, &system);
        let j = judge(&small, &system, PICK_CONTEXT);
        assert_eq!(j.config, Some(rf));
        assert!(!j.too_slow);
    }

    #[test]
    fn a_config_that_had_to_shrink_to_stay_on_the_card_is_the_gpu_rung() {
        // 8 GB card, plenty of RAM: the quality config is Q8_0 off the card, the resident one is the
        // Q4_K_M that fits with the reserve.
        let spec = fit::ModelSpec {
            arch: fit::Architecture::Dense,
            active_params_b: 7.0,
            target_context: 32768,
            projector_gb: None,
            candidates: vec![
                fit::QuantCandidate {
                    quant: fit::Quant::Q8_0,
                    weight_gb: 7.5,
                    decode: None,
                },
                fit::QuantCandidate {
                    quant: fit::Quant::Q4_K_M,
                    weight_gb: 4.4,
                    decode: None,
                },
            ],
            kv_geometry: None,
        };
        let hw = fit::FitHardware {
            available_ram_gb: 22.0,
            vram_gb: Some(8.0),
            gpu_bandwidth_gbps: None,
            unified_memory: false,
        };
        let j = judge(&spec, &hw, PICK_CONTEXT);
        assert_eq!(j.rung, Rung::Gpu);
        assert_eq!(j.config.unwrap().quant, Some(fit::Quant::Q4_K_M));
    }

    /// Qwen2.5 7B's three middle quants and its real KV geometry, at its trained 32768.
    fn qwen_7b() -> fit::ModelSpec {
        let q = |quant, weight_gb| fit::QuantCandidate {
            quant,
            weight_gb,
            decode: None,
        };
        fit::ModelSpec {
            arch: fit::Architecture::Dense,
            active_params_b: 7.62,
            target_context: 32768,
            projector_gb: None,
            candidates: vec![
                q(fit::Quant::Q8_0, 7.54),
                q(fit::Quant::Q5_K_M, 5.07),
                q(fit::Quant::Q4_K_M, 4.36),
            ],
            kv_geometry: Some(fit::KvGeometry {
                bytes_per_token: 57_344.0,
                window_bytes_per_token: 0.0,
                window: 0,
                state_bytes: 0.0,
            }),
        }
    }

    fn no_card(free: f64) -> fit::FitHardware {
        fit::FitHardware {
            available_ram_gb: free,
            vram_gb: None,
            gpu_bandwidth_gbps: None,
            unified_memory: false,
        }
    }

    #[test]
    fn off_the_card_a_quicker_quant_of_the_same_model_still_counts() {
        // 12 GB free (a 10 GB budget): the best quant that fits is Q8_0, at 4.95 tok/s from RAM —
        // under the 8.53 floor. The Q4_K_M fits too and clears it at 8.6. Judging the Q8_0 alone
        // threw the whole model away; stepping down keeps it, as the GPU basis already did.
        let spec = qwen_7b();
        let hw = no_card(12.0);
        assert_eq!(fit::fit(&spec, &hw).quant, Some(fit::Quant::Q8_0));
        let j = judge(&spec, &hw, PICK_CONTEXT);
        let config = j.config.expect("the Q4_K_M is quick enough");
        assert_eq!(config.quant, Some(fit::Quant::Q4_K_M));
        assert!(is_runnable(config.verdict));
        assert_eq!(j.rung, Rung::Speed);
        assert!(!j.too_slow);
        assert_eq!(system_config(&spec, &hw, PICK_CONTEXT), Some(config));
    }

    #[test]
    fn more_free_memory_never_takes_a_config_away() {
        // The rule has to be monotonic in free memory: the pick used to SHRINK as memory was freed,
        // because a bigger budget reached a higher, slower quant that then failed the floor.
        for (label, hw_at) in [
            ("system", no_card as fn(f64) -> fit::FitHardware),
            ("shared", |free| fit::FitHardware {
                vram_gb: Some(free),
                unified_memory: true,
                ..no_card(free)
            }),
            // The dev laptop's card, where the chat floor applies as well as the card's own room.
            ("card", |free| fit::FitHardware {
                vram_gb: Some(7.96),
                gpu_bandwidth_gbps: Some(384.0),
                ..no_card(free)
            }),
        ] {
            let mut had = false;
            for tenth in 40..=640 {
                let free = f64::from(tenth) / 10.0;
                let has = judge(&qwen_7b(), &hw_at(free), PICK_CONTEXT)
                    .config
                    .is_some();
                assert!(
                    has || !had,
                    "{label}: a config at less memory was lost at {free} GB"
                );
                had |= has;
            }
            assert!(had, "{label}: it must fit somewhere in the sweep");
        }
    }

    #[test]
    fn the_pick_judges_a_long_context_model_at_the_context_pm_runs_it_at() {
        assert_eq!(pick_context(131_072, None), PICK_CONTEXT);
        assert_eq!(
            pick_context(8192, None),
            8192,
            "never longer than it was trained for"
        );
        // A server proven to hold more holds more, so that is what is judged...
        assert_eq!(pick_context(131_072, Some(65_536)), 65_536);
        // ...but a short proven window is only the server's current setting: PM's setup raises it.
        assert_eq!(pick_context(131_072, Some(4096)), PICK_CONTEXT);

        // Trained on 131072, it fits an 8 GB card only below that — but at the 32768 PM will run
        // it at, it fits at full context. Judged at its trained context, it was a halved context
        // and could never be the pick.
        let spec = fit::ModelSpec {
            target_context: 131_072,
            ..qwen_7b()
        };
        let card = fit::FitHardware {
            available_ram_gb: 20.0,
            vram_gb: Some(8.0),
            gpu_bandwidth_gbps: Some(384.0),
            unified_memory: false,
        };
        let trained = judge(&spec, &card, 131_072);
        assert!(trained.config.is_none(), "{:?}", trained.config);
        let j = judge(&spec, &card, pick_context(spec.target_context, None));
        let config = j.config.expect("it fits the card at 32k");
        assert_eq!(config.context, Some(PICK_CONTEXT));
        assert!(is_runnable(config.verdict));
    }

    // ---- the chat floor ----

    /// One build of a dense model whose decode bytes are given outright.
    fn build(quant: fit::Quant, weight_gb: f64, fast: f64, slow: f64) -> fit::QuantCandidate {
        fit::QuantCandidate {
            quant,
            weight_gb,
            decode: Some(fit::DecodeBytes { fast, slow }),
        }
    }

    /// A dense model at 8192 tokens, sized by the parameter-count KV proxy.
    fn model(active_b: f64, candidates: Vec<fit::QuantCandidate>) -> fit::ModelSpec {
        fit::ModelSpec {
            arch: fit::Architecture::Dense,
            active_params_b: active_b,
            target_context: 8192,
            projector_gb: None,
            candidates,
            kv_geometry: None,
        }
    }

    fn card(vram: f64, bandwidth: f64, free: f64) -> fit::FitHardware {
        fit::FitHardware {
            vram_gb: Some(vram),
            gpu_bandwidth_gbps: Some(bandwidth),
            ..no_card(free)
        }
    }

    /// A curated option judged for real against `hw`, fetchable whenever it has a config.
    fn judged_option(
        repo: &str,
        params: f64,
        spec: &fit::ModelSpec,
        hw: &fit::FitHardware,
    ) -> CatalogueOption {
        let judged = judge(spec, hw, 8192);
        let has = judged.config.is_some();
        CatalogueOption {
            repo: repo.to_string(),
            display_name: repo.to_string(),
            parameters_b: params,
            judged,
            tag: has.then(|| format!("hf.co/{repo}:Q3_K_M")),
            download_gb: has.then_some(params * 0.5),
            system_ok: false,
        }
    }

    /// A 12B whose one build fits an 8 GB card and is estimated at 24.2 tok/s on 384 GB/s, and a
    /// 9B whose one build is estimated at 43.3 there.
    fn big() -> fit::ModelSpec {
        model(11.91, vec![build(fit::Quant::Q3_K_M, 5.5, 0.0, 6.0e9)])
    }
    fn mid() -> fit::ModelSpec {
        model(8.95, vec![build(fit::Quant::Q3_K_M, 4.2, 2.0e9, 2.0e9)])
    }

    /// A curated option too slow for chat on the card, as `judge` would leave it: no config, and
    /// its build on the card estimated at 29.4.
    fn too_slow(repo: &str, params: f64) -> CatalogueOption {
        let mut o = option(repo, params, false);
        let mut on_card = config(fit::Verdict::Tight);
        on_card.quant = Some(fit::Quant::Q3_K_M);
        on_card.est_tokens_per_sec = Some(29.4);
        o.judged.on_card = Some(on_card);
        o.judged.too_slow_for_chat = true;
        o
    }

    #[test]
    fn the_chat_floor_is_an_answer_of_a_few_paragraphs_in_ten_seconds() {
        assert_eq!(chat_floor_tps(), 30.0);
        assert_eq!(
            chat_speed(),
            ChatSpeed {
                floor_tps: 30.0,
                reply_tokens: 300,
                reply_secs: 10.0,
            }
        );
    }

    #[test]
    fn the_chat_floor_compares_the_figure_the_ui_shows() {
        // The UI prints a whole number, so "about 30" must never sit beside "under 30".
        assert!(quick_enough_for_chat(29.5), "shows as 30");
        assert!(
            quick_enough_for_chat(29.46),
            "29.5 to one decimal, shows as 30"
        );
        assert!(!quick_enough_for_chat(29.44), "shows as 29");
        assert!(
            !quick_enough_for_chat(29.4),
            "gemma 4 12b on the dev laptop"
        );
        assert!(quick_enough_for_chat(42.7));
    }

    #[test]
    fn on_a_card_the_largest_model_quick_enough_for_chat_is_the_pick() {
        let hw = card(8.0, 384.0, 20.0);
        // The 12B fits the card, but its one build is under the floor there.
        let j = judge(&big(), &hw, 8192);
        assert!(j.config.is_none() && j.too_slow_for_chat && !j.too_slow);
        let on_card = j.on_card.clone().expect("it fits the card");
        assert_eq!(on_card.est_tokens_per_sec, Some(24.2));
        assert_eq!(j.rung, Rung::Quality, "no config, no rung");

        let pool = [
            judged_option("big", 11.91, &big(), &hw),
            judged_option("mid", 8.95, &mid(), &hw),
        ];
        match pick(PickBasis::Gpu, &pool, &[]) {
            Pick::Catalogue {
                repo,
                fit,
                passed_over,
                ..
            } => {
                assert_eq!(repo, "mid");
                assert_eq!(fit.est_tokens_per_sec, Some(43.3));
                // Named with its build on the card and that build's own estimate.
                assert_eq!(
                    passed_over,
                    Some(PassedOver {
                        repo: "big".to_string(),
                        display_name: "big".to_string(),
                        quant: fit::Quant::Q3_K_M,
                        est_tokens_per_sec: 24.2,
                        have: None,
                    })
                );
            }
            other => panic!("expected a catalogue pick, got {other:?}"),
        }
    }

    #[test]
    fn on_a_card_a_quicker_build_of_the_same_model_still_counts() {
        // Both builds fit the card; the larger is estimated at 26.9, the smaller at 43.3. The model
        // stays, at the smaller build, rather than being turned away for its larger one.
        let spec = model(
            11.91,
            vec![
                build(fit::Quant::Q4_K_M, 5.6, 8.0e9, 0.0),
                build(fit::Quant::Q3_K_M, 5.0, 2.0e9, 2.0e9),
            ],
        );
        let hw = card(8.0, 384.0, 20.0);
        let j = judge(&spec, &hw, 8192);
        let config = j.config.expect("the Q3_K_M is quick enough");
        assert_eq!(config.quant, Some(fit::Quant::Q3_K_M));
        assert_eq!(config.est_tokens_per_sec, Some(43.3));
        assert_eq!(j.rung, Rung::Chat);
        let on_card = j.on_card.expect("the Q4_K_M fits the card");
        assert_eq!(on_card.quant, Some(fit::Quant::Q4_K_M));
        assert_eq!(on_card.est_tokens_per_sec, Some(26.9));
        assert!(!j.too_slow_for_chat);
    }

    #[test]
    fn the_cards_bandwidth_moves_the_pick_but_never_what_fits() {
        // The bandwidth is not a display-only figure: through the chat floor it decides which build,
        // if any, the pick runs here. What fits — free memory's verdict, and the best build on the
        // card — is the same at any bandwidth; only the speed estimate beside it moves.
        let spec = model(
            11.91,
            vec![
                build(fit::Quant::Q4_K_M, 5.6, 8.0e9, 0.0),
                build(fit::Quant::Q3_K_M, 5.0, 2.0e9, 2.0e9),
            ],
        );
        let what_fits = |f: &fit::FitResult| (f.verdict, f.quant, f.context, f.kv, f.est_memory_gb);
        let at = |bandwidth: f64| card(8.0, bandwidth, 20.0);
        let ram = what_fits(&fit::fit(&spec, &at(384.0)));
        let resident = what_fits(&judge(&spec, &at(384.0), 8192).on_card.unwrap());
        for (bandwidth, picked) in [
            // Q4_K_M at 42.0: the best build on the card is quick enough.
            (600.0, Some(fit::Quant::Q4_K_M)),
            // Q4_K_M at 26.9, Q3_K_M at 43.3: a quicker build steps in.
            (384.0, Some(fit::Quant::Q3_K_M)),
            // Q3_K_M at 11.3: nothing is.
            (100.0, None),
        ] {
            let hw = at(bandwidth);
            assert_eq!(what_fits(&fit::fit(&spec, &hw)), ram, "{bandwidth} GB/s");
            let j = judge(&spec, &hw, 8192);
            let on_card = j.on_card.expect("the card's room is the same");
            assert_eq!(what_fits(&on_card), resident, "{bandwidth} GB/s");
            assert_eq!(j.config.and_then(|c| c.quant), picked, "{bandwidth} GB/s");
            assert_eq!(j.too_slow_for_chat, picked.is_none(), "{bandwidth} GB/s");
        }
    }

    #[test]
    fn the_chat_floor_applies_on_the_graphics_card_only() {
        // 3 GB a token: 18.6 tok/s on a 100 GB/s card, under the chat floor, and 13.3 from system
        // memory, over the background floor.
        let spec = model(7.0, vec![build(fit::Quant::Q4_K_M, 4.0, 3.0e9, 0.0)]);
        let j = judge(&spec, &card(8.0, 100.0, 20.0), 8192);
        assert!(j.config.is_none() && j.on_card.is_some());
        assert!(j.too_slow_for_chat && !j.too_slow);

        let shared = fit::FitHardware {
            vram_gb: Some(12.0),
            unified_memory: true,
            ..no_card(20.0)
        };
        for hw in [no_card(20.0), shared] {
            let j = judge(&spec, &hw, 8192);
            assert!(j.config.is_some(), "{hw:?}");
            assert!(j.on_card.is_none() && !j.too_slow_for_chat, "{hw:?}");
        }
    }

    #[test]
    fn a_card_where_nothing_is_quick_enough_says_so() {
        // 20 GB/s: even a 1B is 8.6 tok/s on it. Something else would run from system memory, and
        // the small one would be fine there too — but the reason is the speed, not the card's room,
        // so there is no "some would run, several times slower" fallback to offer.
        let hw = card(8.0, 20.0, 20.0);
        let small = model(1.24, vec![build(fit::Quant::Q8_0, 1.23, 1.3e9, 0.0)]);
        let mut slow = judged_option("small", 1.24, &small, &hw);
        assert!(slow.judged.too_slow_for_chat, "{:?}", slow.judged);
        slow.system_ok = true;
        let spills = option("spills", 14.0, false);
        assert!(spills.judged.ram_runnable);
        assert_eq!(
            pick(PickBasis::Gpu, &[spills, slow], &[]),
            Pick::Nothing {
                reason: NoPick::TooSlowForChat,
                basis: PickBasis::Gpu,
                system_fallback: false,
            }
        );
    }

    #[test]
    fn a_copy_you_have_that_is_too_slow_for_chat_is_named_not_picked() {
        let hw = card(8.0, 384.0, 20.0);
        let pool = [
            judged_option("big", 11.91, &big(), &hw),
            judged_option("mid", 8.95, &mid(), &hw),
        ];
        // Two copies of the 12B, both too slow for chat on this card; a role uses the second.
        let copy = |id: &str, bound: bool, est: f64| {
            let mut o = owned(id, 11.91, true, bound);
            o.repo = "big".to_string();
            o.display_name = "big".to_string();
            let mut on_card = config(fit::Verdict::Tight);
            on_card.quant = Some(fit::Quant::Q3_K_M);
            on_card.est_tokens_per_sec = Some(est);
            o.config = None;
            o.on_card = Some(on_card);
            o
        };
        let have = [copy("big:a", false, 25.0), copy("big:b", true, 26.0)];
        assert!(have.iter().all(OwnedOption::too_slow_for_chat));
        assert!(!owned("quick", 8.95, true, true).too_slow_for_chat());
        for catalogue in [&pool[..], &pool[1..]] {
            match pick(PickBasis::Gpu, catalogue, &have) {
                Pick::Catalogue {
                    repo,
                    also_have,
                    passed_over,
                    ..
                } => {
                    assert_eq!(repo, "mid");
                    assert_eq!(also_have, None, "not a copy the pick could use");
                    let po = passed_over.expect("the copy is named");
                    assert_eq!(po.repo, "big");
                    assert_eq!(
                        po.have,
                        Some(OwnedRef {
                            id: "big:b".to_string(),
                            display_name: "big".to_string(),
                            served: true,
                        }),
                        "the one a role uses, first"
                    );
                    assert_eq!(
                        po.est_tokens_per_sec, 26.0,
                        "that copy's build, not the list's"
                    );
                }
                other => panic!("expected a catalogue pick, got {other:?}"),
            }
        }
    }

    #[test]
    fn passed_over_names_only_a_larger_model_and_only_on_a_card() {
        let quick = || option("quick", 7.62, true);
        let named = |p: Pick| match p {
            Pick::Catalogue { passed_over, .. } | Pick::Owned { passed_over, .. } => {
                passed_over.map(|po| po.repo)
            }
            Pick::Nothing { .. } => panic!("expected a pick"),
        };
        // Smaller than the pick, or the same size: nothing was passed over for it.
        for params in [3.0, 7.62] {
            assert_eq!(
                named(pick(
                    PickBasis::Gpu,
                    &[quick(), too_slow("slow", params)],
                    &[]
                )),
                None,
                "{params}"
            );
        }
        // Larger: the largest of them, ties to the lower repo.
        let pool = [
            quick(),
            too_slow("smaller", 9.0),
            too_slow("b-slow", 12.0),
            too_slow("a-slow", 12.0),
        ];
        assert_eq!(
            named(pick(PickBasis::Gpu, &pool, &[])).as_deref(),
            Some("a-slow")
        );
        // Off the card the chat floor never applies, so nothing is passed over for it.
        for basis in [PickBasis::System, PickBasis::Shared] {
            assert_eq!(named(pick(basis, &pool, &[])), None, "{basis:?}");
        }
        // A model the user has, picked over the list, names one too.
        let p = pick(PickBasis::Gpu, &pool, &[owned("have", 7.62, true, false)]);
        assert!(matches!(p, Pick::Owned { .. }), "{p:?}");
        assert_eq!(named(p).as_deref(), Some("a-slow"));
    }

    #[test]
    fn the_pick_serializes_the_shape_the_frontend_mirrors() {
        let p = pick(
            PickBasis::Gpu,
            &[option("qwen-7b", 7.62, true)],
            &[owned("gemma3:4b", 3.88, true, false)],
        );
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "catalogue");
        assert_eq!(v["rung"], "gpu");
        assert_eq!(v["basis"], "gpu");
        assert_eq!(v["tag"], "hf.co/qwen-7b:Q4_K_M");
        assert_eq!(v["fit"]["speed_basis"], "gpu_published");
        assert_eq!(v["also_have"]["id"], "gemma3:4b");
        assert_eq!(v["also_have"]["served"], true);
        assert_eq!(v["passed_over"], serde_json::Value::Null);

        // A larger model too slow for chat on the card, which the user has a copy of.
        let mut copy = owned("gemma:12b", 11.91, true, true);
        // Its entry's, as every owned option's display name is.
        copy.repo = "gemma-12b".to_string();
        copy.display_name = "gemma-12b".to_string();
        copy.config = None;
        copy.on_card = too_slow("gemma-12b", 11.91).judged.on_card;
        let v = serde_json::to_value(pick(
            PickBasis::Gpu,
            &[option("qwen-9b", 8.95, true), too_slow("gemma-12b", 11.91)],
            &[copy],
        ))
        .unwrap();
        assert_eq!(v["kind"], "catalogue");
        assert_eq!(v["passed_over"]["repo"], "gemma-12b");
        assert_eq!(v["passed_over"]["display_name"], "gemma-12b");
        assert_eq!(v["passed_over"]["quant"], "Q3_K_M");
        assert_eq!(v["passed_over"]["est_tokens_per_sec"], 29.4);
        assert_eq!(v["passed_over"]["have"]["id"], "gemma:12b");
        assert_eq!(v["passed_over"]["have"]["display_name"], "gemma-12b");
        assert_eq!(v["passed_over"]["have"]["served"], true);

        let v = serde_json::to_value(pick(
            PickBasis::Gpu,
            &[],
            &[owned("disk-model", 7.0, false, false)],
        ))
        .unwrap();
        assert_eq!(v["kind"], "owned");
        assert_eq!(v["source"], "ollama");
        assert_eq!(v["measured"], true);
        assert_eq!(v["path"], serde_json::Value::Null);
        assert_eq!(v["passed_over"], serde_json::Value::Null);

        let v = serde_json::to_value(pick(PickBasis::System, &[], &[])).unwrap();
        assert_eq!(v["kind"], "nothing");
        assert_eq!(v["reason"], "too_little_memory");
        assert_eq!(v["basis"], "system");
        assert_eq!(v["system_fallback"], false);
        for (reason, wire) in [
            (NoPick::NothingOnGpu, "nothing_on_gpu"),
            (NoPick::TooSlowForChat, "too_slow_for_chat"),
            (NoPick::TooSlow, "too_slow"),
        ] {
            assert_eq!(serde_json::to_value(reason).unwrap(), wire);
        }
        assert_eq!(serde_json::to_value(PickBasis::Shared).unwrap(), "shared");
        assert_eq!(serde_json::to_value(Rung::Quality).unwrap(), "quality");
        assert_eq!(serde_json::to_value(Rung::Chat).unwrap(), "chat");
        assert_eq!(serde_json::to_value(Rung::Speed).unwrap(), "speed");
    }
}
