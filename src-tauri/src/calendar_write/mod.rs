// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The pure core of changing a Google event (#884): every decision a save makes, with no HTTP, no
//! keychain and no database, so each is table-tested on its own. The commands that do the I/O (C4)
//! call into it; nothing here can reach Google by itself, which is why it needs no fence.
//!
//! - [`dto`]: what crosses the IPC boundary and what a save returns.
//! - [`gate`]: what PM may change about an event, and why not.
//! - [`time`]: wall-clock times in a named zone, DST included, and Google's start/end shapes.
//! - [`patch`]: the PATCH body built from a fresh copy of the event, and the rules for a changed one.
//! - [`classify`]: what Google's answer means, and whether one retry is safe.
//! - [`plan`]: the requests themselves, ids percent-encoded and `sendUpdates` always explicit.
//! - [`reconcile`]: keeping a just-saved event on screen until a sync shows it.
//!
//! The rules it rests on are in `docs/calendar-full-editing-plan-2026-10-09.md` §3: every edit is
//! diffed against a fresh GET, never the mirror (R1); a change Google made meanwhile is a conflict,
//! never overwritten (R2); the mirror is only ever written from Google's reply (R3).

pub mod classify;
pub mod dto;
pub mod gate;
pub mod patch;
pub mod plan;
pub mod reconcile;
pub mod time;
