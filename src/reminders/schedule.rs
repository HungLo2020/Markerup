use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset,
    TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;
use markdown::{Constructs, ParseOptions, mdast::Node, to_mdast};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    Once,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub start: NaiveDateTime,
    pub tz: String,
    pub repeat: Repeat,
    pub every: u32,
    pub weekdays: Vec<u32>, // Monday = 1
    pub last_day: bool,
    pub until: Option<NaiveDate>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reminder {
    pub id: String,
    pub title: String,
    pub offset: usize,
    #[serde(default)]
    pub end: usize,
    pub schedule: Schedule,
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Parsed {
    pub reminders: Vec<Reminder>,
    pub errors: Vec<String>,
}

pub fn parse_spec(text: &str) -> Result<(Schedule, Option<String>), String> {
    let mut fields = text.split(';').map(str::trim);
    let date = fields.next().unwrap_or_default();
    let mut values = std::collections::BTreeMap::new();
    for field in fields {
        let (key, value) = field
            .split_once('=')
            .ok_or("Expected key=value after ';'")?;
        if values.insert(key.trim(), value.trim()).is_some() {
            return Err(format!("Duplicate field: {key}"));
        }
    }
    for key in values.keys() {
        if ![
            "tz", "repeat", "every", "weekdays", "day", "month", "start", "until", "id",
        ]
        .contains(key)
        {
            return Err(format!("Unknown reminder field: {key}"));
        }
    }
    let tz = values
        .get("tz")
        .ok_or("Choose a time zone (for example tz=America/Los_Angeles)")?
        .to_string();
    tz.parse::<Tz>().map_err(|_| "Unknown time zone")?;
    let repeat = match values.get("repeat").copied().unwrap_or("once") {
        "once" => Repeat::Once,
        "daily" => Repeat::Daily,
        "weekly" => Repeat::Weekly,
        "monthly" => Repeat::Monthly,
        "yearly" => Repeat::Yearly,
        _ => return Err("Repeat must be once, daily, weekly, monthly, or yearly".into()),
    };
    let every = values
        .get("every")
        .unwrap_or(&"1")
        .parse::<u32>()
        .map_err(|_| "Invalid repeat interval")?;
    if !(1..=100).contains(&every) {
        return Err("Repeat interval must be between 1 and 100".into());
    }
    let time_only = NaiveTime::parse_from_str(date, "%H:%M").ok();
    let mut start = if let Some(time) = time_only {
        if repeat == Repeat::Once {
            return Err("A one-time reminder needs a date".into());
        }
        let anchor = values.get("start").copied().unwrap_or("1970-01-05");
        NaiveDate::parse_from_str(anchor, "%Y-%m-%d")
            .map_err(|_| "Invalid start date")?
            .and_time(time)
    } else {
        NaiveDateTime::parse_from_str(date, "%Y-%m-%d %H:%M")
            .or_else(|_| NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M"))
            .map_err(|_| "Use YYYY-MM-DD HH:MM for the reminder date")?
    };
    if !(1970..=9999).contains(&start.year()) {
        return Err("Date is outside the supported range".into());
    }
    let last_day = values.get("day") == Some(&"last");
    if values.contains_key("day") || values.contains_key("month") {
        if !matches!(repeat, Repeat::Monthly | Repeat::Yearly) {
            return Err("Day/month fields require monthly or yearly repetition".into());
        }
        let day = if last_day {
            start.day()
        } else {
            values
                .get("day")
                .map(|v| v.parse::<u32>())
                .transpose()
                .map_err(|_| "Invalid day")?
                .unwrap_or(start.day())
        };
        let month = values
            .get("month")
            .map(|v| v.parse::<u32>())
            .transpose()
            .map_err(|_| "Invalid month")?
            .unwrap_or(start.month());
        // A leap-year anchor permits yearly February 29 and monthly day 31.
        if time_only.is_some() && !values.contains_key("start") {
            let anchor_month = if repeat == Repeat::Monthly { 1 } else { month };
            start = NaiveDate::from_ymd_opt(2000, anchor_month, day)
                .ok_or("Invalid calendar date")?
                .and_time(start.time());
        } else {
            start = NaiveDate::from_ymd_opt(start.year(), month, day)
                .ok_or("Invalid calendar date")?
                .and_time(start.time());
        }
    }
    let mut weekdays = Vec::new();
    if let Some(days) = values.get("weekdays") {
        if repeat != Repeat::Weekly {
            return Err("Weekdays require weekly repetition".into());
        }
        for day in days.split(',') {
            let number = match day.trim().to_ascii_lowercase().as_str() {
                "mon" => 1,
                "tue" => 2,
                "wed" => 3,
                "thu" => 4,
                "fri" => 5,
                "sat" => 6,
                "sun" => 7,
                _ => return Err("Use weekdays such as Mon,Wed,Fri".into()),
            };
            if !weekdays.contains(&number) {
                weekdays.push(number);
            }
        }
    }
    if repeat == Repeat::Weekly && weekdays.is_empty() {
        weekdays.push(start.weekday().number_from_monday());
    }
    let until = values
        .get("until")
        .map(|v| NaiveDate::parse_from_str(v, "%Y-%m-%d"))
        .transpose()
        .map_err(|_| "Invalid end date")?;
    if until.is_some_and(|end| end < start.date()) {
        return Err("End date is before the start date".into());
    }
    let id = values.get("id").map(|v| v.to_string());
    if id.as_ref().is_some_and(|s| {
        s.is_empty() || s.len() > 100 || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    }) {
        return Err("Reminder ID must contain letters, numbers, or hyphens".into());
    }
    Ok((
        Schedule {
            start,
            tz,
            repeat,
            every,
            weekdays,
            last_day,
            until,
        },
        id,
    ))
}

pub fn parse(source: &str) -> Parsed {
    let mut out = Parsed::default();
    if !source.contains("@remind(") {
        return out;
    }
    let options = ParseOptions {
        constructs: Constructs {
            frontmatter: true,
            ..Constructs::gfm()
        },
        ..ParseOptions::gfm()
    };
    match to_mdast(source, &options) {
        Ok(tree) => visit(&tree, source, false, &mut out),
        Err(error) => out.errors.push(error.to_string()),
    }
    let mut ids = std::collections::HashSet::new();
    out.reminders.retain(|r| {
        if ids.insert(r.id.clone()) {
            true
        } else {
            out.errors.push(format!("Duplicate reminder ID: {}", r.id));
            false
        }
    });
    out
}

fn visit(node: &Node, source: &str, done: bool, out: &mut Parsed) {
    let done = done || matches!(node, Node::ListItem(item) if item.checked == Some(true));
    if done
        || matches!(
            node,
            Node::Code(_)
                | Node::InlineCode(_)
                | Node::Html(_)
                | Node::Yaml(_)
                | Node::Toml(_)
                | Node::Image(_)
                | Node::ImageReference(_)
        )
    {
        return;
    }
    if let Node::Text(text) = node
        && let Some(position) = &text.position
    {
        let raw = &source[position.start.offset..position.end.offset];
        let mut cursor = 0;
        while let Some(index) = raw[cursor..].find("@remind(") {
            let begin = cursor + index;
            let slashes = source[..position.start.offset + begin]
                .chars()
                .rev()
                .take_while(|c| *c == '\\')
                .count();
            if slashes % 2 == 1 {
                cursor = begin + 8;
                continue;
            }
            let Some(end) = raw[begin + 8..].find(')').map(|i| begin + 8 + i) else {
                out.errors
                    .push(format!("Unclosed reminder at line {}", position.start.line));
                break;
            };
            cursor = end + 1;
            let offset = position.start.offset + begin;
            let line_start = source[..offset].rfind('\n').map_or(0, |n| n + 1);
            let line_end = source[offset..]
                .find('\n')
                .map_or(source.len(), |n| offset + n);
            let title = source[line_start..offset]
                .trim()
                .trim_start_matches(['-', '*', '>', ' ', '#'])
                .trim_start_matches("[ ]")
                .trim();
            let title = if title.is_empty() { "Reminder" } else { title };
            match parse_spec(&raw[begin + 8..end]) {
                Ok((schedule, id)) => out.reminders.push(Reminder {
                    id: id.unwrap_or_else(|| {
                        format!("text-{:016x}", stable_hash(&source[line_start..line_end]))
                    }),
                    title: title.chars().take(200).collect(),
                    offset,
                    end: position.start.offset + end + 1,
                    schedule,
                }),
                Err(error) => out.errors.push(format!(
                    "Line {}: {error}",
                    source[..offset].bytes().filter(|b| *b == b'\n').count() + 1
                )),
            }
        }
    }
    if let Some(children) = node.children() {
        for child in children {
            visit(child, source, done, out);
        }
    }
}

pub fn stable_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

// Choose the first occurrence of ambiguous wall time. For a missing time, move
// forward by the DST gap (02:30 -> 03:30), not to the first valid minute.
fn resolve(zone: Tz, wall: NaiveDateTime) -> Option<DateTime<Utc>> {
    let earliest = |result: LocalResult<DateTime<Tz>>| match result {
        LocalResult::Single(t) => Some(t),
        LocalResult::Ambiguous(a, b) => Some(a.min(b)),
        LocalResult::None => None,
    };
    if let Some(time) = earliest(zone.from_local_datetime(&wall)) {
        return Some(time.with_timezone(&Utc));
    }
    for minutes in 1..=1500 {
        let before = earliest(zone.from_local_datetime(&(wall - Duration::minutes(minutes))));
        let after = earliest(zone.from_local_datetime(&(wall + Duration::minutes(minutes))));
        if let (Some(before), Some(after)) = (before, after) {
            let gap =
                after.offset().fix().local_minus_utc() - before.offset().fix().local_minus_utc();
            return earliest(zone.from_local_datetime(&(wall + Duration::seconds(i64::from(gap)))))
                .map(|t| t.with_timezone(&Utc));
        }
    }
    None
}

impl Schedule {
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let zone = self.tz.parse::<Tz>().ok()?;
        if self.repeat == Repeat::Once {
            return resolve(zone, self.start).filter(|t| *t > after);
        }
        let first = self.start.date();
        let mut date = after.with_timezone(&zone).date_naive().max(first);
        for _ in 0..146_100 {
            // Gregorian cycle, also bounds malformed/unreachable schedules.
            if date.year() > 9999 || self.until.is_some_and(|end| date > end) {
                return None;
            }
            let days = (date - first).num_days();
            let months =
                (date.year() - first.year()) * 12 + date.month() as i32 - first.month() as i32;
            let last_day = date
                .succ_opt()
                .is_none_or(|next| next.month() != date.month());
            let day_matches = if self.last_day {
                last_day
            } else {
                date.day() == first.day()
            };
            let matches = match self.repeat {
                Repeat::Once => false,
                Repeat::Daily => days % i64::from(self.every) == 0,
                Repeat::Weekly => {
                    ((days + i64::from(first.weekday().num_days_from_monday())) / 7)
                        % i64::from(self.every)
                        == 0
                        && self.weekdays.contains(&date.weekday().number_from_monday())
                }
                Repeat::Monthly => months % self.every as i32 == 0 && day_matches,
                Repeat::Yearly => {
                    (date.year() - first.year()) % self.every as i32 == 0
                        && date.month() == first.month()
                        && day_matches
                }
            };
            if matches
                && let Some(time) = resolve(zone, date.and_time(self.start.time()))
                && time > after
            {
                return Some(time);
            }
            date = date.succ_opt()?;
        }
        None
    }
    pub fn label(&self) -> String {
        let frequency = match self.repeat {
            Repeat::Once => "Once",
            Repeat::Daily => "Daily",
            Repeat::Weekly => "Weekly",
            Repeat::Monthly => "Monthly",
            Repeat::Yearly => "Yearly",
        };
        let mut parts = vec![
            self.start.format("%Y-%m-%d %H:%M").to_string(),
            frequency.to_string(),
        ];
        if self.every > 1 {
            parts.push(format!("every {} periods", self.every));
        }
        if !self.weekdays.is_empty() {
            parts.push(
                self.weekdays
                    .iter()
                    .filter_map(|day| {
                        ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
                            .get(day.saturating_sub(1) as usize)
                            .copied()
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if self.last_day {
            parts.push("last day of month".into());
        }
        if let Some(until) = self.until {
            parts.push(format!("until {until}"));
        }
        parts.push(self.tz.clone());
        parts.join(" · ")
    }
    pub fn hour(&self) -> u32 {
        self.start.hour()
    }
    pub fn minute(&self) -> u32 {
        self.start.minute()
    }
}
