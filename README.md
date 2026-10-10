<div align="center">
  <img alt="gray-recall" src="assets/gray-logo.svg" width="120" height="120" />
  <h1>gray-recall</h1>
  <p><strong>Search your past gray sessions for ranked, cited prior work.</strong><br/>"Has this come up before?" answered in about 500 tokens.</p>
  <p>
    <a href="https://gray.alignment.id">Website</a> ·
    <a href="https://gray.alignment.id/plugins/gray-recall">Store</a> ·
    <a href="https://github.com/vstaln/gray-recall/releases">Releases</a> ·
    <a href="https://github.com/vstaln/gray">gray</a>
  </p>
  <p>
    <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-1c1c20?style=flat-square&labelColor=0a0a0b" /></a>
    <a href="https://www.rust-lang.org"><img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-rust-1c1c20?style=flat-square&labelColor=0a0a0b&logo=rust&logoColor=d4a373" /></a>
    <a href="https://gray.alignment.id/plugins/gray-recall"><img alt="gray plugin" src="https://img.shields.io/badge/gray-plugin-1c1c20?style=flat-square&labelColor=0a0a0b&color=7aa2f7" /></a>
    <a href="https://github.com/vstaln/gray-recall/releases"><img alt="Latest release" src="https://img.shields.io/github/v/release/vstaln/gray-recall?style=flat-square&labelColor=0a0a0b&color=131316" /></a>
  </p>
</div>

<br/>

gray-recall gives the agent a memory of its own past work. Ask "has this come up before", "what fixed it", or "what did we do to this file", and get a few ranked, cited turns from earlier sessions. Nothing is injected into prompts; the agent searches only when it asks.

```bash
gray plugin install gray-recall
```

```bash
gray recall search prefix cache warmup     # any word may match; more matches rank higher
```

## Why gray-recall

| | |
|---|---|
| **Cited, not dumped** | Ranked cards with session ids and turn numbers. `show` opens any one turn in full. |
| **Three surfaces, one syntax** | The `recall` tool for the agent, `/recall …` inside a session, and `gray recall …` on the command line with no agent running. |
| **Cheap** | About 500 tokens per search. Output is capped with `--max-chars`, and `--json` gives machine-readable hits. |
| **Local and private** | The index lives in `$GRAY_HOME/recall` (directory 0700, file 0600). No network. Secrets such as API keys and tokens are redacted before indexing. Raw command output is not indexed. |
| **Always current** | Every search catches the index up first, reading only changed session files and only new bytes when a file grew. |

## Usage

```bash
gray recall search '"prefix cache"' warm   # an "exact phrase" is required
gray recall search 'sso -cookie'           # -word excludes; keep it inside one quoted argument
gray recall search deploy --all --since 2w --failed -n 10
gray recall search -f src/render.rs        # turns that touched a path containing this
gray recall show 1900bae3:3154 --around 2  # one turn in full, by the id on a card
gray recall projects                       # indexed projects
gray recall status                         # counts, size, last catch-up
gray recall reindex                        # delete the index and rebuild it
```

| | |
|---|---|
| `recall` (tool) | the agent calls it when it wants history |
| `/recall …` | inside a session, same arguments as the CLI |
| `gray recall …` | on the command line, no agent running |

Searches are scoped to the current git repo by default. A worktree counts as its main repo. `-p <name or path>` picks one project, and `--all` searches everything. The calling session's own turns are left out.

Exit codes: `0` ok (including zero hits), `1` error, `2` bad request, `3` unknown or ambiguous project or id.

## Install

From the store (once listed):

```bash
gray plugin install gray-recall
```

From source:

```bash
cargo install --path . --locked
gray plugin check ~/.cargo/bin/gray-recall
gray plugin install ~/.cargo/bin/gray-recall
```

`manifest` answers the install probe, so one install wires the sidecar, `/recall`, and `gray recall`.

## Tags

`recall` · `search` · `sessions` · `memory` · `history` · `plugin` · `rust`

## License

MIT. Third-party notices are in [`NOTICE.md`](NOTICE.md) and [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

---
Part of the [gray](https://github.com/vstaln/gray) plugin ecosystem, the open-source AI agent harness. <https://gray.alignment.id>
