//! Minimal cron expression parser.
//!
//! Supports standard 5-field cron: `minute hour day-of-month month day-of-week`.
//! Special values: `*` (any), `*/N` (step), `N-M` (range), `N,M` (list).

use chrono::{Datelike, NaiveDateTime, Timelike};

/// A parsed cron expression.
#[derive(Debug, Clone)]
pub struct CronExpr {
    minutes: FieldSet,
    hours: FieldSet,
    days_of_month: FieldSet,
    months: FieldSet,
    days_of_week: FieldSet,
    raw: String,
}

/// Set of allowed values for one cron field.
#[derive(Debug, Clone)]
struct FieldSet {
    values: Vec<u32>,
}

impl FieldSet {
    fn contains(&self, v: u32) -> bool {
        self.values.contains(&v)
    }

    fn parse(field: &str, min: u32, max: u32) -> Result<Self, String> {
        let mut values = Vec::new();
        for part in field.split(',') {
            let part = part.trim();
            if part == "*" {
                return Ok(Self {
                    values: (min..=max).collect(),
                });
            } else if let Some(step) = part.strip_prefix("*/") {
                let step: u32 = step.parse().map_err(|_| format!("invalid step: {part}"))?;
                if step == 0 {
                    return Err("step cannot be 0".into());
                }
                let mut v = min;
                while v <= max {
                    values.push(v);
                    v += step;
                }
            } else if part.contains('-') {
                let parts: Vec<&str> = part.splitn(2, '-').collect();
                let lo: u32 = parts[0]
                    .parse()
                    .map_err(|_| format!("invalid range start: {}", parts[0]))?;
                let hi: u32 = parts[1]
                    .parse()
                    .map_err(|_| format!("invalid range end: {}", parts[1]))?;
                if lo > hi || lo < min || hi > max {
                    return Err(format!("range {lo}-{hi} out of bounds ({min}-{max})"));
                }
                values.extend(lo..=hi);
            } else {
                let v: u32 = part.parse().map_err(|_| format!("invalid value: {part}"))?;
                if v < min || v > max {
                    return Err(format!("value {v} out of bounds ({min}-{max})"));
                }
                values.push(v);
            }
        }
        values.sort_unstable();
        values.dedup();
        Ok(Self { values })
    }
}

impl CronExpr {
    /// Parse a 5-field cron expression.
    pub fn parse(expr: &str) -> Result<Self, String> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!(
                "expected 5 fields (minute hour dom month dow), got {}",
                fields.len()
            ));
        }

        Ok(Self {
            minutes: FieldSet::parse(fields[0], 0, 59)?,
            hours: FieldSet::parse(fields[1], 0, 23)?,
            days_of_month: FieldSet::parse(fields[2], 1, 31)?,
            months: FieldSet::parse(fields[3], 1, 12)?,
            days_of_week: FieldSet::parse(fields[4], 0, 6)?,
            raw: expr.to_string(),
        })
    }

    /// Check whether a given datetime matches this cron expression.
    pub fn matches(&self, dt: &NaiveDateTime) -> bool {
        self.minutes.contains(dt.minute())
            && self.hours.contains(dt.hour())
            && self.days_of_month.contains(dt.day())
            && self.months.contains(dt.month())
            && self
                .days_of_week
                .contains(dt.weekday().num_days_from_sunday())
    }

    /// Return the raw cron string.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Find the next matching datetime strictly after `after`.
    ///
    /// Scans minute-by-minute up to 366 days out. Returns `None` if
    /// no match is found (e.g. impossible expression like `30 2 30 2 *`).
    pub fn next_after(&self, after: &NaiveDateTime) -> Option<NaiveDateTime> {
        // Start from the next whole minute.
        let mut candidate = *after + chrono::Duration::minutes(1);
        candidate = candidate.with_second(0)?.with_nanosecond(0)?;

        let limit = *after + chrono::Duration::days(366);
        while candidate < limit {
            if self.matches(&candidate) {
                return Some(candidate);
            }
            candidate += chrono::Duration::minutes(1);
        }
        None
    }
}

impl std::fmt::Display for CronExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap()
    }

    #[test]
    fn test_parse_every_minute() {
        let expr = CronExpr::parse("* * * * *").unwrap();
        assert!(expr.matches(&dt(2026, 4, 5, 12, 30)));
    }

    #[test]
    fn test_parse_specific_time() {
        let expr = CronExpr::parse("30 9 * * *").unwrap();
        assert!(expr.matches(&dt(2026, 4, 5, 9, 30)));
        assert!(!expr.matches(&dt(2026, 4, 5, 9, 31)));
        assert!(!expr.matches(&dt(2026, 4, 5, 10, 30)));
    }

    #[test]
    fn test_parse_step() {
        let expr = CronExpr::parse("*/15 * * * *").unwrap();
        assert!(expr.matches(&dt(2026, 1, 1, 0, 0)));
        assert!(expr.matches(&dt(2026, 1, 1, 0, 15)));
        assert!(expr.matches(&dt(2026, 1, 1, 0, 30)));
        assert!(expr.matches(&dt(2026, 1, 1, 0, 45)));
        assert!(!expr.matches(&dt(2026, 1, 1, 0, 10)));
    }

    #[test]
    fn test_parse_range() {
        let expr = CronExpr::parse("0 9-17 * * *").unwrap();
        assert!(expr.matches(&dt(2026, 1, 1, 9, 0)));
        assert!(expr.matches(&dt(2026, 1, 1, 17, 0)));
        assert!(!expr.matches(&dt(2026, 1, 1, 8, 0)));
        assert!(!expr.matches(&dt(2026, 1, 1, 18, 0)));
    }

    #[test]
    fn test_parse_list() {
        let expr = CronExpr::parse("0 9,12,18 * * *").unwrap();
        assert!(expr.matches(&dt(2026, 1, 1, 9, 0)));
        assert!(expr.matches(&dt(2026, 1, 1, 12, 0)));
        assert!(expr.matches(&dt(2026, 1, 1, 18, 0)));
        assert!(!expr.matches(&dt(2026, 1, 1, 10, 0)));
    }

    #[test]
    fn test_day_of_week() {
        // 2026-04-06 is a Monday (dow=1)
        let expr = CronExpr::parse("0 9 * * 1").unwrap();
        assert!(expr.matches(&dt(2026, 4, 6, 9, 0)));
        assert!(!expr.matches(&dt(2026, 4, 5, 9, 0))); // Sunday
    }

    #[test]
    fn test_next_after() {
        let expr = CronExpr::parse("30 9 * * *").unwrap();
        let now = dt(2026, 4, 5, 8, 0);
        let next = expr.next_after(&now).unwrap();
        assert_eq!(next, dt(2026, 4, 5, 9, 30));
    }

    #[test]
    fn test_next_after_wraps_day() {
        let expr = CronExpr::parse("0 6 * * *").unwrap();
        let now = dt(2026, 4, 5, 23, 0);
        let next = expr.next_after(&now).unwrap();
        assert_eq!(next, dt(2026, 4, 6, 6, 0));
    }

    #[test]
    fn test_invalid_field_count() {
        assert!(CronExpr::parse("* * *").is_err());
    }

    #[test]
    fn test_invalid_value() {
        assert!(CronExpr::parse("60 * * * *").is_err());
    }

    #[test]
    fn test_invalid_step_zero() {
        assert!(CronExpr::parse("*/0 * * * *").is_err());
    }
}
