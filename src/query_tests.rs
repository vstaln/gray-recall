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
    Context {
        cwd: format!("/work/{project}"),
        session: None,
        now_ms: T0 + 10 * DAY,
    }
}

fn req(q: &str) -> Request {
    Request {
        query: q.into(),
        ..Request::default()
    }
}

fn found(s: &Search) -> Vec<String> {
    s.hits.iter().map(|h| h.row.prompt.clone()).collect()
}

fn info(name: &str) -> ProjectInfo {
    ProjectInfo {
        key: format!("/w/{name}"),
        name: name.into(),
        turns: 1,
    }
}

#[test]
fn match_expressions() {
    let cases = [
        ("cache warm", Some(r#""cache" OR "warm""#)),
        (
            r#""prefix cache" warm"#,
            Some(r#""prefix cache" AND ("prefix cache" OR "warm")"#),
        ),
        (r#""a b" "c""#, Some(r#""a b" AND "c""#)),
        (
            "cache -cookie -token",
            Some(r#"("cache") NOT "cookie" NOT "token""#),
        ),
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
    let f = fixture(&[&session(
        "s1",
        "gray",
        &["cache only", "nothing relevant", "cache warm compaction"],
    )]);
    let s = search(&f.conn, &req("cache warm compaction"), &ctx("gray")).unwrap();
    assert_eq!(found(&s), ["cache warm compaction", "cache only"]);
    assert_eq!(
        (s.total, s.scope.as_str(), s.scope_turns),
        (2, "project gray", 3)
    );
}

#[test]
fn phrases_are_required() {
    let f = fixture(&[&session(
        "s1",
        "gray",
        &["prefix cache warmup", "cache the prefix"],
    )]);
    for q in [r#""prefix cache""#, r#""prefix cache" warmup"#] {
        assert_eq!(
            found(&search(&f.conn, &req(q), &ctx("gray")).unwrap()),
            ["prefix cache warmup"],
            "{q}"
        );
    }
}

#[test]
fn exclusions_remove_turns() {
    let f = fixture(&[&session(
        "s1",
        "gray",
        &["sso login cookie", "sso login token"],
    )]);
    assert_eq!(
        found(&search(&f.conn, &req("sso -cookie"), &ctx("gray")).unwrap()),
        ["sso login token"]
    );
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
    let s = search(
        &f.conn,
        &Request {
            project: Some("gray".into()),
            ..req("pages")
        },
        &ctx("graysite"),
    )
    .unwrap();
    assert!(s.hits.is_empty() && !s.fell_back && s.total == 0);
    assert_eq!(s.scope, "project gray");
    let s = search(
        &f.conn,
        &Request {
            all: true,
            ..req("deploy")
        },
        &ctx("gray"),
    )
    .unwrap();
    assert_eq!(
        (s.total, s.scope.as_str(), s.all, s.scope_turns),
        (2, "all projects", true, 2)
    );
    assert!(s.hits.iter().all(|h| !h.other_project));
    let e = search(
        &f.conn,
        &Request {
            project: Some("nope".into()),
            ..req("x")
        },
        &ctx("gray"),
    )
    .unwrap_err();
    assert_eq!(e.code, 3);
}

#[test]
fn project_names_resolve_in_tiers() {
    let ps = [
        info("gray"),
        info("graysite"),
        info("Vibe_Coding"),
        info("alignment"),
    ];
    for (name, want) in [
        ("gray", "gray"),
        ("/w/graysite", "graysite"),
        ("vibe_coding", "Vibe_Coding"),
        ("site", "graysite"),
        ("alignmnet", "alignment"),
    ] {
        assert_eq!(resolve_project(&ps, name).unwrap().name, want, "{name}");
    }
    let e = resolve_project(&ps, "gra").unwrap_err();
    assert_eq!(e.code, 3);
    assert!(
        e.message.contains("ambiguous")
            && e.message.contains("gray (/w/gray)")
            && e.message.contains("graysite"),
        "{}",
        e.message
    );
    let twins = [
        ProjectInfo {
            key: "/a/app".into(),
            ..info("app")
        },
        ProjectInfo {
            key: "/b/app".into(),
            ..info("app")
        },
    ];
    let e = resolve_project(&twins, "app").unwrap_err();
    assert!(
        e.message.contains("/a/app") && e.message.contains("/b/app"),
        "{}",
        e.message
    );
    assert_eq!(resolve_project(&twins, "/b/app").unwrap().key, "/b/app");
    let e = resolve_project(&ps, "zzzz").unwrap_err();
    assert!(
        e.code == 3 && e.message.contains("closest"),
        "{}",
        e.message
    );
    assert_eq!(resolve_project(&ps, " ").unwrap_err().code, 2);
}

#[test]
fn the_calling_session_is_excluded() {
    let f = fixture(&[
        &session("s1", "gray", &["flaky test"]),
        &session("s2", "gray", &["flaky test again"]),
    ]);
    let s = search(
        &f.conn,
        &req("flaky"),
        &Context {
            session: Some("s2".into()),
            ..ctx("gray")
        },
    )
    .unwrap();
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
    let c = Context {
        now_ms: T0 + 30 * DAY,
        ..ctx("gray")
    };
    let q = |since: Option<&str>, until: Option<&str>| {
        let r = Request {
            since: since.map(Into::into),
            until: until.map(Into::into),
            ..req("work")
        };
        found(&search(&f.conn, &r, &c).unwrap())
    };
    assert_eq!(q(Some("2026-10"), None), ["october work"]);
    assert_eq!(q(None, Some("2026-09")), ["september work"]);
    assert_eq!(q(Some("15d"), None), ["october work"]);
    assert_eq!(q(Some("2026"), Some("2026")).len(), 2);
    let e = search(
        &f.conn,
        &Request {
            since: Some("last week".into()),
            ..req("work")
        },
        &c,
    )
    .unwrap_err();
    assert_eq!(e.code, 2);
}

#[test]
fn dates_parse_to_local_period_bounds() {
    let local = |y: i32, m: u32, d: u32| {
        Local
            .with_ymd_and_hms(y, m, d, 0, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis()
    };
    let now = T0;
    assert_eq!(parse_date("2026", now, false).unwrap(), local(2026, 1, 1));
    assert_eq!(
        parse_date("2026", now, true).unwrap(),
        local(2027, 1, 1) - 1
    );
    assert_eq!(
        parse_date("2026-02", now, true).unwrap(),
        local(2026, 3, 1) - 1
    );
    assert_eq!(
        parse_date("2026-12", now, true).unwrap(),
        local(2027, 1, 1) - 1
    );
    assert_eq!(
        parse_date("2026-09-21", now, false).unwrap(),
        local(2026, 9, 21)
    );
    assert_eq!(
        parse_date("2026-09-21", now, true).unwrap(),
        local(2026, 9, 22) - 1
    );
    assert_eq!(parse_date("3d", now, false).unwrap(), now - 3 * DAY);
    assert_eq!(parse_date("2w", now, true).unwrap(), now - 14 * DAY);
    assert_eq!(parse_date("1m", now, false).unwrap(), now - 30 * DAY);
    for bad in [
        "",
        "2026-13",
        "2026-02-30",
        "26",
        "yesterday",
        "3y",
        "-3d",
        "d",
        "2026-1-1-1",
    ] {
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
    let failed = search(
        &f.conn,
        &Request {
            failed: true,
            ..req("fix")
        },
        &ctx("gray"),
    )
    .unwrap();
    assert_eq!(found(&failed), ["fix module a"]);
    let by_file = search(
        &f.conn,
        &Request {
            file: Some("b.rs".into()),
            ..req("")
        },
        &ctx("gray"),
    )
    .unwrap();
    assert_eq!(found(&by_file), ["fix module b"]);
}

#[test]
fn an_empty_query_lists_the_newest_turns() {
    let f = fixture(&[&session("s1", "gray", &["one", "two", "three"])]);
    let s = search(&f.conn, &req(""), &ctx("gray")).unwrap();
    assert_eq!(
        (found(&s), s.total),
        (vec!["three".to_string(), "two".into(), "one".into()], 3)
    );
    let s = search(
        &f.conn,
        &Request {
            limit: Some(2),
            offset: 1,
            ..req("")
        },
        &ctx("gray"),
    )
    .unwrap();
    assert_eq!(
        (found(&s), s.total),
        (vec!["two".to_string(), "one".into()], 3)
    );
}

#[test]
fn limit_and_offset_page_through_matches() {
    let prompts: Vec<String> = (0..7).map(|i| format!("deploy step {i}")).collect();
    let refs: Vec<&str> = prompts.iter().map(String::as_str).collect();
    let f = fixture(&[&session("s1", "gray", &refs)]);
    let first = search(&f.conn, &req("deploy"), &ctx("gray")).unwrap();
    assert_eq!((first.hits.len(), first.total), (5, 7));
    let second = search(
        &f.conn,
        &Request {
            offset: 5,
            ..req("deploy")
        },
        &ctx("gray"),
    )
    .unwrap();
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
    assert!(
        !did.contains(MARK_START) && !did.contains(MARK_END),
        "{did:?}"
    );
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
    assert!(
        score(-8.0, now - 365 * DAY, now, true) > fresh,
        "relevance outweighs recency"
    );
}

#[test]
fn request_limits_are_clamped() {
    let r = Request::default();
    assert_eq!((r.limit(), r.max_chars(), r.around()), (5, 2500, 0));
    let r = Request {
        limit: Some(100),
        max_chars: Some(10),
        around: 50,
        ..Request::default()
    };
    assert_eq!((r.limit(), r.max_chars(), r.around()), (20, 500, 10));
    let r = Request {
        limit: Some(0),
        max_chars: Some(1_000_000),
        ..Request::default()
    };
    assert_eq!((r.limit(), r.max_chars()), (1, 50_000));
    assert_eq!(
        Request {
            id: Some("x:1".into()),
            ..Request::default()
        }
        .max_chars(),
        8000
    );
}

#[test]
fn fts_syntax_in_user_input_is_plain_text() {
    let f = fixture(&[&session("s1", "gray", &["and or not"])]);
    for q in [
        "AND OR NOT",
        "( ) * ^ :",
        "NEAR(a b)",
        "\"unbalanced",
        "col:value",
        "a*",
    ] {
        assert!(search(&f.conn, &req(q), &ctx("gray")).is_ok(), "{q}");
    }
    assert_eq!(
        found(&search(&f.conn, &req("NOT"), &ctx("gray")).unwrap()),
        ["and or not"]
    );
}

#[test]
fn show_finds_a_turn_by_id_or_prefix_with_neighbors() {
    let a_id = "abc11111-0000-4000-8000-000000000000";
    let b_id = "abc22222-0000-4000-8000-000000000000";
    let f = fixture(&[
        &session(a_id, "gray", &["one", "two", "three", "four"]),
        &session(b_id, "gray", &["other"]),
    ]);
    let prompts = |s: &Shown| s.turns.iter().map(|t| t.prompt.clone()).collect::<Vec<_>>();
    let s = show(&f.conn, "abc11111:5", 1).unwrap();
    assert_eq!((s.session.as_str(), s.target), (a_id, 5));
    assert_eq!(prompts(&s), ["two", "three", "four"]);
    assert_eq!(
        prompts(&show(&f.conn, &format!("{a_id}:1"), 0).unwrap()),
        ["one"]
    );
    assert_eq!(show(&f.conn, "abc11111:1", 10).unwrap().turns.len(), 4);
    let e = show(&f.conn, "abc:1", 0).unwrap_err();
    assert!(
        e.code == 3 && e.message.contains(a_id) && e.message.contains(b_id),
        "{}",
        e.message
    );
    for (id, code) in [
        ("zzz:1", 3),
        ("abc11111:2", 3),
        ("abc11111", 2),
        ("abc11111:x", 2),
        (":5", 2),
    ] {
        assert_eq!(show(&f.conn, id, 0).unwrap_err().code, code, "{id}");
    }
}

#[test]
fn an_exact_session_id_wins_over_longer_ids_it_prefixes() {
    let f = fixture(&[
        &session("s1", "gray", &["short"]),
        &session("s10", "gray", &["long"]),
    ]);
    assert_eq!(show(&f.conn, "s1:1", 0).unwrap().turns[0].prompt, "short");
}
