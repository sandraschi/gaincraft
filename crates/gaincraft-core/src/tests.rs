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
