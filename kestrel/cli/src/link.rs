use std::path::{Path, PathBuf};
use std::process::Command;

/// Which static library a program is linked against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lib {
    /// The runtime alone: every program without `window`, `draw` or `input`.
    Runtime,
    /// The window natives, raylib and, inside them, the whole runtime.
    Window,
}

impl Lib {
    fn file(self) -> &'static str {
        match (self, cfg!(windows)) {
            (Lib::Runtime, true) => "kestrel_runtime.lib",
            (Lib::Runtime, false) => "libkestrel_runtime.a",
            (Lib::Window, true) => "kestrel_window.lib",
            (Lib::Window, false) => "libkestrel_window.a",
        }
    }

    /// Where the library was left when this compiler was built. Only Windows builds have the
    /// window library; elsewhere a window program is refused before it gets here.
    fn built(self) -> &'static str {
        match self {
            Lib::Runtime => env!("KESTREL_RUNTIME_LIB"),
            Lib::Window => option_env!("KESTREL_WINDOW_LIB").unwrap_or(""),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Lib::Runtime => "runtime",
            Lib::Window => "window",
        }
    }
}

/// Links an object file with the library `lib` into an executable.
pub fn link(obj: &Path, exe: &Path, lib: Lib) -> Result<(), String> {
    let library = find_in_place(lib)?;
    let mut cmd = linker()?;
    if cfg!(windows) {
        cmd.arg("/NOLOGO")
            .arg(format!("/OUT:{}", exe.display()))
            .arg(obj)
            .arg(&library)
            // What Rust's std needs (`rustc --print native-static-libs`).
            .args(["kernel32.lib", "ntdll.lib", "userenv.lib", "ws2_32.lib", "dbghelp.lib", "msvcrt.lib"]);
        if lib == Lib::Window {
            // What raylib needs.
            cmd.args(["opengl32.lib", "gdi32.lib", "winmm.lib", "user32.lib", "shell32.lib"]);
        }
        cmd.args(["/NODEFAULTLIB:libcmt", "/SUBSYSTEM:CONSOLE"]);
    } else {
        // What Rust's std needs (`rustc --print native-static-libs` on Linux).
        cmd.arg(obj).arg(&library).arg("-o").arg(exe).args(["-lgcc_s", "-lutil", "-lrt", "-lpthread", "-lm", "-ldl", "-lc"]);
    }
    let out = cmd.output().map_err(|e| format!("couldn't start the linker: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "linking failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// The library: next to this executable, else where the build left it.
fn find_in_place(lib: Lib) -> Result<PathBuf, String> {
    let exe_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    find_lib(lib, exe_dir.as_deref(), Path::new(lib.built()))
}

fn find_lib(lib: Lib, exe_dir: Option<&Path>, built: &Path) -> Result<PathBuf, String> {
    exe_dir
        .map(|dir| dir.join(lib.file()))
        .filter(|path| path.is_file())
        .or_else(|| built.is_file().then(|| built.to_path_buf()))
        .ok_or_else(|| format!("can't find the Kestrel {} library ({}); rebuild kestrel", lib.name(), lib.file()))
}

#[cfg(windows)]
fn linker() -> Result<Command, String> {
    // Never a bare `link.exe` from PATH: Git for Windows ships an unrelated `link`.
    find_msvc_tools::find("x86_64-pc-windows-msvc", "link.exe")
        .ok_or_else(|| "couldn't find the Microsoft linker; install Visual Studio Build Tools (C++)".to_string())
}

#[cfg(not(windows))]
fn linker() -> Result<Command, String> {
    Ok(Command::new("cc"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{find_lib, Lib};

    const LIBS: [Lib; 2] = [Lib::Runtime, Lib::Window];

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kestrel-link-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn library_next_to_the_executable_wins() {
        for lib in LIBS {
            let (exe_dir, other) = (scratch(&format!("exe-{lib:?}")), scratch(&format!("built-{lib:?}")));
            fs::write(exe_dir.join(lib.file()), b"a").unwrap();
            fs::write(other.join(lib.file()), b"b").unwrap();
            let found = find_lib(lib, Some(&exe_dir), &other.join(lib.file())).unwrap();
            assert_eq!(found, exe_dir.join(lib.file()));
        }
    }

    #[test]
    fn falls_back_to_the_built_path() {
        for lib in LIBS {
            let (exe_dir, other) = (scratch(&format!("exe_empty-{lib:?}")), scratch(&format!("built_only-{lib:?}")));
            fs::write(other.join(lib.file()), b"b").unwrap();
            let built = other.join(lib.file());
            assert_eq!(find_lib(lib, Some(&exe_dir), &built).unwrap(), built);
            assert_eq!(find_lib(lib, None, &built).unwrap(), built);
        }
    }

    #[test]
    fn a_library_is_only_looked_for_under_its_own_name() {
        // The runtime library next to the executable doesn't stand in for the window one.
        let exe_dir = scratch("exe_other_lib");
        fs::write(exe_dir.join(Lib::Runtime.file()), b"a").unwrap();
        assert!(find_lib(Lib::Window, Some(&exe_dir), Path::new("no/such/lib")).is_err());
    }

    #[test]
    fn missing_library_is_a_clear_error() {
        let exe_dir = scratch("exe_none");
        let err = find_lib(Lib::Runtime, Some(&exe_dir), Path::new("no/such/lib")).unwrap_err();
        assert_eq!(err, format!("can't find the Kestrel runtime library ({}); rebuild kestrel", Lib::Runtime.file()));
        let err = find_lib(Lib::Window, Some(&exe_dir), Path::new("no/such/lib")).unwrap_err();
        assert_eq!(err, format!("can't find the Kestrel window library ({}); rebuild kestrel", Lib::Window.file()));
    }
}
