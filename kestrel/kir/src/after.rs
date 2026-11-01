//! The passes that run after range analysis has marked the checks it proves: accumulator
//! recursion as a loop (`recurse`), loop-invariant checks moved before their loop
//! (`hoist_checks`), and xor, and, or chains regrouped (`reassoc`).

use crate::ir::Module;
use crate::range::Arith;

/// Runs the passes that need range analysis's marks, in order: `recurse`, `hoist_checks`,
/// `reassoc`. Codegen calls this on the build's analysed copy of the module (after
/// `range::mark_proven`, or on a copy of a module the driver analysed already), so
/// `kestrel check --report` and contracts never see what these passes change. `arith` is the
/// build's arithmetic, the one the marks were proven for.
pub fn after_marking(m: &mut Module, arith: Arith) {
    crate::recurse::run(m, arith);
    crate::hoist_checks::run(m, arith);
    crate::reassoc::run(m, arith);
}
