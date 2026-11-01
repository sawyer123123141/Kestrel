use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Builds kestrel-runtime and kestrel-window as staticlibs (which `cargo test` doesn't do
/// for workspace members), exports their paths as KESTREL_RUNTIME_LIB and
/// KESTREL_WINDOW_LIB, and copies them next to the `kestrel` binary where `kestrel build`
/// looks first. Programs that use `window`, `draw` or `input` link the window library
/// (which carries the runtime as well); every other program links the runtime library.
/// Only Windows builds have the window, so elsewhere only the runtime library is built.
fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let workspace = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let target_dir = out.join("runtime-target");
    let release = env::var("PROFILE").unwrap() == "release";
    let windows = env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows";

    let libs: &[(&str, &str)] = if windows {
        &[("KESTREL_RUNTIME_LIB", "kestrel_runtime"), ("KESTREL_WINDOW_LIB", "kestrel_window")]
    } else {
        &[("KESTREL_RUNTIME_LIB", "kestrel_runtime")]
    };

    let mut cmd = Command::new(env::var("CARGO").unwrap());
    cmd.current_dir(&workspace).arg("build");
    for (_, krate) in libs {
        cmd.args(["-p", &krate.replace('_', "-")]);
    }
    cmd.args(["--lib", "--target-dir"]).arg(&target_dir);
    if release {
        cmd.arg("--release");
    }
    // Keep the outer build's settings (profile, features, target dir) out of the nested one.
    for (key, _) in env::vars() {
        if key.starts_with("CARGO_") && key != "CARGO_HOME" {
            cmd.env_remove(&key);
        }
    }
    cmd.env_remove("RUSTFLAGS");
    let status = cmd.status().expect("start cargo for the Kestrel libraries");
    assert!(status.success(), "building the Kestrel libraries failed");

    let profile_dir = target_dir.join(if release { "release" } else { "debug" });
    // Best effort: `kestrel build` falls back to the paths below if installing is skipped or fails.
    let beside = binary_dir(&out);
    for &(var, krate) in libs {
        let file = if windows { format!("{krate}.lib") } else { format!("lib{krate}.a") };
        let lib = profile_dir.join(&file);
        assert!(lib.exists(), "{krate} staticlib missing at {}", lib.display());
        println!("cargo:rustc-env={var}={}", lib.display());
        if let Some(dir) = &beside {
            if let Err(e) = install_beside(&lib, &dir.join(&file)) {
                println!("cargo:warning=couldn't copy {file} next to the kestrel binary: {e}");
            }
        }
    }
    for path in [
        "../runtime/src",
        "../runtime/Cargo.toml",
        "../window/src",
        "../window/raylib",
        "../window/build.rs",
        "../window/Cargo.toml",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
}

/// The directory the `kestrel` binary lands in, for an OUT_DIR shaped
/// `<dir>/build/<package>-<hash>/out`. Any other layout gives `None`, so we never guess.
fn binary_dir(out: &Path) -> Option<PathBuf> {
    let build = out.ancestors().nth(2)?;
    if build.file_name()? != "build" {
        return None;
    }
    build.parent().map(Path::to_path_buf)
}

/// Replaces `dest` with a copy of `lib`. Going through a temporary file keeps
/// cargo's own hard-linked output of the same name (and its `deps/` original) untouched.
fn install_beside(lib: &Path, dest: &Path) -> std::io::Result<()> {
    let tmp = dest.with_extension("tmp");
    fs::copy(lib, &tmp)?;
    fs::rename(&tmp, dest).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}
