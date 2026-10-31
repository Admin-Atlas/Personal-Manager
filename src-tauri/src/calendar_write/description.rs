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
/// ends inside. Never stricter than the webview, which also checks links, styles and nesting; it
/// answers only for a description the webview read too (the one the user was shown). One the user
/// was never shown answers to [`pm_can_edit_unseen`].
pub fn pm_can_edit(s: &str) -> bool {
    scan(s, |_, _| true)
}

/// [`pm_can_edit`], for a description the webview never read, so its stricter check never ran: a
/// repeating event's own description under an occurrence that shows another (C19 review 1). Every
/// attribute must also be one that draws nothing PM must keep (Google's bookkeeping on links and
/// spans, a `data-` or `aria-` one), a link's address as the webview reads one ([`plain_href`]), or
/// a style of bold, italic, underline and Google's theme colours only, a list's of the colours alone
/// (the webview can't write a list inside a mark). A colour, a class, a direction, a link without an
/// address or an odd link fails, as in the webview. A name written twice is checked both times
/// (browsers read the first). Laxer than the webview still on nesting and on references outside
/// links, which lose nothing PM can't draw.
pub fn pm_can_edit_unseen(s: &str) -> bool {
    scan(s, |tag, attrs| {
        let link = tag.eq_ignore_ascii_case(b"a");
        let list = tag.eq_ignore_ascii_case(b"ul") || tag.eq_ignore_ascii_case(b"ol");
        let every = attrs.iter().all(|(name, value)| match name.as_slice() {
            b"id" | b"target" | b"rel" | b"lang" | b"__is_owner" => true,
            n if n.starts_with(b"data-") || n.starts_with(b"aria-") => true,
            b"href" => link && plain_href(value),
            b"style" => plain_style(value, !list),
            _ => false,
        });
        every && (!link || attrs.iter().any(|(name, _)| name == b"href"))
    })
}

/// Longest name [`plain_ampersands`] looks ahead over: the webview's `MAX_ANY_NAME`.
const MAX_ANY_NAME: usize = 32;

/// The names a browser reads without their `;`, the webview's `LEGACY`, and the fixture's `legacy`.
const LEGACY_NAMES: &[&str] = &[
    "AElig", "AMP", "Aacute", "Acirc", "Agrave", "Aring", "Atilde", "Auml", "COPY", "Ccedil",
    "ETH", "Eacute", "Ecirc", "Egrave", "Euml", "GT", "Iacute", "Icirc", "Igrave", "Iuml", "LT",
    "Ntilde", "Oacute", "Ocirc", "Ograve", "Oslash", "Otilde", "Ouml", "QUOT", "REG", "THORN",
    "Uacute", "Ucirc", "Ugrave", "Uuml", "Yacute", "aacute", "acirc", "acute", "aelig", "agrave",
    "amp", "aring", "atilde", "auml", "brvbar", "ccedil", "cedil", "cent", "copy", "curren", "deg",
    "divide", "eacute", "ecirc", "egrave", "eth", "euml", "frac12", "frac14", "frac34", "gt",
    "iacute", "icirc", "iexcl", "igrave", "iquest", "iuml", "laquo", "lt", "macr", "micro",
    "middot", "nbsp", "not", "ntilde", "oacute", "ocirc", "ograve", "ordf", "ordm", "oslash",
    "otilde", "ouml", "para", "plusmn", "pound", "quot", "raquo", "reg", "sect", "shy", "sup1",
    "sup2", "sup3", "szlig", "thorn", "times", "uacute", "ucirc", "ugrave", "uml", "uuml",
    "yacute", "yen", "yuml",
];

/// A link address the webview's reader takes as it is, its `linkOfHref` on the fixture's `hrefs`:
/// `http://`, `https://` or `mailto:`, a phone number, or an address Google stored without its scheme
/// (a domain, or an email address); nothing with whitespace or a control character in it. References
/// are stricter here (no scheme hides in one): none but `&amp;`, where the webview would decode them.
fn plain_href(value: &[u8]) -> bool {
    let Ok(written) = std::str::from_utf8(value) else {
        return false;
    };
    if !plain_ampersands(value) {
        return false;
    }
    let href = written.replace("&amp;", "&");
    let unbroken = href.chars().all(|c| {
        // JavaScript's `\s` holds U+FEFF, which Unicode's whitespace doesn't.
        c > ' ' && !('\u{7f}'..='\u{9f}').contains(&c) && !c.is_whitespace() && c != '\u{feff}'
    });
    if !unbroken {
        return false;
    }
    let starts = |scheme: &[u8]| {
        href.as_bytes()
            .get(..scheme.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(scheme))
    };
    if starts(b"http://") || starts(b"https://") || starts(b"mailto:") {
        return true;
    }
    if starts(b"tel:") {
        let phone = &href.as_bytes()[b"tel:".len()..];
        return phone.iter().any(u8::is_ascii_digit)
            && phone.iter().all(|c| {
                c.is_ascii_digit()
                    || matches!(
                        c,
                        b'+' | b'-' | b'.' | b'(' | b')' | b',' | b';' | b'#' | b'*'
                    )
            });
    }
    if href.encode_utf16().count() > MAX_TYPED {
        return false;
    }
    if let Some((local, domain)) = href.split_once('@') {
        if !local.is_empty()
            && local
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".!#$&'*+=?^_`{|}~-".contains(&c))
            && is_domain(domain)
        {
            return true;
        }
    }
    let path = href.find(['/', '?', '#']).unwrap_or(href.len());
    is_domain(&href[..path])
}

/// The longest address the webview reads as typed: its `MAX_TYPED`.
const MAX_TYPED: usize = 2000;

/// Two or more labels of letters, digits and `-`, split by `.`.
fn is_domain(s: &str) -> bool {
    s.contains('.')
        && s.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

/// Whether every `&` in an attribute's value is `&amp;` or a plain `&`, as HTML's rule for attributes
/// reads it: no number (`&#38`, with or without its `;`), no name with its `;`, known or not, and no
/// legacy name a browser would draw without it (`&copy` at the end, or before anything but `=`, a
/// letter or a digit). So `?a=1&region=eu&copy=3` is a plain address, as it is to the webview.
fn plain_ampersands(value: &[u8]) -> bool {
    let mut at = find(value, b'&', 0);
    while let Some(amp) = at {
        if !value[amp..].starts_with(b"&amp;") && reads_as_reference(value, amp + 1) {
            return false;
        }
        at = find(value, b'&', amp + 1);
    }
    true
}

/// [`plain_ampersands`] for the characters from `at`, just after an `&`.
fn reads_as_reference(value: &[u8], at: usize) -> bool {
    let rest = &value[at..];
    if rest.first() == Some(&b'#') {
        return match rest.get(1) {
            Some(b'x' | b'X') => rest.get(2).is_some_and(u8::is_ascii_hexdigit),
            next => next.is_some_and(u8::is_ascii_digit),
        };
    }
    if !rest.first().is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let run = rest
        .iter()
        .take(MAX_ANY_NAME)
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    if rest.get(run) == Some(&b';') {
        return true;
    }
    // The longest legacy name is the one a browser takes; a name that runs on, or an `=`, stops it.
    let name = LEGACY_NAMES
        .iter()
        .filter(|name| rest.starts_with(name.as_bytes()))
        .map(|name| name.len())
        .max();
    name.is_some_and(|len| {
        !rest
            .get(len)
            .is_some_and(|c| *c == b'=' || c.is_ascii_alphanumeric())
    })
}

/// A style the webview reads as marks or as nothing: each declaration a colour that is only Google's
/// theme variable and, with `marks`, bold (or a weight of 600 to 900), italic or underline.
fn plain_style(value: &[u8], marks: bool) -> bool {
    let Ok(style) = std::str::from_utf8(value) else {
        return false;
    };
    if style.contains('&') {
        return false;
    }
    style.split(';').all(|part| {
        let declaration = part.trim().to_ascii_lowercase();
        if declaration.is_empty() {
            return true;
        }
        let Some((property, value)) = declaration.split_once(':') else {
            return false;
        };
        let value = value.trim();
        match property.trim() {
            "font-weight" => {
                marks
                    && (matches!(value, "bold" | "bolder")
                        || value.parse::<u16>().is_ok_and(|n| (600..=900).contains(&n)))
            }
            "font-style" => marks && matches!(value, "italic" | "oblique"),
            "text-decoration" | "text-decoration-line" => marks && value == "underline",
            "color" | "background-color" => theme_variable(value),
            _ => false,
        }
    })
}

/// `var(--name)`, spaces allowed inside the brackets: the webview's `VARIABLE`.
fn theme_variable(value: &str) -> bool {
    let Some(inner) = value.strip_prefix("var(").and_then(|v| v.strip_suffix(')')) else {
        return false;
    };
    let Some(name) = inner.trim().strip_prefix("--") else {
        return false;
    };
    !name.is_empty()
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// An attribute: its name lower-cased, its value as written.
type Attr<'a> = (Vec<u8>, &'a [u8]);

/// One scan of a formatted description's tags: every tag one the editor writes, no comment,
/// declaration or tag the text ends inside, and `check(tag, attributes)` true for every start tag.
fn scan(s: &str, mut check: impl FnMut(&[u8], &[Attr<'_>]) -> bool) -> bool {
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
        let mut attrs = Vec::new();
        let Some(end) = tag_end(b, j, &mut attrs) else {
            return false;
        };
        // An end tag's attributes mean nothing.
        if !closing && !check(tag, &attrs) {
            return false;
        }
        at = find(b, b'<', end);
    }
    true
}

/// Just past the `>` ending a tag whose name ends at `from`, read as the webview's tokenizer reads
/// it (`readTag`), each attribute put in `attrs` (its name lower-cased, its value as written):
/// whitespace and `/` between attributes; a name runs to whitespace, `/`, `>` or `=` (one starting
/// with `=` keeps it); after it, whitespace, then `=`, whitespace and a value: a quote opens a quoted
/// one, in which a `>` doesn't count, and anything else an unquoted one, to whitespace or `>`, in
/// which a `=` or a quote means nothing. `None` when the text ends inside the tag.
fn tag_end<'a>(b: &'a [u8], from: usize, attrs: &mut Vec<Attr<'a>>) -> Option<usize> {
    let n = b.len();
    let mut i = from;
    loop {
        while i < n && (is_space(b[i]) || b[i] == b'/') {
            i += 1;
        }
        if i >= n {
            return None;
        }
        if b[i] == b'>' {
            return Some(i + 1);
        }
        let mut j = if b[i] == b'=' { i + 1 } else { i };
        while j < n && !is_space(b[j]) && !matches!(b[j], b'/' | b'>' | b'=') {
            j += 1;
        }
        let name = b[i..j].to_ascii_lowercase();
        i = j;
        let mut k = i;
        while k < n && is_space(b[k]) {
            k += 1;
        }
        let mut value: &[u8] = &[];
        if b.get(k) == Some(&b'=') {
            k += 1;
            while k < n && is_space(b[k]) {
                k += 1;
            }
            if k >= n {
                return None;
            }
            let quote = b[k];
            if quote == b'"' || quote == b'\'' {
                let close = find(b, quote, k + 1)?;
                value = &b[k + 1..close];
                i = close + 1;
            } else {
                let mut e = k;
                while e < n && !is_space(b[e]) && b[e] != b'>' {
                    e += 1;
                }
                value = &b[k..e];
                i = e;
            }
        }
        attrs.push((name, value));
    }
}

/// HTML's whitespace, as the webview's tokenizer reads it: space, tab, line feed, form feed and
/// carriage return.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
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

    #[test]
    fn the_legacy_names_are_the_fixtures() {
        let fixture = fixture();
        let names: Vec<&str> = fixture["legacy"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert_eq!(names, LEGACY_NAMES);
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
            // C19 review 1: an unquoted value runs to whitespace or `>`, and a `=` or a quote
            // inside it opens nothing, as the tokenizer reads it.
            (
                "<a href=https://example.com/?q=\">y</a>",
                true,
                "a quote inside an unquoted value",
            ),
            (
                "<b id=x=\">y</b>",
                true,
                "= and a quote inside an unquoted value",
            ),
            (
                "<b id=x=\"><table><tr><td>a</td></tr></table>\">y</b>",
                false,
                "a table after an unquoted value is a table",
            ),
            (
                "<b id = x>y</b>",
                true,
                "spaces around = before an unquoted value",
            ),
            ("<b =x>y</b>", true, "a name starting with ="),
        ] {
            assert!(pm_can_edit(description) == editable, "{why}");
        }
        // Plain text is editable whatever it holds.
        assert!(pm_can_edit("a<!-- note -->b"));
        assert!(pm_can_edit("5 < 6 & 7 > 3"));
    }

    /// C19 review 1: a description the webview never read passes only with attributes that draw
    /// nothing PM must keep; never more than `pm_can_edit` lets through.
    #[test]
    fn an_unseen_description_passes_only_with_attributes_pm_keeps() {
        for kept in [
            "plain words, 5 < 6 & such",
            "<b>a</b><br><i>b</i> <u>c</u><p>d</p><ul><li>e</li></ul>",
            "<a href=\"https://example.com/?a=1&amp;b=2\" target=\"_blank\" id=\"ow1\" \
             __is_owner=\"true\" rel=\"noopener\">site</a>",
            "<A HREF=\"MAILTO:ann@example.com\">mail</A>",
            "<a href=\"tel:+15550100\">call</a>",
            "<a href=\"www.example.org/path\">site</a>",
            "<a href=https://example.com>unquoted</a>",
            "<a href=\"https://zoom.us/j/1?pwd=x&uname=y\">a raw ampersand</a>",
            "<ul style=\"color: var(--on-surface)\"><li>x</li></ul>",
            "<span style=\"background-color: var(--textfield-surface); color: var( --on-surface );\">x</span>",
            "<span style=\"font-weight: bold; font-style: italic; text-decoration: underline;\">x</span>",
            "<span style=\"font-weight: 600\">x</span>",
            "<li data-id=\"1\" aria-label=\"x\" lang=\"en\">x</li>",
            "</b class=\"x\">",
        ] {
            assert!(pm_can_edit_unseen(kept), "{kept}");
        }
        for refused in [
            "<span style=\"color: red\">x</span>",
            "<span style=\"color: var(--x); font-size: 20px\">x</span>",
            "<span style=\"font-weight: normal\">x</span>",
            "<span style=\"font-weight: bold !important\">x</span>",
            "<span style=\"color: &#114;ed\">x</span>",
            "<p dir=\"rtl\">x</p>",
            "<span class=\"x\">x</span>",
            "<b title=\"x\">x</b>",
            "<b onclick=\"x\">x</b>",
            "<span href=\"https://example.com\">x</span>",
            "<a href=\"sms:1\">x</a>",
            "<a href=\"javascript:alert(1)\">x</a>",
            "<a href=\"javascript&#58;alert(1)\">x</a>",
            "<a href=\"https://example.com/a b\">x</a>",
            "<a href=\"\">x</a>",
            "<a>x</a>",
            "<table><tr><td>x</td></tr></table>",
            // No reference in a link but `&amp;`, where the webview would decode them.
            "<a href=\"https://example.com/&eacute;\">x</a>",
            "<a href=\"https://example.com/&#65;\">x</a>",
            "<a href=\"https://example.com/&#65\">x</a>",
            "<a href=\"https://example.com/&#x41\">x</a>",
            // A list inside a mark is nothing the webview writes back.
            "<ul style=\"font-weight:bold\"><li>x</li></ul>",
            "<ol style=\"font-style:italic\"><li>x</li></ol>",
            "<ul style=\"text-decoration:underline\"><li>x</li></ul>",
        ] {
            assert!(!pm_can_edit_unseen(refused), "{refused}");
        }
        // The shared table of addresses, as the webview's reader takes them.
        let fixture_rows = fixture();
        for row in fixture_rows["hrefs"].as_array().unwrap() {
            let href = row["href"].as_str().unwrap();
            assert!(
                pm_can_edit_unseen(&format!("<a href=\"{href}\">x</a>"))
                    == row["kept"].as_bool().unwrap(),
                "{href}"
            );
        }
        assert!(
            fixture_rows["hrefs"].as_array().unwrap().len() > 20,
            "the fixture holds its addresses"
        );
        // Never laxer than the backend's own check, on every fixture row.
        let fixture = fixture();
        for row in fixture["rows"].as_array().unwrap() {
            let description = row["description"].as_str().unwrap();
            if pm_can_edit_unseen(description) {
                assert!(pm_can_edit(description), "{description}");
            }
        }
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
