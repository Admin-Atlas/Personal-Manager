// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The splitter (spec §21.4 — retrieval foundation, PR 1): turns a Markdown document into
//! the chunks that get embedded, indexed, and cited. It sits behind a [`Splitter`] trait so
//! a non-Markdown source (Stage 3 Drive/Gmail, Stage 4 screen/location) can get its own
//! splitter later without touching the schema or the embedder.
//!
//! The one implementation today, [`RecursiveSplitter`], is:
//! - **structure-aware** — never splits inside a fenced code block or mid-table-row, and
//!   tracks the heading breadcrumb in force at each chunk;
//! - **token-sized** — chunks are packed to a token budget (the active embedder's, via an
//!   injected [`TokenCounter`]), never a char budget, so dense text isn't silently
//!   truncated by the embedder;
//! - **heading-prepended** — the embedded/indexed text carries the doc title + heading
//!   breadcrumb (the free recall win) while the displayed/cited text stays clean;
//! - **two-tier** — consecutive leaf chunks under a heading are grouped under a structural
//!   *parent* chunk (stored, never embedded) so a future retrieval strategy can widen a hit
//!   to its section without re-chunking;
//! - **deterministic** — the same body + [`SPLITTER_VERSION`] always yields identical chunks
//!   and identical stable uids, so a Rebuild reproduces the index exactly (and a Stage-5 sync
//!   peer can regenerate it).

use sha2::{Digest, Sha256};

use crate::error::Result;

/// Target leaf-chunk size, in tokens — comfortably under the 512-token window of the default
/// embedder so nothing is truncated, small enough for precise retrieval.
pub const CHUNK_TARGET_TOKENS: usize = 256;
/// Token overlap carried between adjacent leaf chunks so meaning straddling a boundary stays
/// retrievable.
pub const CHUNK_OVERLAP_TOKENS: usize = 32;
/// The splitter implementation version. Bump on any change to chunk boundaries; the retrieval
/// stamp folds this in, so a bump prompts a one-time Rebuild. (Bumped to 2 with the token-count
/// padding fix: the counter change shifts every chunk boundary, so existing vaults must rechunk.)
pub const SPLITTER_VERSION: u32 = 2;
/// The boundary-strategy id recorded in the retrieval stamp.
pub const BOUNDARY_STRATEGY: &str = "recursive-structure-v1";

/// Last-resort fallback only: when a single line exceeds the token target and has no internal
/// boundary to break on, a rough chars-per-token figure for English.
const APPROX_CHARS_PER_TOKEN: usize = 4;

/// Counts tokens for a batch of strings with the active embedder's tokenizer. Injected so the
/// splitter core stays pure and Python-free in tests (production wires it to the sidecar via
/// the model gateway; tests use a whitespace counter). Implementations MUST be called with the
/// whole document's units in one batch — one round-trip per document, never one per boundary.
pub trait TokenCounter {
    fn count(&self, texts: &[&str]) -> Result<Vec<usize>>;
}

/// Whether a chunk is an embedded leaf or a structural-only parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    Leaf,
    Parent,
}

impl ChunkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChunkKind::Leaf => "leaf",
            ChunkKind::Parent => "parent",
        }
    }
}

/// One chunk produced by the splitter.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Stable, deterministic id — reproduced exactly on a rebuild of the same document.
    pub uid: String,
    /// The parent chunk's uid for a leaf inside a multi-chunk section; `None` otherwise.
    pub parent_uid: Option<String>,
    pub kind: ChunkKind,
    /// The heading in force where this chunk starts (for display + the `chunks.heading` column).
    pub heading: Option<String>,
    /// Clean text shown to the user and used for citations.
    pub display_content: String,
    /// Title + heading breadcrumb + body — the text embedded and FTS-indexed. Empty for parents
    /// (they are never embedded).
    pub embed_content: String,
    /// Byte offsets of this chunk's span in the original body.
    pub start_offset: usize,
    pub end_offset: usize,
}

/// Per-document context the splitter needs: the title (for the breadcrumb) and the content hash
/// (folded into the stable uid so ids survive a rebuild).
pub struct SplitMeta<'a> {
    pub title: &'a str,
    pub content_hash: &'a str,
}

/// A document splitter. One impl today ([`RecursiveSplitter`]); the trait is the seam for
/// per-source splitters later.
pub trait Splitter {
    fn split(&self, body: &str, meta: &SplitMeta, counter: &dyn TokenCounter)
        -> Result<Vec<Chunk>>;
}

/// The recursive, structure-aware, token-sized Markdown splitter.
pub struct RecursiveSplitter {
    pub target_tokens: usize,
    pub overlap_tokens: usize,
}

impl Default for RecursiveSplitter {
    fn default() -> Self {
        RecursiveSplitter {
            target_tokens: CHUNK_TARGET_TOKENS,
            overlap_tokens: CHUNK_OVERLAP_TOKENS,
        }
    }
}

/// A structural block of the body — a heading line, a fenced code block, a table, or a
/// paragraph. Blocks never straddle a fenced-code boundary (blank lines inside a fence don't
/// break it) so code and tables stay atomic up to the size limit.
struct Block {
    start: usize,
    end: usize,
    text: String,
    is_heading: bool,
    heading_level: usize,
    heading_label: String,
}

/// A block plus its resolved context: the heading breadcrumb in force and which section it
/// belongs to (a section starts at each heading), and its token count (filled by one batch).
struct Unit {
    text: String,
    start: usize,
    end: usize,
    heading_path: Vec<String>,
    immediate_heading: Option<String>,
    section: usize,
    tokens: usize,
}

/// An assembled leaf before uid/parent assignment.
struct LeafDraft {
    section: usize,
    heading_path: Vec<String>,
    immediate_heading: Option<String>,
    display: String,
    start: usize,
    end: usize,
}

impl Splitter for RecursiveSplitter {
    fn split(
        &self,
        body: &str,
        meta: &SplitMeta,
        counter: &dyn TokenCounter,
    ) -> Result<Vec<Chunk>> {
        let blocks = parse_blocks(body);
        let mut units = resolve_units(blocks);
        if units.is_empty() {
            return Ok(Vec::new());
        }

        // One batch token count for the whole document (never per boundary).
        let texts: Vec<&str> = units.iter().map(|u| u.text.as_str()).collect();
        let counts = counter.count(&texts)?;
        for (u, c) in units.iter_mut().zip(counts) {
            u.tokens = c.max(1);
        }

        let leaves = self.pack(&units);
        Ok(self.assemble(leaves, meta))
    }
}

impl RecursiveSplitter {
    /// Greedily pack units into leaf drafts, flushing at section boundaries (a chunk never
    /// crosses a heading) and when the token budget is reached, carrying a token-sized overlap
    /// of trailing whole units within a section. Oversized single units are split first.
    fn pack(&self, units: &[Unit]) -> Vec<LeafDraft> {
        let mut leaves: Vec<LeafDraft> = Vec::new();
        let mut cur: Vec<usize> = Vec::new();
        let mut cur_tokens = 0usize;
        let mut cur_section: Option<usize> = None;

        let flush = |leaves: &mut Vec<LeafDraft>, cur: &Vec<usize>| {
            if let Some(d) = draft_from(units, cur) {
                leaves.push(d);
            }
        };

        for (i, u) in units.iter().enumerate() {
            // A heading begins a new section → close the current chunk so it never crosses it.
            if let Some(sec) = cur_section {
                if sec != u.section && !cur.is_empty() {
                    flush(&mut leaves, &cur);
                    cur.clear();
                    cur_tokens = 0;
                }
            }
            cur_section = Some(u.section);

            if u.tokens > self.target_tokens {
                // The unit alone overflows the budget: flush what we have, then break the unit
                // at safe boundaries (lines — never mid-line, so never mid-fence/mid-row).
                if !cur.is_empty() {
                    flush(&mut leaves, &cur);
                    cur.clear();
                    cur_tokens = 0;
                }
                leaves.extend(split_oversized(u, self.target_tokens));
                continue;
            }

            if cur_tokens + u.tokens > self.target_tokens && !cur.is_empty() {
                flush(&mut leaves, &cur);
                // Re-seed with trailing whole units summing up to the overlap budget.
                let seed = overlap_seed(units, &cur, self.overlap_tokens);
                cur = seed;
                cur_tokens = cur.iter().map(|&j| units[j].tokens).sum();
            }
            cur.push(i);
            cur_tokens += u.tokens;
        }
        if !cur.is_empty() {
            flush(&mut leaves, &cur);
        }
        leaves
    }

    /// Turn leaf drafts into final [`Chunk`]s: prepend the title + breadcrumb into the embedded
    /// text, group multi-leaf sections under a structural parent (stored, never embedded), and
    /// assign deterministic stable uids. Parents precede their children so the caller can
    /// resolve `parent_uid` → row id by inserting in order.
    fn assemble(&self, leaves: Vec<LeafDraft>, meta: &SplitMeta) -> Vec<Chunk> {
        // Group leaves by section, preserving document order.
        let mut sections: Vec<(usize, Vec<LeafDraft>)> = Vec::new();
        for leaf in leaves {
            match sections.last_mut() {
                Some((sec, group)) if *sec == leaf.section => group.push(leaf),
                _ => sections.push((leaf.section, vec![leaf])),
            }
        }

        let mut out: Vec<Chunk> = Vec::new();
        for (section, group) in sections {
            let multi = group.len() > 1;
            // A parent only when the section actually splits into >1 leaf (else it would just
            // duplicate the single leaf).
            let parent_uid = if multi {
                let start = group.iter().map(|l| l.start).min().unwrap_or(0);
                let end = group.iter().map(|l| l.end).max().unwrap_or(0);
                let heading_path = group[0].heading_path.clone();
                let uid = make_uid(
                    meta.content_hash,
                    &format!("p|{section}|{}", heading_path.join(">")),
                );
                let display = group
                    .iter()
                    .map(|l| l.display.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                out.push(Chunk {
                    uid: uid.clone(),
                    parent_uid: None,
                    kind: ChunkKind::Parent,
                    heading: group[0].immediate_heading.clone(),
                    display_content: display,
                    embed_content: String::new(), // parents are never embedded
                    start_offset: start,
                    end_offset: end,
                });
                Some(uid)
            } else {
                None
            };

            for (idx, leaf) in group.into_iter().enumerate() {
                let structural = format!("{}|{}|{}", section, idx, leaf.heading_path.join(">"));
                let uid = make_uid(meta.content_hash, &structural);
                let embed = breadcrumb(meta.title, &leaf.heading_path, &leaf.display);
                out.push(Chunk {
                    uid,
                    parent_uid: parent_uid.clone(),
                    kind: ChunkKind::Leaf,
                    heading: leaf.immediate_heading,
                    display_content: leaf.display,
                    embed_content: embed,
                    start_offset: leaf.start,
                    end_offset: leaf.end,
                });
            }
        }
        out
    }
}

/// Build a leaf draft from accumulated unit indices, or `None` if the text is blank.
fn draft_from(units: &[Unit], idxs: &[usize]) -> Option<LeafDraft> {
    if idxs.is_empty() {
        return None;
    }
    let display = idxs
        .iter()
        .map(|&i| units[i].text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if display.trim().is_empty() {
        return None;
    }
    let first = &units[idxs[0]];
    let start = idxs
        .iter()
        .map(|&i| units[i].start)
        .min()
        .unwrap_or(first.start);
    let end = idxs
        .iter()
        .map(|&i| units[i].end)
        .max()
        .unwrap_or(first.end);
    Some(LeafDraft {
        section: first.section,
        heading_path: first.heading_path.clone(),
        immediate_heading: first.immediate_heading.clone(),
        display,
        start,
        end,
    })
}

/// Trailing whole units of the just-flushed chunk whose token sum fits the overlap budget — the
/// seed for the next chunk so meaning across the boundary stays retrievable.
fn overlap_seed(units: &[Unit], flushed: &[usize], overlap_tokens: usize) -> Vec<usize> {
    if overlap_tokens == 0 {
        return Vec::new();
    }
    let mut seed: Vec<usize> = Vec::new();
    let mut total = 0usize;
    for &i in flushed.iter().rev() {
        let t = units[i].tokens;
        if total + t > overlap_tokens {
            break;
        }
        total += t;
        seed.push(i);
    }
    seed.reverse();
    seed
}

/// Split one oversized unit into leaf drafts at line boundaries (so a code block breaks between
/// lines, never mid-line, and a table breaks between rows, never mid-row), hard-splitting only a
/// single line longer than the budget. Token counts are estimated proportionally from the unit's
/// batch count, so this needs no extra round-trip.
fn split_oversized(u: &Unit, target_tokens: usize) -> Vec<LeafDraft> {
    let unit_chars = u.text.chars().count().max(1);
    let tokens_per_char = u.tokens as f64 / unit_chars as f64;
    let est = |s: &str| -> usize {
        ((s.chars().count() as f64 * tokens_per_char).ceil() as usize).max(1)
    };

    // Pieces = lines, each with its byte offset within the unit; a too-long line is hard-split.
    let mut pieces: Vec<(usize, String)> = Vec::new(); // (byte offset within unit, text)
    let mut off = 0usize;
    for line in u.text.split_inclusive('\n') {
        if est(line) > target_tokens {
            for (rel, seg) in hard_split(line, target_tokens) {
                pieces.push((off + rel, seg));
            }
        } else {
            pieces.push((off, line.to_string()));
        }
        off += line.len();
    }

    // Greedily pack pieces up to the token target.
    let mut leaves = Vec::new();
    let mut buf = String::new();
    let mut buf_start: Option<usize> = None;
    let mut buf_tokens = 0usize;
    for (rel, seg) in pieces {
        let t = est(&seg);
        if buf_tokens + t > target_tokens && !buf.trim().is_empty() {
            push_oversized_leaf(&mut leaves, u, buf_start, &buf);
            buf.clear();
            buf_start = None;
            buf_tokens = 0;
        }
        if buf_start.is_none() {
            buf_start = Some(u.start + rel);
        }
        buf.push_str(&seg);
        buf_tokens += t;
    }
    push_oversized_leaf(&mut leaves, u, buf_start, &buf);
    leaves
}

fn push_oversized_leaf(leaves: &mut Vec<LeafDraft>, u: &Unit, start: Option<usize>, buf: &str) {
    let text = buf.trim();
    if text.is_empty() {
        return;
    }
    let start = start.unwrap_or(u.start);
    leaves.push(LeafDraft {
        section: u.section,
        heading_path: u.heading_path.clone(),
        immediate_heading: u.immediate_heading.clone(),
        display: text.to_string(),
        start,
        end: start + buf.len(),
    });
}

/// Hard-split a single over-budget line into char windows (char-safe). Returns (byte offset
/// within the line, segment). The only place a boundary can fall inside a "line".
fn hard_split(line: &str, target_tokens: usize) -> Vec<(usize, String)> {
    let window = (target_tokens * APPROX_CHARS_PER_TOKEN).max(1);
    let mut out = Vec::new();
    let mut seg = String::new();
    let mut seg_start = 0usize;
    let mut count = 0usize;
    let mut byte = 0usize;
    for ch in line.chars() {
        if count == 0 {
            seg_start = byte;
        }
        seg.push(ch);
        byte += ch.len_utf8();
        count += 1;
        if count >= window {
            out.push((seg_start, std::mem::take(&mut seg)));
            count = 0;
        }
    }
    if !seg.is_empty() {
        out.push((seg_start, seg));
    }
    out
}

/// Parse the body into structural blocks, fence-aware: blank lines break a paragraph, but not a
/// run inside a ``` / ~~~ fence; a heading line is its own block.
fn parse_blocks(body: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut start: Option<usize> = None;
    let mut text = String::new();
    let mut end = 0usize;
    let mut in_fence = false;
    let mut offset = 0usize;

    for line in body.split_inclusive('\n') {
        let line_start = offset;
        let line_len = line.len();
        offset += line_len;

        let stripped = line.trim_end_matches(['\n', '\r']);
        let lead = stripped.trim_start();
        let is_fence = lead.starts_with("```") || lead.starts_with("~~~");
        let is_blank = stripped.trim().is_empty();

        if in_fence {
            if start.is_none() {
                start = Some(line_start);
            }
            text.push_str(line);
            end = line_start + line_len;
            if is_fence {
                in_fence = false; // closing fence
            }
            continue;
        }

        if is_fence {
            if start.is_none() {
                start = Some(line_start);
            }
            text.push_str(line);
            end = line_start + line_len;
            in_fence = true;
            continue;
        }

        if is_blank {
            flush_para(&mut blocks, &mut start, &mut text, end);
            continue;
        }

        if is_heading_line(lead) {
            flush_para(&mut blocks, &mut start, &mut text, end);
            let (level, label) = heading_parts(lead);
            blocks.push(Block {
                start: line_start,
                end: line_start + line_len,
                text: stripped.to_string(),
                is_heading: true,
                heading_level: level,
                heading_label: label,
            });
            continue;
        }

        if start.is_none() {
            start = Some(line_start);
        }
        text.push_str(line);
        end = line_start + line_len;
    }
    flush_para(&mut blocks, &mut start, &mut text, end);
    blocks
}

fn flush_para(blocks: &mut Vec<Block>, start: &mut Option<usize>, text: &mut String, end: usize) {
    if let Some(s) = start.take() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            blocks.push(Block {
                start: s,
                end,
                text: trimmed.to_string(),
                is_heading: false,
                heading_level: 0,
                heading_label: String::new(),
            });
        }
        text.clear();
    }
}

/// Walk blocks, maintaining a heading stack, to attach a breadcrumb + section index to each.
/// A new section starts at every heading (content before the first heading is section 0).
fn resolve_units(blocks: Vec<Block>) -> Vec<Unit> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut section = 0usize;
    let mut seen_heading = false;
    let mut units = Vec::new();

    for b in blocks {
        if b.is_heading {
            while matches!(stack.last(), Some(&(lvl, _)) if lvl >= b.heading_level) {
                stack.pop();
            }
            stack.push((b.heading_level, b.heading_label.clone()));
            section += 1;
            seen_heading = true;
        }
        let path: Vec<String> = stack.iter().map(|(_, l)| l.clone()).collect();
        let immediate = stack.last().map(|(_, l)| l.clone());
        units.push(Unit {
            text: b.text,
            start: b.start,
            end: b.end,
            heading_path: path,
            immediate_heading: immediate,
            section: if seen_heading { section } else { 0 },
            tokens: 0,
        });
    }
    units
}

fn is_heading_line(lead: &str) -> bool {
    let hashes = lead.chars().take_while(|&c| c == '#').count();
    (1..=6).contains(&hashes) && lead[hashes..].starts_with(' ')
}

fn heading_parts(lead: &str) -> (usize, String) {
    let level = lead.chars().take_while(|&c| c == '#').count().min(6);
    let label = lead.trim_start_matches('#').trim().to_string();
    (level, label)
}

/// `title > h1 > h2\n\nbody` — the breadcrumb prepended to the embedded/indexed text. The
/// displayed text never carries it.
fn breadcrumb(title: &str, heading_path: &[String], body: &str) -> String {
    let mut crumbs: Vec<&str> = Vec::new();
    let title = title.trim();
    if !title.is_empty() {
        crumbs.push(title);
    }
    for h in heading_path {
        if !h.trim().is_empty() {
            crumbs.push(h.as_str());
        }
    }
    if crumbs.is_empty() {
        body.to_string()
    } else {
        format!("{}\n\n{}", crumbs.join(" > "), body)
    }
}

/// Deterministic stable chunk id: `sha256(content_hash : splitter_version : structural_path)`,
/// truncated to 16 bytes (32 hex chars). Reproduced exactly on a rebuild of the same document.
fn make_uid(content_hash: &str, structural_path: &str) -> String {
    let mut h = Sha256::new();
    h.update(content_hash.as_bytes());
    h.update(b":");
    h.update(SPLITTER_VERSION.to_string().as_bytes());
    h.update(b":");
    h.update(structural_path.as_bytes());
    let digest = h.finalize();
    hex16(&digest[..16])
}

fn hex16(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic, Python-free token counter for tests: whitespace-delimited words.
    struct WhitespaceCounter;
    impl TokenCounter for WhitespaceCounter {
        fn count(&self, texts: &[&str]) -> Result<Vec<usize>> {
            Ok(texts
                .iter()
                .map(|t| t.split_whitespace().count().max(1))
                .collect())
        }
    }

    fn split(body: &str, title: &str) -> Vec<Chunk> {
        let s = RecursiveSplitter::default();
        let meta = SplitMeta {
            title,
            content_hash: "hash123",
        };
        s.split(body, &meta, &WhitespaceCounter).unwrap()
    }

    fn split_with(body: &str, title: &str, target: usize, overlap: usize) -> Vec<Chunk> {
        let s = RecursiveSplitter {
            target_tokens: target,
            overlap_tokens: overlap,
        };
        let meta = SplitMeta {
            title,
            content_hash: "hash123",
        };
        s.split(body, &meta, &WhitespaceCounter).unwrap()
    }

    #[test]
    fn breadcrumb_is_in_embed_content_not_display_content() {
        let body = "# Finances\n\nThe quarterly revenue rose.";
        let chunks = split(body, "My Notes");
        let leaf = chunks.iter().find(|c| c.kind == ChunkKind::Leaf).unwrap();
        assert!(leaf.embed_content.contains("My Notes"));
        assert!(leaf.embed_content.contains("Finances"));
        assert!(leaf.embed_content.contains("quarterly revenue"));
        // The displayed/cited text stays clean — no title/breadcrumb injected.
        assert!(!leaf.display_content.contains("My Notes"));
        assert!(leaf.display_content.contains("quarterly revenue"));
    }

    #[test]
    fn small_fenced_code_block_is_not_split() {
        let body =
            "Intro paragraph here.\n\n```python\nx = 1\n\ny = 2\nprint(x + y)\n```\n\nAfter.";
        let chunks = split_with(body, "Code", 1000, 0);
        // The whole fenced block lands inside a single chunk's text (blank line inside the
        // fence did not break it, and it was not split across chunks).
        let has_full_fence = chunks.iter().any(|c| {
            c.display_content.contains("x = 1") && c.display_content.contains("print(x + y)")
        });
        assert!(has_full_fence, "fenced code must stay intact: {chunks:#?}");
    }

    #[test]
    fn table_rows_are_not_split_mid_row() {
        let body = "| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |";
        let chunks = split_with(body, "T", 1000, 0);
        // One small table → one chunk containing every row intact.
        assert_eq!(
            chunks.iter().filter(|c| c.kind == ChunkKind::Leaf).count(),
            1
        );
        let leaf = &chunks[0];
        for row in ["| a | b |", "| 1 | 2 |", "| 3 | 4 |"] {
            assert!(leaf.display_content.contains(row), "row split: {row}");
        }
    }

    #[test]
    fn long_text_is_packed_into_token_sized_windows() {
        // 40 paragraphs of 10 words each, no headings; target 50 tokens → several chunks, each
        // comfortably bounded (allowing the last/overlap to vary).
        let para = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let body = vec![para; 40].join("\n\n");
        let chunks = split_with(&body, "Doc", 50, 10);
        let leaves: Vec<_> = chunks
            .iter()
            .filter(|c| c.kind == ChunkKind::Leaf)
            .collect();
        assert!(leaves.len() > 1, "should split into multiple leaves");
        for c in &leaves {
            let words = c.display_content.split_whitespace().count();
            assert!(words <= 60, "leaf too big: {words} words");
        }
    }

    #[test]
    fn oversized_single_block_is_split_to_target() {
        // One paragraph far over the budget, no internal blank lines → must still be broken up.
        let big = (0..400)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = split_with(&big, "Big", 50, 0);
        let leaves: Vec<_> = chunks
            .iter()
            .filter(|c| c.kind == ChunkKind::Leaf)
            .collect();
        assert!(
            leaves.len() > 1,
            "oversized block must split: {} leaves",
            leaves.len()
        );
    }

    #[test]
    fn split_is_deterministic_in_uids_and_offsets() {
        let body = "# A\n\none two three four\n\n## B\n\nfive six seven eight\n\nnine ten";
        let first = split_with(body, "Doc", 6, 2);
        let second = split_with(body, "Doc", 6, 2);
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.uid, b.uid, "uids must be reproducible");
            assert_eq!(
                (a.start_offset, a.end_offset),
                (b.start_offset, b.end_offset)
            );
            assert_eq!(a.kind, b.kind);
        }
    }

    #[test]
    fn multi_leaf_section_gets_a_parent_with_linked_children_and_no_embed_text() {
        // A single section big enough to split into several leaves → one parent over them.
        let para = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let body = format!("# Section\n\n{}", vec![para; 20].join("\n\n"));
        let chunks = split_with(&body, "Doc", 40, 5);
        let parents: Vec<_> = chunks
            .iter()
            .filter(|c| c.kind == ChunkKind::Parent)
            .collect();
        assert_eq!(parents.len(), 1, "exactly one parent for the split section");
        let parent = parents[0];
        assert!(
            parent.embed_content.is_empty(),
            "parents are never embedded"
        );
        let children: Vec<_> = chunks
            .iter()
            .filter(|c| {
                c.kind == ChunkKind::Leaf && c.parent_uid.as_deref() == Some(parent.uid.as_str())
            })
            .collect();
        assert!(children.len() > 1, "parent should link >1 child");
        // The parent's span covers its children.
        assert!(parent.start_offset <= children.iter().map(|c| c.start_offset).min().unwrap());
        assert!(parent.end_offset >= children.iter().map(|c| c.end_offset).max().unwrap());
    }

    #[test]
    fn single_leaf_section_has_no_parent() {
        let body = "# Tiny\n\njust a little text";
        let chunks = split(body, "Doc");
        assert!(chunks.iter().all(|c| c.kind == ChunkKind::Leaf));
        assert!(chunks.iter().all(|c| c.parent_uid.is_none()));
    }

    #[test]
    fn empty_body_yields_no_chunks() {
        assert!(split("   \n\n  ", "Doc").is_empty());
    }
}
