mod common;

use kestrel_codegen::{dump, Options};

fn step_index() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../bench/kernels/step_index.kes")).unwrap()
}

/// The IR of the loop whose body starts with the block computing `sqrt`: from that block to the
/// jump back to the loop's header.
fn step_loop(code: &str) -> &str {
    let ir = code.split("\n}\n").next().unwrap();
    let lines: Vec<&str> = ir.lines().collect();
    let sqrt = lines.iter().position(|l| l.contains(" sqrt ")).expect("a sqrt");
    let start = (0..sqrt).rev().find(|&k| lines[k].starts_with("block")).unwrap();
    let body = lines[start].split([':', '(', ' ']).next().unwrap();
    let test = lines.iter().position(|l| l.contains("brif") && l.trim_end().ends_with(&format!(", {body}"))).expect("the loop's test");
    let header_start = (0..test).rev().find(|&k| lines[k].starts_with("block")).unwrap();
    let header = lines[header_start].split([':', '(']).next().unwrap();
    let from = ir.find(lines[start]).unwrap();
    let to = from + ir[from..].find(&format!("jump {header}(")).expect("the jump back");
    &ir[from..to]
}

#[test]
fn step_index_loads_and_stores_each_field_once_per_particle() {
    let m = common::optimized(&step_index());
    for opts in [Options::default(), Options { checked_arith: false, ..Options::default() }] {
        let code = dump(&m, opts, "bench").unwrap();
        let body = step_loop(&code);
        // Four fields loaded and four stored, through one data pointer (at most one header load),
        // and nothing in a stack slot.
        assert_eq!(body.matches("= load.f32").count(), 4, "{body}");
        assert_eq!(body.matches("store").count(), 4, "{body}");
        assert!(body.matches("= load.i64").count() <= 1, "{body}");
        assert!(!body.contains("stack_load") && !body.contains("stack_store") && !body.contains("stack_addr"), "{body}");
    }
}
