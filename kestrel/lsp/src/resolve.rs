//! What the node under the cursor is: how hover shows it, and where it is defined.

use kestrel_check::{FnId, LocalId, Res, Tables, Ty};
use kestrel_driver::Analysis;
use kestrel_syntax::ast::{Expr, ExprKind, Item, Program, TypeKind};
use kestrel_syntax::Span;

use crate::find::{find, Hit};

/// A name or expression, resolved.
#[derive(Debug, PartialEq)]
pub struct Symbol {
    /// What hover shows, written like Kestrel: `var x: int`, `area(w: int) -> int`, `float`.
    pub code: String,
    /// Where it is defined, in the analysis' shared span space.
    pub def: Option<Span>,
}

impl Symbol {
    fn new(code: String, def: Option<Span>) -> Self {
        Symbol { code, def: def.filter(|s| *s != Span::default()) }
    }
}

/// The symbol at shared offset `offset` of `a`.
pub fn resolve(a: &Analysis, offset: u32) -> Option<Symbol> {
    if let Some(s) = in_uses(a, offset) {
        return Some(s);
    }
    let (p, t) = (a.program.as_ref()?, a.tables.as_ref()?);
    let r = Resolver { p, t };
    match find(p, offset)? {
        Hit::Expr(e) => Some(r.expr(e)),
        Hit::Name(e) => Some(r.name(e)),
        Hit::Binding(b) => match t.bind_local.get(&b.id) {
            Some(&l) => Some(r.local(l)),
            None => r.named(&b.name),
        },
        Hit::Type(ty) => match &ty.kind {
            TypeKind::Named { name, .. } => r.named(name).or_else(|| Some(Symbol::new(name.clone(), None))),
            _ => None,
        },
        Hit::Decl(span) => r.decl(span),
        Hit::Iface(name, _) => r.named(name),
        Hit::Pattern { matched, arm, .. } => r.pattern(matched, arm),
        Hit::Remove { stmt, .. } => t.removes.get(&stmt).map(|&l| r.local(l)),
    }
}

/// A name in a `use` line of the entry file: the item it imports, or the module's file.
fn in_uses(a: &Analysis, offset: u32) -> Option<Symbol> {
    let within = |s: Span| s.start <= offset && offset <= s.end;
    let resolved = a.resolved.first()?;
    for (k, c) in a.uses.first()?.iter().flat_map(|u| &u.clauses).enumerate() {
        // The module the loader found for the clause (a module is the source of its index), so a
        // second spelling of one file's name (`Net.packet` on a disk that ignores case) finds it.
        let source = resolved.get(k).copied().flatten().and_then(|m| a.sources.get(m));
        if within(c.module_span) {
            let source = source?;
            return Some(Symbol::new(format!("module {}", c.module), Some(Span { start: source.base, end: source.base })));
        }
        if let Some((name, _)) = c.names.iter().find(|(_, s)| within(*s)) {
            let (p, t) = (a.program.as_ref()?, a.tables.as_ref()?);
            return Resolver { p, t }.named(&format!("{}.{name}", source?.module));
        }
    }
    None
}

struct Resolver<'a> {
    p: &'a Program,
    t: &'a Tables,
}

impl Resolver<'_> {
    fn ty(&self, ty: Ty) -> String {
        self.t.ty_name(ty)
    }

    fn expr(&self, e: &Expr) -> Symbol {
        let ty = self.ty(self.t.ty(e.id));
        let ExprKind::Ident(name) = &e.kind else { return Symbol::new(ty, None) };
        match self.t.res.get(&e.id) {
            Some(&Res::Local(l)) => self.local(l),
            Some(Res::Const(c)) => {
                let info = &self.t.consts[c.0 as usize];
                let def = match &self.p.items[info.item] {
                    Item::Const(c) => Some(c.bind.span),
                    _ => None,
                };
                Symbol::new(format!("let {}: {}", info.name, self.ty(info.ty)), def)
            }
            Some(&Res::Fn(f) | &Res::FnValue(f)) => self.func(f),
            _ => self.named(name).unwrap_or_else(|| Symbol::new(format!("{name}: {ty}"), None)),
        }
    }

    /// The name part of a call, method call, field access or type call.
    fn name(&self, e: &Expr) -> Symbol {
        let ty = self.t.ty(e.id);
        let res = self.t.res.get(&e.id).copied();
        let by_res = match res {
            Some(Res::Fn(f) | Res::FnValue(f)) => Some(self.func(f)),
            Some(Res::IfaceCall { iface, slot } | Res::ParamCall { iface, slot, .. }) => Some(self.func(self.t.iface(iface).methods[slot as usize])),
            Some(Res::Construct(s)) => Some(self.strukt(s)),
            Some(Res::Variant(en, v)) => Some(self.variant(en, v)),
            Some(Res::Local(l)) => Some(self.local(l)),
            Some(Res::CallValue(_)) => self.t.call_local.get(&e.id).map(|&l| self.local(l)),
            _ => None,
        };
        if let Some(s) = by_res {
            return s;
        }
        match &e.kind {
            // A field, or a call of a function held in a field (`b.on_click()`).
            ExprKind::Field { base, name, .. } | ExprKind::MethodCall { receiver: base, name, .. } => {
                if let Some(s) = self.field(self.t.ty(base.id), e, name) {
                    return s;
                }
                Symbol::new(format!("{name}: {}", self.ty(ty)), None)
            }
            // `Maybe` in `Maybe[int].Nothing`: the declaration (a type has no resolution here).
            ExprKind::TypeApply { name, .. } => self.named(name).unwrap_or_else(|| Symbol::new(format!("{name}: {}", self.ty(ty)), None)),
            ExprKind::Call { name, .. } | ExprKind::TypeCall { name, .. } => Symbol::new(format!("{name}: {}", self.ty(ty)), None),
            _ => Symbol::new(self.ty(ty), None),
        }
    }

    fn field(&self, base: Ty, e: &Expr, name: &str) -> Option<Symbol> {
        let index = self.t.field_index.get(&e.id).map(|&i| i as usize);
        let (owner, fields) = match base {
            Ty::Struct(s) => (self.t.strukt(s).name.clone(), &self.t.strukt(s).fields),
            Ty::Variant(en, v) => {
                let info = self.t.enm(en);
                (format!("{}.{}", info.name, info.variants[v as usize].name), &info.variants[v as usize].fields)
            }
            _ => return None,
        };
        let f = match index {
            Some(i) => fields.get(i)?,
            None => fields.iter().find(|f| f.name == name)?,
        };
        Some(Symbol::new(format!("{owner}.{}: {}", f.name, self.ty(f.ty)), Some(f.span)))
    }

    fn local(&self, l: LocalId) -> Symbol {
        let info = self.t.local(l);
        let keyword = match (info.is_param, info.is_var_param, info.mutable) {
            (true, true, _) => "var ",
            (true, false, _) => "",
            (false, _, true) => "var ",
            (false, _, false) => "let ",
        };
        Symbol::new(format!("{keyword}{}: {}", info.name, self.ty(info.ty)), Some(info.span))
    }

    fn func(&self, f: FnId) -> Symbol {
        let info = self.t.func(f);
        let params: Vec<String> = info
            .params
            .iter()
            .map(|p| {
                let var = if p.is_var { "var " } else { "" };
                if p.name == "self" {
                    format!("{var}self")
                } else {
                    format!("{var}{}: {}", p.name, self.ty(p.ty))
                }
            })
            .collect();
        let ret = if info.ret == Ty::Unit { String::new() } else { format!(" -> {}", self.ty(info.ret)) };
        let pure = if info.is_pure { "pure " } else { "" };
        let native = if info.is_native { "native " } else { "" };
        Symbol::new(format!("{native}{pure}{}({}){ret}", info.name, params.join(", ")), Some(info.span))
    }

    fn strukt(&self, s: kestrel_check::StructId) -> Symbol {
        let info = self.t.strukt(s);
        let fields: Vec<String> = info.fields.iter().map(|f| format!("    {}: {}", f.name, self.ty(f.ty))).collect();
        let body = if fields.is_empty() { " {}".to_string() } else { format!(" {{\n{}\n}}", fields.join("\n")) };
        Symbol::new(format!("struct {}{body}", info.name), Some(info.span))
    }

    fn enm(&self, e: kestrel_check::EnumId) -> Symbol {
        let info = self.t.enm(e);
        let variants: Vec<String> = info.variants.iter().map(|v| format!("    {}", self.variant_text(&v.name, &v.fields))).collect();
        Symbol::new(format!("enum {} {{\n{}\n}}", info.name, variants.join("\n")), Some(info.span))
    }

    fn variant(&self, e: kestrel_check::EnumId, v: u32) -> Symbol {
        let info = self.t.enm(e);
        let var = &info.variants[v as usize];
        Symbol::new(format!("{}.{}", info.name, self.variant_text(&var.name, &var.fields)), Some(var.span))
    }

    fn variant_text(&self, name: &str, fields: &[kestrel_check::FieldInfo]) -> String {
        if fields.is_empty() {
            return name.to_string();
        }
        let fields: Vec<String> = fields.iter().map(|f| format!("{}: {}", f.name, self.ty(f.ty))).collect();
        format!("{name}({})", fields.join(", "))
    }

    /// A struct, enum, interface, function or constant by its (qualified) name.
    fn named(&self, name: &str) -> Option<Symbol> {
        let t = self.t;
        if let Some(&s) = t.struct_by_name.get(name) {
            return Some(self.strukt(s));
        }
        if let Some(&e) = t.enum_by_name.get(name) {
            return Some(self.enm(e));
        }
        if let Some(&i) = t.iface_by_name.get(name) {
            let info = t.iface(i);
            let methods: Vec<String> = info.methods.iter().map(|&m| format!("    {}", self.func(m).code)).collect();
            return Some(Symbol::new(format!("interface {} {{\n{}\n}}", info.name, methods.join("\n")), Some(info.span)));
        }
        if let Some(&f) = t.fn_by_name.get(name) {
            return Some(self.func(f));
        }
        let &c = t.const_by_name.get(name)?;
        let info = &t.consts[c.0 as usize];
        let def = match &self.p.items[info.item] {
            Item::Const(c) => Some(c.bind.span),
            _ => None,
        };
        Some(Symbol::new(format!("let {}: {}", info.name, self.ty(info.ty)), def))
    }

    /// The declaration whose name is at `span`.
    fn decl(&self, span: Span) -> Option<Symbol> {
        // The built-in vectors' structs and fields have no span.
        if span == Span::default() {
            return None;
        }
        let t = self.t;
        if let Some(f) = t.fns.iter().position(|f| f.span == span) {
            return Some(self.func(FnId(f as u32)));
        }
        if let Some(s) = t.structs.iter().position(|s| s.span == span) {
            return Some(self.strukt(kestrel_check::StructId(s as u32)));
        }
        for info in &t.structs {
            if let Some(f) = info.fields.iter().find(|f| f.span == span) {
                return Some(Symbol::new(format!("{}.{}: {}", info.name, f.name, self.ty(f.ty)), Some(f.span)));
            }
        }
        for (i, info) in t.enums.iter().enumerate() {
            let id = kestrel_check::EnumId(i as u32);
            if info.span == span {
                return Some(self.enm(id));
            }
            for (v, var) in info.variants.iter().enumerate() {
                if var.span == span {
                    return Some(self.variant(id, v as u32));
                }
                if let Some(f) = var.fields.iter().find(|f| f.span == span) {
                    return Some(Symbol::new(format!("{}.{}.{}: {}", info.name, var.name, f.name, self.ty(f.ty)), Some(f.span)));
                }
            }
        }
        let i = t.ifaces.iter().find(|i| i.span == span)?;
        self.named(&i.name)
    }

    fn pattern(&self, matched: &Expr, arm: usize) -> Option<Symbol> {
        let ExprKind::Match { scrutinee, .. } = &matched.kind else { return None };
        let v = (*self.t.match_arms.get(&matched.id)?.get(arm)?)?;
        let mut ty = self.t.ty(scrutinee.id);
        if let Ty::Optional(e) = ty {
            ty = self.t.elem(e);
        }
        match ty {
            Ty::Enum(e) => Some(self.variant(e, v)),
            _ => None,
        }
    }
}
