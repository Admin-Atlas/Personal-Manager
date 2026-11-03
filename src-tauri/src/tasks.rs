// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tasks (PA-1, #1035): the user's one global to-do list, and the single owner of the `tasks` and
//! `task_stacks` tables (v59).
//!
//! Every surface is a live view over the same rows: the project page, the pinboard Task list
//! widget, the calendar's Runway agenda and its overlay. Every write goes through the functions
//! here and, at the command layer, emits [`TASKS_CHANGED`] so the open views re-read. [`list`] is a
//! plain `&Connection` function, so a later reader (the briefing, the gatherer, chat grounding) can
//! call it with no IPC.
//!
//! **Links are independent and optional.** A task may name a project, a milestone (of that project,
//! or alone, in which case it inherits the milestone's project) and a calendar event by iCal UID,
//! in any combination. **The deadline can follow a link** (`deadline_from`): the task's own date, the
//! linked milestone's resolved date, or the linked event's date in the user's zone. An event
//! deadline is pinned to one occurrence: the task's own `deadline` holds the date of the occurrence
//! the user picked, and the deadline is that uid's occurrence nearest the pin (R1), so a recurring
//! event's chosen Friday goes late once it passes rather than rolling on. When the link can't
//! resolve (the event is gone, the milestone has no date or was deleted), the task falls back
//! to its own `deadline` and says so with `deadline_missing`. Every surface uses
//! `effective_deadline`, never `deadline`, for tone, sorting and next-up.
//!
//! **Tasks don't write to the project activity log.** Its kind CHECK admits only chat / ingest /
//! milestone, and `project_activity::record` swallows errors, so a `task` kind would be dropped
//! silently. Task writes only `projects::touch` the project or projects involved (command layer).
//!
//! Free of model paths, so it sits outside the calendar write fence by construction.

use std::collections::{HashMap, HashSet};

use chrono::{Datelike, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Emitted (with `()`) after every task or stack write, once the DB guard is dropped. Every live
/// view listens for it, so a change made on one surface shows on all of them.
pub const TASKS_CHANGED: &str = "tasks://changed";

// --- the closed value sets (I-12: one owner per CHECK set; v59 mirrors these) ---------------

/// `tasks.status`.
pub const STATUSES: [&str; 3] = ["todo", "doing", "done"];
/// The one status that means finished, and so the one that stamps `completed_at`.
pub const STATUS_DONE: &str = "done";
/// The status a new task starts at.
pub const STATUS_TODO: &str = "todo";
/// `tasks.lane`.
pub const LANES: [&str; 2] = ["foreground", "background"];
/// `tasks.deadline_kind`: a hard deadline, or a date to aim for.
pub const DEADLINE_KINDS: [&str; 2] = ["hard", "soft"];
/// `tasks.deadline_from`: where the task's deadline comes from.
pub const DEADLINE_FROM: [&str; 3] = [FROM_DATE, FROM_MILESTONE, FROM_EVENT];
/// The task's own `deadline`.
pub const FROM_DATE: &str = "date";
/// The linked milestone's resolved due date.
pub const FROM_MILESTONE: &str = "milestone";
/// The linked calendar event's date, in the user's zone.
pub const FROM_EVENT: &str = "event";
/// `task_stacks.kind`.
pub const STACK_KINDS: [&str; 3] = ["sequential", "in_order", "waiting_on"];
/// `tasks.source`: who made the task.
pub const SOURCES: [&str; 2] = ["user", "assistant"];
/// A task the user made (every path in this PR).
pub const SOURCE_USER: &str = "user";

/// The longest title a task may have, in characters.
pub const TITLE_MAX: usize = 500;
/// The longest why-note (or stack note) a task may have, in characters.
pub const NOTE_MAX: usize = 10_000;

/// One task as every surface sees it: the stored row plus its resolved links.
#[derive(Clone, Debug, Serialize)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub why: Option<String>,
    pub project_name: Option<String>,
    /// The linked project is archived (from `projects.archived_at`). Lists hide these by default.
    pub project_archived: bool,
    pub milestone_id: Option<i64>,
    pub event_uid: Option<String>,
    pub do_start: Option<String>,
    pub do_end: Option<String>,
    /// The task's OWN deadline, as stored. With `deadline_from = 'event'` it is the pin: the date of
    /// the occurrence the user picked. Display logic reads `effective_deadline` instead.
    pub deadline: Option<String>,
    pub deadline_kind: String,
    /// One of [`DEADLINE_FROM`].
    pub deadline_from: String,
    /// The date every surface uses: the own deadline, or the linked milestone's or event's date
    /// per `deadline_from` (falling back to the own deadline when that link can't resolve).
    pub effective_deadline: Option<String>,
    /// `deadline_from` names a link that couldn't resolve, so `effective_deadline` fell back to
    /// the task's own date (which may be absent).
    pub deadline_missing: bool,
    /// The linked milestone's label, when it still exists.
    pub milestone_label: Option<String>,
    /// The linked event's title from the calendar mirror, when it's there.
    pub event_title: Option<String>,
    pub status: String,
    pub completed_at: Option<String>,
    pub lane: String,
    pub stack_id: Option<i64>,
    pub parked: bool,
    pub ordinal: i64,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A named group of tasks.
#[derive(Clone, Debug, Serialize)]
pub struct TaskStack {
    pub id: i64,
    pub title: String,
    /// One of [`STACK_KINDS`].
    pub kind: String,
    pub note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// The fields the editor writes: everything but status, ordinal and source.
#[derive(Clone, Debug)]
pub struct TaskFields {
    pub title: String,
    pub why: Option<String>,
    pub project: Option<String>,
    pub milestone_id: Option<i64>,
    pub event_uid: Option<String>,
    pub do_start: Option<String>,
    pub do_end: Option<String>,
    pub deadline: Option<String>,
    pub deadline_kind: String,
    pub deadline_from: String,
    pub lane: String,
    pub stack_id: Option<i64>,
    pub parked: bool,
}

/// One checklist line to import (from a pinboard note).
#[derive(Clone, Debug, Deserialize)]
pub struct ChecklistItem {
    pub title: String,
    pub done: bool,
}

/// What a checklist import did: the ids it created, and how many lines it skipped (blank, already
/// a task in that scope, or repeated within the batch).
#[derive(Clone, Debug, Serialize)]
pub struct ImportResult {
    pub created: Vec<i64>,
    pub skipped: usize,
}

// --- pure validation ----------------------------------------------------------------------

/// Trim a value and treat blank as absent (mirrors `milestones::clean`).
fn clean(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Reject a value outside its closed set before it reaches SQL, so the CHECK is a backstop.
fn validate_one(what: &str, value: &str, allowed: &[&str]) -> Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "unknown {what} {value:?} (expected one of {})",
            allowed.join(", ")
        )))
    }
}

pub fn validate_status(status: &str) -> Result<()> {
    validate_one("task status", status, &STATUSES)
}

pub fn validate_lane(lane: &str) -> Result<()> {
    validate_one("task lane", lane, &LANES)
}

pub fn validate_deadline_kind(kind: &str) -> Result<()> {
    validate_one("deadline kind", kind, &DEADLINE_KINDS)
}

pub fn validate_deadline_from(from: &str) -> Result<()> {
    validate_one("deadline source", from, &DEADLINE_FROM)
}

pub fn validate_stack_kind(kind: &str) -> Result<()> {
    validate_one("stack kind", kind, &STACK_KINDS)
}

pub fn validate_source(source: &str) -> Result<()> {
    validate_one("task source", source, &SOURCES)
}

/// A real calendar day written `YYYY-MM-DD`, cleaned; blank is absent.
fn validate_date(what: &str, v: Option<String>) -> Result<Option<String>> {
    let Some(s) = clean(v) else {
        return Ok(None);
    };
    let real = s.len() == 10
        && NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .is_ok_and(|d| d.format("%Y-%m-%d").to_string() == s);
    if real {
        Ok(Some(s))
    } else {
        Err(Error::Other(format!(
            "the {what} isn't a real date (expected YYYY-MM-DD)"
        )))
    }
}

/// The plan window: both dates real, an end only with a start, and never before it.
fn validate_plan(
    do_start: Option<String>,
    do_end: Option<String>,
) -> Result<(Option<String>, Option<String>)> {
    let do_start = validate_date("start date", do_start)?;
    let do_end = validate_date("end date", do_end)?;
    match (&do_start, &do_end) {
        (None, Some(_)) => Err(Error::Other("a plan that ends needs a start date".into())),
        // Lexicographic order is chronological for validated `YYYY-MM-DD`.
        (Some(s), Some(e)) if e < s => {
            Err(Error::Other("the plan can't end before it starts".into()))
        }
        _ => Ok((do_start, do_end)),
    }
}

/// A title: trimmed, non-empty, at most [`TITLE_MAX`] characters.
fn validate_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        return Err(Error::Other("a task needs a title".into()));
    }
    if title.chars().count() > TITLE_MAX {
        return Err(Error::Other(format!(
            "a task title can be at most {TITLE_MAX} characters"
        )));
    }
    Ok(title.to_string())
}

/// A note: cleaned, at most [`NOTE_MAX`] characters.
fn validate_note(v: Option<String>) -> Result<Option<String>> {
    let v = clean(v);
    if v.as_ref().is_some_and(|s| s.chars().count() > NOTE_MAX) {
        return Err(Error::Other(format!(
            "a note can be at most {NOTE_MAX} characters"
        )));
    }
    Ok(v)
}

/// Every check that needs no database, returning the cleaned fields. Pure.
pub fn validate_fields(f: &TaskFields) -> Result<TaskFields> {
    let title = validate_title(&f.title)?;
    let why = validate_note(f.why.clone())?;
    let project = clean(f.project.clone());
    let event_uid = clean(f.event_uid.clone());
    let (do_start, do_end) = validate_plan(f.do_start.clone(), f.do_end.clone())?;
    let deadline = validate_date("deadline", f.deadline.clone())?;
    validate_deadline_kind(&f.deadline_kind)?;
    validate_deadline_from(&f.deadline_from)?;
    validate_lane(&f.lane)?;
    if f.deadline_from == FROM_MILESTONE && f.milestone_id.is_none() {
        return Err(Error::Other(
            "to take its deadline from a milestone, the task needs a linked milestone".into(),
        ));
    }
    if f.deadline_from == FROM_EVENT && event_uid.is_none() {
        return Err(Error::Other(
            "to take its deadline from an event, the task needs a linked event".into(),
        ));
    }
    Ok(TaskFields {
        title,
        why,
        project,
        milestone_id: f.milestone_id,
        event_uid,
        do_start,
        do_end,
        deadline,
        deadline_kind: f.deadline_kind.clone(),
        deadline_from: f.deadline_from.clone(),
        lane: f.lane.clone(),
        stack_id: f.stack_id,
        parked: f.parked,
    })
}

/// The checks that read the database: the project is made canonical ([`canonical_project`]), the
/// milestone exists (and the task inherits its project, or refuses one that differs) and the stack
/// exists.
fn resolve_links(conn: &Connection, f: &mut TaskFields) -> Result<()> {
    if let Some(typed) = f.project.take() {
        f.project = Some(canonical_project(conn, &typed)?);
    }
    if let Some(mid) = f.milestone_id {
        let owner = crate::milestones::project_of(conn, mid)?
            .ok_or_else(|| Error::Other("that milestone no longer exists".into()))?;
        match &f.project {
            // A case variant of the owner is the owner: the task takes the milestone's spelling.
            Some(p) if p.to_lowercase() != owner.to_lowercase() => {
                return Err(Error::Other(
                    "that milestone belongs to another project".into(),
                ))
            }
            _ => f.project = Some(owner),
        }
    }
    if let Some(sid) = f.stack_id {
        if !stack_exists(conn, sid)? {
            return Err(Error::Other("that stack no longer exists".into()));
        }
    }
    Ok(())
}

fn stack_exists(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_stacks WHERE id = ?1)",
        params![id],
        |r| r.get(0),
    )?)
}

/// The project a typed name means (R1), so a task never lands on a stray spelling that every
/// project surface, rename, merge and delete would miss (they all match the exact name).
///
/// In order: a name a project already goes by exactly (its `projects` row, or its entity's
/// canonical name, since a project can be an entity before it has a triage row); then the same,
/// case-insensitively, preferring an entity's canonical name; then the entity aliases, the way
/// canon.rs resolves a name (`entities::resolve_project`, exact). Otherwise the typed name, trimmed:
/// a genuinely new project, whose row [`ensure_project`] then mints.
pub fn canonical_project(conn: &Connection, typed: &str) -> Result<String> {
    let typed = typed.trim();
    let mut stmt = conn.prepare(
        "SELECT p.name, EXISTS(SELECT 1 FROM entities e \
                                WHERE e.type = 'project' AND e.canonical_name = p.name) \
           FROM projects p \
         UNION \
         SELECT canonical_name, 1 FROM entities WHERE type = 'project'",
    )?;
    let names = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if names.iter().any(|(n, _)| n == typed) {
        return Ok(typed.to_string());
    }
    // Rust's case folding, not SQLite's `lower()`, which folds ASCII only.
    let folded = typed.to_lowercase();
    if let Some((name, _)) = names
        .iter()
        .filter(|(n, _)| n.to_lowercase() == folded)
        .min_by(|(a, a_entity), (b, b_entity)| b_entity.cmp(a_entity).then_with(|| a.cmp(b)))
    {
        return Ok(name.clone());
    }
    match crate::entities::resolve_project(conn, typed, false)? {
        Some(id) => crate::entities::canonical_name(conn, id),
        None => Ok(typed.to_string()),
    }
}

/// Upsert a bare `projects` row so the FK holds for a lazy (never-triaged) project.
fn ensure_project(conn: &Connection, project: Option<&str>) -> Result<()> {
    if let Some(p) = project {
        conn.execute(
            "INSERT INTO projects(name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
            params![p],
        )?;
    }
    Ok(())
}

// --- reading ------------------------------------------------------------------------------

/// Resolve `deadline_from` into `(effective_deadline, deadline_missing)`. Pure.
fn effective_deadline(
    deadline_from: &str,
    own: Option<&str>,
    milestone_date: Option<&str>,
    event_date: Option<&str>,
) -> (Option<String>, bool) {
    let linked = match deadline_from {
        FROM_MILESTONE => Some(milestone_date),
        FROM_EVENT => Some(event_date),
        _ => None,
    };
    match linked {
        None => (own.map(str::to_string), false),
        Some(Some(d)) => (Some(d.to_string()), false),
        Some(None) => (own.map(str::to_string), true),
    }
}

/// The first day the calendar mirror holds, from `today`: the start of last month, as
/// `calendar::time_window` anchors it (`MIRROR_BACK`). Worked out from the `today` the caller passes
/// rather than the wall clock, so `list` stays a function of its inputs. Empty for a bad date, which
/// turns the aged-out rule off.
fn mirror_start(today: &str) -> String {
    NaiveDate::parse_from_str(today, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.with_day(1))
        .and_then(|d| d.checked_sub_months(chrono::Months::new(1)))
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// The one occurrence of a linked event a task's deadline follows, from that uid's occurrence dates
/// (ascending, as [`crate::milestones::calendar_occurrences_by_uid`] gives them). Pure.
///
/// With a pin (the occurrence the user picked, held in the task's own `deadline`, R1): the
/// occurrence nearest the pin, an earlier one winning a tie. So a recurring event's chosen Friday
/// stays the deadline and goes late once it passes, and a moved occurrence is still found. With no
/// pin: the soonest on or after `today`, else the latest past one. `None` when there are none.
///
/// `mirror_from` is the first day the calendar mirror still holds (`calendar::time_window`). A pin
/// before it is an occurrence that has aged out of the mirror while the series' later ones remain,
/// so the nearest survivor would be a different, later occurrence and the deadline would creep
/// forward each month. The pin itself is the answer then: it is the day the user chose. Only for a
/// series (two or more occurrences still held): a lone occurrence is the event itself, perhaps moved
/// far from its old day, and following the move is right however old the pin is.
fn event_occurrence<'a>(
    dates: &'a [String],
    pin: Option<&'a str>,
    today: &str,
    mirror_from: &str,
) -> Option<&'a str> {
    let day = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
    let Some(pin_day) = pin.and_then(day) else {
        return dates
            .iter()
            .find(|d| d.as_str() >= today)
            .or_else(|| dates.last())
            .map(String::as_str);
    };
    if dates.len() >= 2 && day(mirror_from).is_some_and(|from| pin_day < from) {
        return pin;
    }
    let pin = pin_day;
    let mut best: Option<(i64, &str)> = None;
    for d in dates {
        let Some(at) = day(d) else { continue };
        let distance = (at - pin).num_days().abs();
        // Ascending dates, so keeping only a strictly nearer one lets the earlier win a tie.
        if best.is_none_or(|(b, _)| distance < b) {
            best = Some((distance, d.as_str()));
        }
    }
    best.map(|(_, d)| d)
}

/// The stored rows, joined with the project's archive flag and the linked event's title, links not
/// yet resolved.
fn load_rows(conn: &Connection, project: Option<&str>) -> Result<Vec<Task>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.why, t.project_name, p.archived_at IS NOT NULL, \
                t.milestone_id, t.event_uid, t.do_start, t.do_end, t.deadline, \
                t.deadline_kind, t.deadline_from, t.status, t.completed_at, t.lane, \
                t.stack_id, t.parked, t.ordinal, t.source, t.created_at, t.updated_at, \
                (SELECT e.summary FROM calendar_events e \
                  WHERE e.uid = t.event_uid ORDER BY e.start LIMIT 1) \
           FROM tasks t LEFT JOIN projects p ON p.name = t.project_name \
          WHERE ?1 IS NULL OR t.project_name = ?1 \
          ORDER BY t.ordinal, t.id",
    )?;
    let rows = stmt
        .query_map(params![project], |r| {
            Ok(Task {
                id: r.get(0)?,
                title: r.get(1)?,
                why: r.get(2)?,
                project_name: r.get(3)?,
                project_archived: r.get(4)?,
                milestone_id: r.get(5)?,
                event_uid: r.get(6)?,
                do_start: r.get(7)?,
                do_end: r.get(8)?,
                deadline: r.get(9)?,
                deadline_kind: r.get(10)?,
                deadline_from: r.get(11)?,
                effective_deadline: None,
                deadline_missing: false,
                milestone_label: None,
                status: r.get(12)?,
                completed_at: r.get(13)?,
                lane: r.get(14)?,
                stack_id: r.get(15)?,
                parked: r.get(16)?,
                ordinal: r.get(17)?,
                source: r.get(18)?,
                created_at: r.get(19)?,
                updated_at: r.get(20)?,
                event_title: r.get(21)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Every task (`project = None`) or one project's, in the one global order (`ordinal`, then id),
/// with links resolved. `today` is the user's zone-local civil date (`clock::today_sql_in`). An event
/// deadline resolves to the occurrence nearest the task's pin (its own `deadline`), or with no pin
/// the way the milestone surfaces see it: the soonest occurrence on or after today, else the latest
/// past one (see [`event_occurrence`]).
pub fn list(conn: &Connection, project: Option<&str>, today: &str) -> Result<Vec<Task>> {
    let rows = load_rows(conn, project.map(str::trim).filter(|p| !p.is_empty()))?;
    // Resolve the links only when some task has one: both scans read the calendar mirror.
    let milestones: HashMap<i64, (String, Option<String>)> =
        if rows.iter().any(|t| t.milestone_id.is_some()) {
            crate::milestones::list_all(conn, today)?
                .into_iter()
                .map(|m| (m.id, (m.label, m.due_date)))
                .collect()
        } else {
            HashMap::new()
        };
    // Only a deadline that follows its event reads the occurrences (an event link alone shows just
    // the title, which `load_rows` already joined).
    let (event_dates, mirror_from): (HashMap<String, Vec<String>>, String) = if rows
        .iter()
        .any(|t| t.deadline_from == FROM_EVENT && t.event_uid.is_some())
    {
        (
            crate::milestones::calendar_occurrences_by_uid(conn)?,
            mirror_start(today),
        )
    } else {
        (HashMap::new(), String::new())
    };
    Ok(rows
        .into_iter()
        .map(|mut task| {
            let milestone = task.milestone_id.and_then(|id| milestones.get(&id));
            task.milestone_label = milestone.map(|(label, _)| label.clone());
            let milestone_date = milestone.and_then(|(_, due)| due.as_deref());
            let event_date = task
                .event_uid
                .as_deref()
                .and_then(|uid| event_dates.get(uid))
                .and_then(|dates| {
                    event_occurrence(dates, task.deadline.as_deref(), today, &mirror_from)
                        .map(str::to_string)
                });
            let (effective, missing) = effective_deadline(
                &task.deadline_from,
                task.deadline.as_deref(),
                milestone_date,
                event_date.as_deref(),
            );
            task.effective_deadline = effective;
            task.deadline_missing = missing;
            task
        })
        .collect())
}

/// The project a task belongs to: `None` for an unknown id or a task with no project. The command
/// layer touches it after an id-only edit, and captures it before an update or delete.
pub fn project_of(conn: &Connection, id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT project_name FROM tasks WHERE id = ?1",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// How many tasks name `name` as their project (for the delete and merge previews).
pub fn count_for_project(conn: &Connection, name: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE project_name = ?1",
        params![name],
        |r| r.get(0),
    )?)
}

// --- writing ------------------------------------------------------------------------------

/// Add a task at the end of the global order. A milestone link fills in the project when none is
/// given. Returns the new id.
pub fn add(conn: &Connection, fields: &TaskFields, source: &str) -> Result<i64> {
    validate_source(source)?;
    let mut f = validate_fields(fields)?;
    let tx = conn.unchecked_transaction()?;
    resolve_links(&tx, &mut f)?;
    ensure_project(&tx, f.project.as_deref())?;
    tx.execute(
        "INSERT INTO tasks(title, why, project_name, milestone_id, event_uid, do_start, do_end, \
                           deadline, deadline_kind, deadline_from, lane, stack_id, parked, \
                           source, ordinal) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                 (SELECT COALESCE(MAX(ordinal) + 1, 0) FROM tasks))",
        params![
            f.title,
            f.why,
            f.project,
            f.milestone_id,
            f.event_uid,
            f.do_start,
            f.do_end,
            f.deadline,
            f.deadline_kind,
            f.deadline_from,
            f.lane,
            f.stack_id,
            f.parked,
            source
        ],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id)
}

/// Rewrite a task's editable fields. An unknown id is an error.
pub fn update(conn: &Connection, id: i64, fields: &TaskFields) -> Result<()> {
    let mut f = validate_fields(fields)?;
    let tx = conn.unchecked_transaction()?;
    resolve_links(&tx, &mut f)?;
    ensure_project(&tx, f.project.as_deref())?;
    let changed = tx.execute(
        "UPDATE tasks SET title = ?2, why = ?3, project_name = ?4, milestone_id = ?5, \
                          event_uid = ?6, do_start = ?7, do_end = ?8, deadline = ?9, \
                          deadline_kind = ?10, deadline_from = ?11, lane = ?12, stack_id = ?13, \
                          parked = ?14, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE id = ?1",
        params![
            id,
            f.title,
            f.why,
            f.project,
            f.milestone_id,
            f.event_uid,
            f.do_start,
            f.do_end,
            f.deadline,
            f.deadline_kind,
            f.deadline_from,
            f.lane,
            f.stack_id,
            f.parked
        ],
    )?;
    if changed == 0 {
        return Err(no_task());
    }
    tx.commit()?;
    Ok(())
}

fn no_task() -> Error {
    Error::Other("that task no longer exists".into())
}

/// Re-plan a task (the Runway's drag and Move… menu): only the plan window, lane and parked flag
/// change, with the same date checks as the editor.
pub fn plan(
    conn: &Connection,
    id: i64,
    do_start: Option<String>,
    do_end: Option<String>,
    lane: &str,
    parked: bool,
) -> Result<()> {
    let (do_start, do_end) = validate_plan(do_start, do_end)?;
    validate_lane(lane)?;
    let changed = conn.execute(
        "UPDATE tasks SET do_start = ?2, do_end = ?3, lane = ?4, parked = ?5, \
                          updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE id = ?1",
        params![id, do_start, do_end, lane, parked],
    )?;
    if changed == 0 {
        return Err(no_task());
    }
    Ok(())
}

/// Move a task to `status`, carrying `completed_at` in the same UPDATE so the two can never
/// disagree: stamped on the move into done, kept when it was already done, cleared otherwise.
pub fn change_status(conn: &Connection, id: i64, status: &str) -> Result<()> {
    validate_status(status)?;
    let changed = conn.execute(
        "UPDATE tasks SET \
             completed_at = CASE \
                 WHEN ?2 <> ?3 THEN NULL \
                 WHEN status = ?3 AND completed_at IS NOT NULL THEN completed_at \
                 ELSE strftime('%Y-%m-%dT%H:%M:%fZ','now') \
             END, \
             status = ?2, \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE id = ?1",
        params![id, status, STATUS_DONE],
    )?;
    if changed == 0 {
        return Err(no_task());
    }
    Ok(())
}

/// Put `ordered_ids` in that order, within their own slots: they take the subset's existing
/// ordinals, sorted ascending, in the given order, so every task outside the subset keeps its
/// place. An unknown or repeated id is an error, and nothing changes. One transaction.
pub fn reorder(conn: &Connection, ordered_ids: &[i64]) -> Result<()> {
    let mut seen = HashSet::new();
    if !ordered_ids.iter().all(|id| seen.insert(*id)) {
        return Err(Error::Other("a task appears twice in that order".into()));
    }
    let tx = conn.unchecked_transaction()?;
    let mut slots: Vec<i64> = Vec::with_capacity(ordered_ids.len());
    for id in ordered_ids {
        let ordinal: Option<i64> = tx
            .query_row(
                "SELECT ordinal FROM tasks WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        slots.push(ordinal.ok_or_else(no_task)?);
    }
    slots.sort_unstable();
    for (id, ordinal) in ordered_ids.iter().zip(slots) {
        tx.execute(
            "UPDATE tasks SET ordinal = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
             WHERE id = ?1 AND ordinal <> ?2",
            params![id, ordinal],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Delete a task. An unknown id is a no-op.
pub fn remove(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
    Ok(())
}

/// Turn checklist lines into tasks in one transaction, under the canonical spelling of `project`
/// ([`canonical_project`]). A line is skipped when its trimmed title is
/// blank, or matches (case-insensitively) a task already in the same project scope (no project
/// matches no project) or an earlier line of this batch. A ticked line becomes a done task. A line
/// longer than [`TITLE_MAX`] is shortened to it rather than failing the whole import.
pub fn import_checklist(
    conn: &Connection,
    project: Option<&str>,
    items: &[ChecklistItem],
) -> Result<ImportResult> {
    let tx = conn.unchecked_transaction()?;
    let project = match clean(project.map(str::to_string)) {
        Some(typed) => Some(canonical_project(&tx, &typed)?),
        None => None,
    };
    let mut known: HashSet<String> = {
        let mut stmt = tx.prepare("SELECT title FROM tasks WHERE project_name IS ?1")?;
        let titles = stmt
            .query_map(params![project], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        titles.iter().map(|t| t.trim().to_lowercase()).collect()
    };
    let mut created = Vec::new();
    let mut skipped = 0;
    let mut ensured = false;
    for item in items {
        let title: String = item.title.trim().chars().take(TITLE_MAX).collect();
        let title = title.trim().to_string();
        if title.is_empty() || !known.insert(title.to_lowercase()) {
            skipped += 1;
            continue;
        }
        if !ensured {
            ensure_project(&tx, project.as_deref())?;
            ensured = true;
        }
        let status = if item.done { STATUS_DONE } else { STATUS_TODO };
        tx.execute(
            "INSERT INTO tasks(title, project_name, status, completed_at, source, ordinal) \
             VALUES (?1, ?2, ?3, \
                     CASE WHEN ?3 = ?4 THEN strftime('%Y-%m-%dT%H:%M:%fZ','now') END, \
                     ?5, (SELECT COALESCE(MAX(ordinal) + 1, 0) FROM tasks))",
            params![title, project, status, STATUS_DONE, SOURCE_USER],
        )?;
        created.push(tx.last_insert_rowid());
    }
    tx.commit()?;
    Ok(ImportResult { created, skipped })
}

// --- stacks -------------------------------------------------------------------------------

/// Every stack, oldest first.
pub fn list_stacks(conn: &Connection) -> Result<Vec<TaskStack>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, kind, note, created_at, updated_at FROM task_stacks ORDER BY id",
    )?;
    let out = stmt
        .query_map([], |r| {
            Ok(TaskStack {
                id: r.get(0)?,
                title: r.get(1)?,
                kind: r.get(2)?,
                note: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(out)
}

/// A stack's title: trimmed, may be blank (the UI names an untitled one), at most [`TITLE_MAX`].
fn validate_stack_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.chars().count() > TITLE_MAX {
        return Err(Error::Other(format!(
            "a stack title can be at most {TITLE_MAX} characters"
        )));
    }
    Ok(title.to_string())
}

/// Add a stack. Returns its id.
pub fn add_stack(conn: &Connection, title: &str, kind: &str, note: Option<String>) -> Result<i64> {
    let title = validate_stack_title(title)?;
    validate_stack_kind(kind)?;
    let note = validate_note(note)?;
    conn.execute(
        "INSERT INTO task_stacks(title, kind, note) VALUES (?1, ?2, ?3)",
        params![title, kind, note],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Rewrite a stack. An unknown id is an error.
pub fn update_stack(
    conn: &Connection,
    id: i64,
    title: &str,
    kind: &str,
    note: Option<String>,
) -> Result<()> {
    let title = validate_stack_title(title)?;
    validate_stack_kind(kind)?;
    let note = validate_note(note)?;
    let changed = conn.execute(
        "UPDATE task_stacks SET title = ?2, kind = ?3, note = ?4, \
                                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE id = ?1",
        params![id, title, kind, note],
    )?;
    if changed == 0 {
        return Err(Error::Other("that stack no longer exists".into()));
    }
    Ok(())
}

/// Delete a stack. Its tasks stay, unstacked (the FK's ON DELETE SET NULL).
pub fn remove_stack(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM task_stacks WHERE id = ?1", params![id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: &str = "2026-11-03";

    fn store() -> (tempfile::TempDir, Connection) {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        (dir, conn)
    }

    fn fields(title: &str) -> TaskFields {
        TaskFields {
            title: title.into(),
            why: None,
            project: None,
            milestone_id: None,
            event_uid: None,
            do_start: None,
            do_end: None,
            deadline: None,
            deadline_kind: "hard".into(),
            deadline_from: FROM_DATE.into(),
            lane: "foreground".into(),
            stack_id: None,
            parked: false,
        }
    }

    fn in_project(title: &str, project: &str) -> TaskFields {
        TaskFields {
            project: Some(project.into()),
            ..fields(title)
        }
    }

    fn err(r: Result<impl std::fmt::Debug>) -> String {
        r.unwrap_err().to_string()
    }

    fn get(conn: &Connection, id: i64) -> Task {
        list(conn, None, TODAY)
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap()
    }

    fn titles(conn: &Connection) -> Vec<String> {
        list(conn, None, TODAY)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    }

    /// Mirror rows of the calendar event `ical`, one per start.
    fn event(conn: &Connection, ical: &str, summary: &str, starts: &[&str]) {
        for (i, start) in starts.iter().enumerate() {
            conn.execute(
                "INSERT INTO calendar_events (id, calendar_id, summary, start, all_day, uid) \
                 VALUES (?1, 'cal', ?2, ?3, 0, ?4)",
                params![format!("{ical}#{i}"), summary, start, ical],
            )
            .unwrap();
        }
    }

    // --- pure validation ---

    #[test]
    fn a_title_is_trimmed_required_and_capped() {
        assert_eq!(
            validate_fields(&fields("  Ship it  ")).unwrap().title,
            "Ship it"
        );
        assert!(err(validate_fields(&fields("   "))).contains("needs a title"));
        let long = "x".repeat(TITLE_MAX + 1);
        assert!(err(validate_fields(&fields(&long))).contains("at most 500"));
        assert!(validate_fields(&fields(&"é".repeat(TITLE_MAX))).is_ok());
    }

    #[test]
    fn text_fields_are_cleaned_and_the_note_capped() {
        let f = TaskFields {
            why: Some("  ".into()),
            project: Some(" Atlas ".into()),
            event_uid: Some("".into()),
            ..fields("t")
        };
        let c = validate_fields(&f).unwrap();
        assert_eq!(c.why, None);
        assert_eq!(c.project.as_deref(), Some("Atlas"));
        assert_eq!(c.event_uid, None);
        let f = TaskFields {
            why: Some("y".repeat(NOTE_MAX + 1)),
            ..fields("t")
        };
        assert!(err(validate_fields(&f)).contains("at most 10000"));
    }

    #[test]
    fn every_date_must_be_a_real_day() {
        for bad in [
            "2026-02-30",
            "2026-13-01",
            "2026-1-05",
            "05-11-2026",
            "tomorrow",
        ] {
            let f = TaskFields {
                deadline: Some(bad.into()),
                ..fields("t")
            };
            assert!(err(validate_fields(&f)).contains("real date"), "{bad}");
            let f = TaskFields {
                do_start: Some(bad.into()),
                ..fields("t")
            };
            assert!(err(validate_fields(&f)).contains("real date"), "{bad}");
        }
        let f = TaskFields {
            deadline: Some("2028-02-29".into()),
            ..fields("t")
        };
        assert!(validate_fields(&f).is_ok(), "a leap day is real");
    }

    #[test]
    fn a_plan_needs_a_start_before_its_end() {
        let plan = |s: Option<&str>, e: Option<&str>| {
            validate_fields(&TaskFields {
                do_start: s.map(str::to_string),
                do_end: e.map(str::to_string),
                ..fields("t")
            })
        };
        assert!(err(plan(None, Some("2026-11-05"))).contains("needs a start"));
        assert!(err(plan(Some("2026-11-05"), Some("2026-11-04"))).contains("before it starts"));
        assert!(plan(Some("2026-11-05"), Some("2026-11-05")).is_ok());
        assert!(plan(Some("2026-11-05"), None).is_ok());
    }

    #[test]
    fn closed_sets_are_checked_before_sql() {
        let f = TaskFields {
            deadline_kind: "firm".into(),
            ..fields("t")
        };
        assert!(err(validate_fields(&f)).contains("firm"));
        let f = TaskFields {
            lane: "side".into(),
            ..fields("t")
        };
        assert!(err(validate_fields(&f)).contains("side"));
        let f = TaskFields {
            deadline_from: "stack".into(),
            ..fields("t")
        };
        assert!(err(validate_fields(&f)).contains("stack"));
        assert!(validate_status("blocked").is_err());
        assert!(validate_stack_kind("parallel").is_err());
        assert!(validate_source("robot").is_err());
        for s in STATUSES {
            validate_status(s).unwrap();
        }
        for k in STACK_KINDS {
            validate_stack_kind(k).unwrap();
        }
    }

    #[test]
    fn a_followed_deadline_needs_its_link() {
        let f = TaskFields {
            deadline_from: FROM_MILESTONE.into(),
            ..fields("t")
        };
        assert!(err(validate_fields(&f)).contains("linked milestone"));
        let f = TaskFields {
            deadline_from: FROM_EVENT.into(),
            event_uid: Some("  ".into()),
            ..fields("t")
        };
        assert!(err(validate_fields(&f)).contains("linked event"));
    }

    #[test]
    fn effective_deadline_follows_the_chosen_source() {
        let own = Some("2026-11-10");
        assert_eq!(
            effective_deadline(FROM_DATE, own, Some("2026-11-20"), Some("2026-11-30")),
            (Some("2026-11-10".into()), false)
        );
        assert_eq!(
            effective_deadline(FROM_MILESTONE, own, Some("2026-11-20"), None),
            (Some("2026-11-20".into()), false)
        );
        assert_eq!(
            effective_deadline(FROM_EVENT, own, None, Some("2026-11-30")),
            (Some("2026-11-30".into()), false)
        );
        assert_eq!(
            effective_deadline(FROM_EVENT, own, None, None),
            (Some("2026-11-10".into()), true)
        );
        assert_eq!(
            effective_deadline(FROM_MILESTONE, None, None, None),
            (None, true)
        );
    }

    // --- the store ---

    #[test]
    fn tasks_list_in_the_order_they_were_added() {
        let (_dir, conn) = store();
        let a = add(&conn, &fields("a"), SOURCE_USER).unwrap();
        let b = add(&conn, &in_project("b", "Atlas"), SOURCE_USER).unwrap();
        let c = add(&conn, &fields("c"), "assistant").unwrap();
        let all = list(&conn, None, TODAY).unwrap();
        assert_eq!(all.iter().map(|t| t.id).collect::<Vec<_>>(), vec![a, b, c]);
        assert_eq!(
            all.iter().map(|t| t.ordinal).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(all[2].source, "assistant");
        assert_eq!(all[0].status, "todo");
        let atlas = list(&conn, Some("Atlas"), TODAY).unwrap();
        assert_eq!(atlas.len(), 1);
        assert_eq!(atlas[0].id, b);
        assert!(err(add(&conn, &fields("d"), "robot")).contains("robot"));
    }

    #[test]
    fn update_rewrites_the_fields_and_refuses_an_unknown_id() {
        let (_dir, conn) = store();
        let id = add(&conn, &fields("draft"), SOURCE_USER).unwrap();
        let f = TaskFields {
            why: Some("for the pitch".into()),
            do_start: Some("2026-11-09".into()),
            deadline: Some("2026-11-13".into()),
            deadline_kind: "soft".into(),
            lane: "background".into(),
            parked: true,
            ..in_project("final draft", "Atlas")
        };
        update(&conn, id, &f).unwrap();
        let t = get(&conn, id);
        assert_eq!(t.title, "final draft");
        assert_eq!(t.why.as_deref(), Some("for the pitch"));
        assert_eq!(t.project_name.as_deref(), Some("Atlas"));
        assert_eq!(t.do_start.as_deref(), Some("2026-11-09"));
        assert_eq!(t.effective_deadline.as_deref(), Some("2026-11-13"));
        assert_eq!(t.deadline_kind, "soft");
        assert_eq!(t.lane, "background");
        assert!(t.parked);
        assert!(err(update(&conn, id + 99, &f)).contains("no longer exists"));
        // A refused edit changes nothing.
        let bad = TaskFields {
            title: " ".into(),
            ..f
        };
        assert!(update(&conn, id, &bad).is_err());
        assert_eq!(get(&conn, id).title, "final draft");
    }

    #[test]
    fn plan_moves_only_the_window_lane_and_parked() {
        let (_dir, conn) = store();
        let f = TaskFields {
            deadline: Some("2026-11-20".into()),
            ..in_project("t", "Atlas")
        };
        let id = add(&conn, &f, SOURCE_USER).unwrap();
        plan(
            &conn,
            id,
            Some("2026-11-09".into()),
            Some("2026-11-11".into()),
            "background",
            true,
        )
        .unwrap();
        let t = get(&conn, id);
        assert_eq!(
            (t.do_start.as_deref(), t.do_end.as_deref()),
            (Some("2026-11-09"), Some("2026-11-11"))
        );
        assert_eq!(t.lane, "background");
        assert!(t.parked);
        assert_eq!(
            t.deadline.as_deref(),
            Some("2026-11-20"),
            "deadline untouched"
        );
        assert_eq!(t.project_name.as_deref(), Some("Atlas"));
        assert!(plan(
            &conn,
            id,
            None,
            Some("2026-11-11".into()),
            "foreground",
            false
        )
        .is_err());
        assert!(plan(&conn, id, None, None, "sideways", false).is_err());
        assert!(err(plan(&conn, id + 9, None, None, "foreground", false)).contains("no longer"));
    }

    #[test]
    fn reorder_permutes_within_the_subsets_own_slots() {
        let (_dir, conn) = store();
        let ids: Vec<i64> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|t| add(&conn, &fields(t), SOURCE_USER).unwrap())
            .collect();
        // Reorder b, d, e as e, b, d: they take slots 1, 3, 4; a and c keep theirs.
        reorder(&conn, &[ids[4], ids[1], ids[3]]).unwrap();
        assert_eq!(titles(&conn), vec!["a", "e", "c", "b", "d"]);
        // An unknown or repeated id is refused and changes nothing.
        assert!(err(reorder(&conn, &[ids[0], 999])).contains("no longer"));
        assert!(err(reorder(&conn, &[ids[0], ids[0]])).contains("twice"));
        assert_eq!(titles(&conn), vec!["a", "e", "c", "b", "d"]);
        reorder(&conn, &[]).unwrap();
    }

    #[test]
    fn completed_at_follows_the_status() {
        let (_dir, conn) = store();
        let id = add(&conn, &fields("t"), SOURCE_USER).unwrap();
        assert_eq!(get(&conn, id).completed_at, None);
        change_status(&conn, id, "doing").unwrap();
        assert_eq!(get(&conn, id).completed_at, None);
        change_status(&conn, id, "done").unwrap();
        let stamped = get(&conn, id)
            .completed_at
            .expect("done stamps completed_at");
        // Already done: the stamp is kept, not renewed.
        conn.execute(
            "UPDATE tasks SET completed_at = '2026-01-01T00:00:00.000Z' WHERE id = ?1",
            params![id],
        )
        .unwrap();
        change_status(&conn, id, "done").unwrap();
        assert_eq!(
            get(&conn, id).completed_at.as_deref(),
            Some("2026-01-01T00:00:00.000Z")
        );
        assert!(!stamped.is_empty());
        change_status(&conn, id, "todo").unwrap();
        let t = get(&conn, id);
        assert_eq!((t.status.as_str(), t.completed_at), ("todo", None));
        assert!(err(change_status(&conn, id, "finished")).contains("finished"));
        assert!(err(change_status(&conn, id + 5, "done")).contains("no longer"));
    }

    #[test]
    fn remove_deletes_and_ignores_an_unknown_id() {
        let (_dir, conn) = store();
        let id = add(&conn, &fields("t"), SOURCE_USER).unwrap();
        remove(&conn, id).unwrap();
        remove(&conn, id).unwrap();
        assert!(list(&conn, None, TODAY).unwrap().is_empty());
    }

    #[test]
    fn import_skips_blanks_existing_titles_and_repeats() {
        let (_dir, conn) = store();
        add(&conn, &in_project("Buy milk", "Home"), SOURCE_USER).unwrap();
        add(&conn, &fields("Call Sam"), SOURCE_USER).unwrap();
        let items = |pairs: &[(&str, bool)]| -> Vec<ChecklistItem> {
            pairs
                .iter()
                .map(|(t, d)| ChecklistItem {
                    title: t.to_string(),
                    done: *d,
                })
                .collect()
        };
        let r = import_checklist(
            &conn,
            Some("Home"),
            &items(&[
                (" buy MILK ", false), // already a Home task
                ("Call Sam", false),   // exists, but with no project: a different scope
                ("Fix tap", true),
                ("fix tap", false), // repeats an earlier line
                ("   ", false),
            ]),
        )
        .unwrap();
        assert_eq!(r.created.len(), 2);
        assert_eq!(r.skipped, 3);
        let home = list(&conn, Some("Home"), TODAY).unwrap();
        let tap = home.iter().find(|t| t.title == "Fix tap").unwrap();
        assert_eq!(tap.status, "done");
        assert!(
            tap.completed_at.is_some(),
            "a ticked line is done, with a stamp"
        );
        let sam = home.iter().find(|t| t.title == "Call Sam").unwrap();
        assert_eq!(sam.status, "todo");
        assert_eq!(sam.source, "user");

        // With no project, NULL matches NULL.
        let r =
            import_checklist(&conn, None, &items(&[("call sam", false), ("New", false)])).unwrap();
        assert_eq!((r.created.len(), r.skipped), (1, 1));
        // An over-long line is shortened, not refused.
        let long = "y".repeat(TITLE_MAX + 20);
        let r = import_checklist(&conn, None, &items(&[(long.as_str(), false)])).unwrap();
        assert_eq!(get(&conn, r.created[0]).title.chars().count(), TITLE_MAX);
    }

    #[test]
    fn a_milestone_link_lends_its_project_and_refuses_another() {
        let (_dir, conn) = store();
        let mid = crate::milestones::add(&conn, "Atlas", "Pitch", None, None).unwrap();
        let f = TaskFields {
            milestone_id: Some(mid),
            ..fields("deck")
        };
        let id = add(&conn, &f, SOURCE_USER).unwrap();
        let t = get(&conn, id);
        assert_eq!(t.project_name.as_deref(), Some("Atlas"));
        assert_eq!(t.milestone_label.as_deref(), Some("Pitch"));

        let f = TaskFields {
            milestone_id: Some(mid),
            ..in_project("deck", "Other")
        };
        assert!(err(add(&conn, &f, SOURCE_USER)).contains("belongs to another project"));
        let f = TaskFields {
            milestone_id: Some(mid + 50),
            ..fields("deck")
        };
        assert!(err(add(&conn, &f, SOURCE_USER)).contains("milestone no longer exists"));
        let f = TaskFields {
            stack_id: Some(77),
            ..fields("deck")
        };
        assert!(err(add(&conn, &f, SOURCE_USER)).contains("stack no longer exists"));
    }

    #[test]
    fn a_deadline_can_follow_the_milestone() {
        let (_dir, conn) = store();
        let dated =
            crate::milestones::add(&conn, "Atlas", "Pitch", Some("2026-11-20".into()), None)
                .unwrap();
        let undated = crate::milestones::add(&conn, "Atlas", "Later", None, None).unwrap();
        let follow = |mid: i64| TaskFields {
            milestone_id: Some(mid),
            deadline: Some("2026-11-15".into()),
            deadline_from: FROM_MILESTONE.into(),
            ..fields("deck")
        };
        let a = add(&conn, &follow(dated), SOURCE_USER).unwrap();
        let t = get(&conn, a);
        assert_eq!(t.effective_deadline.as_deref(), Some("2026-11-20"));
        assert!(!t.deadline_missing);
        assert_eq!(t.deadline.as_deref(), Some("2026-11-15"), "own date kept");

        // A milestone with no date: the task's own date, flagged.
        let b = add(&conn, &follow(undated), SOURCE_USER).unwrap();
        let t = get(&conn, b);
        assert_eq!(t.effective_deadline.as_deref(), Some("2026-11-15"));
        assert!(t.deadline_missing);

        // A calendar-linked milestone resolves through its event.
        event(&conn, "pitch@x", "Pitch day", &["2026-11-27T12:00:00Z"]);
        crate::milestones::set_event(&conn, dated, Some("pitch@x".into()), None, None).unwrap();
        assert_eq!(
            get(&conn, a).effective_deadline.as_deref(),
            Some("2026-11-27")
        );

        // A deleted milestone nulls the link; the deadline falls back, flagged.
        crate::milestones::remove(&conn, dated).unwrap();
        let t = get(&conn, a);
        assert_eq!(t.milestone_id, None);
        assert_eq!(t.milestone_label, None);
        assert_eq!(t.effective_deadline.as_deref(), Some("2026-11-15"));
        assert!(t.deadline_missing);
        assert_eq!(
            t.project_name.as_deref(),
            Some("Atlas"),
            "the project stays"
        );
    }

    #[test]
    fn with_no_pin_an_event_deadline_takes_the_next_occurrence_and_falls_back_when_it_is_gone() {
        let (_dir, conn) = store();
        // A repeating event and no pin (no own date): the next occurrence on or after today wins.
        event(
            &conn,
            "review@x",
            "Friday review",
            &[
                "2026-10-30T15:00:00Z",
                "2026-11-06T15:00:00Z",
                "2026-11-13T15:00:00Z",
            ],
        );
        // "Do it Monday, the deadline is Friday's event."
        let f = TaskFields {
            do_start: Some("2026-11-02".into()),
            event_uid: Some("review@x".into()),
            deadline_from: FROM_EVENT.into(),
            ..fields("prep slides")
        };
        let id = add(&conn, &f, SOURCE_USER).unwrap();
        let t = get(&conn, id);
        assert_eq!(t.effective_deadline.as_deref(), Some("2026-11-06"));
        assert!(!t.deadline_missing);
        assert_eq!(t.event_title.as_deref(), Some("Friday review"));
        assert_eq!(t.deadline, None);

        // Bucketed in the user's zone: 23:30 UTC on the 6th is the 7th in Tokyo.
        conn.execute("DELETE FROM calendar_events", []).unwrap();
        event(
            &conn,
            "review@x",
            "Friday review",
            &["2026-11-06T23:30:00Z"],
        );
        assert_eq!(
            get(&conn, id).effective_deadline.as_deref(),
            Some("2026-11-06")
        );
        crate::db::set_setting(&conn, crate::settings::TIME_ZONE_KEY, "Asia/Tokyo").unwrap();
        assert_eq!(
            get(&conn, id).effective_deadline.as_deref(),
            Some("2026-11-07")
        );

        // Only past occurrences: the latest past one.
        conn.execute("DELETE FROM calendar_events", []).unwrap();
        event(
            &conn,
            "review@x",
            "Friday review",
            &["2026-10-23T03:00:00Z", "2026-10-30T03:00:00Z"],
        );
        assert_eq!(
            get(&conn, id).effective_deadline.as_deref(),
            Some("2026-10-30")
        );

        // The event is gone: no date of its own, so none at all, flagged; no title either.
        conn.execute("DELETE FROM calendar_events", []).unwrap();
        let t = get(&conn, id);
        assert_eq!(t.effective_deadline, None);
        assert!(t.deadline_missing);
        assert_eq!(t.event_title, None);
        // With an own date (a pin) it falls back to that.
        let f = TaskFields {
            deadline: Some("2026-11-05".into()),
            ..f
        };
        update(&conn, id, &f).unwrap();
        let t = get(&conn, id);
        assert_eq!(t.effective_deadline.as_deref(), Some("2026-11-05"));
        assert!(t.deadline_missing);
    }

    /// A task following `ical`'s date, pinned to the occurrence on `pin`.
    fn pinned(ical: &str, pin: &str) -> TaskFields {
        TaskFields {
            do_start: Some("2026-11-02".into()),
            event_uid: Some(ical.into()),
            deadline: Some(pin.into()),
            deadline_from: FROM_EVENT.into(),
            ..fields("prep slides")
        }
    }

    fn effective_on(conn: &Connection, id: i64, today: &str) -> (Option<String>, bool) {
        let t = list(conn, None, today)
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap();
        (t.effective_deadline, t.deadline_missing)
    }

    #[test]
    fn a_pinned_recurring_occurrence_stays_the_deadline_and_goes_late() {
        let (_dir, conn) = store();
        event(
            &conn,
            "review@x",
            "Friday review",
            &[
                "2026-10-30T15:00:00Z",
                "2026-11-06T15:00:00Z",
                "2026-11-13T15:00:00Z",
            ],
        );
        // "Do it Monday, the deadline is this Friday's review."
        let id = add(&conn, &pinned("review@x", "2026-11-06"), SOURCE_USER).unwrap();
        let on_time = (Some("2026-11-06".to_string()), false);
        assert_eq!(effective_on(&conn, id, TODAY), on_time);
        // Saturday: still the Friday that was picked, so it reads late, not "due on the 13th".
        assert_eq!(effective_on(&conn, id, "2026-11-07"), on_time);
        assert_eq!(effective_on(&conn, id, "2026-11-20"), on_time);
        // A pin already in the past is late today, not rolled on to the next Friday.
        let past = add(&conn, &pinned("review@x", "2026-10-30"), SOURCE_USER).unwrap();
        assert_eq!(
            effective_on(&conn, past, TODAY),
            (Some("2026-10-30".to_string()), false)
        );
        // An occurrence moved in the series is still the nearest one to its pin.
        conn.execute(
            "UPDATE calendar_events SET start = '2026-11-05T15:00:00Z' WHERE id = 'review@x#1'",
            [],
        )
        .unwrap();
        assert_eq!(
            effective_on(&conn, id, TODAY),
            (Some("2026-11-05".to_string()), false)
        );
    }

    #[test]
    fn a_pinned_single_event_follows_its_move_and_falls_back_when_it_is_gone() {
        let (_dir, conn) = store();
        event(&conn, "pitch@x", "Pitch", &["2026-11-06T09:00:00Z"]);
        let id = add(&conn, &pinned("pitch@x", "2026-11-06"), SOURCE_USER).unwrap();
        // Moved a week later in Google: the deadline follows it, however far from the pin.
        conn.execute(
            "UPDATE calendar_events SET start = '2026-11-20T09:00:00Z' WHERE uid = 'pitch@x'",
            [],
        )
        .unwrap();
        assert_eq!(
            effective_on(&conn, id, TODAY),
            (Some("2026-11-20".to_string()), false)
        );
        // Gone: the pin itself, flagged.
        conn.execute("DELETE FROM calendar_events", []).unwrap();
        assert_eq!(
            effective_on(&conn, id, TODAY),
            (Some("2026-11-06".to_string()), true)
        );
    }

    /// The mirror's first day in these pure tests: before every fixture date, so the aged-out rule
    /// stays out of the way unless a test asks for it.
    const MIRROR_FROM: &str = "2026-10-01";

    #[test]
    fn the_mirror_starts_at_the_start_of_last_month() {
        assert_eq!(mirror_start("2026-11-03"), "2026-10-01");
        assert_eq!(mirror_start("2027-01-31"), "2026-12-01");
        assert_eq!(mirror_start("nonsense"), "");
    }

    #[test]
    fn a_pin_older_than_the_mirror_stays_the_deadline_rather_than_creeping_forward() {
        // A weekly series whose pinned 06-11 occurrence has aged out: the mirror now starts 01-12 and
        // holds only later Fridays. The deadline stays the chosen Friday, never the oldest survivor.
        let dates: Vec<String> = ["2026-12-04", "2026-12-11", "2026-12-18"]
            .iter()
            .map(|d| d.to_string())
            .collect();
        assert_eq!(
            event_occurrence(&dates, Some("2026-11-06"), "2027-01-04", "2026-12-01"),
            Some("2026-11-06")
        );
        // A pin inside the mirror is still matched to its nearest occurrence (an event moved later).
        assert_eq!(
            event_occurrence(&dates, Some("2026-12-02"), "2026-12-01", "2026-12-01"),
            Some("2026-12-04")
        );
        // A one-off event moved far past its pin, the pin now older than the mirror: follow the move.
        let moved = vec!["2026-11-20".to_string()];
        assert_eq!(
            event_occurrence(&moved, Some("2026-09-28"), "2026-11-03", "2026-10-01"),
            Some("2026-11-20")
        );
        // With no occurrences at all the caller's fallback applies, flagged missing.
        assert_eq!(
            event_occurrence(&[], Some("2026-11-06"), "2027-01-04", "2026-12-01"),
            None
        );
    }

    #[test]
    fn the_occurrence_nearest_the_pin_wins_and_an_earlier_one_wins_a_tie() {
        let dates: Vec<String> = ["2026-10-30", "2026-11-06", "2026-11-12", "2026-11-20"]
            .iter()
            .map(|d| d.to_string())
            .collect();
        // Nearest.
        assert_eq!(
            event_occurrence(&dates, Some("2026-11-07"), TODAY, MIRROR_FROM),
            Some("2026-11-06")
        );
        assert_eq!(
            event_occurrence(&dates, Some("2026-12-25"), TODAY, MIRROR_FROM),
            Some("2026-11-20")
        );
        // 11-09 is three days from both 11-06 and 11-12: the earlier wins.
        assert_eq!(
            event_occurrence(&dates, Some("2026-11-09"), TODAY, MIRROR_FROM),
            Some("2026-11-06")
        );
        // No pin: today's rule, soonest on or after today, else the latest past one.
        assert_eq!(
            event_occurrence(&dates, None, TODAY, MIRROR_FROM),
            Some("2026-11-06")
        );
        assert_eq!(
            event_occurrence(&dates, None, "2026-12-01", MIRROR_FROM),
            Some("2026-11-20")
        );
        assert_eq!(
            event_occurrence(&[], Some("2026-11-06"), TODAY, MIRROR_FROM),
            None
        );
        assert_eq!(event_occurrence(&[], None, TODAY, MIRROR_FROM), None);
    }

    #[test]
    fn occurrences_are_bucketed_in_the_users_zone_ascending_without_repeats() {
        let (_dir, conn) = store();
        event(
            &conn,
            "late@x",
            "Late call",
            &[
                "2026-11-13T23:30:00Z",
                "2026-11-06T23:30:00Z",
                "2026-11-06T10:00:00Z",
            ],
        );
        let by_uid = crate::milestones::calendar_occurrences_by_uid(&conn).unwrap();
        assert_eq!(by_uid["late@x"], vec!["2026-11-06", "2026-11-13"]);
        crate::db::set_setting(&conn, crate::settings::TIME_ZONE_KEY, "Asia/Tokyo").unwrap();
        let by_uid = crate::milestones::calendar_occurrences_by_uid(&conn).unwrap();
        assert_eq!(
            by_uid["late@x"],
            vec!["2026-11-06", "2026-11-07", "2026-11-14"]
        );
        // The milestone resolver reads the same buckets.
        let soonest = crate::milestones::calendar_dates_by_uid(&conn, "2026-11-07").unwrap();
        assert_eq!(soonest["late@x"], "2026-11-07");
        let soonest = crate::milestones::calendar_dates_by_uid(&conn, "2026-12-01").unwrap();
        assert_eq!(soonest["late@x"], "2026-11-14");
    }

    // --- canonical project names (R1) ---

    #[test]
    fn a_case_variant_of_a_project_lands_on_the_project() {
        let (_dir, conn) = store();
        let a = add(&conn, &in_project("first", "Atlas"), SOURCE_USER).unwrap();
        let b = add(&conn, &in_project("email Sam", " atlas "), SOURCE_USER).unwrap();
        assert_eq!(get(&conn, b).project_name.as_deref(), Some("Atlas"));
        assert_eq!(count_for_project(&conn, "Atlas").unwrap(), 2);
        let stray: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE name = 'atlas'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stray, 0, "no row minted for the variant");
        // An edit canonicalises too.
        update(&conn, a, &in_project("first", "ATLAS")).unwrap();
        assert_eq!(get(&conn, a).project_name.as_deref(), Some("Atlas"));
        // A name nothing matches is a new project, kept as typed.
        let c = add(&conn, &in_project("c", "Brand New"), SOURCE_USER).unwrap();
        assert_eq!(get(&conn, c).project_name.as_deref(), Some("Brand New"));
    }

    #[test]
    fn an_alias_or_an_entity_only_project_resolves_to_its_canonical_name() {
        let (_dir, conn) = store();
        let pm = crate::entities::resolve_project(&conn, "Personal Manager", true)
            .unwrap()
            .unwrap();
        crate::entities::add_alias(&conn, pm, "PM").unwrap();
        // An exact alias, with no projects row yet.
        let a = add(&conn, &in_project("a", "PM"), SOURCE_USER).unwrap();
        assert_eq!(
            get(&conn, a).project_name.as_deref(),
            Some("Personal Manager")
        );
        // A case variant of an entity's canonical name.
        let b = add(&conn, &in_project("b", "personal manager"), SOURCE_USER).unwrap();
        assert_eq!(
            get(&conn, b).project_name.as_deref(),
            Some("Personal Manager")
        );
        // With a stray variant row as well, the entity's canonical name wins the case match.
        conn.execute("INSERT INTO projects(name) VALUES ('personal MANAGER')", [])
            .unwrap();
        assert_eq!(
            canonical_project(&conn, "PERSONAL manager").unwrap(),
            "Personal Manager"
        );
        // But an exact name is kept as is.
        assert_eq!(
            canonical_project(&conn, "personal MANAGER").unwrap(),
            "personal MANAGER"
        );
    }

    #[test]
    fn a_milestone_link_accepts_its_project_by_any_name_it_goes_by() {
        let (_dir, conn) = store();
        let mid = crate::milestones::add(&conn, "Atlas", "Pitch", None, None).unwrap();
        let f = TaskFields {
            milestone_id: Some(mid),
            ..in_project("deck", "atlas")
        };
        let id = add(&conn, &f, SOURCE_USER).unwrap();
        assert_eq!(get(&conn, id).project_name.as_deref(), Some("Atlas"));
        let f = TaskFields {
            milestone_id: Some(mid),
            ..in_project("deck", "Other")
        };
        assert!(err(add(&conn, &f, SOURCE_USER)).contains("belongs to another project"));
    }

    #[test]
    fn an_import_files_under_the_canonical_project() {
        let (_dir, conn) = store();
        add(&conn, &in_project("Buy milk", "Home"), SOURCE_USER).unwrap();
        let r = import_checklist(
            &conn,
            Some(" home "),
            &[
                ChecklistItem {
                    title: "buy milk".into(),
                    done: false,
                },
                ChecklistItem {
                    title: "Fix tap".into(),
                    done: false,
                },
            ],
        )
        .unwrap();
        // The existing Home task is seen (so skipped), and the new one is filed under Home.
        assert_eq!((r.created.len(), r.skipped), (1, 1));
        assert_eq!(
            project_of(&conn, r.created[0]).unwrap().as_deref(),
            Some("Home")
        );
        assert_eq!(count_for_project(&conn, "Home").unwrap(), 2);
    }

    #[test]
    fn an_event_link_with_its_own_deadline_keeps_the_own_date() {
        let (_dir, conn) = store();
        event(&conn, "sync@x", "Team sync", &["2026-11-04T09:00:00Z"]);
        let f = TaskFields {
            event_uid: Some("sync@x".into()),
            deadline: Some("2026-11-10".into()),
            ..fields("notes")
        };
        let id = add(&conn, &f, SOURCE_USER).unwrap();
        let t = get(&conn, id);
        assert_eq!(t.effective_deadline.as_deref(), Some("2026-11-10"));
        assert!(!t.deadline_missing);
        assert_eq!(t.event_title.as_deref(), Some("Team sync"));
    }

    #[test]
    fn a_deleted_project_leaves_its_task_without_one() {
        let (_dir, conn) = store();
        let id = add(&conn, &in_project("t", "Doomed"), SOURCE_USER).unwrap();
        assert_eq!(count_for_project(&conn, "Doomed").unwrap(), 1);
        assert_eq!(project_of(&conn, id).unwrap().as_deref(), Some("Doomed"));
        conn.execute("DELETE FROM projects WHERE name = 'Doomed'", [])
            .unwrap();
        let t = get(&conn, id);
        assert_eq!(t.project_name, None);
        assert_eq!(project_of(&conn, id).unwrap(), None);
        assert_eq!(count_for_project(&conn, "Doomed").unwrap(), 0);
    }

    #[test]
    fn a_deleted_stack_unstacks_its_tasks() {
        let (_dir, conn) = store();
        let sid = add_stack(&conn, " Launch ", "in_order", Some(" ".into())).unwrap();
        let f = TaskFields {
            stack_id: Some(sid),
            ..fields("t")
        };
        let id = add(&conn, &f, SOURCE_USER).unwrap();
        assert_eq!(get(&conn, id).stack_id, Some(sid));
        let stacks = list_stacks(&conn).unwrap();
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].title, "Launch");
        assert_eq!(stacks[0].note, None);

        update_stack(&conn, sid, "Launch week", "waiting_on", Some("Sam".into())).unwrap();
        let s = &list_stacks(&conn).unwrap()[0];
        assert_eq!(
            (s.title.as_str(), s.kind.as_str(), s.note.as_deref()),
            ("Launch week", "waiting_on", Some("Sam"))
        );
        assert!(err(update_stack(&conn, sid + 1, "x", "in_order", None)).contains("no longer"));
        assert!(err(add_stack(&conn, "x", "parallel", None)).contains("parallel"));

        remove_stack(&conn, sid).unwrap();
        assert!(list_stacks(&conn).unwrap().is_empty());
        let t = get(&conn, id);
        assert_eq!(t.stack_id, None);
        assert_eq!(t.title, "t", "the task stays");
    }

    #[test]
    fn project_archived_comes_from_the_project() {
        let (_dir, conn) = store();
        let a = add(&conn, &in_project("a", "Old"), SOURCE_USER).unwrap();
        let b = add(&conn, &fields("b"), SOURCE_USER).unwrap();
        crate::projects::set_archived(&conn, "Old", true).unwrap();
        assert!(get(&conn, a).project_archived);
        assert!(!get(&conn, b).project_archived);
        crate::projects::set_archived(&conn, "Old", false).unwrap();
        assert!(!get(&conn, a).project_archived);
    }
}
