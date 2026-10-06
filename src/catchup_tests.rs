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
    let mut stmt = c
        .prepare("SELECT prompt FROM turns ORDER BY session, entry_id")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn reply_of(c: &Connection, entry_id: i64) -> String {
    c.query_row(
        "SELECT reply FROM turns WHERE entry_id = ?1",
        [entry_id],
        |r| r.get(0),
    )
    .unwrap()
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
    assert_eq!(
        r,
        Report {
            changed: 12,
            ..Report::default()
        }
    );
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
    assert!(
        matches!(mode, Mode::Tail { entry, .. } if entry == q2),
        "{mode:?}"
    );
    assert_eq!(catch_up(&mut c, h.path()).unwrap().changed, 1);
    assert_eq!(
        prompts(&c),
        ["first question", "second question", "third question"]
    );
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
    assert_eq!(
        detect(Some(&st), &path, size, mtime).unwrap().unwrap().0,
        Mode::Full
    );
    catch_up(&mut c, h.path()).unwrap();
    assert_eq!(prompts(&c), ["different start", "more"]);
    let key: String = c
        .query_row("SELECT DISTINCT project_key FROM turns", [], |r| r.get(0))
        .unwrap();
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
    assert_eq!(
        r,
        Report {
            removed: 1,
            ..Report::default()
        }
    );
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
    assert_eq!(
        catch_up(&mut c, h.path()).unwrap(),
        Report {
            busy: true,
            ..Report::default()
        }
    );
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
        .query_row(
            &format!("SELECT {} FROM turns t", index::ROW_COLUMNS),
            [],
            |r| Row::read(r, 0),
        )
        .unwrap();
    assert_eq!(
        (row.project_name.as_str(), row.cwd.as_str()),
        ("app", "/work/app")
    );
    assert_eq!(row.files, ["src/lib.rs"]);
    assert_eq!(row.commits, ["2ba5532"]);
    assert_eq!(row.commands, "edit src/lib.rs\ngit commit -am fix");
    assert_eq!(
        (row.gist.as_str(), row.failed, row.has_reply),
        ("Done.", true, true)
    );
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
    let (prompt, commands, reply): (String, String, String) = c
        .query_row("SELECT prompt, commands, reply FROM turns", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    for field in [&prompt, &commands, &reply] {
        assert!(field.contains(crate::redact::REDACTED), "{field}");
    }
}
