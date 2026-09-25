//! Evidence-gated access-path diagnostics. Instance correlations never prove segment structure.
use crate::awr::{AWRSCollection, AWR};
use crate::measurements::{available, host_cpu_available, seconds, selected, timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const SCAN: &str = "table scan blocks gotten";
const CONTINUED: &str = "table fetch continued row";
pub const LIMITS: &str = "Instance statistics describe the instance, not a SQL or segment. table scan rows gotten and aggregate ROWS_PROCESSED are not live rows or useful application work. Model agreement is not causal evidence. Sparse AWR TOP SQL sections are partial observations. No per-segment table fetch continued row statistic is invented. Space measurements and chained-row lists do not by themselves prove performance impact. No automatic MOVE, SHRINK, rebuild or ANALYZE is performed.";

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Scope {
    pub dbid: u64,
    pub inst_id: u32,
    pub con_id: Option<u32>,
    pub sql_id: String,
    pub child_number: Option<u32>,
    pub plan_hash_value: Option<u64>,
    pub object_id: Option<u64>,
    pub data_object_id: Option<u64>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct UsefulWork {
    pub name: String,
    pub unit: String,
    pub completed: f64,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct StructureEvidence {
    pub evidence_ref: String,
    pub observed_at: String,
    /// Explicit source method, e.g. block inspection, DBMS_SPACE, chained-row list.
    pub method: String,
    pub blocks_below_hwm: Option<u64>,
    /// Must be verified empty blocks, not FS4 (75-100% free space).
    pub verified_empty_blocks_below_hwm: Option<u64>,
    pub fs4_blocks: Option<u64>,
    pub chained_rows: Option<u64>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct InterventionEvidence {
    pub evidence_ref: String,
    pub controls_evidence_ref: String,
    pub before_begin_snap_id: u64,
    pub after_begin_snap_id: u64,
    /// Exact post-intervention identity, including a possibly changed DATAOBJ#.
    pub after_scope: Scope,
    pub same_plan: bool,
    pub same_logical_data: bool,
    pub comparable_cache_and_concurrency: bool,
    pub equivalent_work: bool,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct SqlObservation {
    pub scope: Scope,
    pub evidence_ref: String,
    pub executions: u64,
    pub buffer_gets: Option<f64>,
    pub elapsed_s: Option<f64>,
    pub useful_work: Option<UsefulWork>,
    /// SQL-attributed session/trace deltas, never copied from V$SYSSTAT.
    pub scan_blocks: Option<u64>,
    pub continued_rows: Option<u64>,
    pub structure: Option<StructureEvidence>,
    pub intervention: Option<InterventionEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub minimum_events: u64,
    pub minimum_events_per_second: f64,
    pub minimum_cost_growth_pct: f64,
    pub minimum_baseline_samples: usize,
    pub minimum_recent_samples: usize,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            minimum_events: 100,
            minimum_events_per_second: 1.0,
            minimum_cost_growth_pct: 25.0,
            minimum_baseline_samples: 3,
            minimum_recent_samples: 2,
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct AccessPathReport {
    pub schema_version: String,
    pub evidence_level: String,
    pub policy: Option<Policy>,
    pub windows: Value,
    pub coverage: Value,
    pub instance_signals: Vec<Value>,
    pub sql_costs: Vec<Value>,
    pub segment_candidates: Vec<Value>,
    pub limitations: String,
}

fn rate_window(awrs: &[&AWR], name: &str) -> (u64, f64, usize) {
    let mut total = 0u64;
    let mut exposure = 0.0;
    let mut n = 0;
    for a in awrs {
        if !available(a, "instance_stats", !a.instance_stats.is_empty()) {
            continue;
        }
        if let (Some(s), Some(v)) = (
            seconds(a),
            a.instance_stats.iter().find(|v| v.statname == name),
        ) {
            total = total.saturating_add(v.total);
            exposure += s;
            n += 1;
        }
    }
    (total, exposure, n)
}
fn ratio(value: f64, denominator: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0 && denominator.is_finite() && denominator > 0.0)
        .then_some(value / denominator)
}
fn growth(b: Option<f64>, r: Option<f64>) -> Option<f64> {
    match (b, r) {
        (Some(b), Some(r)) if b > 0.0 => Some((r / b - 1.0) * 100.0),
        _ => None,
    }
}
fn scope_valid(s: &Scope, c: &AWRSCollection) -> bool {
    s.dbid == c.db_instance_information.db_id
        && s.inst_id == c.db_instance_information.instance_num as u32
        && !s.sql_id.trim().is_empty()
}
fn numeric_valid(o: &SqlObservation) -> bool {
    o.executions > 0
        && !o.evidence_ref.trim().is_empty()
        && [o.buffer_gets, o.elapsed_s]
            .iter()
            .flatten()
            .all(|v| v.is_finite() && *v >= 0.0)
        && o.useful_work.as_ref().is_none_or(|w| {
            w.completed.is_finite()
                && w.completed > 0.0
                && !w.name.trim().is_empty()
                && !w.unit.trim().is_empty()
        })
}
fn complete_scope(s: &Scope) -> bool {
    s.con_id.is_some()
        && s.child_number.is_some()
        && s.plan_hash_value.is_some_and(|x| x > 0)
        && s.object_id.is_some_and(|x| x > 0)
        && s.data_object_id.is_some_and(|x| x > 0)
}

#[derive(Default)]
struct Cost {
    gets: f64,
    elapsed: f64,
    gets_exec: f64,
    elapsed_exec: f64,
    gets_n: usize,
    elapsed_n: usize,
    work: f64,
    work_gets: f64,
    work_elapsed: f64,
    work_n: usize,
}
impl Cost {
    fn add(&mut self, o: &SqlObservation) {
        if let Some(g) = o.buffer_gets {
            self.gets += g;
            self.gets_exec += o.executions as f64;
            self.gets_n += 1;
        }
        if let Some(e) = o.elapsed_s {
            self.elapsed += e;
            self.elapsed_exec += o.executions as f64;
            self.elapsed_n += 1;
        }
        if let (Some(w), Some(g), Some(e)) = (&o.useful_work, o.buffer_gets, o.elapsed_s) {
            self.work += w.completed;
            self.work_gets += g;
            self.work_elapsed += e;
            self.work_n += 1;
        }
    }
    fn value(&self) -> Value {
        json!({"gets_samples":self.gets_n,"elapsed_samples":self.elapsed_n,"gets_executions":self.gets_exec,"elapsed_executions":self.elapsed_exec,"buffer_gets_per_execution":ratio(self.gets,self.gets_exec),"elapsed_s_per_execution":ratio(self.elapsed,self.elapsed_exec),"useful_work_samples":self.work_n,"useful_work_completed":self.work,"buffer_gets_per_work_unit":ratio(self.work_gets,self.work),"elapsed_s_per_work_unit":ratio(self.work_elapsed,self.work)})
    }
}

/// Weighted costs use sum(delta)/sum(executions), never averages of ratios or 1 for zero executions.
fn compare_costs(
    rows: &[(usize, &AWR, SqlObservation)],
    split: usize,
    p: &Policy,
) -> (Value, bool) {
    let mut b = Cost::default();
    let mut r = Cost::default();
    let mut work_kinds = std::collections::BTreeSet::new();
    for (i, _, o) in rows {
        if let Some(w) = &o.useful_work {
            work_kinds.insert((&w.name, &w.unit));
        }
        if *i < split {
            b.add(o)
        } else {
            r.add(o)
        }
    }
    let gets_growth = growth(ratio(b.gets, b.gets_exec), ratio(r.gets, r.gets_exec));
    let elapsed_growth = growth(
        ratio(b.elapsed, b.elapsed_exec),
        ratio(r.elapsed, r.elapsed_exec),
    );
    let observed = b.gets_n >= p.minimum_baseline_samples
        && b.elapsed_n >= p.minimum_baseline_samples
        && r.gets_n >= p.minimum_recent_samples
        && r.elapsed_n >= p.minimum_recent_samples;
    let work_comparable = work_kinds.len() == 1
        && b.work_n >= p.minimum_baseline_samples
        && r.work_n >= p.minimum_recent_samples;
    let work_gets_growth = work_comparable
        .then(|| growth(ratio(b.work_gets, b.work), ratio(r.work_gets, r.work)))
        .flatten();
    let work_elapsed_growth = work_comparable
        .then(|| growth(ratio(b.work_elapsed, b.work), ratio(r.work_elapsed, r.work)))
        .flatten();
    let cost_increase = observed
        && gets_growth.is_some_and(|x| x >= p.minimum_cost_growth_pct)
        && elapsed_growth.is_some_and(|x| x >= p.minimum_cost_growth_pct);
    // When application work is supplied it must support the per-execution signal.
    let useful_increase = work_kinds.is_empty()
        || (work_comparable
            && work_gets_growth.is_some_and(|x| x >= p.minimum_cost_growth_pct)
            && work_elapsed_growth.is_some_and(|x| x >= p.minimum_cost_growth_pct));
    (
        json!({"baseline":b.value(),"recent":r.value(),"buffer_gets_per_execution_growth_pct":gets_growth,"elapsed_s_per_execution_growth_pct":elapsed_growth,"sufficient_observations":observed,"useful_work_comparable":work_comparable,"useful_work_definitions":work_kinds,"buffer_gets_per_work_unit_growth_pct":work_gets_growth,"elapsed_s_per_work_unit_growth_pct":work_elapsed_growth,"cost_increase_detected":cost_increase && useful_increase}),
        cost_increase && useful_increase,
    )
}

pub fn build(
    c: &AWRSCollection,
    range: &(u64, u64),
    db_degraded: bool,
    policy: Policy,
) -> AccessPathReport {
    let awrs = selected(c, range);
    let n = awrs.len();
    let split = if n >= 5 {
        crate::degradation::split_windows(n).0.len()
    } else {
        n
    };
    let (baseline, recent) = awrs.split_at(split);
    let mut coverage = BTreeMap::new();
    for (domain, count) in [
        (
            "host_cpu",
            awrs.iter().filter(|a| host_cpu_available(a)).count(),
        ),
        (
            "instance_stats",
            awrs.iter()
                .filter(|a| available(a, "instance_stats", !a.instance_stats.is_empty()))
                .count(),
        ),
        (
            "foreground_wait_events",
            awrs.iter()
                .filter(|a| {
                    available(
                        a,
                        "foreground_wait_events",
                        !a.foreground_wait_events.is_empty(),
                    )
                })
                .count(),
        ),
        (
            "time_model_stats",
            awrs.iter()
                .filter(|a| available(a, "time_model_stats", !a.time_model_stats.is_empty()))
                .count(),
        ),
        (
            "interval_exposure",
            awrs.iter().filter(|a| seconds(a).is_some()).count(),
        ),
        (
            "sql_elapsed_time",
            awrs.iter()
                .filter(|a| available(a, "sql_elapsed_time", !a.sql_elapsed_time.is_empty()))
                .count(),
        ),
        (
            "sql_gets",
            awrs.iter()
                .filter(|a| available(a, "sql_gets", !a.sql_gets.is_empty()))
                .count(),
        ),
        (
            "ash",
            awrs.iter()
                .filter(|a| available(a, "ash", !a.top_sql_with_top_events.is_empty()))
                .count(),
        ),
        (
            "segments",
            awrs.iter()
                .filter(|a| available(a, "segment_stats", !a.segment_stats.is_empty()))
                .count(),
        ),
        (
            "attributed_sql_observations",
            awrs.iter()
                .filter(|a| {
                    available(
                        a,
                        "access_path_observations",
                        !a.access_path_observations.is_empty(),
                    )
                })
                .count(),
        ),
    ] {
        coverage.insert(domain,json!({"observed_windows":count,"total_windows":n,"status":if count==0 {"unavailable"} else if count==n {"observed"} else {"partial"}}));
    }
    let mut instance_signals = Vec::new();
    for name in [
        SCAN,
        "table scan rows gotten",
        CONTINUED,
        "cleanouts only - consistent read gets",
        "session logical reads",
        "consistent gets",
    ] {
        let (bt, bs, bn) = rate_window(baseline, name);
        let (rt, rs, rn) = rate_window(recent, name);
        let br = ratio(bt as f64, bs);
        let rr = ratio(rt as f64, rs);
        let material = bn >= policy.minimum_baseline_samples
            && rn >= policy.minimum_recent_samples
            && rt >= policy.minimum_events
            && rr.is_some_and(|x| x >= policy.minimum_events_per_second);
        instance_signals.push(json!({"name":name,"unit":"events/s","baseline_total":bt,"recent_total":rt,"baseline_exposure_s":bs,"recent_exposure_s":rs,"baseline_observed_windows":bn,"recent_observed_windows":rn,"baseline_rate":br,"recent_rate":rr,"rate_growth_pct":growth(br,rr),"material_activity":material,"evidence_level":if material {"instance_activity_requires_sql_attribution"} else {"context_only"},"mechanism_confirmed":false}));
    }
    let mut groups: BTreeMap<(Scope, String), Vec<(usize, &AWR, SqlObservation)>> = BTreeMap::new();
    let mut rejected = 0usize;
    for (i, a) in awrs.iter().enumerate() {
        if available(
            a,
            "access_path_observations",
            !a.access_path_observations.is_empty(),
        ) {
            let mut counts = BTreeMap::new();
            for o in &a.access_path_observations {
                *counts.entry(&o.scope).or_insert(0usize) += 1;
            }
            for o in &a.access_path_observations {
                if counts[&o.scope] != 1
                    || !scope_valid(&o.scope, c)
                    || !numeric_valid(o)
                    || seconds(a).is_none()
                {
                    rejected += 1;
                    continue;
                }
                groups
                    .entry((o.scope.clone(), "attributed".into()))
                    .or_default()
                    .push((i, a, o.clone()));
            }
        }
        // Existing AWR TOP sections can establish SQL cost inflation, but contain no child/segment identity.
        let ids: std::collections::BTreeSet<_> = a
            .sql_elapsed_time
            .iter()
            .map(|x| x.sql_id.clone())
            .chain(a.sql_gets.keys().cloned())
            .collect();
        for id in ids {
            let e: Vec<_> = a
                .sql_elapsed_time
                .iter()
                .filter(|x| {
                    x.sql_id == id && x.executions > 0 && available(a, "sql_elapsed_time", true)
                })
                .collect();
            let g = a
                .sql_gets
                .get(&id)
                .filter(|x| x.executions > 0 && available(a, "sql_gets", true));
            let scope = Scope {
                dbid: c.db_instance_information.db_id,
                inst_id: c.db_instance_information.instance_num as u32,
                sql_id: id.clone(),
                ..Default::default()
            };
            // Independent denominators for independently sampled TOP sections.
            for o in [
                e.first().filter(|_| e.len() == 1).map(|e| SqlObservation {
                    scope: scope.clone(),
                    executions: e.executions,
                    elapsed_s: Some(e.elapsed_time_s),
                    evidence_ref: a.file_name.clone(),
                    ..Default::default()
                }),
                g.map(|g| SqlObservation {
                    scope: scope.clone(),
                    executions: g.executions,
                    buffer_gets: Some(g.buffer_gets),
                    evidence_ref: a.file_name.clone(),
                    ..Default::default()
                }),
            ]
            .into_iter()
            .flatten()
            {
                if numeric_valid(&o) {
                    groups
                        .entry((scope.clone(), "awr_top_sql".into()))
                        .or_default()
                        .push((i, a, o));
                }
            }
        }
    }
    coverage.insert("rejected_attributed_observations", json!(rejected));
    let mut sql_costs = Vec::new();
    let mut segment_candidates = Vec::new();
    for ((scope, source), rows) in &groups {
        let (costs, increased) = compare_costs(rows, split, &policy);
        sql_costs.push(json!({"scope":scope,"source":source,"costs":costs,"attribution":if source=="awr_top_sql" {"SQL_ID only; child, plan and segment not supplied"} else {"explicit SQL-attributed measurements"}}));
        if source != "attributed" {
            continue;
        }
        for (mechanism, counter) in [
            ("scan_amplification", SCAN),
            ("row_continuation", CONTINUED),
        ] {
            let relevant: Vec<_> = rows.iter().filter(|(i, _, _)| *i >= split).collect();
            let mut total = 0u64;
            let mut exposure = 0.0;
            let mut observations = 0;
            for (_, a, o) in &relevant {
                if let Some(v) = if counter == SCAN {
                    o.scan_blocks
                } else {
                    o.continued_rows
                } {
                    total = total.saturating_add(v);
                    exposure += seconds(a).unwrap_or(0.0);
                    observations += 1;
                }
            }
            let material = observations >= policy.minimum_recent_samples
                && total >= policy.minimum_events
                && ratio(total as f64, exposure)
                    .is_some_and(|x| x >= policy.minimum_events_per_second);
            let mut level = "context_only";
            let mut missing = Vec::new();
            if !costs["useful_work_comparable"].as_bool().unwrap_or(false) {
                missing.push("application-defined comparable useful work; executions alone do not establish workload equivalence");
            }
            if !material {
                missing.push("material SQL-attributed scan/continuation volume and exposure");
            }
            if !increased {
                missing.push(
                    "observed increase in buffer gets and elapsed time per comparable work unit",
                );
            }
            if material && increased {
                level = "access_path_suspected";
            }
            if !complete_scope(scope) {
                missing.push("SQL child/plan and OBJ#/DATAOBJ# identity");
            }
            if material && increased && complete_scope(scope) {
                level = "segment_candidate";
            }
            let structure = relevant.iter().find_map(|(_, a, o)| {
                o.structure.as_ref().filter(|s| {
                    let within = timestamp(&s.observed_at)
                        .zip(timestamp(&a.snap_info.begin_snap_time))
                        .zip(timestamp(&a.snap_info.end_snap_time))
                        .is_some_and(|((t, b), e)| t >= b && t <= e);
                    within
                        && !s.evidence_ref.trim().is_empty()
                        && !s.method.trim().is_empty()
                        && if counter == SCAN {
                            s.verified_empty_blocks_below_hwm
                                .zip(s.blocks_below_hwm)
                                .is_some_and(|(empty, all)| empty > 0 && empty <= all)
                        } else {
                            s.chained_rows.is_some_and(|x| x > 0)
                        }
                })
            });
            if structure.is_none() {
                missing.push("same-window verified empty blocks below HWM or chained-row list; FS4 alone is insufficient");
            }
            if level == "segment_candidate" && structure.is_some() {
                level = "structure_confirmed";
            }
            let verified = rows.iter().any(|(_, _, o)| {
                o.intervention
                    .as_ref()
                    .is_some_and(|ab| verify_intervention(ab, scope, rows, &groups, &policy, split))
            });
            if !verified {
                missing.push("controlled before/after intervention with linked windows, equivalent work and measured cost reduction");
            }
            if level == "structure_confirmed" && verified {
                level = "intervention_verified";
            }
            segment_candidates.push(json!({"scope":scope,"mechanism":mechanism,"evidence_level":level,"costs":costs,"recent_attributed_events":total,"recent_exposure_s":exposure,"material_activity":material,"structure":structure,"missing_evidence":missing,"evidence_refs":rows.iter().map(|(_,a,o)|json!({"begin_snap_id":a.snap_info.begin_snap_id,"end_snap_id":a.snap_info.end_snap_id,"begin":a.snap_info.begin_snap_time,"end":a.snap_info.end_snap_time,"reference":o.evidence_ref})).collect::<Vec<_>>(),"interpretation":"row_continuation includes chaining and migration; distinguish them by structural inspection"}));
        }
    }
    AccessPathReport {
        schema_version: "2026-09-13.1".into(),
        evidence_level: if db_degraded {
            "degradation_detected"
        } else {
            "no_db_time_degradation_detected"
        }
        .into(),
        policy: Some(policy),
        windows: json!({"selection":"last ceil(0.25*N) windows, capped at 48, versus earlier windows; not phase labels","baseline":baseline.iter().map(|a|&a.snap_info).collect::<Vec<_>>(),"recent":recent.iter().map(|a|&a.snap_info).collect::<Vec<_>>()}),
        coverage: json!(coverage),
        instance_signals,
        sql_costs,
        segment_candidates,
        limitations: LIMITS.into(),
    }
}

type Groups<'a> = BTreeMap<(Scope, String), Vec<(usize, &'a AWR, SqlObservation)>>;
fn verify_intervention(
    ab: &InterventionEvidence,
    scope: &Scope,
    before_rows: &[(usize, &AWR, SqlObservation)],
    groups: &Groups<'_>,
    p: &Policy,
    split: usize,
) -> bool {
    if ab.evidence_ref.trim().is_empty()
        || ab.controls_evidence_ref.trim().is_empty()
        || !ab.same_plan
        || !ab.same_logical_data
        || !ab.comparable_cache_and_concurrency
        || !ab.equivalent_work
    {
        return false;
    }
    let s = &ab.after_scope;
    if !complete_scope(s)
        || s.dbid != scope.dbid
        || s.inst_id != scope.inst_id
        || s.con_id != scope.con_id
        || s.sql_id != scope.sql_id
        || s.plan_hash_value != scope.plan_hash_value
    {
        return false;
    }
    let Some((_, ba, b)) = before_rows
        .iter()
        .find(|(i, a, _)| *i >= split && a.snap_info.begin_snap_id == ab.before_begin_snap_id)
    else {
        return false;
    };
    let Some((_, aa, a)) = groups.get(&(s.clone(), "attributed".into())).and_then(|r| {
        r.iter()
            .find(|(_, a, _)| a.snap_info.begin_snap_id == ab.after_begin_snap_id)
    }) else {
        return false;
    };
    if !timestamp(&aa.snap_info.begin_snap_time)
        .zip(timestamp(&ba.snap_info.end_snap_time))
        .is_some_and(|(a, b)| a >= b)
    {
        return false;
    }
    let (bd, ad) = match (&b.useful_work, &a.useful_work) {
        (Some(b), Some(a)) if b.name == a.name && b.unit == a.unit => (b.completed, a.completed),
        (None, None) if b.executions == a.executions => (b.executions as f64, a.executions as f64),
        _ => return false,
    };
    [(b.buffer_gets, a.buffer_gets), (b.elapsed_s, a.elapsed_s)]
        .iter()
        .all(|(bv, av)| {
            bv.zip(*av)
                .and_then(|(b, a)| growth(ratio(b, bd), ratio(a, ad)))
                .is_some_and(|x| x <= -p.minimum_cost_growth_pct)
        })
}

/// Shared bounded view for classic functions, local tools and MCP.
pub fn query(report: Option<&AccessPathReport>, args: &Value) -> Value {
    let Some(report) = report else {
        return json!({"status":"unavailable","reason":"access path diagnostics were not computed"});
    };
    let mut v = json!(report);
    let limit = args["limit"].as_u64().unwrap_or(20).clamp(1, 100) as usize;
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    for key in ["sql_costs", "segment_candidates"] {
        let rows = v[key]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| {
                args["sql_id"]
                    .as_str()
                    .is_none_or(|id| r["scope"]["sql_id"] == id)
            })
            .cloned()
            .collect::<Vec<_>>();
        v[format!("{key}_total")] = json!(rows.len());
        v[key] = json!(rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>());
    }
    let evidence_offset = args["evidence_offset"].as_u64().unwrap_or(0) as usize;
    let evidence_limit = args["evidence_limit"].as_u64().unwrap_or(20).clamp(1, 100) as usize;
    if let Some(rows) = v["segment_candidates"].as_array_mut() {
        for row in rows {
            if let Some(refs) = row["evidence_refs"].as_array() {
                let total = refs.len();
                let page: Vec<_> = refs
                    .iter()
                    .skip(evidence_offset)
                    .take(evidence_limit)
                    .cloned()
                    .collect();
                row["evidence_refs_total"] = json!(total);
                row["evidence_refs"] = json!(page);
            }
        }
    }
    v["evidence_offset"] = json!(evidence_offset);
    v["evidence_limit"] = json!(evidence_limit);
    v["offset"] = json!(offset);
    v["limit"] = json!(limit);
    // Window boundaries remain accessible without returning thousands of raw snapshots.
    for key in ["baseline", "recent"] {
        if let Some(rows) = v["windows"][key].as_array() {
            let compact = json!({"count":rows.len(),"first":rows.first(),"last":rows.last()});
            v["windows"][key] = compact;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awr::{DBInstance, InstanceStats, SnapInfo};
    pub fn fixture() -> AWRSCollection {
        let awrs = (0..8)
            .map(|i| {
                let recent = i >= 6;
                let mut a = AWR::default();
                a.file_name = format!("fixture_{i}");
                a.snap_info = SnapInfo {
                    begin_snap_id: i + 1,
                    end_snap_id: i + 2,
                    begin_snap_time: format!("2026-09-13T10:{:02}:00Z", i),
                    end_snap_time: format!("2026-09-13T10:{:02}:00Z", i + 1),
                };
                let scope = Scope {
                    dbid: 1,
                    inst_id: 1,
                    con_id: Some(3),
                    sql_id: "testsql".into(),
                    child_number: Some(0),
                    plan_hash_value: Some(10),
                    object_id: Some(20),
                    data_object_id: Some(30),
                };
                a.instance_stats = vec![
                    InstanceStats {
                        statname: SCAN.into(),
                        total: if recent { 6000 } else { 1000 },
                    },
                    InstanceStats {
                        statname: CONTINUED.into(),
                        total: if recent { 2 } else { 1 },
                    },
                ];
                a.access_path_observations = vec![SqlObservation {
                    scope,
                    evidence_ref: format!("measurement_{i}"),
                    executions: 10,
                    buffer_gets: Some(if recent { 6000.0 } else { 1000.0 }),
                    elapsed_s: Some(if recent { 6.0 } else { 1.0 }),
                    scan_blocks: Some(if recent { 6000 } else { 1000 }),
                    continued_rows: Some(0),
                    ..Default::default()
                }];
                a
            })
            .collect();
        AWRSCollection {
            db_instance_information: DBInstance {
                db_id: 1,
                instance_num: 1,
                ..Default::default()
            },
            initialization_parameters: Default::default(),
            awrs,
            sql_text: Default::default(),
            nmon: None,
        }
    }
    fn report(c: &AWRSCollection) -> AccessPathReport {
        build(c, &(0, u64::MAX), true, Policy::default())
    }
    fn candidate(r: &AccessPathReport, mechanism: &str) -> Value {
        r.segment_candidates
            .iter()
            .find(|v| v["mechanism"] == mechanism)
            .unwrap()
            .clone()
    }
    #[test]
    fn sql_cost_increase_and_scoped_activity_only_reach_candidate() {
        let r = report(&fixture());
        assert_eq!(
            candidate(&r, "scan_amplification")["evidence_level"],
            "segment_candidate"
        );
        assert_eq!(
            candidate(&r, "row_continuation")["evidence_level"],
            "context_only"
        );
        assert_eq!(r.coverage["host_cpu"]["status"], "unavailable");
        assert_eq!(r.instance_signals[2]["evidence_level"], "context_only");
    }
    #[test]
    fn more_executions_alone_are_not_cost_inflation() {
        let mut c = fixture();
        for a in &mut c.awrs[6..] {
            a.access_path_observations[0].executions = 60;
        }
        let r = report(&c);
        assert_eq!(
            candidate(&r, "scan_amplification")["evidence_level"],
            "context_only"
        );
    }
    #[test]
    fn zero_executions_and_missing_observations_do_not_become_cheap_work() {
        let mut c = fixture();
        c.awrs[7].access_path_observations[0].executions = 0;
        let r = report(&c);
        assert_eq!(r.coverage["rejected_attributed_observations"], 1);
        assert!(!r.sql_costs[0]["costs"]["sufficient_observations"]
            .as_bool()
            .unwrap());
    }
    #[test]
    fn workload_growth_and_inconsistent_work_units_block_promotion() {
        let mut c = fixture();
        for (i, a) in c.awrs.iter_mut().enumerate() {
            a.access_path_observations[0].useful_work = Some(UsefulWork {
                name: "orders completed".into(),
                unit: "orders".into(),
                completed: if i >= 6 { 600.0 } else { 100.0 },
            });
        }
        assert_eq!(
            candidate(&report(&c), "scan_amplification")["evidence_level"],
            "context_only"
        );
        c.awrs[7].access_path_observations[0]
            .useful_work
            .as_mut()
            .unwrap()
            .unit = "batches".into();
        assert_eq!(
            report(&c).sql_costs[0]["costs"]["useful_work_comparable"],
            false
        );
    }
    #[test]
    fn different_children_plans_objects_and_containers_are_not_merged() {
        for field in 0..4 {
            let mut c = fixture();
            for a in &mut c.awrs[6..] {
                let s = &mut a.access_path_observations[0].scope;
                match field {
                    0 => s.child_number = Some(1),
                    1 => s.plan_hash_value = Some(11),
                    2 => s.data_object_id = Some(31),
                    _ => s.con_id = Some(4),
                };
            }
            assert!(report(&c)
                .segment_candidates
                .iter()
                .all(|v| v["evidence_level"] == "context_only"));
        }
    }
    #[test]
    fn fs4_is_not_an_empty_block_confirmation_and_stale_structure_is_rejected() {
        let mut c = fixture();
        let a = &mut c.awrs[7];
        a.access_path_observations[0].structure = Some(StructureEvidence {
            evidence_ref: "space evidence".into(),
            method: "DBMS_SPACE".into(),
            observed_at: a.snap_info.begin_snap_time.clone(),
            blocks_below_hwm: Some(100),
            fs4_blocks: Some(80),
            ..Default::default()
        });
        assert_eq!(
            candidate(&report(&c), "scan_amplification")["evidence_level"],
            "segment_candidate"
        );
        c.awrs[7].access_path_observations[0]
            .structure
            .as_mut()
            .unwrap()
            .verified_empty_blocks_below_hwm = Some(60);
        assert_eq!(
            candidate(&report(&c), "scan_amplification")["evidence_level"],
            "structure_confirmed"
        );
        c.awrs[7].access_path_observations[0]
            .structure
            .as_mut()
            .unwrap()
            .observed_at = "2020-01-01T00:00:00Z".into();
        assert_eq!(
            candidate(&report(&c), "scan_amplification")["evidence_level"],
            "segment_candidate"
        );
    }
    #[test]
    fn instance_chaining_cannot_override_zero_sql_session_chaining() {
        let mut c = fixture();
        for a in &mut c.awrs[6..] {
            a.instance_stats[1].total = 100000;
        }
        let r = report(&c);
        assert_eq!(r.instance_signals[2]["material_activity"], true);
        assert_eq!(
            candidate(&r, "row_continuation")["evidence_level"],
            "context_only"
        );
    }
    #[test]
    fn duplicate_attributed_rows_and_wrong_instance_are_rejected() {
        let mut c = fixture();
        let duplicate = c.awrs[7].access_path_observations[0].clone();
        c.awrs[6].access_path_observations.push(duplicate);
        c.awrs[7].access_path_observations[0].scope.inst_id = 2;
        assert_eq!(report(&c).coverage["rejected_attributed_observations"], 3);
    }
    #[test]
    fn intervention_requires_linked_measured_reduction_and_explicit_controls() {
        let mut c = fixture();
        let mut after = c.awrs[7].clone();
        after.snap_info = SnapInfo {
            begin_snap_id: 9,
            end_snap_id: 10,
            begin_snap_time: "2026-09-13T10:08:00Z".into(),
            end_snap_time: "2026-09-13T10:09:00Z".into(),
        };
        after.access_path_observations[0].scope.data_object_id = Some(31);
        after.access_path_observations[0].buffer_gets = Some(1000.0);
        after.access_path_observations[0].elapsed_s = Some(1.0);
        let ab = InterventionEvidence {
            evidence_ref: "controlled AB".into(),
            controls_evidence_ref: "plan data cache concurrency work checks".into(),
            before_begin_snap_id: 7,
            after_begin_snap_id: 9,
            after_scope: after.access_path_observations[0].scope.clone(),
            same_plan: true,
            same_logical_data: true,
            comparable_cache_and_concurrency: true,
            equivalent_work: true,
        };
        c.awrs[7].access_path_observations[0].structure = Some(StructureEvidence {
            evidence_ref: "block inspection".into(),
            observed_at: "2026-09-13T10:07:30Z".into(),
            method: "block_inspection".into(),
            blocks_below_hwm: Some(100),
            verified_empty_blocks_below_hwm: Some(60),
            ..Default::default()
        });
        c.awrs[7].access_path_observations[0].intervention = Some(ab);
        c.awrs.push(after);
        assert_eq!(
            candidate(&report(&c), "scan_amplification")["evidence_level"],
            "intervention_verified"
        );
        c.awrs[7].access_path_observations[0]
            .intervention
            .as_mut()
            .unwrap()
            .equivalent_work = false;
        assert_eq!(
            candidate(&report(&c), "scan_amplification")["evidence_level"],
            "structure_confirmed"
        );
        c.awrs[7].access_path_observations[0]
            .intervention
            .as_mut()
            .unwrap()
            .equivalent_work = true;
        c.awrs[8].access_path_observations[0].elapsed_s = Some(7.0);
        assert_eq!(
            candidate(&report(&c), "scan_amplification")["evidence_level"],
            "structure_confirmed"
        );
    }

    #[test]
    fn pagination_preserves_limits_and_exact_sql_filter() {
        let r = report(&fixture());
        let v = query(Some(&r), &json!({"sql_id":"testsql","limit":1,"offset":1}));
        assert_eq!(v["segment_candidates_total"], 2);
        assert_eq!(v["segment_candidates"].as_array().unwrap().len(), 1);
        assert_eq!(
            query(Some(&r), &json!({"sql_id":"absent"}))["segment_candidates_total"],
            0
        );
    }
}
