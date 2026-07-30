# SPDX-FileCopyrightText: 2026 Bobby Yu
# SPDX-License-Identifier: AGPL-3.0-or-later

"""Regression tests for the sidecar: the token counter and the stdio protocol loop.

The bug these lock down: fastembed's tokenizer has batch padding enabled (it pads
every text to the batch's longest), and it is the SAME instance used for
embedding, so the counter must not disable padding. `do_count_tokens` therefore
counts real tokens via the attention mask. If it ever regresses to `len(ids)`, a
mixed-length batch reports every text as the longest one's length, and the Rust
splitter then sizes every block by the document's largest block and shatters long
documents into tiny chunks. See `pm_sidecar.do_count_tokens`.

The primary test is model-free (a fake pad-to-longest tokenizer), so it runs
anywhere on the standard library alone — including CI, where fastembed is not
installed. A second test exercises the real cached bge-small tokenizer and is
skipped when fastembed is unavailable (it never downloads).

Run: `python sidecar/test_pm_sidecar.py` (or `just sidecar-test`).
"""

import importlib.util
import json
import os
import subprocess
import sys
import unittest
from unittest import mock

import pm_sidecar as S


class _FakeEncoding:
    def __init__(self, ids, attention_mask):
        self.ids = ids
        self.attention_mask = attention_mask


class _PadToLongestTokenizer:
    """Mimics fastembed's tokenizer: `encode_batch` pads to the batch's longest, so
    `len(ids)` over-reports while `attention_mask` still marks the real tokens. One
    whitespace word == one token, so the true lengths are obvious and deterministic.
    """

    @staticmethod
    def _len(text):
        return max(1, len(text.split()))

    def encode(self, text):
        n = self._len(text)
        return _FakeEncoding(list(range(n)), [1] * n)

    def encode_batch(self, texts):
        lengths = [self._len(t) for t in texts]
        width = max(lengths) if lengths else 1
        out = []
        for n in lengths:
            pad = width - n
            out.append(_FakeEncoding(list(range(n)) + [0] * pad, [1] * n + [0] * pad))
        return out


class CountTokensTest(unittest.TestCase):
    def test_padded_batch_reports_true_per_text_lengths(self):
        # Texts of clearly different true lengths, counted in one batch.
        texts = ["one", "one two three", " ".join(["w"] * 40)]
        with mock.patch.object(S, "get_tokenizer", return_value=_PadToLongestTokenizer()):
            counts = S.do_count_tokens({"texts": texts})["counts"]
        # Each text's TRUE length — NOT all equal to the batch max (the padding bug).
        self.assertEqual(counts, [1, 3, 40])
        self.assertNotEqual(counts[0], max(counts))
        self.assertEqual(len(set(counts)), 3)

    def test_falls_back_to_char_estimate_without_a_tokenizer(self):
        with mock.patch.object(S, "get_tokenizer", return_value=None):
            counts = S.do_count_tokens({"texts": ["", "a" * 40]})["counts"]
        self.assertEqual(counts, [1, 10])

    def test_a_truncating_shared_tokenizer_is_never_used_for_counting(self):
        # When the independent clone can't be made, the fallback used to be the SHARED tokenizer —
        # which has truncation on. Every chunk past the model's window then reports exactly the
        # window size, so the splitter reads "512, it fits" for a block that does not and stops
        # splitting the very chunks the counter exists to catch. A rough estimate that keeps
        # growing beats an exact number that lies.
        import types

        fake = types.ModuleType("tokenizers")

        class Tokenizer:
            @staticmethod
            def from_str(_s):
                raise RuntimeError("clone unavailable")

            @staticmethod
            def from_pretrained(_m):
                raise RuntimeError("not cached")

        fake.Tokenizer = Tokenizer

        class _SharedTruncating:
            def encode(self, text):
                return _FakeEncoding([0] * 512, [1] * 512)

            def to_str(self):
                return "{}"

        shared = _SharedTruncating()

        class _Emb:
            tokenizer = shared

        with (
            mock.patch.dict(sys.modules, {"tokenizers": fake}),
            mock.patch.object(S, "get_embedder", return_value=_Emb()),
            mock.patch.dict(S._tokenizers, {}, clear=True),
        ):
            self.assertIsNone(S.get_tokenizer("some-model"))

    @unittest.skipUnless(
        importlib.util.find_spec("fastembed") is not None,
        "fastembed not installed (real-tokenizer counting is a local-only check)",
    )
    def test_real_bge_tokenizer_mixed_batch_is_not_inflated(self):
        texts = ["Hi.", "the quarterly revenue rose across regions", " ".join(["lorem"] * 80)]
        counts = S.do_count_tokens({"texts": texts})["counts"]
        # The batch counts must equal each text encoded on its own (the ground truth).
        truth = [S.do_count_tokens({"texts": [t]})["counts"][0] for t in texts]
        self.assertEqual(counts, truth)
        self.assertNotEqual(counts[0], max(counts))


class EmbedTest(unittest.TestCase):
    """Embedding must survive a chunk longer than the model's 512-token window. Regression for
    the get_tokenizer bug: disabling truncation for COUNTING also disabled it on the SAME
    tokenizer fastembed embeds with, so an over-window chunk crashed ONNX (a 512-vs-N broadcast)
    instead of being truncated. Real-model, so local-only (skipped where fastembed is absent)."""

    @unittest.skipUnless(
        importlib.util.find_spec("fastembed") is not None,
        "fastembed not installed (real embedding is a local-only check)",
    )
    def test_over_window_chunk_embeds_without_crashing(self):
        # ~750 words — comfortably past the 512-token window. Counting first caches the cloned
        # counting tokenizer; the bug would have stripped the embedder's own truncation, so this
        # embed would raise instead of truncating to a single 384-d vector.
        long_text = "lorem ipsum dolor sit amet " * 150
        S.do_count_tokens({"texts": [long_text]})
        out = S.do_embed({"texts": [long_text]})
        self.assertEqual(len(out["vectors"]), 1)
        self.assertEqual(len(out["vectors"][0]), S.EMBED_DIM)


class ReduceTest(unittest.TestCase):
    """The 2-D reducer for the semantic memory map. The PCA path (the bundled default) needs only
    numpy, so these run wherever numpy is present and skip otherwise (CI is model-free)."""

    def test_empty_and_tiny_inputs(self):
        # No numpy needed for the degenerate guards.
        self.assertEqual(S.do_reduce({"vectors": []}), {"coords": [], "method": "none"})
        out = S.do_reduce({"vectors": [[1.0, 2.0], [3.0, 4.0]], "method": "pca"})
        self.assertEqual(out["method"], "trivial")  # n <= 3 short-circuits before any reducer
        self.assertEqual(len(out["coords"]), 2)

    @unittest.skipUnless(
        importlib.util.find_spec("numpy") is not None,
        "numpy not installed (the PCA reducer is a local-only check)",
    )
    def test_pca_projects_into_unit_square(self):
        # Two clusters in 4-d → PCA to 2-d. One point per row; coords must be scaled into [0,1]^2.
        vectors = [
            [0.0, 0.0, 0.0, 0.0],
            [0.1, 0.0, 0.1, 0.0],
            [0.0, 0.1, 0.0, 0.1],
            [9.0, 9.0, 9.0, 9.0],
            [9.1, 9.0, 9.1, 9.0],
            [9.0, 9.1, 9.0, 9.1],
        ]
        out = S.do_reduce({"vectors": vectors, "method": "pca"})
        self.assertEqual(out["method"], "pca")
        self.assertEqual(len(out["coords"]), len(vectors))
        for x, y in out["coords"]:
            self.assertGreaterEqual(x, 0.0)
            self.assertLessEqual(x, 1.0)
            self.assertGreaterEqual(y, 0.0)
            self.assertLessEqual(y, 1.0)
        # An unknown/absent t-SNE install falls back to PCA, never an empty map.
        fallback = S.do_reduce({"vectors": vectors, "method": "tsne"})
        self.assertEqual(len(fallback["coords"]), len(vectors))
        self.assertIn(fallback["method"], ("tsne", "pca"))


class _OcrResultObj:
    """Mimics rapidocr 3.x's output object: recognised lines on a `.txts` tuple."""

    def __init__(self, txts):
        self.txts = txts


class _FakeOcrEngine:
    """A callable standing in for rapidocr — returns whatever shape the test wants, no models."""

    def __init__(self, result):
        self._result = result

    def __call__(self, _target):
        return self._result


class AnalyzeImageTest(unittest.TestCase):
    """Photo analysis: EXIF GPS/date parsing (pure), OCR result-shape tolerance (fake engine), and
    the EXIF-only path when OCR is declined. The OCR engine and HEIC codec are OPTIONAL components,
    so nothing here installs or downloads anything."""

    def test_gps_to_decimal_signs_and_malformed(self):
        # 55°57'00"N, 3°11'24"W (Edinburgh-ish). N/E positive, S/W negated.
        self.assertAlmostEqual(S._gps_to_decimal((55, 57, 0), "N"), 55.95, places=4)
        self.assertAlmostEqual(S._gps_to_decimal((3, 11, 24), "W"), -3.19, places=4)
        self.assertAlmostEqual(S._gps_to_decimal((10, 30, 0), "S"), -10.5, places=4)
        # Absent or malformed → None, never a crash.
        self.assertIsNone(S._gps_to_decimal(None, "N"))
        self.assertIsNone(S._gps_to_decimal((1, 2), "N"))
        self.assertIsNone(S._gps_to_decimal((1, 2, 3), None))

    def test_gps_non_finite_is_no_location_not_a_nan(self):
        """A GPS rational with a zero denominator makes float() produce nan/inf. A non-finite float
        serializes as bare `NaN`/`Infinity` — not valid JSON — so the Rust reader skips the reply
        line and blocks on an answer that never comes, wedging the WHOLE serialized sidecar (ingest,
        retrieval, rerank, transcribe, map) until the per-method timeout expires. One photo. No
        location is the honest answer."""
        self.assertIsNone(S._gps_to_decimal((float("nan"), 0, 0), "N"))
        self.assertIsNone(S._gps_to_decimal((float("inf"), 0, 0), "N"))
        # The negation runs before the check, so a non-finite southern value is caught too.
        self.assertIsNone(S._gps_to_decimal((float("inf"), 0, 0), "S"))

    def test_a_non_finite_result_fails_one_request_not_the_engine(self):
        """The structural guard behind the above: whatever any handler returns, a non-finite number
        must never reach the wire as bare `NaN`. This is what makes the fix hold for handlers that
        don't exist yet."""
        with self.assertRaises(ValueError):
            json.dumps({"result": float("nan")}, allow_nan=False)
        # And the error reply PM sends instead is itself valid JSON.
        fallback = json.dumps({"id": 1, "ok": False, "error": "non-finite number in result"})
        self.assertEqual(json.loads(fallback)["ok"], False)

    def test_run_ocr_parses_object_and_list_shapes(self):
        # New rapidocr: object with a .txts tuple.
        obj = _OcrResultObj(("hello", "world"))
        self.assertEqual(S._run_ocr(_FakeOcrEngine(obj), None, "x.png"), "hello\nworld")
        # Older rapidocr: ([[box, text, score], ...], elapse).
        old = ([[[0, 0], "line one", 0.9], [[1, 1], "line two", 0.8]], 0.1)
        self.assertEqual(S._run_ocr(_FakeOcrEngine(old), None, "x.png"), "line one\nline two")
        # Nothing recognised → empty string, not an error.
        self.assertEqual(S._run_ocr(_FakeOcrEngine(_OcrResultObj(())), None, "x.png"), "")

    @unittest.skipUnless(
        importlib.util.find_spec("PIL") is not None,
        "Pillow not installed (image decode is a local-only check)",
    )
    def test_analyze_image_exif_only_when_ocr_declined(self):
        import tempfile

        from PIL import Image

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/shot.png"
            Image.new("RGB", (24, 16), (10, 20, 30)).save(path)
            out = S.do_analyze_image({"path": path, "run_ocr": False})
        # OCR was declined: no engine touched, no text, but dimensions still read.
        self.assertEqual(out["ocr_ran"], False)
        self.assertEqual(out["ocr_text"], "")
        self.assertEqual((out["width"], out["height"]), (24, 16))
        # A plain generated PNG has no EXIF → date/location null (Rust supplies the fallback date).
        self.assertIsNone(out["capture_date"])
        self.assertIsNone(out["lat"])

    def test_analyze_image_unreadable_path_is_null_not_crash(self):
        out = S.do_analyze_image({"path": "/no/such/image.png", "run_ocr": False})
        self.assertEqual(out["ocr_ran"], False)
        self.assertIsNone(out["width"])
        self.assertIsNone(out["capture_date"])

    @unittest.skipUnless(
        importlib.util.find_spec("PIL") is not None,
        "Pillow not installed (image decode is a local-only check)",
    )
    def test_broken_ocr_degrades_to_exif_only(self):
        # F-56: if the OCR component is broken/offline (engine load or call raises), photo ingest
        # must still return dimensions/EXIF with ocr_ran=false, not fail the whole file.
        import tempfile

        from PIL import Image

        def _boom():
            raise RuntimeError("rapidocr models missing")

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/shot.png"
            Image.new("RGB", (12, 8), (1, 2, 3)).save(path)
            with mock.patch.object(S, "get_ocr_engine", _boom):
                out = S.do_analyze_image({"path": path, "run_ocr": True})
        self.assertEqual(out["ocr_ran"], False, "a broken OCR engine degrades, not crashes")
        self.assertEqual(out["ocr_text"], "")
        self.assertEqual((out["width"], out["height"]), (12, 8), "dimensions still read")

    def test_a_cold_model_cache_is_not_swallowed_as_exif_only(self):
        # The F-56 guard above degrades a BROKEN component, which is right. A cold model cache is
        # not that: swallowing it commits a photo that claims to hold no text and never looks
        # again. It has to escape the handler so main() reports a miss, Rust runs the fetcher, and
        # the retry actually reads the receipt.
        def _cold():
            raise S.ModelNotCached("rapidocr models are not downloaded yet")

        with mock.patch.object(S, "get_ocr_engine", _cold):
            with self.assertRaises(S.ModelNotCached):
                S.do_analyze_image({"path": "/no/such/image.png", "run_ocr": True})


def _fake_rapidocr(captured, model_file="/pm/models/rapidocr/det.onnx"):
    """A stand-in `rapidocr` package: enough of the real shape to pin the offline contract.

    Two details are load-bearing and mirrored exactly from rapidocr 3.9.0:

    1. the engine holds its OWN reference to the downloader class (`from ... import DownloadFile`),
       so patching the class attribute has to reach it; and
    2. the engine calls `DownloadFile.run` UNCONDITIONALLY — the "it's already downloaded" check
       lives inside `run`, not around the call. A guard that simply refuses would therefore break
       the warm cache as thoroughly as the cold one.
    """
    import types

    pkg = types.ModuleType("rapidocr")
    utils = types.ModuleType("rapidocr.utils")
    dl = types.ModuleType("rapidocr.utils.download_file")

    class DownloadFileInput:
        def __init__(self, save_path):
            self.save_path = save_path

    class DownloadFile:
        @classmethod
        def run(cls, params):
            captured.setdefault("downloads", []).append(params.save_path)

    class RapidOCR:
        def __init__(self, params=None):
            captured["params"] = params
            DownloadFile.run(DownloadFileInput(model_file))

    dl.DownloadFile = DownloadFile
    dl.DownloadFileInput = DownloadFileInput
    utils.download_file = dl
    pkg.utils = utils
    pkg.RapidOCR = RapidOCR
    return {
        "rapidocr": pkg,
        "rapidocr.utils": utils,
        "rapidocr.utils.download_file": dl,
    }


class OcrOfflineContractTest(unittest.TestCase):
    """H5: rapidocr is the one model that does NOT fetch through huggingface_hub, so the offline
    flags every other loader honours do not reach it. Under the no-network confinement that means
    it could never obtain its models at all — the constructor raised on every photo, the F-56 guard
    swallowed it, and OCR was permanently and silently off. These pin the contract that fixes it.
    """

    def _engine(self, captured, offline, models_dir="/pm/models", model_file=None):
        mods = _fake_rapidocr(captured, model_file) if model_file else _fake_rapidocr(captured)
        with (
            mock.patch.dict(sys.modules, mods),
            mock.patch.object(S, "_ocr_engine", None),
            mock.patch.object(S, "_OFFLINE", offline),
            mock.patch.object(S, "_MODELS_DIR", models_dir),
        ):
            return S.get_ocr_engine()

    def test_the_engine_is_pinned_to_the_shared_model_dir(self):
        # Left to itself rapidocr caches inside site-packages, so removing the optional OCR
        # component (a one-click Settings action) or rebuilding the venv throws the weights away.
        captured = {}
        self._engine(captured, offline=False)
        self.assertEqual(
            captured["params"],
            {"Global.model_root_dir": os.path.join("/pm/models", "rapidocr")},
        )

    def test_offline_a_cold_cache_is_a_model_miss_not_an_outbound_socket(self):
        # The worker parses untrusted file bytes. It must never open a socket to fetch a model —
        # which is exactly what an unguarded rapidocr does the moment the sandbox falls open.
        captured = {}
        with self.assertRaises(S.ModelNotCached):
            self._engine(captured, offline=True)
        self.assertNotIn("downloads", captured, "nothing may reach rapidocr's downloader offline")

    def test_offline_a_warm_cache_still_builds_the_engine(self):
        # The retry after the fetch is the whole point, and it runs in the SAME offline worker. The
        # engine calls the downloader unconditionally and lets IT decide whether the file is
        # already there — so a guard that simply refuses rejects the warm cache exactly like the
        # cold one, the retry fails as hard as the first attempt, and OCR stays off for good.
        import tempfile

        captured = {}
        with tempfile.TemporaryDirectory() as d:
            present = os.path.join(d, "det.onnx")
            with open(present, "wb") as fh:
                fh.write(b"onnx")
            engine = self._engine(captured, offline=True, model_file=present)
        self.assertIsNotNone(engine)
        self.assertNotIn("downloads", captured, "a warm cache still needs no network")

    def test_the_fetcher_may_download(self):
        # The mirror image: the short-lived --fetch helper runs WITHOUT the offline flag and is the
        # one child allowed a socket, so the same call must go through.
        captured = {}
        self._engine(captured, offline=False)
        self.assertEqual(len(captured["downloads"]), 1)

    def test_ensure_model_knows_how_to_fetch_for_analyze_image(self):
        # Without this branch the fetcher raised "nothing to fetch for method 'analyze_image'", so
        # the miss above could never be satisfied and the retry never happened.
        captured = {}
        with (
            mock.patch.dict(sys.modules, _fake_rapidocr(captured)),
            mock.patch.object(S, "_ocr_engine", None),
            mock.patch.object(S, "_OFFLINE", False),
            mock.patch.object(S, "_MODELS_DIR", "/pm/models"),
        ):
            # No path in the params: Rust strips them, and nothing here may touch the image.
            S._ensure_model("analyze_image", {})
        self.assertEqual(len(captured["downloads"]), 1)


class SpreadsheetTest(unittest.TestCase):
    """The dedicated spreadsheet processor that bypasses MarkItDown. The type heuristic and the CSV
    path are pure/stdlib, so they run in CI; the .xlsx reader needs openpyxl and is skipped where it
    isn't installed (it never downloads anything). Legacy .xls is no longer supported."""

    def test_infer_column_type_heuristic(self):
        self.assertEqual(S.infer_column_type(["1", "2", "3"]), "int")
        self.assertEqual(S.infer_column_type(["1.5", "2", "3"]), "float")  # mixed numeric → float
        self.assertEqual(S.infer_column_type(["2026-01-02", "2026-03-04"]), "date")
        self.assertEqual(S.infer_column_type(["true", "no", "Yes"]), "bool")
        self.assertEqual(
            S.infer_column_type(["Atlas", "42", "2026-01-01"]), "string"
        )  # heterogeneous
        self.assertEqual(S.infer_column_type(["", "  ", None]), "empty")

    def test_inspect_columns_is_decoupled_schema(self):
        headers = ["Project", "Amount", "Due"]
        rows = [["Atlas", "1200", "2026-03-01"], ["Beacon", "800", "2026-04-15"]]
        cols = S.inspect_columns(headers, rows)
        self.assertEqual([c["name"] for c in cols], ["Project", "Amount", "Due"])
        self.assertEqual([c["inferred_type"] for c in cols], ["string", "int", "date"])

    def test_analyze_csv_metadata_and_rows(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/budget.csv"
            with open(path, "w", encoding="utf-8", newline="") as fh:
                fh.write("Project,Amount,Due\nAtlas,1200,2026-03-01\nBeacon,800,2026-04-15\n")
            out = S.do_analyze_spreadsheet({"path": path, "ext": "csv"})
        self.assertEqual(len(out["sheets"]), 1)
        sheet = out["sheets"][0]
        self.assertEqual(sheet["name"], "budget")  # CSV sheet is named after the file
        self.assertEqual(sheet["headers"], ["Project", "Amount", "Due"])
        self.assertEqual(sheet["row_count"], 2)
        self.assertEqual(sheet["inferred_types"], ["string", "int", "date"])
        self.assertEqual(sheet["date_range"], ["2026-03-01", "2026-04-15"])
        self.assertFalse(sheet["truncated"])
        self.assertEqual(sheet["rows"][0], ["Atlas", "1200", "2026-03-01"])

    def test_row_cap_truncates_but_reports_true_total(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/big.csv"
            with open(path, "w", encoding="utf-8", newline="") as fh:
                fh.write("Name,N\n")
                for i in range(5):
                    fh.write(f"row{i},{i}\n")
            with mock.patch.object(S, "SPREADSHEET_ROW_CAP", 3):
                out = S.do_analyze_spreadsheet({"path": path, "ext": "csv"})
        sheet = out["sheets"][0]
        self.assertEqual(sheet["row_count"], 5)  # TRUE total, not the cap
        self.assertEqual(len(sheet["rows"]), 3)  # capped
        self.assertTrue(sheet["truncated"])

    def test_semicolon_delimiter_is_sniffed(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/semi.csv"
            with open(path, "w", encoding="utf-8", newline="") as fh:
                fh.write("A;B\n1;2\n3;4\n")
            out = S.do_analyze_spreadsheet({"path": path, "ext": "csv"})
        sheet = out["sheets"][0]
        self.assertEqual(sheet["headers"], ["A", "B"])
        self.assertEqual(sheet["rows"], [["1", "2"], ["3", "4"]])

    def test_cp1252_csv_does_not_crash_ingest(self):
        # F-55: an Excel export saved as Windows-1252 (é = 0xE9) must not fail the whole ingest
        # with a UnicodeDecodeError — it falls back to cp1252 and decodes the accented text.
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/latin1.csv"
            with open(path, "w", encoding="cp1252", newline="") as fh:
                fh.write("Name,Note\nRené,café\n")
            out = S.do_analyze_spreadsheet({"path": path, "ext": "csv"})
        sheet = out["sheets"][0]
        self.assertEqual(sheet["headers"], ["Name", "Note"])
        self.assertEqual(sheet["rows"][0], ["René", "café"])

    def test_oversized_input_is_refused(self):
        # F-57: a file past the cap is refused with a clear error, not an OOM. Shrink the cap so a
        # tiny file trips it; do_analyze_spreadsheet raises, which main() turns into {ok: false}.
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/big.csv"
            with open(path, "w", encoding="utf-8", newline="") as fh:
                fh.write("A,B\n1,2\n")
            with mock.patch.object(S, "MAX_INPUT_FILE_BYTES", 4):
                with self.assertRaises(ValueError):
                    S.do_analyze_spreadsheet({"path": path, "ext": "csv"})

    @unittest.skipUnless(
        importlib.util.find_spec("openpyxl") is not None,
        "openpyxl not installed (.xlsx reading is a local-only check)",
    )
    def test_analyze_xlsx_multi_sheet(self):
        import tempfile

        from openpyxl import Workbook

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/book.xlsx"
            wb = Workbook()
            ws1 = wb.active
            ws1.title = "Budget"
            ws1.append(["Project", "Amount"])
            ws1.append(["Atlas", 1200])
            ws2 = wb.create_sheet("Team")
            ws2.append(["Name"])
            ws2.append(["Alex"])
            wb.save(path)
            out = S.do_analyze_spreadsheet({"path": path, "ext": "xlsx"})
        self.assertEqual([s["name"] for s in out["sheets"]], ["Budget", "Team"])
        self.assertEqual(out["sheets"][0]["inferred_types"], ["string", "int"])

    def test_xlsx_inflation_guard_rejects_a_zip_bomb(self):
        # Pure stdlib (no openpyxl), so it runs in CI: a highly compressible zip member has an
        # inflation ratio far above the cap and must be refused before openpyxl reads it (H-1).
        import tempfile
        import zipfile

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/bomb.xlsx"
            with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as zf:
                # ~4 MiB of zeros deflates to a few KiB — a ratio far above the 200x limit.
                zf.writestr("xl/sharedStrings.xml", b"\0" * (4 * 1024 * 1024))
            with self.assertRaises(ValueError):
                S._guard_archive_inflation(path)

    def test_the_bomb_guard_also_covers_the_documents_markitdown_reads(self):
        # The guard used to be .xlsx-only, which left .docx / .pptx / .epub — every one of them a
        # zip, every one handed straight to MarkItDown — with nothing but the on-disk size cap. The
        # refusal must happen in do_convert, BEFORE MarkItDown is even imported (which is also what
        # lets this run without markitdown installed).
        import tempfile
        import zipfile

        with tempfile.TemporaryDirectory() as d:
            for name in ("bomb.docx", "bomb.pptx", "bomb.epub"):
                path = f"{d}/{name}"
                with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as zf:
                    zf.writestr("word/document.xml", b"\0" * (4 * 1024 * 1024))
                with self.assertRaises(ValueError, msg=name):
                    S.do_convert({"path": path})

    def test_a_wide_sheet_is_column_capped_and_says_so(self):
        # The row cap bounds one dimension only: a sheet 4,000 columns wide clears it and still
        # produces a reply the reader must reject. The kept width is capped, the TRUE width is
        # still reported, and cols_truncated makes Rust's overview say so out loud.
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/wide.csv"
            with open(path, "w", encoding="utf-8", newline="") as fh:
                fh.write(",".join(f"c{i}" for i in range(10)) + "\n")
                fh.write(",".join(str(i) for i in range(10)) + "\n")
            with mock.patch.object(S, "SPREADSHEET_COL_CAP", 4):
                out = S.do_analyze_spreadsheet({"path": path, "ext": "csv"})
        sheet = out["sheets"][0]
        self.assertEqual(sheet["col_count"], 10)  # TRUE width
        self.assertEqual(sheet["headers"], ["c0", "c1", "c2", "c3"])  # capped
        self.assertEqual(sheet["rows"][0], ["0", "1", "2", "3"])
        self.assertTrue(sheet["cols_truncated"])

    def test_an_unbounded_sheet_is_stopped_by_the_reply_budget(self):
        # The row and column caps bound each sheet's SHAPE; their product is still three orders of
        # magnitude past the reader's 64 MiB line cap. The budget is the bound that actually holds
        # — and a sheet it stops must report itself truncated, exactly as a row-capped one does.
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/fat.csv"
            with open(path, "w", encoding="utf-8", newline="") as fh:
                fh.write("Note\n")
                for _ in range(20):
                    fh.write("x" * 10 + "\n")
            with mock.patch.object(S, "SPREADSHEET_TEXT_BUDGET", 35):
                out = S.do_analyze_spreadsheet({"path": path, "ext": "csv"})
        sheet = out["sheets"][0]
        self.assertEqual(sheet["row_count"], 20)  # TRUE total, counted past the budget
        self.assertEqual(len(sheet["rows"]), 3)  # 3 x 10 chars fits 35; the 4th does not
        self.assertTrue(sheet["truncated"])

    def test_the_budget_is_shared_across_a_workbooks_sheets(self):
        # A workbook can hold any number of sheets, so a per-sheet budget would multiply. Once the
        # pot is empty the next sheet keeps NO rows at all, and still reports its true row count.
        budget = S._TextBudget(12)
        first = S._build_sheet("one", [["H"], ["aaaaaaaaaa"], ["bbbbbbbbbb"]], budget)
        second = S._build_sheet("two", [["H"], ["cccccccccc"]], budget)
        self.assertEqual(len(first["rows"]), 1)
        self.assertTrue(first["truncated"])
        self.assertEqual(second["rows"], [])
        self.assertEqual(second["row_count"], 1)
        self.assertTrue(second["truncated"])

    def test_one_huge_cell_cannot_carry_a_row_past_the_cell_cap(self):
        sheet = S._build_sheet(
            "s", [["Note"], ["y" * 5000]], S._TextBudget(S.SPREADSHEET_TEXT_BUDGET)
        )
        self.assertEqual(len(sheet["rows"][0][0]), S.SPREADSHEET_CELL_CHARS)


# --- the protocol loop itself (driven as a real subprocess) ---------------

_SIDECAR_PATH = S.__file__


class ProtocolTest(unittest.TestCase):
    """Drive main() as a real subprocess over its newline-JSON stdio contract.

    The per-handler tests above call do_* directly; nothing exercises the loop —
    JSON-line framing, the ok/error envelope, id-echo on a parse failure, the
    oversized-line drop, and loop survival after a handler error. This does, with
    the standard library only (no model deps), so it runs in CI via `just sidecar-test`.
    """

    def _exchange(self, lines, env=None, timeout=30):
        """Feed request lines to a fresh sidecar; return the parsed reply objects."""
        proc_env = dict(os.environ)
        if env:
            proc_env.update(env)
        proc = subprocess.run(
            [sys.executable, _SIDECAR_PATH],
            input="".join(line + "\n" for line in lines),
            capture_output=True,
            text=True,
            timeout=timeout,
            env=proc_env,
        )
        return [json.loads(line) for line in proc.stdout.splitlines() if line.strip()]

    def test_ping_roundtrips_with_its_id(self):
        replies = self._exchange([json.dumps({"id": 7, "method": "ping"})])
        self.assertEqual(replies, [{"id": 7, "ok": True, "result": {"ok": True}}])

    def test_reduce_over_the_wire_returns_a_well_formed_result(self):
        # `reduce` with <=3 vectors takes its dependency-free "trivial" path (no numpy,
        # no models), so a real data-returning handler round-trips through the loop even
        # in a bare environment — the case CI actually runs. (count_tokens/embed need
        # fastembed and error without it; their math is pinned by CountTokensTest.)
        req = {"id": 8, "method": "reduce", "params": {"vectors": [[0.1, 0.2], [0.3, 0.4]]}}
        [reply] = self._exchange([json.dumps(req)])
        self.assertEqual(reply["id"], 8)
        self.assertTrue(reply["ok"])
        self.assertEqual(reply["result"]["method"], "trivial")
        self.assertEqual(len(reply["result"]["coords"]), 2)

    def test_unknown_method_is_an_error_envelope_not_a_crash(self):
        [reply] = self._exchange([json.dumps({"id": 9, "method": "nope"})])
        self.assertEqual(reply["id"], 9)
        self.assertFalse(reply["ok"])
        self.assertIn("unknown method", reply["error"])

    def test_malformed_json_replies_with_a_null_id(self):
        [reply] = self._exchange(["{ not valid json"])
        self.assertIsNone(reply["id"])
        self.assertFalse(reply["ok"])

    def test_blank_lines_draw_no_reply(self):
        replies = self._exchange(["", "   ", json.dumps({"id": 1, "method": "ping"})])
        self.assertEqual([r["id"] for r in replies], [1])

    def test_oversized_line_is_dropped_and_the_loop_survives(self):
        # Shrink the cap via env so the drop path is cheap to hit (no 64-MiB pipe).
        # An over-cap line draws NO reply; the next request must still be answered.
        over = json.dumps({"id": 2, "method": "ping", "params": {"pad": "A" * 500}})
        after = json.dumps({"id": 3, "method": "ping"})
        replies = self._exchange([over, after], env={"PM_SIDECAR_MAX_LINE_CHARS": "100"})
        ids = [r["id"] for r in replies]
        self.assertNotIn(2, ids)  # dropped, unanswered
        self.assertIn(3, ids)  # loop kept going

    def test_a_library_print_cannot_corrupt_the_reply_channel(self):
        # stdout IS the reply channel. One `print` inside an unaudited dependency lands mid-line in
        # the newline-JSON, so Rust can't parse the reply, skips the line, and then blocks on an
        # answer already sent — wedging the serialized sidecar for the whole per-method timeout.
        # This used to be guarded only around the two rapidocr calls, so the reply survived exactly
        # one library's chatter. Driven through the REAL loop with a handler that prints.
        import tempfile
        import textwrap

        wrapper_src = textwrap.dedent(
            """
            import sys
            sys.path.insert(0, {dirname!r})
            import pm_sidecar as S

            def chatty(_params):
                print("Downloading model: 100%")
                return {{"said": "hello"}}

            S.HANDLERS["chatty"] = chatty
            S.main()
            """
        ).format(dirname=os.path.dirname(_SIDECAR_PATH))

        with tempfile.TemporaryDirectory() as d:
            wrapper = os.path.join(d, "chatty_sidecar.py")
            with open(wrapper, "w", encoding="utf-8") as fh:
                fh.write(wrapper_src)
            proc = subprocess.run(
                [sys.executable, wrapper],
                input=json.dumps({"id": 4, "method": "chatty"}) + "\n",
                capture_output=True,
                text=True,
                timeout=30,
            )

        lines = [line for line in proc.stdout.splitlines() if line.strip()]
        self.assertEqual(len(lines), 1, f"stdout must hold the reply and nothing else: {lines!r}")
        self.assertEqual(json.loads(lines[0]), {"id": 4, "ok": True, "result": {"said": "hello"}})
        self.assertIn("Downloading model", proc.stderr, "the chatter goes to stderr, not nowhere")

    def test_loop_survives_a_handler_error_between_requests(self):
        reqs = [
            json.dumps({"id": 1, "method": "ping"}),
            json.dumps({"id": 2, "method": "nope"}),  # raises inside the handler
            json.dumps({"id": 3, "method": "ping"}),  # must still be answered
        ]
        replies = self._exchange(reqs)
        self.assertEqual([r["id"] for r in replies], [1, 2, 3])
        self.assertEqual([r["ok"] for r in replies], [True, False, True])


if __name__ == "__main__":
    unittest.main()
