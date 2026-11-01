//! Loop-invariant checks moved before their loop (spec decision 7 of the performance round,
//! `docs/superpowers/specs/2026-11-01-perf-round-design.md`). Does nothing yet.

use crate::ir::Module;
use crate::range::Arith;

/// Moves each check the loop never changes to the loop's entry (decision 7 fills it).
pub(crate) fn run(_m: &mut Module, _arith: Arith) {}
