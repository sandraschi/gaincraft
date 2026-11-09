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
