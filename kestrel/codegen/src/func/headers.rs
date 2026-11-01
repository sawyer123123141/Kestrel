//! List headers kept in Cranelift variables across a loop that can't change them (spec decision 3
//! of the performance round, `docs/superpowers/specs/2026-11-01-perf-round-design.md`). Caches
//! nothing yet: `cache_headers` returns 0, so every header is read from memory as before.

use cranelift_frontend::Variable;
use cranelift_module::Module;
use kestrel_kir::{self as kir, LocalIdx};

use super::FuncLowerer;

/// A list place whose header a loop keeps in variables: the root local, the fields from it to the
/// list, and the variables holding the header's `data`, `len` and (for a list the loop adds to)
/// `cap`.
#[expect(dead_code, reason = "decision 3 makes and reads these; remove this then")]
pub(crate) struct Cached {
    pub root: LocalIdx,
    pub fields: Vec<u32>,
    pub data: Variable,
    pub len: Variable,
    pub cap: Option<Variable>,
}

impl<'a, 'b, M: Module> FuncLowerer<'a, 'b, M> {
    /// Before a loop with this condition (`None` for a `for` range) and body is lowered: loads the
    /// header of each list place the loop can't change into variables, pushes it on `hdr_cache`,
    /// and returns how many it pushed, for `uncache` after the loop.
    pub(super) fn cache_headers(&mut self, _cond: Option<&kir::Expr>, _body: &[kir::Stmt]) -> usize {
        0
    }

    /// After the loop: drops the `n` places `cache_headers` pushed for it.
    pub(super) fn uncache(&mut self, n: usize) {
        let keep = self.hdr_cache.len() - n;
        self.hdr_cache.truncate(keep);
    }
}
