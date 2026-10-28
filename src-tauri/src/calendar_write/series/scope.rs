// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Which of "This event", "This and following" and "All events" a change can offer (T10). This is the
//! shape of the choice only: whether a scope can actually be carried out (a move that would lose an
//! occurrence, an exception PM can't carry) is the planner's answer, and C9 runs the planner for each
//! row it offers when the dialog opens, so each row shows its own warnings or reason (DS12).

use std::fmt;

use serde_json::Value;

use crate::calendar_recur::lines::TextOnly;
use crate::calendar_recur::{Point, RecurError};

use super::super::dto::{EventPatchDraft, SeriesScope, TimeDraft};
use super::super::patch::{build_patch, PatchError};
use super::super::time;
use super::rule::rule_change;
use super::{original_of, point_of, FreshEvent, Length, Master, SeriesRefusal};

/// The three choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    This,
    Following,
    All,
}

impl From<SeriesScope> for Scope {
    fn from(scope: SeriesScope) -> Scope {
        match scope {
            SeriesScope::This => Scope::This,
            SeriesScope::Following => Scope::Following,
            SeriesScope::All => Scope::All,
        }
    }
}

/// What a change does to the occurrence, as far as the choice cares.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Change {
    /// Title, place, description, busy/free, guests (C12), notifications or colour (C13).
    pub fields: bool,
    pub visibility: bool,
    /// What guests may do, which is the whole series', like visibility (C12).
    pub guest_permissions: bool,
    pub time: Option<TimeChange>,
    /// A new rule, or none at all (C11).
    pub rule: Option<RuleChange>,
    pub delete: bool,
}

/// A change to how the series repeats, as far as the choice cares (C11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleChange {
    /// A rule other than the series' own: a rule PM can't read is one too, and its planner says why.
    New,
    /// "Does not repeat".
    Stop,
}

/// A new time, as far as the choice cares. A move between all-day and timed, or into another zone,
/// is one like any other here: every occurrence follows it, rebuilt from the series' rule (C11
/// §12.4), and its planner says where that can't be done.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimeChange {
    /// Measured from where the occurrence came from, not where it is now (T7).
    pub date_moved: bool,
    pub length_changed: bool,
}

impl Change {
    pub fn delete() -> Change {
        Change {
            delete: true,
            ..Change::default()
        }
    }

    /// The change `draft` makes to the occurrence `instance` of `master`.
    pub fn of(
        draft: &EventPatchDraft,
        instance: &FreshEvent,
        master: &Master,
    ) -> Result<Change, PatchError> {
        let it = instance.json();
        let body = build_patch(it, draft)?;
        let permissions = [
            "guestsCanModify",
            "guestsCanInviteOthers",
            "guestsCanSeeOtherGuests",
        ];
        let mut change = Change {
            fields: [
                "summary",
                "location",
                "description",
                "transparency",
                "attendees",
                // Per occurrence in Google, so every row is offered, as for a title (C13).
                "reminders",
                "colorId",
            ]
            .iter()
            .chain(&permissions)
            .any(|k| body.contains_key(*k)),
            visibility: body.contains_key("visibility"),
            guest_permissions: permissions.iter().any(|k| body.contains_key(*k)),
            rule: rule_change(draft, master),
            ..Change::default()
        };
        if let (Some(draft_time), true) = (&draft.time, body.contains_key("start")) {
            let when = time::resolve_against(draft_time, it.get("start"), it.get("end"))?;
            change.time = Some(time_change(&when.start, &when.end, it, master));
        }
        Ok(change)
    }
}

/// Whether a timed draft puts the occurrence's start or end in a zone other than its own (an end
/// with no zone of its own is in the start's): the series' rule can't follow it, so every occurrence
/// is rebuilt from it in the new zone (C11 §12.4).
pub(super) fn zones_changed(draft: &TimeDraft, instance: &Value) -> bool {
    let TimeDraft::Timed {
        start_zone,
        end_zone,
        ..
    } = draft
    else {
        return false;
    };
    let zone_of = |key: &str| {
        instance
            .get(key)
            .and_then(|n| n.get("timeZone"))
            .and_then(Value::as_str)
    };
    let own_start = zone_of("start");
    let own_end = zone_of("end").or(own_start);
    own_start.is_some_and(|own| own != start_zone) || own_end.is_some_and(|own| own != end_zone)
}

fn time_change(start: &Value, end: &Value, instance: &Value, master: &Master) -> TimeChange {
    let (new_start, new_end) = (point_of(Some(start)), point_of(Some(end)));
    let zone = master.start.zone();
    let date_moved = match (original_of(instance), new_start) {
        (Some(o), Some(n)) => o.day_in(zone) != n.day_in(zone),
        _ => true,
    };
    let length_changed = match (master.length, new_start, new_end) {
        (Length::Timed(len), Some(Point::At(s)), Some(Point::At(e))) => e - s != len,
        (Length::AllDay(days), Some(Point::Day(s)), Some(Point::Day(e))) => {
            (e - s).num_days() != days
        }
        _ => true,
    };
    TimeChange {
        date_moved,
        length_changed,
    }
}

/// Why a row isn't offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeReason {
    /// Google applies an occurrence's visibility to the whole series (stricter) or ignores it.
    VisibilityIsSeriesWide,
    /// PM shows this series' rule as text only, so it can't move or cut it.
    TextOnly(TextOnly),
    /// A new rule, or none, can't apply to one occurrence (C11).
    RuleIsSeriesWide,
    /// What guests may do is the whole series' (C12 critiques C1-M7, C2-L11).
    GuestPermissionsAreSeriesWide,
}

/// Shown on the row as is, in the planners' own words for the same refusals.
impl fmt::Display for ScopeReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScopeReason::VisibilityIsSeriesWide => SeriesRefusal::VisibilityIsSeriesWide.fmt(f),
            ScopeReason::TextOnly(why) => RecurError::TextOnly(why.clone()).fmt(f),
            ScopeReason::RuleIsSeriesWide => SeriesRefusal::RuleIsSeriesWide.fmt(f),
            ScopeReason::GuestPermissionsAreSeriesWide => {
                SeriesRefusal::GuestPermissionsAreSeriesWide.fmt(f)
            }
        }
    }
}

/// One row of the choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Offered,
    /// "This and following" on the series' first occurrence is the whole series.
    AsAll,
    Refused(ScopeReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeOptions {
    pub this: Row,
    pub following: Row,
    pub all: Row,
}

/// The rows a `change` to an occurrence offers. `first`: it's the series' first occurrence.
/// `text_only`: why PM can only show the series' rule as text, if it can.
pub fn scope_options(change: &Change, first: bool, text_only: Option<&TextOnly>) -> ScopeOptions {
    let mut this = Row::Offered;
    let mut following = if first { Row::AsAll } else { Row::Offered };
    let mut all = Row::Offered;
    if change.visibility {
        this = Row::Refused(ScopeReason::VisibilityIsSeriesWide);
    }
    // Until a probe shows Google applies them to one occurrence (C12 critique C1-M7).
    if change.guest_permissions {
        this = Row::Refused(ScopeReason::GuestPermissionsAreSeriesWide);
    }
    // How the series repeats is the series' own: never one occurrence's (the plan's "This event is
    // absent when the rule changes", shown refused with its reason instead, C11 §0.4).
    if change.rule.is_some() {
        this = Row::Refused(ScopeReason::RuleIsSeriesWide);
    }
    // A new time moves the series' rule, which a rule PM can only show can't be. A new rule replaces
    // the old one whole, so it doesn't stand in the way then (C11 §9.2). A move between all-day and
    // timed, or into another zone, rebuilds every occurrence from the rule (C11 §12.4): offered like
    // any move, its planner refusing in words what it can't rebuild.
    if let (Some(_), None, Some(why)) = (change.time, change.rule, text_only) {
        following = Row::Refused(ScopeReason::TextOnly(why.clone()));
        all = Row::Refused(ScopeReason::TextOnly(why.clone()));
    }
    // Cutting a series needs its rule, whatever the change.
    if let Some(why) = text_only {
        if !first {
            following = Row::Refused(ScopeReason::TextOnly(why.clone()));
        }
    }
    ScopeOptions {
        this,
        following,
        all,
    }
}

/// Whether `instance` is its series' first occurrence (compared exactly: a master in the second pass
/// of a repeated hour is that one; AS15).
pub fn is_first(master: &Master, instance: &FreshEvent) -> bool {
    original_of(instance.json()) == Some(master.first)
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::*;
    use super::*;

    /// The scope the webview names is the scope a plan is made for, from the wire on.
    #[test]
    fn the_scope_sent_is_the_scope_planned() {
        for (json, scope) in [
            (r#""this""#, Scope::This),
            (r#""following""#, Scope::Following),
            (r#""all""#, Scope::All),
        ] {
            let sent: SeriesScope = serde_json::from_str(json).unwrap();
            assert_eq!(Scope::from(sent), scope, "{json}");
        }
    }

    fn timed(date: &str, start: &str, end: &str, zone: &str) -> TimeDraft {
        TimeDraft::Timed {
            start_date: date.into(),
            start_time: start.into(),
            start_zone: zone.into(),
            end_date: date.into(),
            end_time: end.into(),
            end_zone: zone.into(),
        }
    }

    fn change(draft: EventPatchDraft) -> Change {
        let m = Master::read(&fresh(master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]))).unwrap();
        Change::of(&draft, &fresh(plain("2026-10-12T09:00:00+01:00")), &m).unwrap()
    }

    /// What a draft changes, as the choice needs it, measured from where the occurrence came from.
    #[test]
    fn what_a_draft_changes() {
        let title = change(EventPatchDraft {
            summary: Some("Retro".into()),
            ..Default::default()
        });
        assert_eq!(
            title,
            Change {
                fields: true,
                ..Default::default()
            }
        );
        // The title it already has changes nothing.
        assert_eq!(
            change(EventPatchDraft {
                summary: Some("Standup".into()),
                ..Default::default()
            }),
            Change::default()
        );
        let later = change(EventPatchDraft {
            time: Some(timed("2026-10-12", "10:00", "11:00", "Europe/London")),
            ..Default::default()
        });
        assert_eq!(
            later.time,
            Some(TimeChange {
                date_moved: false,
                length_changed: false,
            })
        );
        let next_day_longer = change(EventPatchDraft {
            time: Some(timed("2026-10-13", "09:00", "11:00", "Europe/London")),
            ..Default::default()
        });
        assert_eq!(
            next_day_longer.time,
            Some(TimeChange {
                date_moved: true,
                length_changed: true,
            })
        );
        // Another zone, or the other kind, is a new time like any other here (C11 §12.4).
        let all_day = change(EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-12".into(),
                last_day: "2026-10-12".into(),
            }),
            ..Default::default()
        });
        assert!(all_day.time.is_some());
        // Which the planners rebuild every occurrence for, as they read it. C8b review G7: an end
        // put in another zone is a zone change too.
        let instance = plain("2026-10-12T09:00:00+01:00");
        let end_in_paris = TimeDraft::Timed {
            start_date: "2026-10-12".into(),
            start_time: "10:00".into(),
            start_zone: "Europe/London".into(),
            end_date: "2026-10-12".into(),
            end_time: "12:00".into(),
            end_zone: "Europe/Paris".into(),
        };
        for (draft, zoned) in [
            (
                timed("2026-10-12", "04:00", "05:00", "America/New_York"),
                true,
            ),
            (end_in_paris, true),
            (
                timed("2026-10-12", "10:00", "11:00", "Europe/London"),
                false,
            ),
        ] {
            assert_eq!(zones_changed(&draft, &instance), zoned);
        }
    }

    /// T10.
    #[test]
    fn which_choices_a_change_offers() {
        let text = Change {
            fields: true,
            ..Default::default()
        };
        assert_eq!(
            scope_options(&text, false, None),
            ScopeOptions {
                this: Row::Offered,
                following: Row::Offered,
                all: Row::Offered
            }
        );
        // The first occurrence: "this and following" is the whole series.
        assert_eq!(scope_options(&text, true, None).following, Row::AsAll);
        // Visibility is the series' (Google), so not for one occurrence.
        let visibility = Change {
            visibility: true,
            ..Default::default()
        };
        assert_eq!(
            scope_options(&visibility, false, None).this,
            Row::Refused(ScopeReason::VisibilityIsSeriesWide)
        );
        // A new time, of either kind and in any zone (C11 §12.4), on every row.
        let moved = Change {
            time: Some(TimeChange {
                date_moved: true,
                length_changed: true,
            }),
            ..Default::default()
        };
        assert_eq!(
            scope_options(&moved, false, None),
            ScopeOptions {
                this: Row::Offered,
                following: Row::Offered,
                all: Row::Offered
            }
        );
        // A rule PM shows as text: its time and its cut can't change, its text can.
        let custom = TextOnly::Part("BYSETPOS".into());
        let o = scope_options(&text, false, Some(&custom));
        assert_eq!(o.all, Row::Offered);
        assert_eq!(
            o.following,
            Row::Refused(ScopeReason::TextOnly(custom.clone()))
        );
        let o = scope_options(
            &Change {
                time: Some(TimeChange::default()),
                ..Default::default()
            },
            false,
            Some(&custom),
        );
        assert_eq!(o.all, Row::Refused(ScopeReason::TextOnly(custom.clone())));
        // A delete: every row, but a text-only rule can't be cut.
        let delete = scope_options(&Change::delete(), false, Some(&custom));
        assert_eq!(delete.this, Row::Offered);
        assert_eq!(delete.all, Row::Offered);
        assert_eq!(
            delete.following,
            Row::Refused(ScopeReason::TextOnly(custom))
        );
    }

    /// A refused row says why in the planners' words for the same refusal.
    #[test]
    fn a_refused_row_says_why() {
        assert_eq!(
            ScopeReason::VisibilityIsSeriesWide.to_string(),
            SeriesRefusal::VisibilityIsSeriesWide.to_string()
        );
        assert_eq!(
            ScopeReason::TextOnly(TextOnly::Part("BYSETPOS".into())).to_string(),
            "PM can't change how this event repeats: its rule uses BYSETPOS, which PM doesn't \
             change."
        );
        assert_eq!(
            ScopeReason::RuleIsSeriesWide.to_string(),
            "Changing how it repeats can't apply to just this occurrence."
        );
        assert_eq!(
            ScopeReason::RuleIsSeriesWide.to_string(),
            SeriesRefusal::RuleIsSeriesWide.to_string()
        );
        assert_eq!(
            ScopeReason::GuestPermissionsAreSeriesWide.to_string(),
            SeriesRefusal::GuestPermissionsAreSeriesWide.to_string()
        );
    }

    /// C12 §6.8: a guest list change is a change of fields like a title's, offered on every row;
    /// what guests may do is the whole series', so never for "This event" (critiques C1-M7,
    /// C2-L11). One Google already holds changes nothing.
    #[test]
    fn guests_are_fields_and_what_they_may_do_is_the_whole_series() {
        use crate::calendar_write::dto::{GuestOp, GuestState};
        let guests = change(EventPatchDraft {
            guests: Some(vec![GuestOp {
                email: "b@example.com".into(),
                to: GuestState::Required,
            }]),
            ..Default::default()
        });
        assert!(
            guests
                == Change {
                    fields: true,
                    ..Default::default()
                },
            "a change of fields"
        );
        assert!(scope_options(&guests, false, None).this == Row::Offered);
        let modify = change(EventPatchDraft {
            guests_can_modify: Some(true),
            ..Default::default()
        });
        assert!(modify.fields && modify.guest_permissions);
        let options = scope_options(&modify, false, None);
        assert!(options.this == Row::Refused(ScopeReason::GuestPermissionsAreSeriesWide));
        assert!(options.following == Row::Offered && options.all == Row::Offered);
        // Google's default, already so.
        let invite = change(EventPatchDraft {
            guests_can_invite_others: Some(true),
            ..Default::default()
        });
        assert!(invite == Change::default(), "nothing changes");
    }

    fn repeat(days: &[crate::calendar_recur::wire::DayCode]) -> EventPatchDraft {
        use crate::calendar_recur::wire::{RepeatEnds, RepeatPattern, RepeatRule};
        EventPatchDraft {
            recurrence: Some(crate::calendar_write::dto::RepeatDraft::Rule {
                rule: RepeatRule {
                    pattern: RepeatPattern::Weekly {
                        days: days.to_vec(),
                    },
                    interval: 1,
                    ends: RepeatEnds::Never,
                },
            }),
            ..Default::default()
        }
    }

    /// The test moved here from C9 (c9-design:108): a change to how it repeats never offers "This
    /// event", shown refused with its reason (C11 §0.4.5); a stop is one too.
    #[test]
    fn a_rule_change_refuses_this_event() {
        use crate::calendar_recur::wire::DayCode;
        let new = change(repeat(&[DayCode::Tue]));
        assert_eq!(new.rule, Some(RuleChange::New));
        let o = scope_options(&new, false, None);
        assert_eq!(o.this, Row::Refused(ScopeReason::RuleIsSeriesWide));
        assert_eq!((o.following, o.all), (Row::Offered, Row::Offered));
        let stop = change(EventPatchDraft {
            recurrence: Some(crate::calendar_write::dto::RepeatDraft::Once),
            ..Default::default()
        });
        assert_eq!(stop.rule, Some(RuleChange::Stop));
        assert_eq!(
            scope_options(&stop, false, None).this,
            Row::Refused(ScopeReason::RuleIsSeriesWide)
        );
        // From the first occurrence, "this and following" is still every occurrence.
        assert_eq!(scope_options(&new, true, None).following, Row::AsAll);
    }

    /// A rule PM can only show is replaced whole on every occurrence, but can't be cut, so "this and
    /// following" stays refused; a new time with the new rule doesn't stand in the way either.
    #[test]
    fn a_new_rule_on_a_custom_rule_offers_all_but_not_following() {
        let custom = TextOnly::Part("BYSETPOS".into());
        for time in [None, Some(TimeChange::default())] {
            let new = Change {
                rule: Some(RuleChange::New),
                time,
                ..Default::default()
            };
            let o = scope_options(&new, false, Some(&custom));
            assert_eq!(o.all, Row::Offered);
            assert_eq!(
                o.following,
                Row::Refused(ScopeReason::TextOnly(custom.clone()))
            );
        }
    }

    /// C11 §12.4: a change between timed and all-day, or into another zone, goes to every occurrence
    /// or from this one on, with the series' own rule or a new one, rebuilt in it. A rule PM can only
    /// show still can't be moved there.
    #[test]
    fn scope_rows_no_longer_refuse_kind_or_zone() {
        use crate::calendar_recur::wire::DayCode;
        let all_day = TimeDraft::AllDay {
            first_day: "2026-10-12".into(),
            last_day: "2026-10-12".into(),
        };
        let new_york = timed("2026-10-12", "09:00", "10:00", "America/New_York");
        let custom = TextOnly::Part("BYSETPOS".into());
        for time in [all_day, new_york] {
            for draft in [
                EventPatchDraft {
                    time: Some(time.clone()),
                    ..Default::default()
                },
                EventPatchDraft {
                    time: Some(time.clone()),
                    ..repeat(&[DayCode::Tue])
                },
            ] {
                let c = change(draft);
                assert!(c.time.is_some());
                let o = scope_options(&c, false, None);
                assert_eq!((o.following, o.all), (Row::Offered, Row::Offered));
                // Its own rule unreadable, only a new one can take it there.
                let o = scope_options(&c, false, Some(&custom));
                let refused = Row::Refused(ScopeReason::TextOnly(custom.clone()));
                assert_eq!(o.following, refused);
                let all = if c.rule.is_some() {
                    Row::Offered
                } else {
                    refused
                };
                assert_eq!(o.all, all);
            }
        }
    }

    /// The rule the series already has, sent back, is no change of rule: "This event" stays offered.
    #[test]
    fn the_same_rule_is_no_rule_change() {
        use crate::calendar_recur::wire::DayCode;
        let same = change(EventPatchDraft {
            summary: Some("Retro".into()),
            ..repeat(&[DayCode::Mon])
        });
        assert_eq!(same.rule, None);
        assert!(same.fields);
        assert_eq!(scope_options(&same, false, None).this, Row::Offered);
    }

    /// C13: a colour or notifications are per occurrence in Google, so all three rows are offered,
    /// as for a title; a value the occurrence already holds changes nothing, and one Google
    /// wouldn't take is said before any row.
    #[test]
    fn a_colour_or_notification_change_offers_all_three_rows() {
        use crate::calendar_write::dto::{ColourDraft, RemindersDraft};
        let colour = change(EventPatchDraft {
            colour: Some(ColourDraft::Colour { id: "3".into() }),
            ..Default::default()
        });
        let notifications = change(EventPatchDraft {
            reminders: Some(RemindersDraft {
                use_default: false,
                overrides: Vec::new(),
            }),
            ..Default::default()
        });
        for c in [colour, notifications] {
            assert_eq!(
                c,
                Change {
                    fields: true,
                    ..Default::default()
                }
            );
            let o = scope_options(&c, false, None);
            assert_eq!(
                (o.this, o.following, o.all),
                (Row::Offered, Row::Offered, Row::Offered)
            );
        }
        // The occurrence uses the defaults already.
        let same = change(EventPatchDraft {
            reminders: Some(RemindersDraft {
                use_default: true,
                overrides: Vec::new(),
            }),
            ..Default::default()
        });
        assert_eq!(same, Change::default());
        let m = Master::read(&fresh(master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]))).unwrap();
        let unknown = EventPatchDraft {
            colour: Some(ColourDraft::Colour { id: "12".into() }),
            ..Default::default()
        };
        assert!(Change::of(&unknown, &fresh(plain("2026-10-12T09:00:00+01:00")), &m).is_err());
    }
}
