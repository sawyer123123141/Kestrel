//! Makes the report's items: the checks the analysis left, and the events of the walk (with the
//! calls followed through the summaries), put in the order the report prints them.

use std::collections::HashMap;

use super::events::{Event, What};
use super::report::{callee_name, Class, FuncReport, Item, ItemKind, Owners};
use super::summary::{chain_text, reason_text, Part, Summary};
use super::Analysis;
use crate::ir::*;

/// A call the inliner left: where, why, and whether a call of the final code has taken it.
struct LeftCall {
    line: u32,
    reason: String,
    used: bool,
}

/// The calls the inliner left, by (caller, callee).
type LeftCalls = HashMap<(u32, u32), Vec<LeftCall>>;

pub(super) struct Cx<'a> {
    m: &'a Module,
    a: &'a Analysis,
    classes: &'a [Class],
    events: &'a [Vec<Event>],
    summaries: &'a [Summary],
    owners: Owners,
    left_calls: LeftCalls,
}

impl<'a> Cx<'a> {
    pub fn new(m: &'a Module, a: &'a Analysis, classes: &'a [Class], events: &'a [Vec<Event>], summaries: &'a [Summary]) -> Self {
        let mut left_calls = LeftCalls::new();
        for n in &m.not_inlined {
            left_calls.entry((n.caller.0, n.callee.0)).or_default().push(LeftCall { line: n.line, reason: n.reason.clone(), used: false });
        }
        Cx { m, a, classes, events, summaries, owners: Owners::new(m), left_calls }
    }

    /// The report of user function `home`, with the closures `with[1..]` inside it (and, for a
    /// reloadable program's `setup`, `init`, which builds the constants before it).
    pub fn func_report(&mut self, home: usize, with: &[usize]) -> FuncReport {
        let f = &self.m.funcs[home];
        let name = super::report::display_name(self.m, FuncIdx(home as u32));
        let mut report = FuncReport { func: FuncIdx(home as u32), name, lines: f.lines, ..FuncReport::default() };
        let first_line = f.lines.0;
        let (mut checks, mut others) = (Vec::new(), Vec::new());
        for &g in with {
            let c = &self.a.funcs[g];
            report.removed += c.removed;
            if c.gave_up && !report.gave_up {
                report.gave_up = true;
                report.stopped = if c.deep { "its code nests too deeply" } else { "it took too long to analyse" }.to_string();
            }
            let (mut own_checks, mut own_others) = (self.checks_of(g), self.events_of(g));
            // What `init`'s own code does (the arena) is at the first line of the function it runs
            // before, as it is when inlined into a `main`.
            if self.m.init == Some(FuncIdx(g as u32)) {
                for item in own_checks.iter_mut().chain(own_others.iter_mut()).filter(|i| i.line == 0) {
                    item.line = first_line;
                }
            }
            checks.extend(own_checks);
            others.extend(own_others);
            self.bodies(g, FuncIdx(home as u32), &mut report.par_bodies);
        }
        report.items = self.arrange(Some(FuncIdx(home as u32)), checks, others);
        report
    }

    /// The `par for` bodies function `g` runs, with their own items.
    fn bodies(&mut self, g: usize, host: FuncIdx, out: &mut Vec<(u32, FuncReport)>) {
        let events = self.events;
        for e in &events[g] {
            let What::Body(b) = e.what else { continue };
            let b = b.0 as usize;
            let mut body = FuncReport { func: FuncIdx(b as u32), name: "par for".to_string(), lines: self.m.funcs[b].lines, loops: e.loops.clone(), ..FuncReport::default() };
            let c = &self.a.funcs[b];
            body.removed = c.removed;
            body.gave_up = c.gave_up;
            if c.gave_up {
                body.stopped = if c.deep { "its code nests too deeply" } else { "it took too long to analyse" }.to_string();
            }
            let (mut checks, mut others) = (self.checks_of(b), self.events_of(b));
            for item in checks.iter_mut().chain(others.iter_mut()) {
                if item.line == 0 {
                    item.line = e.line;
                }
                // The loop over the thread's chunk is the `par for` itself.
                if item.loops.first() == Some(&e.line) {
                    item.loops.remove(0);
                }
            }
            self.bodies(b, host, &mut body.par_bodies);
            body.items = self.arrange(Some(host), checks, others);
            out.push((e.line, body));
        }
    }

    /// The checks the analysis left in function `g`, in the order the code runs them.
    fn checks_of(&self, g: usize) -> Vec<Item> {
        self.a.funcs[g]
            .left
            .iter()
            .map(|c| Item { line: c.line, loops: c.loops.iter().copied().filter(|&l| l != 0).collect(), kind: ItemKind::Check(c.kind), text: c.text.clone() })
            .collect()
    }

    /// The allocations, ways to fail and calls of function `g`, with the calls followed.
    fn events_of(&mut self, g: usize) -> Vec<Item> {
        let mut out = Vec::new();
        let events = self.events;
        for e in &events[g] {
            let home = unpack_line(e.line).1;
            let mut item = |kind: ItemKind, text: String| out.push(Item { line: e.line, loops: e.loops.clone(), kind, text });
            match &e.what {
                What::Alloc(r) => item(ItemKind::Allocation, reason_text(self.m, r, home)),
                // (A logic part is printed with the resource part after it, whose words say both.)
                What::Fail(r) if r.part == Part::Logic => {}
                What::Fail(r) => item(ItemKind::CanFail, reason_text(self.m, r, home)),
                What::Unknown(kind, r) => {
                    if kind.allocates() {
                        item(ItemKind::Allocation, reason_text(self.m, r, home));
                    }
                    if kind.fails() {
                        item(ItemKind::CanFail, reason_text(self.m, r, home));
                    }
                }
                // (The report doesn't list what breaks `deterministic`.)
                What::Body(_) | What::Closure(_) | What::Cut | What::Effect(..) => {}
                What::Call(callee) => {
                    let c = callee.0 as usize;
                    let summary = &self.summaries[c];
                    if let Some(chain) = &summary.allocates {
                        item(ItemKind::Allocation, chain_text(self.m, *callee, chain, e.line));
                    }
                    if let Some(chain) = &summary.fails {
                        item(ItemKind::CanFail, chain_text(self.m, *callee, chain, e.line));
                    }
                    if matches!(self.classes[c], Class::User | Class::Closure) {
                        let why = Self::reason_for(&mut self.left_calls, g, c, e.line);
                        let checks = match summary.checks_left {
                            0 => String::new(),
                            1 => " (it and what it calls have 1 check left)".to_string(),
                            n => format!(" (it and what it calls have {n} checks left)"),
                        };
                        item(ItemKind::Call, format!("{} isn't inlined: {why}{checks}", callee_name(self.m, *callee)));
                    }
                }
            }
        }
        out
    }

    /// Why the inliner left the call in `caller` of `callee` at `line`: the one it decided at that
    /// line if there is one, else the first it hasn't given out.
    fn reason_for(left: &mut LeftCalls, caller: usize, callee: usize, line: u32) -> String {
        let Some(list) = left.get_mut(&(caller as u32, callee as u32)) else { return "it wasn't inlined here".to_string() };
        let at = list.iter().position(|c| !c.used && c.line == line).or_else(|| list.iter().position(|c| !c.used));
        match at {
            Some(i) => {
                list[i].used = true;
                list[i].reason.clone()
            }
            None => list.first().map_or("it wasn't inlined here".to_string(), |c| c.reason.clone()),
        }
    }

    /// Puts the items in the order the report prints them. An item inlined from another
    /// function sorts where its predecessor does, so it stays next to the code it was inlined
    /// into; the items of one line are the checks, then the rest, each in the order the code runs
    /// them; then the loops are grouped.
    fn arrange(&self, home: Option<FuncIdx>, checks: Vec<Item>, others: Vec<Item>) -> Vec<Item> {
        arrange(self.m, &self.owners, home, checks, others)
    }
}

/// Puts the items of function `home` in the order the report prints them (see `Cx::arrange`).
pub(super) fn arrange(m: &Module, owners: &Owners, home: Option<FuncIdx>, checks: Vec<Item>, others: Vec<Item>) -> Vec<Item> {
    // (The copies of a generic are all the generic: they have its lines.)
    let own = |line: u32| match (owners.of(line), home) {
        (Some(o), Some(h)) => o == owners.home(h),
        _ => true,
    };
    let mut keyed: Vec<(u32, u8, Item)> = Vec::new();
    for list in [checks, others] {
        let mut anchor = home.map_or(0, |h| owners.first_line(m, h));
        for item in list {
            if own(item.line) {
                anchor = item.line;
            }
            let rank = u8::from(!matches!(item.kind, ItemKind::Check(_)));
            keyed.push((anchor, rank, item));
        }
    }
    keyed.sort_by_key(|(anchor, rank, _)| (*anchor, *rank));
    group(keyed.into_iter().map(|(_, _, item)| item).collect())
}

/// A loop and what is inside it, or an item.
enum Child {
    Item(Item),
    Loop(u32, Vec<Child>),
}

/// Keeps the items in order but puts all the items of one loop together, at the place of the
/// loop's first item.
fn group(items: Vec<Item>) -> Vec<Item> {
    let mut root = Vec::new();
    for item in items {
        let mut at = &mut root;
        for &l in &item.loops {
            let found = at.iter().position(|c| matches!(c, Child::Loop(line, _) if *line == l));
            let i = found.unwrap_or_else(|| {
                at.push(Child::Loop(l, Vec::new()));
                at.len() - 1
            });
            let Child::Loop(_, inner) = &mut at[i] else { unreachable!("just found a loop") };
            at = inner;
        }
        at.push(Child::Item(item));
    }
    let mut out = Vec::new();
    flatten(root, &mut out);
    out
}

fn flatten(children: Vec<Child>, out: &mut Vec<Item>) {
    for c in children {
        match c {
            Child::Item(i) => out.push(i),
            Child::Loop(_, inner) => flatten(inner, out),
        }
    }
}
