//! Runs Kestrel calls on a thread of their own. Each call gets a fresh
//! arena, freed once its result is copied out. A runtime panic parks the
//! thread for good (unwinding through compiled code isn't safe) after
//! sending the error back; the next call starts a new worker. `exit(code)`
//! does the same: the editor's process never ends, and the code is reported,
//! not applied.

use std::cell::RefCell;
use std::sync::mpsc::{channel, Receiver, Sender};

use kestrel_codegen::jit::Thunk;
use kestrel_runtime::arena;

use crate::bridge::Heap;

pub struct Job {
    pub thunk: Thunk,
    pub args: Vec<u8>,
    pub ret_size: usize,
    pub heap: Heap,
}

pub enum Reply {
    Done { ret: Vec<u8>, heap: Vec<u8>, output: Vec<u8> },
    Failed { msg: String, output: Vec<u8> },
    /// The call ran `exit(0)`: a normal end with no result. The worker is parked.
    Exited { output: Vec<u8> },
}

pub struct Worker {
    jobs: Sender<Job>,
    replies: Receiver<Reply>,
}

thread_local! {
    /// Where this worker sends a failure from the runtime's hook.
    static REPLIES: RefCell<Option<Sender<Reply>>> = const { RefCell::new(None) };
}

/// The runtime's failure hook: report, then park this worker forever.
fn on_failure(msg: &str) -> ! {
    let output = kestrel_runtime::print::take_output();
    park_after(Reply::Failed { msg: msg.to_string(), output })
}

/// The runtime's exit hook: `exit(0)` sends the call's output back as a normal end with no result,
/// another code as the failure "the program called exit(3)". Then this worker parks forever, as
/// after a failure, and the next call starts a new one.
fn on_exit(code: i32) -> ! {
    let output = kestrel_runtime::print::take_output();
    if code == 0 {
        park_after(Reply::Exited { output })
    } else {
        park_after(Reply::Failed { msg: format!("the program called exit({code})"), output })
    }
}

/// Sends `reply` for the running call, then parks this worker forever: compiled code can't be
/// unwound.
fn park_after(reply: Reply) -> ! {
    REPLIES.with_borrow(|r| {
        if let Some(tx) = r {
            let _ = tx.send(reply);
        }
    });
    loop {
        std::thread::park();
    }
}

impl Worker {
    pub fn start() -> Result<Worker, String> {
        kestrel_runtime::panic::set_failure_hook(on_failure);
        kestrel_runtime::panic::set_exit_hook(on_exit);
        let (jobs, job_rx) = channel::<Job>();
        let (reply_tx, replies) = channel::<Reply>();
        std::thread::Builder::new()
            .name("kestrel".into())
            .stack_size(256 * 1024 * 1024)
            .spawn(move || run(job_rx, reply_tx))
            .map_err(|e| format!("can't start a Kestrel thread: {e}"))?;
        Ok(Worker { jobs, replies })
    }

    /// Runs `job`, waiting for it. `None` means the worker is gone.
    pub fn call(&self, job: Job) -> Option<Reply> {
        self.jobs.send(job).ok()?;
        self.replies.recv().ok()
    }
}

fn run(jobs: Receiver<Job>, replies: Sender<Reply>) {
    kestrel_runtime::print::capture_output();
    REPLIES.with_borrow_mut(|r| *r = Some(replies.clone()));
    while let Ok(job) = jobs.recv() {
        let a = arena::kestrel_rt_arena_new();
        arena::kestrel_rt_arena_push(a);
        let mut ret = vec![0u8; job.ret_size.max(16)];
        // SAFETY: `args` is laid out as the thunk's `ExportInfo` describes, and
        // the host keeps any memory it points to alive until we reply.
        unsafe { (job.thunk)(job.args.as_ptr(), ret.as_mut_ptr()) };
        let heap = match job.heap {
            Heap::None => Vec::new(),
            Heap::Seq { elem_size } => {
                let ptr = u64::from_le_bytes(ret[..8].try_into().expect("8 bytes")) as *const u8;
                let len = i64::from_le_bytes(ret[8..16].try_into().expect("8 bytes")) as usize;
                if ptr.is_null() || len == 0 {
                    Vec::new()
                } else {
                    // SAFETY: a string or array result points at `len` elements in this call's arena.
                    unsafe { std::slice::from_raw_parts(ptr, len * elem_size) }.to_vec()
                }
            }
        };
        arena::kestrel_rt_arena_pop();
        // SAFETY: `a` came from `kestrel_rt_arena_new` and is no longer pushed.
        unsafe { arena::kestrel_rt_arena_free(a) };
        let output = kestrel_runtime::print::take_output();
        if replies.send(Reply::Done { ret, heap, output }).is_err() {
            break;
        }
    }
}
