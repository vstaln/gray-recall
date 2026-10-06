//! Builds session files shaped like gray's, for tests.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Header timestamp; entries follow at one-second steps (2026-09-21 UTC).
pub const T0: i64 = 1_790_000_000_000;

pub struct SessionBuilder {
    id: String,
    lines: Vec<String>,
    next: i64,
    last: Option<i64>,
    parent: Option<i64>,
    ts: i64,
}

impl SessionBuilder {
    pub fn new(id: &str, cwd: &str) -> Self {
        let header = json!({"version": 1, "id": id, "timestamp": T0, "cwd": cwd, "model": "test"});
        Self {
            id: id.to_string(),
            lines: vec![header.to_string()],
            next: 1,
            last: None,
            parent: None,
            ts: T0,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn entry_at(&mut self, ts: i64, role: &str, content: Value, boundary: bool) -> i64 {
        let id = self.next;
        self.next += 1;
        let parent = self.parent.take().or(self.last);
        let mut e = json!({"entry_id": id, "parent_id": parent, "timestamp": ts,
                           "message": {"role": role, "content": content}});
        if boundary {
            e["compaction_boundary"] = json!(true);
        }
        self.lines.push(e.to_string());
        self.last = Some(id);
        id
    }

    fn entry(&mut self, role: &str, content: Value) -> i64 {
        self.ts += 1000;
        let ts = self.ts;
        self.entry_at(ts, role, content, false)
    }

    pub fn user(&mut self, text: &str) -> i64 {
        self.entry("user", json!([{"type": "text", "text": text}]))
    }

    pub fn assistant(&mut self, text: &str) -> i64 {
        self.entry("assistant", json!([{"type": "text", "text": text}]))
    }

    /// An assistant `tool_use` and the user `tool_result` answering it; returns the result's id.
    pub fn tool(&mut self, name: &str, args: Value, result: &str, is_error: bool) -> i64 {
        let call = format!("call_{}", self.next);
        self.entry(
            "assistant",
            json!([{"type": "tool_use", "id": call, "name": name, "args": args}]),
        );
        self.entry(
            "user",
            json!([{"type": "tool_result", "id": call, "content": result, "is_error": is_error}]),
        )
    }

    pub fn bash(&mut self, command: &str, result: &str) -> i64 {
        self.tool("bash", json!({"command": command}), result, false)
    }

    pub fn image_only(&mut self) -> i64 {
        self.entry(
            "user",
            json!([{"type": "image", "mime": "image/png", "data": "AAAA"}]),
        )
    }

    /// The next entry's parent is `parent` (a rewind), not the last entry.
    pub fn branch_from(&mut self, parent: i64) -> &mut Self {
        self.parent = Some(parent);
        self
    }

    /// The next entry is stamped `ts + 1000`.
    pub fn clock(&mut self, ts: i64) -> &mut Self {
        self.ts = ts;
        self
    }

    /// A compaction boundary followed by copies of `kept`, all stamped with the boundary's time.
    pub fn compaction(&mut self, kept: &[&str]) {
        self.ts += 1000;
        let ts = self.ts;
        let marker =
            "compaction boundary: entries before this point are superseded; replay starts after it";
        self.entry_at(
            ts,
            "system",
            json!([{"type": "text", "text": marker}]),
            true,
        );
        for text in kept {
            self.entry_at(ts, "user", json!([{"type": "text", "text": text}]), false);
        }
    }

    pub fn raw(&mut self, line: &str) {
        self.lines.push(line.to_string());
    }

    pub fn text(&self) -> String {
        let mut s = self.lines.join("\n");
        s.push('\n');
        s
    }

    pub fn write(&self, dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{}.jsonl", self.id));
        std::fs::write(&path, self.text()).unwrap();
        path
    }
}
