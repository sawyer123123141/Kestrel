//! Compact prefix forms of AST nodes, for tests.

use crate::ast::*;

impl Stmt {
    pub fn to_sexpr(&self) -> String {
        match self {
            Stmt::Let { is_var, bind, value, else_name, else_block, .. } => {
                let kw = if *is_var { "var" } else { "let" };
                let name = else_name.as_ref().map_or(String::new(), |n| format!("{} ", n.name));
                match else_block {
                    Some(b) => format!("({kw} {} {} else {name}{})", bind.name, value.to_sexpr(), b.to_sexpr()),
                    None => format!("({kw} {} {})", bind.name, value.to_sexpr()),
                }
            }
            Stmt::Assign { target, op, value, .. } => {
                format!("({} {} {})", op.symbol(), target.to_sexpr(), value.to_sexpr())
            }
            Stmt::While { cond, body, .. } => format!("(while {} {})", cond.to_sexpr(), body.to_sexpr()),
            Stmt::ForRange { var, start, end, body, .. } => format!(
                "(for {} {} {} {})",
                var.name,
                start.to_sexpr(),
                end.to_sexpr(),
                body.to_sexpr()
            ),
            Stmt::ForIn { par, index, var, iter, body, .. } => {
                let names = match index {
                    Some(i) => format!("{} {}", i.name, var.name),
                    None => var.name.clone(),
                };
                let head = if *par { "par-for" } else { "for-in" };
                format!("({head} {names} {} {})", iter.to_sexpr(), body.to_sexpr())
            }
            Stmt::Arena { body, .. } => format!("(arena {})", body.to_sexpr()),
            Stmt::In { arena, body, .. } => format!("(in {} {})", arena.to_sexpr(), body.to_sexpr()),
            Stmt::Break(_) => "break".into(),
            Stmt::Continue(_) => "continue".into(),
            Stmt::Return { value: None, .. } => "return".into(),
            Stmt::Return { value: Some(v), .. } => format!("(return {})", v.to_sexpr()),
            Stmt::Remove { name, .. } => format!("(remove {name})"),
            Stmt::Assume { cond, .. } => format!("(assume {})", cond.to_sexpr()),
            Stmt::Expr(e) => e.to_sexpr(),
        }
    }
}

impl Block {
    pub fn to_sexpr(&self) -> String {
        let inner: Vec<String> = self.stmts.iter().map(Stmt::to_sexpr).collect();
        format!("{{{}}}", inner.join("; "))
    }
}

impl Expr {
    /// Compact prefix form for tests: `1 + 2 * 3` is `(+ 1 (* 2 3))`.
    pub fn to_sexpr(&self) -> String {
        match &self.kind {
            ExprKind::Int(n) => n.to_string(),
            ExprKind::Float(f) => f.to_string(),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::None => "none".into(),
            ExprKind::Str(s) => format!("{s:?}"),
            ExprKind::Interp(parts) => {
                let parts: Vec<String> = parts
                    .iter()
                    .map(|p| match p {
                        InterpPart::Lit(s) => format!("{s:?}"),
                        InterpPart::Expr(e) => e.to_sexpr(),
                    })
                    .collect();
                format!("(interp {})", parts.join(" "))
            }
            ExprKind::Ident(name) => name.clone(),
            ExprKind::Unary { op, expr } => {
                let sym = match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Not => "not",
                    UnaryOp::BitNot => "~",
                };
                format!("({sym} {})", expr.to_sexpr())
            }
            ExprKind::Try(inner) => format!("(try {})", inner.to_sexpr()),
            ExprKind::Binary { op, lhs, rhs } => format!("({} {} {})", op.symbol(), lhs.to_sexpr(), rhs.to_sexpr()),
            ExprKind::Call { name, args, .. } => format!("(call {name}{})", args_sexpr(args)),
            ExprKind::Field { base, name, .. } => format!("(. {} {name})", base.to_sexpr()),
            ExprKind::MethodCall { receiver, name, args, .. } => {
                format!("(.call {} {name}{})", receiver.to_sexpr(), args_sexpr(args))
            }
            ExprKind::Match { scrutinee, arms } => {
                let mut s = format!("(match {}", scrutinee.to_sexpr());
                for arm in arms {
                    let pat = match &arm.pattern {
                        Pattern::Variant { name, binding: Some(b), .. } => format!("{name} {}", b.name),
                        Pattern::Variant { name, binding: None, .. } => name.clone(),
                        Pattern::Else(_) => "else".to_string(),
                    };
                    s.push_str(&format!(" [{pat} {}]", arm.body.to_sexpr()));
                }
                s.push(')');
                s
            }
            ExprKind::Index { base, index } => format!("(index {} {})", base.to_sexpr(), index.to_sexpr()),
            ExprKind::Brackets { base, index, types } => {
                let tys: Vec<String> = types.iter().map(TypeExpr::to_sexpr).collect();
                let index = index.as_ref().map_or("-".to_string(), |i| i.to_sexpr());
                format!("(brackets {} [{}] {index})", base.to_sexpr(), tys.join(", "))
            }
            ExprKind::TypeApply { name, type_args, .. } => {
                let tys: Vec<String> = type_args.iter().map(TypeExpr::to_sexpr).collect();
                format!("(apply {name}[{}])", tys.join(", "))
            }
            ExprKind::ArrayLit(items) => {
                let items: Vec<String> = items.iter().map(Expr::to_sexpr).collect();
                format!("[{}]", items.join(" "))
            }
            ExprKind::MapLit(entries) => {
                let entries: Vec<String> = entries.iter().map(|(k, v)| format!(" ({} {})", k.to_sexpr(), v.to_sexpr())).collect();
                format!("(map{})", entries.concat())
            }
            ExprKind::TypeCall { name, type_args, args, .. } => {
                let tys: Vec<String> = type_args.iter().map(TypeExpr::to_sexpr).collect();
                format!("(make {name}[{}]{})", tys.join(", "), args_sexpr(args))
            }
            ExprKind::CallValue { callee, args } => format!("(callv {}{})", callee.to_sexpr(), args_sexpr(args)),
            ExprKind::Closure(c) => {
                let params: Vec<String> = c
                    .params
                    .iter()
                    .map(|p| {
                        let var = if p.is_var { "var " } else { "" };
                        match &p.ty {
                            Some(t) => format!("{var}{}: {}", p.bind.name, t.to_sexpr()),
                            None => format!("{var}{}", p.bind.name),
                        }
                    })
                    .collect();
                let ret = c.ret.as_ref().map_or(String::new(), |r| format!(" -> {}", r.to_sexpr()));
                format!("(closure ({}){ret} {})", params.join(", "), c.body.to_sexpr())
            }
            ExprKind::If { cond, then_block, else_block } => match else_block {
                Some(e) => format!("(if {} {} else {})", cond.to_sexpr(), then_block.to_sexpr(), e.to_sexpr()),
                None => format!("(if {} {})", cond.to_sexpr(), then_block.to_sexpr()),
            },
        }
    }
}

impl TypeExpr {
    pub fn to_sexpr(&self) -> String {
        match &self.kind {
            TypeKind::Named { name, args } if args.is_empty() => name.clone(),
            TypeKind::Named { name, args } => {
                let args: Vec<String> = args.iter().map(TypeExpr::to_sexpr).collect();
                format!("{name}[{}]", args.join(", "))
            }
            TypeKind::Array(inner) => format!("[{}]", inner.to_sexpr()),
            TypeKind::Optional(inner) => format!("{}?", inner.to_sexpr()),
            TypeKind::Fn { params, ret, is_pure } => {
                let params: Vec<String> =
                    params.iter().map(|p| format!("{}{}", if p.is_var { "var " } else { "" }, p.ty.to_sexpr())).collect();
                let pure = if *is_pure { "pure " } else { "" };
                let ret = ret.as_ref().map_or(String::new(), |r| format!(" -> {}", r.to_sexpr()));
                format!("{pure}({}){ret}", params.join(", "))
            }
        }
    }
}

impl TypeParam {
    /// `T`, or `T: Damageable + Named` with its rules.
    pub fn to_sexpr(&self) -> String {
        if self.rules.is_empty() {
            return self.name.clone();
        }
        let rules: Vec<&str> = self.rules.iter().map(|(r, _)| r.as_str()).collect();
        format!("{}: {}", self.name, rules.join(" + "))
    }
}

/// A placeholder list as written: `[T, K: key]`; the empty string for none.
pub fn type_params_sexpr(params: &[TypeParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let params: Vec<String> = params.iter().map(TypeParam::to_sexpr).collect();
    format!("[{}]", params.join(", "))
}

fn args_sexpr(args: &[Arg]) -> String {
    let mut s = String::new();
    for a in args {
        s.push(' ');
        if let Some(l) = &a.label {
            s.push_str(&format!("{l}: "));
        }
        if a.is_var {
            s.push_str("var ");
        }
        s.push_str(&a.value.to_sexpr());
    }
    s
}
