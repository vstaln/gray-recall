# gray-recall: design

Date: 2026-10-06 · Status: draft for review

## Goal

Searchable memory of past gray sessions. `~/.gray/sessions` holds 459 MB
across 1,387 session files and nothing can search it. An agent or the user
asks "has this come up before", "what fixed it", "what did we do to this
file" and gets a few ranked, cited turns in roughly 500 tokens.

The idea was proven with a throwaway prototype (Python extractor plus the
leviathan CLI, in `/tmp/levgray`): 6,102 turns from 39 projects indexed in
1.1 s into 24 MB, and three test queries returned the right turn first.

## Decisions

| Question | Decision |
|---|---|
| How recall reaches the agent | On demand: a `recall` tool, plus `/recall` and `gray recall`. Nothing is injected into prompts. |
| What is indexed | Session turns only. Raw command output is not indexed. |
| Default scope | The current git repo; labeled fallback to other projects on zero hits; `--all` and `-p <name>` override. |
| Engine | One Rust sidecar binary, SQLite FTS5 + BM25, index caught up at query time. |
| Leviathan | Not a dependency, not in the name or code. Its design ideas are credited in `THIRD_PARTY_NOTICES.md`. |

## Non-goals (v1)

Auto-injecting results into prompts; indexing `~/.gray/shell` output;
gray-ledger integration; embeddings or semantic search; an MCP server;
generic CSV/SQLite/TOML import; schema inference.

## Architecture

Repo `~/grayplugins/gray-recall` (GitHub `vstaln/gray-recall`), one binary
`gray-recall`, MIT. Wire protocol v1.1, same shape as gray-memory: a sidecar
loop on stdio plus `cli_argv` forwarding so `gray recall ...` runs the same
binary with no agent.

| Module | Responsibility |
|---|---|
| `main.rs` | Entry point: sidecar loop (`plugin/manifest`, `tool/call` for `recall`, `command/run` for `/recall`, `plugin/shutdown`) or CLI mode. |
| `cli.rs` | Argument parsing shared by the CLI, `/recall` and the tool (tool args map onto the same request struct). |
| `session.rs` | Parse one session file into turns along the active branch; drop injected prompts. |
| `extract.rs` | Per turn: reply text, commands, files touched, commit SHAs, failed flag. |
| `redact.rs` | Copied from gray-memory `src/redact.rs`; scrubs every text field before it is written. |
| `project.rs` | Map a session cwd to its git repo root (cached per cwd); the root is the project key, its basename the name. |
| `index.rs` | SQLite schema, open/migrate, write helpers. |
| `catchup.rs` | Bring the index up to date with the sessions directory. |
| `query.rs` | Query parsing, project resolution, retrieval and ranking, fallback. |
| `render.rs` | Text cards and `--json`. |

Flow for every search, show or status call: `catch_up()`, then the query,
then rendering.

## Data model

One record per **turn**: a real user prompt plus everything the agent did
until the next real user prompt.

| Field | Content |
|---|---|
| `id` | `<first 8 chars of session id>:<entry_id of the prompt entry>`. Stable across file rewrites because entry ids are. |
| `session` | Full session id. |
| `project_key` / `project_name` | Git root (absolute) and its basename; the cwd itself when not in a repo. |
| `cwd` | Session cwd from the header line. |
| `date` | Prompt timestamp (unix ms). |
| `prompt` | User text, capped at 2 KB. |
| `reply` | Assistant text blocks joined, capped at 8 KB (first 2 KB + last 6 KB). |
| `commands` | One line per tool call: bash `command` (first 300 chars) or `<tool> <path>`; capped at 4 KB. |
| `files` | Deduplicated paths touched, at most 30, relative to the project root when inside it. |
| `commits` | Short commit SHAs created in the turn. |
| `failed` | Any tool result in the turn reported a non-zero exit or an error. |
| `has_reply` | The turn has assistant text. |

If two sessions share the 8-char prefix, `show` reports the id as ambiguous
and lists the candidates (exit 3); a longer session prefix resolves it.

SQLite file `$GRAY_HOME/recall/index.db` (`GRAY_HOME` defaults to `~/.gray`),
WAL mode, directory 0700, file 0600. Tables:

- `meta(key, value)`: schema version.
- `sessions(session PK, path, size, mtime_ns, head_hash, tail_offset, tail_hash, project_key, skipped_lines)`.
- `projects(key PK, name, name_lower, turns)`.
- `turns(rowid, id UNIQUE, session, project_key, project_name, cwd, date, prompt, reply, commands, files, commits, failed, has_reply)`, indexed on `(project_key, date)` and `(session)`.
- `turns_fts`: FTS5, external content on `turns`, columns `prompt, files, reply, commands`, tokenizer `porter unicode61`. Kept in sync by explicit insert/delete statements in the same transaction as `turns`.

Hashes are FNV-1a 64 over fixed byte windows (no extra dependency).

## Session parsing

Observed format: line 1 is a header `{version, id, timestamp, cwd, model}`;
later lines are entries `{entry_id, parent_id, timestamp, message{role,
content[]}}` with content blocks `text`, `image`, `tool_use{id, name, args}`,
`tool_result{id, content}`.

- **Active branch:** follow `parent_id` back from the last entry; only
  entries on that path are indexed. Rewound branches were undone on purpose.
- **Turn start:** a user message with at least one text block, no
  `tool_result` block, and text that is not an injected prompt.
- **Injected prompts** (not turn starts, not indexed): background-task
  notifications, loop-guard notices, compaction and continuation summaries,
  interrupt markers. The exact prefixes are collected in the implementation
  plan by surveying the first 80 characters of every user text across all
  local sessions, and kept as a constant list with a test per entry.
- **Malformed lines** (bad JSON, wrong shape such as a list where an object
  is expected) are skipped and counted in `sessions.skipped_lines`.
- **A last line without a trailing newline** is not consumed; it is read on
  the next catch-up once complete.
- **Compaction replacement entries** (`append_compaction_replacement` in
  gray's `session_store.rs`) never start a turn; their exact shape is
  confirmed in the plan.

## Extraction

- **Files:** `path` / `file_path` args of `read`, `edit`, `write`; from bash
  commands, tokens that contain `/` or end in a known source extension,
  resolved against cwd.
- **Commits:** tool result lines matching git commit output
  `[<branch> <sha>] <message>`.
- **Failed:** a tool result whose header reports `exit N` with N != 0, or a
  result flagged as an error.
- **Redaction:** `prompt`, `reply`, `commands` and `files` pass through
  `redact_for_disclosure` before insertion. Secrets never reach the index.

## Catch-up

Scans `$GRAY_HOME/sessions/*.jsonl`. Lock files, quarantined and archived
files are skipped; their naming is confirmed in the plan. Per file:

| State | Action |
|---|---|
| Size and mtime unchanged | Skip. |
| Grew, head hash and tail hash still match | Parse from `tail_offset` (start of the last indexed turn, which may have been in progress): replace that turn, add new ones. If a new entry's `parent_id` points before the last turn (a branch), do a full reread instead. |
| Shrank, or a hash differs | Delete the session's turns and reread the whole file. |
| File gone | Delete its turns. |

One writer at a time through `std::fs::File::try_lock` on
`$GRAY_HOME/recall/index.lock`. If another process holds it, the query runs
on the current index and the header adds `(index catching up in another
session)`. Each session is committed in its own transaction, so an
interrupted catch-up loses no completed work.

Targets, measured on the real sessions directory: full build under 3 s;
catch-up with nothing changed under 50 ms.

## Query

**Request** (shared by the tool, `/recall` and the CLI): `query, project,
all, file, since, until, failed, limit (default 5, max 20), offset, id,
around, json, max_chars (default 2500)`. With `id`, the call shows that full
turn (redacted) plus `around` turns on each side, instead of searching.

**Syntax:** plain words match any of them (OR) and turns matching more words
rank higher; `"exact phrase"` is required; `-word` excludes. Exclusions
with no words or phrases are a bad request. An empty query lists the newest turns in
scope. Dates: `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, or relative `Nd` (days) / `Nw`
(weeks) / `Nm` (months); `until` is inclusive.

**Scope:**

- Default: the project of the calling session's cwd (`session.cwd` on the
  wire; the process cwd in CLI mode).
- `-p name` resolves in tiers: exact name, case-insensitive name, substring,
  fuzzy (Jaro-Winkler >= 0.85). More than one candidate in the winning tier
  is an error listing them (exit 3). No match lists the closest names.
- Zero hits in the project: rerun across the other projects; those cards
  carry `OTHER PROJECT <name>`.
- `--all` searches everything. The calling session (`session.id`) is always
  excluded.
- `--file x` keeps turns whose `files` contain `x` as a substring.
  `--failed` keeps turns with a failed command.

**Ranking:** take the top 200 FTS matches inside the scope and filters by
`bm25(turns_fts, 3.0, 2.0, 1.0, 0.5)` (prompt, files, reply, commands), then
rescore:

~~~text
score = -bm25 * (1 + 0.3 * exp(-age_days / 90)) * (0.5 if no reply else 1)
~~~

Sort by score, newest first on ties. The total match count `M` comes from
the same FTS query.

## Output

~~~text
recall · project gray (3,886 turns) · "prefix cache compaction" · since 2026-09 · shown 3 of 41
[1] 1900bae3:3154 · 2026-09-15 21:18 · ✗ failed cmd
  you: and uh give a minimal solution to this
  did: Cache path is byte-identical to HEAD … three one-time prefix edits, uncommitted
  files: crates/gray/src/lib.rs, crates/gray-provider/…   commits: 2ba5532
next: recall id=<turn> around=2 for detail · resume: gray -r 1900bae3-…
~~~

- `did:` is the FTS5 `snippet()` of the reply column; when the reply did not
  match, the first line of the last reply.
- Lines are truncated so the whole output fits `max_chars`. The header always
  shows `shown N of M`, and zero results say "none found", not "none exist".
- `--json` returns the same data as objects.

## Surfaces

- **Tool `recall`:** one tool, the request above as its JSON schema, and a
  short description of when to use it (past work, prior fixes, history of a
  file).
- **`/recall <args>`** over `command/run`, same argument syntax as the CLI.
- **CLI:** `gray recall [search] <words> [flags]`, `gray recall show <id>
  [--around N]`, `gray recall projects`, `gray recall status` (turns,
  sessions, projects, skipped lines, index size, last catch-up time),
  `gray recall reindex` (delete and rebuild).
- **Exit codes (CLI):** 0 ok, including zero hits; 1 error; 2 bad request
  (bad date, bad query); 3 ambiguous or unknown project, ambiguous id. Tool
  and command replies carry the same messages as text.
- Manifest fields and the tool/command reply shape are copied from
  gray-memory's `main.rs`.

## Security

Read-only access to session files; writes only under `$GRAY_HOME/recall`;
no network; no `host/*` capabilities. Redaction runs before indexing, so the
index never holds a secret. The index file is 0600.

## Dependencies

`rusqlite` (bundled SQLite with FTS5), `serde`, `serde_json`, `clap`,
`anyhow`, `regex`, `chrono`, `strsim`; `tempfile` for tests. Versions pinned
to releases at least 7 days old.

## Testing

Unit tests live in `*_tests.rs` next to each module, as in gray-memory;
`cargo test` is the whole check.

- `session_tests.rs`: branches; each injected-prompt prefix; image-only
  messages; a list where an object is expected; a half-written last line.
- `extract_tests.rs`: files, commit SHAs and the failed flag against real
  tool-result shapes such as `exit 1 · ...`.
- `catchup_tests.rs`: unchanged, grew (last turn replaced, not duplicated),
  shrank or rewritten, deleted, branch after rewind, lock held elsewhere.
- `query_tests.rs`: phrases, exclusions, dates, the four project tiers,
  ambiguity errors, the `OTHER PROJECT` fallback, current-session exclusion,
  `shown N of M`, the `max_chars` cap.
- Redaction: planted fake `sk-`, `ghp_` and Discord tokens never appear in
  `index.db` bytes or in any output.
- Protocol: spawn the binary, send `plugin/manifest`, `tool/call` and
  `command/run`, check the replies; then gray's plugin conformance check on
  the built binary.
- Manual: on the real `~/.gray/sessions`, the three prototype queries return
  the same turns or better; build and no-op catch-up timings recorded against
  the targets.

## License, notices, release

- `LICENSE`: MIT.
- `THIRD_PARTY_NOTICES.md`: leviathan (elstongun/leviathan, Apache-2.0) for
  the design ideas (FTS5/BM25 ranking, cited cards, `shown N of M`, tiered
  group resolution, never guessing), with its NOTICE text; gray-memory
  `redact.rs`, which derives from Hmbown/CodeWhale (MIT). No leviathan code
  is copied; anything that ends up close to it is marked in a source
  comment. The name appears nowhere else.
- CI: gray-memory's `.github/workflows/test.yml`, adapted.
- Release builds run on maid and the binary is copied back; local dev and
  test builds use `CARGO_BUILD_JOBS=4`.
- Install: `cargo install --path .`, then
  `gray plugin install ~/.cargo/bin/gray-recall`.
- gray repo: one entry under "Where plugins live" in `docs/plugins.md`; no
  core code changes.
- The plugin index entry in `vstaln/graysite` comes only after the GitHub
  repo is public, as a separate PR, on approval. Pushing to GitHub also
  waits for approval.
