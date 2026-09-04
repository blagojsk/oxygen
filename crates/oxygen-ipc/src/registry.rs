//! The name-to-object table: how a program asks for "console" instead of being handed a number
//! out of band.
//!
//! A number passed out of band — baked into a config file, or agreed by convention between two
//! programs — is authority nobody else can see or check. A name looked up in a shared registry is
//! discoverable: any holder of the registry can ask what exists, which is the same property the
//! capability system already gives handles, extended to the names that resolve to them.
//! [`Registry::iter`] is the point of the type as much as [`Registry::lookup`] is: a program can
//! enumerate what is offered rather than needing to already know.

use crate::IpcError;

/// Longest name a service can register under, in bytes.
pub const MAX_NAME: usize = 16;

/// A validated, fixed-width name.
///
/// Stored zero-padded to [`MAX_NAME`] rather than as a `&str` with a separate length, so two
/// `Name`s built from the same string are byte-identical and comparable with a derived `Eq` —
/// no custom comparison that has to remember to ignore trailing padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Name([u8; MAX_NAME]);

impl Name {
    /// Validates and encodes `name`.
    ///
    /// Rejects empty, longer than [`MAX_NAME`], or containing any byte outside printable ASCII
    /// (`0x21..=0x7E`) as `InvalidName`. The restriction is deliberately tight: this surface is
    /// read by both a program and the human debugging it, and a name that can hide a control byte
    /// or a space is one that cannot be printed back to that human unambiguously — a log line or
    /// a listing would have to guess where the name ends or what it "really" contains.
    pub fn new(name: &str) -> Result<Name, IpcError> {
        let bytes = name.as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_NAME {
            return Err(IpcError::InvalidName);
        }
        if !bytes.iter().all(|&b| (0x21..=0x7E).contains(&b)) {
            return Err(IpcError::InvalidName);
        }
        let mut buf = [0u8; MAX_NAME];
        buf[..bytes.len()].copy_from_slice(bytes);
        Ok(Name(buf))
    }

    /// The name with its zero padding trimmed off.
    pub fn as_str(&self) -> &str {
        let len = self.0.iter().position(|&b| b == 0).unwrap_or(MAX_NAME);
        // `new` is the only constructor and it accepts nothing outside 0x21..=0x7E, which is a
        // subset of ASCII and therefore always valid UTF-8, so this cannot fail.
        core::str::from_utf8(&self.0[..len]).expect("Name holds only validated printable ASCII")
    }
}

/// The name table itself: `N` slots, fixed at compile time, so a program can hold one without an
/// allocator.
pub struct Registry<const N: usize> {
    entries: [Option<(Name, u64)>; N],
    live: usize,
}

impl<const N: usize> Registry<N> {
    /// An empty registry with room for `N` names.
    pub const fn new() -> Self {
        Registry {
            entries: [None; N],
            live: 0,
        }
    }

    /// How many names are currently registered.
    pub const fn len(&self) -> usize {
        self.live
    }

    pub const fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Binds `name` to `value` — typically an object index or a raw handle the lookup side turns
    /// back into one.
    ///
    /// Fails `NameTaken` if `name` is already bound, without disturbing the existing binding, and
    /// `RegistryFull` if every slot is occupied by something else.
    pub fn register(&mut self, name: Name, value: u64) -> Result<(), IpcError> {
        if self.lookup(&name).is_some() {
            return Err(IpcError::NameTaken);
        }
        let index = self
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(IpcError::RegistryFull)?;
        self.entries[index] = Some((name, value));
        self.live += 1;
        Ok(())
    }

    /// Resolves `name` to its bound value, or `None` if nothing is registered under it.
    pub fn lookup(&self, name: &Name) -> Option<u64> {
        self.entries
            .iter()
            .flatten()
            .find_map(|(n, v)| (n == name).then_some(*v))
    }

    /// Removes `name`'s binding and returns the value it was bound to, freeing the slot for
    /// reuse. `None` if `name` was not registered.
    pub fn unregister(&mut self, name: &Name) -> Option<u64> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|(n, _)| n == name))?;
        let (_, value) = self.entries[index].take()?;
        self.live -= 1;
        Some(value)
    }

    /// Every live binding, as its trimmed name and value.
    ///
    /// What makes the registry *enumerable* rather than just a lookup table: a program can
    /// discover what is offered without already knowing the name to ask for. See the module
    /// docs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, u64)> + '_ {
        self.entries.iter().flatten().map(|(n, v)| (n.as_str(), *v))
    }
}

impl<const N: usize> Default for Registry<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_lookup_returns_the_value_absent_name_misses() {
        let mut reg: Registry<4> = Registry::new();
        let console = Name::new("console").unwrap();
        reg.register(console, 42).unwrap();

        assert_eq!(reg.lookup(&console), Some(42));
        assert_eq!(reg.lookup(&Name::new("shell").unwrap()), None);
    }

    #[test]
    fn duplicate_name_is_refused_and_keeps_the_existing_binding() {
        let mut reg: Registry<4> = Registry::new();
        let console = Name::new("console").unwrap();
        reg.register(console, 1).unwrap();

        assert_eq!(reg.register(console, 2), Err(IpcError::NameTaken));
        assert_eq!(reg.lookup(&console), Some(1));
    }

    #[test]
    fn unregister_returns_the_value_and_frees_the_slot() {
        let mut reg: Registry<1> = Registry::new();
        let console = Name::new("console").unwrap();
        reg.register(console, 7).unwrap();

        assert_eq!(reg.unregister(&console), Some(7));
        assert_eq!(reg.lookup(&console), None);
        assert_eq!(reg.unregister(&console), None);

        // The freed slot is reusable, not permanently lost.
        reg.register(Name::new("shell").unwrap(), 8).unwrap();
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn registering_past_capacity_is_refused() {
        let mut reg: Registry<1> = Registry::new();
        reg.register(Name::new("a").unwrap(), 1).unwrap();

        assert_eq!(
            reg.register(Name::new("b").unwrap(), 2),
            Err(IpcError::RegistryFull)
        );
    }

    #[test]
    fn invalid_names_are_rejected() {
        assert_eq!(Name::new(""), Err(IpcError::InvalidName));
        assert_eq!(Name::new("has space"), Err(IpcError::InvalidName));
        assert_eq!(Name::new("bad\tbyte"), Err(IpcError::InvalidName));
        assert_eq!(
            Name::new("this-name-is-far-too-long"),
            Err(IpcError::InvalidName)
        );

        // Exactly MAX_NAME bytes is the accepted boundary; one more is not. Built from byte
        // arrays rather than `String::repeat` so the test needs no allocator either.
        let exactly_max = [b'a'; MAX_NAME];
        assert!(Name::new(core::str::from_utf8(&exactly_max).unwrap()).is_ok());
        let one_over = [b'a'; MAX_NAME + 1];
        assert_eq!(
            Name::new(core::str::from_utf8(&one_over).unwrap()),
            Err(IpcError::InvalidName)
        );
    }

    #[test]
    fn iter_yields_exactly_the_live_entries() {
        let mut reg: Registry<4> = Registry::new();
        reg.register(Name::new("console").unwrap(), 1).unwrap();
        reg.register(Name::new("shell").unwrap(), 2).unwrap();
        reg.register(Name::new("timer").unwrap(), 3).unwrap();
        reg.unregister(&Name::new("shell").unwrap()).unwrap();

        let mut seen: [(&str, u64); 4] = [("", 0); 4];
        let mut count = 0;
        for entry in reg.iter() {
            seen[count] = entry;
            count += 1;
        }
        assert_eq!(count, 2, "unregistered entry must not appear");

        let has = |name: &str, value: u64| seen[..count].contains(&(name, value));
        assert!(has("console", 1));
        assert!(has("timer", 3));
        assert!(!seen[..count].iter().any(|&(name, _)| name == "shell"));
    }
}
