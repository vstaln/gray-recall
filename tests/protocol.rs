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
    let message =
        |role: &str, text: &str| json!({"role": role, "content": [{"type": "text", "text": text}]});
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
    assert_eq!(
        (m["name"].as_str(), m["tools"][0]["name"].as_str()),
        (Some("recall"), Some("recall"))
    );
    let out = cli(home.path(), &["deploy", "-p", "gray"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("you: deploy the site") && text.ends_with('\n'),
        "{text}"
    );
    let out = cli(home.path(), &["deploy", "--since", "soon"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stdout.is_empty() && String::from_utf8_lossy(&out.stderr).contains("recall: bad date")
    );
    assert_eq!(
        cli(home.path(), &["deploy", "-p", "nope"]).status.code(),
        Some(3)
    );
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
    assert_eq!(
        (m["id"].as_i64(), m["result"]["name"].as_str()),
        (Some(1), Some("recall"))
    );
    let hit = next();
    assert_eq!(
        (hit["id"].as_i64(), hit["result"]["is_error"].as_bool()),
        (Some(3), Some(false))
    );
    assert!(
        hit["result"]["content"]
            .as_str()
            .unwrap()
            .contains("you: deploy the site"),
        "{hit}"
    );
    let bad = next();
    assert_eq!(
        (bad["id"].as_str(), bad["result"]["is_error"].as_bool()),
        (Some("s4"), Some(true))
    );
    let unknown = next();
    assert_eq!(
        (
            unknown["id"].as_i64(),
            unknown["result"]["is_error"].as_bool()
        ),
        (Some(5), Some(true))
    );
    let cmd = next();
    assert_eq!(cmd["id"].as_i64(), Some(6));
    assert!(
        cmd["result"]["text"]
            .as_str()
            .unwrap()
            .contains("you: deploy the site"),
        "{cmd}"
    );
    writeln!(
        stdin,
        "{}",
        json!({"method": "plugin/shutdown", "params": {"reason": "session_end"}})
    )
    .unwrap();
    assert!(child.wait().unwrap().success());
    assert!(
        lines.next().is_none(),
        "no reply to unclaimed methods, notifications or bad lines"
    );
}
