# gray-recall: design

Date: 2026-10-06 · Status: draft for review

## Goal

Searchable memory of past gray sessions. `~/.gray/sessions` holds 459 MB
in about 820 session files (`*.jsonl`; the directory's other entries are
lock and marker files) and nothing can search it. An agent or the user
asks "has this come up before", "what fixed it", "what did we do to this
file" and gets a few ranked, cited turns in roughly 500 tokens.

The idea was proven with a throwaway prototype (Python extractor plus the
leviathan CLI, in `/tmp/levgray`): 6,102 turns from 39 projects indexed in
1.1 s into 24 MB, and three test queries returned the right turn first.
gray-recall counts about 4,140 turns in the same files: an injected prompt
(a background-job notice, a compaction summary) belongs to the turn it
arrives in instead of starting its own, and abandoned branches are left out.

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
| `redact.rs` | Copied from gray-memory `src/redact.rs` with path stripping removed: secrets only, so file paths stay searchable. Scrubs every text field before it is written. |
| `project.rs` | Map a session cwd to its git repo root (cached per cwd); a worktree maps to its main repository. The root is the project key, its basename the name. |
| `index.rs` | SQLite schema, open/migrate, write helpers. |
| `catchup.rs` | Bring the index up to date with the sessions directory. |
| `query.rs` | Query parsing, project resolution, retrieval and ranking, fallback. |
| `render.rs` | Text cards and `--json`. |

Flow for every search, show or projects call: `catch_up()`, then the query,
then rendering. `status` is a diagnostic: it reports the index as it is,
without catching up, and whether another process holds the writer lock.

## Data model

One record per **turn**: a real user prompt plus everything the agent did
until the next real user prompt.

| Field | Content |
|---|---|
| `session` / `entry_id` | Full session id and the `entry_id` of the prompt entry; unique together. Stable across file rewrites because entry ids are. |
| display id | `<short session>:<entry_id>`. Short session: the first 8 characters of a UUID-shaped id, the whole id otherwise (gray also names sessions like `kinetic-photon-flux`). |
| `project_key` / `project_name` | Git root (absolute) and its basename; the cwd itself when not in a repo. |
| `cwd` | Session cwd from the header line. |
| `date` | Prompt timestamp (unix ms). |
| `prompt` | User text, capped at 2 KB. |
| `reply` | Assistant text blocks joined, capped at 8 KB (first 2 KB + last 6 KB). |
| `gist` | First non-empty line of the final assistant text, capped at 300 chars. |
| `commands` | One line per tool call: bash `command` (first 300 chars) or `<tool> <path>`; capped at 4 KB. |
| `files` | Deduplicated paths touched, at most 30, relative to the project root when inside it. |
| `commits` | Short commit SHAs created in the turn. |
| `failed` | Any tool result in the turn reported a non-zero exit or `is_error`. |
| `has_reply` | The turn has assistant text. |

`show` accepts any unique session prefix before the colon. A prefix that
matches several sessions is reported as ambiguous with the full ids (exit 3).

SQLite file `$GRAY_HOME/recall/index.db` (`GRAY_HOME` defaults to `~/.gray`),
WAL mode, directory 0700, file 0600. Tables:

- `meta(key, value)`: schema version, last catch-up time.
- `sessions(session PK, path, cwd, project_key, size, mtime_ns, head_hash, tail_offset, tail_hash, tail_entry, skipped)`.
- `projects(key PK, name, name_lower, turns)`, rebuilt after each catch-up.
- `turns(rowid, session, entry_id, project_key, project_name, cwd, date, prompt, reply, gist, commands, files, commits, failed, has_reply)`, `UNIQUE(session, entry_id)`, indexed on `(project_key, date)`.
- `turns_fts`: FTS5, external content on `turns`, columns `prompt, files, reply, commands`, tokenizer `porter unicode61`, kept in sync by insert/delete triggers on `turns`.

Hashes are FNV-1a 64 over fixed byte windows (no extra dependency).

## Session parsing

Confirmed format: line 1 is a header `{version, id, timestamp, cwd, model}`;
later lines are entries `{entry_id, parent_id, timestamp, message{role,
content[]}}`, plus `compaction_boundary: true` on boundary markers. Content
blocks are `text`, `image`, `tool_use{id, name, args}` and
`tool_result{id, content, is_error}` (`content` is always a string; checked
on 25,056 results).

- **Active branch:** follow `parent_id` back from the last entry; only
  entries on that path are indexed. Rewound branches were undone on purpose.
- **Turn start:** a user message with non-empty text, no `tool_result`
  block, and text that is not an injected prompt. Image-only user messages
  are ignored.
- **Injected prompts** (never turn starts, text dropped). Prefixes found by
  surveying every user text in the local sessions:
  `[Background task notification]` (1,050+), `Another language model started
  to solve this problem` (58), `[gray loop guard:` (8), and `The conversation
  history before this point was compacted` (compaction summary).
- **Skill invocations** (`<skill name="X" ...>` + the skill body) start a
  turn whose prompt is `[skill X]`; the body is not indexed.
- **Compaction:** gray appends a `system` entry with `compaction_boundary:
  true`, then copies of the kept messages, all with the boundary's
  timestamp (confirmed in 64 sessions). The boundary and every following
  entry whose timestamp is within 500 ms of it are skipped, so nothing is
  indexed twice. Work after the batch continues the open turn.
- **Malformed lines** (bad JSON, or not an object) are skipped and counted
  in `sessions.skipped`.
- **A last line without a trailing newline** is not consumed; it is read on
  the next catch-up once complete.

## Extraction

- **Files:** `path` / `file_path` args of `read`, `edit`, `write`; from bash
  commands, tokens that contain `/` or end in a known source extension,
  resolved against cwd.
- **Commits:** tool result lines matching git commit output
  `[<branch> <sha>] <message>`.
- **Failed:** a tool result whose header reports `exit N` with N != 0, or a
  result flagged as an error.
- **Gist:** the first non-empty line of the turn's final assistant text,
  shown when the reply did not match the query.
- **Redaction:** `prompt`, `reply`, `gist`, `commands` and `files` pass
  through `redact_secrets` before insertion. Secrets never reach the index.

## Catch-up

Scans the top level of `$GRAY_HOME/sessions` for `*.jsonl` files; the
session id is the file stem. Everything else there is ignored by that rule:
`<id>.lock`, `<id>.open`, `<id>.corrupt-<n>` (quarantine) and the
`archive/` directory (rewind and maintenance originals). Per file:

| State | Action |
|---|---|
| Size and mtime unchanged | Skip. |
| Grew, head hash (first 4 KB) and tail hash (256 bytes before `tail_offset`) still match | Parse from `tail_offset`, the start of the last indexed turn's prompt line. The chunk's active branch must start at `tail_entry`; then that turn is replaced and new turns are added. Otherwise (a branch), do a full reread. |
| Shrank, or a hash differs | Delete the session's turns and reread the whole file. |
| File gone | Delete its turns and its session row. |

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
around, json, max_chars (default 2500 for search, 8000 for show)`. With `id`, the call shows that full
turn (redacted) plus `around` turns on each side, instead of searching.

**Syntax:** plain words match any of them (OR) and turns matching more words
rank higher; `"exact phrase"` is required; `-word` excludes. Exclusions
with no words or phrases are a bad request. On the command line and in
`/recall`, one argument that contains spaces is a phrase, unless it already
contains `"` or a `-word` (then it is read as written). An empty query lists the newest turns in
scope. Dates: `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, or relative `Nd` (days) / `Nw`
(weeks) / `Nm` (months); `until` is inclusive.

**Scope:**

- Default: the project of the calling session's cwd (`session.cwd` on the
  wire; the process cwd in CLI mode).
- The calling session is `session.id` on the wire and `GRAY_SESSION_ID`
  (exported by gray's bash tool) in CLI mode.
- `-p name` resolves in tiers: exact name, case-insensitive name, substring,
  fuzzy (Jaro-Winkler >= 0.85). More than one candidate in the winning tier
  is an error listing them (exit 3). No match lists the closest names.
- Zero hits in the project: rerun across the other projects; those cards
  carry `OTHER PROJECT <name>`.
- `--all` searches everything. The calling session is always excluded.
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
  match, the turn's `gist`.
- Lines are truncated so the whole output fits `max_chars`. The header always
  shows `shown N of M`, and zero results say "none found", not "none exist".
- `--json` returns the same data as objects. It ignores `max_chars`; long
  fields are capped one by one instead.

## Surfaces

- **Manifest** (same shape as gray-memory and graysearch): `name: "recall"`
  (so the host forwards `gray recall ...` to this binary via `cli_argv`),
  `protocol: "1.1"`, one tool `{name, description, parameters, snippet}`,
  `commands: ["/recall"]`, `hooks: []`, and `completion` words. The binary
  answers `gray-recall manifest` on the command line for the install probe.
- **Tool `recall`:** the request above as its JSON schema and a short
  description of when to use it (past work, prior fixes, history of a file).
  Replies `{"content": text, "is_error": bool}`; `is_error` is true only for
  exit codes 1-3, never for zero hits.
- **`/recall <args>`** over `command/run`, same argument syntax as the CLI;
  replies `{"text": text}`.
- **CLI:** `gray recall [search] <words> [flags]`, `gray recall show <id>
  [--around N]`, `gray recall projects`, `gray recall status` (turns,
  sessions, projects, skipped lines, index size, last catch-up time),
  `gray recall reindex` (delete and rebuild). The first word picks a
  subcommand only when it is exactly one of those names; `search` forces a
  search. Exclusions must be inside one quoted argument (`'sso -cookie'`),
  because a bare `-word` is read as a flag.
- **Exit codes (CLI):** 0 ok, including zero hits; 1 error; 2 bad request
  (bad date, bad query); 3 ambiguous or unknown project, ambiguous or
  unknown id.

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
  `redact.rs` (path stripping removed), which derives from Hmbown/CodeWhale
  (MIT). No leviathan code
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
