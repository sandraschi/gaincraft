//! gaincraft-core: the command bus shared by CLI, MCP and the desktop app.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod health;

use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};
use craft_biz::at::parse_date;
use craft_biz::bus::Bus;
pub use craft_biz::confine::Confinement;
use craft_biz::mcp::tool;
use craft_biz::Money;
use gaincraft_store::{Account, AccountPatch, Day, Gain, GainError, NewAccount, NewSignal};
use health::{Health, Weights};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const APP: &str = "gaincraft";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_IMPORT_BYTES: u64 = 20 * 1024 * 1024;
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
                },
            },
            "gain_telemetry_import" => Command::TelemetryImport {
                path: s(a, "path")?.into(),
                dry_run: flag(a, "dry_run"),
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
    }
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
    let mut raise = |kind: &str,
                     reference: &str,
                     title: String,
                     due: Option<NaiveDate>|
     -> Result<(), String> {
        if g.raise_cta(a.id, kind, reference, &title, due, as_of)
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
            Some(as_of + Duration::days(3)),
        )?;
    }
    if let Some(t) = h.trend_14d.filter(|t| *t < 0.6) {
        raise(
            "adoption_drop",
            "",
            format!(
                "Usage down {:.0}% vs. the two weeks before",
                (1.0 - t) * 100.0
            ),
            Some(as_of + Duration::days(7)),
        )?;
    }
    if let Some(seats) = a.seats.filter(|s| h.avg_active_30d < 0.5 * *s as f64) {
        raise(
            "seats_unused",
            "",
            format!(
                "Only {:.1} of {seats} seats active on average",
                h.avg_active_30d
            ),
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
    // Resolve every account before writing anything.
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
    Ok(json!({ "dry_run": dry_run, "accounts": summary.len(), "rows": summary }))
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
            "signal.negative" => apply_signal(
                &ctx.gain,
                &ev.source,
                &ev.id,
                &ev.payload,
                event_date(&ev.created),
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

fn apply_signal(
    g: &Gain,
    source: &str,
    event_id: &str,
    p: &Value,
    at: NaiveDate,
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
                polarity: "negative".into(),
                score,
                reference,
                note,
                at,
                event_id: Some(event_id.into()),
            },
        )
        .map_err(e)?;
    Ok(format!(
        "{} signal on {}{}",
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
        tool("gain_account_update", "Change account fields; null clears. active=false retires an account.", json!({ "account": acc, "name": {"type":"string"}, "domain": {"type":["string","null"]}, "owner": {"type":["string","null"]}, "arr": money, "seats": {"type":["integer","null"]}, "contract_id": {"type":["string","null"]}, "term_end": {"type":["string","null"]}, "last_notice": {"type":["string","null"]}, "renewal_months": {"type":["integer","null"]}, "active": {"type":"boolean"}, "notes": {"type":"string"} }), &["account"]),
        tool("gain_telemetry_import", "Import usage aggregates per account and day: account;date;active_users[;sessions;key_actions]. Per-person columns are refused (ArbVG / DSGVO). Re-importing a day replaces it.", json!({ "path": {"type":"string"}, "dry_run": {"type":"boolean"} }), &["path"]),
        tool("gain_signal_add", "Record a customer signal by hand (e.g. after a call): negative or positive, score 1-10, with a note.", json!({ "account": acc, "polarity": {"type":"string","enum":["negative","positive"]}, "score": {"type":"integer"}, "note": {"type":"string"}, "reference": {"type":"string"}, "date": date }), &["account", "score", "note"]),
        tool("gain_health", "Health score 0-100 of one account with components (adoption, recency, sentiment, modifier); stores a snapshot and raises CTAs.", json!({ "account": acc, "as_of": date }), &["account"]),
        tool("gain_health_all", "Health of all active accounts, worst first, ARR at risk (yellow + red); stores snapshots and raises CTAs.", json!({ "as_of": date }), &[]),
        tool("gain_triage_set", "Set an advisory modifier (0.8-1.2, with a reason, expires after N days) from an LLM or human read of the account; clear=true removes it.", json!({ "account": acc, "modifier": {"type":"number"}, "reason": {"type":"string"}, "days": {"type":"integer"}, "clear": {"type":"boolean"} }), &["account"]),
        tool("gain_cta_list", "Open calls to action (low health, adoption drop, unused seats, renewal, negative signal), due first.", json!({ "account": acc, "include_closed": {"type":"boolean"} }), &[]),
        tool("gain_cta_resolve", "Resolve (or dismiss) a CTA with a note saying what was done.", json!({ "id": {"type":"integer"}, "note": {"type":"string"}, "dismiss": {"type":"boolean"} }), &["id", "note"]),
        tool("gain_deadlines", "Notice deadlines and term ends of accounts within N days (default 90).", json!({ "within_days": {"type":"integer"}, "as_of": date }), &[]),
        tool("gain_weights", "Show or set the health weights (adoption, recency, sentiment; must sum to 1).", json!({ "set": {"type":"object","properties":{"adoption":{"type":"number"},"recency":{"type":"number"},"sentiment":{"type":"number"}}} }), &[]),
        tool("gain_inbox_drain", "Apply events from other craft apps: contractcraft contract.executed (ARR, contract dates) and deskcraft/formcraft signal.negative (sentiment). Unknown accounts are created and flagged.", json!({}), &[]),
        tool("gain_demo_seed", "Fill an EMPTY database with three labelled demo accounts (healthy, dropping, fading) whose names match the other craft demos.", json!({}), &[]),
    ]
}

#[cfg(test)]
mod tests;
