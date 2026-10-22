// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The repeat menu's ready-made rules and a rule's words, for the event editor (#884, C11). Pure: no
//! state, no I/O, no window check (they write nothing), so the webview needs no rrule package and PM
//! says a rule one way everywhere.
//!
//! Both are `async`, so they run off the main thread: the editor asks on every change, with no
//! debounce. Each asks the rrule crate for one occurrence at most, and a rule that has none at all is
//! refused before the crate is asked (C11 critique 1 L5), so neither takes more than a moment.

use crate::calendar_recur::expand::sync_start;
use crate::calendar_recur::spec::{describe, presets, SpecError};
use crate::calendar_recur::wire::{
    parse_date, series_start, RepeatPreset, RepeatRule, RepeatStart, RepeatView,
};

/// The menu's ready-made rules for an event starting on `day` (`YYYY-MM-DD`), as Google offers them.
/// An unreadable day gives none; the menu then has only "Does not repeat" and "Custom…".
#[tauri::command]
pub async fn recurrence_presets(day: String) -> Vec<RepeatPreset> {
    parse_date(&day)
        .map(presets)
        .unwrap_or_default()
        .into_iter()
        .map(|preset| RepeatPreset {
            rule: RepeatRule::from_spec(&preset.spec),
            label: preset.label,
        })
        .collect()
}

/// `rule` in words for an event starting at `start`, and the day its first occurrence falls on:
/// "Weekly on Monday and Wednesday" chosen on a Tuesday starts on the Wednesday (A27). A rule outside
/// the limits, or a first occurrence the clocks skip the start or end time of, is refused in words.
#[tauri::command]
pub async fn describe_recurrence(rule: RepeatRule, start: RepeatStart) -> RepeatView {
    let refused = |message: String| RepeatView::Refused { message };
    let asked = match series_start(&start) {
        Ok(asked) => asked,
        Err(e) => return refused(e.to_string()),
    };
    let spec = match rule.to_spec() {
        Ok(spec) => spec,
        Err(e) => return refused(e.to_string()),
    };
    let synced = match sync_start(&spec, &asked) {
        Ok(synced) => synced,
        Err(e) => return refused(e.to_string()),
    };
    let moved = (synced.date() - asked.date()).num_days();
    // The save moves the end with the start, so a first occurrence moved onto the night the clocks
    // skip its end time would be refused there: say so here (C11 critique 2 L7).
    if let RepeatStart::Timed { end: Some(end), .. } = &start {
        if let Some(day) = end.skipped_after(moved) {
            return refused(SpecError::EndGap(day).to_string());
        }
    }
    RepeatView::Ready {
        summary: describe(&spec, &synced),
        starts: synced.date().format("%Y-%m-%d").to_string(),
        moved: moved != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_recur::wire::{DayCode, RepeatEnds, RepeatPattern, WallTime};

    fn timed(date: &str, time: &str) -> RepeatStart {
        RepeatStart::Timed {
            date: date.into(),
            time: time.into(),
            zone: "Europe/London".into(),
            end: None,
        }
    }

    fn all_day(date: &str) -> RepeatStart {
        RepeatStart::AllDay { date: date.into() }
    }

    fn weekly(days: &[DayCode], ends: RepeatEnds) -> RepeatRule {
        RepeatRule {
            pattern: RepeatPattern::Weekly {
                days: days.to_vec(),
            },
            interval: 1,
            ends,
        }
    }

    fn daily(ends: RepeatEnds) -> RepeatRule {
        RepeatRule {
            pattern: RepeatPattern::Daily,
            interval: 1,
            ends,
        }
    }

    fn ready(summary: &str, starts: &str, moved: bool) -> RepeatView {
        RepeatView::Ready {
            summary: summary.into(),
            starts: starts.into(),
            moved,
        }
    }

    fn refused(message: impl ToString) -> RepeatView {
        RepeatView::Refused {
            message: message.to_string(),
        }
    }

    /// T3's five, in the menu's order, each with the rule it stands for.
    #[tokio::test]
    async fn presets_for_a_day_come_back_labelled() {
        let presets = recurrence_presets("2026-10-13".into()).await;
        let labels: Vec<&str> = presets.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Daily",
                "Weekly on Tuesday",
                "Monthly on the second Tuesday",
                "Annually on 13 October",
                "Every weekday (Monday to Friday)",
            ]
        );
        assert!(presets[1].rule == weekly(&[DayCode::Tue], RepeatEnds::Never));
        assert!(presets
            .iter()
            .all(|p| p.rule.interval == 1 && p.rule.ends == RepeatEnds::Never));
    }

    #[tokio::test]
    async fn an_unreadable_day_gives_no_presets() {
        for day in ["13-10-2026", "", "2026-02-30", "tomorrow"] {
            assert!(recurrence_presets(day.into()).await.is_empty(), "{day}");
        }
    }

    /// A27: a rule whose first day isn't the one typed says where it starts.
    #[tokio::test]
    async fn a_moved_start_is_said() {
        let view = describe_recurrence(
            weekly(&[DayCode::Mon, DayCode::Wed], RepeatEnds::Never),
            timed("2026-10-13", "09:00"),
        )
        .await;
        assert!(view == ready("Weekly on Monday and Wednesday", "2026-10-14", true));
        let weekdays = [
            DayCode::Mon,
            DayCode::Tue,
            DayCode::Wed,
            DayCode::Thu,
            DayCode::Fri,
        ];
        let view =
            describe_recurrence(weekly(&weekdays, RepeatEnds::Never), all_day("2026-10-31")).await;
        assert!(view == ready("Every weekday (Monday to Friday)", "2026-11-02", true));
        // A rule the typed day already fits doesn't move it.
        let view =
            describe_recurrence(daily(RepeatEnds::Never), timed("2026-10-13", "09:00")).await;
        assert!(view == ready("Daily", "2026-10-13", false));
    }

    /// London skips 01:00-02:00 on Sunday 29-03-2026: a Sunday rule chosen at 01:30 on the
    /// Saturday would start then.
    #[tokio::test]
    async fn a_gap_is_refused_in_words() {
        let view = describe_recurrence(
            weekly(&[DayCode::Sun], RepeatEnds::Never),
            timed("2026-03-28", "01:30"),
        )
        .await;
        assert!(
            view == refused(
                "The first occurrence would be on 29-03-2026, when the clocks skip that time. \
                 Pick a time outside that hour."
            ),
            "the skipped day in words"
        );
    }

    /// C11 critique 2 L7: an end the move puts in the skipped hour is refused too, and only then.
    #[tokio::test]
    async fn an_end_the_clocks_skip_is_refused() {
        let with_end = |time: &str| RepeatStart::Timed {
            date: "2026-03-27".into(),
            time: "23:00".into(),
            zone: "Europe/London".into(),
            end: Some(WallTime {
                date: "2026-03-28".into(),
                time: time.into(),
                zone: "Europe/London".into(),
            }),
        };
        // Friday 23:00 to Saturday 01:30, made to repeat on Saturdays: it starts on the Saturday and
        // would end at 01:30 on Sunday 29-03, which London skips.
        let saturdays = weekly(&[DayCode::Sat], RepeatEnds::Never);
        let view = describe_recurrence(saturdays.clone(), with_end("01:30")).await;
        assert!(
            view == refused(SpecError::EndGap(
                chrono::NaiveDate::from_ymd_opt(2026, 3, 29).unwrap()
            )),
            "the end's skipped day in words"
        );
        let view = describe_recurrence(saturdays, with_end("02:30")).await;
        assert!(view == ready("Weekly on Saturday", "2026-03-28", true));
    }

    #[tokio::test]
    async fn a_rule_past_730_is_refused() {
        let view = describe_recurrence(
            daily(RepeatEnds::After { count: 731 }),
            timed("2026-10-13", "09:00"),
        )
        .await;
        assert!(view == refused("Google allows at most 730 occurrences of a repeating event."));
    }

    #[tokio::test]
    async fn seven_hundred_thirty_is_accepted() {
        let view = describe_recurrence(
            daily(RepeatEnds::After { count: 730 }),
            timed("2026-10-13", "09:00"),
        )
        .await;
        assert!(view == ready("Daily, 730 times", "2026-10-13", false));
    }

    #[tokio::test]
    async fn an_end_before_the_start_is_refused() {
        let view = describe_recurrence(
            daily(RepeatEnds::OnDate {
                date: "2026-10-12".into(),
            }),
            timed("2026-10-13", "09:00"),
        )
        .await;
        assert!(view == refused("The end date is before the first occurrence."));
        // One PM can't read is said so, never guessed.
        let view = describe_recurrence(
            daily(RepeatEnds::OnDate {
                date: "12-10-2026".into(),
            }),
            timed("2026-10-13", "09:00"),
        )
        .await;
        assert!(view == refused("PM couldn't read that end date."));
    }

    #[tokio::test]
    async fn an_end_on_the_start_day_is_accepted() {
        let view = describe_recurrence(
            daily(RepeatEnds::OnDate {
                date: "2026-10-13".into(),
            }),
            timed("2026-10-13", "09:00"),
        )
        .await;
        assert!(view == ready("Daily, until 13-10-2026", "2026-10-13", false));
    }

    /// C11 critique 1 L5: "on day 31" of every twelfth April never happens, and is refused at once
    /// rather than looked for; a start PM can't read is said so.
    #[tokio::test]
    async fn a_rule_with_no_occurrence_or_an_unread_start_is_refused() {
        let never = RepeatRule {
            pattern: RepeatPattern::MonthlyDay { day: 31 },
            interval: 12,
            ends: RepeatEnds::After { count: 5 },
        };
        let view = describe_recurrence(never, timed("2026-04-13", "09:00")).await;
        assert!(view == refused(SpecError::NoOccurrence));
        let view = describe_recurrence(daily(RepeatEnds::Never), timed("2026-04-13", "9am")).await;
        assert!(view == refused("PM couldn't read that time."));
    }

    /// The summary is `describe`'s words for every T2 rule, from the fixture both languages read.
    #[tokio::test]
    async fn the_words_are_describes() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../src/lib/calendarEdit/repeatRules.fixture.json"
        ))
        .unwrap();
        let start: RepeatStart = serde_json::from_value(fixture["start"].clone()).unwrap();
        let asked = series_start(&start).unwrap();
        let rows = fixture["rows"].as_array().unwrap();
        assert!(!rows.is_empty());
        for row in rows {
            let rule: RepeatRule = serde_json::from_value(row["spec"].clone()).unwrap();
            let spec = rule.to_spec().unwrap();
            let synced = sync_start(&spec, &asked).unwrap();
            let words = describe(&spec, &synced);
            assert_eq!(row["words"].as_str(), Some(words.as_str()));
            let view = describe_recurrence(rule, start.clone()).await;
            assert!(
                matches!(&view, RepeatView::Ready { summary, .. } if *summary == words),
                "{words}"
            );
        }
    }
}
