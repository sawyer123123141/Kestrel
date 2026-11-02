//! The replay file that `kestrel run --record` writes and `kestrel replay` reads (the I/O round's
//! spec, decision 7). Little-endian throughout, with no padding, in the byte format's own words:
//!
//! - the 4 bytes `KESR` (`MAGIC`), then the format version as a `u8` (`VERSION`);
//! - four strings, each a `u32` byte length then its UTF-8 bytes: the program's path as it was
//!   given, the compiler's version, the build (`checked` or `release`) and the window mode
//!   (`real`, `hidden`, `headless` or `headless-quiet`);
//! - the program's arguments (`args.all()`): a `u32` count, then each as a string;
//! - the frame count, a `u64`: how many `window.frame()` calls gave `true`;
//! - the native log: a `u32` byte length, then its entries, each a native's id byte then its
//!   result (see `replay::NativeId`);
//! - the output log: a `u32` entry count, then each entry as a kind byte (0 a line that ended in
//!   a line break, 1 the last line, with no line break, 2 the failure that stopped the program)
//!   then its text as a string; or the kind byte 3, the program called `exit`, then its code as
//!   an `i32` (the language round's spec, decision 5). A failure or an exit is the last entry; a
//!   run that ended normally has neither, and ended with code 0.
//!
//! Nothing may follow. A file that breaks any of this is not a replay file. Version 1 files, which
//! have no exit entries, are read too.

use std::path::Path;

/// The first four bytes of every replay file.
pub const MAGIC: [u8; 4] = *b"KESR";

/// The format version, the file's fifth byte.
pub const VERSION: u8 = 2;

/// The builds a recording can come from.
pub const BUILDS: [&str; 2] = ["checked", "release"];

/// One entry of the output log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A printed line, without its line break.
    Line(Vec<u8>),
    /// What was printed after the last line break, when the program ended.
    Partial(Vec<u8>),
    /// The message of the runtime failure that stopped the program: always the last entry.
    Failure(String),
    /// The code the program gave `exit`: always the last entry.
    Exit(i32),
}

/// A replay file's contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayFile {
    pub program: String,
    pub version: String,
    pub build: String,
    pub window: String,
    pub args: Vec<String>,
    pub frames: u64,
    pub natives: Vec<u8>,
    pub output: Vec<Entry>,
}

/// Why bytes are not a replay file this Kestrel can read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unreadable {
    NotReplay,
    Newer,
}

impl Unreadable {
    /// The sentence for a file shown as `name`.
    pub fn message(self, name: &str) -> String {
        match self {
            Unreadable::NotReplay => {
                format!("{name} is not a Kestrel replay file; record one with `kestrel run game.kes --record {name}`")
            }
            Unreadable::Newer => format!("{name} was written by a newer Kestrel; replay it with that version"),
        }
    }
}

/// Appends a `u32` length or count, or says what is too long for one.
fn put_count(n: usize, what: &str, out: &mut Vec<u8>) -> Result<(), String> {
    let n = u32::try_from(n).map_err(|_| format!("the replay's {what} is too long to save"))?;
    out.extend_from_slice(&n.to_le_bytes());
    Ok(())
}

fn put_bytes(bytes: &[u8], what: &str, out: &mut Vec<u8>) -> Result<(), String> {
    put_count(bytes.len(), what, out)?;
    out.extend_from_slice(bytes);
    Ok(())
}

/// Reads the parts of a replay file in order; every read fails past the end.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Unreadable> {
        let end = self.at.checked_add(n).filter(|&end| end <= self.bytes.len()).ok_or(Unreadable::NotReplay)?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn u8(&mut self) -> Result<u8, Unreadable> {
        Ok(self.take(1)?[0])
    }

    fn count(&mut self) -> Result<usize, Unreadable> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }

    fn bytes(&mut self) -> Result<Vec<u8>, Unreadable> {
        let n = self.count()?;
        Ok(self.take(n)?.to_vec())
    }

    fn string(&mut self) -> Result<String, Unreadable> {
        String::from_utf8(self.bytes()?).map_err(|_| Unreadable::NotReplay)
    }
}

impl ReplayFile {
    /// The file's bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut out = MAGIC.to_vec();
        out.push(VERSION);
        for (text, what) in [(&self.program, "program path"), (&self.version, "version"), (&self.build, "build"), (&self.window, "window mode")] {
            put_bytes(text.as_bytes(), what, &mut out)?;
        }
        put_count(self.args.len(), "argument list", &mut out)?;
        for arg in &self.args {
            put_bytes(arg.as_bytes(), "argument", &mut out)?;
        }
        out.extend_from_slice(&self.frames.to_le_bytes());
        put_bytes(&self.natives, "native log", &mut out)?;
        put_count(self.output.len(), "output log", &mut out)?;
        for entry in &self.output {
            let (kind, text) = match entry {
                Entry::Line(text) => (0, text.as_slice()),
                Entry::Partial(text) => (1, text.as_slice()),
                Entry::Failure(msg) => (2, msg.as_bytes()),
                Entry::Exit(code) => {
                    out.push(3);
                    out.extend_from_slice(&code.to_le_bytes());
                    continue;
                }
            };
            out.push(kind);
            put_bytes(text, "output line", &mut out)?;
        }
        Ok(out)
    }

    /// Reads a replay file's bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<ReplayFile, Unreadable> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != MAGIC {
            return Err(Unreadable::NotReplay);
        }
        let file_version = r.u8()?;
        if file_version > VERSION {
            return Err(Unreadable::Newer);
        }
        if file_version == 0 {
            return Err(Unreadable::NotReplay);
        }
        let (program, version, build, window) = (r.string()?, r.string()?, r.string()?, r.string()?);
        if !BUILDS.contains(&build.as_str()) || crate::replay::WindowMode::parse(&window).is_none() {
            return Err(Unreadable::NotReplay);
        }
        let count = r.count()?;
        // Each argument takes at least its 4-byte length, so a count can't claim more than that.
        if count > (bytes.len() - r.at) / 4 {
            return Err(Unreadable::NotReplay);
        }
        let args = (0..count).map(|_| r.string()).collect::<Result<Vec<_>, _>>()?;
        let f = r.take(8)?;
        let frames = u64::from_le_bytes(f.try_into().expect("8 bytes"));
        let natives = r.bytes()?;
        let count = r.count()?;
        if count > (bytes.len() - r.at) / 5 {
            return Err(Unreadable::NotReplay);
        }
        let mut output = Vec::with_capacity(count);
        for i in 0..count {
            let last = i + 1 == count;
            output.push(match r.u8()? {
                0 => Entry::Line(r.bytes()?),
                // Last, or just before the failure or exit that ended the run after it.
                1 if i + 2 >= count => Entry::Partial(r.bytes()?),
                2 if last => Entry::Failure(String::from_utf8(r.bytes()?).map_err(|_| Unreadable::NotReplay)?),
                3 if last && file_version >= 2 => Entry::Exit(i32::from_le_bytes(r.take(4)?.try_into().expect("4 bytes"))),
                _ => return Err(Unreadable::NotReplay),
            });
        }
        if let [.., Entry::Partial(_), last] = &output[..] {
            if !matches!(last, Entry::Failure(_) | Entry::Exit(_)) {
                return Err(Unreadable::NotReplay);
            }
        }
        if r.at != bytes.len() {
            return Err(Unreadable::NotReplay);
        }
        Ok(ReplayFile { program, version, build, window, args, frames, natives, output })
    }

    /// Writes the file at `path`, replacing any file there.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let bytes = self.to_bytes()?;
        std::fs::write(path, bytes).map_err(|e| {
            format!("could not write the replay file {}: {}", path.display(), crate::natives::error::os_words(&e))
        })
    }

    /// Reads the file at `path`, or gives the sentence that says why it can't be replayed.
    pub fn read(path: &Path) -> Result<ReplayFile, String> {
        let name = path.display().to_string();
        let bytes = std::fs::read(path)
            .map_err(|e| format!("could not read the replay file {name}: {}", crate::natives::error::os_words(&e)))?;
        ReplayFile::from_bytes(&bytes).map_err(|why| why.message(&name))
    }

    /// The note `kestrel replay` prints on stderr before it runs, when the recording was made
    /// from another program file, by another version of Kestrel or in another build: `name` is
    /// the replay file as given, and the rest describe this run. `None` when all of them agree.
    pub fn mismatch_note(&self, name: &str, program: &str, version: &str, build: &str) -> Option<String> {
        let file_name = |p: &str| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (recorded_name, this_name) = (file_name(&self.program), file_name(program));
        let mut now = Vec::new();
        if recorded_name != this_name {
            now.push(this_name);
        }
        if self.version != version {
            now.push(version.to_string());
        }
        if self.build != build {
            now.push(format!("a {build} build"));
        }
        if now.is_empty() {
            return None;
        }
        Some(format!(
            "{name} was recorded from {recorded_name} by Kestrel {} in a {} build; this is {}, so the replay may differ",
            self.version,
            self.build,
            now.join(", ")
        ))
    }
}
