#!/usr/bin/env python3
"""Copy gray-memory's src/redact.rs with path stripping removed.

Usage: trim_redact.py <gray-memory/src/redact.rs> <gray-recall/src/redact.rs>
"""
import re, sys

src, dst = sys.argv[1], sys.argv[2]
lines = open(src, encoding="utf-8").read().split("\n")
DROP_FNS = {"looks_absolute", "classify_path", "trim_path_punctuation", "unescape_path", "looks_relative"}
DROP_CONSTS = ("const PATH_PLACEHOLDER", "pub const REDACTION_ABSOLUTE_PATH", "pub const REDACTION_RELATIVE_PATH")
GONE = DROP_FNS | {"PATH_PLACEHOLDER", "REDACTION_ABSOLUTE_PATH", "REDACTION_RELATIVE_PATH"}


def code(line):
    line = re.sub(r'"(?:\\.|[^"\\])*"', '""', line)
    line = re.sub(r"'(?:\\.|[^'\\])'", "''", line)
    return line.split("//")[0]


def block_end(i):
    depth, opened = 0, False
    for j in range(i, len(lines)):
        c = code(lines[j])
        depth += c.count("{") - c.count("}")
        opened = opened or "{" in c
        if opened and depth == 0:
            return j
    sys.exit(f"unbalanced block at line {i + 1}")


def lead_start(i, prefixes):
    while i > 0 and lines[i - 1].lstrip().startswith(prefixes):
        i -= 1
    return i


drop = set()
for i, line in enumerate(lines):
    m = re.match(r"\s*(?:pub\s+)?fn\s+(\w+)", line)
    if m and m.group(1) in DROP_FNS:
        drop.update(range(lead_start(i, ("///", "#[")), block_end(i) + 1))
    elif line.lstrip().startswith(DROP_CONSTS):
        drop.update(range(lead_start(i, ("///",)), i + 1))
    elif re.match(r"\s*if let Some\(kind\) = classify_path\(", line):
        drop.update(range(lead_start(i, ("//",)), block_end(i) + 1))

kept = [l for i, l in enumerate(lines) if i not in drop]
start = next(i for i, l in enumerate(kept) if l.startswith("use std::collections::BTreeSet;"))
HEADER = """// Copied from vstaln/gray-memory `src/redact.rs` with path stripping removed,
// so file paths stay searchable. That file is vendored from gray
// `crates/gray-core/src/redaction.rs`, itself vendored from Hmbown/CodeWhale
// (MIT) `crates/workflow/src/redaction.rs`.
// Copyright (c) 2024-2025 DeepSeek CLI Contributors. See THIRD_PARTY_NOTICES.md.
//! Secret redaction for everything gray-recall writes to its index.
//!
//! Secret-shaped tokens (provider keys, bearer tokens, `SOMETHING_KEY=value`
//! assignments, Discord bot tokens) become `<redacted>`. Paths are kept:
//! they are what `--file` searches.

use std::sync::LazyLock;

use regex::Regex;
"""
TAIL = r'''
/// A Discord bot token: three base64url segments joined by dots.
static DISCORD_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[MNO][A-Za-z0-9_-]{23,27}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27,40}\b")
        .expect("valid regex")
});

/// `input` with every secret replaced by [`REDACTED`]; everything else,
/// paths included, is kept verbatim.
#[must_use]
pub fn redact_secrets(input: &str) -> String {
    let text = redact_secret_tokens(input).into_text();
    DISCORD_TOKEN.replace_all(&text, REDACTED).into_owned()
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod redact_tests;
'''
out = HEADER + "\n".join(kept[start:])
out = out.replace("redact_for_disclosure", "redact_secret_tokens")
out = out.replace("/// Redact absolute paths and secret-shaped tokens from `input`.",
                  "/// Redact secret-shaped tokens from `input`; paths are kept.")
out = re.sub(r"\n{3,}", "\n\n", out).rstrip("\n") + "\n" + TAIL
left = sorted({n for l in out.split("\n") for n in GONE if n in code(l)})
if left:
    sys.exit(f"still referenced in code: {left}")
open(dst, "w", encoding="utf-8").write(out)
print(f"wrote {dst}: {len(lines)} -> {out.count(chr(10))} lines, dropped {len(drop)}")
