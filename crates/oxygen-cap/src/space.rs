//! `CapSpace`: a task's flat table of capability slots, and the derivation tree that makes a
//! grant withdrawable.
//!
//! Backed by a fixed-capacity array (`CapSpace<const N: usize>`) rather than anything that grows,
//! for the same reason the frame allocator is a bitmap: at the point a task's capability table
//! exists there may be no allocator to grow it from, and a table sized to the task up front is
//! memory the target hardware can afford in a way an unbounded one is not. `N` is the caller's
//! call to make and justify, per the workspace rule on resident memory.
//!
//! ## Slots are linked by index, not by pointer
//!
//! Each `Slot` records its place in the derivation tree — `parent`, `first_child`, `next_sibling`
//! — as `u32` indices into the same array, never as references or raw pointers. Two reasons: a
//! `u32` link keeps a `Slot` at 16 bytes instead of 24, which matters once `N` is in the hundreds
//! and there is one table per task; and an index survives the table being moved or copied, where
//! a pointer would need fixing up. `Slot::NONE` (`u32::MAX`) is the sentinel for "no link" —
//! chosen because a real table can never hold that many slots, so it can never be confused with a
//! live index.
//!
//! ## Revocation without recursion
//!
//! `revoke` and `delete` walk a subtree with an explicit index stack sized to the table's own
//! capacity, never with recursion. A derivation tree's depth is bounded only by how many times a
//! capability gets re-delegated, and this runs on a 16 KiB kernel stack — a stack frame per level
//! of a tree whose depth a task ultimately controls is exactly the unbounded growth that stack
//! exists not to have.
//!
//! ## Why a generation mismatch is `Stale`, not `Bad`, even once the slot is empty again
//!
//! A freed slot is handed to the very next `insert`, so by the time a stale handle is checked the
//! slot behind it may already belong to someone else, or may still be sitting empty. Both mean
//! the same thing to the handle's holder — *the thing you had is gone* — so both report
//! `StaleHandle`. `BadHandle` is reserved for a handle that could never have named anything: an
//! out-of-range index, or one that happens to guess a never-allocated slot's starting generation
//! of zero.

use crate::{handle::Handle, object::Object, rights::Rights};

/// Why a capability-space operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    /// The table is full: no free slot for a new capability.
    OutOfSlots,
    /// The index names no slot a live handle could refer to — out of range, or one that has never
    /// been occupied by the generation this handle claims.
    BadHandle,
    /// The index is real, but the slot has since been freed and possibly reused: this handle no
    /// longer names what it did when it was issued.
    StaleHandle,
    /// The operation needs a right the source capability does not hold.
    InsufficientRights,
}

/// One entry in the table: what it refers to, what its holder may do, and where it sits in the
/// derivation tree. See the module docs for why the tree links are indices.
#[derive(Debug, Clone, Copy)]
struct Slot {
    object: Object,
    rights: Rights,
    /// Bumped, wrapping, every time this slot is freed. The mechanism that makes a stale handle
    /// detectable instead of silently resolving to whatever now occupies the same index.
    generation: u32,
    parent: u32,
    first_child: u32,
    next_sibling: u32,
}

impl Slot {
    /// Sentinel for "no link" in `parent`, `first_child` and `next_sibling`. See the module docs.
    const NONE: u32 = u32::MAX;

    const fn free() -> Self {
        Slot {
            object: Object::Null,
            rights: Rights::NONE,
            generation: 0,
            parent: Self::NONE,
            first_child: Self::NONE,
            next_sibling: Self::NONE,
        }
    }
}

/// A task's capability table: `N` slots, fixed at compile time.
pub struct CapSpace<const N: usize> {
    slots: [Slot; N],
    /// Occupied slot count, maintained incrementally by every path that occupies or frees a slot
    /// — the same discipline `FrameAllocator::used` follows, so a `count` query never walks the
    /// table.
    live: usize,
}

impl<const N: usize> CapSpace<N> {
    /// An empty table: every slot free, every generation at zero.
    pub const fn new() -> Self {
        CapSpace {
            slots: [Slot::free(); N],
            live: 0,
        }
    }

    /// Live (occupied) slot count.
    pub const fn count(&self) -> usize {
        self.live
    }

    /// Inserts a fresh root capability with no parent.
    ///
    /// What the kernel calls to seed a new task with its starting authority — a console handle,
    /// say — before that task exists to derive anything of its own.
    pub fn insert(&mut self, object: Object, rights: Rights) -> Result<Handle, CapError> {
        let index = self.find_free()?;
        let generation = self.occupy(index, object, rights, Slot::NONE);
        Ok(Handle::new(index as u32, generation))
    }

    /// Reads back what a handle currently names.
    ///
    /// Fails if it does not currently name anything live — see the module docs for the
    /// `BadHandle`/`StaleHandle` split.
    pub fn get(&self, handle: Handle) -> Result<(Object, Rights), CapError> {
        let slot = self.slot_for(handle)?;
        Ok((slot.object, slot.rights))
    }

    pub fn rights(&self, handle: Handle) -> Result<Rights, CapError> {
        self.slot_for(handle).map(|slot| slot.rights)
    }

    pub fn object(&self, handle: Handle) -> Result<Object, CapError> {
        self.slot_for(handle).map(|slot| slot.object)
    }

    /// Derives a child capability from `from`, within this same table.
    ///
    /// The child's rights are `from`'s rights narrowed by `rights` — never wider, so a holder can
    /// only ever hand out a subset of what it has. Requires `GRANT` on `from`.
    ///
    /// Granting across a *different* task's table is the kernel's job: it validates and narrows
    /// rights exactly as this does, then calls `insert_derived` on the target task's own
    /// `CapSpace`.
    pub fn delegate(&mut self, from: Handle, rights: Rights) -> Result<Handle, CapError> {
        let slot = self.slot_for(from)?;
        if !slot.rights.contains(Rights::GRANT) {
            return Err(CapError::InsufficientRights);
        }
        let narrowed = slot.rights.intersection(rights);
        let object = slot.object;
        let parent = from.index();
        self.insert_derived(object, narrowed, parent)
    }

    /// The structural primitive `delegate` builds on: links a new slot as a child of `parent`,
    /// with no rights check of its own.
    ///
    /// FUTURE: cross-space derivation — the child capability landing in a *different* task's
    /// `CapSpace` than its parent — is not implemented here. `parent` names a slot in *this*
    /// table only; a cross-space grant would need the kernel to also record which table the
    /// parent lives in, which this crate has no notion of. Until that exists, this is the
    /// single-space primitive, and the kernel is responsible for whatever bookkeeping a grant
    /// that crosses a task boundary needs on top of it.
    pub fn insert_derived(
        &mut self,
        object: Object,
        rights: Rights,
        parent: u32,
    ) -> Result<Handle, CapError> {
        let parent_occupied = self
            .slots
            .get(parent as usize)
            .is_some_and(|slot| slot.object != Object::Null);
        if !parent_occupied {
            return Err(CapError::BadHandle);
        }
        let index = self.find_free()?;
        let generation = self.occupy(index, object, rights, parent);
        self.link_child(parent, index as u32);
        Ok(Handle::new(index as u32, generation))
    }

    /// Frees every descendant of `handle`'s slot, depth-first, leaving the slot itself intact and
    /// live. Returns how many slots were freed.
    ///
    /// Requires `REVOKE` on the slot. This is the operation that makes a grant withdrawable: you
    /// revoke what you *gave*, not what you *hold* — that is `delete`.
    pub fn revoke(&mut self, handle: Handle) -> Result<usize, CapError> {
        let slot = self.slot_for(handle)?;
        if !slot.rights.contains(Rights::REVOKE) {
            return Err(CapError::InsufficientRights);
        }
        let index = handle.index();
        let freed = self.free_descendants(index);
        self.slots[index as usize].first_child = Slot::NONE;
        Ok(freed)
    }

    /// Frees the slot itself and every descendant. Returns the count including the slot.
    ///
    /// No right beyond a live handle is required: deleting discards the caller's *own* authority,
    /// not authority it gave to someone else, so holding the handle at all is the only
    /// permission this needs. A capability tree can never be left with a descendant holding
    /// authority its ancestor no longer has, which is why the subtree goes with it.
    pub fn delete(&mut self, handle: Handle) -> Result<usize, CapError> {
        self.slot_for(handle)?;
        let index = handle.index();
        let mut freed = self.free_descendants(index);
        self.unlink_from_parent(index);
        self.free_slot(index);
        freed += 1;
        Ok(freed)
    }

    // -- internals --

    fn find_free(&self) -> Result<usize, CapError> {
        self.slots
            .iter()
            .position(|slot| slot.object == Object::Null)
            .ok_or(CapError::OutOfSlots)
    }

    /// Validates a handle against the slot it names: index in range, and the generation still
    /// matching what was issued. See the module docs for why a generation mismatch is always
    /// `Stale`, even once the slot underneath has gone back to empty.
    fn slot_for(&self, handle: Handle) -> Result<&Slot, CapError> {
        let slot = self
            .slots
            .get(handle.index() as usize)
            .ok_or(CapError::BadHandle)?;
        if slot.generation != handle.generation() {
            return Err(CapError::StaleHandle);
        }
        if slot.object == Object::Null {
            return Err(CapError::BadHandle);
        }
        Ok(slot)
    }

    /// Writes a slot into occupied state and returns the generation it now carries, so the caller
    /// can hand back a handle that matches it.
    fn occupy(&mut self, index: usize, object: Object, rights: Rights, parent: u32) -> u32 {
        let slot = &mut self.slots[index];
        slot.object = object;
        slot.rights = rights;
        slot.parent = parent;
        slot.first_child = Slot::NONE;
        slot.next_sibling = Slot::NONE;
        self.live += 1;
        slot.generation
    }

    /// Prepends `child` to `parent`'s child list. Prepending rather than appending keeps this
    /// O(1) — the list's order carries no meaning of its own, only its membership does.
    fn link_child(&mut self, parent: u32, child: u32) {
        let previous_first = self.slots[parent as usize].first_child;
        self.slots[child as usize].next_sibling = previous_first;
        self.slots[parent as usize].first_child = child;
    }

    /// Removes `index` from its parent's child list, leaving the rest of the list intact. A root
    /// capability has no parent, so this is a no-op for one.
    fn unlink_from_parent(&mut self, index: u32) {
        let parent = self.slots[index as usize].parent;
        if parent == Slot::NONE {
            return;
        }
        if self.slots[parent as usize].first_child == index {
            self.slots[parent as usize].first_child = self.slots[index as usize].next_sibling;
            return;
        }
        let mut cursor = self.slots[parent as usize].first_child;
        while cursor != Slot::NONE {
            let next = self.slots[cursor as usize].next_sibling;
            if next == index {
                self.slots[cursor as usize].next_sibling = self.slots[index as usize].next_sibling;
                return;
            }
            cursor = next;
        }
    }

    /// Resets a slot to free: bumps its generation so every outstanding handle to it becomes
    /// detectable as stale, drops the object and rights, and clears its own tree links.
    ///
    /// Does not touch the *parent's* child list — `free_descendants` relies on that, since every
    /// slot it frees has its parent inside the same subtree (freed in the same pass, so the link
    /// stops mattering), and `delete` unlinks the root of the subtree itself separately.
    fn free_slot(&mut self, index: u32) {
        let slot = &mut self.slots[index as usize];
        slot.object = Object::Null;
        slot.rights = Rights::NONE;
        slot.generation = slot.generation.wrapping_add(1);
        slot.parent = Slot::NONE;
        slot.first_child = Slot::NONE;
        slot.next_sibling = Slot::NONE;
        self.live -= 1;
    }

    /// Frees every descendant of `root`, depth-first, `root` itself excluded.
    ///
    /// Iterative over an explicit stack sized to the table's own capacity rather than recursive
    /// — see the module docs. Every node's children are pushed before that node is freed, and
    /// since every node in the subtree has its parent inside the subtree too, nothing outside
    /// `root` itself is left pointing at a freed slot.
    fn free_descendants(&mut self, root: u32) -> usize {
        let mut stack = [0u32; N];
        let mut len = 0usize;
        Self::push_children(&self.slots, &mut stack, &mut len, root);

        let mut freed = 0usize;
        while len > 0 {
            len -= 1;
            let node = stack[len];
            Self::push_children(&self.slots, &mut stack, &mut len, node);
            self.free_slot(node);
            freed += 1;
        }
        freed
    }

    /// Pushes `of`'s children onto the explicit traversal stack used by `free_descendants`.
    ///
    /// A standalone associated function rather than a method, so `free_descendants` can call it
    /// both before the loop and from inside it without holding a borrow of `self` across the
    /// `free_slot` call that follows in the same iteration.
    fn push_children(slots: &[Slot; N], stack: &mut [u32; N], len: &mut usize, of: u32) {
        let mut child = slots[of as usize].first_child;
        while child != Slot::NONE {
            stack[*len] = child;
            *len += 1;
            child = slots[child as usize].next_sibling;
        }
    }
}

impl<const N: usize> Default for CapSpace<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_round_trips_and_null_is_rejected() {
        let h = Handle::new(7, 42);
        assert_eq!(h.index(), 7);
        assert_eq!(h.generation(), 42);
        assert_eq!(Handle::from_raw(h.raw()), h);

        let space: CapSpace<4> = CapSpace::new();
        assert_eq!(space.get(Handle::NULL), Err(CapError::BadHandle));
    }

    #[test]
    fn get_returns_what_was_inserted() {
        let mut space: CapSpace<4> = CapSpace::new();
        let h = space.insert(Object::Console, Rights::READ).unwrap();

        assert_eq!(space.get(h), Ok((Object::Console, Rights::READ)));
        assert_eq!(space.object(h), Ok(Object::Console));
        assert_eq!(space.rights(h), Ok(Rights::READ));
    }

    #[test]
    fn delegation_narrows_rights_and_never_widens() {
        let mut space: CapSpace<4> = CapSpace::new();
        // Read access plus the ability to grant it onward — everything needed to prove that
        // delegation cannot manufacture WRITE or REVOKE that the parent never had.
        let parent_rights = Rights::READ.union(Rights::GRANT);
        let parent = space.insert(Object::Console, parent_rights).unwrap();

        let child = space.delegate(parent, Rights::ALL).unwrap();
        let child_rights = space.rights(child).unwrap();

        assert_eq!(
            child_rights, parent_rights,
            "child exceeded its parent's rights"
        );
        assert!(!child_rights.contains(Rights::WRITE));
        assert!(!child_rights.contains(Rights::REVOKE));
    }

    #[test]
    fn delegation_without_grant_is_refused() {
        let mut space: CapSpace<4> = CapSpace::new();
        let parent = space.insert(Object::Console, Rights::READ).unwrap();

        assert_eq!(
            space.delegate(parent, Rights::READ),
            Err(CapError::InsufficientRights)
        );
    }

    #[test]
    fn revoking_a_parent_invalidates_the_child_but_not_itself() {
        let mut space: CapSpace<8> = CapSpace::new();
        let parent = space.insert(Object::Console, Rights::ALL).unwrap();
        let child = space.delegate(parent, Rights::ALL).unwrap();

        let freed = space.revoke(parent).unwrap();

        assert_eq!(freed, 1);
        assert_eq!(space.get(child), Err(CapError::StaleHandle));
        assert!(space.get(parent).is_ok());
    }

    #[test]
    fn revocation_is_transitive_across_generations() {
        let mut space: CapSpace<8> = CapSpace::new();
        let root = space.insert(Object::Console, Rights::ALL).unwrap();
        let child = space.delegate(root, Rights::ALL).unwrap();
        let grandchild = space.delegate(child, Rights::ALL).unwrap();

        let freed = space.revoke(root).unwrap();

        assert_eq!(freed, 2, "child and grandchild should both be freed");
        assert_eq!(space.get(child), Err(CapError::StaleHandle));
        assert_eq!(space.get(grandchild), Err(CapError::StaleHandle));
        assert!(space.get(root).is_ok());
    }

    #[test]
    fn revoke_without_revoke_right_is_refused_and_frees_nothing() {
        let mut space: CapSpace<8> = CapSpace::new();
        let root = space.insert(Object::Console, Rights::GRANT).unwrap();
        let child = space.delegate(root, Rights::GRANT).unwrap();

        assert_eq!(space.revoke(root), Err(CapError::InsufficientRights));
        assert!(space.get(child).is_ok());
        assert_eq!(space.count(), 2);
    }

    #[test]
    fn a_reused_slot_gets_a_fresh_generation() {
        let mut space: CapSpace<2> = CapSpace::new();
        let first = space.insert(Object::Console, Rights::ALL).unwrap();
        space.delete(first).unwrap();
        let second = space.insert(Object::Console, Rights::ALL).unwrap();

        assert_eq!(
            first.index(),
            second.index(),
            "the freed slot should be reused"
        );
        assert_ne!(first.raw(), second.raw());
        assert_eq!(space.get(first), Err(CapError::StaleHandle));
        assert!(space.get(second).is_ok());
    }

    #[test]
    fn delete_removes_the_slot_and_its_subtree() {
        let mut space: CapSpace<8> = CapSpace::new();
        let root = space.insert(Object::Console, Rights::ALL).unwrap();
        let child = space.delegate(root, Rights::ALL).unwrap();
        let grandchild = space.delegate(child, Rights::ALL).unwrap();
        assert_eq!(space.count(), 3);

        let freed = space.delete(root).unwrap();

        assert_eq!(freed, 3);
        assert_eq!(space.count(), 0);
        assert_eq!(space.get(root), Err(CapError::StaleHandle));
        assert_eq!(space.get(child), Err(CapError::StaleHandle));
        assert_eq!(space.get(grandchild), Err(CapError::StaleHandle));
    }

    #[test]
    fn insert_past_capacity_is_refused() {
        let mut space: CapSpace<1> = CapSpace::new();
        assert!(space.insert(Object::Console, Rights::READ).is_ok());
        assert_eq!(
            space.insert(Object::Console, Rights::READ),
            Err(CapError::OutOfSlots)
        );
    }

    #[test]
    fn unlinking_a_middle_sibling_preserves_the_rest_of_the_list() {
        let mut space: CapSpace<8> = CapSpace::new();
        let parent = space.insert(Object::Console, Rights::ALL).unwrap();
        let a = space.delegate(parent, Rights::ALL).unwrap();
        let b = space.delegate(parent, Rights::ALL).unwrap();
        let c = space.delegate(parent, Rights::ALL).unwrap();

        // delegate prepends, so the child list reads c, b, a — b sits in the middle.
        let freed = space.delete(b).unwrap();
        assert_eq!(freed, 1);
        assert!(space.get(a).is_ok());
        assert!(space.get(c).is_ok());
        assert_eq!(space.count(), 3, "parent, a and c remain");

        // If unlinking b had corrupted the list, this would miss a sibling or walk a dangling
        // link.
        let freed = space.revoke(parent).unwrap();
        assert_eq!(freed, 2);
        assert_eq!(space.get(a), Err(CapError::StaleHandle));
        assert_eq!(space.get(c), Err(CapError::StaleHandle));
    }
}
