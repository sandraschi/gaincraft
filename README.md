# gaincraft

![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)
![Rust 1.90+](https://img.shields.io/badge/rust-1.90%2B-orange?logo=rust)
![MCP stdio](https://img.shields.io/badge/MCP-stdio-purple)
![ArtCraft harness](https://img.shields.io/badge/ArtCraft-harness-green)

Customer health for a small B2B business: usage aggregates per account and day, contract data from contractcraft, unhappy-customer signals from deskcraft, an explainable 0-100 health score, and calls to action (CTAs) that say what to do this week. One SQLite file, one binary that is both CLI and MCP server. Part of the back-office craft suite (replaces Gainsight / ChurnZero / Totango / a "customer health" spreadsheet).

Status: **v0.3.0, Tier 4** (store + CLI + MCP + desktop window with the Craft menu bar and Help suite + installer, tested). Synthetic demo data only. **No per-person tracking**: gaincraft only stores counts per account and day and refuses import files with user, e-mail, name, login or IP columns.

- How to use it day to day: [HELP.md](HELP.md)
- What it is for and where it stops: [docs/PRD.md](docs/PRD.md)
- Every MCP tool: [docs/TOOLS.md](docs/TOOLS.md) (generated, checked by a test)
- Changes: [CHANGELOG.md](CHANGELOG.md)

## What it does

- **Accounts** with key, domain (links mail signals), owner, ARR, seats, contract number, term end, notice date. Found by key, domain or name.
- **Telemetry import:** CSV `account;date;active_users[;sessions;key_actions]` (`;` or `,`). Every row is validated and every account resolved before anything is written; re-importing a day replaces it. Per-person columns are refused before a single row is read (ArbVG section 96a, DSGVO data minimisation).
- **Health score**, deterministic and shown with its parts:
  `score = 100 x (0.45 adoption + 0.35 recency + 0.20 sentiment) x modifier`
  - adoption: average active users over 30 days / seats (or / best day in 90 days without seats), capped at 1
  - recency: halves every 14 days without activity
  - sentiment: 1 minus decayed negative signals (30-day half-life) plus positive ones, over 180 days
  - modifier: optional advisory 0.8-1.2 from an LLM or human read, with a reason and an expiry
  - bands: green >= 70, yellow >= 40, red below. Weights are configurable and must sum to 1.
- **CTAs**, opened once per account, kind and reference: `low_health`, `adoption_drop` (last 14 days below 60% of the 14 before), `seats_unused` (under half the seats), `renewal` (notice date within 60 days), `negative_signal` (last 30 days). Resolving needs a note.
- **Bus inbox:** `contract.executed` from contractcraft sets ARR, contract number, term end and notice date (matched by contract number, then counterparty name); `signal.negative` from deskcraft (and later formcraft) becomes a sentiment signal, matched by mail domain. The same ticket counts once even if it is sent again. Unknown accounts are created and flagged `auto_created`.
- **Deadlines:** notice dates and term ends within N days, so renewals are not missed.
- **Churn audit:** who is at risk now and why (band, a 10-point drop in 30 days, notice date close while not green, low sentiment), plus a backtest: for every retired account with a churn date, where the score ranked it N days before it left. With fewer than five churned accounts it says the result is an anecdote.
- **Report and backup:** health report of all accounts as CSV (`;`, DE numbers) or Markdown; copy the whole database to a new file. Neither contains per-person data, and neither overwrites a file unless told to (backup never does).
- **More bus events:** `contract.amended` (only the fields sent change), `contract.terminated` (no renewal deadline, renewal CTAs dismissed, an exit-call CTA), `signal.positive`. gaincraft handles them; contractcraft and formcraft do not send them yet.

## Desktop window

`gaincraft-app` is laid out like a customer success platform:

- **Overview:** ARR, ARR at risk, band distribution, who needs attention, next deadlines.
- **Accounts** with a **Customer 360** panel: score ring, the three components with their raw values, 90-day usage chart against seats, contract dates, triage, open CTAs, signals.
- **CTA cockpit:** a board by due date. Drop a card on Done to resolve it with a note; other moves are refused with the reason.
- **Renewals:** 12-month timeline of notice dates and term ends.
- **Signals:** feed of the last 180 days, record new ones.
- **Import usage:** wizard that checks the file for personal columns and unknown accounts before anything is written.
- **Settings:** health weights, demo data, apply signals.

Every change in the window goes through the same command path as CLI and MCP. Ctrl+K lists every tool with a generated form, and a test fails if a tool has no place in the window. High-contrast light and dark themes, Ctrl+mouse wheel zoom (Ctrl+0 resets), German and English (CTA titles included), signals dock for the other craft apps.

Menu bar per the Craft Menu Bar Standard: **File** (new account, import, open database, recent, save, save copy, export report / CSV, close, quit), **Edit** (undo / redo of edits, cut / copy / paste / select all, preferences), **View** (every view, zoom, theme, language), **Insert** (account, signal, CTA, triage), **Modify** (ARR / seats, resolve CTA, weights, recompute, retire), **Commands** (drain signals, recalculate, CTA rules, churn audit, deadlines), **Window** (panel toggles, reset layout), **Help** (command palette, keyboard shortcuts F1, project docs, licenses, about). Hovering from one open menu to the next switches menus. `--smoke` checks without a window that every menu command is a real tool with real arguments, that no shortcut is used twice, and that every label can be drawn by the loaded fonts.

```powershell
gaincraft-app --demo                 # on an empty database: load the demo first
gaincraft-app --dark --lang en
gaincraft-app --view 2 --screenshot cockpit.png
```

## Quick start

```powershell
gaincraft demo-seed
gaincraft health-all
gaincraft show mockinger.example
gaincraft cta
gaincraft deadlines --days 120
```

With the other suite apps (same bus, `%LOCALAPPDATA%\craft-biz\bus` or `$CRAFT_BIZ_BUS`):

```powershell
deskcraft signals --emit
contractcraft move C-2026-003 executed     # after sealing it
gaincraft inbox-drain
```

Your own data: `gaincraft add "Kunde GmbH" --domain kunde.at --seats 12 --arr "14.400,00"`, then export daily active users per customer from your product (a SQL `GROUP BY customer, day`) and `gaincraft import usage.csv`.

Database: `--db PATH`, else `$GAINCRAFT_DB`, else `%LOCALAPPDATA%\gaincraft\gaincraft.db`.

## MCP

```json
{ "mcpServers": { "gaincraft": { "command": "D:/Dev/cargo-target/release/gaincraft.exe", "args": ["mcp", "--allow-path", "D:/Exports/usage"] } } }
```

20 tools, all `gain_*`; see [docs/TOOLS.md](docs/TOOLS.md). `gain_export` and `gain_backup` write files, so they also need `--allow-path`. An agent can read an account and set an advisory triage modifier with a reason; it cannot move the score outside 0.8-1.2 of the deterministic value. Agent actions are recorded with actor `agent`. Without `--allow-path` the server cannot read any file.

## Build

```powershell
just ci           # clippy -D warnings, tests (incl. docs/TOOLS.md and mcpb sync), fmt check
just docs         # regenerate docs/TOOLS.md after changing a tool
just smoke        # headless: status, menu parity, glyph check
just build-release
just release-dry  # dist/: gaincraft-app.exe, gaincraft.exe, per-user setup.exe, .mcpb, SHA256SUMS
just release      # the same, then tag + GitHub release (clean, pushed main)
```

Needs `../craft-biz` checked out next to this repo. The installer (NSIS, no admin rights) puts the programs in `%LOCALAPPDATA%\Programs\GainCraft`; the database stays in `%LOCALAPPDATA%\gaincraft`, so uninstalling never deletes customer data.

## Not done (honest list)

See the "Gaps" section of [docs/PRD.md](docs/PRD.md). In short: no connectors to product databases (CSV only); the weights are a sensible default, and the backtest needs churned accounts with dates before it can say whether they work; undo covers edits made in the window this session, not records; contractcraft and formcraft do not send the new event kinds yet.
