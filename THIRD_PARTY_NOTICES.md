# Third-party notices

gray-recall is MIT-licensed (see `LICENSE`). It includes, or is informed by,
the following.

## Design ideas: elstongun/leviathan (Apache-2.0)

The retrieval design follows ideas from Leviathan: SQLite FTS5 with BM25
ranking, compact cited result cards, an explicit "shown N of M" count, and
tiered name resolution that reports ambiguity instead of guessing. No
Leviathan code is copied. Leviathan's NOTICE:

    Leviathan
    Copyright 2026 Joshua Baker

    This product includes SQLite (public domain), compiled in via the
    rusqlite / libsqlite3-sys crates. See THIRD_PARTY.md for all dependencies
    and their licenses.

## MIT: Hmbown/CodeWhale, via gray and gray-memory

`src/redact.rs` is a copy of vstaln/gray-memory `src/redact.rs` with path
stripping removed. That file comes from gray
`crates/gray-core/src/redaction.rs`, which is vendored from Hmbown/CodeWhale
`crates/workflow/src/redaction.rs`.

    MIT License

    Copyright (c) 2024-2025 DeepSeek CLI Contributors

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.

## SQLite (public domain)

Compiled in through rusqlite's `bundled` feature.
