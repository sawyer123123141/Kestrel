//! Accumulator recursion as a loop, in `--release` only (spec decision 6 of the performance
//! round, `docs/superpowers/specs/2026-11-01-perf-round-design.md`). Does nothing yet.

use crate::ir::Module;
use crate::range::Arith;

/// Rewrites `if c { base } else { f(a) op f(b) }` and `f(a) op x` as a loop (decision 6 fills it).
pub(crate) fn run(_m: &mut Module, _arith: Arith) {}
