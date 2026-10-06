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
    let has_meta: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |r| r.get(0),
    )?;
    if has_meta == 0 {
        Ok(None)
    } else {
        meta_get(conn, "schema")
    }
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
            files: text(11)?
                .split('\n')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
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
    Ok(conn
        .prepare_cached("DELETE FROM turns WHERE session = ?1")?
        .execute([session])?)
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
    let ids = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Remove a session's turns and its catch-up state.
pub fn delete_session(conn: &Connection, session: &str) -> Result<()> {
    delete_session_turns(conn, session)?;
    conn.prepare_cached("DELETE FROM sessions WHERE session = ?1")?
        .execute([session])?;
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
    let mut stmt = conn.prepare(
        "SELECT project_key, max(project_name), count(*) FROM turns GROUP BY project_key",
    )?;
    let rows: Vec<(String, String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    conn.execute("DELETE FROM projects", [])?;
    let mut insert = conn
        .prepare("INSERT INTO projects (key, name, name_lower, turns) VALUES (?1, ?2, ?3, ?4)")?;
    for (key, name, turns) in rows {
        insert.execute(params![key, name, name.to_lowercase(), turns])?;
    }
    Ok(())
}

pub fn list_projects(conn: &Connection) -> Result<Vec<ProjectInfo>> {
    let mut stmt =
        conn.prepare("SELECT key, name, turns FROM projects ORDER BY turns DESC, name")?;
    let projects = stmt
        .query_map([], |r| {
            Ok(ProjectInfo {
                key: r.get(0)?,
                name: r.get(1)?,
                turns: r.get(2)?,
            })
        })?
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
    conn.prepare_cached("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)")?
        .execute([key, value])?;
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
