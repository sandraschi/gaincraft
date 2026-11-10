//! `gaincraft-app`: Native eframe/egui desktop workstation for GainCraft.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::Utc;
use craft_biz::bus::Bus;
use craft_biz::confine::Confinement;
use eframe::egui::{self, Color32, RichText};
use gaincraft_core::{dispatch, Command, Ctx};
use gaincraft_store::{Account, Cta, Gain};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewFilter {
    Accounts,
    Ctas,
    Deadlines,
}

struct App {
    ctx: Ctx,
    view: ViewFilter,
    accounts: Vec<Account>,
    scores: HashMap<String, (i64, String)>, // key -> (score, band)
    ctas: Vec<Cta>,
    selected_account: Option<Account>,
    selected_account_ctas: Vec<Cta>,
    resolve_note: String,
    status_msg: String,
}

impl App {
    fn new(ctx: Ctx) -> Self {
        let mut app = Self {
            ctx,
            view: ViewFilter::Accounts,
            accounts: Vec::new(),
            scores: HashMap::new(),
            ctas: Vec::new(),
            selected_account: None,
            selected_account_ctas: Vec::new(),
            resolve_note: String::new(),
            status_msg: "Ready".to_string(),
        };
        app.reload();
        app
    }

    fn reload(&mut self) {
        if let Ok(accs) = self.ctx.gain.accounts(false) {
            self.accounts = accs;
            if self.selected_account.is_none() && !self.accounts.is_empty() {
                let first = self.accounts[0].clone();
                self.select_account(first);
            }
        }

        if let Ok(res) = dispatch(&mut self.ctx, Command::HealthAll { as_of: None }) {
            if let Some(arr) = res.get("accounts").and_then(|v| v.as_array()) {
                let mut map = HashMap::new();
                for a in arr {
                    let k = a.get("key").and_then(|v| v.as_str()).unwrap_or("");
                    let score = a.pointer("/health/score").and_then(|v| v.as_i64()).unwrap_or(0);
                    let band = a.pointer("/health/band").and_then(|v| v.as_str()).unwrap_or("yellow");
                    map.insert(k.to_string(), (score, band.to_string()));
                }
                self.scores = map;
            }
        }

        if let Ok(ctas) = self.ctx.gain.ctas(None, false) {
            self.ctas = ctas;
        }

        if let Some(ref a) = self.selected_account {
            let ac = a.clone();
            self.select_account(ac);
        }
    }

    fn select_account(&mut self, a: Account) {
        if let Ok(ctas) = self.ctx.gain.ctas(Some(a.id), false) {
            self.selected_account_ctas = ctas;
        }
        self.selected_account = Some(a);
    }

    fn resolve_cta(&mut self, cta_id: i64, dismiss: bool) {
        let note = if self.resolve_note.trim().is_empty() {
            "Resolved in GainCraft workstation".to_string()
        } else {
            self.resolve_note.trim().to_string()
        };

        match dispatch(
            &mut self.ctx,
            Command::CtaResolve {
                id: cta_id,
                note,
                dismiss,
            },
        ) {
            Ok(_) => {
                self.status_msg = format!("CTA #{} resolved", cta_id);
                self.resolve_note.clear();
                self.reload();
            }
            Err(e) => {
                self.status_msg = format!("Resolve error: {e}");
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let (green, yellow, red) = self.scores.values().fold((0, 0, 0), |(g, y, r), (_, b)| {
            match b.as_str() {
                "green" => (g + 1, y, r),
                "yellow" => (g, y + 1, r),
                "red" => (g, y, r + 1),
                _ => (g, y, r),
            }
        });

        ui.horizontal(|ui| {
            ui.heading(
                RichText::new("📈 GainCraft Customer Success 360")
                    .color(Color32::from_rgb(56, 189, 248))
                    .strong(),
            );
            ui.separator();

            if ui
                .selectable_label(self.view == ViewFilter::Accounts, "🏢 Accounts")
                .clicked()
            {
                self.view = ViewFilter::Accounts;
            }
            if ui
                .selectable_label(
                    self.view == ViewFilter::Ctas,
                    format!("🎯 CTA Cockpit ({})", self.ctas.len()),
                )
                .clicked()
            {
                self.view = ViewFilter::Ctas;
            }
            if ui
                .selectable_label(self.view == ViewFilter::Deadlines, "📅 Deadlines")
                .clicked()
            {
                self.view = ViewFilter::Deadlines;
            }

            ui.separator();
            ui.colored_label(Color32::from_rgb(46, 204, 113), format!("● {green} Green"));
            ui.colored_label(Color32::from_rgb(241, 196, 15), format!("● {yellow} Yellow"));
            ui.colored_label(Color32::from_rgb(231, 76, 60), format!("● {red} Red"));

            ui.separator();
            if ui.button("🔄 Refresh").clicked() {
                self.reload();
                self.status_msg = "Reloaded".to_string();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("Actor: {}", self.ctx.actor));
            });
        });

        ui.separator();

        match self.view {
            ViewFilter::Accounts => {
                ui.horizontal(|ui| {
                    // Left: Accounts Table
                    ui.vertical(|ui| {
                        ui.set_width(360.0);
                        ui.heading(RichText::new("Accounts Portfolio").size(15.0));
                        ui.separator();

                        let mut to_select = None;
                        egui::ScrollArea::vertical()
                            .id_salt("acc_scroll")
                            .show(ui, |ui| {
                                for a in &self.accounts {
                                    let is_sel = self
                                        .selected_account
                                        .as_ref()
                                        .map(|sa| sa.id == a.id)
                                        .unwrap_or(false);

                                    let (score, band) = self
                                        .scores
                                        .get(&a.key)
                                        .cloned()
                                        .unwrap_or((50, "yellow".to_string()));

                                    let band_color = match band.as_str() {
                                        "green" => Color32::from_rgb(46, 204, 113),
                                        "yellow" => Color32::from_rgb(241, 196, 15),
                                        _ => Color32::from_rgb(231, 76, 60),
                                    };

                                    let arr_str = a
                                        .arr_cents
                                        .map(|c| format!("€{:.0}k", (c as f64) / 100_000.0))
                                        .unwrap_or_else(|| "-".to_string());

                                    ui.group(|ui| {
                                        if ui
                                            .selectable_label(
                                                is_sel,
                                                format!("{}: {}", a.key, a.name),
                                            )
                                            .clicked()
                                        {
                                            to_select = Some(a.clone());
                                        }
                                        ui.horizontal(|ui| {
                                            ui.colored_label(
                                                band_color,
                                                format!("[{score} {band}]"),
                                            );
                                            ui.label(format!("ARR: {arr_str}"));
                                            if let Some(owner) = &a.owner {
                                                ui.label(owner);
                                            }
                                        });
                                    });
                                    ui.add_space(2.0);
                                }
                            });

                        if let Some(a) = to_select {
                            self.select_account(a);
                        }
                    });

                    ui.separator();

                    // Right: Customer 360 Drawer
                    ui.vertical(|ui| {
                        if let Some(ref a) = self.selected_account {
                            let (score, band) = self
                                .scores
                                .get(&a.key)
                                .cloned()
                                .unwrap_or((50, "yellow".to_string()));

                            ui.horizontal(|ui| {
                                ui.heading(
                                    RichText::new(format!("{}: {}", a.key, a.name)).size(18.0),
                                );
                                ui.separator();
                                let band_color = match band.as_str() {
                                    "green" => Color32::from_rgb(46, 204, 113),
                                    "yellow" => Color32::from_rgb(241, 196, 15),
                                    _ => Color32::from_rgb(231, 76, 60),
                                };
                                ui.colored_label(
                                    band_color,
                                    RichText::new(format!("Health Score: {score}/100 ({band})"))
                                        .size(15.0)
                                        .strong(),
                                );
                            });

                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "Domain: {}",
                                    a.domain.as_deref().unwrap_or("-")
                                ));
                                ui.label(format!(
                                    "Owner: {}",
                                    a.owner.as_deref().unwrap_or("-")
                                ));
                                ui.label(format!("Seats: {}", a.seats.unwrap_or(0)));
                                if let Some(end) = a.term_end {
                                    ui.label(format!("Term End: {end}"));
                                }
                            });

                            ui.separator();

                            ui.heading(RichText::new("Active Calls-To-Action (CTAs):").size(15.0));
                            let mut resolve_target = None;
                            egui::ScrollArea::vertical()
                                .id_salt("cta_scroll")
                                .max_height(250.0)
                                .show(ui, |ui| {
                                    if self.selected_account_ctas.is_empty() {
                                        ui.label(RichText::new("No active CTAs for this account.").italics());
                                    } else {
                                        for cta in &self.selected_account_ctas {
                                            ui.group(|ui| {
                                                ui.horizontal(|ui| {
                                                    ui.colored_label(
                                                        Color32::from_rgb(230, 126, 34),
                                                        format!("[{}]", cta.kind),
                                                    );
                                                    ui.label(RichText::new(&cta.title).strong());
                                                    if let Some(due) = cta.due {
                                                        ui.label(format!("Due: {due}"));
                                                    }
                                                });
                                                ui.horizontal(|ui| {
                                                    if ui.button("✅ Resolve").clicked() {
                                                        resolve_target = Some((cta.id, false));
                                                    }
                                                    if ui.button("❌ Dismiss").clicked() {
                                                        resolve_target = Some((cta.id, true));
                                                    }
                                                });
                                            });
                                            ui.add_space(3.0);
                                        }
                                    }
                                });

                            if let Some((cid, dismiss)) = resolve_target {
                                self.resolve_cta(cid, dismiss);
                            }
                        } else {
                            ui.centered_and_justified(|ui| {
                                ui.label("Select an account to inspect Customer 360.");
                            });
                        }
                    });
                });
            }

            ViewFilter::Ctas => {
                ui.heading(RichText::new("Company-Wide CTA Cockpit").size(18.0));
                ui.separator();

                let mut to_resolve = None;
                egui::ScrollArea::vertical()
                    .id_salt("all_ctas_scroll")
                    .show(ui, |ui| {
                        for cta in &self.ctas {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&cta.account).strong());
                                    ui.colored_label(
                                        Color32::from_rgb(230, 126, 34),
                                        format!("[{}]", cta.kind),
                                    );
                                    ui.label(&cta.title);
                                    if let Some(due) = cta.due {
                                        ui.label(format!("Due: {due}"));
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.button("Dismiss").clicked() {
                                                to_resolve = Some((cta.id, true));
                                            }
                                            if ui.button("Resolve").clicked() {
                                                to_resolve = Some((cta.id, false));
                                            }
                                        },
                                    );
                                });
                            });
                            ui.add_space(3.0);
                        }
                    });

                if let Some((cid, dismiss)) = to_resolve {
                    self.resolve_cta(cid, dismiss);
                }
            }

            ViewFilter::Deadlines => {
                ui.heading(RichText::new("Upcoming Renewal Deadlines (Next 90 Days)").size(18.0));
                ui.separator();

                let today_dt = Utc::now().date_naive();
                egui::Grid::new("deadlines_grid")
                    .striped(true)
                    .spacing([20.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Account").strong());
                        ui.label(RichText::new("Term End").strong());
                        ui.label(RichText::new("Notice Deadline").strong());
                        ui.label(RichText::new("ARR").strong());
                        ui.label(RichText::new("Owner").strong());
                        ui.end_row();

                        for a in &self.accounts {
                            if let Some(end) = a.term_end {
                                let days = (end - today_dt).num_days();
                                if days >= 0 && days <= 90 {
                                    ui.label(&a.name);
                                    ui.label(format!("{end} ({days}d)"));
                                    ui.label(
                                        a.last_notice
                                            .map(|d| d.to_string())
                                            .unwrap_or_else(|| "-".to_string()),
                                    );
                                    let arr = a
                                        .arr_cents
                                        .map(|c| format!("€{:.0}", (c as f64) / 100.0))
                                        .unwrap_or_else(|| "-".to_string());
                                    ui.label(arr);
                                    ui.label(a.owner.as_deref().unwrap_or("-"));
                                    ui.end_row();
                                }
                            }
                        }
                    });
            }
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label(format!(
                "Accounts: {} | Active CTAs: {} | Selected: {}",
                self.accounts.len(),
                self.ctas.len(),
                self.selected_account.as_ref().map(|a| a.name.as_str()).unwrap_or("-")
            ));
            ui.separator();
            ui.label(&self.status_msg);
        });
    }
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

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--smoke") {
        println!("smoke: ok (gaincraft-app)");
        return Ok(());
    }

    let db_path = default_db();
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let gain = match Gain::open(&db_path) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Failed to open gain database at {}: {e}", db_path.display());
            std::process::exit(1);
        }
    };

    let ctx = Ctx {
        gain,
        confinement: Confinement(None),
        bus: Bus::new(Bus::default_root()),
        actor: std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "desktop".to_string()),
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("GainCraft Customer Success 360"),
        ..Default::default()
    };

    eframe::run_native(
        "GainCraft",
        native_options,
        Box::new(|_cc| Ok(Box::new(App::new(ctx)))),
    )
}
