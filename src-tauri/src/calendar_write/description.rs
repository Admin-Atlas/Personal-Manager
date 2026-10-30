// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What PM's editor can change in an event description (C19). Google's `description` "can contain
//! HTML" (events reference). The webview reads and writes it (src/lib/calendarEdit/descriptionHtml.ts);
//! this side answers the gate's two questions about Google's fresh copy without building anything: is
//! it formatted at all, and is every piece of formatting in it one the editor writes back?
//!
//! Both answers are one scan each, in linear time, and agree with the webview row for row on the
//! shared fixture (src/lib/calendarEdit/descriptionHtml.fixture.json). [`pm_can_edit`] is laxer than
//! the webview, which also checks links, styles and nesting: the backend is never the stricter side,
//! so a description the editor can write back is never refused here.

/// The tags the editor reads and writes; the shared fixture's `tags`, and the webview's
/// `EDITABLE_TAGS`.
pub const EDITABLE_TAGS: &[&str] = &[
    "a",
    "b",
    "br",
    "div",
    "em",
    "html-blob",
    "i",
    "li",
    "ol",
    "p",
    "span",
    "strong",
    "u",
    "ul",
];

/// Whether a description is formatted rather than plain text: a tag (`<b>`, `</p>`, `<a href=…>`)
/// with a `>` after it, or a character reference (`&amp;`, `&sup2;`, `&#39;`, `&#x27;`). The webview's
/// `looksLikeHtml` exactly (the fixture's `html` column pins both). The last `>` is found once, so a
/// run of `<a<a<a…` is linear.
pub fn is_html(s: &str) -> bool {
    let b = s.as_bytes();
    let last_gt = b.iter().rposition(|&c| c == b'>');
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'<' => {
                let name = if b.get(i + 1) == Some(&b'/') {
                    i + 2
                } else {
                    i + 1
                };
                if b.get(name).is_some_and(u8::is_ascii_alphabetic)
                    && last_gt.is_some_and(|gt| gt > name)
                {
                    return true;
                }
            }
            b'&' if is_reference(b, i) => return true,
            _ => {}
        }
    }
    false
}

/// A character reference at `from` (an `&`): a letter and up to seven letters or digits, `#` and
/// digits, or `#x` (or `#X`) and hex digits; then `;`. Any name, known or not: neither side keeps
/// the table of names, and both must agree.
fn is_reference(b: &[u8], from: usize) -> bool {
    let mut j = from + 1;
    if b.get(j).is_some_and(u8::is_ascii_alphabetic) {
        j += 1;
        while j < b.len() && j <= from + 8 && b[j].is_ascii_alphanumeric() {
            j += 1;
        }
        return b.get(j) == Some(&b';');
    }
    if b.get(j) != Some(&b'#') {
        return false;
    }
    j += 1;
    let hex = matches!(b.get(j), Some(b'x' | b'X'));
    if hex {
        j += 1;
    }
    let digits = j;
    while j < b.len()
        && if hex {
            b[j].is_ascii_hexdigit()
        } else {
            b[j].is_ascii_digit()
        }
    {
        j += 1;
    }
    j > digits && b.get(j) == Some(&b';')
}

/// Whether PM's editor can change this description with nothing Google shows lost: plain text, or
/// formatting whose every tag is one it writes, with no comment or declaration and no tag the text
/// ends inside. Never stricter than the webview, which also checks links, styles and nesting; a
/// description the webview can't write back is never sent unless the user chose to replace it.
pub fn pm_can_edit(s: &str) -> bool {
    if !is_html(s) {
        return true;
    }
    let b = s.as_bytes();
    let mut at = find(b, b'<', 0);
    while let Some(lt) = at {
        let next = b.get(lt + 1).copied();
        // A comment, `<!doctype…>` or `<?xml…?>`: something another app may read, never written back.
        if matches!(next, Some(b'!' | b'?')) {
            return false;
        }
        let closing = next == Some(b'/');
        let name = if closing { lt + 2 } else { lt + 1 };
        if !b.get(name).is_some_and(u8::is_ascii_alphabetic) {
            // `</` and no letter is a comment to the webview's reader; any other `<` is text.
            if closing {
                return false;
            }
            at = find(b, b'<', lt + 1);
            continue;
        }
        let mut j = name;
        while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'/' && b[j] != b'>' {
            j += 1;
        }
        let tag = &b[name..j];
        if !EDITABLE_TAGS
            .iter()
            .any(|t| t.as_bytes().eq_ignore_ascii_case(tag))
        {
            return false;
        }
        let Some(end) = tag_end(b, j) else {
            return false;
        };
        at = find(b, b'<', end);
    }
    true
}

/// Just past the `>` ending a tag whose name ends at `from`, found as the webview finds it: a quote
/// opens a value only after `=` (spaces between allowed), and a `>` inside a value doesn't count.
/// `None` when the text ends inside the tag.
fn tag_end(b: &[u8], from: usize) -> Option<usize> {
    let mut after_equals = false;
    let mut k = from;
    while k < b.len() {
        let c = b[k];
        if c == b'>' {
            return Some(k + 1);
        }
        if (c == b'"' || c == b'\'') && after_equals {
            k = find(b, c, k + 1)?;
            after_equals = false;
        } else if c == b'=' {
            after_equals = true;
        } else if !c.is_ascii_whitespace() {
            after_equals = false;
        }
        k += 1;
    }
    None
}

/// The first `c` in `b` at or after `from`.
fn find(b: &[u8], c: u8, from: usize) -> Option<usize> {
    b.get(from..)?
        .iter()
        .position(|&x| x == c)
        .map(|i| from + i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::time::{Duration, Instant};

    /// The rows both languages read (C19 §2.9): the webview checks `html` and `supported`, this side
    /// `html` and `pm_can_edit`, and the webview checks that every supported row passes
    /// `pm_can_edit`.
    const FIXTURE: &str =
        include_str!("../../../src/lib/calendarEdit/descriptionHtml.fixture.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).unwrap()
    }

    #[test]
    fn pm_can_edit_reads_the_shared_fixture() {
        let fixture = fixture();
        let rows = fixture["rows"].as_array().unwrap();
        assert!(rows.len() > 40, "the fixture holds its rows");
        for (k, row) in rows.iter().enumerate() {
            let description = row["description"].as_str().unwrap();
            let n = k + 1;
            assert!(
                is_html(description) == row["html"].as_bool().unwrap(),
                "row F{n}: is_html"
            );
            assert!(
                pm_can_edit(description) == row["pm_can_edit"].as_bool().unwrap(),
                "row F{n}: pm_can_edit"
            );
            // Never stricter than the webview: what it can write back, this side lets through.
            if row["supported"].as_bool().unwrap() {
                assert!(pm_can_edit(description), "row F{n}: supported, so editable");
            }
        }
    }

    #[test]
    fn the_editable_tags_are_the_fixtures() {
        let fixture = fixture();
        let tags: Vec<&str> = fixture["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert_eq!(tags, EDITABLE_TAGS);
    }

    /// `patch::description_is_html`'s table, moved here (C19 §9), with the two references it read as
    /// plain: hex, and a name with a digit.
    #[test]
    fn is_html_reads_tags_and_references() {
        for html in [
            "<b>Agenda</b>",
            "line one<br>line two",
            "see <a href=\"https://x\">this</a>",
            "</p>",
            "fish &amp; chips",
            "it&#39;s",
            "a&nbsp;b",
            "&#x27;quoted&#x27;",
            "&#X27;",
            "x&sup2;",
            "&abcdefgh;",
            "&#1234567890;",
            "<B>",
            "<x>",
        ] {
            assert!(is_html(html), "{html}");
        }
        for plain in [
            "a < b and c > d",
            "Q&A at 3",
            "5<6",
            "just notes",
            "",
            "café ☕ <",
            "a<!-- note -->b",
            "&abcdefghi;",
            "&1a;",
            "&#;",
            "&#x;",
            "&#xg;",
            "&#12",
            "&amp",
            "< b>",
            "</ b>",
            "<b",
            "> <b",
        ] {
            assert!(!is_html(plain), "{plain}");
        }
    }

    /// A run of `<a` with no `>` after it was quadratic when every `<` searched the rest of the text
    /// for one (C19 §5.1).
    #[test]
    fn is_html_finds_the_last_gt_once() {
        for hostile in [
            "<a".repeat(150_000),
            format!(">{}", "<a".repeat(150_000)),
            "&#1".repeat(100_000),
            "&a".repeat(150_000),
        ] {
            let started = Instant::now();
            assert!(!is_html(&hostile));
            assert!(pm_can_edit(&hostile));
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "telling formatting from plain text is linear"
            );
        }
    }

    #[test]
    fn pm_can_edit_is_linear() {
        for hostile in [
            format!("{}>", "<a".repeat(150_000)),
            format!("{}>", "<b ".repeat(100_000)),
            format!("<b>{}", "<b x=\"".repeat(100_000)),
            format!("<b>{}", "<b>x</b>".repeat(50_000)),
            format!("<b>{}", "< < <".repeat(60_000)),
        ] {
            let started = Instant::now();
            let _ = pm_can_edit(&hostile);
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "checking formatting is linear"
            );
        }
    }

    #[test]
    fn markup_in_quotes_comments_or_a_cut_reads_as_the_webview_reads_it() {
        for (description, editable, why) in [
            ("<b title=\"x>y\">a</b>", true, "a > in a quoted value"),
            (
                "<b title='<table>'>a</b>",
                true,
                "a tag in a quoted value is words",
            ),
            ("<b title = \"x><h2>\">a</b>", true, "spaces around ="),
            (
                "<b title=\"x><h2>\">a</b>",
                true,
                "a tag after a > in a quoted value is words",
            ),
            ("<b title='x><h2>'>a</b>", true, "single quotes as double"),
            (
                "<b \"x><h2>\">a</b>",
                false,
                "a quote opens a value only after =",
            ),
            (
                "<b a=x \"y><h2>\">z</b>",
                false,
                "an unquoted value ends, and the quote after it opens nothing",
            ),
            (
                "<b title=x <h2>a</b>",
                true,
                "a < inside a tag is part of it",
            ),
            (
                "<b title=\"x>a</b>",
                false,
                "a quote never closed: the text ends in the tag",
            ),
            (
                "<b>a</b> <i class=\"x",
                false,
                "the text ends inside a value",
            ),
            ("<b>a</b> <u class=x", false, "the text ends inside a tag"),
            ("<b>a</b><!-- note -->b", false, "a comment"),
            ("<b>a</b><!doctype html>", false, "a declaration"),
            (
                "<?xml version=\"1.0\"?><b>x</b>",
                false,
                "a processing instruction",
            ),
            ("<b>a</b></ x>", false, "</ and no letter is a comment"),
            ("<b>a</b> < b", true, "a < that opens nothing is words"),
            ("<b>1 <2</b>", true, "a < before a digit is words"),
            ("<B>Upper</B><BR>", true, "names in any case"),
            ("<STRONG>a</STRONG>", true, "an alias in capitals"),
            ("<b>x</b><h2>y</h2>", false, "a heading"),
            ("<b>x</b><br/>", true, "a slash ends the name"),
            ("<html-blob>a</html-blob>", true, "Google's wrapper"),
            ("<span\tstyle=\"x\">a</span>", true, "a tab ends the name"),
            ("<bb>x</bb>", false, "a name that only starts like one"),
            (
                "<b>x</b><https://example.com>",
                false,
                "an address in angle brackets",
            ),
            (
                "<b>x</b><ann@example.com>",
                false,
                "a mail address in angle brackets",
            ),
        ] {
            assert!(pm_can_edit(description) == editable, "{why}");
        }
        // Plain text is editable whatever it holds.
        assert!(pm_can_edit("a<!-- note -->b"));
        assert!(pm_can_edit("5 < 6 & 7 > 3"));
    }

    /// A small seeded generator, so the property test needs no dependency and fails the same way
    /// every run.
    struct Lcg(u64);
    impl Lcg {
        fn below(&mut self, n: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % n
        }
    }

    /// The webview's writer (C19 §2.5), reduced to what it can put out: text and an `href` through
    /// the four escapes, `<br>`, and the editor's marks, blocks and links, nested as the strict
    /// reader allows.
    fn escaped(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    /// `inline`: inside a paragraph, a mark or a link, where no block may go.
    fn write_nodes(rng: &mut Lcg, depth: u32, inline: bool, in_link: bool, out: &mut String) {
        const WORDS: [&str; 8] = [
            "a < b",
            "Tom & Jerry",
            "say \"hi\"",
            "x > y",
            "<b>",
            "&amp;",
            "café",
            "plain",
        ];
        let count = 1 + rng.below(4);
        for _ in 0..count {
            let pick = if depth >= 5 { 0 } else { rng.below(10) };
            match pick {
                0..=2 => out.push_str(&escaped(WORDS[rng.below(8) as usize])),
                3 => out.push_str("<br>"),
                4..=6 => {
                    let tag = ["b", "i", "u"][rng.below(3) as usize];
                    out.push_str(&format!("<{tag}>"));
                    write_nodes(rng, depth + 1, true, in_link, out);
                    out.push_str(&format!("</{tag}>"));
                }
                7 if !in_link => {
                    let href = [
                        "https://example.com/?a=1&b=\"2\"",
                        "mailto:someone@example.com",
                        "https://example.com/<x>",
                    ][rng.below(3) as usize];
                    out.push_str(&format!("<a href=\"{}\">", escaped(href)));
                    write_nodes(rng, depth + 1, true, true, out);
                    out.push_str("</a>");
                }
                8 if !inline => {
                    let list = ["ul", "ol"][rng.below(2) as usize];
                    out.push_str(&format!("<{list}>"));
                    for _ in 0..1 + rng.below(3) {
                        out.push_str("<li>");
                        write_nodes(rng, depth + 1, false, false, out);
                        out.push_str("</li>");
                    }
                    out.push_str(&format!("</{list}>"));
                }
                9 if !inline => {
                    out.push_str("<p>");
                    write_nodes(rng, depth + 1, true, false, out);
                    out.push_str("</p>");
                }
                _ => out.push_str(&escaped(WORDS[rng.below(8) as usize])),
            }
        }
    }

    /// C19 critique C1-L12: whatever the editor writes, the backend lets it save.
    #[test]
    fn everything_the_editor_writes_pm_can_edit() {
        let mut rng = Lcg(0x5eed_c19c);
        for k in 0..500 {
            let mut out = String::new();
            write_nodes(&mut rng, 0, false, false, &mut out);
            assert!(pm_can_edit(&out), "generated description {k}");
        }
    }
}
