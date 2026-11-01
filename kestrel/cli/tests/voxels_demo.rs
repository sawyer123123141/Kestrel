//! The voxel viewer demo (`demos/voxels`): its world, its flying camera and the viewer itself.
//! The world and camera modules are tested by small programs written next to copies of them in
//! a scratch folder, so the demo folder holds only the demo.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

/// The demo's file `file`.
fn demo(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../demos/voxels").join(file)
}

/// A fresh, empty folder for one test's files, under the target folder's scratch space like the
/// other CLI tests' (so runs don't pile folders up in the system's temp folder). Each test needs
/// its own `name`: tests run in parallel, and the folder is cleared when a test starts.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("voxels_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `main` as `main.kes` next to copies of the demo's `modules`, runs it with
/// `kestrel run` (checks on), and gives what it printed. A failed run fails the test with its stderr.
fn run_beside(name: &str, modules: &[&str], main: &str) -> String {
    let dir = scratch(name);
    for module in modules {
        fs::copy(demo(module), dir.join(module)).unwrap_or_else(|e| panic!("copy {module}: {e}"));
    }
    fs::write(dir.join("main.kes"), main).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    let out = common::hermetic(cmd.arg("run").arg(dir.join("main.kes"))).output().unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{name}: exit {:?}\n{stderr}", out.status.code());
    stdout.into_owned()
}

/// One solid block makes six faces, wound and coloured right; blocks are coloured by their depth
/// (grass, dirt, stone, snow on the peaks) with a little variation each; face corners are darkened
/// by the blocks next to them, and the quad is cut along the diagonal that keeps that smooth; and
/// the whole 8x4x8 world has the benchmark's 463,749 faces. The program is long, so it is a file
/// of its own (`voxels/world_check.kes`, beside this one), which cargo doesn't treat as a test.
const WORLD_CHECK: &str = include_str!("voxels/world_check.kes");

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn the_world_makes_the_benchmarks_faces_wound_and_coloured_right() {
    let out = run_beside("world", &["world.kes"], WORLD_CHECK);
    let mut expected = String::from("block 24 24 0 36\n");
    for f in 0..6 {
        expected += &format!("face {f} true true [0, 1, 2, 0, 2, 3]\n");
    }
    expected += "shaded true\n";
    expected += "layers true true true true true true true true\n";
    expected += "variation true true true\n";
    expected += "depth [true, true, true, true, true]\n";
    expected += "snow true true\n";
    expected += "ao true true true true [0, 1, 2, 0, 2, 3]\n";
    expected += "flip true true true true [1, 2, 3, 1, 3, 0] true\n";
    expected += "side ao true true true true [0, 1, 2, 0, 2, 3] true\n";
    expected += "side flip true true true true [1, 2, 3, 1, 3, 0] true\n";
    expected += "rows true true true\n";
    expected += "far vec3(x: 38, y: 70, z: 103) true\n";
    expected += "world 463749 1854996 1854996 0 2782494\n";
    assert_eq!(out, expected);
}

/// The flying camera's directions, moves, turns, limits and the `Camera` it gives.
const FLY_CHECK: &str = r#"use { Flyer, forward, right, view from fly }

// Rounded to three decimals, with -0 made 0, so the printed numbers are exact.
pure r(v: vec3) -> vec3 {
    vec3(round(v.x * 1000) / 1000 + 0.0, round(v.y * 1000) / 1000 + 0.0, round(v.z * 1000) / 1000 + 0.0)
}

main() {
    print(r(forward(0, 0)), r(forward(90, 0)), r(forward(0, 90)))
    print(r(right(0)), r(right(90)))
    var f = Flyer(pos: vec3(0, 0, 0), yaw: 0, pitch: 0)
    f.fly(vec3(0, 0, 1), vec2(0, 0), 0, 0.5)
    print(r(f.pos))
    f.fly(vec3(1, 1, 0), vec2(0, 0), 0, 0.5)
    print(r(f.pos))
    f.fly(vec3(0, 0, 0), vec2(-450, 0), 0, 0.1)
    print(f.yaw, f.pitch)
    f.fly(vec3(0, 0, 0), vec2(0, -10000), 0, 0.1)
    print(f.pitch)
    f.fly(vec3(0, 0, 0), vec2(0, 20000), 0, 0.1)
    print(f.pitch)
    f.fly(vec3(0, 0, 0), vec2(0, 0), 1, 0.1)
    print(f.speed)
    f.fly(vec3(0, 0, 0), vec2(0, 0), 100, 0.1)
    print(f.speed)
    f.fly(vec3(0, 0, 0), vec2(0, 0), -100, 0.1)
    print(f.speed)
    let cam = view(f)
    print(r(cam.pos), r(cam.target - cam.pos))
}
"#;

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn the_flying_camera_moves_turns_and_keeps_its_limits() {
    let out = run_beside("fly", &["fly.kes"], FLY_CHECK);
    let expected = [
        // Yaw 0 looks along +z and yaw 90 along +x; pitch 90 looks up.
        "vec3(x: 0, y: 0, z: 1) vec3(x: 1, y: 0, z: 0) vec3(x: 0, y: 1, z: 0)",
        // Right-handed with y up: looking along +z, right is -x.
        "vec3(x: -1, y: 0, z: 0) vec3(x: 0, y: 0, z: 1)",
        // W for half a second at 20 blocks a second.
        "vec3(x: 0, y: 0, z: 10)",
        // D and Space together: the move is normalised, so diagonals aren't faster.
        "vec3(x: -7.071, y: 7.071, z: 10)",
        // Dragging 450 pixels left turns 90 degrees left (0.2 degrees a pixel).
        "90 0",
        // Dragging far up or down stops at 89 degrees, so the view never lines up with up.
        "89",
        "-89",
        // One wheel notch is 1.25 times as fast; 100 notches stop at 400, and -100 at 2.
        "25",
        "400",
        "2",
        // The camera looks from the flyer's position along its view (yaw 90, pitch -89).
        "vec3(x: -7.071, y: 7.071, z: 10) vec3(x: 0.017, y: -1, z: 0)",
    ];
    assert_eq!(out, expected.map(|line| format!("{line}\n")).concat());
}

/// Runs the viewer with `--release` in a headless window of `frames` frames, with these extra
/// environment variables.
fn run_viewer(frames: u32, env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    let cmd = common::hermetic(cmd.arg("run").arg("--release").arg(demo("voxels.kes"))).env("KESTREL_HEADLESS", frames.to_string());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

/// The number after `word` in `line` (`meshes 230` gives 230 for "meshes").
fn number_after(line: &str, word: &str) -> usize {
    let mut words = line.split(' ');
    words.position(|w| w == word).unwrap_or_else(|| panic!("no {word:?} in {line:?}"));
    words.next().and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("no number after {word:?} in {line:?}"))
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn the_viewer_builds_the_world_and_reports_its_times() {
    // The headless clock is 60 frames a second and the viewer prints once a second, so 70
    // frames give the start line and one report line.
    let out = run_viewer(70, &[("KESTREL_HEADLESS_QUIET", "1")]);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "exit {:?}\n{stderr}", out.status.code());
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "expected the start line and one report line, got:\n{stdout}");
    assert!(lines[0].starts_with("voxels world 8 chunks 256 meshes "), "start line: {}", lines[0]);
    assert_eq!(number_after(lines[0], "faces"), 463_749, "start line: {}", lines[0]);
    assert!(lines[0].contains(" build ") && lines[0].contains(" upload ") && lines[0].ends_with(" ms"), "start line: {}", lines[0]);
    let meshes = number_after(lines[0], "meshes");
    assert!(meshes > 0 && meshes <= 256, "meshes: {meshes}");
    assert!(lines[1].starts_with(&format!("voxels 8 meshes {meshes} faces 463749 draw ")), "report line: {}", lines[1]);
    assert!(lines[1].ends_with(" fps 60"), "the headless clock is 60 fps: {}", lines[1]);
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn the_viewer_uploads_each_non_empty_chunk_once_and_draws_every_mesh_each_frame() {
    let out = run_viewer(2, &[]);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "exit {:?}\n{stderr}", out.status.code());
    let start = stdout.lines().find(|l| l.starts_with("voxels world ")).unwrap_or_else(|| panic!("no start line:\n{stdout}"));
    let meshes = number_after(start, "meshes");
    let called = |name: &str| stdout.lines().filter(|line| line.split(' ').next() == Some(name)).collect::<Vec<_>>();
    let made = called("draw3d.make_mesh");
    assert_eq!(made.len(), meshes, "one make_mesh per mesh");
    for line in &made {
        // `draw3d.make_mesh MeshData(vertices: N, colors: N, uvs: 0, indices: N)`
        let n = |field: &str| number_after(&line.replace([',', '(', ')'], " "), field);
        assert!(n("vertices:") > 0, "an empty chunk was uploaded: {line}");
        assert_eq!(n("colors:"), n("vertices:"), "one colour per vertex: {line}");
        assert_eq!(n("uvs:"), 0, "{line}");
        assert_eq!(n("indices:"), n("vertices:") / 4 * 6, "two triangles per four vertices: {line}");
    }
    assert_eq!(called("draw3d.camera").len(), 2, "one camera a frame");
    let drawn = called("draw3d.mesh");
    assert_eq!(drawn.len(), 2 * meshes, "every mesh in both frames");
    // In each frame the camera is set before that frame's meshes are drawn: the order of the
    // draw3d lines is camera, then `meshes` meshes, twice.
    let order: Vec<&str> = stdout
        .lines()
        .filter_map(|line| line.split(' ').next())
        .filter(|name| matches!(*name, "draw3d.camera" | "draw3d.mesh"))
        .collect();
    let frame = std::iter::once("draw3d.camera").chain(std::iter::repeat_n("draw3d.mesh", meshes));
    assert_eq!(order, frame.clone().chain(frame).collect::<Vec<_>>(), "each frame sets the camera, then draws its meshes");
    // The `Mesh` values were made inside the `arena { }` and are drawn after it: each frame draws
    // the meshes `make_mesh` made, in order (a list the arena had freed would give other counts).
    for (k, line) in drawn.iter().enumerate() {
        let made = made[k % meshes].replace([',', '(', ')'], " ");
        let line = line.replace([',', '(', ')'], " ");
        assert_eq!(number_after(&line, "vertex_count:"), number_after(&made, "vertices:"), "mesh {k}: {line}");
        assert_eq!(number_after(&line, "triangle_count:") * 3, number_after(&made, "indices:"), "mesh {k}: {line}");
    }
}

#[test]
#[cfg_attr(not(windows), ignore = "no window on this platform")]
fn the_viewer_frees_the_cpu_mesh_data_once_it_is_uploaded() {
    // The world's mesh lists live in an `arena { }` that ends after the upload, so the program
    // arena stays small (about 64 KiB measured). Without the arena it holds all of them: a
    // scratch copy with `if true {` in its place kept about 184 MiB, more than the lists' own
    // size because lists grow by doubling and leave their old buffers behind.
    let dir = scratch("arena");
    let exe = dir.join("voxels").with_extension(std::env::consts::EXE_EXTENSION);
    let mut build = Command::new(env!("CARGO_BIN_EXE_kestrel"));
    let built = common::hermetic(build.arg("build").arg("--release").arg(demo("voxels.kes")).arg("-o").arg(&exe)).output().unwrap();
    assert!(built.status.success(), "build failed:\n{}", String::from_utf8_lossy(&built.stderr));
    let mut run = Command::new(&exe);
    let out = common::hermetic(&mut run)
        .env("KESTREL_HEADLESS", "2")
        .env("KESTREL_HEADLESS_QUIET", "1")
        .env("KESTREL_ARENA_REPORT", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "exit {:?}\n{}", out.status.code(), String::from_utf8_lossy(&out.stderr));
    // Read the report the way cli/tests/text_memory.rs does (stderr, the `kestrel: program arena` line).
    let stderr = String::from_utf8_lossy(&out.stderr);
    let bytes: u64 = stderr
        .lines()
        .find_map(|l| l.strip_prefix("kestrel: program arena ")?.strip_suffix(" bytes")?.parse().ok())
        .unwrap_or_else(|| panic!("no arena report in stderr:\n{stderr}"));
    assert!(bytes < 8 << 20, "the program arena kept {bytes} bytes: the mesh data wasn't freed");
}
