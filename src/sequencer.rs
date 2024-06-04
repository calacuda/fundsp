//! The sequencer unit mixes together scheduled audio units with sample accurate timing.

use super::audiounit::*;
use super::buffer::*;
use super::math::*;
use super::realseq::*;
use super::signal::*;
use super::*;
use duplicate::duplicate_item;
use std::cmp::{Eq, Ord, Ordering};
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
use thingbuf::mpsc::blocking::{channel, Receiver, Sender};

/// Fade curves.
#[derive(Clone, Default)]
pub enum Fade {
    /// Equal power fade. Results in equal power mixing
    /// when fade out of one event coincides with the fade in of another.
    Power,
    /// Smooth polynomial fade.
    #[default]
    Smooth,
}

impl Fade {
    /// Evaluate fade curve at `x` (0.0 <= `x` <= 1.0).
    #[inline]
    pub fn at<T: Float>(&self, x: T) -> T {
        match self {
            Fade::Power => sine_ease(x),
            Fade::Smooth => smooth5(x),
        }
    }
}

/// Globally unique ID for a sequencer event.
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub struct EventId(u64);

/// This atomic supplies globally unique IDs.
static GLOBAL_EVENT_ID: AtomicU32 = AtomicU32::new(0);

impl EventId {
    /// Create a new, globally unique event ID.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        EventId(GLOBAL_EVENT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[duplicate_item(
    f48       Event48       AudioUnit48;
    [ f64 ]   [ Event64 ]   [ AudioUnit64 ];
    [ f32 ]   [ Event32 ]   [ AudioUnit32 ];
)]
#[derive(Clone)]
pub struct Event48 {
    pub unit: Box<dyn AudioUnit48>,
    pub start_time: f48,
    pub end_time: f48,
    pub fade_ease: Fade,
    pub fade_in: f48,
    pub fade_out: f48,
    pub id: EventId,
}

#[duplicate_item(
    f48       Event48       AudioUnit48;
    [ f64 ]   [ Event64 ]   [ AudioUnit64 ];
    [ f32 ]   [ Event32 ]   [ AudioUnit32 ];
)]
impl Event48 {
    pub fn new(
        unit: Box<dyn AudioUnit48>,
        start_time: f48,
        end_time: f48,
        fade_ease: Fade,
        fade_in: f48,
        fade_out: f48,
    ) -> Self {
        Self {
            unit,
            start_time,
            end_time,
            fade_ease,
            fade_in,
            fade_out,
            id: EventId::new(),
        }
    }
}

#[duplicate_item(
    f48       Event48       AudioUnit48;
    [ f64 ]   [ Event64 ]   [ AudioUnit64 ];
    [ f32 ]   [ Event32 ]   [ AudioUnit32 ];
)]
impl PartialEq for Event48 {
    fn eq(&self, other: &Event48) -> bool {
        self.start_time == other.start_time
    }
}

impl Eq for Event32 {}
impl Eq for Event64 {}

#[duplicate_item(
    f48       Event48       AudioUnit48;
    [ f64 ]   [ Event64 ]   [ AudioUnit64 ];
    [ f32 ]   [ Event32 ]   [ AudioUnit32 ];
)]
impl PartialOrd for Event48 {
    fn partial_cmp(&self, other: &Event48) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[duplicate_item(
    f48       Event48       AudioUnit48;
    [ f64 ]   [ Event64 ]   [ AudioUnit64 ];
    [ f32 ]   [ Event32 ]   [ AudioUnit32 ];
)]
impl Ord for Event48 {
    fn cmp(&self, other: &Self) -> Ordering {
        other.start_time.total_cmp(&self.start_time)
    }
}

#[duplicate_item(
    f48       Edit48       AudioUnit48;
    [ f64 ]   [ Edit64 ]   [ AudioUnit64 ];
    [ f32 ]   [ Edit32 ]   [ AudioUnit32 ];
)]
#[derive(Clone)]
pub struct Edit48 {
    pub end_time: f48,
    pub fade_out: f48,
}

#[duplicate_item(
    f48       Event48       AudioUnit48       fade_in48;
    [ f64 ]   [ Event64 ]   [ AudioUnit64 ]   [ fade_in64 ];
    [ f32 ]   [ Event32 ]   [ AudioUnit32 ]   [ fade_in32 ];
)]
#[inline]
fn fade_in48(
    sample_duration: f48,
    time: f48,
    end_time: f48,
    start_index: usize,
    end_index: usize,
    ease: Fade,
    fade_duration: f48,
    fade_start_time: f48,
    output: &mut [&mut [f48]],
) {
    let fade_end_time = fade_start_time + fade_duration;
    if fade_duration > 0.0 && fade_end_time > time {
        let fade_end_i = if fade_end_time >= end_time {
            end_index
        } else {
            round((fade_end_time - time) / sample_duration) as usize
        };
        let fade_d = sample_duration / fade_duration;

        let fade_phase = delerp(
            fade_start_time,
            fade_end_time,
            time + start_index as f48 * sample_duration,
        );
        match ease {
            Fade::Power => {
                for channel in 0..output.len() {
                    let mut fade = fade_phase;
                    for x in output[channel][..fade_end_i].iter_mut() {
                        *x *= sine_ease(fade);
                        fade += fade_d;
                    }
                }
            }
            Fade::Smooth => {
                for channel in 0..output.len() {
                    let mut fade = fade_phase;
                    for x in output[channel][..fade_end_i].iter_mut() {
                        *x *= smooth5(fade);
                        fade += fade_d;
                    }
                }
            }
        }
    }
}

#[duplicate_item(
    f48       Event48       AudioUnit48       fade_out48;
    [ f64 ]   [ Event64 ]   [ AudioUnit64 ]   [ fade_out64 ];
    [ f32 ]   [ Event32 ]   [ AudioUnit32 ]   [ fade_out32 ];
)]
#[inline]
fn fade_out48(
    sample_duration: f48,
    time: f48,
    end_time: f48,
    _start_index: usize,
    end_index: usize,
    ease: Fade,
    fade_duration: f48,
    fade_end_time: f48,
    output: &mut [&mut [f48]],
) {
    let fade_start_time = fade_end_time - fade_duration;
    if fade_duration > 0.0 && fade_start_time < end_time {
        let fade_i = if fade_start_time <= time {
            0
        } else {
            round((fade_start_time - time) / sample_duration) as usize
        };
        let fade_d = sample_duration / fade_duration;
        let fade_phase = delerp(
            fade_start_time,
            fade_end_time,
            time + fade_i as f48 * sample_duration,
        );
        match ease {
            Fade::Power => {
                for channel in 0..output.len() {
                    let mut fade = fade_phase;
                    for x in output[channel][fade_i..end_index].iter_mut() {
                        *x *= sine_ease(1.0 - fade);
                        fade += fade_d;
                    }
                }
            }
            Fade::Smooth => {
                for channel in 0..output.len() {
                    let mut fade = fade_phase;
                    for x in output[channel][fade_i..end_index].iter_mut() {
                        *x *= smooth5(1.0 - fade);
                        fade += fade_d;
                    }
                }
            }
        }
    }
}

/// Sequencer unit.
/// The sequencer mixes together outputs of audio units with sample accurate timing.
#[duplicate_item(
    f48       Event48       AudioUnit48       Sequencer48       Message48        Edit48;
    [ f64 ]   [ Event64 ]   [ AudioUnit64 ]   [ Sequencer64 ]   [ Message64 ]    [ Edit64 ];
    [ f32 ]   [ Event32 ]   [ AudioUnit32 ]   [ Sequencer32 ]   [ Message32 ]    [ Edit32 ];
)]
pub struct Sequencer48 {
    /// Current events, unsorted.
    active: Vec<Event48>,
    /// IDs of current events.
    active_map: HashMap<EventId, usize>,
    /// Events that start before the active threshold are active.
    active_threshold: f48,
    /// Future events sorted by start time.
    ready: BinaryHeap<Event48>,
    /// Past events, unsorted.
    past: Vec<Event48>,
    /// Map of edits to be made to events in the ready queue.
    edit_map: HashMap<EventId, Edit48>,
    /// Number of output channels.
    outputs: usize,
    /// Current time. Does not apply to frontends.
    time: f48,
    /// Current sample rate.
    sample_rate: f48,
    /// Current sample duration.
    sample_duration: f48,
    /// Intermediate output buffer.
    buffer: Buffer<f48>,
    /// Intermediate output frame.
    tick_buffer: Vec<f48>,
    /// Optional frontend.
    front: Option<(Sender<Message48>, Receiver<Option<Event48>>)>,
    /// Whether we replay existing events after a call to `reset`.
    replay_events: bool,
}

#[duplicate_item(
    f48       Event48       AudioUnit48       Sequencer48       Message48;
    [ f64 ]   [ Event64 ]   [ AudioUnit64 ]   [ Sequencer64 ]   [ Message64 ];
    [ f32 ]   [ Event32 ]   [ AudioUnit32 ]   [ Sequencer32 ]   [ Message32 ];
)]
impl Clone for Sequencer48 {
    fn clone(&self) -> Self {
        if self.has_backend() {
            panic!("Frontends cannot be cloned.");
        }
        Self {
            active: self.active.clone(),
            active_map: self.active_map.clone(),
            active_threshold: self.active_threshold,
            ready: self.ready.clone(),
            past: self.past.clone(),
            edit_map: self.edit_map.clone(),
            outputs: self.outputs,
            time: self.time,
            sample_rate: self.sample_rate,
            sample_duration: self.sample_duration,
            buffer: self.buffer.clone(),
            tick_buffer: self.tick_buffer.clone(),
            front: None,
            replay_events: self.replay_events,
        }
    }
}

#[allow(clippy::unnecessary_cast)]
#[duplicate_item(
    f48       Event48       AudioUnit48       Sequencer48       SequencerBackend48       Message48       Edit48;
    [ f64 ]   [ Event64 ]   [ AudioUnit64 ]   [ Sequencer64 ]   [ SequencerBackend64 ]   [ Message64 ]   [ Edit64 ];
    [ f32 ]   [ Event32 ]   [ AudioUnit32 ]   [ Sequencer32 ]   [ SequencerBackend32 ]   [ Message32 ]   [ Edit32 ];
)]
impl Sequencer48 {
    /// Create a new sequencer. The sequencer has zero inputs.
    /// The number of outputs is decided by the user.
    /// If `replay_events` is true, then past events will be retained
    /// and played back after a reset.
    /// If false, then all events will be cleared on reset.
    pub fn new(replay_events: bool, outputs: usize) -> Self {
        Self {
            active: Vec::with_capacity(16384),
            active_map: HashMap::with_capacity(16384),
            active_threshold: -f48::INFINITY,
            ready: BinaryHeap::with_capacity(16384),
            past: Vec::with_capacity(16384),
            edit_map: HashMap::with_capacity(16384),
            outputs,
            time: 0.0,
            sample_rate: DEFAULT_SR as f48,
            sample_duration: 1.0 / DEFAULT_SR as f48,
            buffer: Buffer::with_channels(outputs),
            tick_buffer: vec![0.0; outputs],
            front: None,
            replay_events,
        }
    }

    /// Current time in seconds.
    /// This method is not applicable to frontends, which do not process audio.
    pub fn time(&self) -> f48 {
        self.time
    }

    /// Add an event. All times are specified in seconds.
    /// Fade in and fade out may overlap but may not exceed the duration of the event.
    /// Returns the ID of the event.
    pub fn push(
        &mut self,
        start_time: f48,
        end_time: f48,
        fade_ease: Fade,
        fade_in_time: f48,
        fade_out_time: f48,
        mut unit: Box<dyn AudioUnit48>,
    ) -> EventId {
        assert_eq!(unit.inputs(), 0);
        assert_eq!(unit.outputs(), self.outputs);
        let duration = end_time - start_time;
        assert!(fade_in_time <= duration && fade_out_time <= duration);
        // Make sure the sample rate of the unit matches ours.
        unit.set_sample_rate(self.sample_rate as f64);
        unit.allocate();
        let event = Event48::new(
            unit,
            start_time,
            end_time,
            fade_ease,
            fade_in_time,
            fade_out_time,
        );
        let id = event.id;
        self.push_event(event);
        id
    }

    /// Add event. This is an internal method.
    pub(crate) fn push_event(&mut self, event: Event48) {
        if let Some((sender, receiver)) = &mut self.front {
            // Deallocate all past events.
            while receiver.try_recv().is_ok() {}
            // Send the new event over.
            if sender.try_send(Message48::Push(event)).is_ok() {}
        } else if event.start_time < self.active_threshold {
            self.active_map.insert(event.id, self.active.len());
            self.active.push(event);
        } else {
            self.ready.push(event);
        }
    }

    /// Add an event. All times are specified in seconds.
    /// Start and end times are relative to current time.
    /// A start time of zero will start the event as soon as possible.
    /// Fade in and fade out may overlap but may not exceed the duration of the event.
    /// Returns the ID of the event.
    pub fn push_relative(
        &mut self,
        start_time: f48,
        end_time: f48,
        fade_ease: Fade,
        fade_in_time: f48,
        fade_out_time: f48,
        mut unit: Box<dyn AudioUnit48>,
    ) -> EventId {
        assert!(unit.inputs() == 0 && unit.outputs() == self.outputs);
        let duration = end_time - start_time;
        assert!(fade_in_time <= duration && fade_out_time <= duration);
        // Make sure the sample rate of the unit matches ours.
        unit.set_sample_rate(self.sample_rate as f64);
        unit.allocate();
        let event = Event48::new(
            unit,
            start_time,
            end_time,
            fade_ease,
            fade_in_time,
            fade_out_time,
        );
        let id = event.id;
        self.push_relative_event(event);
        id
    }

    /// Add relative event. This is an internal method.
    pub(crate) fn push_relative_event(&mut self, mut event: Event48) {
        if let Some((sender, receiver)) = &mut self.front {
            // Deallocate all past events.
            while receiver.try_recv().is_ok() {}
            // Send the new event over.
            if sender.try_send(Message48::PushRelative(event)).is_ok() {}
        } else {
            event.start_time += self.time;
            event.end_time += self.time;
            if event.start_time < self.active_threshold {
                self.active_map.insert(event.id, self.active.len());
                self.active.push(event);
            } else {
                self.ready.push(event);
            }
        }
    }

    /// Add an event using start time and duration.
    /// Fade in and fade out may overlap but may not exceed the duration of the event.
    /// Returns the ID of the event.
    pub fn push_duration(
        &mut self,
        start_time: f48,
        duration: f48,
        fade_ease: Fade,
        fade_in_time: f48,
        fade_out_time: f48,
        unit: Box<dyn AudioUnit48>,
    ) -> EventId {
        self.push(
            start_time,
            start_time + duration,
            fade_ease,
            fade_in_time,
            fade_out_time,
            unit,
        )
    }

    /// Make a change to an existing event. Only the end time and fade out time
    /// of the event may be changed. The new end time can only be used to shorten events.
    /// Edits are intended to be used with events where we do not know ahead of time
    /// how long they need to play. The original end time can be set to infinity,
    /// for example.
    pub fn edit(&mut self, id: EventId, end_time: f48, fade_out_time: f48) {
        if let Some((sender, receiver)) = &mut self.front {
            // Deallocate all past events.
            while receiver.try_recv().is_ok() {}
            // Send the new edit over.
            if sender
                .try_send(Message48::Edit(
                    id,
                    Edit48 {
                        end_time,
                        fade_out: fade_out_time,
                    },
                ))
                .is_ok()
            {}
        } else if self.active_map.contains_key(&id) {
            // The edit applies to an active event.
            let i = self.active_map[&id];
            self.active[i].end_time = end_time;
            self.active[i].fade_out = fade_out_time;
        } else if end_time < self.active_threshold {
            // The edit is already in the past.
        } else {
            // The edit is in the future.
            self.edit_map.insert(
                id,
                Edit48 {
                    end_time,
                    fade_out: fade_out_time,
                },
            );
        }
    }

    /// Make a change to an existing event. Only the end time and fade out time
    /// of the event may be changed. The new end time can only be used to shorten events.
    /// The end time is relative to current time.
    /// The event starts fading out immediately if end time is equal to fade out time.
    /// Edits are intended to be used with events where we do not know ahead of time
    /// how long they need to play. The original end time can be set to infinity,
    /// for example.
    pub fn edit_relative(&mut self, id: EventId, end_time: f48, fade_out_time: f48) {
        if let Some((sender, receiver)) = &mut self.front {
            // Deallocate all past events.
            while receiver.try_recv().is_ok() {}
            // Send the new edit over.
            if sender
                .try_send(Message48::EditRelative(
                    id,
                    Edit48 {
                        end_time,
                        fade_out: fade_out_time,
                    },
                ))
                .is_ok()
            {}
        } else if self.active_map.contains_key(&id) {
            // The edit applies to an active event.
            let i = self.active_map[&id];
            self.active[i].end_time = self.time + end_time;
            self.active[i].fade_out = fade_out_time;
        } else if self.time + end_time < self.active_threshold {
            // The edit is already in the past.
        } else {
            // The edit is in the future.
            self.edit_map.insert(
                id,
                Edit48 {
                    end_time: self.time + end_time,
                    fade_out: fade_out_time,
                },
            );
        }
    }

    /// Move units that start before the end time to the active set.
    fn ready_to_active(&mut self, next_end_time: f48) {
        self.active_threshold = next_end_time - self.sample_duration * 0.5;
        while let Some(ready) = self.ready.peek() {
            // Test whether start time rounded to a sample comes before the end time,
            // which always falls on a sample.
            if ready.start_time < self.active_threshold {
                if let Some(mut ready) = self.ready.pop() {
                    self.active_map.insert(ready.id, self.active.len());
                    // Check for edits to the event.
                    if self.edit_map.contains_key(&ready.id) {
                        let edit = &self.edit_map[&ready.id];
                        ready.fade_out = edit.fade_out;
                        ready.end_time = edit.end_time;
                        self.edit_map.remove(&ready.id);
                    }
                    self.active.push(ready);
                }
            } else {
                break;
            }
        }
    }

    /// Create a real-time friendly backend for this sequencer.
    /// This sequencer is then the frontend and any changes made are reflected in the backend.
    /// The backend renders audio while the frontend manages memory and
    /// communicates changes made to the backend.
    /// The backend is initialized with the current state of the sequencer.
    /// This can be called only once for a sequencer.
    pub fn backend(&mut self) -> SequencerBackend48 {
        assert!(!self.has_backend());
        // Create huge channel buffers to make sure we don't run out of space easily.
        let (sender_a, receiver_a) = channel(16384);
        let (sender_b, receiver_b) = channel(16384);
        let mut sequencer = self.clone();
        sequencer.allocate();
        self.front = Some((sender_a, receiver_b));
        SequencerBackend48::new(sender_b, receiver_a, sequencer)
    }

    /// Returns whether this sequencer has a backend.
    pub fn has_backend(&self) -> bool {
        self.front.is_some()
    }

    /// Returns whether we retain past events and replay them after a reset.
    pub fn replay_events(&self) -> bool {
        self.replay_events
    }

    /// Get past events. This is an internal method.
    pub(crate) fn get_past_event(&mut self) -> Option<Event48> {
        self.past.pop()
    }

    /// Get ready events. This is an internal method.
    pub(crate) fn get_ready_event(&mut self) -> Option<Event48> {
        self.ready.pop()
    }

    /// Get active events. This is an internal method.
    pub(crate) fn get_active_event(&mut self) -> Option<Event48> {
        if let Some(event) = self.active.pop() {
            self.active_map.remove(&event.id);
            return Some(event);
        }
        None
    }
}

#[allow(clippy::unnecessary_cast)]
#[duplicate_item(
    f48       Event48       AudioUnit48       Sequencer48      fade_in48      fade_out48;
    [ f64 ]   [ Event64 ]   [ AudioUnit64 ]   [ Sequencer64 ]  [ fade_in64 ]  [ fade_out64 ];
    [ f32 ]   [ Event32 ]   [ AudioUnit32 ]   [ Sequencer32 ]  [ fade_in32 ]  [ fade_out32 ];
)]
impl AudioUnit48 for Sequencer48 {
    fn reset(&mut self) {
        if self.replay_events {
            while let Some(ready) = self.ready.pop() {
                self.active.push(ready);
            }
            while let Some(past) = self.past.pop() {
                self.active.push(past);
            }
            for i in 0..self.active.len() {
                self.active[i].unit.reset();
            }
            while let Some(active) = self.active.pop() {
                self.ready.push(active);
            }
            self.active_map.clear();
        } else {
            while let Some(_ready) = self.ready.pop() {}
            while let Some(_past) = self.past.pop() {}
            while let Some(_active) = self.active.pop() {}
            self.edit_map.clear();
            self.active_map.clear();
        }
        self.time = 0.0;
        self.active_threshold = -f48::INFINITY;
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        let sample_rate = sample_rate as f48;
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.sample_duration = 1.0 / sample_rate;
            // Move everything to the active queue, then set sample rate and move
            // everything to the ready heap.
            while let Some(ready) = self.ready.pop() {
                self.active.push(ready);
            }
            while let Some(past) = self.past.pop() {
                self.active.push(past);
            }
            for i in 0..self.active.len() {
                self.active[i].unit.set_sample_rate(sample_rate as f64);
            }
            while let Some(active) = self.active.pop() {
                self.ready.push(active);
            }
            self.active_map.clear();
            self.active_threshold = -f48::INFINITY;
        }
    }

    #[inline]
    fn tick(&mut self, input: &[f48], output: &mut [f48]) {
        if !self.replay_events {
            while let Some(_past) = self.past.pop() {}
        }
        for channel in 0..self.outputs {
            output[channel] = 0.0;
        }
        let end_time = self.time + self.sample_duration;
        self.ready_to_active(end_time);
        let mut i = 0;
        while i < self.active.len() {
            if self.active[i].end_time <= self.time + 0.5 * self.sample_duration {
                self.active_map.remove(&self.active[i].id);
                if i + 1 < self.active.len() {
                    self.active_map
                        .insert(self.active[self.active.len() - 1].id, i);
                }
                self.past.push(self.active.swap_remove(i));
            } else {
                self.active[i].unit.tick(input, &mut self.tick_buffer);
                if self.active[i].fade_in > 0.0 {
                    let fade_in = delerp(
                        self.active[i].start_time,
                        self.active[i].start_time + self.active[i].fade_in,
                        self.time,
                    );
                    if fade_in < 1.0 {
                        match self.active[i].fade_ease {
                            Fade::Power => {
                                for channel in 0..self.outputs {
                                    self.tick_buffer[channel] *= sine_ease(fade_in);
                                }
                            }
                            Fade::Smooth => {
                                for channel in 0..self.outputs {
                                    self.tick_buffer[channel] *= smooth5(fade_in);
                                }
                            }
                        }
                    }
                }
                if self.active[i].fade_out > 0.0 {
                    let fade_out = delerp(
                        self.active[i].end_time - self.active[i].fade_out,
                        self.active[i].end_time,
                        self.time,
                    );
                    if fade_out > 0.0 {
                        match self.active[i].fade_ease {
                            Fade::Power => {
                                for channel in 0..self.outputs {
                                    self.tick_buffer[channel] *= sine_ease(1.0 - fade_out);
                                }
                            }
                            Fade::Smooth => {
                                for channel in 0..self.outputs {
                                    self.tick_buffer[channel] *= smooth5(1.0 - fade_out);
                                }
                            }
                        }
                    }
                }
                for channel in 0..self.outputs {
                    output[channel] += self.tick_buffer[channel];
                }
                i += 1;
            }
        }
        self.time = end_time;
    }

    fn process(&mut self, size: usize, input: &[&[f48]], output: &mut [&mut [f48]]) {
        if !self.replay_events {
            while let Some(_past) = self.past.pop() {}
        }
        for channel in 0..self.outputs {
            output[channel][..size].fill(0.0);
        }
        let end_time = self.time + self.sample_duration * size as f48;
        self.ready_to_active(end_time);
        let buffer_output = self.buffer.get_mut(self.outputs);
        let mut i = 0;
        while i < self.active.len() {
            if self.active[i].end_time <= self.time + 0.5 * self.sample_duration {
                self.active_map.remove(&self.active[i].id);
                if i + 1 < self.active.len() {
                    self.active_map
                        .insert(self.active[self.active.len() - 1].id, i);
                }
                self.past.push(self.active.swap_remove(i));
            } else {
                let start_index = if self.active[i].start_time <= self.time {
                    0
                } else {
                    round((self.active[i].start_time - self.time) * self.sample_rate) as usize
                };
                let end_index = if self.active[i].end_time >= end_time {
                    size
                } else {
                    round((self.active[i].end_time - self.time) * self.sample_rate) as usize
                };
                if end_index > start_index {
                    self.active[i]
                        .unit
                        .process(end_index - start_index, input, buffer_output);
                    fade_in48(
                        self.sample_duration,
                        self.time,
                        end_time,
                        start_index,
                        end_index,
                        self.active[i].fade_ease.clone(),
                        self.active[i].fade_in,
                        self.active[i].start_time,
                        buffer_output,
                    );
                    fade_out48(
                        self.sample_duration,
                        self.time,
                        end_time,
                        start_index,
                        end_index,
                        self.active[i].fade_ease.clone(),
                        self.active[i].fade_out,
                        self.active[i].end_time,
                        buffer_output,
                    );
                    for channel in 0..self.outputs {
                        for j in start_index..end_index {
                            output[channel][j] += buffer_output[channel][j - start_index];
                        }
                    }
                }
                i += 1;
            }
        }
        self.time = end_time;
    }

    fn get_id(&self) -> u64 {
        const ID: u64 = 64;
        ID
    }

    fn inputs(&self) -> usize {
        0
    }
    fn outputs(&self) -> usize {
        self.outputs
    }

    fn route(&mut self, _input: &SignalFrame, _frequency: f64) -> SignalFrame {
        // Treat the sequencer as a generator.
        let mut signal = new_signal_frame(AudioUnit48::outputs(self));
        for i in 0..AudioUnit48::outputs(self) {
            signal[i] = Signal::Latency(0.0);
        }
        signal
    }

    fn footprint(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
