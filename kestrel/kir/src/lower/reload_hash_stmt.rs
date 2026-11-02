//! The statement half of `setup`'s hash (see `reload_hash`): what each statement is, without the
//! statements, expressions and places inside it, which the walk visits on its own.

use super::reload_hash::{len, Hasher};
use crate::ir::*;

impl Hasher<'_> {
    /// What a statement is, without the statements, expressions and places inside it (the walk
    /// visits those) and without its lines.
    pub(super) fn stmt(&mut self, s: &Stmt) {
        let (m, f) = (self.m, self.f);
        match s {
            Stmt::Let(l, _) => self.tag("let", &[u64::from(l.0)]),
            Stmt::Assign(..) => self.tag("assign", &[]),
            Stmt::If { then_body, else_body, .. } => self.tag("if", &[len(then_body), len(else_body)]),
            Stmt::While { body, .. } => self.tag("while", &[len(body)]),
            Stmt::ForRange { var, body, .. } => self.tag("for", &[u64::from(var.0), len(body)]),
            Stmt::Break => self.tag("break", &[]),
            Stmt::Continue => self.tag("continue", &[]),
            Stmt::Return(v) => self.tag("return", &[u64::from(v.is_some())]),
            Stmt::Call(g, args, _) => {
                self.tag("call", &[]);
                self.func(*g);
                self.args(args);
            }
            Stmt::CallIndirect { args, sig, .. } => {
                self.tag("call indirect", &[]);
                self.sig(sig);
                self.args(args);
            }
            Stmt::CallVirtual { slot, args, sig, keeps, method, .. } => self.virtual_call(*slot, sig, *keeps, *method, args),
            Stmt::Print(parts) => {
                self.tag("print", &[]);
                self.print_parts(parts);
            }
            Stmt::Write(parts) => {
                self.tag("write", &[]);
                self.print_parts(parts);
            }
            Stmt::RandSeed(_) => self.tag("rand seed", &[]),
            Stmt::ListAdd(..) => self.tag("list add", &[]),
            Stmt::List(op, _, xs) => {
                self.tag("list", &[len(xs)]);
                self.debug(op);
            }
            Stmt::SetLen(..) => self.tag("set len", &[]),
            Stmt::CopyItems(..) => self.tag("copy items", &[]),
            Stmt::Bind(l, _) => self.tag("bind", &[u64::from(l.0)]),
            // A heap constant: by the constant's name (its local's) and type, not its global.
            Stmt::BindGlobal(l, g) => {
                self.tag("bind global", &[u64::from(l.0)]);
                self.str(&f.locals[l.0 as usize].name);
                self.ty(m.globals[g.0 as usize]);
            }
            Stmt::BindEnv { local, offset, .. } => self.tag("bind env", &[u64::from(local.0), u64::from(*offset)]),
            Stmt::CopyFnEnv(_) => self.tag("copy fn env", &[]),
            Stmt::CopyIface(_) => self.tag("copy iface", &[]),
            Stmt::PushArena(p, _) => self.tag("push arena", &[u64::from(p.is_some())]),
            Stmt::PopArena { free } => self.tag("pop arena", &[u64::from(*free)]),
            Stmt::FreeArena(_) => self.tag("free arena", &[]),
            Stmt::DropGpu(_) => self.tag("drop gpu", &[]),
            Stmt::Dup(_) => self.tag("dup", &[]),
            Stmt::MapSet(_, _, _, copy_key) => self.tag("map set", &[u64::from(*copy_key)]),
            Stmt::MapUnlock(..) => self.tag("map unlock", &[]),
            Stmt::ParFor { func, args, .. } => {
                self.tag("par for", &[]);
                self.func(*func);
                self.args(args);
            }
            Stmt::SetLine(_) => self.tag("set line", &[]),
            Stmt::Trap => self.tag("trap", &[]),
            // (The message and the code are expressions the walk visits.)
            Stmt::Fail { .. } => self.tag("fail", &[]),
            Stmt::Exit { .. } => self.tag("exit", &[]),
            // An assumption and a contract by what they say, not by their ids.
            Stmt::Assume { conds, check, body } => {
                self.tag("assume", &[len(conds), len(body)]);
                for (id, _) in conds {
                    self.str(&m.assumptions[*id as usize].text);
                }
                self.debug(check);
            }
            Stmt::Prove { id, at, body } => {
                let site = &m.contracts[*id as usize];
                self.tag("prove", &[len(&site.contracts), u64::from(*at != 0), len(body)]);
                for (c, _) in &site.contracts {
                    self.debug(c);
                }
            }
        }
    }
}
