//! The frozen voxel world's sample block after the performance round's decision 9: the hashes'
//! invariant parts are combined before the sample loop, so the block has fewer xors than the 108 it
//! had after Cranelift 0.137 dropped its rebalancing (84 on Cranelift 0.116,
//! `bench/voxels3d/results.md`).

mod common;

use kestrel_codegen::{dump, Options};

const WORLD: &str = include_str!("../../bench/voxels3d/voxels3d.kes");

#[test]
fn the_released_world_sample_block_has_at_most_84_xors() {
    let m = common::optimized(&format!("{WORLD}\nmain() {{}}\n"));
    let code = dump(&m, Options { checked_arith: false, ..Options::default() }, "mesh").unwrap();
    let ir = code.split("\n}\n").next().unwrap();
    // `--release` has no check in the sample loop, so its body is one block: the one holding the
    // first hash multiply (`* 73856093`, 0x0466_f45d).
    let block = ir.split("\n\n").find(|b| b.lines().any(|l| l.contains("imul") && l.contains("0x0466_f45d"))).expect("the sample block");
    let xors = block.lines().filter(|l| l.contains("= bxor")).count();
    assert!(xors <= 84, "{xors} xors:\n{block}");
}
