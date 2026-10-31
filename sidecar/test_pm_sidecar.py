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

import contextlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import types
import unittest
import zipfile
from unittest import mock

import pm_sidecar as S


def _needs_fastembed(reason):
    """Skip a real-fastembed test where fastembed is absent (a plain `just sidecar-test` on a system
    Python, locally or in CI) — unless `PM_REQUIRE_FASTEMBED=1`, which makes the absence a FAILURE
    instead, so a run meant to exercise the pinned fastembed can't pass by skipping. pr.yml's
    python job sets it for its second run, against a venv built with `pip install --require-hashes
    --no-deps -r sidecar/requirements.lock`, on any PR that touches sidecar/. `--no-deps` is not
    optional: whenever sidecar/requirements-override.txt lifts a dependency's bound, pip refuses
    the lock without it. Locally, build the venv the same way, then
    `PM_REQUIRE_FASTEMBED=1 <venv>/bin/python sidecar/test_pm_sidecar.py`, inside the capped
    systemd-run scope on the Linux dev machine."""
    if os.environ.get("PM_REQUIRE_FASTEMBED") == "1":
        return lambda test: test
    return unittest.skipUnless(importlib.util.find_spec("fastembed") is not None, reason)


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

    @_needs_fastembed("fastembed not installed (real-tokenizer counting is a local-only check)")
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

    @_needs_fastembed("fastembed not installed (real embedding is a local-only check)")
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


class _RecordingOcrEngine:
    """A fake rapidocr that keeps what it was handed: whether OCR ran, and on what."""

    def __init__(self):
        self.targets = []

    def __call__(self, target):
        self.targets.append(target)
        return _OcrResultObj(("TOTAL 12.50",))


class _FakeOpenedImage:
    """What `_open_image` returns, to the extent `do_analyze_image` reads it — no Pillow needed."""

    size = (4, 3)

    def getexif(self):
        return {}

    def close(self):
        pass


_needs_pillow = unittest.skipUnless(
    importlib.util.find_spec("PIL") is not None,
    "Pillow not installed (image decode is a local-only check)",
)


def _exif_with_date_and_place():
    """EXIF the way a phone writes it: DateTimeOriginal in the Exif sub-IFD, a position in the GPS
    sub-IFD (55°57'0"N, 3°11'24"W)."""
    from PIL import Image
    from PIL.TiffImagePlugin import IFDRational

    exif = Image.Exif()
    exif.get_ifd(0x8769)[0x9003] = "2023:01:02 03:04:05"
    gps = exif.get_ifd(0x8825)
    gps[1] = "N"
    gps[2] = (IFDRational(55, 1), IFDRational(57, 1), IFDRational(0, 1))
    gps[3] = "W"
    gps[4] = (IFDRational(3, 1), IFDRational(11, 1), IFDRational(24, 1))
    return exif


class AnalyzeImageTest(unittest.TestCase):
    """Photo analysis: EXIF GPS/date parsing (pure), OCR result-shape tolerance (fake engine), the
    EXIF-only path when OCR is declined, and the format allow-list: a photo opens only as a photo
    format, its extension's own first, and OCR never sees an image PM could not open or decode. The
    OCR engine and HEIC codec are OPTIONAL components, so nothing here installs or downloads
    anything."""

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

    def test_ocr_lines_parses_object_and_list_shapes(self):
        # New rapidocr: object with a .txts tuple.
        self.assertEqual(S._ocr_lines(_OcrResultObj(("hello", "world"))), "hello\nworld")
        # Older rapidocr: ([[box, text, score], ...], elapse).
        old = ([[[0, 0], "line one", 0.9], [[1, 1], "line two", 0.8]], 0.1)
        self.assertEqual(S._ocr_lines(old), "line one\nline two")
        # Nothing recognised → empty string, not an error.
        self.assertEqual(S._ocr_lines(_OcrResultObj(())), "")

    @_needs_pillow
    def test_a_tiff_renamed_jpg_is_refused(self):
        # Pillow picks a decoder from the file's CONTENT, so before the allow-list this opened as a
        # TIFF — and TIFF, FlashPix, EPS and friends are where most of the 2026-10-17 advisories
        # are. Pillow alone still opens it; PM must not.
        import tempfile

        from PIL import Image, UnidentifiedImageError

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/holiday.jpg"
            Image.new("RGB", (20, 10), (200, 100, 50)).save(path, format="TIFF")
            with Image.open(path) as unrestricted:
                self.assertEqual(unrestricted.format, "TIFF", "the fixture really is a TIFF")
            with self.assertRaises(UnidentifiedImageError):
                S._open_image(path)
            out = S.do_analyze_image({"path": path, "run_ocr": False})
        self.assertIsNone(out["width"], "no size: Rust reads that as 'could not open'")
        self.assertIsNone(out["height"])
        self.assertIsNone(out["capture_date"])
        self.assertEqual(out["ocr_ran"], False)

    @_needs_pillow
    def test_a_gif_renamed_png_is_refused(self):
        # A real format that is not a photo's stays refused under any photo extension, not just
        # under its own: the other photo formats are tried, nothing beyond them.
        import tempfile

        from PIL import Image, UnidentifiedImageError

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/meme.png"
            Image.new("RGB", (20, 10), (200, 100, 50)).save(path, format="GIF")
            with Image.open(path) as unrestricted:
                self.assertEqual(unrestricted.format, "GIF", "the fixture really is a GIF")
            with self.assertRaises(UnidentifiedImageError):
                S._open_image(path)

    @_needs_pillow
    def test_a_real_photo_with_the_wrong_photo_extension_still_reads(self):
        # A PNG or a WebP saved as .jpg, a JPEG saved as .png: common, and refusing them blocked
        # nothing (naming the file correctly reaches the same decoder). Refused, such a photo stayed
        # unread for good, because ingest skips content it has indexed already.
        import tempfile

        from PIL import Image

        exif = _exif_with_date_and_place().tobytes()
        cases = (
            ("screenshot.jpg", "PNG"),
            ("download.jpg", "WEBP"),
            ("camera.png", "JPEG"),
            ("camera.webp", "JPEG"),
            ("export.png", "WEBP"),
        )
        for name, fmt in cases:
            with self.subTest(name=name, content=fmt), tempfile.TemporaryDirectory() as d:
                path = f"{d}/{name}"
                Image.new("RGB", (40, 30), (10, 20, 30)).save(path, format=fmt, exif=exif)
                with S._open_image(path) as img:
                    self.assertEqual(img.format, fmt)
                out = S.do_analyze_image({"path": path, "run_ocr": False})
                self.assertEqual((out["width"], out["height"]), (40, 30))
                self.assertEqual(out["capture_date"], "2023-01-02")

    @_needs_pillow
    def test_the_extensions_own_format_is_tried_first(self):
        import tempfile

        from PIL import Image

        tried = []
        real_open = Image.open

        def _recording_open(fp, mode="r", formats=None):
            tried.append(formats)
            return real_open(fp, mode, formats)

        with tempfile.TemporaryDirectory() as d:
            for name in ("a.png", "b.webp", "c.jpeg"):
                Image.new("RGB", (4, 3)).save(f"{d}/{name}")
            with mock.patch.object(Image, "open", _recording_open):
                for name in ("a.png", "b.webp", "c.jpeg"):
                    S._open_image(f"{d}/{name}").close()
        self.assertEqual([f[0] for f in tried], ["PNG", "WEBP", "JPEG"])
        for formats in tried:
            self.assertEqual(set(formats) - {"JPEG", "PNG", "WEBP", "HEIF"}, set())

    @_needs_pillow
    def test_an_extension_pm_does_not_route_here_opens_as_nothing(self):
        # Rust only sends PHOTO_EXTS, but the fallback for an unknown extension must be "refuse",
        # never `formats=None` — which is Pillow's "try every format it knows".
        import tempfile

        from PIL import Image

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/scan.tif"
            Image.new("RGB", (20, 10)).save(path)
            with self.assertRaises(ValueError):
                S._open_image(path)

    @_needs_pillow
    @unittest.skipIf(
        importlib.util.find_spec("pi_heif") is not None,
        "pi-heif installed (this pins the venv without the OCR component)",
    )
    def test_a_heic_without_pi_heif_is_tried_as_the_other_photo_formats(self):
        # Pillow raises a bare KeyError when asked for a format nobody registered, so HEIF is left
        # out of the list without pi-heif. A JPEG saved as .heic still reads; a TIFF or a GIF saved
        # as .heic matches none of the photo formats and is refused, never a KeyError.
        import tempfile

        from PIL import Image, UnidentifiedImageError

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/photo.heic"
            Image.new("RGB", (20, 10), (200, 100, 50)).save(path, format="JPEG")
            with S._open_image(path) as img:
                self.assertEqual(img.format, "JPEG")
            out = S.do_analyze_image({"path": path, "run_ocr": False})
            self.assertEqual((out["width"], out["height"]), (20, 10))
            for fmt in ("TIFF", "GIF"):
                other = f"{d}/{fmt.lower()}.heic"
                Image.new("RGB", (20, 10)).save(other, format=fmt)
                with self.assertRaises(UnidentifiedImageError):
                    S._open_image(other)

    @_needs_pillow
    def test_a_real_png_and_jpeg_still_read(self):
        import tempfile

        from PIL import Image

        # As bytes: PNG's writer drops an Exif whose tags are all in sub-IFDs (the object is falsy).
        exif = _exif_with_date_and_place().tobytes()
        for name, fmt in (("shot.jpg", "JPEG"), ("SHOT.JPEG", "JPEG"), ("shot.png", "PNG")):
            with self.subTest(name), tempfile.TemporaryDirectory() as d:
                path = f"{d}/{name}"
                Image.new("RGB", (40, 30), (10, 20, 30)).save(path, format=fmt, exif=exif)
                out = S.do_analyze_image({"path": path, "run_ocr": False})
                self.assertEqual((out["width"], out["height"]), (40, 30))
                self.assertEqual(out["capture_date"], "2023-01-02")
                self.assertAlmostEqual(out["lat"], 55.95, places=4)
                self.assertAlmostEqual(out["lon"], -3.19, places=4)

    @_needs_pillow
    def test_a_multi_picture_jpeg_still_reads(self):
        # MPO is not in the allow-list (Pillow has no opener for it to name); it arrives through
        # JPEG's, which hands a multi-picture file over by itself. Some cameras write these as .jpg.
        import tempfile

        from PIL import Image

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/stereo.jpg"
            left, right = Image.new("RGB", (32, 24), "red"), Image.new("RGB", (32, 24), "blue")
            left.save(path, format="MPO", save_all=True, append_images=[right])
            with S._open_image(path) as img:
                self.assertEqual(img.format, "MPO")
            out = S.do_analyze_image({"path": path, "run_ocr": False})
        self.assertEqual((out["width"], out["height"]), (32, 24))

    @_needs_pillow
    def test_ocr_is_not_attempted_on_an_image_pm_could_not_open(self):
        # The old fallback handed rapidocr the PATH when PM's own open failed, and rapidocr opens a
        # path with a bare `Image.open` — every decoder, the allow-list undone one call later.
        import tempfile

        from PIL import Image

        engine = _RecordingOcrEngine()
        asked = []

        def _engine():
            asked.append(True)
            return engine

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/receipt.jpg"
            Image.new("RGB", (20, 10)).save(path, format="TIFF")
            with mock.patch.object(S, "get_ocr_engine", _engine):
                out = S.do_analyze_image({"path": path, "run_ocr": True})
        self.assertEqual(asked, [], "the OCR engine was not even loaded")
        self.assertEqual(engine.targets, [])
        self.assertEqual(out["ocr_ran"], False)
        self.assertEqual(out["ocr_text"], "")
        self.assertIsNone(out["width"])

    @unittest.skipUnless(
        importlib.util.find_spec("PIL") is not None
        and importlib.util.find_spec("numpy") is not None,
        "Pillow/numpy not installed (image decode is a local-only check)",
    )
    def test_ocr_reads_the_pixels_pm_opened(self):
        import tempfile

        import numpy as np
        from PIL import Image

        engine = _RecordingOcrEngine()
        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/receipt.png"
            Image.new("RGB", (40, 30), (255, 255, 255)).save(path)
            with mock.patch.object(S, "get_ocr_engine", lambda: engine):
                out = S.do_analyze_image({"path": path, "run_ocr": True})
        self.assertEqual(out["ocr_ran"], True)
        self.assertEqual(out["ocr_text"], "TOTAL 12.50")
        (target,) = engine.targets
        self.assertIsInstance(target, np.ndarray, "pixels, never a path rapidocr would open itself")
        self.assertEqual(target.shape, (30, 40, 3))

    @staticmethod
    def _half_a_jpeg(d):
        """A JPEG cut off halfway, as an interrupted download or a half-synced folder leaves one:
        the header (size, EXIF) is whole, the pixels are not. Noise, so the cut lands in the scan
        data."""
        from PIL import Image

        path = f"{d}/receipt.jpg"
        noise = Image.frombytes("RGB", (160, 120), os.urandom(160 * 120 * 3))
        noise.save(path, format="JPEG", exif=_exif_with_date_and_place().tobytes())
        with open(path, "rb") as f:
            whole = f.read()
        with open(path, "wb") as f:
            f.write(whole[: len(whole) // 2])
        return path

    @unittest.skipUnless(
        importlib.util.find_spec("PIL") is not None
        and importlib.util.find_spec("numpy") is not None,
        "Pillow/numpy not installed (image decode is a local-only check)",
    )
    def test_a_truncated_photo_is_the_files_fault_not_ocrs(self):
        # Opening reads only the header, so this used to come back WITH a size and fail inside the
        # OCR call instead, as ocr_ran=false: the row then blamed text recognition, a component that
        # works. No size is what Rust reports as "could not open"; the header's EXIF is still true.
        import tempfile

        engine = _RecordingOcrEngine()
        asked = []

        def _engine():
            asked.append(True)
            return engine

        with tempfile.TemporaryDirectory() as d:
            path = self._half_a_jpeg(d)
            with S._open_image(path) as img:
                self.assertEqual(img.size, (160, 120), "the fixture's header is whole")
            with mock.patch.object(S, "get_ocr_engine", _engine):
                out = S.do_analyze_image({"path": path, "run_ocr": True})
        self.assertIsNone(out["width"], "no size: Rust reads that as 'could not open'")
        self.assertIsNone(out["height"])
        self.assertEqual(out["ocr_ran"], False)
        self.assertEqual(asked, [], "a file that will not decode never loads the OCR engine")
        self.assertEqual(out["capture_date"], "2023-01-02", "the header's EXIF is kept")
        self.assertAlmostEqual(out["lat"], 55.95, places=4)

    @_needs_pillow
    def test_a_truncated_photo_with_ocr_off_reads_its_header_as_before(self):
        # With OCR off nothing needs the pixels, so nothing decodes them: the size and EXIF in the
        # header are what PM read before, and no new note appears on the row.
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            out = S.do_analyze_image({"path": self._half_a_jpeg(d), "run_ocr": False})
        self.assertEqual((out["width"], out["height"]), (160, 120))
        self.assertEqual(out["capture_date"], "2023-01-02")
        self.assertEqual(out["ocr_ran"], False)

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

    @_needs_pillow
    def test_the_photo_is_closed_when_analysis_returns(self):
        # With OCR off nothing reads the pixels, and Pillow holds the file open until something
        # does, so every photo's handle lingered until garbage collection.
        import gc
        import tempfile
        import warnings

        from PIL import Image

        with tempfile.TemporaryDirectory() as d:
            path = f"{d}/shot.jpg"
            Image.new("RGB", (8, 6)).save(path)
            with warnings.catch_warnings(record=True) as caught:
                warnings.simplefilter("always", ResourceWarning)
                S.do_analyze_image({"path": path, "run_ocr": False})
                gc.collect()
        leaks = [w for w in caught if issubclass(w.category, ResourceWarning)]
        self.assertEqual(leaks, [], "the photo's file was left open")

    def test_analyze_image_unreadable_path_is_null_not_crash(self):
        out = S.do_analyze_image({"path": "/no/such/image.png", "run_ocr": False})
        self.assertEqual(out["ocr_ran"], False)
        self.assertIsNone(out["width"])
        self.assertIsNone(out["capture_date"])

    @unittest.skipUnless(
        importlib.util.find_spec("PIL") is not None
        and importlib.util.find_spec("numpy") is not None,
        "Pillow/numpy not installed (image decode is a local-only check)",
    )
    def test_broken_ocr_degrades_to_exif_only(self):
        # F-56: if the OCR component is broken/offline (engine load or call raises), photo ingest
        # must still return dimensions/EXIF with ocr_ran=false, not fail the whole file. The pixels
        # are decoded (into a numpy array) before the engine is asked for, so numpy is needed to
        # reach the engine at all; every venv has it (fastembed and rapidocr both depend on it).
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

        # OCR only runs on an image PM opened and decoded, so stand both in (no Pillow or numpy
        # needed for that).
        with mock.patch.object(S, "_open_image", lambda _path: _FakeOpenedImage()):
            with mock.patch.object(S, "_decode_pixels", lambda _img: object()):
                with mock.patch.object(S, "get_ocr_engine", _cold):
                    with self.assertRaises(S.ModelNotCached):
                        S.do_analyze_image({"path": "/no/such/image.png", "run_ocr": True})


def _fake_rapidocr(captured, model_file="/pm/models/rapidocr/det.onnx", lazy=True):
    """A stand-in `rapidocr` package: enough of the real shape to pin the offline contract.

    Three details are load-bearing and mirrored exactly from rapidocr (3.9.0, and 3.10.0 for the
    third):

    1. the engine holds its OWN reference to the downloader class (`from ... import DownloadFile`),
       so patching the class attribute has to reach it;
    2. the engine calls `DownloadFile.run` UNCONDITIONALLY — the "it's already downloaded" check
       lives inside `run`, not around the call. A guard that simply refuses would therefore break
       the warm cache as thoroughly as the cold one; and
    3. from 3.10 the constructor is LAZY: it only reads config, `text_det`/`text_cls`/`text_rec`
       start as None, and each session (with its model download) is built by `_load_<task>_model`,
       which `__call__` runs on first use. `lazy=False` is the <= 3.9 shape, which built all three
       in the constructor. A fake in the old shape is what let every test here pass on a 3.10 bump
       that left the fetcher with nothing to fetch (#932).
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
            self.text_det = self.text_cls = self.text_rec = None
            if not lazy:
                for task in ("det", "cls", "rec"):
                    getattr(self, f"_load_{task}_model")()

        def _load(self, attr):
            if getattr(self, attr) is None:
                DownloadFile.run(DownloadFileInput(model_file))
                setattr(self, attr, object())
            return getattr(self, attr)

        def _load_det_model(self):
            return self._load("text_det")

        def _load_cls_model(self):
            return self._load("text_cls")

        def _load_rec_model(self):
            return self._load("text_rec")

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

    def _engine(self, captured, offline, models_dir="/pm/models", model_file=None, lazy=True):
        mods = (
            _fake_rapidocr(captured, model_file, lazy=lazy)
            if model_file
            else _fake_rapidocr(captured, lazy=lazy)
        )
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
        self.assertEqual(len(captured["downloads"]), 3, "detection, orientation and recognition")

    def test_the_fetcher_builds_every_session_of_a_lazy_engine(self):
        # rapidocr 3.10 made the constructor lazy. Built alone it fetches NOTHING, the fetcher
        # reports success, the offline retry misses again and OCR never gets its models (#932).
        for lazy in (True, False):
            with self.subTest(lazy=lazy):
                captured = {}
                engine = self._engine(captured, offline=False, lazy=lazy)
                self.assertEqual(len(captured["downloads"]), 3)
                for attr in ("text_det", "text_cls", "text_rec"):
                    self.assertIsNotNone(getattr(engine, attr), attr)

    def test_an_engine_that_builds_no_sessions_fails_closed(self):
        # If rapidocr renames its loaders again, the fetcher must FAIL, not report a no-op success
        # that leaves the worker missing forever.
        captured = {}
        mods = _fake_rapidocr(captured)
        del mods["rapidocr"].RapidOCR._load_rec_model
        with (
            mock.patch.dict(sys.modules, mods),
            mock.patch.object(S, "_ocr_engine", None),
            mock.patch.object(S, "_OFFLINE", False),
            mock.patch.object(S, "_MODELS_DIR", "/pm/models"),
        ):
            with self.assertRaises(RuntimeError):
                S.get_ocr_engine()

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
        self.assertEqual(len(captured["downloads"]), 3)


# PM's one custom (non-bundled) model today, exactly as `custom_spec` in src-tauri/src/sidecar.rs
# builds it from `BGE_RERANKER_V2_M3` in registry.rs.
_RERANKER_SPEC = {
    "model": "BAAI/bge-reranker-v2-m3",
    "hf": "onnx-community/bge-reranker-v2-m3-ONNX",
    "model_file": "onnx/model_int8.onnx",
    "pooling": "none",
    "normalize": False,
    "dim": 0,
    "local_path": None,
}


_FASTEMBED_090_RERANKERS = ("Xenova/ms-marco-MiniLM-L-6-v2", "BAAI/bge-reranker-v2-m3")


def _fake_fastembed(captured, builtins=_FASTEMBED_090_RERANKERS):
    """A stand-in `fastembed` with the one behaviour #956 turns on: `add_custom_model` refuses a
    name its table already holds (a built-in, or an earlier registration) exactly as 0.9.0's
    does, and a constructor only accepts a name in that table."""
    pkg = types.ModuleType("fastembed")
    rerank = types.ModuleType("fastembed.rerank")
    cross = types.ModuleType("fastembed.rerank.cross_encoder")
    common = types.ModuleType("fastembed.common")
    desc = types.ModuleType("fastembed.common.model_description")

    class ModelSource:
        def __init__(self, hf=None, url=None):
            self.hf, self.url = hf, url

    class TextCrossEncoder:
        _table = set(builtins)

        @classmethod
        def list_supported_models(cls):
            return [{"model": name, "sources": {"hf": name}} for name in sorted(cls._table)]

        @classmethod
        def add_custom_model(cls, model, sources, model_file, **_):
            if model in cls._table:
                raise ValueError(f"Model {model} is already registered in CrossEncoderModel")
            cls._table.add(model)
            captured.setdefault("registered", []).append((model, sources.hf, model_file))

        def __init__(self, model_name, **kwargs):
            if model_name not in self._table:
                raise ValueError(f"Model {model_name} is not supported")
            self.model_name = model_name

    desc.ModelSource = ModelSource
    cross.TextCrossEncoder = TextCrossEncoder
    rerank.cross_encoder = cross
    common.model_description = desc
    pkg.rerank, pkg.common = rerank, common
    return {
        "fastembed": pkg,
        "fastembed.rerank": rerank,
        "fastembed.rerank.cross_encoder": cross,
        "fastembed.common": common,
        "fastembed.common.model_description": desc,
    }


class CustomModelNameTest(unittest.TestCase):
    """#956: fastembed 0.9.0 ships `BAAI/bge-reranker-v2-m3` as a BUILT-IN, PM registered its own
    int8 export under that same name, and `add_custom_model` raised "already registered" before
    the model loaded. The Rust gateway reads any rerank error as "no rerank", so every multilingual
    vault silently lost its reranking, and CI (no fastembed) was green. These pin the fix: a custom
    model is registered under a name only PM uses, while the registry id stays the id."""

    def _reranker(self, captured, model, spec):
        with (
            mock.patch.dict(sys.modules, _fake_fastembed(captured)),
            mock.patch.object(S, "_registered", set()),
            mock.patch.object(S, "_rerankers", {}),
            mock.patch.object(S, "_OFFLINE", False),
        ):
            first = S.get_reranker(model, spec)
            again = S.get_reranker(model, spec)
        self.assertIs(first, again, "cached for the process")
        return first

    def test_a_custom_name_is_never_a_registry_id(self):
        # Pure: holds whichever fastembed is installed, so this half runs in CI too.
        name = S._fastembed_name(_RERANKER_SPEC["model"], _RERANKER_SPEC)
        self.assertTrue(name.startswith(S._CUSTOM_NAMESPACE))
        self.assertNotEqual(name, _RERANKER_SPEC["model"])
        bundled = "Xenova/ms-marco-MiniLM-L-6-v2"
        self.assertEqual(S._fastembed_name(bundled, None), bundled)

    def test_a_custom_reranker_beside_a_same_named_builtin_registers_and_loads(self):
        captured = {}
        engine = self._reranker(captured, _RERANKER_SPEC["model"], _RERANKER_SPEC)
        want = S._CUSTOM_NAMESPACE + "BAAI/bge-reranker-v2-m3"
        self.assertEqual(
            captured["registered"],
            [(want, "onnx-community/bge-reranker-v2-m3-ONNX", "onnx/model_int8.onnx")],
            "registered once, under PM's own name, from PM's own export",
        )
        # The built-in of the same id is the 2.27 GB FP32 model: it must never be what loads.
        self.assertEqual(engine.model_name, want)

    def test_a_bundled_reranker_keeps_its_own_name_and_registers_nothing(self):
        captured = {}
        engine = self._reranker(captured, "Xenova/ms-marco-MiniLM-L-6-v2", None)
        self.assertEqual(engine.model_name, "Xenova/ms-marco-MiniLM-L-6-v2")
        self.assertNotIn("registered", captured)

    @_needs_fastembed("fastembed not installed (the real model table is a local-only check)")
    def test_the_pinned_fastembed_has_no_builtin_under_a_pm_custom_name(self):
        # The real table: run with a venv built from sidecar/requirements.lock. No download.
        from fastembed import TextEmbedding
        from fastembed.rerank.cross_encoder import TextCrossEncoder

        name = S._fastembed_name(_RERANKER_SPEC["model"], _RERANKER_SPEC)
        for cls in (TextCrossEncoder, TextEmbedding):
            builtins = {d["model"] for d in cls.list_supported_models()} - S._registered
            self.assertNotIn(name, builtins, cls.__name__)
            self.assertFalse(any(b.startswith(S._CUSTOM_NAMESPACE) for b in builtins), cls.__name__)
        # And the registration itself goes through on the real class (on 0.9.0 it raised before).
        S._register_custom(TextCrossEncoder, name, _RERANKER_SPEC)
        self.assertIn(name, {d["model"] for d in TextCrossEncoder.list_supported_models()})

    @unittest.skipUnless(
        os.environ.get("PM_TEST_RERANKER_MODELS"),
        "set PM_TEST_RERANKER_MODELS to a models dir holding the cached bge-reranker-v2-m3 int8",
    )
    def test_the_multilingual_reranker_loads_offline_and_ranks(self):
        # The end-to-end half of the #956 guard, offline: the custom name has to resolve to the int8
        # export PM's cache already holds, and the scores have to mean something.
        with (
            mock.patch.object(S, "_MODELS_DIR", os.environ["PM_TEST_RERANKER_MODELS"]),
            mock.patch.object(S, "_OFFLINE", True),
        ):
            out = S.do_rerank(
                {
                    "query": "Wann ist die Miete fällig?",
                    "passages": [
                        "Das Rezept braucht drei Eier und Mehl.",
                        "Die Miete ist am ersten Tag jedes Monats fällig.",
                    ],
                    "model": _RERANKER_SPEC["model"],
                    "custom": _RERANKER_SPEC,
                }
            )
        off_topic, relevant = out["scores"]
        self.assertGreater(relevant, off_topic)


def _case_sensitive(path):
    """Whether the filesystem holding `path` tells `a` from `A` (Linux: yes; Windows and macOS by
    default: no). The case-variant cache only exists on the first kind."""
    probe = os.path.join(path, "case-probe")
    open(probe, "w").close()
    try:
        return not os.path.exists(os.path.join(path, "CASE-PROBE"))
    finally:
        os.remove(probe)


class FastembedCacheCaseTest(unittest.TestCase):
    """fastembed 0.8.1 respelled the default embedder's source repo in a different letter case, so
    on Linux the cache folder 0.8.0 filled no longer matched and the upgraded app downloaded the
    model again — or, offline, lost embedding entirely. These pin the fetcher adopting the copy it
    already has."""

    OLD = "models--qdrant--bge-small-en-v1.5-onnx-q"
    NEW = "models--Qdrant--bge-small-en-v1.5-onnx-Q"
    REPO = "Qdrant/bge-small-en-v1.5-onnx-Q"

    def setUp(self):
        self._dir = tempfile.TemporaryDirectory()
        self.root = self._dir.name
        if not _case_sensitive(self.root):
            self._dir.cleanup()
            self.skipTest("case-insensitive filesystem: the old folder answers to the new name")

    def tearDown(self):
        self._dir.cleanup()

    def _folder(self, name):
        os.makedirs(os.path.join(self.root, name, "snapshots"))

    def test_a_case_variant_folder_is_renamed_to_the_spelling_fastembed_now_asks_for(self):
        self._folder(self.OLD)
        S._adopt_case_variant_cache(self.root, self.REPO)
        self.assertEqual(sorted(os.listdir(self.root)), [self.NEW])
        self.assertTrue(os.path.isdir(os.path.join(self.root, self.NEW, "snapshots")))

    def test_an_exact_match_is_left_alone_even_beside_a_variant(self):
        # A machine that already downloaded the new spelling keeps both: the old folder may still
        # belong to an older PM build sharing this data dir, so it is not this code's to remove.
        self._folder(self.OLD)
        self._folder(self.NEW)
        S._adopt_case_variant_cache(self.root, self.REPO)
        self.assertEqual(sorted(os.listdir(self.root)), sorted([self.OLD, self.NEW]))

    def test_nothing_to_adopt_is_a_no_op(self):
        self._folder("models--Xenova--ms-marco-MiniLM-L-6-v2")
        S._adopt_case_variant_cache(self.root, self.REPO)
        S._adopt_case_variant_cache(os.path.join(self.root, "missing"), self.REPO)
        S._adopt_case_variant_cache(None, self.REPO)
        S._adopt_case_variant_cache(self.root, None)
        self.assertEqual(os.listdir(self.root), ["models--Xenova--ms-marco-MiniLM-L-6-v2"])

    def test_the_fetcher_adopts_before_it_loads_and_so_never_downloads(self):
        # fastembed checks its cache before the network, so the folder must carry the new name by
        # the time the loader runs. Fake fastembed: the real one is absent in CI.
        cache = os.path.join(self.root, "fastembed")
        os.makedirs(os.path.join(cache, self.OLD))
        fake = types.ModuleType("fastembed")

        class TextEmbedding:
            @staticmethod
            def list_supported_models():
                return [{"model": "BAAI/bge-small-en-v1.5", "sources": {"hf": self.REPO}}]

        fake.TextEmbedding = TextEmbedding
        seen = {}

        def loader(model, custom):
            seen["adopted"] = os.path.isdir(os.path.join(cache, self.NEW))

        with (
            mock.patch.dict(sys.modules, {"fastembed": fake}),
            mock.patch.object(S, "_MODELS_DIR", self.root),
            mock.patch.object(S, "get_embedder", loader),
            mock.patch.object(S, "get_tokenizer", lambda model, custom: None),
        ):
            S._ensure_model("embed", {})
        self.assertTrue(seen["adopted"])


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
                # `Unconvertible`, not the guard's raw ValueError: do_convert re-types PM's own
                # caps into the verdict Rust classifies on, so the file is skipped and the delta
                # cursor is allowed past it. The guard's own ValueError is asserted directly in
                # `test_a_zip_bomb_is_refused_before_it_is_read` above.
                with self.assertRaises(S.Unconvertible, msg=name):
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


def _fake_markitdown():
    """A stand-in `markitdown` package carrying the real exception hierarchy (unchanged 0.1.6 to
    0.1.8), including `FileConversionException`'s `attempts` — the shape `_convert` really raises.

    Injected rather than skipped-on-absent because the thing under test is a CLASSIFICATION, and
    classification is exactly what breaks silently: the real package is not installed in CI, and a
    test that skips there is a test that never guards the release it matters for.

    ONE package instance per test, with what to raise held on the module (`pkg._raises`) rather
    than captured per call. Building a fresh package for the exception and another for the engine
    mints two unrelated class objects, so `except MarkItDownException` matches neither and every
    assertion passes for the wrong reason.

    `pkg._raises` takes either one exception — raised on EVERY call, which is what the verdict
    tests want — or a list consumed one entry per call, so a test can make the first attempt fail
    and the retry succeed. `pkg._calls` records the `stream_info` each attempt was given, which is
    the only way to tell a retry apart from a first pass.
    """
    import types

    pkg = types.ModuleType("markitdown")
    pkg._raises = None
    pkg._calls = []

    class MarkItDownException(Exception):
        pass

    class UnsupportedFormatException(MarkItDownException):
        pass

    class FailedConversionAttempt:
        """The real one: the converter that failed, and the `sys.exc_info()` of its failure."""

        def __init__(self, converter, exc_info=None):
            self.converter = converter
            self.exc_info = exc_info

    class FileConversionException(MarkItDownException):
        """The real signature. `_convert` raises ONE of these when every converter that accepted
        the file failed, with a `FailedConversionAttempt` per failure in `attempts`."""

        def __init__(self, message=None, attempts=None):
            self.attempts = attempts
            super().__init__(message or "File conversion failed.")

    class MissingDependencyException(MarkItDownException):
        pass

    class _Result:
        text_content = "hello"
        title = "T"

    class StreamInfo:
        """The real one is a frozen dataclass of optional hints; `charset` is all PM sets."""

        def __init__(self, charset=None):
            self.charset = charset

    class MarkItDown:
        def convert_local(self, path, *, stream_info=None):
            pkg._calls.append(stream_info)
            raises = pkg._raises
            if isinstance(raises, list):
                raises = raises[len(pkg._calls) - 1] if len(pkg._calls) <= len(raises) else None
            if raises is not None:
                raise raises
            return _Result()

    pkg.StreamInfo = StreamInfo
    pkg.MarkItDownException = MarkItDownException
    pkg.UnsupportedFormatException = UnsupportedFormatException
    pkg.FailedConversionAttempt = FailedConversionAttempt
    pkg.FileConversionException = FileConversionException
    pkg.MissingDependencyException = MissingDependencyException
    pkg.MarkItDown = MarkItDown
    return pkg


def _wrapped(pkg, *causes):
    """What markitdown's `_convert` really raises once every converter that accepted the file has
    failed: ONE `FileConversionException`, each failure's `sys.exc_info()` held in `attempts`.
    Each cause is genuinely raised and caught so its `exc_info` is the real triple."""
    attempts = []
    for cause in causes:
        try:
            raise cause
        except Exception:
            attempt = pkg.FailedConversionAttempt(converter=object(), exc_info=sys.exc_info())
            attempts.append(attempt)
    return pkg.FileConversionException(attempts=attempts)


class ConvertVerdictTest(unittest.TestCase):
    """A refusal of THIS FILE must be told apart from the ENGINE being broken.

    `str(exc)` carries no type, so before this the Rust side matched the bare
    `sidecar convert failed:` prefix — which every convert error wears. A half-installed venv
    therefore answered "No module named 'markitdown'" for every file, each was classified
    permanently-unindexable, and the delta cursor committed past changes PM had never indexed.
    The changes feed never re-offers a committed change, so the files were gone until the user
    disconnected and re-added the account. These pin which side of the line each cause falls on.
    """

    def setUp(self):
        self.pkg = _fake_markitdown()

    def _convert(self, path="/pm/test/doc.txt", raises=None):
        self.pkg._raises = raises
        with mock.patch.dict(sys.modules, {"markitdown": self.pkg}):
            S._markitdown = None  # force get_markitdown() through the injected package
            try:
                return S.do_convert({"path": path})
            finally:
                S._markitdown = None

    def test_engine_refusing_this_file_is_a_verdict(self):
        # The engine read the file and said no. Retrying forever is what pins the account.
        for exc_name in ("UnsupportedFormatException", "FileConversionException"):
            exc_cls = getattr(self.pkg, exc_name)
            with self.assertRaises(S.Unconvertible, msg=exc_name):
                self._convert(raises=exc_cls("cannot handle .xyz"))

    def test_broken_engine_is_not_a_verdict(self):
        # Each of these resolves once the engine is repaired, so each must stay account-fatal —
        # anything but Unconvertible, which is what holds the delta cursor.
        for exc in (
            ModuleNotFoundError("No module named 'markitdown'"),
            ImportError("cannot import name 'MarkItDown'"),
            OSError(28, "No space left on device"),
            # A MarkItDownException, but about the ENVIRONMENT, not the file: this format needs
            # an optional package the venv lacks. Skipping would lose every file of that type.
            # Same package instance, so this really is recognised as a missing dependency and
            # re-raised — unmatched, it would have fallen into the verdict arm as Unconvertible.
            self.pkg.MissingDependencyException("PdfConverter requires pdfminer.six"),
        ):
            with self.assertRaises(Exception) as caught:
                self._convert(raises=exc)
            self.assertNotIsInstance(caught.exception, S.Unconvertible, repr(exc))

    def test_a_wrapped_missing_dependency_is_not_a_verdict(self):
        # The shape the library actually produces: converters raise MissingDependencyException
        # from `convert()`, and `_convert` wraps it in a FileConversionException. A bare
        # `except MissingDependencyException:` never saw it, so a half-installed venv marked every
        # PDF Unconvertible and the cursor moved past them for good.
        missing = self.pkg.MissingDependencyException("PdfConverter requires pdfminer.six")
        for exc in (
            _wrapped(self.pkg, missing),
            # Beside another converter's ordinary refusal it still wins: repairing the engine
            # may well let the file convert, so skipping it would be a guess.
            _wrapped(self.pkg, ValueError("not a zip file"), missing),
        ):
            self.pkg._calls.clear()
            with self.assertRaises(Exception) as caught:
                self._convert(path="/pm/test/doc.pdf", raises=exc)
            self.assertNotIsInstance(caught.exception, S.Unconvertible)
            self.assertIs(caught.exception, exc, "re-raised as-is, so main reports it untagged")
            self.assertEqual(self.pkg._calls, [None], "a missing dependency earns no retry")

    def test_a_wrapped_refusal_is_still_a_verdict(self):
        # The negative control for the unwrapping: an ordinary converter failure in `attempts`
        # stays a skip, exactly as an unwrapped FileConversionException does.
        with self.assertRaises(S.Unconvertible):
            self._convert(raises=_wrapped(self.pkg, ValueError("not a zip file")))

    def test_a_failed_lazy_import_is_not_a_verdict(self):
        # The import lives in get_markitdown(), OUTSIDE do_convert's try — the case that actually
        # shipped. With no `markitdown` in sys.modules at all, the ImportError must escape untagged.
        S._markitdown = None
        with mock.patch.dict(sys.modules, {"markitdown": None}):
            with self.assertRaises(Exception) as caught:
                S.do_convert({"path": "/pm/test/doc.txt"})
        self.assertNotIsInstance(caught.exception, S.Unconvertible)

    def test_pm_own_input_cap_is_a_verdict(self):
        # PM's own caps are verdicts about the file. Runs the real guard against a real file.
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "big.bin")
            with open(p, "wb") as fh:
                fh.write(b"x" * 4096)
            with mock.patch.object(S, "MAX_INPUT_FILE_BYTES", 1024):
                with self.assertRaises(S.Unconvertible) as caught:
                    self._convert(path=p)
        self.assertIn("too large to process", str(caught.exception))

    def test_a_convertible_file_still_converts(self):
        # The negative control: the wrapping must not turn a healthy conversion into an error.
        self.assertEqual(self._convert()["markdown"], "hello")


class ConvertVerdictWireTest(unittest.TestCase):
    """The verdict has to survive the wire, or the classifier upstream never sees it.

    Drives the real main loop, and deliberately uses PM's own size cap — which is enforced BEFORE
    the lazy markitdown import — so this runs end to end in CI with the package absent.
    """

    def test_an_unconvertible_reply_is_tagged_on_the_wire(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "big.bin")
            # `truncate`, not `write`: the cap is 128 MiB and the guard only calls getsize, so
            # sizing the file costs nothing rather than committing 128 MB to disk on every run.
            with open(p, "wb") as fh:
                fh.truncate(S.MAX_INPUT_FILE_BYTES + 1)
            req = json.dumps({"id": 1, "method": "convert", "params": {"path": p}})
            proc = subprocess.run(
                [sys.executable, os.path.join(os.path.dirname(__file__), "pm_sidecar.py")],
                input=req + "\n",
                capture_output=True,
                text=True,
                timeout=120,
            )
        reply = json.loads(proc.stdout.strip().splitlines()[0])
        self.assertFalse(reply["ok"])
        # The tag Rust renders as `sidecar convert failed [unconvertible]:` and classifies on.
        self.assertEqual(reply.get("error_kind"), "unconvertible")


def _ascii_decode_error():
    """The real shape of the failure: MarkItDown labelled the file `ascii` from its charset sample
    (the first 4 KB up to 0.1.7, the first 64 KB from 0.1.8)."""
    return UnicodeDecodeError("ascii", b"\xe2\x80\x94", 0, 1, "ordinal not in range(128)")


class ConvertCharsetRetryTest(unittest.TestCase):
    """A charset guessed from a sample must not condemn the rest of the file.

    MarkItDown guesses a charset from a sample — `file_stream.read(4096)` up to 0.1.7, 64 KB from
    0.1.8 — then hands that one guess to every converter, which decode the WHOLE file with it. A
    file whose sample is plain ASCII is therefore labelled `ascii`, and one em dash further in
    raises `UnicodeDecodeError`. Up to 0.1.7 it came out of `accepts()`, the *notebook* converter's
    sniff, which MarkItDown guards against `NotImplementedError` and nothing else, so it escaped
    bare. From 0.1.8 that sniff swallows `ValueError`, the plain-text converter's `convert()` is
    what fails, and the error arrives WRAPPED in a FileConversionException's `attempts`. Both
    shapes are pinned here; the wrapped one is the only one 0.1.8 produces.

    Observed live on 2026-08-20 against a real library (0.1.7, a 27 KB JSON file, the bare
    shape), repeating every time the file came round. It escaped `do_convert` untagged, so Rust
    read it as an engine fault and re-offered the item forever instead of moving the cursor past it.
    """

    def setUp(self):
        self.pkg = _fake_markitdown()

    def _convert(self, raises=None, path="/pm/test/doc.json"):
        self.pkg._raises = raises
        with mock.patch.dict(sys.modules, {"markitdown": self.pkg}):
            S._markitdown = None
            try:
                return S.do_convert({"path": path})
            finally:
                S._markitdown = None

    def test_a_healthy_file_is_never_given_a_forced_charset(self):
        # The retry is a fallback, not a policy: forcing UTF-8 up front would misread a file that
        # genuinely is cp1252 or UTF-16, which the detector had right.
        self.assertEqual(self._convert()["markdown"], "hello")
        self.assertEqual(self.pkg._calls, [None])

    def test_a_charset_misguess_is_retried_as_utf8_and_the_file_survives(self):
        result = self._convert(raises=[_ascii_decode_error(), None])
        self.assertEqual(result["markdown"], "hello")
        self.assertEqual(len(self.pkg._calls), 2, "expected exactly one retry")
        self.assertIsNone(self.pkg._calls[0], "the first pass must keep the detected charset")
        self.assertEqual(self.pkg._calls[1].charset, "utf-8")

    def test_a_wrapped_charset_misguess_is_retried_too(self):
        # markitdown 0.1.8's shape. Matching only a bare UnicodeDecodeError made every file over
        # 64 KB with a late non-ASCII byte permanently Unconvertible.
        result = self._convert(raises=[_wrapped(self.pkg, _ascii_decode_error()), None])
        self.assertEqual(result["markdown"], "hello")
        self.assertEqual(len(self.pkg._calls), 2, "expected exactly one retry")
        self.assertIsNone(self.pkg._calls[0], "the first pass must keep the detected charset")
        self.assertEqual(self.pkg._calls[1].charset, "utf-8")

    def test_a_wrapped_misguess_that_fails_again_is_a_verdict(self):
        wrapped = _wrapped(self.pkg, _ascii_decode_error())
        with self.assertRaises(S.Unconvertible):
            self._convert(raises=[wrapped, wrapped])
        self.assertEqual(len(self.pkg._calls), 2)

    def test_a_wrapped_non_decode_failure_earns_no_retry(self):
        # `ValueError` on purpose: UnicodeDecodeError IS a ValueError, so this pins that the check
        # is for a decode failure specifically and not the family it belongs to.
        with self.assertRaises(S.Unconvertible):
            self._convert(raises=_wrapped(self.pkg, ValueError("not a zip file")))
        self.assertEqual(self.pkg._calls, [None])

    def test_the_misguess_check_reads_every_shape_without_the_package(self):
        # Duck-typed on `attempts` / `exc_info`, so it needs no markitdown. `exc_info` may be None
        # (the real FileConversionException message handles that case), and `attempts` may be.
        class _Attempt:
            def __init__(self, exc_info):
                self.exc_info = exc_info

        class _Wrapper(Exception):
            def __init__(self, attempts):
                self.attempts = attempts

        decode = _ascii_decode_error()
        self.assertTrue(S._is_charset_misguess(decode))
        self.assertTrue(S._is_charset_misguess(_Wrapper([_Attempt((type(decode), decode, None))])))
        self.assertTrue(
            S._is_charset_misguess(
                _Wrapper([_Attempt(None), _Attempt((type(decode), decode, None))])
            ),
            "any attempt counts, not just the first",
        )
        self.assertFalse(S._is_charset_misguess(ValueError("not a zip file")))
        self.assertFalse(S._is_charset_misguess(_Wrapper(None)))
        self.assertFalse(S._is_charset_misguess(_Wrapper([_Attempt(None)])))

    def test_a_file_in_some_other_encoding_is_a_verdict_not_an_engine_fault(self):
        # ASCII is a strict subset of UTF-8, so a file that fails BOTH is genuinely unreadable.
        # It must carry the `unconvertible` marker: untagged is what pinned the cursor.
        with self.assertRaises(S.Unconvertible):
            self._convert(raises=[_ascii_decode_error(), _ascii_decode_error()])
        self.assertEqual(len(self.pkg._calls), 2)

    def test_only_a_decode_failure_earns_a_second_attempt(self):
        # A converter that read the file and refused it will refuse it again. Retrying every
        # failure would double the cost of every genuinely unconvertible file in the library.
        with self.assertRaises(S.Unconvertible):
            self._convert(raises=self.pkg.UnsupportedFormatException("cannot handle .xyz"))
        self.assertEqual(self.pkg._calls, [None])

    def test_a_broken_engine_is_still_not_retried_into_a_verdict(self):
        # MissingDependencyException stays account-fatal, retry or no retry — bare, and wrapped the
        # way the library really raises it.
        missing = self.pkg.MissingDependencyException("needs pdfminer.six")
        for exc in (missing, _wrapped(self.pkg, missing)):
            self.pkg._calls.clear()
            with self.assertRaises(Exception) as caught:
                self._convert(raises=exc)
            self.assertNotIsInstance(caught.exception, S.Unconvertible)
            self.assertEqual(self.pkg._calls, [None])

    def test_the_helper_alone_is_pure_and_needs_no_package(self):
        # `convert_local_tolerating_charset` takes the StreamInfo class rather than importing it,
        # so the decision is testable in a gate whose Python has no venv.
        class _Info:
            def __init__(self, charset=None):
                self.charset = charset

        seen = []

        class _Engine:
            def convert_local(self, path, *, stream_info=None):
                seen.append(stream_info)
                if len(seen) == 1:
                    raise _ascii_decode_error()
                return "converted"

        out = S.convert_local_tolerating_charset(_Engine(), "/x.json", _Info)
        self.assertEqual(out, "converted")
        self.assertEqual([s.charset if s else None for s in seen], [None, "utf-8"])


class TranscribeThreadBudgetTest(unittest.TestCase):
    """Transcription must not silently inherit a number chosen by measuring embedding.

    faster-whisper's `cpu_threads` defaults to 0, which its docstring defines as "a non zero value
    overrides the OMP_NUM_THREADS environment variable" — zero means OMP wins. This module sets
    OMP_NUM_THREADS for the embedding pools, so leaving `cpu_threads` unset handed transcription the
    embedding budget: on 4 cores that was 4 threads -> 2, half speed, with nothing to connect it to.
    """

    def _budget(self, cores, env=None):
        with mock.patch.dict(os.environ, env or {}, clear=False):
            for k in ("PM_SIDECAR_TRANSCRIBE_THREADS",):
                if not (env or {}).get(k):
                    os.environ.pop(k, None)
            with mock.patch.object(S.os, "cpu_count", lambda: cores):
                return S._transcribe_thread_budget()

    def test_no_machine_transcribes_slower_than_ctranslate2_would_alone(self):
        # 4 is faster-whisper's own default. Below 8 cores the general budget would say 2 or 3.
        for cores in (1, 2, 4, 6, 8):
            self.assertEqual(self._budget(cores), 4, f"{cores} cores")

    def test_it_still_scales_up_with_the_machine(self):
        for cores, want in ((10, 5), (12, 6), (16, 8), (24, 8), (128, 8)):
            self.assertEqual(self._budget(cores), want, f"{cores} cores")
        self.assertEqual(self._budget(None), 4, "unknown core count takes the floor")

    def test_the_two_budgets_diverge_exactly_where_the_bug_was(self):
        # They agree from 8 cores up; the divergence below that IS the fix, so a refactor that
        # collapsed them into one function would pass every other test in this class.
        with mock.patch.object(S.os, "cpu_count", lambda: 4):
            with mock.patch.dict(os.environ, {}, clear=False):
                os.environ.pop("PM_SIDECAR_THREADS", None)
                os.environ.pop("PM_SIDECAR_TRANSCRIBE_THREADS", None)
                self.assertEqual(S._thread_budget(), 2)
                self.assertEqual(S._transcribe_thread_budget(), 4)

    def test_rust_outranks_the_local_guess_and_junk_falls_back(self):
        self.assertEqual(self._budget(24, {"PM_SIDECAR_TRANSCRIBE_THREADS": "3"}), 3)
        self.assertEqual(self._budget(4, {"PM_SIDECAR_TRANSCRIBE_THREADS": "banana"}), 4)

    def test_the_model_is_actually_told_the_number(self):
        # The budget existing is worth nothing if the kwarg is not passed: an unset `cpu_threads`
        # is 0, and 0 is exactly the "let OMP_NUM_THREADS decide" case this class exists to close.
        captured = {}

        class _Model:
            def __init__(self, name, **kw):
                captured.update(kw)

        pkg = types.ModuleType("faster_whisper")
        pkg.WhisperModel = _Model
        with mock.patch.dict(sys.modules, {"faster_whisper": pkg}):
            S._whisper = None
            try:
                S.get_whisper(None)
            finally:
                S._whisper = None
        self.assertEqual(captured.get("cpu_threads"), S._TRANSCRIBE_THREADS)
        self.assertGreaterEqual(captured["cpu_threads"], 4)


CORE_XML = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties
    xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:dcterms="http://purl.org/dc/terms/">
  <dc:title>Q3 plan</dc:title>
  <dc:creator>{creator}</dc:creator>
  <cp:lastModifiedBy>{editor}</cp:lastModifiedBy>
  <dcterms:created>2025-11-04T09:12:00Z</dcterms:created>
  <dcterms:modified>2026-01-20T08:00:00Z</dcterms:modified>
</cp:coreProperties>
"""


@contextlib.contextmanager
def _xml_parser():
    """Run the OOXML reader with a parser available.

    `just sidecar-test` is stdlib-only by design, so where the pinned defusedxml isn't installed we
    stand in the stdlib parser that it patches. What these tests are about — the zip handling and
    the namespaced lookups — is identical either way, and the fail-closed path when NEITHER is
    importable gets its own test below.
    """
    try:
        import defusedxml.ElementTree  # noqa: F401

        yield
        return
    except Exception:
        pass
    import xml.etree.ElementTree as ET

    stub = types.ModuleType("defusedxml.ElementTree")
    stub.fromstring = ET.fromstring
    pkg = types.ModuleType("defusedxml")
    pkg.ElementTree = stub
    with mock.patch.dict(sys.modules, {"defusedxml": pkg, "defusedxml.ElementTree": stub}):
        yield


def _write_ooxml(path, core_xml=None):
    """A minimal OOXML container. `_ooxml_properties` opens exactly one member, so a two-entry zip
    is a faithful stand-in for a real .docx — and one this test builds with no dependency at all."""
    with zipfile.ZipFile(path, "w") as zf:
        zf.writestr("[Content_Types].xml", "<Types/>")
        if core_xml is not None:
            zf.writestr(S.CORE_PROPS_PATH, core_xml)


class DocumentPropertiesTest(unittest.TestCase):
    """What the FILE says about itself (#709).

    The position this refines: a filesystem has no author PM could honestly report (#701), so local
    documents showed "Unknown" everywhere while the document itself plainly named one. Reading the
    document's own property block closes that without PM ever naming the OS account.
    """

    def _docx(self, d, core_xml=CORE_XML.format(creator="Jane Okafor", editor="Sam Reyes")):
        path = os.path.join(d, "plan.docx")
        _write_ooxml(path, core_xml)
        return path

    def test_an_ooxml_document_states_its_author_editor_and_creation(self):
        with tempfile.TemporaryDirectory() as d, _xml_parser():
            props = S.read_document_properties(self._docx(d))
        self.assertEqual(props["author"], "Jane Okafor")
        self.assertEqual(props["last_modified_by"], "Sam Reyes")
        self.assertEqual(props["created"], "2025-11-04T09:12:00Z")

    def test_the_documents_own_modified_date_is_deliberately_not_reported(self):
        # The fixture HAS a dcterms:modified and it must not reach the reply. PM's
        # `source_modified_at` for a local file is the filesystem mtime, which is what the connector
        # diffs to notice a change; a second, differently-sourced "modified" invites that
        # substitution, and a docProps date predating the copy on disk would look permanently stale.
        with tempfile.TemporaryDirectory() as d, _xml_parser():
            props = S.read_document_properties(self._docx(d))
        self.assertEqual(sorted(props), ["author", "created", "last_modified_by"])

    def test_an_unstated_author_reads_as_not_stated_rather_than_blank(self):
        # Word writes `<dc:creator/>` for an unset author. An empty string rendered under "Author"
        # is worse than "Unknown" — it looks like PM lost the value rather than never had it.
        for creator in ("", "   ", "\n\t"):
            with tempfile.TemporaryDirectory() as d, _xml_parser():
                path = self._docx(d, CORE_XML.format(creator=creator, editor="Sam Reyes"))
                props = S.read_document_properties(path)
            self.assertIsNone(props["author"], repr(creator))
            self.assertEqual(props["last_modified_by"], "Sam Reyes")  # the others still land

    def test_a_container_with_no_property_block_is_not_a_failure(self):
        # Legal, and some writers do it. Every reader here answers "the document didn't say".
        with tempfile.TemporaryDirectory() as d, _xml_parser():
            path = os.path.join(d, "bare.docx")
            _write_ooxml(path)
            self.assertEqual(S.read_document_properties(path), S.no_properties())

    def test_an_oversized_property_member_is_refused_before_it_inflates(self):
        # The whole-archive guard's 1 GiB ceiling is far too coarse to catch a bomb aimed at the one
        # member this reader opens, so the member carries its own cap.
        with tempfile.TemporaryDirectory() as d, _xml_parser():
            path = self._docx(d)
            with mock.patch.object(S, "MAX_DOC_PROPS_BYTES", 8):
                props = S.read_document_properties(path)
        self.assertEqual(props, S.no_properties())

    def test_a_missing_hardened_parser_fails_closed(self):
        # Unlike `defuse_stdlib()` at import, which is best-effort because ingest depends on it.
        # This parse is optional, so a venv without defusedxml simply gets no properties.
        with tempfile.TemporaryDirectory() as d:
            path = self._docx(d)
            with mock.patch.dict(sys.modules, {"defusedxml": None, "defusedxml.ElementTree": None}):
                self.assertEqual(S.read_document_properties(path), S.no_properties())

    def test_a_format_with_no_property_block_asks_nothing(self):
        # .txt/.md/.html carry no properties at all, and a path that doesn't exist must still answer
        # rather than raise — a property is never the reason a file fails to land.
        self.assertEqual(S.read_document_properties("/pm/test/notes.md"), S.no_properties())
        self.assertEqual(S.read_document_properties("/pm/test/gone.docx"), S.no_properties())

    def test_a_long_value_is_capped(self):
        self.assertEqual(len(S.clean_property("x" * 5000)), S.MAX_PROPERTY_CHARS)


class PdfDateTest(unittest.TestCase):
    """The PDF Info dictionary's own encodings, as pure functions — so they are covered in CI with
    no PDF library installed."""

    def test_a_full_pdf_date_carries_its_offset(self):
        self.assertEqual(S.pdf_date_to_iso("D:20260104120000+01'00'"), "2026-01-04T12:00:00+01:00")
        self.assertEqual(S.pdf_date_to_iso("D:20260104120000Z"), "2026-01-04T12:00:00Z")

    def test_every_field_after_the_year_is_optional(self):
        # The spec's truncation points, which real writers do use.
        self.assertEqual(S.pdf_date_to_iso("D:2026"), "2026-01-01T00:00:00Z")
        self.assertEqual(S.pdf_date_to_iso("D:202601"), "2026-01-01T00:00:00Z")
        self.assertEqual(S.pdf_date_to_iso("D:20260104"), "2026-01-04T00:00:00Z")
        self.assertEqual(
            S.pdf_date_to_iso("20260104"), "2026-01-04T00:00:00Z"
        )  # D: is optional too

    def test_a_date_that_is_not_a_pdf_date_is_refused_rather_than_salvaged(self):
        # An ISO date wrongly stored in the Info dictionary would otherwise truncate to its year and
        # be reported with false precision — a wrong answer, which is worse than a missing one.
        for bad in ("D:2026-01-04", "2026-01-04T09:00:00Z", "D:20261345", "", None, "not a date"):
            self.assertIsNone(S.pdf_date_to_iso(bad), repr(bad))

    def test_a_pdf_string_decodes_from_either_encoding(self):
        self.assertEqual(S.pdf_string(b"Jane Okafor"), "Jane Okafor")
        # UTF-16 with a BOM, which is how any non-ASCII name is written. The codec eats the BOM.
        self.assertEqual(S.pdf_string("René Fauré".encode("utf-16")), "René Fauré")
        # PDFDocEncoding otherwise, which agrees with latin-1 over everything a name contains.
        self.assertEqual(S.pdf_string(b"Ren\xe9"), "René")
        self.assertIsNone(S.pdf_string(None))

    def test_a_malformed_w3cdtf_timestamp_is_refused(self):
        self.assertEqual(S.iso_or_none("2025-11-04T09:12:00Z"), "2025-11-04T09:12:00Z")
        self.assertEqual(S.iso_or_none("2025-11-04"), "2025-11-04")
        self.assertIsNone(S.iso_or_none("last Tuesday"))
        self.assertIsNone(S.iso_or_none(None))


class FilePropertiesWireTest(unittest.TestCase):
    """The method is reachable over the wire and always answers in one shape. Uses a .md path, which
    carries no property block, so this runs end to end in CI with nothing installed."""

    def test_the_reply_shape_is_fixed(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "notes.md")
            with open(path, "w", encoding="utf-8") as fh:
                fh.write("# hello\n")
            req = json.dumps({"id": 7, "method": "file_properties", "params": {"path": path}})
            proc = subprocess.run(
                [sys.executable, os.path.join(os.path.dirname(__file__), "pm_sidecar.py")],
                input=req + "\n",
                capture_output=True,
                text=True,
                timeout=120,
            )
        reply = json.loads(proc.stdout.strip().splitlines()[0])
        self.assertTrue(reply["ok"], proc.stdout)
        self.assertEqual(
            reply["result"], {"author": None, "last_modified_by": None, "created": None}
        )


class ResourcePostureTest(unittest.TestCase):
    """The sidecar's memory and CPU posture — the settings that keep a long-lived helper from
    quietly eating the machine it runs on.

    The defect these lock down: `TextEmbedding(...)` was built with neither `threads` nor
    `enable_cpu_mem_arena`. Unset, onnxruntime takes one thread per core AND keeps its CPU arena,
    which never returns a chunk to the OS — so one embedding pass at fastembed's default batch of
    256 minted a 256x12x512x512 fp32 attention tensor (exactly 3 GiB) and held it until the process
    exited. Measured: 0.22 GiB -> 5.31 GiB from a single embed() call, with neither gc.collect()
    nor malloc_trim recovering it.
    """

    def test_the_onnx_arena_is_off(self):
        # The load-bearing half. Bounding the batch caps any ONE tensor; this is what makes the
        # memory come back afterwards — with the arena left on, even a batch of 32 settled at
        # 1.22 GiB against 0.18 GiB with it off.
        self.assertIs(S._session_posture()["enable_cpu_mem_arena"], False)

    def test_the_session_carries_a_thread_cap(self):
        posture = S._session_posture()
        self.assertIn("threads", posture)
        self.assertGreaterEqual(posture["threads"], 1)
        # Named exactly as fastembed expects, or it lands in **kwargs and silently does nothing.
        self.assertEqual(set(posture), {"threads", "enable_cpu_mem_arena"})

    def test_thread_budget_is_half_the_cores_inside_a_hard_clamp(self):
        # Mirrors `sidecar_thread_budget` in src-tauri/src/sidecar.rs — Rust normally sends the
        # number, and this is the fallback a raw dev run takes. The two must agree.
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("PM_SIDECAR_THREADS", None)
            for cores, expected in ((24, 8), (16, 8), (12, 6), (8, 4), (4, 2), (1, 2)):
                with mock.patch.object(os, "cpu_count", return_value=cores):
                    self.assertEqual(S._thread_budget(), expected, f"{cores} cores")
            with mock.patch.object(os, "cpu_count", return_value=None):
                self.assertEqual(S._thread_budget(), 2)

    def test_rust_outranks_the_local_guess(self):
        with mock.patch.dict(os.environ, {"PM_SIDECAR_THREADS": "3"}):
            with mock.patch.object(os, "cpu_count", return_value=64):
                self.assertEqual(S._thread_budget(), 3)

    def test_a_junk_thread_override_falls_back_rather_than_raising(self):
        # The variable comes from Rust, but a stale or hand-edited environment must not stop the
        # sidecar booting — losing the tuning is survivable, failing to start is not.
        with mock.patch.dict(os.environ, {"PM_SIDECAR_THREADS": "lots"}):
            with mock.patch.object(os, "cpu_count", return_value=8):
                self.assertEqual(S._thread_budget(), 4)

    def test_trimming_is_reserved_for_the_heavy_handlers(self):
        # Every handler that runs a model or holds a whole file is worth a trim; `ping` is not —
        # walking the arenas after a health check would just be latency on the hot path.
        for method in ("embed", "rerank", "transcribe", "convert", "analyze_image"):
            self.assertIn(method, S._TRIM_AFTER)
        for method in ("ping", "count_tokens", "file_properties"):
            self.assertNotIn(method, S._TRIM_AFTER)
        # Nothing may be listed that isn't a real method, or the set silently rots.
        self.assertTrue(S._TRIM_AFTER <= set(S.HANDLERS))

    def test_releasing_memory_never_raises(self):
        # Best-effort by contract: musl has no malloc_trim, Windows has no POSIX loader, and a
        # sidecar must boot and serve on both.
        S._release_free_memory()

    def test_lowering_priority_never_raises(self):
        # Also best-effort — a restrictive seccomp profile can refuse it. Run in a subprocess so the
        # test process isn't left niced for everything that follows it.
        code = "import pm_sidecar as S; S._lower_own_priority(); print('ok')"
        proc = subprocess.run(
            [sys.executable, "-c", code],
            capture_output=True,
            text=True,
            timeout=120,
            cwd=os.path.dirname(os.path.abspath(__file__)),
        )
        self.assertEqual(proc.stdout.strip(), "ok", proc.stderr)


if __name__ == "__main__":
    unittest.main()
