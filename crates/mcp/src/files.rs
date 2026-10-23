// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `mailcraft-cli mcp --files DIR`: the one folder an agent may name files in.
//!
//! Without it, a param that names a file is refused (`mailcraft_engine::agent::file_param`). With
//! it, the server itself reads and writes inside DIR and hands the command `data`, so no command
//! ever opens a path an agent chose:
//! - a path is taken relative to DIR (an absolute one must lie in it); `..` is refused outright;
//! - what is read must, with every link followed, still lie in DIR and be a plain file;
//! - what is written goes into a folder that, with every link followed, lies in DIR, as a new
//!   file (`create_new`: an existing file, or a link of any kind in its place, is never written
//!   through or over), never hidden (a leading dot), with the extension the command produces
//!   (any but programs, scripts, web pages and macro-capable Office files, the kinds the attachment opener refuses or asks about, for an attachment).

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use mailcraft_engine::opening::is_risky;

/// Largest file read for an agent.
pub const MAX_READ: u64 = 64 << 20;

#[derive(Clone, Debug)]
pub struct FilesDir {
    root: PathBuf,
}

/// A file to be written, checked before the command runs.
#[derive(Clone, Debug)]
pub struct Target {
    pub path: PathBuf,
    /// What the agent called it.
    pub given: String,
}

fn bad_name(name: &str) -> bool {
    name.is_empty()
        || name.starts_with('.')
        || name.chars().any(|c| c.is_control() || matches!(c, ':' | '\\' | '/' | '*' | '?' | '"' | '<' | '>' | '|'))
}

fn extension(name: &str) -> Option<String> {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())
}

impl FilesDir {
    /// `dir` must be a folder that exists.
    pub fn new(dir: &Path) -> Result<Self, String> {
        let root = dir.canonicalize().map_err(|e| format!("--files {}: {e}", dir.display()))?;
        if !root.is_dir() {
            return Err(format!("--files {}: not a folder", dir.display()));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `given` under the root, refusing `..` and what names no file.
    fn join(&self, given: &str) -> Result<(PathBuf, String), String> {
        let refuse = |why: &str| Err(format!("{given}: {why} (files are only read and written inside {})", self.root.display()));
        if given.trim().is_empty() || given.contains('\0') {
            return refuse("not a file name");
        }
        let p = Path::new(given);
        if p.components().any(|c| matches!(c, Component::ParentDir)) {
            return refuse("`..` is not allowed");
        }
        let joined = if p.is_absolute() { p.to_path_buf() } else { self.root.join(p) };
        let Some(name) = joined.file_name().and_then(|n| n.to_str()).map(str::to_string) else { return refuse("not a file name") };
        Ok((joined, name))
    }

    fn inside(&self, p: &Path) -> bool {
        p.starts_with(&self.root)
    }

    /// The bytes of the file `given` names, and its name.
    pub fn read(&self, given: &str) -> Result<(String, Vec<u8>), String> {
        let (joined, name) = self.join(given)?;
        let real = joined.canonicalize().map_err(|e| format!("{given}: {e}"))?;
        if !self.inside(&real) {
            return Err(format!("{given}: leads outside {} (a link that leaves it is not followed)", self.root.display()));
        }
        let meta = std::fs::metadata(&real).map_err(|e| format!("{given}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{given}: not a file"));
        }
        if meta.len() > MAX_READ {
            return Err(format!("{given} is larger than {} MB", MAX_READ >> 20));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&real).and_then(|f| f.take(MAX_READ + 1).read_to_end(&mut bytes)).map_err(|e| format!("{given}: {e}"))?;
        if bytes.len() as u64 > MAX_READ {
            return Err(format!("{given} is larger than {} MB", MAX_READ >> 20));
        }
        Ok((name, bytes))
    }

    /// Check where `given` would be written, with the extension `ext` ("" for any but programs
    /// and scripts).
    pub fn target(&self, given: &str, ext: &str) -> Result<Target, String> {
        let (joined, name) = self.join(given)?;
        if bad_name(&name) {
            return Err(format!("{given}: not a file name that is written for an agent (no hidden files, no : \\ * ? \" < > |)"));
        }
        let e = extension(&name);
        if !ext.is_empty() && e.as_deref() != Some(ext) {
            return Err(format!("{given}: this command writes a .{ext} file; the name must end in .{ext}"));
        }
        if e.as_deref().is_some_and(is_risky) {
            return Err(format!("{given}: programs, scripts, web pages and Office files that can run macros are not written for an agent"));
        }
        let parent = joined.parent().ok_or_else(|| format!("{given}: no folder"))?;
        let real_parent = parent.canonicalize().map_err(|e| format!("{given}: {e}"))?;
        if !self.inside(&real_parent) {
            return Err(format!("{given}: leads outside {} (a link that leaves it is not followed)", self.root.display()));
        }
        let path = real_parent.join(&name);
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(format!("{given} already exists; nothing is overwritten for an agent (choose a new name)"));
        }
        Ok(Target { path, given: given.to_string() })
    }

    /// Write a new file (never over one that is there, never through a link).
    pub fn write(&self, t: &Target, bytes: &[u8]) -> Result<(), String> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&t.path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                format!("{} already exists; nothing is overwritten for an agent", t.given)
            } else {
                format!("{}: {e}", t.given)
            }
        })?;
        if let Err(e) = f.write_all(bytes).and_then(|()| f.flush()) {
            drop(f);
            let _ = std::fs::remove_file(&t.path);
            return Err(format!("{}: {e}", t.given));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mailcraft-mcp-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_and_writes_only_inside() {
        let base = dir("inside");
        let root = base.join("files");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(base.join("secret"), "s3cret").unwrap();
        std::fs::write(root.join("note.txt"), "hello").unwrap();
        let f = FilesDir::new(&root).unwrap();
        assert_eq!(f.read("note.txt").unwrap().1, b"hello");
        assert_eq!(f.read(&root.join("note.txt").to_string_lossy()).unwrap().0, "note.txt");
        for bad in ["../secret", "sub/../../secret", base.join("secret").to_string_lossy().as_ref(), "/etc/passwd", "", "sub"] {
            assert!(f.read(bad).is_err(), "{bad}");
        }
        assert!(f.target("../out.eml", "eml").is_err());
        assert!(f.target(&base.join("out.eml").to_string_lossy(), "eml").is_err());
        assert!(f.target("out.bat", "eml").is_err());
        assert!(f.target("out.bat", "").is_err());
        assert!(f.target(".profile", "").is_err());
        let t = f.target("sub/out.eml", "eml").unwrap();
        f.write(&t, b"x").unwrap();
        assert_eq!(std::fs::read(root.join("sub/out.eml")).unwrap(), b"x");
        // Never over an existing file.
        assert!(f.target("note.txt", "").is_err());
        assert!(f.write(&Target { path: root.join("note.txt").canonicalize().unwrap(), given: "note.txt".into() }, b"over").is_err());
        assert_eq!(std::fs::read(root.join("note.txt")).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn links_that_leave_are_refused() {
        let base = dir("links");
        let root = base.join("files");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(base.join("outside")).unwrap();
        std::fs::write(base.join("secret"), "s3cret").unwrap();
        std::os::unix::fs::symlink(base.join("secret"), root.join("link")).unwrap();
        std::os::unix::fs::symlink(base.join("outside"), root.join("away")).unwrap();
        std::os::unix::fs::symlink(base.join("outside/new.eml"), root.join("dangling.eml")).unwrap();
        std::fs::write(root.join("inner"), "in").unwrap();
        std::os::unix::fs::symlink(root.join("inner"), root.join("inlink")).unwrap();
        let f = FilesDir::new(&root).unwrap();
        assert!(f.read("link").is_err());
        assert!(f.read("away/x").is_err());
        assert_eq!(f.read("inlink").unwrap().1, b"in", "a link that stays inside is fine to read");
        assert!(f.target("away/out.eml", "eml").is_err());
        assert!(f.target("dangling.eml", "eml").is_err(), "never written through a link");
        assert!(f.target("link", "").is_err());
        assert!(!base.join("outside/new.eml").exists());
        assert_eq!(std::fs::read(base.join("secret")).unwrap(), b"s3cret");
        let _ = std::fs::remove_dir_all(&base);
    }
}
