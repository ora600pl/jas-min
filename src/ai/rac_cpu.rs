//! Portfolio evidence for a synchronized, counterfactual RAC foreground CPU scenario.
use super::{
    function_definition, tool_error, AWRSCollection, AnalysisRuntime, Map, Value, MAX_MCP_PROJECTS,
};
use crate::awr::AWR;
use crate::measurements::{available, time_model_seconds, timestamp, DbLoadMetric};
use chrono::{Duration, NaiveDateTime};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

const TOOL: &str = "get_rac_cpu_scenario";
const ALIGNMENT_SECONDS: i64 = 60;
type ScenarioResult<T> = Result<T, Value>;

pub(super) fn tools() -> Vec<Value> {
    vec![function_definition(TOOL,
        "Measure foreground DB CPU across aligned AWR intervals on distinct instances of one RAC database. Model removal of captured monitoring SELECT CPU and a percentage reduction in a disjoint set of captured SELECT statements, keeping all other foreground CPU and wall exposure unchanged. A missing SQL Ordered by CPU Time row means unknown activity and receives no savings credit. Returns instance and cluster peaks, actual wall exposure, measured components and modeled savings for the full capture.",
        json!({"type":"object","additionalProperties":false,"properties":{
            "project_ids":{"type":"array","items":{"type":"string"},"minItems":2,"maxItems":MAX_MCP_PROJECTS,"uniqueItems":true},
            "monitor_module_prefix":{"type":"string","minLength":1,"maxLength":128},
            "selected_sql_ids":{"type":"array","items":{"type":"string","pattern":"^[0-9A-Za-z]{13}$"},"minItems":1,"maxItems":256,"uniqueItems":true},
            "sql_reduction_pct":{"type":"number","minimum":0,"maximum":100}
        },"required":["project_ids","monitor_module_prefix","selected_sql_ids","sql_reduction_pct"]})
    )]
}

fn invalid(message: impl Into<String>) -> Value {
    tool_error("INVALID_RAC_CPU_SCENARIO", message)
}

fn gap(message: impl Into<String>) -> Value {
    tool_error("RAC_CPU_EVIDENCE_GAP", message)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    project_ids: Vec<String>,
    monitor_module_prefix: String,
    selected_sql_ids: Vec<String>,
    sql_reduction_pct: f64,
}

impl Request {
    fn parse(args: &Map<String, Value>) -> ScenarioResult<Self> {
        let mut args = args.clone();
        args.remove("analysis_id");
        let mut request: Self =
            serde_json::from_value(Value::Object(args)).map_err(|e| invalid(e.to_string()))?;
        request.project_ids.sort();
        request.monitor_module_prefix = request.monitor_module_prefix.trim().to_ascii_uppercase();
        if request.monitor_module_prefix.is_empty() || request.monitor_module_prefix.len() > 128 {
            return Err(invalid(
                "Expected a literal monitoring module prefix of 1 to 128 bytes.",
            ));
        }
        if !(0.0..=100.0).contains(&request.sql_reduction_pct) {
            return Err(invalid("sql_reduction_pct must be between 0 and 100."));
        }
        if !(1..=256).contains(&request.selected_sql_ids.len()) {
            return Err(invalid("Select between 1 and 256 distinct SQL IDs."));
        }
        for id in &mut request.selected_sql_ids {
            if id.len() != 13 || !id.bytes().all(|c| c.is_ascii_alphanumeric()) {
                return Err(invalid(
                    "Each selected SQL ID must contain 13 ASCII letters or digits.",
                ));
            }
            id.make_ascii_lowercase();
        }
        request.selected_sql_ids.sort();
        if request
            .selected_sql_ids
            .windows(2)
            .any(|ids| ids[0] == ids[1])
        {
            return Err(invalid("The selected SQL basket contains a duplicate ID."));
        }
        Ok(request)
    }
}

fn validate_sources(sources: &[(&str, &AWRSCollection)]) -> ScenarioResult<()> {
    if !(2..=MAX_MCP_PROJECTS).contains(&sources.len()) {
        return Err(invalid("Select at least two distinct RAC instances."));
    }
    let db_id = sources[0].1.db_instance_information.db_id;
    let mut projects = BTreeSet::new();
    let mut instances = BTreeSet::new();
    for &(id, collection) in sources {
        let db = &collection.db_instance_information;
        if db_id == 0 || db_id != db.db_id || !db.rac.trim().eq_ignore_ascii_case("YES") {
            return Err(invalid(
                "Each source must identify a RAC instance of the same known database ID.",
            ));
        }
        if db.instance_num == 0 || !projects.insert(id) || !instances.insert(db.instance_num) {
            return Err(invalid(
                "Each project must represent a distinct, known instance number.",
            ));
        }
    }
    Ok(())
}

impl AnalysisRuntime {
    pub(super) fn validate_rac_projects(&self, ids: &[String]) -> ScenarioResult<()> {
        let projects = ids
            .iter()
            .map(|id| self.project(id))
            .collect::<Result<Vec<_>, _>>()?;
        let sources = projects
            .iter()
            .map(|p| (p.project_id.as_str(), p.collection.as_ref()))
            .collect::<Vec<_>>();
        validate_sources(&sources)
    }

    pub(super) fn rac_cpu_scenario(&self, args: &Map<String, Value>) -> ScenarioResult<Value> {
        let analysis_id = Self::analysis_id(args)?;
        let session = self.session(analysis_id)?;
        let request = Request::parse(args)?;
        let projects = request
            .project_ids
            .iter()
            .map(|id| self.selected_project(analysis_id, id).map(|(_, p)| p))
            .collect::<Result<Vec<_>, _>>()?;
        let sources = projects
            .iter()
            .map(|p| (p.project_id.as_str(), p.collection.as_ref()))
            .collect::<Vec<_>>();
        let result = calculate(&request, &sources)?;
        self.store_comparison_evidence(analysis_id, &session, TOOL, json!(request), result)
    }
}

fn clock<S: Serializer>(t: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&t.format("%Y-%m-%d %H:%M:%S%.f").to_string())
}

#[derive(Clone, Copy, Serialize)]
struct Window {
    #[serde(serialize_with = "clock")]
    begin: NaiveDateTime,
    #[serde(serialize_with = "clock")]
    end: NaiveDateTime,
}

#[derive(Serialize)]
struct Peak {
    #[serde(flatten)]
    window: Window,
    active_cpu: f64,
}

#[derive(Default, Serialize)]
struct Totals {
    observed_peak: Option<Peak>,
    scenario_peak: Option<Peak>,
    saved_cpu_hours: f64,
}

impl Totals {
    fn record(&mut self, window: Window, observed: f64, scenario: f64, saved_s: f64) {
        // Sorted intervals give ties to the first measured window.
        if self
            .observed_peak
            .as_ref()
            .is_none_or(|p| observed > p.active_cpu)
        {
            self.observed_peak = Some(Peak {
                window,
                active_cpu: observed,
            });
        }
        if self
            .scenario_peak
            .as_ref()
            .is_none_or(|p| scenario > p.active_cpu)
        {
            self.scenario_peak = Some(Peak {
                window,
                active_cpu: scenario,
            });
        }
        self.saved_cpu_hours += saved_s / 3600.0;
    }
}

#[derive(Serialize)]
struct InstanceWindow<'a> {
    project_id: &'a str,
    instance_number: u8,
    #[serde(flatten)]
    window: Window,
    wall_seconds: f64,
    db_cpu_seconds: f64,
    monitor_cpu_seconds: f64,
    selected_sql_cpu_seconds: f64,
    monitor_top_rows: usize,
    selected_top_rows: usize,
    observed_active_cpu: f64,
    scenario_active_cpu: f64,
}

#[derive(Serialize)]
struct InstanceTotal<'a> {
    project_id: &'a str,
    instance_number: u8,
    #[serde(flatten)]
    totals: Totals,
}

fn is_query(sql: &str) -> bool {
    let mut sql = sql.trim_start();
    loop {
        if let Some(comment) = sql.strip_prefix("--") {
            let Some((_, rest)) = comment.split_once('\n') else {
                return false;
            };
            sql = rest.trim_start();
        } else if let Some(comment) = sql.strip_prefix("/*") {
            let Some((_, rest)) = comment.split_once("*/") else {
                return false;
            };
            sql = rest.trim_start();
        } else {
            let first = sql
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
                .unwrap_or("");
            return first.eq_ignore_ascii_case("select") || first.eq_ignore_ascii_case("with");
        }
    }
}

struct InstanceSource<'a> {
    project_id: &'a str,
    collection: &'a AWRSCollection,
    snapshots: BTreeMap<(u64, u64), &'a AWR>,
    checked_sql: BTreeSet<String>,
    previous_end: Option<(u64, NaiveDateTime)>,
    totals: InstanceTotal<'a>,
}

impl<'a> InstanceSource<'a> {
    fn new(project_id: &'a str, collection: &'a AWRSCollection) -> ScenarioResult<Self> {
        let mut snapshots = BTreeMap::new();
        for awr in &collection.awrs {
            let snap = &awr.snap_info;
            let pair = (snap.begin_snap_id, snap.end_snap_id);
            if pair.1 <= pair.0 || snapshots.insert(pair, awr).is_some() {
                return Err(gap(format!(
                    "{project_id}: duplicate or invalid AWR snapshot pair {pair:?}."
                )));
            }
        }
        if snapshots.is_empty() {
            return Err(gap(format!("{project_id}: no AWR intervals.")));
        }
        let totals = InstanceTotal {
            project_id,
            instance_number: collection.db_instance_information.instance_num,
            totals: Totals::default(),
        };
        Ok(Self {
            project_id,
            collection,
            snapshots,
            totals,
            checked_sql: BTreeSet::new(),
            previous_end: None,
        })
    }

    fn measure(
        &mut self,
        pair: (u64, u64),
        request: &Request,
    ) -> ScenarioResult<InstanceWindow<'a>> {
        let awr = self.snapshots[&pair];
        let problem = |why: &str| {
            gap(format!(
                "{} at snapshots {}–{}: {why}",
                self.project_id, pair.0, pair.1
            ))
        };
        let snap = &awr.snap_info;
        let begin =
            timestamp(&snap.begin_snap_time).ok_or_else(|| problem("invalid begin timestamp"))?;
        let end = timestamp(&snap.end_snap_time).ok_or_else(|| problem("invalid end timestamp"))?;
        let wall_seconds = (end - begin)
            .num_microseconds()
            .filter(|&us| us > 0)
            .ok_or_else(|| problem("invalid wall exposure"))? as f64
            / 1e6;
        if self
            .previous_end
            .is_some_and(|previous| previous != (pair.0, begin))
        {
            return Err(problem("discontinuous snapshot coverage"));
        }
        self.previous_end = Some((pair.1, end));
        let db_cpu_seconds = time_model_seconds(awr, DbLoadMetric::DbCpu)
            .ok_or_else(|| problem("missing Time Model DB CPU seconds"))?;
        if !available(awr, "sql_cpu_time", !awr.sql_cpu_time.is_empty()) {
            return Err(problem("missing TOP SQL by CPU measurements"));
        }
        let mut row = InstanceWindow {
            project_id: self.project_id,
            instance_number: self.totals.instance_number,
            window: Window { begin, end },
            wall_seconds,
            db_cpu_seconds,
            monitor_cpu_seconds: 0.0,
            selected_sql_cpu_seconds: 0.0,
            monitor_top_rows: 0,
            selected_top_rows: 0,
            observed_active_cpu: db_cpu_seconds / wall_seconds,
            scenario_active_cpu: 0.0,
        };
        // Floating-point sums must replay identically after the hash map is reloaded.
        let ordered: BTreeMap<_, _> = awr.sql_cpu_time.iter().collect();
        for (id, sql) in ordered {
            let monitor = sql
                .sql_module
                .to_ascii_uppercase()
                .starts_with(&request.monitor_module_prefix);
            let selected = request.selected_sql_ids.binary_search(id).is_ok();
            if !id.eq_ignore_ascii_case(&sql.sql_id) || id != &id.to_ascii_lowercase() {
                return Err(problem("inconsistent SQL ID in TOP CPU table"));
            }
            if !monitor && !selected {
                continue;
            }
            if monitor && selected {
                return Err(problem("selected SQL overlaps the monitoring group"));
            }
            if !sql.cpu_time_s.is_finite() || sql.cpu_time_s < 0.0 {
                return Err(problem("invalid SQL CPU seconds"));
            }
            if self.checked_sql.insert(id.clone())
                && !self
                    .collection
                    .sql_text
                    .get(id)
                    .is_some_and(|s| is_query(s))
            {
                return Err(problem("credited SQL must have a captured SELECT or WITH text to exclude procedural parents"));
            }
            if monitor {
                row.monitor_cpu_seconds += sql.cpu_time_s;
                row.monitor_top_rows += 1;
            } else {
                row.selected_sql_cpu_seconds += sql.cpu_time_s;
                row.selected_top_rows += 1;
            }
        }
        if row.monitor_cpu_seconds + row.selected_sql_cpu_seconds > db_cpu_seconds {
            return Err(problem(
                "captured scenario components exceed measured foreground DB CPU",
            ));
        }
        let saved_s = row.monitor_cpu_seconds
            + row.selected_sql_cpu_seconds * (request.sql_reduction_pct / 100.0);
        row.scenario_active_cpu = (db_cpu_seconds - saved_s) / wall_seconds;
        self.totals.totals.record(
            row.window,
            row.observed_active_cpu,
            row.scenario_active_cpu,
            saved_s,
        );
        Ok(row)
    }
}

fn aligned_window(rows: &[InstanceWindow<'_>]) -> Option<Window> {
    let min_begin = rows.iter().map(|x| x.window.begin).min()?;
    let max_begin = rows.iter().map(|x| x.window.begin).max()?;
    let min_end = rows.iter().map(|x| x.window.end).min()?;
    let max_end = rows.iter().map(|x| x.window.end).max()?;
    let tolerance = Duration::seconds(ALIGNMENT_SECONDS);
    (max_begin - min_begin <= tolerance && max_end - min_end <= tolerance && max_begin < min_end)
        .then_some(Window {
            begin: min_begin,
            end: max_end,
        })
}

fn calculate(request: &Request, collections: &[(&str, &AWRSCollection)]) -> ScenarioResult<Value> {
    validate_sources(collections)?;
    let mut sources = collections
        .iter()
        .map(|&(id, c)| InstanceSource::new(id, c))
        .collect::<Result<Vec<_>, _>>()?;
    let pairs: Vec<_> = sources[0].snapshots.keys().copied().collect();
    if sources
        .iter()
        .skip(1)
        .any(|s| !s.snapshots.keys().eq(pairs.iter()))
    {
        return Err(gap("RAC instances have different snapshot coverage."));
    }
    let mut intervals = Vec::with_capacity(pairs.len());
    let mut totals = Totals::default();
    let mut monitor_s = 0.0;
    let mut selected_s = 0.0;
    let mut monitor_rows = 0;
    for pair in pairs {
        let rows = sources
            .iter_mut()
            .map(|source| source.measure(pair, request))
            .collect::<Result<Vec<_>, _>>()?;
        let window = aligned_window(&rows).ok_or_else(|| {
            gap(format!(
                "Snapshots {pair:?} are not aligned within {ALIGNMENT_SECONDS} seconds."
            ))
        })?;
        let observed: f64 = rows.iter().map(|r| r.observed_active_cpu).sum();
        let scenario: f64 = rows.iter().map(|r| r.scenario_active_cpu).sum();
        let monitor: f64 = rows.iter().map(|r| r.monitor_cpu_seconds).sum();
        let selected: f64 = rows.iter().map(|r| r.selected_sql_cpu_seconds).sum();
        monitor_rows += rows.iter().map(|r| r.monitor_top_rows).sum::<usize>();
        let saved_s = monitor + selected * (request.sql_reduction_pct / 100.0);
        totals.record(window, observed, scenario, saved_s);
        monitor_s += monitor;
        selected_s += selected;
        let mut interval = json!(window);
        interval["begin_snap_id"] = pair.0.into();
        interval["end_snap_id"] = pair.1.into();
        interval["observed_active_cpu"] = observed.into();
        interval["scenario_active_cpu"] = scenario.into();
        interval["saved_cpu_seconds"] = saved_s.into();
        interval["instances"] = json!(rows);
        intervals.push(interval);
    }
    if monitor_rows == 0 {
        return Err(gap("No TOP CPU row matched the monitoring module prefix."));
    }
    if request
        .selected_sql_ids
        .iter()
        .any(|id| !sources.iter().any(|s| s.checked_sql.contains(id)))
    {
        return Err(gap(
            "One or more selected SQL IDs have no measured TOP CPU row on any instance.",
        ));
    }
    let mut result = json!(totals);
    result["project_ids"] = json!(request.project_ids);
    result["paired_intervals"] = intervals.len().into();
    result["alignment_tolerance_seconds"] = ALIGNMENT_SECONDS.into();
    result["monitor_cpu_hours"] = (monitor_s / 3600.0).into();
    result["selected_sql_cpu_hours"] = (selected_s / 3600.0).into();
    result["assumptions"] = json!({"monitor_module_prefix":request.monitor_module_prefix,"selected_sql_ids":request.selected_sql_ids,"sql_reduction_pct":request.sql_reduction_pct,"monitor_reduction_pct":100});
    result["instances"] = json!(sources.into_iter().map(|s| s.totals).collect::<Vec<_>>());
    result["intervals"] = Value::Array(intervals);
    Ok(result)
}

#[cfg(test)]
#[path = "rac_cpu_tests.rs"]
mod tests;
