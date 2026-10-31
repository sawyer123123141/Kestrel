//! A `kestrel run` that a test talks to while it runs (the hot reload tests): its output is read
//! line by line as it comes, and its program file can be edited under it.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime};

/// How long a test waits for a line it expects before it fails.
pub const WAIT: Duration = Duration::from_secs(20);

pub struct Session {
    child: Child,
    out: Receiver<String>,
    err: Receiver<String>,
    /// The program file the session edits.
    pub path: PathBuf,
    /// The modification time the last edit gave the file.
    stamp: SystemTime,
    /// Every line read so far, to show when a test fails.
    pub outs: Vec<String>,
    pub errs: Vec<String>,
}

/// Sends each line of `pipe` down a channel, from a thread of its own.
fn lines(pipe: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

impl Session {
    /// Starts `cmd`, which runs the program at `path`, with its output piped to the test.
    pub fn start(mut cmd: Command, path: &Path) -> Session {
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("start kestrel");
        let out = lines(child.stdout.take().unwrap());
        let err = lines(child.stderr.take().unwrap());
        let stamp = fs::metadata(path).and_then(|m| m.modified()).unwrap_or(SystemTime::now());
        Session { child, out, err, path: path.to_path_buf(), stamp, outs: Vec::new(), errs: Vec::new() }
    }

    /// Replaces the program with `src` the way an editor that saves atomically does (a new file
    /// renamed over the old), so the host never reads half a file, with a modification time
    /// later than any before.
    pub fn edit(&mut self, src: &str) {
        let tmp = self.path.with_extension("kes.tmp");
        fs::write(&tmp, src).unwrap();
        self.stamp = SystemTime::now().max(self.stamp + Duration::from_millis(10));
        fs::File::options().write(true).open(&tmp).unwrap().set_modified(self.stamp).unwrap();
        fs::rename(&tmp, &self.path).unwrap();
    }

    /// Replaces the program with `src` the way an editor that writes in place does, holding the
    /// file open with no sharing for `hold` (Windows then refuses to let anyone else open it), as
    /// a slow save does.
    #[cfg(windows)]
    pub fn edit_held(&mut self, src: &str, hold: Duration) {
        use std::io::Write;
        use std::os::windows::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new().write(true).truncate(true).share_mode(0).open(&self.path).unwrap();
        file.write_all(src.as_bytes()).unwrap();
        file.flush().unwrap();
        self.stamp = SystemTime::now().max(self.stamp + Duration::from_millis(10));
        file.set_modified(self.stamp).unwrap();
        std::thread::sleep(hold);
        drop(file);
    }

    /// Everything read so far, for a failure message.
    pub fn transcript(&self) -> String {
        format!("stdout:\n{}\nstderr:\n{}", self.outs.join("\n"), self.errs.join("\n"))
    }

    /// The next line of stdout, skipping the headless window's own lines.
    pub fn next_out(&mut self) -> String {
        loop {
            match self.out.recv_timeout(WAIT) {
                Ok(line) => {
                    self.outs.push(line.clone());
                    if !line.starts_with("window.") && !line.starts_with("draw.") {
                        return line;
                    }
                }
                Err(e) => panic!("no stdout line ({e:?}):\n{}", self.transcript()),
            }
        }
    }

    /// Reads stderr until a line containing `needle`, and gives it.
    pub fn wait_err(&mut self, needle: &str) -> String {
        let end = Instant::now() + WAIT;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.err.recv_timeout(left) {
                Ok(line) => {
                    self.errs.push(line.clone());
                    if line.contains(needle) {
                        return line;
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    panic!("no stderr line with {needle:?}:\n{}", self.transcript())
                }
            }
        }
    }

    /// Waits for the program to end by itself and gives its exit status, having read the rest of
    /// both outputs.
    pub fn finish(mut self) -> (ExitStatus, Vec<String>, Vec<String>) {
        let end = Instant::now() + WAIT;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < end, "the program didn't end:\n{}", self.transcript());
            std::thread::sleep(Duration::from_millis(20));
        };
        let outs: Vec<String> = self.out.iter().collect();
        let errs: Vec<String> = self.err.iter().collect();
        self.outs.extend(outs);
        self.errs.extend(errs);
        (status, std::mem::take(&mut self.outs), std::mem::take(&mut self.errs))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
