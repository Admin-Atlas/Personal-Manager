# SPDX-FileCopyrightText: 2026 Bobby Yu
# SPDX-License-Identifier: AGPL-3.0-or-later

"""PM document sidecar.

A long-lived helper process that does the jobs the Rust backend cannot do
natively: convert arbitrary files to Markdown (MarkItDown), embed text locally
with an on-device ONNX model (fastembed), re-rank passages with a local
cross-encoder (fastembed), transcribe voice clips locally (faster-whisper), and
read text + capture metadata out of photos (rapidocr OCR + Pillow EXIF — an
OPTIONAL component the user installs on demand, like the t-SNE reducer).
It is the only part of PM that runs Python, and it is deliberately dumb: it
converts, embeds, scores, transcribes, and analyses images, nothing else.
Chunking, hashing, prefixes, the vault, and the database all live in Rust.

Protocol: newline-delimited JSON over stdio. One request per line, where method is one of
ping, convert, embed, count_tokens, rerank, transcribe, reduce, analyze_image, analyze_spreadsheet:

    {"id": <int>, "method": <name>, "params": {...}}

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
import logging
import os
import sys
import traceback

# pdfminer (which MarkItDown uses to extract text from PDFs) logs a warning for every glyph
# whose font descriptor has no FontBBox ("None cannot be parsed as 4 floats") — cosmetic noise
# that can flood the console on a single PDF and tells us nothing actionable. Quiet that logger
# to errors; genuine extraction failures still surface.
logging.getLogger("pdfminer").setLevel(logging.ERROR)

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
_ocr_engine = None
_heif_registered = False


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
    """A tokenizer for sizing chunks by tokens in Rust — an INDEPENDENT COPY of the
    embedder's tokenizer.

    We disable truncation so an oversized chunk reports its *true* length (the splitter
    must see it overflow to break it up). The catch: fastembed's tokenizer is the SAME
    object it embeds with, and it ships with truncation enabled to the model's 512-token
    window. Mutating that shared object with `no_truncation()` (as this used to) stripped
    the embedder's safety net, so a chunk longer than 512 tokens crashed ONNX with a
    `512 vs N` broadcast error instead of being harmlessly truncated. So we CLONE it
    (`from_str(to_str())`) and disable truncation on the copy only — the embedder keeps
    its truncation. The clone reuses the in-memory weights (no download). Padding is left
    as fastembed set it (batch padding on); `do_count_tokens` strips it via the attention
    mask. Falls back to loading the repo tokenizer, then to `None` (caller estimates).
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
        try:
            from tokenizers import Tokenizer

            # Independent copy: disabling truncation here must never touch the embedder's.
            tok = Tokenizer.from_str(candidate.to_str())
            tok.no_truncation()
        except Exception:
            # Clone unavailable: count with the shared tokenizer AS-IS (truncation stays
            # on, so a long chunk just reports a capped length) — never mutate it.
            tok = candidate
    if tok is None:
        try:
            from tokenizers import Tokenizer

            tok = Tokenizer.from_pretrained(model)
            tok.no_truncation()
        except Exception:
            tok = None
    if tok is not None:
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


# The largest source file the sidecar will read for conversion / spreadsheet / image analysis
# (F-57). A file past this would balloon the child's memory (the reader materialises it) before the
# 64 MiB response-line cap could ever trip — an OOM on the 8 GB target. Rust pre-flights the same
# limit before it even asks; this is the in-process backstop. Kept in sync with sidecar.rs
# MAX_SIDECAR_INPUT_BYTES.
MAX_INPUT_FILE_BYTES = 128 * 1024 * 1024


def _guard_file_size(path):
    """Refuse an over-cap input file (F-57) with a clean error instead of OOMing the child. main()
    turns the raised ValueError into an `{ok: false, error}` reply the Rust side surfaces. A missing
    or unreadable file is NOT this guard's concern — it returns quietly so the handler's own path
    handling (e.g. analyze_image's graceful nulls) still applies."""
    import os

    try:
        size = os.path.getsize(path)
    except OSError:
        return
    if size > MAX_INPUT_FILE_BYTES:
        raise ValueError(
            f"file is too large to process ({size // (1024 * 1024)} MiB; "
            f"the limit is {MAX_INPUT_FILE_BYTES // (1024 * 1024)} MiB)"
        )


def do_convert(params):
    """Convert one file to Markdown. Returns its text and a best-effort title."""
    path = params["path"]
    _guard_file_size(path)
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


def get_ocr_engine():
    """The OPTIONAL on-device OCR engine (rapidocr), cached. Lazy-imported so the engine — and its
    image-processing deps (opencv/shapely/pyclipper) — only load when OCR is actually requested, and
    so the base sidecar runs without them installed. `RapidOCR` runs on the SAME onnxruntime that
    fastembed already ships and downloads its small detection/recognition ONNX models on first use
    (like the embedder/whisper). Raises ImportError if the optional component isn't installed — the
    Rust side only requests OCR once the install is confirmed, so that never fires in normal use.
    """
    global _ocr_engine
    if _ocr_engine is None:
        # rapidocr (3.x) prints model-download/init chatter; keep it off stdout so it can never
        # corrupt the newline-delimited JSON protocol (stdout is the reply channel).
        import contextlib

        with contextlib.redirect_stdout(sys.stderr):
            from rapidocr import RapidOCR

            _ocr_engine = RapidOCR()
    return _ocr_engine


def _open_image(path):
    """Open an image with Pillow, registering the HEIC opener if pillow-heif is available. Pillow is
    already present (a markitdown dependency); pillow-heif ships in the optional OCR component, so
    HEIC decoding needs that component installed — a non-HEIC image opens regardless.
    """
    global _heif_registered
    from PIL import Image

    if not _heif_registered:
        try:
            from pillow_heif import register_heif_opener

            register_heif_opener()
        except Exception:
            pass  # pillow-heif absent → HEIC won't open, every other format still does
        _heif_registered = True
    return Image.open(path)


def _gps_to_decimal(value, ref):
    """Convert an EXIF GPS coordinate (a (deg, min, sec) triple of rationals) + its hemisphere ref
    ('N'/'S'/'E'/'W') to a signed decimal degree, or None if absent/malformed.
    """
    if not value or ref is None:
        return None
    try:
        deg, minute, sec = (float(v) for v in value)
    except Exception:
        return None
    dec = deg + minute / 60.0 + sec / 3600.0
    if str(ref).strip().upper() in ("S", "W"):
        dec = -dec
    return round(dec, 6)


def _exif_meta(img):
    """Best-effort (capture_date, lat, lon) from an image's EXIF. capture_date is the EXIF
    DateTimeOriginal normalised to 'YYYY-MM-DD' (the GPS/Exif sub-IFDs are where phones keep these);
    any field absent → None, and the Rust side fills capture_date from the filename/ingest time.
    """
    capture_date = lat = lon = None
    try:
        exif = img.getexif()
    except Exception:
        return capture_date, lat, lon
    if not exif:
        return capture_date, lat, lon

    # DateTimeOriginal (0x9003) is in the Exif sub-IFD (0x8769); fall back to base DateTime (306).
    dt = None
    try:
        dt = exif.get_ifd(0x8769).get(0x9003)
    except Exception:
        pass
    dt = dt or exif.get(306)
    # EXIF stores "YYYY:MM:DD HH:MM:SS"; take the date and switch ':' to '-'. Guard against the
    # all-zero placeholder some cameras write.
    if isinstance(dt, str) and len(dt) >= 10 and dt[:4].isdigit() and dt[:4] != "0000":
        capture_date = dt[:10].replace(":", "-")

    try:
        gps = exif.get_ifd(0x8825)  # GPS IFD: 1=LatRef 2=Lat 3=LonRef 4=Lon
    except Exception:
        gps = None
    if gps:
        lat = _gps_to_decimal(gps.get(2), gps.get(1))
        lon = _gps_to_decimal(gps.get(4), gps.get(3))
    return capture_date, lat, lon


def _run_ocr(engine, img, path):
    """Recognise text in an image, returning the joined lines (newest rapidocr returns an object
    with a `.txts` tuple; tolerate the older list-of-[box,text,score] shape too). Pass a decoded RGB
    array when we already opened the image (HEIC works); else hand rapidocr the path directly.
    """
    import contextlib

    if img is not None:
        import numpy as np

        target = np.array(img.convert("RGB"))
    else:
        target = path  # let rapidocr read the file itself (non-HEIC)
    with contextlib.redirect_stdout(sys.stderr):
        result = engine(target)

    txts = getattr(result, "txts", None)
    if txts:
        return "\n".join(t for t in txts if t)
    # Older shape: ([[box, text, score], ...], elapse) or [[box, text, score], ...].
    rows = result
    if isinstance(result, tuple) and result and isinstance(result[0], (list, tuple)):
        rows = result[0]
    if isinstance(rows, (list, tuple)):
        out = [str(r[1]) for r in rows if isinstance(r, (list, tuple)) and len(r) >= 2]
        return "\n".join(out)
    return ""


def do_analyze_image(params):
    """Analyse one image for photo ingestion: EXIF capture metadata (always) + OCR text (only when
    `run_ocr`). The Rust side sets `run_ocr` from whether the optional component is installed, so
    a user who declined OCR still gets dimensions + EXIF (date/location) for the metadata chunk.
    OCR output is untrusted text — `clean_text`-ed like every other string we return. An unreadable
    image (e.g. HEIC without pillow-heif) yields nulls; Rust falls back to a filename/ingest date.
    """
    path = params["path"]
    _guard_file_size(path)
    run_ocr = bool(params.get("run_ocr", False))
    width = height = None
    capture_date = lat = lon = None
    img = None
    try:
        img = _open_image(path)
        width, height = img.size
        capture_date, lat, lon = _exif_meta(img)
    except Exception:
        pass  # unreadable / HEIC without codec — metadata stays null, OCR may still try the path

    ocr_text = ""
    ocr_ran = False
    if run_ocr:
        try:
            ocr_text = _run_ocr(get_ocr_engine(), img, path)
            ocr_ran = True
        except Exception as exc:
            # F-56: a broken/offline OCR component must degrade to EXIF-only, not fail the whole
            # photo ingest. The metadata gathered above is intact; Rust reads ocr_ran=false.
            sys.stderr.write(f"pm_sidecar: OCR failed for {path!r}, keeping EXIF only: {exc}\n")
            sys.stderr.flush()
            ocr_text = ""
            ocr_ran = False

    return {
        "ocr_text": clean_text(ocr_text),
        "ocr_ran": ocr_ran,
        "capture_date": capture_date,
        "lat": lat,
        "lon": lon,
        "width": width,
        "height": height,
    }


# ---- spreadsheet ingestion (dedicated processor; bypasses MarkItDown) --------
#
# .xlsx/.xls/.csv are routed here by Rust instead of through MarkItDown, which would
# flatten them into one Markdown pipe table the generic chunker then slices badly
# (rows lose their header context). We read VALUES ONLY — no formula evaluation, no
# styling — and hand Rust a per-sheet structure it shapes into a metadata chunk plus
# self-describing row chunks. Column types come from a lightweight try-parse heuristic
# (`infer_column_type`), deliberately NOT pandas dtype inference, which is unreliable
# on messy real-world CSVs. Cells are stringified for output so the Rust side receives
# plain text and never sees a Python type.

# Upper bound on rows returned PER SHEET. A sheet with more rows is truncated to this
# many (its `row_count` still reports the TRUE total and `truncated` is set), so a
# 200k-row sheet can't explode into 200k row chunks + embeddings. Tunable; composes
# with — does not replace — the byte-size cap Rust already enforces on a fetched body.
SPREADSHEET_ROW_CAP = 5000

# How many non-empty values per column the type heuristic samples — enough to be
# confident without scanning a whole column.
_TYPE_SAMPLE = 50

_DATE_FORMATS = (
    "%Y-%m-%d",
    "%Y/%m/%d",
    "%d-%m-%Y",
    "%d/%m/%Y",
    "%m/%d/%Y",
    "%Y-%m-%dT%H:%M:%S",
    "%Y-%m-%d %H:%M:%S",
)


def _parse_date(value):
    """A `datetime.date` if `value` is a date/datetime or a string in a common format, else None.
    Shared by column-type inference and the sheet's date range."""
    import datetime

    if isinstance(value, datetime.datetime):
        return value.date()
    if isinstance(value, datetime.date):
        return value
    if isinstance(value, str):
        s = value.strip()
        for fmt in _DATE_FORMATS:
            try:
                return datetime.datetime.strptime(s, fmt).date()
            except ValueError:
                continue
    return None


def _cell_type(value):
    """Classify one cell into 'int' | 'float' | 'date' | 'bool' | 'string', or None when empty.
    Native types from openpyxl/xlrd (int/float/datetime/bool) are honoured directly; a string (all
    a CSV yields) is try-parsed. Empty/None returns None so it never sways the column's type."""
    import datetime

    if value is None:
        return None
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, int):
        return "int"
    if isinstance(value, float):
        return "float"
    if isinstance(value, (datetime.date, datetime.datetime)):
        return "date"
    s = str(value).strip()
    if not s:
        return None
    if s.lower() in ("true", "false", "yes", "no"):
        return "bool"
    try:
        int(s)
        return "int"
    except ValueError:
        pass
    try:
        float(s)
        return "float"
    except ValueError:
        pass
    if _parse_date(s) is not None:
        return "date"
    return "string"


def infer_column_type(values):
    """The dominant inferred type over a column's non-empty values — 'int' | 'float' | 'date' |
    'bool' | 'string' | 'empty'. Standalone and pure: it takes raw cell values and returns a label
    with NO coupling to how chunks are emitted, so a later card can reuse it to detect Status/Date-
    like columns for a different purpose. A numeric column mixing ints and floats reads as 'float';
    a genuinely heterogeneous column reads as 'string' (the safe catch-all)."""
    seen = set()
    for v in values:
        t = _cell_type(v)
        if t is not None:
            seen.add(t)
    if not seen:
        return "empty"
    if len(seen) == 1:
        return next(iter(seen))
    if seen <= {"int", "float"}:
        return "float"
    return "string"


def inspect_columns(headers, rows):
    """A standalone schema descriptor for a sheet: one {name, inferred_type} per column, inferred
    from a sample of each column's values. Decoupled from chunk emission (that lives in Rust), so
    this is the single reusable place column names + types are derived — a later card reuses it."""
    columns = []
    for i, name in enumerate(headers):
        sample = []
        for row in rows:
            if i < len(row) and row[i] is not None and str(row[i]).strip():
                sample.append(row[i])
                if len(sample) >= _TYPE_SAMPLE:
                    break
        columns.append({"name": clean_text(name), "inferred_type": infer_column_type(sample)})
    return columns


def _date_range(rows, columns):
    """[min, max] 'YYYY-MM-DD' over the first date-typed column, or None if no column is a date."""
    idx = next((i for i, c in enumerate(columns) if c["inferred_type"] == "date"), None)
    if idx is None:
        return None
    dates = []
    for row in rows:
        if idx < len(row):
            d = _parse_date(row[idx])
            if d is not None:
                dates.append(d)
    if not dates:
        return None
    return [min(dates).isoformat(), max(dates).isoformat()]


def _build_sheet(name, rows_iter):
    """Consume one sheet's rows (lazily — the caller keeps the file/workbook open): the first row is
    the header, the rest are data. Keeps up to SPREADSHEET_ROW_CAP data rows but counts the TRUE
    total, so a huge sheet reports honestly and is flagged `truncated`. Returns the per-sheet dict
    Rust consumes, or None for an empty sheet (no header row)."""
    it = iter(rows_iter)
    try:
        header_row = next(it)
    except StopIteration:
        return None
    headers = [clean_text("" if c is None else str(c)) for c in header_row]

    kept = []  # native cell rows (capped) — inferred on before stringifying, for accurate types
    total = 0
    for row in it:
        total += 1
        if len(kept) < SPREADSHEET_ROW_CAP:
            kept.append(list(row))

    columns = inspect_columns(headers, kept)
    date_range = _date_range(kept, columns)
    str_rows = [[("" if c is None else clean_text(str(c))) for c in row] for row in kept]
    return {
        "name": clean_text(name),
        "headers": headers,
        "row_count": total,
        "inferred_types": [c["inferred_type"] for c in columns],
        "date_range": date_range,
        "rows": str_rows,
        "truncated": total > len(kept),
    }


def _csv_encoding(path):
    """Pick a text encoding for a CSV: UTF-8 (BOM-aware) if the head decodes, else cp1252 — Excel's
    Windows default (F-55). Prevents a non-UTF-8 export from failing the whole ingest with a
    UnicodeDecodeError. The caller opens with errors='replace' so a stray byte past the sampled head
    still degrades to a replacement char rather than raising mid-parse."""
    with open(path, "rb") as fh:
        head = fh.read(65536)
    try:
        head.decode("utf-8-sig")
        return "utf-8-sig"
    except UnicodeDecodeError:
        return "cp1252"


def _sheets_from_csv(path):
    """A CSV is a single sheet named after the file; delimiter is sniffed (falls back to comma).
    Encoding is UTF-8 with a cp1252 fallback (F-55) so an Excel export doesn't crash ingest."""
    import csv
    import os

    fh = open(path, "r", encoding=_csv_encoding(path), errors="replace", newline="")
    try:
        sample = fh.read(65536)
        fh.seek(0)
        try:
            dialect = csv.Sniffer().sniff(sample, delimiters=",;\t|")
        except csv.Error:
            dialect = csv.excel
        name = os.path.splitext(os.path.basename(path))[0]
        sheet = _build_sheet(name, csv.reader(fh, dialect))
        return [sheet] if sheet else []
    finally:
        fh.close()


def _sheets_from_xlsx(path):
    """Every worksheet in an .xlsx/.xlsm. `data_only=True` returns cached VALUES (never formulas);
    `read_only=True` streams rows so a large workbook stays memory-bounded."""
    from openpyxl import load_workbook

    wb = load_workbook(path, read_only=True, data_only=True)
    try:
        out = []
        for ws in wb.worksheets:
            sheet = _build_sheet(ws.title, ws.iter_rows(values_only=True))
            if sheet:
                out.append(sheet)
        return out
    finally:
        wb.close()


def _sheets_from_xls(path):
    """Every sheet in a legacy .xls via xlrd. Date cells are Excel serial numbers, so convert the
    ones xlrd flags as dates back to datetimes (values only — no formula/style extraction)."""
    import xlrd

    book = xlrd.open_workbook(path)
    out = []
    for sh in book.sheets():

        def _rows(sh=sh, book=book):
            for r in range(sh.nrows):
                cells = []
                for c in range(sh.ncols):
                    val = sh.cell_value(r, c)
                    if sh.cell_type(r, c) == xlrd.XL_CELL_DATE:
                        try:
                            val = xlrd.xldate_as_datetime(val, book.datemode)
                        except Exception:
                            pass
                    cells.append(val)
                yield cells

        sheet = _build_sheet(sh.name, _rows())
        if sheet:
            out.append(sheet)
    return out


def do_analyze_spreadsheet(params):
    """Parse a spreadsheet (.xlsx/.xls/.csv) into per-sheet structure for the dedicated ingest path,
    bypassing MarkItDown. Reads VALUES ONLY — no formula evaluation, no styling. Each sheet reports
    its headers, TRUE row_count, per-column inferred types, an optional date range, and up to
    SPREADSHEET_ROW_CAP stringified rows (`truncated` set when the sheet had more). Rust shapes
    these into a metadata chunk + self-describing row chunks. Cell text is untrusted data — cleaned,
    never executed."""
    path = params["path"]
    _guard_file_size(path)
    ext = (params.get("ext") or "").lower().lstrip(".")
    if ext in ("xlsx", "xlsm"):
        sheets = _sheets_from_xlsx(path)
    elif ext == "xls":
        sheets = _sheets_from_xls(path)
    elif ext == "csv":
        sheets = _sheets_from_csv(path)
    else:
        raise ValueError(f"unsupported spreadsheet extension: {ext!r}")
    return {"sheets": sheets, "row_cap": SPREADSHEET_ROW_CAP}


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
    "analyze_image": do_analyze_image,
    "analyze_spreadsheet": do_analyze_spreadsheet,
}


# Mirror the Rust reader's per-line cap. Rust is the trusted sender and never
# sends anything near this, so a line this large can only be a fault or abuse —
# drop it rather than hand it to json.loads. Env-overridable ONLY so the protocol
# test can exercise the drop path without piping 64 MiB; Rust never sets it, so
# production always runs the full cap.
MAX_LINE_CHARS = int(os.environ.get("PM_SIDECAR_MAX_LINE_CHARS", 64 * 1024 * 1024))


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
