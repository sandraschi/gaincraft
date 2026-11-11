//! GainCraft desktop window: customer health in the layout of a customer
//! success platform (Overview, Accounts with a Customer 360 panel, CTA
//! cockpit board, Renewals, Signals, usage import, settings).
//!
//! Every change goes through `gaincraft_core::dispatch` (the same path as
//! CLI and MCP). Reads use the store and `health_of`, which has no side effects.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Duration, Local, NaiveDate};
use craft_biz::bus::Bus;
use craft_biz::Money;
use craft_biz_ui::app::{self, Frame, NavEntry, SuiteApp, Wiring};
use craft_biz_ui::kanban::{self, Board, Card, Column, Lane};
use craft_biz_ui::table::{Cell, Col, Table, TableEvent};
use craft_biz_ui::theme::{self, semibold};
use craft_biz_ui::widgets::Kpi;
use craft_biz_ui::{widgets, Tone};
use egui::{Align2, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use gaincraft_core::health::{Health, Weights};
use gaincraft_core::{dispatch, health_of, parse_telemetry, tools, Command, Confinement, Ctx};
use gaincraft_store::{Account, Cta, Day, Gain, Signal, Stats, Triage};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const V_OVERVIEW: usize = 0;
const V_ACCOUNTS: usize = 1;
const V_COCKPIT: usize = 2;
const V_RENEWALS: usize = 3;
const V_SIGNALS: usize = 4;
const V_IMPORT: usize = 5;
const V_SETTINGS: usize = 6;

/// Every MCP tool and where the window performs it (checked by a test).
pub const COVERAGE: &[(&str, &str)] = &[
    ("gain_status", "Overview KPIs; Settings > Status"),
    (
        "gain_account_add",
        "Top bar + Account; Accounts > New account; Import > Add account",
    ),
    ("gain_account_list", "Accounts table"),
    ("gain_account_show", "Customer 360 panel > Full record"),
    ("gain_account_update", "Customer 360 > Edit, Retire"),
    ("gain_telemetry_import", "Import usage > Dry run, Import"),
    (
        "gain_signal_add",
        "Customer 360 > Add signal; Signals > Record signal",
    ),
    ("gain_health", "Customer 360 > Recompute"),
    ("gain_health_all", "Overview / Accounts > Recompute all"),
    ("gain_triage_set", "Customer 360 > Triage, Clear triage"),
    ("gain_cta_list", "CTA cockpit board"),
    (
        "gain_cta_resolve",
        "CTA cockpit: drop on Done, Resolve / Dismiss buttons",
    ),
    ("gain_deadlines", "Renewals timeline and table"),
    ("gain_weights", "Settings > Health weights"),
    (
        "gain_inbox_drain",
        "Signals dock > Apply all; Settings > Apply signals",
    ),
    (
        "gain_demo_seed",
        "Overview empty state; Settings > Demo data",
    ),
];

struct Row {
    a: Account,
    h: Health,
    trend: Vec<f32>,
    open: Vec<String>,
}

struct Detail {
    key: String,
    days: Vec<Day>,
    triage: Option<Triage>,
    signals: Vec<Signal>,
}

#[derive(Default)]
struct Import {
    path: Option<PathBuf>,
    preview: Option<Result<Preview, String>>,
}

#[derive(Debug)]
struct Preview {
    rows: Vec<(String, Day)>,
    unknown: Vec<String>,
    accounts: usize,
    from: NaiveDate,
    to: NaiveDate,
}

pub struct GainApp {
    ctx: Ctx,
    db: String,
    today: NaiveDate,
    stats: Option<Stats>,
    rows: Vec<Row>,
    ctas: Vec<Cta>,
    signals: Vec<Signal>,
    deadlines: Vec<Value>,
    weights: Weights,
    wdraft: Weights,
    sel: Option<String>,
    sel_cta: Option<String>,
    detail: Option<Detail>,
    import: Import,
    owner_lanes: bool,
}

fn eur(c: i64) -> String {
    Money::from_cents(c).format_de()
}

fn band_tone(b: &str) -> Tone {
    match b {
        "green" => Tone::Ok,
        "yellow" => Tone::Warn,
        _ => Tone::Danger,
    }
}

fn band_label(b: &str, de: bool) -> &'static str {
    match (b, de) {
        ("green", true) => "gesund",
        ("green", false) => "healthy",
        ("yellow", true) => "beobachten",
        ("yellow", false) => "watch",
        (_, true) => "kritisch",
        (_, false) => "at risk",
    }
}

fn kind_label(k: &str, de: bool) -> &'static str {
    match (k, de) {
        ("low_health", true) => "Niedrige Gesundheit",
        ("low_health", false) => "Low health",
        ("adoption_drop", true) => "Nutzung fällt",
        ("adoption_drop", false) => "Usage drop",
        ("seats_unused", true) => "Ungenutzte Lizenzen",
        ("seats_unused", false) => "Unused seats",
        ("renewal", true) => "Verlängerung",
        ("renewal", false) => "Renewal",
        ("negative_signal", true) => "Unzufrieden",
        ("negative_signal", false) => "Unhappy customer",
        (_, true) => "Aufgabe",
        (_, false) => "Task",
    }
}

fn kind_tone(k: &str) -> Tone {
    match k {
        "low_health" => Tone::Danger,
        "adoption_drop" => Tone::Warn,
        "seats_unused" => Tone::Neutral,
        "renewal" => Tone::Info,
        "negative_signal" => Tone::Violet,
        _ => Tone::Neutral,
    }
}

fn frac_tone(x: f64) -> Tone {
    if x >= 0.7 {
        Tone::Ok
    } else if x >= 0.4 {
        Tone::Warn
    } else {
        Tone::Danger
    }
}

fn date_de(d: NaiveDate) -> String {
    d.format("%d.%m.%Y").to_string()
}

/// The weakest component, in words, for the "needs attention" list.
fn main_reason(h: &Health, de: bool) -> String {
    let worst = [("a", h.adoption), ("r", h.recency), ("s", h.sentiment)]
        .into_iter()
        .fold(("a", f64::MAX), |m, x| if x.1 < m.1 { x } else { m });
    match (worst.0, de) {
        ("a", true) => format!("geringe Nutzung ({:.0}%)", h.adoption * 100.0),
        ("a", false) => format!("low adoption ({:.0}%)", h.adoption * 100.0),
        ("r", true) => format!("{} Tage inaktiv", h.days_since_active.unwrap_or(0)),
        ("r", false) => format!("inactive {} days", h.days_since_active.unwrap_or(0)),
        (_, true) => "unzufriedene Signale".into(),
        (_, false) => "unhappy signals".into(),
    }
}

impl GainApp {
    pub fn new(ctx: Ctx, db: String) -> GainApp {
        GainApp {
            ctx,
            db,
            today: Local::now().date_naive(),
            stats: None,
            rows: Vec::new(),
            ctas: Vec::new(),
            signals: Vec::new(),
            deadlines: Vec::new(),
            weights: Weights::default(),
            wdraft: Weights::default(),
            sel: None,
            sel_cta: None,
            detail: None,
            import: Import::default(),
            owner_lanes: false,
        }
    }

    fn row(&self, key: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.a.key == key)
    }

    fn name_of(&self, key: &str) -> String {
        self.row(key)
            .map(|r| r.a.name.clone())
            .unwrap_or_else(|| key.to_string())
    }

    fn select(&mut self, key: &str) {
        self.sel = Some(key.to_string());
        if let Err(e) = self.load_detail() {
            self.detail = None;
            eprintln!("gaincraft-app: {e}");
        }
    }

    fn load_detail(&mut self) -> Result<(), String> {
        let Some(k) = self.sel.clone() else {
            self.detail = None;
            return Ok(());
        };
        let g = &self.ctx.gain;
        let a = match g.account(&k) {
            Ok(a) => a,
            Err(_) => {
                self.sel = None;
                self.detail = None;
                return Ok(());
            }
        };
        let days = g
            .days(a.id, self.today - Duration::days(89), self.today)
            .map_err(|e| e.to_string())?;
        let triage = g.triage(a.id, self.today).map_err(|e| e.to_string())?;
        let signals = g
            .signals(Some(a.id), Some(self.today - Duration::days(180)))
            .map_err(|e| e.to_string())?;
        self.detail = Some(Detail {
            key: a.key,
            days,
            triage,
            signals,
        });
        Ok(())
    }

    fn arr_at_risk(&self) -> i64 {
        self.rows
            .iter()
            .filter(|r| r.h.band != "green")
            .map(|r| r.a.arr_cents.unwrap_or(0))
            .sum()
    }

    fn open_ctas(&self) -> impl Iterator<Item = &Cta> {
        self.ctas.iter().filter(|c| c.status == "open")
    }

    fn bucket(&self, c: &Cta) -> &'static str {
        if c.status != "open" {
            return "done";
        }
        match c.due {
            Some(d) if d < self.today => "overdue",
            Some(d) if d <= self.today + Duration::days(7) => "week",
            Some(_) => "later",
            None => "nodate",
        }
    }

    // ---------------- views ----------------

    fn empty(&mut self, ui: &mut Ui, f: &mut Frame) {
        widgets::card(ui, |ui| {
            widgets::empty_state(ui, f.t("Noch keine Kunden", "No customers yet"), f.t("Lege einen Kunden an oder lade die Demo-Daten (drei Beispielkunden, klar als DEMO markiert).", "Add a customer or load the demo data (three example customers, clearly marked DEMO)."));
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 330.0).max(0.0) / 2.0);
                    if widgets::primary(ui, f.t("Kunden anlegen", "Add customer")).clicked() {
                        f.form("gain_account_add", &[]);
                    }
                    if ui
                        .button(f.t("Demo-Daten laden", "Load demo data"))
                        .clicked()
                    {
                        f.run(f.t("Demo-Daten", "Demo data"), "gain_demo_seed", json!({}));
                    }
                });
            });
        });
    }

    fn view_overview(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        let sub = format!(
            "{} {}  |  {}",
            self.rows.len(),
            f.t("aktive Kunden", "active customers"),
            date_de(self.today)
        );
        widgets::page_header(ui, f.t("Kundengesundheit", "Customer health"), &sub, |ui| {
            if widgets::primary(ui, f.t("Alle neu berechnen", "Recompute all"))
                .on_hover_text("gain_health_all: stores a snapshot per account and opens CTAs")
                .clicked()
            {
                f.run(
                    f.t("Gesundheit berechnet", "Health recomputed"),
                    "gain_health_all",
                    json!({}),
                );
            }
        });
        if self.rows.is_empty() {
            self.empty(ui, f);
            return;
        }
        let red = self.rows.iter().filter(|r| r.h.band == "red").count();
        let yellow = self.rows.iter().filter(|r| r.h.band == "yellow").count();
        let green = self.rows.len() - red - yellow;
        let overdue = self
            .open_ctas()
            .filter(|c| c.due.is_some_and(|d| d < self.today))
            .count();
        let open = self.open_ctas().count();
        let next = self.deadlines.iter().find(|d| d["kind"] == "notice");
        let arr: i64 = self.stats.as_ref().map(|s| s.arr_active_cents).unwrap_or(0);
        let risk = self.arr_at_risk();
        let pct = if arr > 0 {
            format!(
                "{:.0}% {}",
                risk as f64 * 100.0 / arr as f64,
                f.t("des ARR", "of ARR")
            )
        } else {
            "-".into()
        };
        let mut kpis = vec![
            Kpi::new(f.t("ARR aktiv", "Active ARR"), eur(arr)),
            Kpi::new(f.t("ARR gefährdet", "ARR at risk"), eur(risk))
                .note(pct, if risk > 0 { Tone::Danger } else { Tone::Ok }),
            Kpi::new(f.t("Kunden", "Customers"), self.rows.len().to_string()).note(
                format!("{red} {}", f.t("kritisch", "at risk")),
                if red > 0 { Tone::Danger } else { Tone::Ok },
            ),
            Kpi::new(f.t("Offene Aufgaben", "Open CTAs"), open.to_string()).note(
                format!("{overdue} {}", f.t("überfällig", "overdue")),
                if overdue > 0 { Tone::Warn } else { Tone::Ok },
            ),
        ];
        kpis.push(match next {
            Some(d) => {
                let left = d["days_left"].as_i64().unwrap_or(999);
                let date = d["date"]
                    .as_str()
                    .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                    .map(date_de)
                    .unwrap_or_default();
                Kpi::new(f.t("Nächste Kündigungsfrist", "Next notice date"), date).note(
                    format!("{left} {}", f.t("Tage", "days")),
                    if left <= 30 { Tone::Danger } else { Tone::Info },
                )
            }
            None => Kpi::new(f.t("Nächste Kündigungsfrist", "Next notice date"), "-"),
        });
        widgets::kpi_row(ui, &kpis);
        ui.add_space(12.0);
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::section(ui, f.t("Verteilung", "Distribution"), None);
            band_bar(ui, green, yellow, red, de);
        });
        ui.add_space(12.0);
        ui.columns(2, |cols| {
            widgets::card(&mut cols[0], |ui| {
                ui.set_width(ui.available_width());
                widgets::section(ui, f.t("Braucht Aufmerksamkeit", "Needs attention"), None);
                let mut worst: Vec<&Row> =
                    self.rows.iter().filter(|r| r.h.band != "green").collect();
                worst.sort_by_key(|r| r.h.score);
                if worst.is_empty() {
                    ui.label(f.t("Alle Kunden sind gesund.", "Every customer is healthy."));
                }
                let mut pick = None;
                for r in worst.iter().take(6) {
                    if attention_item(ui, r, &main_reason(&r.h, de), de).clicked() {
                        pick = Some(r.a.key.clone());
                    }
                }
                if let Some(k) = pick {
                    self.select(&k);
                    f.goto(V_ACCOUNTS);
                }
            });
            widgets::card(&mut cols[1], |ui| {
                ui.set_width(ui.available_width());
                widgets::section(
                    ui,
                    f.t("Fristen, nächste 90 Tage", "Deadlines, next 90 days"),
                    None,
                );
                let soon: Vec<&Value> = self
                    .deadlines
                    .iter()
                    .filter(|d| d["days_left"].as_i64().unwrap_or(999) <= 90)
                    .collect();
                if soon.is_empty() {
                    ui.label(f.t(
                        "Keine Fristen in den nächsten 90 Tagen.",
                        "No deadlines in the next 90 days.",
                    ));
                }
                let mut pick = None;
                for d in soon.iter().take(8) {
                    if deadline_item(ui, d, de).clicked() {
                        pick = d["account"].as_str().map(str::to_string);
                    }
                }
                if let Some(k) = pick {
                    self.select(&k);
                    f.goto(V_ACCOUNTS);
                }
            });
        });
    }

    fn view_accounts(&mut self, ui: &mut Ui, f: &mut Frame) {
        let sub = f.t(
            "Klick wählt, Spaltenkopf sortiert. Rechts: Kunden-360.",
            "Click to select, click a header to sort. Right: Customer 360.",
        );
        widgets::page_header(ui, f.t("Kunden", "Accounts"), sub, |ui| {
            if widgets::primary(ui, f.t("Neuer Kunde", "New account")).clicked() {
                f.form("gain_account_add", &[]);
            }
            if ui
                .button(f.t("Alle neu berechnen", "Recompute all"))
                .clicked()
            {
                f.run(
                    f.t("Gesundheit berechnet", "Health recomputed"),
                    "gain_health_all",
                    json!({}),
                );
            }
        });
        if self.rows.is_empty() {
            self.empty(ui, f);
            return;
        }
        let de = f.lang_de;
        let cols = vec![
            Col::new(f.t("Kunde", "Account"), 2.4),
            Col::new(f.t("Status", "Health"), 1.0),
            Col::num("Score", 0.6),
            Col::new("Trend", 1.0).prio(1),
            Col::new(f.t("Betreuung", "Owner"), 1.4).prio(2),
            Col::num("ARR", 1.0),
            Col::num(f.t("Lizenzen", "Seats"), 0.7).prio(3),
            Col::new(f.t("Offene Aufgaben", "Open CTAs"), 2.0),
            Col::new(f.t("Kündigungsfrist", "Notice"), 1.0).prio(4),
        ];
        let data: Vec<(String, Vec<Cell>)> = self
            .rows
            .iter()
            .filter(|r| {
                app::search_hit(
                    &f.search,
                    &[
                        &r.a.name,
                        &r.a.key,
                        r.a.domain.as_deref().unwrap_or(""),
                        r.a.owner.as_deref().unwrap_or(""),
                    ],
                )
            })
            .map(|r| {
                let tone = band_tone(r.h.band);
                (
                    r.a.key.clone(),
                    vec![
                        Cell::Strong(r.a.name.clone()),
                        Cell::Pill(band_label(r.h.band, de).into(), tone),
                        Cell::Mono(r.h.score.to_string()),
                        Cell::Spark(r.trend.clone(), tone),
                        r.a.owner
                            .clone()
                            .map(Cell::Person)
                            .unwrap_or(Cell::Weak("-".into())),
                        Cell::Money(r.a.arr_cents.map(eur).unwrap_or_else(|| "-".into())),
                        Cell::Mono(
                            r.a.seats
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "-".into()),
                        ),
                        Cell::Pills(
                            r.open
                                .iter()
                                .map(|k| (kind_label(k, de).to_string(), kind_tone(k)))
                                .collect(),
                        ),
                        Cell::Weak(r.a.last_notice.map(date_de).unwrap_or_else(|| "-".into())),
                    ],
                )
            })
            .collect();
        let mut pick = None;
        widgets::card(ui, |ui| {
            for e in Table::new("gain_accounts", &cols, &data)
                .selected(self.sel.as_deref())
                .show(ui)
            {
                match e {
                    TableEvent::Select(k) | TableEvent::Open(k) => pick = Some(k),
                }
            }
        });
        if let Some(k) = pick {
            self.select(&k);
        }
    }

    fn view_cockpit(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        let mut lanes = self.owner_lanes;
        widgets::page_header(ui, f.t("Aufgaben-Cockpit", "CTA cockpit"), f.t("Spalten nach Fälligkeit. Auf Erledigt ziehen schließt die Aufgabe (mit Notiz). Doppelklick öffnet den Kunden.", "Columns by due date. Drop on Done to close a CTA (with a note). Double-click opens the account."), |ui| {
            ui.checkbox(&mut lanes, f.t("Bahnen nach Betreuung", "Swimlanes by owner"));
        });
        self.owner_lanes = lanes;
        let columns = vec![
            Column::new("overdue", f.t("Überfällig", "Overdue"), Tone::Danger),
            Column::new("week", f.t("Diese Woche", "This week"), Tone::Warn),
            Column::new("later", f.t("Später", "Later"), Tone::Accent),
            Column::new("nodate", f.t("Ohne Datum", "No date"), Tone::Neutral),
            Column::new("done", f.t("Erledigt", "Done"), Tone::Ok),
        ];
        let owner_of = |acc: &str| {
            self.row(acc)
                .and_then(|r| r.a.owner.clone())
                .unwrap_or_else(|| "-".into())
        };
        let recent_done = self.today - Duration::days(30);
        let cards: Vec<Card> = self
            .ctas
            .iter()
            .filter(|c| {
                c.status == "open" || c.last_seen >= recent_done || c.created_on >= recent_done
            })
            .filter(|c| {
                app::search_hit(
                    &f.search,
                    &[&c.title, &c.account, &self.name_of(&c.account)],
                )
            })
            .map(|c| {
                let band = self
                    .row(&c.account)
                    .map(|r| band_tone(r.h.band))
                    .unwrap_or(Tone::Neutral);
                let due = c
                    .due
                    .map(|d| format!("{} {}", f.t("fällig", "due"), d.format("%d.%m.")))
                    .unwrap_or_default();
                let mut meta = vec![self.name_of(&c.account)];
                if !due.is_empty() {
                    meta.push(due);
                }
                let done = c.status != "open";
                if done {
                    meta = vec![format!(
                        "{} {}",
                        if c.status == "dismissed" {
                            f.t("verworfen", "dismissed")
                        } else {
                            f.t("gelöst", "resolved")
                        },
                        c.resolved_by.clone().unwrap_or_default()
                    )];
                }
                Card {
                    id: c.id.to_string(),
                    column: self.bucket(c).into(),
                    lane: if self.owner_lanes {
                        owner_of(&c.account)
                    } else {
                        String::new()
                    },
                    key: format!("CTA-{}", c.id),
                    title: c.title.clone(),
                    tags: vec![(kind_label(&c.kind, de).into(), kind_tone(&c.kind))],
                    accent: band,
                    badge: self.row(&c.account).map(|r| (r.h.score.to_string(), band)),
                    assignee: self.row(&c.account).and_then(|r| r.a.owner.clone()),
                    meta,
                    blocked: None,
                    done,
                }
            })
            .collect();
        let mut lane_list: Vec<Lane> = Vec::new();
        if self.owner_lanes {
            for c in &cards {
                if !lane_list.iter().any(|l| l.id == c.lane) {
                    lane_list.push(Lane {
                        id: c.lane.clone(),
                        title: c.lane.clone(),
                    });
                }
            }
            lane_list.sort_by(|a, b| a.title.cmp(&b.title));
        }
        if cards.is_empty() {
            widgets::card(ui, |ui| {
                widgets::empty_state(
                    ui,
                    f.t("Keine Aufgaben", "No CTAs"),
                    f.t(
                        "Aufgaben entstehen beim Berechnen der Gesundheit.",
                        "CTAs appear when health is computed.",
                    ),
                )
            });
            return;
        }
        let msg_done = f
            .t(
                "Erledigte Aufgaben bleiben geschlossen. Ein neuer Anlass öffnet eine neue.",
                "Closed CTAs stay closed. A new reason opens a new one.",
            )
            .to_string();
        let msg_due = f
            .t(
                "Die Fälligkeit bestimmt die Spalte. Zum Schließen auf Erledigt ziehen.",
                "The due date decides the column. Drop on Done to close it.",
            )
            .to_string();
        let rule = move |c: &Card, to: &str| -> Result<(), String> {
            if c.done {
                Err(msg_done.clone())
            } else if to == "done" {
                Ok(())
            } else {
                Err(msg_due.clone())
            }
        };
        let events = {
            let r: kanban::Rule = &rule;
            let mut b = Board::new("gain_cockpit", &columns, &cards)
                .rule(r)
                .selected(self.sel_cta.as_deref())
                .add_buttons(false);
            if self.owner_lanes {
                b = b.lanes(&lane_list);
            }
            b.show(ui)
        };
        for e in events {
            match e {
                kanban::Event::Moved { card, to, .. } if to == "done" => {
                    self.sel_cta = Some(card.clone());
                    f.form("gain_cta_resolve", &[("id", &card)]);
                    f.toast(
                        f.t(
                            "Was wurde getan? Notiz eintragen und ausführen.",
                            "What was done? Add a note and run.",
                        ),
                        Tone::Info,
                    );
                }
                kanban::Event::Moved { .. } | kanban::Event::Add { .. } | kanban::Event::Undo => {}
                kanban::Event::Refused { reason, .. } => f.toast(reason, Tone::Warn),
                kanban::Event::Select(id) => {
                    self.sel_cta = Some(id.clone());
                    if let Some(acc) = self
                        .ctas
                        .iter()
                        .find(|c| c.id.to_string() == id)
                        .map(|c| c.account.clone())
                    {
                        self.select(&acc);
                    }
                }
                kanban::Event::Open(id) => {
                    if let Some(acc) = self
                        .ctas
                        .iter()
                        .find(|c| c.id.to_string() == id)
                        .map(|c| c.account.clone())
                    {
                        self.select(&acc);
                        f.goto(V_ACCOUNTS);
                    }
                }
            }
        }
    }

    fn view_renewals(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        widgets::page_header(ui, f.t("Verlängerungen", "Renewals"), f.t("Kündigungsfristen und Vertragsenden der nächsten 12 Monate. Verträge kommen aus contractcraft.", "Notice dates and term ends of the next 12 months. Contracts come from contractcraft."), |_| {});
        if self.deadlines.is_empty() {
            widgets::card(ui, |ui| {
                widgets::empty_state(ui, f.t("Keine Fristen", "No deadlines"), f.t("Unterzeichnete Verträge aus contractcraft bringen Fristen mit (Signale > Anwenden).", "Executed contracts from contractcraft bring their dates (Signals > Apply)."))
            });
            return;
        }
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            timeline(ui, &self.deadlines, self.today, de);
        });
        ui.add_space(10.0);
        let cols = vec![
            Col::new(f.t("Datum", "Date"), 1.0),
            Col::num(f.t("Tage", "Days"), 0.7),
            Col::new(f.t("Art", "Kind"), 1.1),
            Col::new(f.t("Kunde", "Account"), 2.4),
            Col::num("ARR", 1.0),
            Col::new(f.t("Vertrag", "Contract"), 1.0),
        ];
        let data: Vec<(String, Vec<Cell>)> = self
            .deadlines
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let days = d["days_left"].as_i64().unwrap_or(0);
                let tone = if days <= 30 {
                    Tone::Danger
                } else if days <= 60 {
                    Tone::Warn
                } else {
                    Tone::Info
                };
                let notice = d["kind"] == "notice";
                (
                    format!("{i}:{}", d["account"].as_str().unwrap_or("")),
                    vec![
                        Cell::Mono(
                            d["date"]
                                .as_str()
                                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                                .map(date_de)
                                .unwrap_or_default(),
                        ),
                        Cell::Pill(days.to_string(), tone),
                        Cell::Pill(
                            if notice {
                                f.t("Kündigungsfrist", "Notice date").into()
                            } else {
                                f.t("Vertragsende", "Term end").into()
                            },
                            if notice { Tone::Warn } else { Tone::Info },
                        ),
                        Cell::Strong(d["name"].as_str().unwrap_or("").into()),
                        Cell::Money(d["arr"].as_str().unwrap_or("-").into()),
                        Cell::Mono(d["contract"].as_str().unwrap_or("-").into()),
                    ],
                )
            })
            .collect();
        let mut pick = None;
        widgets::card(ui, |ui| {
            for e in Table::new("gain_renewals", &cols, &data)
                .filter_box(false)
                .show(ui)
            {
                let (TableEvent::Select(id) | TableEvent::Open(id)) = e;
                {
                    pick = id.split_once(':').map(|(_, k)| k.to_string());
                }
            }
        });
        if let Some(k) = pick {
            self.select(&k);
        }
    }

    fn view_signals(&mut self, ui: &mut Ui, f: &mut Frame) {
        let sel = self.sel.clone().unwrap_or_default();
        widgets::page_header(ui, f.t("Signale", "Signals"), f.t("Stimmung der letzten 180 Tage: aus deskcraft, formcraft und von Hand. Ältere Signale verblassen (Halbwertszeit 30 Tage).", "Mood of the last 180 days: from deskcraft, formcraft and by hand. Older signals fade (30-day half-life)."), |ui| {
            if widgets::primary(ui, f.t("Signal erfassen", "Record signal")).clicked() {
                f.form("gain_signal_add", &[("account", &sel)]);
            }
        });
        if self.signals.is_empty() {
            widgets::card(ui, |ui| {
                widgets::empty_state(
                    ui,
                    f.t("Keine Signale", "No signals"),
                    f.t(
                        "Unzufriedene Mails aus deskcraft landen hier (Signale > Anwenden).",
                        "Unhappy mails from deskcraft land here (Signals > Apply).",
                    ),
                )
            });
            return;
        }
        let cols = vec![
            Col::new(f.t("Datum", "Date"), 0.9),
            Col::new(f.t("Kunde", "Account"), 1.8),
            Col::new(f.t("Richtung", "Polarity"), 0.9),
            Col::num("Score", 0.6),
            Col::new(f.t("Quelle", "Source"), 1.0).prio(1),
            Col::new(f.t("Bezug", "Reference"), 0.8).prio(2),
            Col::new(f.t("Notiz", "Note"), 3.0),
        ];
        let data: Vec<(String, Vec<Cell>)> = self
            .signals
            .iter()
            .filter(|s| {
                app::search_hit(
                    &f.search,
                    &[
                        &s.account,
                        &s.note,
                        &s.source,
                        s.reference.as_deref().unwrap_or(""),
                    ],
                )
            })
            .map(|s| {
                let neg = s.polarity == "negative";
                (
                    format!("{}:{}", s.id, s.account),
                    vec![
                        Cell::Mono(date_de(s.at)),
                        Cell::Strong(self.name_of(&s.account)),
                        Cell::Pill(
                            if neg {
                                f.t("negativ", "negative").into()
                            } else {
                                f.t("positiv", "positive").into()
                            },
                            if neg { Tone::Danger } else { Tone::Ok },
                        ),
                        Cell::Mono(s.score.to_string()),
                        Cell::Text(s.source.clone()),
                        Cell::Mono(s.reference.clone().unwrap_or_else(|| "-".into())),
                        Cell::Text(s.note.clone()),
                    ],
                )
            })
            .collect();
        let mut pick = None;
        widgets::card(ui, |ui| {
            for e in Table::new("gain_signals", &cols, &data).show(ui) {
                let (TableEvent::Select(id) | TableEvent::Open(id)) = e;
                {
                    pick = id.split_once(':').map(|(_, k)| k.to_string());
                }
            }
        });
        if let Some(k) = pick {
            self.select(&k);
        }
    }

    fn preview(&self, path: &Path) -> Result<Preview, String> {
        let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if meta.len() > 20 * 1024 * 1024 {
            return Err("file larger than 20 MB".into());
        }
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let rows = parse_telemetry(&text)?;
        let mut names: Vec<String> = rows.iter().map(|(a, _)| a.clone()).collect();
        names.sort();
        names.dedup();
        let unknown: Vec<String> = names
            .iter()
            .filter(|n| self.ctx.gain.account(n).is_err())
            .cloned()
            .collect();
        let from = rows.iter().map(|(_, d)| d.date).min().unwrap_or(self.today);
        let to = rows.iter().map(|(_, d)| d.date).max().unwrap_or(self.today);
        Ok(Preview {
            accounts: names.len(),
            rows,
            unknown,
            from,
            to,
        })
    }

    fn view_import(&mut self, ui: &mut Ui, f: &mut Frame) {
        widgets::page_header(ui, f.t("Nutzung importieren", "Import usage"), f.t("CSV mit Summen pro Kunde und Tag: account;date;active_users[;sessions;key_actions]", "CSV with totals per customer and day: account;date;active_users[;sessions;key_actions]"), |_| {});
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            let (fg, bg) = theme::tone_colors(Tone::Info, ui.visuals().dark_mode);
            egui::Frame::new().fill(bg).corner_radius(8).inner_margin(10).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(f.t("Keine Personendaten: Dateien mit Spalten wie user, email, name, login oder ip werden abgelehnt (ArbVG § 96a, DSGVO). Nur Summen pro Kunde und Tag.", "No personal data: files with columns like user, email, name, login or ip are refused (ArbVG section 96a, GDPR). Only totals per customer and day.")).color(fg));
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::primary(ui, f.t("CSV-Datei wählen...", "Choose CSV file...")).clicked()
                {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("CSV", &["csv", "txt"])
                        .pick_file()
                    {
                        self.import.preview = Some(self.preview(&p));
                        self.import.path = Some(p);
                    }
                }
                if let Some(p) = &self.import.path {
                    ui.label(
                        RichText::new(p.display().to_string()).family(egui::FontFamily::Monospace),
                    );
                    if ui.button(f.t("Neu einlesen", "Re-read")).clicked() {
                        let p = p.clone();
                        self.import.preview = Some(self.preview(&p));
                    }
                }
            });
        });
        ui.add_space(10.0);
        let Some(prev) = &self.import.preview else {
            return;
        };
        let path = self
            .import
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        match prev {
            Err(e) => {
                widgets::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let (fg, bg) = theme::tone_colors(Tone::Danger, ui.visuals().dark_mode);
                    egui::Frame::new()
                        .fill(bg)
                        .corner_radius(8)
                        .inner_margin(10)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(f.t("Datei abgelehnt", "File refused"))
                                    .color(fg)
                                    .family(semibold()),
                            );
                            ui.label(RichText::new(e).color(fg));
                        });
                });
            }
            Ok(p) => {
                let unknown = p.unknown.clone();
                widgets::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        widgets::pill(
                            ui,
                            &format!("{} {}", p.rows.len(), f.t("Zeilen", "rows")),
                            Tone::Accent,
                        );
                        widgets::pill(
                            ui,
                            &format!("{} {}", p.accounts, f.t("Kunden", "accounts")),
                            Tone::Accent,
                        );
                        widgets::pill(
                            ui,
                            &format!("{} - {}", date_de(p.from), date_de(p.to)),
                            Tone::Neutral,
                        );
                        if unknown.is_empty() {
                            widgets::pill(
                                ui,
                                f.t("alle Kunden bekannt", "all accounts known"),
                                Tone::Ok,
                            );
                        } else {
                            widgets::pill(
                                ui,
                                &format!("{} {}", unknown.len(), f.t("unbekannt", "unknown")),
                                Tone::Danger,
                            );
                        }
                    });
                    for u in &unknown {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(u).family(egui::FontFamily::Monospace));
                            if ui
                                .button(f.t("Als Kunde anlegen", "Add as account"))
                                .clicked()
                            {
                                f.form(
                                    "gain_account_add",
                                    &[
                                        ("name", u),
                                        ("domain", if u.contains('.') { u } else { "" }),
                                    ],
                                );
                            }
                        });
                    }
                    ui.add_space(6.0);
                    let cols = vec![
                        Col::new("account", 2.0),
                        Col::new("date", 1.0),
                        Col::num("active_users", 1.0),
                        Col::num("sessions", 1.0),
                        Col::num("key_actions", 1.0),
                    ];
                    let data: Vec<(String, Vec<Cell>)> = p
                        .rows
                        .iter()
                        .take(200)
                        .enumerate()
                        .map(|(i, (a, d))| {
                            (
                                i.to_string(),
                                vec![
                                    Cell::Text(a.clone()),
                                    Cell::Mono(date_de(d.date)),
                                    Cell::Mono(d.active_users.to_string()),
                                    Cell::Mono(d.sessions.to_string()),
                                    Cell::Mono(d.key_actions.to_string()),
                                ],
                            )
                        })
                        .collect();
                    ui.push_id("import_preview", |ui| {
                        ui.set_max_height(300.0);
                        Table::new("gain_import", &cols, &data)
                            .filter_box(false)
                            .show(ui);
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ok = unknown.is_empty();
                        if ui
                            .add_enabled(ok, egui::Button::new(f.t("Probelauf", "Dry run")))
                            .clicked()
                        {
                            f.run_show(
                                "gain_telemetry_import",
                                json!({ "path": path, "dry_run": true }),
                            );
                        }
                        let q = format!(
                            "{} {} {}?",
                            f.t("Nutzung aus", "Import usage from"),
                            path,
                            f.t("importieren", "now")
                        );
                        if ui
                            .add_enabled(
                                ok,
                                egui::Button::new(
                                    RichText::new(f.t("Importieren", "Import")).family(semibold()),
                                ),
                            )
                            .clicked()
                        {
                            f.run_confirm(
                                q,
                                f.t("Nutzung importiert", "Usage imported"),
                                "gain_telemetry_import",
                                json!({ "path": path }),
                            );
                        }
                        if !ok {
                            ui.label(f.t(
                                "Erst alle Kunden anlegen; vorher wird nichts geschrieben.",
                                "Add every account first; nothing is written before that.",
                            ));
                        }
                    });
                });
            }
        }
    }

    fn view_settings(&mut self, ui: &mut Ui, f: &mut Frame) {
        widgets::page_header(ui, f.t("Einstellungen", "Settings"), &self.db, |_| {});
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::section(ui, f.t("Gewichte der Gesundheit", "Health weights"), None);
            ui.label(
                RichText::new(
                    "score = 100 x (adoption x wA + recency x wR + sentiment x wS) x modifier",
                )
                .family(egui::FontFamily::Monospace),
            );
            ui.add_space(6.0);
            egui::Grid::new("weights")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    for (label, v) in [
                        (
                            f.t("Nutzung (adoption)", "Adoption"),
                            &mut self.wdraft.adoption,
                        ),
                        (
                            f.t("Aktualität (recency)", "Recency"),
                            &mut self.wdraft.recency,
                        ),
                        (
                            f.t("Stimmung (sentiment)", "Sentiment"),
                            &mut self.wdraft.sentiment,
                        ),
                    ] {
                        ui.label(label);
                        ui.add(
                            egui::Slider::new(v, 0.0..=1.0)
                                .step_by(0.05)
                                .fixed_decimals(2),
                        );
                        ui.end_row();
                    }
                });
            let sum = self.wdraft.adoption + self.wdraft.recency + self.wdraft.sentiment;
            let ok = (sum - 1.0).abs() < 0.001;
            ui.horizontal(|ui| {
                widgets::pill(ui, &format!("{} {sum:.2}", f.t("Summe", "Sum")), if ok { Tone::Ok } else { Tone::Danger });
                let changed = self.wdraft != self.weights;
                if ui.add_enabled(ok && changed, egui::Button::new(RichText::new(f.t("Speichern", "Save")).family(semibold()))).clicked() {
                    let w = self.wdraft;
                    f.run(f.t("Gewichte gespeichert", "Weights saved"), "gain_weights", json!({ "set": { "adoption": w.adoption, "recency": w.recency, "sentiment": w.sentiment } }));
                }
                if ui.button(f.t("Standard", "Default")).clicked() {
                    self.wdraft = Weights::default();
                }
                if changed && ui.button(f.t("Verwerfen", "Discard")).clicked() {
                    self.wdraft = self.weights;
                }
            });
        });
        ui.add_space(10.0);
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::section(ui, f.t("Daten", "Data"), None);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(f.t("Signale anwenden", "Apply signals"))
                    .on_hover_text("gain_inbox_drain")
                    .clicked()
                {
                    f.run_show("gain_inbox_drain", json!({}));
                }
                if ui.button("Status").clicked() {
                    f.run_show("gain_status", json!({}));
                }
                if ui
                    .add_enabled(
                        self.rows.is_empty(),
                        egui::Button::new(f.t("Demo-Daten laden", "Load demo data")),
                    )
                    .on_disabled_hover_text("only on an empty database")
                    .clicked()
                {
                    f.run(f.t("Demo-Daten", "Demo data"), "gain_demo_seed", json!({}));
                }
            });
        });
    }

    fn detail_360(&mut self, ui: &mut Ui, f: &mut Frame) -> bool {
        let Some(d) = &self.detail else {
            ui.label(f.t(
                "Kunden wählen, um Details zu sehen.",
                "Select an account to see details.",
            ));
            return true;
        };
        let Some(r) = self.row(&d.key) else {
            ui.label(f.t("Kunde ist inaktiv.", "Account is inactive."));
            return true;
        };
        let de = f.lang_de;
        let t = theme::current(ui.ctx());
        let a = &r.a;
        let h = &r.h;
        let key = a.key.clone();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(&a.name).family(semibold()).size(19.0));
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(&a.key)
                            .family(egui::FontFamily::Monospace)
                            .color(t.text_weak),
                    );
                    if let Some(dm) = &a.domain {
                        ui.label(RichText::new(dm).color(t.text_weak));
                    }
                    if a.auto_created {
                        widgets::pill(ui, f.t("automatisch angelegt", "auto-created"), Tone::Warn)
                            .on_hover_text("created by a bus event; check seats and owner");
                    }
                    if a.notes.contains("DEMO") {
                        widgets::pill(ui, "DEMO", Tone::Violet);
                    }
                });
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            score_ring(ui, h.score, band_tone(h.band));
            ui.vertical(|ui| {
                widgets::pill(ui, band_label(h.band, de), band_tone(h.band));
                if let Some(o) = &a.owner {
                    ui.horizontal(|ui| {
                        widgets::avatar(ui, o, 10.0);
                        ui.label(o);
                    });
                }
                ui.label(
                    RichText::new(format!(
                        "ARR {}",
                        a.arr_cents.map(eur).unwrap_or_else(|| "-".into())
                    ))
                    .family(egui::FontFamily::Monospace),
                );
            });
        });
        ui.add_space(6.0);
        let seats = a
            .seats
            .map(|s| format!("{:.1} / {s}", h.avg_active_30d))
            .unwrap_or_else(|| format!("{:.1}", h.avg_active_30d));
        let since = h
            .days_since_active
            .map(|n| format!("{n} {}", f.t("Tage", "days")))
            .unwrap_or_else(|| "-".into());
        for (label, v, note) in [
            (
                f.t("Nutzung", "Adoption"),
                h.adoption,
                format!("{} {seats}", f.t("aktiv/Lizenzen", "active/seats")),
            ),
            (
                f.t("Aktualität", "Recency"),
                h.recency,
                format!("{} {since}", f.t("zuletzt vor", "last seen")),
            ),
            (
                f.t("Stimmung", "Sentiment"),
                h.sentiment,
                format!("{} {}", d.signals.len(), f.t("Signale", "signals")),
            ),
        ] {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [86.0, 18.0],
                    egui::Label::new(RichText::new(label).family(semibold())),
                );
                widgets::meter(ui, v as f32, 120.0, frac_tone(v));
                ui.label(
                    RichText::new(format!("{:.0}%", v * 100.0)).family(egui::FontFamily::Monospace),
                );
                ui.label(RichText::new(note).color(t.text_weak).size(13.0));
            });
        }
        ui.horizontal(|ui| {
            ui.add_sized(
                [86.0, 18.0],
                egui::Label::new(RichText::new(f.t("Faktor", "Modifier")).family(semibold())),
            );
            ui.label(
                RichText::new(format!("x {:.2}", h.modifier)).family(egui::FontFamily::Monospace),
            );
            if let Some(tr) = &d.triage {
                ui.label(
                    RichText::new(format!(
                        "{} ({} {})",
                        tr.reason,
                        f.t("bis", "until"),
                        date_de(tr.expires)
                    ))
                    .color(t.text_weak)
                    .size(13.0),
                );
            }
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if widgets::primary(ui, f.t("Neu berechnen", "Recompute")).clicked() {
                f.run(
                    f.t("Gesundheit berechnet", "Health recomputed"),
                    "gain_health",
                    json!({ "account": key }),
                );
            }
            if ui.button(f.t("Bearbeiten", "Edit")).clicked() {
                f.form("gain_account_update", &[("account", &key)]);
            }
            if ui.button(f.t("Signal", "Add signal")).clicked() {
                f.form("gain_signal_add", &[("account", &key)]);
            }
            if ui
                .button("Triage")
                .on_hover_text("advisory modifier 0.8-1.2 with a reason")
                .clicked()
            {
                f.form("gain_triage_set", &[("account", &key), ("days", "30")]);
            }
            if d.triage.is_some() && ui.button(f.t("Triage löschen", "Clear triage")).clicked() {
                f.run(
                    f.t("Triage gelöscht", "Triage cleared"),
                    "gain_triage_set",
                    json!({ "account": key, "clear": true }),
                );
            }
            if ui.button(f.t("Ganzer Datensatz", "Full record")).clicked() {
                f.run_show("gain_account_show", json!({ "account": key }));
            }
            if widgets::danger(ui, f.t("Stilllegen", "Retire")).clicked() {
                let q = format!(
                    "{} {}?",
                    f.t("Kunden stilllegen:", "Retire account"),
                    a.name
                );
                f.run_confirm(
                    q,
                    f.t("Kunde stillgelegt", "Account retired"),
                    "gain_account_update",
                    json!({ "account": key, "active": false }),
                );
            }
        });
        ui.add_space(10.0);
        widgets::section(ui, f.t("Nutzung, 90 Tage", "Usage, 90 days"), None);
        usage_chart(ui, &d.days, a.seats, self.today);
        ui.add_space(10.0);
        widgets::section(ui, f.t("Vertrag", "Contract"), None);
        widgets::field(ui, f.t("Vertrag", "Contract"), |ui| {
            ui.label(
                RichText::new(a.contract_id.clone().unwrap_or_else(|| "-".into()))
                    .family(egui::FontFamily::Monospace),
            );
        });
        widgets::field(ui, f.t("Kündigungsfrist", "Notice date"), |ui| {
            match a.last_notice {
                Some(n) => {
                    let left = (n - self.today).num_days();
                    ui.label(date_de(n));
                    widgets::pill(
                        ui,
                        &format!("{left} {}", f.t("Tage", "days")),
                        if left <= 30 {
                            Tone::Danger
                        } else if left <= 60 {
                            Tone::Warn
                        } else {
                            Tone::Info
                        },
                    );
                }
                None => {
                    ui.label("-");
                }
            }
        });
        widgets::field(ui, f.t("Vertragsende", "Term end"), |ui| {
            ui.label(a.term_end.map(date_de).unwrap_or_else(|| "-".into()));
        });
        widgets::field(ui, f.t("Verlängerung", "Renewal"), |ui| {
            ui.label(
                a.renewal_months
                    .map(|m| format!("{m} {}", f.t("Monate", "months")))
                    .unwrap_or_else(|| "-".into()),
            );
        });
        ui.add_space(10.0);
        let open: Vec<&Cta> = self
            .ctas
            .iter()
            .filter(|c| c.account == key && c.status == "open")
            .collect();
        widgets::section(ui, f.t("Offene Aufgaben", "Open CTAs"), Some(open.len()));
        for c in open {
            widgets::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    widgets::pill(ui, kind_label(&c.kind, de), kind_tone(&c.kind));
                    if let Some(dd) = c.due {
                        let tone = if dd < self.today {
                            Tone::Danger
                        } else {
                            Tone::Neutral
                        };
                        widgets::pill(
                            ui,
                            &format!("{} {}", f.t("fällig", "due"), date_de(dd)),
                            tone,
                        );
                    }
                });
                ui.label(&c.title);
                ui.horizontal(|ui| {
                    let id = c.id.to_string();
                    if ui.button(f.t("Lösen", "Resolve")).clicked() {
                        f.form("gain_cta_resolve", &[("id", &id)]);
                    }
                    if ui.button(f.t("Verwerfen", "Dismiss")).clicked() {
                        f.form("gain_cta_resolve", &[("id", &id), ("dismiss", "true")]);
                    }
                });
            });
            ui.add_space(4.0);
        }
        ui.add_space(10.0);
        widgets::section(
            ui,
            f.t("Signale, 180 Tage", "Signals, 180 days"),
            Some(d.signals.len()),
        );
        for s in &d.signals {
            ui.horizontal_wrapped(|ui| {
                let neg = s.polarity == "negative";
                widgets::pill(
                    ui,
                    &format!("{}{}", if neg { "-" } else { "+" }, s.score),
                    if neg { Tone::Danger } else { Tone::Ok },
                );
                ui.label(RichText::new(date_de(s.at)).family(egui::FontFamily::Monospace));
                ui.label(RichText::new(&s.source).color(t.text_weak));
                if let Some(rf) = &s.reference {
                    ui.label(RichText::new(rf).family(egui::FontFamily::Monospace));
                }
            });
            if !s.note.is_empty() {
                ui.label(RichText::new(&s.note).size(13.5));
            }
            ui.add_space(4.0);
        }
        true
    }

    fn detail_cta(&mut self, ui: &mut Ui, f: &mut Frame) -> bool {
        let Some(c) = self
            .sel_cta
            .as_ref()
            .and_then(|id| self.ctas.iter().find(|c| &c.id.to_string() == id))
            .cloned()
        else {
            return self.detail_360(ui, f);
        };
        let de = f.lang_de;
        ui.label(RichText::new(format!("CTA-{}", c.id)).family(egui::FontFamily::Monospace));
        ui.label(RichText::new(&c.title).family(semibold()).size(17.0));
        ui.horizontal_wrapped(|ui| {
            widgets::pill(ui, kind_label(&c.kind, de), kind_tone(&c.kind));
            widgets::pill(
                ui,
                &c.status,
                if c.status == "open" {
                    Tone::Accent
                } else {
                    Tone::Ok
                },
            );
        });
        widgets::field(ui, f.t("Kunde", "Account"), |ui| {
            ui.label(self.name_of(&c.account));
        });
        widgets::field(ui, f.t("Fällig", "Due"), |ui| {
            ui.label(c.due.map(date_de).unwrap_or_else(|| "-".into()));
        });
        widgets::field(ui, f.t("Angelegt", "Created"), |ui| {
            ui.label(date_de(c.created_on));
        });
        if let Some(res) = &c.resolution {
            widgets::field(ui, f.t("Erledigung", "Resolution"), |ui| {
                ui.label(format!(
                    "{res} ({})",
                    c.resolved_by.clone().unwrap_or_default()
                ));
            });
        }
        ui.add_space(8.0);
        let id = c.id.to_string();
        ui.horizontal(|ui| {
            if c.status == "open" {
                if widgets::primary(ui, f.t("Lösen", "Resolve")).clicked() {
                    f.form("gain_cta_resolve", &[("id", &id)]);
                }
                if ui.button(f.t("Verwerfen", "Dismiss")).clicked() {
                    f.form("gain_cta_resolve", &[("id", &id), ("dismiss", "true")]);
                }
            }
            if ui.button(f.t("Kunde öffnen", "Open account")).clicked() {
                self.sel = Some(c.account.clone());
                f.goto(V_ACCOUNTS);
            }
        });
        ui.separator();
        self.select(&c.account);
        self.detail_360(ui, f)
    }
}

// ---------------- painted widgets ----------------

fn score_ring(ui: &mut Ui, score: i64, tone: Tone) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(84.0), Sense::hover());
    let t = theme::current(ui.ctx());
    let c = rect.center();
    let r = 34.0;
    ui.painter().circle_stroke(c, r, Stroke::new(9.0, t.sunken));
    let frac = (score.clamp(0, 100) as f32) / 100.0;
    let n = (64.0 * frac).ceil().max(2.0) as usize;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a =
                -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * frac * i as f32 / n as f32;
            c + Vec2::new(a.cos(), a.sin()) * r
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        pts,
        Stroke::new(9.0, theme::tone_solid(tone, t.dark)),
    ));
    ui.painter().text(
        c,
        Align2::CENTER_CENTER,
        score.to_string(),
        FontId::new(26.0, theme::bold()),
        t.text,
    );
}

fn band_bar(ui: &mut Ui, green: usize, yellow: usize, red: usize, de: bool) {
    let total = (green + yellow + red).max(1) as f32;
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 30.0), Sense::hover());
    let t = theme::current(ui.ctx());
    let mut x = rect.left();
    for (n, band) in [(green, "green"), (yellow, "yellow"), (red, "red")] {
        if n == 0 {
            continue;
        }
        let bw = rect.width() * n as f32 / total;
        let r = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(bw - 2.0, rect.height()));
        let (fg, bg) = theme::tone_colors(band_tone(band), t.dark);
        ui.painter().rect_filled(r, CornerRadius::same(6), bg);
        ui.painter().rect_filled(
            Rect::from_min_size(r.min, Vec2::new(4.0, r.height())),
            CornerRadius {
                nw: 6,
                sw: 6,
                ne: 0,
                se: 0,
            },
            theme::tone_solid(band_tone(band), t.dark),
        );
        ui.painter().text(
            r.left_center() + Vec2::new(12.0, 0.0),
            Align2::LEFT_CENTER,
            format!("{n} {}", band_label(band, de)),
            FontId::new(14.0, semibold()),
            fg,
        );
        x += bw;
    }
}

fn attention_item(ui: &mut Ui, r: &Row, reason: &str, de: bool) -> egui::Response {
    let t = theme::current(ui.ctx());
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 46.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), t.sunken);
    }
    let tone = band_tone(r.h.band);
    let (fg, bg) = theme::tone_colors(tone, t.dark);
    let badge = Rect::from_min_size(rect.left_top() + Vec2::new(6.0, 7.0), Vec2::new(40.0, 32.0));
    ui.painter().rect_filled(badge, CornerRadius::same(6), bg);
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        r.h.score.to_string(),
        FontId::new(16.0, theme::bold()),
        fg,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 56.0, rect.top() + 14.0),
        Align2::LEFT_CENTER,
        &r.a.name,
        FontId::new(14.5, semibold()),
        t.text,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 56.0, rect.top() + 33.0),
        Align2::LEFT_CENTER,
        format!("{}  |  {}", band_label(r.h.band, de), reason),
        FontId::proportional(13.0),
        t.text_weak,
    );
    ui.painter().text(
        Pos2::new(rect.right() - 8.0, rect.center().y),
        Align2::RIGHT_CENTER,
        r.a.arr_cents.map(eur).unwrap_or_default(),
        FontId::new(13.5, egui::FontFamily::Monospace),
        t.text,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn deadline_item(ui: &mut Ui, d: &Value, de: bool) -> egui::Response {
    let t = theme::current(ui.ctx());
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 40.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), t.sunken);
    }
    let days = d["days_left"].as_i64().unwrap_or(0);
    let tone = if days <= 30 {
        Tone::Danger
    } else if days <= 60 {
        Tone::Warn
    } else {
        Tone::Info
    };
    let text = format!("{days} d");
    let s = widgets::pill_size(ui.painter(), &text);
    widgets::paint_pill(
        ui.painter(),
        Pos2::new(rect.left() + 6.0, rect.center().y - s.y / 2.0),
        &text,
        tone,
        t.dark,
    );
    let kind = if d["kind"] == "notice" {
        if de {
            "Kündigungsfrist"
        } else {
            "notice date"
        }
    } else if de {
        "Vertragsende"
    } else {
        "term end"
    };
    let date = d["date"]
        .as_str()
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .map(date_de)
        .unwrap_or_default();
    ui.painter().text(
        Pos2::new(rect.left() + 70.0, rect.top() + 12.0),
        Align2::LEFT_CENTER,
        d["name"].as_str().unwrap_or(""),
        FontId::new(14.0, semibold()),
        t.text,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 70.0, rect.top() + 29.0),
        Align2::LEFT_CENTER,
        format!("{kind} {date}"),
        FontId::proportional(13.0),
        t.text_weak,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Daily active users as bars, seats as a line, weekends lighter.
fn usage_chart(ui: &mut Ui, days: &[Day], seats: Option<i64>, today: NaiveDate) {
    let t = theme::current(ui.ctx());
    let w = ui.available_width().max(200.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 120.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(6), t.sunken);
    let max = days
        .iter()
        .map(|d| d.active_users)
        .max()
        .unwrap_or(0)
        .max(seats.unwrap_or(0))
        .max(1) as f32;
    let plot = rect.shrink2(Vec2::new(8.0, 10.0));
    let n = 90.0;
    let bw = plot.width() / n;
    let start = today - Duration::days(89);
    let col = theme::tone_solid(Tone::Accent, t.dark);
    for d in days {
        let i = (d.date - start).num_days() as f32;
        if !(0.0..n).contains(&i) {
            continue;
        }
        let h = plot.height() * d.active_users as f32 / max;
        let r = Rect::from_min_max(
            Pos2::new(plot.left() + i * bw + 0.5, plot.bottom() - h),
            Pos2::new(plot.left() + (i + 1.0) * bw - 0.5, plot.bottom()),
        );
        ui.painter().rect_filled(r, CornerRadius::ZERO, col);
    }
    if let Some(s) = seats {
        let y = plot.bottom() - plot.height() * s as f32 / max;
        ui.painter().line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(1.5, theme::tone_solid(Tone::Warn, t.dark)),
        );
        ui.painter().text(
            Pos2::new(plot.right() - 2.0, y - 2.0),
            Align2::RIGHT_BOTTOM,
            format!("{s} seats"),
            FontId::proportional(12.0),
            t.text,
        );
    }
    if days.is_empty() {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "no usage data",
            FontId::proportional(13.0),
            t.text_weak,
        );
    }
    if let Some(p) = resp.hover_pos() {
        let i = ((p.x - plot.left()) / bw).floor() as i64;
        let day = start + Duration::days(i.clamp(0, 89));
        let v = days
            .iter()
            .find(|d| d.date == day)
            .map(|d| d.active_users)
            .unwrap_or(0);
        resp.on_hover_text(format!("{}: {v} active", date_de(day)));
    }
}

/// 12-month axis with notice dates and term ends.
fn timeline(ui: &mut Ui, deadlines: &[Value], today: NaiveDate, de: bool) {
    let t = theme::current(ui.ctx());
    let w = ui.available_width().max(300.0);
    let mut names: Vec<String> = Vec::new();
    for d in deadlines {
        let n = d["name"].as_str().unwrap_or("").to_string();
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let row_h = 28.0;
    let label_w = 230.0;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(w, 34.0 + row_h * names.len() as f32),
        Sense::hover(),
    );
    let axis = Rect::from_min_max(Pos2::new(rect.left() + label_w, rect.top()), rect.max);
    let span = 365.0;
    let xat = |d: NaiveDate| {
        axis.left() + axis.width() * ((d - today).num_days() as f32 / span).clamp(0.0, 1.0)
    };
    for m in 0..=12 {
        let d = today + Duration::days((m as f32 * 30.4) as i64);
        let x = xat(d);
        ui.painter().line_segment(
            [Pos2::new(x, rect.top() + 20.0), Pos2::new(x, rect.bottom())],
            Stroke::new(1.0, t.border),
        );
        ui.painter().text(
            Pos2::new(x + 3.0, rect.top() + 2.0),
            Align2::LEFT_TOP,
            d.format("%m/%y").to_string(),
            FontId::proportional(12.0),
            t.text_weak,
        );
    }
    for (i, n) in names.iter().enumerate() {
        let y = rect.top() + 34.0 + row_h * i as f32 + row_h / 2.0;
        ui.painter().text(
            Pos2::new(rect.left(), y),
            Align2::LEFT_CENTER,
            n,
            FontId::new(13.5, semibold()),
            t.text,
        );
        ui.painter().line_segment(
            [Pos2::new(axis.left(), y), Pos2::new(axis.right(), y)],
            Stroke::new(1.0, t.border),
        );
        for d in deadlines
            .iter()
            .filter(|d| d["name"].as_str() == Some(n.as_str()))
        {
            let Some(date) = d["date"]
                .as_str()
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            else {
                continue;
            };
            let notice = d["kind"] == "notice";
            let tone = if notice { Tone::Warn } else { Tone::Info };
            let x = xat(date);
            let c = Pos2::new(x, y);
            ui.painter()
                .circle_filled(c, 7.0, theme::tone_solid(tone, t.dark));
            ui.painter()
                .circle_stroke(c, 7.0, Stroke::new(2.0, t.surface));
            let label = if notice {
                if de {
                    "Frist"
                } else {
                    "notice"
                }
            } else if de {
                "Ende"
            } else {
                "end"
            };
            ui.painter().text(
                c + Vec2::new(10.0, 0.0),
                Align2::LEFT_CENTER,
                format!("{label} {}", date.format("%d.%m.")),
                FontId::proportional(12.5),
                t.text,
            );
        }
    }
}

impl SuiteApp for GainApp {
    fn name(&self) -> &'static str {
        "GainCraft"
    }
    fn id(&self) -> &'static str {
        gaincraft_core::APP
    }
    fn tone(&self) -> Tone {
        Tone::Teal
    }
    fn version(&self) -> &'static str {
        gaincraft_core::VERSION
    }
    fn tools(&self) -> Vec<Value> {
        tools()
    }
    fn call(&mut self, tool: &str, args: &Value) -> Result<Value, String> {
        let cmd = Command::from_tool(tool, args)?;
        dispatch(&mut self.ctx, cmd)
    }
    fn reload(&mut self) -> Result<(), String> {
        self.today = Local::now().date_naive();
        let g = &self.ctx.gain;
        self.stats = Some(g.stats(self.today).map_err(|e| e.to_string())?);
        self.weights = gaincraft_core::weights(g)?;
        if self.wdraft == Weights::default() || self.wdraft == self.weights {
            self.wdraft = self.weights;
        }
        self.ctas = g.ctas(None, true).map_err(|e| e.to_string())?;
        let mut rows = Vec::new();
        for a in g.accounts(false).map_err(|e| e.to_string())? {
            let h = health_of(g, &a, self.today)?;
            let mut snaps = g.snapshots(a.id, 12).map_err(|e| e.to_string())?;
            snaps.sort_by_key(|s| s.date);
            let mut trend: Vec<f32> = snaps.iter().map(|s| s.score as f32).collect();
            if snaps.last().map(|s| s.date) != Some(self.today) {
                trend.push(h.score as f32);
            }
            let open: Vec<String> = self
                .ctas
                .iter()
                .filter(|c| c.account == a.key && c.status == "open")
                .map(|c| c.kind.clone())
                .collect();
            rows.push(Row { a, h, trend, open });
        }
        rows.sort_by_key(|r| r.h.score);
        self.rows = rows;
        self.signals = g
            .signals(None, Some(self.today - Duration::days(180)))
            .map_err(|e| e.to_string())?;
        let dl = dispatch(
            &mut self.ctx,
            Command::Deadlines {
                within_days: 365,
                as_of: None,
            },
        )?;
        self.deadlines = dl["rows"].as_array().cloned().unwrap_or_default();
        // Open on the account that needs attention most.
        if self.sel.as_ref().is_none_or(|k| self.row(k).is_none()) {
            self.sel = self.rows.first().map(|r| r.a.key.clone());
        }
        self.load_detail()
    }
    fn nav(&self, f: &Frame) -> Vec<NavEntry> {
        let red = self.rows.iter().filter(|r| r.h.band == "red").count();
        let open = self.open_ctas().count();
        let overdue = self
            .open_ctas()
            .any(|c| c.due.is_some_and(|d| d < self.today));
        let soon = self
            .deadlines
            .iter()
            .filter(|d| d["kind"] == "notice" && d["days_left"].as_i64().unwrap_or(999) <= 60)
            .count();
        vec![
            NavEntry::new("\u{25C9}", f.t("Übersicht", "Overview"))
                .group(f.t("Kunden", "Customers")),
            NavEntry::new("\u{2630}", f.t("Kunden", "Accounts")).badge(red, Tone::Danger),
            NavEntry::new("\u{25A6}", f.t("Aufgaben-Cockpit", "CTA cockpit"))
                .badge(open, if overdue { Tone::Danger } else { Tone::Warn }),
            NavEntry::new("\u{21BB}", f.t("Verlängerungen", "Renewals")).badge(soon, Tone::Warn),
            NavEntry::new("\u{2709}", f.t("Signale", "Signals")),
            NavEntry::new("\u{21E9}", f.t("Nutzung importieren", "Import usage"))
                .group(f.t("Daten", "Data")),
            NavEntry::new("\u{2699}", f.t("Einstellungen", "Settings")),
        ]
    }
    fn view(&mut self, ui: &mut Ui, view: usize, f: &mut Frame) {
        egui::ScrollArea::vertical()
            .id_salt(("gain_view", view))
            .auto_shrink([false, false])
            .show(ui, |ui| match view {
                V_OVERVIEW => self.view_overview(ui, f),
                V_ACCOUNTS => self.view_accounts(ui, f),
                V_COCKPIT => self.view_cockpit(ui, f),
                V_RENEWALS => self.view_renewals(ui, f),
                V_SIGNALS => self.view_signals(ui, f),
                V_IMPORT => self.view_import(ui, f),
                V_SETTINGS => self.view_settings(ui, f),
                _ => self.view_overview(ui, f),
            });
    }
    fn has_detail(&self, view: usize) -> bool {
        match view {
            V_ACCOUNTS | V_RENEWALS | V_SIGNALS => self.sel.is_some(),
            V_COCKPIT => self.sel_cta.is_some(),
            _ => false,
        }
    }
    fn detail(&mut self, ui: &mut Ui, view: usize, f: &mut Frame) -> bool {
        if view == V_COCKPIT {
            self.detail_cta(ui, f)
        } else {
            self.detail_360(ui, f)
        }
    }
    fn top_actions(&mut self, ui: &mut Ui, f: &mut Frame) {
        if widgets::primary(ui, f.t("+ Kunde", "+ Account")).clicked() {
            f.form("gain_account_add", &[]);
        }
    }
    fn bus(&self) -> &Bus {
        &self.ctx.bus
    }
    fn wiring(&self) -> Wiring {
        (
            vec![
                (
                    "contract.executed",
                    "contractcraft: ARR, contract number, notice date and term end",
                ),
                (
                    "signal.negative",
                    "deskcraft / formcraft: unhappy customer, lowers sentiment",
                ),
            ],
            vec![],
        )
    }
    fn drain_tool(&self) -> Option<&'static str> {
        Some("gain_inbox_drain")
    }
    fn prefill(&self) -> Vec<(String, String)> {
        self.sel
            .iter()
            .map(|k| ("account".to_string(), k.clone()))
            .collect()
    }
    fn status_info(&self) -> String {
        format!("{}  |  {} accounts", self.db, self.rows.len())
    }
    fn smoke_tool(&self) -> &'static str {
        "gain_status"
    }
}

fn default_db() -> PathBuf {
    if let Ok(p) = std::env::var("GAINCRAFT_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("gaincraft")
        .join("gaincraft.db")
}

fn open(db: &Path) -> Result<GainApp, String> {
    let gain = Gain::open(db).map_err(|e| e.to_string())?;
    let actor = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "desktop".into());
    let ctx = Ctx {
        gain,
        confinement: Confinement(None),
        bus: Bus::new(Bus::default_root()),
        actor,
    };
    Ok(GainApp::new(ctx, db.display().to_string()))
}

fn main() -> std::process::ExitCode {
    app::main_with(default_db(), open, "gain_demo_seed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_biz_ui::app::Shell;
    use egui_kittest::Harness;

    fn shell(tag: &str) -> Shell<GainApp> {
        let dir = std::env::temp_dir().join(format!("gaincraft-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = Ctx {
            gain: Gain::open_in_memory().unwrap(),
            confinement: Confinement(None),
            bus: Bus::new(dir.join("bus")),
            actor: "tester".into(),
        };
        let mut app = GainApp::new(ctx, ":memory:".into());
        app.call("gain_demo_seed", &json!({})).unwrap();
        Shell::new(app, None)
    }

    #[test]
    fn every_tool_has_a_place_in_the_window() {
        craft_biz_ui::coverage::check(&tools(), COVERAGE).unwrap();
    }

    #[test]
    fn every_view_renders_light_and_dark_without_panic() {
        for dark in [false, true] {
            let mut sh = shell(if dark { "dark" } else { "light" });
            sh.app.select("mockinger.example");
            sh.app.sel_cta = sh.app.ctas.first().map(|c| c.id.to_string());
            let mut h = Harness::builder()
                .with_size([1440.0, 900.0])
                .build_ui_state(|ui, sh: &mut Shell<GainApp>| sh.show(ui), sh);
            for v in 0..7 {
                h.state_mut().view = v;
                h.run_steps(3);
            }
            if dark {
                craft_biz_ui::theme::apply(&h.ctx, true);
                h.run_steps(2);
            }
            assert!(h.state().last_error.is_none(), "{:?}", h.state().last_error);
        }
    }

    #[test]
    fn dropping_a_cta_on_done_opens_the_resolve_form() {
        let mut sh = shell("drop");
        sh.view = V_COCKPIT;
        let id = sh
            .app
            .ctas
            .iter()
            .find(|c| c.status == "open")
            .map(|c| c.id.to_string())
            .unwrap();
        let mut h = Harness::builder()
            .with_size([1600.0, 900.0])
            .build_ui_state(|ui, sh: &mut Shell<GainApp>| sh.show(ui), sh);
        h.run_steps(3);
        let card = kanban::card_rect(&h.ctx, "gain_cockpit", &id).unwrap();
        let done = kanban::cell_rect(&h.ctx, "gain_cockpit", "done", "").unwrap();
        h.hover_at(card.center());
        h.run_steps(1);
        h.drag_at(card.center());
        h.run_steps(1);
        for i in 1..=8 {
            h.hover_at(card.center() + (done.center() - card.center()) * (i as f32 / 8.0));
            h.run_steps(1);
        }
        h.drop_at(done.center());
        h.run_steps(3);
        assert_eq!(h.state().palette().form_tool(), Some("gain_cta_resolve"));
        assert_eq!(h.state().palette().form_value("id"), Some(id));
    }

    #[test]
    fn actions_go_through_dispatch_and_reload() {
        let mut sh = shell("act");
        let before = sh.app.open_ctas().count();
        let id = sh.app.open_ctas().next().map(|c| c.id).unwrap();
        sh.run_tool(
            "gain_cta_resolve",
            json!({ "id": id, "note": "Called the owner (test)" }),
        );
        assert_eq!(
            sh.app.open_ctas().count(),
            before - 1,
            "view data reloaded after the call"
        );
        assert_eq!(
            sh.app
                .ctas
                .iter()
                .find(|c| c.id == id)
                .and_then(|c| c.resolved_by.clone())
                .as_deref(),
            Some("tester")
        );
        sh.run_tool("gain_cta_resolve", json!({ "id": id }));
        assert!(
            sh.last_error.as_deref().unwrap_or("").contains("note"),
            "missing note is reported, not swallowed"
        );
        sh.run_tool(
            "gain_weights",
            json!({ "set": { "adoption": 0.5, "recency": 0.3, "sentiment": 0.2 } }),
        );
        assert_eq!(sh.app.weights.adoption, 0.5);
    }

    #[test]
    fn import_preview_refuses_personal_columns_and_lists_unknown_accounts() {
        let sh = shell("imp");
        let dir = std::env::temp_dir().join(format!("gaincraft-app-imp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.csv");
        std::fs::write(
            &bad,
            "account;date;user_email;active_users\nx;01.11.2026;a@b;1\n",
        )
        .unwrap();
        assert!(sh.app.preview(&bad).unwrap_err().contains("per-person"));
        let good = dir.join("good.csv");
        std::fs::write(
            &good,
            "account;date;active_users\nmockinger.example;02.11.2026;3\nnew.example;02.11.2026;1\n",
        )
        .unwrap();
        let p = sh.app.preview(&good).unwrap();
        assert_eq!(
            (p.rows.len(), p.accounts, p.unknown.clone()),
            (2, 2, vec!["new.example".to_string()])
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
