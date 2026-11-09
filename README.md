# gaincraft

Customer health for a small B2B business: usage aggregates per account and day, contract data from contractcraft, unhappy-customer signals from deskcraft, an explainable 0-100 health score, and calls to action (CTAs) that say what to do this week. One SQLite file, one binary that is both CLI and MCP server. Part of the back-office craft suite (replaces Gainsight / ChurnZero / Totango / a "customer health" spreadsheet).

Status: **v0.1.0, Tier 1** (store + CLI + MCP, tested). No desktop window yet. Synthetic demo data only. **No per-person tracking**: gaincraft only stores counts per account and day and refuses import files with user, e-mail, name, login or IP columns.

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

16 tools, all `gain_*`; see [docs/TOOLS.md](docs/TOOLS.md). An agent can read an account and set an advisory triage modifier with a reason; it cannot move the score outside 0.8-1.2 of the deterministic value. Agent actions are recorded with actor `agent`. Without `--allow-path` the server cannot read any file.

## Build

```powershell
just ci           # clippy -D warnings, tests (incl. docs/TOOLS.md sync), fmt check
just docs         # regenerate docs/TOOLS.md after changing a tool
just build-release
```

Needs `../craft-biz` checked out next to this repo.

## Not done (honest list)

See the "Gaps" section of [docs/PRD.md](docs/PRD.md). In short: no window, no connectors to product databases (CSV only), the weights are a sensible default and not fitted to churn data, no NPS or survey intake until formcraft exists.
