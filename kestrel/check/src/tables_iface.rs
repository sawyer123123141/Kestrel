//! The interface tables: an interface's id and what the checker knows about it (`Tables::ifaces`,
//! `Tables::iface_by_name`).

use kestrel_syntax::Span;

use crate::tables::{FnId, Tables};

/// An interface, an index into `Tables::ifaces`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IfaceId(pub u32);

#[derive(Clone, Debug)]
pub struct IfaceInfo {
    pub name: String,
    /// Index into `Program::items`.
    pub item: usize,
    pub span: Span,
    /// The methods as abstract `FnInfo`s, in declaration order: also their slot order in a
    /// method table.
    pub methods: Vec<FnId>,
}

impl Tables {
    pub fn iface(&self, id: IfaceId) -> &IfaceInfo {
        &self.ifaces[id.0 as usize]
    }
}
