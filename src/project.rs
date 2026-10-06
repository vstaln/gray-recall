//! A session cwd to its project: the git repository root, with worktrees
//! mapped to their main repository; the cwd itself outside a repository.

use crate::extract::lexical;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Absolute project root: the identity used for scoping.
    pub key: String,
    /// Last component of the key: what `-p` matches.
    pub name: String,
}

/// The project root for `dir`, or `None` outside any repository.
pub fn git_root(dir: &Path) -> Option<PathBuf> {
    for d in dir.ancestors() {
        let dot = d.join(".git");
        if dot.is_dir() {
            return Some(d.to_path_buf());
        }
        if dot.is_file() {
            return Some(main_repo(d, &dot).unwrap_or_else(|| d.to_path_buf()));
        }
    }
    None
}

/// A worktree's `.git` file points at a git dir whose `commondir` names the
/// main repository's `.git`. A submodule's git dir has no `commondir`.
fn main_repo(dir: &Path, dot: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dot).ok()?;
    let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gitdir = lexical(&dir.join(gitdir));
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = lexical(&gitdir.join(common.trim()));
    if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        Some(common)
    }
}

pub fn project_for(cwd: &str) -> Project {
    let path = Path::new(cwd);
    if cwd.is_empty() || !path.is_absolute() {
        return Project {
            key: cwd.to_string(),
            name: "unknown".into(),
        };
    }
    let path = lexical(path);
    let root = git_root(&path).unwrap_or(path);
    let key = root.to_string_lossy().into_owned();
    let name = root
        .file_name()
        .map_or_else(|| key.clone(), |n| n.to_string_lossy().into_owned());
    Project { key, name }
}

/// `project_for` with a per-cwd cache: sessions share a handful of cwds.
#[derive(Default)]
pub struct Resolver {
    cache: HashMap<String, Project>,
}

impl Resolver {
    pub fn project(&mut self, cwd: &str) -> Project {
        self.cache
            .entry(cwd.to_string())
            .or_insert_with(|| project_for(cwd))
            .clone()
    }
}

/// `8-4-4-4-12` lowercase or uppercase hex.
pub fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

pub fn short_session(id: &str) -> &str {
    if is_uuid(id) { &id[..8] } else { id }
}

pub fn display_id(session: &str, entry_id: i64) -> String {
    format!("{}:{entry_id}", short_session(session))
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod project_tests;
