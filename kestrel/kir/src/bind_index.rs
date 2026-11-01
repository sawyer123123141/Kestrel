//! Binding `parts[i]` once for a run of statements, so `promote` keeps its fields in registers
//! (spec decision 4 of the performance round,
//! `docs/superpowers/specs/2026-11-01-perf-round-design.md`). Does nothing yet.

use crate::ir::Module;

/// Inserts `Bind(e, R[i])` before a run of statements whose places of `R` all go through `R[i]`,
/// and rewrites them to `e` (decision 4 fills it). `optimize` runs it after inlining and before
/// `scalarize`.
pub fn bind_indexes(_m: &mut Module) {}
