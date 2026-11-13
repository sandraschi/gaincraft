use super::*;
use serde_json::json;

fn ctx(tag: &str) -> Ctx {
    let dir = std::env::temp_dir().join(format!("gaincraft-core-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Ctx {
        gain: Gain::open_in_memory().unwrap(),
        confinement: Confinement(None),
        bus: Bus::new(dir.join("bus")),
        actor: "tester".into(),
    }
}

fn run(c: &mut Ctx, tool: &str, args: Value) -> Result<Value, String> {
    let cmd = Command::from_tool(tool, &args)?;
    dispatch(c, cmd)
}

fn d(s: &str) -> NaiveDate {
    parse_date(s).unwrap()
}

fn band_of(all: &Value, key: &str) -> String {
    all["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["account"] == key)
        .map(|r| r["band"].as_str().unwrap().to_string())
        .unwrap()
}

#[test]
fn demo_has_one_account_per_band_and_ctas_open_once() {
    let mut c = ctx("demo");
    let seed = run(&mut c, "gain_demo_seed", json!({})).unwrap();
    assert_eq!(seed["demo"], true);
    let all = health_all(&c.gain, d("2026-11-09")).unwrap();
    assert_eq!(band_of(&all, "mock-software-gmbh-demo"), "green", "{all}");
    assert_eq!(band_of(&all, "mockingbird-inc.-demo"), "yellow", "{all}");
    assert_eq!(band_of(&all, "hannes-mockinger-e.u.-demo"), "red", "{all}");
    assert_eq!(
        all["rows"][0]["account"], "hannes-mockinger-e.u.-demo",
        "worst first"
    );
    assert_eq!(
        all["new_ctas"], 0,
        "seed already raised them; a re-run must not duplicate"
    );
    let kinds = |k: &str| -> Vec<String> {
        let a = c.gain.account(k).unwrap();
        c.gain
            .ctas(Some(a.id), false)
            .unwrap()
            .into_iter()
            .map(|x| x.kind)
            .collect()
    };
    assert!(kinds("mock-software.example").contains(&"renewal".to_string()));
    assert!(kinds("mockingbird.example").contains(&"adoption_drop".to_string()));
    let m = kinds("mockinger.example");
    assert!(
        m.contains(&"low_health".to_string()) && m.contains(&"negative_signal".to_string()),
        "{m:?}"
    );
    assert!(
        run(&mut c, "gain_demo_seed", json!({})).is_err(),
        "only on an empty db"
    );
}

#[test]
fn telemetry_header_refuses_personal_columns() {
    for bad in [
        "account;date;user_email;active_users",
        "account;date;Mitarbeiter;active_users",
        "account,date,active_users,ip",
        "account;date;active_users;login_name",
    ] {
        let err = parse_telemetry(&format!("{bad}\nx;01.11.2026;1;2\n")).unwrap_err();
        assert!(
            err.contains("per-person") || err.contains("ArbVG"),
            "{bad}: {err}"
        );
    }
    assert!(
        parse_telemetry("account;date;active_users;seats\nx;01.11.2026;1;2\n")
            .unwrap_err()
            .contains("unknown column")
    );
    assert!(parse_telemetry("account;active_users\nx;1\n")
        .unwrap_err()
        .contains("missing column date"));
    let rows = parse_telemetry("\u{feff}account,date,active_users,sessions\n# comment\nmock.example,2026-11-02,4,9\nmock.example,03.11.2026,5,\n").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[1].1,
        Day {
            date: d("2026-11-03"),
            active_users: 5,
            sessions: 0,
            key_actions: 0
        }
    );
    assert!(
        parse_telemetry("account;date;active_users\nx;01.11.2026;-3\n")
            .unwrap_err()
            .contains("line 2")
    );
}

fn product_db(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("gaincraft-pull-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("product.db");
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute_batch(
        "CREATE TABLE events (customer TEXT, day TEXT, user_id INTEGER);
         INSERT INTO events VALUES
           ('mockova.example','2026-11-02',1),
           ('mockova.example','2026-11-02',2),
           ('mockova.example','2026-11-03',1),
           ('mockova.example','2026-11-03',2),
           ('mockova.example','2026-11-03',3);",
    )
    .unwrap();
    p
}

#[test]
fn usage_pull_aggregates_a_product_database_read_only() {
    let mut c = ctx("pull");
    run(
        &mut c,
        "gain_account_add",
        json!({ "name": "Anna Mockova KG", "domain": "mockova.example" }),
    )
    .unwrap();
    let p = product_db("ok");
    let q = "SELECT customer AS account, day AS date, COUNT(DISTINCT user_id) AS active_users \
             FROM events GROUP BY customer, day ORDER BY day";
    let dry = run(
        &mut c,
        "gain_usage_pull",
        json!({ "path": p.to_string_lossy(), "query": q, "dry_run": true }),
    )
    .unwrap();
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["source"]["rows_read"], 2);
    assert_eq!(dry["source"]["kind"], "sqlite");
    let a = c.gain.account("mockova.example").unwrap();
    assert!(
        c.gain
            .days(a.id, d("2026-11-01"), d("2026-11-30"))
            .unwrap()
            .is_empty(),
        "a dry run writes nothing"
    );
    let got = run(
        &mut c,
        "gain_usage_pull",
        json!({ "path": p.to_string_lossy(), "query": q }),
    )
    .unwrap();
    assert_eq!(got["rows"][0]["inserted"], 2);
    let days = c.gain.days(a.id, d("2026-11-01"), d("2026-11-30")).unwrap();
    assert_eq!(days.len(), 2);
    assert_eq!(
        days.iter()
            .find(|x| x.date == d("2026-11-02"))
            .unwrap()
            .active_users,
        2
    );
    assert_eq!(
        days.iter()
            .find(|x| x.date == d("2026-11-03"))
            .unwrap()
            .active_users,
        3
    );
    let again = run(
        &mut c,
        "gain_usage_pull",
        json!({ "path": p.to_string_lossy(), "query": q }),
    )
    .unwrap();
    assert_eq!(
        again["rows"][0]["replaced"], 2,
        "re-running a day replaces it"
    );
    // The product database was opened read-only and is untouched.
    let probe = rusqlite::Connection::open(&p).unwrap();
    let n: i64 = probe
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 5);
}

#[test]
fn usage_pull_refuses_writes_personal_columns_and_other_sources() {
    let mut c = ctx("pull-bad");
    run(
        &mut c,
        "gain_account_add",
        json!({ "name": "Anna Mockova KG", "domain": "mockova.example" }),
    )
    .unwrap();
    let p = product_db("bad");
    let path = p.to_string_lossy().to_string();
    for q in [
        "DELETE FROM events",
        "PRAGMA writable_schema = ON",
        "SELECT 1; DROP TABLE events",
    ] {
        let err = run(
            &mut c,
            "gain_usage_pull",
            json!({ "path": path, "query": q }),
        )
        .unwrap_err();
        assert!(
            err.contains("SELECT") || err.contains("single statement"),
            "{q}: {err}"
        );
    }
    let err = run(
        &mut c,
        "gain_usage_pull",
        json!({
            "path": path,
            "query": "SELECT customer AS account, day AS date, user_id AS email FROM events"
        }),
    )
    .unwrap_err();
    assert!(err.contains("per-person"), "{err}");
    let err = run(
        &mut c,
        "gain_usage_pull",
        json!({
            "path": path,
            "query": "SELECT customer AS account, day AS date FROM events"
        }),
    )
    .unwrap_err();
    assert!(err.contains("missing column active_users"), "{err}");
    let err = run(
        &mut c,
        "gain_usage_pull",
        json!({
            "source": "postgres",
            "path": path,
            "query": "SELECT 1 AS account, '2026-11-02' AS date, 1 AS active_users"
        }),
    )
    .unwrap_err();
    assert!(err.contains("not supported"), "{err}");
    let probe = rusqlite::Connection::open(&p).unwrap();
    let n: i64 = probe
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 5, "nothing was written to the product database");
}

fn product_db_days(tag: &str, days: i64) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("gaincraft-tick-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("product.db");
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute_batch("CREATE TABLE events (customer TEXT, day TEXT, user_id INTEGER);")
        .unwrap();
    let today = Local::now().date_naive();
    let mut ins = c.prepare("INSERT INTO events VALUES (?1, ?2, ?3)").unwrap();
    for i in 0..days {
        let day = (today - Duration::days(i)).to_string();
        for user in 1..=6 {
            ins.execute(rusqlite::params!["mockova.example", day, user])
                .unwrap();
        }
    }
    drop(ins);
    p
}

#[test]
fn tick_pulls_health_records_the_run_and_writes_a_digest() {
    let mut c = ctx("tick");
    run(
        &mut c,
        "gain_account_add",
        json!({
            "name": "Anna Mockova KG", "domain": "mockova.example",
            "seats": 8, "arr": "9.600,00"
        }),
    )
    .unwrap();
    run(
        &mut c,
        "gain_account_update",
        json!({
            "account": "mockova.example",
            "last_notice": (Local::now().date_naive() + Duration::days(30)).format("%d.%m.%Y").to_string()
        }),
    )
    .unwrap();
    let a = c.gain.account("mockova.example").unwrap();
    // A previous reading that was red: today's green must raise a change alert.
    c.gain
        .snapshot(
            a.id,
            Local::now().date_naive() - Duration::days(1),
            10,
            "red",
            "{}",
        )
        .unwrap();
    let p = product_db_days("ok", 30);
    let dir = std::env::temp_dir().join(format!("gaincraft-tick-rep-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let report = dir.join("digest.md");
    let q = "SELECT customer AS account, day AS date, COUNT(DISTINCT user_id) AS active_users \
             FROM events GROUP BY customer, day";
    let out = run(
        &mut c,
        "gain_tick",
        json!({
            "path": p.to_string_lossy(),
            "query": q,
            "report": report.to_string_lossy(),
        }),
    )
    .unwrap();
    assert_eq!(out["run"]["status"], "ok");
    assert_eq!(out["run"]["rows_read"], 30);
    assert_eq!(out["run"]["accounts"], 1);
    assert_eq!(out["health"]["accounts"], 1);
    let changed = out["alerts"]["changed"].as_array().unwrap();
    assert!(
        changed
            .iter()
            .any(|v| v["to"] == "green" && v["from"] == "red"),
        "{out}"
    );
    assert_eq!(out["alerts"]["renewals"].as_array().unwrap().len(), 1);
    let text = std::fs::read_to_string(&report).unwrap();
    assert!(text.contains("# GainCraft daily digest"), "{text}");
    assert!(text.contains("Health changes"), "{text}");
    let recorded = c.gain.last_sync_run().unwrap().unwrap();
    assert_eq!(recorded.status, "ok");
    assert_eq!(
        (recorded.rows_read, recorded.wrote, recorded.accounts),
        (30, true, 1)
    );
    assert!(recorded.alerts >= 2, "change + renewal: {recorded:?}");
    // A health-only tick is recorded too and does not duplicate CTAs.
    let before = c.gain.ctas(None, true).unwrap().len();
    let again = run(&mut c, "gain_tick", json!({})).unwrap();
    assert_eq!(again["run"]["source"], "none");
    assert_eq!(again["run"]["rows_read"], 0);
    assert_eq!(c.gain.ctas(None, true).unwrap().len(), before);
}

#[test]
fn failed_tick_is_recorded_and_writes_nothing() {
    let mut c = ctx("tick-fail");
    run(
        &mut c,
        "gain_account_add",
        json!({ "name": "Anna Mockova KG", "domain": "mockova.example", "seats": 8 }),
    )
    .unwrap();
    let p = product_db_days("fail", 3);
    let err = run(
        &mut c,
        "gain_tick",
        json!({ "path": p.to_string_lossy(), "query": "DELETE FROM events" }),
    )
    .unwrap_err();
    assert!(err.contains("SELECT"), "{err}");
    let recorded = c.gain.last_sync_run().unwrap().unwrap();
    assert_eq!(recorded.status, "error");
    assert!(recorded.error.unwrap().contains("SELECT"));
    assert_eq!(
        (recorded.rows_read, recorded.accounts, recorded.alerts),
        (0, 0, 0)
    );
    let a = c.gain.account("mockova.example").unwrap();
    assert!(c
        .gain
        .days(
            a.id,
            Local::now().date_naive() - Duration::days(10),
            Local::now().date_naive()
        )
        .unwrap()
        .is_empty());
}

#[test]
fn alerts_filter_by_since_and_name_the_reasons() {
    let mut c = ctx("alerts");
    let today = Local::now().date_naive();
    run(
        &mut c,
        "gain_account_add",
        json!({ "name": "Anna Mockova KG", "domain": "mockova.example", "seats": 8 }),
    )
    .unwrap();
    run(
        &mut c,
        "gain_cta_add",
        json!({ "account": "mockova.example", "title": "Call the owner", "due": today.to_string() }),
    )
    .unwrap();
    let a = c.gain.account("mockova.example").unwrap();
    let days: Vec<Day> = (0..30)
        .map(|i| Day {
            date: today - Duration::days(i),
            active_users: 6,
            sessions: 8,
            key_actions: 20,
        })
        .collect();
    c.gain.upsert_days(a.id, &days).unwrap();
    c.gain
        .snapshot(a.id, today - Duration::days(1), 10, "red", "{}")
        .unwrap();
    let all = run(&mut c, "gain_alerts", json!({ "since": today.to_string() })).unwrap();
    assert!(
        all["new_ctas"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["title"] == "Call the owner"),
        "{all}"
    );
    assert!(
        all["changed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["from"] == "red"),
        "{all}"
    );
    let tomorrow = (today + Duration::days(1)).to_string();
    let none = run(&mut c, "gain_alerts", json!({ "since": tomorrow })).unwrap();
    assert!(none["new_ctas"].as_array().unwrap().is_empty());
    assert_eq!(none["since"], tomorrow);
}

#[test]
fn import_resolves_every_account_before_writing() {
    let mut c = ctx("import");
    run(
        &mut c,
        "gain_account_add",
        json!({ "name": "Anna Mockova KG", "domain": "mockova.example" }),
    )
    .unwrap();
    let dir = std::env::temp_dir().join(format!("gaincraft-imp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("bad.csv");
    std::fs::write(
        &bad,
        "account;date;active_users\nmockova.example;02.11.2026;3\nunknown.example;02.11.2026;1\n",
    )
    .unwrap();
    assert!(run(
        &mut c,
        "gain_telemetry_import",
        json!({ "path": bad.to_string_lossy() })
    )
    .is_err());
    let a = c.gain.account("mockova.example").unwrap();
    assert!(
        c.gain
            .days(a.id, d("2026-01-01"), d("2026-12-31"))
            .unwrap()
            .is_empty(),
        "nothing written"
    );
    let good = dir.join("good.csv");
    std::fs::write(&good, "account;date;active_users;sessions;key_actions\nmockova.example;02.11.2026;3;7;20\nAnna Mockova KG;03.11.2026;4;8;21\n").unwrap();
    let r = run(
        &mut c,
        "gain_telemetry_import",
        json!({ "path": good.to_string_lossy() }),
    )
    .unwrap();
    assert_eq!(r["rows"][0]["inserted"], 2);
    let again = run(
        &mut c,
        "gain_telemetry_import",
        json!({ "path": good.to_string_lossy() }),
    )
    .unwrap();
    assert_eq!(
        again["rows"][0]["replaced"], 2,
        "re-import replaces the day"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inbox_applies_contract_and_signal_and_rejects_others() {
    let mut c = ctx("inbox");
    run(&mut c, "gain_demo_seed", json!({})).unwrap();
    c.bus
        .emit("contractcraft", APP, "contract.executed", json!({ "contract_id": "C-2026-007", "counterparty": "Mockingbird Inc. (DEMO)", "annual_value_cents": 2_640_000, "renewal_months": 12, "term_end": "2027-12-31", "last_notice": "2027-09-30" }))
        .unwrap();
    c.bus.emit("deskcraft", APP, "signal.negative", json!({ "ticket_id": "D-9", "account": "mockinger.example", "email": "hannes@mockinger.example", "subject": "Schon wieder", "score": 14 })).unwrap();
    c.bus
        .emit(
            "formcraft",
            APP,
            "signal.negative",
            json!({ "response_id": "R-1", "email": "mia@mockerl.example", "score": 6 }),
        )
        .unwrap();
    c.bus
        .emit("deskcraft", APP, "ticket.escalated", json!({}))
        .unwrap();
    let r = run(&mut c, "gain_inbox_drain", json!({})).unwrap();
    assert_eq!(r["applied"].as_array().unwrap().len(), 3, "{r}");
    assert_eq!(r["rejected"].as_array().unwrap().len(), 1);
    let bird = c.gain.account("mockingbird.example").unwrap();
    assert_eq!(
        (
            bird.arr_cents,
            bird.contract_id.as_deref(),
            bird.last_notice
        ),
        (Some(2_640_000), Some("C-2026-007"), Some(d("2027-09-30")))
    );
    assert!(!bird.auto_created);
    let m = c.gain.account("mockinger.example").unwrap();
    let sig = c.gain.signals(Some(m.id), None).unwrap();
    assert!(
        sig.iter()
            .any(|s| s.score == 10 && s.reference.as_deref() == Some("D-9")),
        "score capped at 10"
    );
    let new = c.gain.account("mockerl.example").unwrap();
    assert!(new.auto_created, "unknown domain becomes a flagged account");
    assert!(
        run(&mut c, "gain_inbox_drain", json!({})).unwrap()["applied"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(c.bus.root());
}

#[test]
fn triage_moves_the_score_within_the_cap() {
    let mut c = ctx("triage");
    run(&mut c, "gain_demo_seed", json!({})).unwrap();
    let before = run(
        &mut c,
        "gain_health",
        json!({ "account": "mockingbird.example", "as_of": "09.11.2026" }),
    )
    .unwrap()["health"]["score"]
        .as_i64()
        .unwrap();
    assert!(run(&mut c, "gain_triage_set", json!({ "account": "mockingbird.example", "modifier": 1.5, "reason": "far too optimistic here" })).is_err());
    assert!(
        run(
            &mut c,
            "gain_triage_set",
            json!({ "account": "mockingbird.example", "modifier": 0.8, "reason": "short" })
        )
        .is_err(),
        "reason >= 10 chars"
    );
    let today_ = today();
    run(&mut c, "gain_triage_set", json!({ "account": "mockingbird.example", "modifier": 0.8, "reason": "champion left the company in October" })).unwrap();
    let after = run(
        &mut c,
        "gain_health",
        json!({ "account": "mockingbird.example", "as_of": today_.format("%Y-%m-%d").to_string() }),
    )
    .unwrap();
    assert_eq!(after["health"]["modifier"], 0.8);
    let _ = before;
}

#[test]
fn weights_and_money_are_validated() {
    let mut c = ctx("weights");
    assert!(run(
        &mut c,
        "gain_weights",
        json!({ "set": { "adoption": 0.5, "recency": 0.5, "sentiment": 0.5 } })
    )
    .is_err());
    let w = run(
        &mut c,
        "gain_weights",
        json!({ "set": { "adoption": 0.5, "recency": 0.3, "sentiment": 0.2 } }),
    )
    .unwrap();
    assert_eq!(w["weights"]["adoption"], 0.5);
    assert_eq!(weights(&c.gain).unwrap().recency, 0.3);
    assert!(
        run(
            &mut c,
            "gain_account_add",
            json!({ "name": "Float GmbH", "arr": 9600.0 })
        )
        .is_err(),
        "no floats for money"
    );
    let a = run(
        &mut c,
        "gain_account_add",
        json!({ "name": "Euro GmbH", "arr": "9.600,00" }),
    )
    .unwrap();
    assert_eq!(a["arr"], "9.600,00");
    assert!(
        Command::from_tool("gain.health_all", &json!({})).is_ok(),
        "dotted alias"
    );
}

#[test]
fn deadlines_list_notice_and_term_end() {
    let mut c = ctx("deadlines");
    run(&mut c, "gain_demo_seed", json!({})).unwrap();
    let r = run(
        &mut c,
        "gain_deadlines",
        json!({ "as_of": "09.11.2026", "within_days": 120 }),
    )
    .unwrap();
    let kinds: Vec<&str> = r["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["kind"].as_str())
        .collect();
    assert_eq!(kinds, vec!["notice", "term_end"], "{r}");
    assert_eq!(r["rows"][0]["days_left"], 21);
}

#[test]
fn every_tool_parses_its_minimal_arguments() {
    for t in tools() {
        let name = t["name"].as_str().unwrap();
        let req = t["inputSchema"]["required"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut args = serde_json::Map::new();
        for r in req {
            let k = r.as_str().unwrap();
            let v = match k {
                "id" | "score" => json!(3),
                _ => json!("x"),
            };
            args.insert(k.into(), v);
        }
        assert!(
            Command::from_tool(name, &Value::Object(args)).is_ok(),
            "{name}"
        );
    }
}

fn out_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("gaincraft-out-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn manual_cta_opens_with_a_unique_reference_and_resolves_like_any_other() {
    let mut c = ctx("manual-cta");
    run(&mut c, "gain_demo_seed", json!({})).unwrap();
    let a = run(
        &mut c,
        "gain_cta_add",
        json!({ "account": "mockinger.example", "title": "Send the webshop post-mortem", "due": "20.11.2026", "note": "promised in the call" }),
    )
    .unwrap();
    let b = run(
        &mut c,
        "gain_cta_add",
        json!({ "account": "mockinger.example", "title": "Second follow-up" }),
    )
    .unwrap();
    assert_eq!(a["kind"], "manual");
    assert_ne!(
        a["reference"], b["reference"],
        "two manual CTAs, two references"
    );
    assert_eq!(a["due"], "2026-11-20");
    assert!(a["params"]
        .as_str()
        .unwrap()
        .contains("promised in the call"));
    assert!(run(
        &mut c,
        "gain_cta_add",
        json!({ "account": "mockinger.example", "title": " " })
    )
    .is_err());
    let r = run(
        &mut c,
        "gain_cta_resolve",
        json!({ "id": a["id"], "note": "sent on Friday" }),
    )
    .unwrap();
    assert_eq!(r["status"], "resolved");
}

#[test]
fn generated_ctas_carry_their_values_for_translation() {
    let mut c = ctx("cta-params");
    run(&mut c, "gain_demo_seed", json!({})).unwrap();
    let ctas = c.gain.ctas(None, false).unwrap();
    let drop = ctas.iter().find(|x| x.kind == "adoption_drop").unwrap();
    let p: Value = serde_json::from_str(&drop.params).unwrap();
    assert!(p["pct"].as_i64().unwrap() > 40, "{p}");
    assert!(drop.title.contains(&format!("{}%", p["pct"])));
    let renewal = ctas.iter().find(|x| x.kind == "renewal").unwrap();
    let p: Value = serde_json::from_str(&renewal.params).unwrap();
    assert_eq!(p["date"], "30.11.2026");
}

#[test]
fn export_writes_csv_and_markdown_worst_first_and_does_not_overwrite() {
    let mut c = ctx("export");
    run(&mut c, "gain_demo_seed", json!({})).unwrap();
    let dir = out_dir("export");
    let csv = dir.join("health.csv");
    let r = run(
        &mut c,
        "gain_export",
        json!({ "path": csv.to_string_lossy(), "as_of": "09.11.2026" }),
    )
    .unwrap();
    assert_eq!(
        (r["format"].as_str(), r["rows"].as_i64()),
        (Some("csv"), Some(3))
    );
    let text = std::fs::read_to_string(&csv).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("account;name;owner;score;band"));
    assert!(
        lines[1].starts_with("hannes-mockinger"),
        "worst first: {text}"
    );
    assert!(lines[1].contains(";red;"), "{text}");
    assert!(lines[1].contains("5.400,00"), "DE money: {text}");
    assert!(
        !text.to_lowercase().contains("user"),
        "no per-person data in the report"
    );
    assert!(
        run(
            &mut c,
            "gain_export",
            json!({ "path": csv.to_string_lossy() })
        )
        .is_err(),
        "existing file needs overwrite"
    );
    run(
        &mut c,
        "gain_export",
        json!({ "path": csv.to_string_lossy(), "overwrite": true }),
    )
    .unwrap();
    let md = dir.join("health.md");
    run(
        &mut c,
        "gain_export",
        json!({ "path": md.to_string_lossy(), "as_of": "09.11.2026" }),
    )
    .unwrap();
    let text = std::fs::read_to_string(&md).unwrap();
    assert!(text.starts_with("# gaincraft health report"));
    assert!(text.contains("| Hannes Mockinger e.U. (DEMO) |"), "{text}");
    assert!(run(
        &mut c,
        "gain_export",
        json!({ "path": dir.join("x.pdf").to_string_lossy() })
    )
    .is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn export_and_backup_respect_confinement() {
    let mut c = ctx("confine");
    c.confinement = Confinement(Some(Vec::new()));
    let dir = out_dir("confine");
    for tool in ["gain_export", "gain_backup"] {
        let err = run(
            &mut c,
            tool,
            json!({ "path": dir.join("x.csv").to_string_lossy() }),
        )
        .unwrap_err();
        assert!(err.contains("allow-path"), "{tool}: {err}");
    }
    c.confinement = Confinement(None);
    let b = run(
        &mut c,
        "gain_backup",
        json!({ "path": dir.join("copy.db").to_string_lossy() }),
    )
    .unwrap();
    assert!(b["bytes"].as_i64().unwrap() > 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn inbox_handles_positive_signals_amendments_and_terminations() {
    let mut c = ctx("inbox-more");
    run(&mut c, "gain_demo_seed", json!({})).unwrap();
    c.bus
        .emit(
            "formcraft",
            APP,
            "signal.positive",
            json!({ "account": "mock-software.example", "score": 8, "response_id": "R-1", "subject": "NPS 10" }),
        )
        .unwrap();
    c.bus
        .emit(
            "contractcraft",
            APP,
            "contract.amended",
            json!({ "contract_id": "C-2026-001", "annual_value_cents": 1_200_000, "term_end": "2028-02-29" }),
        )
        .unwrap();
    c.bus
        .emit(
            "contractcraft",
            APP,
            "contract.amended",
            json!({ "contract_id": "C-9999", "annual_value_cents": 1 }),
        )
        .unwrap();
    let r = run(&mut c, "gain_inbox_drain", json!({})).unwrap();
    assert_eq!(r["applied"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(
        r["rejected"].as_array().unwrap().len(),
        1,
        "unknown contract: {r}"
    );
    let a = c.gain.account("mock-software.example").unwrap();
    assert_eq!(a.arr_cents, Some(1_200_000));
    assert_eq!(a.term_end, Some(d("2028-02-29")));
    assert_eq!(
        a.last_notice,
        Some(d("2026-11-30")),
        "untouched field stays"
    );
    let pos = c.gain.signals(Some(a.id), None).unwrap();
    assert!(pos
        .iter()
        .any(|s| s.polarity == "positive" && s.source == "formcraft"));

    // Termination: renewal CTA dismissed, termination CTA opened, no notice date.
    assert!(c
        .gain
        .ctas(Some(a.id), false)
        .unwrap()
        .iter()
        .any(|x| x.kind == "renewal"));
    c.bus
        .emit(
            "contractcraft",
            APP,
            "contract.terminated",
            json!({ "contract_id": "C-2026-001", "effective": "28.02.2027", "reason": "moved to a competitor" }),
        )
        .unwrap();
    let r = run(&mut c, "gain_inbox_drain", json!({})).unwrap();
    assert_eq!(r["applied"].as_array().unwrap().len(), 1, "{r}");
    let a = c.gain.account("mock-software.example").unwrap();
    assert_eq!((a.last_notice, a.term_end), (None, Some(d("2027-02-28"))));
    assert!(a.active, "still a customer until a person retires it");
    let open = c.gain.ctas(Some(a.id), false).unwrap();
    assert!(!open.iter().any(|x| x.kind == "renewal"), "{open:?}");
    let t = open.iter().find(|x| x.kind == "termination").unwrap();
    assert!(t.title.contains("28.02.2027") && t.params.contains("competitor"));
}

#[test]
fn churn_audit_flags_risk_and_backtests_retired_accounts() {
    let mut c = ctx("audit");
    run(&mut c, "gain_demo_seed", json!({})).unwrap();
    let r = run(&mut c, "gain_churn_audit", json!({ "as_of": "09.11.2026" })).unwrap();
    assert_eq!(r["backtest"]["churned"], 0);
    assert!(r["backtest"]["verdict"]
        .as_str()
        .unwrap()
        .contains("nothing to backtest"));
    let risk = r["risk"].as_array().unwrap();
    assert_eq!(risk[0]["account"], "hannes-mockinger-e.u.-demo", "{r}");
    assert_eq!(risk[0]["at_risk"], true);
    let bird = risk
        .iter()
        .find(|x| x["account"] == "mockingbird-inc.-demo")
        .unwrap();
    assert!(
        bird["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x.as_str().unwrap().contains("30 days")),
        "dropping account shows the drop: {bird}"
    );

    // Mockinger churns on 09.11.2026; 14 days before, was it already the worst?
    run(
        &mut c,
        "gain_account_update",
        json!({ "account": "mockinger.example", "active": false, "retired_on": "09.11.2026" }),
    )
    .unwrap();
    let r = run(
        &mut c,
        "gain_churn_audit",
        json!({ "as_of": "09.11.2026", "lead_days": 14 }),
    )
    .unwrap();
    let b = &r["backtest"];
    assert_eq!(b["churned"], 1, "{r}");
    let m = &b["accounts"][0];
    assert_eq!(
        (m["rank"].as_i64(), m["of"].as_i64()),
        (Some(1), Some(3)),
        "{m}"
    );
    assert_eq!(m["flagged"], true, "{m}");
    assert!(
        b["verdict"].as_str().unwrap().contains("Too few"),
        "honest about n=1"
    );
    assert!(
        !r["risk"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["account"] == "hannes-mockinger-e.u.-demo"),
        "retired accounts are not in the live risk list"
    );
}
