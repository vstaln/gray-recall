# gray-recall

Search past [gray](https://github.com/vstaln/gray) sessions for prior work,
as a sidecar plugin (wire v1.1). Ask "has this come up before", "what fixed
it", "what did we do to this file" and get a few ranked, cited turns in
about 500 tokens.

One binary, three surfaces, one argument syntax:

- **Tool `recall`**: the agent calls it when it wants history. Nothing is
  injected into prompts.
- **`/recall …`** inside a session, over `command/run`.
- **`gray recall …`** on the command line (`cli_argv` forwarding: the host
  `exec`s this binary, so it works with no agent running).

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
still a search unless it is a subcommand name, but two cases differ:
`gray recall manifest` prints the plugin manifest (the install probe), and
bare `gray recall` with stdin not a terminal runs the sidecar loop instead
of listing the newest turns. `/recall` and the tool have neither quirk.

One argument with spaces is a phrase: `gray recall search "prefix cache"`
matches the two words together, because the shell (and `/recall`) pass it
as one word. Add `-word` inside it to make it a word list with an
exclusion instead.

## Scope and output

- Default scope is the current git repo (a worktree counts as its main
  repo). When it has no hits, the cards come from the other projects and
  are labeled `OTHER PROJECT`. `-p <name or path>` picks one project and is
  never widened; `--all` searches everything. The calling session's own
  turns are left out.
- Text output fits `--max-chars` (default 2500 for search, 8000 for
  `show`), and the header's `shown N of M` always matches the cards printed.
  `--json` ignores `--max-chars`; its fields are capped one by one instead.
- Exit codes: 0 ok, including zero hits; 1 error; 2 bad request (bad date,
  bad query, bad flag); 3 unknown or ambiguous project or id. The tool sets
  `is_error` only for 1-3.

## Index

`$GRAY_HOME/recall/index.db` (`~/.gray` by default; directory 0700, file
0600). Every search, `show` and `projects` first catches it up: only session
files that changed are read, and only the new bytes when a file grew. While
another session is catching up, the answer comes from the index as it is,
with `(index catching up in another session)` in the header. `status` reports
the index without catching up.

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
`/recall` and `gray recall`.

## License

MIT. Credits for design ideas and vendored code are in
`THIRD_PARTY_NOTICES.md`.
