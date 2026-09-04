//! [`Journal`]: a fixed-capacity ring of the most recently recorded [`Event`]s.
//!
//! Tracking `head` and a `len` count, rather than `head` and `tail`, is the same trick the
//! capability table and the IPC message queue use: `len` says directly whether the ring is empty
//! or full, with no spare slot sacrificed to keep the two tellable apart when the indices would
//! otherwise coincide.
//!
//! Unlike those two, this ring never refuses a write. A full capability table can hand `insert`
//! back `OutOfSlots` and a full mailbox can make its sender wait, because both have a caller on
//! the other end able to do something about it. An audit event has no such caller to push back
//! on — the entire point is to record what an agent did, or was refused, whether or not there
//! happens to be room. So [`Journal::record`] always succeeds: once full, it overwrites the
//! oldest slot and counts the eviction, rather than dropping the new event or silently keeping
//! the stale one over it. See the crate docs for why counting the eviction is what keeps that
//! unconditional acceptance trustworthy.

use crate::action::Action;
use crate::event::Event;

/// Fills unused ring slots so the backing array can be value-initialized with no allocator and no
/// per-slot `Option`. Never observed as data: every read goes through `get`/`iter`, which only
/// ever look at the `len` slots starting at `head`.
const EMPTY_EVENT: Event = Event {
    seq: 0,
    actor: 0,
    action: Action::Insert,
    kind: 0,
    rights: 0,
    detail: 0,
};

/// A ring of the most recent `N` audit events. See the module docs for why it evicts instead of
/// refusing, and the crate docs for why that eviction is always counted.
pub struct Journal<const N: usize> {
    events: [Event; N],
    /// Index of the oldest retained event.
    head: usize,
    /// How many events are currently retained (never more than `N`).
    len: usize,
    /// The sequence number the next `record` will assign. Starts at 1, since 0 means "no event".
    next_seq: u64,
    /// Total events evicted over this journal's lifetime. Never decreases, and never resets when
    /// the ring is replaced by a fresh one — a caller that wants a clean count starts a new
    /// journal.
    dropped: u64,
}

impl<const N: usize> Journal<N> {
    /// An empty journal with room for `N` events.
    pub const fn new() -> Self {
        Journal {
            events: [EMPTY_EVENT; N],
            head: 0,
            len: 0,
            next_seq: 1,
            dropped: 0,
        }
    }

    /// How many events this journal can hold at once.
    pub const fn capacity(&self) -> usize {
        N
    }

    /// How many events are currently retained.
    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// How many events have been evicted to make room for a newer one, over this journal's
    /// entire lifetime.
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The most recently assigned sequence number, or `0` if nothing has been recorded yet.
    pub const fn latest_seq(&self) -> u64 {
        if self.len == 0 { 0 } else { self.next_seq - 1 }
    }

    /// Records a new event and returns the sequence number assigned to it.
    ///
    /// Always succeeds — see the module docs. When the ring is already full this evicts the
    /// oldest retained event and counts it in [`Journal::dropped`].
    pub fn record(
        &mut self,
        actor: u64,
        action: Action,
        kind: u8,
        rights: u32,
        detail: u64,
    ) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        let event = Event {
            seq,
            actor,
            action,
            kind,
            rights,
            detail,
        };

        if self.len == N {
            // Full: the slot a fresh write would land on is the oldest one, so writing there and
            // then advancing `head` past it is eviction and insertion in one step.
            self.events[self.head] = event;
            self.head = (self.head + 1) % N;
            self.dropped += 1;
        } else {
            let tail = (self.head + self.len) % N;
            self.events[tail] = event;
            self.len += 1;
        }
        seq
    }

    /// The event at `index`, where `0` is the oldest retained event and `len() - 1` the newest.
    /// `None` if `index >= len()`.
    ///
    /// Indexed rather than cursor-based because the reader is across a privilege boundary and
    /// cannot hold a borrow into this journal between calls.
    pub fn get(&self, index: usize) -> Option<Event> {
        if index >= self.len {
            return None;
        }
        Some(self.events[(self.head + index) % N])
    }

    /// All retained events, oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = Event> + '_ {
        (0..self.len).map(move |i| self.events[(self.head + i) % N])
    }

    /// Every retained event with `seq` strictly greater than `seq`, oldest to newest.
    ///
    /// How a reader catches up without re-reading: remember the highest `seq` you last saw and
    /// ask for everything after it. If `seq` names an event this journal has already evicted, the
    /// answer is simply everything currently retained — compare [`Journal::dropped`] against what
    /// you last saw to notice that a gap happened.
    pub fn since(&self, seq: u64) -> impl Iterator<Item = Event> + '_ {
        self.iter().filter(move |event| event.seq > seq)
    }
}

impl<const N: usize> Default for Journal<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_numbers_start_at_one_and_strictly_increase() {
        // Capacity large enough that nothing gets evicted, so `get(0)` also confirms the first
        // sequence number assigned — not just the first one returned by `record`.
        let mut journal: Journal<64> = Journal::new();
        let first = journal.record(0, Action::Insert, 1, 0, 0);
        assert_eq!(first, 1);

        let mut previous = first;
        for i in 1..50u64 {
            let seq = journal.record(i, Action::Insert, 1, 0, 0);
            assert!(seq > previous, "sequence numbers must strictly increase");
            previous = seq;
        }
        assert_eq!(journal.get(0).unwrap().seq, 1);
    }

    #[test]
    fn filling_past_capacity_evicts_the_oldest() {
        const N: usize = 4;
        let mut journal: Journal<N> = Journal::new();
        for i in 0..(N as u64 + 3) {
            journal.record(i, Action::Insert, 1, 0, 0);
        }

        assert_eq!(journal.get(0).unwrap().seq, 4);
        assert_eq!(journal.len(), N);
        assert_eq!(journal.capacity(), N);
        assert_eq!(journal.dropped(), 3);
    }

    #[test]
    fn sequence_numbers_keep_climbing_after_many_wraps() {
        const N: usize = 4;
        let mut journal: Journal<N> = Journal::new();
        for i in 0..(3 * N) as u64 {
            journal.record(i, Action::Insert, 1, 0, 0);
        }

        assert_eq!(journal.latest_seq(), 3 * N as u64);
        assert_eq!(journal.dropped(), (2 * N) as u64);
    }

    #[test]
    fn since_returns_exactly_the_newer_events_in_order_and_nothing_past_the_latest() {
        let mut journal: Journal<8> = Journal::new();
        for i in 0..6u64 {
            journal.record(i, Action::Insert, 1, 0, 0);
        }
        // Sequence numbers are 1..=6; ask for everything after 3.
        let expected = [4u64, 5, 6];
        assert_eq!(journal.since(3).count(), expected.len());
        for (event, &want) in journal.since(3).zip(expected.iter()) {
            assert_eq!(event.seq, want);
        }

        assert_eq!(journal.since(journal.latest_seq()).count(), 0);
    }

    #[test]
    fn since_of_an_already_evicted_seq_returns_everything_retained() {
        const N: usize = 4;
        let mut journal: Journal<N> = Journal::new();
        for i in 0..(N as u64 + 3) {
            journal.record(i, Action::Insert, 1, 0, 0);
        }
        // Sequence numbers 1..=3 were evicted (dropped() == 3); asking since a seq at or before
        // that point can only mean "everything currently retained".
        assert_eq!(journal.since(2).count(), journal.len());
        assert!(journal.dropped() > 0);
    }

    #[test]
    fn get_past_len_is_none_and_the_journal_stays_usable() {
        let mut journal: Journal<4> = Journal::new();
        journal.record(1, Action::Insert, 1, 0, 0);

        assert_eq!(journal.get(1), None);
        assert_eq!(journal.get(100), None);

        let seq = journal.record(2, Action::Insert, 1, 0, 0);
        assert_eq!(journal.get(1).unwrap().seq, seq);
    }
}
