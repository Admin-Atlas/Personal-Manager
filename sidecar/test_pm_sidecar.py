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
                S._guard_xlsx_inflation(path)


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
