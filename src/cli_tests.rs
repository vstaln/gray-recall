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
    Context {
        cwd: "/work/gray".into(),
        session: None,
        now_ms: T0 + 86_400_000,
    }
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
    for w in [
        "search",
        "projects",
        "status",
        "reindex",
        "--help",
        "-h",
        "--version",
        "-V",
    ] {
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
    assert!(
        request_from_json(&json!({"failed": "true"}))
            .unwrap()
            .failed
    );
    assert_eq!(
        request_from_json(&json!({"limit": 3.0})).unwrap().limit,
        Some(3)
    );
    assert_eq!(
        request_from_json(&json!({"id": "abc:1", "around": 2}))
            .unwrap()
            .around,
        2
    );
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
    assert!(
        out.text
            .starts_with(r#"recall · project gray (2 turns) · "prefix cache" · shown 1 of 1"#),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("✗ failed cmd")
            && out.text.contains("next: recall id=1900bae3:1 around=2"),
        "{}",
        out.text
    );
    let out = run(home.path(), &["show", "1900bae3:1", "--around", "1"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains("\n▶ 1900bae3:1 · ") && out.text.contains("\n- 1900bae3:5 · "),
        "{}",
        out.text
    );
    assert!(out.text.contains("\ncommands:\ncargo test"), "{}", out.text);
    let out = run(home.path(), &["projects"]);
    assert!(
        out.code == 0
            && out
                .text
                .starts_with("recall · 1 project · 2 turns\n  gray  2  "),
        "{}",
        out.text
    );
    let out = run(home.path(), &["status"]);
    assert!(
        out.code == 0
            && out
                .text
                .starts_with("recall index · 2 turns · 1 session · 1 project\n"),
        "{}",
        out.text
    );
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
    assert!(
        out.code == 0 && out.text.contains("none found"),
        "{}",
        out.text
    );
    let help = run(home.path(), &["--help"]);
    assert!(
        help.code == 0 && help.text.contains("Usage"),
        "{}",
        help.text
    );
}

#[test]
fn the_tool_reports_errors_but_not_empty_results() {
    let home = home_with(&[&gray_session()]);
    let t = |args: Value| tool_call(home.path(), &args, &ctx());
    let ok = t(json!({"query": "deploy"}));
    assert_eq!(ok["is_error"], false);
    assert!(
        ok["content"]
            .as_str()
            .unwrap()
            .contains("you: deploy the site"),
        "{ok}"
    );
    assert_eq!(t(json!({"query": "zzzz"}))["is_error"], false);
    assert_eq!(t(json!({}))["is_error"], false);
    assert_eq!(t(Value::Null)["is_error"], false);
    let bad = t(json!({"since": "soon"}));
    assert!(
        bad["is_error"] == true
            && bad["content"]
                .as_str()
                .unwrap()
                .starts_with("recall: bad date"),
        "{bad}"
    );
    assert_eq!(t(json!({"limit": "x"}))["is_error"], true);
    let shown = t(json!({"id": "1900bae3:5"}));
    assert!(
        shown["content"].as_str().unwrap().contains("▶ 1900bae3:5"),
        "{shown}"
    );
}

#[test]
fn reindex_rebuilds_and_a_held_lock_is_reported() {
    let home = home_with(&[&gray_session()]);
    assert_eq!(run(home.path(), &["deploy"]).code, 0);
    let out = run(home.path(), &["reindex"]);
    assert!(
        out.code == 0 && out.text.starts_with("reindexed 1 session (2 turns) in "),
        "{}",
        out.text
    );
    let _held = writer_lock(home.path()).unwrap().unwrap();
    let out = run(home.path(), &["reindex"]);
    assert!(
        out.code == 1 && out.text.contains("try again"),
        "{}",
        out.text
    );
    let out = run(home.path(), &["deploy"]);
    assert!(
        out.code == 0 && out.text.contains(BUSY_NOTE) && out.text.contains("deploy the site"),
        "{}",
        out.text
    );
    let out = run(home.path(), &["status"]);
    assert!(out.text.ends_with(BUSY_NOTE), "{}", out.text);
}

#[test]
fn the_manifest_declares_the_tool_and_command() {
    let m = manifest();
    assert_eq!(
        (m["name"].as_str(), m["protocol"].as_str()),
        (Some("recall"), Some("1.1"))
    );
    assert_eq!(
        (m["commands"].clone(), m["hooks"].clone()),
        (json!(["/recall"]), json!([]))
    );
    assert_eq!(m["completion"], json!(SUBCOMMANDS));
    let tool = &m["tools"][0];
    assert_eq!(
        (tool["name"].as_str(), tool["parameters"]["type"].as_str()),
        (Some("recall"), Some("object"))
    );
    assert!(tool["description"].as_str().unwrap().len() > 80 && tool["snippet"].is_string());
    let props = tool["parameters"]["properties"].as_object().unwrap();
    let fields = [
        "query",
        "project",
        "all",
        "file",
        "since",
        "until",
        "failed",
        "limit",
        "offset",
        "id",
        "around",
        "json",
        "max_chars",
    ];
    assert_eq!(props.len(), fields.len());
    for k in fields {
        assert!(props[k]["description"].is_string(), "{k}");
    }
}

#[test]
fn wire_context_reads_the_session_and_falls_back_to_the_process_cwd() {
    let c = wire_context(&json!({"session": {"id": "abc", "cwd": "/w/x"}}));
    assert_eq!(
        (c.cwd.as_str(), c.session.as_deref()),
        ("/w/x", Some("abc"))
    );
    let c = wire_context(&json!({"cwd": "/w/y", "session": {"id": "", "cwd": ""}}));
    assert_eq!((c.cwd.as_str(), c.session), ("/w/y", None));
    let c = wire_context(&Value::Null);
    assert_eq!(c.cwd, std::env::current_dir().unwrap().to_string_lossy());
    assert!(c.now_ms > T0);
}
