// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Learning You (spec §4.5) — **legacy, frozen**. Step 4a captured every change the user made to an
//! AI proposal into the `corrections` table; this module distilled those into a single free-text
//! **profile** blob (one `settings` value, model-self-edited) that was injected WHOLE into every
//! chat/proposal/briefing prompt.
//!
//! That "blob-in-context" pattern is superseded by the **structured preference model**
//! ([`crate::preferences`]): typed records retrieved by scope+condition at the decision point. The
//! blob distiller and its whole-blob preamble are retired; the stored blob is kept ARCHIVED and is
//! migrated ONCE into structured records (`commands::migrate_preferences_once`), so nothing
//! accumulated is lost. `corrections` keeps logging — it feeds the entity-alias loop and is the seam
//! for the deferred Stage-5 inferred-preference learning.
//!
//! What remains here is only the read path that Settings still uses to show the archived profile.

use rusqlite::Connection;
use serde::Serialize;

use crate::db;
use crate::error::Result;

/// Settings keys. The (now frozen) profile lives in the key/value `settings` table.
const PROFILE_KEY: &str = "learning_profile";
const PROFILE_UPDATED_KEY: &str = "learning_profile_updated_at";

/// The archived profile as shown in Settings (legacy view).
#[derive(Serialize)]
pub struct LearningProfile {
    pub profile: String,
    pub updated_at: Option<String>,
    pub correction_count: i64,
}

/// Read the stored (archived) profile + metadata for display.
pub fn get_profile(conn: &Connection) -> Result<LearningProfile> {
    let profile = db::get_setting(conn, PROFILE_KEY)?.unwrap_or_default();
    let updated_at = db::get_setting(conn, PROFILE_UPDATED_KEY)?;
    let correction_count: i64 =
        conn.query_row("SELECT count(*) FROM corrections", [], |r| r.get(0))?;
    Ok(LearningProfile {
        profile,
        updated_at,
        correction_count,
    })
}
