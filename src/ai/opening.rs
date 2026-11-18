use crate::awr::AWRSCollection;
use crate::measurements::available;
use crate::reasonings::{LoadProfileAnomalies, ReportForAI};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const HYPOTHESIS_FIELDS: [&str; 5] = [
    "statement",
    "gradient_signal",
    "workload_profile",
    "comparison_window",
    "test",
];

fn top_observed_sql<'a>(rows: impl Iterator<Item = (&'a str, f64)>) -> Vec<Value> {
    let mut totals = BTreeMap::<&str, (f64, usize)>::new();
    for (id, seconds) in rows {
        if seconds.is_finite() && seconds >= 0.0 {
            let entry = totals.entry(id).or_default();
            entry.0 += seconds;
            entry.1 += 1;
        }
    }
    let mut totals: Vec<_> = totals.into_iter().collect();
    totals.sort_by(|(left_id, (left_s, _)), (right_id, (right_s, _))| {
        right_s
            .total_cmp(left_s)
            .then_with(|| left_id.cmp(right_id))
    });
    totals.into_iter().take(4).map(|(id, (seconds, count))|
        json!({"sql_id": id, "observed_seconds": seconds, "observed_rows": count})
    ).collect()
}

pub(super) fn workload(collection: &AWRSCollection) -> Value {
    let summary = crate::ai_tools::dispatch_tool_call_value(
        "get_database_load_summary",
        &json!({"top_n": 4}),
        collection,
        "",
    );
    // Rank all retained SQL rows, including steady work absent from fitted predictors.
    let sql_elapsed = collection
        .awrs
        .iter()
        .filter(|s| available(s, "sql_elapsed_time", !s.sql_elapsed_time.is_empty()))
        .flat_map(|s| {
            s.sql_elapsed_time
                .iter()
                .map(|row| (row.sql_id.as_str(), row.elapsed_time_s))
        });
    let sql_cpu = collection
        .awrs
        .iter()
        .filter(|s| available(s, "sql_cpu_time", !s.sql_cpu_time.is_empty()))
        .flat_map(|s| {
            s.sql_cpu_time
                .iter()
                .map(|(id, row)| (id.as_str(), row.cpu_time_s))
        });
    let first = collection
        .awrs
        .iter()
        .min_by_key(|s| s.snap_info.begin_snap_id);
    let last = collection
        .awrs
        .iter()
        .max_by_key(|s| s.snap_info.end_snap_id);
    json!({
        "instance_number": collection.db_instance_information.instance_num,
        "snapshot_count": collection.awrs.len(),
        "first_snapshot": first.map(|s| json!({"snap_id":s.snap_info.begin_snap_id,"begin_time":s.snap_info.begin_snap_time})),
        "last_snapshot": last.map(|s| json!({"end_snap_id":s.snap_info.end_snap_id,"end_time":s.snap_info.end_snap_time})),
        "total_db_time_s": (!collection.awrs.is_empty()).then_some(&summary["total_db_time_s"]),
        "total_db_cpu_s": (!collection.awrs.is_empty()).then_some(&summary["total_db_cpu_s"]),
        "top_foreground_waits_by_observed_seconds": summary["top_foreground_waits_by_total_time"],
        "top_sql_by_observed_elapsed_seconds": top_observed_sql(sql_elapsed),
        "top_sql_by_observed_cpu_seconds": top_observed_sql(sql_cpu)
    })
}

/// Keep original measurements and selected windows together throughout both AI sessions.
pub(super) fn seed(report: &ReportForAI, collection: &AWRSCollection) -> Value {
    let mut seed = crate::local_agent::build_case_seed(report);
    seed["absolute_workload"] = workload(collection);
    let mut anomalies: Vec<_> = report.load_profile_anomalies.iter().collect();
    anomalies.sort_by(|a, b| LoadProfileAnomalies::compare_severity(a, b));
    anomalies.truncate(3);
    let mut clusters: Vec<_> = report.anomaly_clusters.iter().collect();
    clusters.sort_by_key(|c| {
        (
            std::cmp::Reverse(c.number_of_anomalies),
            &c.begin_snap_date,
            c.begin_snap_id,
        )
    });
    let windows: Vec<_> = clusters
        .into_iter()
        .take(3)
        .map(|c| {
            let mut members: Vec<_> = c.anomalies_detected.iter().collect();
            members.sort_by_key(|m| (&m.area_of_anomaly, &m.statistic_name));
            members.truncate(3);
            json!({"begin_snap_id":c.begin_snap_id,"begin_snap_date":c.begin_snap_date,
               "number_of_anomalies":c.number_of_anomalies,"sample_members":members})
        })
        .collect();
    seed["anomaly_windows"] = json!({
        "load_profile_anomalies_total": report.load_profile_anomalies.len(), "highest_mad": anomalies,
        "anomaly_clusters_total": report.anomaly_clusters.len(), "largest_recorded_clusters": windows
    });
    seed
}

pub(super) fn identify(mut opening: Value) -> Result<Value, String> {
    let gap = opening["coverage_gap"]
        .as_str()
        .ok_or("Expected coverage_gap text")?
        .trim();
    let has_gap = !gap.is_empty();
    let rows = opening["hypotheses"]
        .as_array_mut()
        .ok_or("Expected a hypotheses array")?;
    if rows.len() > 3 || (rows.len() < 2 && !has_gap) {
        return Err("Provide two or three opening hypotheses, or explain the missing comparison in coverage_gap".into());
    }
    let mut statements = std::collections::BTreeSet::new();
    for (i, row) in rows.iter_mut().enumerate() {
        for field in HYPOTHESIS_FIELDS {
            if row[field].as_str().is_none_or(|s| s.trim().is_empty()) {
                return Err(format!("Opening hypothesis {} needs {field}", i + 1));
            }
        }
        let statement = row["statement"]
            .as_str()
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if !statements.insert(statement) {
            return Err("Provide distinct opening hypotheses".into());
        }
        row["id"] = json!(format!("H{}", i + 1));
    }
    Ok(opening)
}

pub(super) fn schema() -> Value {
    let properties = HYPOTHESIS_FIELDS
        .iter()
        .map(|field| (field.to_string(), json!({"type":"string"})))
        .collect::<serde_json::Map<_, _>>();
    json!({
        "type":"object",
        "additionalProperties":false,
        "properties":{
            "hypotheses":{
                "type":"array", "maxItems":3,
                "items":{
                    "type":"object", "additionalProperties":false,
                    "properties":properties, "required":HYPOTHESIS_FIELDS
                }
            },
            "coverage_gap":{"type":"string"}
        },
        "required":["hypotheses","coverage_gap"]
    })
}

pub(super) fn instructions(language: &str) -> String {
    format!("Write in {language}. Begin an Oracle investigation after establishing its scope. Compare the measured workload with DB Time and DB CPU gradients and locate the supplied MAD anomalies and clusters by timestamp and snapshot. Use comparable windows and distinguish empty detector output from measured baseline activity. Consider expensive steady work, typical changes and peak signals. Give two or three distinct, falsifiable explanations. For each name the actual gradient observation and the absolute measurement with units, the comparison window and a check that could disprove it. State the source boundary when a timeline or comparison is missing. If fewer hypotheses can be formed, explain why in coverage_gap. Otherwise leave coverage_gap empty. Use only the supplied evidence. Return one JSON object with a hypotheses array and a coverage_gap string. Each hypothesis has exactly five text fields: statement, gradient_signal, workload_profile, comparison_window, test. Keep each statement short for use as a report heading.")
}

pub(super) fn evidence(seed: &Value) -> Value {
    let mut gradients = BTreeMap::new();
    for (name, section) in seed["gradients"].as_object().into_iter().flatten() {
        if section.is_null() || !(name.starts_with("db_time_") || name.starts_with("db_cpu_")) {
            continue;
        }
        let consensus = &section["cross_model_classifications"];
        let rows = if consensus.as_array().is_some_and(|rows| !rows.is_empty()) {
            consensus.clone()
        } else {
            json!({
                "ridge": section["ridge_top"], "elastic_net": section["elastic_net_top"],
                "huber": section["huber_top"], "quantile95": section["quantile95_top"]
            })
        };
        gradients.insert(name, rows);
    }
    let peaks: Vec<_> = seed["performance_peaks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            json!({"snap_id": p["snap_id"], "begin_time": p["report_date"],
            "db_time_s": p["db_time_value"], "db_cpu_s": p["db_cpu_value"]})
        })
        .collect();
    json!({
        "absolute_workload": seed["absolute_workload"], "gradients": gradients,
        "db_time_degradation": seed["db_time_degradation"], "performance_peaks": peaks,
        "anomaly_windows": seed["anomaly_windows"]
    })
}

pub(super) fn statements(opening: &Value) -> Vec<&str> {
    opening["hypotheses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["statement"].as_str())
        .collect()
}
