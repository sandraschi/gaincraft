# gaincraft v0.3.1

Customer health for small B2B, local, one SQLite file. Windows x64.

## Downloads

- `gaincraft-app.exe`: the desktop window.
- `gaincraft.exe`: CLI and MCP server (`gaincraft mcp --allow-path <dir>`).
- `gaincraft-0.3.1-setup.exe`: per-user installer for both (no admin rights). Customer data stays in `%LOCALAPPDATA%\gaincraft`.
- `gaincraft-v0.3.1.mcpb`: MCP bundle.

## What is new

- `gain_usage_pull` reads aggregate usage from a SQLite product database. The connection is read-only. Per-person columns are refused.
- Commands > Run Scheduled Pass (`gain_tick`) and Commands > Show Alerts (`gain_alerts`).

- Full menu bar (File, Edit, View, Insert, Modify, Commands, Window, Help) with keyboard shortcuts and the Help suite: command palette, shortcut sheet (F1), project docs, licenses, about.
- Undo / redo for edits made in the window.
- Churn audit: who is at risk now and why, and a backtest of customers who left.
- Health report export (CSV or Markdown) and database backup.
- Manual calls to action; German CTA titles; 12-month health trend in Customer 360.
- Contract amendments and terminations and positive survey signals are understood (the sending apps do not emit them yet).
- Databases from 0.2.0 are upgraded in place on first open.

Synthetic demo data only (`gaincraft demo-seed`). No per-person tracking. See HELP.md and docs/PRD.md.
