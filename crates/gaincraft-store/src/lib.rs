//! gaincraft-store: customer accounts and their health inputs.
//!
//! Invariants:
//! - Telemetry is stored per account and day only. There is no column that
//!   could hold a person (ArbVG section 96a / DSGVO data minimisation): the
//!   safe path is the only path.
//! - A signal from another app is stored once (its event id is unique).
//! - A call to action (CTA) of one kind and reference is open at most once per
//!   account; resolving it needs a note.
//! - LLM triage can only nudge a score: modifier 0.8-1.2, with a reason and an
//!   expiry.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use std::path::Path;

/// 2: `accounts.retired_on` (churn date for the backtest), `ctas.params`
/// (structured values so the window can word CTA titles in any language).
/// 3: `sync_runs` (history of scheduled usage pulls and health passes).
pub const SCHEMA_VERSION: i64 = 3;

#[derive(Debug, thiserror::Error)]
pub enum GainError {
    #[error("database: {0}")]
    Db(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
}

impl From<rusqlite::Error> for GainError {
    fn from(e: rusqlite::Error) -> Self {
        GainError::Db(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, GainError>;

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub domain: Option<String>,
    pub owner: Option<String>,
    pub arr_cents: Option<i64>,
    pub seats: Option<i64>,
    pub contract_id: Option<String>,
    pub term_end: Option<NaiveDate>,
    pub last_notice: Option<NaiveDate>,
    pub renewal_months: Option<i64>,
    pub auto_created: bool,
    pub active: bool,
    pub notes: String,
    /// Day the account was retired (churned). Set when `active` turns false.
    pub retired_on: Option<NaiveDate>,
}

#[derive(Debug, Clone, Default)]
pub struct NewAccount {
    pub key: Option<String>,
    pub name: String,
    pub domain: Option<String>,
    pub owner: Option<String>,
    pub arr_cents: Option<i64>,
    pub seats: Option<i64>,
    pub notes: String,
}

/// Outer Option = unchanged, inner None = clear.
#[derive(Debug, Clone, Default)]
pub struct AccountPatch {
    pub name: Option<String>,
    pub domain: Option<Option<String>>,
    pub owner: Option<Option<String>>,
    pub arr_cents: Option<Option<i64>>,
    pub seats: Option<Option<i64>>,
    pub contract_id: Option<Option<String>>,
    pub term_end: Option<Option<NaiveDate>>,
    pub last_notice: Option<Option<NaiveDate>>,
    pub renewal_months: Option<Option<i64>>,
    pub active: Option<bool>,
    pub notes: Option<String>,
    /// Churn date. Unset: today when `active` turns false, cleared when it turns true.
    pub retired_on: Option<Option<NaiveDate>>,
}

/// One account-day of usage. Deliberately no user field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Day {
    pub date: NaiveDate,
    pub active_users: i64,
    pub sessions: i64,
    pub key_actions: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Signal {
    pub id: i64,
    pub account: String,
    pub source: String,
    pub polarity: String,
    pub score: i64,
    pub reference: Option<String>,
    pub note: String,
    pub at: NaiveDate,
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct NewSignal {
    pub source: String,
    /// "negative" or "positive"
    pub polarity: String,
    /// 1-10
    pub score: i64,
    pub reference: Option<String>,
    pub note: String,
    pub at: NaiveDate,
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Triage {
    pub modifier: f64,
    pub reason: String,
    pub source: String,
    pub set_on: NaiveDate,
    pub expires: NaiveDate,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cta {
    pub id: i64,
    pub account: String,
    pub kind: String,
    pub reference: String,
    pub title: String,
    pub due: Option<NaiveDate>,
    pub status: String,
    pub created_on: NaiveDate,
    pub last_seen: NaiveDate,
    pub resolution: Option<String>,
    pub resolved_by: Option<String>,
    /// JSON object with the values behind the title (score, percent, date...).
    pub params: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub date: NaiveDate,
    pub score: i64,
    pub band: String,
}

/// One scheduled tick: what was pulled, what was computed, what was raised.
#[derive(Debug, Clone, Serialize)]
pub struct SyncRun {
    pub id: i64,
    pub started: String,
    pub finished: String,
    pub source: String,
    pub accounts: i64,
    pub rows_read: i64,
    pub wrote: bool,
    pub alerts: i64,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewSyncRun {
    pub started: String,
    pub finished: String,
    pub source: String,
    pub accounts: i64,
    pub rows_read: i64,
    pub wrote: bool,
    pub alerts: i64,
    /// `ok` or `error`.
    pub status: String,
    pub error: Option<String>,
}

const SYNC_COLS: &str =
    "id, started, finished, source, accounts, rows_read, wrote, alerts, status, error";

fn sync_row(r: &Row) -> rusqlite::Result<SyncRun> {
    Ok(SyncRun {
        id: r.get(0)?,
        started: r.get(1)?,
        finished: r.get(2)?,
        source: r.get(3)?,
        accounts: r.get(4)?,
        rows_read: r.get(5)?,
        wrote: r.get::<_, i64>(6)? != 0,
        alerts: r.get(7)?,
        status: r.get(8)?,
        error: r.get(9)?,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub accounts_active: i64,
    pub arr_active_cents: i64,
    pub telemetry_days: i64,
    pub ctas_open: i64,
    pub signals_90d: i64,
    pub demo: bool,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    domain TEXT UNIQUE,
    owner TEXT,
    arr_cents INTEGER CHECK (arr_cents IS NULL OR arr_cents >= 0),
    seats INTEGER CHECK (seats IS NULL OR seats > 0),
    contract_id TEXT,
    term_end TEXT,
    last_notice TEXT,
    renewal_months INTEGER,
    auto_created INTEGER NOT NULL DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 1,
    notes TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS telemetry_daily (
    account_id INTEGER NOT NULL REFERENCES accounts(id),
    date TEXT NOT NULL,
    active_users INTEGER NOT NULL CHECK (active_users >= 0),
    sessions INTEGER NOT NULL DEFAULT 0 CHECK (sessions >= 0),
    key_actions INTEGER NOT NULL DEFAULT 0 CHECK (key_actions >= 0),
    PRIMARY KEY (account_id, date)
);
CREATE TABLE IF NOT EXISTS signals (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id),
    source TEXT NOT NULL,
    polarity TEXT NOT NULL CHECK (polarity IN ('negative', 'positive')),
    score INTEGER NOT NULL CHECK (score BETWEEN 1 AND 10),
    reference TEXT,
    note TEXT NOT NULL DEFAULT '',
    at TEXT NOT NULL,
    event_id TEXT UNIQUE
);
CREATE TABLE IF NOT EXISTS triage (
    account_id INTEGER PRIMARY KEY REFERENCES accounts(id),
    modifier REAL NOT NULL CHECK (modifier >= 0.8 AND modifier <= 1.2),
    reason TEXT NOT NULL,
    source TEXT NOT NULL,
    set_on TEXT NOT NULL,
    expires TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS ctas (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id),
    kind TEXT NOT NULL,
    reference TEXT NOT NULL DEFAULT '',
    title TEXT NOT NULL,
    due TEXT,
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved', 'dismissed')),
    created_on TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    resolution TEXT,
    resolved_by TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_cta_one_open ON ctas(account_id, kind, reference) WHERE status = 'open';
CREATE TABLE IF NOT EXISTS snapshots (
    account_id INTEGER NOT NULL REFERENCES accounts(id),
    date TEXT NOT NULL,
    score INTEGER NOT NULL,
    band TEXT NOT NULL,
    components TEXT NOT NULL,
    PRIMARY KEY (account_id, date)
);
CREATE TABLE IF NOT EXISTS sync_runs (
    id INTEGER PRIMARY KEY,
    started TEXT NOT NULL,
    finished TEXT NOT NULL,
    source TEXT NOT NULL,
    accounts INTEGER NOT NULL DEFAULT 0,
    rows_read INTEGER NOT NULL DEFAULT 0,
    wrote INTEGER NOT NULL DEFAULT 0,
    alerts INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL CHECK (status IN ('ok', 'error')),
    error TEXT
);
";

const ACC_COLS: &str = "id, key, name, domain, owner, arr_cents, seats, contract_id, term_end, last_notice, renewal_months, auto_created, active, notes, retired_on";

/// Schema steps after version 1. Each runs once, in order, in one transaction.
const MIGRATIONS: &[(i64, &str)] = &[
    (
        2,
        "ALTER TABLE accounts ADD COLUMN retired_on TEXT;
         ALTER TABLE ctas ADD COLUMN params TEXT NOT NULL DEFAULT '{}';",
    ),
    (
        3,
        "CREATE TABLE IF NOT EXISTS sync_runs (
             id INTEGER PRIMARY KEY,
             started TEXT NOT NULL,
             finished TEXT NOT NULL,
             source TEXT NOT NULL,
             accounts INTEGER NOT NULL DEFAULT 0,
             rows_read INTEGER NOT NULL DEFAULT 0,
             wrote INTEGER NOT NULL DEFAULT 0,
             alerts INTEGER NOT NULL DEFAULT 0,
             status TEXT NOT NULL CHECK (status IN ('ok', 'error')),
             error TEXT
         );",
    ),
];

fn acc_row(r: &Row) -> rusqlite::Result<Account> {
    Ok(Account {
        id: r.get(0)?,
        key: r.get(1)?,
        name: r.get(2)?,
        domain: r.get(3)?,
        owner: r.get(4)?,
        arr_cents: r.get(5)?,
        seats: r.get(6)?,
        contract_id: r.get(7)?,
        term_end: r.get(8)?,
        last_notice: r.get(9)?,
        renewal_months: r.get(10)?,
        auto_created: r.get(11)?,
        active: r.get(12)?,
        notes: r.get(13)?,
        retired_on: r.get(14)?,
    })
}

/// `Mock Software GmbH` -> `mock-software-gmbh`.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().to_lowercase().chars() {
        let c = match c {
            'ä' => "ae".to_string(),
            'ö' => "oe".to_string(),
            'ü' => "ue".to_string(),
            'ß' => "ss".to_string(),
            c if c.is_ascii_alphanumeric() || c == '.' => c.to_string(),
            _ => "-".to_string(),
        };
        out.push_str(&c);
    }
    let parts: Vec<&str> = out.split('-').filter(|p| !p.is_empty()).collect();
    parts.join("-").chars().take(60).collect()
}

fn clean(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn clean_domain(v: Option<String>) -> Result<Option<String>> {
    match clean(v) {
        None => Ok(None),
        Some(d) => {
            let d = d.to_lowercase().trim_start_matches('@').to_string();
            if !d.contains('.') || d.contains(char::is_whitespace) || d.contains('@') {
                return Err(GainError::Invalid(format!(
                    "domain {d:?} (e.g. mock-software.example)"
                )));
            }
            Ok(Some(d))
        }
    }
}

pub struct Gain {
    conn: Connection,
}

impl Gain {
    pub fn open(path: &Path) -> Result<Gain> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| GainError::Db(e.to_string()))?;
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Gain> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Gain> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        conn.execute_batch(SCHEMA)?;
        let v: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let have: i64 = match v.as_deref() {
            None => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema_version', '1')",
                    [],
                )?;
                1
            }
            Some(x) => x
                .parse()
                .map_err(|_| GainError::Db(format!("schema_version {x:?} is not a number")))?,
        };
        if have > SCHEMA_VERSION {
            return Err(GainError::Db(format!(
                "database schema {have} is newer than this build ({SCHEMA_VERSION})"
            )));
        }
        for (to, sql) in MIGRATIONS.iter().filter(|(to, _)| *to > have) {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
                params![to.to_string()],
            )?;
            tx.commit()?;
        }
        Ok(Gain { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn is_demo(&self) -> Result<bool> {
        Ok(self.meta("demo")?.as_deref() == Some("1"))
    }

    // ---------- accounts ----------

    pub fn add_account(&self, n: &NewAccount, auto_created: bool) -> Result<Account> {
        let name = n.name.trim();
        if name.is_empty() || name.chars().count() > 200 {
            return Err(GainError::Invalid("name must be 1-200 characters".into()));
        }
        let key = slug(n.key.as_deref().unwrap_or(name));
        if key.is_empty() {
            return Err(GainError::Invalid(
                "account key would be empty; pass key".into(),
            ));
        }
        if self.account(&key).is_ok() {
            return Err(GainError::Conflict(format!("account {key} exists")));
        }
        let domain = clean_domain(n.domain.clone())?;
        if let Some(d) = &domain {
            if let Some(other) = self.account_by_domain(d)? {
                return Err(GainError::Conflict(format!(
                    "domain {d} belongs to {}",
                    other.key
                )));
            }
        }
        if n.arr_cents.is_some_and(|a| a < 0) || n.seats.is_some_and(|s| s <= 0) {
            return Err(GainError::Invalid(
                "ARR cannot be negative, seats must be positive".into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO accounts (key, name, domain, owner, arr_cents, seats, auto_created, notes, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![key, name, domain, clean(n.owner.clone()), n.arr_cents, n.seats, auto_created, n.notes.trim(), now()],
        )?;
        self.account(&key)
    }

    /// By key, domain or exact name (case-insensitive).
    pub fn account(&self, k: &str) -> Result<Account> {
        let t = k.trim();
        let by_key = self
            .conn
            .query_row(
                &format!("SELECT {ACC_COLS} FROM accounts WHERE key = ?1"),
                params![slug(t)],
                acc_row,
            )
            .optional()?;
        if let Some(a) = by_key {
            return Ok(a);
        }
        if let Some(a) = self.account_by_domain(&t.to_lowercase())? {
            return Ok(a);
        }
        self.conn
            .query_row(
                &format!("SELECT {ACC_COLS} FROM accounts WHERE lower(name) = lower(?1)"),
                params![t],
                acc_row,
            )
            .optional()?
            .ok_or_else(|| GainError::NotFound(format!("account {k}")))
    }

    pub fn account_by_domain(&self, domain: &str) -> Result<Option<Account>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ACC_COLS} FROM accounts WHERE domain = ?1"),
                params![domain.trim().to_lowercase()],
                acc_row,
            )
            .optional()?)
    }

    pub fn account_by_id(&self, id: i64) -> Result<Account> {
        self.conn
            .query_row(
                &format!("SELECT {ACC_COLS} FROM accounts WHERE id = ?1"),
                params![id],
                acc_row,
            )
            .optional()?
            .ok_or_else(|| GainError::NotFound(format!("account {id}")))
    }

    pub fn accounts(&self, include_inactive: bool) -> Result<Vec<Account>> {
        let sql = format!(
            "SELECT {ACC_COLS} FROM accounts {} ORDER BY name",
            if include_inactive {
                ""
            } else {
                "WHERE active = 1"
            }
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st
            .query_map([], acc_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn update_account(&self, k: &str, p: &AccountPatch) -> Result<Account> {
        let a = self.account(k)?;
        let name = p
            .name
            .as_deref()
            .map(str::trim)
            .unwrap_or(&a.name)
            .to_string();
        if name.is_empty() {
            return Err(GainError::Invalid("name cannot be empty".into()));
        }
        let domain = match &p.domain {
            Some(d) => clean_domain(d.clone())?,
            None => a.domain.clone(),
        };
        if let Some(d) = &domain {
            if let Some(other) = self.account_by_domain(d)?.filter(|o| o.id != a.id) {
                return Err(GainError::Conflict(format!(
                    "domain {d} belongs to {}",
                    other.key
                )));
            }
        }
        let arr = p.arr_cents.unwrap_or(a.arr_cents);
        let seats = p.seats.unwrap_or(a.seats);
        let active = p.active.unwrap_or(a.active);
        let retired_on = match p.retired_on {
            Some(d) => d,
            None if !active && a.active => Some(Utc::now().date_naive()),
            None if active => None,
            None => a.retired_on,
        };
        if active && retired_on.is_some() {
            return Err(GainError::Invalid(
                "an active account has no retirement date (pass active=false)".into(),
            ));
        }
        if arr.is_some_and(|v| v < 0) || seats.is_some_and(|v| v <= 0) {
            return Err(GainError::Invalid(
                "ARR cannot be negative, seats must be positive".into(),
            ));
        }
        self.conn.execute(
            "UPDATE accounts SET name = ?2, domain = ?3, owner = ?4, arr_cents = ?5, seats = ?6, contract_id = ?7, term_end = ?8, last_notice = ?9, renewal_months = ?10, active = ?11, notes = ?12, retired_on = ?13, auto_created = 0 WHERE id = ?1",
            params![
                a.id,
                name,
                domain,
                p.owner.clone().map(clean).unwrap_or(a.owner),
                arr,
                seats,
                p.contract_id.clone().map(clean).unwrap_or(a.contract_id),
                p.term_end.unwrap_or(a.term_end),
                p.last_notice.unwrap_or(a.last_notice),
                p.renewal_months.unwrap_or(a.renewal_months),
                active,
                p.notes.as_deref().map(str::trim).unwrap_or(&a.notes),
                retired_on
            ],
        )?;
        self.account_by_id(a.id)
    }

    // ---------- telemetry ----------

    /// Insert or replace account-days. Returns (inserted, replaced).
    pub fn upsert_days(&self, account_id: i64, days: &[Day]) -> Result<(usize, usize)> {
        let tx = self.conn.unchecked_transaction()?;
        let (mut ins, mut rep) = (0, 0);
        for d in days {
            if d.active_users < 0 || d.sessions < 0 || d.key_actions < 0 {
                return Err(GainError::Invalid(format!("negative value on {}", d.date)));
            }
            let exists: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM telemetry_daily WHERE account_id = ?1 AND date = ?2)",
                params![account_id, d.date],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO telemetry_daily (account_id, date, active_users, sessions, key_actions) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(account_id, date) DO UPDATE SET active_users = excluded.active_users, sessions = excluded.sessions, key_actions = excluded.key_actions",
                params![account_id, d.date, d.active_users, d.sessions, d.key_actions],
            )?;
            if exists {
                rep += 1;
            } else {
                ins += 1;
            }
        }
        tx.commit()?;
        Ok((ins, rep))
    }

    pub fn days(&self, account_id: i64, from: NaiveDate, to: NaiveDate) -> Result<Vec<Day>> {
        let mut st = self.conn.prepare("SELECT date, active_users, sessions, key_actions FROM telemetry_daily WHERE account_id = ?1 AND date >= ?2 AND date <= ?3 ORDER BY date")?;
        let rows = st
            .query_map(params![account_id, from, to], |r| {
                Ok(Day {
                    date: r.get(0)?,
                    active_users: r.get(1)?,
                    sessions: r.get(2)?,
                    key_actions: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn last_active_day(&self, account_id: i64, as_of: NaiveDate) -> Result<Option<NaiveDate>> {
        Ok(self.conn.query_row("SELECT MAX(date) FROM telemetry_daily WHERE account_id = ?1 AND active_users > 0 AND date <= ?2", params![account_id, as_of], |r| r.get(0))?)
    }

    // ---------- signals ----------

    pub fn add_signal(&self, account_id: i64, s: &NewSignal) -> Result<Option<Signal>> {
        if !matches!(s.polarity.as_str(), "negative" | "positive") {
            return Err(GainError::Invalid(
                "polarity is negative or positive".into(),
            ));
        }
        if !(1..=10).contains(&s.score) {
            return Err(GainError::Invalid("score must be 1-10".into()));
        }
        if let Some(e) = &s.event_id {
            let seen: bool = self.conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM signals WHERE event_id = ?1)",
                params![e],
                |r| r.get(0),
            )?;
            if seen {
                return Ok(None);
            }
        }
        // One signal per source document: a ticket or form response that is
        // re-emitted (or was entered by hand before) must not count twice.
        if let Some(r) = s.reference.as_deref().filter(|r| !r.trim().is_empty()) {
            let seen: bool = self.conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM signals WHERE account_id = ?1 AND source = ?2 AND reference = ?3 AND polarity = ?4)",
                params![account_id, s.source, r.trim(), s.polarity],
                |row| row.get(0),
            )?;
            if seen {
                return Ok(None);
            }
        }
        self.conn.execute(
            "INSERT INTO signals (account_id, source, polarity, score, reference, note, at, event_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![account_id, s.source, s.polarity, s.score, s.reference, s.note.trim(), s.at, s.event_id],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(self
            .signals(Some(account_id), None)?
            .into_iter()
            .find(|x| x.id == id))
    }

    pub fn signals(
        &self,
        account_id: Option<i64>,
        since: Option<NaiveDate>,
    ) -> Result<Vec<Signal>> {
        let mut st = self.conn.prepare(
            "SELECT s.id, a.key, s.source, s.polarity, s.score, s.reference, s.note, s.at, s.event_id FROM signals s JOIN accounts a ON a.id = s.account_id
             WHERE (?1 IS NULL OR s.account_id = ?1) AND (?2 IS NULL OR s.at >= ?2) ORDER BY s.at DESC, s.id DESC",
        )?;
        let rows = st
            .query_map(params![account_id, since], |r| {
                Ok(Signal {
                    id: r.get(0)?,
                    account: r.get(1)?,
                    source: r.get(2)?,
                    polarity: r.get(3)?,
                    score: r.get(4)?,
                    reference: r.get(5)?,
                    note: r.get(6)?,
                    at: r.get(7)?,
                    event_id: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ---------- triage ----------

    pub fn set_triage(
        &self,
        account_id: i64,
        modifier: f64,
        reason: &str,
        source: &str,
        set_on: NaiveDate,
        days: i64,
    ) -> Result<Triage> {
        if !modifier.is_finite() || !(0.8..=1.2).contains(&modifier) {
            return Err(GainError::Invalid(
                "modifier must be between 0.8 and 1.2".into(),
            ));
        }
        if reason.trim().chars().count() < 10 {
            return Err(GainError::Invalid(
                "a triage modifier needs a reason (at least 10 characters)".into(),
            ));
        }
        if !(1..=90).contains(&days) {
            return Err(GainError::Invalid("triage lasts 1-90 days".into()));
        }
        let expires = set_on + chrono::Duration::days(days);
        self.conn.execute(
            "INSERT INTO triage (account_id, modifier, reason, source, set_on, expires) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(account_id) DO UPDATE SET modifier = excluded.modifier, reason = excluded.reason, source = excluded.source, set_on = excluded.set_on, expires = excluded.expires",
            params![account_id, modifier, reason.trim(), source, set_on, expires],
        )?;
        Ok(Triage {
            modifier,
            reason: reason.trim().into(),
            source: source.into(),
            set_on,
            expires,
        })
    }

    /// Triage in force on `as_of`.
    pub fn triage(&self, account_id: i64, as_of: NaiveDate) -> Result<Option<Triage>> {
        Ok(self
            .conn
            .query_row(
                "SELECT modifier, reason, source, set_on, expires FROM triage WHERE account_id = ?1 AND set_on <= ?2 AND expires >= ?2",
                params![account_id, as_of],
                |r| Ok(Triage { modifier: r.get(0)?, reason: r.get(1)?, source: r.get(2)?, set_on: r.get(3)?, expires: r.get(4)? }),
            )
            .optional()?)
    }

    pub fn clear_triage(&self, account_id: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM triage WHERE account_id = ?1",
            params![account_id],
        )? > 0)
    }

    // ---------- CTAs ----------

    /// Open a CTA unless the same one is already open; refresh `last_seen` either way.
    /// Returns true when a new CTA was opened.
    pub fn raise_cta(
        &self,
        account_id: i64,
        kind: &str,
        reference: &str,
        title: &str,
        due: Option<NaiveDate>,
        today: NaiveDate,
    ) -> Result<bool> {
        self.raise_cta_with(account_id, kind, reference, title, "{}", due, today)
    }

    /// [`Gain::raise_cta`] with the structured values behind the title (JSON object).
    #[allow(clippy::too_many_arguments)]
    pub fn raise_cta_with(
        &self,
        account_id: i64,
        kind: &str,
        reference: &str,
        title: &str,
        params_json: &str,
        due: Option<NaiveDate>,
        today: NaiveDate,
    ) -> Result<bool> {
        if title.trim().is_empty() || title.chars().count() > 300 {
            return Err(GainError::Invalid(
                "CTA title must be 1-300 characters".into(),
            ));
        }
        let n = self.conn.execute("UPDATE ctas SET last_seen = ?4, title = ?5, due = ?6, params = ?7 WHERE account_id = ?1 AND kind = ?2 AND reference = ?3 AND status = 'open'", params![account_id, kind, reference, today, title.trim(), due, params_json])?;
        if n > 0 {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO ctas (account_id, kind, reference, title, due, status, created_on, last_seen, params) VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6, ?6, ?7)",
            params![account_id, kind, reference, title.trim(), due, today, params_json],
        )?;
        Ok(true)
    }

    /// Next free reference for a hand-made CTA (`manual-N`).
    pub fn next_manual_ref(&self) -> Result<String> {
        let n: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM ctas", [], |r| {
                r.get(0)
            })?;
        Ok(format!("manual-{n}"))
    }

    /// One CTA by id, open or closed.
    pub fn cta(&self, id: i64) -> Result<Cta> {
        self.ctas(None, true)?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| GainError::NotFound(format!("CTA {id}")))
    }

    /// Copy the whole database to `path` (a consistent snapshot, WAL included).
    /// Refuses to overwrite an existing file.
    pub fn backup_to(&self, path: &Path) -> Result<u64> {
        if path.exists() {
            return Err(GainError::Conflict(format!(
                "{} exists; choose a new file name",
                path.display()
            )));
        }
        self.conn
            .execute("VACUUM INTO ?1", params![path.to_string_lossy()])?;
        std::fs::metadata(path)
            .map(|m| m.len())
            .map_err(|e| GainError::Db(e.to_string()))
    }

    pub fn ctas(&self, account_id: Option<i64>, include_closed: bool) -> Result<Vec<Cta>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT c.id, a.key, c.kind, c.reference, c.title, c.due, c.status, c.created_on, c.last_seen, c.resolution, c.resolved_by, c.params FROM ctas c JOIN accounts a ON a.id = c.account_id
             WHERE (?1 IS NULL OR c.account_id = ?1) {} ORDER BY CASE WHEN c.due IS NULL THEN 1 ELSE 0 END, c.due, c.id",
            if include_closed { "" } else { "AND c.status = 'open'" }
        ))?;
        let rows = st
            .query_map(params![account_id], |r| {
                Ok(Cta {
                    id: r.get(0)?,
                    account: r.get(1)?,
                    kind: r.get(2)?,
                    reference: r.get(3)?,
                    title: r.get(4)?,
                    due: r.get(5)?,
                    status: r.get(6)?,
                    created_on: r.get(7)?,
                    last_seen: r.get(8)?,
                    resolution: r.get(9)?,
                    resolved_by: r.get(10)?,
                    params: r.get(11)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn resolve_cta(&self, id: i64, dismiss: bool, note: &str, by: &str) -> Result<Cta> {
        if note.trim().is_empty() {
            return Err(GainError::Invalid("say what was done (note)".into()));
        }
        let n = self.conn.execute(
            "UPDATE ctas SET status = ?2, resolution = ?3, resolved_by = ?4 WHERE id = ?1 AND status = 'open'",
            params![id, if dismiss { "dismissed" } else { "resolved" }, note.trim(), by],
        )?;
        if n == 0 {
            return Err(GainError::NotFound(format!("open CTA {id}")));
        }
        self.cta(id)
    }

    // ---------- snapshots ----------

    pub fn snapshot(
        &self,
        account_id: i64,
        date: NaiveDate,
        score: i64,
        band: &str,
        components: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO snapshots (account_id, date, score, band, components) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_id, date) DO UPDATE SET score = excluded.score, band = excluded.band, components = excluded.components",
            params![account_id, date, score, band, components],
        )?;
        Ok(())
    }

    pub fn snapshots(&self, account_id: i64, limit: usize) -> Result<Vec<Snapshot>> {
        let mut st = self.conn.prepare("SELECT date, score, band FROM snapshots WHERE account_id = ?1 ORDER BY date DESC LIMIT ?2")?;
        let rows = st
            .query_map(params![account_id, limit as i64], |r| {
                Ok(Snapshot {
                    date: r.get(0)?,
                    score: r.get(1)?,
                    band: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ---------- scheduled runs ----------

    pub fn record_sync_run(&self, n: &NewSyncRun) -> Result<SyncRun> {
        if n.status != "ok" && n.status != "error" {
            return Err(GainError::Invalid(format!(
                "sync status {:?} (ok or error)",
                n.status
            )));
        }
        self.conn.execute(
            "INSERT INTO sync_runs (started, finished, source, accounts, rows_read, wrote, alerts, status, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                n.started,
                n.finished,
                n.source,
                n.accounts,
                n.rows_read,
                n.wrote as i64,
                n.alerts,
                n.status,
                n.error
            ],
        )?;
        self.sync_run(self.conn.last_insert_rowid())?
            .ok_or_else(|| GainError::Db("sync run vanished after insert".into()))
    }

    pub fn sync_run(&self, id: i64) -> Result<Option<SyncRun>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {SYNC_COLS} FROM sync_runs WHERE id = ?1"),
                params![id],
                sync_row,
            )
            .optional()?)
    }

    pub fn last_sync_run(&self) -> Result<Option<SyncRun>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {SYNC_COLS} FROM sync_runs ORDER BY id DESC LIMIT 1"),
                [],
                sync_row,
            )
            .optional()?)
    }

    pub fn sync_runs(&self, limit: usize) -> Result<Vec<SyncRun>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {SYNC_COLS} FROM sync_runs ORDER BY id DESC LIMIT ?1"
        ))?;
        let rows = st
            .query_map(params![limit as i64], sync_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn stats(&self, as_of: NaiveDate) -> Result<Stats> {
        let since = as_of - chrono::Duration::days(90);
        Ok(Stats {
            accounts_active: self.conn.query_row(
                "SELECT COUNT(*) FROM accounts WHERE active = 1",
                [],
                |r| r.get(0),
            )?,
            arr_active_cents: self.conn.query_row(
                "SELECT COALESCE(SUM(arr_cents), 0) FROM accounts WHERE active = 1",
                [],
                |r| r.get(0),
            )?,
            telemetry_days: self.conn.query_row(
                "SELECT COUNT(*) FROM telemetry_daily",
                [],
                |r| r.get(0),
            )?,
            ctas_open: self.conn.query_row(
                "SELECT COUNT(*) FROM ctas WHERE status = 'open'",
                [],
                |r| r.get(0),
            )?,
            signals_90d: self.conn.query_row(
                "SELECT COUNT(*) FROM signals WHERE at >= ?1",
                params![since],
                |r| r.get(0),
            )?,
            demo: self.is_demo()?,
        })
    }
}

#[cfg(test)]
mod tests;
