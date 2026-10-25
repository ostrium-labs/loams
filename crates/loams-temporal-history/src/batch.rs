//! One commit's worth of new history events.

use prost::Message as _;

use loams_temporal_proto::temporal::api::sdk::v1::UserMetadata;

use crate::time::ts;
use crate::{Attributes, HistoryEvent, MutableState, event_type};

/// The events one state change appends, with contiguous IDs from the run's
/// `next_event_id` (§13.2 invariant 1). Pushing an event advances the
/// state's `next_event_id`, `last_task_id` and `history_size_bytes`, so the
/// state and the events always agree; the caller commits both or neither.
#[derive(Debug)]
pub struct Batch<'a> {
    state: &'a mut MutableState,
    now_ms: i64,
    events: Vec<HistoryEvent>,
}

impl<'a> Batch<'a> {
    /// A batch on `state` whose events are stamped `now_ms` unless pushed
    /// with [`Batch::push_at`].
    pub fn new(state: &'a mut MutableState, now_ms: i64) -> Self {
        Batch {
            state,
            now_ms,
            events: Vec::new(),
        }
    }

    /// The batch's time.
    pub fn now_ms(&self) -> i64 {
        self.now_ms
    }

    /// The run's state as the batch has advanced it.
    pub fn state(&mut self) -> &mut MutableState {
        self.state
    }

    /// The run's state, read only.
    pub fn state_ref(&self) -> &MutableState {
        self.state
    }

    /// Appends an event stamped with the batch's time; returns its ID.
    pub fn push(&mut self, attributes: Attributes) -> i64 {
        self.push_at(self.now_ms, attributes)
    }

    /// The ID the next pushed event will get.
    pub fn next_event_id(&self) -> i64 {
        self.state.next_event_id
    }

    /// Appends an event stamped `time_ms` (an activity's started event
    /// carries the time it started, though it is written with its outcome).
    pub fn push_at(&mut self, time_ms: i64, attributes: Attributes) -> i64 {
        let event_id = self.state.next_event_id;
        self.state.next_event_id += 1;
        self.state.last_task_id += 1;
        let event = HistoryEvent {
            event_id,
            event_time: Some(ts(time_ms)),
            event_type: event_type(&attributes) as i32,
            task_id: self.state.last_task_id,
            attributes: Some(attributes),
            ..Default::default()
        };
        self.state.history_size_bytes += event.encoded_len() as i64;
        self.events.push(event);
        event_id
    }

    /// Appends an event stamped with the batch's time that carries the
    /// command's user metadata (a summary the SDK attached); returns its ID.
    pub fn push_with_metadata(
        &mut self,
        user_metadata: Option<UserMetadata>,
        attributes: Attributes,
    ) -> i64 {
        let id = self.push(attributes);
        if user_metadata.is_some() {
            let e = self.events.last_mut().expect("just pushed");
            self.state.history_size_bytes -= e.encoded_len() as i64;
            e.user_metadata = user_metadata;
            self.state.history_size_bytes += e.encoded_len() as i64;
        }
        id
    }

    /// Appends an event that was buffered while a workflow task ran: it
    /// keeps its attributes and arrival time and gets the next ID.
    pub fn push_buffered(&mut self, mut event: HistoryEvent) -> i64 {
        let time_ms = event
            .event_time
            .as_ref()
            .map_or(self.now_ms, crate::time::ms);
        let attributes = event
            .attributes
            .take()
            .expect("buffered events always carry attributes");
        self.push_at(time_ms, attributes)
    }

    /// The events pushed so far.
    pub fn events(&self) -> &[HistoryEvent] {
        &self.events
    }

    /// The first event's ID, if any event was pushed.
    pub fn first_event_id(&self) -> Option<i64> {
        self.events.first().map(|e| e.event_id)
    }

    /// The pushed events, for the commit.
    pub fn finish(self) -> Vec<HistoryEvent> {
        self.events
    }
}
