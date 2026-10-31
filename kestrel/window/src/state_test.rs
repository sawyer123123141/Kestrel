//! Tests of the window's state (`state.rs`).

use super::*;

/// Runs `body` on a fresh thread, so it starts with no window.
fn on_a_thread<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(body).join().unwrap()
}

fn headless(frames: u64) -> Settings {
    Settings { headless: Some(frames), hidden: false, quiet: false }
}

#[test]
fn settings_come_from_the_environment_values() {
    assert_eq!(Settings::parse(None, None, None), Ok(Settings { headless: None, hidden: false, quiet: false }));
    assert_eq!(Settings::parse(Some("3"), None, None).unwrap().headless, Some(3));
    assert_eq!(Settings::parse(Some(" 100000 "), None, None).unwrap().headless, Some(100_000));
    assert_eq!(Settings::parse(Some("0"), None, None).unwrap().headless, Some(0));
    assert_eq!(Settings::parse(Some(""), None, None).unwrap().headless, None, "empty is unset");
    assert!(Settings::parse(None, Some("1"), None).unwrap().hidden);
    assert!(!Settings::parse(None, Some("0"), None).unwrap().hidden);
    assert!(!Settings::parse(None, Some("yes"), None).unwrap().hidden);
    assert!(Settings::parse(None, None, Some("1")).unwrap().quiet);
    assert!(!Settings::parse(None, None, Some("0")).unwrap().quiet);
    assert!(!Settings::parse(None, None, Some("true")).unwrap().quiet);
    let err = Settings::parse(Some("lots"), None, None).unwrap_err();
    assert!(err.starts_with("KESTREL_HEADLESS must be a whole number of frames"), "{err}");
    assert!(Settings::parse(Some("-1"), None, None).is_err());
}

#[test]
fn a_recording_names_the_window_the_settings_ask_for() {
    let mode = |headless, hidden, quiet| Settings::parse(headless, hidden, quiet).unwrap().window_mode();
    assert_eq!(mode(None, None, None), WindowMode::Real);
    assert_eq!(mode(None, Some("1"), None), WindowMode::Hidden);
    assert_eq!(mode(Some("5"), None, None), WindowMode::Headless);
    assert_eq!(mode(Some("5"), Some("1"), Some("1")), WindowMode::HeadlessQuiet);
    assert_eq!(mode(None, None, Some("1")), WindowMode::Real, "quiet means nothing without headless");
}

#[test]
fn calls_before_open_name_the_function() {
    on_a_thread(|| {
        assert_eq!(check_drawing("draw.circle"), Err("draw.circle was called before window.open".to_string()));
        assert_eq!(check_view("input.held"), Err("input.held was called before window.open".to_string()));
        assert_eq!(frame(), Err("window.frame was called before window.open".to_string()));
        assert_eq!(close(), Err("window.close was called before window.open".to_string()));
    });
}

#[test]
fn a_second_open_applies_the_new_size() {
    on_a_thread(|| {
        assert_eq!(open(&headless(2), 320, 200, b"a"), Ok(Mode::Headless));
        let size = |view: Result<View, String>| match view {
            Ok(View::Fixed(fixed)) => (fixed.size.x, fixed.size.y),
            other => panic!("a headless window gives fixed values, not {other:?}"),
        };
        assert_eq!(size(check_view("window.size")), (320.0, 200.0));
        assert_eq!(frame(), Ok(true));
        // Open again: the size changes and the window keeps its frames and its place in the run.
        assert_eq!(open(&headless(2), 640, 480, b"b"), Ok(Mode::Headless));
        assert_eq!(size(check_view("window.size")), (640.0, 480.0));
        assert_eq!(frame(), Ok(true));
        assert_eq!(frame(), Ok(false), "the frame count did not restart");
        assert_eq!(size(check_view("window.size")), (640.0, 480.0), "and a closed window keeps the new size");
    });
}

#[test]
fn a_second_open_still_needs_a_size() {
    on_a_thread(|| {
        open(&headless(1), 320, 200, b"a").unwrap();
        let err = open(&headless(1), 0, 480, b"b").unwrap_err();
        assert_eq!(err, "window.open needs a width and height above 0, not 0 by 480");
        assert!(matches!(check_view("window.size"), Ok(View::Fixed(f)) if f.size == V2 { x: 320.0, y: 200.0 }));
    });
}

#[test]
fn a_window_needs_a_size() {
    on_a_thread(|| {
        let err = open(&headless(1), 0, 10, b"a").unwrap_err();
        assert_eq!(err, "window.open needs a width and height above 0, not 0 by 10");
        assert!(open(&headless(1), 10, -5, b"a").is_err());
        // Refusing a size left the window unopened.
        assert_eq!(open(&headless(1), 10, 10, b"a"), Ok(Mode::Headless));
    });
}

#[test]
fn a_headless_window_runs_its_frames_then_closes() {
    on_a_thread(|| {
        open(&headless(2), 320, 200, b"hi").unwrap();
        assert_eq!(check_drawing("draw.clear"), Ok(Mode::Headless));
        let at = |frames: i64| {
            let (size, time, ticks) = (V2 { x: 320.0, y: 200.0 }, frames as f32 / 60.0, frames * 1_000_000 / 60);
            View::Fixed(Fixed { size, time, ticks, delta: 1.0 / 60.0, fps: 60 })
        };
        assert_eq!(check_view("window.time"), Ok(at(0)));
        assert_eq!(frame(), Ok(true));
        assert_eq!(check_view("window.time"), Ok(at(1)));
        assert_eq!(frame(), Ok(true));
        assert_eq!(check_view("window.time"), Ok(at(2)));
        assert_eq!(frame(), Ok(false));
        assert_eq!(frame(), Ok(false), "and it stays closed");
        // Closed: drawing is an error; the queries keep their last values.
        assert_eq!(check_drawing("draw.circle"), Err("draw.circle was called after the window closed".to_string()));
        assert_eq!(check_view("window.size"), Ok(at(2)), "and the clock stops at 33333 microseconds");
        assert_eq!(close(), Ok(None), "closing a closed window does nothing");
        assert_eq!(open(&headless(1), 10, 10, b"a"), Err("window.open was called after the window closed".to_string()));
    });
}

#[test]
fn zero_frames_never_start() {
    on_a_thread(|| {
        open(&headless(0), 10, 10, b"a").unwrap();
        assert_eq!(frame(), Ok(false));
    });
}

#[test]
fn close_takes_effect_at_the_next_frame() {
    on_a_thread(|| {
        open(&headless(100), 10, 10, b"a").unwrap();
        assert_eq!(frame(), Ok(true));
        assert_eq!(close(), Ok(Some(Mode::Headless)));
        assert_eq!(check_drawing("draw.circle"), Ok(Mode::Headless), "still open until the frame ends");
        assert_eq!(frame(), Ok(false));
        assert_eq!(frame(), Ok(false));
    });
}

#[test]
fn closing_before_the_first_frame_runs_no_frame() {
    on_a_thread(|| {
        open(&headless(100), 10, 10, b"a").unwrap();
        close().unwrap();
        assert_eq!(frame(), Ok(false));
    });
}
