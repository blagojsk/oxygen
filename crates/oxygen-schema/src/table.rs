//! Every interface a program's schema knows about, keyed by id and enumerable.
//!
//! The schema-crate counterpart to [`oxygen_ipc::Registry`]: a fixed-capacity table with no
//! allocator, looked up by a stable key, and — just as important as the lookup — iterable, so a
//! holder can list what an object supports instead of needing to already know an id to ask
//! about. A table that only answered "does X exist" and never "what exists" would still be
//! typed, but it would not be discoverable, and discoverability is the property this crate
//! exists to add. See the crate's module docs.

use crate::{SchemaError, interface::Interface};

/// A fixed-capacity table of [`Interface`]s, `N` slots, no allocator — `N` is the caller's call
/// to make, the same as every other fixed-size collection in this workspace.
pub struct SchemaTable<const N: usize> {
    interfaces: [Option<Interface>; N],
    /// Occupied slot count, maintained incrementally rather than recomputed, the same discipline
    /// `oxygen_ipc::Registry` and `oxygen_cap::CapSpace` both follow.
    live: usize,
}

impl<const N: usize> SchemaTable<N> {
    /// An empty table with room for `N` interfaces.
    pub const fn new() -> Self {
        SchemaTable {
            interfaces: [None; N],
            live: 0,
        }
    }

    /// How many interfaces are currently registered.
    pub const fn len(&self) -> usize {
        self.live
    }

    pub const fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Adds `interface` under its own id.
    ///
    /// Fails `DuplicateInterface` if that id is already registered, leaving the existing entry
    /// untouched, and `TableFull` if every slot is occupied.
    pub fn register(&mut self, interface: Interface) -> Result<(), SchemaError> {
        if self.get(interface.id()).is_some() {
            return Err(SchemaError::DuplicateInterface);
        }
        let index = self
            .interfaces
            .iter()
            .position(Option::is_none)
            .ok_or(SchemaError::TableFull)?;
        self.interfaces[index] = Some(interface);
        self.live += 1;
        Ok(())
    }

    /// The interface registered under `id`, or `None` if nothing is.
    pub fn get(&self, id: u32) -> Option<&Interface> {
        self.interfaces
            .iter()
            .flatten()
            .find(|iface| iface.id() == id)
    }

    /// Every registered interface. What makes the table enumerable rather than just a lookup —
    /// the same property [`oxygen_ipc::Registry::iter`] gives service names. See the module docs.
    pub fn iter(&self) -> impl Iterator<Item = &Interface> + '_ {
        self.interfaces.iter().flatten()
    }
}

impl<const N: usize> Default for SchemaTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::method::{ArgKind, Method};
    use oxygen_ipc::Name;

    fn iface(id: u32, n: &str) -> Interface {
        Interface::new(id, Name::new(n).unwrap()).unwrap()
    }

    #[test]
    fn register_then_get_returns_the_interface_duplicate_id_is_refused() {
        let mut table: SchemaTable<4> = SchemaTable::new();
        table.register(iface(1, "console")).unwrap();

        assert_eq!(table.get(1).unwrap().name().as_str(), "console");
        assert_eq!(table.get(2), None);

        let impostor = iface(1, "impostor");
        assert_eq!(
            table.register(impostor),
            Err(SchemaError::DuplicateInterface)
        );
        assert_eq!(
            table.get(1).unwrap().name().as_str(),
            "console",
            "a duplicate registration must not replace the original"
        );
    }

    #[test]
    fn iter_yields_exactly_what_was_registered() {
        let mut table: SchemaTable<4> = SchemaTable::new();
        table.register(iface(1, "a")).unwrap();
        table.register(iface(2, "b")).unwrap();
        table.register(iface(3, "c")).unwrap();

        let mut seen: [u32; 4] = [0; 4];
        let mut count = 0;
        for i in table.iter() {
            seen[count] = i.id();
            count += 1;
        }
        assert_eq!(count, 3);
        assert!(seen[..count].contains(&1));
        assert!(seen[..count].contains(&2));
        assert!(seen[..count].contains(&3));
    }

    #[test]
    fn registering_past_capacity_is_refused() {
        let mut table: SchemaTable<1> = SchemaTable::new();
        table.register(iface(1, "a")).unwrap();

        assert_eq!(table.register(iface(2, "b")), Err(SchemaError::TableFull));
    }

    #[test]
    fn method_added_to_a_registered_interface_is_visible_through_the_table() {
        // Exercises the whole path end to end: build an interface, give it a method, register
        // it, and read the method back out through the table rather than the local variable.
        let mut svc = iface(1, "svc");
        svc.add(Method::new(5, Name::new("read").unwrap(), &[], ArgKind::None).unwrap())
            .unwrap();

        let mut table: SchemaTable<2> = SchemaTable::new();
        table.register(svc).unwrap();

        let methods = table.get(1).unwrap().methods();
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].id(), 5);
    }
}
