//! Tests of the replay file and of the pieces of `replay` that don't touch its global state
//! (the CLI's `tests/replay.rs` runs whole recordings and replays).

use super::compare::{Check, Keep};
use super::log::Cursor;
use super::*;
use crate::replay_file::{Entry, ReplayFile, Unreadable};

fn sample() -> ReplayFile {
    let mut natives = Vec::new();
    natives.push(NativeId::WindowFrame as u8);
    true.put(&mut natives);
    natives.push(NativeId::WindowDelta as u8);
    (1.0f32 / 60.0).put(&mut natives);
    natives.push(NativeId::InputMouse as u8);
    [3.5f32, -2.0].put(&mut natives);
    ReplayFile {
        program: "games/pong.kes".to_string(),
        version: "0.1.0".to_string(),
        build: "checked".to_string(),
        window: "headless-quiet".to_string(),
        args: vec!["a".to_string(), "b c".to_string(), String::new()],
        frames: 600,
        natives,
        output: vec![Entry::Line(b"frames 600".to_vec()), Entry::Line(Vec::new()), Entry::Partial(b"x".to_vec())],
    }
}

#[test]
fn a_file_reads_back_as_it_was_written() {
    let file = sample();
    let bytes = file.to_bytes().unwrap();
    assert_eq!(&bytes[..5], b"KESR\x02");
    assert_eq!(ReplayFile::from_bytes(&bytes), Ok(file.clone()));

    let mut failed = file;
    failed.output.pop();
    failed.output.push(Entry::Failure("index 2 out of bounds for length 2 on line 8".to_string()));
    assert_eq!(ReplayFile::from_bytes(&failed.to_bytes().unwrap()), Ok(failed));
}

#[test]
fn the_header_is_laid_out_as_documented() {
    let mut file = sample();
    file.args.clear();
    file.natives = vec![1, 1];
    file.output.clear();
    let mut expected = b"KESR\x02".to_vec();
    for s in ["games/pong.kes", "0.1.0", "checked", "headless-quiet"] {
        expected.extend_from_slice(&(s.len() as u32).to_le_bytes());
        expected.extend_from_slice(s.as_bytes());
    }
    expected.extend_from_slice(&0u32.to_le_bytes());
    expected.extend_from_slice(&600u64.to_le_bytes());
    expected.extend_from_slice(&[2, 0, 0, 0, 1, 1]);
    expected.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(file.to_bytes().unwrap(), expected);
}

#[test]
fn a_damaged_file_is_not_a_replay_file() {
    let bytes = sample().to_bytes().unwrap();
    for n in 0..bytes.len() {
        assert_eq!(ReplayFile::from_bytes(&bytes[..n]), Err(Unreadable::NotReplay), "cut to {n} bytes");
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert_eq!(ReplayFile::from_bytes(&longer), Err(Unreadable::NotReplay), "a byte too many");

    let mut newer = bytes.clone();
    newer[4] = 3;
    assert_eq!(ReplayFile::from_bytes(&newer), Err(Unreadable::Newer));
    let mut older = bytes.clone();
    older[4] = 0;
    assert_eq!(ReplayFile::from_bytes(&older), Err(Unreadable::NotReplay));
    assert_eq!(ReplayFile::from_bytes(b"KESB\x01"), Err(Unreadable::NotReplay));

    let mut window = sample();
    window.window = "sideways".to_string();
    assert_eq!(ReplayFile::from_bytes(&window.to_bytes().unwrap()), Err(Unreadable::NotReplay));
    let mut build = sample();
    build.build = "fast".to_string();
    assert_eq!(ReplayFile::from_bytes(&build.to_bytes().unwrap()), Err(Unreadable::NotReplay));
    // Only the last entry may be an unfinished line or a failure.
    let mut early = sample();
    early.output.insert(0, Entry::Failure("boom".to_string()));
    assert_eq!(ReplayFile::from_bytes(&early.to_bytes().unwrap()), Err(Unreadable::NotReplay));
    // A count that claims more than the bytes could hold is refused before anything is made.
    let mut huge = b"KESR\x01".to_vec();
    for s in ["p.kes", "0.1.0", "checked", "real"] {
        huge.extend_from_slice(&(s.len() as u32).to_le_bytes());
        huge.extend_from_slice(s.as_bytes());
    }
    huge.extend_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(ReplayFile::from_bytes(&huge), Err(Unreadable::NotReplay));
}

#[test]
fn unreadable_files_are_named_in_the_sentences() {
    assert_eq!(
        Unreadable::NotReplay.message("run.replay"),
        "run.replay is not a Kestrel replay file; record one with `kestrel run game.kes --record run.replay`"
    );
    assert_eq!(Unreadable::Newer.message("run.replay"), "run.replay was written by a newer Kestrel; replay it with that version");
}

#[test]
fn the_mismatch_note_names_what_changed() {
    let file = sample();
    assert_eq!(file.mismatch_note("run.replay", "other/dir/pong.kes", "0.1.0", "checked"), None, "only the file name counts");
    assert_eq!(
        file.mismatch_note("run.replay", "pong.kes", "0.4.0", "checked").as_deref(),
        Some("run.replay was recorded from pong.kes by Kestrel 0.1.0 in a checked build; this is 0.4.0, so the replay may differ")
    );
    assert_eq!(
        file.mismatch_note("r", "game.kes", "0.1.0", "release").as_deref(),
        Some("r was recorded from pong.kes by Kestrel 0.1.0 in a checked build; this is game.kes, a release build, so the replay may differ")
    );
}

#[test]
fn ids_are_their_bytes_in_order_and_widths_fit_their_values() {
    for (i, id) in NativeId::ALL.into_iter().enumerate() {
        assert_eq!(id as usize, i + 1);
        assert_eq!(NativeId::from_u8(id as u8), Some(id));
    }
    assert_eq!(NativeId::from_u8(0), None);
    assert_eq!(NativeId::from_u8(19), None);
    assert_eq!(NativeId::from_u8(255), None);
    let names: std::collections::HashSet<_> = NativeId::ALL.iter().map(|id| id.name()).collect();
    assert_eq!(names.len(), 18);
    assert_eq!((NativeId::WindowFrame.width(), bool::SIZE), (1, 1));
    assert_eq!((NativeId::WindowDelta.width(), f32::SIZE), (4, 4));
    assert_eq!((NativeId::WindowTicksUsec.width(), i64::SIZE), (8, 8));
    assert_eq!((NativeId::WindowFps.width(), i32::SIZE), (4, 4));
    assert_eq!((NativeId::DrawLoadTexture.width(), <[i32; 2]>::SIZE), (8, 8));
    assert_eq!((NativeId::InputMouse.width(), <[f32; 2]>::SIZE), (8, 8));
    assert_eq!((NativeId::TimeNow.width(), f64::SIZE), (8, 8));
}

#[test]
fn values_keep_their_bits() {
    let mut out = Vec::new();
    f32::from_bits(0x7FC0_1234).put(&mut out);
    (-0.0f64).put(&mut out);
    [i32::MIN, 7].put(&mut out);
    assert_eq!(out.len(), 4 + 8 + 8);
    assert_eq!(f32::get(&out).to_bits(), 0x7FC0_1234, "a NaN's payload is kept");
    assert_eq!(f64::get(&out[4..]).to_bits(), (-0.0f64).to_bits());
    assert_eq!(<[i32; 2]>::get(&out[12..]), [i32::MIN, 7]);
}

#[test]
fn the_cursor_checks_the_call_order() {
    let log = sample().natives;
    let mut c = Cursor::default();
    assert_eq!(c.next::<bool>(&log, NativeId::WindowFrame, 0), Ok(true));
    assert_eq!(
        c.next::<[f32; 2]>(&log, NativeId::InputMouse, 1),
        Err("replay differs at frame 1: the program called input.mouse where the recording has window.delta".to_string())
    );
    assert_eq!(c.left_over(&log, 1).as_deref(), Some("replay differs at frame 1: the program ended where the recording has window.delta"));
    assert_eq!(c.next::<f32>(&log, NativeId::WindowDelta, 1), Ok(1.0 / 60.0));
    assert_eq!(c.next::<[f32; 2]>(&log, NativeId::InputMouse, 1), Ok([3.5, -2.0]));
    assert_eq!(c.left_over(&log, 1), None);
    assert_eq!(
        c.next::<bool>(&log, NativeId::WindowFrame, 1),
        Err("replay differs at frame 1: the program called window.frame where the recording has ended".to_string())
    );
    // A log cut inside an entry has ended too.
    let mut c = Cursor::default();
    assert!(c.next::<f32>(&[NativeId::WindowDelta as u8, 0, 0], NativeId::WindowDelta, 4).unwrap_err().ends_with("has ended"));
    assert!(Cursor::default().next::<bool>(&[200, 0], NativeId::InputHeld, 0).unwrap_err().ends_with("has a native this Kestrel doesn't log"));
}

#[test]
fn lines_are_split_however_they_are_printed() {
    let mut keep = Keep::default();
    for chunk in [&b"fra"[..], b"mes 600\n\nlast", b" one"] {
        keep.print(chunk);
    }
    keep.end(&Ending::Returned);
    assert_eq!(keep.entries, [Entry::Line(b"frames 600".to_vec()), Entry::Line(Vec::new()), Entry::Partial(b"last one".to_vec())]);

    let mut check = Check::default();
    for chunk in [&b"frames 6"[..], b"00\n", b"\nlast one"] {
        check.print(&keep.entries, chunk, 600).unwrap();
    }
    assert_eq!(check.end(&keep.entries, &Ending::Returned, 600), Ok(3));
}

#[test]
fn a_different_line_shows_both() {
    let output = [Entry::Line(b"a".to_vec()), Entry::Line(b"checksum 5".to_vec())];
    let mut check = Check::default();
    check.print(&output, b"a\n", 3).unwrap();
    assert_eq!(
        check.print(&output, b"checksum 6\n", 9),
        Err("replay differs at frame 9, line 2:\n  recorded: checksum 5\n  this run: checksum 6".to_string())
    );

    // More output than was recorded, and less.
    let mut check = Check::default();
    check.print(&output, b"a\nchecksum 5\nmore", 9).unwrap();
    assert_eq!(
        check.end(&output, &Ending::Returned, 9),
        Err("replay differs at frame 9, line 3:\n  recorded: <end of output>\n  this run: more <no line break>".to_string())
    );
    let mut check = Check::default();
    check.print(&output, b"a\n", 9).unwrap();
    assert_eq!(
        check.end(&output, &Ending::Returned, 9),
        Err("replay differs at frame 9, line 2:\n  recorded: checksum 5\n  this run: <end of output>".to_string())
    );
}

#[test]
fn a_failure_after_an_unfinished_line_reads_back() {
    // What a run that prints "partial" with no line break and then fails is recorded as.
    let mut keep = Keep::default();
    keep.print(b"frame 2
partial");
    keep.end(&Ending::Failure("boom on line 3".to_string()));
    let mut file = sample();
    file.output = keep.entries;
    assert!(matches!(file.output[..], [Entry::Line(_), Entry::Partial(_), Entry::Failure(_)]), "{:?}", file.output);
    assert_eq!(ReplayFile::from_bytes(&file.to_bytes().unwrap()), Ok(file.clone()));

    // An unfinished line is last, or just before the failure; nowhere else.
    for output in [
        vec![Entry::Partial(b"a".to_vec()), Entry::Line(b"b".to_vec())],
        vec![Entry::Partial(b"a".to_vec()), Entry::Partial(b"b".to_vec())],
        vec![Entry::Partial(b"a".to_vec()), Entry::Line(b"b".to_vec()), Entry::Failure("boom".to_string())],
        vec![Entry::Failure("boom".to_string()), Entry::Partial(b"a".to_vec())],
    ] {
        file.output = output;
        assert_eq!(ReplayFile::from_bytes(&file.to_bytes().unwrap()), Err(Unreadable::NotReplay), "{:?}", file.output);
    }
}

#[test]
fn a_recorded_failure_must_recur() {
    let mut keep = Keep::default();
    keep.print(b"frame 2\npartial");
    keep.end(&Ending::Failure("boom on line 3".to_string()));
    let output = keep.entries;
    assert_eq!(output.len(), 3);

    let mut check = Check::default();
    check.print(&output, b"frame 2\npartial", 3).unwrap();
    assert_eq!(check.end(&output, &Ending::Failure("boom on line 3".to_string()), 3), Ok(2));

    let mut check = Check::default();
    check.print(&output, b"frame 2\npartial", 3).unwrap();
    assert_eq!(
        check.end(&output, &Ending::Returned, 3),
        Err("replay differs at the end: the recording failed, this run exited with code 0".to_string())
    );
    let mut check = Check::default();
    check.print(&output, b"frame 2\npartial", 3).unwrap();
    assert!(check.end(&output, &Ending::Failure("bang".to_string()), 3).unwrap_err().ends_with("this run: <failure: bang>"));
}

#[test]
fn reports_say_what_happened() {
    assert_eq!(Report::Matches { frames: 600, lines: 4 }.text().as_deref(), Some("replay matches (600 frames, 4 lines)"));
    assert_eq!(Report::Matches { frames: 600, lines: 4 }.exit_code(), 0);
    assert_eq!(Report::Differs("x".to_string()).exit_code(), 1);
    assert_eq!(Report::Recorded.text(), None);
    assert_eq!(WindowMode::parse("headless-quiet"), Some(WindowMode::HeadlessQuiet));
    for mode in [WindowMode::Real, WindowMode::Hidden, WindowMode::Headless, WindowMode::HeadlessQuiet] {
        assert_eq!(WindowMode::parse(mode.as_str()), Some(mode));
    }
}

#[test]
fn an_exit_entry_reads_back_and_version_1_files_still_read() {
    let mut file = sample();
    file.output.push(Entry::Exit(3));
    let bytes = file.to_bytes().unwrap();
    // Kind 3, then the code as an `i32`.
    assert_eq!(&bytes[bytes.len() - 5..], [3, 3, 0, 0, 0]);
    assert_eq!(ReplayFile::from_bytes(&bytes), Ok(file.clone()));
    // Only last, after an unfinished line at most.
    file.output.insert(0, Entry::Exit(1));
    assert_eq!(ReplayFile::from_bytes(&file.to_bytes().unwrap()), Err(Unreadable::NotReplay));
    file.output = vec![Entry::Exit(0), Entry::Failure("boom".to_string())];
    assert_eq!(ReplayFile::from_bytes(&file.to_bytes().unwrap()), Err(Unreadable::NotReplay));

    // A version 1 file is the same bytes with a 1, and has no exit entries.
    let old = sample();
    let mut bytes = old.to_bytes().unwrap();
    bytes[4] = 1;
    assert_eq!(ReplayFile::from_bytes(&bytes), Ok(old));
    let mut exited = sample();
    exited.output = vec![Entry::Exit(3)];
    let mut bytes = exited.to_bytes().unwrap();
    bytes[4] = 1;
    assert_eq!(ReplayFile::from_bytes(&bytes), Err(Unreadable::NotReplay));
}

#[test]
fn a_recording_keeps_an_exit_code_and_a_replay_compares_it() {
    let mut keep = Keep::default();
    keep.print(b"bye\nlast");
    keep.end(&Ending::Exit(3));
    assert_eq!(keep.entries, [Entry::Line(b"bye".to_vec()), Entry::Partial(b"last".to_vec()), Entry::Exit(3)]);
    let output = keep.entries;

    let ended = |ending: Ending| {
        let mut check = Check::default();
        check.print(&output, b"bye\nlast", 4).unwrap();
        check.end(&output, &ending, 4)
    };
    assert_eq!(ended(Ending::Exit(3)), Ok(2));
    assert_eq!(ended(Ending::Exit(1)), Err("replay differs at the end: the recording exited with code 3, this run with 1".to_string()));
    assert_eq!(ended(Ending::Returned), Err("replay differs at the end: the recording exited with code 3, this run with 0".to_string()));
    assert_eq!(
        ended(Ending::Failure("boom".to_string())),
        Err("replay differs at the end: the recording exited with code 3, this run failed".to_string())
    );

    // A normal end is code 0, the same as `exit(0)`.
    let output = [Entry::Line(b"bye".to_vec())];
    let ended = |ending: Ending| {
        let mut check = Check::default();
        check.print(&output, b"bye\n", 4).unwrap();
        check.end(&output, &ending, 4)
    };
    assert_eq!(ended(Ending::Exit(0)), Ok(1));
    assert_eq!(ended(Ending::Exit(1)), Err("replay differs at the end: the recording exited with code 0, this run with 1".to_string()));
    let output = [Entry::Failure("boom".to_string())];
    let mut check = Check::default();
    assert_eq!(
        check.end(&output, &Ending::Exit(2), 0),
        Err("replay differs at the end: the recording failed, this run exited with code 2".to_string())
    );

    // More recorded output than this run printed before it exited is a difference in the lines.
    let output = [Entry::Line(b"bye".to_vec()), Entry::Exit(3)];
    let mut check = Check::default();
    assert_eq!(
        check.end(&output, &Ending::Exit(3), 0),
        Err("replay differs at frame 0, line 1:\n  recorded: bye\n  this run: <exit with code 3>".to_string())
    );
}
