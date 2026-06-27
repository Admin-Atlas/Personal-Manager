# SPDX-FileCopyrightText: 2026 Bobby Yu
# SPDX-License-Identifier: AGPL-3.0-or-later

"""PM document sidecar.

A long-lived helper process that does the jobs the Rust backend cannot do
natively: convert arbitrary files to Markdown (MarkItDown), embed text locally
with an on-device ONNX model (fastembed), re-rank passages with a local
cross-encoder (fastembed), and transcribe voice clips locally (faster-whisper).
It is the only part of PM that runs Python, and it is deliberately dumb: it
converts, embeds, scores, and transcribes, nothing else. Chunking, hashing,
prefixes, the vault, and the database all live in Rust.

Protocol: newline-delimited JSON over stdio. One request per line:

    {"id": <int>, "method": "ping|convert|embed|count_tokens|rerank|transcribe", "params": {...}}

One response per line:

    {"id": <int>, "ok": true,  "result": {...}}
    {"id": <int>, "ok": false, "error": "message"}

Heavy objects (the MarkItDown instance, the embedding/reranking models, the
Whisper model) are created lazily on first use so `ping` stays instant and
startup is cheap. A model downloads its weights the first time it is used, then
runs fully on-device.

Security: ingested content — including transcribed audio and passages scored by
the reranker — is untrusted data, never instructions. This process only
converts/embeds/scores/transcribes bytes; it never executes file contents.
"""

import json
import sys
import traceback

# The default embedder + dimension (the English model). PR 2 makes embedding
# model-parameterised: Rust sends the selected model id (and, for a custom model
# not bundled in fastembed, a registration spec) per request, so a vault can use a
# multilingual embedder. These remain the fallback when no model is supplied.
EMBED_MODEL = "BAAI/bge-small-en-v1.5"
EMBED_DIM = 384

# The speech-to-text model for voice input. Fixed for v1, like EMBED_MODEL.
# English-only, ~145 MB, run int8 on CPU — the speed/accuracy sweet spot for
# dictation. Weights download once on first `transcribe`, then it is fully local.
WHISPER_MODEL = "base.en"

_markitdown = None
# Per-model caches, keyed by model id, so one process can serve several models
# (e.g. the English embedder plus a multilingual one). `_registered` tracks the
# custom models already handed to fastembed's `add_custom_model` (calling it twice
# for the same id raises).
_embedders = {}
_tokenizers = {}
_rerankers = {}
_registered = set()
_whisper = None


def get_markitdown():
    global _markitdown
    if _markitdown is None:
        from markitdown import MarkItDown

        _markitdown = MarkItDown()
    return _markitdown


def _pooling_type(name):
    """Map a pooling name from Rust to fastembed's PoolingType (default MEAN)."""
    from fastembed.common.model_description import PoolingType

    return {"mean": PoolingType.MEAN, "cls": PoolingType.CLS}.get(
        (name or "mean").lower(), PoolingType.MEAN
    )


def get_embedder(model=None, spec=None):
    """The embedder for `model` (default: the English EMBED_MODEL), cached per id.

    `spec` (present for a custom, non-bundled model) carries the fastembed
    `add_custom_model` arguments Rust derived from the registry: the HF source
    repo, the ONNX file, pooling, normalization, and dimension. We register a
    custom model with fastembed once per id (calling it twice raises).
    """
    model = model or EMBED_MODEL
    if model not in _embedders:
        from fastembed import TextEmbedding

        if spec and spec["model"] not in _registered:
            from fastembed.common.model_description import ModelSource

            TextEmbedding.add_custom_model(
                model=spec["model"],
                pooling=_pooling_type(spec.get("pooling")),
                normalization=bool(spec.get("normalize", True)),
                sources=ModelSource(hf=spec["hf"]),
                dim=int(spec["dim"]),
                model_file=spec.get("model_file", "onnx/model.onnx"),
            )
            _registered.add(spec["model"])
        _embedders[model] = TextEmbedding(model_name=model)
    return _embedders[model]


def get_tokenizer(model=None, spec=None):
    """The selected embedder's tokenizer, for sizing chunks by tokens in Rust.

    fastembed wraps a `tokenizers.Tokenizer`; we reach it defensively across
    versions (the attribute path is internal), falling back to loading the repo's
    tokenizer directly, then to `None` so the caller can estimate. Truncation is
    disabled so an oversized chunk reports its *true* length (the splitter must see
    it overflow to break it up). Padding is deliberately left untouched: this is the
    SAME tokenizer instance fastembed batches embeddings with, so disabling padding
    here could break/degrade embedding — `do_count_tokens` strips padding from the
    count via the attention mask instead. Reuses the embedder's already-downloaded
    weights — no extra download.
    """
    model = model or EMBED_MODEL
    if model in _tokenizers:
        return _tokenizers[model]
    tok = None
    emb = get_embedder(model, spec)
    candidate = getattr(emb, "tokenizer", None) or getattr(
        getattr(emb, "model", None), "tokenizer", None
    )
    if candidate is not None and hasattr(candidate, "encode"):
        tok = candidate
    if tok is None:
        try:
            from tokenizers import Tokenizer

            tok = Tokenizer.from_pretrained(model)
        except Exception:
            tok = None
    if tok is not None:
        try:
            tok.no_truncation()
        except Exception:
            pass
        # Cache only on success so a transient failure retries next time.
        _tokenizers[model] = tok
    return tok


def get_reranker(model, spec=None):
    """The cross-encoder reranker for `model`, cached per id.

    `spec` (present for a custom, non-bundled reranker) registers it via
    fastembed's `add_custom_model` from the HF source repo + ONNX file, once per id.
    """
    if model not in _rerankers:
        from fastembed.rerank.cross_encoder import TextCrossEncoder

        if spec and spec["model"] not in _registered:
            from fastembed.common.model_description import ModelSource

            TextCrossEncoder.add_custom_model(
                model=spec["model"],
                sources=ModelSource(hf=spec["hf"]),
                model_file=spec.get("model_file", "onnx/model.onnx"),
            )
            _registered.add(spec["model"])
        _rerankers[model] = TextCrossEncoder(model_name=model)
    return _rerankers[model]


def get_whisper(model_dir):
    global _whisper
    if _whisper is None:
        from faster_whisper import WhisperModel

        # `model_dir` (passed by Rust) keeps the weights inside PM's data dir so
        # they uninstall cleanly with it; None falls back to the default cache.
        _whisper = WhisperModel(
            WHISPER_MODEL,
            device="cpu",
            compute_type="int8",
            download_root=model_dir or None,
        )
    return _whisper


def clean_text(value):
    """Coerce to tokenizer- and JSON-safe text.

    Converting arbitrary files (and OCR) can yield text carrying lone UTF-16
    surrogates — e.g. when a source file is decoded with surrogate-escape. Those
    are not valid Unicode scalars, so two things downstream reject them: the Rust
    side's strict JSON parser refuses the surrogate escape in our reply (silently
    desyncing the protocol), and HuggingFace `tokenizers` raises "TextEncodeInput
    must be ..." which fails the whole embed batch. Round-tripping through UTF-8
    with errors="ignore" drops only the lone surrogates and leaves all normal
    text untouched. We also coerce non-strings defensively so one odd value can
    never abort a batch.
    """
    if not isinstance(value, str):
        value = "" if value is None else str(value)
    return value.encode("utf-8", "ignore").decode("utf-8", "ignore")


def do_convert(params):
    """Convert one file to Markdown. Returns its text and a best-effort title."""
    path = params["path"]
    result = get_markitdown().convert(path)
    title = (getattr(result, "title", None) or "").strip()
    return {
        "markdown": clean_text(result.text_content or ""),
        "title": clean_text(title),
    }


def do_embed(params):
    """Embed a batch of strings with the selected ONNX model.

    Rust prepends any asymmetric retrieval prefix (e5's `query:` / `passage:`)
    before sending, so the text is embedded as-is. `model`/`custom` select and
    (for a custom model) register the embedder; both default to the English model.
    `batch_size`, when present, caps how many texts the embedder processes per
    forward pass — the "gentle" indexing lever, which bounds peak activation
    memory on a low-memory machine. Absent → fastembed's own default batch.
    """
    # Sanitize before tokenizing: one chunk with a stray surrogate would
    # otherwise fail the entire batch (and thus the whole document).
    texts = [clean_text(t) for t in params.get("texts", [])]
    model = params.get("model") or EMBED_MODEL
    spec = params.get("custom")
    embedder = get_embedder(model, spec)
    batch_size = params.get("batch_size")
    # fastembed yields numpy arrays; hand back plain lists so they serialize.
    if batch_size:
        vecs = embedder.embed(texts, batch_size=int(batch_size))
    else:
        vecs = embedder.embed(texts)
    vectors = [vec.tolist() for vec in vecs]
    dim = int(spec["dim"]) if spec and spec.get("dim") else EMBED_DIM
    return {"vectors": vectors, "dim": dim, "model": model}


def do_count_tokens(params):
    """Token counts for a batch, using the selected embedder's own tokenizer.

    The Rust splitter sizes chunks by tokens (never chars) so a chunk never
    overflows the embedder's input window. Counting with the same tokenizer that
    embeds keeps the two in lockstep. If no tokenizer can be loaded, fall back to a
    rough chars/4 estimate so chunking still works rather than failing the document.
    """
    texts = [clean_text(t) for t in params.get("texts", [])]
    model = params.get("model") or EMBED_MODEL
    spec = params.get("custom")
    tok = get_tokenizer(model, spec)
    if tok is not None:
        try:
            encodings = tok.encode_batch(texts)
            # `encode_batch` pads every text to the batch's longest (fastembed
            # leaves batch padding enabled, and this is the same shared tokenizer
            # it embeds with, so we must not turn padding off here). `len(e.ids)`
            # would then count pad tokens and report every text as the longest
            # one's length — which made the splitter size every block by the
            # document's largest block and shatter long documents. The attention
            # mask is 1 for real tokens and 0 for padding, so summing it gives each
            # text's TRUE length, independent of the rest of the batch.
            return {"counts": [int(sum(e.attention_mask)) for e in encodings]}
        except Exception:
            pass
    return {"counts": [max(1, len(t) // 4) for t in texts]}


def do_rerank(params):
    """Score query<->passage relevance with the selected cross-encoder reranker.

    Returns one score per passage (higher = more relevant); Rust reorders by them.
    `model`/`custom` select and (for a custom model) register the reranker. Passage
    text is untrusted data — scored, never executed.
    """
    query = clean_text(params.get("query", ""))
    passages = [clean_text(p) for p in params.get("passages", [])]
    if not passages:
        return {"scores": []}
    reranker = get_reranker(params.get("model"), params.get("custom"))
    scores = list(reranker.rerank(query, passages))
    return {"scores": [float(s) for s in scores]}


def do_transcribe(params):
    """Transcribe one audio clip to text with the local Whisper model.

    The Rust side records the clip in the webview, writes the bytes to a temp
    file, and passes its path here; faster-whisper decodes the audio itself via
    the bundled PyAV (no system ffmpeg needed). The text is the user's own speech
    bound for the chat box — still untrusted data, so it is `clean_text`-ed like
    every other string we return.
    """
    path = params["path"]
    model = get_whisper(params.get("model_dir"))
    segments, _info = model.transcribe(path)
    text = " ".join(segment.text.strip() for segment in segments).strip()
    return {"text": clean_text(text)}


def _pca(x, k):
    """Project mean-centred rows `x` (n, d) onto their top-`k` principal axes via SVD.

    numpy only (already a dependency via fastembed), so this path needs no extra install. Used both
    as the default 2-D reducer and as the 50-d initialisation for t-SNE.
    """
    import numpy as np

    k = min(k, x.shape[1], x.shape[0])
    # Economy SVD: columns of Vt are the principal directions; U*S is the projection onto them.
    u, s, _vt = np.linalg.svd(x, full_matrices=False)
    return (u[:, :k] * s[:k]).astype(np.float32)


def _fit_unit(coords):
    """Min-max scale each axis into [0,1] so the Rust/TS side never guesses the coord range."""
    import numpy as np

    c = np.asarray(coords, dtype=np.float32)
    mn = c.min(axis=0)
    mx = c.max(axis=0)
    span = np.where((mx - mn) > 1e-9, mx - mn, 1.0)
    return (c - mn) / span


def _reduce(vecs, method):
    """Reduce per-document vectors to 2-D. Returns (coords, method_actually_used).

    `pca` (the bundled default) is numpy-only and instant. `tsne` uses openTSNE — an OPTIONAL
    component the user downloads from Settings; if it isn't installed (or fails) we fall back to
    PCA and report `pca`, so the map is always usable. The marker the Rust side checks decides what
    to *request*; this decides what actually ran.
    """
    import numpy as np

    x = vecs - vecs.mean(axis=0, keepdims=True)  # mean-centre
    if method != "tsne":
        return _pca(x, 2), "pca"
    try:
        from openTSNE import TSNE

        n, d = x.shape
        # Pre-reduce to 50-d with PCA to speed up the neighbour search (a standard t-SNE step); the
        # 2-D start is PCA-initialised, which keeps the layout deterministic given random_state.
        x_in = _pca(x, 50) if d > 50 else x
        ts = TSNE(
            n_components=2,
            perplexity=min(30, max(5, (n - 1) // 3)),
            metric="cosine",
            initialization="pca",
            n_jobs=1,
            random_state=42,
            verbose=False,
        )
        return np.asarray(ts.fit(x_in), dtype=np.float32), "tsne"
    except Exception:
        # Not installed, or a runtime failure — a usable PCA map beats no map.
        return _pca(x, 2), "pca"


def do_reduce(params):
    """Project a batch of per-document vectors to 2-D for the semantic memory map.

    params: {"vectors": [[float]...], "method": "pca"|"tsne"}
    returns: {"coords": [[x,y]...] in [0,1]^2, "method": <actually used>}

    Untrusted-data rule still holds: numbers in, numbers out, never executed.
    """
    raw = params.get("vectors") or []
    n = len(raw)
    if n == 0:
        return {"coords": [], "method": "none"}
    if n <= 3:
        # t-SNE/PCA degenerate for a handful of points; a deterministic spread keeps the map sane.
        # (These guards stay numpy-free so a tiny map needs no scientific stack at all.)
        return {"coords": [[float(i), 0.0] for i in range(n)], "method": "trivial"}
    import numpy as np

    vecs = np.asarray(raw, dtype=np.float32)
    coords, used = _reduce(vecs, (params.get("method") or "pca").lower())
    return {"coords": _fit_unit(coords).tolist(), "method": used}


HANDLERS = {
    "ping": lambda params: {"ok": True},
    "convert": do_convert,
    "embed": do_embed,
    "count_tokens": do_count_tokens,
    "rerank": do_rerank,
    "transcribe": do_transcribe,
    "reduce": do_reduce,
}


# Mirror the Rust reader's per-line cap. Rust is the trusted sender and never
# sends anything near this, so a line this large can only be a fault or abuse —
# drop it rather than hand it to json.loads.
MAX_LINE_CHARS = 64 * 1024 * 1024


def main():
    # Line-buffered stdout so the Rust side sees each reply immediately.
    out = sys.stdout
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        if len(line) > MAX_LINE_CHARS:
            sys.stderr.write("pm_sidecar: dropping oversized request line\n")
            sys.stderr.flush()
            continue

        req_id = None
        try:
            req = json.loads(line)
            req_id = req.get("id")
            method = req.get("method")
            handler = HANDLERS.get(method)
            if handler is None:
                raise ValueError(f"unknown method: {method!r}")
            result = handler(req.get("params") or {})
            response = {"id": req_id, "ok": True, "result": result}
        except Exception as exc:  # report, never crash the loop
            traceback.print_exc(file=sys.stderr)
            response = {"id": req_id, "ok": False, "error": str(exc)}

        # `default=str` so a non-JSON-serializable result stringifies instead of
        # raising here (outside the try) and silently killing the read loop.
        out.write(json.dumps(response, default=str) + "\n")
        out.flush()


if __name__ == "__main__":
    main()
