//! Five-field cron expressions for fixed schedule triggers (spec §7.1).
//!
//! A rule pack may express a fixed schedule as `cron: "30 18 * * mon-fri"` instead of naming a
//! single `at` time, which is what "cron-like expression" in the spec means. Parsing happens here,
//! once, at load time; the scheduler then evaluates the parsed value against local wall-clock time.

/// The set of values a single cron field accepts, as a bitset over at most 64 values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldSet {
    bits: u64,
}

impl FieldSet {
    const fn empty() -> Self {
        Self { bits: 0 }
    }

    fn all(range: std::ops::RangeInclusive<u32>) -> Self {
        let mut bits = 0u64;
        for v in range {
            bits |= 1u64 << v;
        }
        Self { bits }
    }

    fn insert(&mut self, value: u32) {
        self.bits |= 1u64 << value;
    }

    fn contains(&self, value: u32) -> bool {
        self.bits & (1u64 << value) != 0
    }

    /// Whether every value in `range` is set, i.e. the field was written as `*`.
    fn is_full(&self, range: std::ops::RangeInclusive<u32>) -> bool {
        Self::all(range).bits == self.bits
    }
}

/// Why a cron expression was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CronError {
    /// The expression did not have exactly five fields.
    #[error(
        "cron expression must have 5 fields (minute hour day-of-month month day-of-week), got {0}"
    )]
    FieldCount(usize),
    /// One field could not be parsed.
    #[error("invalid cron {field} field `{value}`: {reason}")]
    Field {
        /// Which field failed, e.g. `minute`.
        field: &'static str,
        /// The offending text.
        value: String,
        /// Why it is not valid.
        reason: &'static str,
    },
}

/// A parsed five-field cron expression: `minute hour day-of-month month day-of-week`.
///
/// Supported syntax per field:
///
/// * `*` — every value;
/// * `5` — a single value;
/// * `1-5` — an inclusive range;
/// * `*/15` and `1-40/15` — a step over `*` or over a range;
/// * `1,5,9` — a list of any of the above.
///
/// Month and weekday fields also accept the usual three-letter names (`jan`..`dec`, `mon`..`sun`).
/// Weekday `7` is accepted as an alias for Sunday.
///
/// When both `day-of-month` and `day-of-week` are restricted, a day matches if **either** does,
/// which is the traditional cron rule and the one operators expect from a show file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    minutes: FieldSet,
    hours: FieldSet,
    days_of_month: FieldSet,
    months: FieldSet,
    days_of_week: FieldSet,
    /// True when `day-of-month` is not `*`.
    day_of_month_restricted: bool,
    /// True when `day-of-week` is not `*`.
    day_of_week_restricted: bool,
}

/// One cron field: its name, accepted range and optional name aliases.
struct FieldSpec {
    name: &'static str,
    min: u32,
    max: u32,
    /// Aliases mapped to their numeric value, e.g. `jan` to `1`.
    names: &'static [(&'static str, u32)],
}

const MONTHS: &[(&str, u32)] = &[
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];

const WEEKDAYS: &[(&str, u32)] = &[
    ("sun", 0),
    ("mon", 1),
    ("tue", 2),
    ("wed", 3),
    ("thu", 4),
    ("fri", 5),
    ("sat", 6),
];

const MINUTE: FieldSpec = FieldSpec {
    name: "minute",
    min: 0,
    max: 59,
    names: &[],
};
const HOUR: FieldSpec = FieldSpec {
    name: "hour",
    min: 0,
    max: 23,
    names: &[],
};
const DAY_OF_MONTH: FieldSpec = FieldSpec {
    name: "day-of-month",
    min: 1,
    max: 31,
    names: &[],
};
const MONTH: FieldSpec = FieldSpec {
    name: "month",
    min: 1,
    max: 12,
    names: MONTHS,
};
const DAY_OF_WEEK: FieldSpec = FieldSpec {
    name: "day-of-week",
    min: 0,
    max: 7,
    names: WEEKDAYS,
};

impl FieldSpec {
    fn range(&self) -> std::ops::RangeInclusive<u32> {
        self.min..=self.max
    }

    fn error(&self, value: &str, reason: &'static str) -> CronError {
        CronError::Field {
            field: self.name,
            value: value.to_owned(),
            reason,
        }
    }

    /// Resolves one atom (a number or an alias) to its numeric value.
    fn resolve(&self, atom: &str) -> Option<u32> {
        let atom = atom.trim();
        if atom.is_empty() {
            return None;
        }
        if let Ok(n) = atom.parse::<u32>() {
            // Weekday 7 is Sunday; normalize it to 0 so the set stays within 0-6.
            let n = if self.name == "day-of-week" && n == 7 {
                0
            } else {
                n
            };
            return (self.min..=self.max).contains(&n).then_some(n);
        }
        self.names
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(atom))
            .map(|(_, v)| *v)
    }

    fn parse(&self, text: &str) -> Result<FieldSet, CronError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(self.error(text, "field is empty"));
        }
        let mut set = FieldSet::empty();
        for part in text.split(',') {
            self.parse_part(part.trim(), &mut set)?;
        }
        if set.bits == 0 {
            return Err(self.error(text, "field matches nothing"));
        }
        Ok(set)
    }

    fn parse_part(&self, part: &str, set: &mut FieldSet) -> Result<(), CronError> {
        let (range_part, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step
                    .trim()
                    .parse()
                    .map_err(|_| self.error(part, "step must be a positive number"))?;
                if step == 0 {
                    return Err(self.error(part, "step must be greater than zero"));
                }
                (range.trim(), step)
            }
            None => (part, 1),
        };

        let (start, end) = if range_part == "*" {
            (self.min, self.max)
        } else if let Some((from, to)) = range_part.split_once('-') {
            let from = self
                .resolve(from)
                .ok_or_else(|| self.error(range_part, "value out of range"))?;
            let to = self
                .resolve(to)
                .ok_or_else(|| self.error(range_part, "value out of range"))?;
            (from, to)
        } else {
            let only = self
                .resolve(range_part)
                .ok_or_else(|| self.error(range_part, "value out of range"))?;
            (only, only)
        };

        if start > end {
            return Err(self.error(part, "range start is after its end"));
        }
        let mut value = start;
        while value <= end {
            set.insert(value);
            match value.checked_add(step) {
                Some(next) => value = next,
                None => break,
            }
        }
        Ok(())
    }
}

impl CronSchedule {
    /// Parses `minute hour day-of-month month day-of-week`.
    pub fn parse(expression: &str) -> Result<Self, CronError> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(CronError::FieldCount(fields.len()));
        }
        let minutes = MINUTE.parse(fields[0])?;
        let hours = HOUR.parse(fields[1])?;
        let days_of_month = DAY_OF_MONTH.parse(fields[2])?;
        let months = MONTH.parse(fields[3])?;
        let mut days_of_week = DAY_OF_WEEK.parse(fields[4])?;
        // Weekday 7 normalizes to 0, so the weekday domain is 0-6 for rendering and matching.
        days_of_week.bits &= (1u64 << 7) - 1;
        Ok(Self {
            minutes,
            hours,
            day_of_month_restricted: !days_of_month.is_full(DAY_OF_MONTH.range()),
            day_of_week_restricted: !days_of_week.is_full(0..=6),
            days_of_month,
            months,
            days_of_week,
        })
    }

    /// The canonical form of this expression, as `*`, `a`, `a-b` or comma-separated runs.
    ///
    /// Parsing the result yields an identical schedule, so labels written to execution records are
    /// stable no matter how the operator spaced the original expression.
    pub fn to_expression(&self) -> String {
        format!(
            "{} {} {} {} {}",
            render(&self.minutes, 0..=59),
            render(&self.hours, 0..=23),
            render(&self.days_of_month, 1..=31),
            render(&self.months, 1..=12),
            render(&self.days_of_week, 0..=6)
        )
    }

    /// Whether this expression matches the given local wall-clock fields.
    ///
    /// `weekday_index` is Monday-first (`0` = Monday) as produced by the scheduler's local clock;
    /// cron itself numbers Sunday as `0`, so the index is rotated here.
    pub fn matches(
        &self,
        minute: u32,
        hour: u32,
        day_of_month: u32,
        month: u32,
        weekday_index: usize,
    ) -> bool {
        if !self.minutes.contains(minute.min(59)) || !self.hours.contains(hour.min(23)) {
            return false;
        }
        if !self.months.contains(month.clamp(1, 12)) {
            return false;
        }
        let cron_weekday = ((weekday_index % 7) as u32 + 1) % 7;
        let dom_hit = self.days_of_month.contains(day_of_month.clamp(1, 31));
        let dow_hit = self.days_of_week.contains(cron_weekday);
        match (self.day_of_month_restricted, self.day_of_week_restricted) {
            // Both restricted: traditional cron OR semantics.
            (true, true) => dom_hit || dow_hit,
            (true, false) => dom_hit,
            (false, true) => dow_hit,
            (false, false) => true,
        }
    }
}

/// Renders a set back to the compactest of `*`, `a` or `a-b` form.
fn render(set: &FieldSet, domain: std::ops::RangeInclusive<u32>) -> String {
    if set.is_full(domain.clone()) {
        return "*".to_string();
    }
    let (min, max) = (*domain.start(), *domain.end());
    let values: Vec<u32> = (min..=max).filter(|v| set.contains(*v)).collect();
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0usize;
    while start < values.len() {
        let mut end = start;
        while end + 1 < values.len() && values[end + 1] == values[end] + 1 {
            end += 1;
        }
        let (a, b) = (values[start], values[end]);
        parts.push(if a == b {
            a.to_string()
        } else {
            format!("{a}-{b}")
        });
        start = end + 1;
    }
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(text: &str) -> CronSchedule {
        CronSchedule::parse(text).unwrap_or_else(|e| panic!("{text:?}: {e}"))
    }

    #[test]
    fn a_wildcard_expression_matches_every_minute() {
        let c = expr("* * * * *");
        assert!(c.matches(0, 0, 1, 1, 0));
        assert!(c.matches(37, 13, 15, 6, 6));
    }

    #[test]
    fn fixed_time_matches_only_that_minute() {
        let c = expr("55 18 * * *");
        assert!(c.matches(55, 18, 1, 1, 0));
        assert!(!c.matches(54, 18, 1, 1, 0));
        assert!(!c.matches(55, 17, 1, 1, 0));
    }

    #[test]
    fn lists_ranges_and_steps_are_supported() {
        let hourly = expr("0,30 * * * *");
        assert!(hourly.matches(0, 5, 1, 1, 0));
        assert!(hourly.matches(30, 5, 1, 1, 0));
        assert!(!hourly.matches(15, 5, 1, 1, 0));

        let quarter_hours = expr("*/15 * * * *");
        for minute in [0u32, 15, 30, 45] {
            assert!(quarter_hours.matches(minute, 3, 1, 1, 0), "{minute}");
        }
        assert!(!quarter_hours.matches(16, 3, 1, 1, 0));

        let range_step = expr("0 0-12/6 * * *");
        for hour in [0u32, 6, 12] {
            assert!(range_step.matches(0, hour, 1, 1, 0), "{hour}");
        }
        assert!(!range_step.matches(0, 5, 1, 1, 0));
        assert!(!range_step.matches(0, 18, 1, 1, 0));
    }

    #[test]
    fn a_minutely_expression_matches_every_day_it_names() {
        let c = expr("*/5 8-10 * * *");
        let mut hits = 0;
        for hour in 8..=10 {
            for minute in 0..60 {
                if c.matches(minute, hour, 4, 4, 0) {
                    hits += 1;
                }
            }
        }
        assert_eq!(hits, 36, "3 hours x 12 five-minute marks");
        assert!(!c.matches(0, 11, 4, 4, 0));
        assert!(!c.matches(1, 9, 4, 4, 0));
    }

    #[test]
    fn weekday_names_and_the_seven_alias_are_accepted() {
        let c = expr("0 9 * * mon-fri");
        assert!(c.matches(0, 9, 1, 1, 0), "monday");
        assert!(c.matches(0, 9, 1, 1, 4), "friday");
        assert!(!c.matches(0, 9, 1, 1, 5), "saturday");
        assert!(!c.matches(0, 9, 1, 1, 6), "sunday");

        let sunday = expr("0 0 * * 7");
        assert!(sunday.matches(0, 0, 1, 1, 6), "weekday 7 is sunday");
        assert!(!sunday.matches(0, 0, 1, 1, 0));
        assert_eq!(sunday.to_expression(), "0 0 * * 0");
    }

    #[test]
    fn month_names_are_accepted() {
        let c = expr("0 12 * dec *");
        assert!(c.matches(0, 12, 25, 12, 0));
        assert!(!c.matches(0, 12, 25, 11, 0));
    }

    #[test]
    fn day_of_month_and_day_of_week_use_cron_or_semantics() {
        let both = expr("0 0 1 * mon");
        // The 1st of the month, whatever weekday it is...
        assert!(both.matches(0, 0, 1, 6, 3));
        // ...or any Monday.
        assert!(both.matches(0, 0, 17, 6, 0));
        // Neither.
        assert!(!both.matches(0, 0, 17, 6, 1));

        // With only the weekday restricted, the day of month is ignored.
        let dow_only = expr("0 0 * * mon");
        assert!(dow_only.matches(0, 0, 17, 6, 0));
        assert!(!dow_only.matches(0, 0, 17, 6, 1));
    }

    #[test]
    fn out_of_range_arguments_are_clamped_rather_than_panicking() {
        let c = expr("* * * * *");
        assert!(c.matches(999, 999, 999, 999, 99));
    }

    #[test]
    fn malformed_expressions_are_rejected_with_a_reason() {
        assert_eq!(
            CronSchedule::parse("* * * *").unwrap_err(),
            CronError::FieldCount(4)
        );
        assert_eq!(
            CronSchedule::parse("* * * * * *").unwrap_err(),
            CronError::FieldCount(6)
        );
        assert!(
            CronSchedule::parse("60 * * * *").is_err(),
            "minute above 59"
        );
        assert!(CronSchedule::parse("* 24 * * *").is_err(), "hour above 23");
        assert!(
            CronSchedule::parse("* * 0 * *").is_err(),
            "day of month below 1"
        );
        assert!(CronSchedule::parse("* * * 13 *").is_err(), "month above 12");
        assert!(CronSchedule::parse("* * * * 8").is_err(), "weekday above 7");
        assert!(CronSchedule::parse("*/0 * * * *").is_err(), "zero step");
        assert!(
            CronSchedule::parse("*/x * * * *").is_err(),
            "non-numeric step"
        );
        assert!(
            CronSchedule::parse("10-5 * * * *").is_err(),
            "reversed range"
        );
        assert!(CronSchedule::parse("  * * * *  ").is_err(), "empty field");
        assert!(
            CronSchedule::parse("1,,2 * * * *").is_err(),
            "empty list element"
        );
        assert!(CronSchedule::parse("nonsense * * * *").is_err());
        assert!(CronSchedule::parse("").is_err());
    }

    #[test]
    fn errors_name_the_offending_field() {
        let err = CronSchedule::parse("0 99 * * *").unwrap_err().to_string();
        assert!(err.contains("hour"), "{err}");
        assert!(err.contains("99"), "{err}");
        let err = CronSchedule::parse("0 0 * bogus *")
            .unwrap_err()
            .to_string();
        assert!(err.contains("month"), "{err}");
        let err = CronSchedule::parse("* * * * funday")
            .unwrap_err()
            .to_string();
        assert!(err.contains("day-of-week"), "{err}");
    }

    #[test]
    fn canonical_rendering_round_trips() {
        for text in [
            "* * * * *",
            "0,30 8-18 * * mon-fri",
            "*/15 * * * *",
            "0 0 1 jan sun",
        ] {
            let once = expr(text).to_expression();
            let twice = expr(&once).to_expression();
            assert_eq!(once, twice, "{text}");
            assert_eq!(expr(text).to_expression(), twice, "{text}");
        }
        assert_eq!(
            expr("0,30 8-18 * * mon-fri").to_expression(),
            "0,30 8-18 * * 1-5"
        );
    }
}
