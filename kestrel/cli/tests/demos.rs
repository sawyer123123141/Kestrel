//! The demos still compile and run. Each runs in a headless window for a short while, so no
//! window opens and no graphics are needed.

use std::path::Path;
use std::process::{Command, Output};

mod common;

/// Runs the particles demo with `--release` in a headless window of `frames` frames. Its lines
/// are printed unless `quiet`.
fn run_particles(frames: u32, quiet: bool) -> Output {
    let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../demos/particles/particles.kes");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    common::hermetic(cmd.arg("run").arg("--release").arg(&demo))
        .env("KESTREL_HEADLESS", frames.to_string())
        .env("KESTREL_HEADLESS_QUIET", if quiet { "1" } else { "0" })
        .output()
        .unwrap()
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn the_particles_demo_runs_headless_and_reports_its_times() {
    // The headless clock runs at 60 frames a second, and the demo prints a line once a second,
    // so 70 frames give exactly one line.
    let out = run_particles(70, true);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "exit {:?}\n{stderr}", out.status.code());
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "expected one report line, got:\n{stdout}");
    assert!(lines[0].starts_with("particles 50000 update "), "unexpected report line: {}", lines[0]);
    assert!(lines[0].ends_with(" fps 60"), "the headless clock is 60 fps: {}", lines[0]);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn the_particles_demo_draws_every_particle_with_one_rects_call_a_frame() {
    // Not quiet: each headless line shows what the demo drew. `draw.rects` prints its positions'
    // count, so a frame that copied too few of the 50,000 particles would show it.
    let out = run_particles(3, false);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "exit {:?}\n{stderr}", out.status.code());
    let called = |name: &str| stdout.lines().filter(|line| line.split(' ').next() == Some(name)).collect::<Vec<_>>();
    let rects = called("draw.rects");
    assert_eq!(rects.len(), 3, "one draw.rects call in each of the three frames, got:\n{rects:?}");
    for line in rects {
        assert!(line.starts_with("draw.rects 50000 vec2(x: 2, y: 2) Color("), "every particle, in one call: {line}");
    }
    assert!(called("draw.rect").is_empty(), "no draw.rect call for a particle with BULK on");
}
