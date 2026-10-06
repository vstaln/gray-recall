# gray-recall Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A gray plugin (`gray-recall`) that searches past gray sessions and returns a few ranked, cited turns in about 500 tokens, through a `recall` tool, `/recall` and `gray recall`.

**Architecture:** One Rust sidecar binary (wire protocol 1.1, same shape as gray-memory). Every call first catches the SQLite index up with `$GRAY_HOME/sessions` (only changed files, only new bytes when a file grew), then queries an FTS5 table with BM25 plus a recency factor, then renders compact cards inside a character budget.

**Tech Stack:** Rust 2024 (rustc 1.98.1), rusqlite 0.40.2 (bundled SQLite with FTS5), serde_json, clap 4 (derive), regex, chrono, strsim, anyhow; tempfile for tests.

**Spec:** `docs/specs/2026-10-06-gray-recall-design.md`. Read it with this plan; when they disagree, the spec wins and the plan gets fixed.

## Global Constraints

- Repo `~/grayplugins/gray-recall`, binary `gray-recall`, library `gray_recall`, MIT, edition 2024.
- Manifest `name: "recall"`, `protocol: "1.1"`, one tool `recall`, `commands: ["/recall"]`, `hooks: []`.
- Crate versions, all published at least 7 days ago: rusqlite 0.40.2 (`bundled`), serde_json 1.0.151, clap 4.6.7 (`derive`), anyhow 1.0.104, regex 1.13.1, chrono 0.4.45, strsim 0.11.1; dev: tempfile 3.27.0. `serde` (derive) from the spec is not added: `serde_json::json!` covers all JSON output.
- The word "leviathan" appears only in `THIRD_PARTY_NOTICES.md`. No leviathan code is copied.
- Index: `$GRAY_HOME/recall/index.db` (`GRAY_HOME` defaults to `~/.gray`), directory 0700, file 0600, WAL. Writes only under `$GRAY_HOME/recall`; no network; no `host/*` capabilities.
- Every text field passes `redact::redact_secrets` before it is written. File paths are never redacted.
- Exit codes: 0 ok (including zero hits), 1 error, 2 bad request, 3 ambiguous or unknown project / id. The tool reply sets `is_error` only for codes 1-3.
- Defaults: `limit` 5 (max 20), `max_chars` 2500 for search and 8000 for show (clamped to 500..=50000), `around` 0 (max 10).
- The `OTHER PROJECT` fallback runs only for the default scope (no `-p`, no `--all`): an explicit `-p` is the caller's choice and is not widened.
- Tests are `*_tests.rs` files next to each module, included with `#[cfg(test)] #[path = "x_tests.rs"] mod x_tests;` (gray-memory's pattern). `cargo test` is the whole check.
- Build pressure: every cargo command runs with `CARGO_BUILD_JOBS=4`.
- Before each commit: `cargo fmt`, then `cargo clippy --all-targets -- -D warnings` and `cargo test` clean. The code below is not hand-formatted to rustfmt's exact output; `cargo fmt` fixes that.

## How to use this plan

Every complete file is a block opened by `~~~rust file=<path>` (or another language) and closed by `~~~`. Write blocks to disk with the extractor instead of copying by hand:

~~~bash
python3 docs/plans/extract.py docs/plans/2026-10-06-gray-recall-plan.md src/text.rs src/text_tests.rs
~~~

`--list` prints every block path. Edits to existing files are shown as exact commands. Shell snippets are bash (some use `<(...)`). `P=docs/plans/2026-10-06-gray-recall-plan.md` and `X="python3 docs/plans/extract.py $P"` are assumed in every task.

## File map

| File | Responsibility | Task |
|---|---|---|
| `Cargo.toml`, `.gitignore`, `LICENSE`, `THIRD_PARTY_NOTICES.md`, `.github/workflows/test.yml` | Crate, license, notices, CI | 1 |
| `src/lib.rs` | Module list, plugin constants, `gray_home()` | 1 (one `pub mod` line added per task) |
| `src/text.rs` | `cap`, `cap_tail`, `head_tail`, `first_line`, `clip`, `fnv1a` | 1 |
| `src/redact.rs` | Secrets-only redaction, generated from gray-memory's copy | 1 |
| `src/testkit.rs` | `SessionBuilder`: writes session files shaped like gray's | 2 |
| `src/session.rs` | Session file bytes to turns on the active branch | 2 |
| `src/extract.rs` | Turn to reply, gist, commands, files, commits, failed | 3 |
| `src/project.rs` | cwd to git-root project (worktrees to main repo), short ids | 4 |
| `src/index.rs` | Schema, open/reset, rows, sessions, projects, meta, stats | 5 |
| `src/catchup.rs` | Writer lock, change detection, parallel parse, per-session commits | 6 |
| `src/query.rs` | Request, match expression, dates, project tiers, ranking, fallback, show | 7 |
| `src/render.rs` | Cards in a budget, show view, projects, status, JSON | 8 |
| `src/cli.rs` | clap CLI, tool JSON to request, dispatch, exit codes | 9 |
| `src/main.rs` | Manifest, sidecar loop, CLI entry | 9 |
| `tests/protocol.rs` | Spawns the binary: wire replies and CLI exit codes | 9 |
| `README.md` | Usage, install, the `gray recall search` note | 10 |

---

### Task 1: Repo, text helpers, secrets-only redaction, notices, CI

**Files:**
- Create: `Cargo.toml` (via `cargo init` + `cargo add`), `.gitignore`, `LICENSE`, `src/lib.rs`, `src/main.rs` (placeholder until Task 9), `src/text.rs`, `src/text_tests.rs`, `src/redact.rs` (generated), `src/redact_tests.rs`, `THIRD_PARTY_NOTICES.md`, `.github/workflows/test.yml`, `docs/plans/trim_redact.py`

**Interfaces:**
- Produces: `gray_recall::{PLUGIN_NAME, PROTOCOL, COMMANDS, gray_home() -> anyhow::Result<PathBuf>}`;
  `text::{cap(&str, usize) -> &str, cap_tail(&str, usize) -> &str, head_tail(&str, usize, usize) -> String, first_line(&str) -> &str, clip(&str, usize) -> String, fnv1a(&[u8]) -> u64}`;
  `redact::{redact_secrets(&str) -> String, REDACTED: &str}`.

- [ ] **Step 1: Create the crate**

~~~bash
cd ~/grayplugins/gray-recall
cargo init --name gray-recall --vcs none
grep -n '^edition' Cargo.toml   # expect: edition = "2024"
cargo add anyhow@1.0.104 chrono@0.4.45 regex@1.13.1 serde_json@1.0.151 strsim@0.11.1
cargo add clap@4.6.7 --features derive
cargo add rusqlite@0.40.2 --features bundled
cargo add --dev tempfile@3.27.0
cat >> Cargo.toml <<'EOF'

[lib]
name = "gray_recall"
path = "src/lib.rs"

[[bin]]
name = "gray-recall"
path = "src/main.rs"

[profile.release]
strip = true
EOF
sed -i '0,/^version = /s//license = "MIT"\ndescription = "Search past gray sessions: ranked, cited turns (wire v1.1)."\nversion = /' Cargo.toml
printf '/target/\n' > .gitignore
cp ~/grayplugins/gray-memory/LICENSE LICENSE
head -3 LICENSE   # expect: MIT License / blank / Copyright (c) 2026 vstaln
~~~

- [ ] **Step 2: Write `lib.rs`, a placeholder `main.rs`, and the text tests**

~~~rust file=src/lib.rs
//! gray-recall: search past gray sessions for prior work.

use std::path::PathBuf;

pub mod redact;
pub mod text;

/// Manifest name: the host forwards `gray recall ...` to this binary.
pub const PLUGIN_NAME: &str = "recall";
/// Wire protocol version.
pub const PROTOCOL: &str = "1.1";
/// Slash commands claimed in the TUI.
pub const COMMANDS: &[&str] = &["/recall"];

/// `~/.gray`, overridable with `GRAY_HOME` (same resolution as gray-memory).
pub fn gray_home() -> anyhow::Result<PathBuf> {
    std::env::var_os("GRAY_HOME")
        .filter(|v| !v.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".gray"))
        })
        .ok_or_else(|| anyhow::anyhow!("cannot resolve home: set GRAY_HOME or HOME"))
}
~~~

~~~rust file=src/text_tests.rs
use super::*;

#[test]
fn cap_keeps_char_boundaries() {
    assert_eq!(cap("héllo", 2), "h");
    assert_eq!(cap("héllo", 3), "hé");
    assert_eq!(cap("abc", 10), "abc");
}

#[test]
fn cap_tail_keeps_char_boundaries() {
    assert_eq!(cap_tail("abcé", 1), "");
    assert_eq!(cap_tail("abcé", 2), "é");
    assert_eq!(cap_tail("abc", 10), "abc");
}

#[test]
fn head_tail_joins_the_ends_of_long_text() {
    assert_eq!(head_tail("short", 3, 3), "short");
    assert_eq!(head_tail("abcdefghij", 3, 2), "abc … ij");
}

#[test]
fn first_line_skips_blank_lines() {
    assert_eq!(first_line("\n  \n  Done: fixed it  \nmore"), "Done: fixed it");
    assert_eq!(first_line(""), "");
}

#[test]
fn clip_collapses_whitespace_and_marks_cuts() {
    assert_eq!(clip("a\n  b\tc", 10), "a b c");
    assert_eq!(clip("abcdefgh", 5), "abcd…");
    assert_eq!(clip("abcdefgh", 5).chars().count(), 5);
}

#[test]
fn fnv1a_matches_reference_values() {
    assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
}
~~~

~~~bash
$X src/lib.rs src/text_tests.rs
printf 'fn main() {}\n' > src/main.rs
printf '#[cfg(test)]\n#[path = "text_tests.rs"]\nmod text_tests;\n' > src/text.rs
: > src/redact.rs
~~~

- [ ] **Step 3: Run the tests to see them fail**

Run: `CARGO_BUILD_JOBS=4 cargo test --lib text`
Expected: compile errors such as `cannot find function cap in this scope`.

- [ ] **Step 4: Implement `text.rs`**

~~~rust file=src/text.rs
//! Small string helpers shared by extraction, indexing and rendering.

/// Longest prefix of `s` that is at most `max` bytes and ends on a char boundary.
pub fn cap(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Longest suffix of `s` that is at most `max` bytes and starts on a char boundary.
pub fn cap_tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// `s` if it fits in `head + tail` bytes, else its first `head` and last
/// `tail` bytes joined by ` … `.
pub fn head_tail(s: &str, head: usize, tail: usize) -> String {
    if s.len() <= head + tail {
        return s.to_string();
    }
    format!("{} … {}", cap(s, head), cap_tail(s, tail))
}

/// The first line of `s` that is not blank, trimmed.
pub fn first_line(s: &str) -> &str {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

/// Whitespace runs collapsed to one space, cut to `max` chars with a trailing `…`.
pub fn clip(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// FNV-1a, 64-bit: the change-detection hash for catch-up's byte windows.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod text_tests;
~~~

- [ ] **Step 5: Run the text tests**

Run: `$X src/text.rs && CARGO_BUILD_JOBS=4 cargo test --lib text`
Expected: 6 passed.

- [ ] **Step 6: Write the redaction tests**

Fake secrets are assembled at runtime so the repository never holds a secret-shaped literal.

~~~rust file=src/redact_tests.rs
use super::*;

fn fake(prefix: &str) -> String {
    format!("{prefix}{}", "Zq3xK9".repeat(6))
}

fn discord_token() -> String {
    let first = "Tk4Lm".repeat(5);
    format!("M{}.{}.{}", &first[..23], "Gh7k2a", "Q9w".repeat(10))
}

#[test]
fn provider_keys_are_redacted() {
    for prefix in ["sk-", "ghp_", "xoxb-"] {
        let key = fake(prefix);
        let out = redact_secrets(&format!("use {key} here"));
        assert!(!out.contains(&key), "{prefix} leaked: {out}");
        assert!(out.contains(REDACTED), "{out}");
        assert!(out.starts_with("use ") && out.ends_with(" here"), "{out}");
    }
}

#[test]
fn named_assignments_are_redacted() {
    let value = "Zq3xK9Zq3xK9Zq3xK9";
    let out = redact_secrets(&format!("export DISCORD_BOT_TOKEN={value}"));
    assert!(!out.contains(value), "{out}");
}

#[test]
fn bare_discord_tokens_are_redacted() {
    let token = discord_token();
    let out = redact_secrets(&format!("the bot uses {token} now"));
    assert!(!out.contains(&token), "{out}");
    assert!(out.starts_with("the bot uses "), "{out}");
}

#[test]
fn bearer_values_are_redacted() {
    let out = redact_secrets("Authorization: Bearer qqq");
    assert!(!out.contains("qqq"), "{out}");
}

#[test]
fn paths_and_prose_survive() {
    let s = "edited crates/gray/src/lib.rs and /home/x/notes.md; basic auth is fine";
    assert_eq!(redact_secrets(s), s);
}

#[test]
fn every_line_of_multiline_input_is_checked() {
    let key = fake("sk-");
    let out = redact_secrets(&format!("line one\nkey {key}\nline three"));
    assert!(!out.contains(&key), "{out}");
    assert!(out.starts_with("line one\n") && out.ends_with("\nline three"), "{out}");
}
~~~

Run: `$X src/redact_tests.rs && printf '#[cfg(test)]\n#[path = "redact_tests.rs"]\nmod redact_tests;\n' > src/redact.rs && CARGO_BUILD_JOBS=4 cargo test --lib redact`
Expected: compile error `cannot find function redact_secrets`.

- [ ] **Step 7: Generate `redact.rs` from gray-memory's copy**

The generator drops the five path functions, the three path constants and the two `if let Some(kind) = classify_path(..) { .. }` blocks inside `redact_token` (brace-matched, with string and char literals ignored). It renames `redact_for_disclosure` to `redact_secret_tokens`, replaces the header, and appends `redact_secrets` (which adds a Discord bot-token pattern) plus the test hook. It fails if any removed name is still referenced in code.

~~~python file=docs/plans/trim_redact.py
#!/usr/bin/env python3
"""Copy gray-memory's src/redact.rs with path stripping removed.

Usage: trim_redact.py <gray-memory/src/redact.rs> <gray-recall/src/redact.rs>
"""
import re, sys

src, dst = sys.argv[1], sys.argv[2]
lines = open(src, encoding="utf-8").read().split("\n")
DROP_FNS = {"looks_absolute", "classify_path", "trim_path_punctuation", "unescape_path", "looks_relative"}
DROP_CONSTS = ("const PATH_PLACEHOLDER", "pub const REDACTION_ABSOLUTE_PATH", "pub const REDACTION_RELATIVE_PATH")
GONE = DROP_FNS | {"PATH_PLACEHOLDER", "REDACTION_ABSOLUTE_PATH", "REDACTION_RELATIVE_PATH"}


def code(line):
    line = re.sub(r'"(?:\\.|[^"\\])*"', '""', line)
    line = re.sub(r"'(?:\\.|[^'\\])'", "''", line)
    return line.split("//")[0]


def block_end(i):
    depth, opened = 0, False
    for j in range(i, len(lines)):
        c = code(lines[j])
        depth += c.count("{") - c.count("}")
        opened = opened or "{" in c
        if opened and depth == 0:
            return j
    sys.exit(f"unbalanced block at line {i + 1}")


def lead_start(i, prefixes):
    while i > 0 and lines[i - 1].lstrip().startswith(prefixes):
        i -= 1
    return i


drop = set()
for i, line in enumerate(lines):
    m = re.match(r"\s*(?:pub\s+)?fn\s+(\w+)", line)
    if m and m.group(1) in DROP_FNS:
        drop.update(range(lead_start(i, ("///", "#[")), block_end(i) + 1))
    elif line.lstrip().startswith(DROP_CONSTS):
        drop.update(range(lead_start(i, ("///",)), i + 1))
    elif re.match(r"\s*if let Some\(kind\) = classify_path\(", line):
        drop.update(range(lead_start(i, ("//",)), block_end(i) + 1))

kept = [l for i, l in enumerate(lines) if i not in drop]
start = next(i for i, l in enumerate(kept) if l.startswith("use std::collections::BTreeSet;"))
HEADER = """// Copied from vstaln/gray-memory `src/redact.rs` with path stripping removed,
// so file paths stay searchable. That file is vendored from gray
// `crates/gray-core/src/redaction.rs`, itself vendored from Hmbown/CodeWhale
// (MIT) `crates/workflow/src/redaction.rs`.
// Copyright (c) 2024-2025 DeepSeek CLI Contributors. See THIRD_PARTY_NOTICES.md.
//! Secret redaction for everything gray-recall writes to its index.
//!
//! Secret-shaped tokens (provider keys, bearer tokens, `SOMETHING_KEY=value`
//! assignments, Discord bot tokens) become `<redacted>`. Paths are kept:
//! they are what `--file` searches.

use std::sync::LazyLock;

use regex::Regex;
"""
TAIL = r'''
/// A Discord bot token: three base64url segments joined by dots.
static DISCORD_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[MNO][A-Za-z0-9_-]{23,27}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27,40}\b")
        .expect("valid regex")
});

/// `input` with every secret replaced by [`REDACTED`]; everything else,
/// paths included, is kept verbatim.
#[must_use]
pub fn redact_secrets(input: &str) -> String {
    let text = redact_secret_tokens(input).into_text();
    DISCORD_TOKEN.replace_all(&text, REDACTED).into_owned()
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod redact_tests;
'''
out = HEADER + "\n".join(kept[start:])
out = out.replace("redact_for_disclosure", "redact_secret_tokens")
out = out.replace("/// Redact absolute paths and secret-shaped tokens from `input`.",
                  "/// Redact secret-shaped tokens from `input`; paths are kept.")
out = re.sub(r"\n{3,}", "\n\n", out).rstrip("\n") + "\n" + TAIL
left = sorted({n for l in out.split("\n") for n in GONE if n in code(l)})
if left:
    sys.exit(f"still referenced in code: {left}")
open(dst, "w", encoding="utf-8").write(out)
print(f"wrote {dst}: {len(lines)} -> {out.count(chr(10))} lines, dropped {len(drop)}")
~~~

~~~bash
$X docs/plans/trim_redact.py
python3 docs/plans/trim_redact.py ~/grayplugins/gray-memory/src/redact.rs src/redact.rs
~~~

Expected: `wrote src/redact.rs: 609 -> ~4xx lines`. Then confirm that only path code was removed:

~~~bash
diff <(sed 's/redact_for_disclosure/redact_secret_tokens/' ~/grayplugins/gray-memory/src/redact.rs) src/redact.rs | grep '^<' | grep -v '^< *//' | head -100
~~~

Every removed non-comment line must belong to a path function, a path constant or a `classify_path` block. Anything else means the generator cut too much: fix the generator and rerun it; never hand-patch `redact.rs`.

- [ ] **Step 8: Run the redaction tests**

Run: `CARGO_BUILD_JOBS=4 cargo test --lib redact`
Expected: 6 passed. If `paths_and_prose_survive` fails, a remaining secret rule treats a path as a credential: locate it (`grep -n 'fn looks_like_credential_value' -A20 src/redact.rs`) and report it before changing anything.

- [ ] **Step 9: Notices and CI**

~~~markdown file=THIRD_PARTY_NOTICES.md
# Third-party notices

gray-recall is MIT-licensed (see `LICENSE`). It includes, or is informed by,
the following.

## Design ideas: elstongun/leviathan (Apache-2.0)

The retrieval design follows ideas from Leviathan: SQLite FTS5 with BM25
ranking, compact cited result cards, an explicit "shown N of M" count, and
tiered name resolution that reports ambiguity instead of guessing. No
Leviathan code is copied. Leviathan's NOTICE:

    Leviathan
    Copyright 2026 Joshua Baker

    This product includes SQLite (public domain), compiled in via the
    rusqlite / libsqlite3-sys crates. See THIRD_PARTY.md for all dependencies
    and their licenses.

## MIT: Hmbown/CodeWhale, via gray and gray-memory

`src/redact.rs` is a copy of vstaln/gray-memory `src/redact.rs` with path
stripping removed. That file comes from gray
`crates/gray-core/src/redaction.rs`, which is vendored from Hmbown/CodeWhale
`crates/workflow/src/redaction.rs`.

    MIT License

    Copyright (c) 2024-2025 DeepSeek CLI Contributors

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.

## SQLite (public domain)

Compiled in through rusqlite's `bundled` feature.
~~~

The MIT text must match the source byte for byte (after removing the 4-space indent); this prints nothing:

~~~bash
$X THIRD_PARTY_NOTICES.md
diff <(sed -n '/^    MIT License/,/^    SOFTWARE.$/p' THIRD_PARTY_NOTICES.md | sed 's/^    //') ~/gray/reference/codewhale/LICENSE
~~~

~~~yaml file=.github/workflows/test.yml
name: test
on: [push, pull_request]
permissions:
  contents: read
jobs:
  rust:
    runs-on: ubuntu-latest
    env:
      CARGO_BUILD_JOBS: '4'
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --locked --all-targets -- -D warnings
      - run: cargo test --locked --all-targets -- --test-threads=1
~~~

- [ ] **Step 10: Format, lint, test, commit**

~~~bash
$X .github/workflows/test.yml
CARGO_BUILD_JOBS=4 cargo fmt
CARGO_BUILD_JOBS=4 cargo clippy --all-targets -- -D warnings
CARGO_BUILD_JOBS=4 cargo test
git add -A
git commit -m "Crate skeleton, text helpers, secrets-only redaction, notices, CI"
~~~

Expected: clippy clean, 12 tests pass.

---

### Task 2: Session builder for tests, and session parsing

**Files:**
- Create: `src/testkit.rs`, `src/session.rs`, `src/session_tests.rs`
- Modify: `src/lib.rs` (add `pub mod session;` and `#[cfg(test)] pub mod testkit;`)

**Interfaces:**
- Consumes: `text::cap`.
- Produces:
  - `session::{INJECTED_PREFIXES, COMPACTION_WINDOW_MS, PROMPT_CAP}`
  - `session::ToolUse { name: String, args: serde_json::Value }`
  - `session::ToolResult { content: String, is_error: bool }`
  - `session::Turn { entry_id: i64, date: i64, offset: u64, prompt: String, texts: Vec<String>, tools: Vec<ToolUse>, results: Vec<ToolResult> }`
  - `session::Parsed { cwd: Option<String>, turns: Vec<Turn>, root: Option<i64>, skipped_offsets: Vec<u64> }`
  - `session::parse(bytes: &[u8], base: u64) -> Parsed`: `bytes` start at absolute file offset `base`; the header is read only when `base == 0`; a last line without `\n` is not consumed.
  - `testkit::{T0, SessionBuilder}` with `new(id, cwd)`, `id()`, `user(text) -> i64`, `assistant(text) -> i64`, `tool(name, args, result, is_error) -> i64`, `bash(command, result) -> i64`, `image_only() -> i64`, `branch_from(parent) -> &mut Self`, `clock(ts) -> &mut Self`, `compaction(kept: &[&str])`, `raw(line)`, `text() -> String`, `write(dir) -> PathBuf` (writes `<dir>/<id>.jsonl`).

- [ ] **Step 1: Write the session builder**

It writes exactly the shape confirmed on the real sessions: a header `{version, id, timestamp, cwd, model}`, then entries `{entry_id, parent_id, timestamp, message{role, content[]}}`, integer ids and millisecond timestamps, `compaction_boundary: true` on boundary markers.

~~~rust file=src/testkit.rs
//! Builds session files shaped like gray's, for tests.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Header timestamp; entries follow at one-second steps (2026-09-21 UTC).
pub const T0: i64 = 1_790_000_000_000;

pub struct SessionBuilder {
    id: String,
    lines: Vec<String>,
    next: i64,
    last: Option<i64>,
    parent: Option<i64>,
    ts: i64,
}

impl SessionBuilder {
    pub fn new(id: &str, cwd: &str) -> Self {
        let header = json!({"version": 1, "id": id, "timestamp": T0, "cwd": cwd, "model": "test"});
        Self { id: id.to_string(), lines: vec![header.to_string()], next: 1, last: None, parent: None, ts: T0 }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn entry_at(&mut self, ts: i64, role: &str, content: Value, boundary: bool) -> i64 {
        let id = self.next;
        self.next += 1;
        let parent = self.parent.take().or(self.last);
        let mut e = json!({"entry_id": id, "parent_id": parent, "timestamp": ts,
                           "message": {"role": role, "content": content}});
        if boundary {
            e["compaction_boundary"] = json!(true);
        }
        self.lines.push(e.to_string());
        self.last = Some(id);
        id
    }

    fn entry(&mut self, role: &str, content: Value) -> i64 {
        self.ts += 1000;
        let ts = self.ts;
        self.entry_at(ts, role, content, false)
    }

    pub fn user(&mut self, text: &str) -> i64 {
        self.entry("user", json!([{"type": "text", "text": text}]))
    }

    pub fn assistant(&mut self, text: &str) -> i64 {
        self.entry("assistant", json!([{"type": "text", "text": text}]))
    }

    /// An assistant `tool_use` and the user `tool_result` answering it; returns the result's id.
    pub fn tool(&mut self, name: &str, args: Value, result: &str, is_error: bool) -> i64 {
        let call = format!("call_{}", self.next);
        self.entry("assistant", json!([{"type": "tool_use", "id": call, "name": name, "args": args}]));
        self.entry("user", json!([{"type": "tool_result", "id": call, "content": result, "is_error": is_error}]))
    }

    pub fn bash(&mut self, command: &str, result: &str) -> i64 {
        self.tool("bash", json!({"command": command}), result, false)
    }

    pub fn image_only(&mut self) -> i64 {
        self.entry("user", json!([{"type": "image", "mime": "image/png", "data": "AAAA"}]))
    }

    /// The next entry's parent is `parent` (a rewind), not the last entry.
    pub fn branch_from(&mut self, parent: i64) -> &mut Self {
        self.parent = Some(parent);
        self
    }

    /// The next entry is stamped `ts + 1000`.
    pub fn clock(&mut self, ts: i64) -> &mut Self {
        self.ts = ts;
        self
    }

    /// A compaction boundary followed by copies of `kept`, all stamped with the boundary's time.
    pub fn compaction(&mut self, kept: &[&str]) {
        self.ts += 1000;
        let ts = self.ts;
        let marker = "compaction boundary: entries before this point are superseded; replay starts after it";
        self.entry_at(ts, "system", json!([{"type": "text", "text": marker}]), true);
        for text in kept {
            self.entry_at(ts, "user", json!([{"type": "text", "text": text}]), false);
        }
    }

    pub fn raw(&mut self, line: &str) {
        self.lines.push(line.to_string());
    }

    pub fn text(&self) -> String {
        let mut s = self.lines.join("\n");
        s.push('\n');
        s
    }

    pub fn write(&self, dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{}.jsonl", self.id));
        std::fs::write(&path, self.text()).unwrap();
        path
    }
}
~~~

- [ ] **Step 2: Write the failing session tests**

~~~rust file=src/session_tests.rs
use super::*;
use crate::testkit::{SessionBuilder, T0};
use serde_json::json;

fn parse_all(b: &SessionBuilder) -> Parsed {
    parse(b.text().as_bytes(), 0)
}

fn prompts(p: &Parsed) -> Vec<&str> {
    p.turns.iter().map(|t| t.prompt.as_str()).collect()
}

#[test]
fn a_prompt_and_its_work_form_one_turn() {
    let mut b = SessionBuilder::new("s1", "/work/app");
    let p = b.user("fix the build");
    b.bash("cargo build", "exit 0 · 1.2s");
    b.assistant("Fixed: missing import.");
    let parsed = parse_all(&b);
    assert_eq!(parsed.cwd.as_deref(), Some("/work/app"));
    assert_eq!(parsed.turns.len(), 1);
    let t = &parsed.turns[0];
    assert_eq!((t.entry_id, t.date, t.prompt.as_str()), (p, T0 + 1000, "fix the build"));
    assert_eq!(t.tools.len(), 1);
    assert_eq!(t.tools[0].name, "bash");
    assert_eq!(t.tools[0].args, json!({"command": "cargo build"}));
    assert_eq!(t.results, vec![ToolResult { content: "exit 0 · 1.2s".into(), is_error: false }]);
    assert_eq!(t.texts, vec!["Fixed: missing import.".to_string()]);
    assert!(parsed.skipped_offsets.is_empty());
    assert_eq!(parsed.root, Some(p));
}

#[test]
fn work_attaches_to_the_turn_it_follows() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.user("one");
    b.assistant("a");
    b.user("two");
    b.bash("ls", "exit 0");
    b.assistant("b");
    let p = parse_all(&b);
    assert_eq!(prompts(&p), ["one", "two"]);
    assert!(p.turns[0].tools.is_empty());
    assert_eq!(p.turns[1].tools.len(), 1);
    assert_eq!(p.turns[1].texts, vec!["b".to_string()]);
}

#[test]
fn turn_offsets_point_at_the_prompt_line() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.user("one");
    b.assistant("a");
    let second = b.user("two");
    b.assistant("b");
    let text = b.text();
    let parsed = parse(text.as_bytes(), 0);
    let off = parsed.turns[1].offset as usize;
    let line = text[off..].lines().next().unwrap();
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(v["entry_id"], json!(second));
}

#[test]
fn parsing_from_an_offset_reports_the_branch_root() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.user("one");
    b.assistant("a");
    let second = b.user("two");
    b.assistant("b");
    let text = b.text();
    let off = parse(text.as_bytes(), 0).turns[1].offset;
    let tail = parse(&text.as_bytes()[off as usize..], off);
    assert_eq!(tail.root, Some(second));
    assert_eq!(tail.cwd, None);
    assert_eq!(prompts(&tail), ["two"]);
    assert_eq!(tail.turns[0].offset, off);
}

#[test]
fn rewound_branches_are_dropped() {
    let mut b = SessionBuilder::new("s1", "/w");
    let first = b.user("first");
    let ok = b.assistant("ok");
    b.user("abandoned");
    b.assistant("x");
    b.branch_from(ok);
    b.user("retry");
    b.assistant("y");
    let p = parse_all(&b);
    assert_eq!(prompts(&p), ["first", "retry"]);
    assert_eq!(p.root, Some(first));
}

#[test]
fn injected_prompts_never_start_a_turn() {
    for prefix in INJECTED_PREFIXES {
        let mut b = SessionBuilder::new("s1", "/w");
        b.user("real question");
        b.user(&format!("{prefix} details"));
        b.assistant("answer");
        let p = parse_all(&b);
        assert_eq!(prompts(&p), ["real question"], "{prefix}");
        assert_eq!(p.turns[0].texts, vec!["answer".to_string()], "{prefix}");
    }
}

#[test]
fn injected_prompts_before_any_prompt_are_ignored() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.user("[Background task notification] job 1 finished");
    b.assistant("noted");
    assert!(parse_all(&b).turns.is_empty());
}

#[test]
fn skill_invocations_become_named_turns() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.user("<skill name=\"sureforge\" path=\"/x/SKILL.md\">\n# SureForge\nbody text\n</skill>");
    b.assistant("following the skill");
    let p = parse_all(&b);
    assert_eq!(prompts(&p), ["[skill sureforge]"]);
}

#[test]
fn image_only_messages_do_not_start_turns() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.user("look at this");
    b.image_only();
    b.assistant("I see a chart");
    let p = parse_all(&b);
    assert_eq!(prompts(&p), ["look at this"]);
    assert_eq!(p.turns[0].texts, vec!["I see a chart".to_string()]);
}

#[test]
fn malformed_lines_are_skipped_and_counted() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.user("q");
    b.raw("not json");
    b.raw("[1, 2]");
    b.raw(r#"{"no": "entry"}"#);
    b.assistant("a");
    let p = parse_all(&b);
    assert_eq!(prompts(&p), ["q"]);
    assert_eq!(p.turns[0].texts, vec!["a".to_string()]);
    assert_eq!(p.skipped_offsets.len(), 3);
}

#[test]
fn a_half_written_last_line_is_left_for_later() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.user("q");
    b.assistant("done");
    let text = b.text();
    let p = parse(&text.as_bytes()[..text.len() - 10], 0);
    assert_eq!(prompts(&p), ["q"]);
    assert!(p.turns[0].texts.is_empty());
    assert!(p.skipped_offsets.is_empty());
}

#[test]
fn compaction_copies_are_not_indexed_twice() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.user("first question");
    b.assistant("first answer");
    b.compaction(&["first question", "a copy of the first answer"]);
    b.user("second question");
    b.assistant("second answer");
    let p = parse_all(&b);
    assert_eq!(prompts(&p), ["first question", "second question"]);
    assert_eq!(p.turns[0].texts, vec!["first answer".to_string()]);
}

#[test]
fn string_content_is_read_as_text() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.raw(&json!({"entry_id": 1, "parent_id": null, "timestamp": T0,
                  "message": {"role": "user", "content": "plain string prompt"}}).to_string());
    assert_eq!(prompts(&parse_all(&b)), ["plain string prompt"]);
}

#[test]
fn tool_errors_are_kept() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.user("q");
    b.tool("read", json!({"path": "x.rs"}), "no such file", true);
    let p = parse_all(&b);
    assert!(p.turns[0].results[0].is_error);
}

#[test]
fn long_prompts_are_capped() {
    let mut b = SessionBuilder::new("s1", "/w");
    b.user(&"x".repeat(5000));
    assert_eq!(parse_all(&b).turns[0].prompt.len(), PROMPT_CAP);
}
~~~

~~~bash
$X src/testkit.rs src/session_tests.rs
sed -i 's/^pub mod redact;/pub mod redact;\npub mod session;/; s/^pub mod text;/pub mod text;\n#[cfg(test)]\npub mod testkit;/' src/lib.rs
printf '#[cfg(test)]\n#[path = "session_tests.rs"]\nmod session_tests;\n' > src/session.rs
~~~

- [ ] **Step 3: Run the tests to see them fail**

Run: `CARGO_BUILD_JOBS=4 cargo test --lib session`
Expected: compile errors (`cannot find function parse`, `cannot find type Parsed`).

- [ ] **Step 4: Implement `session.rs`**

~~~rust file=src/session.rs
//! One session file's bytes to the turns on its active branch.

use crate::text::cap;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

/// Prompts gray injects on its own (found by surveying every user text in
/// the local sessions). They never start a turn and their text is dropped.
pub const INJECTED_PREFIXES: &[&str] = &[
    "[Background task notification]",
    "Another language model started to solve this problem",
    "[gray loop guard:",
    "The conversation history before this point was compacted",
];
/// Entries this close to a compaction boundary are copies of kept messages.
pub const COMPACTION_WINDOW_MS: i64 = 500;
/// Prompt cap, in bytes.
pub const PROMPT_CAP: usize = 2048;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolUse {
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

/// A real user prompt and everything the agent did until the next one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Turn {
    pub entry_id: i64,
    /// Prompt timestamp, unix ms.
    pub date: i64,
    /// Absolute byte offset of the prompt's line in the file.
    pub offset: u64,
    pub prompt: String,
    /// Non-blank assistant text blocks, in order.
    pub texts: Vec<String>,
    pub tools: Vec<ToolUse>,
    pub results: Vec<ToolResult>,
}

#[derive(Debug, Default)]
pub struct Parsed {
    /// Header cwd; only read when parsing from offset 0.
    pub cwd: Option<String>,
    pub turns: Vec<Turn>,
    /// First entry of the active branch within the parsed bytes.
    pub root: Option<i64>,
    /// Absolute offsets of complete lines that were not valid entries.
    pub skipped_offsets: Vec<u64>,
}

struct Entry {
    id: i64,
    parent: Option<i64>,
    ts: i64,
    offset: u64,
    boundary: bool,
    role: String,
    blocks: Vec<Value>,
}

enum Line {
    Entry(Entry),
    Header(Option<String>),
    Bad,
}

/// Parse `bytes`, which start at absolute file offset `base`. A last line
/// without a trailing newline is not consumed; the next call reads it.
pub fn parse(bytes: &[u8], base: u64) -> Parsed {
    let mut out = Parsed::default();
    let mut entries = Vec::new();
    let mut pos = 0;
    while let Some(len) = bytes[pos..].iter().position(|b| *b == b'\n') {
        let line = &bytes[pos..pos + len];
        let offset = base + pos as u64;
        pos += len + 1;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match read_line(line, offset) {
            Line::Entry(e) => entries.push(e),
            Line::Header(cwd) if offset == 0 => out.cwd = cwd,
            _ => out.skipped_offsets.push(offset),
        }
    }
    let branch = active_branch(&entries);
    out.root = branch.first().map(|&i| entries[i].id);
    out.turns = turns(entries, &branch);
    out
}

fn read_line(line: &[u8], offset: u64) -> Line {
    let Ok(Value::Object(mut obj)) = serde_json::from_slice::<Value>(line) else {
        return Line::Bad;
    };
    let Some(id) = obj.get("entry_id").and_then(Value::as_i64) else {
        return if obj.contains_key("id") {
            Line::Header(obj.get("cwd").and_then(Value::as_str).map(str::to_string))
        } else {
            Line::Bad
        };
    };
    let mut msg = obj.remove("message").unwrap_or(Value::Null);
    let role = msg.get("role").and_then(Value::as_str).unwrap_or("").to_string();
    let blocks = match msg.get_mut("content").map(Value::take) {
        Some(Value::Array(a)) => a,
        Some(Value::String(s)) => vec![json!({"type": "text", "text": s})],
        _ => Vec::new(),
    };
    Line::Entry(Entry {
        id,
        parent: obj.get("parent_id").and_then(Value::as_i64),
        ts: obj.get("timestamp").and_then(Value::as_i64).unwrap_or(0),
        offset,
        boundary: obj.get("compaction_boundary").and_then(Value::as_bool).unwrap_or(false),
        role,
        blocks,
    })
}

/// Indices (file order) of the entries on the path from the last entry back
/// through `parent_id`. Rewound branches are off that path.
fn active_branch(entries: &[Entry]) -> Vec<usize> {
    let index: HashMap<i64, usize> = entries.iter().enumerate().map(|(i, e)| (e.id, i)).collect();
    let mut on = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = entries.len().checked_sub(1);
    while let Some(i) = cur {
        if !seen.insert(i) {
            break;
        }
        on.push(i);
        cur = entries[i].parent.and_then(|p| index.get(&p).copied());
    }
    on.sort_unstable();
    on
}

fn kind(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

fn turns(mut entries: Vec<Entry>, branch: &[usize]) -> Vec<Turn> {
    let mut out = Vec::new();
    let mut cur: Option<Turn> = None;
    let mut boundary: Option<i64> = None;
    for &i in branch {
        let e = &mut entries[i];
        if e.boundary {
            boundary = Some(e.ts);
            continue;
        }
        if let Some(b) = boundary {
            if (e.ts - b).abs() <= COMPACTION_WINDOW_MS {
                continue;
            }
            boundary = None;
        }
        let blocks = std::mem::take(&mut e.blocks);
        match e.role.as_str() {
            "user" => {
                if let Some(prompt) = prompt_of(&blocks) {
                    out.extend(cur.take());
                    cur = Some(Turn { entry_id: e.id, date: e.ts, offset: e.offset, prompt, ..Turn::default() });
                } else if let Some(t) = cur.as_mut() {
                    t.results.extend(blocks.iter().filter(|b| kind(b) == "tool_result").map(|b| ToolResult {
                        content: match b.get("content") {
                            Some(Value::String(s)) => s.clone(),
                            Some(v) => v.to_string(),
                            None => String::new(),
                        },
                        is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                    }));
                }
            }
            "assistant" => {
                let Some(t) = cur.as_mut() else { continue };
                for b in &blocks {
                    match kind(b) {
                        "text" => {
                            if let Some(s) = b.get("text").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                                t.texts.push(s.to_string());
                            }
                        }
                        "tool_use" => t.tools.push(ToolUse {
                            name: b.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                            args: b.get("args").cloned().unwrap_or(Value::Null),
                        }),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out.extend(cur);
    out
}

/// The prompt a user message opens a turn with, or `None` when it is not a
/// real prompt (tool results, image-only, injected text).
fn prompt_of(blocks: &[Value]) -> Option<String> {
    if blocks.iter().any(|b| kind(b) == "tool_result") {
        return None;
    }
    let text = blocks
        .iter()
        .filter(|b| kind(b) == "text")
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    if text.is_empty() || INJECTED_PREFIXES.iter().any(|p| text.starts_with(p)) {
        return None;
    }
    if let Some(rest) = text.strip_prefix("<skill name=\"") {
        return Some(format!("[skill {}]", rest.split('"').next().unwrap_or("")));
    }
    Some(cap(text, PROMPT_CAP).to_string())
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod session_tests;
~~~

- [ ] **Step 5: Run the session tests**

Run: `$X src/session.rs && CARGO_BUILD_JOBS=4 cargo test --lib session`
Expected: 15 passed.

- [ ] **Step 6: Format, lint, commit**

~~~bash
CARGO_BUILD_JOBS=4 cargo fmt && CARGO_BUILD_JOBS=4 cargo clippy --all-targets -- -D warnings && CARGO_BUILD_JOBS=4 cargo test
git add -A && git commit -m "Parse session files into turns on the active branch"
~~~

---

### Task 3: Extract the indexed fields from a turn

**Files:**
- Create: `src/extract.rs`, `src/extract_tests.rs`
- Modify: `src/lib.rs` (add `pub mod extract;`)

**Interfaces:**
- Consumes: `session::{Turn, ToolUse, ToolResult}`, `text::{clip, first_line, head_tail}`.
- Produces:
  - `extract::{REPLY_HEAD, REPLY_TAIL, GIST_CHARS, COMMAND_CHARS, COMMANDS_CAP, MAX_FILES}`
  - `extract::Extracted { reply: String, gist: String, commands: String, files: Vec<String>, commits: Vec<String>, failed: bool, has_reply: bool }`
  - `extract::extract(turn: &Turn, cwd: &Path, root: &Path) -> Extracted`: no redaction here; catch-up redacts every field before writing.
  - `extract::lexical(path: &Path) -> PathBuf`: `.` and `..` resolved without touching the disk (also used by `project.rs`).

Rules (the spec's Extraction section, made exact):
- `reply`: non-blank assistant texts joined by a blank line, then `head_tail(2048, 6144)`. `gist`: `clip(first_line(last text), 300)`. `has_reply`: any text.
- `commands`: one line per tool call. bash: `clip(command, 300)`; bash calls with no `command` (job actions) are skipped. Other tools: `<name> <first of path, file_path, url, query, pattern>`, or just `<name>`. Lines are added while the joined text stays within 4096 bytes.
- `files`: `path` / `file_path` of `read`, `edit`, `write`, plus bash tokens. A command is split on whitespace and ``;|&()<>'"`=,``; a trailing `:N` (repeated) is stripped; a token counts when it uses only `[A-Za-z0-9._/~+@-]`, has a letter or digit, does not start with `-`, either has a known extension or starts with `/`, `./`, `../` or `~/`, and is not under `/dev/` or `/proc/`. The token after `cd` and heredoc bodies are skipped. `~/` paths are kept verbatim; others are joined to cwd, made lexical, and made relative to root when inside it (root itself is dropped). Deduplicated in first-seen order, at most 30.
- `commits`: every tool result line matching `(?m)^\[[^\]\s]+(?: \(root-commit\))? ([0-9a-f]{7,40})\] `, deduplicated.
- `failed`: a result with `is_error`, or whose first non-blank line matches `^exit (\d+)` with a non-zero code.

- [ ] **Step 1: Write the failing tests**

~~~rust file=src/extract_tests.rs
use super::*;
use crate::session::{ToolResult, ToolUse, Turn};
use serde_json::{Value, json};
use std::path::Path;

type Call<'a> = (&'a str, Value, &'a str, bool);

fn turn(texts: &[&str], calls: &[Call]) -> Turn {
    Turn {
        entry_id: 1,
        prompt: "q".into(),
        texts: texts.iter().map(|s| s.to_string()).collect(),
        tools: calls.iter().map(|(n, a, _, _)| ToolUse { name: n.to_string(), args: a.clone() }).collect(),
        results: calls.iter().map(|(_, _, c, e)| ToolResult { content: c.to_string(), is_error: *e }).collect(),
        ..Turn::default()
    }
}

fn bash<'a>(command: &str, result: &'a str) -> Call<'a> {
    ("bash", json!({"command": command}), result, false)
}

fn run(t: &Turn) -> Extracted {
    extract(t, Path::new("/work/app"), Path::new("/work/app"))
}

#[test]
fn reply_joins_texts_and_gist_is_the_first_line_of_the_last_text() {
    let e = run(&turn(&["Looking.", "\n  Fixed: missing import.\nDetails follow."], &[]));
    assert_eq!(e.reply, "Looking.\n\n\n  Fixed: missing import.\nDetails follow.");
    assert_eq!(e.gist, "Fixed: missing import.");
    assert!(e.has_reply);
}

#[test]
fn long_replies_keep_head_and_tail() {
    let long = format!("{}{}", "h".repeat(5000), "t".repeat(5000));
    let e = run(&turn(&[&long], &[]));
    assert!(e.reply.starts_with(&"h".repeat(REPLY_HEAD)), "head");
    assert!(e.reply.ends_with(&"t".repeat(5000)), "tail");
    assert_eq!(e.reply.len(), REPLY_HEAD + " … ".len() + REPLY_TAIL);
}

#[test]
fn a_turn_without_text_has_no_reply() {
    let e = run(&turn(&[], &[bash("ls", "exit 0")]));
    assert_eq!((e.reply.as_str(), e.gist.as_str(), e.has_reply), ("", "", false));
}

#[test]
fn commands_are_one_line_per_tool() {
    let e = run(&turn(
        &[],
        &[
            bash("cargo   build\n  --release", "exit 0"),
            ("read", json!({"path": "src/lib.rs"}), "...", false),
            ("web_fetch", json!({"url": "https://example.com/doc"}), "...", false),
            ("grep", json!({"pattern": "fn main"}), "...", false),
            ("bash", json!({"action": "output", "job_id": "j1"}), "...", false),
            ("discord_send", json!({"content": "hi"}), "ok", false),
        ],
    ));
    assert_eq!(
        e.commands,
        "cargo build --release\nread src/lib.rs\nweb_fetch https://example.com/doc\ngrep fn main\ndiscord_send"
    );
}

#[test]
fn commands_are_clipped_and_capped() {
    let long = "x".repeat(1000);
    let calls: Vec<Call> = (0..40).map(|_| bash(&long, "exit 0")).collect();
    let e = run(&turn(&[], &calls));
    assert!(e.commands.lines().all(|l| l.chars().count() <= COMMAND_CHARS));
    assert!(e.commands.len() <= COMMANDS_CAP, "{}", e.commands.len());
    assert!(e.commands.lines().count() >= 10);
}

#[test]
fn file_tool_paths_are_relative_to_the_project_root() {
    let t = turn(
        &[],
        &[
            ("read", json!({"path": "src/lib.rs"}), "", false),
            ("edit", json!({"file_path": "/work/app/README.md"}), "", false),
            ("write", json!({"path": "/etc/hosts"}), "", false),
            ("read", json!({"path": "../src/lib.rs"}), "", false),
            ("read", json!({"path": "./src/lib.rs"}), "", false),
            ("read", json!({"path": "~/notes/todo.md"}), "", false),
            ("web_fetch", json!({"path": "ignored.rs"}), "", false),
        ],
    );
    let e = extract(&t, Path::new("/work/app/crates"), Path::new("/work/app"));
    assert_eq!(e.files, ["crates/src/lib.rs", "README.md", "/etc/hosts", "src/lib.rs", "~/notes/todo.md"]);
}

#[test]
fn bash_tokens_that_look_like_paths_are_files() {
    let cmd = "cargo test --manifest-path=crates/x/Cargo.toml && grep -n foo src/main.rs:12:3 2>/dev/null; cat ../other/notes.md | head -5";
    let e = run(&turn(&[], &[bash(cmd, "exit 0")]));
    assert_eq!(e.files, ["crates/x/Cargo.toml", "src/main.rs", "/work/other/notes.md"]);
}

#[test]
fn bash_tokens_that_are_not_paths_are_ignored() {
    let cmd = "git push origin feat/x && curl https://example.com/a.js -o $HOME/a.js; echo 1.5 / ./";
    let e = run(&turn(&[], &[bash(cmd, "exit 0")]));
    assert!(e.files.is_empty(), "{:?}", e.files);
}

#[test]
fn cd_targets_and_heredoc_bodies_are_not_files() {
    let cmd = "cd ~/proj && cat > notes.md <<'EOF'\nsee src/hidden.rs\nEOF\nls tests/a.rs";
    let e = run(&turn(&[], &[bash(cmd, "exit 0")]));
    assert_eq!(e.files, ["notes.md", "tests/a.rs"]);
}

#[test]
fn files_are_deduplicated_and_capped() {
    let cmd: String = (0..50).map(|i| format!("f{i}.rs f{i}.rs ")).collect();
    let e = run(&turn(&[], &[bash(&cmd, "exit 0")]));
    assert_eq!(e.files.len(), MAX_FILES);
    assert_eq!(e.files[..2], ["f0.rs", "f1.rs"]);
}

#[test]
fn commits_come_from_git_commit_output() {
    let one = "exit 0 · 0.3s · 3 lines\n<untrusted-output>\n[main 2ba5532] Fix cache warmup\n 1 file changed, 2 insertions(+)\n</untrusted-output>";
    let two = "exit 0\n[feat/batch-x (root-commit) abcdef1234] Initial commit\n[1/3] building\nsee [main 1234567] above\n[main 2ba5532] Fix cache warmup";
    let e = run(&turn(&[], &[bash("git commit -m x", one), bash("git commit -m y", two)]));
    assert_eq!(e.commits, ["2ba5532", "abcdef1234"]);
}

#[test]
fn nonzero_exit_headers_mark_the_turn_failed() {
    for (result, failed) in [
        ("exit 1 · 0.2s · 4 lines", true),
        ("exit 127 · 0.0s", true),
        ("\n  exit 2 · 0.1s", true),
        ("exit 0 · 1.2s · 7 lines", false),
        ("exit 0 (`tail` masks `cd`'s exit; rerun `cd` alone for its status) · 0.0s", false),
        ("the script printed exit 1", false),
    ] {
        assert_eq!(run(&turn(&[], &[bash("x", result)])).failed, failed, "{result}");
    }
}

#[test]
fn error_results_mark_the_turn_failed() {
    let t = turn(&[], &[("read", json!({"path": "x.rs"}), "no such file", true)]);
    assert!(run(&t).failed);
    assert!(!run(&turn(&["ok"], &[])).failed);
}

#[test]
fn lexical_resolves_dots_without_touching_the_disk() {
    for (input, want) in [("/a/b/../c/./d", "/a/c/d"), ("a/../../b", "../b"), ("/..", "/"), ("./x", "x"), ("", "")] {
        assert_eq!(lexical(Path::new(input)), Path::new(want), "{input}");
    }
}
~~~

~~~bash
$X src/extract_tests.rs
sed -i 's/^pub mod redact;/pub mod extract;\npub mod redact;/' src/lib.rs
printf '#[cfg(test)]\n#[path = "extract_tests.rs"]\nmod extract_tests;\n' > src/extract.rs
~~~

- [ ] **Step 2: Run the tests to see them fail**

Run: `CARGO_BUILD_JOBS=4 cargo test --lib extract`
Expected: compile errors (`cannot find function extract`, `cannot find type Extracted`).

- [ ] **Step 3: Implement `extract.rs`**

~~~rust file=src/extract.rs
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
    "rs", "toml", "lock", "md", "txt", "json", "jsonl", "yaml", "yml", "py", "js", "mjs", "cjs", "ts", "tsx",
    "jsx", "vue", "svelte", "html", "css", "scss", "sh", "bash", "zsh", "fish", "go", "c", "h", "cc", "cpp", "hpp",
    "java", "kt", "swift", "rb", "lua", "zig", "nix", "sql", "proto", "xml", "csv", "ini", "cfg", "conf", "env",
    "log", "diff", "patch", "png", "jpg", "jpeg", "webp", "gif", "svg", "pdf", "mp4", "mov", "webm", "wav", "mp3",
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
    let mut files = Files { cwd, root, out: Vec::new() };
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
        gist: turn.texts.last().map(|t| clip(first_line(t), GIST_CHARS)).unwrap_or_default(),
        commands: commands.join("\n"),
        files: files.out,
        commits,
        failed: turn.results.iter().any(|r| r.is_error || exit_code(&r.content).is_some_and(|c| c != 0)),
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
    args.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

fn exit_code(content: &str) -> Option<u64> {
    EXIT.captures(first_line(content)).and_then(|c| c[1].parse().ok())
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
            let rel = if self.root.as_os_str().is_empty() { None } else { abs.strip_prefix(self.root).ok() };
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
        for tok in line.split(|c: char| c.is_whitespace() || SPLIT.contains(&c)).filter(|t| !t.is_empty()) {
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
    tok.chars().all(|c| c.is_ascii_alphanumeric() || "._/~+@-".contains(c))
        && tok.chars().any(|c| c.is_ascii_alphanumeric())
        && !tok.starts_with('-')
        && (known_ext || ANCHORS.iter().any(|a| tok.starts_with(a)))
        && !tok.starts_with("/dev/")
        && !tok.starts_with("/proc/")
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod extract_tests;
~~~

- [ ] **Step 4: Run the extract tests**

Run: `$X src/extract.rs && CARGO_BUILD_JOBS=4 cargo test --lib extract`
Expected: 14 passed.

- [ ] **Step 5: Format, lint, commit**

~~~bash
CARGO_BUILD_JOBS=4 cargo fmt && CARGO_BUILD_JOBS=4 cargo clippy --all-targets -- -D warnings && CARGO_BUILD_JOBS=4 cargo test
git add -A && git commit -m "Extract reply, commands, files, commits and failure from each turn"
~~~

---

### Task 4: Projects and session ids

**Files:**
- Create: `src/project.rs`, `src/project_tests.rs`
- Modify: `src/lib.rs` (add `pub mod project;`)

**Interfaces:**
- Consumes: `extract::lexical`.
- Produces:
  - `project::Project { key: String, name: String }` (`Debug, Clone, PartialEq, Eq`)
  - `project::git_root(dir: &Path) -> Option<PathBuf>`: nearest ancestor with `.git`. A `.git` directory: that ancestor. A `.git` file (`gitdir: <dir>`): when that git dir has `commondir` (a worktree), the main repository (the parent of the common `.git`, or the common dir itself when it is not named `.git`); otherwise (a submodule) the ancestor itself.
  - `project::project_for(cwd: &str) -> Project`: key = git root of the lexical cwd, else the lexical cwd; name = its last component (`/` for the root). An empty or relative cwd is kept as the key with name `unknown`. A deleted cwd still resolves through its existing ancestors (a removed worktree under a repo maps to that repo).
  - `project::Resolver` (`Default`) with `project(&mut self, cwd: &str) -> Project`, cached per cwd string.
  - `project::{is_uuid(&str) -> bool, short_session(&str) -> &str, display_id(session: &str, entry_id: i64) -> String}`: short session is the first 8 chars of a UUID, else the whole id; display id is `<short>:<entry_id>`.

- [ ] **Step 1: Write the failing tests**

~~~rust file=src/project_tests.rs
use super::*;
use std::fs;
use std::path::Path;

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn repo(dir: &Path) {
    fs::create_dir_all(dir.join(".git")).unwrap();
}

#[test]
fn a_subdirectory_maps_to_its_repo_root() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("app");
    repo(&root);
    fs::create_dir_all(root.join("crates/x")).unwrap();
    let p = project_for(&s(&root.join("crates/x")));
    assert_eq!(p, Project { key: s(&root), name: "app".into() });
}

#[test]
fn a_worktree_maps_to_its_main_repo() {
    let t = tempfile::tempdir().unwrap();
    let main = t.path().join("main");
    let gitdir = main.join(".git/worktrees/wt");
    fs::create_dir_all(&gitdir).unwrap();
    fs::write(gitdir.join("commondir"), "../..\n").unwrap();
    for (name, pointer) in [("wt-abs", s(&gitdir)), ("wt-rel", "../main/.git/worktrees/wt".to_string())] {
        let wt = t.path().join(name);
        fs::create_dir_all(wt.join("src")).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {pointer}\n")).unwrap();
        assert_eq!(project_for(&s(&wt.join("src"))).key, s(&main), "{name}");
    }
}

#[test]
fn a_submodule_is_its_own_project() {
    let t = tempfile::tempdir().unwrap();
    let sup = t.path().join("super");
    fs::create_dir_all(sup.join(".git/modules/sub")).unwrap();
    let sub = sup.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join(".git"), "gitdir: ../.git/modules/sub\n").unwrap();
    assert_eq!(project_for(&s(&sub)), Project { key: s(&sub), name: "sub".into() });
}

#[test]
fn outside_a_repo_the_cwd_is_the_project() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("scratch");
    fs::create_dir_all(&dir).unwrap();
    assert_eq!(project_for(&s(&dir)), Project { key: s(&dir), name: "scratch".into() });
}

#[test]
fn a_deleted_cwd_resolves_through_its_ancestors() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("app");
    repo(&root);
    assert_eq!(project_for(&s(&root.join(".worktrees/gone/src"))).key, s(&root));
}

#[test]
fn dots_in_the_cwd_are_resolved() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("app");
    repo(&root);
    fs::create_dir_all(root.join("crates")).unwrap();
    assert_eq!(project_for(&format!("{}/crates/../.", s(&root))).key, s(&root));
}

#[test]
fn empty_or_relative_cwds_are_unknown() {
    for cwd in ["", "rel/path"] {
        assert_eq!(project_for(cwd), Project { key: cwd.into(), name: "unknown".into() });
    }
}

#[test]
fn the_resolver_caches_per_cwd() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("app");
    repo(&root);
    let cwd = s(&root.join("src"));
    let mut r = Resolver::default();
    let first = r.project(&cwd);
    fs::remove_dir_all(root.join(".git")).unwrap();
    assert_eq!(r.project(&cwd), first);
    assert_ne!(project_for(&cwd), first);
}

#[test]
fn uuid_sessions_get_short_ids() {
    let uuid = "1900bae3-5c1e-4c47-9a0e-1234567890ab";
    assert!(is_uuid(uuid));
    assert_eq!(short_session(uuid), "1900bae3");
    assert_eq!(display_id(uuid, 3154), "1900bae3:3154");
    for other in ["kinetic-photon-flux", "1900bae3-5c1e-4c47-9a0e-1234567890aZ", "1900bae3"] {
        assert!(!is_uuid(other), "{other}");
        assert_eq!(short_session(other), other);
    }
}
~~~

~~~bash
$X src/project_tests.rs
sed -i 's/^pub mod redact;/pub mod project;\npub mod redact;/' src/lib.rs
printf '#[cfg(test)]\n#[path = "project_tests.rs"]\nmod project_tests;\n' > src/project.rs
~~~

- [ ] **Step 2: Run the tests to see them fail**

Run: `CARGO_BUILD_JOBS=4 cargo test --lib project`
Expected: compile errors (`cannot find function project_for`, `cannot find type Project`).

- [ ] **Step 3: Implement `project.rs`**

~~~rust file=src/project.rs
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
        return Project { key: cwd.to_string(), name: "unknown".into() };
    }
    let path = lexical(path);
    let root = git_root(&path).unwrap_or(path);
    let key = root.to_string_lossy().into_owned();
    let name = root.file_name().map_or_else(|| key.clone(), |n| n.to_string_lossy().into_owned());
    Project { key, name }
}

/// `project_for` with a per-cwd cache: sessions share a handful of cwds.
#[derive(Default)]
pub struct Resolver {
    cache: HashMap<String, Project>,
}

impl Resolver {
    pub fn project(&mut self, cwd: &str) -> Project {
        self.cache.entry(cwd.to_string()).or_insert_with(|| project_for(cwd)).clone()
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
~~~

- [ ] **Step 4: Run the project tests**

Run: `$X src/project.rs && CARGO_BUILD_JOBS=4 cargo test --lib project`
Expected: 9 passed.

- [ ] **Step 5: Format, lint, commit**

~~~bash
CARGO_BUILD_JOBS=4 cargo fmt && CARGO_BUILD_JOBS=4 cargo clippy --all-targets -- -D warnings && CARGO_BUILD_JOBS=4 cargo test
git add -A && git commit -m "Map session cwds to git projects and short session ids"
~~~

---

### Task 5: The SQLite index

**Files:**
- Create: `src/index.rs`, `src/index_tests.rs`
- Modify: `src/lib.rs` (add `pub mod index;`)

**Interfaces:**
- Produces:
  - `index::{SCHEMA_VERSION, LAST_CATCH_UP, ROW_COLUMNS, ROW_COLUMN_COUNT}`; `ROW_COLUMNS` selects from `turns t`.
  - `index::{recall_dir(home) -> PathBuf, db_path(home) -> PathBuf, open(home) -> Result<Connection>, reset(&Connection) -> Result<()>}`
  - `index::Row { rowid, session, entry_id, project_key, project_name, cwd, date, prompt, reply, gist, commands, files: Vec<String>, commits: Vec<String>, failed: bool, has_reply: bool }` with `Row::read(&rusqlite::Row, at) -> rusqlite::Result<Row>` (reads `ROW_COLUMNS` starting at column `at`).
  - `index::{insert_turn(&Connection, &Row), delete_session_turns(&Connection, &str) -> Result<usize>}`
  - `index::SessionState { session, path, cwd, project_key, size: u64, mtime_ns: i64, head_hash: u64, tail_offset: u64, tail_hash: u64, tail_entry: Option<i64>, skipped: u64 }` with `get_session`, `put_session`, `session_ids`, `delete_session`.
  - `index::{rebuild_projects, list_projects -> Vec<ProjectInfo>}`, `ProjectInfo { key, name, turns: i64 }` ordered by turns descending, then name.
  - `index::{meta_get, meta_set}`, `index::Stats { turns, sessions, projects, skipped, bytes: u64, last_catch_up: Option<i64> }`, `index::stats(&Connection, home)`.

Notes: every function takes `&Connection`, so a `Transaction` works through deref. `u64` values are stored as `i64` bit casts (rusqlite rejects `u64 > i64::MAX`). `open` checks the schema version inside an IMMEDIATE transaction and resets on mismatch, so two processes opening a fresh index cannot wipe each other's work.

- [ ] **Step 1: Write the failing tests**

~~~rust file=src/index_tests.rs
use super::*;
use rusqlite::Connection;
use std::path::Path;

fn row(session: &str, entry_id: i64, project: &str, prompt: &str) -> Row {
    Row {
        session: session.into(),
        entry_id,
        project_key: format!("/w/{project}"),
        project_name: project.into(),
        cwd: format!("/w/{project}"),
        date: 1_790_000_000_000 + entry_id,
        prompt: prompt.into(),
        reply: "a reply".into(),
        gist: "a reply".into(),
        commands: "cargo clippy".into(),
        files: vec!["crates/gray/src/lib.rs".into(), "README.md".into()],
        commits: vec!["2ba5532".into()],
        failed: true,
        has_reply: true,
        ..Row::default()
    }
}

fn hits(c: &Connection, q: &str) -> i64 {
    c.query_row("SELECT count(*) FROM turns_fts WHERE turns_fts MATCH ?1", [q], |r| r.get(0)).unwrap()
}

fn count(c: &Connection, table: &str) -> i64 {
    c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap()
}

fn state(session: &str) -> SessionState {
    SessionState {
        session: session.into(),
        path: format!("/h/sessions/{session}.jsonl"),
        cwd: "/w/app".into(),
        project_key: "/w/app".into(),
        size: 1234,
        mtime_ns: 1_790_000_000_123_456_789,
        head_hash: u64::MAX - 5,
        tail_offset: 900,
        tail_hash: 1 << 63,
        tail_entry: Some(42),
        skipped: 2,
    }
}

#[test]
fn open_creates_a_private_wal_index() {
    let home = tempfile::tempdir().unwrap();
    let c = open(home.path()).unwrap();
    assert_eq!(meta_get(&c, "schema").unwrap().as_deref(), Some(SCHEMA_VERSION));
    let mode: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(mode, "wal");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(m(&recall_dir(home.path())), 0o700);
        assert_eq!(m(&db_path(home.path())), 0o600);
    }
}

#[test]
fn reopening_keeps_the_data() {
    let home = tempfile::tempdir().unwrap();
    let c = open(home.path()).unwrap();
    insert_turn(&c, &row("s1", 1, "app", "hello")).unwrap();
    drop(c);
    assert_eq!(count(&open(home.path()).unwrap(), "turns"), 1);
}

#[test]
fn a_schema_mismatch_resets_the_index() {
    let home = tempfile::tempdir().unwrap();
    let c = open(home.path()).unwrap();
    insert_turn(&c, &row("s1", 1, "app", "hello")).unwrap();
    meta_set(&c, "schema", "0").unwrap();
    drop(c);
    let c = open(home.path()).unwrap();
    assert_eq!(count(&c, "turns"), 0);
    assert_eq!(meta_get(&c, "schema").unwrap().as_deref(), Some(SCHEMA_VERSION));
}

#[test]
fn inserting_the_same_entry_replaces_it() {
    let home = tempfile::tempdir().unwrap();
    let c = open(home.path()).unwrap();
    insert_turn(&c, &row("s1", 1, "app", "oldword prompt")).unwrap();
    insert_turn(&c, &row("s1", 1, "app", "newword prompt")).unwrap();
    assert_eq!(count(&c, "turns"), 1);
    assert_eq!((hits(&c, "oldword"), hits(&c, "newword")), (0, 1));
}

#[test]
fn rows_round_trip() {
    let home = tempfile::tempdir().unwrap();
    let c = open(home.path()).unwrap();
    let want = row("s1", 7, "app", "hello");
    insert_turn(&c, &want).unwrap();
    let got = c.query_row(&format!("SELECT {ROW_COLUMNS} FROM turns t"), [], |r| Row::read(r, 0)).unwrap();
    assert!(got.rowid > 0);
    assert_eq!(got, Row { rowid: got.rowid, ..want.clone() });
    let shifted = c.query_row(&format!("SELECT 42, {ROW_COLUMNS} FROM turns t"), [], |r| Row::read(r, 1)).unwrap();
    assert_eq!(shifted, got);
}

#[test]
fn fts_covers_prompt_files_reply_and_commands_with_stemming() {
    let home = tempfile::tempdir().unwrap();
    let c = open(home.path()).unwrap();
    insert_turn(&c, &row("s1", 1, "app", "compacting the history")).unwrap();
    for q in ["\"lib.rs\"", "clippy", "compaction", "readme", "reply"] {
        assert_eq!(hits(&c, q), 1, "{q}");
    }
    assert_eq!(hits(&c, "2ba5532"), 0, "commits are not full-text indexed");
}

#[test]
fn deleting_a_sessions_turns_removes_their_fts_rows() {
    let home = tempfile::tempdir().unwrap();
    let c = open(home.path()).unwrap();
    insert_turn(&c, &row("a", 1, "app", "alpha one")).unwrap();
    insert_turn(&c, &row("a", 2, "app", "alpha two")).unwrap();
    insert_turn(&c, &row("b", 1, "app", "alpha three")).unwrap();
    assert_eq!(delete_session_turns(&c, "a").unwrap(), 2);
    assert_eq!(hits(&c, "alpha"), 1);
    assert_eq!(count(&c, "turns"), 1);
}

#[test]
fn session_state_round_trips_and_deletes_with_its_turns() {
    let home = tempfile::tempdir().unwrap();
    let c = open(home.path()).unwrap();
    let s = state("s1");
    put_session(&c, &s).unwrap();
    assert_eq!(get_session(&c, "s1").unwrap(), Some(s.clone()));
    put_session(&c, &SessionState { size: 2000, tail_entry: None, ..s.clone() }).unwrap();
    let got = get_session(&c, "s1").unwrap().unwrap();
    assert_eq!((got.size, got.tail_entry), (2000, None));
    assert_eq!(get_session(&c, "nope").unwrap(), None);
    insert_turn(&c, &row("s1", 1, "app", "x")).unwrap();
    assert_eq!(session_ids(&c).unwrap(), ["s1"]);
    delete_session(&c, "s1").unwrap();
    assert_eq!(count(&c, "turns"), 0);
    assert!(session_ids(&c).unwrap().is_empty());
}

#[test]
fn projects_are_rebuilt_from_turns() {
    let home = tempfile::tempdir().unwrap();
    let c = open(home.path()).unwrap();
    for (i, p) in ["App", "App", "tool"].into_iter().enumerate() {
        insert_turn(&c, &row("s1", i as i64, p, "x")).unwrap();
    }
    rebuild_projects(&c).unwrap();
    assert_eq!(
        list_projects(&c).unwrap(),
        [
            ProjectInfo { key: "/w/App".into(), name: "App".into(), turns: 2 },
            ProjectInfo { key: "/w/tool".into(), name: "tool".into(), turns: 1 },
        ]
    );
    let lower: String = c.query_row("SELECT name_lower FROM projects WHERE key = '/w/App'", [], |r| r.get(0)).unwrap();
    assert_eq!(lower, "app");
    delete_session_turns(&c, "s1").unwrap();
    rebuild_projects(&c).unwrap();
    assert!(list_projects(&c).unwrap().is_empty());
}

#[test]
fn stats_count_everything() {
    let home = tempfile::tempdir().unwrap();
    let c = open(home.path()).unwrap();
    insert_turn(&c, &row("s1", 1, "app", "x")).unwrap();
    insert_turn(&c, &row("s1", 2, "app", "y")).unwrap();
    put_session(&c, &SessionState { skipped: 3, ..state("s1") }).unwrap();
    rebuild_projects(&c).unwrap();
    meta_set(&c, LAST_CATCH_UP, "1790000000000").unwrap();
    let st = stats(&c, home.path()).unwrap();
    assert_eq!((st.turns, st.sessions, st.projects, st.skipped), (2, 1, 1, 3));
    assert_eq!(st.last_catch_up, Some(1_790_000_000_000));
    assert!(st.bytes > 0);
}

#[test]
fn reset_empties_everything_and_keeps_the_index_usable() {
    let home = tempfile::tempdir().unwrap();
    let c = open(home.path()).unwrap();
    insert_turn(&c, &row("s1", 1, "app", "alpha")).unwrap();
    put_session(&c, &state("s1")).unwrap();
    meta_set(&c, "x", "y").unwrap();
    reset(&c).unwrap();
    assert_eq!((count(&c, "turns"), count(&c, "sessions"), hits(&c, "alpha")), (0, 0, 0));
    assert_eq!(meta_get(&c, "x").unwrap(), None);
    assert_eq!(meta_get(&c, "schema").unwrap().as_deref(), Some(SCHEMA_VERSION));
    insert_turn(&c, &row("s1", 1, "app", "alpha")).unwrap();
    assert_eq!(hits(&c, "alpha"), 1);
}
~~~

~~~bash
$X src/index_tests.rs
sed -i 's/^pub mod project;/pub mod index;\npub mod project;/' src/lib.rs
printf '#[cfg(test)]\n#[path = "index_tests.rs"]\nmod index_tests;\n' > src/index.rs
~~~

- [ ] **Step 2: Run the tests to see them fail**

Run: `CARGO_BUILD_JOBS=4 cargo test --lib index`
Expected: compile errors (`cannot find function open`, `cannot find type Row`).

- [ ] **Step 3: Implement `index.rs`**

~~~rust file=src/index.rs
//! The SQLite index: schema, open and reset, turn rows, per-session catch-up
//! state, projects, meta and stats.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const SCHEMA_VERSION: &str = "1";
/// Meta key: unix ms of the last completed catch-up.
pub const LAST_CATCH_UP: &str = "last_catch_up";
/// Every `turns` column, in [`Row::read`] order, from the alias `t`.
pub const ROW_COLUMNS: &str = "t.rowid, t.session, t.entry_id, t.project_key, t.project_name, t.cwd, t.date, \
     t.prompt, t.reply, t.gist, t.commands, t.files, t.commits, t.failed, t.has_reply";
pub const ROW_COLUMN_COUNT: usize = 15;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (
    session TEXT PRIMARY KEY, path TEXT NOT NULL, cwd TEXT NOT NULL, project_key TEXT NOT NULL,
    size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, head_hash INTEGER NOT NULL,
    tail_offset INTEGER NOT NULL, tail_hash INTEGER NOT NULL, tail_entry INTEGER,
    skipped INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS projects (
    key TEXT PRIMARY KEY, name TEXT NOT NULL, name_lower TEXT NOT NULL, turns INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS turns (
    rowid INTEGER PRIMARY KEY, session TEXT NOT NULL, entry_id INTEGER NOT NULL,
    project_key TEXT NOT NULL, project_name TEXT NOT NULL, cwd TEXT NOT NULL, date INTEGER NOT NULL,
    prompt TEXT NOT NULL, reply TEXT NOT NULL, gist TEXT NOT NULL, commands TEXT NOT NULL,
    files TEXT NOT NULL, commits TEXT NOT NULL, failed INTEGER NOT NULL, has_reply INTEGER NOT NULL,
    UNIQUE (session, entry_id));
CREATE INDEX IF NOT EXISTS turns_project_date ON turns (project_key, date);
CREATE VIRTUAL TABLE IF NOT EXISTS turns_fts USING fts5 (
    prompt, files, reply, commands,
    content = 'turns', content_rowid = 'rowid', tokenize = 'porter unicode61');
CREATE TRIGGER IF NOT EXISTS turns_ai AFTER INSERT ON turns BEGIN
    INSERT INTO turns_fts (rowid, prompt, files, reply, commands)
    VALUES (new.rowid, new.prompt, new.files, new.reply, new.commands);
END;
CREATE TRIGGER IF NOT EXISTS turns_ad AFTER DELETE ON turns BEGIN
    INSERT INTO turns_fts (turns_fts, rowid, prompt, files, reply, commands)
    VALUES ('delete', old.rowid, old.prompt, old.files, old.reply, old.commands);
END;
";

pub fn recall_dir(home: &Path) -> PathBuf {
    home.join("recall")
}

pub fn db_path(home: &Path) -> PathBuf {
    recall_dir(home).join("index.db")
}

/// Open (creating if needed) the index: directory 0700, file 0600, WAL, a
/// 5 s busy timeout. A missing or different schema version resets it.
pub fn open(home: &Path) -> Result<Connection> {
    let dir = recall_dir(home);
    std::fs::create_dir_all(&dir)?;
    restrict(&dir, 0o700)?;
    let path = db_path(home);
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(&path)?;
    restrict(&path, 0o600)?;
    let mut conn = Connection::open(&path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    if schema_version(&conn)?.as_deref() != Some(SCHEMA_VERSION) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if schema_version(&tx)?.as_deref() != Some(SCHEMA_VERSION) {
            reset(&tx)?;
        }
        tx.commit()?;
    }
    Ok(conn)
}

fn schema_version(conn: &Connection) -> Result<Option<String>> {
    let has_meta: i64 =
        conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'", [], |r| r.get(0))?;
    if has_meta == 0 { Ok(None) } else { meta_get(conn, "schema") }
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict(_: &Path, _: u32) -> Result<()> {
    Ok(())
}

/// Drop everything and recreate the empty schema.
pub fn reset(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS turns_fts; DROP TABLE IF EXISTS turns; DROP TABLE IF EXISTS sessions;
         DROP TABLE IF EXISTS projects; DROP TABLE IF EXISTS meta;",
    )?;
    conn.execute_batch(SCHEMA)?;
    meta_set(conn, "schema", SCHEMA_VERSION)
}

/// One indexed turn, every text field already redacted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    pub rowid: i64,
    pub session: String,
    pub entry_id: i64,
    pub project_key: String,
    pub project_name: String,
    pub cwd: String,
    pub date: i64,
    pub prompt: String,
    pub reply: String,
    pub gist: String,
    pub commands: String,
    pub files: Vec<String>,
    pub commits: Vec<String>,
    pub failed: bool,
    pub has_reply: bool,
}

impl Row {
    /// Read the [`ROW_COLUMNS`] starting at column `at`.
    pub fn read(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Row> {
        let text = |i: usize| r.get::<_, String>(at + i);
        Ok(Row {
            rowid: r.get(at)?,
            session: text(1)?,
            entry_id: r.get(at + 2)?,
            project_key: text(3)?,
            project_name: text(4)?,
            cwd: text(5)?,
            date: r.get(at + 6)?,
            prompt: text(7)?,
            reply: text(8)?,
            gist: text(9)?,
            commands: text(10)?,
            files: text(11)?.split('\n').filter(|s| !s.is_empty()).map(str::to_string).collect(),
            commits: text(12)?.split_whitespace().map(str::to_string).collect(),
            failed: r.get(at + 13)?,
            has_reply: r.get(at + 14)?,
        })
    }
}

/// Insert `row`, replacing any turn with the same session and entry id.
pub fn insert_turn(conn: &Connection, row: &Row) -> Result<()> {
    conn.prepare_cached("DELETE FROM turns WHERE session = ?1 AND entry_id = ?2")?
        .execute(params![row.session, row.entry_id])?;
    conn.prepare_cached(
        "INSERT INTO turns (session, entry_id, project_key, project_name, cwd, date, prompt, reply, gist,
                            commands, files, commits, failed, has_reply)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?
    .execute(params![
        row.session,
        row.entry_id,
        row.project_key,
        row.project_name,
        row.cwd,
        row.date,
        row.prompt,
        row.reply,
        row.gist,
        row.commands,
        row.files.join("\n"),
        row.commits.join(" "),
        row.failed,
        row.has_reply,
    ])?;
    Ok(())
}

pub fn delete_session_turns(conn: &Connection, session: &str) -> Result<usize> {
    Ok(conn.prepare_cached("DELETE FROM turns WHERE session = ?1")?.execute([session])?)
}

/// What catch-up remembers about one session file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionState {
    pub session: String,
    pub path: String,
    pub cwd: String,
    pub project_key: String,
    pub size: u64,
    pub mtime_ns: i64,
    /// FNV-1a of the first 4 KB.
    pub head_hash: u64,
    /// Start of the last indexed turn's prompt line.
    pub tail_offset: u64,
    /// FNV-1a of the 256 bytes before `tail_offset`.
    pub tail_hash: u64,
    /// Entry id of the last indexed turn.
    pub tail_entry: Option<i64>,
    /// Malformed lines seen in the file.
    pub skipped: u64,
}

pub fn get_session(conn: &Connection, session: &str) -> Result<Option<SessionState>> {
    let mut stmt = conn.prepare_cached(
        "SELECT session, path, cwd, project_key, size, mtime_ns, head_hash, tail_offset, tail_hash,
                tail_entry, skipped FROM sessions WHERE session = ?1",
    )?;
    let state = stmt
        .query_row([session], |r| {
            Ok(SessionState {
                session: r.get(0)?,
                path: r.get(1)?,
                cwd: r.get(2)?,
                project_key: r.get(3)?,
                size: r.get::<_, i64>(4)? as u64,
                mtime_ns: r.get(5)?,
                head_hash: r.get::<_, i64>(6)? as u64,
                tail_offset: r.get::<_, i64>(7)? as u64,
                tail_hash: r.get::<_, i64>(8)? as u64,
                tail_entry: r.get(9)?,
                skipped: r.get::<_, i64>(10)? as u64,
            })
        })
        .optional()?;
    Ok(state)
}

pub fn put_session(conn: &Connection, s: &SessionState) -> Result<()> {
    conn.prepare_cached(
        "INSERT OR REPLACE INTO sessions (session, path, cwd, project_key, size, mtime_ns, head_hash,
                                          tail_offset, tail_hash, tail_entry, skipped)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?
    .execute(params![
        s.session,
        s.path,
        s.cwd,
        s.project_key,
        s.size as i64,
        s.mtime_ns,
        s.head_hash as i64,
        s.tail_offset as i64,
        s.tail_hash as i64,
        s.tail_entry,
        s.skipped as i64,
    ])?;
    Ok(())
}

pub fn session_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT session FROM sessions ORDER BY session")?;
    let ids = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Remove a session's turns and its catch-up state.
pub fn delete_session(conn: &Connection, session: &str) -> Result<()> {
    delete_session_turns(conn, session)?;
    conn.prepare_cached("DELETE FROM sessions WHERE session = ?1")?.execute([session])?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub key: String,
    pub name: String,
    pub turns: i64,
}

/// Recount `projects` from `turns`.
pub fn rebuild_projects(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT project_key, max(project_name), count(*) FROM turns GROUP BY project_key")?;
    let rows: Vec<(String, String, i64)> =
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    conn.execute("DELETE FROM projects", [])?;
    let mut insert = conn.prepare("INSERT INTO projects (key, name, name_lower, turns) VALUES (?1, ?2, ?3, ?4)")?;
    for (key, name, turns) in rows {
        insert.execute(params![key, name, name.to_lowercase(), turns])?;
    }
    Ok(())
}

pub fn list_projects(conn: &Connection) -> Result<Vec<ProjectInfo>> {
    let mut stmt = conn.prepare("SELECT key, name, turns FROM projects ORDER BY turns DESC, name")?;
    let projects = stmt
        .query_map([], |r| Ok(ProjectInfo { key: r.get(0)?, name: r.get(1)?, turns: r.get(2)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(projects)
}

pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    let value = conn
        .prepare_cached("SELECT value FROM meta WHERE key = ?1")?
        .query_row([key], |r| r.get(0))
        .optional()?;
    Ok(value)
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.prepare_cached("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)")?.execute([key, value])?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    pub turns: i64,
    pub sessions: i64,
    pub projects: i64,
    pub skipped: i64,
    /// `index.db` plus its WAL, in bytes.
    pub bytes: u64,
    pub last_catch_up: Option<i64>,
}

pub fn stats(conn: &Connection, home: &Path) -> Result<Stats> {
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0));
    let db = db_path(home);
    let bytes = [db.clone(), db.with_extension("db-wal")]
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    Ok(Stats {
        turns: count("SELECT count(*) FROM turns")?,
        sessions: count("SELECT count(*) FROM sessions")?,
        projects: count("SELECT count(*) FROM projects")?,
        skipped: count("SELECT coalesce(sum(skipped), 0) FROM sessions")?,
        bytes,
        last_catch_up: meta_get(conn, LAST_CATCH_UP)?.and_then(|v| v.parse().ok()),
    })
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod index_tests;
~~~

- [ ] **Step 4: Run the index tests**

Run: `$X src/index.rs && CARGO_BUILD_JOBS=4 cargo test --lib index`
Expected: 11 passed.

- [ ] **Step 5: Format, lint, commit**

~~~bash
CARGO_BUILD_JOBS=4 cargo fmt && CARGO_BUILD_JOBS=4 cargo clippy --all-targets -- -D warnings && CARGO_BUILD_JOBS=4 cargo test
git add -A && git commit -m "SQLite index: schema, turn rows, session state, projects, stats"
~~~

---

### Task 6: Catch-up

**Files:**
- Create: `src/catchup.rs`, `src/catchup_tests.rs`
- Modify: `src/lib.rs` (add `pub mod catchup;`)

**Interfaces:**
- Consumes: `session::parse`, `extract::extract`, `project::Resolver`, `redact::redact_secrets`, `text::fnv1a`, everything in `index`.
- Produces:
  - `catchup::{HEAD_BYTES, TAIL_BYTES, sessions_dir(home) -> PathBuf}`
  - `catchup::Report { busy: bool, changed: usize, removed: usize, failed: usize }` (`Debug, Clone, Copy, Default, PartialEq, Eq`)
  - `catchup::Mode { Full, Tail { offset: u64, entry: i64, cwd: String, skipped: u64, old_size: u64 } }`
  - `catchup::writer_lock(home) -> Result<Option<File>>`: `None` when another process holds `recall/index.lock`.
  - `catchup::catch_up(&mut Connection, home) -> Result<Report>`: takes the lock, or returns `Report { busy: true, .. }` without touching the index.
  - `catchup::catch_up_locked(&mut Connection, home) -> Result<Report>`: for callers already holding the lock (`reindex`).
  - `catchup::{stat(path) -> Result<(u64, i64)>, detect(state: Option<&SessionState>, path, size, mtime_ns) -> Result<Option<(Mode, u64)>>}`: `None` when unchanged; otherwise the mode and the new head hash.

Rules (the spec's Catch-up section, made exact):
- Files: regular files directly in `sessions/` named `<id>.jsonl`, `id` non-empty. Everything else (locks, `.open`, `.corrupt-N`, `archive/`) is ignored.
- Unchanged: same size and mtime. Tail: the file grew, it has an indexed turn, the hash of its first `min(4096, old size)` bytes equals the stored head hash, and the hash of the 256 bytes before `tail_offset` is unchanged. Anything else is Full. The head window uses the old size because a file under 4 KB that grows changes its first 4 KB.
- Tail parsing starts at `tail_offset`; the chunk's active-branch root and its first turn must both be `tail_entry`, otherwise the file is reread in Full mode. Tail mode reads from 256 bytes earlier so the new tail hash can be computed from the same buffer.
- Only `size` bytes (from the `stat`) are read, so a write racing the read is picked up next time.
- Skipped lines: Full counts all; Tail adds only offsets at or past the old size (earlier ones were counted before).
- Parsing runs on `min(4, CPUs, jobs)` scoped threads pulling from an atomic job index, largest files first, each with its own `Resolver`; the calling thread writes results as they arrive, one transaction per session (Full deletes the session's turns first). A file that fails to stat, read or write is counted in `failed` and left for the next run.
- Sessions in the index whose file is gone are deleted (turns and state). `projects` is rebuilt when anything changed; `last_catch_up` is set on every run that held the lock.
- Every stored text field goes through `redact_secrets`: prompt, reply, gist, commands and each file.

- [ ] **Step 1: Write the failing tests**

~~~rust file=src/catchup_tests.rs
use super::*;
use crate::index::{self, Row, get_session};
use crate::testkit::SessionBuilder;
use rusqlite::Connection;
use serde_json::json;

fn setup() -> (tempfile::TempDir, Connection) {
    let home = tempfile::tempdir().unwrap();
    let conn = index::open(home.path()).unwrap();
    (home, conn)
}

fn sess(id: &str) -> SessionBuilder {
    SessionBuilder::new(id, "/work/app")
}

fn prompts(c: &Connection) -> Vec<String> {
    let mut stmt = c.prepare("SELECT prompt FROM turns ORDER BY session, entry_id").unwrap();
    stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

fn reply_of(c: &Connection, entry_id: i64) -> String {
    c.query_row("SELECT reply FROM turns WHERE entry_id = ?1", [entry_id], |r| r.get(0)).unwrap()
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[test]
fn the_first_run_indexes_every_session_file_and_nothing_else() {
    let (h, mut c) = setup();
    let dir = sessions_dir(h.path());
    for i in 0..12 {
        let mut b = sess(&format!("s{i:02}"));
        b.user(&format!("question {i}"));
        b.assistant("answer");
        b.write(&dir);
    }
    for junk in ["s00.lock", "s00.open", "s01.corrupt-1", "notes.txt"] {
        std::fs::write(dir.join(junk), "{}\n").unwrap();
    }
    let mut archived = sess("old");
    archived.user("archived question");
    archived.write(&dir.join("archive"));
    let r = catch_up(&mut c, h.path()).unwrap();
    assert_eq!(r, Report { changed: 12, ..Report::default() });
    let all = prompts(&c);
    assert_eq!(all.len(), 12);
    assert!(!all.iter().any(|p| p.contains("archived")));
    assert_eq!(index::list_projects(&c).unwrap()[0].name, "app");
    assert!(index::meta_get(&c, index::LAST_CATCH_UP).unwrap().is_some());
}

#[test]
fn unchanged_files_are_skipped() {
    let (h, mut c) = setup();
    let mut b = sess("s1");
    b.user("q");
    b.write(&sessions_dir(h.path()));
    catch_up(&mut c, h.path()).unwrap();
    assert_eq!(catch_up(&mut c, h.path()).unwrap(), Report::default());
}

#[test]
fn a_missing_sessions_directory_is_empty() {
    let (h, mut c) = setup();
    assert_eq!(catch_up(&mut c, h.path()).unwrap(), Report::default());
}

#[test]
fn a_grown_file_replaces_its_last_turn_and_adds_new_ones() {
    let (h, mut c) = setup();
    let dir = sessions_dir(h.path());
    let mut b = sess("s1");
    b.user("first question");
    b.assistant("first answer");
    let q2 = b.user("second question");
    b.bash("cargo build", "exit 0");
    let path = b.write(&dir);
    catch_up(&mut c, h.path()).unwrap();
    assert_eq!(get_session(&c, "s1").unwrap().unwrap().tail_entry, Some(q2));
    b.assistant("final answer for the second question");
    b.user("third question");
    b.assistant("third answer");
    b.write(&dir);
    let st = get_session(&c, "s1").unwrap().unwrap();
    let (size, mtime) = stat(&path).unwrap();
    let (mode, _) = detect(Some(&st), &path, size, mtime).unwrap().unwrap();
    assert!(matches!(mode, Mode::Tail { entry, .. } if entry == q2), "{mode:?}");
    assert_eq!(catch_up(&mut c, h.path()).unwrap().changed, 1);
    assert_eq!(prompts(&c), ["first question", "second question", "third question"]);
    assert_eq!(reply_of(&c, q2), "final answer for the second question");
}

#[test]
fn a_shrunk_or_rewritten_file_is_reread_from_scratch() {
    let (h, mut c) = setup();
    let dir = sessions_dir(h.path());
    let mut b = sess("s1");
    b.user("old question one");
    b.assistant("a");
    b.user("old question two");
    b.assistant("b");
    let path = b.write(&dir);
    catch_up(&mut c, h.path()).unwrap();
    let mut shorter = sess("s1");
    shorter.user("new question");
    shorter.assistant("c");
    shorter.write(&dir);
    catch_up(&mut c, h.path()).unwrap();
    assert_eq!(prompts(&c), ["new question"]);
    let mut other = SessionBuilder::new("s1", "/work/other");
    other.user("different start");
    other.assistant(&"x".repeat(2000));
    other.user("more");
    other.write(&dir);
    let st = get_session(&c, "s1").unwrap().unwrap();
    let (size, mtime) = stat(&path).unwrap();
    assert_eq!(detect(Some(&st), &path, size, mtime).unwrap().unwrap().0, Mode::Full);
    catch_up(&mut c, h.path()).unwrap();
    assert_eq!(prompts(&c), ["different start", "more"]);
    let key: String = c.query_row("SELECT DISTINCT project_key FROM turns", [], |r| r.get(0)).unwrap();
    assert_eq!(key, "/work/other");
}

#[test]
fn a_rewind_past_the_last_indexed_turn_falls_back_to_a_full_read() {
    let (h, mut c) = setup();
    let dir = sessions_dir(h.path());
    let mut b = sess("s1");
    b.user("first");
    let ok = b.assistant("ok");
    b.user("abandoned");
    b.assistant("x");
    b.write(&dir);
    catch_up(&mut c, h.path()).unwrap();
    b.branch_from(ok);
    b.user("retry");
    b.assistant("y");
    b.write(&dir);
    catch_up(&mut c, h.path()).unwrap();
    assert_eq!(prompts(&c), ["first", "retry"]);
}

#[test]
fn deleted_files_lose_their_turns_and_state() {
    let (h, mut c) = setup();
    let mut b = sess("s1");
    b.user("q");
    let path = b.write(&sessions_dir(h.path()));
    catch_up(&mut c, h.path()).unwrap();
    std::fs::remove_file(path).unwrap();
    let r = catch_up(&mut c, h.path()).unwrap();
    assert_eq!(r, Report { removed: 1, ..Report::default() });
    assert!(prompts(&c).is_empty());
    assert!(index::session_ids(&c).unwrap().is_empty());
    assert!(index::list_projects(&c).unwrap().is_empty());
}

#[test]
fn a_lock_held_elsewhere_reports_busy_and_writes_nothing() {
    let (h, mut c) = setup();
    let mut b = sess("s1");
    b.user("q");
    b.write(&sessions_dir(h.path()));
    let held = writer_lock(h.path()).unwrap().expect("lock is free");
    assert_eq!(catch_up(&mut c, h.path()).unwrap(), Report { busy: true, ..Report::default() });
    assert!(prompts(&c).is_empty());
    drop(held);
    assert_eq!(catch_up(&mut c, h.path()).unwrap().changed, 1);
}

#[test]
fn malformed_lines_are_counted_once_across_growth() {
    let (h, mut c) = setup();
    let dir = sessions_dir(h.path());
    let mut b = sess("s1");
    b.user("q1");
    b.raw("not json");
    b.assistant("a1");
    b.write(&dir);
    catch_up(&mut c, h.path()).unwrap();
    assert_eq!(get_session(&c, "s1").unwrap().unwrap().skipped, 1);
    b.raw("[1, 2]");
    b.user("q2");
    b.assistant("a2");
    b.write(&dir);
    catch_up(&mut c, h.path()).unwrap();
    assert_eq!(get_session(&c, "s1").unwrap().unwrap().skipped, 2);
    assert_eq!(prompts(&c), ["q1", "q2"]);
}

#[test]
fn a_half_written_last_line_is_indexed_once_complete() {
    let (h, mut c) = setup();
    let dir = sessions_dir(h.path());
    let mut b = sess("s1");
    let q = b.user("q");
    b.assistant("the full answer");
    let text = b.text();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s1.jsonl"), &text[..text.len() - 10]).unwrap();
    catch_up(&mut c, h.path()).unwrap();
    assert_eq!(reply_of(&c, q), "");
    b.write(&dir);
    catch_up(&mut c, h.path()).unwrap();
    assert_eq!(reply_of(&c, q), "the full answer");
    assert_eq!(prompts(&c), ["q"]);
}

#[test]
fn extracted_fields_are_stored() {
    let (h, mut c) = setup();
    let mut b = sess("s1");
    b.user("fix it");
    b.tool("edit", json!({"path": "src/lib.rs"}), "ok", false);
    b.bash("git commit -am fix", "exit 1 · 0.1s\n[main 2ba5532] fix");
    b.assistant("Done.");
    b.write(&sessions_dir(h.path()));
    catch_up(&mut c, h.path()).unwrap();
    let row = c
        .query_row(&format!("SELECT {} FROM turns t", index::ROW_COLUMNS), [], |r| Row::read(r, 0))
        .unwrap();
    assert_eq!((row.project_name.as_str(), row.cwd.as_str()), ("app", "/work/app"));
    assert_eq!(row.files, ["src/lib.rs"]);
    assert_eq!(row.commits, ["2ba5532"]);
    assert_eq!(row.commands, "edit src/lib.rs\ngit commit -am fix");
    assert_eq!((row.gist.as_str(), row.failed, row.has_reply), ("Done.", true, true));
}

#[test]
fn planted_secrets_never_reach_the_index_files() {
    let (h, mut c) = setup();
    let sk = format!("sk-{}", "Zq3xK9".repeat(6));
    let ghp = format!("ghp_{}", "Zq3xK9".repeat(6));
    let first = "Tk4Lm".repeat(5);
    let discord = format!("M{}.{}.{}", &first[..23], "Gh7k2a", "Q9w".repeat(10));
    let mut b = sess("s1");
    b.user(&format!("rotate {sk} please"));
    b.bash(&format!("gh auth login --with-token {ghp}"), "exit 0");
    b.assistant(&format!("The bot token was {discord}; rotated."));
    b.write(&sessions_dir(h.path()));
    catch_up(&mut c, h.path()).unwrap();
    for name in ["index.db", "index.db-wal"] {
        let bytes = std::fs::read(index::recall_dir(h.path()).join(name)).unwrap_or_default();
        for secret in [&sk, &ghp, &discord] {
            assert!(!contains(&bytes, secret), "{name} holds a planted secret");
        }
    }
    let (prompt, commands, reply): (String, String, String) =
        c.query_row("SELECT prompt, commands, reply FROM turns", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    for field in [&prompt, &commands, &reply] {
        assert!(field.contains(crate::redact::REDACTED), "{field}");
    }
}
~~~

~~~bash
$X src/catchup_tests.rs
sed -i 's/^pub mod extract;/pub mod catchup;\npub mod extract;/' src/lib.rs
printf '#[cfg(test)]\n#[path = "catchup_tests.rs"]\nmod catchup_tests;\n' > src/catchup.rs
~~~

- [ ] **Step 2: Run the tests to see them fail**

Run: `CARGO_BUILD_JOBS=4 cargo test --lib catchup`
Expected: compile errors (`cannot find function catch_up`, `cannot find type Report`).

- [ ] **Step 3: Implement `catchup.rs`**

~~~rust file=src/catchup.rs
//! Bring the index up to date with `$GRAY_HOME/sessions`: only changed
//! files, and only their new bytes when a file just grew.

use crate::extract::extract;
use crate::index::{self, Row, SessionState};
use crate::project::Resolver;
use crate::redact::redact_secrets;
use crate::session;
use crate::text::fnv1a;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashSet;
use std::fs::{File, TryLockError};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Window hashed at the start of a file to notice rewrites.
pub const HEAD_BYTES: u64 = 4096;
/// Window hashed just before `tail_offset` to notice edits near the tail.
pub const TAIL_BYTES: u64 = 256;
const MAX_THREADS: usize = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Another process holds the writer lock; nothing was written.
    pub busy: bool,
    /// Sessions (re)indexed.
    pub changed: usize,
    /// Sessions whose file is gone.
    pub removed: usize,
    /// Files that could not be read or written; retried next run.
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Reread the whole file and replace all of the session's turns.
    Full,
    /// Parse from `offset` (the last indexed turn's prompt line).
    Tail { offset: u64, entry: i64, cwd: String, skipped: u64, old_size: u64 },
}

pub fn sessions_dir(home: &Path) -> PathBuf {
    home.join("sessions")
}

/// The writer lock, or `None` when another process holds it.
pub fn writer_lock(home: &Path) -> Result<Option<File>> {
    let dir = index::recall_dir(home);
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("index.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

pub fn catch_up(conn: &mut Connection, home: &Path) -> Result<Report> {
    match writer_lock(home)? {
        Some(_lock) => catch_up_locked(conn, home),
        None => Ok(Report { busy: true, ..Report::default() }),
    }
}

pub fn catch_up_locked(conn: &mut Connection, home: &Path) -> Result<Report> {
    let mut report = Report::default();
    let files = list_sessions(&sessions_dir(home))?;
    let mut jobs = Vec::new();
    for (id, path) in &files {
        let state = index::get_session(conn, id)?;
        match stat(path).and_then(|(size, mtime_ns)| Ok((size, mtime_ns, detect(state.as_ref(), path, size, mtime_ns)?))) {
            Ok((size, mtime_ns, Some((mode, head_hash)))) => {
                jobs.push(Work { id: id.clone(), path: path.clone(), size, mtime_ns, head_hash, mode })
            }
            Ok((_, _, None)) => {}
            Err(_) => report.failed += 1,
        }
    }
    let present: HashSet<&str> = files.iter().map(|(id, _)| id.as_str()).collect();
    let gone: Vec<String> =
        index::session_ids(conn)?.into_iter().filter(|id| !present.contains(id.as_str())).collect();
    if !gone.is_empty() {
        let tx = conn.transaction()?;
        for id in &gone {
            index::delete_session(&tx, id)?;
        }
        tx.commit()?;
        report.removed = gone.len();
    }
    jobs.sort_by_key(|w| std::cmp::Reverse(w.size));
    parse_parallel(&jobs, |result| {
        match result.and_then(|fresh| write(conn, fresh)) {
            Ok(()) => report.changed += 1,
            Err(_) => report.failed += 1,
        }
        Ok(())
    })?;
    if report.changed + report.removed > 0 {
        index::rebuild_projects(conn)?;
    }
    index::meta_set(conn, index::LAST_CATCH_UP, &now_ms().to_string())?;
    Ok(report)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn list_sessions(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let id = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".jsonl"));
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            out.push((id.to_string(), path.clone()));
        }
    }
    out.sort();
    Ok(out)
}

/// Size and mtime (unix ns) of `path`.
pub fn stat(path: &Path) -> Result<(u64, i64)> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta.modified()?.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i64);
    Ok((meta.len(), mtime))
}

/// How to bring `path` up to date given its stored state, or `None` when it
/// is unchanged. Also returns the hash of the file's current head window.
pub fn detect(state: Option<&SessionState>, path: &Path, size: u64, mtime_ns: i64) -> Result<Option<(Mode, u64)>> {
    if let Some(s) = state
        && s.size == size
        && s.mtime_ns == mtime_ns
    {
        return Ok(None);
    }
    let mut file = File::open(path)?;
    let head = read_at(&mut file, 0, size.min(HEAD_BYTES))?;
    let head_hash = fnv1a(&head);
    let mut mode = Mode::Full;
    if let Some(s) = state
        && let Some(entry) = s.tail_entry
        && size > s.size
        && head.get(..s.size.min(HEAD_BYTES) as usize).map(fnv1a) == Some(s.head_hash)
        && window_hash(&mut file, s.tail_offset)? == s.tail_hash
    {
        mode = Mode::Tail { offset: s.tail_offset, entry, cwd: s.cwd.clone(), skipped: s.skipped, old_size: s.size };
    }
    Ok(Some((mode, head_hash)))
}

fn read_at(file: &mut File, from: u64, len: u64) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::with_capacity(len as usize);
    file.take(len).read_to_end(&mut buf)?;
    Ok(buf)
}

fn window_hash(file: &mut File, end: u64) -> Result<u64> {
    let start = end.saturating_sub(TAIL_BYTES);
    Ok(fnv1a(&read_at(file, start, end - start)?))
}

#[derive(Debug, Clone)]
struct Work {
    id: String,
    path: PathBuf,
    size: u64,
    mtime_ns: i64,
    head_hash: u64,
    mode: Mode,
}

/// A parsed session, ready to write.
struct Fresh {
    full: bool,
    state: SessionState,
    rows: Vec<Row>,
}

fn parse_parallel(jobs: &[Work], mut sink: impl FnMut(Result<Fresh>) -> Result<()>) -> Result<()> {
    if jobs.is_empty() {
        return Ok(());
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_THREADS).min(jobs.len());
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        for _ in 0..threads {
            let tx = tx.clone();
            let next = &next;
            s.spawn(move || {
                let mut resolver = Resolver::default();
                while let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    if tx.send(process(job, &mut resolver)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        rx.into_iter().try_for_each(&mut sink)
    })
}

fn process(work: &Work, resolver: &mut Resolver) -> Result<Fresh> {
    let start = match &work.mode {
        Mode::Full => 0,
        Mode::Tail { offset, .. } => *offset,
    };
    let from = start.saturating_sub(TAIL_BYTES);
    let buf = read_at(&mut File::open(&work.path)?, from, work.size.saturating_sub(from))?;
    let parsed = session::parse(buf.get((start - from) as usize..).unwrap_or_default(), start);
    let (cwd, skipped) = match &work.mode {
        Mode::Full => (parsed.cwd.clone().unwrap_or_default(), parsed.skipped_offsets.len() as u64),
        Mode::Tail { entry, cwd, skipped, old_size, .. } => {
            if parsed.root != Some(*entry) || parsed.turns.first().map(|t| t.entry_id) != Some(*entry) {
                return process(&Work { mode: Mode::Full, ..work.clone() }, resolver);
            }
            let new = parsed.skipped_offsets.iter().filter(|&&o| o >= *old_size).count() as u64;
            (cwd.clone(), skipped + new)
        }
    };
    let project = resolver.project(&cwd);
    let rows = parsed
        .turns
        .iter()
        .map(|t| {
            let e = extract(t, Path::new(&cwd), Path::new(&project.key));
            Row {
                rowid: 0,
                session: work.id.clone(),
                entry_id: t.entry_id,
                project_key: project.key.clone(),
                project_name: project.name.clone(),
                cwd: cwd.clone(),
                date: t.date,
                prompt: redact_secrets(&t.prompt),
                reply: redact_secrets(&e.reply),
                gist: redact_secrets(&e.gist),
                commands: redact_secrets(&e.commands),
                files: e.files.iter().map(|f| redact_secrets(f)).collect(),
                commits: e.commits,
                failed: e.failed,
                has_reply: e.has_reply,
            }
        })
        .collect();
    let last = parsed.turns.last();
    let tail_offset = last.map_or(0, |t| t.offset);
    let window = (tail_offset.saturating_sub(TAIL_BYTES) - from) as usize..(tail_offset - from) as usize;
    Ok(Fresh {
        full: work.mode == Mode::Full,
        state: SessionState {
            session: work.id.clone(),
            path: work.path.to_string_lossy().into_owned(),
            cwd,
            project_key: project.key,
            size: work.size,
            mtime_ns: work.mtime_ns,
            head_hash: work.head_hash,
            tail_offset,
            tail_hash: fnv1a(buf.get(window).unwrap_or_default()),
            tail_entry: last.map(|t| t.entry_id),
            skipped,
        },
        rows,
    })
}

fn write(conn: &mut Connection, fresh: Fresh) -> Result<()> {
    let tx = conn.transaction()?;
    if fresh.full {
        index::delete_session_turns(&tx, &fresh.state.session)?;
    }
    for row in &fresh.rows {
        index::insert_turn(&tx, row)?;
    }
    index::put_session(&tx, &fresh.state)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
#[path = "catchup_tests.rs"]
mod catchup_tests;
~~~

- [ ] **Step 4: Run the catch-up tests**

Run: `$X src/catchup.rs && CARGO_BUILD_JOBS=4 cargo test --lib catchup`
Expected: 12 passed.

- [ ] **Step 5: Format, lint, commit**

~~~bash
CARGO_BUILD_JOBS=4 cargo fmt && CARGO_BUILD_JOBS=4 cargo clippy --all-targets -- -D warnings && CARGO_BUILD_JOBS=4 cargo test
git add -A && git commit -m "Catch the index up with session files incrementally and in parallel"
~~~

---

### Task 7: Query: search and show

**Files:**
- Create: `src/query.rs`, `src/query_tests.rs`
- Modify: `src/lib.rs` (add `pub mod query;`)

**Interfaces:**
- Consumes: `index::{list_projects, ProjectInfo, Row, ROW_COLUMNS, ROW_COLUMN_COUNT}`, `project::project_for`.
- Produces:
  - Constants `DEFAULT_LIMIT` 5, `MAX_LIMIT` 20, `SEARCH_CHARS` 2500, `SHOW_CHARS` 8000, `MIN_CHARS` 500, `MAX_CHARS` 50000, `MAX_AROUND` 10, `CANDIDATES` 200, `FUZZY_MIN` 0.85, `MARK_START` / `MARK_END` (`\u{2}` / `\u{3}`).
  - `query::Request { query, project: Option<String>, all, file: Option<String>, since: Option<String>, until: Option<String>, failed, limit: Option<usize>, offset, id: Option<String>, around, json, max_chars: Option<usize> }` (`Default`) with clamping getters `limit()`, `max_chars()` (default 8000 when `id` is set), `around()`.
  - `query::Context { cwd: String, session: Option<String>, now_ms: i64 }` and `query::now_ms() -> i64`.
  - `query::RecallError { code: i32, message: String }` with `error` (1), `bad` (2), `unknown` (3); `Display`, `Error`, `From<rusqlite::Error>` (FTS syntax errors become 2), `From<anyhow::Error>`.
  - `query::match_expr(&str) -> Result<Option<String>, RecallError>`, `query::parse_date(&str, now_ms, end: bool) -> Result<i64, RecallError>`, `query::resolve_project(&[ProjectInfo], &str) -> Result<ProjectInfo, RecallError>`, `query::score(bm25, date, now_ms, has_reply) -> f64`.
  - `query::Hit { row: Row, did: String, score: f64, other_project: bool }`, `query::Search { scope: String, scope_turns: i64, all: bool, hits: Vec<Hit>, total: usize, fell_back: bool }`, `query::search(&Connection, &Request, &Context) -> Result<Search, RecallError>`.
  - `query::Shown { session: String, target: i64, turns: Vec<Row> }`, `query::show(&Connection, id: &str, around: usize) -> Result<Shown, RecallError>`.

Rules (the spec's Query section, made exact):
- Terms: `"..."` is a phrase (an unclosed quote runs to the end), `-x` an exclusion, anything else a word. Terms without a letter or digit are dropped. Every term is quoted for FTS5 (`"` doubled), so FTS operators and punctuation in user input are plain text.
- Expression: words alone `w1 OR w2`; phrases alone `p1 AND p2`; both `p1 AND p2 AND (p1 OR w1 OR w2)` (words stay optional but add rank); exclusions `(core) NOT x1 NOT x2`. Exclusions alone are a bad request; an empty query lists the newest turns in scope.
- Dates are local time. `since` is the period's first millisecond, `until` its last. `Nd` / `Nw` / `Nm` mean N days / 7N days / 30N days before now, for either bound.
- Project tiers, first non-empty tier wins: key (absolute path), exact name, case-insensitive name, case-insensitive substring, Jaro-Winkler >= 0.85 on lowercase names. Several candidates in the winning tier: code 3 listing `name (key)`. None: code 3 with the 3 closest names.
- With a query: the top 200 FTS matches in scope by `bm25(turns_fts, 3.0, 2.0, 1.0, 0.5)`, rescored with `score`, sorted by score then newest, then `offset` / `limit`. `total` is the full match count. Without a query: newest first, paginated in SQL.
- `did`: the reply `snippet()` with markers removed when it contains a match, else the gist.
- Fallback: only for the default scope (no `project`, no `all`) and only when `total` is 0: rerun on every other project and mark hits `other_project`.
- The calling session is always excluded from search (not from show).
- `show`: `<session or prefix>:<entry_id>`. An exact session id wins over prefixes; several prefix matches: code 3 listing full ids; none, or no turn with that entry id: code 3; malformed: code 2. Neighbors are the `around` turns before and after by entry id.

- [ ] **Step 1: Write the failing tests**

~~~rust file=src/query_tests.rs
use super::*;
use crate::catchup::{catch_up, sessions_dir};
use crate::index::ProjectInfo;
use crate::testkit::{SessionBuilder, T0};
use chrono::{Local, TimeZone};
use rusqlite::Connection;
use serde_json::json;

const DAY: i64 = 86_400_000;

struct Fixture {
    _home: tempfile::TempDir,
    conn: Connection,
}

fn fixture(sessions: &[&SessionBuilder]) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    for s in sessions {
        s.write(&sessions_dir(home.path()));
    }
    let mut conn = crate::index::open(home.path()).unwrap();
    catch_up(&mut conn, home.path()).unwrap();
    Fixture { _home: home, conn }
}

/// A session in `/work/<project>`: one turn per prompt, each answered briefly.
fn session(id: &str, project: &str, prompts: &[&str]) -> SessionBuilder {
    let mut b = SessionBuilder::new(id, &format!("/work/{project}"));
    for p in prompts {
        b.user(p);
        b.assistant(&format!("Answer about {p}."));
    }
    b
}

fn ctx(project: &str) -> Context {
    Context { cwd: format!("/work/{project}"), session: None, now_ms: T0 + 10 * DAY }
}

fn req(q: &str) -> Request {
    Request { query: q.into(), ..Request::default() }
}

fn found(s: &Search) -> Vec<String> {
    s.hits.iter().map(|h| h.row.prompt.clone()).collect()
}

fn info(name: &str) -> ProjectInfo {
    ProjectInfo { key: format!("/w/{name}"), name: name.into(), turns: 1 }
}

#[test]
fn match_expressions() {
    let cases = [
        ("cache warm", Some(r#""cache" OR "warm""#)),
        (r#""prefix cache" warm"#, Some(r#""prefix cache" AND ("prefix cache" OR "warm")"#)),
        (r#""a b" "c""#, Some(r#""a b" AND "c""#)),
        ("cache -cookie -token", Some(r#"("cache") NOT "cookie" NOT "token""#)),
        (r#""open phrase"#, Some(r#""open phrase""#)),
        (r#"say"hi"#, Some(r#""say""hi""#)),
        ("lib.rs", Some(r#""lib.rs""#)),
        ("", None),
        ("   ", None),
        ("· --", None),
    ];
    for (q, want) in cases {
        assert_eq!(match_expr(q).unwrap().as_deref(), want, "{q}");
    }
    assert_eq!(match_expr("-cookie").unwrap_err().code, 2);
}

#[test]
fn any_word_matches_and_more_words_rank_higher() {
    let f = fixture(&[&session("s1", "gray", &["cache only", "nothing relevant", "cache warm compaction"])]);
    let s = search(&f.conn, &req("cache warm compaction"), &ctx("gray")).unwrap();
    assert_eq!(found(&s), ["cache warm compaction", "cache only"]);
    assert_eq!((s.total, s.scope.as_str(), s.scope_turns), (2, "project gray", 3));
}

#[test]
fn phrases_are_required() {
    let f = fixture(&[&session("s1", "gray", &["prefix cache warmup", "cache the prefix"])]);
    for q in [r#""prefix cache""#, r#""prefix cache" warmup"#] {
        assert_eq!(found(&search(&f.conn, &req(q), &ctx("gray")).unwrap()), ["prefix cache warmup"], "{q}");
    }
}

#[test]
fn exclusions_remove_turns() {
    let f = fixture(&[&session("s1", "gray", &["sso login cookie", "sso login token"])]);
    assert_eq!(found(&search(&f.conn, &req("sso -cookie"), &ctx("gray")).unwrap()), ["sso login token"]);
}

#[test]
fn the_default_scope_falls_back_to_other_projects_with_a_label() {
    let f = fixture(&[
        &session("s1", "gray", &["deploy the site"]),
        &session("s2", "graysite", &["deploy graysite pages"]),
    ]);
    let s = search(&f.conn, &req("deploy"), &ctx("gray")).unwrap();
    assert_eq!(found(&s), ["deploy the site"]);
    assert!(!s.fell_back);
    let s = search(&f.conn, &req("pages"), &ctx("gray")).unwrap();
    assert!(s.fell_back);
    assert_eq!(found(&s), ["deploy graysite pages"]);
    assert!(s.hits[0].other_project);
    assert_eq!(s.hits[0].row.project_name, "graysite");
}

#[test]
fn an_explicit_project_is_not_widened_and_all_searches_everything() {
    let f = fixture(&[
        &session("s1", "gray", &["deploy the site"]),
        &session("s2", "graysite", &["deploy graysite pages"]),
    ]);
    let s = search(&f.conn, &Request { project: Some("gray".into()), ..req("pages") }, &ctx("graysite")).unwrap();
    assert!(s.hits.is_empty() && !s.fell_back && s.total == 0);
    assert_eq!(s.scope, "project gray");
    let s = search(&f.conn, &Request { all: true, ..req("deploy") }, &ctx("gray")).unwrap();
    assert_eq!((s.total, s.scope.as_str(), s.all, s.scope_turns), (2, "all projects", true, 2));
    assert!(s.hits.iter().all(|h| !h.other_project));
    let e = search(&f.conn, &Request { project: Some("nope".into()), ..req("x") }, &ctx("gray")).unwrap_err();
    assert_eq!(e.code, 3);
}

#[test]
fn project_names_resolve_in_tiers() {
    let ps = [info("gray"), info("graysite"), info("Vibe_Coding"), info("alignment")];
    for (name, want) in
        [("gray", "gray"), ("/w/graysite", "graysite"), ("vibe_coding", "Vibe_Coding"), ("site", "graysite"), ("alignmnet", "alignment")]
    {
        assert_eq!(resolve_project(&ps, name).unwrap().name, want, "{name}");
    }
    let e = resolve_project(&ps, "gra").unwrap_err();
    assert_eq!(e.code, 3);
    assert!(e.message.contains("ambiguous") && e.message.contains("gray (/w/gray)") && e.message.contains("graysite"), "{}", e.message);
    let twins = [ProjectInfo { key: "/a/app".into(), ..info("app") }, ProjectInfo { key: "/b/app".into(), ..info("app") }];
    let e = resolve_project(&twins, "app").unwrap_err();
    assert!(e.message.contains("/a/app") && e.message.contains("/b/app"), "{}", e.message);
    assert_eq!(resolve_project(&twins, "/b/app").unwrap().key, "/b/app");
    let e = resolve_project(&ps, "zzzz").unwrap_err();
    assert!(e.code == 3 && e.message.contains("closest"), "{}", e.message);
    assert_eq!(resolve_project(&ps, " ").unwrap_err().code, 2);
}

#[test]
fn the_calling_session_is_excluded() {
    let f = fixture(&[&session("s1", "gray", &["flaky test"]), &session("s2", "gray", &["flaky test again"])]);
    let s = search(&f.conn, &req("flaky"), &Context { session: Some("s2".into()), ..ctx("gray") }).unwrap();
    assert_eq!((found(&s), s.total), (vec!["flaky test".to_string()], 1));
}

#[test]
fn since_and_until_filter_by_date() {
    let mut b = SessionBuilder::new("s1", "/work/gray");
    b.user("september work");
    b.assistant("done");
    b.clock(T0 + 20 * DAY);
    b.user("october work");
    b.assistant("done");
    let f = fixture(&[&b]);
    let c = Context { now_ms: T0 + 30 * DAY, ..ctx("gray") };
    let q = |since: Option<&str>, until: Option<&str>| {
        let r = Request { since: since.map(Into::into), until: until.map(Into::into), ..req("work") };
        found(&search(&f.conn, &r, &c).unwrap())
    };
    assert_eq!(q(Some("2026-10"), None), ["october work"]);
    assert_eq!(q(None, Some("2026-09")), ["september work"]);
    assert_eq!(q(Some("15d"), None), ["october work"]);
    assert_eq!(q(Some("2026"), Some("2026")).len(), 2);
    let e = search(&f.conn, &Request { since: Some("last week".into()), ..req("work") }, &c).unwrap_err();
    assert_eq!(e.code, 2);
}

#[test]
fn dates_parse_to_local_period_bounds() {
    let local = |y: i32, m: u32, d: u32| Local.with_ymd_and_hms(y, m, d, 0, 0, 0).earliest().unwrap().timestamp_millis();
    let now = T0;
    assert_eq!(parse_date("2026", now, false).unwrap(), local(2026, 1, 1));
    assert_eq!(parse_date("2026", now, true).unwrap(), local(2027, 1, 1) - 1);
    assert_eq!(parse_date("2026-02", now, true).unwrap(), local(2026, 3, 1) - 1);
    assert_eq!(parse_date("2026-12", now, true).unwrap(), local(2027, 1, 1) - 1);
    assert_eq!(parse_date("2026-09-21", now, false).unwrap(), local(2026, 9, 21));
    assert_eq!(parse_date("2026-09-21", now, true).unwrap(), local(2026, 9, 22) - 1);
    assert_eq!(parse_date("3d", now, false).unwrap(), now - 3 * DAY);
    assert_eq!(parse_date("2w", now, true).unwrap(), now - 14 * DAY);
    assert_eq!(parse_date("1m", now, false).unwrap(), now - 30 * DAY);
    for bad in ["", "2026-13", "2026-02-30", "26", "yesterday", "3y", "-3d", "d", "2026-1-1-1"] {
        assert_eq!(parse_date(bad, now, false).unwrap_err().code, 2, "{bad}");
    }
}

#[test]
fn failed_and_file_filters() {
    let mut b = SessionBuilder::new("s1", "/work/gray");
    b.user("fix module a");
    b.tool("edit", json!({"path": "src/a.rs"}), "ok", false);
    b.bash("cargo test", "exit 101 · 3.1s");
    b.assistant("Tests still fail.");
    b.user("fix module b");
    b.tool("edit", json!({"path": "src/b.rs"}), "ok", false);
    b.assistant("Fixed.");
    let f = fixture(&[&b]);
    let failed = search(&f.conn, &Request { failed: true, ..req("fix") }, &ctx("gray")).unwrap();
    assert_eq!(found(&failed), ["fix module a"]);
    let by_file = search(&f.conn, &Request { file: Some("b.rs".into()), ..req("") }, &ctx("gray")).unwrap();
    assert_eq!(found(&by_file), ["fix module b"]);
}

#[test]
fn an_empty_query_lists_the_newest_turns() {
    let f = fixture(&[&session("s1", "gray", &["one", "two", "three"])]);
    let s = search(&f.conn, &req(""), &ctx("gray")).unwrap();
    assert_eq!((found(&s), s.total), (vec!["three".to_string(), "two".into(), "one".into()], 3));
    let s = search(&f.conn, &Request { limit: Some(2), offset: 1, ..req("") }, &ctx("gray")).unwrap();
    assert_eq!((found(&s), s.total), (vec!["two".to_string(), "one".into()], 3));
}

#[test]
fn limit_and_offset_page_through_matches() {
    let prompts: Vec<String> = (0..7).map(|i| format!("deploy step {i}")).collect();
    let refs: Vec<&str> = prompts.iter().map(String::as_str).collect();
    let f = fixture(&[&session("s1", "gray", &refs)]);
    let first = search(&f.conn, &req("deploy"), &ctx("gray")).unwrap();
    assert_eq!((first.hits.len(), first.total), (5, 7));
    let second = search(&f.conn, &Request { offset: 5, ..req("deploy") }, &ctx("gray")).unwrap();
    assert_eq!((second.hits.len(), second.total), (2, 7));
    let mut all: Vec<String> = found(&first).into_iter().chain(found(&second)).collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 7);
}

#[test]
fn did_is_the_reply_snippet_when_it_matches_else_the_gist() {
    let mut b = SessionBuilder::new("s1", "/work/gray");
    b.user("why was ci red");
    b.assistant("Looking.");
    b.assistant("The flaky test was a race in the scheduler.");
    b.user("rotate the keys");
    b.assistant("Done: both replaced.");
    let f = fixture(&[&b]);
    let s = search(&f.conn, &req("flaky scheduler"), &ctx("gray")).unwrap();
    let did = &s.hits[0].did;
    assert!(did.contains("flaky") && did.contains("scheduler"), "{did}");
    assert!(!did.contains(MARK_START) && !did.contains(MARK_END), "{did:?}");
    let s = search(&f.conn, &req("keys"), &ctx("gray")).unwrap();
    assert_eq!(s.hits[0].did, "Done: both replaced.");
}

#[test]
fn scores_favor_relevance_then_recency_and_replies() {
    let now = T0;
    let fresh = score(-5.0, now, now, true);
    assert!((fresh - 6.5).abs() < 1e-9, "{fresh}");
    assert!(score(-5.0, now - 365 * DAY, now, true) < fresh);
    assert!((score(-5.0, now, now, false) - fresh / 2.0).abs() < 1e-9);
    assert!(score(-8.0, now - 365 * DAY, now, true) > fresh, "relevance outweighs recency");
}

#[test]
fn request_limits_are_clamped() {
    let r = Request::default();
    assert_eq!((r.limit(), r.max_chars(), r.around()), (5, 2500, 0));
    let r = Request { limit: Some(100), max_chars: Some(10), around: 50, ..Request::default() };
    assert_eq!((r.limit(), r.max_chars(), r.around()), (20, 500, 10));
    let r = Request { limit: Some(0), max_chars: Some(1_000_000), ..Request::default() };
    assert_eq!((r.limit(), r.max_chars()), (1, 50_000));
    assert_eq!(Request { id: Some("x:1".into()), ..Request::default() }.max_chars(), 8000);
}

#[test]
fn fts_syntax_in_user_input_is_plain_text() {
    let f = fixture(&[&session("s1", "gray", &["and or not"])]);
    for q in ["AND OR NOT", "( ) * ^ :", "NEAR(a b)", "\"unbalanced", "col:value", "a*"] {
        assert!(search(&f.conn, &req(q), &ctx("gray")).is_ok(), "{q}");
    }
    assert_eq!(found(&search(&f.conn, &req("NOT"), &ctx("gray")).unwrap()), ["and or not"]);
}

#[test]
fn show_finds_a_turn_by_id_or_prefix_with_neighbors() {
    let a_id = "abc11111-0000-4000-8000-000000000000";
    let b_id = "abc22222-0000-4000-8000-000000000000";
    let f = fixture(&[&session(a_id, "gray", &["one", "two", "three", "four"]), &session(b_id, "gray", &["other"])]);
    let prompts = |s: &Shown| s.turns.iter().map(|t| t.prompt.clone()).collect::<Vec<_>>();
    let s = show(&f.conn, "abc11111:5", 1).unwrap();
    assert_eq!((s.session.as_str(), s.target), (a_id, 5));
    assert_eq!(prompts(&s), ["two", "three", "four"]);
    assert_eq!(prompts(&show(&f.conn, &format!("{a_id}:1"), 0).unwrap()), ["one"]);
    assert_eq!(show(&f.conn, "abc11111:1", 10).unwrap().turns.len(), 4);
    let e = show(&f.conn, "abc:1", 0).unwrap_err();
    assert!(e.code == 3 && e.message.contains(a_id) && e.message.contains(b_id), "{}", e.message);
    for (id, code) in [("zzz:1", 3), ("abc11111:2", 3), ("abc11111", 2), ("abc11111:x", 2), (":5", 2)] {
        assert_eq!(show(&f.conn, id, 0).unwrap_err().code, code, "{id}");
    }
}

#[test]
fn an_exact_session_id_wins_over_longer_ids_it_prefixes() {
    let f = fixture(&[&session("s1", "gray", &["short"]), &session("s10", "gray", &["long"])]);
    assert_eq!(show(&f.conn, "s1:1", 0).unwrap().turns[0].prompt, "short");
}
~~~

~~~bash
$X src/query_tests.rs
sed -i 's/^pub mod redact;/pub mod query;\npub mod redact;/' src/lib.rs
printf '#[cfg(test)]\n#[path = "query_tests.rs"]\nmod query_tests;\n' > src/query.rs
~~~

- [ ] **Step 2: Run the tests to see them fail**

Run: `CARGO_BUILD_JOBS=4 cargo test --lib query`
Expected: compile errors (`cannot find function search`, `cannot find type Request`).

- [ ] **Step 3: Implement `query.rs`**

~~~rust file=src/query.rs
//! Search and show: match expression, dates, project resolution, ranking
//! and the other-project fallback.

use crate::index::{self, ProjectInfo, ROW_COLUMN_COUNT, ROW_COLUMNS, Row};
use crate::project::project_for;
use chrono::{Local, NaiveDate, TimeZone};
use rusqlite::types::Value;
use rusqlite::{Connection, params, params_from_iter};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_LIMIT: usize = 5;
pub const MAX_LIMIT: usize = 20;
pub const SEARCH_CHARS: usize = 2500;
pub const SHOW_CHARS: usize = 8000;
pub const MIN_CHARS: usize = 500;
pub const MAX_CHARS: usize = 50_000;
pub const MAX_AROUND: usize = 10;
/// FTS matches rescored per search.
pub const CANDIDATES: usize = 200;
pub const FUZZY_MIN: f64 = 0.85;
/// `snippet()` markers around matched terms; removed before display.
pub const MARK_START: char = '\u{2}';
pub const MARK_END: char = '\u{3}';
const DAY_MS: i64 = 86_400_000;
const BM25: &str = "bm25(turns_fts, 3.0, 2.0, 1.0, 0.5)";

/// One call, from the tool, `/recall` or the CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub query: String,
    pub project: Option<String>,
    pub all: bool,
    pub file: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub failed: bool,
    pub limit: Option<usize>,
    pub offset: usize,
    /// Show this turn instead of searching.
    pub id: Option<String>,
    pub around: usize,
    pub json: bool,
    pub max_chars: Option<usize>,
}

impl Request {
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }

    pub fn max_chars(&self) -> usize {
        let default = if self.id.is_some() { SHOW_CHARS } else { SEARCH_CHARS };
        self.max_chars.unwrap_or(default).clamp(MIN_CHARS, MAX_CHARS)
    }

    pub fn around(&self) -> usize {
        self.around.min(MAX_AROUND)
    }

    fn explicit_project(&self) -> Option<&str> {
        self.project.as_deref().filter(|p| !p.trim().is_empty())
    }
}

/// Who is asking: the calling session's cwd and id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    pub cwd: String,
    pub session: Option<String>,
    pub now_ms: i64,
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// A failure with its exit code: 1 error, 2 bad request, 3 ambiguous or unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallError {
    pub code: i32,
    pub message: String,
}

impl RecallError {
    pub fn error(message: impl Into<String>) -> Self {
        Self { code: 1, message: message.into() }
    }

    pub fn bad(message: impl Into<String>) -> Self {
        Self { code: 2, message: message.into() }
    }

    pub fn unknown(message: impl Into<String>) -> Self {
        Self { code: 3, message: message.into() }
    }
}

impl fmt::Display for RecallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RecallError {}

impl From<rusqlite::Error> for RecallError {
    fn from(e: rusqlite::Error) -> Self {
        let message = e.to_string();
        if message.contains("fts5: syntax error") {
            Self::bad(format!("bad query: {message}"))
        } else {
            Self::error(message)
        }
    }
}

impl From<anyhow::Error> for RecallError {
    fn from(e: anyhow::Error) -> Self {
        Self::error(format!("{e:#}"))
    }
}

enum Term {
    Word(String),
    Phrase(String),
    Not(String),
}

fn terms(query: &str) -> Vec<Term> {
    let mut out = Vec::new();
    let mut rest = query.trim_start();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('"') {
            let (phrase, after) = r.split_once('"').unwrap_or((r, ""));
            out.push(Term::Phrase(phrase.to_string()));
            rest = after;
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let word = &rest[..end];
            out.push(match word.strip_prefix('-') {
                Some(x) if !x.is_empty() => Term::Not(x.to_string()),
                _ => Term::Word(word.to_string()),
            });
            rest = &rest[end..];
        }
        rest = rest.trim_start();
    }
    out.retain(|t| match t {
        Term::Word(s) | Term::Phrase(s) | Term::Not(s) => s.chars().any(char::is_alphanumeric),
    });
    out
}

fn quote(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// The FTS5 expression for `query`, or `None` for an empty query.
pub fn match_expr(query: &str) -> Result<Option<String>, RecallError> {
    let (mut words, mut phrases, mut nots) = (Vec::new(), Vec::new(), Vec::new());
    for t in terms(query) {
        match t {
            Term::Word(w) => words.push(quote(&w)),
            Term::Phrase(p) => phrases.push(quote(&p)),
            Term::Not(n) => nots.push(quote(&n)),
        }
    }
    if words.is_empty() && phrases.is_empty() {
        if nots.is_empty() {
            return Ok(None);
        }
        return Err(RecallError::bad("exclusions need at least one word or phrase to exclude from"));
    }
    let core = if phrases.is_empty() {
        words.join(" OR ")
    } else if words.is_empty() {
        phrases.join(" AND ")
    } else {
        format!("{} AND ({} OR {})", phrases.join(" AND "), phrases[0], words.join(" OR "))
    };
    Ok(Some(if nots.is_empty() { core } else { format!("({core}) NOT {}", nots.join(" NOT ")) }))
}

/// `YYYY`, `YYYY-MM`, `YYYY-MM-DD` (local time; `end` gives the period's last
/// millisecond) or `Nd` / `Nw` / `Nm` before `now_ms`.
pub fn parse_date(s: &str, now_ms: i64, end: bool) -> Result<i64, RecallError> {
    let s = s.trim();
    let bad = || RecallError::bad(format!("bad date '{s}': use YYYY, YYYY-MM, YYYY-MM-DD, or Nd / Nw / Nm"));
    if let Some(n) = s.strip_suffix(['d', 'w', 'm']) {
        let n = i64::from(n.parse::<u32>().map_err(|_| bad())?);
        let days = match s.as_bytes()[s.len() - 1] {
            b'd' => n,
            b'w' => 7 * n,
            _ => 30 * n,
        };
        return Ok(now_ms - days * DAY_MS);
    }
    let parts: Vec<&str> = s.split('-').collect();
    let num = |i: usize| {
        parts.get(i).filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).and_then(|p| p.parse::<u32>().ok())
    };
    let year = num(0).filter(|_| parts[0].len() == 4).ok_or_else(bad)? as i32;
    let ymd = |y: i32, m: u32, d: u32| NaiveDate::from_ymd_opt(y, m, d);
    let range = match parts.len() {
        1 => ymd(year, 1, 1).zip(ymd(year + 1, 1, 1)),
        2 => num(1).and_then(|m| {
            let next = if m == 12 { ymd(year + 1, 1, 1) } else { ymd(year, m + 1, 1) };
            ymd(year, m, 1).zip(next)
        }),
        3 => num(1).zip(num(2)).and_then(|(m, d)| ymd(year, m, d)).and_then(|d| Some((d, d.succ_opt()?))),
        _ => None,
    };
    let (start, next) = range.ok_or_else(bad)?;
    let local = |d: NaiveDate| Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).earliest().map(|t| t.timestamp_millis());
    if end { local(next).map(|ms| ms - 1).ok_or_else(bad) } else { local(start).ok_or_else(bad) }
}

/// Resolve `-p name` in tiers; the first tier with candidates decides.
pub fn resolve_project(projects: &[ProjectInfo], name: &str) -> Result<ProjectInfo, RecallError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RecallError::bad("empty project name"));
    }
    let lower = name.to_lowercase();
    let similarity = |p: &ProjectInfo| strsim::jaro_winkler(&p.name.to_lowercase(), &lower);
    let tiers: [&dyn Fn(&ProjectInfo) -> bool; 5] = [
        &|p| p.key == name,
        &|p| p.name == name,
        &|p| p.name.to_lowercase() == lower,
        &|p| p.name.to_lowercase().contains(&lower),
        &|p| similarity(p) >= FUZZY_MIN,
    ];
    for tier in tiers {
        match projects.iter().filter(|p| tier(p)).collect::<Vec<_>>().as_slice() {
            [] => {}
            [one] => return Ok((*one).clone()),
            many => {
                let list: Vec<String> = many.iter().map(|p| format!("{} ({})", p.name, p.key)).collect();
                return Err(RecallError::unknown(format!(
                    "project '{name}' is ambiguous: {}; pass a full name or path",
                    list.join(", ")
                )));
            }
        }
    }
    let mut close: Vec<(f64, &str)> = projects.iter().map(|p| (similarity(p), p.name.as_str())).collect();
    close.sort_by(|a, b| b.0.total_cmp(&a.0));
    let names: Vec<&str> = close.iter().take(3).map(|(_, n)| *n).collect();
    Err(RecallError::unknown(if names.is_empty() {
        format!("no project matches '{name}': the index has no projects yet")
    } else {
        format!("no project matches '{name}'; closest: {}", names.join(", "))
    }))
}

/// BM25 (negative, lower is better) to a score where higher is better,
/// with a recency boost of up to 30% and half weight for reply-less turns.
pub fn score(bm25: f64, date: i64, now_ms: i64, has_reply: bool) -> f64 {
    let age_days = (now_ms - date).max(0) as f64 / DAY_MS as f64;
    let reply = if has_reply { 1.0 } else { 0.5 };
    -bm25 * (1.0 + 0.3 * (-age_days / 90.0).exp()) * reply
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub row: Row,
    /// The matching part of the reply, else the gist.
    pub did: String,
    pub score: f64,
    /// Found by the fallback outside the caller's project.
    pub other_project: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Search {
    /// `project <name>` or `all projects`.
    pub scope: String,
    pub scope_turns: i64,
    pub all: bool,
    pub hits: Vec<Hit>,
    /// Every match in the scope that produced `hits`.
    pub total: usize,
    pub fell_back: bool,
}

#[derive(Default)]
struct Filter {
    since: Option<i64>,
    until: Option<i64>,
    file: Option<String>,
    failed: bool,
    session: Option<String>,
}

pub fn search(conn: &Connection, req: &Request, ctx: &Context) -> Result<Search, RecallError> {
    let expr = match_expr(&req.query)?;
    let filter = Filter {
        since: req.since.as_deref().map(|s| parse_date(s, ctx.now_ms, false)).transpose()?,
        until: req.until.as_deref().map(|s| parse_date(s, ctx.now_ms, true)).transpose()?,
        file: req.file.clone().filter(|f| !f.is_empty()),
        failed: req.failed,
        session: ctx.session.clone().filter(|s| !s.is_empty()),
    };
    let projects = index::list_projects(conn)?;
    let (scope, key) = if req.all {
        ("all projects".to_string(), None)
    } else if let Some(name) = req.explicit_project() {
        let p = resolve_project(&projects, name)?;
        (format!("project {}", p.name), Some(p.key))
    } else {
        let p = project_for(&ctx.cwd);
        (format!("project {}", p.name), Some(p.key))
    };
    let scope_turns = match &key {
        Some(k) => projects.iter().find(|p| &p.key == k).map_or(0, |p| p.turns),
        None => projects.iter().map(|p| p.turns).sum(),
    };
    let (hits, total) = run(conn, expr.as_deref(), &filter, key.as_deref().map(|k| (k, true)), req, ctx.now_ms)?;
    if total == 0
        && !req.all
        && req.explicit_project().is_none()
        && let Some(k) = key.as_deref()
    {
        let (mut hits, total) = run(conn, expr.as_deref(), &filter, Some((k, false)), req, ctx.now_ms)?;
        hits.iter_mut().for_each(|h| h.other_project = true);
        return Ok(Search { scope, scope_turns, all: false, hits, total, fell_back: true });
    }
    Ok(Search { scope, scope_turns, all: req.all, hits, total, fell_back: false })
}

/// One scoped query. `project` is `(key, inside)`: inside the project, or
/// everywhere but it.
fn run(
    conn: &Connection,
    expr: Option<&str>,
    f: &Filter,
    project: Option<(&str, bool)>,
    req: &Request,
    now_ms: i64,
) -> Result<(Vec<Hit>, usize), RecallError> {
    let mut clauses: Vec<&str> = Vec::new();
    let mut args: Vec<Value> = Vec::new();
    let mut add = |clause: &'static str, arg: Option<Value>| {
        clauses.push(clause);
        args.extend(arg);
    };
    if let Some(e) = expr {
        add("turns_fts MATCH ?", Some(Value::Text(e.to_string())));
    }
    if let Some((key, inside)) = project {
        add(if inside { "t.project_key = ?" } else { "t.project_key != ?" }, Some(Value::Text(key.to_string())));
    }
    if let Some(s) = &f.session {
        add("t.session != ?", Some(Value::Text(s.clone())));
    }
    if let Some(v) = f.since {
        add("t.date >= ?", Some(Value::Integer(v)));
    }
    if let Some(v) = f.until {
        add("t.date <= ?", Some(Value::Integer(v)));
    }
    if let Some(file) = &f.file {
        add("instr(t.files, ?) > 0", Some(Value::Text(file.clone())));
    }
    if f.failed {
        add("t.failed = 1", None);
    }
    let filter = if clauses.is_empty() { String::new() } else { format!("WHERE {}", clauses.join(" AND ")) };
    let from = if expr.is_some() { "turns_fts JOIN turns t ON t.rowid = turns_fts.rowid" } else { "turns t" };
    let total: i64 =
        conn.query_row(&format!("SELECT count(*) FROM {from} {filter}"), params_from_iter(&args), |r| r.get(0))?;
    let limit = req.limit();
    let hits = if expr.is_some() {
        let sql = format!(
            "SELECT {ROW_COLUMNS}, {BM25}, snippet(turns_fts, 2, char(2), char(3), ' … ', 24)
             FROM {from} {filter} ORDER BY {BM25} LIMIT {CANDIDATES}"
        );
        let mut hits = hits(conn, &sql, &args, now_ms)?;
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(b.row.date.cmp(&a.row.date)));
        hits.into_iter().skip(req.offset).take(limit).collect()
    } else {
        let sql = format!(
            "SELECT {ROW_COLUMNS}, 0.0, '' FROM {from} {filter} ORDER BY t.date DESC LIMIT {limit} OFFSET {}",
            req.offset
        );
        hits(conn, &sql, &args, now_ms)?
    };
    Ok((hits, total as usize))
}

fn hits(conn: &Connection, sql: &str, args: &[Value], now_ms: i64) -> Result<Vec<Hit>, RecallError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params_from_iter(args), |r| {
            Ok((Row::read(r, 0)?, r.get::<_, f64>(ROW_COLUMN_COUNT)?, r.get::<_, String>(ROW_COLUMN_COUNT + 1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .map(|(row, bm25, snippet)| {
            let did = if snippet.contains(MARK_START) {
                snippet.replace([MARK_START, MARK_END], "")
            } else {
                row.gist.clone()
            };
            Hit { score: score(bm25, row.date, now_ms, row.has_reply), did, row, other_project: false }
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub session: String,
    pub target: i64,
    /// The target and its neighbors, in entry order.
    pub turns: Vec<Row>,
}

/// The turn `<session or prefix>:<entry_id>` and `around` turns on each side.
pub fn show(conn: &Connection, id: &str, around: usize) -> Result<Shown, RecallError> {
    let id = id.trim();
    let bad = || RecallError::bad(format!("bad id '{id}': expected <session>:<entry> as printed on a card"));
    let (prefix, entry) = id.rsplit_once(':').ok_or_else(bad)?;
    let target: i64 = entry.parse().map_err(|_| bad())?;
    if prefix.is_empty() {
        return Err(bad());
    }
    let session = resolve_session(conn, prefix)?;
    let rows = |tail: &str, n: i64| -> Result<Vec<Row>, RecallError> {
        let mut stmt = conn.prepare(&format!("SELECT {ROW_COLUMNS} FROM turns t WHERE t.session = ?1 {tail}"))?;
        let rows = stmt.query_map(params![session, target, n], |r| Row::read(r, 0))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    };
    let found = rows("AND t.entry_id = ?2 LIMIT ?3", 1)?;
    if found.is_empty() {
        return Err(RecallError::unknown(format!("no turn {target} in session {session}")));
    }
    let n = around as i64;
    let mut turns = rows("AND t.entry_id < ?2 ORDER BY t.entry_id DESC LIMIT ?3", n)?;
    turns.reverse();
    turns.extend(found);
    turns.extend(rows("AND t.entry_id > ?2 ORDER BY t.entry_id LIMIT ?3", n)?);
    Ok(Shown { session, target, turns })
}

fn resolve_session(conn: &Connection, prefix: &str) -> Result<String, RecallError> {
    let exact: i64 = conn.query_row("SELECT count(*) FROM sessions WHERE session = ?1", [prefix], |r| r.get(0))?;
    if exact > 0 {
        return Ok(prefix.to_string());
    }
    let mut stmt =
        conn.prepare("SELECT session FROM sessions WHERE substr(session, 1, length(?1)) = ?1 ORDER BY session LIMIT 11")?;
    let found: Vec<String> = stmt.query_map([prefix], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    match found.as_slice() {
        [] => Err(RecallError::unknown(format!("no session matches '{prefix}'"))),
        [one] => Ok(one.clone()),
        many => Err(RecallError::unknown(format!(
            "session prefix '{prefix}' is ambiguous: {}",
            many.iter().take(10).cloned().collect::<Vec<_>>().join(", ")
        ))),
    }
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod query_tests;
~~~

- [ ] **Step 4: Run the query tests**

Run: `$X src/query.rs && CARGO_BUILD_JOBS=4 cargo test --lib query`
Expected: 19 passed.

- [ ] **Step 5: Format, lint, commit**

~~~bash
CARGO_BUILD_JOBS=4 cargo fmt && CARGO_BUILD_JOBS=4 cargo clippy --all-targets -- -D warnings && CARGO_BUILD_JOBS=4 cargo test
git add -A && git commit -m "Search and show: match expressions, dates, project tiers, ranking, fallback"
~~~

---

### Task 8: Render: text and JSON views

**Files:**
- Create: `src/render.rs`, `src/render_tests.rs`
- Modify: `src/lib.rs` (add `pub mod render;`)

**Interfaces:**
- Consumes: `query::{Hit, Request, Search, Shown}`, `index::{ProjectInfo, Row, Stats}`, `project::display_id`, `text::{cap, clip}`.
- Produces:
  - `render::BUSY_NOTE` (`(index catching up in another session)`).
  - `render::search_text(&Search, &Request, busy: bool) -> String` and `render::search_json(&Search, &Request, busy) -> String`.
  - `render::show_text(&Shown, max_chars: usize, busy) -> String` and `render::show_json(&Shown, busy) -> String`.
  - `render::projects_text(&[ProjectInfo], max_chars) -> String`, `render::projects_json(&[ProjectInfo]) -> String`.
  - `render::status_text(&Stats, index: &Path, now_ms, busy) -> String`, `render::status_json(&Stats, index: &Path, busy) -> String`.
  - Helpers `thousands(i64)`, `plural(n, word)` (`1 turn`, `4,140 turns`), `size(u64)`, `when(ms)` (local `YYYY-MM-DD HH:MM`).
  - Every view returns text without a trailing newline.

Rules (the spec's Output section, made exact):
- Search header: `recall · <scope> (<N> turns) · "<query>" · since X · until X · file X · failed only · shown N of M from [k]`. Empty parts are left out; a query that holds its own `"` is echoed as written, without the outer quotes; the query and filter values are clipped to 60 chars; ` from [k]` appears only with an offset.
- Notes under the header: `no match in <scope>; searched the other projects` after a fallback, and `BUSY_NOTE` while another session holds the writer lock.
- Card: `[n] <display_id> · <when> · ✗ failed cmd · OTHER PROJECT <name>` (or ` · <name>` when searching all projects). Then `  you:` (prompt), `  did:` (snippet or gist) and `  files: ...   commits: ...` (files clipped to 120 chars, up to 3 commits cut to 7 chars). Cards are numbered from `offset + 1`.
- Card forms, richest first: full (you 200, did 300, files and commits), short (you 100, did 140), floor (you 60). First count how many cards fit in their floor form; then, in order, give each card the richest form that still leaves room for the rest at their floor. The header's `shown N` is the number of cards printed. A final cut to `max_chars` only guards against an oversized header.
- Zero cards: `none found: try --all, fewer or other words, or a wider date range` in an explicit project. After a fallback or with `all`: `none found in any project: ...`. Matches exist but the offset passed them: `none on this page: lower the offset`. No `next:` line.
- Footer: `next: recall id=<first card's id> around=2 for detail · resume: gray -r <full session id>`.
- Show: header `recall show <session>:<entry> · project <name> · <cwd>`, then turns in entry order. Neighbors are `- <id> · <when>` with `  you:` / `  did: <gist>`. The target is `▶ <id> · <when> · ✗ failed cmd`, then `you: <prompt>`, `reply:` and the reply on its own lines (`reply: (none)` when empty), `commands:` and the commands when there are any, and the files/commits line.
- Show budget: neighbors get at most half of `max_chars`, at (you 150, did 150), or else (you 60). Nearest neighbors are kept first, and the rest are counted in `(<k> neighbors left out to fit max_chars)`. What remains goes to the target: the prompt takes up to 1/4 (more if the reply and commands need less), commands up to 1/3 of the rest (more if the reply needs less), and the reply the remainder. Footer: `resume: gray -r <session>`.
- JSON views carry the same fields as objects. They are bounded by `limit` and field caps (prompts in search hits and show neighbors cut to 500 chars) instead of `max_chars`; the show target is complete.

- [ ] **Step 1: Write the failing tests**

~~~rust file=src/render_tests.rs
use super::*;
use crate::index::{ProjectInfo, Row, Stats};
use crate::query::{Hit, Request, Search, Shown};
use crate::testkit::T0;
use serde_json::Value;
use std::path::Path;

const S: &str = "1900bae3-0000-4000-8000-000000000000";

fn row(entry_id: i64, prompt: &str) -> Row {
    Row {
        rowid: entry_id,
        session: S.into(),
        entry_id,
        project_key: "/work/gray".into(),
        project_name: "gray".into(),
        cwd: "/work/gray".into(),
        date: T0,
        prompt: prompt.into(),
        reply: format!("Reply to {prompt}."),
        gist: format!("Gist of {prompt}."),
        commands: String::new(),
        files: Vec::new(),
        commits: Vec::new(),
        failed: false,
        has_reply: true,
    }
}

fn hit(entry_id: i64, prompt: &str) -> Hit {
    Hit { row: row(entry_id, prompt), did: format!("Did {prompt}."), score: 1.0, other_project: false }
}

fn search(hits: Vec<Hit>, total: usize) -> Search {
    Search { scope: "project gray".into(), scope_turns: 3886, all: false, hits, total, fell_back: false }
}

fn req(q: &str) -> Request {
    Request { query: q.into(), ..Request::default() }
}

fn shown_turns(target: i64, turns: Vec<Row>) -> Shown {
    Shown { session: S.into(), target, turns }
}

/// `N` from the header's `shown N of M`.
fn shown(out: &str) -> usize {
    let header = out.lines().next().unwrap();
    header.split("shown ").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap()
}

fn cards(out: &str) -> usize {
    out.lines().filter(|l| l.starts_with('[')).count()
}

fn long_hits(n: usize) -> Vec<Hit> {
    (0..n)
        .map(|i| {
            let mut h = hit(i as i64 + 1, &format!("prompt {i} {}", "word ".repeat(400)));
            h.did = format!("did {i} {}", "detail ".repeat(100));
            h.row.files = (0..30).map(|f| format!("src/module_{f}/file.rs")).collect();
            h.row.commits = vec!["2ba5532".into(), "aaaaaaa".into()];
            h
        })
        .collect()
}

#[test]
fn a_card_matches_the_spec_layout() {
    let mut h = hit(3154, "and uh give a minimal solution to this");
    h.row.failed = true;
    h.row.files = vec!["crates/gray/src/lib.rs".into()];
    h.row.commits = vec!["2ba5532".into()];
    h.did = "Cache path is byte-identical to HEAD".into();
    let r = Request { since: Some("2026-09".into()), ..req("prefix cache compaction") };
    let want = [
        r#"recall · project gray (3,886 turns) · "prefix cache compaction" · since 2026-09 · shown 1 of 41"#.to_string(),
        format!("[1] 1900bae3:3154 · {} · ✗ failed cmd", when(T0)),
        "  you: and uh give a minimal solution to this".into(),
        "  did: Cache path is byte-identical to HEAD".into(),
        "  files: crates/gray/src/lib.rs   commits: 2ba5532".into(),
        format!("next: recall id=1900bae3:3154 around=2 for detail · resume: gray -r {S}"),
    ]
    .join("\n");
    assert_eq!(search_text(&search(vec![h], 41), &r, false), want);
}

#[test]
fn a_query_with_its_own_quotes_is_echoed_as_written() {
    let out = search_text(&search(vec![], 0), &req(r#""prefix cache" warm"#), false);
    assert!(out.starts_with(r#"recall · project gray (3,886 turns) · "prefix cache" warm · shown 0 of 0"#), "{out}");
}

#[test]
fn fallback_and_all_scope_cards_name_the_project() {
    let mut h = hit(7, "deploy graysite pages");
    h.row.project_name = "graysite".into();
    h.other_project = true;
    let s = Search { fell_back: true, ..search(vec![h.clone()], 1) };
    let out = search_text(&s, &req("pages"), false);
    assert!(out.contains("\nno match in project gray; searched the other projects\n"), "{out}");
    assert!(out.contains(" · OTHER PROJECT graysite\n"), "{out}");
    h.other_project = false;
    let s = Search { scope: "all projects".into(), all: true, ..search(vec![h], 1) };
    let out = search_text(&s, &req("pages"), false);
    assert!(out.contains(&format!("[1] 1900bae3:7 · {} · graysite\n", when(T0))), "{out}");
    assert!(!out.contains("OTHER PROJECT") && !out.contains("no match in"), "{out}");
}

#[test]
fn zero_hits_say_none_found_and_suggest_a_wider_search() {
    let out = search_text(&search(vec![], 0), &Request { project: Some("gray".into()), ..req("zzz") }, false);
    let want = r#"recall · project gray (3,886 turns) · "zzz" · shown 0 of 0
none found: try --all, fewer or other words, or a wider date range"#;
    assert_eq!(out, want);
    let out = search_text(&Search { fell_back: true, ..search(vec![], 0) }, &req("zzz"), false);
    let tail = "searched the other projects\nnone found in any project: try fewer or other words, or a wider date range";
    assert!(out.ends_with(tail), "{out}");
    let out = search_text(&search(vec![], 7), &Request { offset: 10, ..req("deploy") }, false);
    assert!(out.contains("shown 0 of 7 from [11]") && out.ends_with("none on this page: lower the offset"), "{out}");
    assert!(!out.contains("next:"), "{out}");
}

#[test]
fn text_fits_max_chars_and_the_header_counts_the_cards_shown() {
    let s = search(long_hits(20), 20);
    for max in [500, 1000, 2500, 8000, 50_000] {
        let out = search_text(&s, &Request { max_chars: Some(max), ..req("word") }, true);
        assert!(out.chars().count() <= max, "{max}: {}", out.chars().count());
        assert_eq!(shown(&out), cards(&out), "{max}");
        assert!(shown(&out) >= 1, "{max}: {out}");
        assert!(out.contains(BUSY_NOTE) && out.contains("next: recall id=1900bae3:1 "), "{max}: {out}");
    }
    let roomy = search_text(&s, &Request { max_chars: Some(50_000), ..req("word") }, false);
    assert_eq!(shown(&roomy), 20);
    assert_eq!(roomy.matches("\n  files: ").count(), 20);
    let floor = search_text(&s, &Request { max_chars: Some(8000), ..req("word") }, false);
    assert_eq!(shown(&floor), 20, "every card fits in its floor form");
}

#[test]
fn cards_shrink_before_any_is_dropped_and_earlier_cards_stay_richer() {
    let out = search_text(&search(long_hits(5), 5), &req("word"), false);
    assert!(out.chars().count() <= 2500, "{}", out.chars().count());
    assert_eq!(shown(&out), 5);
    let blocks: Vec<&str> = out.split("\n[").collect();
    assert!(blocks[1].contains("\n  files: "), "{out}");
    assert!(!blocks[5].contains("\n  files: "), "{out}");
}

#[test]
fn offset_numbers_the_cards_and_the_header() {
    let out = search_text(&search(vec![hit(6, "six"), hit(7, "seven")], 7), &Request { offset: 5, ..req("") }, false);
    assert!(out.starts_with("recall · project gray (3,886 turns) · shown 2 of 7 from [6]\n[6] "), "{out}");
    assert!(out.contains("\n[7] "), "{out}");
}

#[test]
fn the_header_echoes_filters_and_clips_long_values() {
    let r = Request {
        file: Some("lib.rs".into()),
        until: Some("2026-10-01".into()),
        failed: true,
        ..req(&"q".repeat(500))
    };
    let out = search_text(&search(vec![hit(1, "x")], 1), &r, false);
    let header = out.lines().next().unwrap();
    assert!(header.chars().count() < 200, "{header}");
    assert!(header.ends_with(" · until 2026-10-01 · file lib.rs · failed only · shown 1 of 1"), "{header}");
}

#[test]
fn search_json_carries_the_same_data() {
    let mut h = hit(3, &"long ".repeat(300));
    h.row.project_name = "graysite".into();
    h.other_project = true;
    let s = Search { fell_back: true, ..search(vec![h], 1) };
    let v: Value = serde_json::from_str(&search_json(&s, &req("long"), true)).unwrap();
    assert_eq!(
        (v["total"].as_u64(), v["shown"].as_u64(), v["fell_back"].as_bool(), v["busy"].as_bool()),
        (Some(1), Some(1), Some(true), Some(true))
    );
    let first = &v["hits"][0];
    assert_eq!((first["n"].as_u64(), first["id"].as_str(), first["session"].as_str()), (Some(1), Some("1900bae3:3"), Some(S)));
    assert_eq!((first["project"].as_str(), first["other_project"].as_bool()), (Some("graysite"), Some(true)));
    assert_eq!(first["time"].as_str(), Some(when(T0).as_str()));
    assert_eq!(first["prompt"].as_str().unwrap().chars().count(), 500);
}

#[test]
fn show_prints_the_target_in_full_between_its_neighbors() {
    let mut t = row(5, "three");
    t.commands = "$ cargo test".into();
    t.files = vec!["src/a.rs".into()];
    t.failed = true;
    let d = when(T0);
    let want = [
        format!("recall show {S}:5 · project gray · /work/gray"),
        format!("- 1900bae3:3 · {d}"),
        "  you: two".into(),
        "  did: Gist of two.".into(),
        format!("▶ 1900bae3:5 · {d} · ✗ failed cmd"),
        "you: three".into(),
        "reply:".into(),
        "Reply to three.".into(),
        "commands:".into(),
        "$ cargo test".into(),
        "files: src/a.rs".into(),
        format!("- 1900bae3:7 · {d}"),
        "  you: four".into(),
        "  did: Gist of four.".into(),
        format!("resume: gray -r {S}"),
    ]
    .join("\n");
    assert_eq!(show_text(&shown_turns(5, vec![row(3, "two"), t, row(7, "four")]), 8000, false), want);
    let mut empty = row(1, "hi");
    empty.reply = String::new();
    assert!(show_text(&shown_turns(1, vec![empty]), 8000, true).contains(&format!("\n{BUSY_NOTE}\n▶ 1900bae3:1 · {d}\nyou: hi\nreply: (none)\nresume:")));
}

#[test]
fn show_splits_the_budget_between_reply_and_commands() {
    let mut t = row(5, "why is ci red");
    t.reply = "reply ".repeat(4000);
    t.commands = "$ cargo test --workspace\n".repeat(400);
    let sh = shown_turns(5, vec![row(4, "before"), t, row(6, "after")]);
    for max in [500, 2000, 8000] {
        let out = show_text(&sh, max, false);
        assert!(out.chars().count() <= max, "{max}: {}", out.chars().count());
        let reply = out.split("\nreply:\n").nth(1).unwrap().split("\ncommands:\n").next().unwrap();
        let commands = out.split("\ncommands:\n").nth(1).unwrap().split("\n- ").next().unwrap();
        assert!(reply.chars().count() > commands.chars().count(), "{max}");
        assert!(commands.starts_with("$ cargo test"), "{max}: {out}");
        assert!(out.contains("\n- 1900bae3:4 · ") && out.contains("\n- 1900bae3:6 · "), "{max}: {out}");
        assert!(out.ends_with(&format!("resume: gray -r {S}")), "{max}: {out}");
    }
    let out = show_text(&sh, 8000, false);
    assert!(out.chars().count() > 7900, "the budget is used: {}", out.chars().count());
}

#[test]
fn far_neighbors_shrink_then_drop_to_fit() {
    let long = "context ".repeat(50);
    let sh = shown_turns(11, (1..=21).map(|i| row(i, &format!("{i} {long}"))).collect());
    let out = show_text(&sh, 500, false);
    assert!(out.chars().count() <= 500, "{}", out.chars().count());
    assert!(out.contains("\n▶ 1900bae3:11 · ") && out.contains("neighbors left out to fit max_chars"), "{out}");
    let mid = show_text(&sh, 4000, false);
    assert!(mid.contains("\n- 1900bae3:10 · ") && mid.contains("\n- 1900bae3:12 · "), "{mid}");
    let roomy = show_text(&sh, 50_000, false);
    assert!(!roomy.contains("left out") && roomy.matches("\n- 1900bae3:").count() == 20, "{roomy}");
}

#[test]
fn show_json_has_the_target_in_full_and_neighbor_gists() {
    let mut t = row(5, "three");
    t.commands = "$ ls".into();
    let v: Value = serde_json::from_str(&show_json(&shown_turns(5, vec![row(3, "two"), t]), false)).unwrap();
    assert_eq!((v["session"].as_str(), v["target"].as_i64(), v["project"].as_str()), (Some(S), Some(5), Some("gray")));
    let turns = v["turns"].as_array().unwrap();
    assert_eq!(
        (turns[0]["target"].as_bool(), turns[0]["gist"].as_str(), turns[0].get("reply")),
        (Some(false), Some("Gist of two."), None)
    );
    assert_eq!(
        (turns[1]["target"].as_bool(), turns[1]["reply"].as_str(), turns[1]["commands"].as_str()),
        (Some(true), Some("Reply to three."), Some("$ ls"))
    );
}

#[test]
fn projects_list_aligned_with_totals_and_fit_the_budget() {
    let ps = vec![
        ProjectInfo { key: "/home/u/gray".into(), name: "gray".into(), turns: 3886 },
        ProjectInfo { key: "/home/u/graysite".into(), name: "graysite".into(), turns: 254 },
    ];
    let want = "recall · 2 projects · 4,140 turns\n  gray      3,886  /home/u/gray\n  graysite    254  /home/u/graysite";
    assert_eq!(projects_text(&ps, 2500), want);
    assert_eq!(projects_text(&[], 2500), "recall · 0 projects · 0 turns\nnone indexed yet");
    let many: Vec<ProjectInfo> = (0..100)
        .map(|i| ProjectInfo { key: format!("/home/u/project-{i}"), name: format!("project-{i}"), turns: 1 })
        .collect();
    let out = projects_text(&many, 500);
    assert!(out.chars().count() <= 500 && out.ends_with(" more (--json lists all)"), "{out}");
    let v: Value = serde_json::from_str(&projects_json(&ps)).unwrap();
    assert_eq!(
        (v[0]["name"].as_str(), v[1]["turns"].as_i64(), v[1]["key"].as_str()),
        (Some("gray"), Some(254), Some("/home/u/graysite"))
    );
}

#[test]
fn status_reports_counts_size_and_last_catch_up() {
    let st = Stats { turns: 4140, sessions: 819, projects: 21, skipped: 3, bytes: 25_200_000, last_catch_up: Some(T0 - 180_000) };
    let want = format!(
        "recall index · 4,140 turns · 819 sessions · 21 projects\nsize 24.0 MB · skipped lines 3 · last catch-up {} (3 min ago)\nfile /h/.gray/recall/index.db",
        when(T0 - 180_000)
    );
    assert_eq!(status_text(&st, Path::new("/h/.gray/recall/index.db"), T0, false), want);
    let never = status_text(&Stats { last_catch_up: None, ..st.clone() }, Path::new("/x"), T0, true);
    assert!(never.contains("last catch-up never") && never.ends_with(BUSY_NOTE), "{never}");
    let v: Value = serde_json::from_str(&status_json(&st, Path::new("/x"), false)).unwrap();
    assert_eq!(
        (v["turns"].as_i64(), v["bytes"].as_u64(), v["last_catch_up"].as_i64(), v["index"].as_str()),
        (Some(4140), Some(25_200_000), Some(T0 - 180_000), Some("/x"))
    );
}

#[test]
fn numbers_sizes_and_ages_format() {
    let n = [thousands(0), thousands(999), thousands(1000), thousands(1_234_567), thousands(-4140)];
    assert_eq!(n, ["0", "999", "1,000", "1,234,567", "-4,140"]);
    assert_eq!([size(12), size(2048), size(25_200_000)], ["12 B", "2 KB", "24.0 MB"]);
    assert_eq!([plural(1, "turn"), plural(0, "turn"), plural(4140, "turn")], ["1 turn", "0 turns", "4,140 turns"]);
    let a = [ago(5_000), ago(180_000), ago(7_200_000), ago(3 * 86_400_000), ago(-1)];
    assert_eq!(a, ["just now", "3 min ago", "2 h ago", "3 d ago", "just now"]);
}
~~~

~~~bash
$X src/render_tests.rs
sed -i 's/^pub mod redact;/pub mod redact;\npub mod render;/' src/lib.rs
printf '#[cfg(test)]\n#[path = "render_tests.rs"]\nmod render_tests;\n' > src/render.rs
~~~

- [ ] **Step 2: Run the tests to see them fail**

Run: `CARGO_BUILD_JOBS=4 cargo test --lib render`
Expected: compile errors (`cannot find function search_text`, `cannot find function when`).

- [ ] **Step 3: Implement `render.rs`**

~~~rust file=src/render.rs
//! Text and JSON views of search, show, projects and status. Text views fit
//! the request's character budget; JSON views are bounded by the result
//! limit and per-field caps instead.

use crate::index::{ProjectInfo, Row, Stats};
use crate::project::display_id;
use crate::query::{Hit, Request, Search, Shown};
use crate::text::{cap, clip};
use chrono::{Local, TimeZone};
use serde_json::{Value, json};
use std::path::Path;

pub const BUSY_NOTE: &str = "(index catching up in another session)";
/// Longest query or filter value echoed in a header.
const ECHO_CHARS: usize = 60;
const FILES_CHARS: usize = 120;
const MAX_COMMITS: usize = 3;
const NAME_CHARS: usize = 24;
/// Prompt cap in JSON search hits and show neighbors.
const JSON_PROMPT_CHARS: usize = 500;

/// How much of a turn a card shows.
#[derive(Clone, Copy)]
struct Level {
    you: usize,
    did: usize,
    extras: bool,
}

/// Search card forms, richest first; the last is the floor.
const CARD_LEVELS: [Level; 3] = [
    Level { you: 200, did: 300, extras: true },
    Level { you: 100, did: 140, extras: false },
    Level { you: 60, did: 0, extras: false },
];
/// Show neighbor forms, richest first.
const NEIGHBOR_LEVELS: [Level; 2] =
    [Level { you: 150, did: 150, extras: false }, Level { you: 60, did: 0, extras: false }];

pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `1 turn`, `4,140 turns`.
pub fn plural(n: i64, word: &str) -> String {
    format!("{} {word}{}", thousands(n), if n == 1 { "" } else { "s" })
}

/// `24.0 MB`, `512 KB`, `12 B`.
pub fn size(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b >= K * K {
        format!("{:.1} MB", b / K / K)
    } else if b >= K {
        format!("{:.0} KB", b / K)
    } else {
        format!("{bytes} B")
    }
}

/// Local `YYYY-MM-DD HH:MM`.
pub fn when(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).earliest().map_or_else(|| "?".to_string(), |t| t.format("%Y-%m-%d %H:%M").to_string())
}

fn ago(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s < 60 {
        "just now".to_string()
    } else if s < 3600 {
        format!("{} min ago", s / 60)
    } else if s < 86_400 {
        format!("{} h ago", s / 3600)
    } else {
        format!("{} d ago", s / 86_400)
    }
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// `s` cut to `max` chars, ending in `…` when cut; newlines kept.
fn fit(s: &str, max: usize) -> String {
    if chars(s) <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    if max > 0 {
        out.push('…');
    }
    out
}

/// `1900bae3:3154 · 2026-09-15 21:18 · ✗ failed cmd`
fn turn_head(r: &Row) -> String {
    let mut out = format!("{} · {}", display_id(&r.session, r.entry_id), when(r.date));
    if r.failed {
        out.push_str(" · ✗ failed cmd");
    }
    out
}

/// `files: a, b   commits: 2ba5532`, empty when the turn has neither.
fn extras(r: &Row) -> String {
    let mut parts = Vec::new();
    if !r.files.is_empty() {
        parts.push(format!("files: {}", clip(&r.files.join(", "), FILES_CHARS)));
    }
    if !r.commits.is_empty() {
        let commits: Vec<&str> = r.commits.iter().take(MAX_COMMITS).map(|c| cap(c, 7)).collect();
        parts.push(format!("commits: {}", commits.join(" ")));
    }
    parts.join("   ")
}

/// The indented `you:` / `did:` / extras lines under a card head.
fn body(r: &Row, did: &str, l: Level) -> String {
    let mut out = format!("\n  you: {}", clip(&r.prompt, l.you));
    if l.did > 0 && !did.is_empty() {
        out += &format!("\n  did: {}", clip(did, l.did));
    }
    let extras = if l.extras { extras(r) } else { String::new() };
    if !extras.is_empty() {
        out += &format!("\n  {extras}");
    }
    out
}

fn card(n: usize, h: &Hit, all: bool, l: Level) -> String {
    let r = &h.row;
    let place = if h.other_project {
        format!(" · OTHER PROJECT {}", r.project_name)
    } else if all {
        format!(" · {}", r.project_name)
    } else {
        String::new()
    };
    format!("\n[{n}] {}{place}{}", turn_head(r), body(r, &h.did, l))
}

fn search_header(s: &Search, req: &Request, shown: usize) -> String {
    let mut parts = vec!["recall".to_string(), format!("{} ({})", s.scope, plural(s.scope_turns, "turn"))];
    if !req.query.trim().is_empty() {
        let q = clip(&req.query, ECHO_CHARS);
        parts.push(if q.contains('"') { q } else { format!(r#""{q}""#) });
    }
    for (label, value) in [("since", &req.since), ("until", &req.until), ("file", &req.file)] {
        if let Some(v) = value.as_deref().filter(|v| !v.trim().is_empty()) {
            parts.push(format!("{label} {}", clip(v, ECHO_CHARS)));
        }
    }
    if req.failed {
        parts.push("failed only".to_string());
    }
    let from = if req.offset > 0 { format!(" from [{}]", req.offset + 1) } else { String::new() };
    parts.push(format!("shown {shown} of {}{from}", thousands(s.total as i64)));
    parts.join(" · ")
}

fn search_notes(s: &Search, busy: bool) -> String {
    let mut out = String::new();
    if s.fell_back {
        out += &format!("\nno match in {}; searched the other projects", s.scope);
    }
    if busy {
        out += &format!("\n{BUSY_NOTE}");
    }
    out
}

fn none_found(s: &Search) -> &'static str {
    if s.total > 0 {
        "none on this page: lower the offset"
    } else if s.all || s.fell_back {
        "none found in any project: try fewer or other words, or a wider date range"
    } else {
        "none found: try --all, fewer or other words, or a wider date range"
    }
}

/// Header, notes, as many cards as fit `max_chars` (richest forms first)
/// and a `next:` footer.
pub fn search_text(s: &Search, req: &Request, busy: bool) -> String {
    let max = req.max_chars();
    let notes = search_notes(s, busy);
    let Some(first) = s.hits.first() else {
        return fit(&format!("{}{notes}\n{}", search_header(s, req, 0), none_found(s)), max);
    };
    let footer = format!(
        "\nnext: recall id={} around=2 for detail · resume: gray -r {}",
        display_id(&first.row.session, first.row.entry_id),
        first.row.session
    );
    let forms: Vec<[String; 3]> = s
        .hits
        .iter()
        .enumerate()
        .map(|(i, h)| CARD_LEVELS.map(|l| card(req.offset + i + 1, h, s.all, l)))
        .collect();
    let fixed = chars(&search_header(s, req, s.hits.len())) + chars(&notes) + chars(&footer);
    let mut budget = max.saturating_sub(fixed);
    // As many cards as fit in their floor form, then the richest form each
    // can afford while the rest still fit at their floor.
    let mut floor_rest = 0;
    let count = forms
        .iter()
        .take_while(|f| {
            floor_rest += chars(&f[2]);
            floor_rest <= budget
        })
        .count();
    let mut floor_rest: usize = forms[..count].iter().map(|f| chars(&f[2])).sum();
    let mut cards = String::new();
    for f in &forms[..count] {
        floor_rest -= chars(&f[2]);
        let c = f.iter().find(|c| chars(c) + floor_rest <= budget).unwrap_or(&f[2]);
        budget -= chars(c);
        cards.push_str(c);
    }
    fit(&format!("{}{notes}{cards}{footer}", search_header(s, req, count)), max)
}

/// The same data as objects; prompts cut to 500 chars.
pub fn search_json(s: &Search, req: &Request, busy: bool) -> String {
    let hits: Vec<Value> = s
        .hits
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let r = &h.row;
            json!({
                "n": req.offset + i + 1,
                "id": display_id(&r.session, r.entry_id),
                "session": r.session,
                "entry_id": r.entry_id,
                "project": r.project_name,
                "project_key": r.project_key,
                "cwd": r.cwd,
                "date": r.date,
                "time": when(r.date),
                "prompt": fit(&r.prompt, JSON_PROMPT_CHARS),
                "did": h.did,
                "files": r.files,
                "commits": r.commits,
                "failed": r.failed,
                "has_reply": r.has_reply,
                "score": h.score,
                "other_project": h.other_project,
            })
        })
        .collect();
    json!({
        "scope": s.scope,
        "scope_turns": s.scope_turns,
        "all": s.all,
        "query": req.query,
        "total": s.total,
        "offset": req.offset,
        "shown": hits.len(),
        "fell_back": s.fell_back,
        "busy": busy,
        "hits": hits,
    })
    .to_string()
}

fn neighbor(r: &Row, l: Level) -> String {
    format!("\n- {}{}", turn_head(r), body(r, &r.gist, l))
}

/// The target turn with `p` / `r` / `c` chars of prompt, reply and commands.
fn target(t: &Row, p: usize, r: usize, c: usize) -> String {
    let mut out = format!("\n▶ {}\nyou: {}", turn_head(t), fit(&t.prompt, p));
    if t.reply.is_empty() {
        out.push_str("\nreply: (none)");
    } else {
        out += &format!("\nreply:\n{}", fit(&t.reply, r));
    }
    if !t.commands.is_empty() {
        out += &format!("\ncommands:\n{}", fit(&t.commands, c));
    }
    let extras = extras(t);
    if !extras.is_empty() {
        out += &format!("\n{extras}");
    }
    out
}

/// The target turn in full and its neighbors, fitted to `max` chars: far
/// neighbors shrink, then drop; the target's prompt, reply and commands
/// share what is left.
pub fn show_text(sh: &Shown, max: usize, busy: bool) -> String {
    let Some(pos) = sh.turns.iter().position(|t| t.entry_id == sh.target) else {
        return fit(&format!("recall show {}:{} · not found", sh.session, sh.target), max);
    };
    let t = &sh.turns[pos];
    let mut out = format!("recall show {}:{} · project {} · {}", sh.session, sh.target, t.project_name, t.cwd);
    if busy {
        out += &format!("\n{BUSY_NOTE}");
    }
    let half = max / 2;
    let others: Vec<usize> = (0..sh.turns.len()).filter(|&i| i != pos).collect();
    let level = NEIGHBOR_LEVELS
        .into_iter()
        .find(|&l| others.iter().map(|&i| chars(&neighbor(&sh.turns[i], l))).sum::<usize>() <= half)
        .unwrap_or(NEIGHBOR_LEVELS[1]);
    let mut nearest = others.clone();
    nearest.sort_by_key(|&i| i.abs_diff(pos));
    let mut kept: Vec<Option<String>> = vec![None; sh.turns.len()];
    let mut used = 0;
    for i in nearest {
        let n = neighbor(&sh.turns[i], level);
        if used + chars(&n) > half {
            break;
        }
        used += chars(&n);
        kept[i] = Some(n);
    }
    let dropped = others.len() - kept.iter().flatten().count();
    let left_out =
        if dropped > 0 { format!("\n({} left out to fit max_chars)", plural(dropped as i64, "neighbor")) } else { String::new() };
    let footer = format!("{left_out}\nresume: gray -r {}", sh.session);
    let fixed = chars(&out) + used + chars(&target(t, 0, 0, 0)) + chars(&footer);
    let avail = max.saturating_sub(fixed);
    let (pn, rn, cn) = (chars(&t.prompt), chars(&t.reply), chars(&t.commands));
    let p = pn.min((avail / 4).max(avail.saturating_sub(rn + cn)));
    let rest = avail - p;
    let c = cn.min((rest / 3).max(rest.saturating_sub(rn)));
    let r = rn.min(rest - c);
    for (i, n) in kept.iter().enumerate() {
        if i == pos {
            out += &target(t, p, r, c);
        } else if let Some(n) = n {
            out += n;
        }
    }
    fit(&(out + &footer), max)
}

/// The target complete; neighbors as prompt (cut to 500 chars) and gist.
pub fn show_json(sh: &Shown, busy: bool) -> String {
    let turns: Vec<Value> = sh
        .turns
        .iter()
        .map(|t| {
            let is_target = t.entry_id == sh.target;
            let mut v = json!({
                "id": display_id(&t.session, t.entry_id),
                "entry_id": t.entry_id,
                "date": t.date,
                "time": when(t.date),
                "target": is_target,
                "failed": t.failed,
                "files": t.files,
                "commits": t.commits,
            });
            if is_target {
                v["prompt"] = json!(t.prompt);
                v["reply"] = json!(t.reply);
                v["commands"] = json!(t.commands);
                v["has_reply"] = json!(t.has_reply);
            } else {
                v["prompt"] = json!(fit(&t.prompt, JSON_PROMPT_CHARS));
                v["gist"] = json!(t.gist);
            }
            v
        })
        .collect();
    let t = sh.turns.iter().find(|t| t.entry_id == sh.target);
    json!({
        "session": sh.session,
        "target": sh.target,
        "project": t.map(|t| &t.project_name),
        "project_key": t.map(|t| &t.project_key),
        "cwd": t.map(|t| &t.cwd),
        "busy": busy,
        "turns": turns,
    })
    .to_string()
}

/// One aligned line per project, as many as fit `max` chars.
pub fn projects_text(ps: &[ProjectInfo], max: usize) -> String {
    let total: i64 = ps.iter().map(|p| p.turns).sum();
    let mut out = format!("recall · {} · {}", plural(ps.len() as i64, "project"), plural(total, "turn"));
    if ps.is_empty() {
        out.push_str("\nnone indexed yet");
        return fit(&out, max);
    }
    let w = ps.iter().map(|p| chars(&p.name)).max().unwrap_or(0).min(NAME_CHARS);
    let tw = ps.iter().map(|p| thousands(p.turns).len()).max().unwrap_or(0);
    let more = |n: usize| format!("\n… {n} more (--json lists all)");
    let reserve = chars(&more(ps.len()));
    for (i, p) in ps.iter().enumerate() {
        let line = format!("\n  {:<w$}  {:>tw$}  {}", clip(&p.name, w), thousands(p.turns), p.key);
        let after = if i + 1 < ps.len() { reserve } else { 0 };
        if chars(&out) + chars(&line) + after > max {
            out += &more(ps.len() - i);
            break;
        }
        out += &line;
    }
    fit(&out, max)
}

pub fn projects_json(ps: &[ProjectInfo]) -> String {
    let list: Vec<Value> = ps.iter().map(|p| json!({"name": p.name, "key": p.key, "turns": p.turns})).collect();
    Value::Array(list).to_string()
}

pub fn status_text(st: &Stats, index: &Path, now_ms: i64, busy: bool) -> String {
    let last = st.last_catch_up.map_or_else(|| "never".to_string(), |t| format!("{} ({})", when(t), ago(now_ms - t)));
    let mut out = format!(
        "recall index · {} · {} · {}\nsize {} · skipped lines {} · last catch-up {last}\nfile {}",
        plural(st.turns, "turn"),
        plural(st.sessions, "session"),
        plural(st.projects, "project"),
        size(st.bytes),
        thousands(st.skipped),
        index.display()
    );
    if busy {
        out += &format!("\n{BUSY_NOTE}");
    }
    out
}

pub fn status_json(st: &Stats, index: &Path, busy: bool) -> String {
    json!({
        "turns": st.turns,
        "sessions": st.sessions,
        "projects": st.projects,
        "skipped": st.skipped,
        "bytes": st.bytes,
        "last_catch_up": st.last_catch_up,
        "index": index.display().to_string(),
        "busy": busy,
    })
    .to_string()
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod render_tests;
~~~

- [ ] **Step 4: Run the render tests**

Run: `$X src/render.rs && CARGO_BUILD_JOBS=4 cargo test --lib render`
Expected: 16 passed.

- [ ] **Step 5: Format, lint, commit**

~~~bash
CARGO_BUILD_JOBS=4 cargo fmt && CARGO_BUILD_JOBS=4 cargo clippy --all-targets -- -D warnings && CARGO_BUILD_JOBS=4 cargo test
git add -A && git commit -m "Render: budgeted search cards, show, projects and status views"
~~~

---

### Task 9: CLI, sidecar and manifest

**Files:**
- Create: `src/cli.rs`, `src/cli_tests.rs`, `tests/protocol.rs`
- Modify: `src/lib.rs` (add `pub mod cli;`), `src/main.rs` (replace the placeholder)

**Interfaces:**
- Consumes: everything above; `catchup::{catch_up, catch_up_locked, writer_lock}`, `index::{open, reset, stats, db_path, list_projects}`, `query::{search, show, now_ms}`, the `render` views.
- Produces:
  - `cli::TOOL_NAME` (`recall`) and `cli::SUBCOMMANDS` (`search show projects status reindex`).
  - `cli::Cli` (clap) with `Command::{Search, Show, Projects, Status, Reindex}`.
  - `cli::Output { text, code }`.
  - `cli::normalize(&[String]) -> Vec<String>` and `cli::query_from(&[String]) -> String`.
  - `cli::run_argv(home, &[String], &Context) -> Output` and `cli::run_request(home, &Request, &Context) -> Output`.
  - `cli::request_from_json(&Value) -> Result<Request, RecallError>`.
  - `cli::tool_call(home, &Value, &Context) -> Value` (`{content, is_error}`).
  - `cli::wire_context(&Value) -> Context` and `cli::cli_context() -> Context`.
  - `cli::manifest() -> Value` and `cli::tool_def() -> Value`.

Rules (the spec's Surfaces section, made exact):
- Argv: `search` is put in front unless the first word is one of `SUBCOMMANDS` or `-h`, `--help`, `-V`, `--version`. Empty argv is a search with an empty query (the newest turns).
- Positional words are joined with spaces. A word containing whitespace (one shell-quoted or shlex-split argument) becomes a `"phrase"` unless it already contains `"` or a `-word`, so `'sso -cookie'` and `'"a b" c'` pass through as written.
- Clap errors exit 2; `--help` / `--version` exit 0 with their text. Request errors print `recall: <message>` with their code.
- Search, show and projects catch up first. While another process writes, they answer from the index as it is, with `BUSY_NOTE`. Status does not catch up; it reports the index as it is and whether the lock is held. Reindex takes the writer lock (busy: code 1, "try again"), resets, rebuilds and reports `reindexed N sessions (M turns) in Xs`.
- Tool JSON: unknown keys are ignored, and null or an empty string means absent. Numbers also accept numeric strings and whole floats; flags accept `"true"` / `"false"`; strings accept numbers (`since: 2026`). Anything else is code 2. A non-object `args` is code 2; null `args` is the empty request.
- Tool replies: `{content, is_error}`, where `is_error` is true only for codes 1-3 (never for zero hits). `command/run` replies `{text}` with the same text as the CLI. An unknown tool name is `is_error`.
- Context: on the wire, `session.cwd` (else `cwd`, else the process cwd) and `session.id` (empty is none). In CLI mode, the process cwd and `GRAY_SESSION_ID`.
- Binary: `manifest` alone prints the manifest. No arguments with stdin not a terminal runs the NDJSON loop; no arguments on a terminal prints help. Anything else runs the CLI: stdout on code 0, stderr otherwise, exit with the code.
- Loop: `plugin/shutdown` ends it. Lines that are not JSON, lines without an `id` (notifications) and unclaimed methods get no reply. The reply echoes the request's `id` (number or string).

- [ ] **Step 1: Write the failing CLI tests**

~~~rust file=src/cli_tests.rs
use super::*;
use crate::catchup::{sessions_dir, writer_lock};
use crate::render::BUSY_NOTE;
use crate::testkit::{SessionBuilder, T0};
use serde_json::{Value, json};
use std::path::Path;

const SESSION: &str = "1900bae3-0000-4000-8000-000000000000";

fn home_with(sessions: &[&SessionBuilder]) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    for s in sessions {
        s.write(&sessions_dir(home.path()));
    }
    home
}

/// Turn 1 (failed `cargo test`) and turn 5 in `/work/gray`.
fn gray_session() -> SessionBuilder {
    let mut b = SessionBuilder::new(SESSION, "/work/gray");
    b.user("fix the prefix cache warmup");
    b.bash("cargo test", "exit 101 · 2.0s");
    b.assistant("The prefix cache now warms before compaction.");
    b.user("deploy the site");
    b.assistant("Deployed.");
    b
}

fn ctx() -> Context {
    Context { cwd: "/work/gray".into(), session: None, now_ms: T0 + 86_400_000 }
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn run(home: &Path, words: &[&str]) -> Output {
    run_argv(home, &argv(words), &ctx())
}

#[test]
fn the_first_word_picks_a_subcommand_only_when_it_is_one() {
    let n = |w: &[&str]| normalize(&argv(w));
    assert_eq!(n(&["deploy", "site"]), ["search", "deploy", "site"]);
    assert_eq!(n(&["show", "x:1"]), ["show", "x:1"]);
    for w in ["search", "projects", "status", "reindex", "--help", "-h", "--version", "-V"] {
        assert_eq!(n(&[w])[0], w);
    }
    assert_eq!(n(&["help"]), ["search", "help"]);
    assert_eq!(n(&["--all", "x"]), ["search", "--all", "x"]);
    assert_eq!(n(&[]), ["search"]);
}

#[test]
fn arguments_with_spaces_become_phrases_unless_already_quoted_or_excluding() {
    let q = |w: &[&str]| query_from(&argv(w));
    assert_eq!(q(&["prefix", "cache"]), "prefix cache");
    assert_eq!(q(&["prefix cache", "warm"]), r#""prefix cache" warm"#);
    assert_eq!(q(&["sso -cookie"]), "sso -cookie");
    assert_eq!(q(&[r#""a b" c"#]), r#""a b" c"#);
    assert_eq!(q(&["  spaced  out "]), r#""spaced  out""#);
    assert_eq!(q(&[]), "");
}

#[test]
fn json_arguments_accept_numeric_strings_and_reject_wrong_types() {
    let args = json!({"query": "cache", "limit": "3", "offset": 2, "all": true, "since": 2026, "id": null, "until": "", "extra": 1});
    let want = Request {
        query: "cache".into(),
        limit: Some(3),
        offset: 2,
        all: true,
        since: Some("2026".into()),
        ..Request::default()
    };
    assert_eq!(request_from_json(&args).unwrap(), want);
    assert_eq!(request_from_json(&Value::Null).unwrap(), Request::default());
    assert!(request_from_json(&json!({"failed": "true"})).unwrap().failed);
    assert_eq!(request_from_json(&json!({"limit": 3.0})).unwrap().limit, Some(3));
    assert_eq!(request_from_json(&json!({"id": "abc:1", "around": 2})).unwrap().around, 2);
    let bad = [
        json!({"limit": -1}),
        json!({"limit": "many"}),
        json!({"limit": 1.5}),
        json!({"query": ["a"]}),
        json!({"all": "yes"}),
        json!({"project": {}}),
        json!("cache"),
    ];
    for args in bad {
        assert_eq!(request_from_json(&args).unwrap_err().code, 2, "{args}");
    }
}

#[test]
fn search_show_projects_and_status_run_from_argv() {
    let home = home_with(&[&gray_session()]);
    let out = run(home.path(), &["prefix", "cache"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(out.text.starts_with(r#"recall · project gray (2 turns) · "prefix cache" · shown 1 of 1"#), "{}", out.text);
    assert!(out.text.contains("✗ failed cmd") && out.text.contains("next: recall id=1900bae3:1 around=2"), "{}", out.text);
    let out = run(home.path(), &["show", "1900bae3:1", "--around", "1"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(out.text.contains("\n▶ 1900bae3:1 · ") && out.text.contains("\n- 1900bae3:5 · "), "{}", out.text);
    assert!(out.text.contains("\ncommands:\ncargo test"), "{}", out.text);
    let out = run(home.path(), &["projects"]);
    assert!(out.code == 0 && out.text.starts_with("recall · 1 project · 2 turns\n  gray  2  "), "{}", out.text);
    let out = run(home.path(), &["status"]);
    assert!(out.code == 0 && out.text.starts_with("recall index · 2 turns · 1 session · 1 project\n"), "{}", out.text);
    let v: Value = serde_json::from_str(&run(home.path(), &["deploy", "--json"]).text).unwrap();
    assert_eq!(v["hits"][0]["id"], "1900bae3:5");
    let v: Value = serde_json::from_str(&run(home.path(), &["status", "--json"]).text).unwrap();
    assert_eq!(v["turns"], 2);
}

#[test]
fn errors_carry_their_exit_codes() {
    let home = home_with(&[&gray_session()]);
    let cases: [(&[&str], i32, &str); 6] = [
        (&["x", "--since", "soon"], 2, "recall: bad date"),
        (&["x", "-p", "nope"], 3, "recall: no project matches 'nope'"),
        (&["show", "zzz:1"], 3, "recall: no session matches"),
        (&["show", "nocolon"], 2, "recall: bad id"),
        (&["x", "--limit", "many"], 2, "invalid value"),
        (&["sso", "-cookie"], 2, "unexpected argument"),
    ];
    for (words, code, needle) in cases {
        let out = run(home.path(), words);
        assert_eq!(out.code, code, "{words:?}: {}", out.text);
        assert!(out.text.contains(needle), "{words:?}: {}", out.text);
    }
    let out = run(home.path(), &["zzzz"]);
    assert!(out.code == 0 && out.text.contains("none found"), "{}", out.text);
    let help = run(home.path(), &["--help"]);
    assert!(help.code == 0 && help.text.contains("Usage"), "{}", help.text);
}

#[test]
fn the_tool_reports_errors_but_not_empty_results() {
    let home = home_with(&[&gray_session()]);
    let t = |args: Value| tool_call(home.path(), &args, &ctx());
    let ok = t(json!({"query": "deploy"}));
    assert_eq!(ok["is_error"], false);
    assert!(ok["content"].as_str().unwrap().contains("you: deploy the site"), "{ok}");
    assert_eq!(t(json!({"query": "zzzz"}))["is_error"], false);
    assert_eq!(t(json!({}))["is_error"], false);
    assert_eq!(t(Value::Null)["is_error"], false);
    let bad = t(json!({"since": "soon"}));
    assert!(bad["is_error"] == true && bad["content"].as_str().unwrap().starts_with("recall: bad date"), "{bad}");
    assert_eq!(t(json!({"limit": "x"}))["is_error"], true);
    let shown = t(json!({"id": "1900bae3:5"}));
    assert!(shown["content"].as_str().unwrap().contains("▶ 1900bae3:5"), "{shown}");
}

#[test]
fn reindex_rebuilds_and_a_held_lock_is_reported() {
    let home = home_with(&[&gray_session()]);
    assert_eq!(run(home.path(), &["deploy"]).code, 0);
    let out = run(home.path(), &["reindex"]);
    assert!(out.code == 0 && out.text.starts_with("reindexed 1 session (2 turns) in "), "{}", out.text);
    let _held = writer_lock(home.path()).unwrap().unwrap();
    let out = run(home.path(), &["reindex"]);
    assert!(out.code == 1 && out.text.contains("try again"), "{}", out.text);
    let out = run(home.path(), &["deploy"]);
    assert!(out.code == 0 && out.text.contains(BUSY_NOTE) && out.text.contains("deploy the site"), "{}", out.text);
    let out = run(home.path(), &["status"]);
    assert!(out.text.ends_with(BUSY_NOTE), "{}", out.text);
}

#[test]
fn the_manifest_declares_the_tool_and_command() {
    let m = manifest();
    assert_eq!((m["name"].as_str(), m["protocol"].as_str()), (Some("recall"), Some("1.1")));
    assert_eq!((m["commands"].clone(), m["hooks"].clone()), (json!(["/recall"]), json!([])));
    assert_eq!(m["completion"], json!(SUBCOMMANDS));
    let tool = &m["tools"][0];
    assert_eq!((tool["name"].as_str(), tool["parameters"]["type"].as_str()), (Some("recall"), Some("object")));
    assert!(tool["description"].as_str().unwrap().len() > 80 && tool["snippet"].is_string());
    let props = tool["parameters"]["properties"].as_object().unwrap();
    let fields = ["query", "project", "all", "file", "since", "until", "failed", "limit", "offset", "id", "around", "json", "max_chars"];
    assert_eq!(props.len(), fields.len());
    for k in fields {
        assert!(props[k]["description"].is_string(), "{k}");
    }
}

#[test]
fn wire_context_reads_the_session_and_falls_back_to_the_process_cwd() {
    let c = wire_context(&json!({"session": {"id": "abc", "cwd": "/w/x"}}));
    assert_eq!((c.cwd.as_str(), c.session.as_deref()), ("/w/x", Some("abc")));
    let c = wire_context(&json!({"cwd": "/w/y", "session": {"id": "", "cwd": ""}}));
    assert_eq!((c.cwd.as_str(), c.session), ("/w/y", None));
    let c = wire_context(&Value::Null);
    assert_eq!(c.cwd, std::env::current_dir().unwrap().to_string_lossy());
    assert!(c.now_ms > T0);
}
~~~

~~~bash
$X src/cli_tests.rs
sed -i 's/^pub mod catchup;/pub mod catchup;\npub mod cli;/' src/lib.rs
printf '#[cfg(test)]\n#[path = "cli_tests.rs"]\nmod cli_tests;\n' > src/cli.rs
~~~

- [ ] **Step 2: Run the tests to see them fail**

Run: `CARGO_BUILD_JOBS=4 cargo test --lib cli`
Expected: compile errors (`cannot find function run_argv`, `cannot find type Output`).

- [ ] **Step 3: Implement `cli.rs`**

~~~rust file=src/cli.rs
//! One argument surface: `gray recall` argv, `/recall` argv and the tool's
//! JSON all become a [`Request`] and run through [`run_request`].

use crate::query::{self, Context, RecallError, Request};
use crate::{COMMANDS, PLUGIN_NAME, PROTOCOL, catchup, index, render};
use clap::{Args, Parser, Subcommand};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::path::Path;
use std::time::Instant;

pub const TOOL_NAME: &str = "recall";
/// Words that pick a subcommand when they come first.
pub const SUBCOMMANDS: &[&str] = &["search", "show", "projects", "status", "reindex"];
const PASS_THROUGH: &[&str] = &["-h", "--help", "-V", "--version"];
const TOOL_DESCRIPTION: &str = "Search this machine's past gray sessions for prior work: how a bug was fixed before, \
    what was decided and why, what was done in a project, the history of a file. Returns short cards (the prompt, \
    what was done, files, commits), current project first; pass `id` from a card to read that turn in full.";

#[derive(Parser, Debug)]
#[command(name = "gray recall", version, about = "Search past gray sessions for prior work", disable_help_subcommand = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Search turns (the default when the first word is not a subcommand)
    Search(SearchArgs),
    /// Show one turn in full, by the id printed on a card
    Show(ShowArgs),
    /// List the indexed projects
    Projects(ListArgs),
    /// Index counts, size and last catch-up
    Status(StatusArgs),
    /// Delete the index and rebuild it from the session files
    Reindex,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Words (any may match), "exact phrase" (required), 'word -excluded' (quoted)
    pub words: Vec<String>,
    /// Project name or path (default: the current project)
    #[arg(short, long)]
    pub project: Option<String>,
    /// Search every project
    #[arg(short, long)]
    pub all: bool,
    /// Only turns that touched a path containing this
    #[arg(short, long)]
    pub file: Option<String>,
    /// YYYY, YYYY-MM, YYYY-MM-DD, or Nd / Nw / Nm ago
    #[arg(long)]
    pub since: Option<String>,
    /// Same formats as --since; inclusive
    #[arg(long)]
    pub until: Option<String>,
    /// Only turns with a failed command
    #[arg(long)]
    pub failed: bool,
    /// Results to show (default 5, max 20)
    #[arg(short = 'n', long)]
    pub limit: Option<usize>,
    /// Skip this many results
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
    /// Print JSON
    #[arg(long)]
    pub json: bool,
    /// Output budget in characters (default 2500)
    #[arg(long)]
    pub max_chars: Option<usize>,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// `<session>:<entry>`; a unique session prefix is enough
    pub id: String,
    /// Also show this many turns before and after (max 10)
    #[arg(long, default_value_t = 0)]
    pub around: usize,
    /// Print JSON
    #[arg(long)]
    pub json: bool,
    /// Output budget in characters (default 8000)
    #[arg(long)]
    pub max_chars: Option<usize>,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Print JSON
    #[arg(long)]
    pub json: bool,
    /// Output budget in characters (default 2500)
    #[arg(long)]
    pub max_chars: Option<usize>,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Print JSON
    #[arg(long)]
    pub json: bool,
}

/// Text for stdout (code 0) or stderr, and the exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub text: String,
    pub code: i32,
}

impl Output {
    fn from_result(r: Result<String, RecallError>) -> Self {
        match r {
            Ok(text) => Self { text, code: 0 },
            Err(e) => Self { text: format!("recall: {}", e.message), code: e.code },
        }
    }
}

/// `search` in front unless the first word is a subcommand or a help flag.
pub fn normalize(argv: &[String]) -> Vec<String> {
    match argv.first() {
        Some(w) if SUBCOMMANDS.contains(&w.as_str()) || PASS_THROUGH.contains(&w.as_str()) => argv.to_vec(),
        _ => std::iter::once("search".to_string()).chain(argv.iter().cloned()).collect(),
    }
}

/// Positional words as one query; a word with spaces is a phrase unless it
/// already holds quotes or exclusions.
pub fn query_from(words: &[String]) -> String {
    let terms: Vec<String> = words
        .iter()
        .map(|w| {
            let w = w.trim();
            let excludes = w.split_whitespace().any(|t| t.len() > 1 && t.starts_with('-'));
            if w.contains(char::is_whitespace) && !w.contains('"') && !excludes {
                format!("\"{w}\"")
            } else {
                w.to_string()
            }
        })
        .collect();
    terms.join(" ")
}

/// Run `gray recall` / `/recall` arguments.
pub fn run_argv(home: &Path, argv: &[String], ctx: &Context) -> Output {
    let cli = match Cli::try_parse_from(std::iter::once("gray recall".to_string()).chain(normalize(argv))) {
        Ok(cli) => cli,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            return Output { text: e.to_string().trim_end().to_string(), code };
        }
    };
    match cli.command {
        Command::Search(a) => {
            let req = Request {
                query: query_from(&a.words),
                project: a.project,
                all: a.all,
                file: a.file,
                since: a.since,
                until: a.until,
                failed: a.failed,
                limit: a.limit,
                offset: a.offset,
                json: a.json,
                max_chars: a.max_chars,
                ..Request::default()
            };
            run_request(home, &req, ctx)
        }
        Command::Show(a) => {
            let req =
                Request { id: Some(a.id), around: a.around, json: a.json, max_chars: a.max_chars, ..Request::default() };
            run_request(home, &req, ctx)
        }
        Command::Projects(a) => Output::from_result(projects(home, &a)),
        Command::Status(a) => Output::from_result(status(home, a.json, ctx.now_ms)),
        Command::Reindex => Output::from_result(reindex(home)),
    }
}

/// Catch up, then search or show.
pub fn run_request(home: &Path, req: &Request, ctx: &Context) -> Output {
    Output::from_result(answer(home, req, ctx))
}

/// `tool/call` for `recall`: `{content, is_error}`; zero hits are not an error.
pub fn tool_call(home: &Path, args: &Value, ctx: &Context) -> Value {
    let out = Output::from_result(request_from_json(args).and_then(|req| answer(home, &req, ctx)));
    json!({"content": out.text, "is_error": out.code != 0})
}

/// The index, caught up; `true` when another process holds the writer lock.
fn open_fresh(home: &Path) -> Result<(Connection, bool), RecallError> {
    let mut conn = index::open(home)?;
    let report = catchup::catch_up(&mut conn, home)?;
    Ok((conn, report.busy))
}

fn answer(home: &Path, req: &Request, ctx: &Context) -> Result<String, RecallError> {
    let (conn, busy) = open_fresh(home)?;
    if let Some(id) = &req.id {
        let shown = query::show(&conn, id, req.around())?;
        return Ok(if req.json { render::show_json(&shown, busy) } else { render::show_text(&shown, req.max_chars(), busy) });
    }
    let found = query::search(&conn, req, ctx)?;
    Ok(if req.json { render::search_json(&found, req, busy) } else { render::search_text(&found, req, busy) })
}

fn projects(home: &Path, a: &ListArgs) -> Result<String, RecallError> {
    let (conn, _) = open_fresh(home)?;
    let list = index::list_projects(&conn)?;
    let max = Request { max_chars: a.max_chars, ..Request::default() }.max_chars();
    Ok(if a.json { render::projects_json(&list) } else { render::projects_text(&list, max) })
}

/// The index as it is, without catching up: a diagnostic, not a refresh.
fn status(home: &Path, json: bool, now_ms: i64) -> Result<String, RecallError> {
    let conn = index::open(home)?;
    let busy = catchup::writer_lock(home)?.is_none();
    let st = index::stats(&conn, home)?;
    let db = index::db_path(home);
    Ok(if json { render::status_json(&st, &db, busy) } else { render::status_text(&st, &db, now_ms, busy) })
}

fn reindex(home: &Path) -> Result<String, RecallError> {
    let mut conn = index::open(home)?;
    let Some(_lock) = catchup::writer_lock(home)? else {
        return Err(RecallError::error("another session is catching up the index; try again in a moment"));
    };
    let started = Instant::now();
    index::reset(&conn)?;
    let report = catchup::catch_up_locked(&mut conn, home)?;
    let st = index::stats(&conn, home)?;
    let failed = if report.failed > 0 {
        format!(" · {} unreadable", render::plural(report.failed as i64, "file"))
    } else {
        String::new()
    };
    Ok(format!(
        "reindexed {} ({}) in {:.1}s{failed}",
        render::plural(report.changed as i64, "session"),
        render::plural(st.turns, "turn"),
        started.elapsed().as_secs_f64()
    ))
}

/// The tool's JSON arguments as a [`Request`].
pub fn request_from_json(args: &Value) -> Result<Request, RecallError> {
    let empty = Map::new();
    let obj = match args {
        Value::Object(m) => m,
        Value::Null => &empty,
        _ => return Err(RecallError::bad("tool arguments must be an object")),
    };
    Ok(Request {
        query: text(obj, "query")?.unwrap_or_default(),
        project: text(obj, "project")?,
        all: flag(obj, "all")?,
        file: text(obj, "file")?,
        since: text(obj, "since")?,
        until: text(obj, "until")?,
        failed: flag(obj, "failed")?,
        limit: number(obj, "limit")?,
        offset: number(obj, "offset")?.unwrap_or(0),
        id: text(obj, "id")?,
        around: number(obj, "around")?.unwrap_or(0),
        json: flag(obj, "json")?,
        max_chars: number(obj, "max_chars")?,
    })
}

fn text(obj: &Map<String, Value>, key: &str) -> Result<Option<String>, RecallError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone()).filter(|s| !s.is_empty())),
        Some(Value::Number(n)) => Ok(Some(n.to_string())),
        Some(_) => Err(RecallError::bad(format!("{key} must be a string"))),
    }
}

fn flag(obj: &Map<String, Value>, key: &str) -> Result<bool, RecallError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::String(s)) if s == "true" || s == "false" => Ok(s == "true"),
        Some(_) => Err(RecallError::bad(format!("{key} must be true or false"))),
    }
}

fn number(obj: &Map<String, Value>, key: &str) -> Result<Option<usize>, RecallError> {
    let bad = || RecallError::bad(format!("{key} must be a non-negative integer"));
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => s.trim().parse().map(Some).map_err(|_| bad()),
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && *f >= 0.0).map(|f| f as u64))
            .map(|v| Some(v as usize))
            .ok_or_else(bad),
        Some(_) => Err(bad()),
    }
}

/// Who is calling, from a wire request's params.
pub fn wire_context(params: &Value) -> Context {
    let field = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    Context {
        cwd: field(&params["session"]["cwd"]).or_else(|| field(&params["cwd"])).unwrap_or_else(process_cwd),
        session: field(&params["session"]["id"]),
        now_ms: query::now_ms(),
    }
}

/// Who is calling in CLI mode: the process cwd and `GRAY_SESSION_ID`.
pub fn cli_context() -> Context {
    Context {
        cwd: process_cwd(),
        session: std::env::var("GRAY_SESSION_ID").ok().filter(|s| !s.is_empty()),
        now_ms: query::now_ms(),
    }
}

fn process_cwd() -> String {
    std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn tool_def() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": TOOL_DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Words (any may match; more matches rank higher), \"exact phrase\" (required), -word (excluded). Empty lists the newest turns."},
                "project": {"type": "string", "description": "Project name or path. Default: the current project, widening to the others when nothing matches."},
                "all": {"type": "boolean", "description": "Search every project."},
                "file": {"type": "string", "description": "Only turns that touched a file path containing this."},
                "since": {"type": "string", "description": "YYYY, YYYY-MM, YYYY-MM-DD, or Nd / Nw / Nm ago."},
                "until": {"type": "string", "description": "Same formats as since; inclusive."},
                "failed": {"type": "boolean", "description": "Only turns where a command failed."},
                "limit": {"type": "integer", "description": "Results to show: default 5, max 20."},
                "offset": {"type": "integer", "description": "Skip this many results (paging)."},
                "id": {"type": "string", "description": "Show this turn in full (<session>:<entry> from a card) instead of searching."},
                "around": {"type": "integer", "description": "With id: also show this many turns before and after (max 10)."},
                "json": {"type": "boolean", "description": "Return JSON instead of text."},
                "max_chars": {"type": "integer", "description": "Output budget: default 2500 for search, 8000 with id."}
            }
        },
        "snippet": "recall <words> [project] [since]",
    })
}

pub fn manifest() -> Value {
    json!({
        "name": PLUGIN_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL,
        "tools": [tool_def()],
        "commands": COMMANDS,
        "hooks": [],
        "completion": SUBCOMMANDS,
    })
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod cli_tests;
~~~

- [ ] **Step 4: Run the CLI tests**

Run: `$X src/cli.rs && CARGO_BUILD_JOBS=4 cargo test --lib cli`
Expected: 9 passed.

- [ ] **Step 5: Write the failing protocol test**

~~~rust file=tests/protocol.rs
//! The built binary: the sidecar wire and the CLI exit codes.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_gray-recall");
const SESSION: &str = "1900bae3-0000-4000-8000-000000000000";

/// A home with one session in `/work/gray`: "deploy the site".
fn home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let t0: i64 = 1_790_000_000_000;
    let message = |role: &str, text: &str| json!({"role": role, "content": [{"type": "text", "text": text}]});
    let lines = [
        json!({"version": 1, "id": SESSION, "timestamp": t0, "cwd": "/work/gray", "model": "test"}),
        json!({"entry_id": 1, "parent_id": null, "timestamp": t0 + 1000, "message": message("user", "deploy the site")}),
        json!({"entry_id": 2, "parent_id": 1, "timestamp": t0 + 2000, "message": message("assistant", "Deployed to pages.")}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(dir.join(format!("{SESSION}.jsonl")), text).unwrap();
    home
}

fn cli(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .env("GRAY_HOME", home)
        .env_remove("GRAY_SESSION_ID")
        .current_dir(home)
        .output()
        .unwrap()
}

#[test]
fn the_cli_answers_manifest_search_and_exit_codes() {
    let home = home();
    let m: Value = serde_json::from_slice(&cli(home.path(), &["manifest"]).stdout).unwrap();
    assert_eq!((m["name"].as_str(), m["tools"][0]["name"].as_str()), (Some("recall"), Some("recall")));
    let out = cli(home.path(), &["deploy", "-p", "gray"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("you: deploy the site") && text.ends_with('\n'), "{text}");
    let out = cli(home.path(), &["deploy", "--since", "soon"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty() && String::from_utf8_lossy(&out.stderr).contains("recall: bad date"));
    assert_eq!(cli(home.path(), &["deploy", "-p", "nope"]).status.code(), Some(3));
    let out = cli(home.path(), &["zzzz", "--all"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("none found"));
}

#[test]
fn the_sidecar_serves_the_manifest_tool_and_command_then_shuts_down() {
    let home = home();
    let mut child = Command::new(BIN)
        .env("GRAY_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let session = json!({"id": "other-session", "cwd": "/work/gray"});
    let requests = [
        json!({"id": 1, "method": "plugin/manifest"}),
        json!({"method": "event/notify", "params": {"type": "turn_end"}}),
        json!({"id": 2, "method": "prompt/context", "params": {}}),
        json!({"id": 3, "method": "tool/call", "params": {"name": "recall", "args": {"query": "deploy"}, "session": session}}),
        json!({"id": "s4", "method": "tool/call", "params": {"name": "recall", "args": {"since": "soon"}, "session": session}}),
        json!({"id": 5, "method": "tool/call", "params": {"name": "nope", "args": {}}}),
        json!({"id": 6, "method": "command/run", "params": {"name": "/recall", "argv": ["deploy"], "session": session}}),
    ];
    for r in &requests {
        writeln!(stdin, "{r}").unwrap();
    }
    writeln!(stdin, "not json").unwrap();
    let mut next = || serde_json::from_str::<Value>(&lines.next().unwrap().unwrap()).unwrap();
    let m = next();
    assert_eq!((m["id"].as_i64(), m["result"]["name"].as_str()), (Some(1), Some("recall")));
    let hit = next();
    assert_eq!((hit["id"].as_i64(), hit["result"]["is_error"].as_bool()), (Some(3), Some(false)));
    assert!(hit["result"]["content"].as_str().unwrap().contains("you: deploy the site"), "{hit}");
    let bad = next();
    assert_eq!((bad["id"].as_str(), bad["result"]["is_error"].as_bool()), (Some("s4"), Some(true)));
    let unknown = next();
    assert_eq!((unknown["id"].as_i64(), unknown["result"]["is_error"].as_bool()), (Some(5), Some(true)));
    let cmd = next();
    assert_eq!(cmd["id"].as_i64(), Some(6));
    assert!(cmd["result"]["text"].as_str().unwrap().contains("you: deploy the site"), "{cmd}");
    writeln!(stdin, "{}", json!({"method": "plugin/shutdown", "params": {"reason": "session_end"}})).unwrap();
    assert!(child.wait().unwrap().success());
    assert!(lines.next().is_none(), "no reply to unclaimed methods, notifications or bad lines");
}
~~~

~~~bash
$X tests/protocol.rs
~~~

Run: `CARGO_BUILD_JOBS=4 cargo test --test protocol`
Expected: both fail. `manifest` prints nothing, and the sidecar exits at once (`called Option::unwrap() on a None value`).

- [ ] **Step 6: Implement `main.rs`**

~~~rust file=src/main.rs
//! `gray-recall`, sidecar and CLI in one binary:
//!
//! - `gray-recall manifest`: the manifest on stdout (the install probe).
//! - `gray-recall <args>`: the `gray recall` CLI (the host forwards here).
//! - No arguments, stdin not a terminal: the wire v1.1 NDJSON loop.

use gray_recall::cli;
use serde_json::{Value, json};
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

/// One request's result, or `None` for methods this plugin does not claim.
fn handle(home: &Path, method: &str, params: &Value) -> Option<Value> {
    Some(match method {
        "plugin/manifest" => cli::manifest(),
        "tool/call" => match params["name"].as_str().unwrap_or("") {
            cli::TOOL_NAME => cli::tool_call(home, &params["args"], &cli::wire_context(params)),
            other => json!({"content": format!("recall: unknown tool '{other}'"), "is_error": true}),
        },
        "command/run" => {
            let argv: Vec<String> = params["argv"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            json!({"text": cli::run_argv(home, &argv, &cli::wire_context(params)).text})
        }
        _ => return None,
    })
}

fn sidecar_loop(home: &Path) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let method = v["method"].as_str().unwrap_or("");
        if method == "plugin/shutdown" {
            break;
        }
        // Notifications carry no id and get no reply.
        let id = &v["id"];
        if id.is_null() {
            continue;
        }
        let Some(result) = handle(home, method, &v["params"]) else { continue };
        let mut out = stdout.lock();
        writeln!(out, "{}", json!({"id": id, "result": result}))?;
        out.flush()?;
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["manifest"] {
        println!("{}", cli::manifest());
        return;
    }
    let home = match gray_recall::gray_home() {
        Ok(home) => home,
        Err(e) => {
            eprintln!("recall: {e:#}");
            std::process::exit(1);
        }
    };
    if args.is_empty() && !std::io::stdin().is_terminal() {
        if let Err(e) = sidecar_loop(&home) {
            eprintln!("recall: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    let args = if args.is_empty() { vec!["--help".to_string()] } else { args };
    let out = cli::run_argv(&home, &args, &cli::cli_context());
    if out.code == 0 {
        println!("{}", out.text);
    } else {
        eprintln!("{}", out.text);
    }
    std::process::exit(out.code);
}
~~~

- [ ] **Step 7: Run everything**

Run: `$X src/main.rs && CARGO_BUILD_JOBS=4 cargo test`
Expected: 117 lib tests and 2 protocol tests pass.

- [ ] **Step 8: Format, lint, commit**

~~~bash
CARGO_BUILD_JOBS=4 cargo fmt && CARGO_BUILD_JOBS=4 cargo clippy --all-targets -- -D warnings && CARGO_BUILD_JOBS=4 cargo test
git add -A && git commit -m "CLI, sidecar loop and manifest: one request surface for the tool, /recall and gray recall"
~~~

---

### Task 10: README, real-data run, install

**Files:**
- Create: `README.md`

**Interfaces:** none new. This task checks the built plugin against the real `~/.gray/sessions`, gray's conformance check and a live agent.

Rules:
- The real-data run uses a throwaway `GRAY_HOME` whose `sessions` is a symlink to `~/.gray/sessions`, so timings start from an empty index and the real `~/.gray/recall` is not touched until install.
- Secret checks print counts only, never matches (the operator streams this terminal).
- The README never names the project whose design ideas are credited; that is `THIRD_PARTY_NOTICES.md`'s job.

- [ ] **Step 1: Write the README**

~~~markdown file=README.md
# gray-recall

Search past [gray](https://github.com/vstaln/gray) sessions for prior work,
as a sidecar plugin (wire v1.1). Ask "has this come up before", "what fixed
it", "what did we do to this file" and get a few ranked, cited turns in
about 500 tokens.

One binary, three surfaces, one argument syntax:

- **Tool `recall`**: the agent calls it when it wants history. Nothing is
  injected into prompts.
- **`/recall …`** inside a session, over `command/run`.
- **`gray recall …`** on the command line (`cli_argv` forwarding: the host
  `exec`s this binary, so it works with no agent running).

```sh
gray recall search prefix cache warmup     # any word may match; more matches rank higher
gray recall search '"prefix cache"' warm   # an "exact phrase" is required
gray recall search 'sso -cookie'           # -word excludes; keep it inside one quoted argument
gray recall search deploy --all --since 2w --failed -n 10
gray recall search -f src/render.rs        # turns that touched a path containing this
gray recall show 1900bae3:3154 --around 2  # one turn in full, by the id on a card
gray recall projects                       # indexed projects
gray recall status                         # counts, size, last catch-up
gray recall reindex                        # delete the index and rebuild it
```

Start command-line searches with `search`. Without it the first word is
still a search unless it is a subcommand name, but two cases differ:
`gray recall manifest` prints the plugin manifest (the install probe), and
bare `gray recall` with stdin not a terminal runs the sidecar loop instead
of listing the newest turns. `/recall` and the tool have neither quirk.

One argument with spaces is a phrase: `gray recall search "prefix cache"`
matches the two words together, because the shell (and `/recall`) pass it
as one word. Add `-word` inside it to make it a word list with an
exclusion instead.

## Scope and output

- Default scope is the current git repo (a worktree counts as its main
  repo). When it has no hits, the cards come from the other projects and
  are labeled `OTHER PROJECT`. `-p <name or path>` picks one project and is
  never widened; `--all` searches everything. The calling session's own
  turns are left out.
- Text output fits `--max-chars` (default 2500 for search, 8000 for
  `show`), and the header's `shown N of M` always matches the cards printed.
  `--json` ignores `--max-chars`; its fields are capped one by one instead.
- Exit codes: 0 ok, including zero hits; 1 error; 2 bad request (bad date,
  bad query, bad flag); 3 unknown or ambiguous project or id. The tool sets
  `is_error` only for 1-3.

## Index

`$GRAY_HOME/recall/index.db` (`~/.gray` by default; directory 0700, file
0600). Every search, `show` and `projects` first catches it up: only session
files that changed are read, and only the new bytes when a file grew. While
another session is catching up, the answer comes from the index as it is,
with `(index catching up in another session)` in the header. `status` reports
the index without catching up.

Secrets (API keys, tokens, `KEY=value` assignments, Discord bot tokens) are
redacted before anything is written; file paths are kept. Raw command
output is not indexed. No network, no `host/*` capabilities, no writes
outside `$GRAY_HOME/recall`.

## Install

```sh
cargo install --path . --locked
gray plugin check ~/.cargo/bin/gray-recall
gray plugin install ~/.cargo/bin/gray-recall
```

`manifest` answers the install probe, so one install wires the sidecar,
`/recall` and `gray recall`.

## License

MIT. Credits for design ideas and vendored code are in
`THIRD_PARTY_NOTICES.md`.
~~~

~~~bash
$X README.md
! grep -n -i leviathan README.md src/*.rs tests/*.rs Cargo.toml
~~~

- [ ] **Step 2: Build release and time a full build and a no-op catch-up**

~~~bash
CARGO_BUILD_JOBS=4 cargo build --release
B=$PWD/target/release/gray-recall
H=$(mktemp -d) && ln -s ~/.gray/sessions "$H/sessions"
ms() { s=$(date +%s%N); "$@" >/dev/null 2>&1; echo "$(( ($(date +%s%N) - s) / 1000000 )) ms"; }
GRAY_HOME=$H $B reindex
for i in 1 2 3; do GRAY_HOME=$H ms $B projects; done
GRAY_HOME=$H $B status
~~~

Expected: `reindexed N sessions (M turns) in Xs` with X under 3.0 (target: full build under 3 s); each `projects` under 50 ms (target: no-op catch-up under 50 ms, here with a small query on top); `status` shows the counts, skipped lines and index size.

Measured while writing this plan (4-core laptop, load average 7.5): full build 2.6-3.5 s, no-op 23-32 ms. The single writer is the critical path: FTS5 tokenizing about 9 MB of text takes 1.3-2.4 s of the pipeline, while the four parse threads finish first. SQLite cache size, one transaction for the whole build and FTS5 `automerge` settings each changed nothing beyond noise, so none is in the code. If the full build misses 3 s on an idle machine, time the writer first (wrap `write` in `parse_parallel`'s sink with an `Instant`). Record the numbers for the PR description either way.

- [ ] **Step 3: The three prototype queries**

~~~bash
cd ~/gray
GRAY_HOME=$H $B search prefix cache warm compaction
GRAY_HOME=$H $B search glibc maid build binary
GRAY_HOME=$H $B search discord bot token rotate gateway --all
cd -
~~~

Expected: each prints cards with the right turn first: the prefix-cache warmup fix, the maid release build and the glibc mismatch, and the Discord bot token rotation. If the prototype in `/tmp/levgray` still exists, compare against its first hit (`cd /tmp/levgray && /tmp/leviathan/target/release/leviathan search "<query>" -n 1`, adding `-g gray` for the first two). Each top card must be the same turn or a better one: same fix, more of the answer in the card. Read the cards and judge; a mismatch is a ranking bug to fix in `query.rs` with a test, not a note.

- [ ] **Step 4: Secrets: counts only**

~~~bash
pat='(^|[^A-Za-z0-9_-])(sk-[A-Za-z0-9_-]{20,}|ghp_[A-Za-z0-9]{20,}|xox[bp]-[A-Za-z0-9-]{20,}|[MNO][A-Za-z0-9_-]{23,27}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27,40})'
echo "session lines with a token: $(cat ~/.gray/sessions/*.jsonl | grep -cE "$pat")"
echo "index files with a token: $(grep -laE "$pat" "$H"/recall/index.db* | wc -l)"
~~~

Expected: the first count is above zero (the sessions do hold tokens; 48 lines when this plan was written) and the second is `0`. The leading boundary keeps words such as `task-…` and `mask-…` from counting as `sk-` keys. A non-zero second count is a redaction bug: find which pattern matched with `grep -oaE "$pat" "$H"/recall/index.db* | cut -c1-4 | sort | uniq -c` (prefixes only), add a failing case to `redact_tests.rs`, fix, and repeat Steps 2-4.

- [ ] **Step 5: Conformance check, install, live agent**

~~~bash
rm -rf "$H"
CARGO_BUILD_JOBS=4 cargo install --path . --locked
gray plugin check ~/.cargo/bin/gray-recall
gray plugin install ~/.cargo/bin/gray-recall
gray plugin list
gray recall status
cd ~/gray && gray -p "Use the recall tool: how was the prefix cache warmup fixed before? Cite the turn id you used."; cd -
~~~

`rm -rf "$H"` removes the temporary home and the symlink in it, not the sessions it points to.

Expected: `gray plugin check` passes every step (manifest, the tool with `{}`, two concurrent tool calls, `/recall` with an empty argv, shutdown). The first call builds the real `~/.gray/recall/index.db`, so it takes about as long as Step 2's full build. `gray plugin list` shows `recall` enabled; `gray recall status` shows the real index; the headless answer cites a `<session>:<entry>` id that `gray recall show <id>` opens.

- [ ] **Step 6: Format, lint, test, commit**

~~~bash
CARGO_BUILD_JOBS=4 cargo fmt --check && CARGO_BUILD_JOBS=4 cargo clippy --all-targets -- -D warnings && CARGO_BUILD_JOBS=4 cargo test
git add -A && git commit -m "README; checked on real sessions, gray plugin check and a live agent"
~~~

The PR description carries the Step 2 timings, the Step 3 verdicts and the Step 4 counts. Pushing, the GitHub repo, the `docs/plugins.md` line in gray and the plugin-index entry each wait for approval (spec, "License, notices, release").
