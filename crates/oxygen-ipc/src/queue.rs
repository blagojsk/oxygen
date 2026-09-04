//! Where a [`Message`] waits between send and receive.
//!
//! Backed by a fixed-capacity array, `MessageQueue<const N: usize>`, for the same reason the
//! capability table and the frame allocator are: at the point a task's mailbox exists there may
//! be no allocator to grow it from, and `N` sized to the endpoint up front is memory the target
//! hardware can afford in a way an unbounded one is not.
//!
//! ## A genuine ring, not a shifting array
//!
//! A queue that shifts every remaining element down on `pop` is O(n) per receive — fine for a
//! handful of messages, ruinous for a busy endpoint under load, since the cost grows with however
//! much is still queued behind the one just taken. This is a real ring: `head` tracks the next
//! slot to read, and the next slot to write is derived from `head` and how many messages are
//! currently queued, both wrapping via `% N`. Every `push` and `pop` touches exactly one slot,
//! regardless of how full the queue is.
//!
//! Tracking a count alongside `head` — rather than a second `tail` index — sidesteps the usual
//! ring-buffer ambiguity of telling "empty" from "full" when the two indices coincide: `count`
//! says which one it is directly, with no spare slot sacrificed to break the tie.

use crate::IpcError;
use crate::message::{Header, MAX_PAYLOAD, Message};

/// Fills unused ring slots so the backing array can be value-initialized with no allocator and no
/// per-slot `Option`. Never observed as data: `pop` only ever reads the `count` slots starting at
/// `head`, and this occupies every other one.
const EMPTY_SLOT: Message = Message {
    header: Header {
        interface: 0,
        method: 0,
        len: 0,
    },
    payload: [0u8; MAX_PAYLOAD],
};

/// A fixed-capacity FIFO of messages. See the module docs for why it is a genuine ring buffer.
pub struct MessageQueue<const N: usize> {
    slots: [Message; N],
    /// Index of the next message `pop` will return.
    head: usize,
    /// How many messages are queued. The next `push` writes to `(head + count) % N`.
    count: usize,
}

impl<const N: usize> MessageQueue<N> {
    /// An empty queue with room for `N` messages.
    pub const fn new() -> Self {
        MessageQueue {
            slots: [EMPTY_SLOT; N],
            head: 0,
            count: 0,
        }
    }

    /// How many messages this queue can hold at once.
    pub const fn capacity(&self) -> usize {
        N
    }

    /// How many messages are currently queued.
    pub const fn len(&self) -> usize {
        self.count
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub const fn is_full(&self) -> bool {
        self.count == N
    }

    /// Enqueues a message. Fails `QueueFull` without disturbing anything already queued.
    pub fn push(&mut self, message: Message) -> Result<(), IpcError> {
        if self.is_full() {
            return Err(IpcError::QueueFull);
        }
        let tail = (self.head + self.count) % N;
        self.slots[tail] = message;
        self.count += 1;
        Ok(())
    }

    /// Dequeues the oldest message, or `None` if the queue is empty. A queue that has just
    /// returned `None` is still usable — the next `push` succeeds exactly as it would have.
    pub fn pop(&mut self) -> Option<Message> {
        if self.is_empty() {
            return None;
        }
        let message = self.slots[self.head];
        self.head = (self.head + 1) % N;
        self.count -= 1;
        Some(message)
    }
}

impl<const N: usize> Default for MessageQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(method: u32) -> Message {
        Message::new(1, method, &[]).unwrap()
    }

    #[test]
    fn fifo_order_is_preserved() {
        let mut q: MessageQueue<4> = MessageQueue::new();
        q.push(msg(1)).unwrap();
        q.push(msg(2)).unwrap();
        q.push(msg(3)).unwrap();

        assert_eq!(q.pop().unwrap().header.method, 1);
        assert_eq!(q.pop().unwrap().header.method, 2);
        assert_eq!(q.pop().unwrap().header.method, 3);
    }

    #[test]
    fn wraps_correctly_across_multiple_fill_drain_cycles() {
        let mut q: MessageQueue<3> = MessageQueue::new();

        // First cycle: fill, drain completely. This walks head and the write cursor off the end
        // of the backing array, which is exactly the arithmetic that breaks if head/tail wrap is
        // wrong.
        for i in 0..3 {
            q.push(msg(i)).unwrap();
        }
        for i in 0..3 {
            assert_eq!(q.pop().unwrap().header.method, i);
        }

        // Second cycle, from a wrapped `head`: fill and drain again with different values, so a
        // stale slot from the first cycle would show up as a wrong method number.
        for i in 10..13 {
            q.push(msg(i)).unwrap();
        }
        for i in 10..13 {
            assert_eq!(q.pop().unwrap().header.method, i);
        }
    }

    #[test]
    fn push_to_full_queue_is_refused_and_preserves_contents() {
        let mut q: MessageQueue<2> = MessageQueue::new();
        q.push(msg(1)).unwrap();
        q.push(msg(2)).unwrap();

        assert!(q.is_full());
        assert_eq!(q.push(msg(3)), Err(IpcError::QueueFull));

        // Nothing already queued was disturbed by the rejected push.
        assert_eq!(q.pop().unwrap().header.method, 1);
        assert_eq!(q.pop().unwrap().header.method, 2);
    }

    #[test]
    fn pop_on_empty_queue_is_none_and_queue_stays_usable() {
        let mut q: MessageQueue<2> = MessageQueue::new();
        assert_eq!(q.pop(), None);
        assert!(q.is_empty());

        q.push(msg(1)).unwrap();
        assert_eq!(q.pop().unwrap().header.method, 1);
    }

    #[test]
    fn len_capacity_and_full_report_correctly() {
        let mut q: MessageQueue<2> = MessageQueue::new();
        assert_eq!(q.capacity(), 2);
        assert_eq!(q.len(), 0);
        assert!(q.is_empty());

        q.push(msg(1)).unwrap();
        assert_eq!(q.len(), 1);
        assert!(!q.is_full());

        q.push(msg(2)).unwrap();
        assert_eq!(q.len(), 2);
        assert!(q.is_full());
    }
}
