//! List methods (`add`, `remove_at`, `pop`, `insert`, `clear`, `has`, `find`, `join`, and the four that
//! take a function, see `list_fns`), and the loop
//! check that keeps a list from changing length while a `for ... in` loop runs over it.

use kestrel_syntax::ast::{Arg, Expr, ExprKind};

use super::BodyChecker;
use crate::tables::{ListMethod, LocalId, Res};
use crate::ty::Ty;

/// A step into a place: a field's name, or `None` for an index (which may be any index).
pub(crate) type Step = Option<String>;

/// A `for ... in` loop over a list or an array that is being checked.
pub(crate) struct LoopList {
    /// The loop's item, which stands for one element of the list.
    pub(crate) item: LocalId,
    /// The variable the list belongs to, aliases followed.
    pub(crate) root: LocalId,
    /// The list's place inside `root`.
    pub(crate) path: Vec<Step>,
    /// How many loops (of lists and of maps) were open outside this one.
    pub(crate) order: usize,
}

/// The loop that stops a list from changing: the list as its message names it, and the loop's item
/// when it is a loop over a list (a `remove` of it is the way out).
pub(crate) struct Overlap {
    pub(crate) list: String,
    pub(crate) item: Option<String>,
}

/// Whether the list at `changed` is the list at `looped`, or holds it: `changed` is `looped` or
/// a start of it, an index standing for any index. A list inside an element of the looped
/// list (`row` while looping over `grid`) is a different list, and is free to change.
pub(crate) fn covers(changed: &[Step], looped: &[Step]) -> bool {
    changed.len() <= looped.len() && changed.iter().zip(looped).all(|(a, b)| a.is_none() || b.is_none() || a == b)
}

/// How a list method is written, to tell `add` from the rest.
#[derive(Clone, Copy)]
enum Which {
    Add,
    Other(ListMethod),
}

impl<'a> BodyChecker<'a> {
    /// The root variable and steps of a place, with a loop's item standing for an element of its
    /// list. Call after the expression has been checked.
    pub(crate) fn list_place(&self, e: &Expr) -> Option<(LocalId, Vec<Step>)> {
        match &e.kind {
            ExprKind::Ident(_) => {
                let Some(&Res::Local(l)) = self.t.res.get(&e.id) else { return None };
                if let Some(x) = self.for_in_lists.iter().rev().find(|x| x.item == l) {
                    let mut path = x.path.clone();
                    path.push(None);
                    return Some((x.root, path));
                }
                // A `match` binding or `let ... else` view stands for a part of the place it reads.
                if let Some(p) = self.removing.views.get(&l) {
                    return Some(p.clone());
                }
                Some((self.t.local(l).alias.unwrap_or(l), Vec::new()))
            }
            ExprKind::Field { .. } if self.t.swizzles.contains_key(&e.id) => None,
            ExprKind::Field { base, .. } if matches!(self.t.ty(base.id), Ty::Array(_) | Ty::List(_) | Ty::Map(..) | Ty::Str) => None,
            ExprKind::Field { base, name, .. } => {
                let (root, mut path) = self.list_place(base)?;
                path.push(Some(name.clone()));
                Some((root, path))
            }
            ExprKind::Index { base, .. } => {
                let (root, mut path) = self.list_place(base)?;
                path.push(None);
                Some((root, path))
            }
            _ => None,
        }
    }

    /// The innermost enclosing `for ... in` loop whose list is the one at `place`, or holds it:
    /// changing the length or the order of `place` there would shorten, move or drop the items the
    /// loop is reading. A loop over a map held in a list's item counts too: the loop re-reads the map
    /// by its place, so moving the items would swap another map in under it.
    pub(crate) fn overlapping_loop(&self, place: &Expr) -> Option<Overlap> {
        let (root, path) = self.list_place(place)?;
        let inner = |loops: &[LoopList]| loops.iter().rposition(|x| x.root == root && covers(&path, &x.path));
        let (list, map) = (inner(&self.for_in_lists), inner(&self.for_in_maps));
        // (The map loop is the innermost when it was entered after the list loop.)
        match (list, map) {
            (Some(i), Some(j)) if self.for_in_maps[j].order > self.for_in_lists[i].order => Some(self.map_overlap(j)),
            (Some(i), _) => Some(Overlap { list: self.loop_list_name(i), item: Some(self.t.local(self.for_in_lists[i].item).name.clone()) }),
            (None, Some(j)) => Some(self.map_overlap(j)),
            (None, None) => None,
        }
    }

    /// Whether a call given `place` as `var` could break an enclosing loop over a map: `place` is
    /// the map, or holds it (a list, a list's item, a struct), with or without an index on the
    /// way. The call could replace the map, or move the items that hold it, which would swap
    /// another map in under the loop. A sibling field or another list can't, so it passes.
    pub(crate) fn map_loop_inside(&self, place: &Expr) -> bool {
        let Some((root, path)) = self.list_place(place) else { return false };
        self.for_in_maps.iter().any(|x| x.root == root && covers(&path, &x.path))
    }

    fn map_overlap(&self, i: usize) -> Overlap {
        Overlap { list: Self::name_of(&self.t.local(self.for_in_maps[i].root).name, &self.for_in_maps[i].path), item: None }
    }

    /// Pushes the loop over the map `iter`, whose item is `item`, if `iter` is a place.
    pub(crate) fn enter_map_loop(&mut self, item: LocalId, iter: &Expr) {
        if let Some((root, path)) = self.list_place(iter) {
            let order = self.for_in_lists.len() + self.for_in_maps.len();
            self.for_in_maps.push(LoopList { item, root, path, order });
        }
    }

    /// Pops the loop `enter_map_loop` pushed, if it did.
    pub(crate) fn leave_map_loop(&mut self, item: LocalId) {
        if self.for_in_maps.last().is_some_and(|x| x.item == item) {
            self.for_in_maps.pop();
        }
    }

    /// A place's variable and fields, up to the first index, as a message names it.
    fn name_of(root: &str, path: &[Step]) -> String {
        let mut name = root.to_string();
        for step in path.iter().map_while(Option::as_ref) {
            name.push_str(&format!(".{step}"));
        }
        name
    }

    /// Pushes the loop over `iter`, whose item is `item`, if `iter` is a place.
    pub(crate) fn enter_list_loop(&mut self, item: LocalId, iter: &Expr) {
        if let Some((root, path)) = self.list_place(iter) {
            let order = self.for_in_lists.len() + self.for_in_maps.len();
            self.for_in_lists.push(LoopList { item, root, path, order });
        }
    }

    /// Pops the loop `enter_list_loop` pushed, if it did.
    pub(crate) fn leave_list_loop(&mut self, item: LocalId) {
        if self.for_in_lists.last().is_some_and(|x| x.item == item) {
            self.for_in_lists.pop();
        }
    }

    /// The list a loop reads, as the loop's error names it: its variable and fields, up to the
    /// first index.
    pub(crate) fn loop_list_name(&self, i: usize) -> String {
        let x = &self.for_in_lists[i];
        Self::name_of(&self.t.local(x.root).name, &x.path)
    }

    /// An index argument: any integer type.
    pub(crate) fn check_int_index(&mut self, index: &Expr) {
        let ty = self.check_expr(index, None);
        let ty = self.reject_never(index, ty);
        if !ty.is_int() && ty != Ty::Error {
            let msg = format!("an index must be an integer, found {}", self.tn(ty));
            self.error(index.span, msg);
        }
    }

    /// `xs.add(v)`, `xs.remove_at(i)`, `xs.pop()`, `xs.insert(i, v)`, `xs.clear()`, `xs.has(v)`
    /// and `xs.find(v)`. Arrays only take the last two. The ones that change `xs` need it to be
    /// changeable.
    pub(crate) fn list_method(&mut self, e: &Expr, receiver: &Expr, recv_ty: Ty, args: &[Arg]) -> Ty {
        let ExprKind::MethodCall { name, name_span, .. } = &e.kind else { unreachable!("not a method call") };
        let elem = self.elem_of(recv_ty).expect("a collection type");
        let which = match name.as_str() {
            "add" => Which::Add,
            "remove_at" => Which::Other(ListMethod::RemoveAt),
            "pop" => Which::Other(ListMethod::Pop),
            "insert" => Which::Other(ListMethod::Insert),
            "clear" => Which::Other(ListMethod::Clear),
            "has" => Which::Other(ListMethod::Has),
            "find" => Which::Other(ListMethod::Find),
            "join" => Which::Other(ListMethod::Join),
            "filter" => Which::Other(ListMethod::Filter),
            "map" => Which::Other(ListMethod::Map),
            "any" => Which::Other(ListMethod::Any),
            "all" => Which::Other(ListMethod::All),
            "sort" => Which::Other(ListMethod::Sort),
            "sort_by" => Which::Other(ListMethod::SortBy),
            _ => {
                let help = match recv_ty {
                    // A slip for `remove_at`, or for the statement `remove b` in a loop.
                    Ty::List(_) if matches!(name.as_str(), "remove" | "erase") => {
                        let item = match args.first().map(|a| &a.value.kind) {
                            Some(ExprKind::Ident(b)) => b.clone(),
                            _ => "b".to_string(),
                        };
                        let xs = written(receiver);
                        format!("use `{xs}.remove_at(i)`, or `remove {item}` inside `for {item} in {xs}`")
                    }
                    Ty::List(_) => "lists have `add`, `remove_at`, `pop`, `insert`, `clear`, `has`, `find`, `join`, `filter`, `map`, `any`, `all`, `sort`, `sort_by` and `.len`".to_string(),
                    _ => "arrays have `has`, `find`, `join`, `filter`, `map`, `any`, `all`, `sort`, `sort_by` and `.len`".to_string(),
                };
                let msg = format!("`{}` has no method `{name}`", self.tn(recv_ty));
                self.error_help(*name_span, msg, help);
                self.check_loose_args(args);
                return Ty::Error;
            }
        };
        let changes = match which {
            Which::Add => true,
            // (Sorting changes the order of an array's items, not its length.)
            Which::Other(m) => m.changes_receiver() && !m.reorders(),
        };
        if changes && matches!(recv_ty, Ty::Array(_)) {
            let msg = match which {
                Which::Add => format!("arrays have a fixed length; use a `list[{}]` to add items", self.tn(elem)),
                Which::Other(_) => format!("arrays have a fixed length; use a `list[{}]` if items come and go", self.tn(elem)),
            };
            self.error(*name_span, msg);
            self.check_loose_args(args);
            return Ty::Error;
        }
        let Which::Other(method) = which else { return self.list_add(e, receiver, args, elem) };
        if matches!(method, ListMethod::Has | ListMethod::Find) && self.list_param_rule(name, elem, *name_span) {
            self.check_loose_args(args);
            return Ty::Error;
        }
        // (A placeholder that gets here has the rule.)
        let comparable = matches!(elem, Ty::Param(_)) || self.t.equatable(elem);
        if matches!(method, ListMethod::Has | ListMethod::Find) && !comparable {
            let msg = format!(
                "`{name}` needs items that `==` can compare (numbers, bools, strings, vectors, and structs and enums of them); `{}` holds `{}`",
                self.tn(recv_ty),
                self.tn(elem)
            );
            self.error(*name_span, msg);
            self.check_loose_args(args);
            return Ty::Error;
        }
        if method == ListMethod::Join && !matches!(elem, Ty::Str | Ty::Error) {
            let msg = format!("`join` needs a list of strings, found `{}`", self.tn(recv_ty));
            self.error(*name_span, msg);
            self.check_loose_args(args);
            return Ty::Error;
        }
        let usage = match method {
            ListMethod::RemoveAt => ("`remove_at` takes one index", 1),
            ListMethod::Pop => ("`pop` takes no arguments", 0),
            ListMethod::Insert => ("`insert` takes an index and a value", 2),
            ListMethod::Clear => ("`clear` takes no arguments", 0),
            ListMethod::Has => ("`has` takes one value", 1),
            ListMethod::Find => ("`find` takes one value", 1),
            ListMethod::Join => ("`join` takes one value", 1),
            ListMethod::Filter => ("`filter` takes one function", 1),
            ListMethod::Map => ("`map` takes one function", 1),
            ListMethod::Any => ("`any` takes one function", 1),
            ListMethod::All => ("`all` takes one function", 1),
            ListMethod::Sort => ("`sort` takes no arguments", 0),
            ListMethod::SortBy => ("`sort_by` takes one function", 1),
        };
        if args.len() != usage.1 || args.iter().any(|a| a.label.is_some() || a.is_var) {
            self.error(e.span, usage.0);
            self.check_loose_args(args);
            return Ty::Error;
        }
        self.t.res.insert(e.id, Res::List(method));
        if method.reorders() {
            return self.list_sort(e, receiver, recv_ty, method, *name_span, args);
        }
        if method.takes_fn() {
            return self.list_fn(e, receiver, recv_ty, method, *name_span, args);
        }
        if method.changes_receiver() && self.receiver_root(receiver, name).is_some() {
            if let Some(Overlap { list, item }) = self.overlapping_loop(receiver) {
                let way_out = match item {
                    Some(item) => format!("use `remove {item}` to remove the loop's item, or change `{list}` after the loop"),
                    None => format!("change `{list}` after the loop"),
                };
                self.error(e.span, format!("can't change how many items `{list}` has while looping over it; {way_out}"));
            }
        }
        match method {
            ListMethod::RemoveAt => self.check_int_index(&args[0].value),
            ListMethod::Insert => {
                self.check_int_index(&args[0].value);
                self.check_element(&args[1].value, Some(elem));
                self.check_store(receiver, &args[1].value);
            }
            ListMethod::Has | ListMethod::Find => {
                self.check_value(&args[0].value, Some(elem));
            }
            ListMethod::Join => {
                self.check_value(&args[0].value, Some(Ty::Str));
            }
            ListMethod::Pop => self.check_pop_owner(e, receiver, elem),
            ListMethod::Clear => {}
            ListMethod::Filter | ListMethod::Map | ListMethod::Any | ListMethod::All | ListMethod::Sort | ListMethod::SortBy => {
                unreachable!("handled by `list_fn` and `list_sort`")
            }
        }
        match method {
            ListMethod::Pop => Ty::Optional(self.t.intern_elem(elem)),
            ListMethod::Find => Ty::Optional(self.t.intern_elem(Ty::Int)),
            // Lowering searches with `find`, whose `int?` needs its element type interned.
            ListMethod::Has => {
                self.t.intern_elem(Ty::Int);
                Ty::Bool
            }
            ListMethod::Join => Ty::Str,
            ListMethod::RemoveAt | ListMethod::Insert | ListMethod::Clear => Ty::Unit,
            ListMethod::Filter | ListMethod::Map | ListMethod::Any | ListMethod::All | ListMethod::Sort | ListMethod::SortBy => {
                unreachable!("handled by `list_fn` and `list_sort`")
            }
        }
    }

    /// `xs.add(v)`: `xs` must be changeable. Adding while looping over `xs` is fine: the loop
    /// reads the length once.
    fn list_add(&mut self, e: &Expr, receiver: &Expr, args: &[Arg], elem: Ty) -> Ty {
        self.t.res.insert(e.id, Res::ListAdd);
        self.receiver_root(receiver, "add");
        match args {
            [a] if a.label.is_none() && !a.is_var => {
                self.check_element(&a.value, Some(elem));
                self.check_store(receiver, &a.value);
            }
            _ => {
                self.error(e.span, "`add` takes one value");
                self.check_loose_args(args);
            }
        }
        Ty::Unit
    }
}

/// A place as the user wrote it, for a message: `levels[0].items`. An index that isn't a
/// number or a name shows as `_`, and a call as `f(...)`.
pub(crate) fn written(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Ident(n) => n.clone(),
        ExprKind::Field { base, name, .. } => format!("{}.{name}", written(base)),
        ExprKind::Index { base, index } => {
            let at = match &index.kind {
                ExprKind::Int(n) => n.to_string(),
                ExprKind::Ident(n) => n.clone(),
                _ => "_".to_string(),
            };
            format!("{}[{at}]", written(base))
        }
        ExprKind::Call { name, .. } => format!("{name}(...)"),
        ExprKind::MethodCall { receiver, name, .. } => format!("{}.{name}(...)", written(receiver)),
        _ => "_".to_string(),
    }
}
