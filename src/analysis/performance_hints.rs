//! Deterministic, evidence-backed hints shared by HTML, classic AI and MCP.
//! A hint is a hypothesis: scan counters cannot distinguish sparse blocks from data growth.
use crate::awr::{AWRSCollection, SegmentStats, AWR};
use crate::measurements::{self, DbLoadMetric};
use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const RULE_ID: &str = "possible_table_fragmentation";
pub const VERSION: &str = "2026-09-14.2";
const CONTINUATION_SCOPE: &str = "instance:row_continuation";
const CONTINUED: &str = "table fetch continued row";
const ROWID: &str = "table fetch by rowid";
pub const NO_SEGMENTS: &str = "No conclusive data to identify affected segments.";
const BLOCKS: &str = "table scan blocks gotten";
const SHORT: &str = "table scans (short tables)";
const LONG: &str = "table scans (long tables)";
const PATH_COUNTERS: &[&str] = &[
    "table scans (direct read)",
    "table scans (rowid ranges)",
    "table scans (cache partitions)",
    "table scans (IM)",
];
const CAUTION: &str = "Observed access cost is not proof of physical fragmentation. Sparse table/index blocks, row chaining/migration, data growth, bind selectivity, changed plans and CR/undo are alternatives. No recoverable DB Time is proven.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// Optional explicit baseline, using the same inclusive interval bounds as --snap-range.
    pub baseline_snap_range: Option<(u64, u64)>,
    /// Maximum comparable historical observations, not consecutive clock hours.
    pub baseline_windows: usize,
    pub minimum_baseline_windows: usize,
    pub minimum_recurrences: usize,
    pub noise_multiplier: f64,
    /// Long collections use same-hour/day-type cohorts; short collections use local history.
    pub seasonal_minimum_hours: f64,
    pub maximum_scan_mix_change: f64,
    pub maximum_scans_per_execution_factor: f64,
    pub persistent_minimum_cpu_seconds: f64,
    pub persistent_minimum_gets: f64,
    pub persistent_minimum_cpu_share: f64,
    pub consecutive_windows: usize,
    pub minimum_period_seconds: f64,
    pub minimum_scan_starts: f64,
    pub minimum_rowid_fetches: f64,
    pub minimum_continuation_delta_per_rowid: f64,
    pub minimum_segment_excess_reads_per_second: f64,
    pub minimum_growth_pct: f64,
    pub minimum_extra_blocks_per_second: f64,
    pub workload_band_pct: f64,
    pub maximum_baseline_ratio_spread: f64,
    pub minimum_cost_growth_pct: f64,
    pub minimum_db_cost_delta_s_per_second: f64,
    pub minimum_sql_cost_delta_s_per_execution: f64,
    pub minimum_top_coverage: f64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            baseline_snap_range: None,
            baseline_windows: 10,
            minimum_baseline_windows: 3,
            minimum_recurrences: 2,
            noise_multiplier: 3.0,
            seasonal_minimum_hours: 24.0,
            maximum_scan_mix_change: 0.15,
            maximum_scans_per_execution_factor: 2.0,
            persistent_minimum_cpu_seconds: 60.0,
            persistent_minimum_gets: 1_000_000.0,
            persistent_minimum_cpu_share: 0.01,
            consecutive_windows: 3,
            minimum_period_seconds: 120.0,
            minimum_scan_starts: 30.0,
            minimum_rowid_fetches: 30.0,
            minimum_continuation_delta_per_rowid: 0.01,
            minimum_segment_excess_reads_per_second: 100.0,
            minimum_growth_pct: 25.0,
            minimum_extra_blocks_per_second: 100.0,
            workload_band_pct: 20.0,
            maximum_baseline_ratio_spread: 1.5,
            minimum_cost_growth_pct: 15.0,
            minimum_db_cost_delta_s_per_second: 0.01,
            minimum_sql_cost_delta_s_per_execution: 0.0001,
            minimum_top_coverage: 0.8,
        }
    }
}

impl Policy {
    pub fn validate(&self) -> Result<(), String> {
        let positive = [
            self.minimum_period_seconds,
            self.minimum_scan_starts,
            self.minimum_rowid_fetches,
            self.minimum_continuation_delta_per_rowid,
            self.minimum_segment_excess_reads_per_second,
            self.minimum_growth_pct,
            self.minimum_extra_blocks_per_second,
            self.minimum_cost_growth_pct,
            self.minimum_db_cost_delta_s_per_second,
            self.minimum_sql_cost_delta_s_per_execution,
        ];
        if self.minimum_baseline_windows < 3
            || self.baseline_windows < self.minimum_baseline_windows
            || self.minimum_recurrences < 2
            || self.minimum_recurrences > self.consecutive_windows
            || !self.noise_multiplier.is_finite()
            || self.noise_multiplier <= 0.0
            || !self.seasonal_minimum_hours.is_finite()
            || self.seasonal_minimum_hours < 1.0
            || !self.maximum_scans_per_execution_factor.is_finite()
            || self.maximum_scans_per_execution_factor <= 1.0
            || !self.maximum_scan_mix_change.is_finite()
            || !(0.0..=1.0).contains(&self.maximum_scan_mix_change)
            || !self.persistent_minimum_cpu_seconds.is_finite()
            || self.persistent_minimum_cpu_seconds <= 0.0
            || !self.persistent_minimum_gets.is_finite()
            || self.persistent_minimum_gets <= 0.0
            || !self.persistent_minimum_cpu_share.is_finite()
            || !(0.0..=1.0).contains(&self.persistent_minimum_cpu_share)
            || self.consecutive_windows < 3
            || positive.iter().any(|x| !x.is_finite() || *x <= 0.0)
            || !self.workload_band_pct.is_finite()
            || !(0.0..100.0).contains(&self.workload_band_pct)
            || !self.maximum_baseline_ratio_spread.is_finite()
            || self.maximum_baseline_ratio_spread < 1.0
            || !self.minimum_top_coverage.is_finite()
            || !(0.5..=1.0).contains(&self.minimum_top_coverage)
            || self.baseline_snap_range.is_some_and(|(b, e)| b >= e)
        {
            return Err("Invalid HINTS policy: at least 3 baseline/consecutive windows, positive finite thresholds, workload band in [0,100), baseline spread >= 1 and TOP coverage in [0.5,1] are required".into());
        }
        Ok(())
    }
    pub fn load(path: &str) -> Result<Self, String> {
        let p: Self = if path.is_empty() {
            Self::default()
        } else {
            serde_json::from_str(
                &std::fs::read_to_string(path).map_err(|e| format!("HINTS policy: {e}"))?,
            )
            .map_err(|e| format!("HINTS policy: {e}"))?
        };
        p.validate()?;
        Ok(p)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PerformanceHintsReport {
    pub version: String,
    pub policy: Policy,
    pub policy_status: String,
    #[serde(default)]
    pub policy_notes: Vec<String>,
    pub hints: Vec<PerformanceHint>,
    pub rule_evaluations: Vec<RuleEvaluation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleEvaluation {
    pub rule_id: String,
    pub status: String,
    pub selected_windows: usize,
    pub assessed_comparisons: usize,
    pub excluded_windows: usize,
    pub reasons: BTreeMap<String, usize>,
    #[serde(default)]
    pub scope_evaluations: Vec<ScopeEvaluation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Period {
    pub begin_snap_id: u64,
    pub end_snap_id: u64,
    pub begin_time: String,
    pub end_time: String,
    pub windows: usize,
    pub seconds: f64,
    /// Exact observed intervals: the enclosing range may contain nights, gaps and other profiles.
    #[serde(default)]
    pub intervals: Vec<(u64, u64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comparison {
    pub metric: String,
    pub unit: String,
    pub baseline: f64,
    pub recent: f64,
    pub delta: f64,
    pub growth_pct: Option<f64>,
    pub baseline_observations: usize,
    pub recent_observations: usize,
    pub source: String,
}
impl Comparison {
    fn new(metric: &str, unit: &str, b: f64, r: f64, bn: usize, rn: usize, source: &str) -> Self {
        Self {
            metric: metric.into(),
            unit: unit.into(),
            baseline: b,
            recent: r,
            delta: r - b,
            growth_pct: (b > 0.0)
                .then(|| (r / b - 1.0) * 100.0)
                .filter(|v| v.is_finite()),
            baseline_observations: bn,
            recent_observations: rn,
            source: source.into(),
        }
    }
    fn increased(&self, growth: f64, delta: f64) -> bool {
        self.growth_pct.is_some_and(|g| g >= growth) && self.delta >= delta
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlEvidence {
    pub sql_id: String,
    pub costs: Vec<Comparison>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentCandidate {
    pub segment: SegmentStats,
    pub logical_reads: Comparison,
    pub identity_status: String,
    pub attribution: String,
}

/// A measurement over one observed period, not a baseline/recent comparison.
/// Keeping the numerator and its own exposure makes the ratio auditable without
/// manufacturing zero growth (which would wrongly imply stability was tested).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub metric: String,
    pub unit: String,
    pub value: f64,
    pub total: f64,
    pub exposure: f64,
    pub exposure_unit: String,
    pub period: Period,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentObservation {
    pub segment: SegmentStats,
    pub logical_reads: Observation,
    pub identity_status: String,
    pub attribution: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceHint {
    pub hint_id: String,
    pub rule_id: String,
    pub rule_version: String,
    pub title: String,
    pub dbid: u64,
    pub instance_id: u8,
    pub episode_status: String,
    pub episode: Period,
    /// Absent for an absolute cost observation: the period was not compared to itself.
    pub baseline: Option<Period>,
    pub comparison: Option<Period>,
    pub evidence: Vec<Comparison>,
    pub sql_evidence: Vec<SqlEvidence>,
    pub sql_candidates_total: usize,
    pub possibly_affected_segments: Vec<SegmentCandidate>,
    pub segment_candidates_total: usize,
    pub segment_message: String,
    pub limitations: Vec<String>,
    pub next_check: String,
    #[serde(default)]
    pub assessment_status: String,
    #[serde(default)]
    pub confidence: String,
    #[serde(default)]
    pub assessment_scope: String,
    #[serde(default)]
    pub mechanisms: Vec<String>,
    #[serde(default)]
    pub supporting_evidence: Vec<String>,
    #[serde(default)]
    pub counterevidence: Vec<String>,
    #[serde(default)]
    pub alternative_explanations: Vec<String>,
    #[serde(default)]
    pub comparisons: Vec<Assessment>,
    #[serde(default)]
    pub plan_objects: Vec<PlanObject>,
    #[serde(default)]
    pub observed_cost_seconds: f64,
    #[serde(default)]
    pub observed_metrics: Vec<Observation>,
    #[serde(default)]
    pub observed_segments: Vec<SegmentObservation>,
    #[serde(default)]
    pub signal_kind: String,
    #[serde(default)]
    pub time_impact_status: String,
    #[serde(default)]
    pub context_evidence: Vec<ContextEvidence>,
    #[serde(default)]
    pub segment_context: Vec<SegmentContext>,
    #[serde(default)]
    pub sql_controls: Vec<SqlEvidence>,
    #[serde(default)]
    pub trajectory: Vec<TrajectoryPoint>,
}

impl PerformanceHint {
    fn is_cost_observation(&self) -> bool {
        self.assessment_status == "persistent_cost_observation"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScopeEvaluation {
    pub scope: String,
    pub profile: String,
    pub observed_opportunities: usize,
    pub missing_observations: usize,
    pub assessed_comparisons: usize,
    pub status: String,
    pub reasons: BTreeMap<String, usize>,
    #[serde(default)]
    pub cpu_observations: usize,
    #[serde(default)]
    pub elapsed_observations: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assessment {
    pub profile: String,
    pub baseline_selection: String,
    pub baseline: Period,
    pub recent: Period,
    pub work: Comparison,
    pub cost: Option<Comparison>,
    pub work_threshold_pct: f64,
    pub cost_threshold_pct: Option<f64>,
    #[serde(default)]
    pub cost_baseline_intervals: Vec<(u64, u64)>,
    pub recurring: bool,
    pub baseline_sufficient: bool,
    pub extra_observed_cost_seconds: f64,
    pub limitations: Vec<String>,
    #[serde(default)]
    pub cost_evaluations: Vec<CostEvaluation>,
    #[serde(default)]
    pub time_impact_status: String,
}

/// Each time domain has its own observed baseline, independent of TOP gets.
/// An absent comparison is unknown; it is never serialized as a zero cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEvaluation {
    pub domain: String,
    pub status: String,
    pub comparison: Option<Comparison>,
    pub baseline: Option<Period>,
    pub baseline_observations: usize,
    pub recent_observations: usize,
    pub baseline_sufficient: bool,
    pub threshold_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEvidence {
    pub scope: String,
    pub label: String,
    pub status: String,
    pub comparison: Option<Comparison>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentContext {
    pub segment: SegmentStats,
    pub logical_reads: Comparison,
    pub activity_growth_factor: Option<f64>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryPoint {
    pub period: Period,
    pub profile: String,
    pub work: Option<f64>,
    pub baseline: Option<f64>,
    pub threshold: Option<f64>,
    pub status: String,
}

#[derive(Default)]
struct Evaluation {
    assessments: Vec<Assessment>,
    trajectory: Vec<TrajectoryPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanObject {
    pub sql_id: String,
    pub plan_hash: String,
    pub object_name: String,
    pub operation: String,
    pub attribution: String,
}

fn stat(a: &AWR, name: &str) -> Option<f64> {
    if !measurements::available(a, "instance_stats", !a.instance_stats.is_empty()) {
        return None;
    }
    let mut rows = a.instance_stats.iter().filter(|r| r.statname == name);
    let value = rows.next()?.total as f64;
    rows.next().is_none().then_some(value)
}
fn scans(a: &AWR) -> Option<f64> {
    Some(stat(a, SHORT)? + stat(a, LONG)?)
}
fn scan_ratio(a: &AWR) -> Option<f64> {
    let n = scans(a)?;
    (n > 0.0).then(|| stat(a, BLOCKS).map(|b| b / n)).flatten()
}
fn period(rows: &[&AWR]) -> Period {
    Period {
        begin_snap_id: rows[0].snap_info.begin_snap_id,
        end_snap_id: rows.last().unwrap().snap_info.end_snap_id,
        begin_time: rows[0].snap_info.begin_snap_time.clone(),
        end_time: rows.last().unwrap().snap_info.end_snap_time.clone(),
        windows: rows.len(),
        seconds: rows.iter().filter_map(|a| measurements::seconds(a)).sum(),
        intervals: rows
            .iter()
            .map(|a| (a.snap_info.begin_snap_id, a.snap_info.end_snap_id))
            .collect(),
    }
}
fn median(v: &[f64]) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        v[n / 2 - 1] / 2.0 + v[n / 2] / 2.0
    }
}
fn ratio_of_sums(rows: &[&AWR]) -> f64 {
    rows.iter().filter_map(|a| stat(a, BLOCKS)).sum::<f64>()
        / rows.iter().filter_map(|a| scans(a)).sum::<f64>()
}
fn segment_key(s: &SegmentStats) -> String {
    serde_json::to_string(&(
        s.con_id,
        &s.pdb_name,
        &s.owner,
        &s.object_name,
        &s.subobject_name,
        &s.object_type,
        s.obj,
        s.objd,
    ))
    .unwrap()
}

/// Costs and denominators belong to their own AWR domain. An absent TOP entry,
/// disabled collection or an unfinished execution is not an observed zero cost.
fn sql_value(a: &AWR, id: &str, domain: &str) -> Option<(f64, f64)> {
    let value = match domain {
        "sql_gets" => a.sql_gets.get(id).map(|v| (v.buffer_gets, v.executions)),
        "sql_cpu_time" => a.sql_cpu_time.get(id).map(|v| (v.cpu_time_s, v.executions)),
        "sql_elapsed_time" => {
            let mut values = a.sql_elapsed_time.iter().filter(|v| v.sql_id == id);
            let v = values.next()?;
            if values.next().is_some() {
                return None;
            }
            Some((v.elapsed_time_s, v.executions))
        }
        _ => None,
    }?;
    (measurements::available(a, domain, true)
        && value.0.is_finite()
        && value.0 >= 0.0
        && value.1 > 0)
        .then_some((value.0, value.1 as f64))
}

#[derive(Clone)]
struct Sample<'a> {
    a: &'a AWR,
    work: f64,
    cpu: Option<f64>,
    elapsed: Option<f64>,
    cpu_total: Option<f64>,
    elapsed_total: Option<f64>,
}
fn sql_sample<'a>(a: &'a AWR, id: &str) -> Option<Sample<'a>> {
    let (work, executions) = sql_value(a, id, "sql_gets")?;
    let cpu = sql_value(a, id, "sql_cpu_time");
    let elapsed = sql_value(a, id, "sql_elapsed_time");
    Some(Sample {
        a,
        work: work / executions,
        cpu: cpu.map(|(v, e)| v / e),
        elapsed: elapsed.map(|(v, e)| v / e),
        cpu_total: cpu.map(|(v, _)| v),
        elapsed_total: elapsed.map(|(v, _)| v),
    })
}
fn instance_sample<'a>(a: &'a AWR, p: &Policy) -> Option<Sample<'a>> {
    let work = scan_ratio(a)?;
    if scans(a)? < p.minimum_scan_starts {
        return None;
    }
    let seconds = measurements::seconds(a)?;
    // Normalize by executions when supplied. The fallback is explicitly a rate,
    // never a fabricated execution count; separate profiles keep both units apart.
    let denominator = stat(a, "execute count")
        .filter(|e| *e > 0.0)
        .unwrap_or(seconds);
    let cpu = measurements::db_load_rate(a, DbLoadMetric::DbCpu).map(|v| v.per_second * seconds);
    let elapsed =
        measurements::db_load_rate(a, DbLoadMetric::DbTime).map(|v| v.per_second * seconds);
    Some(Sample {
        a,
        work,
        cpu: cpu.map(|v| v / denominator),
        elapsed: elapsed.map(|v| v / denominator),
        cpu_total: cpu,
        elapsed_total: elapsed,
    })
}
fn scope_sample<'a>(a: &'a AWR, scope: &str, p: &Policy) -> Option<Sample<'a>> {
    if let Some(id) = scope.strip_prefix("sql:") {
        return sql_sample(a, id);
    }
    if scope != CONTINUATION_SCOPE {
        return instance_sample(a, p);
    }
    let rowid = stat(a, ROWID)?;
    if rowid < p.minimum_rowid_fetches {
        return None;
    }
    let work = stat(a, CONTINUED)? / rowid;
    let cpu = time_value(a, scope, "CPU");
    let elapsed = time_value(a, scope, "elapsed");
    Some(Sample {
        a,
        work,
        cpu: cpu.map(|x| x.0 / x.1),
        elapsed: elapsed.map(|x| x.0 / x.1),
        cpu_total: cpu.map(|x| x.0),
        elapsed_total: elapsed.map(|x| x.0),
    })
}

fn time_value(a: &AWR, scope: &str, domain: &str) -> Option<(f64, f64)> {
    if let Some(id) = scope.strip_prefix("sql:") {
        return sql_value(
            a,
            id,
            if domain == "CPU" {
                "sql_cpu_time"
            } else {
                "sql_elapsed_time"
            },
        );
    }
    let seconds = measurements::seconds(a)?;
    let total = measurements::db_load_rate(
        a,
        if domain == "CPU" {
            DbLoadMetric::DbCpu
        } else {
            DbLoadMetric::DbTime
        },
    )?
    .per_second
        * seconds;
    Some((
        total,
        stat(a, "execute count")
            .filter(|v| *v > 0.0)
            .unwrap_or(seconds),
    ))
}

fn work_metric(scope: &str) -> (&'static str, &'static str) {
    if scope == CONTINUATION_SCOPE {
        ("continued_per_rowid", "continued encounters / ROWID fetch")
    } else if scope == "instance" {
        ("blocks_per_scan", "blocks/scan")
    } else {
        ("sql_gets", "gets/execution")
    }
}

fn work_delta(scope: &str, p: &Policy) -> f64 {
    if scope == CONTINUATION_SCOPE {
        p.minimum_continuation_delta_per_rowid
    } else {
        1.0
    }
}

fn known_plan(a: &AWR, id: &str) -> Option<u64> {
    if !measurements::available(
        a,
        "top_sql_with_top_events",
        !a.top_sql_with_top_events.is_empty(),
    ) {
        return None;
    }
    a.top_sql_with_top_events
        .get(id)
        .map(|s| s.plan_hash_value)
        .filter(|h| *h != 0)
}
fn blocking_wait(a: &AWR) -> bool {
    if !measurements::available(
        a,
        "foreground_wait_events",
        !a.foreground_wait_events.is_empty(),
    ) {
        return false;
    }
    let total = measurements::db_load_rate(a, DbLoadMetric::DbTime)
        .map(|v| v.per_second * measurements::seconds(a).unwrap_or(0.0))
        .unwrap_or(0.0);
    let waits: f64 = a
        .foreground_wait_events
        .iter()
        .filter(|w| {
            w.event.contains("library cache")
                || w.event.contains("cursor: pin")
                || w.event.starts_with("enq: TX")
        })
        .map(|w| w.total_wait_time_s)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .sum();
    total > 0.0 && waits / total > 0.5
}

/// Calendar grouping uses only time, day type and exposure class, not the cost
/// being tested. Fitting a cohort to low/high gets or CPU would hide degradation.
fn profile(a: &AWR, seasonal: bool) -> String {
    let t = measurements::timestamp(&a.snap_info.begin_snap_time).unwrap();
    let seconds = measurements::seconds(a).unwrap();
    let exposure = if seconds < 300.0 {
        "short"
    } else if seconds < 1800.0 {
        "subhour"
    } else if seconds < 7200.0 {
        "hourly"
    } else {
        "long"
    };
    if seasonal {
        format!(
            "{}:{:02}:{}",
            if t.weekday().number_from_monday() <= 5 {
                "weekday"
            } else {
                "weekend"
            },
            t.hour(),
            exposure
        )
    } else {
        format!("local:{exposure}")
    }
}

fn scan_context(b: &[Sample<'_>], r: &Sample<'_>, p: &Policy) -> Vec<String> {
    let mut reasons = Vec::new();
    // Path counts need not form disjoint categories: compare each descriptor
    // separately, never subtract direct/PX/IM counts from short+long scans.
    for name in [
        LONG,
        PATH_COUNTERS[0],
        PATH_COUNTERS[1],
        PATH_COUNTERS[2],
        PATH_COUNTERS[3],
    ] {
        let get = |a: &AWR| {
            stat(a, name)
                .zip(scans(a))
                .filter(|(_, s)| *s > 0.0)
                .map(|(v, s)| v / s)
        };
        let bv: Option<Vec<_>> = b.iter().map(|s| get(s.a)).collect();
        match (bv, get(r.a)) {
            (Some(v),Some(rv)) if (rv-median(&v)).abs()>p.maximum_scan_mix_change => reasons.push(format!("Changed scan-path descriptor: {name}; aggregate blocks/scan is not comparable.")),
            (None,_) | (_,None) => reasons.push(format!("Unknown scan-path coverage: {name}.")), _=>{}
        }
    }
    // Scans per execution describes the operation mix, not its throughput. This
    // catches a burst of many tiny scans without reinstating a +/-20% rate veto.
    let per_exec = |a: &AWR| {
        scans(a)
            .zip(stat(a, "execute count"))
            .filter(|(_, e)| *e > 0.0)
            .map(|(s, e)| s / e)
    };
    if let (Some(bv), Some(rv)) = (
        b.iter().map(|s| per_exec(s.a)).collect::<Option<Vec<_>>>(),
        per_exec(r.a),
    ) {
        let base = median(&bv);
        if base > 0.0
            && (rv / base > p.maximum_scans_per_execution_factor
                || rv / base < 1.0 / p.maximum_scans_per_execution_factor)
        {
            reasons.push("Changed scans per execution: operation mix is not comparable for aggregate blocks/scan.".into());
        }
    }
    reasons.extend(execution_mix_context(b, r, p));
    reasons
}

fn execution_mix_context(b: &[Sample<'_>], r: &Sample<'_>, p: &Policy) -> Vec<String> {
    let mut reasons = Vec::new();
    // No inferred zeros for missing SQLs: compare composition only over common
    // explicitly observed executions; incomplete overlap is a limitation.
    let ids: BTreeSet<_> =
        r.a.sql_gets
            .keys()
            .filter(|id| b.iter().all(|s| sql_value(s.a, id, "sql_gets").is_some()))
            .cloned()
            .collect();
    if ids.len() >= 2 {
        let bv: Vec<f64> = ids
            .iter()
            .map(|id| {
                median(
                    &b.iter()
                        .map(|s| sql_value(s.a, id, "sql_gets").unwrap().1)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let rv: Vec<f64> = ids
            .iter()
            .map(|id| sql_value(r.a, id, "sql_gets").map_or(0.0, |x| x.1))
            .collect();
        let bs: f64 = bv.iter().sum();
        let rs: f64 = rv.iter().sum();
        if bs > 0.0
            && rs > 0.0
            && bv
                .iter()
                .zip(&rv)
                .map(|(b, r)| (b / bs - r / rs).abs())
                .sum::<f64>()
                / 2.0
                > p.maximum_scan_mix_change
        {
            reasons.push(
                "Changed observed SQL execution mix; aggregate work ratio is not comparable."
                    .into(),
            );
        }
    } else {
        reasons.push("Insufficient common TOP SQL to verify the instance workload mix.".into());
    }
    reasons
}

fn work_context(b: &[Sample<'_>], r: &Sample<'_>, scope: &str, p: &Policy) -> Vec<String> {
    if b.is_empty() {
        return Vec::new();
    }
    if scope == "instance" {
        return scan_context(b, r, p);
    }
    if scope != CONTINUATION_SCOPE {
        return Vec::new();
    }
    let mut limits = execution_mix_context(b, r, p);
    // The denominator describes a workload mix, not a unique-row population.
    // A changing ROWID/execution mix invalidates the ratio comparison locally.
    let ratio = |a: &AWR| {
        stat(a, ROWID)
            .zip(stat(a, "execute count"))
            .filter(|(_, e)| *e > 0.0)
            .map(|(v, e)| v / e)
    };
    match (
        b.iter().map(|s| ratio(s.a)).collect::<Option<Vec<_>>>(),
        ratio(r.a),
    ) {
        (Some(values), Some(recent)) => {
            let base = median(&values);
            if base > 0.0
                && (recent / base > p.maximum_scans_per_execution_factor
                    || recent / base < 1.0 / p.maximum_scans_per_execution_factor)
            {
                limits.push("Changed ROWID fetches per execution: continuation workload mix is not comparable.".into());
            }
        }
        _ => limits.push(
            "ROWID/execution mix is unavailable; continuation ratio is contextual only.".into(),
        ),
    }
    limits.push("Continued/ROWID is an instance workload ratio, not a percentage of chained rows or SQL/segment attribution.".into());
    limits
}

fn growth_threshold(values: &[f64], floor: f64, p: &Policy) -> Option<(f64, f64)> {
    if values.is_empty() {
        return None;
    }
    let center = median(values);
    let logs: Vec<_> = values
        .iter()
        .filter(|v| **v > 0.0)
        .map(|v| v.ln())
        .collect();
    let noise = if logs.len() == values.len() {
        let m = median(&logs);
        1.4826 * median(&logs.iter().map(|v| (v - m).abs()).collect::<Vec<_>>())
    } else {
        0.0
    };
    let threshold = (floor / 100.0)
        .ln_1p()
        .max(p.noise_multiplier * noise)
        .min(100.0);
    Some((center, (threshold.exp() - 1.0) * 100.0))
}

/// Median/MAD defines a noise floor, not a confidence interval. Require an
/// absolute difference as well; a zero historical cost cannot produce infinity.
fn growth(values: &[f64], recent: f64, floor: f64, delta: f64, p: &Policy) -> Option<(f64, f64)> {
    if !recent.is_finite() {
        return None;
    }
    let (center, pct) = growth_threshold(values, floor, p)?;
    if recent - center + 1e-12 < delta
        || (center > 0.0 && recent + 1e-12 < center * (1.0 + pct / 100.0))
    {
        return None;
    }
    Some((center, pct))
}

/// Time histories contain every observed time-domain sample in this profile,
/// including windows omitted from TOP gets. Freeze their upper boundary with the
/// work reference, so later TOP entry cannot teach the degraded period as normal.
fn evaluate_costs(
    history: &[&AWR],
    b: &[Sample<'_>],
    r: &Sample<'_>,
    scope: &str,
    p: &Policy,
) -> Vec<CostEvaluation> {
    let end = p
        .baseline_snap_range
        .map(|(_, end)| end)
        .unwrap_or(b.last().unwrap().a.snap_info.end_snap_id);
    ["CPU", "elapsed"].into_iter().map(|domain| {
        let mut observed: Vec<_> = history.iter()
            .filter(|a| a.snap_info.end_snap_id<=end)
            .filter_map(|a| time_value(a,scope,domain).map(|v|(*a,v.0/v.1)))
            .collect();
        if p.baseline_snap_range.is_none() && observed.len()>p.baseline_windows {
            observed.drain(..observed.len()-p.baseline_windows);
        }
        let rows: Vec<_> = observed.iter().map(|v|v.0).collect();
        let values: Vec<_> = observed.iter().map(|v|v.1).collect();
        let recent=time_value(r.a,scope,domain).map(|v|v.0/v.1);
        let unit=if scope.starts_with("instance") && stat(r.a,"execute count").is_none_or(|v|v<=0.0) {"s/s"} else {"s/execution"};
        let threshold = growth_threshold(&values,p.minimum_cost_growth_pct,p).map(|v|v.1);
        let comparison = recent.zip((!values.is_empty()).then(||median(&values))).map(|(rv,bv)| Comparison::new(domain,unit,bv,rv,values.len(),1,
            if scope.starts_with("instance") {"Instance DB CPU/DB Time / global execution count (or wall seconds); not scan or continuation CPU."} else {"Observed SQL time domain / its own executions; independent TOP coverage."}));
        let is_growth=recent.is_some_and(|v| growth(&values,v,p.minimum_cost_growth_pct,p.minimum_sql_cost_delta_s_per_execution,p).is_some());
        let status=if recent.is_none() {"recent_unavailable"} else if values.is_empty() {"baseline_unavailable"}
            else if is_growth && domain=="elapsed" && blocking_wait(r.a) {"growth_with_wait_confounder"}
            else if is_growth {"growth"} else {"no_growth"};
        let baseline=(!rows.is_empty()).then(||period(&rows));
        CostEvaluation { domain:domain.into(),status:status.into(),comparison,baseline_observations:rows.len(),recent_observations:usize::from(recent.is_some()),
            baseline_sufficient:rows.len()>=p.minimum_baseline_windows && baseline.as_ref().is_some_and(|v|v.seconds>=p.minimum_period_seconds),baseline,threshold_pct:threshold }
    }).collect()
}

fn assess(
    b: &[Sample<'_>],
    r: &Sample<'_>,
    time_history: &[&AWR],
    scope: &str,
    key: &str,
    selection: &str,
    p: &Policy,
) -> Option<Assessment> {
    let work_values: Vec<_> = b.iter().map(|s| s.work).collect();
    let (bw, wt) = growth(
        &work_values,
        r.work,
        p.minimum_growth_pct,
        work_delta(scope, p),
        p,
    )?;
    let mut limits = work_context(b, r, scope, p);
    if limits.iter().any(|s| s.contains("not comparable")) {
        return None;
    }
    let cost_evaluations = evaluate_costs(time_history, b, r, scope, p);
    let chosen = cost_evaluations.iter().find(|c| {
        c.status == "growth"
            || (!scope.starts_with("instance") && c.status == "growth_with_wait_confounder")
    });
    let cost = chosen.and_then(|c| c.comparison.clone());
    let time_status = if let Some(c) = chosen {
        if c.status == "growth" {
            "observed_time_growth"
        } else {
            "time_growth_with_wait_confounder"
        }
    } else if cost_evaluations.iter().all(|c| c.comparison.is_none()) {
        "time_comparison_unavailable"
    } else {
        "no_unconfounded_time_growth"
    };
    if cost_evaluations
        .iter()
        .any(|c| c.status == "growth_with_wait_confounder")
    {
        limits.push("Wait confounder: instance library/transaction waits accompany elapsed growth; this does not establish a physical access cause.".into());
    }
    if chosen.is_none() {
        limits.push(
            "Work inflation is observed; its time impact is not established by these comparisons."
                .into(),
        );
    }
    if chosen.is_some_and(|c| !c.baseline_sufficient) {
        limits.push("Incomplete cost-domain history: time support has fewer than the required baseline observations or exposure.".into());
    }
    if let Some(id) = scope.strip_prefix("sql:") {
        let plans: BTreeSet<_> = b
            .iter()
            .chain(std::iter::once(r))
            .filter_map(|s| known_plan(s.a, id))
            .collect();
        if plans.len() > 1 {
            limits.push("Observed plan change/multiple plans: SQL_ID totals cannot be assigned to one plan; data-access change remains a competing explanation.".into());
        }
        if b.iter()
            .chain(std::iter::once(r))
            .any(|s| known_plan(s.a, id).is_none())
        {
            limits.push("Plan history is incomplete; matching SQL_ID does not establish matching plan, binds or useful work.".into());
        }
    }
    let base_rows: Vec<_> = b.iter().map(|s| s.a).collect();
    let enough = b.len() >= p.minimum_baseline_windows
        && period(&base_rows).seconds >= p.minimum_period_seconds;
    if !enough {
        limits.push("Short baseline for this comparison: provisional work evidence; recurrence needs more comparable history.".into());
    }
    let extra = cost
        .as_ref()
        .and_then(|c| {
            time_value(r.a, scope, &c.metric).map(|v| {
                if c.recent > 0.0 {
                    (v.0 * (1.0 - c.baseline / c.recent)).max(0.0)
                } else {
                    0.0
                }
            })
        })
        .unwrap_or(0.0);
    let (metric, unit) = work_metric(scope);
    Some(Assessment {
        profile:key.into(),baseline_selection:selection.into(),baseline:period(&base_rows),recent:period(&[r.a]),
        work:Comparison::new(metric,unit,bw,r.work,b.len(),1,"Median of observed work-domain ratios; missing time does not remove work observations."),
        cost,work_threshold_pct:wt,cost_threshold_pct:chosen.and_then(|c|c.threshold_pct),
        cost_baseline_intervals:chosen.and_then(|c|c.baseline.as_ref()).map(|b|b.intervals.clone()).unwrap_or_default(),
        recurring:false,baseline_sufficient:enough,extra_observed_cost_seconds:extra,
        limitations:limits,time_impact_status:time_status.into(),cost_evaluations,
    })
}

fn bump(reasons: &mut BTreeMap<String, usize>, reason: &str) {
    *reasons.entry(reason.into()).or_default() += 1;
}

/// Evaluate a scope over independent calendar cohorts. The short horizon adapts
/// to workload, while a fixed reference retains gradual deterioration. Once an
/// adequately supported change appears, freeze the baseline to avoid learning
/// that change as normal. Missing TOP opportunities are counted, never imputed.
fn evaluate<'a>(
    rows: &[&'a AWR],
    scope: &str,
    seasonal: bool,
    p: &Policy,
    eval: &mut RuleEvaluation,
) -> Evaluation {
    let mut groups: BTreeMap<String, Vec<&AWR>> = BTreeMap::new();
    for a in rows {
        let unit =
            if scope.starts_with("instance") && stat(a, "execute count").is_none_or(|n| n <= 0.0) {
                ":rate"
            } else {
                ""
            };
        groups
            .entry(format!("{}{unit}", profile(a, seasonal)))
            .or_default()
            .push(a);
    }
    let mut found = Evaluation::default();
    for (key, opportunities) in groups {
        let mut se = ScopeEvaluation {
            scope: scope.into(),
            profile: key.clone(),
            ..Default::default()
        };
        let mut history: Vec<Sample<'a>> = Vec::new();
        let mut time_history: Vec<&AWR> = Vec::new();
        let mut trajectory = Vec::new();
        let mut frozen: Option<Vec<Sample<'a>>> = None;
        let mut hits: Vec<bool> = Vec::new();
        let mut candidates: Vec<Assessment> = Vec::new();
        for a in opportunities {
            se.cpu_observations += usize::from(time_value(a, scope, "CPU").is_some());
            se.elapsed_observations += usize::from(time_value(a, scope, "elapsed").is_some());
            // Collect time even when work is missing, but never use current/future
            // time to evaluate a preceding reference (evaluate_costs enforces cutoff).
            if p.baseline_snap_range.is_none_or(|(begin, end)| {
                a.snap_info.begin_snap_id >= begin && a.snap_info.end_snap_id <= end
            }) {
                time_history.push(a);
            }
            let mut point = TrajectoryPoint {
                period: period(&[a]),
                profile: key.clone(),
                work: None,
                baseline: None,
                threshold: None,
                status: "work_unavailable".into(),
            };
            let sample = scope_sample(a, scope, p);
            let Some(r) = sample else {
                se.missing_observations += 1;
                trajectory.push(point);
                continue;
            };
            point.work = Some(r.work);
            point.status = "reference_observation".into();
            se.observed_opportunities += 1;
            if let Some((begin, end)) = p.baseline_snap_range {
                if a.snap_info.begin_snap_id >= begin && a.snap_info.end_snap_id <= end {
                    history.push(r);
                    trajectory.push(point);
                    continue;
                }
                if a.snap_info.begin_snap_id < end {
                    trajectory.push(point);
                    continue;
                }
            }
            if history.is_empty() {
                if p.baseline_snap_range.is_some() {
                    bump(&mut se.reasons, "explicit_baseline_missing_for_profile");
                    point.status = "profile_reference_unavailable".into();
                    trajectory.push(point);
                    continue;
                }
                history.push(r);
                bump(&mut se.reasons, "no_prior_comparable_observation");
                trajectory.push(point);
                continue;
            }
            let short: Vec<_> = history
                .iter()
                .rev()
                .take(p.baseline_windows)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let anchor: Vec<_> = history
                .iter()
                .take(p.minimum_baseline_windows)
                .cloned()
                .collect();
            let choices = if let Some(b) = frozen.as_ref() {
                vec![(b.as_slice(), "frozen_comparable_reference")]
            } else if p.baseline_snap_range.is_some() {
                vec![(history.as_slice(), "explicit_comparable_reference")]
            } else {
                vec![
                    (short.as_slice(), "recent_comparable_history"),
                    (anchor.as_slice(), "fixed_comparable_reference"),
                ]
            };
            let mut hit = None;
            let reference = choices[0].0;
            let reference_values: Vec<_> = reference.iter().map(|s| s.work).collect();
            if let Some((base, pct)) = growth_threshold(&reference_values, p.minimum_growth_pct, p)
            {
                point.baseline = Some(base);
                point.threshold =
                    Some((base * (1.0 + pct / 100.0)).max(base + work_delta(scope, p)));
                point.status = if work_context(reference, &r, scope, p)
                    .iter()
                    .any(|s| s.contains("not comparable"))
                {
                    "not_comparable"
                } else {
                    "within_reference"
                }
                .into();
            }
            for (baseline, selection) in choices {
                if let Some(v) = assess(baseline, &r, &time_history, scope, &key, selection, p) {
                    hit = Some((v, baseline.to_vec()));
                    break;
                }
            }
            if scope.starts_with("instance") && hit.is_none() {
                for reason in work_context(&short, &r, scope, p)
                    .iter()
                    .filter(|s| s.contains("not comparable"))
                {
                    bump(&mut se.reasons, reason);
                }
            }
            se.assessed_comparisons += 1;
            hits.push(hit.is_some());
            if let Some((mut v, baseline)) = hit {
                point.status = "work_growth".into();
                point.baseline = Some(v.work.baseline);
                point.threshold = Some(
                    (v.work.baseline * (1.0 + v.work_threshold_pct / 100.0))
                        .max(v.work.baseline + work_delta(scope, p)),
                );
                let tail = &hits[hits.len().saturating_sub(p.consecutive_windows)..];
                v.recurring = tail.iter().filter(|v| **v).count() >= p.minimum_recurrences;
                if v.recurring {
                    // Promote only the candidates in the qualifying opportunity
                    // window; separated episodes must not confer recurrence.
                    for old in candidates
                        .iter_mut()
                        .rev()
                        .take(tail.iter().filter(|v| **v).count() - 1)
                    {
                        old.recurring = true;
                    }
                }
                if v.baseline_sufficient && frozen.is_none() {
                    frozen = Some(baseline);
                }
                candidates.push(v);
            }
            if frozen.is_none() && p.baseline_snap_range.is_none() {
                history.push(r);
            }
            trajectory.push(point);
        }
        se.status = if !candidates.is_empty() {
            "candidate_observed"
        } else if se.assessed_comparisons > 0 {
            "assessed_no_growth_signal"
        } else {
            "insufficient_history"
        }
        .into();
        if se.missing_observations > 0 {
            bump(&mut se.reasons, "missing_or_invalid_work_observation");
            let coverage = se.observed_opportunities as f64
                / (se.observed_opportunities + se.missing_observations) as f64;
            if coverage < p.minimum_top_coverage {
                for candidate in &mut candidates {
                    candidate.limitations.push(format!("Censored coverage in this profile: {}/{} observed opportunities; completeness is not established.",se.observed_opportunities,se.observed_opportunities+se.missing_observations));
                }
            }
        }
        eval.assessed_comparisons += se.assessed_comparisons;
        found.assessments.extend(candidates);
        found.trajectory.extend(trajectory);
        // Keep scoped coverage for observed workloads, not every absent SQL/hour.
        if se.observed_opportunities > 0
            || se.cpu_observations > 0
            || se.elapsed_observations > 0
            || scope.starts_with("instance")
        {
            eval.scope_evaluations.push(se);
        }
    }
    found.assessments.sort_by_key(|a| a.recent.begin_snap_id);
    found.trajectory.sort_by_key(|a| a.period.begin_snap_id);
    found
}

/// Screen material temporal associations above observed activity growth. The
/// instance activity factor is only an exclusion screen, not segment executions
/// and not a claim that a segment belongs to an observed SQL.
fn segment_assessments(b: &[&AWR], r: &[&AWR], p: &Policy) -> Vec<SegmentContext> {
    let identities: BTreeMap<_, _> = b
        .iter()
        .chain(r)
        .filter(|a| measurements::available(a, "segment_stats", !a.segment_stats.is_empty()))
        .flat_map(|a| a.segment_stats.get("Logical Reads").into_iter().flatten())
        .map(|s| (segment_key(s), s))
        .collect();
    let rate = |rows: &[&AWR], scans_domain: bool| -> Option<f64> {
        let values: Option<Vec<_>> = rows
            .iter()
            .map(|a| {
                if scans_domain {
                    scans(a)
                } else {
                    stat(a, "execute count")
                }
            })
            .collect();
        let seconds: f64 = rows.iter().filter_map(|a| measurements::seconds(a)).sum();
        values
            .filter(|_| seconds > 0.0)
            .map(|v| v.iter().sum::<f64>() / seconds)
    };
    identities.into_iter().filter_map(|(key,segment)| {
        let bv=observed_segment_reads(b,&key)?;
        let rv=observed_segment_reads(r,&key)?;
        let br=rows_for(b,&bv.period);let rr=rows_for(r,&rv.period);
        let factor=[true,false].into_iter().filter_map(|scans| {
            rate(&br,scans).zip(rate(&rr,scans)).filter(|(v,_)|*v>0.0).map(|(v,r)|r/v)
        }).reduce(f64::max);
        let material=rv.value-bv.value>=p.minimum_segment_excess_reads_per_second &&
            (bv.value==0.0 || rv.value>=bv.value*(1.0+p.minimum_growth_pct/100.0));
        let status=if !material {"no_material_read_growth"}
            else if factor.is_none() {"activity_reference_unavailable"}
            else if rv.value-bv.value*factor.unwrap().max(1.0)<p.minimum_segment_excess_reads_per_second {"activity_growth_can_explain_reads"}
            else {"material_read_growth_above_activity"};
        Some(SegmentContext {segment:segment.clone(),logical_reads:Comparison::new("Logical Reads","logical reads/s",bv.value,rv.value,bv.period.windows,rv.period.windows,
            "Segment totals / its own observed seconds. Activity adjustment is an instance-level screening proxy, not segment unit cost."),activity_growth_factor:factor,status:status.into()})
    }).collect()
}
fn segment_candidates(b: &[&AWR], r: &[&AWR], p: &Policy) -> Vec<SegmentCandidate> {
    let mut result:Vec<_>=segment_assessments(b,r,p).into_iter()
        .filter(|v|v.status=="material_read_growth_above_activity")
        .map(|v| SegmentCandidate {segment:v.segment,logical_reads:v.logical_reads,
            identity_status:"observed_segment_identity_no_sql_attribution".into(),
            attribution:"Material temporal read growth above instance activity growth; not a measured segment cost per execution or physical fragmentation proof.".into()}).collect();
    result.sort_by(|a, b| {
        b.logical_reads
            .delta
            .total_cmp(&a.logical_reads.delta)
            .then_with(|| segment_key(&a.segment).cmp(&segment_key(&b.segment)))
    });
    result
}

fn sql_controls(b: &[&AWR], r: &[&AWR], p: &Policy) -> Vec<SqlEvidence> {
    let ids: BTreeSet<_> = r.iter().flat_map(|a| a.sql_gets.keys().cloned()).collect();
    let mut result = Vec::new();
    for id in ids {
        let ratios = |rows: &[&AWR], domain: &str| {
            rows.iter()
                .filter_map(|a| sql_value(a, &id, domain).map(|v| v.0 / v.1))
                .collect::<Vec<_>>()
        };
        let bv = ratios(b, "sql_gets");
        let rv = ratios(r, "sql_gets");
        if bv.len() < p.minimum_baseline_windows || rv.is_empty() {
            continue;
        }
        if growth(&bv, median(&rv), p.minimum_growth_pct, 1.0, p).is_some() {
            continue;
        }
        let mut costs=vec![Comparison::new("sql_gets","gets/execution",median(&bv),median(&rv),bv.len(),rv.len(),"Contemporaneous SQL context without material unit-work growth; no mapping to a segment is inferred.")];
        let bc = ratios(b, "sql_cpu_time");
        let rc = ratios(r, "sql_cpu_time");
        if !bc.is_empty() && !rc.is_empty() {
            costs.push(Comparison::new(
                "CPU",
                "s/execution",
                median(&bc),
                median(&rc),
                bc.len(),
                rc.len(),
                "SQL CPU / its own executions; context only.",
            ));
        }
        result.push(SqlEvidence { sql_id: id, costs });
    }
    result.sort_by(|a, b| {
        b.costs[0]
            .baseline
            .total_cmp(&a.costs[0].baseline)
            .then_with(|| a.sql_id.cmp(&b.sql_id))
    });
    result.truncate(3);
    result
}

fn observed_segment_reads(rows: &[&AWR], key: &str) -> Option<Observation> {
    let mut observed = Vec::new();
    let mut total = 0.0;
    let mut exposure = 0.0;
    for a in rows {
        if !measurements::available(a, "segment_stats", !a.segment_stats.is_empty()) {
            continue;
        }
        let matching: Vec<_> = a
            .segment_stats
            .get("Logical Reads")
            .into_iter()
            .flatten()
            .filter(|s| segment_key(s) == key)
            .collect();
        // Ambiguous duplicate identity is unknown, not twice the measured cost.
        if matching.len() > 1 {
            return None;
        }
        if let Some(s) = matching.first() {
            if s.stat_vlalue.is_finite() && s.stat_vlalue >= 0.0 {
                let seconds = measurements::seconds(a)?;
                if seconds <= 0.0 {
                    return None;
                }
                total += s.stat_vlalue;
                exposure += seconds;
                observed.push(*a);
            }
        }
    }
    (!observed.is_empty()).then(|| Observation {
        metric: "Logical Reads".into(), unit: "logical reads/s".into(), value: total / exposure,
        total, exposure, exposure_unit: "seconds".into(), period: period(&observed),
        source: "Observed segment totals / observed seconds; TOP absence is unknown. No baseline comparison.".into(),
    })
}

fn segment_observations(rows: &[&AWR], names: &BTreeSet<&str>) -> Vec<SegmentObservation> {
    let identities: BTreeMap<_, _> = rows
        .iter()
        .filter(|a| measurements::available(a, "segment_stats", !a.segment_stats.is_empty()))
        .flat_map(|a| a.segment_stats.get("Logical Reads").into_iter().flatten())
        .filter(|s| names.contains(s.object_name.as_str()))
        .map(|s| (segment_key(s), s))
        .collect();
    let mut result: Vec<_> = identities.into_iter().filter_map(|(key, s)| {
        observed_segment_reads(rows, &key).map(|logical_reads| SegmentObservation {
            segment: s.clone(), logical_reads,
            identity_status: "plan_name_match_owner_container_not_proven".into(),
            attribution: "Name matches a supplied plan object; owner/container and physical epoch require verification. Segment reads are not attributed to this SQL or to fragmentation.".into(),
        })
    }).collect();
    result.sort_by(|a, b| {
        b.logical_reads
            .value
            .total_cmp(&a.logical_reads.value)
            .then_with(|| segment_key(&a.segment).cmp(&segment_key(&b.segment)))
    });
    result
}

fn rows_for<'a>(rows: &[&'a AWR], p: &Period) -> Vec<&'a AWR> {
    rows.iter()
        .filter(|a| {
            p.intervals
                .contains(&(a.snap_info.begin_snap_id, a.snap_info.end_snap_id))
        })
        .copied()
        .collect()
}
fn make_hint(
    c: &AWRSCollection,
    rows: &[&AWR],
    scope: &str,
    evaluation: Evaluation,
    p: &Policy,
) -> PerformanceHint {
    let assessments = evaluation.assessments;
    let representative = assessments
        .iter()
        .max_by(|a, b| {
            (a.recurring && a.baseline_sufficient)
                .cmp(&(b.recurring && b.baseline_sufficient))
                .then_with(|| a.cost.is_some().cmp(&b.cost.is_some()))
                .then_with(|| {
                    a.extra_observed_cost_seconds
                        .total_cmp(&b.extra_observed_cost_seconds)
                })
                .then_with(|| a.work.recent.total_cmp(&b.work.recent))
        })
        .unwrap();
    let b = rows_for(rows, &representative.baseline);
    let r = rows_for(rows, &representative.recent);
    let episode_rows: Vec<_> = rows
        .iter()
        .filter(|a| {
            assessments
                .iter()
                .any(|v| v.recent.begin_snap_id == a.snap_info.begin_snap_id)
        })
        .copied()
        .collect();
    let mut recurrence: BTreeMap<&str, usize> = BTreeMap::new();
    for a in &assessments {
        if a.recurring && a.baseline_sufficient {
            *recurrence.entry(&a.profile).or_default() += 1;
        }
    }
    let recurring = recurrence.values().any(|n| *n >= p.minimum_recurrences);
    // Card-level limitations describe the displayed comparison. Earlier short
    // references remain attached to their own Assessment, never to a later one.
    let mut limits=vec![CAUTION.into(),"TOP omissions are unknown. Same SQL/plan does not guarantee equivalent binds, row width, active branches or useful rows.".into(),"Per-window startup/container changes are not supplied; comparisons assume unchanged collected scope.".into()];
    limits.extend(representative.limitations.clone());
    let sql = scope.strip_prefix("sql:");
    let mut evidence = vec![representative.work.clone()];
    evidence.extend(representative.cost.clone());
    let mut context = Vec::new();
    let mut counterevidence: Vec<_> = representative
        .limitations
        .iter()
        .filter(|s| {
            s.contains("plan change")
                || s.contains("Short baseline")
                || s.contains("mix")
                || s.contains("Wait confounder")
        })
        .cloned()
        .collect();
    // Global counters are context only. Even a positive continuation counter
    // cannot assign a mechanism to a SQL or the first segment in a ranked list.
    if scope != CONTINUATION_SCOPE {
        let rate = |rr: &[&AWR]| -> Option<f64> {
            let v: Option<Vec<_>> = rr
                .iter()
                .map(|a| stat(a, CONTINUED).zip(measurements::seconds(a)))
                .collect();
            v.map(|v| v.iter().map(|v| v.0).sum::<f64>() / v.iter().map(|v| v.1).sum::<f64>())
        };
        let comparison = rate(&b).zip(rate(&r)).map(|(bv, rv)| {
            Comparison::new(
                CONTINUED,
                "encounters/s",
                bv,
                rv,
                b.len(),
                r.len(),
                "Instance-wide counter / wall seconds; not SQL or segment attribution.",
            )
        });
        let status = match &comparison {
            Some(c) if c.recent > c.baseline => "increased",
            Some(_) => "no_increase",
            None => "unavailable",
        };
        if status == "no_increase" {
            counterevidence.push("Global continuation did not increase in the displayed comparison. It supplies no positive continuation evidence here; it does not establish absence in a particular table.".into());
        }
        context.push(ContextEvidence {
            scope: "instance".into(),
            label: if sql.is_some() {
                "Instance context — not SQL attribution"
            } else {
                "Instance continuation context — separate from scan evidence"
            }
            .into(),
            status: status.into(),
            comparison,
        });
    }
    let mut segments = if scope == "instance" {
        segment_candidates(&b, &r, p)
    } else {
        Vec::new()
    };
    let total = segments.len();
    segments.truncate(5);
    let mut segment_context = if scope == "instance" {
        segment_assessments(&b, &r, p)
            .into_iter()
            .filter(|v| v.status != "material_read_growth_above_activity")
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    segment_context.sort_by(|a, b| b.logical_reads.recent.total_cmp(&a.logical_reads.recent));
    segment_context.truncate(5);
    let time_supported = representative
        .cost_evaluations
        .iter()
        .any(|c| c.status == "growth" && c.baseline_sufficient);
    let confidence = if recurring
        && time_supported
        && !limits.iter().any(|s| {
            s.contains("plan change")
                || s.contains("not comparable")
                || s.contains("Censored coverage")
                || s.contains("Wait confounder")
                || s.contains("Incomplete cost-domain")
        }) {
        "moderate"
    } else {
        "low"
    };
    let latest = evaluation.trajectory.last().unwrap();
    let episode_status = match latest.status.as_str() {
        "work_growth" if latest.work.is_some_and(|v| v < representative.work.recent) => {
            "growth_persists_below_representative_value"
        }
        "work_growth" => "growth_in_latest_supplied_window",
        "within_reference" => "historical_growth_later_within_reference",
        "not_comparable" => "historical_growth_latest_mix_not_comparable",
        "work_unavailable" => "historical_growth_latest_work_unavailable",
        _ => "historical_growth_latest_profile_without_reference",
    };
    let signal = if scope == "instance" {
        "scan_work_inflation"
    } else if scope == CONTINUATION_SCOPE {
        "row_continuation_work_inflation"
    } else {
        "sql_logical_read_work_inflation"
    };
    let title = if scope == "instance" {
        "Increased blocks per table scan"
    } else if scope == CONTINUATION_SCOPE {
        "Increased row-continuation work"
    } else {
        "Increased logical reads per SQL execution"
    };
    let cost = representative.cost.clone();
    let time_status = representative.time_impact_status.clone();
    PerformanceHint {
        hint_id:format!("{RULE_ID}:{}:{}:{scope}",c.db_instance_information.db_id,c.db_instance_information.instance_num),rule_id:RULE_ID.into(),rule_version:VERSION.into(),title:title.into(),dbid:c.db_instance_information.db_id,instance_id:c.db_instance_information.instance_num,
        episode_status:episode_status.into(),episode:period(&episode_rows),baseline:Some(representative.baseline.clone()),comparison:Some(representative.recent.clone()),evidence,
        sql_evidence:sql.map(|id|vec![SqlEvidence {sql_id:id.into(),costs:std::iter::once(representative.work.clone()).chain(cost).collect()}]).unwrap_or_default(),sql_candidates_total:usize::from(sql.is_some()),
        possibly_affected_segments:segments,segment_candidates_total:total,segment_message:if total==0{NO_SEGMENTS}else{"Material temporal candidates; SQL/object attribution and physical cause remain unconfirmed."}.into(),
        next_check:"JAS-only conclusion: access work increased; the physical cause remains unresolved. Compare supplied plans, workload and time coverage. Space/row checks are optional external confirmation, not a requirement to complete this analysis.".into(),
        assessment_status:match (recurring,representative.cost.is_some()) {(true,true)=>"recurrent_hypothesis",(false,true)=>"provisional_hypothesis",(true,false)=>"recurrent_work_inflation",(false,false)=>"provisional_work_inflation"}.into(),
        confidence:confidence.into(),assessment_scope:scope.into(),
        // Physical mechanisms remain alternatives until object-level evidence is
        // supplied. The measured signal is described independently by signal_kind.
        mechanisms:Vec::new(),signal_kind:signal.into(),time_impact_status:time_status,
        supporting_evidence:vec![format!("{} work-inflation comparisons across {} profiles. Repeated windows describe persistence, not independent experiments or calibrated significance.",assessments.len(),assessments.iter().map(|a|&a.profile).collect::<BTreeSet<_>>().len())],
        counterevidence,alternative_explanations:vec!["Sparse table or index blocks, or row migration — unconfirmed".into(),"Data growth: more rows or wider rows, including necessary row continuation due to wider data".into(),"Changed bind selectivity, useful work, plans, index probes, caching or CR/undo".into()],
        observed_cost_seconds:assessments.iter().map(|a|a.extra_observed_cost_seconds).sum(),comparisons:assessments,limitations:limits,plan_objects:Vec::new(),observed_metrics:Vec::new(),observed_segments:Vec::new(),
        context_evidence:context,segment_context,sql_controls:if scope=="instance"{sql_controls(&b,&r,p)}else{Vec::new()},trajectory:evaluation.trajectory,
    }
}

pub fn build(c: &AWRSCollection, range: &(u64, u64), policy: Policy) -> PerformanceHintsReport {
    let mut out=PerformanceHintsReport{version:VERSION.into(),policy:policy.clone(),policy_status:"independent_work_and_time_v3_accuracy_not_calibrated".into(),policy_notes:vec![
        "Legacy workload_band_pct, maximum_baseline_ratio_spread, minimum_extra_blocks_per_second and minimum_db_cost_delta_s_per_second remain readable for v1 policy compatibility; v2 does not use them as exclusion gates.".into(),
        "minimum_top_coverage is a confidence descriptor only. Recurrence uses observed comparable opportunities; missing TOP remains unknown.".into(),
        "Robust medians/MAD screen per-window work and time independently. Missing time does not remove work observations or delay freezing their reference. Period totals retain observed exposure. Thresholds are heuristic, not calibrated probabilities.".into()],..Default::default()};
    let mut selected = measurements::selected(c, range);
    selected.sort_by_key(|a| {
        (
            measurements::timestamp(&a.snap_info.begin_snap_time),
            a.snap_info.begin_snap_id,
        )
    });
    let mut eval = RuleEvaluation {
        rule_id: RULE_ID.into(),
        status: "not_assessable".into(),
        selected_windows: selected.len(),
        assessed_comparisons: 0,
        excluded_windows: 0,
        reasons: BTreeMap::new(),
        scope_evaluations: Vec::new(),
    };
    if let Err(e) = policy.validate() {
        bump(&mut eval.reasons, &e);
        out.rule_evaluations.push(eval);
        return out;
    }
    let mut bad = BTreeSet::new();
    let mut active = Vec::new();
    for (i, a) in selected.iter().enumerate() {
        if measurements::seconds(a).is_none()
            || a.snap_info.begin_snap_id >= a.snap_info.end_snap_id
        {
            bad.insert(i);
            continue;
        }
        let begin = measurements::timestamp(&a.snap_info.begin_snap_time).unwrap();
        let end = measurements::timestamp(&a.snap_info.end_snap_time).unwrap();
        active.retain(|(_, prior_end)| *prior_end > begin);
        for (j, _) in &active {
            bad.insert(i);
            bad.insert(*j);
        }
        active.push((i, end));
    }
    // Invalid/overlapping samples are local exclusions. They form epoch boundaries
    // so an apparently improving value across corrupt time data is never evidence.
    let mut groups: Vec<Vec<&AWR>> = vec![Vec::new()];
    for (i, a) in selected.iter().enumerate() {
        if bad.contains(&i) {
            eval.excluded_windows += 1;
            bump(&mut eval.reasons, "invalid_or_overlapping_interval");
            groups.push(Vec::new());
        } else {
            groups.last_mut().unwrap().push(a);
        }
    }
    let all: Vec<_> = groups.iter().flatten().copied().collect();
    if let Some((begin, end)) = policy.baseline_snap_range {
        if !all.iter().any(|a| a.snap_info.begin_snap_id == begin)
            || !all.iter().any(|a| a.snap_info.end_snap_id == end)
        {
            bump(&mut eval.reasons, "explicit_baseline_not_in_selected_range");
            out.rule_evaluations.push(eval);
            return out;
        }
    }
    for (epoch, rows) in groups.into_iter().filter(|v| !v.is_empty()).enumerate() {
        let elapsed = (measurements::timestamp(&rows.last().unwrap().snap_info.end_snap_time)
            .unwrap()
            - measurements::timestamp(&rows[0].snap_info.begin_snap_time).unwrap())
        .num_seconds() as f64;
        let seasonal = elapsed >= policy.seasonal_minimum_hours * 3600.0;
        let ids: BTreeSet<_> = rows
            .iter()
            .flat_map(|a| a.sql_gets.keys().cloned())
            .collect();
        for scope in ["instance".to_string(), CONTINUATION_SCOPE.to_string()]
            .into_iter()
            .chain(ids.iter().map(|id| format!("sql:{id}")))
        {
            let evaluation = evaluate(&rows, &scope, seasonal, &policy, &mut eval);
            if !evaluation.assessments.is_empty() {
                let mut h = make_hint(c, &rows, &scope, evaluation, &policy);
                h.hint_id.push_str(&format!(":epoch{epoch}"));
                out.hints.push(h);
            }
        }
        persistent_costs(c, &rows, &ids, &policy, &mut out.hints);
    }
    for h in &mut out.hints {
        if h.assessment_scope.starts_with("sql:") {
            h.possibly_affected_segments.clear();
            h.segment_candidates_total = 0;
            h.segment_message = NO_SEGMENTS.into();
        }
    }
    out.hints.sort_by(|a, b| {
        // Absolute footprint and baseline-relative excess have different meanings.
        // Rank hypotheses first, then rank observations within their own category.
        a.is_cost_observation()
            .cmp(&b.is_cost_observation())
            .then_with(|| b.observed_cost_seconds.total_cmp(&a.observed_cost_seconds))
            .then_with(|| a.hint_id.cmp(&b.hint_id))
    });
    // Missing dimensions are a partial assessment, not a claim of a healthy DB.
    eval.status = if out.hints.iter().any(|h| !h.is_cost_observation()) {
        "partial_assessment_with_candidates"
    } else if eval.assessed_comparisons > 0 {
        "assessed_no_growth_signal"
    } else {
        "not_assessable"
    }
    .into();
    out.rule_evaluations.push(eval);
    out
}

/// A material access cost can deserve attention even without a growth signal.
/// This does not establish constant cost, deterioration or fragmentation. Use
/// absolute observations only; never fill both sides of a Comparison with a mean.
/// Never add parent PL/SQL and child SQL costs into a savings total.
fn persistent_costs(
    c: &AWRSCollection,
    rows: &[&AWR],
    ids: &BTreeSet<String>,
    p: &Policy,
    hints: &mut Vec<PerformanceHint>,
) {
    let db_cpu: Option<Vec<f64>> = rows
        .iter()
        .map(|a| {
            measurements::db_load_rate(a, DbLoadMetric::DbCpu)
                .map(|v| v.per_second * measurements::seconds(a).unwrap())
        })
        .collect();
    let total_cpu = db_cpu.map(|v| v.iter().sum::<f64>());
    for id in ids {
        if hints
            .iter()
            .any(|h| h.assessment_scope == format!("sql:{id}"))
        {
            continue;
        }
        let samples: Vec<_> = rows
            .iter()
            .filter_map(|a| sql_sample(a, id))
            .filter(|s| s.cpu.is_some())
            .collect();
        if samples.len() < p.minimum_baseline_windows {
            continue;
        }
        let cpu: f64 = samples.iter().filter_map(|s| s.cpu_total).sum();
        let gets: f64 = samples
            .iter()
            .filter_map(|s| sql_value(s.a, id, "sql_gets").map(|v| v.0))
            .sum();
        if cpu < p.persistent_minimum_cpu_seconds || gets < p.persistent_minimum_gets {
            continue;
        }
        if total_cpu
            .is_some_and(|total| total > 0.0 && cpu / total < p.persistent_minimum_cpu_share)
        {
            continue;
        }
        let access: BTreeSet<_> = samples
            .iter()
            .filter(|s| {
                measurements::available(
                    s.a,
                    "top_sql_with_top_events",
                    !s.a.top_sql_with_top_events.is_empty(),
                )
            })
            .filter_map(|s| s.a.top_sql_with_top_events.get(id))
            .map(|s| s.top_row_source.to_uppercase())
            .collect();
        if !access
            .iter()
            .any(|s| s.contains("TABLE ACCESS") || s.contains("INDEX"))
        {
            continue;
        }
        let observed: Vec<_> = samples.iter().map(|s| s.a).collect();
        let n = observed.len();
        let costs = [("sql_gets", "gets/execution"), ("sql_cpu_time", "s/execution")]
            .into_iter()
            .map(|(domain, unit)| {
                let values: Vec<_> = observed.iter().filter_map(|a| sql_value(a, id, domain)).collect();
                let total = values.iter().map(|v| v.0).sum::<f64>();
                let exposure = values.iter().map(|v| v.1).sum::<f64>();
                Observation {
                    metric: domain.into(), unit: unit.into(), value: total / exposure,
                    total, exposure, exposure_unit: "executions".into(), period: period(&observed),
                    source: "Sum of observed domain totals / sum of that domain's executions. No baseline or trend is inferred.".into(),
                }
            }).collect();
        hints.push(PerformanceHint {
            hint_id:format!("{RULE_ID}:{}:{}:persistent:{id}:{}",c.db_instance_information.db_id,c.db_instance_information.instance_num,rows[0].snap_info.begin_snap_id),
            rule_id:RULE_ID.into(),rule_version:VERSION.into(),title:"Material table/index access cost".into(),dbid:c.db_instance_information.db_id,instance_id:c.db_instance_information.instance_num,
            episode_status:"persistent_observed_cost_no_healthy_baseline".into(),episode:period(&observed),baseline:None,comparison:None,evidence:Vec::new(),sql_evidence:Vec::new(),sql_candidates_total:1,
            possibly_affected_segments:Vec::new(),segment_candidates_total:0,segment_message:NO_SEGMENTS.into(),
            limitations:vec!["No baseline comparison is established. These absolute costs establish neither growth nor stability, and provide no evidence of fragmentation.".into(),"Observed TOP cost is a lower bound; excluded/unfinished executions are not zero.".into()],
            next_check:"Establish a comparable reference and validate useful work per execution before assessing efficiency or deterioration.".into(),
            assessment_status:"persistent_cost_observation".into(),confidence:"not_applicable".into(),assessment_scope:format!("sql:{id}"),mechanisms:Vec::new(),
            supporting_evidence:vec![format!("{n} common gets/CPU observations; {gets:.0} observed gets and {cpu:.2} CPU seconds; ASH row sources: {}.",access.into_iter().collect::<Vec<_>>().join(", "))],
            counterevidence:vec!["A material access cost can be appropriate for the requested data; fragmentation is unconfirmed.".into()],alternative_explanations:vec!["Necessary full scans, optional predicates/binds, useful data volume, repeated index probes".into()],comparisons:Vec::new(),plan_objects:Vec::new(),observed_cost_seconds:cpu,observed_metrics:costs,observed_segments:Vec::new(),signal_kind:"absolute_access_cost".into(),time_impact_status:"no_comparison".into(),context_evidence:Vec::new(),segment_context:Vec::new(),sql_controls:Vec::new(),trajectory:Vec::new(),
        });
    }
}

/// Reuse the existing attachment parser. A supplied plan is context, not proof
/// that every measured execution used its row sources. Only hashes actually seen
/// in the hint's observed intervals are attached; absent plans do not veto hints.
pub fn enrich_with_plans(report: &mut PerformanceHintsReport, c: &AWRSCollection, stem: &str) {
    let mut cache: BTreeMap<(String, u64), Value> = BTreeMap::new();
    for h in &mut report.hints {
        let Some(id) = h.assessment_scope.strip_prefix("sql:").map(str::to_string) else {
            continue;
        };
        let rows: Vec<_> = c
            .awrs
            .iter()
            .filter(|a| {
                h.episode
                    .intervals
                    .contains(&(a.snap_info.begin_snap_id, a.snap_info.end_snap_id))
                    || h.baseline.as_ref().is_some_and(|b| {
                        b.intervals
                            .contains(&(a.snap_info.begin_snap_id, a.snap_info.end_snap_id))
                    })
                    || h.comparisons.iter().any(|v| {
                        v.baseline
                            .intervals
                            .contains(&(a.snap_info.begin_snap_id, a.snap_info.end_snap_id))
                    })
            })
            .collect();
        let hashes: BTreeSet<_> = rows.iter().filter_map(|a| known_plan(a, &id)).collect();
        for hash in hashes {
            let v = cache.entry((id.clone(), hash)).or_insert_with(|| {
                crate::ai_tools::dispatch_tool_call_value(
                    "get_sql_execution_plan",
                    &json!({"sql_id":id,"plan_hash":hash.to_string()}),
                    c,
                    stem,
                )
            });
            if v["truncated"] == true {
                continue;
            }
            for op in v["plan_graph"]["operations"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let name = op["object_name"].as_str().unwrap_or("");
                let operation = op["operation"].as_str().unwrap_or("");
                if name.is_empty()
                    || !(operation.contains("TABLE ACCESS") || operation.contains("INDEX"))
                {
                    continue;
                }
                if !h.plan_objects.iter().any(|o| {
                    o.plan_hash == hash.to_string()
                        && o.object_name == name
                        && o.operation == operation
                }) {
                    h.plan_objects.push(PlanObject {sql_id:id.clone(),plan_hash:hash.to_string(),object_name:name.into(),operation:operation.into(),attribution:"Object appears in supplied plan with an observed hash; no row-source runtime cost or physical fragmentation is attributed.".into()});
                }
            }
        }
        // An index range scan does not exclude sparse index/table blocks or row
        // continuation. It changes the possible mechanism and confirmation step.
        if !h.is_cost_observation()
            && rows
                .iter()
                .filter(|a| {
                    measurements::available(
                        a,
                        "top_sql_with_top_events",
                        !a.top_sql_with_top_events.is_empty(),
                    )
                })
                .filter_map(|a| a.top_sql_with_top_events.get(&id))
                .any(|s| s.top_row_source.contains("INDEX"))
        {
            h.context_evidence.push(ContextEvidence {scope:h.assessment_scope.clone(),label:"Observed index access does not exclude fragmentation or row continuation; neither is established by a row-source name.".into(),status:"access_path_context".into(),comparison:None});
        }
        let names: BTreeSet<_> = h
            .plan_objects
            .iter()
            .map(|o| o.object_name.as_str())
            .collect();
        if h.is_cost_observation() {
            // Plan-linked counters are context, not possibly fragmented objects.
            // Aggregate each segment over its own observed exposure, once only.
            h.observed_segments = segment_observations(&rows, &names);
            h.observed_segments.truncate(5);
            h.segment_message = if h.observed_segments.is_empty() {
                "No conclusive plan-linked segment measurements are available."
            } else {
                "Observed plan-name matches only; no affected segment or SQL cost attribution is established."
            }.into();
            continue;
        }
        let Some((baseline, recent)) = h.baseline.as_ref().zip(h.comparison.as_ref()) else {
            continue;
        };
        let b = rows_for(&rows, baseline);
        let r = rows_for(&rows, recent);
        if !names.is_empty() {
            let mut candidates = segment_candidates(&b, &r, &report.policy);
            // Also retain a plan-linked segment whose reads/s fell with workload.
            // A domain's own observed exposure is the only valid denominator.
            for s in rows
                .iter()
                .filter(|a| {
                    measurements::available(a, "segment_stats", !a.segment_stats.is_empty())
                })
                .flat_map(|a| a.segment_stats.get("Logical Reads").into_iter().flatten())
            {
                if !names.contains(s.object_name.as_str())
                    || candidates
                        .iter()
                        .any(|v| segment_key(&v.segment) == segment_key(s))
                {
                    continue;
                }
                let sums = |rr: &[&AWR]| {
                    let mut vals = Vec::new();
                    for a in rr {
                        if !measurements::available(a, "segment_stats", !a.segment_stats.is_empty())
                        {
                            continue;
                        }
                        let matching: Vec<_> = a
                            .segment_stats
                            .get("Logical Reads")
                            .into_iter()
                            .flatten()
                            .filter(|v| segment_key(v) == segment_key(s))
                            .collect();
                        if matching.len() > 1 {
                            return None;
                        }
                        if let Some(v) = matching.first() {
                            if v.stat_vlalue.is_finite() && v.stat_vlalue >= 0.0 {
                                vals.push((v.stat_vlalue, measurements::seconds(a)?));
                            }
                        }
                    }
                    (!vals.is_empty()).then(|| {
                        (
                            vals.iter().map(|v| v.0).sum::<f64>()
                                / vals.iter().map(|v| v.1).sum::<f64>(),
                            vals.len(),
                        )
                    })
                };
                if let Some(((bv, bn), (rv, rn))) = sums(&b).zip(sums(&r)) {
                    candidates.push(SegmentCandidate{segment:s.clone(),logical_reads:Comparison::new("Logical Reads","logical reads/s",bv,rv,bn,rn,"Plan object name + observed segment logical reads; independent observed exposure."),identity_status:"plan_name_match_owner_container_not_proven".into(),attribution:"Name matches an object in a supplied plan. Owner/container and physical epoch must be checked; no measured cost attribution.".into()});
                }
            }
            candidates.retain(|s| names.contains(s.segment.object_name.as_str()));
            for s in &mut candidates {
                s.attribution="Plan object name and observed segment identity; owner/container and physical epoch require verification. Fragmentation is not attributed by these data.".into();
            }
            candidates.sort_by(|a, b| {
                b.logical_reads
                    .recent
                    .total_cmp(&a.logical_reads.recent)
                    .then_with(|| segment_key(&a.segment).cmp(&segment_key(&b.segment)))
            });
            h.segment_candidates_total = candidates.len();
            candidates.truncate(5);
            h.possibly_affected_segments = candidates;
        } else {
            // Without a plan, simultaneous TOP segments are not SQL object proof.
            h.possibly_affected_segments.clear();
            h.segment_candidates_total = 0;
            h.limitations.push("No usable plan object mapping was supplied; SQL text alone is not segment attribution.".into());
        }
        h.segment_message=if h.segment_candidates_total==0 {NO_SEGMENTS} else {"Plan-name candidates with observed segment data; verify owner/container and runtime attribution."}.into();
    }
}

pub fn index(report: Option<&PerformanceHintsReport>) -> Value {
    let Some(r) = report else {
        return json!({"status":"not_computed","hints_total":0});
    };
    json!({"hints_total":r.hints.len(),"hypotheses_total":r.hints.iter().filter(|h| !h.is_cost_observation()).count(),"cost_observations_total":r.hints.iter().filter(|h| h.is_cost_observation()).count(),"rule_evaluations":r.rule_evaluations.iter().map(|e|json!({"rule_id":e.rule_id,"status":e.status,"selected_windows":e.selected_windows,"assessed_comparisons":e.assessed_comparisons,"excluded_windows":e.excluded_windows,"reasons":e.reasons})).collect::<Vec<_>>(),"preview":r.hints.iter().take(5).map(|h|json!({"hint_id":h.hint_id,"title":h.title,"assessment_status":h.assessment_status,"confidence":h.confidence,"assessment_scope":h.assessment_scope,"signal_kind":h.signal_kind,"time_impact_status":h.time_impact_status,"episode_status":h.episode_status})).collect::<Vec<_>>(),"access":"get_precomputed_analysis(section=performance_hints)","interpretation":"Work-growth signals preserve gets-only histories and assess time independently. Assessment cost can be null; inspect cost_evaluations for observed, missing or unchanged time. Continued/ROWID is instance context, not SQL/segment attribution or a chained-row percentage. Trajectory describes supplied windows, not repairs. Physical mechanisms remain alternatives. Persistent-cost observations use absolute values and establish neither growth nor stability."})
}
pub fn query(report: Option<&PerformanceHintsReport>, args: &Value) -> Value {
    let Some(r) = report else {
        return json!({"status":"not_computed","hints":[]});
    };
    let limit = args["limit"].as_u64().unwrap_or(20).clamp(1, 100) as usize;
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let filtered: Vec<_> = r
        .hints
        .iter()
        .filter(|h| {
            args["rule_id"].as_str().is_none_or(|s| s == h.rule_id)
                && args["hint_id"].as_str().is_none_or(|s| s == h.hint_id)
                && args["scope"]
                    .as_str()
                    .is_none_or(|s| s == h.assessment_scope)
        })
        .collect();
    // Narrow drills also narrow coverage, so asking about one SQL does not
    // return hundreds of unrelated profile rows. Global totals remain labelled.
    let coverage_scope = args["scope"].as_str().or_else(|| {
        args["hint_id"]
            .as_str()
            .and_then(|_| filtered.first().map(|h| h.assessment_scope.as_str()))
    });
    let mut evaluations = r.rule_evaluations.clone();
    if let Some(scope) = coverage_scope {
        for e in &mut evaluations {
            e.scope_evaluations.retain(|s| s.scope == scope);
        }
    }
    json!({"version":r.version,"policy":r.policy,"policy_status":r.policy_status,"policy_notes":r.policy_notes,"coverage_scope":coverage_scope,"rule_evaluations":evaluations,"total":filtered.len(),"offset":offset,"limit":limit,"hints":filtered.into_iter().skip(offset).take(limit).collect::<Vec<_>>()})
}
fn escape(s: &str) -> String {
    html_escape::encode_text(s).into_owned()
}
fn render_cost_observation(h: &PerformanceHint) -> String {
    // Do not feed absolute costs through the comparison renderer, even when an
    // older saved report carries a fabricated baseline/recent pair. It needs a
    // fresh analysis to populate these measured values with their real exposure.
    let metrics = h.observed_metrics.iter().map(|m| format!(
        "<li>{}: <strong>{:.4} {}</strong> · {:.2} total / {:.0} {} across {} observed windows.</li>",
        escape(&m.metric), m.value, escape(&m.unit), m.total, m.exposure,
        escape(&m.exposure_unit), m.period.windows,
    )).collect::<String>();
    let segments = h.observed_segments.iter().map(|s| format!(
        "<li>{} · {} · OBJ# {} / DATAOBJ# {} · logical reads/s {:.2} across {} observed windows. {}</li>",
        escape(&s.segment.object_name), escape(&s.segment.object_type), s.segment.obj, s.segment.objd,
        s.logical_reads.value, s.logical_reads.period.windows, escape(&s.attribution),
    )).collect::<String>();
    let plans = h
        .plan_objects
        .iter()
        .map(|o| {
            format!(
                "<li>{}: {} (plan {}).</li>",
                escape(&o.object_name),
                escape(&o.operation),
                escape(&o.plan_hash),
            )
        })
        .collect::<String>();
    format!("<article class=cost-observation><p class=muted>{}</p><h2>Observation: Material table/index access cost</h2><p><strong>Measured cost:</strong> Absolute measurements over the observed period; no before/after comparison.</p><ul>{}</ul><p class=muted>Observed SNAP {}–{} ({} windows). Exact metric exposure is available below.</p><h3>Plan-linked segment measurements:</h3><ul>{}</ul><p>{}</p><details><summary>Objects in supplied plans</summary><ul>{}</ul></details><p><strong>Interpretation:</strong> Material cost alone provides no evidence of fragmentation or deterioration. Necessary work can also be expensive.</p><p><strong>Next check:</strong> {}</p><details><summary>Measurements and coverage</summary><pre>{}</pre></details></article>",
        escape(&h.assessment_scope), if metrics.is_empty() { "<li>Regenerate this report to obtain absolute measurements.</li>".into() } else { metrics },
        h.episode.begin_snap_id, h.episode.end_snap_id, h.episode.windows, segments,
        escape(&h.segment_message), plans, escape(&h.next_check), escape(&serde_json::to_string_pretty(h).unwrap()))
}

fn comparison_html(c: &Comparison, instance: bool) -> String {
    let (scale, unit) = if c.unit == "s/execution" {
        (1000.0, "ms/execution")
    } else {
        (1.0, c.unit.as_str())
    };
    let label = if instance && c.metric == "CPU" {
        if c.unit == "s/s" {
            "instance DB CPU / wall seconds"
        } else {
            "instance DB CPU / execution count"
        }
    } else if instance && c.metric == "elapsed" {
        "instance DB Time / execution count or wall seconds"
    } else {
        c.metric.as_str()
    };
    format!(
        "{}: <strong>{:.3} → {:.3} {}</strong>{}",
        escape(label),
        c.baseline * scale,
        c.recent * scale,
        escape(unit),
        c.growth_pct
            .map(|v| format!(" ({v:+.1}%)"))
            .unwrap_or_else(|| " (zero baseline; percentage undefined)".into())
    )
}

fn status_label(s: &str) -> &str {
    match s {
        "historical_growth_later_within_reference" => {
            "Historical growth; later work returned within the reference threshold"
        }
        "growth_persists_below_representative_value" => {
            "Growth persists; latest work is below the representative value"
        }
        "growth_in_latest_supplied_window" => "Growth remains in the latest supplied window",
        "historical_growth_latest_mix_not_comparable" => {
            "Historical growth; latest workload mix is not comparable"
        }
        "historical_growth_latest_work_unavailable" => {
            "Historical growth; latest work measurement is unavailable"
        }
        "historical_growth_latest_profile_without_reference" => {
            "Historical growth; latest profile has no comparable reference"
        }
        "observed_time_growth" => "Time growth observed in the representative comparison",
        "time_growth_with_wait_confounder" => {
            "Elapsed growth accompanies waits; access-time impact remains uncertain"
        }
        "time_comparison_unavailable" => {
            "Time impact unknown: a comparable time baseline or recent measurement is missing"
        }
        "no_unconfounded_time_growth" => {
            "Work growth observed; no unconfounded time growth established"
        }
        "no_material_read_growth" => "No material segment-read growth; context only",
        "activity_reference_unavailable" => {
            "Activity reference missing; segment cost growth is unestablished"
        }
        "activity_growth_can_explain_reads" => {
            "Activity growth can explain the extra reads; context only"
        }
        _ => s,
    }
}

fn render_trajectory(h: &PerformanceHint) -> String {
    if h.trajectory.is_empty() {
        return String::new();
    }
    let points = &h.trajectory;
    let start = measurements::timestamp(&points[0].period.begin_time)
        .unwrap()
        .and_utc()
        .timestamp() as f64;
    let end = measurements::timestamp(&points.last().unwrap().period.end_time)
        .unwrap()
        .and_utc()
        .timestamp() as f64;
    let high = points
        .iter()
        .flat_map(|p| [p.work, p.baseline])
        .flatten()
        .fold(0.0, f64::max)
        .max(1e-12);
    let mut marks = String::new();
    let mut table = String::new();
    let mut previous: Option<(&TrajectoryPoint, f64, f64)> = None;
    for p in points {
        let t = measurements::timestamp(&p.period.begin_time)
            .unwrap()
            .and_utc()
            .timestamp() as f64;
        let x = 28.0 + (t - start) / (end - start).max(1.0) * 704.0;
        if let Some(value) = p.work {
            let y = 146.0 - value / high * 124.0;
            // Never bridge missing measurements, disjoint windows or different
            // calendar cohorts with a line that would invent a continuous trend.
            if let Some((prev, px, py)) = previous {
                if prev.profile == p.profile && prev.period.end_time == p.period.begin_time {
                    marks.push_str(&format!(
                        "<path d='M{px:.2},{py:.2} L{x:.2},{y:.2}' stroke='#32678a' fill='none'/>"
                    ));
                }
            }
            if let Some(base) = p.baseline {
                let by = 146.0 - base / high * 124.0;
                marks.push_str(&format!(
                    "<path d='M{:.2},{by:.2} h8' stroke='#81909b' stroke-width='2'/>",
                    x - 4.0
                ));
            }
            marks.push_str(&format!("<circle cx='{x:.2}' cy='{y:.2}' r='3' fill='{}'><title>SNAP {}–{}: {:.4}; reference {}; {}</title></circle>",if p.status=="work_growth"{"#b85022"}else{"#32678a"},p.period.begin_snap_id,p.period.end_snap_id,value,p.baseline.map(|v|format!("{v:.4}")).unwrap_or_else(||"unavailable".into()),escape(&p.status)));
            previous = Some((p, x, y));
        } else {
            previous = None;
        }
        let limits = h
            .comparisons
            .iter()
            .find(|a| a.recent.intervals == p.period.intervals)
            .map(|a| a.limitations.join("; "))
            .unwrap_or_default();
        table.push_str(&format!("<tr><td>{}–{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",p.period.begin_snap_id,p.period.end_snap_id,escape(&p.period.begin_time),escape(&p.profile),p.work.map(|v|format!("{v:.4}")).unwrap_or_else(||"unknown".into()),p.baseline.map(|v|format!("{v:.4}")).unwrap_or_else(||"unavailable".into()),escape(&p.status),escape(&limits)));
    }
    format!("<figure><svg viewBox='0 0 760 178' role=img aria-label='Observed work and comparable reference'><path d='M28,12 V146 H742' fill='none' stroke='#cad4da'/>{marks}<text x='28' y='170' font-size='12'>SNAP {}</text><text x='650' y='170' font-size='12'>SNAP {}</text></svg><figcaption>{}: observed work (blue/orange), reference (grey). Orange marks work growth. Gaps and different profiles are not joined. No repair is inferred.</figcaption></figure><details><summary>Observed work history and period-specific limitations</summary><div class=table-scroll><table><thead><tr><th>SNAP</th><th>Begin</th><th>Profile</th><th>Work</th><th>Reference</th><th>Assessment</th><th>Limits for this comparison</th></tr></thead><tbody>{table}</tbody></table></div></details>",points[0].period.begin_snap_id,points.last().unwrap().period.end_snap_id,escape(work_metric(&h.assessment_scope).1))
}

fn render_growth_hint(h: &PerformanceHint) -> String {
    let instance = h.assessment_scope.starts_with("instance");
    let evidence = h
        .evidence
        .iter()
        .map(|c| format!("<li>{}</li>", comparison_html(c, instance)))
        .collect::<String>();
    let representative = h.comparison.as_ref().and_then(|r| {
        h.comparisons
            .iter()
            .find(|a| a.recent.intervals == r.intervals)
    });
    let coverage = representative
        .map(|a| {
            a.cost_evaluations
                .iter()
                .map(|c| {
                    format!(
                        "<li>{}: {} baseline / {} recent time observations; {}. {}</li>",
                        escape(&c.domain),
                        c.baseline_observations,
                        c.recent_observations,
                        escape(&c.status),
                        c.baseline
                            .as_ref()
                            .map(|b| format!(
                                "Time reference SNAP {}–{}.",
                                b.begin_snap_id, b.end_snap_id
                            ))
                            .unwrap_or_else(|| "No comparable time reference.".into())
                    )
                })
                .collect::<String>()
        })
        .unwrap_or_default();
    let counters = h
        .counterevidence
        .iter()
        .map(|s| format!("<li>{}</li>", escape(s)))
        .collect::<String>();
    let context = h
        .context_evidence
        .iter()
        .map(|c| {
            format!(
                "<li><strong>{}</strong>: {}{}</li>",
                escape(&c.label),
                escape(&c.status),
                c.comparison
                    .as_ref()
                    .map(|v| format!(" — {}", comparison_html(v, true)))
                    .unwrap_or_default()
            )
        })
        .collect::<String>();
    let segments = h
        .possibly_affected_segments
        .iter()
        .map(|s| {
            format!(
                "<li>{} · {} · OBJ# {} / DATAOBJ# {} · {}. {}</li>",
                escape(&s.segment.object_name),
                escape(&s.segment.object_type),
                s.segment.obj,
                s.segment.objd,
                comparison_html(&s.logical_reads, true),
                escape(&s.attribution)
            )
        })
        .collect::<String>();
    let segments = if segments.is_empty() {
        format!("<p>{NO_SEGMENTS}</p>")
    } else {
        format!("<ul>{segments}</ul><p>{}</p>", escape(&h.segment_message))
    };
    let plans = if h.plan_objects.is_empty() {
        "<p class=muted>No usable execution plan supplied for this hint; object mapping is unconfirmed.</p>".into()
    } else {
        format!(
            "<details><summary>Objects in supplied plans</summary><ul>{}</ul></details>",
            h.plan_objects
                .iter()
                .map(|o| format!(
                    "<li>{}: {} (plan {}); {}</li>",
                    escape(&o.object_name),
                    escape(&o.operation),
                    escape(&o.plan_hash),
                    escape(&o.attribution)
                ))
                .collect::<String>()
        )
    };
    let mut controls = h
        .segment_context
        .iter()
        .map(|s| {
            format!(
                "<li>{}: {}; {}.</li>",
                escape(&s.segment.object_name),
                comparison_html(&s.logical_reads, true),
                escape(status_label(&s.status))
            )
        })
        .collect::<String>();
    controls.push_str(&h.sql_controls.iter().map(|s|format!("<li>SQL {} — no material unit-work growth: {}. No SQL–segment mapping is inferred.</li>",escape(&s.sql_id),s.costs.iter().map(|c|comparison_html(c,false)).collect::<Vec<_>>().join("; "))).collect::<String>());
    let controls = if controls.is_empty() {
        String::new()
    } else {
        format!("<details><summary>Other segment and SQL observations</summary><ul>{controls}</ul></details>")
    };
    let periods=h.baseline.as_ref().zip(h.comparison.as_ref()).map(|(b,r)|format!("Representative comparison: work reference SNAP {}–{} ({} observations); recent SNAP {}–{}. Time domains use their own references shown below.",b.begin_snap_id,b.end_snap_id,b.windows,r.begin_snap_id,r.end_snap_id)).unwrap_or_default();
    let latest = h
        .trajectory
        .last()
        .map(|p| {
            format!(
                "Latest supplied window: SNAP {}–{}, {}; profile {}.",
                p.period.begin_snap_id,
                p.period.end_snap_id,
                escape(&p.period.begin_time),
                escape(&p.profile)
            )
        })
        .unwrap_or_default();
    format!("<article class=growth-hypothesis><p class=muted>{} · {} · signal confidence: {}</p><h2>Hint: {}</h2><p><strong>{}</strong></p><p class=muted>{latest}</p><p>Observed episode: SNAP {}–{}, {} to {}. Intervals may be non-contiguous.</p>{}<p><strong>Observed evidence</strong> — scope: {}. Physical cause remains unresolved.</p><p class=muted>{periods}</p><ul>{evidence}</ul><p><strong>Time assessment:</strong> {}</p><details><summary>Time-domain coverage</summary><ul>{coverage}</ul></details><ul>{}</ul>{}<h3>Possibly affected segments</h3>{segments}{plans}{controls}<p><strong>Possible causes, not established mechanisms:</strong> {}</p><p><strong>Next check:</strong> {}</p><details><summary>Complete evidence and coverage</summary><pre>{}</pre></details></article>",
        escape(&h.assessment_scope),escape(&h.assessment_status),escape(&h.confidence),escape(&h.title),escape(status_label(&h.episode_status)),h.episode.begin_snap_id,h.episode.end_snap_id,escape(&h.episode.begin_time),escape(&h.episode.end_time),render_trajectory(h),escape(&h.assessment_scope),escape(status_label(&h.time_impact_status)),h.supporting_evidence.iter().map(|v|format!("<li>{}</li>",escape(v))).collect::<String>(),
        if context.is_empty() && counters.is_empty(){String::new()}else{format!("<section class=context-evidence><h3>Context and limits</h3><ul>{context}{counters}</ul></section>")},escape(&h.alternative_explanations.join("; ")),escape(&h.next_check),escape(&serde_json::to_string_pretty(h).unwrap()))
}

pub fn render_html(report: &PerformanceHintsReport) -> String {
    let mut cards = String::new();
    let hypotheses: Vec<_> = report
        .hints
        .iter()
        .filter(|h| !h.is_cost_observation())
        .collect();
    for (i, h) in hypotheses.iter().enumerate() {
        if i == 5 {
            cards.push_str("<details><summary>More observed work-growth hints</summary>");
        }
        cards.push_str(&render_growth_hint(h));
    }
    if hypotheses.len() > 5 {
        cards.push_str("</details>");
    }
    if cards.is_empty() {
        cards=format!("<article><h2>{}</h2><p>This does not establish absence of fragmentation. Inspect scoped coverage and missing observations.</p></article>",if report.rule_evaluations.iter().any(|e|e.assessed_comparisons>0){"No work-growth signal in assessed profiles"}else{"Insufficient comparable data for this hint"});
    }
    let observations: Vec<_> = report
        .hints
        .iter()
        .filter(|h| h.is_cost_observation())
        .collect();
    if !observations.is_empty() {
        cards.push_str(&format!("<details class=access-cost-observations><summary>Access cost observations ({}) — no degradation comparison</summary><p>These measurements establish neither growth nor stability and do not support a fragmentation hypothesis.</p>",observations.len()));
        for h in observations {
            cards.push_str(&render_cost_observation(h));
        }
        cards.push_str("</details>");
    }
    format!(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>JAS-MIN · HINTS</title><style>body{{background:#f2f5f7;color:#182c3b;font:16px/1.6 system-ui;margin:0}}main{{max-width:1080px;margin:auto;padding:28px 20px}}article{{background:white;border:1px solid #d4e0e5;border-top:4px solid #b78025;border-radius:12px;padding:24px;margin:24px 0;overflow-wrap:anywhere}}h2{{font-size:24px;line-height:1.3}}h3{{font-size:17px}}.muted{{color:#516977;font-size:14px}}summary{{cursor:pointer;font-weight:600}}figure{{margin:18px 0}}svg{{width:100%;height:auto}}figcaption{{font-size:13px;color:#516977}}table{{border-collapse:collapse;font-size:13px}}th,td{{padding:8px;border-bottom:1px solid #d4e0e5;text-align:left;vertical-align:top}}.table-scroll{{overflow-x:auto}}.context-evidence{{border-left:3px solid #81909b;padding-left:14px}}details{{margin:16px 0}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;background:#edf3f5;padding:16px;font:13px/1.5 monospace}}@media(max-width:600px){{main{{padding:12px}}article{{padding:16px}}}}</style></head><body><main><h1>HINTS</h1><p>Work-growth signals appear first; time support is assessed independently. Absolute access cost observations are available separately below. Confidence describes evidence strength, not probability of physical fragmentation.</p>{cards}<details><summary>Rule coverage and policy</summary><pre>{}</pre></details><p class=muted>{} Rule version {}. Production accuracy has not been calibrated.</p></main></body></html>"#,escape(&serde_json::to_string_pretty(&json!({"rule_evaluations":report.rule_evaluations,"policy":report.policy,"policy_notes":report.policy_notes})).unwrap()),escape(CAUTION),VERSION)
}

#[cfg(test)]
#[path = "performance_hints_tests.rs"]
pub(crate) mod tests;
