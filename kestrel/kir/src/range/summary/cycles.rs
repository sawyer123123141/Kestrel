//! Which functions can call themselves: those on a cycle of the call graph (a call, a `par for`
//! body run, a closure made, and a call through an interface whose method promises `no_panic`, to
//! each implementation), found as its strongly connected components.

use std::collections::HashMap;

use super::{Part, Reason};
use crate::ir::*;
use crate::range::events::{Event, Unknown, What};

/// For each function on a cycle, its own recursion reason: "can call itself" at the first call
/// that stays on the cycle, with the tail ", so the stack may run out".
pub(super) fn own_reasons(m: &Module, events: &[Vec<Event>]) -> Vec<Option<Reason>> {
    let impls = implementations(m);
    let edges: Vec<Vec<(usize, u32)>> = events.iter().map(|es| es.iter().flat_map(|e| targets(&impls, e)).collect()).collect();
    let succ: Vec<Vec<usize>> = edges.iter().map(|es| es.iter().map(|&(g, _)| g).collect()).collect();
    let comp = components(&succ);
    let mut size = vec![0usize; succ.len()];
    for &c in &comp {
        size[c] += 1;
    }
    edges
        .iter()
        .enumerate()
        .map(|(f, es)| {
            let cyclic = size[comp[f]] > 1 || succ[f].contains(&f);
            if !cyclic {
                return None;
            }
            let &(_, line) = es.iter().find(|(g, _)| comp[*g] == comp[f])?;
            let tail = ", so the stack may run out".to_string();
            Some(Reason { direct: None, via: "can call itself".to_string(), tail, line, part: Part::Whole })
        })
        .collect()
}

/// The implementations of each interface method that promises something, by the method's stub
/// (one per instance of a generic interface, so `Handler[Click].handle`'s never include
/// `Handler[Key].handle`'s).
pub(crate) fn implementations(m: &Module) -> HashMap<FuncIdx, Vec<usize>> {
    let mut out: HashMap<FuncIdx, Vec<usize>> = HashMap::new();
    for site in &m.contracts {
        if let (Some(by), ProveTarget::Func(f)) = (site.through, site.target) {
            out.entry(by).or_default().push(f.0 as usize);
        }
    }
    out
}

/// The functions event `e` may run, each with the line: a call, a `par for` body, a closure made,
/// and for a call through an interface that promises `no_panic` (so it is no unknown call for
/// it), every implementation of the method, each checked to keep `no_panic` on its own.
fn targets(impls: &HashMap<FuncIdx, Vec<usize>>, e: &Event) -> Vec<(usize, u32)> {
    match &e.what {
        What::Call(g) | What::Body(g) | What::Closure(g) => vec![(g.0 as usize, e.line)],
        What::Unknown(Unknown::Virtual(keeps, method), _) if keeps.contains(ContractSet::NO_PANIC) => {
            implementations_of(impls, *method).iter().map(|&f| (f, e.line)).collect()
        }
        _ => Vec::new(),
    }
}

/// The implementations of interface method `method` (its stub), from `implementations`.
pub(crate) fn implementations_of(impls: &HashMap<FuncIdx, Vec<usize>>, method: FuncIdx) -> &[usize] {
    impls.get(&method).map_or(&[], Vec::as_slice)
}

/// The strongly connected component of each node of the graph `succ` (Tarjan's algorithm, without
/// recursion, so a deep call graph can't overflow the stack).
fn components(succ: &[Vec<usize>]) -> Vec<usize> {
    const NONE: usize = usize::MAX;
    let n = succ.len();
    let (mut index, mut low, mut comp) = (vec![NONE; n], vec![0; n], vec![NONE; n]);
    let mut on_stack = vec![false; n];
    let (mut stack, mut work): (Vec<usize>, Vec<(usize, usize)>) = (Vec::new(), Vec::new());
    let (mut next, mut comps) = (0, 0);
    for s in 0..n {
        if index[s] != NONE {
            continue;
        }
        index[s] = next;
        low[s] = next;
        next += 1;
        stack.push(s);
        on_stack[s] = true;
        work.push((s, 0));
        while let Some(&(v, i)) = work.last() {
            if let Some(&w) = succ[v].get(i) {
                work.last_mut().expect("just read").1 += 1;
                if index[w] == NONE {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(u, _)) = work.last() {
                low[u] = low[u].min(low[v]);
            }
            if low[v] == index[v] {
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    comp[w] = comps;
                    if w == v {
                        break;
                    }
                }
                comps += 1;
            }
        }
    }
    comp
}
