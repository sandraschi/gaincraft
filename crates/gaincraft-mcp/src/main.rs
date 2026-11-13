//! `gaincraft`: CLI and MCP stdio server in one binary.
//!
//! Every CLI verb builds the same tool-argument JSON an MCP client would send
//! and goes through `Command::from_tool` + `dispatch`, so CLI and MCP share one
//! code path. In `mcp` mode stdout carries only JSON-RPC; logs go to stderr.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clap::{Parser, Subcommand};
use craft_biz::bus::Bus;
use craft_biz::mcp::{serve_stdio, tools_markdown, ToolHost};
use gaincraft_core::{dispatch, tools, Command, Confinement, Ctx, APP, VERSION};
use gaincraft_store::Gain;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::process::ExitCode;

const AFTER_HELP: &str = "Typical week:
  gaincraft inbox-drain                         # contracts from contractcraft, unhappy mails from deskcraft
  gaincraft import D:\\Exports\\usage.csv         # account;date;active_users[;sessions;key_actions]
  gaincraft usage-pull --from product.db --query \"SELECT customer AS account, day AS date, COUNT(DISTINCT user_id) AS active_users FROM events GROUP BY 1,2\" --dry-run
  gaincraft health-all                          # worst first, ARR at risk
  gaincraft show mockingbird.example            # why the score is what it is
  gaincraft cta                                 # what to do this week
  gaincraft cta-done 4 --note \"Call booked\"
  gaincraft deadlines --days 90                 # notice dates before they pass
  gaincraft audit                               # who is at risk and why; backtest of churned accounts
  gaincraft export D:\\Reports\\health.md        # report for the team (csv or md)
See HELP.md for workflows and docs/TOOLS.md for every MCP tool.";

#[derive(Parser)]
#[command(name = "gaincraft", version, about = "Customer health for small B2B: aggregate usage, contracts, support signals, explainable score, calls to action. CLI + MCP.", after_help = AFTER_HELP)]
struct Cli {
    /// Database. Default: $GAINCRAFT_DB or %LOCALAPPDATA%\gaincraft\gaincraft.db
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Print machine-readable JSON only.
    #[arg(long, global = true)]
    json: bool,
    /// Restrict file access to these directories (repeatable). Required for import in `mcp` mode.
    #[arg(long = "allow-path", global = true)]
    allow_path: Vec<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Accounts, ARR, open CTAs, weights.
    Status {
        #[arg(long)]
        as_of: Option<String>,
    },
    /// Add a customer account.
    Add {
        name: String,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        owner: Option<String>,
        /// DE amount, e.g. 9.600,00
        #[arg(long)]
        arr: Option<String>,
        #[arg(long)]
        seats: Option<i64>,
        #[arg(long)]
        notes: Option<String>,
    },
    /// List accounts.
    List {
        #[arg(long)]
        all: bool,
    },
    /// One account with health, trend, signals, CTAs.
    Show {
        account: String,
        #[arg(long)]
        as_of: Option<String>,
    },
    /// Change account fields.
    Update {
        account: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long)]
        arr: Option<String>,
        #[arg(long)]
        seats: Option<i64>,
        #[arg(long)]
        contract: Option<String>,
        #[arg(long)]
        term_end: Option<String>,
        #[arg(long)]
        last_notice: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        /// Retire the account (churned). Sets the churn date to today unless --retired-on.
        #[arg(long)]
        retire: bool,
        /// Churn date for a retired account (DD.MM.YYYY), used by `audit`.
        #[arg(long)]
        retired_on: Option<String>,
    },
    /// Import aggregate usage CSV.
    Import {
        path: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Pull aggregate usage from a SQLite product database (read-only).
    UsagePull {
        /// Product SQLite database; opened read-only.
        #[arg(long = "from")]
        from: String,
        /// One SELECT/WITH returning account, date, active_users[, sessions, key_actions].
        #[arg(long)]
        query: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// One scheduled pass: optional pull, health for all, alerts, run record.
    Tick {
        /// Product SQLite database (with --query); omit both for a health-only tick.
        #[arg(long = "from")]
        from: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        dry_run: bool,
        /// Write the Markdown digest here.
        #[arg(long)]
        report: Option<String>,
        #[arg(long)]
        as_of: Option<String>,
    },
    /// What needs attention now (band changes, new CTAs, renewals, silence).
    Alerts {
        #[arg(long)]
        as_of: Option<String>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long = "within-days")]
        within_days: Option<i64>,
    },
    /// Record a customer signal by hand.
    Signal {
        account: String,
        /// 1-10
        #[arg(long)]
        score: i64,
        #[arg(long)]
        note: String,
        #[arg(long)]
        positive: bool,
        #[arg(long)]
        reference: Option<String>,
        #[arg(long)]
        date: Option<String>,
    },
    /// Health of one account (stores a snapshot, raises CTAs).
    Health {
        account: String,
        #[arg(long)]
        as_of: Option<String>,
    },
    /// Health of all accounts, worst first.
    HealthAll {
        #[arg(long)]
        as_of: Option<String>,
    },
    /// Advisory modifier 0.8-1.2 with a reason.
    Triage {
        account: String,
        #[arg(long)]
        modifier: Option<f64>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        days: Option<i64>,
        #[arg(long)]
        clear: bool,
    },
    /// Open calls to action.
    Cta {
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Resolve a CTA.
    CtaDone {
        id: i64,
        #[arg(long)]
        note: String,
        #[arg(long)]
        dismiss: bool,
    },
    /// Notice dates and term ends.
    Deadlines {
        #[arg(long)]
        days: Option<i64>,
        #[arg(long)]
        as_of: Option<String>,
    },
    /// Show or set health weights.
    Weights {
        #[arg(long)]
        adoption: Option<f64>,
        #[arg(long)]
        recency: Option<f64>,
        #[arg(long)]
        sentiment: Option<f64>,
    },
    /// Create a call to action by hand.
    CtaAdd {
        account: String,
        #[arg(long)]
        title: String,
        /// Due date (DD.MM.YYYY).
        #[arg(long)]
        due: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Write the health report as CSV or Markdown (format from the extension).
    Export {
        path: String,
        /// csv or md
        #[arg(long)]
        format: Option<String>,
        #[arg(long)]
        as_of: Option<String>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Copy the database to a new file.
    Backup { path: String },
    /// Churn-risk audit and backtest of retired accounts.
    Audit {
        #[arg(long)]
        as_of: Option<String>,
        /// Days before the churn date to score retired accounts (7-365, default 90).
        #[arg(long)]
        lead_days: Option<i64>,
    },
    /// Apply events from contractcraft, deskcraft, formcraft.
    InboxDrain,
    /// Fill an empty database with labelled demo accounts.
    DemoSeed,
    /// Print the MCP tool catalog (JSON, or markdown for docs/TOOLS.md).
    Tools {
        #[arg(long)]
        markdown: bool,
    },
    /// Run the MCP stdio server.
    Mcp,
}

fn default_db() -> PathBuf {
    if let Ok(p) = std::env::var("GAINCRAFT_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("gaincraft").join("gaincraft.db")
}

fn obj(pairs: Vec<(&str, Option<Value>)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        if let Some(v) = v {
            m.insert(k.to_string(), v);
        }
    }
    Value::Object(m)
}
fn sv(s: &str) -> Option<Value> {
    Some(Value::String(s.to_string()))
}
fn os(s: Option<String>) -> Option<Value> {
    s.map(Value::String)
}
fn oi(i: Option<i64>) -> Option<Value> {
    i.map(|v| json!(v))
}

fn to_tool(cmd: Cmd) -> Result<(&'static str, Value), String> {
    Ok(match cmd {
        Cmd::Status { as_of } => ("gain_status", obj(vec![("as_of", os(as_of))])),
        Cmd::Add {
            name,
            key,
            domain,
            owner,
            arr,
            seats,
            notes,
        } => (
            "gain_account_add",
            obj(vec![
                ("name", sv(&name)),
                ("key", os(key)),
                ("domain", os(domain)),
                ("owner", os(owner)),
                ("arr", os(arr)),
                ("seats", oi(seats)),
                ("notes", os(notes)),
            ]),
        ),
        Cmd::List { all } => ("gain_account_list", json!({ "include_inactive": all })),
        Cmd::Show { account, as_of } => (
            "gain_account_show",
            obj(vec![("account", sv(&account)), ("as_of", os(as_of))]),
        ),
        Cmd::Update {
            account,
            name,
            domain,
            owner,
            arr,
            seats,
            contract,
            term_end,
            last_notice,
            notes,
            retire,
            retired_on,
        } => (
            "gain_account_update",
            obj(vec![
                ("account", sv(&account)),
                ("name", os(name)),
                ("domain", os(domain)),
                ("owner", os(owner)),
                ("arr", os(arr)),
                ("seats", oi(seats)),
                ("contract_id", os(contract)),
                ("term_end", os(term_end)),
                ("last_notice", os(last_notice)),
                ("notes", os(notes)),
                (
                    "active",
                    if retire || retired_on.is_some() {
                        Some(json!(false))
                    } else {
                        None
                    },
                ),
                ("retired_on", os(retired_on)),
            ]),
        ),
        Cmd::Import { path, dry_run } => (
            "gain_telemetry_import",
            json!({ "path": path, "dry_run": dry_run }),
        ),
        Cmd::UsagePull {
            from,
            query,
            dry_run,
        } => (
            "gain_usage_pull",
            json!({ "source": "sqlite", "path": from, "query": query, "dry_run": dry_run }),
        ),
        Cmd::Tick {
            from,
            query,
            dry_run,
            report,
            as_of,
        } => (
            "gain_tick",
            obj(vec![
                ("source", from.as_ref().map(|_| serde_json::json!("sqlite"))),
                ("path", os(from)),
                ("query", os(query)),
                ("dry_run", Some(json!(dry_run))),
                ("report", os(report)),
                ("as_of", os(as_of)),
            ]),
        ),
        Cmd::Alerts {
            as_of,
            since,
            within_days,
        } => (
            "gain_alerts",
            obj(vec![
                ("as_of", os(as_of)),
                ("since", os(since)),
                ("within_days", within_days.map(|n| json!(n))),
            ]),
        ),
        Cmd::Signal {
            account,
            score,
            note,
            positive,
            reference,
            date,
        } => (
            "gain_signal_add",
            obj(vec![
                ("account", sv(&account)),
                ("score", Some(json!(score))),
                ("note", sv(&note)),
                (
                    "polarity",
                    sv(if positive { "positive" } else { "negative" }),
                ),
                ("reference", os(reference)),
                ("date", os(date)),
            ]),
        ),
        Cmd::Health { account, as_of } => (
            "gain_health",
            obj(vec![("account", sv(&account)), ("as_of", os(as_of))]),
        ),
        Cmd::HealthAll { as_of } => ("gain_health_all", obj(vec![("as_of", os(as_of))])),
        Cmd::Triage {
            account,
            modifier,
            reason,
            days,
            clear,
        } => (
            "gain_triage_set",
            obj(vec![
                ("account", sv(&account)),
                ("modifier", modifier.map(|m| json!(m))),
                ("reason", os(reason)),
                ("days", oi(days)),
                ("clear", Some(json!(clear))),
            ]),
        ),
        Cmd::Cta { account, all } => (
            "gain_cta_list",
            obj(vec![
                ("account", os(account)),
                ("include_closed", Some(json!(all))),
            ]),
        ),
        Cmd::CtaDone { id, note, dismiss } => (
            "gain_cta_resolve",
            json!({ "id": id, "note": note, "dismiss": dismiss }),
        ),
        Cmd::Deadlines { days, as_of } => (
            "gain_deadlines",
            obj(vec![("within_days", oi(days)), ("as_of", os(as_of))]),
        ),
        Cmd::Weights {
            adoption,
            recency,
            sentiment,
        } => match (adoption, recency, sentiment) {
            (None, None, None) => ("gain_weights", json!({})),
            (Some(a), Some(r), Some(s)) => (
                "gain_weights",
                json!({ "set": { "adoption": a, "recency": r, "sentiment": s } }),
            ),
            _ => {
                return Err(
                    "set all three weights (--adoption --recency --sentiment) or none".into(),
                )
            }
        },
        Cmd::CtaAdd {
            account,
            title,
            due,
            note,
        } => (
            "gain_cta_add",
            obj(vec![
                ("account", sv(&account)),
                ("title", sv(&title)),
                ("due", os(due)),
                ("note", os(note)),
            ]),
        ),
        Cmd::Export {
            path,
            format,
            as_of,
            overwrite,
        } => (
            "gain_export",
            obj(vec![
                ("path", sv(&path)),
                ("format", os(format)),
                ("as_of", os(as_of)),
                ("overwrite", Some(json!(overwrite))),
            ]),
        ),
        Cmd::Backup { path } => ("gain_backup", json!({ "path": path })),
        Cmd::Audit { as_of, lead_days } => (
            "gain_churn_audit",
            obj(vec![("as_of", os(as_of)), ("lead_days", oi(lead_days))]),
        ),
        Cmd::InboxDrain => ("gain_inbox_drain", json!({})),
        Cmd::DemoSeed => ("gain_demo_seed", json!({})),
        Cmd::Tools { .. } | Cmd::Mcp => return Err("not a tool verb".into()),
    })
}

struct Host {
    ctx: Ctx,
}

impl ToolHost for Host {
    fn server_name(&self) -> &str {
        APP
    }
    fn server_version(&self) -> &str {
        VERSION
    }
    fn tools(&self) -> Vec<Value> {
        tools()
    }
    fn call(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let cmd = Command::from_tool(name, args)?;
        dispatch(&mut self.ctx, cmd)
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".into(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() => format!("{f:.2}"),
            _ => n.to_string(),
        },
        Value::Array(a) => a.iter().map(scalar).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

const COLUMN_ORDER: &[&str] = &[
    "id",
    "date",
    "days_left",
    "due",
    "kind",
    "account",
    "key",
    "name",
    "score",
    "band",
    "adoption",
    "recency",
    "sentiment",
    "modifier",
    "trend_14d",
    "arr",
    "seats",
    "open_ctas",
    "title",
    "status",
    "domain",
    "owner",
    "contract_id",
    "contract",
    "last_notice",
    "term_end",
    "days",
    "inserted",
    "replaced",
];
const HIDDEN: &[&str] = &[
    "account_id",
    "arr_cents",
    "params",
    "auto_created",
    "renewal_months",
    "notes",
    "created_on",
    "last_seen",
    "resolution",
    "resolved_by",
    "arr_cents",
    "active",
    "reference",
];

fn columns(rows: &[Value]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for r in rows.iter().filter_map(Value::as_object) {
        for (k, v) in r {
            if !v.is_object() && !HIDDEN.contains(&k.as_str()) && !keys.contains(k) {
                keys.push(k.clone());
            }
        }
    }
    keys.sort_by_key(|k| {
        (
            COLUMN_ORDER
                .iter()
                .position(|c| c == k)
                .unwrap_or(usize::MAX),
            k.clone(),
        )
    });
    keys
}

fn table(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "(none)\n".into();
    }
    let cols = columns(rows);
    let cut = |s: String| {
        if s.chars().count() > 60 {
            format!("{}...", s.chars().take(57).collect::<String>())
        } else {
            s
        }
    };
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            cols.iter()
                .map(|c| cut(r.get(c).map(scalar).unwrap_or_else(|| "-".into())))
                .collect()
        })
        .collect();
    let widths: Vec<usize> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| {
            cells
                .iter()
                .map(|r| r[i].chars().count())
                .chain([c.chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cs: &[String]| -> String {
        let p: Vec<String> = cs
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c}{}", " ".repeat(w.saturating_sub(c.chars().count()))))
            .collect();
        format!("{}\n", p.join("  ").trim_end())
    };
    let mut out = line(&cols);
    for r in &cells {
        out.push_str(&line(r));
    }
    out
}

/// Human rendering: a health breakdown as text, lists as tables, the rest as JSON.
fn render(v: &Value) -> String {
    let Some(obj) = v.as_object() else {
        return scalar(v);
    };
    if let Some(h) = obj.get("health").filter(|h| h.get("score").is_some()) {
        let mut out = format!(
            "{}  {}\nscore {} ({})  = 100 x [adoption {} | recency {} | sentiment {}] x modifier {}\navg active (30d) {}  days since active {}  trend 14d {}\n",
            scalar(obj.get("key").or_else(|| obj.get("account")).unwrap_or(&Value::Null)),
            scalar(obj.get("name").unwrap_or(&Value::Null)),
            scalar(&h["score"]),
            scalar(&h["band"]),
            scalar(&h["adoption"]),
            scalar(&h["recency"]),
            scalar(&h["sentiment"]),
            scalar(&h["modifier"]),
            scalar(&h["avg_active_30d"]),
            scalar(&h["days_since_active"]),
            scalar(&h["trend_14d"]),
        );
        for (label, k) in [
            ("open CTAs", "ctas"),
            ("signals (180d)", "signals"),
            ("score trend", "trend"),
        ] {
            if let Some(rows) = obj.get(k).and_then(Value::as_array) {
                out.push_str(&format!("\n{label}:\n{}", table(rows)));
            }
        }
        if let Some(n) = obj
            .get("new_ctas")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
        {
            out.push_str(&format!("new CTAs: {}\n", scalar(&Value::Array(n.clone()))));
        }
        return out;
    }
    if let (Some(risk), Some(bt)) = (
        obj.get("risk").and_then(Value::as_array),
        obj.get("backtest").filter(|b| b.is_object()),
    ) {
        let mut out = format!(
            "as of {}  at risk {}  ARR at risk {}\n\n{}",
            scalar(&obj["as_of"]),
            scalar(&obj["at_risk"]),
            scalar(&obj["arr_at_risk"]),
            table(risk)
        );
        out.push_str(&format!(
            "\nbacktest ({} days before churn): {}\n",
            scalar(&obj["lead_days"]),
            scalar(&bt["verdict"])
        ));
        if let Some(rows) = bt["accounts"].as_array().filter(|r| !r.is_empty()) {
            out.push_str(&table(rows));
        }
        return out;
    }
    const LISTS: [&str; 6] = ["rows", "accounts", "ctas", "risk", "applied", "rejected"];
    let lists: Vec<&str> = LISTS
        .iter()
        .copied()
        .filter(|k| obj.get(*k).is_some_and(Value::is_array))
        .collect();
    if lists.is_empty() {
        return serde_json::to_string_pretty(v).unwrap_or_else(|e| e.to_string());
    }
    let mut out = String::new();
    for (k, val) in obj {
        if !val.is_array() && !val.is_object() {
            out.push_str(&format!("{k}: {}\n", scalar(val)));
        }
    }
    let many = lists.len() > 1;
    for k in lists {
        if many {
            out.push_str(&format!("{k}:\n"));
        }
        out.push_str(&table(
            obj.get(k)
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        ));
    }
    out
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Cmd::Tools { markdown } = cli.cmd {
        if markdown {
            print!("{}", tools_markdown(APP, &tools()));
        } else {
            println!(
                "{}",
                serde_json::to_string_pretty(&tools()).unwrap_or_default()
            );
        }
        return ExitCode::SUCCESS;
    }
    let db_path = cli.db.clone().unwrap_or_else(default_db);
    let gain = match Gain::open(&db_path) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("gaincraft: cannot open {}: {e}", db_path.display());
            return ExitCode::FAILURE;
        }
    };
    let is_mcp = matches!(cli.cmd, Cmd::Mcp);
    let confinement = if is_mcp || !cli.allow_path.is_empty() {
        match Confinement::roots(&cli.allow_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("gaincraft: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        Confinement(None)
    };
    let actor = if is_mcp {
        "agent".to_string()
    } else {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "cli".into())
    };
    let ctx = Ctx {
        gain,
        confinement,
        bus: Bus::new(Bus::default_root()),
        actor,
    };
    if is_mcp {
        eprintln!("[gaincraft] db {}", db_path.display());
        let mut host = Host { ctx };
        return match serve_stdio(&mut host) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("gaincraft mcp: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let json_out = cli.json;
    let mut ctx = ctx;
    let result = to_tool(cli.cmd)
        .and_then(|(name, args)| Command::from_tool(name, &args))
        .and_then(|cmd| dispatch(&mut ctx, cmd));
    match result {
        Ok(v) => {
            if json_out {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                print!("{}", render(&v));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            if json_out {
                println!("{}", json!({ "error": e }));
            } else {
                eprintln!("gaincraft: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_biz::mcp::{docs_in_sync, handle_line};

    fn host(tag: &str) -> Host {
        let dir = std::env::temp_dir().join(format!("gaincraft-mcp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Host {
            ctx: Ctx {
                gain: Gain::open_in_memory().unwrap(),
                confinement: Confinement(Some(Vec::new())),
                bus: Bus::new(dir.join("bus")),
                actor: "agent".into(),
            },
        }
    }

    fn text(v: &Value) -> String {
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn mcp_probe_initialize_list_call() {
        let mut h = host("probe");
        let init = handle_line(
            &mut h,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "gaincraft");
        let list =
            handle_line(&mut h, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        assert_eq!(list["result"]["tools"].as_array().map(Vec::len), Some(23));
        let seed = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"gain_demo_seed","arguments":{}}}"#).unwrap();
        assert_eq!(seed["result"]["isError"], false, "{seed}");
        let show = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"gain.account_show","arguments":{"account":"mockinger.example","as_of":"09.11.2026"}}}"#).unwrap();
        let v: Value = serde_json::from_str(&text(&show)).unwrap();
        assert_eq!(v["health"]["band"], "red");
        let imp = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"gain_telemetry_import","arguments":{"path":"C:/Windows/win.ini"}}}"#).unwrap();
        assert_eq!(
            imp["result"]["isError"], true,
            "no file access without --allow-path"
        );
        let tri = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"gain_triage_set","arguments":{"account":"mockingbird.example","modifier":0.9,"reason":"agent read: champion left"}}}"#).unwrap();
        assert!(
            text(&tri).contains("\"source\": \"agent\"")
                || text(&tri).contains("\"source\":\"agent\""),
            "{tri}"
        );
    }

    #[test]
    fn cli_verbs_map_to_known_tools() {
        let verbs = vec![
            Cmd::Status { as_of: None },
            Cmd::List { all: false },
            Cmd::Import {
                path: "x.csv".into(),
                dry_run: true,
            },
            Cmd::UsagePull {
                from: "product.db".into(),
                query: "SELECT 1 AS account, '2026-11-02' AS date, 1 AS active_users".into(),
                dry_run: true,
            },
            Cmd::Signal {
                account: "a".into(),
                score: 3,
                note: "n".into(),
                positive: false,
                reference: None,
                date: None,
            },
            Cmd::HealthAll { as_of: None },
            Cmd::Cta {
                account: None,
                all: false,
            },
            Cmd::CtaDone {
                id: 1,
                note: "n".into(),
                dismiss: false,
            },
            Cmd::Deadlines {
                days: None,
                as_of: None,
            },
            Cmd::Weights {
                adoption: None,
                recency: None,
                sentiment: None,
            },
            Cmd::CtaAdd {
                account: "a".into(),
                title: "t".into(),
                due: Some("20.11.2026".into()),
                note: None,
            },
            Cmd::Export {
                path: "r.md".into(),
                format: None,
                as_of: None,
                overwrite: false,
            },
            Cmd::Backup {
                path: "copy.db".into(),
            },
            Cmd::Audit {
                as_of: None,
                lead_days: Some(30),
            },
            Cmd::Update {
                account: "a".into(),
                name: None,
                domain: None,
                owner: None,
                arr: None,
                seats: None,
                contract: None,
                term_end: None,
                last_notice: None,
                notes: None,
                retire: false,
                retired_on: Some("30.06.2026".into()),
            },
            Cmd::InboxDrain,
            Cmd::DemoSeed,
        ];
        let names: Vec<String> = tools()
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect();
        for v in verbs {
            let (name, args) = to_tool(v).unwrap();
            assert!(names.iter().any(|n| n == name), "{name}");
            assert!(Command::from_tool(name, &args).is_ok(), "{name} {args}");
        }
        assert!(to_tool(Cmd::Weights {
            adoption: Some(0.5),
            recency: None,
            sentiment: None
        })
        .is_err());
    }

    #[test]
    fn tools_doc_is_current() {
        let committed = include_str!("../../../docs/TOOLS.md");
        assert!(
            docs_in_sync(committed, &tools_markdown(APP, &tools())),
            "docs/TOOLS.md is stale: run `just docs`"
        );
    }

    #[test]
    fn mcpb_manifest_lists_exactly_the_catalog() {
        let m: Value = serde_json::from_str(include_str!("../../../mcpb/manifest.json")).unwrap();
        let mut listed: Vec<String> = m["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        let mut catalog: Vec<String> = tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        listed.sort();
        catalog.sort();
        assert_eq!(listed, catalog, "update mcpb/manifest.json");
        assert_eq!(m["server"]["entry_point"], "gaincraft.exe");
    }

    #[test]
    fn every_tool_has_a_cli_verb() {
        // Each tool is reachable from the CLI (AGENTS.md: every tool is also a verb).
        let src = include_str!("main.rs");
        let start = src.find("fn to_tool(").unwrap_or(0);
        let end = src.find("struct Host").unwrap_or(src.len());
        let body = &src[start..end];
        for t in tools() {
            let name = t["name"].as_str().unwrap_or_default();
            assert!(
                body.contains(&format!("\"{name}\"")),
                "no CLI verb maps to {name}"
            );
        }
    }

    #[test]
    fn render_audit_shows_risk_and_verdict() {
        let v = json!({ "as_of": "2026-11-09", "at_risk": 1, "arr_at_risk": "5.400,00", "lead_days": 90,
            "risk": [{ "account": "m", "score": 20, "band": "red", "reasons": ["band red"], "arr_cents": 540000 }],
            "backtest": { "verdict": "No retired accounts", "accounts": [] } });
        let s = render(&v);
        assert!(
            s.contains("band red") && s.contains("No retired accounts"),
            "{s}"
        );
        assert!(!s.contains("540000"), "cents column hidden: {s}");
    }

    #[test]
    fn render_health_breakdown() {
        let v = json!({ "key": "k", "name": "N", "health": { "score": 41, "band": "yellow", "adoption": 0.5, "recency": 0.25, "sentiment": 1.0, "modifier": 1.0, "avg_active_30d": 2.5, "days_since_active": 28, "trend_14d": null }, "ctas": [] });
        let s = render(&v);
        assert!(
            s.contains("score 41 (yellow)") && s.contains("recency 0.25"),
            "{s}"
        );
    }
}
