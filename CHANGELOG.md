# Changelog

## 0.3.1 - 2026-11-13

- `gain_usage_pull` (CLI `usage-pull`, `--from` + `--query`): read aggregate usage straight from a SQLite product database. Read-only connection, `query_only`, 30 s interrupt, 1,000,000-row and 20 MB caps, one SELECT/WITH enforced. The result must return `account, date, active_users[, sessions, key_actions]`; extra or per-person columns are refused, unknown accounts stop the pull before writing, and re-running a day replaces it. Rows go through the same importer as a CSV.
- `source` must be `sqlite`; PostgreSQL/MySQL are not built in and say so.
- Desktop: Import usage has a "Pull from SQLite" card with a worked query, and File has "Pull Usage from SQLite..."; the coverage test maps the new tool.
- 21 MCP tools; 52 tests (2 new core tests: read-only aggregation, and refusal of writes, personal columns, missing columns and other sources).
- `docs/TOOLS.md` regenerated; mcpb manifest updated.

## 0.3.0 - 2026-11-12

Tier 4: the PRD's menu bar, Help suite and packaging, plus the PRD "Next" items that belong to gaincraft.

- Menu bar per the Craft Menu Bar Standard (`File Edit View Insert Modify Commands Window Help`) with switch-on-hover, keyboard shortcuts and a quick view row; Window toggles for navigation, quick row, detail panel, signals dock and status bar (remembered). The bookends and the Help suite live in the shared `craft-biz-ui` shell, so every suite app gets them.
- Help suite: Command Palette, Keyboard Shortcuts (generated from the menus, filterable), Project Files & Docs (README, HELP, PRD, plan, tools, changelog, AGENTS compiled into the binary), Licenses & Notices (every registry crate in Cargo.lock with the license from its own manifest, generated at build time), About (version, commit, build date).
- Edit menu: undo / redo for edits made in the window (account fields, retire, triage, weights, add account, add CTA); records stay records. Cut / Copy / Paste / Select All act on the text field that had focus, with its selection restored after the menu took focus; Copy without a text field copies the selected account or CTA.
- New tools (20 in total): `gain_cta_add` (manual CTA), `gain_export` (health report as CSV or Markdown), `gain_backup` (database copy via VACUUM INTO, never overwrites), `gain_churn_audit` (risk now with reasons, backtest of retired accounts with an honest verdict). CLI verbs `cta-add`, `export`, `backup`, `audit`, `update --retired-on`.
- Bus: `contract.amended`, `contract.terminated` (exit CTA, renewal CTAs dismissed, notice cleared) and `signal.positive` are applied; amendments or terminations for unknown contracts are rejected so they show in the dock.
- CTA titles in German in the German window (core stores the values behind each title).
- Customer 360: 12-month health trend recomputed from stored data; months before the first usage show "no data" instead of a low score. New CTA button. Overview: Churn audit button.
- Schema 2 (`accounts.retired_on`, `ctas.params`) with an in-place migration, tested on a 0.2.0 database. Retiring sets the churn date; reactivating clears it.
- `--smoke` also checks menu parity (every menu command is a real tool with real argument names, no duplicate shortcuts) and glyph coverage of every label in DE and EN.
- `unsafe_code = "forbid"` is now a workspace lint (the 0.2.0 plan claimed it; nothing enforced it).
- Packaging: `scripts/release.ps1` (gates, release build, smoke on a throwaway database, plain exe assets, per-user NSIS installer, mcpb bundle, launcher zip as an extra only, SHA256SUMS, publish opt-in), `installer/installer.nsi`, `mcpb/manifest.json` (kept in sync with the catalog by a test), `just smoke`, `just release-dry`, `just release`.
- Fixed while building: Edit > Cut/Copy did nothing after the menu took focus, because egui collapses a text field's selection when focus leaves and returns; the shell now restores it (regression test with real clicks).
- Tests: see docs/PRD.md "Today".

## 0.2.0 - 2026-11-11

- Desktop window rebuilt on the shared craft-biz-ui frame: Overview, Accounts with Customer 360, CTA cockpit board (drop on Done opens the resolve form), Renewals timeline, Signals, Import wizard with privacy pre-check, Settings with weights. All 16 tools reachable from the window (coverage test), Ctrl+K palette, signals dock with apply and retry, high-contrast light/dark, Ctrl+wheel zoom, DE/EN.
- The previous window (0.1.x, 2026-11-10) read the store directly and covered 2 of 16 tools; it is replaced.
- Core: `health_of` is public (health without side effects, for views).
- 5 window tests with egui_kittest, including a real drag of a CTA card onto Done.

## 0.1.0 - 2026-11-09

- Tier 1: gaincraft-store (accounts, aggregate telemetry without person columns, signals, triage, CTAs, snapshots; invariants enforced by SQLite CHECKs and unique indexes), gaincraft-core (`health.rs` deterministic score with explainable components, CTA rules, CSV import with personal-column guard, bus inbox for `contract.executed` and `signal.negative`, deadlines, demo seed), `gaincraft` binary (CLI with `--json`, health breakdown and tables, MCP stdio, 16 tools).
- Docs: README, HELP.md, docs/PRD.md, docs/TOOLS.md generated by `gaincraft tools --markdown` and checked by a test.
- 22 tests; live chains verified: deskcraft `signals --emit` -> gaincraft sentiment, contractcraft seal + `move executed` -> gaincraft ARR, contract number, notice date.
- Fixed before release: a ticket signal re-sent by deskcraft (new event id, same ticket) was counted twice and halved the sentiment. Signals are now unique per account, source, reference and polarity.
