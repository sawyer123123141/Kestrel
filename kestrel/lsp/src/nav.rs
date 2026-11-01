//! Hover and go to definition, in LSP terms. While the document doesn't parse they answer from
//! the last analysis that did, with offsets moved past the edit since (see `Shift`).

use std::path::{Path, PathBuf};

use kestrel_driver::Analysis;
use lsp_types::{Hover, HoverContents, Location, MarkupContent, MarkupKind, Position, Url};

use crate::docs::{url_of, Doc, Docs};
use crate::pos::LineMap;
use crate::resolve::{resolve, Symbol};

/// How offsets moved between an old text and a new one, from what they share at the start and
/// at the end: the edit is the part in between.
struct Shift {
    prefix: usize,
    /// How much longer the new text is (negative when shorter).
    delta: isize,
    old_len: usize,
    suffix: usize,
}

impl Shift {
    fn new(old: &str, new: &str) -> Self {
        let (a, b) = (old.as_bytes(), new.as_bytes());
        let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
        let max = a.len().min(b.len()) - prefix;
        let suffix = a.iter().rev().zip(b.iter().rev()).take(max).take_while(|(x, y)| x == y).count();
        Shift { prefix, delta: b.len() as isize - a.len() as isize, old_len: a.len(), suffix }
    }

    /// The old offset of new offset `n`, unless it is inside the edit.
    fn to_old(&self, n: usize) -> Option<usize> {
        let new_suffix_start = (self.old_len - self.suffix) as isize + self.delta;
        if n <= self.prefix {
            Some(n)
        } else if n as isize >= new_suffix_start {
            Some((n as isize - self.delta) as usize)
        } else {
            None
        }
    }

    /// The new offset of old offset `o`, unless it is inside the edit.
    fn to_new(&self, o: usize) -> Option<usize> {
        if o <= self.prefix {
            Some(o)
        } else if o >= self.old_len - self.suffix {
            Some((o as isize + self.delta) as usize)
        } else {
            None
        }
    }
}

/// The analysis to answer from, the entry file offset to look up in it, and how its entry
/// file's offsets map back to the document's (when it is an older one).
fn answer_from(doc: &Doc, p: Position) -> Option<(&Analysis, u32, Option<Shift>)> {
    let offset = LineMap::new(&doc.text).offset(p);
    match &doc.analysis {
        Some(a) if a.tables.is_some() => Some((a, offset as u32, None)),
        _ => {
            let (old, a) = doc.good.as_ref()?;
            let shift = Shift::new(old, &doc.text);
            Some((a, shift.to_old(offset)? as u32, Some(shift)))
        }
    }
}

pub fn hover(doc: &Doc, p: Position, extra: Option<String>) -> Option<Hover> {
    let found = answer_from(doc, p).and_then(|(a, offset, _)| resolve(a, offset));
    let mut text = match found {
        Some(Symbol { code, .. }) => format!("```kestrel\n{code}\n```"),
        None => String::new(),
    };
    if let Some(extra) = extra {
        if !text.is_empty() {
            text.push_str("\n\n---\n\n");
        }
        text.push_str(&extra);
    }
    if text.is_empty() {
        return None;
    }
    Some(Hover { contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: text }), range: None })
}

pub fn definition(docs: &Docs, doc: &Doc, p: Position) -> Option<Location> {
    let (a, offset, shift) = answer_from(doc, p)?;
    let def = resolve(a, offset)?.def?;
    let i = a.source_at(def.start);
    let s = &a.sources[i];
    let (start, end) = ((def.start - s.base) as usize, (def.end - s.base) as usize);
    if i == 0 {
        // The entry file: the document itself, whose text may be newer than the analysis.
        let (start, end) = match &shift {
            Some(sh) => (sh.to_new(start)?, sh.to_new(end)?),
            None => (start, end),
        };
        return Some(Location::new(doc.uri.clone(), LineMap::new(&doc.text).range(start, end)));
    }
    let uri = if s.builtin { builtin_file(&s.path, &s.src)? } else { url_of(docs, Path::new(&s.path))? };
    // The analysed text: a module open in the editor was read from its buffer.
    Some(Location::new(uri, LineMap::new(&s.src).range(start, end)))
}

/// Where built-in modules are written for the editor to open: they live inside the compiler.
pub fn builtin_dir() -> PathBuf {
    std::env::temp_dir().join("kestrel-lsp").join("builtin")
}

/// A built-in module's text written to a file (once), so go to definition can open it. A part in
/// a subfolder (`<builtin>/rollback/session.kes`) is written in that subfolder.
fn builtin_file(path: &str, src: &str) -> Option<Url> {
    let inside = Path::new(path).strip_prefix("<builtin>").ok()?;
    let file = builtin_dir().join(inside);
    if std::fs::read_to_string(&file).ok().as_deref() != Some(src) {
        std::fs::create_dir_all(file.parent()?).ok()?;
        std::fs::write(&file, src).ok()?;
    }
    Url::from_file_path(file).ok()
}

/// Whether `path` is one of the built-in modules written by `builtin_file` (in the folder or below
/// it): the server doesn't check those (their `native` functions are only allowed inside the
/// compiler).
pub fn is_builtin_copy(path: &Path) -> bool {
    let dir = crate::docs::key(&builtin_dir());
    path.ancestors().skip(1).any(|d| crate::docs::key(d) == dir)
}

#[cfg(test)]
mod tests {
    use super::{builtin_dir, builtin_file, is_builtin_copy, Shift};

    #[test]
    fn a_built_in_part_is_written_in_its_subfolder() {
        let url = builtin_file("<builtin>/navtest/part.kes", "// a part\n").expect("written");
        let path = url.to_file_path().unwrap();
        assert_eq!(path, builtin_dir().join("navtest").join("part.kes"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "// a part\n");
        assert!(is_builtin_copy(&path));
        assert!(is_builtin_copy(&builtin_dir().join("draw.kes")));
        assert!(!is_builtin_copy(&std::env::temp_dir().join("kestrel-lsp").join("draw.kes")));
    }

    #[test]
    fn offsets_move_past_an_edit() {
        let old = "let a = 1\nlet b = a\n";
        let new = "let a = 1 +\nlet b = a\n";
        let s = Shift::new(old, new);
        // Before the edit, nothing moves; after it, two bytes.
        assert_eq!(s.to_old(4), Some(4));
        assert_eq!(s.to_old(new.find("b").unwrap()), Some(old.find("b").unwrap()));
        assert_eq!(s.to_new(old.find("b").unwrap()), Some(new.find("b").unwrap()));
        // Inside the edit there is no old offset.
        assert_eq!(s.to_old(new.find('+').unwrap()), None);
    }

    #[test]
    fn identical_texts_map_every_offset_to_itself() {
        let s = Shift::new("abc", "abc");
        assert_eq!((s.to_old(0), s.to_old(3), s.to_new(2)), (Some(0), Some(3), Some(2)));
    }
}
