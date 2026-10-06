//! One session file's bytes to the turns on its active branch.

use crate::text::cap;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

/// Prompts gray injects on its own (found by surveying every user text in
/// the local sessions). They never start a turn and their text is dropped.
pub const INJECTED_PREFIXES: &[&str] = &[
    "[Background task notification]",
    "Another language model started to solve this problem",
    "[gray loop guard:",
    "The conversation history before this point was compacted",
];
/// Entries this close to a compaction boundary are copies of kept messages.
pub const COMPACTION_WINDOW_MS: i64 = 500;
/// Prompt cap, in bytes.
pub const PROMPT_CAP: usize = 2048;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolUse {
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

/// A real user prompt and everything the agent did until the next one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Turn {
    pub entry_id: i64,
    /// Prompt timestamp, unix ms.
    pub date: i64,
    /// Absolute byte offset of the prompt's line in the file.
    pub offset: u64,
    pub prompt: String,
    /// Non-blank assistant text blocks, in order.
    pub texts: Vec<String>,
    pub tools: Vec<ToolUse>,
    pub results: Vec<ToolResult>,
}

#[derive(Debug, Default)]
pub struct Parsed {
    /// Header cwd; only read when parsing from offset 0.
    pub cwd: Option<String>,
    pub turns: Vec<Turn>,
    /// First entry of the active branch within the parsed bytes.
    pub root: Option<i64>,
    /// Absolute offsets of complete lines that were not valid entries.
    pub skipped_offsets: Vec<u64>,
}

struct Entry {
    id: i64,
    parent: Option<i64>,
    ts: i64,
    offset: u64,
    boundary: bool,
    role: String,
    blocks: Vec<Value>,
}

enum Line {
    Entry(Entry),
    Header(Option<String>),
    Bad,
}

/// Parse `bytes`, which start at absolute file offset `base`. A last line
/// without a trailing newline is not consumed; the next call reads it.
pub fn parse(bytes: &[u8], base: u64) -> Parsed {
    let mut out = Parsed::default();
    let mut entries = Vec::new();
    let mut pos = 0;
    while let Some(len) = bytes[pos..].iter().position(|b| *b == b'\n') {
        let line = &bytes[pos..pos + len];
        let offset = base + pos as u64;
        pos += len + 1;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match read_line(line, offset) {
            Line::Entry(e) => entries.push(e),
            Line::Header(cwd) if offset == 0 => out.cwd = cwd,
            _ => out.skipped_offsets.push(offset),
        }
    }
    let branch = active_branch(&entries);
    out.root = branch.first().map(|&i| entries[i].id);
    out.turns = turns(entries, &branch);
    out
}

fn read_line(line: &[u8], offset: u64) -> Line {
    let Ok(Value::Object(mut obj)) = serde_json::from_slice::<Value>(line) else {
        return Line::Bad;
    };
    let Some(id) = obj.get("entry_id").and_then(Value::as_i64) else {
        return if obj.contains_key("id") {
            Line::Header(obj.get("cwd").and_then(Value::as_str).map(str::to_string))
        } else {
            Line::Bad
        };
    };
    let mut msg = obj.remove("message").unwrap_or(Value::Null);
    let role = msg
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let blocks = match msg.get_mut("content").map(Value::take) {
        Some(Value::Array(a)) => a,
        Some(Value::String(s)) => vec![json!({"type": "text", "text": s})],
        _ => Vec::new(),
    };
    Line::Entry(Entry {
        id,
        parent: obj.get("parent_id").and_then(Value::as_i64),
        ts: obj.get("timestamp").and_then(Value::as_i64).unwrap_or(0),
        offset,
        boundary: obj
            .get("compaction_boundary")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        role,
        blocks,
    })
}

/// Indices (file order) of the entries on the path from the last entry back
/// through `parent_id`. Rewound branches are off that path.
fn active_branch(entries: &[Entry]) -> Vec<usize> {
    let index: HashMap<i64, usize> = entries.iter().enumerate().map(|(i, e)| (e.id, i)).collect();
    let mut on = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = entries.len().checked_sub(1);
    while let Some(i) = cur {
        if !seen.insert(i) {
            break;
        }
        on.push(i);
        cur = entries[i].parent.and_then(|p| index.get(&p).copied());
    }
    on.sort_unstable();
    on
}

fn kind(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

fn turns(mut entries: Vec<Entry>, branch: &[usize]) -> Vec<Turn> {
    let mut out = Vec::new();
    let mut cur: Option<Turn> = None;
    let mut boundary: Option<i64> = None;
    for &i in branch {
        let e = &mut entries[i];
        if e.boundary {
            boundary = Some(e.ts);
            continue;
        }
        if let Some(b) = boundary {
            if (e.ts - b).abs() <= COMPACTION_WINDOW_MS {
                continue;
            }
            boundary = None;
        }
        let blocks = std::mem::take(&mut e.blocks);
        match e.role.as_str() {
            "user" => {
                if let Some(prompt) = prompt_of(&blocks) {
                    out.extend(cur.take());
                    cur = Some(Turn {
                        entry_id: e.id,
                        date: e.ts,
                        offset: e.offset,
                        prompt,
                        ..Turn::default()
                    });
                } else if let Some(t) = cur.as_mut() {
                    t.results
                        .extend(blocks.iter().filter(|b| kind(b) == "tool_result").map(|b| {
                            ToolResult {
                                content: match b.get("content") {
                                    Some(Value::String(s)) => s.clone(),
                                    Some(v) => v.to_string(),
                                    None => String::new(),
                                },
                                is_error: b
                                    .get("is_error")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                            }
                        }));
                }
            }
            "assistant" => {
                let Some(t) = cur.as_mut() else { continue };
                for b in &blocks {
                    match kind(b) {
                        "text" => {
                            if let Some(s) = b
                                .get("text")
                                .and_then(Value::as_str)
                                .filter(|s| !s.trim().is_empty())
                            {
                                t.texts.push(s.to_string());
                            }
                        }
                        "tool_use" => t.tools.push(ToolUse {
                            name: b
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            args: b.get("args").cloned().unwrap_or(Value::Null),
                        }),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out.extend(cur);
    out
}

/// The prompt a user message opens a turn with, or `None` when it is not a
/// real prompt (tool results, image-only, injected text).
fn prompt_of(blocks: &[Value]) -> Option<String> {
    if blocks.iter().any(|b| kind(b) == "tool_result") {
        return None;
    }
    let text = blocks
        .iter()
        .filter(|b| kind(b) == "text")
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    if text.is_empty() || INJECTED_PREFIXES.iter().any(|p| text.starts_with(p)) {
        return None;
    }
    if let Some(rest) = text.strip_prefix("<skill name=\"") {
        return Some(format!("[skill {}]", rest.split('"').next().unwrap_or("")));
    }
    Some(cap(text, PROMPT_CAP).to_string())
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod session_tests;
