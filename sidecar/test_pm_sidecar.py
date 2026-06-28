# SPDX-FileCopyrightText: 2026 Bobby Yu
# SPDX-License-Identifier: AGPL-3.0-or-later

"""Regression tests for the sidecar token counter.

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


if __name__ == "__main__":
    unittest.main()
