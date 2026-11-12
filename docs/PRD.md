# gaincraft PRD

## What it is

A customer-health tool for small B2B businesses (5-200 customers): one place that combines how much each customer uses the product, what their contract says, and whether they are unhappy, turns that into a score you can explain in one sentence, and lists what to do about it. Local, one SQLite file, CLI + MCP + desktop window. Replaces Gainsight, ChurnZero, Totango and the health spreadsheet nobody updates.

## Users

- The owner or account manager of a small software or service business who is responsible for renewals.
- An agent (via MCP) that drains events, recomputes health, audits churn risk and reads tickets to propose a triage modifier.

## Jobs

1. Know which customers are drifting away before the notice date passes.
2. See why a score is what it is (adoption, recency, sentiment, modifier), not just a traffic light.
3. Get a short, deduplicated list of things to do, and record what was done.
4. Pull in contract dates and support mood from the other craft apps without retyping.
5. Check honestly whether the score would have warned about the customers who actually left.
6. Do all of this without building per-employee surveillance of the customer's staff.

## Today (v0.3.0)

- Accounts with ARR, seats, contract number, term end, notice date, churn date (`retired_on`); lookup by key, domain or name.
- Aggregate telemetry import (CSV), all-or-nothing, idempotent per day, refuses per-person columns.
- Direct usage pull from a SQLite product database (`gain_usage_pull` / `usage-pull`): read-only connection, one SELECT, 30 s limit, row cap, the same result-column privacy guard and importer as the CSV path.
- Deterministic health score with configurable weights, stored snapshots, explainable components.
- Advisory triage modifier (0.8-1.2, reason, expiry), enforced in Rust and by a SQLite CHECK.
- Five CTA rules plus hand-made CTAs (`gain_cta_add`) and termination CTAs from the bus; opened once per account/kind/reference, resolution needs a note. CTAs store the values behind their title, so the window words them in German or English.
- Bus inbox: `contract.executed`, `contract.amended`, `contract.terminated` (contractcraft), `signal.negative` (deskcraft, formcraft), `signal.positive` (formcraft); one signal per source document; unknown accounts created and flagged; an amendment or termination for an unknown contract is rejected (visible in the dock, retryable).
- Churn audit (`gain_churn_audit`): accounts at risk now with reasons (band, 30-day drop, notice soon while not green, low sentiment), and a backtest: for each retired account with a churn date, its score and rank `lead_days` before it left, with a verdict that says when the sample is too small to mean anything.
- Health report export as CSV (`;`, DE numbers) or Markdown; database backup to a new file (`VACUUM INTO`, never overwrites).
- Deadlines view for notice dates and term ends.
- 21 MCP tools, every one also a CLI verb (checked by a test), demo seed matching the contractcraft and deskcraft demos.
- Desktop window on the shared craft-biz-ui frame: Overview, Accounts + Customer 360 (with a 12-month health trend recomputed from stored data), CTA cockpit board, Renewals timeline, Signals, Import wizard, Settings. High contrast, Ctrl+wheel zoom, DE/EN, signals dock.
- Craft Menu Bar Standard: `File Edit View Insert Modify Commands Window Help`, switch-on-hover, the five-dialog Help suite, a quick view row, Window toggles for every panel, undo/redo for edits (see below).
- Schema 2 with an in-place migration from 0.2.0 databases (tested on a real v1 file).
- 50 tests in gaincraft (store 9, core 19, CLI/MCP 7, window 15) plus the shared kit's menu tests; `--smoke` checks the status tool, menu parity and glyph coverage without opening a window.

## Boundaries

- No per-person data, ever. Aggregates per account and day only. This is a design decision (ArbVG section 96a, DSGVO), not a missing feature. The export contains no per-person data either (a test checks it).
- No outbound messages. gaincraft says who to call; it does not send campaigns.
- No ML churn prediction. The score is a transparent formula; an LLM may only nudge it within 0.8-1.2 with a written reason. The backtest reports how the formula did; it does not fit weights.
- No CRM pipeline or invoicing; contracts live in contractcraft, invoices in buchcraft.

## Gaps (honest)

- Undo covers edits made in the window during this session (account fields, retire, triage, weights, add account, add CTA). Records are not undoable by design: signals, resolved CTAs, imports, applied bus events. The journal is not persisted across restarts.
- File > Save folds the SQLite write-ahead log into the database file; every change is already written when it is made, so Save never loses or keeps anything back.
- The connector reads SQLite only; no PostgreSQL/MySQL driver is built in. For those, export CSV first.
- The pull trusts the query to aggregate: the schema guard checks the result columns, not the SQL's meaning. A query that projects a person column under an allowed alias cannot be detected.
- The default weights (0.45 / 0.35 / 0.20) and the 14-day and 30-day half-lives are reasoned defaults, not fitted to anyone's churn history. The backtest needs retired accounts with churn dates before it says anything; with fewer than five it says so.
- `seats_unused` and `adoption_drop` are simple thresholds; seasonal businesses will see false alarms (dismiss with a note).
- Senders not built yet: contractcraft does not emit `contract.amended` / `contract.terminated`, and formcraft does not emit `signal.positive`. gaincraft consumes them (tested with synthetic events); until the senders exist, edit with `update` and record praise with `signal --positive`.
- A CTA that is resolved stays closed even if the same condition persists (by design, to avoid nagging); a new reference reopens.
- The Licenses dialog lists every registry crate in Cargo.lock with the license from its own manifest; that is a superset of what one binary links (it includes dev and platform-specific crates).
- Release publishing (`just release`) was not run for 0.3.0 from this change set; `just release-dry` builds and stages the artifacts.

## Next

1. contractcraft: emit `contract.amended` and `contract.terminated` (payload documented in HELP.md).
2. formcraft: emit `signal.positive` for good survey answers.
3. Persist the undo journal per database (opt-in), if it turns out to be wanted.
4. Weight tuning from the backtest, once there are enough churned accounts to justify it (report only, never automatic).

## UI/UX Architecture & Menu Bar Standard

In accordance with [`CRAFT_MENU_BAR_STANDARD.md`](../../mcp-central-docs/standards/rust-artcraft/CRAFT_MENU_BAR_STANDARD.md), Pattern 2 (viewport-centric workstation). The bookends (File, Edit, View, Window, Help) come from the shared `craft-biz-ui` shell, so every back-office suite app gets the same bar; gaincraft adds the middle menus and its items.

- **Top Menu Bar Hierarchy (as built):**
  - `File`: New Account (`Ctrl+N`), Import Usage CSV (`Ctrl+O`, opens the import wizard with its privacy pre-check), Open Database (`Ctrl+Shift+O`), Open Recent ▸ (databases), Save (`Ctrl+S`), Save Copy As (`Ctrl+Shift+S`, `gain_backup`), Export Health Report (`Ctrl+E`, Markdown) / Export Health CSV (`Ctrl+Shift+E`), Close (`Ctrl+W`), Quit (`Ctrl+Q`).
  - `Edit`: Undo (`Ctrl+Z`), Redo (`Ctrl+Y`), Cut, Copy, Paste, Select All (act on the text field that had focus; Copy without one copies the selected account or CTA), Preferences (`Ctrl+,`, opens Settings).
  - `View`: Customer 360 Cockpit (`Alt+0`), Overview, Accounts Table, CTA Kanban Board, Renewals Timeline, Signals Feed, Import Usage, Settings (`Alt+1` to `Alt+7`), Zoom In / Out / 100%, Interface Theme ▸ (Dark, Light), Language ▸, Reload (`F5`).
  - `Insert`: Add Account, Add Manual Signal (`Ctrl+Shift+G`), Create Call-to-Action (`Ctrl+Shift+T`), Add Triage Modifier.
  - `Modify`: Edit Account ARR / Seats, Resolve CTA (selected card), Update Health Weights, Recompute Score, Retire Account (with confirmation).
  - `Commands`: Drain Bus Signals (`Ctrl+Shift+D`), Recalculate Health Scores (`Ctrl+R`), Run CTA Trigger Engine (the CTA rules run inside the health pass; this runs it for all accounts), Audit Churn Risk (`Ctrl+Shift+A`), Deadlines, Status.
  - `Window`: Reset Layout, Navigation Rail, Quick View Row, Detail Panel, Signals Dock, Status Bar, Customer 360 Viewport, CTA Board: Lanes by Owner.
  - `Help`: Command Palette (`Ctrl+K`, `Ctrl+Shift+P`), Keyboard Shortcuts (`F1`, `Ctrl+/`), Project Files & Docs (compiled into the binary), Licenses & Notices, About GainCraft (version, commit, build date).
- **Behavioral Mechanics:** `switch_on_hover` between top-level menus (tested with real pointer events), zero-unsafe immediate-mode rendering (`unsafe_code = "forbid"` in both workspaces), headless `--smoke` parity: every Run/Form menu item names a real tool with real argument names, no shortcut is used twice, and every label has glyphs in the loaded fonts (DE and EN).
- **Extension contract:** add a tool to `gaincraft_core::tools()`, a CLI verb in `to_tool`, a place in the window (`COVERAGE`), the tool in `mcpb/manifest.json`, and regenerate `docs/TOOLS.md`; tests fail until each exists. Add a menu item in `GainApp::menus` when the tool belongs in a menu (not enforced); menu parity fails if an item names a tool or argument that does not exist.
