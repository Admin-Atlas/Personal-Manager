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


if __name__ == "__main__":
    unittest.main()
