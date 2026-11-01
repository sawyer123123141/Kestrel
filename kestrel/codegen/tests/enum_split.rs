mod common;

use kestrel_codegen::{dump, Options};

fn heightmap() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../bench/heightmap/heightmap.kes")).unwrap()
}

#[test]
fn the_heightmap_checksum_keeps_its_block_out_of_memory() {
    let m = common::optimized(&heightmap());
    for opts in [Options::default(), Options { checked_arith: false, ..Options::default() }] {
        let code = dump(&m, opts, "Chunk.checksum").unwrap();
        // `block_at(y, h).weight()` made a `Block` in a stack slot; now its tag and hardness are
        // values.
        assert!(!code.contains("explicit_slot"), "{code}");
    }
}
