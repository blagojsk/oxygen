//! An interface: the set of [`Method`]s a capability's schema exposes under one stable id.
//!
//! An id rather than a bare name, for the same reason a message's header already carries an
//! interface number instead of a string (see `oxygen_ipc::message`): comparing and routing on a
//! `u32` costs nothing and never needs the caller and callee to agree byte-for-byte on spelling.
//! The [`Name`] travels alongside it for the audience that reads the id back as a person, not in
//! place of it.

use crate::{SchemaError, method::Method};
use oxygen_ipc::Name;

/// Most methods one [`Interface`] can declare.
pub const MAX_METHODS: usize = 8;

/// A capability's method table: an id and a name for the interface itself, plus up to
/// [`MAX_METHODS`] [`Method`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interface {
    id: u32,
    name: Name,
    methods: [Method; MAX_METHODS],
    method_count: u8,
}

impl Interface {
    /// An interface with no methods yet — call [`Interface::add`] to declare them.
    ///
    /// Refuses id `0` as `UntypedInterface`. [`oxygen_ipc`] already reserves interface `0` on a
    /// message as "untyped" — see `oxygen_ipc::message`'s module docs — so a schema claiming to
    /// describe that id would be documenting a capability that can never actually receive a call
    /// under it. Keeping the same reserved id in both crates means a caller hits one consistent
    /// story instead of the schema crate quietly permitting what the IPC crate refuses.
    pub fn new(id: u32, name: Name) -> Result<Interface, SchemaError> {
        if id == 0 {
            return Err(SchemaError::UntypedInterface);
        }
        Ok(Interface {
            id,
            name,
            methods: [Method::padding(); MAX_METHODS],
            method_count: 0,
        })
    }

    pub const fn id(&self) -> u32 {
        self.id
    }

    pub const fn name(&self) -> Name {
        self.name
    }

    /// Declares another method. Fails `TooManyMethods` past [`MAX_METHODS`], leaving whatever was
    /// already declared untouched.
    pub fn add(&mut self, method: Method) -> Result<(), SchemaError> {
        let index = self.method_count as usize;
        if index >= MAX_METHODS {
            return Err(SchemaError::TooManyMethods);
        }
        self.methods[index] = method;
        self.method_count += 1;
        Ok(())
    }

    /// The declared methods, in the order they were added — never the placeholder
    /// [`Interface::new`] fills the unused tail of the array with. See
    /// [`Method::padding`](crate::method::Method) for why that placeholder exists at all.
    pub fn methods(&self) -> &[Method] {
        &self.methods[..self.method_count as usize]
    }

    pub fn method(&self, index: usize) -> Option<&Method> {
        self.methods().get(index)
    }

    pub const fn len(&self) -> usize {
        self.method_count as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.method_count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::method::ArgKind;

    fn name(s: &str) -> Name {
        Name::new(s).unwrap()
    }

    #[test]
    fn interface_id_zero_is_refused() {
        assert_eq!(
            Interface::new(0, name("x")),
            Err(SchemaError::UntypedInterface)
        );
    }

    #[test]
    fn adding_past_max_methods_is_refused_and_keeps_the_earlier_methods() {
        let mut iface = Interface::new(1, name("svc")).unwrap();
        for i in 0..MAX_METHODS as u32 {
            let m = Method::new(i, name("m"), &[], ArgKind::None).unwrap();
            iface.add(m).unwrap();
        }
        assert_eq!(iface.len(), MAX_METHODS);

        let extra = Method::new(99, name("extra"), &[], ArgKind::None).unwrap();
        assert_eq!(iface.add(extra), Err(SchemaError::TooManyMethods));

        assert_eq!(
            iface.len(),
            MAX_METHODS,
            "a refused add must not disturb what was already there"
        );
        for i in 0..MAX_METHODS as u32 {
            assert_eq!(iface.method(i as usize).unwrap().id(), i);
        }
    }
}
