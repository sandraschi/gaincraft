# AGENTS.md - gaincraft

Customer health for small B2B. Read first:
`mcp-central-docs/standards/RUST_ARTCRAFT_STANDARD.md`,
`mcp-central-docs/projects/craft-fleet-expansion/BACKOFFICE_CRAFT_SUITE.md` (section 4.7),
then `docs/PRD.md` here.

## Layout

- `crates/gaincraft-store` (L1): accounts, telemetry_daily, signals, triage, ctas, snapshots on SQLite WAL.
- `crates/gaincraft-core` (L1): `Command` bus, `dispatch`, MCP catalog, `health.rs` (score), CTA rules, CSV import, bus inbox, deadlines, demo seed.
- `crates/gaincraft-mcp` (L2): `gaincraft` binary. CLI verbs map onto the same tool calls as MCP.
- `crates/gaincraft-app` (L3): desktop window on `craft-biz-ui::app::Shell`. Views only call `Frame::run`/`form` (dispatch); reads use the store and `health_of` (no side effects). `COVERAGE` maps every tool to its place; the test fails for a tool without one. `GainApp::menus` adds Insert / Modify / Commands and items to the shell's File / View / Window; `build.rs` provides commit, build date and the third-party license list for Help.
- Menu bar, Help suite, undo hooks, `--smoke` menu parity and the tofu guard are in `craft-biz-ui` (`menu.rs`, `help.rs`, `app.rs`); change them there, not per app.
- Shared kernel: `../craft-biz` (money, dates, bus, MCP loop, tools_markdown, confinement).

## Invariants (never weaken)

- No per-person data. `telemetry_daily` has exactly account_id, date, active_users, sessions, key_actions (a test checks the columns). The import refuses person-like headers before reading rows.
- Import is all-or-nothing: every row parsed and every account resolved before the first write.
- The score is deterministic from stored data and weights. The triage modifier is the only judgement input, capped 0.8-1.2 (Rust and SQL CHECK), with a reason >= 10 chars and an expiry.
- Signals are unique per bus event id and per (account, source, reference, polarity).
- A CTA is open at most once per (account, kind, reference); resolving needs a note and records who.
- Money is i64 cents; DE strings in and out, never floats.
- Schema changes are new entries in `MIGRATIONS` (store), never edits of `SCHEMA`; the migration test opens a real old-version file.
- Undo only for edits (journal in `GainApp::call`); records (signals, resolved CTAs, imports, bus events) are not undoable. Undoing an add never sets a churn date.
- Export and backup paths go through `Confinement::check(path, true)`; backup never overwrites.

## Docs (keep current in the same commit)

- `README.md`: what it does, quick start, MCP config, build.
- `HELP.md`: task-oriented how-to for users; update when a verb or workflow changes.
- `docs/PRD.md`: purpose, boundaries, honest gaps, next steps; update "Today" and "Gaps" with every release.
- `docs/TOOLS.md`: generated. After any tool change run `just docs`; the `tools_doc_is_current` test fails otherwise.
- `mcpb/manifest.json`: tool list must equal the catalog (test).
- `CHANGELOG_LATEST.md`: release notes of the current version only (the release script checks the version).
- `CHANGELOG.md`: one entry per change set.
- CLI help: `about`, per-argument doc comments and `AFTER_HELP` in `main.rs`.

## Rules

- Zero-panic: unwrap/expect/panic/todo/unimplemented denied outside tests.
- MCP mode: stdout is JSON-RPC only; diagnostics to stderr.
- Import paths go through `Confinement::check`.
- Demo data only via `demo-seed`, only on an empty database, labelled DEMO, `.example` domains, mock names; names match the contractcraft and deskcraft demos so bus events land on the same accounts.
- Every bug fix ships with a regression test.

## Repo hygiene

- LF line endings only (`.gitattributes`). CRLF breaks commit hooks.
- Single trunk `main`. No agent branches or leftover WIP branches.
- No em dash characters in code, docs or commit messages.
- No `*.bak`, `*.db`, `target/`, binaries in git.

## Gates

1. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
2. `cargo test --workspace` (includes the docs/TOOLS.md sync test)
3. `cargo fmt --all --check`
4. MCP probe: `initialize` -> `tools/list` -> `tools/call`, stdout pure JSON; `just smoke` (menu parity, glyphs)
5. `Cargo.lock` committed, clean `git status`
6. `CHANGELOG.md` entry; README / HELP / PRD updated if behaviour changed
