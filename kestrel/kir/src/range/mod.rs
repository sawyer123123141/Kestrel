//! Range analysis: proves which overflow, shift, division, narrowing and bounds checks can never
//! fail, marks them (`ExprKind::Proven`, `IndexCheck::Proven`), and says which checks are left and
//! why. It runs on KIR after `optimize`, one function at a time, interpreting each function
//! abstractly: a range for every tracked integer local and facts `a - b <= c` between terms
//! (`terms`, `facts`, `state`), one statement at a time (see `stmt`), conditions refining both
//! (`refine`), loops repeated to a fixed point (`loops`). See the range-analysis design spec.

mod arith;
mod assume;
mod bounds;
pub mod contracts;
mod eval;
pub mod events;
pub mod facts;
pub mod interval;
mod items;
mod loops;
mod mark;
mod par;
mod refine;
pub mod render;
pub mod report;
mod report_copies;
pub mod show;
pub mod sites;
pub mod state;
pub mod summary;
mod stmt;
mod vector;
pub mod terms;
pub mod tracked;
pub mod writes;

use std::collections::HashMap;

use crate::ir::*;
use crate::visit::{walk, Node};
use crate::visit_mut::VisitMut;
pub use assume::AssumeUse;
use assume::{Folded, RunLog};
use interval::Interval;
pub use mark::mark_assumptions;
use mark::Marker;
pub use sites::{Arith, CheckKind};
use sites::sites_of;
use state::State;
use tracked::Tracked;

/// The most items a list or an array holds (the runtime refuses more), so every `xs.len` of one
/// fits an `int`.
pub const MAX_ITEMS: i128 = 2_147_483_647;

/// How deep statements and expressions may nest before the analysis gives a function up.
const MAX_DEPTH: u32 = 256;

/// The work on facts a function may spend, per statement visit of its budget (see
/// `Tracked::saturated`).
const FACT_WORK: u64 = 64;

/// What range analysis found in a module: one entry per `Module::funcs` (empty for natives).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub funcs: Vec<FuncChecks>,
    /// Each `@assume` statement analysed (see `AssumeUse`), function by function.
    pub assumptions: Vec<AssumeUse>,
}

/// The checks left in one function, in the order the code runs them, and how many were removed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FuncChecks {
    pub left: Vec<CheckLeft>,
    pub removed: u32,
    /// The function nests too deeply or took too long to analyse: it was left exactly as it was,
    /// with every check in `left`.
    pub gave_up: bool,
    /// With `gave_up`: because the code nests too deeply (otherwise it took too many steps).
    pub deep: bool,
    /// A check site was never decided (a gap in the analysis's walk): it is kept and listed last.
    pub undecided: bool,
}

/// A check the analysis couldn't prove.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckLeft {
    pub kind: CheckKind,
    /// The packed line of the expression (see `pack_line`).
    pub line: u32,
    /// The packed lines of the loops around it, outermost first: a `ForRange`'s `start.line`, a
    /// `While`'s `cond.line`.
    pub loops: Vec<u32>,
    /// The `Stmt::Prove` blocks around it, outermost first (recorded with or without the texts,
    /// for contracts).
    pub regions: Vec<Region>,
    /// Why it may fail, worded for the report after the kind: "`a * b` may exceed `int` (`a` can be
    /// any `int`; `b` can be any `int`)".
    pub text: String,
}

/// Analyses every function with a body, marks the checks it proves, and returns what is left,
/// each check with the text that says why (for `kestrel check --report`). Records the build in
/// `Module::analysed`.
pub fn remove_checks(m: &mut Module, arith: Arith) -> Analysis {
    run(m, arith, true)
}

/// Marks the checks the analysis proves, as `remove_checks` does, without describing the ones
/// left: what a build needs. Their `text`s are empty and their `loops` aren't listed (their
/// `regions` are).
pub fn mark_proven(m: &mut Module, arith: Arith) -> Analysis {
    run(m, arith, false)
}

fn run(m: &mut Module, arith: Arith, texts: bool) -> Analysis {
    let mut a = Analysis { funcs: Vec::with_capacity(m.funcs.len()), assumptions: Vec::new() };
    let lists = par::lists(m);
    for i in 0..m.funcs.len() {
        if m.funcs[i].native.is_some() {
            a.funcs.push(FuncChecks::default());
            continue;
        }
        let (checks, proven, uses, has_assume) = analyse(m, &m.funcs[i], arith, lists.get(&i), texts);
        a.assumptions.extend(uses.into_iter().map(|u| u.into_use(FuncIdx(i as u32))));
        if !proven.is_empty() || has_assume {
            let trusted = mark::trusted(&a, i, arith);
            Marker { proven: &proven, trusted: &trusted }.visit_stmts(&mut m.funcs[i].body);
        }
        a.funcs.push(checks);
    }
    m.analysed = Some(arith);
    a
}

/// The statement visits a function may take over all passes: 200,000, or 100 per statement when
/// that is more; and whether it has an assumption. Each statement counts once more for every
/// `Assume` around it (its block runs once more in a deciding pass), and the budget grows in
/// proportion, so adding an assumption can't push a function past it (I4). A function without
/// one gets exactly the budget it had before assumptions existed.
fn budget(f: &Func) -> (u32, bool) {
    let mut statements: u64 = 0;
    let mut blocks: Vec<&[Stmt]> = Vec::new();
    walk(&f.body, &mut |n| {
        if let Node::Stmt(s) = n {
            statements += 1;
            if let Stmt::Assume { body, .. } = s {
                blocks.push(body);
            }
        }
    });
    let base = statements.saturating_mul(100).max(200_000);
    if blocks.is_empty() {
        return (base.min(u64::from(u32::MAX)) as u32, false);
    }
    let mut again: u64 = 0;
    for b in blocks {
        walk(b, &mut |n| {
            if let Node::Stmt(_) = n {
                again += 1;
            }
        });
    }
    let scaled = base.saturating_mul(statements + again) / statements.max(1);
    (scaled.min(u64::from(u32::MAX)) as u32, true)
}

/// A check site: an arithmetic node, or an index projection (`bounds`), by address. The two kinds
/// are kept apart because an index expression may share its projection's address.
type Key = (usize, bool);

fn expr_key(e: &Expr) -> Key {
    (e as *const Expr as usize, false)
}

fn index_key(p: &Proj) -> Key {
    (p as *const Proj as usize, true)
}

/// One function's checks, the sites it proved, its assumption statements' uses, and whether it
/// has an assumption.
type Analysed = (FuncChecks, HashMap<Key, ()>, Vec<Folded>, bool);

/// One function's checks, and the sites it proved.
fn analyse(m: &Module, f: &Func, arith: Arith, par_list: Option<&Place>, texts: bool) -> Analysed {
    let mut cx = Cx::new(m, f, arith, par_list);
    cx.texts = texts;
    let mut start = cx.top.clone();
    if let Some(list) = par_list {
        cx.par_entry(&mut start, list);
    }
    let _ = cx.stmts(start, &f.body);
    if cx.gave_up {
        let left = sites_of(f, arith).into_iter().map(|(_, s, regions)| s.left(m, f, texts, regions)).collect();
        let checks = FuncChecks { left, removed: 0, gave_up: true, deep: cx.deep, undecided: false };
        return (checks, HashMap::new(), Vec::new(), cx.has_assume);
    }
    let mut proven = HashMap::new();
    let (mut left, mut undecided) = (Vec::new(), false);
    for d in std::mem::take(&mut cx.order) {
        match d.left {
            None => {
                proven.insert(d.key, ());
            }
            Some(c) => left.push(c),
        }
    }
    // Every site is decided in some deciding pass; one that wasn't (a gap in the interpreter's
    // walk) keeps its check and is listed at the end. Its text is only made then.
    for (key, site, regions) in sites_of(f, arith) {
        if !cx.decisions.contains_key(&key) {
            let c = site.left(m, f, texts, regions);
            debug_assert!(false, "range analysis never decided a {:?} check in `{}`: {}", c.kind, f.name, c.text);
            left.push(c);
            undecided = true;
        }
    }
    let removed = proven.len() as u32;
    let uses = std::mem::take(&mut cx.uses);
    (FuncChecks { left, removed, gave_up: false, deep: false, undecided }, proven, uses, cx.has_assume)
}

/// What happened to one check site in the deciding passes.
struct Decision {
    key: Key,
    /// `None` when proven; otherwise what is left.
    left: Option<CheckLeft>,
}

/// A fact from a check that passed, waiting for its statement's step 4.
#[derive(Clone, Copy, Debug)]
enum Learn {
    /// The local's value is also in the range.
    Range(LocalIdx, Interval),
    /// `a - b <= c` between two fact nodes.
    Diff(usize, usize, i128),
}

/// The states that leave a loop's body other than by its end.
#[derive(Default)]
struct Frame {
    breaks: Option<State>,
    continues: Option<State>,
}

/// The interpreter of one function.
struct Cx<'a> {
    m: &'a Module,
    f: &'a Func,
    t: Tracked,
    arith: Arith,
    /// Whether every loop around the current point is in its deciding pass (checks are decided
    /// only then).
    deciding: bool,
    /// Whether a check left gets its text and loops (`remove_checks`), or not (`mark_proven`).
    texts: bool,
    /// More than 0 inside code the analysis found can't run: its checks stay, and nothing flows
    /// out of it.
    keep: u32,
    visits: u32,
    budget: u32,
    depth: u32,
    gave_up: bool,
    /// The reason it gave up was the nesting limit.
    deep: bool,
    /// The loops around the current point, innermost last.
    frames: Vec<Frame>,
    /// The packed lines of their headers.
    loop_lines: Vec<u32>,
    /// The `Prove` blocks around the current point, outermost first, and each block's number
    /// (made at the first one: see `prove_copies`).
    regions: Vec<Region>,
    copies: Option<HashMap<usize, u32>>,
    /// Facts from checks that passed on every path of the statements being analysed: each local's
    /// value is also in the range. A statement takes the ones its operands added (step 4).
    learned: Vec<Learn>,
    /// Where each place belongs (see `terms::Homes`).
    homes: terms::Homes,
    /// Each length term's place from its home, by its number in `Tracked::lens`.
    len_full: Vec<terms::Full>,
    /// The state at the function's start (and in code no state reaches).
    top: State,
    /// Each `ForRange`'s number, by address: its hidden end is `Term::Hidden` of it.
    hidden: HashMap<usize, u32>,
    /// The nest counters of the function and the claims of the nests being analysed (see
    /// `loops::Counters`).
    counters: loops::Counters,
    /// The nodes the expression being evaluated may write (see `within`), by node: a read of one
    /// may come before or after the write, so two reads of it aren't one value.
    volatile: Vec<bool>,
    decisions: HashMap<Key, usize>,
    order: Vec<Decision>,
    /// Whether the function has an `@assume` statement.
    has_assume: bool,
    /// The decisions of the assumption runs under way, innermost last (see `assume.rs`).
    logs: Vec<RunLog>,
    /// More than 0 inside a plain run: decisions go only to its log, not to the function's table.
    plain: u32,
    /// The uses of the assumption statements analysed so far, in the order first recorded.
    uses: Vec<Folded>,
}

impl<'a> Cx<'a> {
    fn new(m: &'a Module, f: &'a Func, arith: Arith, par_list: Option<&Place>) -> Self {
        let (picked, hidden_tys, hidden) = par::terms(m, f, par_list);
        let (budget, has_assume) = budget(f);
        let t = Tracked::of(f, &picked, &hidden_tys, u64::from(budget) * FACT_WORK);
        let par_body = m.par_bodies.iter().any(|b| std::ptr::eq(&m.funcs[b.0 as usize], f));
        let homes = terms::Homes::of(f, par_body);
        let len_full = t.lens.iter().map(|(k, _)| homes.key_full(k)).collect();
        let top = t.top();
        Cx {
            m,
            f,
            t,
            len_full,
            homes,
            top,
            hidden,
            counters: loops::Counters::default(),
            volatile: Vec::new(),
            arith,
            deciding: true,
            texts: true,
            keep: 0,
            visits: 0,
            budget,
            depth: 0,
            gave_up: false,
            deep: false,
            frames: Vec::new(),
            loop_lines: Vec::new(),
            regions: Vec::new(),
            copies: None,
            learned: Vec::new(),
            decisions: HashMap::new(),
            order: Vec::new(),
            has_assume,
            logs: Vec::new(),
            plain: 0,
            uses: Vec::new(),
        }
    }

    /// Enters a statement (`stmt`, counted against the budget) or an expression. `false` when the
    /// analysis has given the function up, now or before; otherwise the caller must `leave`.
    fn enter(&mut self, stmt: bool) -> bool {
        if self.gave_up {
            return false;
        }
        if stmt {
            self.visits += 1;
        }
        if self.visits > self.budget || self.depth >= MAX_DEPTH {
            self.gave_up = true;
            self.deep = self.depth >= MAX_DEPTH;
            return false;
        }
        self.depth += 1;
        true
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    /// Records the decision about a check site, in a deciding pass: proven when `safe` (outside
    /// unreachable code), otherwise left with `text()` (made only when the texts are wanted). A
    /// site decided twice is proven only if both decisions prove it. Inside an assumption's run
    /// it also goes to that run's log; inside a plain run only there.
    fn decide(&mut self, key: Key, kind: CheckKind, line: u32, safe: bool, text: impl FnOnce(&Self) -> String) {
        if !self.deciding {
            return;
        }
        let proven = safe && self.keep == 0;
        if let Some(log) = self.logs.last_mut() {
            assume::note(log, key, proven);
        }
        if self.plain > 0 {
            return;
        }
        let left = (!proven).then(|| {
            let regions = self.regions.clone();
            match self.texts {
                true => CheckLeft { kind, line, loops: self.loop_lines.clone(), regions, text: text(self) },
                false => CheckLeft { kind, line, loops: Vec::new(), regions, text: String::new() },
            }
        });
        match self.decisions.get(&key) {
            Some(&i) => {
                if self.order[i].left.is_none() {
                    self.order[i].left = left;
                }
            }
            None => {
                self.decisions.insert(key, self.order.len());
                self.order.push(Decision { key, left });
            }
        }
    }
}
