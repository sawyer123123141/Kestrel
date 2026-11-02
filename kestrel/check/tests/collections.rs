use kestrel_check::{check, Tables, Ty};
use kestrel_syntax::parse;

fn run(src: &str) -> (Option<Tables>, Vec<String>) {
    let (p, d) = parse(src);
    assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (t, d) = check(&p);
    (t, d.into_iter().map(|d| d.message).collect())
}

fn local<'t>(t: &'t Tables, name: &str) -> &'t kestrel_check::LocalInfo {
    t.locals.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no local {name}"))
}

const PROGRAM: &str = "struct Chunk {
    x: int
    heights: [int] = []
}

pure total(xs: [int]) -> int {
    var s = 0
    for v in xs { s += v }
    s
}

main() {
    var a = [1, 2, 3]
    a[0] = 5
    a[1] += a[2]
    let e: [float] = []
    var grid = array[u8](16)
    grid[3] = 7
    var world = list[Chunk]()
    world.add(Chunk(x: 1))
    world[0].heights = array[int](4)
    world[0].heights[2] = 9
    for i, c in world {
        c.x += i
    }
    var tris: list[vec3] = []
    tris.add(vec3(1, 2, 3))
    let n = a.len + world.len + tris.len
    print(total(a), n, a, e, grid)
}
";

#[test]
fn arrays_and_lists_check() {
    let (t, errs) = run(PROGRAM);
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.unwrap();
    assert_eq!(t.ty_name(local(&t, "a").ty), "[int]");
    assert_eq!(t.ty_name(local(&t, "e").ty), "[float]");
    assert_eq!(t.ty_name(local(&t, "grid").ty), "[u8]");
    assert_eq!(t.ty_name(local(&t, "world").ty), "list[Chunk]");
    assert_eq!(t.ty_name(local(&t, "tris").ty), "list[vec3]");
    assert_eq!(local(&t, "n").ty, Ty::Int);
    // The loop binding stands for `world[i]` and can change it.
    let world = t.locals.iter().position(|l| l.name == "world").unwrap();
    let c = local(&t, "c");
    assert_eq!(c.alias.map(|l| l.0 as usize), Some(world));
    assert!(c.mutable);
    assert_eq!(local(&t, "i").ty, Ty::Int);
}

#[test]
fn collection_errors() {
    let cases: &[(&str, &str)] = &[
        ("main() {\n let xs = []\n}\n", "can't tell what `[]` holds"),
        ("main() {\n let a = [1, true]\n}\n", "expected int, found bool"),
        ("main() {\n let x = 5\n let y = x[0]\n}\n", "can't index int; only arrays, lists, maps and strings can be indexed"),
        ("main() {\n let a = [1]\n let y = a[true]\n}\n", "an index must be an integer, found bool"),
        ("main() {\n let a = [1]\n a[0] = 2\n}\n", "can't change `a`: it was declared with `let`"),
        ("main() {\n var a = [1]\n a.add(2)\n}\n", "arrays have a fixed length; use a `list[int]` to add items"),
        ("main() {\n let l = list[int]()\n l.add(1)\n}\n", "can't call `add` on `l`: it changes `l`, which was declared with `let`"),
        ("main() {\n var l = list[int]()\n l.add(true)\n}\n", "expected int, found bool"),
        ("main() {\n let a = [1]\n let n = a.size\n}\n", "`[int]` has no field `size`"),
        ("main() {\n let a = array[int]()\n}\n", "`array[T](n)` takes one length"),
        ("main() {\n let l = list[int](3)\n}\n", "`list[T]()` takes no arguments"),
        ("main() {\n let a = array[int](1.5)\n}\n", "an array length must be an integer, found float"),
        ("main() {\n for v in 5 { }\n}\n", "can't loop over int; `for ... in` needs an array, list, map or string"),
        ("main() {\n let a = [1]\n for v in a { v = 2 }\n}\n", "can't change `v`: it is an item of `a`, which was declared with `let`"),
        ("main() {\n let list = 3\n}\n", "`list` is a built-in name; pick another"),
        ("f(xs: [int]) {\n xs[0] = 1\n}\nmain() {}\n", "can't change parameter `xs`"),
        ("main() {\n let m = map[vec3, int]()\n}\n", "map keys must be integers, bools or strings, not `vec3`"),
        ("f(x: array[int]) {}\nmain() {}\n", "write array types as `[T]`"),
        ("main() {\n var a = [1]\n a.push(2)\n}\n", "`[int]` has no method `push`"),
        ("main() {\n var l = list[int]()\n l.add()\n}\n", "`add` takes one value"),
    ];
    for (src, expected) in cases {
        let errs = run(src).1;
        assert_eq!(errs, vec![expected.to_string()], "source:\n{src}");
    }
}

#[test]
fn exclusivity_reaches_through_elements() {
    let src = "f(var x: [int], y: [[int]]) {}\nmain() {\n var a = [[1], [2]]\n f(var a[0], a)\n}\n";
    let errs = run(src).1;
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("`a`"), "{errs:?}");
    // Reading a number out of the changed variable is fine: it's copied first.
    let ok = "g(var x: [int], n: int) {}\nmain() {\n var a = [1, 2]\n g(var a, a[0])\n}\n";
    assert_eq!(run(ok).1, Vec::<String>::new());
}

#[test]
fn list_methods_have_their_types() {
    let src = "main() {
    var xs: list[int] = [1, 2, 3]
    xs.remove_at(0)
    xs.insert(1, 5)
    let last = xs.pop()
    let at = xs.find(5)
    let has = xs.has(5)
    xs.clear()
    let arr = [1.5, 2.5]
    let in_arr = arr.has(2.5)
    let words = [\"a\"]
    let word_at = words.find(\"a\")
}
";
    let (t, errs) = run(src);
    assert!(errs.is_empty(), "{errs:?}");
    let t = t.unwrap();
    assert_eq!(t.ty_name(local(&t, "last").ty), "int?");
    assert_eq!(t.ty_name(local(&t, "at").ty), "int?");
    assert_eq!(local(&t, "has").ty, Ty::Bool);
    assert_eq!(local(&t, "in_arr").ty, Ty::Bool);
    assert_eq!(t.ty_name(local(&t, "word_at").ty), "int?");
}

#[test]
fn list_method_errors() {
    let cases: &[(&str, &str)] = &[
        ("main() {\n let l: list[int] = []\n l.remove_at(0)\n}\n", "can't call `remove_at` on `l`: it changes `l`, which was declared with `let`"),
        ("main() {\n var a = [1]\n a.pop()\n}\n", "arrays have a fixed length; use a `list[int]` if items come and go"),
        ("main() {\n var a = [1]\n a.clear()\n}\n", "arrays have a fixed length; use a `list[int]` if items come and go"),
        ("interface N {\n n(self) -> int\n}\nstruct S {\n a: N\n}\nf(l: list[S], s: S) {\n let b = l.find(s)\n}\nmain() {}\n", "`find` needs items that `==` can compare (numbers, bools, strings, vectors, and structs and enums of them); `list[S]` holds `S`"),
        ("main() {\n var l: list[int] = []\n l.find(1)\n}\n", "this only reads the list, so the line does nothing"),
        ("main() {\n var l: list[int] = []\n l.has(1)\n}\n", "this only reads the list, so the line does nothing"),
        ("main() {\n var l: list[int] = []\n l.insert(1.5, 2)\n}\n", "an index must be an integer, found float"),
        ("main() {\n var l: list[int] = []\n l.remove_at()\n}\n", "`remove_at` takes one index"),
        ("main() {\n var l: list[int] = []\n l.insert(0)\n}\n", "`insert` takes an index and a value"),
        ("main() {\n var l: list[int] = []\n l.pop(1)\n}\n", "`pop` takes no arguments"),
        ("main() {\n var l: list[int] = []\n l.insert(0, true)\n}\n", "expected int, found bool"),
        ("main() {\n var l: list[int] = []\n let b = l.has(true)\n}\n", "expected int, found bool"),
        ("main() {\n var l: list[int] = []\n l.reverse()\n}\n", "`list[int]` has no method `reverse`"),
        ("main() {\n var a = [1]\n a.remove_at(0)\n}\n", "arrays have a fixed length; use a `list[int]` if items come and go"),
    ];
    for (src, expected) in cases {
        let errs = run(src).1;
        assert_eq!(errs, vec![expected.to_string()], "source:\n{src}");
    }
}

#[test]
fn a_list_cant_change_length_inside_a_loop_over_it() {
    let refused = |name: &str, setup: &str, looped: &str, changed: &str| {
        let src = format!(
            "struct W {{\n items: list[int]\n}}\nmain() {{\n var xs: list[int] = [1]\n var w = W([1])\n var grid: list[list[int]] = [[1]]\n{setup}\n for {looped} {{\n  {changed}\n }}\n}}\n"
        );
        let errs = run(&src).1;
        assert_eq!(errs.len(), 1, "{src}\n{errs:?}");
        assert!(errs[0].starts_with(&format!("can't change how many items `{name}` has while looping over it; use `remove ")), "{errs:?}");
    };
    refused("xs", "", "x in xs", "xs.remove_at(0)");
    refused("xs", "", "x in xs", "xs.insert(0, x)");
    refused("xs", "", "x in xs", "xs.pop()");
    refused("xs", "", "x in xs", "xs.clear()");
    refused("w.items", "", "x in w.items", "w.items.clear()");
    refused("grid", "", "row in grid", "for x in grid[0] {\n   row.clear()\n  }");
    // Through an index, any list in `grid` might be the looped one.
    refused("grid", "", "x in grid[0]", "grid[1].clear()");
    refused("grid", "", "x in grid[0]", "grid.pop()");

    // These don't change the list the loop reads.
    let ok = "struct W {
 items: list[int]
 other: list[int]
}
main() {
 var xs: list[int] = [1]
 var ys: list[int] = [1]
 var w = W([1], [2])
 var grid: list[list[int]] = [[1]]
 for x in xs { xs.add(x) }
 for x in xs { ys.clear() }
 for x in w.items { w.other.clear() }
 for row in grid { row.clear() }
 for row in grid { row.add(1) }
 for x in xs { }
 xs.clear()
 for x in grid[0] { ys.pop() }
}
";
    assert_eq!(run(ok).1, Vec::<String>::new());
}

#[test]
fn a_popped_item_that_isnt_owning_is_copied_into_the_current_arena() {
    let src = "main() {
 var keep: string? = none
 var n = 1
 var outer: list[string] = []
 outer.add(\"{n}\")
 keep = outer.pop()
 arena {
  var inner: list[string] = []
  inner.add(\"{n}\")
  let own = inner.pop()
 }
}
";
    assert_eq!(run(src).1, Vec::<String>::new());
    // The copy is made in the `arena { }` block, even from a list that lives longer.
    let copied = "main() {
 var keep: string? = none
 var n = 1
 var outer: list[string] = []
 outer.add(\"{n}\")
 arena {
  keep = outer.pop()
 }
}
";
    let escape = "main() {
 var keep: string? = none
 var n = 1
 arena {
  var inner: list[string] = []
  inner.add(\"{n}\")
  keep = inner.pop()
 }
}
";
    for src in [copied, escape] {
        let errs = run(src).1;
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].starts_with("this memory is freed at the end of the `arena { }` block, but `keep` lives longer"), "{errs:?}");
    }
}
