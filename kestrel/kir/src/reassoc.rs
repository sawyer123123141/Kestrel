//! Xor, and, or chains regrouped so a loop's invariant operands combine outside it (spec
//! decision 9 of the performance round, `docs/superpowers/specs/2026-11-01-perf-round-design.md`).
//! Does nothing yet.

use crate::ir::Module;
use crate::range::Arith;

/// Groups the operands a loop never writes in each `^`, `&` or `|` chain (decision 9 fills it).
pub(crate) fn run(_m: &mut Module, _arith: Arith) {}
