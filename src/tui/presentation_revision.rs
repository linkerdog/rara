//! Mutation identities for owned presentation inputs, independent of rendering.

use std::{
    ops::{Deref, DerefMut},
    rc::Rc,
};

#[derive(Clone, Debug)]
pub(crate) struct PresentationRevision(Rc<()>);

impl Default for PresentationRevision {
    fn default() -> Self {
        Self(Rc::new(()))
    }
}

impl PartialEq for PresentationRevision {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for PresentationRevision {}

/// Tracks owned data whose mutations cannot bypass a mutable borrow.
///
/// Do not use this for values mutated through shared references. Retaining the
/// revision keeps its allocation alive, so replacement cannot reuse its identity.
#[derive(Clone, Debug)]
pub(crate) struct PresentationInput<T> {
    value: T,
    revision: PresentationRevision,
}

impl<T> PresentationInput<T> {
    pub(crate) fn revision(&self) -> PresentationRevision {
        self.revision.clone()
    }

    pub(crate) fn into_inner(self) -> T {
        self.value
    }
}

impl<T: PartialEq> PresentationInput<T> {
    pub(crate) fn set_if_changed(&mut self, value: T) {
        if self.value != value {
            **self = value;
        }
    }
}

impl<T> From<T> for PresentationInput<T> {
    fn from(value: T) -> Self {
        Self {
            value,
            revision: PresentationRevision::default(),
        }
    }
}

impl<T: Default> Default for PresentationInput<T> {
    fn default() -> Self {
        T::default().into()
    }
}

impl<T> Deref for PresentationInput<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for PresentationInput<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.revision = PresentationRevision::default();
        &mut self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_mutation_clone_and_replacement_cannot_reuse_a_retained_revision() {
        let mut input = PresentationInput::from(vec![String::from("before")]);
        let original = input.revision();
        assert_eq!(input[0], "before");
        assert_eq!(input.revision(), original);
        let mut copy = input.clone();
        copy[0].replace_range(.., "after!");
        assert_ne!(copy.revision(), original);
        assert_eq!(input.revision(), original);
        input = vec![String::from("before")].into();
        assert_ne!(input.revision(), original);
        assert_eq!(copy.into_inner(), ["after!"]);
    }

    #[test]
    fn identical_status_updates_preserve_revision() {
        let mut input = PresentationInput::from(Some(String::from("streaming")));
        let revision = input.revision();
        input.set_if_changed(Some("streaming".into()));
        assert_eq!(input.revision(), revision);
        input.set_if_changed(None);
        assert_ne!(input.revision(), revision);
    }
}
