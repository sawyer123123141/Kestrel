//! Watching the program's files for a save (hot reload design, decision 6): the game thread asks
//! between frames, and the files' times are read at most four times a second.

use std::time::{Duration, Instant, SystemTime};

use kestrel_driver::SourceFile;

/// How often the files are looked at, at most.
const INTERVAL: Duration = Duration::from_millis(250);

/// The program's files with the times they had when the code now running (or being compiled)
/// read them.
pub struct Watcher {
    files: Vec<SourceFile>,
    last_poll: Instant,
}

impl Watcher {
    pub fn new(files: Vec<SourceFile>) -> Watcher {
        Watcher { files, last_poll: Instant::now() }
    }

    /// Whether a file changed since its time was taken, looked at no more than four times a
    /// second (false in between).
    pub fn changed(&mut self) -> bool {
        if self.last_poll.elapsed() < INTERVAL {
            return false;
        }
        self.changed_now()
    }

    /// Whether a file changed since its time was taken, looked at now.
    pub fn changed_now(&mut self) -> bool {
        self.last_poll = Instant::now();
        self.files.iter().any(SourceFile::changed)
    }

    /// The files with their times as of now, which a compile that starts next has not missed.
    pub fn files_before_compile(&self) -> Vec<SourceFile> {
        self.files.iter().map(|f| SourceFile { path: f.path.clone(), modified: f.current() }).collect()
    }

    /// Takes the files' times as of now. The host does this as a compile starts, so a save that
    /// lands while it runs is seen afterwards, and a failed compile is not tried again (and its
    /// error not printed again) until the next save.
    pub fn refresh(&mut self) {
        self.files = self.files_before_compile();
    }

    /// The program's own files (not the built-in modules), for a compile to wait on.
    pub fn paths(&self) -> Vec<std::path::PathBuf> {
        self.files.iter().filter(|f| !f.is_builtin()).map(|f| f.path.clone()).collect()
    }

    /// When the latest save was, as of the times taken.
    pub fn latest_save(&self) -> Option<SystemTime> {
        self.files.iter().filter_map(|f| f.modified).max()
    }

    /// Watches the files of the code that now runs, with the times its compile read them at.
    pub fn adopt(&mut self, files: Vec<SourceFile>) {
        self.files = files;
    }

    /// After a failed compile: watches `files`, the ones it tried to read, with the times it read
    /// them at (a file it didn't find is watched for appearing), as well as those watched already.
    /// A module only the broken edit uses is then seen when it is fixed.
    pub fn merge(&mut self, files: Vec<SourceFile>) {
        for file in files {
            match self.files.iter_mut().find(|f| f.path == file.path) {
                Some(known) => known.modified = file.modified,
                None => self.files.push(file),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use kestrel_driver::SourceFile;

    use super::Watcher;

    /// A file in a fresh folder of its own, with modification time `at` seconds past a fixed time.
    fn file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kestrel-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prog.kes");
        fs::write(&path, "x").unwrap();
        stamp(&path, 0);
        path
    }

    fn stamp(path: &PathBuf, at: u64) {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + at);
        fs::File::options().write(true).open(path).unwrap().set_modified(time).unwrap();
    }

    fn watching(path: &PathBuf) -> Watcher {
        let modified = fs::metadata(path).unwrap().modified().ok();
        Watcher::new(vec![SourceFile { path: path.clone(), modified }])
    }

    #[test]
    fn a_save_is_seen_once_the_interval_has_passed() {
        let path = file("interval");
        let mut w = watching(&path);
        assert!(!w.changed_now());
        stamp(&path, 1);
        // Looked at just now: not again for a quarter of a second.
        assert!(!w.changed());
        std::thread::sleep(Duration::from_millis(260));
        assert!(w.changed());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_save_during_a_compile_is_seen_after_it() {
        let path = file("during");
        let mut w = watching(&path);
        stamp(&path, 1);
        assert!(w.changed_now());
        // The compile starts: the times are taken first.
        w.refresh();
        assert!(!w.changed_now(), "the save the compile reads is not seen twice");
        // A save while it compiles, which then fails: the watcher keeps the times from its start.
        stamp(&path, 2);
        assert!(w.changed_now(), "the save during the compile is seen");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_files_a_compile_read_are_adopted() {
        let path = file("adopt");
        let mut w = watching(&path);
        stamp(&path, 1);
        let read = w.files_before_compile();
        assert!(!read.iter().any(SourceFile::changed));
        w.adopt(read);
        assert!(!w.changed_now());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_failed_compile_adds_the_files_it_tried() {
        let path = file("merge");
        let mut w = watching(&path);
        let helper = path.with_file_name("helper.kes");
        // The failed compile read the program after a save, and found no `helper.kes`.
        stamp(&path, 1);
        let read = fs::metadata(&path).unwrap().modified().ok();
        w.merge(vec![SourceFile { path: path.clone(), modified: read }, SourceFile { path: helper.clone(), modified: None }]);
        assert!(!w.changed_now(), "the save the compile read is not seen again");
        fs::write(&helper, "pub k() -> int { 3 }\n").unwrap();
        assert!(w.changed_now(), "the module appearing is seen");
        assert!(w.paths().contains(&helper));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
