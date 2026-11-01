//! List headers kept in variables across a loop (the performance round's decision 3): a loop that
//! can't change a list's header reads its `data` and `len` once, before the loop, and a list it
//! only reads and adds to keeps its header in variables too. Each way the rule stops caching
//! (a store to the header, a binding of the list's local in the loop, a call given the list as
//! `var`, a closure call) puts the header loads back in the loop.

mod common;

use std::collections::HashMap;

use kestrel_codegen::{dump, Options};

fn release() -> Options {
    Options { checked_arith: false, ..Options::default() }
}

/// One block of the optimised IR: its number, whether it is cold, its lines, and the blocks it
/// jumps to.
struct Block<'a> {
    id: u32,
    cold: bool,
    lines: Vec<&'a str>,
    succs: Vec<u32>,
}

/// The numbers `blockN` names on a line.
fn block_refs(line: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(k) = rest.find("block") {
        rest = &rest[k + 5..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse() {
            out.push(n);
        }
    }
    out
}

/// The blocks of the optimised IR of `code` (a `dump`), which comes before the machine code.
fn blocks(code: &str) -> Vec<Block<'_>> {
    let ir = code.split("\n}\n").next().unwrap();
    let mut out = Vec::new();
    for chunk in ir.split("\n\n") {
        let mut lines = chunk.lines().map(str::trim);
        let Some(head) = lines.next().filter(|l| l.starts_with("block")) else { continue };
        let id = block_refs(head)[0];
        let lines: Vec<&str> = lines.collect();
        let succs = lines.iter().filter(|l| l.starts_with("jump") || l.starts_with("brif")).flat_map(|l| block_refs(l)).collect();
        out.push(Block { id, cold: head.contains("cold"), lines, succs });
    }
    out
}

/// The blocks that lie on a cycle of the control flow graph: the loops' blocks.
fn in_loops(bs: &[Block]) -> Vec<u32> {
    let succs: HashMap<u32, &Vec<u32>> = bs.iter().map(|b| (b.id, &b.succs)).collect();
    let reaches = |from: u32, to: u32| {
        let mut seen = vec![from];
        let mut todo = vec![from];
        while let Some(b) = todo.pop() {
            for &s in succs.get(&b).map_or(&[][..], |v| v.as_slice()) {
                if s == to {
                    return true;
                }
                if !seen.contains(&s) {
                    seen.push(s);
                    todo.push(s);
                }
            }
        }
        false
    };
    bs.iter().filter(|b| reaches(b.id, b.id)).map(|b| b.id).collect()
}

/// The header loads (`load.i64 notrap aligned`: a list's `data`, `len` or `cap`) in the blocks of
/// loops that run every time round (cold blocks, the grow path and the failure of a check, left
/// out), of function `func` compiled from `src` with `opts`.
fn header_loads_in_loops(src: &str, func: &str, opts: Options) -> Vec<String> {
    let code = dump(&common::optimized(src), opts, func).unwrap();
    let bs = blocks(&code);
    let looping = in_loops(&bs);
    assert!(!looping.is_empty(), "no loop in:\n{code}");
    bs.iter()
        .filter(|b| !b.cold && looping.contains(&b.id))
        .flat_map(|b| b.lines.iter().filter(|l| l.contains("load.i64 notrap aligned")).map(|l| format!("block{}: {l}", b.id)))
        .collect()
}

const COPY: &str = "let WIDTH = 1280\nlet HEIGHT = 720\nstruct Particle {\n pos: vec2\n vel: vec2\n}\nspawn(var parts: list[Particle], count: int) {\n for i from 0 to count {\n  let pos = vec2(randf_range(0, WIDTH), randf_range(0, HEIGHT))\n  let vel = vec2(randf_range(-200, 200), randf_range(-200, 200))\n  parts.add(Particle(pos: pos, vel: vel))\n }\n}\npub bench(n: int, iters: int) -> float {\n var sum = 0.0\n arena {\n  var parts = list[Particle]()\n  spawn(var parts, n)\n  var positions = list[vec2]()\n  while positions.len < parts.len {\n   positions.add(vec2(0, 0))\n  }\n  for k from 0 to iters {\n   for i, p in parts {\n    positions[i] = p.pos\n   }\n  }\n  for q in positions {\n   sum += q.x + q.y\n  }\n }\n sum\n}\nmain() {}\n";

const MAP: &str = "pub bench(n: int, iters: int) -> float {\n var sum = 0.0\n arena {\n  var vs = list[vec2]()\n  var x = 1\n  for i from 0 to n {\n   x = (x * 75 + 74) % 65537\n   let a = float(x) / 65537.0\n   vs.add(vec2(a, 1.0 - a * 0.5))\n  }\n  for k from 0 to iters {\n   arena {\n    let ls = vs.map((v) { v.length() })\n    sum += ls[0] + ls[ls.len - 1] + float(ls.len)\n   }\n  }\n }\n sum\n}\npub mapped(vs: list[vec2]) -> float {\n let ls = vs.map((v) { v.length() })\n ls[0] + ls[ls.len - 1]\n}\nmain() {}\n";

#[test]
fn the_copy_kernel_loads_no_header_inside_its_loops() {
    // `positions[i] = p.pos` in `for i, p in parts`, the `while` that adds to `positions` while
    // reading `parts.len`, and the sum over `positions`: each list's `data` (and `len`) once.
    for opts in [Options::default(), release()] {
        assert_eq!(header_loads_in_loops(COPY, "bench", opts), Vec::<String>::new());
    }
}

#[test]
fn the_map_kernel_loads_no_header_inside_its_loop() {
    // `map`'s loop reads `vs` and adds to its result, whose room it made before the loop. (The
    // kernel's `for k` loop around it makes the closure literal, a `MakeFn`, so by rule (d) it
    // reads the headers it uses from memory; `map`'s own loop is checked here.)
    for opts in [Options::default(), release()] {
        assert_eq!(header_loads_in_loops(MAP, "mapped", opts), Vec::<String>::new());
    }
}

#[test]
fn a_list_only_added_to_and_read_keeps_its_header_in_variables() {
    let src = "f(n: int) -> int {\n var xs: list[int] = [1]\n var t = 0\n for i from 0 to n {\n  xs.add(xs[i] + 1)\n  t += xs[0] + xs.len\n }\n t\n}\nmain() {}\n";
    assert_eq!(header_loads_in_loops(src, "f", Options::default()), Vec::<String>::new());
}

#[test]
fn writing_another_field_of_the_struct_leaves_the_list_cached() {
    let src = "struct Bag {\n items: list[int]\n count: int\n}\nbump(var n: int, by: int) {\n if by > 0 {\n  n += 1\n  bump(var n, by - 1)\n }\n}\nf(var b: Bag) -> int {\n for i from 0 to b.items.len {\n  b.count += b.items[i]\n  bump(var b.count, 1)\n }\n b.count\n}\nmain() {}\n";
    assert_eq!(header_loads_in_loops(src, "f", Options::default()), Vec::<String>::new());
}

#[test]
fn a_store_to_the_header_puts_its_loads_back_in_the_loop() {
    // (a): `clear`, and an assignment of the whole list.
    let clear = "f(var xs: list[int], n: int) -> int {\n var t = 0\n for i from 0 to n {\n  t += xs[0]\n  xs.clear()\n  xs.add(i)\n }\n t\n}\nmain() {}\n";
    assert_ne!(header_loads_in_loops(clear, "f", Options::default()), Vec::<String>::new());
    let assign = "f(var xs: list[int], ys: list[int], n: int) -> int {\n var t = 0\n for i from 0 to n {\n  t += xs[0]\n  if i == 2 {\n   xs = ys\n  }\n }\n t\n}\nmain() {}\n";
    assert_ne!(header_loads_in_loops(assign, "f", Options::default()), Vec::<String>::new());
}

#[test]
fn a_list_made_inside_the_loop_is_read_from_memory() {
    // (b): a `Let` of the list's local in the loop.
    let src = "f(n: int) -> int {\n var t = 0\n for i from 0 to n {\n  var zs: list[int] = [i, i + 1]\n  zs.add(i)\n  t += zs[i % 3]\n }\n t\n}\nmain() {}\n";
    assert_ne!(header_loads_in_loops(src, "f", Options::default()), Vec::<String>::new());
}

#[test]
fn a_call_given_the_list_as_var_puts_its_loads_back_in_the_loop() {
    // (c): the callee may add to the list.
    let src = "fill(var xs: list[int], n: int) {\n if n > 0 {\n  xs.add(n)\n  fill(var xs, n - 1)\n }\n}\nf(var xs: list[int], n: int) -> int {\n var t = 0\n for i from 0 to n {\n  t += xs[xs.len - 1]\n  fill(var xs, 2)\n }\n t\n}\nmain() {}\n";
    assert_ne!(header_loads_in_loops(src, "f", Options::default()), Vec::<String>::new());
}

#[test]
fn a_call_through_a_function_value_puts_its_loads_back_in_the_loop() {
    // (d): a function value may reach the list.
    let src = "f(xs: list[int], g: (int) -> int, n: int) -> int {\n var t = 0\n for i from 0 to n {\n  t += xs[i % 4] + g(i)\n }\n t\n}\nmain() {}\n";
    assert_ne!(header_loads_in_loops(src, "f", Options::default()), Vec::<String>::new());
}

#[test]
fn a_struct_of_four_byte_numbers_is_copied_in_eight_byte_moves() {
    // 16 bytes of `int`, `float` and `vec2`: two 8-byte moves (without the `aligned` flag, since
    // the struct is 4-aligned), the bits as they are. A `vec3` (12 bytes) keeps its field moves.
    let src = "struct P {\n a: int\n b: float\n c: vec2\n}\nf(var xs: list[P], ys: list[P]) {\n for i from 0 to ys.len {\n  xs[i] = ys[i]\n }\n}\ng(var xs: list[vec3], ys: list[vec3]) {\n for i from 0 to ys.len {\n  xs[i] = ys[i]\n }\n}\nmain() {}\n";
    let m = common::optimized(src);
    let code = dump(&m, release(), "f").unwrap();
    let ir = code.split("\n}\n").next().unwrap();
    let wide = ir.lines().filter(|l| l.contains("load.i64 notrap v")).count();
    assert_eq!(wide, 2, "{code}");
    assert!(!ir.contains("load.f32") && !ir.contains("load.i32"), "{code}");
    let code = dump(&m, release(), "g").unwrap();
    let ir = code.split("\n}\n").next().unwrap();
    assert_eq!(ir.matches("load.f32").count(), 3, "{code}");
}

#[test]
fn map_makes_room_for_every_result_before_its_loop() {
    use kestrel_kir::{ListOp, Stmt};
    let m = common::optimized(MAP);
    let f = m.funcs.iter().find(|f| f.name.starts_with("__kestrel_list_fn_map_")).expect("map's loop");
    let reserve = f.body.iter().position(|s| matches!(s, Stmt::List(ListOp::Reserve, ..))).expect("the room");
    let each = f.body.iter().position(|s| matches!(s, Stmt::ForRange { .. })).expect("the loop");
    assert!(reserve < each);
}
