//! [`Action`]: what an audit event records having happened.

/// One of the operations the audit journal can record.
///
/// Encoded as a stable `u8` (see [`Action::as_u8`]) rather than relying on the compiler's own
/// enum layout, because an [`crate::Event`] crosses the syscall boundary into a separately
/// compiled userspace program. That numeric encoding becomes part of the contract between the
/// kernel and every program reading the journal, so it must not shift when a variant is added or
/// reordered here — new actions get the next unused number, never a renumbering of the existing
/// ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A fresh capability was inserted — the root grant that seeds a task's authority.
    Insert,
    /// A capability was delegated: a narrower child derived from one already held.
    Delegate,
    /// A capability's descendants were revoked, the capability itself left intact and live.
    Revoke,
    /// A capability was deleted, taking its whole subtree with it.
    Delete,
    /// An operation was refused for lack of the right it needed.
    Denied,
    /// A name was bound in the registry.
    Register,
    /// A name was resolved in the registry.
    Lookup,
}

impl Action {
    /// This action's wire value. See the type docs for why it is stable once assigned.
    pub const fn as_u8(self) -> u8 {
        match self {
            Action::Insert => 0,
            Action::Delegate => 1,
            Action::Revoke => 2,
            Action::Delete => 3,
            Action::Denied => 4,
            Action::Register => 5,
            Action::Lookup => 6,
        }
    }

    /// Recovers the `Action` a wire value names, or `None` if it names none — the case a
    /// corrupted or forward-incompatible encoding must be told apart from a real action.
    pub const fn from_u8(byte: u8) -> Option<Action> {
        match byte {
            0 => Some(Action::Insert),
            1 => Some(Action::Delegate),
            2 => Some(Action::Revoke),
            3 => Some(Action::Delete),
            4 => Some(Action::Denied),
            5 => Some(Action::Register),
            6 => Some(Action::Lookup),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_round_trips_through_its_wire_value() {
        let actions = [
            Action::Insert,
            Action::Delegate,
            Action::Revoke,
            Action::Delete,
            Action::Denied,
            Action::Register,
            Action::Lookup,
        ];
        for action in actions {
            assert_eq!(Action::from_u8(action.as_u8()), Some(action));
        }
    }

    #[test]
    fn an_unrecognised_byte_is_rejected() {
        assert_eq!(Action::from_u8(200), None);
    }
}
