//! Views read a place without copying it: nothing may move what a view reads, and
//! `let x = opt else` reads in place when `opt` is a place that can't change.

use kestrel_check::{check, Tables};
use kestrel_syntax::parse;

const HEAD: &str = "struct Bag {\n    mem: Arena = Arena()\n    n: int = 0\n}\nstruct Holder {\n    b: Bag?\n}\nenum Slot {\n    Empty\n    Full { b: Bag }\n}\npeek(o: Bag?) -> int {\n    1\n}\n";

fn run(body: &str) -> (Option<Tables>, Vec<String>) {
    let src = format!("{HEAD}{body}");
    let (p, d) = parse(&src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = check(&p);
    (t, d.into_iter().map(|d| d.message).collect())
}

fn errors(body: &str) -> Vec<String> {
    run(body).1
}

fn none() -> Vec<String> {
    Vec::new()
}

#[test]
fn moving_a_matched_variable_while_an_arm_reads_it_is_an_error() {
    let body = "main() {\n var keep = list[Slot]()\n let s = Slot.Full(Bag())\n match s {\n  Full f {\n   keep.add(s)\n   print(f.b.n)\n  }\n  else { }\n }\n}\n";
    assert_eq!(errors(body), vec!["can't move `s` here: this `match` arm's `f` reads from it".to_string()]);
    // An arm with no binding reads nothing.
    let free = "main() {\n var keep = list[Slot]()\n let s = Slot.Full(Bag())\n match s {\n  Full {\n   keep.add(s)\n  }\n  else { }\n }\n}\n";
    assert_eq!(errors(free), none());
}

#[test]
fn let_else_unwraps_an_owning_parameter_field_or_self_in_place() {
    assert_eq!(errors("size(o: Bag?) -> int {\n let b = o else {\n  return 0\n }\n b.n\n}\nmain() {}\n"), none());
    assert_eq!(errors("main() {\n let h = Holder(b: Bag())\n let b = h.b else {\n  return\n }\n print(b.n)\n}\n"), none());
    let method = "struct Level {\n b: Bag?\n\n size(self) -> int {\n  let b = self.b else {\n   return 0\n  }\n  b.n\n }\n}\nmain() {}\n";
    assert_eq!(errors(method), none());
}

#[test]
fn a_whole_owning_local_still_moves() {
    let body = "main() {\n var keep = list[Bag]()\n let o: Bag? = Bag()\n let b = o else {\n  return\n }\n keep.add(b)\n print(peek(o))\n}\n";
    assert_eq!(errors(body), vec!["`o` was moved earlier, so it can't be used here".to_string()]);
}

#[test]
fn a_var_unwrap_still_moves_or_copies() {
    let body = "main() {\n let h = Holder(b: Bag())\n var b = h.b else {\n  return\n }\n}\n";
    assert_eq!(
        errors(body),
        vec!["can't move this out: it holds an `Arena`, so it can't be copied, and only whole variables move".to_string()]
    );
}

#[test]
fn a_view_and_what_it_reads_cant_move() {
    let field = "main() {\n var keep = list[Bag]()\n let h = Holder(b: Bag())\n let b = h.b else {\n  return\n }\n keep.add(b)\n}\n";
    assert_eq!(errors(field), vec!["can't move `b`: it's inside `h`, and it holds an `Arena`, so it can't be copied".to_string()]);
    let param = "take(o: Bag?, var keep: list[Bag]) {\n let b = o else {\n  return\n }\n keep.add(b)\n}\nmain() {}\n";
    assert_eq!(
        errors(param),
        vec!["can't move `b`: it's inside parameter `o`, which belongs to the caller, and it holds an `Arena`, so it can't be copied".to_string()]
    );
    let root = "main() {\n var keep = list[Holder]()\n let h = Holder(b: Bag())\n let b = h.b else {\n  return\n }\n keep.add(h)\n print(b.n)\n}\n";
    assert_eq!(errors(root), vec!["can't move `h` here: `b` reads from it".to_string()]);
    // After the block that declares the view, the root may move.
    let later = "main() {\n var keep = list[Holder]()\n let h = Holder(b: Bag())\n if true {\n  let b = h.b else {\n   return\n  }\n  print(b.n)\n }\n keep.add(h)\n}\n";
    assert_eq!(errors(later), none());
    // At the end of a block or function, where the value moves out after the block has closed.
    let moved_in = "can't move `b`: it's inside parameter `h`, which belongs to the caller, and it holds an `Arena`, so it can't be copied";
    let tail = "f(h: Holder) -> Bag {\n let b = h.b else {\n  return Bag()\n }\n b\n}\nmain() {}\n";
    assert_eq!(errors(tail), vec![moved_in.to_string()]);
    let tail_if = "f(h: Holder, c: bool) -> Bag {\n let b = h.b else {\n  return Bag()\n }\n if c { b } else { Bag() }\n}\nmain() {}\n";
    assert_eq!(errors(tail_if), vec![moved_in.to_string()]);
    let local_tail = "f() -> Bag {\n let h = Holder(b: Bag())\n let b = h.b else {\n  return Bag()\n }\n b\n}\nmain() {}\n";
    assert_eq!(errors(local_tail), vec!["can't move `b`: it's inside `h`, and it holds an `Arena`, so it can't be copied".to_string()]);
}

/// `main` with a view of `h.b` inside `shape`'s block (`loop`, `if` or `match`), with `inside` between the
/// view and its use, `sibling` in the branch beside the block (if it has one), and `after` once it is over.
fn guarded(shape: &str, inside: &str, sibling: &str, after: &str) -> String {
    let view = format!("let b = h.b else {{\n   return\n  }}\n  {inside}\n  print(b.n)\n");
    let block = match shape {
        "loop" => format!(" for i from 0 to 1 {{\n  {view} }}\n"),
        "if" => format!(" if true {{\n  {view} }} else {{\n  {sibling}\n }}\n"),
        _ => format!(" match s {{\n  Empty {{\n  {view} }}\n  else {{\n  {sibling}\n }}\n }}\n"),
    };
    format!("main() {{\n var keep = list[Holder]()\n let h = Holder(b: Bag())\n let s = Slot.Empty\n{block} {after}\n}}\n")
}

#[test]
fn a_view_guards_its_root_only_until_its_block_ends() {
    for shape in ["loop", "if", "match"] {
        // After the block, or in the branch beside it, the root may move.
        assert_eq!(errors(&guarded(shape, "", "", "keep.add(h)")), none(), "{shape}: after the block");
        if shape != "loop" {
            assert_eq!(errors(&guarded(shape, "", "keep.add(h)", "")), none(), "{shape}: in the other branch");
        }
        // While the view is live it may not.
        let inside = errors(&guarded(shape, "keep.add(h)", "", ""));
        assert_eq!(inside, vec!["can't move `h` here: `b` reads from it".to_string()], "{shape}: inside the block");
    }
}

#[test]
fn a_receiver_cant_also_be_passed_as_var() {
    let cell = "struct Cell {\n b: Bag?\n\n poke(self, var other: Cell) -> int {\n  let b = self.b else {\n   return 0\n  }\n  other.b = none\n  b.n\n }\n\n bump(var self, other: Cell) {\n  self.b = none\n }\n}\n";
    // `poke` takes `self` by pointer, so changing `c` through `other` would change what `self` reads.
    let poke = format!("{cell}main() {{\n var c = Cell(b: Bag())\n print(c.poke(var c))\n}}\n");
    assert_eq!(errors(&poke), vec!["`c` is passed as `var` and is also the receiver of this call".to_string()]);
    // A different variable is fine.
    let apart = format!("{cell}main() {{\n var c = Cell(b: Bag())\n var d = Cell(b: Bag())\n print(c.poke(var d))\n print(d.poke(var c))\n}}\n");
    assert_eq!(errors(&apart), none());
    // The plain function form, and a `var self` receiver that is also read, were already refused.
    let free = format!("{cell}both(a: Cell, var b: Cell) {{\n}}\nmain() {{\n var c = Cell(b: Bag())\n both(c, var c)\n}}\n");
    assert_eq!(errors(&free), vec!["`c` is passed as `var` and is also passed in another argument of the same call".to_string()]);
    let bump = format!("{cell}main() {{\n var c = Cell(b: Bag())\n c.bump(c)\n}}\n");
    assert_eq!(errors(&bump), vec!["`c` is changed by this call, so it can't also be passed to it".to_string()]);
}

#[test]
fn only_heap_payloads_of_places_that_cant_change_are_views() {
    let views = |body: &str| {
        let (t, errs) = run(body);
        assert!(errs.is_empty(), "{errs:?}");
        t.unwrap().let_else_views.len()
    };
    assert_eq!(views("count(name: string?) -> int {\n let n = name else {\n  return 0\n }\n 1\n}\nmain() {}\n"), 1);
    assert_eq!(views("count(name: string?) -> int {\n var n = name else {\n  return 0\n }\n 1\n}\nmain() {}\n"), 0);
    assert_eq!(views("count(o: int?) -> int {\n let n = o else {\n  return 0\n }\n n\n}\nmain() {}\n"), 0);
    // A `var` root can change while the view reads it, so it's copied.
    assert_eq!(views("main() {\n var names: string? = \"a\"\n let n = names else {\n  return\n }\n print(n)\n}\n"), 0);
}

#[test]
fn a_loop_item_of_a_let_list_roots_a_view() {
    let (t, errs) = run("main() {\n let hs = [Holder(b: Bag()), Holder(b: none)]\n for h in hs {\n  let b = h.b else {\n   continue\n  }\n  print(b.n)\n }\n}\n");
    assert!(errs.is_empty(), "{errs:?}");
    assert_eq!(t.unwrap().let_else_views.len(), 1);
    // The view reads the list's item in place (the message names the list), so it can't move out.
    let moved = "main() {\n var keep = list[Bag]()\n let hs = [Holder(b: Bag())]\n for h in hs {\n  let b = h.b else {\n   continue\n  }\n  keep.add(b)\n }\n}\n";
    assert_eq!(errors(moved), vec!["can't move `b`: it's inside `hs`, and it holds an `Arena`, so it can't be copied".to_string()]);
}
