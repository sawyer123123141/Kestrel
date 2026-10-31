//! The call exclusivity rule: while a call changes a place, nothing else in the call may refer to
//! it by reference. Places are compared by their fields, and a message names both places.

use kestrel_syntax::ast::{Arg, Expr, ExprKind};
use kestrel_syntax::Span;

use super::args::Receiver;
use super::lists::written;
use super::regions::{tail, PathKey};
use super::BodyChecker;
use crate::tables::LocalId;
use crate::ty::Ty;

/// A changed place: its key, its type and the expression written for it (none for a receiver
/// the call has no expression for).
type Changed<'e> = (PathKey, Ty, Option<&'e Expr>);

/// How another place meets a changed one, for the messages.
#[derive(Clone, Copy, PartialEq)]
enum Meet {
    /// The same place, as written.
    Same,
    /// Elements of one list, maybe the same one.
    Element,
    /// Lanes of one `vec4` or `ivec4`.
    Lane,
    /// The other place is inside the changed one.
    Inside,
    /// The other place holds the changed one.
    Holds,
    /// They may overlap some other way (an index on the way to a field).
    Overlaps,
    /// They don't overlap, but the changed place holds an `Arena`.
    Arena,
}

impl Meet {
    /// The clause after "and" that says how `other` (quoted already) meets the changed place.
    fn clause(self, other: &str) -> String {
        match self {
            Meet::Same => format!("{other} is the same place"),
            Meet::Element => format!("{other} may be the same element"),
            Meet::Lane => format!("{other} is a lane of the same vector"),
            Meet::Inside => format!("{other} is inside it"),
            Meet::Holds => format!("{other} holds it"),
            Meet::Overlaps | Meet::Arena => format!("{other} overlaps it"),
        }
    }

    /// The help for two arguments that meet this way.
    fn help(self) -> &'static str {
        match self {
            Meet::Same | Meet::Element => "copy one into a separate variable first",
            Meet::Lane => "copy one lane into a separate variable first",
            Meet::Inside | Meet::Holds | Meet::Overlaps => "pass separate fields, or copy one into a separate variable first",
            Meet::Arena => "copy what you need into a separate variable first",
        }
    }
}

impl<'a> BodyChecker<'a> {
    /// While a call changes a place, nothing else in the call may refer to it by reference: no
    /// second `var` use, and no aggregate argument (those are passed by pointer, so they would see
    /// the change). Number arguments are copied before the call, so they're fine. Places are
    /// compared by their fields, so `step(var g.player, g.world)` passes: the call can't change
    /// `g.world` through `g.player`. A changed place that holds an `Arena` clashes with all of its
    /// variable, since replacing it frees memory a sibling field may hold.
    pub(super) fn check_exclusive(&mut self, args: &[Arg], var_roots: &[(LocalId, Option<usize>)], recv: Option<Receiver<'_>>) {
        let changed: Vec<Changed<'_>> = var_roots
            .iter()
            .map(|&(root, ai)| {
                let e = match ai {
                    Some(ai) => Some(&args[ai].value),
                    None => recv.map(|r| r.expr),
                };
                let key = e.and_then(|e| self.exclusive_key(e)).filter(|k| k.root == root);
                (key.unwrap_or(PathKey { root, steps: Vec::new() }), e.map_or(Ty::Error, |e| self.t.ty(e.id)), e)
            })
            .collect();
        let clash = |c: &Changed<'_>, other: &PathKey| c.0.root == other.root && (c.0.overlaps(other) || self.t.holds_arena(c.1));
        let mut errors: Vec<(Span, String, &str)> = Vec::new();
        for (i, &(_, ai)) in var_roots.iter().enumerate() {
            let Some(ai) = ai else { continue };
            let Some(j) = changed[..i].iter().position(|c| clash(c, &changed[i].0) || clash(&changed[i], &c.0)) else { continue };
            // The later place is named first; with no overlap, the one holding the `Arena` is.
            let (c, o) = match self.meet(&changed[i], &changed[j].0, changed[j].2) {
                Meet::Arena if !self.t.holds_arena(changed[i].1) => (&changed[j], &changed[i]),
                _ => (&changed[i], &changed[j]),
            };
            let meet = self.meet(c, &o.0, o.2);
            errors.push((args[ai].span, self.var_clash_message(c, meet, &self.place_text(o.2, &o.0)), meet.help()));
        }
        // A receiver that isn't `var` is passed by pointer too, so it would see the change.
        if let Some(read_root) = recv.and_then(|r| r.read_root) {
            let read_expr = recv.map(|r| r.expr);
            let read = read_expr.and_then(|e| self.exclusive_key(e)).filter(|k| k.root == read_root);
            let read = read.unwrap_or(PathKey { root: read_root, steps: Vec::new() });
            for (i, &(_, ai)) in var_roots.iter().enumerate() {
                if let (true, Some(ai)) = (clash(&changed[i], &read), ai) {
                    let meet = self.meet(&changed[i], &read, read_expr);
                    let text = self.place_text(changed[i].2, &changed[i].0);
                    let (msg, help) = match meet {
                        Meet::Same => (format!("`{text}` is passed as `var` and is also the receiver of this call"), "copy what the method needs into a separate variable first"),
                        Meet::Arena => (self.var_clash_message(&changed[i], meet, ""), meet.help()),
                        _ => {
                            let receiver = format!("the receiver `{}`", self.place_text(read_expr, &read));
                            (format!("`{text}` is passed as `var` and {}", meet.clause(&receiver)), "copy what the method needs into a separate variable first")
                        }
                    };
                    errors.push((args[ai].span, msg, help));
                }
            }
        }
        for (bi, b) in args.iter().enumerate() {
            if var_roots.iter().any(|&(_, ai)| ai == Some(bi)) || !self.t.ty(b.value.id).is_aggregate() {
                continue;
            }
            // An `if` or `match` passes the place its branch ends in, by pointer, like the place
            // itself (P40): `f(var w, if c { w.s } else { "" })` would see `w` change.
            let mut leaves = Vec::new();
            tail_places(&b.value, &mut leaves);
            let shared = leaves.iter().find_map(|&l| {
                let key = self.exclusive_key(l)?;
                let at = changed.iter().position(|c| clash(c, &key))?;
                Some((at, key, l))
            });
            let Some((at, key, leaf)) = shared else { continue };
            let c = &changed[at];
            let meet = self.meet(c, &key, Some(leaf));
            let (msg, help) = match var_roots[at].1 {
                Some(_) => (self.var_clash_message(c, meet, &self.place_text(Some(leaf), &key)), meet.help()),
                None => self.receiver_change_message(c, meet, &self.place_text(Some(leaf), &key)),
            };
            errors.push((b.span, msg, help));
        }
        for (span, msg, help) in errors {
            self.error_help(span, msg, help);
        }
    }

    /// The message for changed place `c`, passed as `var`, meeting another argument `other` (as
    /// written) the way `meet` says.
    fn var_clash_message(&self, c: &Changed<'_>, meet: Meet, other: &str) -> String {
        let text = self.place_text(c.2, &c.0);
        match meet {
            Meet::Same => format!("`{text}` is passed as `var` and is also passed in another argument of the same call"),
            Meet::Arena => {
                let root = &self.t.local(c.0.root).name;
                format!("`{text}` holds an `Arena`, so nothing else from `{root}` can be passed beside it as `var`")
            }
            _ => format!("`{text}` is passed as `var` and {}", meet.clause(&format!("`{other}`"))),
        }
    }

    /// The message and help for changed place `c`, a `var self` receiver, meeting an argument
    /// `other` (as written) the way `meet` says.
    fn receiver_change_message(&self, c: &Changed<'_>, meet: Meet, other: &str) -> (String, &'static str) {
        let text = self.place_text(c.2, &c.0);
        match meet {
            Meet::Same => (format!("`{text}` is changed by this call, so it can't also be passed to it"), "copy it into a separate variable first"),
            Meet::Element => (format!("`{text}` is changed by this call and `{other}` may be the same element"), "copy it into a separate variable first"),
            Meet::Arena => {
                let root = &self.t.local(c.0.root).name;
                let msg = format!("`{text}` holds an `Arena` and is changed by this call, so nothing else from `{root}` can be passed to it");
                (msg, meet.help())
            }
            _ => (format!("`{text}` is changed by this call and {}", meet.clause(&format!("`{other}`"))), "pass a separate field, or copy it into a separate variable first"),
        }
    }

    /// How the place `other` (keyed `ok`) meets the changed place `c`, which it clashes with.
    fn meet(&self, c: &Changed<'_>, ok: &PathKey, other: Option<&Expr>) -> Meet {
        let ck = &c.0;
        if !ck.overlaps(ok) {
            return Meet::Arena;
        }
        if c.2.is_some_and(|e| self.is_lane(e)) || other.is_some_and(|e| self.is_lane(e)) {
            return Meet::Lane;
        }
        let indexed = ck.steps.iter().chain(&ok.steps).any(Option::is_none);
        if ck.steps.len() == ok.steps.len() {
            let same = self.place_text(c.2, ck) == self.place_text(other, ok);
            return match (same, indexed) {
                (true, _) => Meet::Same,
                (false, true) => Meet::Element,
                (false, false) => Meet::Overlaps,
            };
        }
        match (indexed, ok.steps.len() > ck.steps.len()) {
            (true, _) => Meet::Overlaps,
            (false, true) => Meet::Inside,
            (false, false) => Meet::Holds,
        }
    }

    /// Whether `e` is a lane of a `vec4` or `ivec4`, which the rule counts as the whole vector.
    fn is_lane(&self, e: &Expr) -> bool {
        matches!(&e.kind, ExprKind::Field { base, .. } if matches!(self.t.ty(base.id), Ty::Vec4 | Ty::IVec4))
    }

    /// How a message names a place: as written, or by its key when there is no expression.
    fn place_text(&self, e: Option<&Expr>, key: &PathKey) -> String {
        e.map_or_else(|| self.path_name(key), written)
    }

    /// The place `e` refers to, for the exclusivity rule: its variable and fields, an index
    /// standing for any index (`source_key`), and a `vec4` or `ivec4` lane standing for the whole
    /// vector. `None` when `e` isn't a place.
    fn exclusive_key(&self, e: &Expr) -> Option<PathKey> {
        if self.is_lane(e) {
            if let ExprKind::Field { base, .. } = &e.kind {
                return self.exclusive_key(base);
            }
        }
        self.source_key(e)
    }
}

/// The expressions `e`'s value may be without a copy: `e` itself, or the last expression of each
/// branch of an `if` or `match` (looked through again).
fn tail_places<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    match &e.kind {
        ExprKind::If { then_block, else_block, .. } => {
            for t in std::iter::once(then_block).chain(else_block.as_ref()).filter_map(tail) {
                tail_places(t, out);
            }
        }
        ExprKind::Match { arms, .. } => {
            for t in arms.iter().filter_map(|a| tail(&a.body)) {
                tail_places(t, out);
            }
        }
        _ => out.push(e),
    }
}
