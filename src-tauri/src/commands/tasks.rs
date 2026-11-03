// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tasks and task stacks (PA-1): thin commands over `crate::tasks`, the one owner of both tables.
//!
//! Every mutation touches the project or projects it changed (old and new), drops the DB guard,
//! then emits [`tasks::TASKS_CHANGED`] so every open view (project page, pinboard widget, Runway,
//! calendar overlay) re-reads. All quick synchronous work with no model calls, so the lock is held
//! only briefly; nothing here takes `state.conn()` twice. Free of model paths (the calendar write
//! fence), and no command starts with `set_` (the Settings "Saved" tick).

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, State};

use crate::error::Result;
use crate::tasks::{self, ChecklistItem, ImportResult, Task, TaskFields, TaskStack};
use crate::{clock, projects, AppState};

use super::shared::resolve_zone;

/// Tell every open view the task list changed. Called after the guard is dropped.
fn changed(app: &AppHandle) {
    let _ = app.emit(tasks::TASKS_CHANGED, ());
}

/// Bump the activity date of each named project (a task edit counts as engaging with it). The
/// same project named twice is touched once.
fn touch(conn: &Connection, names: &[Option<&str>]) -> Result<()> {
    let mut done: Vec<&str> = Vec::new();
    for name in names.iter().flatten().copied() {
        if !done.contains(&name) {
            projects::touch(conn, name)?;
            done.push(name);
        }
    }
    Ok(())
}

/// Every task (`project` absent) or one project's, in the global order with links resolved.
#[tauri::command]
pub fn list_tasks(state: State<'_, AppState>, project: Option<String>) -> Result<Vec<Task>> {
    let conn = state.conn()?;
    let today = clock::today_sql_in(resolve_zone(&conn));
    tasks::list(&conn, project.as_deref(), &today)
}

/// Add a task. A milestone link fills in the project when none is given. Returns the new id.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn add_task(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
    why: Option<String>,
    project: Option<String>,
    milestone_id: Option<i64>,
    event_uid: Option<String>,
    do_start: Option<String>,
    do_end: Option<String>,
    deadline: Option<String>,
    deadline_kind: String,
    deadline_from: String,
    lane: String,
    stack_id: Option<i64>,
    parked: bool,
) -> Result<i64> {
    let fields = TaskFields {
        title,
        why,
        project,
        milestone_id,
        event_uid,
        do_start,
        do_end,
        deadline,
        deadline_kind,
        deadline_from,
        lane,
        stack_id,
        parked,
    };
    let conn = state.conn()?;
    let id = tasks::add(&conn, &fields, tasks::SOURCE_USER)?;
    let project = tasks::project_of(&conn, id)?;
    touch(&conn, &[project.as_deref()])?;
    drop(conn);
    changed(&app);
    Ok(id)
}

/// Rewrite a task's editable fields. Touches the project it left and the one it joined.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn update_task(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    title: String,
    why: Option<String>,
    project: Option<String>,
    milestone_id: Option<i64>,
    event_uid: Option<String>,
    do_start: Option<String>,
    do_end: Option<String>,
    deadline: Option<String>,
    deadline_kind: String,
    deadline_from: String,
    lane: String,
    stack_id: Option<i64>,
    parked: bool,
) -> Result<()> {
    let fields = TaskFields {
        title,
        why,
        project,
        milestone_id,
        event_uid,
        do_start,
        do_end,
        deadline,
        deadline_kind,
        deadline_from,
        lane,
        stack_id,
        parked,
    };
    let conn = state.conn()?;
    let before = tasks::project_of(&conn, id)?;
    tasks::update(&conn, id, &fields)?;
    let after = tasks::project_of(&conn, id)?;
    touch(&conn, &[before.as_deref(), after.as_deref()])?;
    drop(conn);
    changed(&app);
    Ok(())
}

/// Re-plan a task: its plan window, lane and parked flag (the Runway's drag and Move… menu).
#[tauri::command]
pub fn plan_task(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    do_start: Option<String>,
    do_end: Option<String>,
    lane: String,
    parked: bool,
) -> Result<()> {
    let conn = state.conn()?;
    tasks::plan(&conn, id, do_start, do_end, &lane, parked)?;
    let project = tasks::project_of(&conn, id)?;
    touch(&conn, &[project.as_deref()])?;
    drop(conn);
    changed(&app);
    Ok(())
}

/// Move a task to `status` (todo / doing / done), stamping or clearing when it was completed.
#[tauri::command]
pub fn change_task_status(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    status: String,
) -> Result<()> {
    let conn = state.conn()?;
    tasks::change_status(&conn, id, &status)?;
    let project = tasks::project_of(&conn, id)?;
    touch(&conn, &[project.as_deref()])?;
    drop(conn);
    changed(&app);
    Ok(())
}

/// Put these tasks in this order, within their own slots of the global order. A reorder changes
/// no project's content, so it touches none: a drag across a Runway lane spanning several
/// projects shouldn't make them all look freshly worked on.
#[tauri::command]
pub fn reorder_tasks(
    app: AppHandle,
    state: State<'_, AppState>,
    ordered_ids: Vec<i64>,
) -> Result<()> {
    let conn = state.conn()?;
    tasks::reorder(&conn, &ordered_ids)?;
    drop(conn);
    changed(&app);
    Ok(())
}

/// Delete a task.
#[tauri::command]
pub fn delete_task(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<()> {
    let conn = state.conn()?;
    // Captured before the row is gone.
    let project = tasks::project_of(&conn, id)?;
    tasks::remove(&conn, id)?;
    touch(&conn, &[project.as_deref()])?;
    drop(conn);
    changed(&app);
    Ok(())
}

/// Turn a note's checklist lines into tasks, skipping blanks and ones already there.
#[tauri::command]
pub fn import_checklist_tasks(
    app: AppHandle,
    state: State<'_, AppState>,
    project: Option<String>,
    items: Vec<ChecklistItem>,
) -> Result<ImportResult> {
    let conn = state.conn()?;
    let result = tasks::import_checklist(&conn, project.as_deref(), &items)?;
    // The project the import resolved the typed name to (its canonical spelling), not the raw one.
    if let Some(first) = result.created.first() {
        let project = tasks::project_of(&conn, *first)?;
        touch(&conn, &[project.as_deref()])?;
    }
    drop(conn);
    changed(&app);
    Ok(result)
}

/// Every task stack.
#[tauri::command]
pub fn list_task_stacks(state: State<'_, AppState>) -> Result<Vec<TaskStack>> {
    let conn = state.conn()?;
    tasks::list_stacks(&conn)
}

/// Add a task stack. Returns its id.
#[tauri::command]
pub fn add_task_stack(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
    kind: String,
    note: Option<String>,
) -> Result<i64> {
    let conn = state.conn()?;
    let id = tasks::add_stack(&conn, &title, &kind, note)?;
    drop(conn);
    changed(&app);
    Ok(id)
}

/// Rewrite a task stack's title, kind and note.
#[tauri::command]
pub fn update_task_stack(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    title: String,
    kind: String,
    note: Option<String>,
) -> Result<()> {
    let conn = state.conn()?;
    tasks::update_stack(&conn, id, &title, &kind, note)?;
    drop(conn);
    changed(&app);
    Ok(())
}

/// Delete a task stack. Its tasks stay, unstacked.
#[tauri::command]
pub fn delete_task_stack(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<()> {
    let conn = state.conn()?;
    tasks::remove_stack(&conn, id)?;
    drop(conn);
    changed(&app);
    Ok(())
}
