// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The command surface exposed to the frontend. DB access locks the shared
//! connection only for quick synchronous work — never across an `.await` — so
//! the streaming chat command stays responsive.
//!
//! Split by surface area. `lib.rs`'s `generate_handler!` still names everything as
//! `commands::<name>`, which the glob re-exports below keep resolving — including the
//! `__cmd__*` macros `#[tauri::command]` generates alongside each fn. Submodules stay
//! private so there is exactly one public path to every item.

mod archivist;
mod assistant;
mod backups;
mod calendars;
mod canon;
mod connectors;
mod conversations;
mod export;
mod keys;
mod organise;
mod prefs;
mod reader;
mod shared;
mod spend;
// Not re-exported: the DB-transaction ⊕ vault-file tail is machinery for the
// modules above, never a command, and `super::vault_writes::…` is how they name it.
mod vault_writes;
mod vaults;

pub use archivist::*;
pub use assistant::*;
pub use backups::*;
pub use calendars::*;
pub use canon::*;
pub use connectors::*;
pub use conversations::*;
pub use export::*;
pub use keys::*;
pub use organise::*;
pub use prefs::*;
pub use reader::*;
pub use spend::*;
pub use vaults::*;

// `resolve_zone` is called from `flags`, `milestones` and `projects` as
// `crate::commands::resolve_zone`, so `shared` is re-exported too — at `pub(crate)`, which is
// as wide as anything in it goes.
pub(crate) use shared::*;
