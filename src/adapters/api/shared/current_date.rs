use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Europe::Bratislava;

/// Resolve on each request, including turns in an existing conversation.
pub fn today() -> NaiveDate {
    local_date(Utc::now())
}

fn local_date(now: DateTime<Utc>) -> NaiveDate {
    now.with_timezone(&Bratislava).date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bratislava_midnight_tracks_winter_and_summer_time() {
        for (instant, expected) in [
            ("2026-01-13T22:59:59Z", "2026-01-13"),
            ("2026-01-13T23:00:00Z", "2026-01-14"),
            ("2026-07-13T21:59:59Z", "2026-07-13"),
            ("2026-07-13T22:00:00Z", "2026-07-14"),
            ("2026-03-29T21:59:59Z", "2026-03-29"),
            ("2026-03-29T22:00:00Z", "2026-03-30"),
            ("2026-10-25T22:59:59Z", "2026-10-25"),
            ("2026-10-25T23:00:00Z", "2026-10-26"),
        ] {
            assert_eq!(
                local_date(instant.parse().unwrap()),
                expected.parse::<NaiveDate>().unwrap()
            );
        }
    }
}
