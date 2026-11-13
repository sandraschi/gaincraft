use super::*;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn gain() -> Gain {
    let g = Gain::open_in_memory().unwrap();
    g.add_account(
        &NewAccount {
            name: "Mock Software GmbH".into(),
            domain: Some("@Mock-Software.example".into()),
            seats: Some(10),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    g
}

#[test]
fn accounts_are_found_by_key_domain_and_name() {
    let g = gain();
    assert_eq!(slug("Mockinger Bäckerei e.U."), "mockinger-baeckerei-e.u.");
    let a = g.account("mock-software-gmbh").unwrap();
    assert_eq!(a.domain.as_deref(), Some("mock-software.example"));
    assert_eq!(g.account("MOCK-SOFTWARE.example").unwrap().id, a.id);
    assert_eq!(g.account("mock software gmbh").unwrap().id, a.id);
    assert!(
        g.add_account(
            &NewAccount {
                name: "Mock Software GmbH".into(),
                ..Default::default()
            },
            false
        )
        .is_err(),
        "same key"
    );
    assert!(
        g.add_account(
            &NewAccount {
                name: "Other".into(),
                domain: Some("mock-software.example".into()),
                ..Default::default()
            },
            false
        )
        .is_err(),
        "same domain"
    );
    assert!(g
        .add_account(
            &NewAccount {
                name: "Bad".into(),
                domain: Some("not a domain".into()),
                ..Default::default()
            },
            false
        )
        .is_err());
    let u = g
        .update_account(
            "mock-software-gmbh",
            &AccountPatch {
                seats: Some(Some(12)),
                owner: Some(Some("Anna".into())),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!((u.seats, u.owner.as_deref()), (Some(12), Some("Anna")));
    assert!(g
        .update_account(
            "mock-software-gmbh",
            &AccountPatch {
                seats: Some(Some(0)),
                ..Default::default()
            }
        )
        .is_err());
}

#[test]
fn telemetry_has_no_person_column() {
    let g = gain();
    let cols: Vec<String> = g
        .conn()
        .prepare("SELECT name FROM pragma_table_info('telemetry_daily')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        cols,
        vec![
            "account_id",
            "date",
            "active_users",
            "sessions",
            "key_actions"
        ]
    );
    let a = g.account("mock-software-gmbh").unwrap();
    let day = |dd: u32, n: i64| Day {
        date: d(2026, 11, dd),
        active_users: n,
        sessions: n * 2,
        key_actions: n * 5,
    };
    assert_eq!(
        g.upsert_days(a.id, &[day(1, 5), day(2, 6)]).unwrap(),
        (2, 0)
    );
    assert_eq!(
        g.upsert_days(a.id, &[day(2, 7), day(3, 0)]).unwrap(),
        (1, 1)
    );
    assert_eq!(
        g.days(a.id, d(2026, 11, 1), d(2026, 11, 30)).unwrap()[1].active_users,
        7
    );
    assert_eq!(
        g.last_active_day(a.id, d(2026, 11, 9)).unwrap(),
        Some(d(2026, 11, 2)),
        "a zero day is not activity"
    );
    assert!(g
        .upsert_days(
            a.id,
            &[Day {
                active_users: -1,
                ..day(4, 0)
            }]
        )
        .is_err());
}

#[test]
fn signals_dedupe_by_event_and_validate() {
    let g = gain();
    let a = g.account("mock-software-gmbh").unwrap();
    let s = NewSignal {
        source: "deskcraft".into(),
        polarity: "negative".into(),
        score: 5,
        reference: Some("D-3".into()),
        at: d(2026, 11, 6),
        event_id: Some("ev1".into()),
        ..Default::default()
    };
    assert!(g.add_signal(a.id, &s).unwrap().is_some());
    assert!(g.add_signal(a.id, &s).unwrap().is_none(), "same event once");
    let re_emitted = NewSignal {
        event_id: Some("ev2".into()),
        ..s.clone()
    };
    assert!(
        g.add_signal(a.id, &re_emitted).unwrap().is_none(),
        "same ticket from the same source counts once"
    );
    let other_ticket = NewSignal {
        event_id: Some("ev3".into()),
        reference: Some("D-4".into()),
        ..s.clone()
    };
    assert!(g.add_signal(a.id, &other_ticket).unwrap().is_some());
    assert!(g
        .add_signal(
            a.id,
            &NewSignal {
                score: 11,
                event_id: None,
                ..s.clone()
            }
        )
        .is_err());
    assert!(g
        .add_signal(
            a.id,
            &NewSignal {
                polarity: "meh".into(),
                event_id: None,
                ..s
            }
        )
        .is_err());
    assert_eq!(g.signals(None, Some(d(2026, 11, 1))).unwrap().len(), 2);
}

#[test]
fn triage_is_capped_reasoned_and_expires() {
    let g = gain();
    let a = g.account("mock-software-gmbh").unwrap();
    assert!(g
        .set_triage(
            a.id,
            1.5,
            "LLM read the last tickets",
            "agent",
            d(2026, 11, 9),
            30
        )
        .is_err());
    assert!(
        g.set_triage(a.id, 0.9, "meh", "agent", d(2026, 11, 9), 30)
            .is_err(),
        "reason required"
    );
    g.set_triage(
        a.id,
        0.85,
        "Escalation tone in the last three tickets",
        "agent",
        d(2026, 11, 9),
        14,
    )
    .unwrap();
    assert!(g.triage(a.id, d(2026, 11, 20)).unwrap().is_some());
    assert!(
        g.triage(a.id, d(2026, 11, 24)).unwrap().is_none(),
        "expired"
    );
    // The SQL check holds even if Rust is bypassed.
    assert!(g
        .conn()
        .execute("UPDATE triage SET modifier = 3.0", [])
        .is_err());
}

#[test]
fn ctas_open_once_and_need_a_note() {
    let g = gain();
    let a = g.account("mock-software-gmbh").unwrap();
    assert!(g
        .raise_cta(
            a.id,
            "renewal",
            "2026-11-30",
            "Renewal notice 30.11.",
            Some(d(2026, 11, 30)),
            d(2026, 11, 9)
        )
        .unwrap());
    assert!(!g
        .raise_cta(
            a.id,
            "renewal",
            "2026-11-30",
            "Renewal notice 30.11.",
            Some(d(2026, 11, 30)),
            d(2026, 11, 10)
        )
        .unwrap());
    let c = &g.ctas(None, false).unwrap()[0];
    assert_eq!(c.last_seen, d(2026, 11, 10));
    assert!(g.resolve_cta(c.id, false, " ", "agent").is_err());
    g.resolve_cta(c.id, false, "Called them, renewing", "agent")
        .unwrap();
    assert!(g.resolve_cta(c.id, false, "again", "agent").is_err());
    assert!(
        g.raise_cta(a.id, "renewal", "2026-11-30", "x", None, d(2026, 11, 11))
            .unwrap(),
        "a resolved CTA can come back"
    );
    assert_eq!(g.ctas(None, true).unwrap().len(), 2);
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("gaincraft-store-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn v1_database_migrates_in_place_and_keeps_its_rows() {
    let dir = temp_dir("migrate");
    let path = dir.join("v1.db");
    {
        // A database exactly as gaincraft 0.2.0 left it.
        let c = Connection::open(&path).unwrap();
        c.execute_batch(SCHEMA).unwrap();
        c.execute_batch(
            "INSERT INTO meta (key, value) VALUES ('schema_version', '1');
             INSERT INTO accounts (key, name, created_at) VALUES ('old', 'Old Customer', '2026-01-01T00:00:00Z');
             INSERT INTO ctas (account_id, kind, reference, title, status, created_on, last_seen) VALUES (1, 'low_health', '', 'Health 20 (red)', 'open', '2026-11-01', '2026-11-01');",
        )
        .unwrap();
    }
    let g = Gain::open(&path).unwrap();
    assert_eq!(g.meta("schema_version").unwrap().as_deref(), Some("3"));
    assert!(g.last_sync_run().unwrap().is_none(), "v3 table exists");
    let a = g.account("old").unwrap();
    assert_eq!(a.retired_on, None);
    let c = g.ctas(None, false).unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].params, "{}", "old CTAs get an empty params object");
    drop(g);
    // Opening again is a no-op, and a newer schema is refused.
    let g = Gain::open(&path).unwrap();
    g.set_meta("schema_version", "99").unwrap();
    drop(g);
    assert!(Gain::open(&path).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn sync_runs_are_recorded_newest_first_with_their_outcome() {
    let g = gain();
    assert!(g.last_sync_run().unwrap().is_none());
    let first = g
        .record_sync_run(&NewSyncRun {
            started: "2026-11-12T06:30:00Z".into(),
            finished: "2026-11-12T06:30:02Z".into(),
            source: "sqlite:product.db".into(),
            accounts: 3,
            rows_read: 90,
            wrote: true,
            alerts: 2,
            status: "ok".into(),
            error: None,
        })
        .unwrap();
    assert_eq!((first.id, first.wrote, first.alerts), (1, true, 2));
    let second = g
        .record_sync_run(&NewSyncRun {
            started: "2026-11-13T06:30:00Z".into(),
            finished: "2026-11-13T06:30:01Z".into(),
            source: "none".into(),
            accounts: 3,
            rows_read: 0,
            wrote: false,
            alerts: 0,
            status: "error".into(),
            error: Some("database is locked".into()),
        })
        .unwrap();
    assert_eq!(second.id, 2);
    assert_eq!(
        g.last_sync_run().unwrap().map(|r| r.id),
        Some(2),
        "newest first"
    );
    let runs = g.sync_runs(10).unwrap();
    assert_eq!(runs.iter().map(|r| r.id).collect::<Vec<_>>(), [2, 1]);
    assert_eq!(runs[0].error.as_deref(), Some("database is locked"));
    assert_eq!(runs[1].rows_read, 90);
    assert!(g
        .record_sync_run(&NewSyncRun {
            started: "x".into(),
            finished: "y".into(),
            source: "none".into(),
            accounts: 0,
            rows_read: 0,
            wrote: false,
            alerts: 0,
            status: "maybe".into(),
            error: None,
        })
        .is_err());
}

#[test]
fn retiring_sets_a_churn_date_and_reactivating_clears_it() {
    let g = gain();
    let a = g
        .update_account(
            "mock-software-gmbh",
            &AccountPatch {
                active: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!a.active);
    assert_eq!(a.retired_on, Some(Utc::now().date_naive()));
    let a = g
        .update_account(
            "mock-software-gmbh",
            &AccountPatch {
                retired_on: Some(Some(d(2026, 6, 30))),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(a.retired_on, Some(d(2026, 6, 30)), "backdated churn");
    let a = g
        .update_account(
            "mock-software-gmbh",
            &AccountPatch {
                active: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!((a.active, a.retired_on), (true, None));
    assert!(
        g.update_account(
            "mock-software-gmbh",
            &AccountPatch {
                retired_on: Some(Some(d(2026, 6, 30))),
                ..Default::default()
            },
        )
        .is_err(),
        "an active account cannot carry a churn date"
    );
}

#[test]
fn cta_params_are_stored_and_manual_refs_are_unique() {
    let g = gain();
    let a = g.account("mock-software-gmbh").unwrap();
    let today = d(2026, 11, 12);
    let r1 = g.next_manual_ref().unwrap();
    assert!(g
        .raise_cta_with(
            a.id,
            "manual",
            &r1,
            "Call about the API",
            r#"{"note":"x"}"#,
            None,
            today
        )
        .unwrap());
    let r2 = g.next_manual_ref().unwrap();
    assert_ne!(r1, r2);
    let c = g.ctas(Some(a.id), false).unwrap();
    assert_eq!(c[0].params, r#"{"note":"x"}"#);
    assert!(g
        .raise_cta_with(a.id, "manual", &r2, "  ", "{}", None, today)
        .is_err());
}

#[test]
fn backup_writes_a_readable_copy_and_never_overwrites() {
    let dir = temp_dir("backup");
    let g = gain();
    let out = dir.join("copy.db");
    let bytes = g.backup_to(&out).unwrap();
    assert!(bytes > 0);
    let copy = Gain::open(&out).unwrap();
    assert_eq!(copy.accounts(true).unwrap().len(), 1);
    assert!(g.backup_to(&out).is_err(), "existing file is refused");
    drop(copy);
    let _ = std::fs::remove_dir_all(dir);
}
