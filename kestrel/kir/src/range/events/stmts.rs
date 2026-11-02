//! The statements of the events walk: each one's operands, then what it does.

use super::{Effect, Walk, What, MAX_DEPTH};
use crate::ir::*;
use crate::range::show::place_text;

impl Walk<'_> {
    pub(super) fn stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    pub(super) fn stmt(&mut self, s: &Stmt) {
        if self.depth >= MAX_DEPTH {
            return self.cut();
        }
        self.depth += 1;
        self.stmt_inner(s);
        self.depth -= 1;
    }

    fn stmt_inner(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(_, e) | Stmt::Return(Some(e)) => self.expr(e),
            Stmt::RandSeed(e) => {
                self.expr(e);
                self.effect(e.line, Effect::SharedRandom, "calls `rand_seed`".to_string());
            }
            Stmt::Assign(p, e) => {
                self.expr(e);
                self.place_walk(p);
            }
            Stmt::If { cond, then_body, else_body } => {
                self.expr(cond);
                self.stmts(then_body);
                self.stmts(else_body);
            }
            Stmt::Assume { conds, check, body } => {
                for (id, cond) in conds {
                    self.expr(cond);
                    if *check == AssumeCheck::Check {
                        self.assumption(*id);
                    }
                }
                self.stmts(body);
            }
            Stmt::Prove { id, at, body } => {
                let f = self.f;
                let copy = self.copies.get_or_insert_with(|| crate::contract::prove_copies(&f.body))[&(s as *const Stmt as usize)];
                self.regions.push(Region { id: *id, at: *at, copy });
                self.stmts(body);
                self.regions.pop();
            }
            Stmt::While { cond, body } => {
                let looped = cond.line != 0;
                if looped {
                    self.loops.push(cond.line);
                }
                self.expr(cond);
                self.stmts(body);
                if looped {
                    self.loops.pop();
                }
            }
            Stmt::ForRange { start, end, body, .. } => {
                self.expr(start);
                self.expr(end);
                let looped = start.line != 0;
                if looped {
                    self.loops.push(start.line);
                }
                self.stmts(body);
                if looped {
                    self.loops.pop();
                }
            }
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::BindGlobal(..) | Stmt::PopArena { .. } | Stmt::DropGpu(_) => {
                if let Stmt::DropGpu(p) = s {
                    self.place_walk(p);
                }
            }
            Stmt::Call(g, args, at) => {
                self.args(args);
                let line = if *at != 0 { *at } else { crate::visit::args_line(args) };
                if line != 0 {
                    self.cur = line;
                }
                self.call(*g, line);
            }
            Stmt::CallIndirect { callee, args, sig } => {
                self.place_walk(callee);
                self.args(args);
                self.indirect(callee, crate::visit::args_line(args), sig.pure);
            }
            Stmt::CallVirtual { recv, args, keeps, method, sig, .. } => {
                self.place_walk(recv);
                self.args(args);
                self.virtual_call(recv, crate::visit::args_line(args), *keeps, *method, sig.pure);
            }
            Stmt::Print(parts) | Stmt::Write(parts) => self.print(parts),
            Stmt::ListAdd(p, value) => {
                self.expr(value);
                self.place_walk(p);
                self.list_add(p, value.line);
            }
            Stmt::List(op, p, xs) => {
                for x in xs {
                    self.expr(x);
                }
                self.place_walk(p);
                self.list_op(*op, p, xs);
            }
            Stmt::SetLen(p, n) => {
                self.expr(n);
                self.place_walk(p);
            }
            // It copies into a buffer that is already there: no event.
            Stmt::CopyItems(dst, src) => {
                self.place_walk(src);
                self.place_walk(dst);
            }
            Stmt::Bind(_, p) => self.place_walk(p),
            Stmt::BindEnv { env, .. } => self.expr(env),
            Stmt::CopyFnEnv(_) | Stmt::CopyIface(_) | Stmt::PushArena(..) | Stmt::FreeArena(_) | Stmt::Dup(_) | Stmt::MapSet(..) | Stmt::Trap => self.memory_stmt(s),
            Stmt::Fail { message, line } => self.fail_stmt(message, *line),
            // `exit` breaks no contract: what it does depends only on its code.
            Stmt::Exit { code, .. } => self.expr(code),
            Stmt::MapUnlock(p, saved) => {
                self.expr(saved);
                self.place_walk(p);
            }
            Stmt::ParFor { func, args, count } => {
                self.expr(count);
                self.args(args);
                let line = count.line;
                self.alloc(line, Some(super::PAR_ARENAS.to_string()), "runs a `par for`".to_string(), " that makes an arena for each thread");
                self.push(line, What::Body(*func));
            }
            Stmt::SetLine(line) => {
                if *line != 0 {
                    self.cur = *line;
                }
            }
        }
    }

    pub(super) fn args(&mut self, args: &[Arg]) {
        for a in args {
            match a {
                Arg::Value(e) => self.expr(e),
                Arg::Ref(p) => self.place_walk(p),
            }
        }
    }

    pub(super) fn print(&mut self, parts: &[PrintArg]) {
        for p in parts {
            match p {
                PrintArg::Value(e) => self.expr(e),
                PrintArg::Interp(inner) => self.print(inner),
                PrintArg::Str(_) => {}
            }
        }
    }

    /// The expressions inside a place, and its checked map entries (which fail when the loop's
    /// map was replaced).
    pub(super) fn place_walk(&mut self, p: &Place) {
        for (i, proj) in p.path.iter().enumerate() {
            if let Some(at) = proj.at() {
                self.expr(at);
            }
            if let Proj::MapValue { at, checked: true } = proj {
                let text = place_text(self.m, self.f, p, i + 1);
                self.fail(
                    at.line,
                    Some(format!("using `{text}` fails if the map was replaced while looping over it")),
                    format!("uses `{text}`"),
                    " and fails if the map was replaced while looping over it",
                );
            }
        }
    }

    /// An assumption's condition evaluated here: a false one stops the program.
    fn assumption(&mut self, id: u32) {
        let info = &self.m.assumptions[id as usize];
        let (text, line) = (info.text.clone(), info.line);
        self.fail(line, Some(format!("the assumption `{text}` is checked here")), format!("checks the assumption `{text}`"), "");
    }
}
