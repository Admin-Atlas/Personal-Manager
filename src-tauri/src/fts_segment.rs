// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CJK-aware FTS tokenisation for multilingual vaults (audit F-33).
//!
//! `chunks_fts` is `fts5(content)` with SQLite's default `unicode61` tokenizer, which breaks a
//! run into tokens only at whitespace/punctuation. A space-less run of CJK ideographs (or kana /
//! Hangul / Thai …) therefore collapses into a **single** token, so a keyword query for any
//! sub-span never matches and hybrid retrieval silently degrades to vector-only — exactly for the
//! multilingual vaults the `intfloat/multilingual-e5-large` embedder serves.
//!
//! Rather than change the tokenizer (which would be a non-additive rewrite of an existing derived
//! index, and `trigram` would miss 2-char queries), we keep `unicode61` and pre-segment the text
//! we feed it: each non-space-segmented run becomes overlapping character **bigrams**, so
//! `unicode61` sees each bigram as its own token. The **same** function runs at index time
//! ([`fts_tokens`] joined by spaces → the stored content) and at query time (each token quoted →
//! an OR-ed `MATCH`), so the two halves can never drift apart. Already-space-delimited scripts
//! (Latin, Cyrillic, Greek, Arabic, digits) pass through as whole words, so an English or Russian
//! term keeps matching exactly as it did before — and this module is invoked only when the vault's
//! embedder is multilingual, so the default English path is byte-for-byte unchanged.
//!
//! Pure and deterministic: no DB, no model, no I/O — trivially unit-testable, and the shared
//! source of truth that keeps index-time and query-time tokenisation identical.

/// Whether `c` is written in a script that omits spaces between words, so FTS needs a
/// character-level fallback. Deliberately scoped to the non-segmented scripts: Latin, Cyrillic,
/// Greek, Arabic, etc. are space-delimited and `unicode61` already tokenises them word-by-word,
/// so bigramming them would only *lose* recall (an indexed `hello` token would no longer match
/// the whole word). Covers the CJK ideograph blocks + Japanese kana + Korean Hangul + Thai — the
/// scripts a multilingual vault realistically holds.
fn is_segmentable(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF        // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF      // CJK Unified Ideographs
        | 0xF900..=0xFAFF      // CJK Compatibility Ideographs
        | 0x3040..=0x309F      // Hiragana
        | 0x30A0..=0x30FF      // Katakana
        | 0x31F0..=0x31FF      // Katakana Phonetic Extensions
        | 0xAC00..=0xD7A3      // Hangul Syllables
        | 0x0E00..=0x0E7F      // Thai
        | 0x20000..=0x2A6DF    // CJK Unified Ideographs Extension B
        | 0x2A700..=0x2EBEF    // CJK Unified Ideographs Extensions C–F
    )
}

/// Split `text` into FTS-matchable tokens. Runs of a [`is_segmentable`] script become overlapping
/// character bigrams (a length-1 run emits the lone character, so a single-glyph term stays
/// findable); every other maximal run of alphanumerics (any other script, digits) is emitted as a
/// whole word; punctuation, whitespace and emoji are separators and drop out — mirroring what
/// `unicode61` itself does, so an ASCII/Latin input yields the same tokens the raw index would.
///
/// Tokens are returned **case-preserved**: the index side inserts them and lets `unicode61`
/// lowercase on write, while the query side lowercases each token as it quotes it — so both sides
/// converge on the same lowercased tokens.
pub(crate) fn fts_tokens(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if is_segmentable(chars[i]) {
            let start = i;
            while i < chars.len() && is_segmentable(chars[i]) {
                i += 1;
            }
            push_bigrams(&chars[start..i], &mut out);
        } else if chars[i].is_alphanumeric() {
            // A whole word in a space-delimited script (Latin, Cyrillic, …) or a digit run. Stops
            // at the first segmentable char, so "GPT模型" splits into the word "GPT" then the CJK
            // run "模型" — without that split unicode61 would fuse them into one unmatchable token.
            let start = i;
            while i < chars.len() && chars[i].is_alphanumeric() && !is_segmentable(chars[i]) {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
        } else {
            i += 1; // separator
        }
    }
    out
}

/// Push the overlapping bigrams of one segmentable run. A single-character run emits that
/// character alone (there is no bigram to form), keeping a one-glyph run findable.
fn push_bigrams(run: &[char], out: &mut Vec<String>) {
    if run.len() == 1 {
        out.push(run[0].to_string());
        return;
    }
    for w in run.windows(2) {
        out.push(w.iter().collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_passes_through_as_whole_words() {
        // The English path must be untouched: same tokens the raw unicode61 index produces.
        assert_eq!(fts_tokens("hello world"), vec!["hello", "world"]);
        assert_eq!(
            fts_tokens("the launch is May 1"),
            vec!["the", "launch", "is", "May", "1"]
        );
    }

    #[test]
    fn space_delimited_non_latin_is_not_bigrammed() {
        // Cyrillic is space-segmented, so unicode61 already tokenises it — bigramming would lose
        // recall. It must stay whole-word, proving is_segmentable is scoped to non-spaced scripts.
        assert_eq!(fts_tokens("привет мир"), vec!["привет", "мир"]);
    }

    #[test]
    fn cjk_run_becomes_overlapping_bigrams() {
        assert_eq!(
            fts_tokens("東京タワー"),
            vec!["東京", "京タ", "タワ", "ワー"]
        );
        assert_eq!(fts_tokens("机器学习"), vec!["机器", "器学", "学习"]);
        assert_eq!(fts_tokens("한국어"), vec!["한국", "국어"]);
    }

    #[test]
    fn mixed_script_run_splits_at_the_boundary() {
        // The whole point: a Latin token adjacent to a CJK run must not fuse into one token.
        assert_eq!(
            fts_tokens("GPT模型很好"),
            vec!["GPT", "模型", "型很", "很好"]
        );
        assert_eq!(fts_tokens("3个苹果"), vec!["3", "个苹", "苹果"]);
    }

    #[test]
    fn single_char_run_emits_the_char_alone() {
        assert_eq!(fts_tokens("水"), vec!["水"]);
        // A lone CJK char between separators still becomes its own token.
        assert_eq!(fts_tokens("买 水 果"), vec!["买", "水", "果"]);
    }

    #[test]
    fn punctuation_and_emoji_are_separators() {
        assert_eq!(fts_tokens("北京，上海！"), vec!["北京", "上海"]);
        assert_eq!(fts_tokens("好👍的"), vec!["好", "的"]);
        assert!(fts_tokens("！？。 …").is_empty());
    }

    #[test]
    fn index_and_query_shapes_agree() {
        // Index side joins with spaces; a query bigram must equal one of those space-delimited
        // tokens, which is what makes the FTS MATCH land. (unicode61 lowercases on both sides.)
        let indexed = fts_tokens("学习机器学习").join(" ");
        assert!(indexed.split(' ').any(|t| t == "机器"));
        assert_eq!(fts_tokens("机器"), vec!["机器"]);
    }
}
