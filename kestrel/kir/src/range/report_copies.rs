//! Copies of generics in the report (G15, R5). A copy is an ordinary user function named for its
//! types (`best[int]`), with the lines of the declaration it was copied from, so every copy of a
//! generic has the same lines. `Owners`, which finds the function a line is in, therefore keeps
//! one span per generic, under the generic itself: an item inlined from any copy is named by the
//! generic's plain name (`best`), never by one copy. Generics nothing was copied from are listed
//! as not checked.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use super::events;
use super::report::{classes, Class, FuncReport};
use crate::ir::*;
use crate::visit::{walk, Node};

/// The function a line is in: the program's own functions, which don't overlap (a closure counts
/// as part of the function around it). The copies of a generic count as the generic.
pub struct Owners {
    /// (first line, last line, function), by first line.
    spans: Vec<(u32, u32, FuncIdx)>,
    /// Each copy's generic.
    generic_of: HashMap<FuncIdx, FuncIdx>,
    /// Each generic that has copies: its plain name as the report writes it, and its first line.
    generics: HashMap<FuncIdx, (String, u32)>,
}

impl Owners {
    pub fn new(m: &Module) -> Self {
        let classes = classes(m);
        // The generic of each name, found by its trapping stub (the first function of the name).
        let names: HashSet<&str> = m.copies.values().map(|note| note.generic.as_str()).collect();
        let mut stubs: HashMap<&str, usize> = HashMap::new();
        for (i, f) in m.funcs.iter().enumerate().filter(|(_, f)| names.contains(f.name.as_str())) {
            stubs.entry(f.name.as_str()).or_insert(i);
        }
        let (mut spans, mut generic_of, mut generics) = (Vec::new(), HashMap::new(), HashMap::new());
        for (i, f) in m.funcs.iter().enumerate() {
            if classes[i] != Class::User {
                continue;
            }
            let at = FuncIdx(i as u32);
            let generic = m.copies.get(&at).and_then(|note| stubs.get(note.generic.as_str()));
            let Some(&g) = generic else {
                spans.push((f.lines.0, f.lines.1, at));
                continue;
            };
            let g = FuncIdx(g as u32);
            generic_of.insert(at, g);
            if let Entry::Vacant(e) = generics.entry(g) {
                e.insert((plain_name(m, &m.copies[&at].generic, f.lines.0), f.lines.0));
                spans.push((f.lines.0, f.lines.1, g));
            }
        }
        spans.sort_by_key(|s| (s.0, std::cmp::Reverse(s.1)));
        Owners { spans, generic_of, generics }
    }

    /// The function that holds `line`, if one does (a generic for a line in any of its copies).
    pub fn of(&self, line: u32) -> Option<FuncIdx> {
        let at = self.spans.partition_point(|s| s.0 <= line);
        self.spans[..at].iter().rev().take(4).find(|s| line <= s.1).map(|s| s.2)
    }

    /// What `of` gives for the code of `f`: the generic for a copy, else `f`.
    pub fn home(&self, f: FuncIdx) -> FuncIdx {
        self.generic_of.get(&f).copied().unwrap_or(f)
    }

    /// The packed first line of `f`'s declaration (a generic's is its copies').
    pub fn first_line(&self, m: &Module, f: FuncIdx) -> u32 {
        self.generics.get(&f).map_or(m.funcs[f.0 as usize].lines.0, |g| g.1)
    }

    /// How the report names an owner: a generic by its plain name.
    pub fn name(&self, m: &Module, f: FuncIdx) -> String {
        match self.generics.get(&f) {
            Some((name, _)) => name.clone(),
            None => super::report::display_name(m, f),
        }
    }

    /// The function a closure is listed under: the function it is written in. When that is a
    /// generic, the copy that makes the closure (directly, or through the closures around it), or
    /// none when no copy does (its literal was inlined elsewhere).
    pub(super) fn closure_host(&self, m: &Module, class: &[Class], makers: &[Vec<usize>], closure: usize) -> Option<FuncIdx> {
        let owner = self.of(m.funcs[closure].lines.0)?;
        if !self.generics.contains_key(&owner) {
            return Some(owner);
        }
        let mut seen = HashSet::new();
        let mut pending = vec![closure];
        while let Some(g) = pending.pop() {
            if !seen.insert(g) {
                continue;
            }
            if class[g] == Class::User && self.home(FuncIdx(g as u32)) == owner {
                return Some(FuncIdx(g as u32));
            }
            pending.extend(makers[g].iter().copied());
        }
        None
    }
}

/// A name as the report writes it: a module's function without the module's full name
/// (`geom.helper` is `helper` in `geom.kes`, `net.packet.helper` is `helper` in `net/packet.kes`),
/// and the same for each of a copy's types (`geom.Stack[geom.Item].put` is `Stack[Item].put`).
/// A name of a module whose full name only starts with it keeps its module (`net.packet.P` stays
/// whole in `net.kes`). `line` is where the function is declared.
pub(super) fn plain_name(m: &Module, name: &str, line: u32) -> String {
    let Some(module) = m.module_names.get(unpack_line(line).1 as usize) else { return name.to_string() };
    let prefix = format!("{module}.");
    let longer: Vec<String> = m.module_names.iter().filter(|n| n.len() > module.len() && n.starts_with(&prefix)).map(|n| format!("{n}.")).collect();
    // Each run of name characters (a dotted path) loses a leading `module.`.
    let mut out = String::with_capacity(name.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let other = longer.iter().any(|l| word.starts_with(l.as_str()));
        out.push_str(word.strip_prefix(prefix.as_str()).filter(|_| !other).unwrap_or(word));
        word.clear();
    };
    for c in name.chars() {
        if c.is_alphanumeric() || c == '_' || c == '.' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// For each function, the functions with a closure literal (`MakeFn`) that names it as its code and
/// something can call it, or a `par for` that runs it. Inlining a literal into the loop that calls it leaves the code behind,
/// unused: its live copy is in the function that holds the loop.
pub(super) fn makers(m: &Module) -> Vec<Vec<usize>> {
    let mut makers = vec![Vec::new(); m.funcs.len()];
    for (i, f) in m.funcs.iter().enumerate() {
        let unused = events::unused_makes(f);
        walk(&f.body, &mut |n| match n {
            Node::Expr(e @ Expr { kind: ExprKind::MakeFn { code, .. }, .. }) if !unused.contains(&(e as *const Expr as usize)) => makers[code.0 as usize].push(i),
            Node::Stmt(Stmt::ParFor { func, .. }) => makers[func.0 as usize].push(i),
            _ => {}
        });
    }
    makers
}

/// A row of the report's text: a function, or a generic with no copies.
pub(super) enum Row<'a> {
    Func(&'a FuncReport),
    Uncopied(String),
}

/// The rows of the functions `funcs` lists and the generics with no copies, in the order the report
/// prints them (by file, then line), each with its file and line. `user_file` picks the files.
pub(super) fn rows<'a>(m: &'a Module, funcs: &'a [FuncReport], user_file: &dyn Fn(u32) -> bool) -> Vec<(u32, u32, Row<'a>)> {
    let mut rows: Vec<(u32, u32, Row)> = Vec::new();
    for f in funcs {
        let (line, file) = unpack_line(f.lines.0);
        if user_file(file) {
            rows.push((file, line, Row::Func(f)));
        }
    }
    for (name, lines) in &m.uncopied {
        let (line, file) = unpack_line(lines.0);
        if user_file(file) {
            rows.push((file, line, Row::Uncopied(uncopied_text(m, name, lines.0))));
        }
    }
    rows.sort_by_key(|(file, line, _)| (*file, *line));
    rows
}

/// "`best`: not checked, no copies", for generic `name` declared at packed line `line`.
fn uncopied_text(m: &Module, name: &str, line: u32) -> String {
    format!("`{}`: not checked, no copies", plain_name(m, name, line))
}
