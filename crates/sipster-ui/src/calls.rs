//! Call history window: state, messages and rendering.
//!
//! Shows history and nothing else. Local-history and call-blocking preferences
//! used to be configured from a modal in here; they are settings, so they live
//! in Settings › Integrations with the rest.

use iced::widget::{column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length, Padding};
use sipster_core::BlockAction;
use sipster_integrations::{normalize_number, number_contains, CallRecord, CallType};

use crate::ui;

/// A name or number as it should appear, masked in streaming mode.
fn show(value: &str, mask: bool) -> String {
    if mask {
        sipster_core::mask_identity(value)
    } else {
        value.to_string()
    }
}

/// Which direction of call the list is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Incoming,
    Outgoing,
    Missed,
    /// Calls that were rejected because the caller is on the block list.
    Blocked,
}

impl Filter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::Incoming,
        Self::Outgoing,
        Self::Missed,
        Self::Blocked,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Incoming => "Incoming",
            Self::Outgoing => "Outgoing",
            Self::Missed => "Missed",
            Self::Blocked => "Blocked",
        }
    }

    fn accepts(self, call: &CallRecord) -> bool {
        match self {
            Self::All => true,
            Self::Incoming => matches!(call.call_type, CallType::Incoming),
            Self::Outgoing => matches!(call.call_type, CallType::Outgoing),
            // Genuinely missed, i.e. rang and went unanswered. Rejected calls
            // have their own filter now, so lumping them in here would double
            // count them.
            Self::Missed => matches!(call.call_type, CallType::Missed),
            Self::Blocked => matches!(call.call_type, CallType::Rejected),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    SearchChanged(String),
    FilterChanged(Filter),
    SyncPressed,
    /// One batch from one source, appended as it arrives.
    CallsBatch(Vec<CallRecord>),
    SyncFinished,
    /// Selects a record, or clears the selection when it is already current.
    Select(String),
    DialNumber(String),
    AddContact(String, Option<String>),
    ClearHistoryPressed,

    BlockNumberPrompt(String, Option<String>),
    ConfirmBlockNumber(String, Option<String>, BlockAction),
    CancelBlockPrompt,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub search: String,
    pub filter: Filter,
    pub calls: Vec<CallRecord>,
    pub loading: bool,
    pub selected: Option<String>,
    pub block_prompt: Option<(String, Option<String>)>,
    /// Newest missed call the user has already looked at; anything newer is
    /// what the Missed badge counts. Mirrored from the config so `view` can
    /// read it without reaching for the whole configuration.
    pub missed_seen_until: Option<String>,
}

impl State {
    pub fn toggle(&mut self, id: &str) {
        if self.selected.as_deref() == Some(id) {
            self.selected = None;
        } else {
            self.selected = Some(id.to_string());
        }
    }

    /// Merges an incoming batch, keeping the list de-duplicated and newest
    /// first so it stays coherent while sources are still arriving.
    pub fn merge(&mut self, batch: Vec<CallRecord>) {
        self.calls.extend(batch);
        self.calls.sort_by(|a, b| {
            sipster_integrations::timestamp_key(&b.timestamp)
                .cmp(&sipster_integrations::timestamp_key(&a.timestamp))
                .then_with(|| a.remote_number.cmp(&b.remote_number))
        });
        self.calls
            .dedup_by(|a, b| a.timestamp == b.timestamp && a.remote_number == b.remote_number);
    }

    fn matching(&self) -> Vec<&CallRecord> {
        let needle = self.search.trim().to_lowercase();
        let digits = normalize_number(&needle);

        self.calls
            .iter()
            .filter(|call| self.filter.accepts(call))
            .filter(|call| {
                if needle.is_empty() {
                    return true;
                }
                if call
                    .remote_name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&needle))
                {
                    return true;
                }
                !digits.is_empty() && number_contains(&call.remote_number, &digits)
            })
            .collect()
    }

    /// Unseen missed calls, for the badge on the Missed chip.
    ///
    /// Only what arrived after the user last opened the Missed filter, so the
    /// badge clears when it is acted on and comes back when something new
    /// lands. Comparison goes through `timestamp_key` because the sources
    /// disagree on format — the router sends `DD.MM.YY`, local records are
    /// ISO-8601, and comparing those as plain strings orders them wrongly.
    fn missed_count(&self) -> usize {
        let seen = self
            .missed_seen_until
            .as_deref()
            .map(sipster_integrations::timestamp_key);
        self.calls
            .iter()
            .filter(|c| matches!(c.call_type, CallType::Missed))
            .filter(|c| {
                seen.as_ref().is_none_or(|seen| {
                    &sipster_integrations::timestamp_key(&c.timestamp) > seen
                })
            })
            .count()
    }

    /// The newest missed call in the list, whether or not it has been seen.
    ///
    /// Acknowledging the badge records this, so calls that arrive later still
    /// count as unseen.
    pub fn newest_missed(&self) -> Option<&str> {
        self.calls
            .iter()
            .filter(|c| matches!(c.call_type, CallType::Missed))
            .max_by_key(|c| sipster_integrations::timestamp_key(&c.timestamp))
            .map(|c| c.timestamp.as_str())
    }
}

pub fn view(state: &State, mask: bool) -> Element<'_, Message> {
    if let Some((number, name)) = &state.block_prompt {
        return block_prompt(number, name.as_deref());
    }

    let matching = state.matching();

    let toolbar = ui::toolbar(
        "History",
        format!("{} of {} calls", matching.len(), state.calls.len()),
        vec![
            ui::tool_button(
                "Clear",
                (!state.calls.is_empty()).then_some(Message::ClearHistoryPressed),
            ),
            ui::tool_button(
                if state.loading { "Syncing…" } else { "Sync" },
                (!state.loading).then_some(Message::SyncPressed),
            ),
        ],
    );

    let missed = state.missed_count();
    let mut chips = row![].spacing(4).align_y(Alignment::Center);
    for filter in Filter::ALL {
        // The missed count is the one number worth surfacing without a click.
        let label = if matches!(filter, Filter::Missed) && missed > 0 {
            format!("Missed ({missed})")
        } else {
            filter.label().to_string()
        };
        chips = chips.push(ui::chip_owned(
            label,
            state.filter == filter,
            Message::FilterChanged(filter),
        ));
    }

    let search = text_input("Search by name or number", &state.search)
        .on_input(Message::SearchChanged)
        .padding(8)
        .size(14);

    let body: Element<'_, Message> = if state.loading && state.calls.is_empty() {
        ui::empty_state("Syncing…", "Reading local history and the router call list.")
    } else if state.calls.is_empty() {
        ui::empty_state(
            "No calls yet",
            "Calls you place and receive appear here once history is enabled.",
        )
    } else if matching.is_empty() {
        ui::empty_state("No matches", "Nothing here matches that search or filter.")
    } else {
        let mut list = column![].width(Length::Fill);
        for (index, call) in matching.iter().enumerate() {
            if index > 0 {
                list = list.push(ui::separator());
            }
            list = list.push(call_row(call, state.selected.as_deref(), mask));
        }
        scrollable(list).height(Length::Fill).into()
    };

    let content = column![toolbar, chips, search, body]
        .spacing(10)
        .padding(Padding::new(16.0));

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn call_row<'a>(
    call: &'a CallRecord,
    selected: Option<&str>,
    mask: bool,
) -> Element<'a, Message> {
    let is_selected = selected == Some(call.id.as_str());

    // Typographic arrows rather than emoji, so weight and baseline match the
    // surrounding text on any system font.
    let (marker, marker_color) = match call.call_type {
        CallType::Incoming => ("↙", iced::Color::from_rgb(0.35, 0.75, 0.45)),
        CallType::Outgoing => ("↗", iced::Color::from_rgb(0.45, 0.6, 0.9)),
        CallType::Missed => ("↙", iced::Color::from_rgb(0.92, 0.35, 0.35)),
        CallType::Rejected => ("⊘", iced::Color::from_rgb(0.75, 0.55, 0.3)),
    };

    let title = show(
        call.remote_name.as_deref().unwrap_or(&call.remote_number),
        mask,
    );

    let mut detail = call.timestamp.clone();
    if call.duration_seconds > 0 {
        detail.push_str("  ·  ");
        detail.push_str(&format_duration(call.duration_seconds));
    }
    if call.remote_name.is_some() {
        detail.push_str("  ·  ");
        detail.push_str(&show(&call.remote_number, mask));
    }

    let summary = row![
        text(marker).size(15).color(marker_color).width(Length::Fixed(18.0)),
        column![text(title).size(14), ui::caption(detail)].spacing(1),
        Space::new().width(Length::Fill),
    ]
    .align_y(Alignment::Center)
    .spacing(8)
    .into();

    let expanded = is_selected.then(|| {
        row![
            ui::row_action("Call back", Message::DialNumber(call.remote_number.clone())),
            ui::row_action(
                "Add contact",
                Message::AddContact(call.remote_number.clone(), call.remote_name.clone()),
            ),
            ui::row_action_danger(
                "Block",
                Message::BlockNumberPrompt(call.remote_number.clone(), call.remote_name.clone()),
            ),
            Space::new().width(Length::Fill),
            ui::caption(call.source.to_string()),
        ]
        .align_y(Alignment::Center)
        .spacing(6)
        .padding(Padding::from([4, 0]))
        .into()
    });

    ui::list_row(summary, expanded, is_selected, Message::Select(call.id.clone()))
}

/// `0:42`, `12:05`, `1:02:03`.
fn format_duration(seconds: u32) -> String {
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn block_prompt<'a>(number: &'a str, name: Option<&'a str>) -> Element<'a, Message> {
    let who = name.map_or_else(|| number.to_string(), |n| format!("{n} ({number})"));

    let content = column![
        text("Block this caller?").size(18),
        ui::caption(who),
        Space::new().height(10),
        text("Reject answers with SIP 603 immediately. Mute lets it ring silently, with no notification.")
            .size(12),
        Space::new().height(12),
        row![
            ui::tool_button("Cancel", Some(Message::CancelBlockPrompt)),
            Space::new().width(Length::Fill),
            ui::tool_button(
                "Mute",
                Some(Message::ConfirmBlockNumber(
                    number.to_string(),
                    name.map(str::to_string),
                    BlockAction::Mute,
                )),
            ),
            ui::tool_button(
                "Reject",
                Some(Message::ConfirmBlockNumber(
                    number.to_string(),
                    name.map(str::to_string),
                    BlockAction::Reject,
                )),
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(4)
    .padding(Padding::new(20.0))
    .max_width(420);

    container(content)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::{format_duration, Filter, State};
    use sipster_integrations::{CallRecord, CallType, RecordSource};

    fn record(id: &str, call_type: CallType, number: &str, name: Option<&str>) -> CallRecord {
        CallRecord {
            id: id.into(),
            call_type,
            remote_number: number.into(),
            remote_name: name.map(str::to_string),
            local_party: None,
            timestamp: "2026-09-04T10:00:00Z".into(),
            duration_seconds: 42,
            source: RecordSource::Local,
        }
    }

    fn state() -> State {
        State {
            calls: vec![
                record("1", CallType::Incoming, "+49301234567", Some("Alice")),
                record("2", CallType::Outgoing, "**610", None),
                record("3", CallType::Missed, "+49309999999", None),
                record("4", CallType::Rejected, "+49308888888", None),
            ],
            ..State::default()
        }
    }

    #[test]
    fn the_all_filter_shows_everything() {
        assert_eq!(state().matching().len(), 4);
    }

    #[test]
    fn filters_by_direction() {
        let mut s = state();
        s.filter = Filter::Incoming;
        assert_eq!(s.matching().len(), 1);
        s.filter = Filter::Outgoing;
        assert_eq!(s.matching().len(), 1);
    }

    /// Missed means rang-and-unanswered. Blocked calls have their own filter,
    /// so counting them here too would show each one twice.
    #[test]
    fn missed_and_blocked_are_separate() {
        let mut s = state();
        s.filter = Filter::Missed;
        assert_eq!(s.matching().len(), 1, "only the genuinely missed call");

        s.filter = Filter::Blocked;
        assert_eq!(s.matching().len(), 1, "only the rejected call");
    }

    /// Every record must be reachable from some filter; a call that appears
    /// under none is a call the user cannot find.
    #[test]
    fn every_record_is_reachable_from_some_filter() {
        let s = state();
        for call in &s.calls {
            assert!(
                Filter::ALL
                    .iter()
                    .any(|f| *f != Filter::All && f.accepts(call)),
                "{:?} is not covered by any filter",
                call.call_type
            );
        }
    }

    /// The badge counts genuinely missed calls, not rejected ones the user
    /// chose to block.
    #[test]
    fn the_missed_badge_excludes_rejected() {
        assert_eq!(state().missed_count(), 1);
    }

    #[test]
    fn search_and_filter_combine() {
        let mut s = state();
        s.filter = Filter::Incoming;
        s.search = "alice".into();
        assert_eq!(s.matching().len(), 1);
        s.search = "bob".into();
        assert!(s.matching().is_empty());
    }

    #[test]
    fn searches_numbers_written_with_separators() {
        let mut s = state();
        s.search = "030 1234".into();
        assert_eq!(s.matching().len(), 1);
    }

    #[test]
    fn durations_render_in_the_usual_shapes() {
        assert_eq!(format_duration(42), "0:42");
        assert_eq!(format_duration(725), "12:05");
        assert_eq!(format_duration(3723), "1:02:03");
        assert_eq!(format_duration(0), "0:00");
    }

    #[test]
    fn selecting_the_open_row_closes_it() {
        let mut s = state();
        s.toggle("1");
        assert_eq!(s.selected.as_deref(), Some("1"));
        s.toggle("1");
        assert_eq!(s.selected, None);
    }

    /// A record at a chosen time, so the seen-marker comparison has something
    /// to order.
    fn missed_at(id: &str, timestamp: &str) -> CallRecord {
        CallRecord {
            timestamp: timestamp.into(),
            ..record(id, CallType::Missed, "+49309999999", None)
        }
    }

    #[test]
    fn every_missed_call_counts_until_one_is_acknowledged() {
        let mut s = State {
            calls: vec![
                missed_at("a", "2026-09-01T08:00:00Z"),
                missed_at("b", "2026-09-02T09:00:00Z"),
            ],
            ..State::default()
        };
        assert_eq!(s.missed_count(), 2);

        // What clicking the Missed filter records.
        s.missed_seen_until = s.newest_missed().map(str::to_owned);
        assert_eq!(s.missed_count(), 0, "the badge clears once they are seen");
    }

    /// The point of the marker: a call arriving after the click is unread
    /// again, rather than staying hidden behind an already-cleared badge.
    #[test]
    fn a_call_arriving_after_the_click_is_unseen_again() {
        let mut s = State {
            calls: vec![missed_at("a", "2026-09-01T08:00:00Z")],
            ..State::default()
        };
        s.missed_seen_until = s.newest_missed().map(str::to_owned);
        assert_eq!(s.missed_count(), 0);

        s.merge(vec![missed_at("b", "2026-09-03T12:00:00Z")]);
        assert_eq!(s.missed_count(), 1);
    }

    /// The router sends `DD.MM.YY` and local records are ISO-8601. Compared as
    /// plain strings, "05.09.26" sorts below any ISO timestamp, so an older
    /// router call would look newer than the marker and never clear.
    #[test]
    fn the_two_timestamp_formats_are_compared_by_instant_not_by_text() {
        let mut s = State {
            calls: vec![
                missed_at("router", "01.09.26 08:00"),
                missed_at("local", "2026-09-02T09:00:00Z"),
            ],
            ..State::default()
        };
        assert_eq!(s.missed_count(), 2);
        s.missed_seen_until = s.newest_missed().map(str::to_owned);
        assert_eq!(s.missed_count(), 0, "both formats must land before the marker");
    }

    /// Clicking Missed with nothing missed must not invent a marker.
    #[test]
    fn an_empty_history_has_nothing_to_acknowledge() {
        let s = State::default();
        assert_eq!(s.newest_missed(), None);
        assert_eq!(s.missed_count(), 0);
    }
}
