//! `@prove` contracts as KIR keeps them: one `ContractSite` per function, method or loop that
//! promises something (an interface method's promise becomes a site on each implementing method),
//! listed in `Module::contracts`. The code a site covers is marked where it is not a whole
//! function: lowering wraps an annotated loop's statements in `Stmt::Prove { id }`, and the inliner
//! wraps each expanded copy of a contracted callee the same way (`id` is the site's index).
//! Range analysis tags each check and event with the `Prove` blocks around it (its regions).

use std::collections::HashMap;

use kestrel_check::SnapshotFault;
use kestrel_syntax::ast::Contract;
use kestrel_syntax::Span;

use crate::ir::{FuncIdx, Stmt};
use crate::visit::{walk, Node};

/// A set of contracts, one bit each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ContractSet(pub u8);

impl ContractSet {
    pub const NO_ALLOC: ContractSet = ContractSet(1);
    pub const NO_CHECKS: ContractSet = ContractSet(2);
    pub const NO_PANIC: ContractSet = ContractSet(4);
    pub const INLINE: ContractSet = ContractSet(8);
    pub const DETERMINISTIC: ContractSet = ContractSet(16);

    /// The set holding just `c`'s bit (`no_panic` brings `no_checks`, as `insert` does).
    pub fn of(c: Contract) -> ContractSet {
        let mut set = ContractSet::default();
        set.insert(match c {
            Contract::NoAlloc => ContractSet::NO_ALLOC,
            Contract::NoChecks => ContractSet::NO_CHECKS,
            Contract::NoPanic => ContractSet::NO_PANIC,
            Contract::Inline => ContractSet::INLINE,
            Contract::Deterministic => ContractSet::DETERMINISTIC,
        });
        set
    }

    /// Whether every contract in `other` is in the set.
    pub fn contains(self, other: ContractSet) -> bool {
        self.0 & other.0 == other.0
    }

    /// Adds the contracts in `other`; `no_panic` includes `no_checks`, so it adds that too.
    pub fn insert(&mut self, other: ContractSet) {
        self.0 |= other.0;
        if self.contains(ContractSet::NO_PANIC) {
            self.0 |= ContractSet::NO_CHECKS.0;
        }
    }
}

/// What a site's contracts are about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProveTarget {
    /// A function or method: its own body, and every copy the inliner made of it.
    Func(FuncIdx),
    /// A `for`, `par for` or `while` loop: every `Stmt::Prove` block with the site's id. `func` is
    /// the function that holds the loop's own code (a `par for` body or a closure's code when the
    /// loop is written in one); `line` is the packed line of the loop's keyword.
    Loop { func: FuncIdx, line: u32 },
}

/// One function, method or loop and the contracts it must keep.
#[derive(Clone, Debug, PartialEq)]
pub struct ContractSite {
    pub target: ProveTarget,
    /// Each contract once, in the order written, with where it is written: inside `@prove(...)`,
    /// or, for a site made from an interface method, the implementing method's name.
    pub contracts: Vec<(Contract, Span)>,
    /// For a site made from an interface method's `@prove`: that method, as "`Shape.area`".
    pub required_by: Option<String>,
    /// For a site made from an interface method's `@prove`: that method's stub, one per instance
    /// of a generic interface (`Handler[Click].handle`), which calls through it name.
    pub through: Option<FuncIdx>,
    /// For a loop in a copy of a generic, or in a field default lowered for an instance: what it
    /// was lowered for (the loop's `func` may be a closure's code, which has no `CopyNote`).
    pub copy: Option<SiteCopy>,
    /// For a function site with `deterministic`: the first part of a `var` parameter (or `var
    /// self`) that a snapshot can't copy and `checksum` can't read, if there is one.
    pub snapshot: Option<SnapshotFault>,
}

/// What a loop site was lowered for (see `ContractSite::copy`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteCopy {
    /// A copy of a generic function or method.
    Func(FuncIdx),
    /// An instance whose field default holds the loop, by name (`Box[string]`).
    Instance(String),
}

/// One `Stmt::Prove` block around a check or an event: which copy of which site. Two inlined
/// copies of one callee in one function have the same `id` (and may have the same `at`); `copy`
/// tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Region {
    /// The site's index in `Module::contracts`.
    pub id: u32,
    /// The block's `at`: the packed line of the call an inlined copy was expanded at; 0 for a loop.
    pub at: u32,
    /// The block's number among the function's `Prove` blocks, in `visit::walk` order (0 first).
    pub copy: u32,
}

/// The number of each `Prove` block in `stmts` (see `Region::copy`), by the block's address.
pub fn prove_copies(stmts: &[Stmt]) -> HashMap<usize, u32> {
    let mut out = HashMap::new();
    walk(stmts, &mut |n| {
        if let Node::Stmt(s @ Stmt::Prove { .. }) = n {
            let copy = out.len() as u32;
            out.insert(s as *const Stmt as usize, copy);
        }
    });
    out
}
