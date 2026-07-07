// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Spreadsheet ingestion (board card: Spreadsheet Processing).
//!
//! `.xlsx/.csv` are a dedicated ingestion path that REUSES the document pipeline wholesale but
//! BYPASSES MarkItDown (which flattens a sheet into one Markdown pipe table the generic chunker then
//! slices arbitrarily, stripping every row of its header context). Instead the Python sidecar's
//! `analyze_spreadsheet` parses values-only into per-sheet structure ([`SheetData`]) and this module
//! shapes that into a synthetic Markdown body — one `## Sheet:` section per sheet, each with a
//! retrievable `### Overview` metadata leaf plus row content — following the same
//! metadata-chunk-plus-content-chunks pattern the photo card established ([`crate::photos`]). No
//! splitter changes: the headings give independent leaves, exactly as photos rely on.
//!
//! The body is the vault truth (a real `.md`), so a spreadsheet rebuilds from the vault for free — the
//! sidecar is never re-run on Rebuild. The per-document [`SpreadsheetRecord`] lands in the
//! `spreadsheets` satellite row (migration v30) and round-trips through the vault frontmatter; its
//! `structured_data_summary` column is RESERVED (no writer this card), parallel to
//! `photos.visual_description`.

use serde::Deserialize;

/// Below this many rows a sheet renders as ONE cohesive block (a compact Markdown table) instead of
/// one self-describing chunk per row: splitting a small sheet only adds retrieval noise for no gain
/// (spec: "sheets under ~40 rows collapse to a single whole-sheet chunk"). A tunable, not a hardcoded
/// literal scattered through the logic. It composes with — never duplicates — the sidecar's row cap
/// (the upper bound) and the splitter's token bound (which still splits a genuinely oversized block).
pub const ROW_COLLAPSE_THRESHOLD: usize = 40;

/// One sheet as parsed by the sidecar's `analyze_spreadsheet` (values only — no formulas, no styles).
/// `row_count` is the sheet's TRUE total; `rows` is capped to the sidecar's `SPREADSHEET_ROW_CAP`, with
/// `truncated` set when the sheet had more (the metadata still reports the true total).
#[derive(Debug, Clone, Deserialize)]
pub struct SheetData {
    pub name: String,
    pub headers: Vec<String>,
    pub row_count: i64,
    #[serde(default)]
    pub inferred_types: Vec<String>,
    /// `[min, max]` as `YYYY-MM-DD` for the first date-typed column, or `None` when the sheet has none.
    #[serde(default)]
    pub date_range: Option<(String, String)>,
    pub rows: Vec<Vec<String>>,
    #[serde(default)]
    pub truncated: bool,
}

/// The per-document spreadsheet truth written to the `spreadsheets` satellite row (migration v30) and
/// round-tripped through the vault frontmatter so a Rebuild reconstructs it without re-parsing the
/// original file. `structured_data_summary` is deliberately NOT here — it is a reserved column with no
/// writer this card (parallel to `photos.visual_description`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpreadsheetRecord {
    pub sheet_count: i64,
    /// True total data rows across every sheet (before the sidecar cap).
    pub total_rows: i64,
    /// Data rows actually rendered into chunks (after the cap) — `< total_rows` iff a sheet truncated.
    pub chunked_rows: i64,
}

/// Build the synthetic Markdown body for a workbook plus its satellite record. Returns `None` when
/// nothing was extractable (every sheet headerless/empty), so the caller skips it like a blank file.
pub fn to_markdown(sheets: &[SheetData]) -> Option<(String, SpreadsheetRecord)> {
    let usable: Vec<&SheetData> = sheets.iter().filter(|s| !s.headers.is_empty()).collect();
    if usable.is_empty() {
        return None;
    }

    let mut body = String::new();
    let (mut total_rows, mut chunked_rows) = (0i64, 0i64);
    for sheet in &usable {
        body.push_str(&sheet_section(sheet));
        body.push('\n');
        total_rows += sheet.row_count;
        chunked_rows += sheet.rows.len() as i64;
    }

    let record = SpreadsheetRecord {
        sheet_count: usable.len() as i64,
        total_rows,
        chunked_rows,
    };
    Some((format!("{}\n", body.trim_end()), record))
}

/// One `## Sheet:` section: a `### Overview` metadata leaf (always) and, when the sheet has rows, a
/// `### Rows` section rendered per the collapse threshold (compact table for a small sheet;
/// self-describing folded lines for a large one).
fn sheet_section(sheet: &SheetData) -> String {
    let mut s = format!(
        "## Sheet: {}\n\n### Overview\n\n{}\n",
        sheet.name,
        overview_sentence(sheet)
    );
    if !sheet.rows.is_empty() {
        s.push_str("\n### Rows\n\n");
        if (sheet.row_count as usize) < ROW_COLLAPSE_THRESHOLD {
            s.push_str(&render_table(&sheet.headers, &sheet.rows));
        } else {
            for row in &sheet.rows {
                s.push_str(&folded_row(&sheet.headers, row));
                s.push('\n');
            }
        }
    }
    s
}

/// The always-present metadata sentence → the retrievable Overview leaf. Carries the sheet name, its
/// columns with inferred types, the true row count, an optional date range, and — when the sheet was
/// row-capped — an explicit truncation note (true total vs chunked), so nothing is silently dropped.
fn overview_sentence(sheet: &SheetData) -> String {
    let cols: Vec<String> = sheet
        .headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let ty = sheet
                .inferred_types
                .get(i)
                .map(String::as_str)
                .unwrap_or("string");
            format!("{h} [{ty}]")
        })
        .collect();
    let mut s = format!(
        "Sheet \"{}\": {} column{} ({}); {} row{}",
        sheet.name,
        sheet.headers.len(),
        plural(sheet.headers.len()),
        cols.join(", "),
        sheet.row_count,
        plural(sheet.row_count as usize),
    );
    if let Some((from, to)) = &sheet.date_range {
        s.push_str(&format!("; dates {from} to {to}"));
    }
    if sheet.truncated {
        s.push_str(&format!(
            "; row-level indexing truncated to {} of {} rows",
            sheet.rows.len(),
            sheet.row_count
        ));
    }
    s.push('.');
    s
}

/// A single self-describing row line with headers folded in as a key-value prefix
/// (`Project: Atlas | Amount: 1200 | Due: 2026-03-01`), so a row chunk stands alone once the splitter
/// scatters it across leaves — no neighbour needed to know what a cell means.
fn folded_row(headers: &[String], row: &[String]) -> String {
    headers
        .iter()
        .enumerate()
        .map(|(i, h)| format!("{h}: {}", row.get(i).map(String::as_str).unwrap_or("")))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// A compact Markdown pipe table for a small sheet — one cohesive whole-sheet chunk. Safe here (unlike
/// the MarkItDown dump this card replaces) precisely because a below-threshold sheet stays small enough
/// to land in a single chunk rather than being sliced. A literal `|` in a cell is escaped so it can't
/// break the table.
fn render_table(headers: &[String], rows: &[Vec<String>]) -> String {
    // Escape a literal `|` in a header the same way body cells are (below), so a pipe in a column
    // name can't inject a spurious column and misalign the table.
    let head = headers
        .iter()
        .map(|h| h.replace('|', "\\|"))
        .collect::<Vec<_>>()
        .join(" | ");
    let mut s = format!("| {head} |\n");
    s.push_str(&format!(
        "| {} |\n",
        headers
            .iter()
            .map(|_| "---")
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    for row in rows {
        let cells: Vec<String> = headers
            .iter()
            .enumerate()
            .map(|(i, _)| {
                row.get(i)
                    .map(String::as_str)
                    .unwrap_or("")
                    .replace('|', "\\|")
            })
            .collect();
        s.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    s
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(name: &str, headers: &[&str], rows: Vec<Vec<&str>>, row_count: i64) -> SheetData {
        SheetData {
            name: name.into(),
            headers: headers.iter().map(|s| s.to_string()).collect(),
            row_count,
            inferred_types: vec!["string".into(); headers.len()],
            date_range: None,
            rows: rows
                .into_iter()
                .map(|r| r.into_iter().map(|c| c.to_string()).collect())
                .collect(),
            truncated: false,
        }
    }

    #[test]
    fn small_sheet_renders_one_overview_and_a_table() {
        let s = sheet(
            "Budget",
            &["Project", "Amount"],
            vec![vec!["Atlas", "1200"], vec!["Beacon", "800"]],
            2,
        );
        let (body, record) = to_markdown(&[s]).unwrap();
        assert!(body.contains("## Sheet: Budget"));
        assert!(body.contains("### Overview"));
        assert!(body
            .contains("Sheet \"Budget\": 2 columns (Project [string], Amount [string]); 2 rows."));
        // Small sheet → a compact table, NOT per-row folded lines.
        assert!(body.contains("| Project | Amount |"));
        assert!(body.contains("| Atlas | 1200 |"));
        assert!(!body.contains("Project: Atlas"));
        assert_eq!(
            record,
            SpreadsheetRecord {
                sheet_count: 1,
                total_rows: 2,
                chunked_rows: 2
            }
        );
    }

    #[test]
    fn large_sheet_renders_self_describing_rows() {
        let rows: Vec<Vec<&str>> = (0..ROW_COLLAPSE_THRESHOLD)
            .map(|_| vec!["Atlas", "In Progress"])
            .collect();
        let n = rows.len() as i64;
        let s = sheet("Tasks", &["Project", "Status"], rows, n);
        let (body, _) = to_markdown(&[s]).unwrap();
        // At/above the threshold → one folded key-value line per row, no pipe table.
        assert!(body.contains("Project: Atlas | Status: In Progress"));
        assert!(!body.contains("| Project | Status |"));
    }

    #[test]
    fn multi_sheet_workbook_sections_each_sheet() {
        let a = sheet("Budget", &["Project"], vec![vec!["Atlas"]], 1);
        let b = sheet("Team", &["Name"], vec![vec!["Alex"]], 1);
        let (body, record) = to_markdown(&[a, b]).unwrap();
        assert!(body.contains("## Sheet: Budget"));
        assert!(body.contains("## Sheet: Team"));
        assert_eq!(record.sheet_count, 2);
    }

    #[test]
    fn truncation_is_noted_and_counted() {
        let mut s = sheet("Big", &["N"], vec![vec!["0"], vec!["1"], vec!["2"]], 5000);
        s.truncated = true; // sidecar kept 3 of a true 5000
        let (body, record) = to_markdown(&[s]).unwrap();
        assert!(body.contains("row-level indexing truncated to 3 of 5000 rows"));
        assert_eq!(record.total_rows, 5000);
        assert_eq!(record.chunked_rows, 3); // total vs chunked diverge exactly here
    }

    #[test]
    fn date_range_surfaces_in_overview() {
        let mut s = sheet("Log", &["Due"], vec![vec!["2026-03-01"]], 1);
        s.date_range = Some(("2026-03-01".into(), "2026-06-30".into()));
        let (body, _) = to_markdown(&[s]).unwrap();
        assert!(body.contains("dates 2026-03-01 to 2026-06-30"));
    }

    #[test]
    fn headerless_workbook_is_none() {
        let empty = SheetData {
            name: "Sheet1".into(),
            headers: vec![],
            row_count: 0,
            inferred_types: vec![],
            date_range: None,
            rows: vec![],
            truncated: false,
        };
        assert!(to_markdown(&[empty]).is_none());
    }

    #[test]
    fn sheet_data_deserializes_from_sidecar_shape() {
        // Guards the Python↔Rust contract: the sidecar's per-sheet dict keys must match SheetData's
        // fields (a rename on either side would break ingestion silently otherwise).
        let json = serde_json::json!({
            "name": "Budget",
            "headers": ["Project", "Due"],
            "row_count": 2,
            "inferred_types": ["string", "date"],
            "date_range": ["2026-03-01", "2026-04-15"],
            "rows": [["Atlas", "2026-03-01"], ["Beacon", "2026-04-15"]],
            "truncated": false
        });
        let sheet: SheetData = serde_json::from_value(json).unwrap();
        assert_eq!(sheet.name, "Budget");
        assert_eq!(sheet.row_count, 2);
        assert_eq!(
            sheet.date_range,
            Some(("2026-03-01".into(), "2026-04-15".into()))
        );

        // A null date_range (no date column) deserializes to None, not an error.
        let no_dates = serde_json::json!({
            "name": "X", "headers": ["A"], "row_count": 0,
            "inferred_types": ["empty"], "date_range": null, "rows": [], "truncated": false
        });
        let s2: SheetData = serde_json::from_value(no_dates).unwrap();
        assert_eq!(s2.date_range, None);
    }
}
