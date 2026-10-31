//! The report behind `kestrel check --report`: for each function (and loop in it) every check
//! range analysis left, every allocation, every other way to fail and every call that wasn't
//! inlined, each with its line and why, followed through calls (`summary`). `render` writes it as
//! text. Round B's contracts (`no_alloc`, `no_panic`, `inline`, checks) read this same data.

use std::collections::HashMap;

use super::events::{self, Event};
use super::items;
use super::sites::CheckKind;
use super::summary::{self, Summary};
use super::Analysis;
use crate::ir::*;
use crate::visit::{walk, Node};

/// Everything the report says about a module.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// The functions of the program (a program's own, and the built-in modules'), by file and
    /// line. Generated functions have no entry: their effects are in the calls to them.
    pub funcs: Vec<FuncReport>,
    /// What every function of the module (`Module::funcs`) allocates, how it can fail and the
    /// checks left in it and what it calls.
    pub summaries: Vec<Summary>,
    /// Totals over `funcs`: every check (left and removed), the ones removed, the ones left, the
    /// allocations and the other ways to fail.
    pub checks: u32,
    pub removed: u32,
    pub left: u32,
    pub allocations: u32,
    pub fails: u32,
}

/// One function's part of the report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FuncReport {
    pub func: FuncIdx,
    pub name: String,
    /// The packed first and last lines of the declaration (`Func::lines`).
    pub lines: (u32, u32),
    /// Nothing calls its own compiled copy: every call to it was inlined, so its checks are listed
    /// where it was inlined (and `items` is left empty).
    pub never_called: bool,
    /// Range analysis stopped early in it (or in a closure inside it): every check in it stays.
    pub gave_up: bool,
    /// With `gave_up`: why, as a clause ("its code nests too deeply").
    pub stopped: String,
    /// How many checks range analysis removed from it.
    pub removed: u32,
    /// The items, in the order the report prints them: by line, loops grouped.
    pub items: Vec<Item>,
    /// A `par for` body, with its own items, keyed by the packed line of the `par for` (its
    /// count). Its `loops` are the loops the `par for` is in.
    pub par_bodies: Vec<(u32, FuncReport)>,
    /// For a `par for` body: the packed lines of the loops around its `par for`.
    pub loops: Vec<u32>,
}

/// One line of the report.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The packed line of the code (see `pack_line`): in the function's file or, for an inlined
    /// body, in the function it came from.
    pub line: u32,
    /// The packed lines of the loops around it, outermost first (a `for`'s start, a `while`'s
    /// condition).
    pub loops: Vec<u32>,
    pub kind: ItemKind,
    /// The sentence after the kind: "`s + xs[i]` may exceed `int` (`s` can be any `int`)".
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Check(CheckKind),
    Allocation,
    CanFail,
    /// A call that wasn't inlined, and why.
    Call,
}

impl FuncReport {
    fn count(&self, f: &impl Fn(&Item) -> bool) -> u32 {
        let own = self.items.iter().filter(|i| f(i)).count() as u32;
        own + self.par_bodies.iter().map(|(_, b)| b.count(f)).sum::<u32>()
    }

    /// The checks left in it, `par for` bodies included.
    pub fn checks_left(&self) -> u32 {
        self.count(&|i| matches!(i.kind, ItemKind::Check(_)))
    }

    /// The checks range analysis removed from it, `par for` bodies included.
    pub fn checks_removed(&self) -> u32 {
        self.removed + self.par_bodies.iter().map(|(_, b)| b.checks_removed()).sum::<u32>()
    }

    pub fn allocations(&self) -> u32 {
        self.count(&|i| i.kind == ItemKind::Allocation)
    }

    pub fn fails(&self) -> u32 {
        self.count(&|i| i.kind == ItemKind::CanFail)
    }
}

/// What a function is for the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    /// A native: its body is Rust.
    Native,
    /// The body of a `par for`.
    ParBody,
    /// A closure literal's code.
    Closure,
    /// Made by the compiler (a copy, a thunk, the loop behind `filter`): no lines of its own.
    Generated,
    /// A function the program declares.
    User,
}

pub(super) fn classes(m: &Module) -> Vec<Class> {
    let bodies: std::collections::HashSet<u32> = m.par_bodies.iter().map(|b| b.0).collect();
    m.funcs
        .iter()
        .enumerate()
        .map(|(i, f)| {
            if f.native.is_some() {
                Class::Native
            } else if bodies.contains(&(i as u32)) {
                Class::ParBody
            } else if f.name.starts_with("__kestrel_closure_") {
                Class::Closure
            } else if f.lines == (0, 0) || f.name.starts_with("__kestrel_") {
                Class::Generated
            } else {
                Class::User
            }
        })
        .collect()
}

/// The function the heap constants are built before: `main`, or a reloadable program's `setup`
/// (the host builds them before it calls `setup`; the `main` the compiler makes for everything else
/// is generated code, which the report leaves out). The report lists what building them does under
/// this function, and its `@prove` contracts count it.
pub(super) fn constants_home(m: &Module) -> Option<FuncIdx> {
    if m.reload.is_some() {
        // The checker makes sure it is the entry file's `pub setup` (a module's is `module.setup`).
        return m.funcs.iter().position(|f| f.name == "setup" && f.exported).map(|i| FuncIdx(i as u32));
    }
    m.main
}

/// Whether the compiler made function `g` (it has no lines of its own).
pub fn is_generated(m: &Module, g: FuncIdx) -> bool {
    m.funcs[g.0 as usize].lines == (0, 0)
}

/// Whether the function named `name` is a sort's generated comparison or key function, which calls
/// the program's key (both read as "the comparison for `sort`").
pub fn is_sort_comparison(name: &str) -> bool {
    name.starts_with("__kestrel_compare_") || name.starts_with("__kestrel_sort_key_")
}

/// The built-in a generated function (named `__kestrel_list_fn_filter_7`) implements.
pub fn builtin_name(function_name: &str) -> Option<&'static str> {
    const NAMES: [(&str, &str); 10] = [
        ("__kestrel_list_fn_filter_", "filter"),
        ("__kestrel_list_fn_map_", "map"),
        ("__kestrel_list_fn_any_", "any"),
        ("__kestrel_list_fn_all_", "all"),
        ("__kestrel_sort_by_", "sort_by"),
        ("__kestrel_to_bytes_", "to_bytes"),
        ("__kestrel_from_bytes_", "from_bytes"),
        ("__kestrel_bytes_read_", "from_bytes"),
        ("__kestrel_first_difference_", "first_difference"),
        ("__kestrel_diff_", "first_difference"),
    ];
    NAMES.iter().find(|(prefix, _)| function_name.starts_with(prefix)).map(|&(_, name)| name)
}

/// A function's name as the report writes it: a module's function without the module's name
/// (`geom.helper` is `helper` in `geom.kes`).
pub fn display_name(m: &Module, g: FuncIdx) -> String {
    let f = &m.funcs[g.0 as usize];
    super::report_copies::plain_name(m, &f.name, f.lines.0)
}

/// A function as a chain names it: "`spawn`", "the closure at line 8", "`filter`", "assigning a
/// `Game`" (a storage-reusing assignment's helper, named by the type it assigns).
pub fn callee_name(m: &Module, g: FuncIdx) -> String {
    let f = &m.funcs[g.0 as usize];
    if f.name.starts_with("__kestrel_closure_") {
        return format!("the closure at line {}", unpack_line(f.lines.0).0);
    }
    if f.name.starts_with("__kestrel_assign_") {
        return format!("assigning a `{}`", super::events::elem_text(m, f.locals[0].ty));
    }
    if m.par_bodies.contains(&g) {
        return "the `par for` body".to_string();
    }
    if is_sort_comparison(&f.name) {
        return "the comparison for `sort`".to_string();
    }
    match builtin_name(&f.name) {
        Some(name) => format!("`{name}`"),
        None => format!("`{}`", display_name(m, g)),
    }
}

pub use super::report_copies::Owners;

/// Builds the report of module `m`, which `remove_checks` has just analysed (`a`) in the build
/// mode `arith`.
pub fn build(m: &Module, a: &Analysis) -> Report {
    let classes = classes(m);
    let events: Vec<Vec<Event>> = m.funcs.iter().map(|f| if f.native.is_some() { Vec::new() } else { events::events(m, f) }).collect();
    let own_checks: Vec<Vec<u64>> = a.funcs.iter().zip(&classes).map(|(c, class)| check_ids(c, *class == Class::Generated)).collect();
    let summaries = summary::summarize(m, a, &events, Some(&own_checks));
    let mut cx = items::Cx::new(m, a, &classes, &events, &summaries);
    let called = called(m);
    let makers = super::report_copies::makers(m);
    let made: Vec<bool> = makers.iter().map(|v| !v.is_empty()).collect();
    let owners = Owners::new(m);
    // The closures inside each user function.
    let mut closures: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut loose: Vec<usize> = Vec::new();
    for (j, class) in classes.iter().enumerate() {
        // (A closure whose literal is gone, inlined into its loop, is dead code: its live copy is
        // in the function that holds the loop.)
        if *class == Class::Closure && made[j] {
            match owners.closure_host(m, &classes, &makers, j) {
                Some(o) => closures.entry(o.0 as usize).or_default().push(j),
                None => loose.push(j),
            }
        }
    }
    let inlined = inlined_into_others(m, &classes, &called, &made);
    // A user's `main` lists the constants through its own code (lowering puts the call to `init`
    // first in it, and the inliner expands it); a reloadable program's `setup` lists `init` itself.
    let constants = match (constants_home(m), m.init) {
        (Some(home), Some(init)) if Some(home) != m.main => Some((home.0 as usize, init.0 as usize)),
        _ => None,
    };
    let mut funcs = Vec::new();
    for (i, class) in classes.iter().enumerate() {
        if *class != Class::User {
            continue;
        }
        let mut with = vec![i];
        with.extend(closures.get(&i).into_iter().flatten().copied());
        with.extend(constants.filter(|&(home, _)| home == i).map(|(_, init)| init));
        // (Every copy of a generic has the same lines, so code can't be traced to one: a copy
        // nothing refers to has been inlined everywhere.)
        let is_copy = m.copies.contains_key(&FuncIdx(i as u32));
        let never_called = !called[i] && (is_copy || inlined.contains(&i)) && with.len() == 1;
        let mut fr = cx.func_report(i, &with);
        if never_called {
            // Its checks are listed where it was inlined, and counted there.
            fr = FuncReport { never_called: true, items: Vec::new(), par_bodies: Vec::new(), removed: 0, ..fr };
        }
        funcs.push(fr);
    }
    for j in loose {
        let mut fr = cx.func_report(j, &[j]);
        fr.name = format!("closure at line {}", unpack_line(m.funcs[j].lines.0).0);
        if !fr.items.is_empty() || fr.removed > 0 || fr.gave_up {
            funcs.push(fr);
        }
    }
    funcs.sort_by_key(|f| (unpack_line(f.lines.0).1, unpack_line(f.lines.0).0));
    let mut r = Report { summaries, ..Report::default() };
    let mut seen = std::collections::HashSet::new();
    for f in &funcs {
        r.add(f, &mut seen);
    }
    r.funcs = funcs;
    r
}

impl Report {
    /// Adds a function's counts to the totals (a `par for` body shared by several functions once).
    fn add(&mut self, f: &FuncReport, seen: &mut std::collections::HashSet<FuncIdx>) {
        let own = |kind: fn(&Item) -> bool| f.items.iter().filter(|i| kind(i)).count() as u32;
        let left = own(|i| matches!(i.kind, ItemKind::Check(_)));
        self.left += left;
        self.removed += f.removed;
        self.checks += left + f.removed;
        self.allocations += own(|i| i.kind == ItemKind::Allocation);
        self.fails += own(|i| i.kind == ItemKind::CanFail);
        for (_, b) in &f.par_bodies {
            if seen.insert(b.func) {
                self.add(b, seen);
            }
        }
    }
}

/// For each function, whether anything refers to it other than an inlined copy: a call, a
/// function value, a method table, a `par for`, the program's entry points, or a host.
fn called(m: &Module) -> Vec<bool> {
    let mut called = vec![false; m.funcs.len()];
    let mut mark = |g: FuncIdx| called[g.0 as usize] = true;
    for f in &m.funcs {
        walk(&f.body, &mut |n| match n {
            Node::Expr(e) => match &e.kind {
                ExprKind::Call(g, _) | ExprKind::FnRef(g) => mark(*g),
                ExprKind::MakeFn { code, env } => {
                    mark(*code);
                    if let EnvKind::Heap(env) | EnvKind::Stack(env) = env {
                        mark(env.copy);
                    }
                }
                _ => {}
            },
            Node::Stmt(Stmt::Call(g, _, _)) => mark(*g),
            Node::Stmt(Stmt::ParFor { func, .. }) => mark(*func),
            _ => {}
        });
    }
    for t in &m.tables {
        mark(t.copy);
        mark(t.print);
        t.methods.iter().for_each(|&g| mark(g));
    }
    m.print_fns.values().for_each(|&g| mark(g));
    m.main.into_iter().chain(m.init).for_each(&mut mark);
    for (i, f) in m.funcs.iter().enumerate() {
        if f.exported {
            called[i] = true;
        }
    }
    called
}

/// The user functions, not called, that code in some other function came from: those whose lines
/// appear in another function's body.
fn inlined_into_others(m: &Module, classes: &[Class], called: &[bool], made: &[bool]) -> std::collections::HashSet<usize> {
    let mut spans: Vec<(u32, u32, usize)> = (0..m.funcs.len())
        .filter(|&i| classes[i] == Class::User && !called[i] && !m.copies.contains_key(&FuncIdx(i as u32)))
        .map(|i| (m.funcs[i].lines.0, m.funcs[i].lines.1, i))
        .collect();
    spans.sort();
    let mut found = std::collections::HashSet::new();
    if spans.is_empty() {
        return found;
    }
    for (g, f) in m.funcs.iter().enumerate() {
        // Only code the report lists or follows counts: not a compiler-made function (a sort's
        // comparison), whose copy of a function is listed nowhere.
        let listed = match classes[g] {
            Class::User | Class::ParBody => true,
            Class::Closure => made[g],
            Class::Generated | Class::Native => false,
        };
        if !listed {
            continue;
        }
        walk(&f.body, &mut |n| {
            if let Node::Expr(e) = n {
                if e.line == 0 {
                    return;
                }
                let at = spans.partition_point(|s| s.0 <= e.line);
                if let Some(&(_, hi, i)) = at.checked_sub(1).map(|k| &spans[k]) {
                    if e.line <= hi && i != g {
                        found.insert(i);
                    }
                }
            }
        });
    }
    found
}


/// A number for each check left in a function, the same for the copies inlining makes of it in
/// another function: its line, its kind and which of that kind it is on its line. (A compiler-made
/// function, like a sort's comparison, holds several copies of the same key: they are all one.)
fn check_ids(c: &super::FuncChecks, collapse: bool) -> Vec<u64> {
    use std::hash::{Hash, Hasher};
    let mut seen: HashMap<(u32, u8), u32> = HashMap::new();
    c.left
        .iter()
        .map(|k| {
            let n = seen.entry((k.line, k.kind as u8)).or_default();
            let ordinal = if collapse { 0 } else { *n };
            *n += 1;
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (k.line, k.kind as u8, ordinal).hash(&mut h);
            h.finish()
        })
        .collect()
}
