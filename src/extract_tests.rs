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
        tools: calls
            .iter()
            .map(|(n, a, _, _)| ToolUse {
                name: n.to_string(),
                args: a.clone(),
            })
            .collect(),
        results: calls
            .iter()
            .map(|(_, _, c, e)| ToolResult {
                content: c.to_string(),
                is_error: *e,
            })
            .collect(),
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
    let e = run(&turn(
        &["Looking.", "\n  Fixed: missing import.\nDetails follow."],
        &[],
    ));
    assert_eq!(
        e.reply,
        "Looking.\n\n\n  Fixed: missing import.\nDetails follow."
    );
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
    assert_eq!(
        (e.reply.as_str(), e.gist.as_str(), e.has_reply),
        ("", "", false)
    );
}

#[test]
fn commands_are_one_line_per_tool() {
    let e = run(&turn(
        &[],
        &[
            bash("cargo   build\n  --release", "exit 0"),
            ("read", json!({"path": "src/lib.rs"}), "...", false),
            (
                "web_fetch",
                json!({"url": "https://example.com/doc"}),
                "...",
                false,
            ),
            ("grep", json!({"pattern": "fn main"}), "...", false),
            (
                "bash",
                json!({"action": "output", "job_id": "j1"}),
                "...",
                false,
            ),
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
    assert!(
        e.commands
            .lines()
            .all(|l| l.chars().count() <= COMMAND_CHARS)
    );
    assert!(e.commands.len() <= COMMANDS_CAP, "{}", e.commands.len());
    assert!(e.commands.lines().count() >= 10);
}

#[test]
fn file_tool_paths_are_relative_to_the_project_root() {
    let t = turn(
        &[],
        &[
            ("read", json!({"path": "src/lib.rs"}), "", false),
            (
                "edit",
                json!({"file_path": "/work/app/README.md"}),
                "",
                false,
            ),
            ("write", json!({"path": "/etc/hosts"}), "", false),
            ("read", json!({"path": "../src/lib.rs"}), "", false),
            ("read", json!({"path": "./src/lib.rs"}), "", false),
            ("read", json!({"path": "~/notes/todo.md"}), "", false),
            ("web_fetch", json!({"path": "ignored.rs"}), "", false),
        ],
    );
    let e = extract(&t, Path::new("/work/app/crates"), Path::new("/work/app"));
    assert_eq!(
        e.files,
        [
            "crates/src/lib.rs",
            "README.md",
            "/etc/hosts",
            "src/lib.rs",
            "~/notes/todo.md"
        ]
    );
}

#[test]
fn bash_tokens_that_look_like_paths_are_files() {
    let cmd = "cargo test --manifest-path=crates/x/Cargo.toml && grep -n foo src/main.rs:12:3 2>/dev/null; cat ../other/notes.md | head -5";
    let e = run(&turn(&[], &[bash(cmd, "exit 0")]));
    assert_eq!(
        e.files,
        ["crates/x/Cargo.toml", "src/main.rs", "/work/other/notes.md"]
    );
}

#[test]
fn bash_tokens_that_are_not_paths_are_ignored() {
    let cmd =
        "git push origin feat/x && curl https://example.com/a.js -o $HOME/a.js; echo 1.5 / ./";
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
    let e = run(&turn(
        &[],
        &[bash("git commit -m x", one), bash("git commit -m y", two)],
    ));
    assert_eq!(e.commits, ["2ba5532", "abcdef1234"]);
}

#[test]
fn nonzero_exit_headers_mark_the_turn_failed() {
    for (result, failed) in [
        ("exit 1 · 0.2s · 4 lines", true),
        ("exit 127 · 0.0s", true),
        ("\n  exit 2 · 0.1s", true),
        ("exit 0 · 1.2s · 7 lines", false),
        (
            "exit 0 (`tail` masks `cd`'s exit; rerun `cd` alone for its status) · 0.0s",
            false,
        ),
        ("the script printed exit 1", false),
    ] {
        assert_eq!(
            run(&turn(&[], &[bash("x", result)])).failed,
            failed,
            "{result}"
        );
    }
}

#[test]
fn error_results_mark_the_turn_failed() {
    let t = turn(
        &[],
        &[("read", json!({"path": "x.rs"}), "no such file", true)],
    );
    assert!(run(&t).failed);
    assert!(!run(&turn(&["ok"], &[])).failed);
}

#[test]
fn lexical_resolves_dots_without_touching_the_disk() {
    for (input, want) in [
        ("/a/b/../c/./d", "/a/c/d"),
        ("a/../../b", "../b"),
        ("/..", "/"),
        ("./x", "x"),
        ("", ""),
    ] {
        assert_eq!(lexical(Path::new(input)), Path::new(want), "{input}");
    }
}
