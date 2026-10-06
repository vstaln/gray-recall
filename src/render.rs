//! Text and JSON views of search, show, projects and status. Text views fit
//! the request's character budget; JSON views are bounded by the result
//! limit and per-field caps instead.

use crate::index::{ProjectInfo, Row, Stats};
use crate::project::display_id;
use crate::query::{Hit, Request, Search, Shown};
use crate::text::{cap, clip};
use chrono::{Local, TimeZone};
use serde_json::{Value, json};
use std::path::Path;

pub const BUSY_NOTE: &str = "(index catching up in another session)";
/// Longest query or filter value echoed in a header.
const ECHO_CHARS: usize = 60;
const FILES_CHARS: usize = 120;
const MAX_COMMITS: usize = 3;
const NAME_CHARS: usize = 24;
/// Prompt cap in JSON search hits and show neighbors.
const JSON_PROMPT_CHARS: usize = 500;

/// How much of a turn a card shows.
#[derive(Clone, Copy)]
struct Level {
    you: usize,
    did: usize,
    extras: bool,
}

/// Search card forms, richest first; the last is the floor.
const CARD_LEVELS: [Level; 3] = [
    Level {
        you: 200,
        did: 300,
        extras: true,
    },
    Level {
        you: 100,
        did: 140,
        extras: false,
    },
    Level {
        you: 60,
        did: 0,
        extras: false,
    },
];
/// Show neighbor forms, richest first.
const NEIGHBOR_LEVELS: [Level; 2] = [
    Level {
        you: 150,
        did: 150,
        extras: false,
    },
    Level {
        you: 60,
        did: 0,
        extras: false,
    },
];

pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `1 turn`, `4,140 turns`.
pub fn plural(n: i64, word: &str) -> String {
    format!("{} {word}{}", thousands(n), if n == 1 { "" } else { "s" })
}

/// `24.0 MB`, `512 KB`, `12 B`.
pub fn size(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b >= K * K {
        format!("{:.1} MB", b / K / K)
    } else if b >= K {
        format!("{:.0} KB", b / K)
    } else {
        format!("{bytes} B")
    }
}

/// Local `YYYY-MM-DD HH:MM`.
pub fn when(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).earliest().map_or_else(
        || "?".to_string(),
        |t| t.format("%Y-%m-%d %H:%M").to_string(),
    )
}

fn ago(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s < 60 {
        "just now".to_string()
    } else if s < 3600 {
        format!("{} min ago", s / 60)
    } else if s < 86_400 {
        format!("{} h ago", s / 3600)
    } else {
        format!("{} d ago", s / 86_400)
    }
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// `s` cut to `max` chars, ending in `…` when cut; newlines kept.
fn fit(s: &str, max: usize) -> String {
    if chars(s) <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    if max > 0 {
        out.push('…');
    }
    out
}

/// `1900bae3:3154 · 2026-09-15 21:18 · ✗ failed cmd`
fn turn_head(r: &Row) -> String {
    let mut out = format!("{} · {}", display_id(&r.session, r.entry_id), when(r.date));
    if r.failed {
        out.push_str(" · ✗ failed cmd");
    }
    out
}

/// `files: a, b   commits: 2ba5532`, empty when the turn has neither.
fn extras(r: &Row) -> String {
    let mut parts = Vec::new();
    if !r.files.is_empty() {
        parts.push(format!("files: {}", clip(&r.files.join(", "), FILES_CHARS)));
    }
    if !r.commits.is_empty() {
        let commits: Vec<&str> = r
            .commits
            .iter()
            .take(MAX_COMMITS)
            .map(|c| cap(c, 7))
            .collect();
        parts.push(format!("commits: {}", commits.join(" ")));
    }
    parts.join("   ")
}

/// The indented `you:` / `did:` / extras lines under a card head.
fn body(r: &Row, did: &str, l: Level) -> String {
    let mut out = format!("\n  you: {}", clip(&r.prompt, l.you));
    if l.did > 0 && !did.is_empty() {
        out += &format!("\n  did: {}", clip(did, l.did));
    }
    let extras = if l.extras { extras(r) } else { String::new() };
    if !extras.is_empty() {
        out += &format!("\n  {extras}");
    }
    out
}

fn card(n: usize, h: &Hit, all: bool, l: Level) -> String {
    let r = &h.row;
    let place = if h.other_project {
        format!(" · OTHER PROJECT {}", r.project_name)
    } else if all {
        format!(" · {}", r.project_name)
    } else {
        String::new()
    };
    format!("\n[{n}] {}{place}{}", turn_head(r), body(r, &h.did, l))
}

fn search_header(s: &Search, req: &Request, shown: usize) -> String {
    let mut parts = vec![
        "recall".to_string(),
        format!("{} ({})", s.scope, plural(s.scope_turns, "turn")),
    ];
    if !req.query.trim().is_empty() {
        let q = clip(&req.query, ECHO_CHARS);
        parts.push(if q.contains('"') {
            q
        } else {
            format!(r#""{q}""#)
        });
    }
    for (label, value) in [
        ("since", &req.since),
        ("until", &req.until),
        ("file", &req.file),
    ] {
        if let Some(v) = value.as_deref().filter(|v| !v.trim().is_empty()) {
            parts.push(format!("{label} {}", clip(v, ECHO_CHARS)));
        }
    }
    if req.failed {
        parts.push("failed only".to_string());
    }
    let from = if req.offset > 0 {
        format!(" from [{}]", req.offset + 1)
    } else {
        String::new()
    };
    parts.push(format!(
        "shown {shown} of {}{from}",
        thousands(s.total as i64)
    ));
    parts.join(" · ")
}

fn search_notes(s: &Search, busy: bool) -> String {
    let mut out = String::new();
    if s.fell_back {
        out += &format!("\nno match in {}; searched the other projects", s.scope);
    }
    if busy {
        out += &format!("\n{BUSY_NOTE}");
    }
    out
}

fn none_found(s: &Search) -> &'static str {
    if s.total > 0 {
        "none on this page: lower the offset"
    } else if s.all || s.fell_back {
        "none found in any project: try fewer or other words, or a wider date range"
    } else {
        "none found: try --all, fewer or other words, or a wider date range"
    }
}

/// Header, notes, as many cards as fit `max_chars` (richest forms first)
/// and a `next:` footer.
pub fn search_text(s: &Search, req: &Request, busy: bool) -> String {
    let max = req.max_chars();
    let notes = search_notes(s, busy);
    let Some(first) = s.hits.first() else {
        return fit(
            &format!("{}{notes}\n{}", search_header(s, req, 0), none_found(s)),
            max,
        );
    };
    let footer = format!(
        "\nnext: recall id={} around=2 for detail · resume: gray -r {}",
        display_id(&first.row.session, first.row.entry_id),
        first.row.session
    );
    let forms: Vec<[String; 3]> = s
        .hits
        .iter()
        .enumerate()
        .map(|(i, h)| CARD_LEVELS.map(|l| card(req.offset + i + 1, h, s.all, l)))
        .collect();
    let fixed = chars(&search_header(s, req, s.hits.len())) + chars(&notes) + chars(&footer);
    let mut budget = max.saturating_sub(fixed);
    // As many cards as fit in their floor form, then the richest form each
    // can afford while the rest still fit at their floor.
    let mut floor_rest = 0;
    let count = forms
        .iter()
        .take_while(|f| {
            floor_rest += chars(&f[2]);
            floor_rest <= budget
        })
        .count();
    let mut floor_rest: usize = forms[..count].iter().map(|f| chars(&f[2])).sum();
    let mut cards = String::new();
    for f in &forms[..count] {
        floor_rest -= chars(&f[2]);
        let c = f
            .iter()
            .find(|c| chars(c) + floor_rest <= budget)
            .unwrap_or(&f[2]);
        budget -= chars(c);
        cards.push_str(c);
    }
    fit(
        &format!("{}{notes}{cards}{footer}", search_header(s, req, count)),
        max,
    )
}

/// The same data as objects; prompts cut to 500 chars.
pub fn search_json(s: &Search, req: &Request, busy: bool) -> String {
    let hits: Vec<Value> = s
        .hits
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let r = &h.row;
            json!({
                "n": req.offset + i + 1,
                "id": display_id(&r.session, r.entry_id),
                "session": r.session,
                "entry_id": r.entry_id,
                "project": r.project_name,
                "project_key": r.project_key,
                "cwd": r.cwd,
                "date": r.date,
                "time": when(r.date),
                "prompt": fit(&r.prompt, JSON_PROMPT_CHARS),
                "did": h.did,
                "files": r.files,
                "commits": r.commits,
                "failed": r.failed,
                "has_reply": r.has_reply,
                "score": h.score,
                "other_project": h.other_project,
            })
        })
        .collect();
    json!({
        "scope": s.scope,
        "scope_turns": s.scope_turns,
        "all": s.all,
        "query": req.query,
        "total": s.total,
        "offset": req.offset,
        "shown": hits.len(),
        "fell_back": s.fell_back,
        "busy": busy,
        "hits": hits,
    })
    .to_string()
}

fn neighbor(r: &Row, l: Level) -> String {
    format!("\n- {}{}", turn_head(r), body(r, &r.gist, l))
}

/// The target turn with `p` / `r` / `c` chars of prompt, reply and commands.
fn target(t: &Row, p: usize, r: usize, c: usize) -> String {
    let mut out = format!("\n▶ {}\nyou: {}", turn_head(t), fit(&t.prompt, p));
    if t.reply.is_empty() {
        out.push_str("\nreply: (none)");
    } else {
        out += &format!("\nreply:\n{}", fit(&t.reply, r));
    }
    if !t.commands.is_empty() {
        out += &format!("\ncommands:\n{}", fit(&t.commands, c));
    }
    let extras = extras(t);
    if !extras.is_empty() {
        out += &format!("\n{extras}");
    }
    out
}

/// The target turn in full and its neighbors, fitted to `max` chars: far
/// neighbors shrink, then drop; the target's prompt, reply and commands
/// share what is left.
pub fn show_text(sh: &Shown, max: usize, busy: bool) -> String {
    let Some(pos) = sh.turns.iter().position(|t| t.entry_id == sh.target) else {
        return fit(
            &format!("recall show {}:{} · not found", sh.session, sh.target),
            max,
        );
    };
    let t = &sh.turns[pos];
    let mut out = format!(
        "recall show {}:{} · project {} · {}",
        sh.session, sh.target, t.project_name, t.cwd
    );
    if busy {
        out += &format!("\n{BUSY_NOTE}");
    }
    let half = max / 2;
    let others: Vec<usize> = (0..sh.turns.len()).filter(|&i| i != pos).collect();
    let level = NEIGHBOR_LEVELS
        .into_iter()
        .find(|&l| {
            others
                .iter()
                .map(|&i| chars(&neighbor(&sh.turns[i], l)))
                .sum::<usize>()
                <= half
        })
        .unwrap_or(NEIGHBOR_LEVELS[1]);
    let mut nearest = others.clone();
    nearest.sort_by_key(|&i| i.abs_diff(pos));
    let mut kept: Vec<Option<String>> = vec![None; sh.turns.len()];
    let mut used = 0;
    for i in nearest {
        let n = neighbor(&sh.turns[i], level);
        if used + chars(&n) > half {
            break;
        }
        used += chars(&n);
        kept[i] = Some(n);
    }
    let dropped = others.len() - kept.iter().flatten().count();
    let left_out = if dropped > 0 {
        format!(
            "\n({} left out to fit max_chars)",
            plural(dropped as i64, "neighbor")
        )
    } else {
        String::new()
    };
    let footer = format!("{left_out}\nresume: gray -r {}", sh.session);
    let fixed = chars(&out) + used + chars(&target(t, 0, 0, 0)) + chars(&footer);
    let avail = max.saturating_sub(fixed);
    let (pn, rn, cn) = (chars(&t.prompt), chars(&t.reply), chars(&t.commands));
    let p = pn.min((avail / 4).max(avail.saturating_sub(rn + cn)));
    let rest = avail - p;
    let c = cn.min((rest / 3).max(rest.saturating_sub(rn)));
    let r = rn.min(rest - c);
    for (i, n) in kept.iter().enumerate() {
        if i == pos {
            out += &target(t, p, r, c);
        } else if let Some(n) = n {
            out += n;
        }
    }
    fit(&(out + &footer), max)
}

/// The target complete; neighbors as prompt (cut to 500 chars) and gist.
pub fn show_json(sh: &Shown, busy: bool) -> String {
    let turns: Vec<Value> = sh
        .turns
        .iter()
        .map(|t| {
            let is_target = t.entry_id == sh.target;
            let mut v = json!({
                "id": display_id(&t.session, t.entry_id),
                "entry_id": t.entry_id,
                "date": t.date,
                "time": when(t.date),
                "target": is_target,
                "failed": t.failed,
                "files": t.files,
                "commits": t.commits,
            });
            if is_target {
                v["prompt"] = json!(t.prompt);
                v["reply"] = json!(t.reply);
                v["commands"] = json!(t.commands);
                v["has_reply"] = json!(t.has_reply);
            } else {
                v["prompt"] = json!(fit(&t.prompt, JSON_PROMPT_CHARS));
                v["gist"] = json!(t.gist);
            }
            v
        })
        .collect();
    let t = sh.turns.iter().find(|t| t.entry_id == sh.target);
    json!({
        "session": sh.session,
        "target": sh.target,
        "project": t.map(|t| &t.project_name),
        "project_key": t.map(|t| &t.project_key),
        "cwd": t.map(|t| &t.cwd),
        "busy": busy,
        "turns": turns,
    })
    .to_string()
}

/// One aligned line per project, as many as fit `max` chars.
pub fn projects_text(ps: &[ProjectInfo], max: usize) -> String {
    let total: i64 = ps.iter().map(|p| p.turns).sum();
    let mut out = format!(
        "recall · {} · {}",
        plural(ps.len() as i64, "project"),
        plural(total, "turn")
    );
    if ps.is_empty() {
        out.push_str("\nnone indexed yet");
        return fit(&out, max);
    }
    let w = ps
        .iter()
        .map(|p| chars(&p.name))
        .max()
        .unwrap_or(0)
        .min(NAME_CHARS);
    let tw = ps
        .iter()
        .map(|p| thousands(p.turns).len())
        .max()
        .unwrap_or(0);
    let more = |n: usize| format!("\n… {n} more (--json lists all)");
    let reserve = chars(&more(ps.len()));
    for (i, p) in ps.iter().enumerate() {
        let line = format!(
            "\n  {:<w$}  {:>tw$}  {}",
            clip(&p.name, w),
            thousands(p.turns),
            p.key
        );
        let after = if i + 1 < ps.len() { reserve } else { 0 };
        if chars(&out) + chars(&line) + after > max {
            out += &more(ps.len() - i);
            break;
        }
        out += &line;
    }
    fit(&out, max)
}

pub fn projects_json(ps: &[ProjectInfo]) -> String {
    let list: Vec<Value> = ps
        .iter()
        .map(|p| json!({"name": p.name, "key": p.key, "turns": p.turns}))
        .collect();
    Value::Array(list).to_string()
}

pub fn status_text(st: &Stats, index: &Path, now_ms: i64, busy: bool) -> String {
    let last = st.last_catch_up.map_or_else(
        || "never".to_string(),
        |t| format!("{} ({})", when(t), ago(now_ms - t)),
    );
    let mut out = format!(
        "recall index · {} · {} · {}\nsize {} · skipped lines {} · last catch-up {last}\nfile {}",
        plural(st.turns, "turn"),
        plural(st.sessions, "session"),
        plural(st.projects, "project"),
        size(st.bytes),
        thousands(st.skipped),
        index.display()
    );
    if busy {
        out += &format!("\n{BUSY_NOTE}");
    }
    out
}

pub fn status_json(st: &Stats, index: &Path, busy: bool) -> String {
    json!({
        "turns": st.turns,
        "sessions": st.sessions,
        "projects": st.projects,
        "skipped": st.skipped,
        "bytes": st.bytes,
        "last_catch_up": st.last_catch_up,
        "index": index.display().to_string(),
        "busy": busy,
    })
    .to_string()
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod render_tests;
