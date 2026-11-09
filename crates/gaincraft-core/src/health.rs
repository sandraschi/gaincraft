//! Deterministic account health (0-100). Every component is reported, so a
//! score can always be explained.
//!
//! - adoption: average daily active users over the last 30 days divided by
//!   licensed seats (capped at 1). Without seats: divided by the best day of
//!   the last 90 days.
//! - recency: 0.5 ^ (days since the last active day / 14), a 14-day half-life.
//! - sentiment: 1 minus decayed negative signals plus decayed positive ones
//!   (30-day half-life), clamped to 0-1.
//! - modifier: optional LLM/human triage, 0.8-1.2, multiplies the result.
//!
//! Bands: green >= 70, yellow >= 40, red below.

use chrono::{Duration, NaiveDate};
use gaincraft_store::{Day, Signal};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Weights {
    pub adoption: f64,
    pub recency: f64,
    pub sentiment: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Weights {
            adoption: 0.45,
            recency: 0.35,
            sentiment: 0.20,
        }
    }
}

impl Weights {
    pub fn validate(&self) -> Result<(), String> {
        let all = [self.adoption, self.recency, self.sentiment];
        if all
            .iter()
            .any(|w| !w.is_finite() || !(0.0..=1.0).contains(w))
        {
            return Err("each weight must be between 0 and 1".into());
        }
        if (all.iter().sum::<f64>() - 1.0).abs() > 0.001 {
            return Err(format!(
                "weights must sum to 1 (got {:.3})",
                all.iter().sum::<f64>()
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Health {
    pub score: i64,
    pub band: &'static str,
    pub adoption: f64,
    pub recency: f64,
    pub sentiment: f64,
    pub modifier: f64,
    pub avg_active_30d: f64,
    pub days_since_active: Option<i64>,
    /// Average active users, last 14 days vs. the 14 before (None without data).
    pub trend_14d: Option<f64>,
}

pub fn band(score: i64) -> &'static str {
    if score >= 70 {
        "green"
    } else if score >= 40 {
        "yellow"
    } else {
        "red"
    }
}

fn avg_active(days: &[Day], from: NaiveDate, to: NaiveDate) -> f64 {
    let n = (to - from).num_days() + 1;
    if n <= 0 {
        return 0.0;
    }
    let sum: i64 = days
        .iter()
        .filter(|d| d.date >= from && d.date <= to)
        .map(|d| d.active_users)
        .sum();
    sum as f64 / n as f64
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// `days` should cover at least the 90 days up to `as_of`; `signals` the last 180.
pub fn compute(
    w: &Weights,
    seats: Option<i64>,
    days: &[Day],
    last_active: Option<NaiveDate>,
    signals: &[Signal],
    modifier: Option<f64>,
    as_of: NaiveDate,
) -> Health {
    let avg30 = avg_active(days, as_of - Duration::days(29), as_of);
    let adoption = match seats.filter(|s| *s > 0) {
        Some(s) => (avg30 / s as f64).min(1.0),
        None => {
            let best = days
                .iter()
                .filter(|d| d.date > as_of - Duration::days(90) && d.date <= as_of)
                .map(|d| d.active_users)
                .max()
                .unwrap_or(0);
            if best > 0 {
                (avg30 / best as f64).min(1.0)
            } else {
                0.0
            }
        }
    };
    let days_since = last_active.map(|d| (as_of - d).num_days().max(0));
    let recency = days_since.map_or(0.0, |d| 0.5f64.powf(d as f64 / 14.0));
    let mut sentiment = 1.0;
    for s in signals
        .iter()
        .filter(|s| s.at <= as_of && (as_of - s.at).num_days() <= 180)
    {
        let decay = 0.5f64.powf((as_of - s.at).num_days() as f64 / 30.0);
        if s.polarity == "negative" {
            sentiment -= 0.08 * s.score as f64 * decay;
        } else {
            sentiment += 0.05 * s.score as f64 * decay;
        }
    }
    let sentiment = sentiment.clamp(0.0, 1.0);
    let m = modifier.unwrap_or(1.0).clamp(0.8, 1.2);
    let base = w.adoption * adoption + w.recency * recency + w.sentiment * sentiment;
    let score = ((100.0 * base * m).round() as i64).clamp(0, 100);
    let last14 = avg_active(days, as_of - Duration::days(13), as_of);
    let prev14 = avg_active(days, as_of - Duration::days(27), as_of - Duration::days(14));
    let trend = if prev14 > 0.0 {
        Some(round2(last14 / prev14))
    } else {
        None
    };
    Health {
        score,
        band: band(score),
        adoption: round2(adoption),
        recency: round2(recency),
        sentiment: round2(sentiment),
        modifier: m,
        avg_active_30d: round2(avg30),
        days_since_active: days_since,
        trend_14d: trend,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).unwrap()
    }
    fn days(n: i64, users: i64, end: NaiveDate) -> Vec<Day> {
        (0..n)
            .map(|i| Day {
                date: end - Duration::days(i),
                active_users: users,
                sessions: 0,
                key_actions: 0,
            })
            .collect()
    }
    fn neg(score: i64, at: NaiveDate) -> Signal {
        Signal {
            id: 1,
            account: "a".into(),
            source: "deskcraft".into(),
            polarity: "negative".into(),
            score,
            reference: None,
            note: String::new(),
            at,
            event_id: None,
        }
    }

    #[test]
    fn perfect_account_scores_100() {
        let as_of = d(11, 9);
        let h = compute(
            &Weights::default(),
            Some(5),
            &days(90, 5, as_of),
            Some(as_of),
            &[],
            None,
            as_of,
        );
        assert_eq!(
            (h.score, h.band, h.adoption, h.recency, h.sentiment),
            (100, "green", 1.0, 1.0, 1.0)
        );
        assert_eq!(h.trend_14d, Some(1.0));
    }

    #[test]
    fn components_by_hand() {
        let as_of = d(11, 9);
        // 2 of 10 seats active every day; last active 14 days ago -> recency 0.5.
        let data = days(90, 2, as_of - Duration::days(14));
        let h = compute(
            &Weights::default(),
            Some(10),
            &data,
            Some(as_of - Duration::days(14)),
            &[neg(5, as_of)],
            None,
            as_of,
        );
        // avg30 = 16 days * 2 / 30 = 1.0667 -> adoption 0.1067; sentiment 1 - 0.4 = 0.6
        assert_eq!(h.avg_active_30d, 1.07);
        assert_eq!(h.recency, 0.5);
        assert_eq!(h.sentiment, 0.6);
        // 100 * (0.45 * 0.10667 + 0.35 * 0.5 + 0.2 * 0.6) = 34.3 -> 34, red
        assert_eq!((h.score, h.band), (34, "red"));
        assert_eq!(h.trend_14d, Some(0.0), "nothing in the last 14 days");
        // Triage nudges but cannot rescue: 34 * 1.2 = 41.
        let t = compute(
            &Weights::default(),
            Some(10),
            &data,
            Some(as_of - Duration::days(14)),
            &[neg(5, as_of)],
            Some(5.0),
            as_of,
        );
        assert_eq!((t.modifier, t.score, t.band), (1.2, 41, "yellow"));
    }

    #[test]
    fn signals_decay_and_old_ones_drop_out() {
        let as_of = d(11, 9);
        let data = days(90, 5, as_of);
        let fresh = compute(
            &Weights::default(),
            Some(5),
            &data,
            Some(as_of),
            &[neg(10, as_of)],
            None,
            as_of,
        );
        let month_old = compute(
            &Weights::default(),
            Some(5),
            &data,
            Some(as_of),
            &[neg(10, as_of - Duration::days(30))],
            None,
            as_of,
        );
        let ancient = compute(
            &Weights::default(),
            Some(5),
            &data,
            Some(as_of),
            &[neg(10, as_of - Duration::days(200))],
            None,
            as_of,
        );
        assert_eq!(
            (fresh.sentiment, month_old.sentiment, ancient.sentiment),
            (0.2, 0.6, 1.0)
        );
    }

    #[test]
    fn no_seats_uses_best_day_and_no_data_is_zero() {
        let as_of = d(11, 9);
        let mut data = days(30, 4, as_of);
        data.push(Day {
            date: as_of - Duration::days(60),
            active_users: 8,
            sessions: 0,
            key_actions: 0,
        });
        assert_eq!(
            compute(
                &Weights::default(),
                None,
                &data,
                Some(as_of),
                &[],
                None,
                as_of
            )
            .adoption,
            0.5
        );
        let empty = compute(&Weights::default(), None, &[], None, &[], None, as_of);
        assert_eq!(
            (empty.score, empty.band),
            (20, "red"),
            "only sentiment remains"
        );
    }

    #[test]
    fn weights_validate() {
        assert!(Weights::default().validate().is_ok());
        assert!(Weights {
            adoption: 0.5,
            recency: 0.5,
            sentiment: 0.5
        }
        .validate()
        .is_err());
        assert!(Weights {
            adoption: 1.2,
            recency: -0.2,
            sentiment: 0.0
        }
        .validate()
        .is_err());
    }
}
