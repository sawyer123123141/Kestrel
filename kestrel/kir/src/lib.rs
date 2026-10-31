mod contract;
pub mod hoist;
pub mod inline;
pub mod ir;
mod ir_expr;
mod ir_stmt;
mod layout;
mod leaves;
mod lower;
mod native;
mod ops;
pub mod promote;
pub mod range;
mod rename;
pub mod scalarize;
mod split;
mod stack_envs;
pub mod visit;
pub mod visit_mut;

pub use ir::*;
pub use layout::*;
pub use lower::{lower, RELOAD_DROP};
pub use native::native_symbol;
pub use stack_envs::check_stack_envs;

/// Inlines small functions, then keeps plain structs and the fields of `var` parameters and loop
/// elements in registers (see `scalarize` and `promote`), then moves pure math whose operands a
/// loop never changes out of the loop (see `hoist`). The driver runs it on every program.
pub fn optimize(m: &mut Module) {
    inline::inline_small(m);
    scalarize::scalarize(m);
    promote::promote(m);
    hoist::hoist_math(m);
}
