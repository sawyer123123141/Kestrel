//! The one rule for "a value that can be copied and checksummed": no `Arena`, no GPU object, no
//! interface value and no function value (`pure` or not) anywhere inside. A snapshot copies the
//! value; `checksum` folds it in a fixed order; neither can do that with memory that belongs to an
//! arena or the GPU, with a table address, or with a code address that differs between builds.
//!
//! `@prove(deterministic)` asks this of the type of every `var` parameter (KIR's `lower::prove`);
//! `checksum` asks it of its argument. `snapshot_fault` finds the first part that breaks the rule.

use crate::tables::Tables;
use crate::ty::Ty;

/// What a part of a value is that a snapshot can't copy and `checksum` can't read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    Arena,
    Gpu,
    Iface,
    Fn,
}

impl FaultKind {
    /// How a message says a value holds it: "an `Arena`", "a GPU object", "an interface value",
    /// "a function value".
    pub fn holds(self) -> &'static str {
        match self {
            FaultKind::Arena => "an `Arena`",
            FaultKind::Gpu => "a GPU object",
            FaultKind::Iface => "an interface value",
            FaultKind::Fn => "a function value",
        }
    }
}

/// The first part of a value that can't be copied and checksummed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotFault {
    /// The name the walk started from (a parameter's).
    pub param: String,
    /// Where the part is, as it would be written: `w.mem`, `ws[i].mem`, `g.level.mem`.
    pub path: String,
    pub kind: FaultKind,
}

/// The first part of a value of type `ty`, called `root`, that a snapshot can't copy and `checksum`
/// can't read; `None` when the whole value can be.
///
/// The walk goes depth first: a struct's fields in order, an enum's payload fields in the order
/// of its variants, each by field name (`root.field`; the variant isn't in the path), and the
/// items of a list, array or map, and its keys, as `root[i]`. An optional's payload has its
/// optional's path. A type that contains itself (through a list, say) is walked once. A
/// placeholder has no fault: generic code is judged at each copy, where it is a real type.
pub fn snapshot_fault(t: &Tables, ty: Ty, root: &str) -> Option<SnapshotFault> {
    let (path, kind) = walk(t, ty, root, &mut Vec::new(), false)?;
    Some(SnapshotFault { param: root.to_string(), path, kind })
}

/// Like `snapshot_fault`, for the state of a reloadable program (hot reload design, decision 1):
/// a reload keeps the state in place on the same thread, so a GPU object (a registry id that stays
/// valid there) is allowed; an `Arena`, a function value and an interface value still aren't.
pub fn reload_fault(t: &Tables, ty: Ty, root: &str) -> Option<SnapshotFault> {
    let (path, kind) = walk(t, ty, root, &mut Vec::new(), true)?;
    Some(SnapshotFault { param: root.to_string(), path, kind })
}

/// What built-in `func` (`checksum`, `to_bytes`, `first_difference`, `from_bytes`) says when it
/// can't take a value of type `ty`: the message, which says to `verb` ("checksum", "save",
/// "compare", "read") the other parts instead (with the verb "compare" it says "can't compare",
/// with "read" "can't read", not "can't take"), and a help line that says where the part it can't
/// read is when the value is a name (`root`, written as `w` or `g.level`) and the part is inside it.
/// `None` when it can take the value.
pub fn snapshot_refusal(t: &Tables, ty: Ty, root: Option<&str>, func: &str, verb: &str) -> Option<(String, Option<String>)> {
    let fault = snapshot_fault(t, ty, root.unwrap_or(""))?;
    let whole = fault.path == fault.param;
    let take = match verb {
        "compare" | "read" => verb,
        _ => "take",
    };
    let msg = match (fault.kind, whole) {
        (FaultKind::Fn, true) => format!("`{func}` can't {take} a function value, whose address differs between builds"),
        (kind, true) => format!("`{func}` can't {take} {}; {verb} the values that hold the game's state instead", kind.holds()),
        (kind, false) => {
            let name = t.ty_name(ty);
            let article = if name.starts_with(['A', 'E', 'I', 'O', 'U', 'a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
            let rest = match kind {
                FaultKind::Fn => format!(", whose address differs between builds; {verb} the other fields instead"),
                _ => format!("; {verb} the fields that hold the game's state instead"),
            };
            format!("`{func}` can't {take} {article} `{name}`, which holds {}{rest}", kind.holds())
        }
    };
    let part = match fault.kind {
        FaultKind::Arena => "the `Arena`",
        FaultKind::Gpu => "the GPU object",
        FaultKind::Iface => "the interface value",
        FaultKind::Fn => "the function value",
    };
    let help = root.filter(|_| !whole).map(|_| format!("{part} is `{}`", fault.path));
    Some((msg, help))
}

/// `snapshot_fault` below `path`, skipping the struct and enum types in `seen` (walked already, or
/// being walked: a type that contains itself). With `gpu`, a GPU object is no fault.
fn walk(t: &Tables, ty: Ty, path: &str, seen: &mut Vec<Ty>, gpu: bool) -> Option<(String, FaultKind)> {
    match ty {
        Ty::Arena => Some((path.to_string(), FaultKind::Arena)),
        Ty::Gpu if gpu => None,
        Ty::Gpu => Some((path.to_string(), FaultKind::Gpu)),
        Ty::Iface(_) => Some((path.to_string(), FaultKind::Iface)),
        Ty::Fn(_) => Some((path.to_string(), FaultKind::Fn)),
        Ty::Optional(e) => walk(t, t.elem(e), path, seen, gpu),
        Ty::Array(e) | Ty::List(e) => walk(t, t.elem(e), &format!("{path}[i]"), seen, gpu),
        Ty::Map(k, v) => {
            let items = format!("{path}[i]");
            walk(t, t.elem(k), &items, seen, gpu).or_else(|| walk(t, t.elem(v), &items, seen, gpu))
        }
        Ty::Struct(_) | Ty::Enum(_) if seen.contains(&ty) => None,
        Ty::Struct(s) => {
            seen.push(ty);
            t.strukt(s).fields.iter().find_map(|f| walk(t, f.ty, &format!("{path}.{}", f.name), seen, gpu))
        }
        Ty::Enum(e) => {
            seen.push(ty);
            t.enm(e).variants.iter().flat_map(|v| &v.fields).find_map(|f| walk(t, f.ty, &format!("{path}.{}", f.name), seen, gpu))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kestrel_syntax::parse;

    /// The fault of a parameter of type `ty` (written as in source) in a program that declares
    /// `decls` first, as (path, kind).
    fn fault(decls: &str, ty: &str) -> Option<(String, FaultKind)> {
        let src = format!("{decls}\nf(var x: {ty}) {{}}\nmain() {{}}\n");
        let (p, d) = parse(&src);
        assert!(d.is_empty(), "parse errors: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
        let (t, diags) = crate::check_keeping_tables(&p, true);
        assert!(diags.iter().all(|d| !d.is_error()), "unexpected errors: {:?}", diags.iter().map(|d| &d.message).collect::<Vec<_>>());
        let f = t.fns.iter().find(|f| f.name == "f").expect("`f`");
        let found = snapshot_fault(&t, f.params[0].ty, "x")?;
        assert_eq!(found.param, "x");
        Some((found.path, found.kind))
    }

    fn at(path: &str, kind: FaultKind) -> Option<(String, FaultKind)> {
        Some((path.to_string(), kind))
    }

    #[test]
    fn the_first_fault_is_found_depth_first() {
        // `inner.f` comes before `mem`, and the earlier field of `Inner` is plain.
        let decls = "struct Inner {
    n: int
    f: (int) -> int
}
struct Outer {
    a: int
    inner: Inner
    mem: Arena
}";
        assert_eq!(fault(decls, "Outer"), at("x.inner.f", FaultKind::Fn));
        // The earlier field wins, whichever kind it is.
        let decls = "struct Both {
    mem: Arena
    f: () -> int
}";
        assert_eq!(fault(decls, "Both"), at("x.mem", FaultKind::Arena));
        let decls = "struct Both {
    f: () -> int
    mem: Arena
}";
        assert_eq!(fault(decls, "Both"), at("x.f", FaultKind::Fn));
        // The items of a list, and its element's own fields.
        let decls = "struct World {
    n: int
    mem: Arena
}";
        assert_eq!(fault(decls, "list[World]"), at("x[i].mem", FaultKind::Arena));
        assert_eq!(fault("", "map[string, () -> int]"), at("x[i]", FaultKind::Fn));
    }

    #[test]
    fn every_kind_of_fault_is_named() {
        let decls = "interface Scene {
    draw(self)
}
struct S {
    scene: Scene
}
struct P {
    f: pure (int) -> int
}";
        assert_eq!(fault(decls, "S"), at("x.scene", FaultKind::Iface));
        assert_eq!(fault(decls, "P"), at("x.f", FaultKind::Fn));
        assert_eq!(fault("", "Arena"), at("x", FaultKind::Arena));
        assert_eq!(fault("", "(int) -> int"), at("x", FaultKind::Fn));
        // An optional keeps its optional's path; an enum's payload is by field name.
        let decls = "struct W {
    mem: Arena
}
enum E {
    A { n: int }
    B { w: W }
}
struct H {
    e: E?
}";
        assert_eq!(fault(decls, "H"), at("x.e.w.mem", FaultKind::Arena));
    }

    #[test]
    fn a_gpu_object_is_a_fault() {
        // (Only the built-in modules can write the type, so it's given to the walk directly.)
        let (p, _) = parse("main() {}");
        let (t, _) = crate::check_keeping_tables(&p, true);
        let found = snapshot_fault(&t, Ty::Gpu, "tex").expect("a fault");
        assert_eq!((found.param.as_str(), found.path.as_str(), found.kind), ("tex", "tex", FaultKind::Gpu));
        assert_eq!(found.kind.holds(), "a GPU object");
    }

    #[test]
    fn a_type_holding_itself_terminates() {
        let decls = "struct Node {
    kids: list[Node]
    f: () -> int
}";
        assert_eq!(fault(decls, "Node"), at("x.f", FaultKind::Fn));
        let decls = "struct Leaf {
    kids: list[Leaf]
    n: int
}";
        assert_eq!(fault(decls, "Leaf"), None);
    }

    #[test]
    fn plain_data_has_no_fault() {
        let decls = "enum Shape {
    Dot
    Circle { r: float }
    Rect { size: vec2, tag: string }
}
struct State {
    n: int
    big: i64
    b: u8
    ok: bool
    x: float
    y: f64
    v: vec2
    w: vec4
    iv: ivec4
    name: string
    xs: list[int]
    grid: [float]
    seen: map[string, int]
    maybe: int?
    shapes: list[Shape]
    shape: Shape
}";
        assert_eq!(fault(decls, "State"), None);
        assert_eq!(fault(decls, "list[State]"), None);
        assert_eq!(fault("", "int"), None);
    }

    /// `checksum`'s refusal of a parameter of type `ty` (written as in source) called `x`.
    fn refusal(decls: &str, ty: &str, root: Option<&str>) -> Option<(String, Option<String>)> {
        let src = format!("{decls}\nf(x: {ty}) {{}}\nmain() {{}}\n");
        let (p, _) = parse(&src);
        let (t, _) = crate::check_keeping_tables(&p, true);
        let f = t.fns.iter().find(|f| f.name == "f").expect("`f`");
        snapshot_refusal(&t, f.params[0].ty, root, "checksum", "checksum")
    }

    #[test]
    fn checksum_refuses_what_a_snapshot_cant_copy() {
        let decls = "struct Enemy {
    hp: int
    mem: Arena
}
struct Fighter {
    on_hit: pure (int) -> int
}";
        let (msg, help) = refusal(decls, "Enemy", Some("e")).expect("refused");
        assert_eq!(msg, "`checksum` can't take an `Enemy`, which holds an `Arena`; checksum the fields that hold the game's state instead");
        assert_eq!(help.as_deref(), Some("the `Arena` is `e.mem`"));
        // A value that isn't a name has no help line.
        let (msg, help) = refusal(decls, "list[Fighter]", None).expect("refused");
        let want = "`checksum` can't take a `list[Fighter]`, which holds a function value, whose address differs between builds; checksum the other fields instead";
        assert_eq!((msg.as_str(), help), (want, None));
        // The value itself.
        let (msg, help) = refusal("", "Arena", Some("mem")).expect("refused");
        assert_eq!((msg.as_str(), help), ("`checksum` can't take an `Arena`; checksum the values that hold the game's state instead", None));
        assert_eq!(refusal("", "() -> int", Some("f")).map(|r| r.0).as_deref(), Some("`checksum` can't take a function value, whose address differs between builds"));
        assert_eq!(refusal(decls, "map[string, list[int]]", Some("m")), None);
    }
}
