//! Durable usage accounting and dashboard aggregation.
//!
//! Raw records are authoritative for recovery. Queries read the in-memory hourly
//! index, which is persisted by `flush_minute` (or its `flush` alias).

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use macbot_store::{Store, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("invalid usage parameters: {0}")]
    Invalid(String),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Totals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub requests: u64,
    pub cost: Option<f64>,
}
impl Totals {
    fn add(&mut self, other: &Self) {
        let first = self.requests == 0;
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.requests = self.requests.saturating_add(other.requests);
        self.cost = if first {
            other.cost
        } else {
            self.cost.zip(other.cost).map(|(a, b)| a + b)
        };
    }
    fn metric(&self, metric: &str) -> f64 {
        match metric {
            "cost" => self.cost.unwrap_or(0.0),
            "requests" => self.requests as f64,
            // Providers report cache reads/writes as a subset of input_tokens.
            _ => (self.input_tokens + self.output_tokens) as f64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Price {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    pub cache_write_per_mtok: f64,
}
impl Price {
    /// Input counts reported by providers include cache reads; bill those separately.
    pub fn cost(&self, usage: &Totals) -> f64 {
        let cached = usage
            .cache_read_tokens
            .saturating_add(usage.cache_write_tokens);
        (usage.input_tokens.saturating_sub(cached) as f64 * self.input_per_mtok
            + usage.output_tokens as f64 * self.output_per_mtok
            + usage.cache_read_tokens as f64 * self.cache_read_per_mtok
            + usage.cache_write_tokens as f64 * self.cache_write_per_mtok)
            / 1_000_000.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    /// Stable request ID prevents charging twice when a durable run resumes.
    pub request_id: String,
    pub ts: DateTime<Utc>,
    pub bot_id: String,
    pub project_id: Option<String>,
    pub chat_id: String,
    pub assignment_id: Option<String>,
    pub run_id: String,
    pub phase: String,
    pub provider_id: String,
    pub model_id: String,
    pub routine: bool,
    #[serde(flatten)]
    pub usage: Totals,
    pub task_done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HourlyAggregate {
    usage: Totals,
    task_ids: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct HourlyPoint {
    ts: DateTime<Utc>,
    bot_id: String,
    project_id: Option<String>,
    routine: bool,
    provider_id: String,
    model_id: String,
    phase: String,
    aggregate: HourlyAggregate,
}

#[derive(Clone)]
pub struct UsageLedger {
    store: Store,
    // Raw records are retained only for recovery and request-id deduplication.
    records: Vec<UsageRecord>,
    hourly: BTreeMap<String, HourlyAggregate>,
}
type BreakdownGroup = (Totals, BTreeMap<String, u64>, Vec<u64>);

impl UsageLedger {
    pub fn open(home: impl AsRef<Path>) -> Result<Self> {
        Self::from_store(Store::open(home)?)
    }

    pub fn from_store(store: Store) -> Result<Self> {
        let raw = store.root().join("data/usage/raw");
        let mut records = Vec::new();
        if raw.exists() {
            let mut files = std::fs::read_dir(&raw)
                .map_err(StoreError::Io)?
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(StoreError::Io)?;
            files.sort_by_key(|f| f.file_name());
            for file in files {
                if file.path().extension().is_some_and(|x| x == "jsonl") {
                    records.extend(store.read_jsonl::<UsageRecord>(file.path())?);
                }
            }
        }
        records.sort_by_key(|r| r.ts);
        let mut seen = std::collections::HashSet::new();
        records.retain(|r| seen.insert(r.request_id.clone()));
        let hourly = build_hourly(&records);
        let ledger = Self {
            store,
            records,
            hourly,
        };
        ledger.flush()?;
        Ok(ledger)
    }

    pub fn record(&mut self, record: UsageRecord) -> Result<bool> {
        if self
            .records
            .iter()
            .any(|r| r.request_id == record.request_id)
        {
            return Ok(false);
        }
        self.store.append_jsonl(
            format!("data/usage/raw/{}.jsonl", record.ts.format("%Y-%m-%d")),
            &record,
        )?;
        add_hourly_record(&mut self.hourly, &record);
        self.records.push(record);
        Ok(true)
    }

    /// Persist the current in-memory hourly index. Call this from the gateway's
    /// minute maintenance task; recording itself updates query results immediately.
    pub fn flush_minute(&self) -> Result<()> {
        self.flush()
    }

    /// Backwards-compatible name for callers that already flush periodically.
    pub fn flush(&self) -> Result<()> {
        let mut months: BTreeMap<String, BTreeMap<String, HourlyAggregate>> = BTreeMap::new();
        for (key, aggregate) in &self.hourly {
            let month = key
                .split('\x1f')
                .next()
                .and_then(|s| s.get(..7))
                .unwrap_or("1970-01")
                .to_owned();
            months
                .entry(month)
                .or_default()
                .insert(key.clone(), aggregate.clone());
        }
        for (month, data) in months {
            self.store
                .write_snapshot(format!("data/usage/hourly/{month}.json"), &data)?;
        }
        Ok(())
    }

    pub fn query(&self, method: &str, params: &Value, timezone: &str) -> Result<Value> {
        let from = parse_time(params, "from")?;
        let to = parse_time(params, "to")?;
        if to < from {
            return Err(Error::Invalid("to must not be before from".into()));
        }
        let tz: Tz = timezone
            .parse()
            .map_err(|_| Error::Invalid("unknown timezone".into()))?;
        let metric = params["metric"].as_str().unwrap_or("tokens");
        if !["tokens", "cost", "requests"].contains(&metric) {
            return Err(Error::Invalid("metric".into()));
        }

        if from == to {
            return empty_query(method, params, metric);
        }

        let records = self.points(from, to);
        match method {
            "usage.summary" => {
                let current = summarize(&records);
                let previous = summarize(&self.points(from - (to - from), from));
                Ok(json!({"current": current, "previous": previous}))
            }
            "usage.heatmap" => {
                if params["mode"].as_str() == Some("weekhour") {
                    let mut matrix = vec![vec![0.0; 24]; 7];
                    for r in &records {
                        let local = r.ts.with_timezone(&tz);
                        matrix[local.weekday().num_days_from_monday() as usize]
                            [local.hour() as usize] += r.aggregate.usage.metric(metric);
                    }
                    let values = matrix.iter().flatten().copied().collect();
                    Ok(json!({"matrix": matrix, "thresholds": thresholds(values)}))
                } else {
                    let mut days: BTreeMap<String, (Totals, BTreeMap<String, u64>)> =
                        BTreeMap::new();
                    let mut date = from.with_timezone(&tz).date_naive();
                    let end = (to - Duration::nanoseconds(1))
                        .with_timezone(&tz)
                        .date_naive();
                    while date <= end {
                        days.insert(date.to_string(), Default::default());
                        date = date
                            .succ_opt()
                            .ok_or_else(|| Error::Invalid("date overflow".into()))?;
                    }
                    for r in &records {
                        let day = days
                            .entry(r.ts.with_timezone(&tz).format("%Y-%m-%d").to_string())
                            .or_default();
                        day.0.add(&r.aggregate.usage);
                        *day.1.entry(r.bot_id.clone()).or_default() +=
                            r.aggregate.usage.metric("tokens") as u64;
                    }
                    let values = days.values().map(|x| x.0.metric(metric)).collect();
                    let days: Vec<_> = days
                        .into_iter()
                        .map(|(date, (usage, bots))| {
                            json!({
                                "date": date,
                                "value": usage.metric(metric),
                                "tokens": usage.metric("tokens"),
                                "cost": usage.cost,
                                "requests": usage.requests,
                                "top_bot_id": bots.into_iter().max_by_key(|x| x.1).map(|x| x.0)
                            })
                        })
                        .collect();
                    Ok(json!({"days": days, "thresholds": thresholds(values)}))
                }
            }
            "usage.timeseries" => self.timeseries(params, from, to, &tz, metric, &records),
            "usage.breakdown" => self.breakdown(params, from, to, &tz, &records),
            _ => Err(Error::Invalid("unknown method".into())),
        }
    }

    fn points(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<HourlyPoint> {
        let start = floor_hour(from);
        let end = ceil_hour(to);
        self.hourly
            .iter()
            .filter_map(|(key, aggregate)| {
                let point = parse_hourly_key(key, aggregate)?;
                (point.ts >= start && point.ts < end).then_some(point)
            })
            .collect()
    }

    fn timeseries(
        &self,
        params: &Value,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        tz: &Tz,
        metric: &str,
        records: &[HourlyPoint],
    ) -> Result<Value> {
        let granularity = choose_granularity(params, from, to)?;
        let buckets = make_buckets(from, to, tz, granularity)?;
        let bucket_index: BTreeMap<String, usize> = buckets
            .iter()
            .enumerate()
            .map(|(i, b)| (bucket_key(*b, tz, granularity), i))
            .collect();
        let dimension = params["dimension"].as_str().unwrap_or("bot");
        validate_dimension(dimension)?;
        let mut groups: BTreeMap<String, Vec<Totals>> = BTreeMap::new();
        for r in records {
            let key = dimension_key(r, dimension);
            let Some(&i) = bucket_index.get(&bucket_key_for_record(r, tz, granularity)) else {
                continue;
            };
            groups
                .entry(key)
                .or_insert_with(|| vec![Totals::default(); buckets.len()])[i]
                .add(&r.aggregate.usage);
        }
        let mut groups: Vec<_> = groups.into_iter().collect();
        groups.sort_by(|a, b| sum_metric(&b.1, metric).total_cmp(&sum_metric(&a.1, metric)));
        let top = params["top"].as_u64().unwrap_or(6).clamp(1, 100) as usize;
        if groups.len() > top {
            let rest = groups.split_off(top);
            let mut other = vec![Totals::default(); buckets.len()];
            for (_, items) in rest {
                for (i, usage) in items.iter().enumerate() {
                    other[i].add(usage);
                }
            }
            groups.push(("other".into(), other));
        }
        let split = params["split_io"].as_bool().unwrap_or(false);
        let series: Vec<_> = groups
            .into_iter()
            .map(|(key, items)| {
                json!({
                    "key": key,
                    "label": key,
                    "values": items.iter().map(|u| u.metric(metric)).collect::<Vec<_>>(),
                    "input_values": if split { Some(items.iter().map(|u| u.input_tokens).collect::<Vec<_>>()) } else { None::<Vec<u64>> },
                    "output_values": if split { Some(items.iter().map(|u| u.output_tokens).collect::<Vec<_>>()) } else { None::<Vec<u64>> },
                    "total": sum_metric(&items, metric)
                })
            })
            .collect();
        Ok(json!({"granularity": granularity, "buckets": buckets, "series": series}))
    }

    fn breakdown(
        &self,
        params: &Value,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        tz: &Tz,
        records: &[HourlyPoint],
    ) -> Result<Value> {
        let dimension = params["dimension"].as_str().unwrap_or("bot");
        validate_dimension(dimension)?;
        let first_date = from.with_timezone(tz).date_naive();
        let last_date = (to - Duration::nanoseconds(1))
            .with_timezone(tz)
            .date_naive();
        let day_count = (last_date - first_date).num_days() as usize + 1;
        let drill = params.get("drill").and_then(Value::as_object);
        let mut groups: BTreeMap<String, BreakdownGroup> = BTreeMap::new();
        for r in records {
            if drill
                .and_then(|d| d.get("bot_id"))
                .and_then(Value::as_str)
                .is_some_and(|id| id != r.bot_id)
                || drill
                    .and_then(|d| d.get("project_id"))
                    .and_then(Value::as_str)
                    .is_some_and(|id| Some(id) != r.project_id.as_deref())
            {
                continue;
            }
            let key = dimension_key(r, dimension);
            let group = groups.entry(key).or_insert_with(|| {
                (
                    Totals::default(),
                    [
                        "chat",
                        "work",
                        "subagent",
                        "coordinate",
                        "memory",
                        "compact",
                    ]
                    .into_iter()
                    .map(|s| (s.into(), 0))
                    .collect(),
                    vec![0; day_count],
                )
            });
            group.0.add(&r.aggregate.usage);
            *group.1.entry(r.phase.clone()).or_default() +=
                r.aggregate.usage.metric("tokens") as u64;
            let day = (r.ts.with_timezone(tz).date_naive() - first_date).num_days() as usize;
            if day < group.2.len() {
                group.2[day] += r.aggregate.usage.metric("tokens") as u64;
            }
        }
        let rows: Vec<_> = groups
            .into_iter()
            .map(|(key, (usage, phases, sparkline))| {
                json!({"key": key, "label": key, "usage": usage, "sparkline": sparkline, "phases": phases})
            })
            .collect();
        Ok(json!({"rows": rows}))
    }

    pub fn export_csv(&self, params: &Value, timezone: &str) -> Result<String> {
        let value = self.query("usage.breakdown", params, timezone)?;
        let mut text =
            "key,label,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,requests,cost\n"
                .to_owned();
        for row in value["rows"].as_array().unwrap_or(&Vec::new()) {
            let safe = |s: &str| {
                format!(
                    "\"{}{}\"",
                    if s.starts_with(['=', '+', '-', '@']) {
                        "'"
                    } else {
                        ""
                    },
                    s.replace('"', "\"\"")
                )
            };
            text.push_str(&format!(
                "{},{},{},{},{},{},{},{}\n",
                safe(row["key"].as_str().unwrap_or("")),
                safe(row["label"].as_str().unwrap_or("")),
                row["usage"]["input_tokens"],
                row["usage"]["output_tokens"],
                row["usage"]["cache_read_tokens"],
                row["usage"]["cache_write_tokens"],
                row["usage"]["requests"],
                row["usage"]["cost"]
                    .as_f64()
                    .map(|n| n.to_string())
                    .unwrap_or_default()
            ));
        }
        Ok(text)
    }
}

fn build_hourly(records: &[UsageRecord]) -> BTreeMap<String, HourlyAggregate> {
    let mut hourly = BTreeMap::new();
    for record in records {
        add_hourly_record(&mut hourly, record);
    }
    hourly
}

fn add_hourly_record(hourly: &mut BTreeMap<String, HourlyAggregate>, record: &UsageRecord) {
    let key = hourly_key(record.ts, record);
    let aggregate = hourly.entry(key).or_default();
    aggregate.usage.add(&record.usage);
    if record.task_done {
        aggregate.task_ids.insert(
            record
                .assignment_id
                .clone()
                .unwrap_or_else(|| record.run_id.clone()),
        );
    }
}

fn hourly_key(ts: DateTime<Utc>, record: &UsageRecord) -> String {
    format!(
        "{}\x1f{}\x1f{}\x1f{}\x1f{}",
        floor_hour(ts).to_rfc3339(),
        record.bot_id,
        record
            .project_id
            .as_deref()
            .unwrap_or(if record.routine { "routine" } else { "dm" }),
        record.provider_id,
        record.model_id
    ) + &format!("\x1f{}", record.phase)
}

fn parse_hourly_key(key: &str, aggregate: &HourlyAggregate) -> Option<HourlyPoint> {
    let mut parts = key.split('\x1f');
    let ts = parts.next()?.parse().ok()?;
    let bot_id = parts.next()?.to_owned();
    let project = parts.next()?.to_owned();
    let provider_id = parts.next()?.to_owned();
    let model_id = parts.next()?.to_owned();
    let phase = parts.next()?.to_owned();
    let routine = project == "routine";
    Some(HourlyPoint {
        ts,
        bot_id,
        project_id: if project == "routine" || project == "dm" {
            None
        } else {
            Some(project)
        },
        routine,
        provider_id,
        model_id,
        phase,
        aggregate: aggregate.clone(),
    })
}

fn summarize(records: &[HourlyPoint]) -> Value {
    let mut usage = Totals::default();
    let mut task_ids = BTreeSet::new();
    for record in records {
        usage.add(&record.aggregate.usage);
        task_ids.extend(record.aggregate.task_ids.iter().cloned());
    }
    let mut value = serde_json::to_value(usage).expect("totals serialize");
    value["tasks_done"] = json!(task_ids.len());
    value
}

fn parse_time(params: &Value, key: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(
        params[key]
            .as_str()
            .ok_or_else(|| Error::Invalid(key.into()))?,
    )
    .map(|t| t.with_timezone(&Utc))
    .map_err(|_| Error::Invalid(key.into()))
}

fn validate_dimension(dimension: &str) -> Result<()> {
    if ["bot", "project", "model"].contains(&dimension) {
        Ok(())
    } else {
        Err(Error::Invalid("dimension".into()))
    }
}

fn dimension_key(record: &HourlyPoint, dimension: &str) -> String {
    match dimension {
        "model" => format!("{}/{}", record.provider_id, record.model_id),
        "project" => record
            .project_id
            .clone()
            .unwrap_or_else(|| if record.routine { "routine" } else { "dm" }.into()),
        _ => record.bot_id.clone(),
    }
}

fn floor_hour(at: DateTime<Utc>) -> DateTime<Utc> {
    at.date_naive()
        .and_hms_opt(at.hour(), 0, 0)
        .unwrap()
        .and_utc()
}

fn ceil_hour(at: DateTime<Utc>) -> DateTime<Utc> {
    let floor = floor_hour(at);
    if floor == at {
        floor
    } else {
        floor + Duration::hours(1)
    }
}

fn choose_granularity(
    params: &Value,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<&'static str> {
    Ok(match params["granularity"].as_str().unwrap_or("auto") {
        "auto" => {
            if to - from <= Duration::days(7) {
                "hour"
            } else if to - from <= Duration::days(90) {
                "day"
            } else {
                "week"
            }
        }
        "hour" => "hour",
        "day" => "day",
        "week" => "week",
        _ => return Err(Error::Invalid("granularity".into())),
    })
}

fn local_midnight(date: NaiveDate, tz: &Tz) -> DateTime<Utc> {
    let naive = date.and_hms_opt(0, 0, 0).unwrap();
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(value) | LocalResult::Ambiguous(value, _) => value.with_timezone(&Utc),
        LocalResult::None => tz
            .from_local_datetime(&(naive + Duration::hours(1)))
            .earliest()
            .unwrap()
            .with_timezone(&Utc),
    }
}

fn week_start(date: NaiveDate) -> NaiveDate {
    date - Duration::days(date.weekday().num_days_from_monday() as i64)
}

fn make_buckets(
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    tz: &Tz,
    granularity: &str,
) -> Result<Vec<DateTime<Utc>>> {
    let mut buckets = Vec::new();
    match granularity {
        "hour" => {
            let mut at = floor_hour(from);
            while at < to {
                buckets.push(at);
                at += Duration::hours(1);
                if buckets.len() > 100_000 {
                    return Err(Error::Invalid("range too large".into()));
                }
            }
        }
        "day" | "week" => {
            let first = from.with_timezone(tz).date_naive();
            let last = (to - Duration::nanoseconds(1))
                .with_timezone(tz)
                .date_naive();
            let step = if granularity == "day" { 1 } else { 7 };
            let mut date = if granularity == "day" {
                first
            } else {
                week_start(first)
            };
            let end = if granularity == "day" {
                last
            } else {
                week_start(last)
            };
            while date <= end {
                buckets.push(local_midnight(date, tz));
                date += Duration::days(step);
                if buckets.len() > 100_000 {
                    return Err(Error::Invalid("range too large".into()));
                }
            }
        }
        _ => unreachable!(),
    }
    Ok(buckets)
}

fn bucket_key(bucket: DateTime<Utc>, tz: &Tz, granularity: &str) -> String {
    if granularity == "hour" {
        bucket.to_rfc3339()
    } else {
        let date = bucket.with_timezone(tz).date_naive();
        if granularity == "week" {
            week_start(date).to_string()
        } else {
            date.to_string()
        }
    }
}

fn bucket_key_for_record(record: &HourlyPoint, tz: &Tz, granularity: &str) -> String {
    if granularity == "hour" {
        floor_hour(record.ts).to_rfc3339()
    } else {
        let date = record.ts.with_timezone(tz).date_naive();
        if granularity == "week" {
            week_start(date).to_string()
        } else {
            date.to_string()
        }
    }
}

fn sum_metric(totals: &[Totals], metric: &str) -> f64 {
    totals.iter().map(|u| u.metric(metric)).sum()
}

fn thresholds(mut values: Vec<f64>) -> [f64; 3] {
    values.retain(|v| *v > 0.0 && v.is_finite());
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return [0.0; 3];
    }
    [
        values[(values.len() - 1) / 4],
        values[(values.len() - 1) / 2],
        values[(values.len() - 1) * 3 / 4],
    ]
}

fn empty_query(method: &str, params: &Value, metric: &str) -> Result<Value> {
    match method {
        "usage.summary" => Ok(
            json!({"current": {"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,"cache_write_tokens":0,"requests":0,"cost":null,"tasks_done":0}, "previous": {"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,"cache_write_tokens":0,"requests":0,"cost":null,"tasks_done":0}}),
        ),
        "usage.heatmap" if params["mode"].as_str() == Some("weekhour") => {
            Ok(json!({"matrix": vec![vec![0.0;24];7], "thresholds":[0.0,0.0,0.0]}))
        }
        "usage.heatmap" => Ok(json!({"days":[], "thresholds":[0.0,0.0,0.0]})),
        "usage.timeseries" => {
            let granularity = match params["granularity"].as_str().unwrap_or("auto") {
                "day" => "day",
                "week" => "week",
                _ => "hour",
            };
            Ok(json!({"granularity":granularity, "buckets":[], "series":[]}))
        }
        "usage.breakdown" => Ok(json!({"rows":[]})),
        _ => {
            let _ = metric;
            Err(Error::Invalid("unknown method".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, time: &str, bot: &str, cost: Option<f64>) -> UsageRecord {
        UsageRecord {
            request_id: id.into(),
            ts: time.parse().unwrap(),
            bot_id: bot.into(),
            project_id: None,
            chat_id: "chat".into(),
            assignment_id: Some(id.into()),
            run_id: id.into(),
            phase: "work".into(),
            provider_id: "p".into(),
            model_id: "m".into(),
            routine: false,
            usage: Totals {
                input_tokens: 100,
                output_tokens: 50,
                requests: 1,
                cost,
                ..Default::default()
            },
            task_done: true,
        }
    }

    fn params() -> Value {
        json!({"from":"2026-10-09T00:00:00Z","to":"2026-10-10T00:00:00Z","dimension":"bot","metric":"tokens","granularity":"auto"})
    }

    #[test]
    fn rebuild_and_request_idempotence() {
        let home = tempfile::tempdir().unwrap();
        {
            let mut ledger = UsageLedger::open(home.path()).unwrap();
            assert!(ledger
                .record(record("1", "2026-10-09T01:00:00Z", "bot", Some(0.1)))
                .unwrap());
            assert!(!ledger
                .record(record("1", "2026-10-09T01:00:00Z", "bot", Some(0.1)))
                .unwrap());
        }
        let ledger = UsageLedger::open(home.path()).unwrap();
        let result = ledger
            .query("usage.summary", &params(), "Asia/Shanghai")
            .unwrap();
        assert_eq!(result["current"]["requests"], 1);
        assert_eq!(result["current"]["tasks_done"], 1);
        assert_eq!(result["current"]["cost"], 0.1);
    }

    #[test]
    fn unknown_price_propagates_and_heatmap_uses_host_timezone() {
        let home = tempfile::tempdir().unwrap();
        let mut ledger = UsageLedger::open(home.path()).unwrap();
        ledger
            .record(record("1", "2026-10-09T23:00:00Z", "a", None))
            .unwrap();
        ledger
            .record(record("2", "2026-10-09T23:10:00Z", "b", Some(0.1)))
            .unwrap();
        assert!(ledger
            .query("usage.summary", &params(), "Asia/Shanghai")
            .unwrap()["current"]["cost"]
            .is_null());
        let result = ledger
            .query("usage.heatmap", &params(), "Asia/Shanghai")
            .unwrap();
        let days = result["days"].as_array().unwrap();
        assert_eq!(days[1]["date"], "2026-10-10");
        assert_eq!(days[1]["tokens"], 300.0);
    }

    #[test]
    fn top_series_is_folded_into_other() {
        let home = tempfile::tempdir().unwrap();
        let mut ledger = UsageLedger::open(home.path()).unwrap();
        for id in ["a", "b", "c"] {
            ledger
                .record(record(id, "2026-10-09T01:00:00Z", id, Some(0.1)))
                .unwrap();
        }
        let mut params = params();
        params["top"] = json!(1);
        let value = ledger.query("usage.timeseries", &params, "UTC").unwrap();
        assert_eq!(value["series"].as_array().unwrap().len(), 2);
        assert_eq!(value["series"][1]["key"], "other");
        assert_eq!(value["series"][1]["total"], 300.0);
        assert_eq!(value["buckets"].as_array().unwrap().len(), 24);
    }

    #[test]
    fn cache_is_a_subset_of_input_and_price_is_separate() {
        let usage = Totals {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 30,
            cache_write_tokens: 10,
            requests: 1,
            ..Default::default()
        };
        let price = Price {
            input_per_mtok: 1.0,
            output_per_mtok: 2.0,
            cache_read_per_mtok: 3.0,
            cache_write_per_mtok: 4.0,
        };
        assert_eq!(usage.metric("tokens"), 120.0);
        assert!((price.cost(&usage) - 0.00023).abs() < f64::EPSILON);
    }

    #[test]
    fn timezone_and_dst_share_local_day() {
        let home = tempfile::tempdir().unwrap();
        let mut ledger = UsageLedger::open(home.path()).unwrap();
        ledger
            .record(record("a", "2026-11-01T05:30:00Z", "bot", Some(0.1)))
            .unwrap();
        ledger
            .record(record("b", "2026-11-01T06:30:00Z", "bot", Some(0.1)))
            .unwrap();
        let p = json!({"from":"2026-11-01T00:00:00Z","to":"2026-11-02T00:00:00Z","granularity":"day","dimension":"bot","metric":"tokens"});
        let value = ledger
            .query("usage.timeseries", &p, "America/New_York")
            .unwrap();
        assert_eq!(value["buckets"].as_array().unwrap().len(), 2);
        assert_eq!(value["series"][0]["values"][1], 300.0);
    }

    #[test]
    fn empty_interval_and_percentiles_are_safe() {
        let home = tempfile::tempdir().unwrap();
        let ledger = UsageLedger::open(home.path()).unwrap();
        let p = json!({"from":"2026-10-09T00:00:00Z","to":"2026-10-09T00:00:00Z","dimension":"bot","metric":"tokens"});
        let value = ledger.query("usage.summary", &p, "UTC").unwrap();
        assert_eq!(value["current"]["requests"], 0);
        assert_eq!(thresholds(vec![1.0, 2.0, 3.0, 4.0]), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn summary_has_previous_window_and_minute_flush_persists_hourly() {
        let home = tempfile::tempdir().unwrap();
        let mut ledger = UsageLedger::open(home.path()).unwrap();
        ledger
            .record(record("previous", "2026-10-08T01:00:00Z", "bot", Some(0.1)))
            .unwrap();
        ledger
            .record(record("current", "2026-10-09T01:00:00Z", "bot", Some(0.2)))
            .unwrap();
        let value = ledger.query("usage.summary", &params(), "UTC").unwrap();
        assert_eq!(value["current"]["requests"], 1);
        assert_eq!(value["previous"]["requests"], 1);
        ledger.flush_minute().unwrap();
        let snapshot = home.path().join("data/usage/hourly/2026-10.json");
        assert!(snapshot.exists());
    }

    #[test]
    fn default_top_is_six_with_other_series() {
        let home = tempfile::tempdir().unwrap();
        let mut ledger = UsageLedger::open(home.path()).unwrap();
        for id in ["a", "b", "c", "d", "e", "f", "g"] {
            ledger
                .record(record(id, "2026-10-09T01:00:00Z", id, Some(0.1)))
                .unwrap();
        }
        let value = ledger.query("usage.timeseries", &params(), "UTC").unwrap();
        assert_eq!(value["series"].as_array().unwrap().len(), 7);
        assert_eq!(value["series"][6]["key"], "other");
    }
}
