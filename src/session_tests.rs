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
    assert_eq!(
        (t.entry_id, t.date, t.prompt.as_str()),
        (p, T0 + 1000, "fix the build")
    );
    assert_eq!(t.tools.len(), 1);
    assert_eq!(t.tools[0].name, "bash");
    assert_eq!(t.tools[0].args, json!({"command": "cargo build"}));
    assert_eq!(
        t.results,
        vec![ToolResult {
            content: "exit 0 · 1.2s".into(),
            is_error: false
        }]
    );
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
    b.raw(
        &json!({"entry_id": 1, "parent_id": null, "timestamp": T0,
                  "message": {"role": "user", "content": "plain string prompt"}})
        .to_string(),
    );
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
