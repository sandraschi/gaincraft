# gaincraft help

Task-oriented. Every command takes `--json` for machine output and `--db PATH` for another database. `gaincraft <verb> --help` lists all options.

## In the window

Everything below also works in `gaincraft-app`:

| Task | Where |
|---|---|
| Who is at risk | Overview: "Needs attention"; Accounts sorted worst first |
| Why a score is what it is | Accounts: select a row, Customer 360 on the right |
| Work the calls to action | CTA cockpit: drag a card onto Done, write what was done |
| Record a call, set triage, edit | Customer 360 buttons |
| Import usage | Import usage: choose the CSV, check the preview, Dry run, Import |
| Apply contracts and unhappy mails | Signals (top right): Apply all; or Commands > Drain Bus Signals |
| Who would have been flagged before they left | Commands > Audit Churn Risk; Overview > Churn audit |
| A follow-up you agreed in a call | Insert > Create Call-to-Action; Customer 360 > New CTA |
| Report for the team | File > Export Health Report (Markdown) or Export Health CSV |
| Copy of the database | File > Save Copy As |
| Take back an edit | Edit > Undo (Ctrl+Z) |
| Anything else | Ctrl+K: every command with a form |

Keys: F1 lists every shortcut (Help > Keyboard Shortcuts). The common ones: Ctrl+K commands, Ctrl+N new account, Ctrl+O import, Ctrl+E export, Ctrl+R recalculate, Ctrl+Z / Ctrl+Y undo / redo, Alt+0 Customer 360, Alt+1 to Alt+7 the views, Ctrl+mouse wheel or Ctrl +/- zoom, Ctrl+0 reset zoom, F5 reload, Esc closes a dialog; on the board arrows select, Ctrl+Left/Right move a card, Enter opens it. Theme, language, zoom, panel layout and recent databases are remembered in `ui.json` next to the database.

Undo covers edits made in the window since it was opened: account fields, retire, triage, weights, a new account, a new CTA. Signals, resolved CTAs, imports and applied bus events are records and stay as they are. File > Save folds SQLite's write-ahead log into the database file; every change is already saved when you make it.

## Add a customer

```powershell
gaincraft add "Kunde GmbH" --domain kunde.at --owner "Anna Mockova" --seats 12 --arr "14.400,00"
```

The domain matters: deskcraft signals arrive with the sender's mail domain and are matched on it. Amounts are DE format (`14.400,00`), never floats. Change later with `gaincraft update kunde.at --seats 15`; retire with `--retire`.

## Get usage numbers in

gaincraft wants one row per customer and day, nothing about individual people:

```
account;date;active_users;sessions;key_actions
kunde.at;02.11.2026;7;19;120
kunde.at;03.11.2026;8;22;131
```

`account` may be the key, the domain or the name. `sessions` and `key_actions` are optional. Typical source: `SELECT customer, day, COUNT(DISTINCT user_id) FROM events GROUP BY customer, day` in your product database, exported as CSV. The user ids stay in your database; gaincraft refuses a file that has a user, e-mail, name, login or IP column.

```powershell
gaincraft import D:\Exports\usage.csv --dry-run
gaincraft import D:\Exports\usage.csv
```

If one account in the file is unknown, nothing is written. Importing the same day again replaces it, so a nightly full export is fine.

## See who is at risk

```powershell
gaincraft health-all                 # worst first, ARR at risk = yellow + red
gaincraft show kunde.at              # the score and why: adoption, recency, sentiment, modifier
```

`health` and `health-all` also store a snapshot (the trend in `show`) and open CTAs. Use `--as-of 09.11.2026` to compute for another day.

## Work the calls to action

```powershell
gaincraft cta                        # open, due first
gaincraft cta-done 4 --note "Call with the owner, webshop fix scheduled"
gaincraft cta-done 5 --dismiss --note "Seasonal business, quiet in November"
```

A CTA opens once per account, kind and reference; it does not come back after you resolve it unless the reason changes (for example a new ticket).

Your own follow-ups get a CTA too:

```powershell
gaincraft cta-add kunde.at --title "Send the API roadmap" --due 20.11.2026 --note "promised in the call"
```

## Record what you heard

```powershell
gaincraft signal kunde.at --score 4 --note "Complained about invoices in the call"
gaincraft signal kunde.at --score 3 --positive --note "Renewed early, wants the API"
```

Score 1-10, older signals fade (30-day half-life) and drop out after 180 days.

## Add a judgement call (triage)

```powershell
gaincraft triage kunde.at --modifier 0.85 --reason "Our champion left in October" --days 30
gaincraft triage kunde.at --clear
```

The modifier multiplies the deterministic score and is capped at 0.8-1.2. It needs a reason of at least 10 characters and expires. This is the slot for an LLM's reading of tickets and calls: it can nudge, not override.

## Renewals

```powershell
gaincraft deadlines --days 120
```

Notice dates and term ends. Contracts executed in contractcraft fill these in automatically after `gaincraft inbox-drain`; a `renewal` CTA opens 60 days before the notice date.

## Link the other craft apps

```powershell
deskcraft signals --emit             # unhappy mails -> signal.negative
contractcraft move C-2026-003 executed   # -> contract.executed
gaincraft inbox-drain
```

Accounts that gaincraft did not know are created and flagged `auto_created`; give them seats and an owner with `update`.

gaincraft also understands these events (the senders are not built yet; until then use `update` and `signal --positive`):

| Event | From | Payload | Effect |
|---|---|---|---|
| `contract.amended` | contractcraft | `contract_id`, any of `annual_value_cents`, `term_end`, `last_notice`, `renewal_months` | only the fields sent change; unknown contract is rejected |
| `contract.terminated` | contractcraft | `contract_id`, `effective` (or `term_end`), `reason` | term end set, notice date and renewal cleared, renewal CTAs dismissed, a `termination` CTA (exit call) opens; the account stays active until you retire it |
| `signal.positive` | formcraft | `account` or `email`, `score`, `response_id`, `subject` | a positive sentiment signal |

## Churned customers and the audit

When a customer leaves, retire the account. The churn date is today unless you give one:

```powershell
gaincraft update kunde.at --retire
gaincraft update alt-kunde.at --retired-on 30.06.2026
```

Then ask how the score did:

```powershell
gaincraft audit                      # at risk now and why; backtest 90 days before each churn
gaincraft audit --lead-days 30
```

The backtest recomputes every account's score as it was `lead-days` before each churn date (from the stored usage and signals) and reports where the lost customer ranked. With fewer than five churned accounts it says the result is an anecdote; nothing is tuned automatically.

## Reports and backups

```powershell
gaincraft export D:\Reports\health.md          # Markdown for the team
gaincraft export D:\Reports\health.csv         # ; separated, DE numbers, opens in Excel
gaincraft export D:\Reports\health.csv --overwrite
gaincraft backup D:\Backup\gaincraft-2026-11-12.db
```

The report has one row per account (no per-person data). A backup never overwrites an existing file.

## Change the weights

```powershell
gaincraft weights
gaincraft weights --adoption 0.5 --recency 0.3 --sentiment 0.2
```

All three or none; they must sum to 1.

## Use it from an agent (MCP)

`gaincraft mcp --allow-path D:\Exports\usage` (add the folders for `gain_export` and `gain_backup` too). Tools are listed in [docs/TOOLS.md](docs/TOOLS.md). A good agent loop: `gain_inbox_drain`, `gain_health_all`, then for each red account `gain_account_show`, read the deskcraft tickets, and either `gain_triage_set` with a reason or leave it.
