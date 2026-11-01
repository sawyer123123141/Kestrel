//! `kestrel run --record <file.replay>` and `kestrel replay <file.kes> <file.replay>` (the I/O
//! round's spec, decision 7). The recorder and the player are the runtime's
//! (`kestrel_runtime::replay`); this starts them around `main` and reports what they found.

use std::path::Path;

use kestrel_runtime::replay::{self, Ending, Recording, Report, WindowMode};
use kestrel_runtime::replay_file::ReplayFile;

use crate::Args;

/// The compiler's version, which a recording names.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The build a recording names: `checked`, or `release` for `--release`.
fn build_name(args: &Args) -> &'static str {
    if args.release {
        "release"
    } else {
        "checked"
    }
}

/// Reads the replay file `file` for `replay`, and notes on stderr when it was recorded from
/// another program file, Kestrel or build (the replay still runs: the bits may still agree).
pub fn read(file: &str, args: &Args) -> Result<ReplayFile, String> {
    let read = ReplayFile::read(Path::new(file))?;
    if let Some(note) = read.mismatch_note(file, &args.path, VERSION, build_name(args)) {
        eprintln!("kestrel: {note}");
    }
    Ok(read)
}

/// Starts replaying `replaying`, or recording when `--record` was given. Nothing otherwise.
pub fn start(args: &Args, replaying: Option<ReplayFile>) {
    if let Some(file) = replaying {
        replay::start_replay(file);
    } else if let Some(path) = &args.record {
        // Settings the window can't read make `window.open` fail with their own message; a
        // program that opens no window doesn't care, and its recording says `real`.
        #[cfg(windows)]
        let window = kestrel_window::window_mode().unwrap_or(WindowMode::Real);
        // Only Windows builds have the window, and elsewhere a window program never gets here.
        #[cfg(not(windows))]
        let window = WindowMode::Real;
        replay::start_recording(Recording {
            path: path.into(),
            program: args.path.clone(),
            version: VERSION.to_string(),
            build: build_name(args).to_string(),
            window,
            args: args.program_args.clone(),
        });
    }
}

/// Ends recording or replaying once `main` has returned with exit code `code`: writes the
/// recording, or prints the replay's report and gives its exit code (0 matches, 1 differs).
pub fn finish(code: i32) -> Result<i32, String> {
    let (text, code) = outcome(code, &replay::finish(Ending::Returned)?);
    if let Some(text) = text {
        println!("{text}");
    }
    Ok(code)
}

/// What `kestrel replay` prints and the exit code it gives, for a run whose `main` gave `code` and
/// ended with `report`. A run that ends with a code of its own (a failure inside the runtime that
/// the recording does not have) stopped the replay: exit 1, never a match. Recording, or neither,
/// prints nothing and keeps `code`.
fn outcome(code: i32, report: &Report) -> (Option<String>, i32) {
    match report {
        Report::Off | Report::Recorded => (None, code),
        _ if code != 0 => (Some(format!("replay stopped: the program failed with exit code {code}")), 1),
        report => (report.text(), report.exit_code()),
    }
}

#[cfg(test)]
mod tests {
    use super::{outcome, Report};

    #[test]
    fn a_replay_that_ends_with_its_own_exit_code_never_matches() {
        let matches = Report::Matches { frames: 5, lines: 5 };
        assert_eq!(outcome(0, &matches), (Some("replay matches (5 frames, 5 lines)".to_string()), 0));
        let stopped = Some("replay stopped: the program failed with exit code 101".to_string());
        assert_eq!(outcome(101, &matches), (stopped.clone(), 1));
        assert_eq!(outcome(101, &Report::Differs("replay differs at frame 2".to_string())), (stopped, 1));
        assert_eq!(outcome(0, &Report::Differs("replay differs at frame 2".to_string())), (Some("replay differs at frame 2".to_string()), 1));
        // Recording, or neither: the program's own code.
        assert_eq!(outcome(3, &Report::Recorded), (None, 3));
        assert_eq!(outcome(0, &Report::Off), (None, 0));
    }
}
