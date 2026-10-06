//! One argument surface: `gray recall` argv, `/recall` argv and the tool's
//! JSON all become a [`Request`] and run through [`run_request`].

use crate::query::{self, Context, RecallError, Request};
use crate::{COMMANDS, PLUGIN_NAME, PROTOCOL, catchup, index, render};
use clap::{Args, Parser, Subcommand};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::path::Path;
use std::time::Instant;

pub const TOOL_NAME: &str = "recall";
/// Words that pick a subcommand when they come first.
pub const SUBCOMMANDS: &[&str] = &["search", "show", "projects", "status", "reindex"];
const PASS_THROUGH: &[&str] = &["-h", "--help", "-V", "--version"];
const TOOL_DESCRIPTION: &str = "Search this machine's past gray sessions for prior work: how a bug was fixed before, \
    what was decided and why, what was done in a project, the history of a file. Returns short cards (the prompt, \
    what was done, files, commits), current project first; pass `id` from a card to read that turn in full.";

#[derive(Parser, Debug)]
#[command(
    name = "gray recall",
    version,
    about = "Search past gray sessions for prior work",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Search turns (the default when the first word is not a subcommand)
    Search(SearchArgs),
    /// Show one turn in full, by the id printed on a card
    Show(ShowArgs),
    /// List the indexed projects
    Projects(ListArgs),
    /// Index counts, size and last catch-up
    Status(StatusArgs),
    /// Delete the index and rebuild it from the session files
    Reindex,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Words (any may match), "exact phrase" (required), 'word -excluded' (quoted)
    pub words: Vec<String>,
    /// Project name or path (default: the current project)
    #[arg(short, long)]
    pub project: Option<String>,
    /// Search every project
    #[arg(short, long)]
    pub all: bool,
    /// Only turns that touched a path containing this
    #[arg(short, long)]
    pub file: Option<String>,
    /// YYYY, YYYY-MM, YYYY-MM-DD, or Nd / Nw / Nm ago
    #[arg(long)]
    pub since: Option<String>,
    /// Same formats as --since; inclusive
    #[arg(long)]
    pub until: Option<String>,
    /// Only turns with a failed command
    #[arg(long)]
    pub failed: bool,
    /// Results to show (default 5, max 20)
    #[arg(short = 'n', long)]
    pub limit: Option<usize>,
    /// Skip this many results
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
    /// Print JSON
    #[arg(long)]
    pub json: bool,
    /// Output budget in characters (default 2500)
    #[arg(long)]
    pub max_chars: Option<usize>,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// `<session>:<entry>`; a unique session prefix is enough
    pub id: String,
    /// Also show this many turns before and after (max 10)
    #[arg(long, default_value_t = 0)]
    pub around: usize,
    /// Print JSON
    #[arg(long)]
    pub json: bool,
    /// Output budget in characters (default 8000)
    #[arg(long)]
    pub max_chars: Option<usize>,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Print JSON
    #[arg(long)]
    pub json: bool,
    /// Output budget in characters (default 2500)
    #[arg(long)]
    pub max_chars: Option<usize>,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Print JSON
    #[arg(long)]
    pub json: bool,
}

/// Text for stdout (code 0) or stderr, and the exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub text: String,
    pub code: i32,
}

impl Output {
    fn from_result(r: Result<String, RecallError>) -> Self {
        match r {
            Ok(text) => Self { text, code: 0 },
            Err(e) => Self {
                text: format!("recall: {}", e.message),
                code: e.code,
            },
        }
    }
}

/// `search` in front unless the first word is a subcommand or a help flag.
pub fn normalize(argv: &[String]) -> Vec<String> {
    match argv.first() {
        Some(w) if SUBCOMMANDS.contains(&w.as_str()) || PASS_THROUGH.contains(&w.as_str()) => {
            argv.to_vec()
        }
        _ => std::iter::once("search".to_string())
            .chain(argv.iter().cloned())
            .collect(),
    }
}

/// Positional words as one query; a word with spaces is a phrase unless it
/// already holds quotes or exclusions.
pub fn query_from(words: &[String]) -> String {
    let terms: Vec<String> = words
        .iter()
        .map(|w| {
            let w = w.trim();
            let excludes = w
                .split_whitespace()
                .any(|t| t.len() > 1 && t.starts_with('-'));
            if w.contains(char::is_whitespace) && !w.contains('"') && !excludes {
                format!("\"{w}\"")
            } else {
                w.to_string()
            }
        })
        .collect();
    terms.join(" ")
}

/// Run `gray recall` / `/recall` arguments.
pub fn run_argv(home: &Path, argv: &[String], ctx: &Context) -> Output {
    let cli = match Cli::try_parse_from(
        std::iter::once("gray recall".to_string()).chain(normalize(argv)),
    ) {
        Ok(cli) => cli,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            return Output {
                text: e.to_string().trim_end().to_string(),
                code,
            };
        }
    };
    match cli.command {
        Command::Search(a) => {
            let req = Request {
                query: query_from(&a.words),
                project: a.project,
                all: a.all,
                file: a.file,
                since: a.since,
                until: a.until,
                failed: a.failed,
                limit: a.limit,
                offset: a.offset,
                json: a.json,
                max_chars: a.max_chars,
                ..Request::default()
            };
            run_request(home, &req, ctx)
        }
        Command::Show(a) => {
            let req = Request {
                id: Some(a.id),
                around: a.around,
                json: a.json,
                max_chars: a.max_chars,
                ..Request::default()
            };
            run_request(home, &req, ctx)
        }
        Command::Projects(a) => Output::from_result(projects(home, &a)),
        Command::Status(a) => Output::from_result(status(home, a.json, ctx.now_ms)),
        Command::Reindex => Output::from_result(reindex(home)),
    }
}

/// Catch up, then search or show.
pub fn run_request(home: &Path, req: &Request, ctx: &Context) -> Output {
    Output::from_result(answer(home, req, ctx))
}

/// `tool/call` for `recall`: `{content, is_error}`; zero hits are not an error.
pub fn tool_call(home: &Path, args: &Value, ctx: &Context) -> Value {
    let out = Output::from_result(request_from_json(args).and_then(|req| answer(home, &req, ctx)));
    json!({"content": out.text, "is_error": out.code != 0})
}

/// The index, caught up; `true` when another process holds the writer lock.
fn open_fresh(home: &Path) -> Result<(Connection, bool), RecallError> {
    let mut conn = index::open(home)?;
    let report = catchup::catch_up(&mut conn, home)?;
    Ok((conn, report.busy))
}

fn answer(home: &Path, req: &Request, ctx: &Context) -> Result<String, RecallError> {
    let (conn, busy) = open_fresh(home)?;
    if let Some(id) = &req.id {
        let shown = query::show(&conn, id, req.around())?;
        return Ok(if req.json {
            render::show_json(&shown, busy)
        } else {
            render::show_text(&shown, req.max_chars(), busy)
        });
    }
    let found = query::search(&conn, req, ctx)?;
    Ok(if req.json {
        render::search_json(&found, req, busy)
    } else {
        render::search_text(&found, req, busy)
    })
}

fn projects(home: &Path, a: &ListArgs) -> Result<String, RecallError> {
    let (conn, _) = open_fresh(home)?;
    let list = index::list_projects(&conn)?;
    let max = Request {
        max_chars: a.max_chars,
        ..Request::default()
    }
    .max_chars();
    Ok(if a.json {
        render::projects_json(&list)
    } else {
        render::projects_text(&list, max)
    })
}

/// The index as it is, without catching up: a diagnostic, not a refresh.
fn status(home: &Path, json: bool, now_ms: i64) -> Result<String, RecallError> {
    let conn = index::open(home)?;
    let busy = catchup::writer_lock(home)?.is_none();
    let st = index::stats(&conn, home)?;
    let db = index::db_path(home);
    Ok(if json {
        render::status_json(&st, &db, busy)
    } else {
        render::status_text(&st, &db, now_ms, busy)
    })
}

fn reindex(home: &Path) -> Result<String, RecallError> {
    let mut conn = index::open(home)?;
    let Some(_lock) = catchup::writer_lock(home)? else {
        return Err(RecallError::error(
            "another session is catching up the index; try again in a moment",
        ));
    };
    let started = Instant::now();
    index::reset(&conn)?;
    let report = catchup::catch_up_locked(&mut conn, home)?;
    let st = index::stats(&conn, home)?;
    let failed = if report.failed > 0 {
        format!(
            " · {} unreadable",
            render::plural(report.failed as i64, "file")
        )
    } else {
        String::new()
    };
    Ok(format!(
        "reindexed {} ({}) in {:.1}s{failed}",
        render::plural(report.changed as i64, "session"),
        render::plural(st.turns, "turn"),
        started.elapsed().as_secs_f64()
    ))
}

/// The tool's JSON arguments as a [`Request`].
pub fn request_from_json(args: &Value) -> Result<Request, RecallError> {
    let empty = Map::new();
    let obj = match args {
        Value::Object(m) => m,
        Value::Null => &empty,
        _ => return Err(RecallError::bad("tool arguments must be an object")),
    };
    Ok(Request {
        query: text(obj, "query")?.unwrap_or_default(),
        project: text(obj, "project")?,
        all: flag(obj, "all")?,
        file: text(obj, "file")?,
        since: text(obj, "since")?,
        until: text(obj, "until")?,
        failed: flag(obj, "failed")?,
        limit: number(obj, "limit")?,
        offset: number(obj, "offset")?.unwrap_or(0),
        id: text(obj, "id")?,
        around: number(obj, "around")?.unwrap_or(0),
        json: flag(obj, "json")?,
        max_chars: number(obj, "max_chars")?,
    })
}

fn text(obj: &Map<String, Value>, key: &str) -> Result<Option<String>, RecallError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone()).filter(|s| !s.is_empty())),
        Some(Value::Number(n)) => Ok(Some(n.to_string())),
        Some(_) => Err(RecallError::bad(format!("{key} must be a string"))),
    }
}

fn flag(obj: &Map<String, Value>, key: &str) -> Result<bool, RecallError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::String(s)) if s == "true" || s == "false" => Ok(s == "true"),
        Some(_) => Err(RecallError::bad(format!("{key} must be true or false"))),
    }
}

fn number(obj: &Map<String, Value>, key: &str) -> Result<Option<usize>, RecallError> {
    let bad = || RecallError::bad(format!("{key} must be a non-negative integer"));
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => s.trim().parse().map(Some).map_err(|_| bad()),
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| {
                n.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                    .map(|f| f as u64)
            })
            .map(|v| Some(v as usize))
            .ok_or_else(bad),
        Some(_) => Err(bad()),
    }
}

/// Who is calling, from a wire request's params.
pub fn wire_context(params: &Value) -> Context {
    let field = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    Context {
        cwd: field(&params["session"]["cwd"])
            .or_else(|| field(&params["cwd"]))
            .unwrap_or_else(process_cwd),
        session: field(&params["session"]["id"]),
        now_ms: query::now_ms(),
    }
}

/// Who is calling in CLI mode: the process cwd and `GRAY_SESSION_ID`.
pub fn cli_context() -> Context {
    Context {
        cwd: process_cwd(),
        session: std::env::var("GRAY_SESSION_ID")
            .ok()
            .filter(|s| !s.is_empty()),
        now_ms: query::now_ms(),
    }
}

fn process_cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn tool_def() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": TOOL_DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Words (any may match; more matches rank higher), \"exact phrase\" (required), -word (excluded). Empty lists the newest turns."},
                "project": {"type": "string", "description": "Project name or path. Default: the current project, widening to the others when nothing matches."},
                "all": {"type": "boolean", "description": "Search every project."},
                "file": {"type": "string", "description": "Only turns that touched a file path containing this."},
                "since": {"type": "string", "description": "YYYY, YYYY-MM, YYYY-MM-DD, or Nd / Nw / Nm ago."},
                "until": {"type": "string", "description": "Same formats as since; inclusive."},
                "failed": {"type": "boolean", "description": "Only turns where a command failed."},
                "limit": {"type": "integer", "description": "Results to show: default 5, max 20."},
                "offset": {"type": "integer", "description": "Skip this many results (paging)."},
                "id": {"type": "string", "description": "Show this turn in full (<session>:<entry> from a card) instead of searching."},
                "around": {"type": "integer", "description": "With id: also show this many turns before and after (max 10)."},
                "json": {"type": "boolean", "description": "Return JSON instead of text."},
                "max_chars": {"type": "integer", "description": "Output budget: default 2500 for search, 8000 with id."}
            }
        },
        "snippet": "recall <words> [project] [since]",
    })
}

pub fn manifest() -> Value {
    json!({
        "name": PLUGIN_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL,
        "tools": [tool_def()],
        "commands": COMMANDS,
        "hooks": [],
        "completion": SUBCOMMANDS,
    })
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod cli_tests;
