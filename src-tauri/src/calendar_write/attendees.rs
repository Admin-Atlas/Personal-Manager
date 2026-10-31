// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Guests: the user's changes to an event's guest list, as operations on Google's own list (R1), and
//! who Google would email about a write (R9). Nothing here fetches or sends.
//!
//! The user is never a guest Google emails (C12 critiques C1-H1, C2-H1): every count leaves out the
//! calendar's own addresses ([`Own`]), so the user's entry copied onto a "this and following"
//! change's new part, which Google hasn't marked `self` yet, is neither invited nor told. Rooms are
//! never emailed either, though they still count as guests when PM asks whether a list changed (R2).
//!
//! CodeQL guardrail: Google's lists are never edited in place (no `Vec::remove`, `insert` or
//! `swap_remove`; a new list is built with `filter_map` and `push`), the sets are PM's own lower-cased
//! copies, and no message names an address: [`GuestError`]'s words name none.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::{Map, Value};

use super::dto::{GuestNotice, GuestOp, GuestState, NewGuest};
use super::gate::{kind_of, Kind};
use crate::calendar::Attendee;

/// The domain Google gives rooms and other bookable resources.
const ROOM_DOMAIN: &str = "@resource.calendar.google.com";

/// Fields that are the user's own on their copy of an event: a change to only these emails nobody
/// (plan §3; R7 sends `none`). Busy/free is private to each guest's copy (Google's inviting-attendees
/// guide, in the plan's C14 card), so the organiser's is the organiser's own (INFERRED, probe L8-3).
pub const PERSONAL: &[&str] = &["transparency", "reminders", "colorId"];

/// Whether `text` is an email address as PM accepts one: trimmed, at most 254 characters, exactly one
/// `@` with 1-64 characters before it, a domain of dot-separated labels (at least two, none empty,
/// none starting or ending with `-`), and no space, double quote, comma, semicolon, colon or angle
/// bracket. Google refuses the rest with a 400 anyway; this says so before sending. Mirrored by
/// `isEmail` in the editor, one table for both.
pub fn is_address(text: &str) -> bool {
    let text = text.trim();
    if text.chars().count() > 254
        || text
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | ',' | ';' | ':' | '<' | '>'))
    {
        return false;
    }
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    let labels: Vec<&str> = domain.split('.').collect();
    (1..=64).contains(&local.chars().count())
        && !domain.contains('@')
        && labels.len() >= 2
        && labels
            .iter()
            .all(|l| !l.is_empty() && !l.starts_with('-') && !l.ends_with('-'))
}

/// Whether `email` is a room's or another bookable resource's (Google's own domain), for lists that
/// don't say (the mirror). Case-insensitive.
pub fn is_room_address(email: &str) -> bool {
    key(email).ends_with(ROOM_DOMAIN)
}

/// An address as PM compares it: trimmed, whatever its case.
fn key(email: &str) -> String {
    email.trim().to_lowercase()
}

fn address_of(entry: &Value) -> Option<String> {
    entry.get("email").and_then(Value::as_str).map(key)
}

fn flag(entry: &Value, name: &str) -> bool {
    entry.get(name).and_then(Value::as_bool).unwrap_or(false)
}

fn entries(list: Option<&Value>) -> impl Iterator<Item = &Value> {
    list.and_then(Value::as_array).into_iter().flatten()
}

/// A room, by Google's flag or, failing it, by its address.
fn is_room(entry: &Value) -> bool {
    flag(entry, "resource") || address_of(entry).is_some_and(|a| a.ends_with(ROOM_DOMAIN))
}

/// Google cut the event's guest list short.
pub fn omitted(event: &Value) -> bool {
    event.get("attendeesOmitted").and_then(Value::as_bool) == Some(true)
}

/// Why guest changes can't be made. Shown as is, and never names an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestError {
    NotAnAddress,
    Repeated,
    CantRemove,
    Omitted,
}

impl fmt::Display for GuestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GuestError::NotAnAddress => "One of the guests isn't an email address.",
            GuestError::Repeated => "A guest is listed twice.",
            GuestError::CantRemove => {
                "PM can't take the organiser or you off the guest list, or change them on it."
            }
            GuestError::Omitted => {
                "Google didn't send this event's whole guest list, so PM can't change it."
            }
        })
    }
}

/// The addresses no change may touch: the organiser's and the user's own entries.
fn fixed(event: &Value) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = entries(event.get("attendees"))
        .filter(|e| flag(e, "self") || flag(e, "organizer"))
        .filter_map(address_of)
        .collect();
    out.extend(event.get("organizer").and_then(address_of));
    out
}

/// The addresses on Google's `list`, lower-cased.
fn listed(list: Option<&Value>) -> BTreeSet<String> {
    entries(list).filter_map(address_of).collect()
}

/// Whether PM may send `op`: an address already on Google's list (`listed`) is matched by address
/// and goes back as Google has it, so only one PM adds is held to [`is_address`]. A calendar Google
/// lists as a guest can have more than 64 characters before its `@` (C12a review), and the user
/// can still take it off or make it optional.
fn sendable(op: &GuestOp, listed: &BTreeSet<String>) -> bool {
    listed.contains(&key(&op.email)) || is_address(&op.email)
}

/// Whether `ops` can be applied to `event` (Google's copy now): every address one PM may send, none
/// twice (by address, whatever the case), none changing the organiser's or the user's own entry, and
/// a whole list to apply them to. An empty `ops` always can.
pub fn check(event: &Value, ops: &[GuestOp]) -> Result<(), GuestError> {
    if ops.is_empty() {
        return Ok(());
    }
    let listed = listed(event.get("attendees"));
    let mut seen = BTreeSet::new();
    for op in ops {
        if !sendable(op, &listed) {
            return Err(GuestError::NotAnAddress);
        }
        if !seen.insert(key(&op.email)) {
            return Err(GuestError::Repeated);
        }
    }
    let fixed = fixed(event);
    if seen.iter().any(|a| fixed.contains(a)) {
        return Err(GuestError::CantRemove);
    }
    if omitted(event) {
        return Err(GuestError::Omitted);
    }
    Ok(())
}

/// What each op wants, by address: the first op for an address PM may send to `list` ([`check`]
/// refuses the rest).
fn wanted(list: Option<&Value>, ops: &[GuestOp]) -> BTreeMap<String, GuestState> {
    let listed = listed(list);
    let mut wanted = BTreeMap::new();
    for op in ops.iter().filter(|op| sendable(op, &listed)) {
        wanted.entry(key(&op.email)).or_insert(op.to);
    }
    wanted
}

/// `entry` as `to` wants it: left out, or with its optional flag set or the key removed. Everything
/// else as Google sent it.
fn as_wanted(entry: &Value, to: GuestState) -> Option<Value> {
    let optional = flag(entry, "optional");
    match (to, entry.as_object()) {
        (GuestState::Absent, _) => None,
        (GuestState::Required, Some(obj)) if optional => Some(Value::Object(
            obj.iter()
                .filter(|(k, _)| k.as_str() != "optional")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )),
        (GuestState::Optional, Some(obj)) if !optional => {
            let mut changed = obj.clone();
            changed.insert("optional".into(), Value::Bool(true));
            Some(Value::Object(changed))
        }
        _ => Some(entry.clone()),
    }
}

/// `list` (Google's `attendees`) with `ops` applied, as Google's own entries: every entry kept exactly
/// as Google sent it (its reply, comment, extra guests, room flag, name and Google's own keys), an
/// optional flag changed in place (`optional: true`, or the key removed for required), a guest taken
/// off left out, and a new one added at the end as `{"email": …}` (+ `"optional": true`), in the
/// order the ops name them. An op asking for what an entry already is changes nothing. Ops [`check`]
/// would refuse are skipped (call it first).
pub fn apply(list: Option<&Value>, ops: &[GuestOp]) -> Vec<Value> {
    let wanted = wanted(list, ops);
    let mut present = BTreeSet::new();
    let mut out: Vec<Value> = entries(list)
        .filter_map(|entry| {
            let Some(address) = address_of(entry) else {
                return Some(entry.clone());
            };
            present.insert(address.clone());
            let fixed = flag(entry, "self") || flag(entry, "organizer");
            match wanted.get(&address) {
                Some(to) if !fixed => as_wanted(entry, *to),
                _ => Some(entry.clone()),
            }
        })
        .collect();
    for op in ops {
        let address = key(&op.email);
        let to = wanted.get(&address).copied();
        if to.is_none_or(|to| to == GuestState::Absent) || !present.insert(address) {
            continue;
        }
        let mut entry = Map::new();
        entry.insert("email".into(), Value::from(op.email.trim()));
        if to == Some(GuestState::Optional) {
            entry.insert("optional".into(), Value::Bool(true));
        }
        out.push(Value::Object(entry));
    }
    out
}

/// Who is on a guest list, as `(address lower-cased, optional)`, sorted: every entry with an address,
/// rooms included, the user's own entry (`self`) left out, replies ignored. Two lists the same by this
/// are the same list: someone's reply is not a change (R2).
pub fn guest_set(list: Option<&Value>) -> Vec<(String, bool)> {
    guest_set_but(list, &Own::default())
}

/// [`guest_set`] with the calendar's own addresses left out too: for a list PM built (a new part's,
/// whose copy of the user's entry Google marks `self` only once it has it) against Google's.
pub fn guest_set_but(list: Option<&Value>, own: &Own) -> Vec<(String, bool)> {
    let mut set: Vec<(String, bool)> = entries(list)
        .filter(|e| !flag(e, "self"))
        .filter_map(|e| Some((address_of(e)?, flag(e, "optional"))))
        .filter(|(a, _)| !own.0.contains(a))
        .collect();
    set.sort_unstable();
    set
}

/// An entry as a new event takes it (a "this and following" change's new part): who, their name,
/// optional, room and extra guests; never their reply, comment, or Google's own keys (`id`, `self`,
/// `organizer`). A new event's guests are invited anew (INFERRED, probe L-R13-3). `None` without an
/// address.
pub fn for_new_event(entry: &Value) -> Option<Value> {
    entry.get("email").and_then(Value::as_str)?;
    let kept: Map<String, Value> = [
        "email",
        "displayName",
        "optional",
        "resource",
        "additionalGuests",
    ]
    .iter()
    .filter_map(|k| entry.get(*k).map(|v| ((*k).to_string(), v.clone())))
    .collect();
    Some(Value::Object(kept))
}

/// A new event's guest list from the editor's (C12d): each guest as `{"email": …}` (+ `"optional":
/// true`), trimmed, in the order added. Every one must be an address ([`is_address`]) and none listed
/// twice (by address, whatever the case): Google would refuse the rest anyway. The editor never adds
/// the user's own address (theirs is the organiser's); one that comes anyway goes as typed and is
/// never counted as someone emailed ([`Reach::of_insert`]).
pub fn new_list(guests: &[NewGuest]) -> Result<Vec<Value>, GuestError> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for guest in guests {
        if !is_address(&guest.email) {
            return Err(GuestError::NotAnAddress);
        }
        if !seen.insert(key(&guest.email)) {
            return Err(GuestError::Repeated);
        }
        let mut entry = Map::new();
        entry.insert("email".into(), Value::from(guest.email.trim()));
        if guest.optional {
            entry.insert("optional".into(), Value::Bool(true));
        }
        out.push(Value::Object(entry));
    }
    Ok(out)
}

/// The calendar's own addresses, lower-cased: never a guest Google emails.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Own(BTreeSet<String>);

impl Own {
    /// `addresses` (the calendar's id, and the account it is read with), and on each of `events` the
    /// entries Google marks `self` and an organiser it marks `self`.
    pub fn of(addresses: &[&str], events: &[&Value]) -> Own {
        let mut own: BTreeSet<String> = addresses
            .iter()
            .filter(|a| !a.trim().is_empty())
            .map(|a| key(a))
            .collect();
        for event in events {
            own.extend(
                entries(event.get("attendees"))
                    .filter(|e| flag(e, "self"))
                    .filter_map(address_of),
            );
            own.extend(
                event
                    .get("organizer")
                    .filter(|o| flag(o, "self"))
                    .and_then(address_of),
            );
        }
        Own(own)
    }
}

/// Whether a write to `event` (Google's copy) emails nobody whoever is on it: a birthday has no
/// guests, and a Gmail event's organiser isn't someone the user invited, so PM never asks about
/// these and sends `none` (C20 §4.4, critiques C1-M4, C2-M2). One rule for every count built on
/// [`Reach`]: a save's question, a delete's, a plan's steps, and an Undo's guest check.
fn emails_nobody(event: &Value) -> bool {
    kind_of(event.get("eventType").and_then(Value::as_str)) != Kind::Default
}

/// Who on `list` Google would email: every entry with an address that is neither the user's
/// (`self`, or one of `own`) nor a room.
fn recipients(list: Option<&Value>, own: &Own) -> BTreeSet<String> {
    entries(list)
        .filter(|e| !flag(e, "self") && !is_room(e))
        .filter_map(address_of)
        .filter(|a| !own.0.contains(a))
        .collect()
}

/// Whether Google's copy `event` has anyone it would email about a change, or a list it cut short:
/// the 412 rule's test (C12 critique C1-M6: never the user, never a room). `own` is the plan's, so
/// the rule and the plan's counts agree on who the user is (C12a review).
pub fn reaches_anyone(event: &Value, own: &Own) -> bool {
    omitted(event) || !recipients(event.get("attendees"), own).is_empty()
}

/// Who one write reaches, by address: built per write, merged for a plan. Counts come from who was on
/// the events before the change (C12 critiques C2-H2, C2-M7, C1-M2), never from a merged set of who
/// hears: someone already on the event who gets the new part's invitation is "already invited".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    /// Who was on the event before the write: none for a new event.
    was: BTreeSet<String>,
    /// Who the write emails.
    reached: BTreeSet<String>,
    /// Who it takes off.
    removed: BTreeSet<String>,
    /// Who only a delete or a cancel reaches: no other write merged reaches them, so all they hear
    /// is that an occurrence is cancelled, never an update.
    only_cancelled: BTreeSet<String>,
    /// Emails it would send: one per guest per event written.
    emails: u32,
    /// PM saw every guest list it counted.
    complete: bool,
    /// Every write merged is a delete or a cancel.
    cancelled: bool,
    /// It changes something guests share, so it would reach anyone on the event.
    shared: bool,
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl Reach {
    /// Nobody: what a plan with no write to anyone comes to.
    pub fn nobody() -> Reach {
        Reach {
            was: BTreeSet::new(),
            reached: BTreeSet::new(),
            removed: BTreeSet::new(),
            only_cancelled: BTreeSet::new(),
            emails: 0,
            complete: true,
            cancelled: false,
            shared: false,
        }
    }

    /// A PATCH of `body` onto `before`, Google's copy. Nobody on a birthday or a Gmail event
    /// ([`emails_nobody`]).
    pub fn of_patch(before: &Value, body: &Map<String, Value>, own: &Own) -> Reach {
        if emails_nobody(before) {
            return Reach::nobody();
        }
        Reach::patch(before.get("attendees"), omitted(before), body, own)
    }

    /// A PATCH of `body` onto an occurrence of a new event whose body was `new_event` (a carry onto a
    /// "this and following" change's new part): its guests are on it, but none was there before.
    pub fn of_carry(new_event: &Map<String, Value>, body: &Map<String, Value>, own: &Own) -> Reach {
        Reach {
            was: BTreeSet::new(),
            ..Reach::patch(new_event.get("attendees"), false, body, own)
        }
    }

    fn patch(list: Option<&Value>, cut_short: bool, body: &Map<String, Value>, own: &Own) -> Reach {
        let before = recipients(list, own);
        let after = match body.get("attendees") {
            Some(list) => recipients(Some(list), own),
            None => before.clone(),
        };
        let shared = body.keys().any(|k| !PERSONAL.contains(&k.as_str()));
        let removed: BTreeSet<String> = before.difference(&after).cloned().collect();
        let mut reached: BTreeSet<String> = after.difference(&before).cloned().collect();
        reached.extend(removed.iter().cloned());
        if shared {
            reached.extend(after.intersection(&before).cloned());
        }
        Reach {
            emails: count(reached.len()),
            was: before,
            reached,
            removed,
            only_cancelled: BTreeSet::new(),
            // A personal-only change emails nobody, however much of the list Google sent.
            complete: !cut_short || !shared,
            cancelled: false,
            shared,
        }
    }

    /// A delete (or a cancel) of `event`, Google's copy: everyone on it is told, but on a birthday
    /// or a Gmail event nobody ([`emails_nobody`]).
    pub fn of_delete(event: &Value, own: &Own) -> Reach {
        if emails_nobody(event) {
            return Reach::nobody();
        }
        let everyone = recipients(event.get("attendees"), own);
        Reach {
            emails: count(everyone.len()),
            reached: everyone.clone(),
            only_cancelled: everyone.clone(),
            was: everyone,
            removed: BTreeSet::new(),
            complete: !omitted(event),
            cancelled: true,
            shared: true,
        }
    }

    /// Putting `event` (Google's copy) on another calendar (C18d), after `body` is PATCHed onto it
    /// in the same Save (empty when it changes nothing else): Google's `sendUpdates` on that request
    /// is "about the change of the event's organizer", so everyone on it then, but the user and
    /// rooms, counts as told of a change. Whether Google emails anyone at all is live probe L-T2; if
    /// it never does, this becomes [`Reach::nobody`] and the question goes away.
    pub fn of_transfer(event: &Value, body: &Map<String, Value>, own: &Own) -> Reach {
        let was = recipients(event.get("attendees"), own);
        let told = match body.get("attendees") {
            Some(list) => recipients(Some(list), own),
            None => was.clone(),
        };
        Reach {
            emails: count(told.len()),
            reached: told,
            was,
            removed: BTreeSet::new(),
            only_cancelled: BTreeSet::new(),
            complete: !omitted(event),
            cancelled: false,
            shared: true,
        }
    }

    /// Whether an answer given for `asked` covers this: nobody it reaches that `asked` didn't count
    /// (someone gone since is simply not told), and no list cut short that `asked` read whole.
    pub fn within(&self, asked: &Reach) -> bool {
        self.reached.is_subset(&asked.reached) && (self.complete || !asked.complete)
    }

    /// A new event whose body is `body`: everyone on it is invited.
    pub fn of_insert(body: &Map<String, Value>, own: &Own) -> Reach {
        let everyone = recipients(body.get("attendees"), own);
        Reach {
            emails: count(everyone.len()),
            reached: everyone,
            was: BTreeSet::new(),
            removed: BTreeSet::new(),
            only_cancelled: BTreeSet::new(),
            complete: true,
            cancelled: false,
            shared: true,
        }
    }

    /// The two together, as one plan sends them.
    pub fn merge(mut self, other: Reach) -> Reach {
        // Anyone a write other than a cancel reaches, on either side, hears more than a cancel.
        let told_more: BTreeSet<String> = self
            .reached
            .difference(&self.only_cancelled)
            .chain(other.reached.difference(&other.only_cancelled))
            .cloned()
            .collect();
        self.only_cancelled.extend(other.only_cancelled);
        self.only_cancelled.retain(|a| !told_more.contains(a));
        self.was.extend(other.was);
        self.reached.extend(other.reached);
        self.removed.extend(other.removed);
        self.emails = self.emails.saturating_add(other.emails);
        self.complete &= other.complete;
        self.cancelled &= other.cancelled;
        self.shared |= other.shared;
        self
    }

    /// Mark it "at least": PM couldn't see every guest list this reaches.
    pub fn short(mut self) -> Reach {
        self.complete = false;
        self
    }

    /// Whether it changes something guests share (so a list PM couldn't see may hold someone).
    pub fn shared(&self) -> bool {
        self.shared
    }

    /// `gone`, who were there before, are taken off too: those a "this and following" change leaves
    /// off its new part (C12 critique C2-M7). Each is told already, by the write to the earlier part.
    pub fn taking_off(mut self, gone: BTreeSet<String>) -> Reach {
        self.removed
            .extend(gone.into_iter().filter(|a| self.was.contains(a)));
        self
    }

    /// Whether it deletes what it reaches: a delete's plan, whatever its writes (a cut is a PATCH).
    pub fn cancelling(mut self, cancelled: bool) -> Reach {
        self.cancelled = cancelled;
        self
    }

    /// What the user is told: `None` when nobody would be emailed and nothing was hidden. With
    /// nobody seen but a list cut short, it is the zeros, and the question still asks.
    pub fn notice(&self) -> Option<GuestNotice> {
        if self.reached.is_empty() && self.complete {
            return None;
        }
        let already = |a: &&String| self.was.contains(*a);
        // When everything it reaches is cancelled, those are among `updated`, told it's cancelled.
        let only_cancelled = |a: &&String| !self.cancelled && self.only_cancelled.contains(*a);
        let kept = |a: &&String| !self.removed.contains(*a);
        Some(GuestNotice {
            updated: count(
                self.reached
                    .iter()
                    .filter(already)
                    .filter(kept)
                    .filter(|a| !only_cancelled(a))
                    .count(),
            ),
            invited: count(self.reached.iter().filter(|a| !already(a)).count()),
            removed: count(self.removed.len()),
            told_cancelled: count(
                self.reached
                    .iter()
                    .filter(kept)
                    .filter(only_cancelled)
                    .count(),
            ),
            cancelled: self.cancelled,
            people: count(self.reached.len()),
            emails: self.emails,
            complete: self.complete,
        })
    }
}

/// Recipients of a new part's list, for [`Reach::taking_off`]: who the series reaches and this
/// list doesn't.
pub fn left_off(series: &Value, new_part: &Map<String, Value>, own: &Own) -> BTreeSet<String> {
    let kept = recipients(new_part.get("attendees"), own);
    recipients(series.get("attendees"), own)
        .into_iter()
        .filter(|a| !kept.contains(a))
        .collect()
}

/// Who Google would email if `body` were PATCHed onto `before` (Google's copy now), or `None` (R9).
/// `before` may be an invitation's copy (C14, A11): everyone but the user counts, the organiser
/// included, so the same table answers for a guest's own edit.
pub fn notify_required(
    before: &Value,
    body: &Map<String, Value>,
    own: &Own,
) -> Option<GuestNotice> {
    Reach::of_patch(before, body, own).notice()
}

/// Who Google would tell if `event` were deleted, or `None`.
pub fn delete_notice(event: &Value, own: &Own) -> Option<GuestNotice> {
    Reach::of_delete(event, own).notice()
}

/// The same from a mirror row's guests (no room flag there: by address). For asking about a delete
/// before it is held, when the row shows guests.
pub fn row_delete_notice(guests: &[Attendee], own: &Own) -> Option<GuestNotice> {
    let everyone: BTreeSet<String> = guests
        .iter()
        .filter(|g| !g.is_self)
        .filter_map(|g| g.email.as_deref().map(key))
        .filter(|a| !a.ends_with(ROOM_DOMAIN) && !own.0.contains(a))
        .collect();
    Reach {
        emails: count(everyone.len()),
        reached: everyone.clone(),
        only_cancelled: everyone.clone(),
        was: everyone,
        removed: BTreeSet::new(),
        complete: true,
        cancelled: true,
        shared: true,
    }
    .notice()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn me() -> Value {
        json!({ "email": "me@x.org", "self": true, "organizer": true, "responseStatus": "accepted" })
    }

    fn a() -> Value {
        json!({ "email": "A@x.org", "responseStatus": "accepted", "comment": "late",
                "additionalGuests": 1 })
    }

    fn b_guest() -> Value {
        json!({ "email": "B@x.org", "responseStatus": "needsAction" })
    }

    fn room() -> Value {
        json!({ "email": "room1@resource.calendar.google.com", "resource": true })
    }

    fn event(list: &[Value]) -> Value {
        json!({ "id": "e1", "organizer": { "email": "me@x.org", "self": true },
                "attendees": list })
    }

    fn op(email: &str, to: GuestState) -> GuestOp {
        GuestOp {
            email: email.into(),
            to,
        }
    }

    fn list(entries: Vec<Value>) -> Value {
        Value::from(entries)
    }

    /// A team calendar Google lists as a guest: 66 characters before its `@`, more than PM types.
    fn team() -> String {
        format!("c_{}@group.calendar.google.com", "0".repeat(64))
    }

    fn team_guest() -> Value {
        json!({ "email": team(), "responseStatus": "accepted" })
    }

    /// One table: Google's list, the ops, and the list sent (O1-O8, C12 §2.4).
    #[test]
    fn guest_ops_apply_to_googles_own_list() {
        use GuestState::*;
        let optional = |mut v: Value| {
            v["optional"] = json!(true);
            v
        };
        let rows: Vec<(&str, Option<Value>, Vec<GuestOp>, Value)> = vec![
            (
                "O1 add a guest",
                Some(list(vec![me(), a()])),
                vec![op("b@x.org", Required)],
                list(vec![me(), a(), json!({ "email": "b@x.org" })]),
            ),
            (
                "O2 the same guest in another case, made optional, keeps its reply and comment",
                Some(list(vec![me(), a()])),
                vec![op("a@X.ORG", Optional)],
                list(vec![me(), optional(a())]),
            ),
            (
                "O3 taken off",
                Some(list(vec![me(), a()])),
                vec![op("A@x.org", Absent)],
                list(vec![me()]),
            ),
            (
                "O4 made required: the key goes",
                Some(list(vec![me(), optional(a())])),
                vec![op("a@x.org", Required)],
                list(vec![me(), a()]),
            ),
            (
                "O5 a room stays as sent",
                Some(list(vec![me(), a(), room()])),
                vec![op("b@x.org", Optional)],
                list(vec![
                    me(),
                    a(),
                    room(),
                    json!({ "email": "b@x.org", "optional": true }),
                ]),
            ),
            (
                "O6 taking off someone not there",
                Some(list(vec![me(), a()])),
                vec![op("c@x.org", Absent)],
                list(vec![me(), a()]),
            ),
            (
                "O7 asking for what is already so",
                Some(list(vec![me(), a()])),
                vec![op("a@x.org", Required)],
                list(vec![me(), a()]),
            ),
            (
                "O8 no list at all",
                None,
                vec![op("b@x.org", Required)],
                list(vec![json!({ "email": "b@x.org" })]),
            ),
            (
                "O9 a guest Google holds that PM wouldn't take typed, taken off",
                Some(list(vec![me(), team_guest()])),
                vec![op(&team(), Absent)],
                list(vec![me()]),
            ),
            (
                "O10 the same guest made optional",
                Some(list(vec![me(), team_guest()])),
                vec![op(&team(), Optional)],
                list(vec![me(), optional(team_guest())]),
            ),
        ];
        for (name, before, ops, want) in rows {
            let got = Value::from(apply(before.as_ref(), &ops));
            assert!(got == want, "{name}");
        }
        // An address typed with spaces round it goes as the address; ops check would refuse are
        // skipped, and the user's own entry is never changed.
        let got = apply(
            Some(&list(vec![me()])),
            &[
                op(" c@x.org ", Required),
                op("not an address", Required),
                op("me@x.org", Absent),
            ],
        );
        assert!(
            Value::from(got) == list(vec![me(), json!({ "email": "c@x.org" })]),
            "trimmed, the rest skipped"
        );
    }

    /// C1-C7: what Google would refuse, or what PM won't do, is refused before anything is sent.
    #[test]
    fn check_refuses_what_google_would() {
        use GuestState::*;
        let organiser =
            json!({ "email": "o@x.org", "organizer": true, "responseStatus": "accepted" });
        let invitation = json!({ "id": "e1", "organizer": { "email": "o@x.org" },
                                 "attendees": [organiser, me(), a()] });
        let mut cut_short = event(&[me(), a()]);
        cut_short["attendeesOmitted"] = json!(true);
        type Row<'a> = (&'a str, Value, Vec<GuestOp>, Result<(), GuestError>);
        let rows: Vec<Row> = vec![
            (
                "C1",
                event(&[me(), a()]),
                vec![op("me@x.org", Absent)],
                Err(GuestError::CantRemove),
            ),
            (
                "C2",
                invitation.clone(),
                vec![op("O@x.org", Absent)],
                Err(GuestError::CantRemove),
            ),
            (
                "C3",
                event(&[me(), a()]),
                vec![op("me@x.org", Optional)],
                Err(GuestError::CantRemove),
            ),
            (
                "C4",
                cut_short.clone(),
                vec![op("b@x.org", Required)],
                Err(GuestError::Omitted),
            ),
            ("C5", cut_short, Vec::new(), Ok(())),
            (
                "C6",
                event(&[me()]),
                vec![op("bob at x", Required)],
                Err(GuestError::NotAnAddress),
            ),
            (
                "C7",
                event(&[me()]),
                vec![op("b@x.org", Required), op("B@X.org", Absent)],
                Err(GuestError::Repeated),
            ),
            (
                "an organiser off the list is still the organiser",
                json!({ "organizer": { "email": "o@x.org" }, "attendees": [me()] }),
                vec![op("o@x.org", Required)],
                Err(GuestError::CantRemove),
            ),
            (
                "anyone else",
                invitation,
                vec![op("a@x.org", Optional), op("c@x.org", Required)],
                Ok(()),
            ),
            (
                "a guest Google holds that PM wouldn't take typed, taken off",
                event(&[me(), team_guest()]),
                vec![op(&team(), Absent)],
                Ok(()),
            ),
            (
                "the same guest made optional, in another case",
                event(&[me(), team_guest()]),
                vec![op(&team().to_uppercase(), Optional)],
                Ok(()),
            ),
            (
                "the same address, added by PM",
                event(&[me(), a()]),
                vec![op(&team(), Required)],
                Err(GuestError::NotAnAddress),
            ),
        ];
        for (name, event, ops, want) in rows {
            assert!(check(&event, &ops) == want, "{name}");
        }
        // Words that name no address.
        for e in [
            GuestError::NotAnAddress,
            GuestError::Repeated,
            GuestError::CantRemove,
            GuestError::Omitted,
        ] {
            assert!(!e.to_string().contains('@'));
        }
    }

    fn title() -> Map<String, Value> {
        json!({ "summary": "Retro" }).as_object().unwrap().clone()
    }

    fn body(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn notice(updated: u32, invited: u32, removed: u32) -> GuestNotice {
        let people = updated + invited + removed;
        GuestNotice {
            updated,
            invited,
            removed,
            told_cancelled: 0,
            cancelled: false,
            people,
            emails: people,
            complete: true,
        }
    }

    /// N1-N15 (C12 §2.4). The editor's own notice runs the same rows (guests.test.ts, C12b): a row
    /// added to one is added to the other.
    #[test]
    fn notify_required_table() {
        let optional_a = {
            let mut v = a();
            v["optional"] = json!(true);
            v
        };
        let mut a_waiting = a();
        a_waiting["responseStatus"] = json!("needsAction");
        let mut cut_short = event(&[me(), a()]);
        cut_short["attendeesOmitted"] = json!(true);
        let organiser = json!({ "email": "o@x.org", "organizer": true });
        let mut invitation = event(&[organiser, json!({ "email": "me@x.org", "self": true }), a()]);
        invitation["organizer"] = json!({ "email": "o@x.org" });
        let busy = body(json!({ "transparency": "transparent" }));
        type Row<'a> = (&'a str, Value, Map<String, Value>, Option<GuestNotice>);
        let rows: Vec<Row> = vec![
            ("N1", json!({ "id": "e1" }), title(), None),
            ("N2", event(&[me()]), title(), None),
            ("N3", event(&[me(), room()]), title(), None),
            ("N4", event(&[me(), a()]), title(), Some(notice(1, 0, 0))),
            ("N5", event(&[me(), a()]), busy.clone(), None),
            (
                "N6",
                event(&[me(), a()]),
                body(json!({ "attendees": [me(), a(), { "email": "b@x.org" }] })),
                Some(notice(1, 1, 0)),
            ),
            (
                "N7",
                event(&[me()]),
                body(json!({ "attendees": [me(), { "email": "b@x.org" }] })),
                Some(notice(0, 1, 0)),
            ),
            (
                "N8",
                event(&[me(), a(), b_guest()]),
                body(json!({ "attendees": [me(), a()] })),
                Some(notice(1, 0, 1)),
            ),
            (
                "N9",
                event(&[me(), a()]),
                body(json!({ "attendees": [me(), optional_a] })),
                Some(notice(1, 0, 0)),
            ),
            // Google's copy now has A's reply; the editor opened on none. A reply changes nothing.
            ("N10", event(&[me(), a()]), title(), Some(notice(1, 0, 0))),
            ("N11", invitation, title(), Some(notice(2, 0, 0))),
            (
                "N12",
                cut_short.clone(),
                title(),
                Some(GuestNotice {
                    complete: false,
                    ..notice(1, 0, 0)
                }),
            ),
            ("N13", cut_short, busy, None),
            (
                "N14",
                event(&[me(), a()]),
                body(json!({ "guestsCanModify": true })),
                Some(notice(1, 0, 0)),
            ),
            (
                "N15",
                event(&[me(), a()]),
                body(json!({ "attendees": [me(), a(), room()] })),
                Some(notice(1, 0, 0)),
            ),
        ];
        // N10's opened copy: the same guest, awaiting a reply.
        assert!(
            guest_set(Some(&list(vec![me(), a_waiting])))
                == guest_set(Some(&list(vec![me(), a()])))
        );
        for (name, before, body, want) in rows {
            let own = Own::of(&["cal@x.org"], &[&before]);
            assert!(notify_required(&before, &body, &own) == want, "{name}");
        }
    }

    /// M1-M5 (C16a): a move sends a start and end, which every guest shares, so it tells every
    /// guest but the user and rooms of a change; on someone else's event the organiser too. The
    /// editor's `moveNotice` runs the same rows from the mirror's row (guests.test.ts; M5, a list
    /// Google cut short, has no mirror form).
    #[test]
    fn notify_required_for_a_move_table() {
        let times = body(json!({
            "start": { "dateTime": "2026-10-12T10:00:00+01:00" },
            "end": { "dateTime": "2026-10-12T11:00:00+01:00" },
        }));
        let organiser = json!({ "email": "o@x.org", "organizer": true });
        let mut invitation = event(&[organiser, json!({ "email": "me@x.org", "self": true }), a()]);
        invitation["organizer"] = json!({ "email": "o@x.org" });
        invitation["guestsCanModify"] = json!(true);
        let mut cut_short = event(&[me(), a()]);
        cut_short["attendeesOmitted"] = json!(true);
        type Row<'a> = (&'a str, Value, Option<GuestNotice>);
        let rows: Vec<Row> = vec![
            ("M1", event(&[me()]), None),
            ("M2", event(&[me(), a()]), Some(notice(1, 0, 0))),
            ("M3", event(&[me(), room()]), None),
            ("M4", invitation, Some(notice(2, 0, 0))),
            (
                "M5",
                cut_short,
                Some(GuestNotice {
                    complete: false,
                    ..notice(1, 0, 0)
                }),
            ),
        ];
        for (name, before, want) in rows {
            let own = Own::of(&["cal@x.org"], &[&before]);
            assert!(notify_required(&before, &times, &own) == want, "{name}");
        }
    }

    /// D1-D5: a delete tells every guest but the user and rooms; the mirror's row, which has no room
    /// flag, by the room's address.
    #[test]
    fn a_special_kinds_writes_email_nobody_whoever_is_on_it() {
        // An event Gmail added, organised by Google's placeholder, who is also on its list, and a
        // birthday Google sent with a guest on it all the same: neither asks, whatever the change.
        let organiser = json!({ "email": "unknownorganizer@calendar.google.com",
                                "organizer": true, "responseStatus": "accepted" });
        let copy = |event_type: Option<&str>| {
            let mut e = event(&[me(), organiser.clone(), a()]);
            if let Some(event_type) = event_type {
                e["eventType"] = json!(event_type);
            }
            e["attendeesOmitted"] = json!(true);
            e
        };
        let shared = body(json!({ "visibility": "private", "summary": "Flight" }));
        for event_type in ["birthday", "fromGmail"] {
            let e = copy(Some(event_type));
            let own = Own::of(&[], &[&e]);
            assert!(
                notify_required(&e, &shared, &own).is_none(),
                "{event_type}: a change"
            );
            assert!(delete_notice(&e, &own).is_none(), "{event_type}: a delete");
            assert!(
                Reach::of_patch(&e, &shared, &own) == Reach::nobody(),
                "{event_type}: nobody, and every list seen"
            );
        }
        // The same copy as an ordinary event reaches them, absent type or Google's default.
        for event_type in [None, Some("default")] {
            let e = copy(event_type);
            let own = Own::of(&[], &[&e]);
            assert!(
                notify_required(&e, &shared, &own).is_some(),
                "{event_type:?}: a change"
            );
            assert!(
                delete_notice(&e, &own).is_some(),
                "{event_type:?}: a delete"
            );
        }
    }

    #[test]
    fn a_delete_tells_every_guest_but_you_and_rooms() {
        let told = |updated| GuestNotice {
            updated,
            people: updated,
            emails: updated,
            cancelled: true,
            complete: true,
            ..Default::default()
        };
        let own = |e: &Value| Own::of(&[], &[e]);
        let d1 = event(&[me()]);
        assert!(delete_notice(&d1, &own(&d1)).is_none(), "D1");
        let d2 = event(&[me(), a()]);
        assert!(delete_notice(&d2, &own(&d2)) == Some(told(1)), "D2");
        let d3 = event(&[me(), room()]);
        assert!(delete_notice(&d3, &own(&d3)).is_none(), "D3");
        let mut d4 = event(&[me(), a()]);
        d4["attendeesOmitted"] = json!(true);
        assert!(
            delete_notice(&d4, &own(&d4))
                == Some(GuestNotice {
                    complete: false,
                    ..told(1)
                }),
            "D4"
        );
        let row = |email: &str, is_self: bool| Attendee {
            name: None,
            email: Some(email.into()),
            response: None,
            optional: false,
            organizer: is_self,
            is_self,
        };
        let mine = row("me@x.org", true);
        let none = Own::default();
        assert!(
            row_delete_notice(&[mine.clone(), row("A@x.org", false)], &none) == Some(told(1)),
            "D5, as D2"
        );
        assert!(
            row_delete_notice(
                &[mine, row("room1@Resource.Calendar.Google.com", false)],
                &none
            )
            .is_none(),
            "D5, as D3"
        );
        // The account's own address, unmarked in the mirror, is the user.
        assert!(
            row_delete_notice(&[row("me@x.org", false)], &Own::of(&["me@x.org"], &[])).is_none(),
            "never the user"
        );
    }

    /// C12 critique C1-M2: a new event with {me, A, B, c} beside a write to the series {me, A, B} and
    /// a cancel of one of its occurrences {me, A, B}: three people, each email counted, and only c
    /// is new.
    #[test]
    fn reach_counts_people_once_and_every_email() {
        let series = event(&[me(), a(), b_guest()]);
        let own = Own::of(&["cal@x.org"], &[&series]);
        // The new part's copy of the user's entry has no `self` yet: it is still the user.
        let new_part = body(json!({ "attendees": [
            { "email": "me@x.org" }, { "email": "A@x.org" }, { "email": "B@x.org" },
            { "email": "c@x.org" }
        ] }));
        let insert = Reach::of_insert(&new_part, &own);
        let cut = Reach::of_patch(
            &series,
            &body(json!({ "recurrence": ["RRULE:FREQ=WEEKLY;UNTIL=20261018"] })),
            &own,
        );
        let cancel = Reach::of_delete(&series, &own);
        assert!(
            insert.notice() == Some(notice(0, 3, 0)),
            "the insert invites three"
        );
        assert!(cut.notice() == Some(notice(2, 0, 0)), "the cut tells two");
        let all = insert.merge(cut).merge(cancel);
        assert!(
            all.notice()
                == Some(GuestNotice {
                    updated: 2,
                    invited: 1,
                    removed: 0,
                    told_cancelled: 0,
                    cancelled: false,
                    people: 3,
                    emails: 3 + 2 + 2,
                    complete: true,
                }),
            "three people, seven emails"
        );
        // Without the calendar's own addresses the user would be "new".
        let blind = Reach::of_insert(&new_part, &Own::default());
        assert!(blind.notice().is_some_and(|n| n.invited == 4));
    }

    /// C12c review round 3: a guest only a cancel reaches, in a plan that also changes something
    /// else, is told that occurrence is cancelled, never of an update; anyone another write reaches
    /// hears more than the cancel, whichever side of the merge, in whatever order.
    #[test]
    fn a_guest_only_a_cancel_reaches_is_told_of_the_cancel() {
        let series = event(&[me()]);
        let own = Own::of(&["cal@x.org"], &[&series]);
        let until = body(json!({ "recurrence": ["RRULE:FREQ=WEEKLY;UNTIL=20261018"] }));
        let cut = || Reach::of_patch(&series, &until, &own);
        let occurrence = event(&[me(), a()]);
        let cancel = || Reach::of_delete(&occurrence, &own);
        let told = GuestNotice {
            told_cancelled: 1,
            people: 1,
            emails: 1,
            complete: true,
            ..Default::default()
        };
        assert!(
            cut().merge(cancel()).notice() == Some(told.clone()),
            "cut, then cancel"
        );
        assert!(
            cancel().merge(cut()).notice() == Some(told),
            "cancel, then cut"
        );
        // A write other than the cancel reaches A too: A hears of an update, on either side.
        let shared = event(&[me(), a()]);
        let retitle = || Reach::of_patch(&shared, &title(), &own);
        let update = GuestNotice {
            updated: 1,
            people: 1,
            emails: 2,
            complete: true,
            ..Default::default()
        };
        for all in [
            cancel().merge(retitle()),
            retitle().merge(cancel()),
            cut().merge(cancel()).merge(retitle()),
            retitle().merge(cut().merge(cancel())),
        ] {
            assert!(all.notice() == Some(update.clone()));
        }
        // Two cancels of A, and a guest only the other write reaches: one each.
        let b_only = event(&[me(), b_guest()]);
        let both = cancel()
            .merge(Reach::of_patch(&b_only, &title(), &own))
            .merge(cancel());
        assert!(
            both.notice()
                == Some(GuestNotice {
                    updated: 1,
                    told_cancelled: 1,
                    people: 2,
                    emails: 3,
                    complete: true,
                    ..Default::default()
                })
        );
        // Taken off as well (left off a new part): counted once, as taken off.
        let gone = cancel()
            .merge(Reach::of_insert(
                &body(json!({ "attendees": [me()] })),
                &own,
            ))
            .taking_off(BTreeSet::from([key("A@x.org")]));
        assert!(
            gone.notice()
                == Some(GuestNotice {
                    removed: 1,
                    people: 1,
                    emails: 1,
                    complete: true,
                    ..Default::default()
                })
        );
        // A plain delete, or a delete's plan, keeps its own words: everyone is told it's cancelled.
        let deleted = GuestNotice {
            updated: 1,
            cancelled: true,
            people: 1,
            emails: 1,
            complete: true,
            ..Default::default()
        };
        assert!(cancel().notice() == Some(deleted.clone()), "a plain delete");
        assert!(
            cut().merge(cancel()).cancelling(true).notice() == Some(deleted),
            "a delete's plan"
        );
    }

    /// A user's entry Google hasn't marked `self` (the new part's, as PM sent it) is still the user,
    /// in every set PM compares by who is on it.
    #[test]
    fn the_users_own_entry_is_never_a_guest() {
        let mine = Own::of(&["Me@x.org"], &[]);
        let sent = list(vec![
            json!({ "email": "me@x.org" }),
            json!({ "email": "a@x.org" }),
        ]);
        let googles = list(vec![
            json!({ "email": "me@x.org", "self": true }),
            json!({ "email": "a@x.org", "responseStatus": "accepted" }),
        ]);
        assert!(guest_set_but(Some(&sent), &mine) == guest_set(Some(&googles)));
        assert!(guest_set(Some(&sent)) != guest_set(Some(&googles)));
        // An organiser marked `self` is the user too.
        let organised = json!({ "organizer": { "email": "team@group.x.org", "self": true } });
        let own = Own::of(&[], &[&organised]);
        let only_the_calendar = body(json!({ "attendees": [{ "email": "team@group.x.org" }] }));
        assert!(Reach::of_insert(&only_the_calendar, &own)
            .notice()
            .is_none());
        // Who is there to email at all, for the 412 rule: never the user, never a room.
        let none = Own::default();
        assert!(!reaches_anyone(&event(&[me(), room()]), &none));
        assert!(reaches_anyone(&event(&[me(), a()]), &none));
        let mut cut_short = event(&[me()]);
        cut_short["attendeesOmitted"] = json!(true);
        assert!(reaches_anyone(&cut_short, &none));
        // The user's address the list doesn't mark is still the user.
        let unmarked = event(&[json!({ "email": "Me@x.org" })]);
        assert!(reaches_anyone(&unmarked, &none));
        assert!(!reaches_anyone(&unmarked, &mine));
    }

    /// The editor's isEmail runs the same table (guests.test.ts, C12b): a row added to one is added to
    /// the other.
    #[test]
    fn an_address_is_checked_as_the_editor_checks_it() {
        let long_local = format!("{}@b.co", "x".repeat(65));
        let long = format!("a@{}.co", "b".repeat(250));
        let rows: Vec<(&str, bool)> = vec![
            ("a@b.co", true),
            ("A.B+tag@sub.x.org", true),
            ("a@b", false),
            ("a@@b.co", false),
            ("@b.co", false),
            ("a@b..co", false),
            ("a@-b.co", false),
            ("a b@c.co", false),
            ("\"a\"@b.co", false),
            ("a@b.co,", false),
            (long_local.as_str(), false),
            (long.as_str(), false),
            (" a@b.co ", true),
            ("o'brien@x.org", true),
        ];
        assert_eq!(long.chars().count(), 255);
        for (i, (text, want)) in rows.into_iter().enumerate() {
            assert!(is_address(text) == want, "row {i}");
        }
    }

    /// A new part's guests are invited anew: who, not their reply.
    #[test]
    fn a_new_event_takes_who_not_their_reply() {
        assert!(for_new_event(&a()) == Some(json!({ "email": "A@x.org", "additionalGuests": 1 })));
        assert!(
            for_new_event(&room())
                == Some(json!({ "email": "room1@resource.calendar.google.com", "resource": true }))
        );
        assert!(for_new_event(&me()) == Some(json!({ "email": "me@x.org" })));
        let named = json!({ "email": "c@x.org", "displayName": "C", "optional": true, "id": "x" });
        assert!(
            for_new_event(&named)
                == Some(json!({ "email": "c@x.org", "displayName": "C", "optional": true }))
        );
        assert!(for_new_event(&json!({ "displayName": "Nobody" })).is_none());
    }

    fn new_guest(email: &str, optional: bool) -> NewGuest {
        NewGuest {
            email: email.into(),
            optional,
        }
    }

    /// C12d: a new event's guests go as new entries, trimmed and in the order added, `optional`
    /// only where it's so; an address that isn't one, or one listed twice in any case, is refused
    /// with words that name no address.
    #[test]
    fn a_new_events_guests_go_as_new_entries() {
        let list = new_list(&[
            new_guest(" b@x.org ", false),
            new_guest("A@x.org", true),
            new_guest("room1@resource.calendar.google.com", false),
        ]);
        assert!(
            list == Ok(vec![
                json!({ "email": "b@x.org" }),
                json!({ "email": "A@x.org", "optional": true }),
                json!({ "email": "room1@resource.calendar.google.com" }),
            ]),
            "every guest, in order, optional only where it's so"
        );
        assert!(new_list(&[]) == Ok(Vec::new()));
        assert!(
            new_list(&[new_guest("b@x.org", false), new_guest("bob at x", false)])
                == Err(GuestError::NotAnAddress)
        );
        assert!(
            new_list(&[new_guest("b@x.org", false), new_guest(" B@X.org", true)])
                == Err(GuestError::Repeated)
        );
        for refused in [GuestError::NotAnAddress, GuestError::Repeated] {
            assert!(
                !refused.to_string().contains('@'),
                "no address in the words"
            );
        }
    }

    #[test]
    fn rooms_are_rooms_by_their_address() {
        assert!(is_room_address("Room1@Resource.Calendar.Google.com"));
        assert!(is_room_address(" room1@resource.calendar.google.com "));
        assert!(!is_room_address("room1@x.org"));
        assert!(!is_room_address("resource.calendar.google.com@x.org"));
        // A room the mirror's flag misses is still a room by its address.
        let unflagged = json!({ "email": "room2@resource.calendar.google.com" });
        assert!(!reaches_anyone(&event(&[me(), unflagged]), &Own::default()));
    }

    /// C18d (§8.8): putting an event on another calendar tells everyone on it but the user and
    /// rooms; in the same Save as a PATCH, each guest hears of both, and the guest list it goes with
    /// is the one the PATCH leaves. The editor counts the same rows (guests.test.ts).
    #[test]
    fn a_transfer_reaches_every_guest_but_the_user_and_rooms() {
        let own = Own::default();
        let none = Map::new();
        let solo = Reach::of_transfer(&event(&[me(), room()]), &none, &own);
        assert!(solo.notice().is_none(), "nobody but the user and a room");
        let two = event(&[me(), a(), b_guest(), room()]);
        assert!(Reach::of_transfer(&two, &none, &own).notice() == Some(notice(2, 0, 0)));
        // A title and the calendar in one Save: the same two people, an email each for both.
        let both = Reach::of_patch(&two, &title(), &own)
            .merge(Reach::of_transfer(&two, &title(), &own))
            .notice();
        assert!(
            both == Some(GuestNotice {
                emails: 4,
                ..notice(2, 0, 0)
            }),
            "two people, four emails"
        );
        // A guest added in the same Save is on the event when it goes: invited, then told.
        let added = body(json!({ "attendees": [me(), a(), { "email": "c@x.org" }] }));
        let with_new = Reach::of_patch(&event(&[me(), a()]), &added, &own)
            .merge(Reach::of_transfer(&event(&[me(), a()]), &added, &own))
            .notice();
        assert!(
            with_new
                == Some(GuestNotice {
                    emails: 4,
                    ..notice(1, 1, 0)
                }),
            "one updated, one invited, two emails each"
        );
        // A list Google cut short: "at least".
        let mut cut = event(&[me(), a()]);
        cut["attendeesOmitted"] = json!(true);
        assert!(
            Reach::of_transfer(&cut, &none, &own)
                .notice()
                .is_some_and(|n| !n.complete),
            "a list cut short is never whole"
        );
    }

    /// An answer covers a calendar change's reach now only for people it counted, and only for a
    /// list as whole as the one it read: a list cut short since may hold people it never counted.
    #[test]
    fn an_answer_covers_only_the_people_and_the_list_it_was_given_for() {
        let own = Own::default();
        let none = Map::new();
        let reach = |guests: &[Value], cut: bool| {
            let mut copy = event(guests);
            if cut {
                copy["attendeesOmitted"] = json!(true);
            }
            Reach::of_transfer(&copy, &none, &own)
        };
        let (one, two) = ([me(), a()], [me(), a(), b_guest()]);
        for (now, asked, covered, why) in [
            (reach(&two, false), reach(&two, false), true, "whole, whole"),
            (reach(&one, false), reach(&two, false), true, "someone gone"),
            (
                reach(&two, true),
                reach(&two, false),
                false,
                "cut short since",
            ),
            (reach(&two, true), reach(&two, true), true, "short, short"),
            (reach(&two, false), reach(&two, true), true, "whole, short"),
            (reach(&two, false), reach(&one, false), false, "someone new"),
            (
                reach(&two, true),
                reach(&one, true),
                false,
                "someone new, short",
            ),
        ] {
            assert!(now.within(&asked) == covered, "{why}");
        }
    }
}
