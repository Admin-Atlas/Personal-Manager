// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Which of "This event", "This and following" and "All events" a change can offer (T10). This is the
//! shape of the choice only: whether a scope can actually be carried out (a move that would lose an
//! occurrence, an exception PM can't carry) is the planner's answer, and C9 runs the planner for each
//! row it offers when the dialog opens, so each row shows its own warnings or reason (DS12).

use std::fmt;

use serde_json::Value;

use crate::calendar_recur::lines::TextOnly;
use crate::calendar_recur::remap::RetimeError;
use crate::calendar_recur::{Point, RecurError};

use super::super::dto::{EventPatchDraft, SeriesScope, TimeDraft};
use super::super::patch::build_patch;
use super::super::time::{self, TimeError};
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
    /// Title, place, description or busy/free.
    pub fields: bool,
    pub visibility: bool,
    pub time: Option<TimeChange>,
    pub delete: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimeChange {
    /// Measured from where the occurrence came from, not where it is now (T7).
    pub date_moved: bool,
    pub length_changed: bool,
    /// Between timed and all-day.
    pub kind_changed: bool,
    /// The user picked another zone for the start.
    pub zone_changed: bool,
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
    ) -> Result<Change, TimeError> {
        let it = instance.json();
        let body = build_patch(it, draft)?;
        let mut change = Change {
            fields: ["summary", "location", "description", "transparency"]
                .iter()
                .any(|k| body.contains_key(*k)),
            visibility: body.contains_key("visibility"),
            ..Change::default()
        };
        if let (Some(draft_time), true) = (&draft.time, body.contains_key("start")) {
            let when = time::resolve_against(draft_time, it.get("start"), it.get("end"))?;
            change.time = Some(time_change(draft_time, &when.start, &when.end, it, master));
        }
        Ok(change)
    }
}

/// Whether a timed draft puts the occurrence's start or end in a zone other than its own (an end
/// with no zone of its own is in the start's): a whole series can't follow it until C11.
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

fn time_change(
    draft: &TimeDraft,
    start: &Value,
    end: &Value,
    instance: &Value,
    master: &Master,
) -> TimeChange {
    let (new_start, new_end) = (point_of(Some(start)), point_of(Some(end)));
    let zone = master.start.zone();
    let kind_changed = matches!(
        (master.length, new_start),
        (Length::Timed(_), Some(Point::Day(_))) | (Length::AllDay(_), Some(Point::At(_)))
    );
    let zone_changed = zones_changed(draft, instance);
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
        kind_changed,
        zone_changed,
    }
}

/// Why a row isn't offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeReason {
    /// Google applies an occurrence's visibility to the whole series (stricter) or ignores it.
    VisibilityIsSeriesWide,
    /// PM shows this series' rule as text only, so it can't move or cut it.
    TextOnly(TextOnly),
    /// Between timed and all-day, or into another zone, for more than one occurrence (until C11).
    KindChange,
    ZoneChange,
}

/// Shown on the row as is, in the planners' own words for the same refusals.
impl fmt::Display for ScopeReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScopeReason::VisibilityIsSeriesWide => SeriesRefusal::VisibilityIsSeriesWide.fmt(f),
            ScopeReason::TextOnly(why) => RecurError::TextOnly(why.clone()).fmt(f),
            ScopeReason::KindChange => RetimeError::KindChange.fmt(f),
            ScopeReason::ZoneChange => SeriesRefusal::ZoneChange.fmt(f),
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
    if let Some(t) = change.time {
        let series_wide = if t.kind_changed {
            Some(ScopeReason::KindChange)
        } else if t.zone_changed {
            Some(ScopeReason::ZoneChange)
        } else {
            text_only.cloned().map(ScopeReason::TextOnly)
        };
        if let Some(why) = series_wide {
            following = Row::Refused(why.clone());
            all = Row::Refused(why);
        }
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
                kind_changed: false,
                zone_changed: false
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
                kind_changed: false,
                zone_changed: false
            })
        );
        let elsewhere = change(EventPatchDraft {
            time: Some(timed("2026-10-12", "04:00", "05:00", "America/New_York")),
            ..Default::default()
        });
        assert!(elsewhere.time.unwrap().zone_changed);
        // C8b review G7: an end put in another zone is a zone change too.
        let end_elsewhere = change(EventPatchDraft {
            time: Some(TimeDraft::Timed {
                start_date: "2026-10-12".into(),
                start_time: "10:00".into(),
                start_zone: "Europe/London".into(),
                end_date: "2026-10-12".into(),
                end_time: "12:00".into(),
                end_zone: "Europe/Paris".into(),
            }),
            ..Default::default()
        });
        assert!(end_elsewhere.time.unwrap().zone_changed);
        let all_day = change(EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-12".into(),
                last_day: "2026-10-12".into(),
            }),
            ..Default::default()
        });
        assert!(all_day.time.unwrap().kind_changed);
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
        // Timed ↔ all-day, or another zone, for one occurrence only.
        for (t, why) in [
            (
                TimeChange {
                    kind_changed: true,
                    ..Default::default()
                },
                ScopeReason::KindChange,
            ),
            (
                TimeChange {
                    zone_changed: true,
                    ..Default::default()
                },
                ScopeReason::ZoneChange,
            ),
        ] {
            let o = scope_options(
                &Change {
                    time: Some(t),
                    ..Default::default()
                },
                false,
                None,
            );
            assert_eq!(o.this, Row::Offered);
            assert_eq!(o.following, Row::Refused(why.clone()));
            assert_eq!(o.all, Row::Refused(why));
        }
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
            "PM can't change how this series repeats: its rule uses BYSETPOS, which PM doesn't \
             change."
        );
        assert!(ScopeReason::KindChange
            .to_string()
            .contains("between all-day and timed"));
        assert!(ScopeReason::ZoneChange
            .to_string()
            .contains("another time zone"));
    }
}
