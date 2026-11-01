//! The frozen voxel world's sample loop after the performance round's decision 7: `--release` has
//! no check left in `mesh` (the counter rule proves `out[i]`), and the checked build's sample
//! loop keeps only the chunk offset that depends on `x`.

mod common;

use kestrel_codegen::{dump, Options};

const WORLD: &str = include_str!("../../bench/voxels3d/voxels3d.kes");

fn mesh(opts: Options) -> String {
    let m = common::optimized(&format!("{WORLD}\nmain() {{}}\n"));
    dump(&m, opts, "mesh").unwrap()
}

#[test]
fn the_released_world_mesh_has_no_check() {
    let code = mesh(Options { checked_arith: false, ..Options::default() });
    let ir = code.split("\n}\n").next().unwrap();
    assert!(!ir.contains("trap user"), "{ir}");
}

#[test]
fn the_checked_world_sample_loop_checks_only_the_offset_that_depends_on_x() {
    // From the sample loop's test (`x >= 34`) to its first hash multiply (`* 73856093`): 12
    // overflow checks before this round (the chunk offsets of `x`, `y` and `z` among them); now
    // only `cx * 32 + x - 1`, which depends on `x`; the rest run before the loops.
    let code = mesh(Options::default());
    let ir = code.split("\n}\n").next().unwrap();
    let hash = ir.lines().position(|l| l.contains("imul") && l.contains("0x0466_f45d")).expect("a hash multiply");
    let lines: Vec<&str> = ir.lines().take(hash).collect();
    let head = lines.iter().rposition(|l| l.contains("icmp sge")).expect("the loop's test");
    let checks = lines[head..].iter().filter(|l| l.contains("_overflow")).count();
    assert_eq!(checks, 2, "{}", lines[head..].join("\n"));
}
