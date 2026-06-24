# SPDX-FileCopyrightText: 2026 Bobby Yu
# SPDX-License-Identifier: AGPL-3.0-or-later

"""PM document sidecar.

A long-lived helper process that does the jobs the Rust backend cannot do
natively: convert arbitrary files to Markdown (MarkItDown), embed text locally
with an on-device ONNX model (fastembed), and transcribe voice clips locally
(faster-whisper). It is the only part of PM that runs Python, and it is
deliberately dumb: it converts, embeds, and transcribes, nothing else. Chunking,
hashing, the vault, and the database all live in Rust.

Protocol: newline-delimited JSON over stdio. One request per line:

    {"id": <int>, "method": "ping|convert|embed|transcribe", "params": {...}}

One response per line:

    {"id": <int>, "ok": true,  "result": {...}}
    {"id": <int>, "ok": false, "error": "message"}

Heavy objects (the MarkItDown instance, the embedding model, the Whisper model)
are created lazily on first use so `ping` stays instant and startup is cheap. The
embedding model downloads its weights (~90 MB) the first time `embed` is called,
and the speech model (~145 MB) the first time `transcribe` is called.

Security: ingested content — including transcribed audio — is untrusted data,
never instructions. This process only converts/embeds/transcribes bytes; it never
executes file contents.
"""

import json
import sys
import traceback

# The embedding model. Fixed for v1 — changing it forces a full re-index, so the
# Rust side pins this name in `settings` and the vec0 column dimension matches.
EMBED_MODEL = "BAAI/bge-small-en-v1.5"
EMBED_DIM = 384

# The speech-to-text model for voice input. Fixed for v1, like EMBED_MODEL.
# English-only, ~145 MB, run int8 on CPU — the speed/accuracy sweet spot for
# dictation. Weights download once on first `transcribe`, then it is fully local.
WHISPER_MODEL = "base.en"

_markitdown = None
_embedder = None
_tokenizer = None
_whisper = None


def get_markitdown():
    global _markitdown
    if _markitdown is None:
        from markitdown import MarkItDown

        _markitdown = MarkItDown()
    return _markitdown


def get_embedder():
    global _embedder
    if _embedder is None:
        from fastembed import TextEmbedding

        _embedder = TextEmbedding(model_name=EMBED_MODEL)
    return _embedder


def get_tokenizer():
    """The active embedder's tokenizer, for sizing chunks by tokens in Rust.

    fastembed wraps a `tokenizers.Tokenizer`; we reach it defensively across versions
    (the attribute path is internal), falling back to loading the repo's tokenizer
    directly, then to `None` so the caller can estimate. Truncation is disabled so an
    oversized chunk reports its *true* length (the splitter must see it overflow to break
    it up). Reuses the embedder's already-downloaded weights — no extra download.
    """
    global _tokenizer
    if _tokenizer is not None:
        return _tokenizer
    tok = None
    emb = get_embedder()
    candidate = getattr(emb, "tokenizer", None) or getattr(
        getattr(emb, "model", None), "tokenizer", None
    )
    if candidate is not None and hasattr(candidate, "encode"):
        tok = candidate
    if tok is None:
        try:
            from tokenizers import Tokenizer

            tok = Tokenizer.from_pretrained(EMBED_MODEL)
        except Exception:
            tok = None
    if tok is not None:
        try:
            tok.no_truncation()
        except Exception:
            pass
    _tokenizer = tok
    return _tokenizer


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
    """Embed a batch of strings into 384-d vectors with the local ONNX model."""
    # Sanitize before tokenizing: one chunk with a stray surrogate would
    # otherwise fail the entire batch (and thus the whole document).
    texts = [clean_text(t) for t in params.get("texts", [])]
    # fastembed yields numpy arrays; hand back plain lists so they serialize.
    vectors = [vec.tolist() for vec in get_embedder().embed(texts)]
    return {"vectors": vectors, "dim": EMBED_DIM, "model": EMBED_MODEL}


def do_count_tokens(params):
    """Token counts for a batch, using the embedder's own tokenizer.

    The Rust splitter sizes chunks by tokens (never chars) so a chunk never overflows the
    embedder's input window. Counting with the same tokenizer that embeds keeps the two in
    lockstep. If no tokenizer can be loaded, fall back to a rough chars/4 estimate so
    chunking still works rather than failing the document.
    """
    texts = [clean_text(t) for t in params.get("texts", [])]
    tok = get_tokenizer()
    if tok is not None:
        try:
            encodings = tok.encode_batch(texts)
            return {"counts": [len(e.ids) for e in encodings]}
        except Exception:
            pass
    return {"counts": [max(1, len(t) // 4) for t in texts]}


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


HANDLERS = {
    "ping": lambda params: {"ok": True},
    "convert": do_convert,
    "embed": do_embed,
    "count_tokens": do_count_tokens,
    "transcribe": do_transcribe,
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
