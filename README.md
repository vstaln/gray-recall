<p align="center">
  <img src="assets/gray-logo.svg" alt="gray" width="96">
</p>
<h1 align="center">gray-recall</h1>
<p align="center">Search past gray sessions for ranked, cited prior work.</p>
<p align="center">
  <a href="https://github.com/vstaln/gray-recall/blob/main/LICENSE"><img alt="MIT License" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="gray plugin" src="https://img.shields.io/badge/gray-plugin-7aa2f7.svg">
  <img alt="rust" src="https://img.shields.io/badge/built%20with-rust-orange.svg">
</p>

Ask "has this come up before", "what fixed it", or "what did we do to this
file" and get a few ranked, cited turns in about 500 tokens.

One binary, three surfaces, one argument syntax:

- **Tool `recall`** — the agent calls it when it wants history; nothing is
  injected into prompts.
- **`/recall …`** — inside a session, over `command/run`.
- **`gray recall …`** — on the command line (`cli_argv` forwarding), with no
  agent running.

```sh
gray recall search prefix cache warmup     # any word may match; more matches rank higher
gray recall search '"prefix cache"' warm   # an "exact phrase" is required
gray recall search 'sso -cookie'           # -word excludes; keep it inside one quoted argument
gray recall search deploy --all --since 2w --failed -n 10
gray recall search -f src/render.rs        # turns that touched a path containing this
gray recall show 1900bae3:3154 --around 2  # one turn in full, by the id on a card
gray recall projects                       # indexed projects
gray recall status                         # counts, size, last catch-up
gray recall reindex                        # delete the index and rebuild it
```

Start command-line searches with `search`. Without it the first word is
still a search unless it is a subcommand name. Two exceptions:
`gray recall manifest` prints the plugin manifest, and bare `gray recall`
with non-terminal stdin runs the sidecar loop.

One argument with spaces is a phrase: `gray recall search "prefix cache"`
matches the words together because the shell (and `/recall`) pass it as one
word. Add `-word` inside it to make it a word list with an exclusion.

## Scope and output

- Default scope is the current git repo (a worktree counts as its main
  repo). With no hits, cards come from other projects and are labeled
  `OTHER PROJECT`. `-p <name or path>` picks one project; `--all` searches
  everything. The calling session's own turns are left out.
- Text output fits `--max-chars` (default 2500 for search, 8000 for
  `show`), and the header's `shown N of M` matches the printed cards.
  `--json` ignores `--max-chars`; its fields are capped one by one.
- Exit codes: 0 ok (including zero hits), 1 error, 2 bad request, 3 unknown
  or ambiguous project/id. The tool sets `is_error` for 1–3.

## Index

`$GRAY_HOME/recall/index.db` (`~/.gray` by default; directory 0700, file
0600). Every search, `show`, and `projects` call catches the index up first:
only changed session files are read, and only new bytes when a file grew.
While another session is catching up, answers come from the current index
with `(index catching up in another session)` in the header.

Secrets (API keys, tokens, `KEY=value` assignments, Discord bot tokens) are
redacted before anything is written; file paths are kept. Raw command
output is not indexed. No network, no `host/*` capabilities, no writes
outside `$GRAY_HOME/recall`.

## Install

```sh
cargo install --path . --locked
gray plugin check ~/.cargo/bin/gray-recall
gray plugin install ~/.cargo/bin/gray-recall
```

`manifest` answers the install probe, so one install wires the sidecar,
`/recall`, and `gray recall`.

## License

MIT. Third-party notices are in `NOTICE.md` and `THIRD_PARTY_NOTICES.md`.

---
Part of the [gray](https://github.com/vstaln/gray) plugin ecosystem —
the open-source AI agent harness. <https://gray.alignment.id>
