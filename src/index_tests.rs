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
    c.query_row(
        "SELECT count(*) FROM turns_fts WHERE turns_fts MATCH ?1",
        [q],
        |r| r.get(0),
    )
    .unwrap()
}

fn count(c: &Connection, table: &str) -> i64 {
    c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
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
    assert_eq!(
        meta_get(&c, "schema").unwrap().as_deref(),
        Some(SCHEMA_VERSION)
    );
    let mode: String = c
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
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
    assert_eq!(
        meta_get(&c, "schema").unwrap().as_deref(),
        Some(SCHEMA_VERSION)
    );
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
    let got = c
        .query_row(&format!("SELECT {ROW_COLUMNS} FROM turns t"), [], |r| {
            Row::read(r, 0)
        })
        .unwrap();
    assert!(got.rowid > 0);
    assert_eq!(
        got,
        Row {
            rowid: got.rowid,
            ..want.clone()
        }
    );
    let shifted = c
        .query_row(&format!("SELECT 42, {ROW_COLUMNS} FROM turns t"), [], |r| {
            Row::read(r, 1)
        })
        .unwrap();
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
    put_session(
        &c,
        &SessionState {
            size: 2000,
            tail_entry: None,
            ..s.clone()
        },
    )
    .unwrap();
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
            ProjectInfo {
                key: "/w/App".into(),
                name: "App".into(),
                turns: 2
            },
            ProjectInfo {
                key: "/w/tool".into(),
                name: "tool".into(),
                turns: 1
            },
        ]
    );
    let lower: String = c
        .query_row(
            "SELECT name_lower FROM projects WHERE key = '/w/App'",
            [],
            |r| r.get(0),
        )
        .unwrap();
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
    put_session(
        &c,
        &SessionState {
            skipped: 3,
            ..state("s1")
        },
    )
    .unwrap();
    rebuild_projects(&c).unwrap();
    meta_set(&c, LAST_CATCH_UP, "1790000000000").unwrap();
    let st = stats(&c, home.path()).unwrap();
    assert_eq!(
        (st.turns, st.sessions, st.projects, st.skipped),
        (2, 1, 1, 3)
    );
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
    assert_eq!(
        (count(&c, "turns"), count(&c, "sessions"), hits(&c, "alpha")),
        (0, 0, 0)
    );
    assert_eq!(meta_get(&c, "x").unwrap(), None);
    assert_eq!(
        meta_get(&c, "schema").unwrap().as_deref(),
        Some(SCHEMA_VERSION)
    );
    insert_turn(&c, &row("s1", 1, "app", "alpha")).unwrap();
    assert_eq!(hits(&c, "alpha"), 1);
}
