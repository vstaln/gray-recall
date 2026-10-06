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
    Hit {
        row: row(entry_id, prompt),
        did: format!("Did {prompt}."),
        score: 1.0,
        other_project: false,
    }
}

fn search(hits: Vec<Hit>, total: usize) -> Search {
    Search {
        scope: "project gray".into(),
        scope_turns: 3886,
        all: false,
        hits,
        total,
        fell_back: false,
    }
}

fn req(q: &str) -> Request {
    Request {
        query: q.into(),
        ..Request::default()
    }
}

fn shown_turns(target: i64, turns: Vec<Row>) -> Shown {
    Shown {
        session: S.into(),
        target,
        turns,
    }
}

/// `N` from the header's `shown N of M`.
fn shown(out: &str) -> usize {
    let header = out.lines().next().unwrap();
    header
        .split("shown ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap()
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
    let r = Request {
        since: Some("2026-09".into()),
        ..req("prefix cache compaction")
    };
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
    assert!(
        out.starts_with(
            r#"recall · project gray (3,886 turns) · "prefix cache" warm · shown 0 of 0"#
        ),
        "{out}"
    );
}

#[test]
fn fallback_and_all_scope_cards_name_the_project() {
    let mut h = hit(7, "deploy graysite pages");
    h.row.project_name = "graysite".into();
    h.other_project = true;
    let s = Search {
        fell_back: true,
        ..search(vec![h.clone()], 1)
    };
    let out = search_text(&s, &req("pages"), false);
    assert!(
        out.contains("\nno match in project gray; searched the other projects\n"),
        "{out}"
    );
    assert!(out.contains(" · OTHER PROJECT graysite\n"), "{out}");
    h.other_project = false;
    let s = Search {
        scope: "all projects".into(),
        all: true,
        ..search(vec![h], 1)
    };
    let out = search_text(&s, &req("pages"), false);
    assert!(
        out.contains(&format!("[1] 1900bae3:7 · {} · graysite\n", when(T0))),
        "{out}"
    );
    assert!(
        !out.contains("OTHER PROJECT") && !out.contains("no match in"),
        "{out}"
    );
}

#[test]
fn zero_hits_say_none_found_and_suggest_a_wider_search() {
    let out = search_text(
        &search(vec![], 0),
        &Request {
            project: Some("gray".into()),
            ..req("zzz")
        },
        false,
    );
    let want = r#"recall · project gray (3,886 turns) · "zzz" · shown 0 of 0
none found: try --all, fewer or other words, or a wider date range"#;
    assert_eq!(out, want);
    let out = search_text(
        &Search {
            fell_back: true,
            ..search(vec![], 0)
        },
        &req("zzz"),
        false,
    );
    let tail = "searched the other projects\nnone found in any project: try fewer or other words, or a wider date range";
    assert!(out.ends_with(tail), "{out}");
    let out = search_text(
        &search(vec![], 7),
        &Request {
            offset: 10,
            ..req("deploy")
        },
        false,
    );
    assert!(
        out.contains("shown 0 of 7 from [11]")
            && out.ends_with("none on this page: lower the offset"),
        "{out}"
    );
    assert!(!out.contains("next:"), "{out}");
}

#[test]
fn text_fits_max_chars_and_the_header_counts_the_cards_shown() {
    let s = search(long_hits(20), 20);
    for max in [500, 1000, 2500, 8000, 50_000] {
        let out = search_text(
            &s,
            &Request {
                max_chars: Some(max),
                ..req("word")
            },
            true,
        );
        assert!(out.chars().count() <= max, "{max}: {}", out.chars().count());
        assert_eq!(shown(&out), cards(&out), "{max}");
        assert!(shown(&out) >= 1, "{max}: {out}");
        assert!(
            out.contains(BUSY_NOTE) && out.contains("next: recall id=1900bae3:1 "),
            "{max}: {out}"
        );
    }
    let roomy = search_text(
        &s,
        &Request {
            max_chars: Some(50_000),
            ..req("word")
        },
        false,
    );
    assert_eq!(shown(&roomy), 20);
    assert_eq!(roomy.matches("\n  files: ").count(), 20);
    let floor = search_text(
        &s,
        &Request {
            max_chars: Some(8000),
            ..req("word")
        },
        false,
    );
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
    let out = search_text(
        &search(vec![hit(6, "six"), hit(7, "seven")], 7),
        &Request {
            offset: 5,
            ..req("")
        },
        false,
    );
    assert!(
        out.starts_with("recall · project gray (3,886 turns) · shown 2 of 7 from [6]\n[6] "),
        "{out}"
    );
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
    assert!(
        header.ends_with(" · until 2026-10-01 · file lib.rs · failed only · shown 1 of 1"),
        "{header}"
    );
}

#[test]
fn search_json_carries_the_same_data() {
    let mut h = hit(3, &"long ".repeat(300));
    h.row.project_name = "graysite".into();
    h.other_project = true;
    let s = Search {
        fell_back: true,
        ..search(vec![h], 1)
    };
    let v: Value = serde_json::from_str(&search_json(&s, &req("long"), true)).unwrap();
    assert_eq!(
        (
            v["total"].as_u64(),
            v["shown"].as_u64(),
            v["fell_back"].as_bool(),
            v["busy"].as_bool()
        ),
        (Some(1), Some(1), Some(true), Some(true))
    );
    let first = &v["hits"][0];
    assert_eq!(
        (
            first["n"].as_u64(),
            first["id"].as_str(),
            first["session"].as_str()
        ),
        (Some(1), Some("1900bae3:3"), Some(S))
    );
    assert_eq!(
        (first["project"].as_str(), first["other_project"].as_bool()),
        (Some("graysite"), Some(true))
    );
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
    assert_eq!(
        show_text(
            &shown_turns(5, vec![row(3, "two"), t, row(7, "four")]),
            8000,
            false
        ),
        want
    );
    let mut empty = row(1, "hi");
    empty.reply = String::new();
    assert!(
        show_text(&shown_turns(1, vec![empty]), 8000, true).contains(&format!(
            "\n{BUSY_NOTE}\n▶ 1900bae3:1 · {d}\nyou: hi\nreply: (none)\nresume:"
        ))
    );
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
        let reply = out
            .split("\nreply:\n")
            .nth(1)
            .unwrap()
            .split("\ncommands:\n")
            .next()
            .unwrap();
        let commands = out
            .split("\ncommands:\n")
            .nth(1)
            .unwrap()
            .split("\n- ")
            .next()
            .unwrap();
        assert!(reply.chars().count() > commands.chars().count(), "{max}");
        assert!(commands.starts_with("$ cargo test"), "{max}: {out}");
        assert!(
            out.contains("\n- 1900bae3:4 · ") && out.contains("\n- 1900bae3:6 · "),
            "{max}: {out}"
        );
        assert!(
            out.ends_with(&format!("resume: gray -r {S}")),
            "{max}: {out}"
        );
    }
    let out = show_text(&sh, 8000, false);
    assert!(
        out.chars().count() > 7900,
        "the budget is used: {}",
        out.chars().count()
    );
}

#[test]
fn far_neighbors_shrink_then_drop_to_fit() {
    let long = "context ".repeat(50);
    let sh = shown_turns(
        11,
        (1..=21).map(|i| row(i, &format!("{i} {long}"))).collect(),
    );
    let out = show_text(&sh, 500, false);
    assert!(out.chars().count() <= 500, "{}", out.chars().count());
    assert!(
        out.contains("\n▶ 1900bae3:11 · ") && out.contains("neighbors left out to fit max_chars"),
        "{out}"
    );
    let mid = show_text(&sh, 4000, false);
    assert!(
        mid.contains("\n- 1900bae3:10 · ") && mid.contains("\n- 1900bae3:12 · "),
        "{mid}"
    );
    let roomy = show_text(&sh, 50_000, false);
    assert!(
        !roomy.contains("left out") && roomy.matches("\n- 1900bae3:").count() == 20,
        "{roomy}"
    );
}

#[test]
fn show_json_has_the_target_in_full_and_neighbor_gists() {
    let mut t = row(5, "three");
    t.commands = "$ ls".into();
    let v: Value =
        serde_json::from_str(&show_json(&shown_turns(5, vec![row(3, "two"), t]), false)).unwrap();
    assert_eq!(
        (
            v["session"].as_str(),
            v["target"].as_i64(),
            v["project"].as_str()
        ),
        (Some(S), Some(5), Some("gray"))
    );
    let turns = v["turns"].as_array().unwrap();
    assert_eq!(
        (
            turns[0]["target"].as_bool(),
            turns[0]["gist"].as_str(),
            turns[0].get("reply")
        ),
        (Some(false), Some("Gist of two."), None)
    );
    assert_eq!(
        (
            turns[1]["target"].as_bool(),
            turns[1]["reply"].as_str(),
            turns[1]["commands"].as_str()
        ),
        (Some(true), Some("Reply to three."), Some("$ ls"))
    );
}

#[test]
fn projects_list_aligned_with_totals_and_fit_the_budget() {
    let ps = vec![
        ProjectInfo {
            key: "/home/u/gray".into(),
            name: "gray".into(),
            turns: 3886,
        },
        ProjectInfo {
            key: "/home/u/graysite".into(),
            name: "graysite".into(),
            turns: 254,
        },
    ];
    let want = "recall · 2 projects · 4,140 turns\n  gray      3,886  /home/u/gray\n  graysite    254  /home/u/graysite";
    assert_eq!(projects_text(&ps, 2500), want);
    assert_eq!(
        projects_text(&[], 2500),
        "recall · 0 projects · 0 turns\nnone indexed yet"
    );
    let many: Vec<ProjectInfo> = (0..100)
        .map(|i| ProjectInfo {
            key: format!("/home/u/project-{i}"),
            name: format!("project-{i}"),
            turns: 1,
        })
        .collect();
    let out = projects_text(&many, 500);
    assert!(
        out.chars().count() <= 500 && out.ends_with(" more (--json lists all)"),
        "{out}"
    );
    let v: Value = serde_json::from_str(&projects_json(&ps)).unwrap();
    assert_eq!(
        (
            v[0]["name"].as_str(),
            v[1]["turns"].as_i64(),
            v[1]["key"].as_str()
        ),
        (Some("gray"), Some(254), Some("/home/u/graysite"))
    );
}

#[test]
fn status_reports_counts_size_and_last_catch_up() {
    let st = Stats {
        turns: 4140,
        sessions: 819,
        projects: 21,
        skipped: 3,
        bytes: 25_200_000,
        last_catch_up: Some(T0 - 180_000),
    };
    let want = format!(
        "recall index · 4,140 turns · 819 sessions · 21 projects\nsize 24.0 MB · skipped lines 3 · last catch-up {} (3 min ago)\nfile /h/.gray/recall/index.db",
        when(T0 - 180_000)
    );
    assert_eq!(
        status_text(&st, Path::new("/h/.gray/recall/index.db"), T0, false),
        want
    );
    let never = status_text(
        &Stats {
            last_catch_up: None,
            ..st.clone()
        },
        Path::new("/x"),
        T0,
        true,
    );
    assert!(
        never.contains("last catch-up never") && never.ends_with(BUSY_NOTE),
        "{never}"
    );
    let v: Value = serde_json::from_str(&status_json(&st, Path::new("/x"), false)).unwrap();
    assert_eq!(
        (
            v["turns"].as_i64(),
            v["bytes"].as_u64(),
            v["last_catch_up"].as_i64(),
            v["index"].as_str()
        ),
        (Some(4140), Some(25_200_000), Some(T0 - 180_000), Some("/x"))
    );
}

#[test]
fn numbers_sizes_and_ages_format() {
    let n = [
        thousands(0),
        thousands(999),
        thousands(1000),
        thousands(1_234_567),
        thousands(-4140),
    ];
    assert_eq!(n, ["0", "999", "1,000", "1,234,567", "-4,140"]);
    assert_eq!(
        [size(12), size(2048), size(25_200_000)],
        ["12 B", "2 KB", "24.0 MB"]
    );
    assert_eq!(
        [plural(1, "turn"), plural(0, "turn"), plural(4140, "turn")],
        ["1 turn", "0 turns", "4,140 turns"]
    );
    let a = [
        ago(5_000),
        ago(180_000),
        ago(7_200_000),
        ago(3 * 86_400_000),
        ago(-1),
    ];
    assert_eq!(
        a,
        ["just now", "3 min ago", "2 h ago", "3 d ago", "just now"]
    );
}
