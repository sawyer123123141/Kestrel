//! Which list places a loop can't change: the aliasing rule of the performance round's decision 3
//! (`docs/superpowers/specs/2026-11-01-perf-round-design.md`).
//!
//! A candidate is a place `P` made of a root local `R` and field steps only, of list or array
//! type, that the loop reads an item of (`P[i]`, a `for ... in P` item pointer), takes the length
//! of, or adds to. Its header, `{data, len, cap}`, lives in its owner's memory: a stack slot, a
//! parameter's memory, or the buffer of an outer list when lists nest; never in `P`'s own buffer.
//! So a store to an item of `P`, or of any list, never changes `P`'s header. Only these can, and
//! any of them in the loop (its condition, its body, nested loops, and statements inside
//! expressions) stops `P` being cached:
//!
//! - (a) a statement that writes `P`, a prefix of `P`, or `P` extended by fields: an assignment, an
//!   `add` (allowed for `P` itself only, which then keeps its `cap` too and is "add-cached"), a
//!   list operation (`clear`, `insert`, `remove_at`, a sort, `Reserve` ...), `SetLen`, `Dup`,
//!   `CopyItems` into it, a map operation, a move out of it. Every place the KIR walker reports as
//!   written counts.
//! - (b) a `Let`, `Bind`, `BindEnv` or `BindGlobal` of `R` (a loop element is bound again each
//!   time round), or of any local `R` is bound through (see below).
//! - (c) a call given, as `var`, a place that reaches `P`'s header: the walker reports a `var`
//!   argument as written, so (a) covers it. A place that leaves `P`'s path at another field of a
//!   struct, or passes through an index of `P` first (a callee given `P[i]` gets an item pointer),
//!   doesn't reach the header.
//! - (d) any call through a function value or an interface, `par for`, and any closure or
//!   interface value made or copied: such a value could hold `R`.
//!
//! Another name can reach the same header: a reference local bound (`Bind`) to a place. Places are
//! compared through their bindings: a reference bound to fields of `S` names `S`'s memory, so a
//! write through it is compared with `P` as a place of `S`; one bound through an index names
//! memory inside an item of a list, which holds no header of a field-only place. A reference that
//! is `R` itself is only cached when its `Bind` runs before the loop on every way to it.
//!
//! Reads of `P`'s header by anything else in the loop (a read-only argument, a copy of `P`, its
//! capacity) read memory, which stays current: `data` and `cap` change only when an add grows
//! the list, which reloads them, and every add stores the new length to memory at once (write-
//! through).

use std::collections::{HashMap, HashSet};

use kestrel_kir::visit::{walk, walk_expr, Node};
use kestrel_kir::{Expr, ExprKind, LocalIdx, Place, Proj, Stmt};

/// More lists added to in one loop than this caches none of the added ones (their `len` and `cap`
/// would crowd out the registers the loop's own work needs).
pub(super) const ADD_MAX: usize = 3;

/// A list place the loop uses: its root, its fields, and whether the loop adds to it.
pub(super) struct Candidate {
    pub root: LocalIdx,
    pub fields: Vec<u32>,
    pub add: bool,
}

/// What a loop does that matters to its lists' headers.
#[derive(Default)]
pub(super) struct Scan<'a> {
    pub candidates: Vec<Candidate>,
    /// Every place the loop writes, and whether it is the target of an `add`.
    writes: Vec<(&'a Place, bool)>,
    /// The `add` targets, by address, so their place is told apart from other writes.
    adds: HashSet<*const Place>,
    /// Locals the loop binds or makes: (b).
    pub bound: HashSet<LocalIdx>,
    /// A call through a function value or interface, a `par for`, or a closure or interface value
    /// made or copied: (d).
    pub opaque: bool,
}

/// The fields of a path made only of fields.
pub(super) fn fields_only(path: &[Proj]) -> Option<Vec<u32>> {
    path.iter().map(|p| if let Proj::Field(f) = p { Some(*f) } else { None }).collect()
}

/// The fields before a path's first step, when that step is an index and every step before it
/// a field: the list place the index reads an item of.
fn fields_before_index(path: &[Proj]) -> Option<Vec<u32>> {
    let k = path.iter().position(|p| !matches!(p, Proj::Field(_)))?;
    matches!(path[k], Proj::Index { .. }).then(|| fields_only(&path[..k]).expect("only fields before"))
}

/// Whether one field path is a prefix of the other (equal included).
fn overlap(a: &[u32], b: &[u32]) -> bool {
    let n = a.len().min(b.len());
    a[..n] == b[..n]
}

impl<'a> Scan<'a> {
    /// Scans a loop's condition (`None` for a `for` range) and body.
    pub(super) fn new(cond: Option<&'a Expr>, body: &'a [Stmt]) -> Self {
        let mut scan = Scan::default();
        if let Some(c) = cond {
            walk_expr(c, &mut |n| scan.node(n));
        }
        walk(body, &mut |n| scan.node(n));
        scan
    }

    fn note(&mut self, root: LocalIdx, fields: Vec<u32>, add: bool) {
        match self.candidates.iter_mut().find(|c| c.root == root && c.fields == fields) {
            Some(c) => c.add |= add,
            None => self.candidates.push(Candidate { root, fields, add }),
        }
    }

    fn node(&mut self, n: Node<'a>) {
        match n {
            Node::Stmt(s) => match s {
                // The statement comes before its place, so the place is known as an add's.
                Stmt::ListAdd(p, _) => {
                    self.adds.insert(p as *const Place);
                    if let Some(f) = fields_only(&p.path) {
                        self.note(p.root, f, true);
                    }
                }
                Stmt::Let(l, _) | Stmt::Bind(l, _) | Stmt::BindGlobal(l, _) | Stmt::BindEnv { local: l, .. } | Stmt::ForRange { var: l, .. } => {
                    self.bound.insert(*l);
                }
                Stmt::CallIndirect { .. } | Stmt::CallVirtual { .. } | Stmt::ParFor { .. } | Stmt::CopyFnEnv(_) | Stmt::CopyIface(_) => {
                    self.opaque = true;
                }
                _ => {}
            },
            Node::Expr(e) => match &e.kind {
                ExprKind::Len(p) => {
                    if let Some(f) = fields_only(&p.path) {
                        self.note(p.root, f, false);
                    }
                }
                ExprKind::CallIndirect { .. } | ExprKind::CallVirtual { .. } | ExprKind::MakeFn { .. } | ExprKind::MakeIface { .. } => {
                    self.opaque = true;
                }
                _ => {}
            },
            Node::Place(p, written) => {
                if let Some(f) = fields_before_index(&p.path) {
                    self.note(p.root, f, false);
                }
                if written {
                    self.writes.push((p, self.adds.contains(&(p as *const Place))));
                }
            }
        }
    }

    /// The roots of the candidates and of the places the loop writes.
    pub(super) fn roots(&self) -> impl Iterator<Item = LocalIdx> + '_ {
        self.candidates.iter().map(|c| c.root).chain(self.writes.iter().map(|(w, _)| w.root))
    }

    /// Whether the loop can change `c`'s header other than by its own adds. `binds` gives each
    /// reference local's bound places in the whole function.
    pub(super) fn changes(&self, c: &Candidate, binds: &Binds) -> bool {
        let chain = binds.chain(c.root);
        if chain.iter().any(|l| self.bound.contains(l)) {
            return true;
        }
        let homes = binds.homes(c.root, &c.fields);
        self.writes.iter().any(|&(w, is_add)| {
            if w.root == c.root {
                // The same local: compare the paths themselves.
                return match fields_only(&w.path) {
                    Some(wf) => !(is_add && c.add && wf == c.fields) && overlap(&wf, &c.fields),
                    // Through an index (an item) or into a variant, payload, map or lane: no
                    // field-only header of this local is there.
                    None => false,
                };
            }
            let whomes = binds.homes(w.root, &field_steps(&w.path));
            homes.iter().any(|h| whomes.iter().any(|wh| h.reaches(wh, w.path.len() != field_steps(&w.path).len())))
        })
    }
}

/// The leading field steps of a path.
fn field_steps(path: &[Proj]) -> Vec<u32> {
    path.iter().map_while(|p| if let Proj::Field(f) = p { Some(*f) } else { None }).collect()
}

/// Where a place's memory is: a local that no `Bind` points elsewhere, the fields from it, and
/// whether those fields are the whole way (false when the way passes through an index or another
/// non-field step, so the memory is inside an item, a payload or a map entry).
#[derive(Clone)]
pub(super) struct Home {
    root: LocalIdx,
    fields: Vec<u32>,
    exact: bool,
}

impl Home {
    /// Whether a write to `w` (`deeper`: the written place's own path goes past its fields) can
    /// change the header at `self`.
    fn reaches(&self, w: &Home, deeper: bool) -> bool {
        if self.root != w.root {
            return false;
        }
        if !self.exact {
            // A header inside an item of a list of `root`: any other write into `root` may be it.
            return true;
        }
        // Inside an item, a payload or a map entry: no field-only header is there.
        w.exact && !deeper && overlap(&self.fields, &w.fields)
    }
}

/// Each reference local's bound places in a function (`Bind`); locals bound by `BindEnv` or
/// `BindGlobal` have memory of their own, which no other local names.
#[derive(Default)]
pub(super) struct Binds<'a> {
    of: HashMap<LocalIdx, Vec<&'a Place>>,
}

/// How many bindings deep a place is followed before every write counts as reaching it.
const DEPTH: usize = 16;

impl<'a> Binds<'a> {
    pub(super) fn new(body: &'a [Stmt]) -> Self {
        let mut of: HashMap<LocalIdx, Vec<&'a Place>> = HashMap::new();
        walk(body, &mut |n| {
            if let Node::Stmt(Stmt::Bind(l, p)) = n {
                of.entry(*l).or_default().push(p);
            }
        });
        Binds { of }
    }

    /// `l` and every local it is bound through.
    fn chain(&self, l: LocalIdx) -> Vec<LocalIdx> {
        let mut out = vec![l];
        let mut todo = vec![l];
        while let Some(x) = todo.pop() {
            for p in self.of.get(&x).into_iter().flatten() {
                if !out.contains(&p.root) && out.len() < DEPTH {
                    out.push(p.root);
                    todo.push(p.root);
                }
            }
        }
        out
    }

    /// Where the memory at `root` and then `fields` can be, through every binding of `root`.
    fn homes(&self, root: LocalIdx, fields: &[u32]) -> Vec<Home> {
        let mut out = Vec::new();
        self.home_into(root, fields.to_vec(), true, 0, &mut out);
        out
    }

    fn home_into(&self, root: LocalIdx, fields: Vec<u32>, exact: bool, depth: usize, out: &mut Vec<Home>) {
        let Some(sources) = self.of.get(&root) else {
            out.push(Home { root, fields, exact });
            return;
        };
        if depth == DEPTH {
            // Too deep to follow: as though it could be anywhere in `root`'s memory.
            out.push(Home { root, fields: Vec::new(), exact: false });
            return;
        }
        for q in sources {
            let lead = field_steps(&q.path);
            let through = lead.len() == q.path.len();
            let mut f = lead;
            if through && exact {
                f.extend(&fields);
            }
            self.home_into(q.root, f, exact && through, depth + 1, out);
        }
    }
}

/// Whether `target`, a loop's body, is inside `list`; if so, `acc` holds the locals bound
/// (`Bind`, `BindEnv`, `BindGlobal`) before the loop on every way to it: earlier in a statement
/// list that holds the loop or a statement holding it.
pub(super) fn bound_before(list: &[Stmt], target: &[Stmt], acc: &mut Vec<LocalIdx>) -> bool {
    let mark = acc.len();
    for s in list {
        let found = match s {
            Stmt::While { body, .. } | Stmt::ForRange { body, .. } => std::ptr::eq(body.as_slice(), target) || bound_before(body, target, acc),
            Stmt::If { then_body, else_body, .. } => bound_before(then_body, target, acc) || bound_before(else_body, target, acc),
            Stmt::Assume { body, .. } | Stmt::Prove { body, .. } => bound_before(body, target, acc),
            _ => false,
        };
        if found {
            return true;
        }
        if let Stmt::Bind(l, _) | Stmt::BindEnv { local: l, .. } | Stmt::BindGlobal(l, _) = s {
            acc.push(*l);
        }
    }
    acc.truncate(mark);
    false
}
