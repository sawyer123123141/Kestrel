//! Deciding `@prove` contracts (the Round B addendum, "What a contract covers"): every contract
//! of every site, in every compiled copy of its code, from range analysis of the build and the
//! events walk, with each call left followed through the summaries. One error per failed contract,
//! naming the first thing that breaks it in the order the report lists items, never a count (the
//! report's totals count inlined code twice and are never read here).

mod region;
mod words;

use std::collections::{HashMap, HashSet};

use kestrel_syntax::ast::Contract;
use kestrel_syntax::Span;

use super::events::{self, Event, Unknown, What, PAR_ARENAS};
use super::items::arrange;
use super::report::{classes, constants_home, Class, Item, ItemKind, Owners};
use super::summary::{self, Chain, Part, Stop, Summary};
use super::Analysis;
use crate::ir::*;
use region::Copy;
use words::Help;

/// A contract that doesn't hold, at the contract's name inside `@prove(...)` (an interface's at
/// the implementing method's name).
#[derive(Clone, Debug, PartialEq)]
pub struct ContractError {
    pub span: Span,
    pub message: String,
    pub help: Option<String>,
}

const NO_ALLOC: u8 = ContractSet::NO_ALLOC.0;
const NO_CHECKS: u8 = ContractSet::NO_CHECKS.0;
const NO_PANIC: u8 = ContractSet::NO_PANIC.0;
const INLINE: u8 = ContractSet::INLINE.0;
const DETERMINISTIC: u8 = ContractSet::DETERMINISTIC.0;
/// The contracts range analysis decides: giving a function up breaks them all.
const ALL: u8 = NO_ALLOC | NO_CHECKS | NO_PANIC | INLINE;

/// The one bit of contract `c` (not `ContractSet::of`, which gives `no_panic` with `no_checks`).
fn bit(c: Contract) -> u8 {
    match c {
        Contract::NoAlloc => NO_ALLOC,
        Contract::NoChecks => NO_CHECKS,
        Contract::NoPanic => NO_PANIC,
        Contract::Inline => INLINE,
        Contract::Deterministic => DETERMINISTIC,
    }
}

/// A chain a contract may break through, with the verb of the item it ends in.
type Pick<'a> = (&'a Chain, &'a str);

/// Something in a copy that breaks the contracts in `breaks`: the reason, after the contract's
/// "...: ", and the help line.
struct Failure {
    breaks: u8,
    text: String,
    help: Help,
}

impl Failure {
    fn new(breaks: u8, text: String, help: Help) -> Self {
        Failure { breaks, text, help }
    }
}

/// Decides every contract of `m`, which range analysis has analysed (`a`) in the build's own mode.
pub fn check(m: &Module, a: &Analysis) -> Vec<ContractError> {
    if m.contracts.is_empty() {
        return Vec::new();
    }
    let events: Vec<Vec<Event>> = m.funcs.iter().map(|f| if f.native.is_some() { Vec::new() } else { events::events(m, f) }).collect();
    let summaries = summary::summarize(m, a, &events, None);
    let mut left_calls: HashMap<(usize, usize), Vec<(u32, &str)>> = HashMap::new();
    for n in &m.not_inlined {
        left_calls.entry((n.caller.0 as usize, n.callee.0 as usize)).or_default().push((n.line, &n.reason));
    }
    let impls = summary::implementations(m);
    let cx = Cx { m, a, events: &events, summaries: &summaries, classes: classes(m), owners: Owners::new(m), left_calls, impls };
    let blocks = region::blocks(m);
    let mut found: Vec<(usize, Contract, ContractError)> = Vec::new();
    for (id, site) in m.contracts.iter().enumerate() {
        found.extend(cx.site(&blocks, id as u32, site).into_iter().map(|(c, e)| (id, c, e)));
    }
    // An implementing method's own `@prove` gives no error for a contract its interface's
    // requires and also fails: that one says more.
    let target = |id: usize| m.contracts[id].target;
    let required: HashSet<(ProveTarget, Contract)> = found.iter().filter(|(id, ..)| m.contracts[*id].required_by.is_some()).map(|(id, c, _)| (target(*id), *c)).collect();
    found
        .into_iter()
        .filter(|(id, c, _)| m.contracts[*id].required_by.is_some() || !required.contains(&(target(*id), *c)))
        .map(|(_, _, e)| e)
        .collect()
}

struct Cx<'a> {
    m: &'a Module,
    a: &'a Analysis,
    events: &'a [Vec<Event>],
    summaries: &'a [Summary],
    classes: Vec<Class>,
    owners: Owners,
    /// The calls the inliner left, by (caller, callee): each one's line and why.
    left_calls: HashMap<(usize, usize), Vec<(u32, &'a str)>>,
    /// The implementations of each interface method that promises something.
    impls: HashMap<&'a str, Vec<usize>>,
}

impl Cx<'_> {
    /// The failed contracts of site `id`, each with its error, in the order written.
    fn site(&self, blocks: &HashMap<u32, Vec<(usize, Region)>>, id: u32, site: &ContractSite) -> Vec<(Contract, ContractError)> {
        let copies = match region::copies(self.m, blocks, id, site) {
            Ok(copies) => copies,
            Err(lost) => {
                let message = words::lost(&lost);
                return site.contracts.iter().map(|&(c, span)| (c, ContractError { span, message: message.clone(), help: None })).collect();
            }
        };
        let home = match site.target {
            ProveTarget::Func(f) => unpack_line(self.m.funcs[f.0 as usize].lines.0).1,
            ProveTarget::Loop { line, .. } => unpack_line(line).1,
        };
        let mut open: Vec<(usize, Contract, Span)> = site.contracts.iter().enumerate().map(|(i, &(c, span))| (i, c, span)).collect();
        let mut out: Vec<(usize, Contract, ContractError)> = Vec::new();
        for copy in copies {
            if open.is_empty() {
                break;
            }
            let failures = self.failures(site, copy, home);
            let at = words::copy_where(self.m, site, copy, home);
            let head = match (&site.required_by, at.is_empty()) {
                (Some(r), true) => format!(", which {r} requires"),
                (Some(r), false) => format!(", which {r} requires,{at}"),
                (None, _) => at,
            };
            open.retain(|&(i, c, span)| {
                let Some(f) = failures.iter().find(|f| f.breaks & bit(c) != 0) else { return true };
                let message = format!("{} can't be proven `{}`{head}: {}", words::target(self.m, site), c.name(), f.text);
                out.push((i, c, ContractError { span, message, help: Some(words::help(self.m, f.help, c)) }));
                false
            });
        }
        out.sort_by_key(|(i, ..)| *i);
        out.into_iter().map(|(_, c, e)| (c, e)).collect()
    }

    /// Everything in `copy` that breaks a contract, in order: a function in it range analysis
    /// couldn't decide, `main`'s constants, then its items as the report orders them.
    fn failures(&self, site: &ContractSite, copy: Copy, home: u32) -> Vec<Failure> {
        let m = self.m;
        let contents = region::contents(self.a, self.events, copy);
        let target = match site.target {
            ProveTarget::Func(f) => Some(f.0 as usize),
            ProveTarget::Loop { .. } => None,
        };
        let mut out = Vec::new();
        // The state the function changes must be copyable, before anything its code does (the
        // function's own body only: an inlined copy has the same parameters).
        if let (Some(fault), true) = (&site.snapshot, copy.block.is_none() && target == Some(copy.func)) {
            out.push(Failure::new(DETERMINISTIC, words::snapshot(fault), Help::Snapshot));
        }
        for &(g, _) in &contents.funcs {
            if let Some(stop) = summary::stop(self.a, self.events, g) {
                out.push(Failure::new(ALL, words::stopped(m, stop, g, target == Some(g)), Help::Report));
            }
        }
        if copy.block.is_none() && target.is_some() && target.map(|t| FuncIdx(t as u32)) == constants_home(m) {
            if let Some(init) = m.init {
                self.constants(init, m.funcs[copy.func].lines.0, home, &mut out);
            }
        }
        // Each item of the copy, then put in the report's order (an item's text is its number).
        let line_in = |g: usize, line: u32| if line != 0 { line } else { contents.funcs.iter().find(|&&(f, _)| f == g).map_or(0, |&(_, at)| at) };
        let mut found: Vec<Option<Failure>> = Vec::new();
        let mut checks = Vec::new();
        for &(g, k) in &contents.checks {
            let line = line_in(g, k.line);
            checks.push(Item { line, loops: k.loops.clone(), kind: ItemKind::Check(k.kind), text: found.len().to_string() });
            found.push(Some(Failure::new(NO_CHECKS | NO_PANIC, words::check(m, k.kind, line, &k.text, home), Help::Report)));
        }
        let mut others = Vec::new();
        for &(g, e) in &contents.events {
            let line = line_in(g, e.line);
            for f in self.event_failures(copy, target, g, e, line, home) {
                others.push(Item { line, loops: e.loops.clone(), kind: ItemKind::Allocation, text: found.len().to_string() });
                found.push(Some(f));
            }
        }
        let host = match self.classes[copy.func] {
            Class::User => Some(FuncIdx(copy.func as u32)),
            _ => self.owners.of(m.funcs[copy.func].lines.0),
        };
        for item in arrange(m, &self.owners, host, checks, others) {
            let i: usize = item.text.parse().expect("an item's number");
            out.extend(found[i].take());
        }
        out
    }

    /// What event `e` (in function `g` of `copy`, at `line`) breaks.
    fn event_failures(&self, copy: Copy, target: Option<usize>, g: usize, e: &Event, line: u32, home: u32) -> Vec<Failure> {
        let m = self.m;
        match &e.what {
            What::Alloc(r) => {
                let help = if r.direct.as_deref() == Some(PAR_ARENAS) { Help::ParFor } else { Help::Report };
                vec![Failure::new(NO_ALLOC, words::allocates(m, r, line, home), help)]
            }
            What::Fail(r) if r.part != Part::Resource => vec![Failure::new(NO_PANIC, words::fails(m, r, line, home), Help::Report)],
            What::Unknown(kind, r) => {
                let breaks = (if kind.allocates() { NO_ALLOC } else { 0 }) | (if kind.checks() { NO_CHECKS } else { 0 }) | (if kind.fails() { NO_PANIC } else { 0 });
                let help = match kind {
                    Unknown::Indirect => Help::FnValue,
                    Unknown::Virtual(_, method) => Help::Iface(*method),
                    Unknown::Native => Help::Report,
                };
                let mut out = Vec::new();
                if breaks != 0 {
                    out.push(Failure::new(breaks, words::unknown(m, r, home), help));
                }
                if *kind != Unknown::Native {
                    let help = if *kind == Unknown::Indirect { Help::FnValue } else { Help::Report };
                    out.push(Failure::new(INLINE, words::not_inlinable(m, "it", r, line, home), help));
                }
                // A call that keeps `no_panic` may still run an implementation that can call
                // itself (each fails its own contract too).
                if let Unknown::Virtual(keeps, method) = kind {
                    let recursion = summary::implementations_of(m, &self.impls, *method).iter().find_map(|&f| self.summaries[f].recursion.as_ref());
                    if let (true, Some(chain)) = (keeps.contains(ContractSet::NO_PANIC), recursion) {
                        out.push(Failure::new(NO_PANIC, words::through_method(m, r, chain, line), Help::Report));
                    }
                }
                out
            }
            What::Effect(effect, r) => vec![Failure::new(DETERMINISTIC, words::unknown(m, r, home), Help::Effect(*effect))],
            // (The other contracts fail through `summary::stop` for a cut walk; `deterministic`
            // only for one in the copy, since range analysis giving up doesn't matter to it.)
            What::Cut => vec![Failure::new(DETERMINISTIC, words::stopped(m, Stop::Deep, g, target == Some(g)), Help::Report)],
            What::Call(callee) => self.call_failures(copy, target, g, *callee, line, home),
            What::Fail(_) | What::Body(_) | What::Closure(_) => Vec::new(),
        }
    }

    /// What a call left in function `g` of `copy`, at `line`, to `callee` breaks: through its
    /// summary, and for `inline` the call itself (or, in code the compiler made, its calls into
    /// the program's code).
    fn call_failures(&self, copy: Copy, target: Option<usize>, g: usize, callee: FuncIdx, line: u32, home: u32) -> Vec<Failure> {
        let (m, c) = (self.m, callee.0 as usize);
        let mut out = Vec::new();
        if target == Some(c) {
            out.push(Failure::new(NO_PANIC, words::calls_itself(m, line, home), Help::Report));
        }
        // (A function's own body calling itself: everything in it is in the copy already.)
        if !(copy.block.is_none() && c == copy.func) {
            self.chains(c, target == Some(c), &mut out, |chain, verb| words::call(m, callee, chain, line, verb));
        }
        match self.classes[c] {
            Class::User | Class::Closure => out.push(Failure::new(INLINE, words::not_inlined(m, "it", callee, line, &self.why(g, c, line), home), Help::Report)),
            Class::Generated => self.generated_calls(c, "it", line, home, &mut out),
            Class::Native | Class::ParBody => {}
        }
        out
    }

    /// The failures a call to function `f` breaks `no_alloc`, `no_checks`, `no_panic` and
    /// `deterministic` through: for each, the chain of its summary with the fewest calls (on a tie,
    /// one the summary can't see into, then a check, a way to fail, recursion), in the words `text`
    /// makes of a chain and the verb of an item it ends in. With `recursion_said`, the call's
    /// recursion is already a failure.
    fn chains(&self, f: usize, recursion_said: bool, out: &mut Vec<Failure>, text: impl Fn(&Chain, &str) -> String) {
        let s = &self.summaries[f];
        let checks = s.checks.as_ref().map(|c| (c, c.reason.via.as_str()));
        let recursion = if recursion_said { None } else { s.recursion.as_ref() };
        let picks: [(u8, [Option<Pick>; 4]); 3] = [
            (NO_ALLOC, [s.unknown_alloc.as_ref().map(|c| (c, "")), s.allocates.as_ref().map(|c| (c, "allocates")), None, None]),
            (NO_CHECKS, [s.unknown_checks.as_ref().map(|c| (c, "")), checks, None, None]),
            (NO_PANIC, [s.unknown_panic.as_ref().map(|c| (c, "")), checks, s.panics.as_ref().map(|c| (c, "can fail")), recursion.map(|c| (c, ""))]),
        ];
        for (breaks, options) in picks {
            if let Some((chain, verb)) = options.into_iter().flatten().min_by_key(|(c, _)| c.steps.len()) {
                out.push(Failure::new(breaks, text(chain, verb), Help::Report));
            }
        }
        if let Some(chain) = &s.effects {
            out.push(Failure::new(DETERMINISTIC, text(chain, ""), self.effect_help(f, chain)));
        }
    }

    /// The help for `deterministic` broken through `chain`, the effects chain of function `f`: by
    /// what the function the chain ends in does first (its own reason).
    fn effect_help(&self, f: usize, chain: &Chain) -> Help {
        let end = chain.steps.last().map_or(f, |s| s.callee.0 as usize);
        let first = self.events[end].iter().find_map(|e| match &e.what {
            What::Effect(effect, _) => Some(Help::Effect(*effect)),
            What::Cut => Some(Help::Report),
            _ => None,
        });
        first.unwrap_or(Help::Report)
    }

    /// `main`'s constants, built by `init` before it starts: what they break, at `line`.
    fn constants(&self, init: FuncIdx, line: u32, home: u32, out: &mut Vec<Failure>) {
        let m = self.m;
        self.chains(init.0 as usize, false, out, |chain, _| words::constants(m, chain, line));
        self.generated_calls(init.0 as usize, &format!("{}, which", words::CONSTANTS), line, home, out);
    }

    /// For `inline`: the calls the compiler-made function `gen` (called at `line`) makes into the
    /// program's code, directly or through other compiler-made functions and the closures they
    /// make; the compiler-made calls themselves don't count. Each says `lead` first.
    fn generated_calls(&self, gen: usize, lead: &str, line: u32, home: u32, out: &mut Vec<Failure>) {
        let mut seen = HashSet::from([gen]);
        let mut stack = vec![gen];
        while let Some(f) = stack.pop() {
            for e in &self.events[f] {
                match &e.what {
                    What::Call(p) if matches!(self.classes[p.0 as usize], Class::User | Class::Closure) => {
                        let why = self.why(f, p.0 as usize, e.line);
                        out.push(Failure::new(INLINE, words::not_inlined(self.m, lead, *p, line, &why, home), Help::Report));
                    }
                    What::Call(p) | What::Body(p) | What::Closure(p) => {
                        if seen.insert(p.0 as usize) {
                            stack.push(p.0 as usize);
                        }
                    }
                    What::Unknown(kind, r) if *kind != Unknown::Native => {
                        out.push(Failure::new(INLINE, words::not_inlinable(self.m, lead, r, line, home), Help::Report));
                    }
                    _ => {}
                }
            }
        }
    }

    /// Why the inliner left the call in `caller` of `callee` at `line`: the one it decided at that
    /// line, else the first.
    fn why(&self, caller: usize, callee: usize, line: u32) -> String {
        let Some(list) = self.left_calls.get(&(caller, callee)) else { return "it wasn't inlined here".to_string() };
        list.iter().find(|(l, _)| *l == line).or(list.first()).map_or("it wasn't inlined here".to_string(), |(_, why)| why.to_string())
    }
}
