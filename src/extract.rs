//! One turn to the fields the index stores.

use crate::session::Turn;
use crate::text::{clip, first_line, head_tail};
use regex::Regex;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

pub const REPLY_HEAD: usize = 2048;
pub const REPLY_TAIL: usize = 6144;
pub const GIST_CHARS: usize = 300;
pub const COMMAND_CHARS: usize = 300;
/// Cap on the joined `commands` text, in bytes.
pub const COMMANDS_CAP: usize = 4096;
pub const MAX_FILES: usize = 30;

const FILE_TOOLS: &[&str] = &["read", "edit", "write"];
/// Argument shown after a non-bash tool's name: the first one present.
const DETAIL_KEYS: &[&str] = &["path", "file_path", "url", "query", "pattern"];
const SPLIT: &[char] = &[';', '|', '&', '(', ')', '<', '>', '\'', '"', '`', '=', ','];
const ANCHORS: &[&str] = &["/", "./", "../", "~/"];
const EXTENSIONS: &[&str] = &[
    "rs", "toml", "lock", "md", "txt", "json", "jsonl", "yaml", "yml", "py", "js", "mjs", "cjs",
    "ts", "tsx", "jsx", "vue", "svelte", "html", "css", "scss", "sh", "bash", "zsh", "fish", "go",
    "c", "h", "cc", "cpp", "hpp", "java", "kt", "swift", "rb", "lua", "zig", "nix", "sql", "proto",
    "xml", "csv", "ini", "cfg", "conf", "env", "log", "diff", "patch", "png", "jpg", "jpeg",
    "webp", "gif", "svg", "pdf", "mp4", "mov", "webm", "wav", "mp3",
];

static COMMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\[[^\]\s]+(?: \(root-commit\))? ([0-9a-f]{7,40})\] ").expect("valid regex")
});
static EXIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^exit (\d+)").expect("valid regex"));

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extracted {
    pub reply: String,
    pub gist: String,
    pub commands: String,
    pub files: Vec<String>,
    pub commits: Vec<String>,
    pub failed: bool,
    pub has_reply: bool,
}

pub fn extract(turn: &Turn, cwd: &Path, root: &Path) -> Extracted {
    let mut files = Files {
        cwd,
        root,
        out: Vec::new(),
    };
    let mut commands = Vec::new();
    let mut used = 0;
    for tool in &turn.tools {
        if let Some(line) = command_line(&tool.name, &tool.args)
            && used + line.len() < COMMANDS_CAP
        {
            used += line.len() + 1;
            commands.push(line);
        }
        if FILE_TOOLS.contains(&tool.name.as_str()) {
            for key in ["path", "file_path"] {
                if let Some(p) = str_arg(&tool.args, key) {
                    files.add(p);
                }
            }
        } else if tool.name == "bash"
            && let Some(c) = str_arg(&tool.args, "command")
        {
            bash_paths(c).into_iter().for_each(|p| files.add(p));
        }
    }
    let mut commits: Vec<String> = Vec::new();
    for r in &turn.results {
        for c in COMMIT.captures_iter(&r.content) {
            if !commits.iter().any(|s| s == &c[1]) {
                commits.push(c[1].to_string());
            }
        }
    }
    Extracted {
        reply: head_tail(&turn.texts.join("\n\n"), REPLY_HEAD, REPLY_TAIL),
        gist: turn
            .texts
            .last()
            .map(|t| clip(first_line(t), GIST_CHARS))
            .unwrap_or_default(),
        commands: commands.join("\n"),
        files: files.out,
        commits,
        failed: turn
            .results
            .iter()
            .any(|r| r.is_error || exit_code(&r.content).is_some_and(|c| c != 0)),
        has_reply: !turn.texts.is_empty(),
    }
}

/// `.` and `..` resolved lexically: no disk access, symlinks are not followed.
pub fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn exit_code(content: &str) -> Option<u64> {
    EXIT.captures(first_line(content))
        .and_then(|c| c[1].parse().ok())
}

fn command_line(name: &str, args: &Value) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if name == "bash" {
        return str_arg(args, "command").map(|c| clip(c, COMMAND_CHARS));
    }
    let line = match DETAIL_KEYS.iter().find_map(|k| str_arg(args, k)) {
        Some(detail) => format!("{name} {detail}"),
        None => name.to_string(),
    };
    Some(clip(&line, COMMAND_CHARS))
}

struct Files<'a> {
    cwd: &'a Path,
    root: &'a Path,
    out: Vec<String>,
}

impl Files<'_> {
    fn add(&mut self, raw: &str) {
        let raw = raw.trim();
        if raw.is_empty() || self.out.len() >= MAX_FILES {
            return;
        }
        let path = if raw.starts_with("~/") {
            raw.to_string()
        } else {
            let abs = lexical(&self.cwd.join(raw));
            let rel = if self.root.as_os_str().is_empty() {
                None
            } else {
                abs.strip_prefix(self.root).ok()
            };
            match rel {
                Some(r) if r.as_os_str().is_empty() => return,
                Some(r) => r.to_string_lossy().into_owned(),
                None => abs.to_string_lossy().into_owned(),
            }
        };
        if !self.out.contains(&path) {
            self.out.push(path);
        }
    }
}

/// Path-shaped tokens of a bash command, skipping `cd` targets and heredoc bodies.
fn bash_paths(command: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut heredoc: Option<String> = None;
    for line in command.lines() {
        if let Some(end) = &heredoc {
            if line.trim() == end {
                heredoc = None;
            }
            continue;
        }
        heredoc = heredoc_delimiter(line);
        let mut after_cd = false;
        for tok in line
            .split(|c: char| c.is_whitespace() || SPLIT.contains(&c))
            .filter(|t| !t.is_empty())
        {
            if std::mem::replace(&mut after_cd, tok == "cd") {
                continue;
            }
            let tok = strip_line_suffix(tok);
            if looks_like_path(tok) {
                out.push(tok);
            }
        }
    }
    out
}

fn heredoc_delimiter(line: &str) -> Option<String> {
    let rest = line.split("<<").nth(1)?;
    if rest.starts_with('<') {
        return None;
    }
    let word: String = rest
        .trim_start_matches('-')
        .trim_start()
        .chars()
        .filter(|c| *c != '\'' && *c != '"')
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!word.is_empty()).then_some(word)
}

/// `src/main.rs:12:3` to `src/main.rs`.
fn strip_line_suffix(mut tok: &str) -> &str {
    while let Some((head, n)) = tok.rsplit_once(':') {
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            break;
        }
        tok = head;
    }
    tok
}

fn looks_like_path(tok: &str) -> bool {
    let known_ext = Path::new(tok)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
    tok.chars()
        .all(|c| c.is_ascii_alphanumeric() || "._/~+@-".contains(c))
        && tok.chars().any(|c| c.is_ascii_alphanumeric())
        && !tok.starts_with('-')
        && (known_ext || ANCHORS.iter().any(|a| tok.starts_with(a)))
        && !tok.starts_with("/dev/")
        && !tok.starts_with("/proc/")
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod extract_tests;
