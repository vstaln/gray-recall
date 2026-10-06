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
        let default = if self.id.is_some() {
            SHOW_CHARS
        } else {
            SEARCH_CHARS
        };
        self.max_chars
            .unwrap_or(default)
            .clamp(MIN_CHARS, MAX_CHARS)
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A failure with its exit code: 1 error, 2 bad request, 3 ambiguous or unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallError {
    pub code: i32,
    pub message: String,
}

impl RecallError {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
        }
    }

    pub fn bad(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }

    pub fn unknown(message: impl Into<String>) -> Self {
        Self {
            code: 3,
            message: message.into(),
        }
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
        return Err(RecallError::bad(
            "exclusions need at least one word or phrase to exclude from",
        ));
    }
    let core = if phrases.is_empty() {
        words.join(" OR ")
    } else if words.is_empty() {
        phrases.join(" AND ")
    } else {
        format!(
            "{} AND ({} OR {})",
            phrases.join(" AND "),
            phrases[0],
            words.join(" OR ")
        )
    };
    Ok(Some(if nots.is_empty() {
        core
    } else {
        format!("({core}) NOT {}", nots.join(" NOT "))
    }))
}

/// `YYYY`, `YYYY-MM`, `YYYY-MM-DD` (local time; `end` gives the period's last
/// millisecond) or `Nd` / `Nw` / `Nm` before `now_ms`.
pub fn parse_date(s: &str, now_ms: i64, end: bool) -> Result<i64, RecallError> {
    let s = s.trim();
    let bad = || {
        RecallError::bad(format!(
            "bad date '{s}': use YYYY, YYYY-MM, YYYY-MM-DD, or Nd / Nw / Nm"
        ))
    };
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
        parts
            .get(i)
            .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|p| p.parse::<u32>().ok())
    };
    let year = num(0).filter(|_| parts[0].len() == 4).ok_or_else(bad)? as i32;
    let ymd = |y: i32, m: u32, d: u32| NaiveDate::from_ymd_opt(y, m, d);
    let range = match parts.len() {
        1 => ymd(year, 1, 1).zip(ymd(year + 1, 1, 1)),
        2 => num(1).and_then(|m| {
            let next = if m == 12 {
                ymd(year + 1, 1, 1)
            } else {
                ymd(year, m + 1, 1)
            };
            ymd(year, m, 1).zip(next)
        }),
        3 => num(1)
            .zip(num(2))
            .and_then(|(m, d)| ymd(year, m, d))
            .and_then(|d| Some((d, d.succ_opt()?))),
        _ => None,
    };
    let (start, next) = range.ok_or_else(bad)?;
    let local = |d: NaiveDate| {
        Local
            .from_local_datetime(&d.and_hms_opt(0, 0, 0)?)
            .earliest()
            .map(|t| t.timestamp_millis())
    };
    if end {
        local(next).map(|ms| ms - 1).ok_or_else(bad)
    } else {
        local(start).ok_or_else(bad)
    }
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
        match projects
            .iter()
            .filter(|p| tier(p))
            .collect::<Vec<_>>()
            .as_slice()
        {
            [] => {}
            [one] => return Ok((*one).clone()),
            many => {
                let list: Vec<String> = many
                    .iter()
                    .map(|p| format!("{} ({})", p.name, p.key))
                    .collect();
                return Err(RecallError::unknown(format!(
                    "project '{name}' is ambiguous: {}; pass a full name or path",
                    list.join(", ")
                )));
            }
        }
    }
    let mut close: Vec<(f64, &str)> = projects
        .iter()
        .map(|p| (similarity(p), p.name.as_str()))
        .collect();
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
        since: req
            .since
            .as_deref()
            .map(|s| parse_date(s, ctx.now_ms, false))
            .transpose()?,
        until: req
            .until
            .as_deref()
            .map(|s| parse_date(s, ctx.now_ms, true))
            .transpose()?,
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
    let (hits, total) = run(
        conn,
        expr.as_deref(),
        &filter,
        key.as_deref().map(|k| (k, true)),
        req,
        ctx.now_ms,
    )?;
    if total == 0
        && !req.all
        && req.explicit_project().is_none()
        && let Some(k) = key.as_deref()
    {
        let (mut hits, total) = run(
            conn,
            expr.as_deref(),
            &filter,
            Some((k, false)),
            req,
            ctx.now_ms,
        )?;
        hits.iter_mut().for_each(|h| h.other_project = true);
        return Ok(Search {
            scope,
            scope_turns,
            all: false,
            hits,
            total,
            fell_back: true,
        });
    }
    Ok(Search {
        scope,
        scope_turns,
        all: req.all,
        hits,
        total,
        fell_back: false,
    })
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
        add(
            if inside {
                "t.project_key = ?"
            } else {
                "t.project_key != ?"
            },
            Some(Value::Text(key.to_string())),
        );
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
    let filter = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    let from = if expr.is_some() {
        "turns_fts JOIN turns t ON t.rowid = turns_fts.rowid"
    } else {
        "turns t"
    };
    let total: i64 = conn.query_row(
        &format!("SELECT count(*) FROM {from} {filter}"),
        params_from_iter(&args),
        |r| r.get(0),
    )?;
    let limit = req.limit();
    let hits = if expr.is_some() {
        let sql = format!(
            "SELECT {ROW_COLUMNS}, {BM25}, snippet(turns_fts, 2, char(2), char(3), ' … ', 24)
             FROM {from} {filter} ORDER BY {BM25} LIMIT {CANDIDATES}"
        );
        let mut hits = hits(conn, &sql, &args, now_ms)?;
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(b.row.date.cmp(&a.row.date))
        });
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

fn hits(
    conn: &Connection,
    sql: &str,
    args: &[Value],
    now_ms: i64,
) -> Result<Vec<Hit>, RecallError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params_from_iter(args), |r| {
            Ok((
                Row::read(r, 0)?,
                r.get::<_, f64>(ROW_COLUMN_COUNT)?,
                r.get::<_, String>(ROW_COLUMN_COUNT + 1)?,
            ))
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
            Hit {
                score: score(bm25, row.date, now_ms, row.has_reply),
                did,
                row,
                other_project: false,
            }
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
    let bad = || {
        RecallError::bad(format!(
            "bad id '{id}': expected <session>:<entry> as printed on a card"
        ))
    };
    let (prefix, entry) = id.rsplit_once(':').ok_or_else(bad)?;
    let target: i64 = entry.parse().map_err(|_| bad())?;
    if prefix.is_empty() {
        return Err(bad());
    }
    let session = resolve_session(conn, prefix)?;
    let rows = |tail: &str, n: i64| -> Result<Vec<Row>, RecallError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {ROW_COLUMNS} FROM turns t WHERE t.session = ?1 {tail}"
        ))?;
        let rows = stmt
            .query_map(params![session, target, n], |r| Row::read(r, 0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    };
    let found = rows("AND t.entry_id = ?2 LIMIT ?3", 1)?;
    if found.is_empty() {
        return Err(RecallError::unknown(format!(
            "no turn {target} in session {session}"
        )));
    }
    let n = around as i64;
    let mut turns = rows("AND t.entry_id < ?2 ORDER BY t.entry_id DESC LIMIT ?3", n)?;
    turns.reverse();
    turns.extend(found);
    turns.extend(rows("AND t.entry_id > ?2 ORDER BY t.entry_id LIMIT ?3", n)?);
    Ok(Shown {
        session,
        target,
        turns,
    })
}

fn resolve_session(conn: &Connection, prefix: &str) -> Result<String, RecallError> {
    let exact: i64 = conn.query_row(
        "SELECT count(*) FROM sessions WHERE session = ?1",
        [prefix],
        |r| r.get(0),
    )?;
    if exact > 0 {
        return Ok(prefix.to_string());
    }
    let mut stmt =
        conn.prepare("SELECT session FROM sessions WHERE substr(session, 1, length(?1)) = ?1 ORDER BY session LIMIT 11")?;
    let found: Vec<String> = stmt
        .query_map([prefix], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    match found.as_slice() {
        [] => Err(RecallError::unknown(format!(
            "no session matches '{prefix}'"
        ))),
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
