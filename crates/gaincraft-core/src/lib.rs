//! gaincraft-core: the command bus shared by CLI, MCP and the desktop app.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod health;

use chrono::{Datelike, Duration, Local, NaiveDate, SecondsFormat, Utc, Weekday};
use craft_biz::at::parse_date;
use craft_biz::bus::Bus;
pub use craft_biz::confine::Confinement;
use craft_biz::mcp::tool;
use craft_biz::Money;
use gaincraft_store::{
    Account, AccountPatch, Day, Gain, GainError, NewAccount, NewSignal, NewSyncRun,
};
use health::{Health, Weights};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub const APP: &str = "gaincraft";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_IMPORT_BYTES: u64 = 20 * 1024 * 1024;
/// Most rows one usage pull may read from a product database.
const MAX_PULL_ROWS: usize = 1_000_000;
/// A product-database query that runs longer is interrupted.
const PULL_TIMEOUT_SECS: u64 = 30;
/// Header words that suggest per-person data. Refused on import (ArbVG / DSGVO).
const PERSONAL_HEADERS: &[&str] = &[
    "user",
    "email",
    "e-mail",
    "mail",
    "name",
    "person",
    "employee",
    "mitarbeiter",
    "login",
    "ip",
    "uid",
    "userid",
    "user_id",
    "benutzer",
];

pub struct Ctx {
    pub gain: Gain,
    pub confinement: Confinement,
    pub bus: Bus,
    pub actor: String,
}

#[derive(Debug, Clone)]
pub enum Command {
    Status {
        as_of: Option<NaiveDate>,
    },
    AccountAdd(NewAccount),
    AccountList {
        include_inactive: bool,
    },
    AccountShow {
        account: String,
        as_of: Option<NaiveDate>,
    },
    AccountUpdate {
        account: String,
        patch: AccountPatch,
    },
    TelemetryImport {
        path: String,
        dry_run: bool,
    },
    /// Read aggregate usage straight from a product database (read-only).
    UsagePull {
        source: String,
        path: String,
        query: String,
        dry_run: bool,
    },
    /// Scheduled pass: optional usage pull, health for all, alerts, run history.
    Tick {
        source: Option<String>,
        path: Option<String>,
        query: Option<String>,
        dry_run: bool,
        report: Option<String>,
        as_of: Option<NaiveDate>,
    },
    /// What needs attention now (band changes, new CTAs, renewals, silence).
    Alerts {
        as_of: Option<NaiveDate>,
        since: Option<NaiveDate>,
        within_days: i64,
    },
    SignalAdd {
        account: String,
        polarity: String,
        score: i64,
        note: String,
        reference: Option<String>,
        at: Option<NaiveDate>,
    },
    Health {
        account: String,
        as_of: Option<NaiveDate>,
    },
    HealthAll {
        as_of: Option<NaiveDate>,
    },
    Triage {
        account: String,
        modifier: Option<f64>,
        reason: String,
        days: i64,
        clear: bool,
    },
    CtaList {
        account: Option<String>,
        include_closed: bool,
    },
    CtaResolve {
        id: i64,
        note: String,
        dismiss: bool,
    },
    Deadlines {
        within_days: i64,
        as_of: Option<NaiveDate>,
    },
    Weights {
        set: Option<Weights>,
    },
    InboxDrain,
    DemoSeed,
    CtaAdd {
        account: String,
        title: String,
        due: Option<NaiveDate>,
        note: Option<String>,
    },
    Export {
        path: String,
        format: Option<String>,
        as_of: Option<NaiveDate>,
        overwrite: bool,
    },
    Backup {
        path: String,
    },
    ChurnAudit {
        as_of: Option<NaiveDate>,
        lead_days: i64,
    },
}

// ---------- argument helpers ----------

fn s<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument {k:?}"))
}
fn opt_s(a: &Value, k: &str) -> Option<String> {
    a.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
}
fn e(err: GainError) -> String {
    err.to_string()
}
fn date(v: &str) -> Result<NaiveDate, String> {
    parse_date(v).map_err(|x| x.to_string())
}
fn opt_date(a: &Value, k: &str) -> Result<Option<NaiveDate>, String> {
    opt_s(a, k).map(|d| date(&d)).transpose()
}
fn int(a: &Value, k: &str) -> Result<Option<i64>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{k} must be an integer")),
    }
}
fn flag(a: &Value, k: &str) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(false)
}
fn today() -> NaiveDate {
    Local::now().date_naive()
}
fn eur(c: i64) -> String {
    Money::from_cents(c).format_de()
}
fn money(v: &Value, k: &str) -> Result<i64, String> {
    match v {
        Value::String(t) => Money::parse_de(t)
            .map(Money::cents)
            .map_err(|x| format!("{k}: {x}")),
        _ => Err(format!(
            "{k} must be a DE amount string like \"9.600,00\" (no floats)"
        )),
    }
}
fn tri<T>(
    a: &Value,
    k: &str,
    f: impl Fn(&Value) -> Result<T, String>,
) -> Result<Option<Option<T>>, String> {
    match a.get(k) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v) => f(v).map(|x| Some(Some(x))),
    }
}
fn str_of(v: &Value) -> Result<String, String> {
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| "expected a string".into())
}

impl Command {
    pub fn from_tool(name: &str, a: &Value) -> Result<Command, String> {
        let name = craft_biz::mcp::normalize_tool_name(name);
        Ok(match name.as_str() {
            "gain_status" => Command::Status {
                as_of: opt_date(a, "as_of")?,
            },
            "gain_account_add" => Command::AccountAdd(NewAccount {
                key: opt_s(a, "key"),
                name: s(a, "name")?.into(),
                domain: opt_s(a, "domain"),
                owner: opt_s(a, "owner"),
                arr_cents: a
                    .get("arr")
                    .filter(|v| !v.is_null())
                    .map(|v| money(v, "arr"))
                    .transpose()?,
                seats: int(a, "seats")?,
                notes: opt_s(a, "notes").unwrap_or_default(),
            }),
            "gain_account_list" => Command::AccountList {
                include_inactive: flag(a, "include_inactive"),
            },
            "gain_account_show" => Command::AccountShow {
                account: s(a, "account")?.into(),
                as_of: opt_date(a, "as_of")?,
            },
            "gain_account_update" => Command::AccountUpdate {
                account: s(a, "account")?.into(),
                patch: AccountPatch {
                    name: opt_s(a, "name"),
                    domain: tri(a, "domain", str_of)?,
                    owner: tri(a, "owner", str_of)?,
                    arr_cents: tri(a, "arr", |v| money(v, "arr"))?,
                    seats: tri(a, "seats", |v| {
                        v.as_i64()
                            .ok_or_else(|| "seats must be an integer".to_string())
                    })?,
                    contract_id: tri(a, "contract_id", str_of)?,
                    term_end: tri(a, "term_end", |v| str_of(v).and_then(|t| date(&t)))?,
                    last_notice: tri(a, "last_notice", |v| str_of(v).and_then(|t| date(&t)))?,
                    renewal_months: tri(a, "renewal_months", |v| {
                        v.as_i64()
                            .ok_or_else(|| "renewal_months must be an integer".to_string())
                    })?,
                    active: a.get("active").and_then(Value::as_bool),
                    notes: a.get("notes").and_then(Value::as_str).map(str::to_string),
                    retired_on: tri(a, "retired_on", |v| str_of(v).and_then(|t| date(&t)))?,
                },
            },
            "gain_telemetry_import" => Command::TelemetryImport {
                path: s(a, "path")?.into(),
                dry_run: flag(a, "dry_run"),
            },
            "gain_usage_pull" => Command::UsagePull {
                source: opt_s(a, "source").unwrap_or_else(|| "sqlite".into()),
                path: s(a, "path")?.into(),
                query: s(a, "query")?.into(),
                dry_run: flag(a, "dry_run"),
            },
            "gain_tick" => {
                let path = opt_s(a, "path");
                let query = opt_s(a, "query");
                if path.is_some() != query.is_some() {
                    return Err(
                        "path and query go together; omit both for a health-only tick".into(),
                    );
                }
                Command::Tick {
                    source: opt_s(a, "source"),
                    path,
                    query,
                    dry_run: flag(a, "dry_run"),
                    report: opt_s(a, "report"),
                    as_of: opt_date(a, "as_of")?,
                }
            }
            "gain_alerts" => Command::Alerts {
                as_of: opt_date(a, "as_of")?,
                since: opt_date(a, "since")?,
                within_days: int(a, "within_days")?.unwrap_or(60).clamp(0, 3650),
            },
            "gain_signal_add" => Command::SignalAdd {
                account: s(a, "account")?.into(),
                polarity: opt_s(a, "polarity").unwrap_or_else(|| "negative".into()),
                score: int(a, "score")?.ok_or("missing score (1-10)")?,
                note: s(a, "note")?.into(),
                reference: opt_s(a, "reference"),
                at: opt_date(a, "date")?,
            },
            "gain_health" => Command::Health {
                account: s(a, "account")?.into(),
                as_of: opt_date(a, "as_of")?,
            },
            "gain_health_all" => Command::HealthAll {
                as_of: opt_date(a, "as_of")?,
            },
            "gain_triage_set" => Command::Triage {
                account: s(a, "account")?.into(),
                modifier: a.get("modifier").and_then(Value::as_f64),
                reason: opt_s(a, "reason").unwrap_or_default(),
                days: int(a, "days")?.unwrap_or(30),
                clear: flag(a, "clear"),
            },
            "gain_cta_list" => Command::CtaList {
                account: opt_s(a, "account"),
                include_closed: flag(a, "include_closed"),
            },
            "gain_cta_resolve" => Command::CtaResolve {
                id: int(a, "id")?.ok_or("missing id")?,
                note: s(a, "note")?.into(),
                dismiss: flag(a, "dismiss"),
            },
            "gain_deadlines" => Command::Deadlines {
                within_days: int(a, "within_days")?.unwrap_or(90).clamp(0, 3650),
                as_of: opt_date(a, "as_of")?,
            },
            "gain_weights" => Command::Weights {
                set: match a.get("set") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(serde_json::from_value(v.clone()).map_err(|x| {
                        format!("set: {x} (object with adoption, recency, sentiment)")
                    })?),
                },
            },
            "gain_inbox_drain" => Command::InboxDrain,
            "gain_demo_seed" => Command::DemoSeed,
            "gain_cta_add" => Command::CtaAdd {
                account: s(a, "account")?.into(),
                title: s(a, "title")?.into(),
                due: opt_date(a, "due")?,
                note: opt_s(a, "note"),
            },
            "gain_export" => Command::Export {
                path: s(a, "path")?.into(),
                format: opt_s(a, "format"),
                as_of: opt_date(a, "as_of")?,
                overwrite: flag(a, "overwrite"),
            },
            "gain_backup" => Command::Backup {
                path: s(a, "path")?.into(),
            },
            "gain_churn_audit" => Command::ChurnAudit {
                as_of: opt_date(a, "as_of")?,
                lead_days: int(a, "lead_days")?.unwrap_or(90).clamp(7, 365),
            },
            other => return Err(format!("unknown tool {other}")),
        })
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|x| x.to_string())
}

pub fn weights(g: &Gain) -> Result<Weights, String> {
    match g.meta("weights").map_err(e)? {
        Some(t) => serde_json::from_str(&t).map_err(|x| format!("stored weights: {x}")),
        None => Ok(Weights::default()),
    }
}

fn account_json(a: &Account) -> Result<Value, String> {
    let mut v = to_json(a)?;
    v["arr"] = json!(a.arr_cents.map(eur));
    Ok(v)
}

pub fn dispatch(ctx: &mut Ctx, cmd: Command) -> Result<Value, String> {
    let actor = ctx.actor.clone();
    let g = &ctx.gain;
    match cmd {
        Command::Status { as_of } => {
            let st = g.stats(as_of.unwrap_or_else(today)).map_err(e)?;
            Ok(
                json!({ "app": APP, "version": VERSION, "stats": to_json(&st)?, "arr_active": eur(st.arr_active_cents), "weights": to_json(&weights(g)?)? }),
            )
        }
        Command::AccountAdd(n) => account_json(&g.add_account(&n, false).map_err(e)?),
        Command::AccountList { include_inactive } => Ok(
            json!({ "accounts": g.accounts(include_inactive).map_err(e)?.iter().map(account_json).collect::<Result<Vec<_>, _>>()? }),
        ),
        Command::AccountShow { account, as_of } => {
            let as_of = as_of.unwrap_or_else(today);
            let a = g.account(&account).map_err(e)?;
            let mut v = account_json(&a)?;
            v["health"] = to_json(&health_of(g, &a, as_of)?)?;
            v["trend"] = to_json(&g.snapshots(a.id, 12).map_err(e)?)?;
            v["signals"] = to_json(
                &g.signals(Some(a.id), Some(as_of - Duration::days(180)))
                    .map_err(e)?,
            )?;
            v["ctas"] = to_json(&g.ctas(Some(a.id), false).map_err(e)?)?;
            v["triage"] = to_json(&g.triage(a.id, as_of).map_err(e)?)?;
            Ok(v)
        }
        Command::AccountUpdate { account, patch } => {
            account_json(&g.update_account(&account, &patch).map_err(e)?)
        }
        Command::TelemetryImport { path, dry_run } => telemetry_import(ctx, &path, dry_run),
        Command::UsagePull {
            source,
            path,
            query,
            dry_run,
        } => usage_pull(ctx, &source, &path, &query, dry_run),
        Command::Tick {
            source,
            path,
            query,
            dry_run,
            report,
            as_of,
        } => tick(
            ctx,
            source.as_deref(),
            path.as_deref(),
            query.as_deref(),
            dry_run,
            report.as_deref(),
            as_of,
        ),
        Command::Alerts {
            as_of,
            since,
            within_days,
        } => alerts(g, as_of.unwrap_or_else(today), since, within_days),
        Command::SignalAdd {
            account,
            polarity,
            score,
            note,
            reference,
            at,
        } => {
            let a = g.account(&account).map_err(e)?;
            let sig = NewSignal {
                source: format!("manual:{actor}"),
                polarity,
                score,
                reference,
                note,
                at: at.unwrap_or_else(today),
                event_id: None,
            };
            to_json(&g.add_signal(a.id, &sig).map_err(e)?)
        }
        Command::Health { account, as_of } => {
            let a = g.account(&account).map_err(e)?;
            let as_of = as_of.unwrap_or_else(today);
            let h = refresh(g, &a, as_of)?;
            Ok(
                json!({ "account": a.key, "name": a.name, "as_of": as_of, "health": to_json(&h.0)?, "new_ctas": h.1 }),
            )
        }
        Command::HealthAll { as_of } => health_all(g, as_of.unwrap_or_else(today)),
        Command::Triage {
            account,
            modifier,
            reason,
            days,
            clear,
        } => {
            let a = g.account(&account).map_err(e)?;
            if clear {
                return Ok(
                    json!({ "account": a.key, "cleared": g.clear_triage(a.id).map_err(e)? }),
                );
            }
            let m = modifier.ok_or("pass modifier (0.8-1.2) or clear")?;
            let t = g
                .set_triage(a.id, m, &reason, &actor, today(), days)
                .map_err(e)?;
            Ok(
                json!({ "account": a.key, "triage": to_json(&t)?, "note": "advisory: multiplies the deterministic score, capped 0.8-1.2" }),
            )
        }
        Command::CtaList {
            account,
            include_closed,
        } => {
            let id = account
                .map(|k| g.account(&k).map(|a| a.id))
                .transpose()
                .map_err(e)?;
            Ok(json!({ "ctas": to_json(&g.ctas(id, include_closed).map_err(e)?)? }))
        }
        Command::CtaResolve { id, note, dismiss } => {
            to_json(&g.resolve_cta(id, dismiss, &note, &actor).map_err(e)?)
        }
        Command::Deadlines { within_days, as_of } => {
            deadlines(g, within_days, as_of.unwrap_or_else(today))
        }
        Command::Weights { set } => {
            if let Some(w) = set {
                w.validate()?;
                g.set_meta(
                    "weights",
                    &serde_json::to_string(&w).map_err(|x| x.to_string())?,
                )
                .map_err(e)?;
            }
            Ok(json!({ "weights": to_json(&weights(g)?)? }))
        }
        Command::InboxDrain => inbox_drain(ctx),
        Command::DemoSeed => demo_seed(&ctx.gain),
        Command::CtaAdd {
            account,
            title,
            due,
            note,
        } => {
            let a = g.account(&account).map_err(e)?;
            let reference = g.next_manual_ref().map_err(e)?;
            let params = json!({ "note": note, "by": actor });
            g.raise_cta_with(
                a.id,
                "manual",
                &reference,
                &title,
                &params.to_string(),
                due,
                today(),
            )
            .map_err(e)?;
            let c = g
                .ctas(Some(a.id), false)
                .map_err(e)?
                .into_iter()
                .find(|c| c.kind == "manual" && c.reference == reference)
                .ok_or("CTA was not stored")?;
            to_json(&c)
        }
        Command::Export {
            path,
            format,
            as_of,
            overwrite,
        } => export(
            ctx,
            &path,
            format.as_deref(),
            as_of.unwrap_or_else(today),
            overwrite,
        ),
        Command::Backup { path } => {
            let p = ctx.confinement.check(&path, true)?;
            let bytes = ctx.gain.backup_to(&p).map_err(e)?;
            Ok(json!({ "path": p.display().to_string(), "bytes": bytes }))
        }
        Command::ChurnAudit { as_of, lead_days } => {
            churn_audit(g, as_of.unwrap_or_else(today), lead_days)
        }
    }
}

/// One row of the health report: account, health now, open CTAs.
fn report_rows(g: &Gain, as_of: NaiveDate) -> Result<Vec<(Account, Health, usize)>, String> {
    let mut rows = Vec::new();
    for a in g.accounts(false).map_err(e)? {
        let h = health_of(g, &a, as_of)?;
        let n = g.ctas(Some(a.id), false).map_err(e)?.len();
        rows.push((a, h, n));
    }
    rows.sort_by(|x, y| x.1.score.cmp(&y.1.score).then(x.0.name.cmp(&y.0.name)));
    Ok(rows)
}

/// DE decimal for CSV (Excel in AT/DE reads `0,45`).
fn dec(x: f64) -> String {
    format!("{x:.2}").replace('.', ",")
}

fn csv_field(s: &str) -> String {
    if s.contains([';', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn de_date(d: Option<NaiveDate>) -> String {
    d.map(|d| d.format("%d.%m.%Y").to_string())
        .unwrap_or_default()
}

/// Health report of all active accounts as CSV (`;`, DE numbers) or Markdown.
fn export(
    ctx: &Ctx,
    path: &str,
    format: Option<&str>,
    as_of: NaiveDate,
    overwrite: bool,
) -> Result<Value, String> {
    let fmt = match format.map(str::to_lowercase) {
        Some(f) => f,
        None => std::path::Path::new(path)
            .extension()
            .and_then(|x| x.to_str())
            .map(str::to_lowercase)
            .unwrap_or_else(|| "csv".into()),
    };
    if !matches!(fmt.as_str(), "csv" | "md") {
        return Err(format!("format {fmt:?}: use csv or md"));
    }
    let p = ctx.confinement.check(path, true)?;
    if p.exists() && !overwrite {
        return Err(format!(
            "{} exists; pass overwrite=true to replace it",
            p.display()
        ));
    }
    let g = &ctx.gain;
    let rows = report_rows(g, as_of)?;
    let at_risk: i64 = rows
        .iter()
        .filter(|r| r.1.band != "green")
        .map(|r| r.0.arr_cents.unwrap_or(0))
        .sum();
    let mut out = String::new();
    if fmt == "csv" {
        out.push_str("account;name;owner;score;band;adoption;recency;sentiment;modifier;arr;notice;term_end;open_ctas\n");
        for (a, h, n) in &rows {
            let cols = [
                csv_field(&a.key),
                csv_field(&a.name),
                csv_field(a.owner.as_deref().unwrap_or("")),
                h.score.to_string(),
                h.band.to_string(),
                dec(h.adoption),
                dec(h.recency),
                dec(h.sentiment),
                dec(h.modifier),
                a.arr_cents.map(eur).unwrap_or_default(),
                de_date(a.last_notice),
                de_date(a.term_end),
                n.to_string(),
            ];
            out.push_str(&cols.join(";"));
            out.push('\n');
        }
    } else {
        out.push_str(&format!(
            "# gaincraft health report\n\nAs of {}. {} active accounts, ARR at risk (yellow + red): {} EUR.\n\nScore = 100 x (adoption, recency, sentiment weighted) x modifier. Weights: {}.\n\n",
            as_of.format("%d.%m.%Y"),
            rows.len(),
            eur(at_risk),
            serde_json::to_string(&weights(g)?).map_err(|x| x.to_string())?
        ));
        out.push_str("| Account | Owner | Score | Band | Adoption | Recency | Sentiment | Modifier | ARR | Notice | Open CTAs |\n|---|---|---:|---|---:|---:|---:|---:|---:|---|---:|\n");
        for (a, h, n) in &rows {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {:.2} | {:.2} | {:.2} | {:.2} | {} | {} | {} |\n",
                md_cell(&a.name),
                md_cell(a.owner.as_deref().unwrap_or("")),
                h.score,
                h.band,
                h.adoption,
                h.recency,
                h.sentiment,
                h.modifier,
                a.arr_cents.map(eur).unwrap_or_default(),
                de_date(a.last_notice),
                n
            ));
        }
    }
    std::fs::write(&p, out).map_err(|x| format!("{}: {x}", p.display()))?;
    Ok(
        json!({ "path": p.display().to_string(), "format": fmt, "rows": rows.len(), "as_of": as_of, "arr_at_risk": eur(at_risk) }),
    )
}

/// Who is at risk now and why, plus an honest backtest: for every retired
/// account with a churn date, how did the score rank it `lead_days` before it left?
pub fn churn_audit(g: &Gain, as_of: NaiveDate, lead_days: i64) -> Result<Value, String> {
    let all = g.accounts(true).map_err(e)?;
    let mut risk = Vec::new();
    for a in all.iter().filter(|a| a.active) {
        let now = health_of(g, a, as_of)?;
        let d30 = now.score - health_of(g, a, as_of - Duration::days(30))?.score;
        let d90 = now.score - health_of(g, a, as_of - Duration::days(90))?.score;
        let notice_in = a
            .last_notice
            .filter(|n| *n >= as_of)
            .map(|n| (n - as_of).num_days());
        let mut reasons = Vec::new();
        if now.band != "green" {
            reasons.push(format!("band {}", now.band));
        }
        if d30 <= -10 {
            reasons.push(format!("score {d30:+} in 30 days"));
        }
        if let Some(d) = notice_in.filter(|d| *d <= 90 && now.band != "green") {
            reasons.push(format!("notice in {d} days while not green"));
        }
        if now.sentiment < 0.6 {
            reasons.push(format!("sentiment {:.2}", now.sentiment));
        }
        risk.push(json!({
            "account": a.key, "name": a.name, "score": now.score, "band": now.band,
            "delta_30d": d30, "delta_90d": d90, "notice_in_days": notice_in,
            "arr": a.arr_cents.map(eur), "arr_cents": a.arr_cents.unwrap_or(0),
            "at_risk": !reasons.is_empty(), "reasons": reasons,
        }));
    }
    risk.sort_by(|x, y| {
        y["at_risk"]
            .as_bool()
            .cmp(&x["at_risk"].as_bool())
            .then(x["score"].as_i64().cmp(&y["score"].as_i64()))
    });
    let arr_at_risk: i64 = risk
        .iter()
        .filter(|r| r["at_risk"] == true)
        .filter_map(|r| r["arr_cents"].as_i64())
        .sum();

    let mut backtest = Vec::new();
    for c in all.iter().filter(|a| !a.active) {
        let Some(left) = c.retired_on else { continue };
        let at = left - Duration::days(lead_days);
        // Accounts alive on `at` that had any usage by then (no data = no fair score).
        let mut cohort = Vec::new();
        for a in &all {
            let alive = a.retired_on.is_none_or(|r| r > at);
            let seen = g.last_active_day(a.id, at).map_err(e)?.is_some();
            if a.id == c.id || (alive && seen) {
                cohort.push((a.id, health_of(g, a, at)?.score));
            }
        }
        cohort.sort_by_key(|x| x.1);
        let h = health_of(g, c, at)?;
        let rank = cohort.iter().position(|x| x.0 == c.id).map(|i| i + 1);
        backtest.push(json!({
            "account": c.key, "name": c.name, "retired_on": left, "scored_on": at,
            "score": h.score, "band": h.band, "flagged": h.band != "green",
            "rank": rank, "of": cohort.len(),
            "had_usage": g.last_active_day(c.id, at).map_err(e)?.is_some(),
        }));
    }
    let n = backtest.len();
    let flagged = backtest.iter().filter(|b| b["flagged"] == true).count();
    let mean_pct = if n == 0 {
        None
    } else {
        let sum: f64 = backtest
            .iter()
            .filter_map(|b| Some(b["rank"].as_f64()? / b["of"].as_f64()?.max(1.0)))
            .sum();
        Some((sum / n as f64 * 100.0).round() / 100.0)
    };
    let verdict = match n {
        0 => "No retired accounts with a churn date: nothing to backtest. Retire churned accounts with update --retire (optionally --retired-on DATE).".to_string(),
        1..=4 => format!("{flagged} of {n} churned accounts were yellow or red {lead_days} days before they left. Too few to tune weights: treat as anecdote, not evidence."),
        _ => format!("{flagged} of {n} churned accounts were yellow or red {lead_days} days before they left; average rank in the bottom {:.0}% of the book.", mean_pct.unwrap_or(1.0) * 100.0),
    };
    Ok(json!({
        "as_of": as_of, "lead_days": lead_days,
        "at_risk": risk.iter().filter(|r| r["at_risk"] == true).count(),
        "arr_at_risk": eur(arr_at_risk),
        "risk": risk,
        "backtest": { "churned": n, "flagged": flagged, "mean_rank_fraction": mean_pct, "verdict": verdict, "accounts": backtest },
    }))
}

/// Compute health without side effects (no snapshot, no CTAs). For views.
pub fn health_of(g: &Gain, a: &Account, as_of: NaiveDate) -> Result<Health, String> {
    let days = g.days(a.id, as_of - Duration::days(89), as_of).map_err(e)?;
    let last = g.last_active_day(a.id, as_of).map_err(e)?;
    let signals = g
        .signals(Some(a.id), Some(as_of - Duration::days(180)))
        .map_err(e)?;
    let modifier = g.triage(a.id, as_of).map_err(e)?.map(|t| t.modifier);
    Ok(health::compute(
        &weights(g)?,
        a.seats,
        &days,
        last,
        &signals,
        modifier,
        as_of,
    ))
}

/// Compute health, store a snapshot and raise CTAs. Returns the health and the CTA kinds newly opened.
pub fn refresh(g: &Gain, a: &Account, as_of: NaiveDate) -> Result<(Health, Vec<String>), String> {
    let h = health_of(g, a, as_of)?;
    g.snapshot(
        a.id,
        as_of,
        h.score,
        h.band,
        &serde_json::to_string(&h).map_err(|x| x.to_string())?,
    )
    .map_err(e)?;
    let mut opened = Vec::new();
    // `params` carries the values behind the English title so the window can
    // word the CTA in the user's language.
    let mut raise = |kind: &str,
                     reference: &str,
                     title: String,
                     params: Value,
                     due: Option<NaiveDate>|
     -> Result<(), String> {
        if g.raise_cta_with(
            a.id,
            kind,
            reference,
            &title,
            &params.to_string(),
            due,
            as_of,
        )
        .map_err(e)?
        {
            opened.push(kind.to_string());
        }
        Ok(())
    };
    if h.band == "red" {
        raise(
            "low_health",
            "",
            format!("Health {} (red): talk to {}", h.score, a.name),
            json!({ "score": h.score, "name": a.name }),
            Some(as_of + Duration::days(3)),
        )?;
    }
    if let Some(t) = h.trend_14d.filter(|t| *t < 0.6) {
        let pct = ((1.0 - t) * 100.0).round() as i64;
        raise(
            "adoption_drop",
            "",
            format!("Usage down {pct}% vs. the two weeks before"),
            json!({ "pct": pct }),
            Some(as_of + Duration::days(7)),
        )?;
    }
    if let Some(seats) = a.seats.filter(|s| h.avg_active_30d < 0.5 * *s as f64) {
        let avg = (h.avg_active_30d * 10.0).round() / 10.0;
        raise(
            "seats_unused",
            "",
            format!("Only {avg:.1} of {seats} seats active on average"),
            json!({ "avg": avg, "seats": seats }),
            None,
        )?;
    }
    if let Some(n) = a
        .last_notice
        .filter(|n| *n >= as_of && (*n - as_of).num_days() <= 60)
    {
        raise(
            "renewal",
            &n.to_string(),
            format!(
                "Notice deadline {}: secure the renewal",
                n.format("%d.%m.%Y")
            ),
            json!({ "date": n.format("%d.%m.%Y").to_string() }),
            Some(n),
        )?;
    }
    for s in g
        .signals(Some(a.id), Some(as_of - Duration::days(30)))
        .map_err(e)?
        .iter()
        .filter(|s| s.polarity == "negative")
    {
        let reference = s
            .reference
            .clone()
            .unwrap_or_else(|| format!("signal-{}", s.id));
        raise(
            "negative_signal",
            &reference,
            format!(
                "Unhappy signal from {} ({reference}), score {}",
                s.source, s.score
            ),
            json!({ "source": s.source, "reference": reference, "score": s.score }),
            Some(s.at + Duration::days(2)),
        )?;
    }
    Ok((h, opened))
}

pub fn health_all(g: &Gain, as_of: NaiveDate) -> Result<Value, String> {
    let mut rows = Vec::new();
    let (mut at_risk, mut new_ctas) = (0i64, 0usize);
    for a in g.accounts(false).map_err(e)? {
        let (h, opened) = refresh(g, &a, as_of)?;
        if h.band != "green" {
            at_risk += a.arr_cents.unwrap_or(0);
        }
        new_ctas += opened.len();
        rows.push(json!({
            "account": a.key, "name": a.name, "score": h.score, "band": h.band,
            "adoption": h.adoption, "recency": h.recency, "sentiment": h.sentiment, "modifier": h.modifier,
            "trend_14d": h.trend_14d, "arr": a.arr_cents.map(eur), "open_ctas": g.ctas(Some(a.id), false).map_err(e)?.len(),
        }));
    }
    rows.sort_by_key(|r| r["score"].as_i64().unwrap_or(0));
    Ok(
        json!({ "as_of": as_of, "accounts": rows.len(), "arr_at_risk": eur(at_risk), "new_ctas": new_ctas, "rows": rows }),
    )
}

fn deadlines(g: &Gain, within: i64, as_of: NaiveDate) -> Result<Value, String> {
    let mut rows = Vec::new();
    for a in g.accounts(false).map_err(e)? {
        for (kind, d) in [("notice", a.last_notice), ("term_end", a.term_end)] {
            if let Some(d) = d.filter(|d| *d >= as_of && (*d - as_of).num_days() <= within) {
                rows.push(json!({ "date": d, "days_left": (d - as_of).num_days(), "kind": kind, "account": a.key, "name": a.name, "arr": a.arr_cents.map(eur), "contract": a.contract_id }));
            }
        }
    }
    rows.sort_by(|x, y| x["date"].as_str().cmp(&y["date"].as_str()));
    Ok(json!({ "as_of": as_of, "within_days": within, "rows": rows }))
}

/// Refuses per-person columns before reading a single row.
pub fn check_header(cols: &[String]) -> Result<(), String> {
    for c in cols {
        let t = c.trim().to_lowercase();
        if PERSONAL_HEADERS
            .iter()
            .any(|p| t == *p || t.split(['_', ' ', '-']).any(|w| w == *p))
        {
            return Err(format!(
                "column {c:?} looks like per-person data. gaincraft only accepts aggregates per account and day (ArbVG section 96a, DSGVO data minimisation): account;date;active_users[;sessions;key_actions]"
            ));
        }
    }
    let need = ["account", "date", "active_users"];
    if let Some(m) = need
        .iter()
        .find(|n| !cols.iter().any(|c| c.trim().eq_ignore_ascii_case(n)))
    {
        return Err(format!(
            "missing column {m} (need account;date;active_users[;sessions;key_actions])"
        ));
    }
    if let Some(u) = cols.iter().find(|c| {
        !["account", "date", "active_users", "sessions", "key_actions"]
            .contains(&c.trim().to_lowercase().as_str())
    }) {
        return Err(format!(
            "unknown column {u:?}; allowed: account, date, active_users, sessions, key_actions"
        ));
    }
    Ok(())
}

/// Parse an aggregate CSV (`;` or `,`). Returns rows keyed by the account text.
pub fn parse_telemetry(text: &str) -> Result<Vec<(String, Day)>, String> {
    let mut lines = text
        .trim_start_matches('\u{feff}')
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let (_, header) = lines.next().ok_or("empty file")?;
    let sep = if header.contains(';') { ';' } else { ',' };
    let cols: Vec<String> = header
        .split(sep)
        .map(|c| c.trim().trim_matches('"').to_string())
        .collect();
    check_header(&cols)?;
    let idx = |n: &str| cols.iter().position(|c| c.trim().eq_ignore_ascii_case(n));
    let (ia, id, iu, is, ik) = (
        idx("account"),
        idx("date"),
        idx("active_users"),
        idx("sessions"),
        idx("key_actions"),
    );
    let mut out = Vec::new();
    for (i, line) in lines {
        let f: Vec<&str> = line
            .split(sep)
            .map(|x| x.trim().trim_matches('"'))
            .collect();
        let at = |m: String| format!("line {}: {m}", i + 1);
        let get = |ix: Option<usize>| ix.and_then(|k| f.get(k).copied()).unwrap_or("");
        let num = |ix: Option<usize>, name: &str| -> Result<i64, String> {
            let v = get(ix);
            if v.is_empty() && ix.is_some() && name != "active_users" {
                return Ok(0);
            }
            if ix.is_none() {
                return Ok(0);
            }
            v.parse::<i64>()
                .ok()
                .filter(|n| *n >= 0)
                .ok_or_else(|| at(format!("{name} {v:?} must be a whole number >= 0")))
        };
        let account = get(ia).to_string();
        if account.is_empty() {
            return Err(at("empty account".into()));
        }
        let day = Day {
            date: date(get(id)).map_err(at)?,
            active_users: num(iu, "active_users")?,
            sessions: num(is, "sessions")?,
            key_actions: num(ik, "key_actions")?,
        };
        out.push((account, day));
    }
    if out.is_empty() {
        return Err("no data rows".into());
    }
    Ok(out)
}

fn telemetry_import(ctx: &mut Ctx, path: &str, dry_run: bool) -> Result<Value, String> {
    let p = ctx.confinement.check(path, false)?;
    let size = std::fs::metadata(&p)
        .map_err(|x| format!("{}: {x}", p.display()))?
        .len();
    if size > MAX_IMPORT_BYTES {
        return Err("telemetry file larger than 20 MB".into());
    }
    let text = std::fs::read_to_string(&p).map_err(|x| format!("{}: {x}", p.display()))?;
    let rows = parse_telemetry(&text)?;
    import_rows(
        ctx,
        rows,
        dry_run,
        json!({ "kind": "csv", "path": p.display().to_string() }),
    )
}

/// Read aggregate usage straight from a SQLite product database.
///
/// The connection is opened read-only (`query_only`), the query must be one
/// SELECT/WITH statement, its result columns must be exactly the aggregate
/// schema (`account, date, active_users[, sessions, key_actions]`), and the
/// rows then go through the same importer as a CSV, so the privacy guard,
/// all-or-nothing resolution and per-day idempotency are identical.
fn usage_pull(
    ctx: &mut Ctx,
    source: &str,
    path: &str,
    query: &str,
    dry_run: bool,
) -> Result<Value, String> {
    let source = source.trim().to_ascii_lowercase();
    if source != "sqlite" {
        return Err(format!(
            "source {source:?} is not supported. gaincraft reads SQLite product databases (source \"sqlite\") and nothing else: no PostgreSQL/MySQL driver is built in."
        ));
    }
    let q = query.trim();
    if q.is_empty() {
        return Err("query is empty".into());
    }
    let head = q
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if head != "SELECT" && head != "WITH" {
        return Err(
            "query must be one SELECT (or WITH ... SELECT). gaincraft opens the product database read-only and never writes to it."
                .into(),
        );
    }
    if q.contains(';') {
        return Err("query must be a single statement; remove the ';'".into());
    }
    let p = ctx.confinement.check(path, false)?;
    let conn = Connection::open_with_flags(
        &p,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|x| format!("{}: {x}", p.display()))?;
    conn.execute_batch("PRAGMA query_only = ON;")
        .map_err(|x| format!("{}: {x}", p.display()))?;
    // rusqlite 0.40 has no public progress handler; interrupt from a timer.
    let done = Arc::new(AtomicBool::new(false));
    let interrupt = conn.get_interrupt_handle();
    let timer = {
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let deadline = Instant::now() + std::time::Duration::from_secs(PULL_TIMEOUT_SECS);
            while Instant::now() < deadline {
                if done.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if !done.load(Ordering::Relaxed) {
                interrupt.interrupt();
            }
        })
    };

    let result = (|| -> Result<(usize, String, Vec<String>), String> {
        let mut stmt = conn.prepare(q).map_err(pull_error)?;
        let cols: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
        check_header(&cols)?;
        let wanted = ["account", "date", "active_users", "sessions", "key_actions"];
        let mut text = String::from("account;date;active_users;sessions;key_actions\n");
        let mut read = 0usize;
        {
            let mut rows = stmt.query([]).map_err(pull_error)?;
            while let Some(row) = rows.next().map_err(pull_error)? {
                read += 1;
                if read > MAX_PULL_ROWS {
                    return Err(format!(
                        "query returned more than {MAX_PULL_ROWS} rows; aggregate further or narrow it"
                    ));
                }
                let mut vals: Vec<String> = Vec::with_capacity(wanted.len());
                for name in wanted {
                    let ix = cols
                        .iter()
                        .position(|c| c.trim().eq_ignore_ascii_case(name));
                    let v = match ix {
                        Some(i) => value_text(row.get_ref(i).map_err(pull_error)?, name)?,
                        None => String::new(),
                    };
                    if (name == "account" || name == "date")
                        && (v.contains(';') || v.contains('\n') || v.contains('\r'))
                    {
                        return Err(format!(
                            "value {v:?} contains ';' or a newline; return one cleaned value per column"
                        ));
                    }
                    vals.push(v);
                }
                let add: usize = vals.iter().map(|v| v.len() + 1).sum();
                if text.len() + add > MAX_IMPORT_BYTES as usize {
                    return Err("query result larger than 20 MB; aggregate further".into());
                }
                text.push_str(&vals.join(";"));
                text.push('\n');
            }
        }
        Ok((read, text, cols))
    })();
    done.store(true, Ordering::Relaxed);
    let _ = timer.join();
    drop(conn);
    let (read, text, cols) = result?;
    if read == 0 {
        return Err("query returned no rows".into());
    }
    let parsed = parse_telemetry(&text)?;
    import_rows(
        ctx,
        parsed,
        dry_run,
        json!({
            "kind": "sqlite",
            "path": p.display().to_string(),
            "rows_read": read,
            "columns": cols,
        }),
    )
}

fn pull_error(x: rusqlite::Error) -> String {
    if let rusqlite::Error::SqliteFailure(e, _) = &x {
        if e.code == rusqlite::ErrorCode::OperationInterrupted {
            return format!(
                "query exceeded {PULL_TIMEOUT_SECS} s and was interrupted; aggregate further or narrow it"
            );
        }
    }
    format!("query: {x} (read-only connection)")
}

/// One SQLite cell as CSV text; NULL stays empty so optional columns fall to 0.
fn value_text(v: rusqlite::types::ValueRef<'_>, col: &str) -> Result<String, String> {
    use rusqlite::types::ValueRef;
    Ok(match v {
        ValueRef::Null => String::new(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => {
            if f.fract() == 0.0 && f.is_finite() {
                format!("{}", f as i64)
            } else {
                return Err(format!(
                    "column {col} is fractional ({f}); counts must be whole numbers"
                ));
            }
        }
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        ValueRef::Blob(_) => return Err(format!("column {col} is a BLOB; return text or numbers")),
    })
}

/// Resolve every account before writing anything, then upsert day by day.
fn import_rows(
    ctx: &mut Ctx,
    rows: Vec<(String, Day)>,
    dry_run: bool,
    source: Value,
) -> Result<Value, String> {
    let mut by_account: BTreeMap<i64, (String, Vec<Day>)> = BTreeMap::new();
    for (acc, day) in rows {
        let a = ctx
            .gain
            .account(&acc)
            .map_err(|x| format!("{x} (add it first with gain_account_add)"))?;
        by_account
            .entry(a.id)
            .or_insert_with(|| (a.key.clone(), Vec::new()))
            .1
            .push(day);
    }
    let mut summary = Vec::new();
    for (id, (key, days)) in &by_account {
        let (ins, rep) = if dry_run {
            (days.len(), 0)
        } else {
            ctx.gain.upsert_days(*id, days).map_err(e)?
        };
        summary
            .push(json!({ "account": key, "days": days.len(), "inserted": ins, "replaced": rep }));
    }
    Ok(json!({ "dry_run": dry_run, "source": source, "accounts": summary.len(), "rows": summary }))
}

fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn date_of_ts(s: &str) -> Option<NaiveDate> {
    s.get(..10)
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
}

/// Band changes, CTAs created since `since`, renewals inside `within` days,
/// and at-risk accounts with no activity for 14+ days.
fn alerts(
    g: &Gain,
    as_of: NaiveDate,
    since: Option<NaiveDate>,
    within: i64,
) -> Result<Value, String> {
    let since = since.unwrap_or(as_of);
    let (mut changed, mut new_ctas, mut renewals, mut silent) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for a in g.accounts(false).map_err(e)? {
        let h = health_of(g, &a, as_of)?;
        let snaps = g.snapshots(a.id, 2).map_err(e)?;
        if let Some(prev) = snaps.iter().find(|s| s.date < as_of) {
            if prev.band != h.band {
                changed.push(json!({
                    "account": a.key, "name": a.name,
                    "from": prev.band, "to": h.band,
                    "score_from": prev.score, "score_to": h.score,
                    "arr": a.arr_cents.map(eur),
                }));
            }
        }
        for c in g.ctas(Some(a.id), true).map_err(e)? {
            if c.created_on >= since {
                new_ctas.push(json!({
                    "id": c.id, "account": a.key, "name": a.name,
                    "kind": c.kind, "title": c.title, "due": c.due,
                    "created_on": c.created_on, "status": c.status,
                    "arr": a.arr_cents.map(eur),
                }));
            }
        }
        for (kind, d) in [("notice", a.last_notice), ("term_end", a.term_end)] {
            if let Some(d) = d.filter(|d| *d >= as_of && (*d - as_of).num_days() <= within) {
                renewals.push(json!({
                    "account": a.key, "name": a.name, "kind": kind, "date": d,
                    "days_left": (d - as_of).num_days(), "arr": a.arr_cents.map(eur),
                }));
            }
        }
        if h.days_since_active.is_some_and(|n| n >= 14) && h.band != "green" {
            silent.push(json!({
                "account": a.key, "name": a.name,
                "days_since_active": h.days_since_active,
                "score": h.score, "band": h.band, "arr": a.arr_cents.map(eur),
            }));
        }
    }
    changed.sort_by_key(|v| v["score_to"].as_i64().unwrap_or(0));
    new_ctas.sort_by(|x, y| y["created_on"].as_str().cmp(&x["created_on"].as_str()));
    renewals.sort_by(|x, y| x["date"].as_str().cmp(&y["date"].as_str()));
    silent.sort_by_key(|v| -v["days_since_active"].as_i64().unwrap_or(0));
    let count = changed.len() + new_ctas.len() + renewals.len() + silent.len();
    Ok(json!({
        "as_of": as_of, "since": since, "within_days": within,
        "count": count,
        "changed": changed, "new_ctas": new_ctas,
        "renewals": renewals, "silent": silent,
    }))
}

/// One scheduled pass: optional pull, health for all, alerts, and a run record.
fn tick(
    ctx: &mut Ctx,
    source: Option<&str>,
    path: Option<&str>,
    query: Option<&str>,
    dry_run: bool,
    report: Option<&str>,
    as_of: Option<NaiveDate>,
) -> Result<Value, String> {
    let started = now_utc();
    let as_of = as_of.unwrap_or_else(today);
    let prev = ctx.gain.last_sync_run().map_err(e)?;
    let since = Some(
        prev.as_ref()
            .and_then(|r| date_of_ts(&r.finished))
            .filter(|d| *d <= as_of)
            .unwrap_or(as_of),
    );
    let source_label = path
        .map(|p| format!("sqlite:{p}"))
        .unwrap_or_else(|| "none".into());
    let mark_error = |ctx: &mut Ctx, err: &str| {
        let _ = ctx.gain.record_sync_run(&NewSyncRun {
            started: started.clone(),
            finished: now_utc(),
            source: source_label.clone(),
            accounts: 0,
            rows_read: 0,
            wrote: false,
            alerts: 0,
            status: "error".into(),
            error: Some(err.to_string()),
        });
    };

    let pull = match (path, query) {
        (Some(p), Some(q)) => match usage_pull(ctx, source.unwrap_or("sqlite"), p, q, dry_run) {
            Ok(v) => Some(v),
            Err(err) => {
                mark_error(ctx, &err);
                return Err(err);
            }
        },
        _ => None,
    };
    let health = match health_all(&ctx.gain, as_of) {
        Ok(v) => v,
        Err(err) => {
            mark_error(ctx, &err);
            return Err(err);
        }
    };
    let al = match alerts(&ctx.gain, as_of, since, 60) {
        Ok(v) => v,
        Err(err) => {
            mark_error(ctx, &err);
            return Err(err);
        }
    };
    let rows_read = pull
        .as_ref()
        .and_then(|p| p["source"]["rows_read"].as_i64())
        .unwrap_or(0);
    let accounts = health["accounts"].as_i64().unwrap_or(0);
    let alert_count = al["count"].as_i64().unwrap_or(0);
    let run = ctx
        .gain
        .record_sync_run(&NewSyncRun {
            started,
            finished: now_utc(),
            source: source_label,
            accounts,
            rows_read,
            wrote: pull.is_some() && !dry_run,
            alerts: alert_count,
            status: "ok".into(),
            error: None,
        })
        .map_err(e)?;
    let mut out = json!({
        "as_of": as_of,
        "run": to_json(&run)?,
        "pull": pull,
        "health": {
            "accounts": accounts,
            "arr_at_risk": health["arr_at_risk"],
            "new_ctas": health["new_ctas"],
        },
        "alerts": al,
    });
    if let Some(path) = report {
        let p = ctx.confinement.check(path, true)?;
        std::fs::write(&p, digest(as_of, &out).as_bytes())
            .map_err(|x| format!("{}: {x}", p.display()))?;
        out["report"] = json!(p.display().to_string());
    }
    Ok(out)
}

fn digest(as_of: NaiveDate, out: &Value) -> String {
    let run = &out["run"];
    let al = &out["alerts"];
    let mut s = String::new();
    s.push_str(&format!("# GainCraft daily digest - {as_of}\n\n"));
    s.push_str(&format!(
        "Run: {} | source: {} | {} accounts | {} rows read | {} new CTAs | {} alerts\n\n",
        run["status"].as_str().unwrap_or("?"),
        run["source"].as_str().unwrap_or("?"),
        run["accounts"].as_i64().unwrap_or(0),
        run["rows_read"].as_i64().unwrap_or(0),
        out["health"]["new_ctas"].as_i64().unwrap_or(0),
        al["count"].as_i64().unwrap_or(0),
    ));
    let section = |s: &mut String, title: &str, rows: &[Value], line: &dyn Fn(&Value) -> String| {
        s.push_str(&format!("## {title} ({})\n\n", rows.len()));
        if rows.is_empty() {
            s.push_str("- none\n\n");
            return;
        }
        for r in rows {
            s.push_str("- ");
            s.push_str(&line(r));
            s.push('\n');
        }
        s.push('\n');
    };
    fn arr(v: &Value) -> &str {
        v.as_str().unwrap_or("-")
    }
    section(
        &mut s,
        "Health changes",
        al["changed"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        &|r| {
            format!(
                "{} ({}): {} -> {} ({} -> {}), ARR {}",
                r["name"].as_str().unwrap_or("?"),
                r["account"].as_str().unwrap_or("?"),
                arr(&r["from"]),
                arr(&r["to"]),
                r["score_from"].as_i64().unwrap_or(0),
                r["score_to"].as_i64().unwrap_or(0),
                arr(&r["arr"]),
            )
        },
    );
    section(
        &mut s,
        "New calls to action",
        al["new_ctas"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        &|r| {
            format!(
                "{} ({}): {} [{}]",
                r["name"].as_str().unwrap_or("?"),
                r["account"].as_str().unwrap_or("?"),
                r["title"].as_str().unwrap_or("?"),
                arr(&r["arr"]),
            )
        },
    );
    section(
        &mut s,
        "Renewals in 60 days",
        al["renewals"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        &|r| {
            format!(
                "{} ({}): {} {} ({} days), ARR {}",
                r["name"].as_str().unwrap_or("?"),
                r["account"].as_str().unwrap_or("?"),
                arr(&r["kind"]),
                arr(&r["date"]),
                r["days_left"].as_i64().unwrap_or(0),
                arr(&r["arr"]),
            )
        },
    );
    section(
        &mut s,
        "At risk and silent",
        al["silent"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        &|r| {
            format!(
                "{} ({}): score {}, {} days without activity, ARR {}",
                r["name"].as_str().unwrap_or("?"),
                r["account"].as_str().unwrap_or("?"),
                r["score"].as_i64().unwrap_or(0),
                r["days_since_active"].as_i64().unwrap_or(0),
                arr(&r["arr"]),
            )
        },
    );
    s
}

fn event_date(created: &str) -> NaiveDate {
    created
        .get(..10)
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .unwrap_or_else(today)
}

/// Turn events from other craft apps into account data.
pub fn inbox_drain(ctx: &mut Ctx) -> Result<Value, String> {
    let events = ctx.bus.pending(APP).map_err(|x| x.to_string())?;
    let (mut done, mut rejected) = (Vec::new(), Vec::new());
    for ev in events {
        let r = match ev.kind.as_str() {
            "contract.executed" => apply_contract(&ctx.gain, &ev.payload),
            "contract.amended" => apply_amendment(&ctx.gain, &ev.payload),
            "contract.terminated" => {
                apply_termination(&ctx.gain, &ev.payload, event_date(&ev.created))
            }
            "signal.negative" | "signal.positive" => apply_signal(
                &ctx.gain,
                &ev.source,
                &ev.id,
                &ev.payload,
                event_date(&ev.created),
                if ev.kind == "signal.positive" {
                    "positive"
                } else {
                    "negative"
                },
            ),
            other => Err(format!("gaincraft does not handle event kind {other}")),
        };
        match r {
            Ok(msg) => {
                ctx.bus.ack(APP, &ev.id).map_err(|x| x.to_string())?;
                done.push(json!({ "event": ev.id, "kind": ev.kind, "result": msg }));
            }
            Err(err) => {
                ctx.bus
                    .reject(APP, &ev.id, &err)
                    .map_err(|x| x.to_string())?;
                rejected.push(json!({ "event": ev.id, "kind": ev.kind, "error": err }));
            }
        }
    }
    Ok(json!({ "applied": done, "rejected": rejected }))
}

fn apply_contract(g: &Gain, p: &Value) -> Result<String, String> {
    let party = s(p, "counterparty")?;
    let contract = s(p, "contract_id")?;
    let existing = g
        .accounts(true)
        .map_err(e)?
        .into_iter()
        .find(|a| a.contract_id.as_deref() == Some(contract));
    let (a, created) = match existing.map(Ok).unwrap_or_else(|| g.account(party)) {
        Ok(a) => (a, false),
        Err(_) => (
            g.add_account(
                &NewAccount {
                    name: party.into(),
                    notes: format!("created from contract {contract}"),
                    ..Default::default()
                },
                true,
            )
            .map_err(e)?,
            true,
        ),
    };
    let d = |k: &str| p.get(k).and_then(Value::as_str).and_then(|t| date(t).ok());
    let patch = AccountPatch {
        arr_cents: p
            .get("annual_value_cents")
            .and_then(Value::as_i64)
            .map(Some),
        contract_id: Some(Some(contract.into())),
        term_end: Some(d("term_end")),
        last_notice: Some(d("last_notice")),
        renewal_months: p.get("renewal_months").and_then(Value::as_i64).map(Some),
        ..Default::default()
    };
    let u = g.update_account(&a.key, &patch).map_err(e)?;
    if created {
        // update_account clears auto_created; keep the flag for accounts the bus made.
        g.conn()
            .execute("UPDATE accounts SET auto_created = 1 WHERE id = ?1", [u.id])
            .map_err(|x| x.to_string())?;
    }
    Ok(format!(
        "{} {} ({contract})",
        if created { "created" } else { "updated" },
        u.key
    ))
}

fn account_by_contract(g: &Gain, contract: &str) -> Result<Option<Account>, String> {
    Ok(g.accounts(true)
        .map_err(e)?
        .into_iter()
        .find(|a| a.contract_id.as_deref() == Some(contract)))
}

/// `contract.amended`: only the fields present in the payload change.
/// An amendment for a contract gaincraft has never seen is rejected (send
/// `contract.executed` first), so the dock shows it and it can be retried.
fn apply_amendment(g: &Gain, p: &Value) -> Result<String, String> {
    let contract = s(p, "contract_id")?;
    let a = account_by_contract(g, contract)?.ok_or_else(|| {
        format!("amendment for unknown contract {contract}: apply its contract.executed first")
    })?;
    let day = |k: &str| -> Result<Option<Option<NaiveDate>>, String> {
        match p.get(k) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(v) => str_of(v).and_then(|t| date(&t)).map(|d| Some(Some(d))),
        }
    };
    let patch = AccountPatch {
        arr_cents: match p.get("annual_value_cents") {
            None => None,
            Some(v) => {
                Some(Some(v.as_i64().filter(|c| *c >= 0).ok_or(
                    "annual_value_cents must be a non-negative integer",
                )?))
            }
        },
        term_end: day("term_end")?,
        last_notice: day("last_notice")?,
        renewal_months: p.get("renewal_months").map(Value::as_i64),
        ..Default::default()
    };
    let auto = a.auto_created;
    let u = g.update_account(&a.key, &patch).map_err(e)?;
    if auto {
        g.conn()
            .execute("UPDATE accounts SET auto_created = 1 WHERE id = ?1", [u.id])
            .map_err(|x| x.to_string())?;
    }
    Ok(format!("amended {} ({contract})", u.key))
}

/// `contract.terminated`: the contract ends on `effective` (or `term_end`).
/// No renewal deadline any more; open renewal CTAs are dismissed; a
/// `termination` CTA asks for an exit call. The account stays active until a
/// person retires it (it is still a customer until the term ends).
fn apply_termination(g: &Gain, p: &Value, today: NaiveDate) -> Result<String, String> {
    let contract = s(p, "contract_id")?;
    let a = account_by_contract(g, contract)?
        .ok_or_else(|| format!("termination for unknown contract {contract}"))?;
    let effective = opt_s(p, "effective")
        .or_else(|| opt_s(p, "term_end"))
        .map(|d| date(&d))
        .transpose()?;
    let reason = opt_s(p, "reason").unwrap_or_default();
    let u = g
        .update_account(
            &a.key,
            &AccountPatch {
                term_end: effective.map(Some),
                last_notice: Some(None),
                renewal_months: Some(None),
                ..Default::default()
            },
        )
        .map_err(e)?;
    for c in g
        .ctas(Some(a.id), false)
        .map_err(e)?
        .iter()
        .filter(|c| c.kind == "renewal")
    {
        g.resolve_cta(
            c.id,
            true,
            &format!("contract {contract} terminated"),
            "contractcraft",
        )
        .map_err(e)?;
    }
    let eff = de_date(effective);
    g.raise_cta_with(
        a.id,
        "termination",
        contract,
        &format!(
            "Contract {contract} terminated{}: exit call, retire after the term ends",
            if eff.is_empty() {
                String::new()
            } else {
                format!(" (effective {eff})")
            }
        ),
        &json!({ "contract": contract, "effective": eff, "reason": reason }).to_string(),
        Some(today + Duration::days(3)),
        today,
    )
    .map_err(e)?;
    Ok(format!("terminated {} ({contract})", u.key))
}

fn apply_signal(
    g: &Gain,
    source: &str,
    event_id: &str,
    p: &Value,
    at: NaiveDate,
    polarity: &str,
) -> Result<String, String> {
    let domain = opt_s(p, "account")
        .or_else(|| opt_s(p, "email").and_then(|m| m.split('@').nth(1).map(str::to_string)))
        .ok_or("signal without account or email")?;
    let (a, created) = match g.account(&domain) {
        Ok(a) => (a, false),
        Err(_) => (
            g.add_account(
                &NewAccount {
                    key: Some(domain.clone()),
                    name: domain.clone(),
                    domain: Some(domain.clone()),
                    notes: format!("created from a {source} signal"),
                    ..Default::default()
                },
                true,
            )
            .map_err(e)?,
            true,
        ),
    };
    let score = p
        .get("score")
        .and_then(Value::as_i64)
        .unwrap_or(3)
        .clamp(1, 10);
    let reference = opt_s(p, "ticket_id").or_else(|| opt_s(p, "response_id"));
    let note = format!(
        "{} {}",
        opt_s(p, "subject").unwrap_or_default(),
        p.get("hits").map(|h| h.to_string()).unwrap_or_default()
    )
    .trim()
    .to_string();
    let added = g
        .add_signal(
            a.id,
            &NewSignal {
                source: source.into(),
                polarity: polarity.into(),
                score,
                reference,
                note,
                at,
                event_id: Some(event_id.into()),
            },
        )
        .map_err(e)?;
    Ok(format!(
        "{} {polarity} signal on {}{}",
        if added.is_some() {
            "recorded"
        } else {
            "duplicate"
        },
        a.key,
        if created { " (account created)" } else { "" }
    ))
}

/// Labelled demo accounts whose names and domains match the other demos
/// (contractcraft, deskcraft), so their events land on the right account.
pub fn demo_seed(g: &Gain) -> Result<Value, String> {
    if !g.accounts(true).map_err(e)?.is_empty() {
        return Err("demo seed only runs on an empty database".into());
    }
    let as_of = NaiveDate::from_ymd_opt(2026, 11, 9).ok_or("bad demo date")?;
    g.set_meta("demo", "1").map_err(e)?;
    let mk = |name: &str, domain: &str, seats: i64, arr: i64, owner: &str| {
        g.add_account(
            &NewAccount {
                key: None,
                name: name.into(),
                domain: Some(domain.into()),
                owner: Some(owner.into()),
                arr_cents: Some(arr),
                seats: Some(seats),
                notes: "DEMO".into(),
            },
            false,
        )
        .map_err(e)
    };
    let software = mk(
        "Mock Software GmbH (DEMO)",
        "mock-software.example",
        10,
        960_000,
        "Anna Mockova",
    )?;
    let bird = mk(
        "Mockingbird Inc. (DEMO)",
        "mockingbird.example",
        25,
        2_400_000,
        "Joe Mocky",
    )?;
    let mockinger = mk(
        "Hannes Mockinger e.U. (DEMO)",
        "mockinger.example",
        5,
        540_000,
        "Joe Mocky",
    )?;
    g.update_account(
        &software.key,
        &AccountPatch {
            contract_id: Some(Some("C-2026-001".into())),
            term_end: Some(NaiveDate::from_ymd_opt(2027, 2, 28)),
            last_notice: Some(NaiveDate::from_ymd_opt(2026, 11, 30)),
            renewal_months: Some(Some(12)),
            ..Default::default()
        },
    )
    .map_err(e)?;
    let gen = |id: i64, f: &dyn Fn(i64, bool) -> i64| -> Result<(), String> {
        let days: Vec<Day> = (0..90)
            .map(|i| {
                let date = as_of - Duration::days(i);
                let weekday = !matches!(date.weekday(), Weekday::Sat | Weekday::Sun);
                let users = f(i, weekday);
                Day {
                    date,
                    active_users: users,
                    sessions: users * 3,
                    key_actions: users * 11,
                }
            })
            .collect();
        g.upsert_days(id, &days).map_err(e).map(|_| ())
    };
    // Healthy and steady.
    gen(software.id, &|i, wd| if wd { 7 + i % 3 } else { 1 })?;
    // Was busy, dropped hard two weeks ago.
    gen(bird.id, &|i, wd| {
        if !wd || i < 4 {
            0
        } else if i < 14 {
            3
        } else {
            18 + i % 4
        }
    })?;
    // Small, fading, last seen 20 days ago.
    gen(mockinger.id, &|i, wd| if wd && i >= 20 { 2 } else { 0 })?;
    g.add_signal(
        mockinger.id,
        &NewSignal {
            source: "deskcraft".into(),
            polarity: "negative".into(),
            score: 5,
            reference: Some("D-3".into()),
            note: "Webshop schon wieder down (DEMO)".into(),
            at: NaiveDate::from_ymd_opt(2026, 11, 6).ok_or("bad date")?,
            event_id: None,
        },
    )
    .map_err(e)?;
    let all = health_all(g, as_of)?;
    Ok(
        json!({ "demo": true, "as_of": as_of, "scores": all["rows"].as_array().map(|r| r.iter().map(|x| json!({ "account": x["account"], "score": x["score"], "band": x["band"] })).collect::<Vec<_>>()), "new_ctas": all["new_ctas"] }),
    )
}

/// MCP tool catalog.
pub fn tools() -> Vec<Value> {
    let acc = json!({ "type": "string", "description": "Account key, domain or name" });
    let date =
        json!({ "type": "string", "description": "DD.MM.YYYY or YYYY-MM-DD (default: today)" });
    let money =
        json!({ "type": ["string", "null"], "description": "DE amount, e.g. \"9.600,00\"" });
    vec![
        tool("gain_status", "Active accounts, ARR, telemetry days, open CTAs, signals in the last 90 days, health weights.", json!({ "as_of": date }), &[]),
        tool("gain_account_add", "Add a customer account. Domain links signals from deskcraft (mail domain).", json!({ "name": {"type":"string"}, "key": {"type":"string"}, "domain": {"type":"string"}, "owner": {"type":"string"}, "arr": money, "seats": {"type":"integer"}, "notes": {"type":"string"} }), &["name"]),
        tool("gain_account_list", "Accounts with ARR, seats, contract dates.", json!({ "include_inactive": {"type":"boolean"} }), &[]),
        tool("gain_account_show", "One account with current health and components, score trend, signals, open CTAs and triage.", json!({ "account": acc, "as_of": date }), &["account"]),
        tool("gain_account_update", "Change account fields; null clears. active=false retires an account.", json!({ "account": acc, "name": {"type":"string"}, "domain": {"type":["string","null"]}, "owner": {"type":["string","null"]}, "arr": money, "seats": {"type":["integer","null"]}, "contract_id": {"type":["string","null"]}, "term_end": {"type":["string","null"]}, "last_notice": {"type":["string","null"]}, "renewal_months": {"type":["integer","null"]}, "active": {"type":"boolean"}, "retired_on": {"type":["string","null"], "description": "churn date (default: today when active turns false)"}, "notes": {"type":"string"} }), &["account"]),
        tool("gain_telemetry_import", "Import usage aggregates per account and day: account;date;active_users[;sessions;key_actions]. Per-person columns are refused (ArbVG / DSGVO). Re-importing a day replaces it.", json!({ "path": {"type":"string"}, "dry_run": {"type":"boolean"} }), &["path"]),
        tool("gain_usage_pull", "Read aggregate usage straight from a SQLite product database (opened read-only, one SELECT/WITH). The result must return account, date, active_users[, sessions, key_actions]; per-person or extra columns are refused. Same validation, account resolution and per-day idempotency as gain_telemetry_import.", json!({ "source": {"type":"string","enum":["sqlite"],"description":"source (default sqlite)"}, "path": {"type":"string","description":"product SQLite file, opened read-only"}, "query": {"type":"string","description":"one SELECT/WITH returning account, date, active_users[, sessions, key_actions]"}, "dry_run": {"type":"boolean"} }), &["path", "query"]),
        tool("gain_tick", "One scheduled pass: optionally pull usage (path+query, SQLite read-only), recompute health for every account, collect alerts (band changes, new CTAs, renewals, silence), record the run and optionally write a Markdown digest. Omit path and query for a health-only tick.", json!({ "source": {"type":"string","enum":["sqlite"]}, "path": {"type":"string","description":"product SQLite file (with query)"}, "query": {"type":"string","description":"one SELECT/WITH returning the aggregate schema"}, "dry_run": {"type":"boolean"}, "report": {"type":"string","description":"write the Markdown digest here"}, "as_of": date }), &[]),
        tool("gain_alerts", "What needs attention now: band changes since the previous snapshot, CTAs created on or after `since`, renewals inside `within_days`, and at-risk accounts silent for 14+ days.", json!({ "as_of": date, "since": date, "within_days": {"type":"integer","description":"7-3650, default 60"} }), &[]),
        tool("gain_signal_add", "Record a customer signal by hand (e.g. after a call): negative or positive, score 1-10, with a note.", json!({ "account": acc, "polarity": {"type":"string","enum":["negative","positive"]}, "score": {"type":"integer"}, "note": {"type":"string"}, "reference": {"type":"string"}, "date": date }), &["account", "score", "note"]),
        tool("gain_health", "Health score 0-100 of one account with components (adoption, recency, sentiment, modifier); stores a snapshot and raises CTAs.", json!({ "account": acc, "as_of": date }), &["account"]),
        tool("gain_health_all", "Health of all active accounts, worst first, ARR at risk (yellow + red); stores snapshots and raises CTAs.", json!({ "as_of": date }), &[]),
        tool("gain_triage_set", "Set an advisory modifier (0.8-1.2, with a reason, expires after N days) from an LLM or human read of the account; clear=true removes it.", json!({ "account": acc, "modifier": {"type":"number"}, "reason": {"type":"string"}, "days": {"type":"integer"}, "clear": {"type":"boolean"} }), &["account"]),
        tool("gain_cta_list", "Open calls to action (low health, adoption drop, unused seats, renewal, negative signal), due first.", json!({ "account": acc, "include_closed": {"type":"boolean"} }), &[]),
        tool("gain_cta_resolve", "Resolve (or dismiss) a CTA with a note saying what was done.", json!({ "id": {"type":"integer"}, "note": {"type":"string"}, "dismiss": {"type":"boolean"} }), &["id", "note"]),
        tool("gain_deadlines", "Notice deadlines and term ends of accounts within N days (default 90).", json!({ "within_days": {"type":"integer"}, "as_of": date }), &[]),
        tool("gain_weights", "Show or set the health weights (adoption, recency, sentiment; must sum to 1).", json!({ "set": {"type":"object","properties":{"adoption":{"type":"number"},"recency":{"type":"number"},"sentiment":{"type":"number"}}} }), &[]),
        tool("gain_inbox_drain", "Apply events from other craft apps: contractcraft contract.executed / contract.amended / contract.terminated (ARR, contract dates, exit CTA) and deskcraft/formcraft signal.negative / signal.positive (sentiment). Unknown accounts are created and flagged.", json!({}), &[]),
        tool("gain_demo_seed", "Fill an EMPTY database with three labelled demo accounts (healthy, dropping, fading) whose names match the other craft demos.", json!({}), &[]),
        tool("gain_cta_add", "Create a call to action by hand (kind manual), e.g. a follow-up agreed in a call.", json!({ "account": acc, "title": {"type":"string"}, "due": date, "note": {"type":"string"} }), &["account", "title"]),
        tool("gain_export", "Write the health report of all active accounts (worst first) as CSV (; and DE numbers) or Markdown. Format from the extension unless given.", json!({ "path": {"type":"string"}, "format": {"type":"string","enum":["csv","md"]}, "as_of": date, "overwrite": {"type":"boolean"} }), &["path"]),
        tool("gain_backup", "Copy the whole database to a new file (consistent snapshot). Never overwrites.", json!({ "path": {"type":"string"} }), &["path"]),
        tool("gain_churn_audit", "Churn-risk audit: accounts at risk now with reasons (band, 30-day drop, notice soon, sentiment), and a backtest of retired accounts: how the score ranked them lead_days before they left.", json!({ "as_of": date, "lead_days": {"type":"integer", "description": "7-365, default 90"} }), &[]),
    ]
}

#[cfg(test)]
mod tests;
