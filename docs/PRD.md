# gaincraft PRD

## What it is

A customer-health tool for small B2B businesses (5-200 customers): one place that combines how much each customer uses the product, what their contract says, and whether they are unhappy, turns that into a score you can explain in one sentence, and lists what to do about it. Local, one SQLite file, CLI + MCP. Replaces Gainsight, ChurnZero, Totango and the health spreadsheet nobody updates.

## Users

- The owner or account manager of a small software or service business who is responsible for renewals.
- An agent (via MCP) that drains events, recomputes health and reads tickets to propose a triage modifier.

## Jobs

1. Know which customers are drifting away before the notice date passes.
2. See why a score is what it is (adoption, recency, sentiment, modifier), not just a traffic light.
3. Get a short, deduplicated list of things to do, and record what was done.
4. Pull in contract dates and support mood from the other craft apps without retyping.
5. Do all of this without building per-employee surveillance of the customer's staff.

## Today (v0.1.0)

- Accounts with ARR, seats, contract number, term end, notice date; lookup by key, domain or name.
- Aggregate telemetry import (CSV), all-or-nothing, idempotent per day, refuses per-person columns.
- Deterministic health score with configurable weights, stored snapshots, explainable components.
- Advisory triage modifier (0.8-1.2, reason, expiry), enforced in Rust and by a SQLite CHECK.
- Five CTA rules, opened once per account/kind/reference, resolution needs a note.
- Bus inbox: `contract.executed` (contractcraft) and `signal.negative` (deskcraft, formcraft later); one signal per source document; unknown accounts created and flagged.
- Deadlines view for notice dates and term ends.
- 16 MCP tools, CLI with tables and a health breakdown, demo seed matching the contractcraft and deskcraft demos.
- 22 tests; live chain verified with the real binaries: deskcraft signal -> gaincraft sentiment; contractcraft seal + execute -> gaincraft ARR, contract, notice date.

- Desktop window (0.2.0): Overview, Accounts + Customer 360, CTA cockpit board, Renewals timeline, Signals, Import wizard, Settings; on the shared craft-biz-ui frame (high contrast, Ctrl+wheel zoom, Ctrl+K palette for every tool, signals dock).

## Boundaries

- No per-person data, ever. Aggregates per account and day only. This is a design decision (ArbVG section 96a, DSGVO), not a missing feature.
- No outbound messages. gaincraft says who to call; it does not send campaigns.
- No ML churn prediction. The score is a transparent formula; an LLM may only nudge it within 0.8-1.2 with a written reason.
- No CRM pipeline or invoicing; contracts live in contractcraft, invoices in buchcraft.

## Gaps (honest)

- CTA titles are written by the core in English, so the German window shows English titles on the board.
- The window has no undo for resolved CTAs (by design: a resolution is a record).
- CSV only: no direct connection to a product database or analytics tool.
- The default weights (0.45 / 0.35 / 0.20) and the 14-day and 30-day half-lives are reasoned defaults, not fitted to anyone's churn history.
- `seats_unused` and `adoption_drop` are simple thresholds; seasonal businesses will see false alarms (dismiss with a note).
- No NPS or survey intake yet; formcraft will send `signal.negative` (and later positive) responses.
- Contract changes after execution (amendments, terminations) are not sent by contractcraft yet, so ARR and dates can go stale; edit with `update`.
- A CTA that is resolved stays closed even if the same condition persists (by design, to avoid nagging); a new reference reopens.

## Next

1. formcraft: survey responses as signals (negative and positive).
2. contractcraft: `contract.terminated` / `contract.amended` events.
3. Health trend chart over months in the Customer 360 (snapshots exist; today only the sparkline).
4. Optional backtest: given past churned accounts, report how the score ranked them before they left (to tune weights honestly).
