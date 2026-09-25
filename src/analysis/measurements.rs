//! Exposure and availability shared by classic, local-agent and MCP analysis.
use crate::awr::{AWRSCollection, AWR};
use chrono::{DateTime, NaiveDateTime};
use std::collections::{BTreeMap, BTreeSet};

pub const DB_LOAD_SOURCE_POLICY: &str = "DB Time/DB CPU rates prefer Time Model seconds / actual snapshot wall seconds; fall back per metric and snapshot to Load Profile per_second. Missing values are unknown, not zero. Raw input is preserved.";

#[derive(Clone, Copy)]
pub enum DbLoadMetric {
    DbTime,
    DbCpu,
}

impl DbLoadMetric {
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim().trim_end_matches(':').trim();
        let name = name.strip_suffix("(s)").unwrap_or(name).trim();
        if name.eq_ignore_ascii_case("DB time") {
            Some(Self::DbTime)
        } else if name.eq_ignore_ascii_case("DB CPU") {
            Some(Self::DbCpu)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::DbTime => "DB time",
            Self::DbCpu => "DB CPU",
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct DbLoadRate {
    pub per_second: f64,
    pub source: &'static str,
}

fn time_model_seconds(a: &AWR, metric: DbLoadMetric) -> Option<f64> {
    if !available(a, "time_model_stats", !a.time_model_stats.is_empty()) {
        return None;
    }
    a.time_model_stats
        .iter()
        .find(|row| row.stat_name.trim().eq_ignore_ascii_case(metric.name()))
        .map(|row| row.time_s)
        .filter(|value| value.is_finite() && *value >= 0.0)
}

/// Never overwrite Load Profile: retain the source values for audit and comparison.
/// A measured zero is valid. Availability masks and invalid exposure trigger fallback.
pub fn db_load_rate(a: &AWR, metric: DbLoadMetric) -> Option<DbLoadRate> {
    if let Some(value) = time_model_seconds(a, metric)
        .zip(seconds(a))
        .map(|(s, wall)| s / wall)
        .filter(|v| v.is_finite())
    {
        return Some(DbLoadRate {
            per_second: value,
            source: "time_model",
        });
    }
    if !available(a, "load_profile", !a.load_profile.is_empty()) {
        return None;
    }
    a.load_profile
        .iter()
        .find(|row| {
            DbLoadMetric::from_name(&row.stat_name).is_some_and(|m| m.name() == metric.name())
        })
        .map(|row| row.per_second)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| DbLoadRate {
            per_second: value,
            source: "load_profile",
        })
}

pub fn db_load_seconds(a: &AWR, metric: DbLoadMetric) -> Option<f64> {
    time_model_seconds(a, metric).or_else(|| {
        let value = db_load_rate(a, metric)?.per_second * seconds(a)?;
        value.is_finite().then_some(value)
    })
}

pub fn db_load_series(c: &AWRSCollection, range: &(u64, u64), metric: DbLoadMetric) -> Vec<f64> {
    selected(c, range)
        .iter()
        .map(|a| {
            db_load_rate(a, metric)
                .map(|m| m.per_second)
                .unwrap_or(f64::NAN)
        })
        .collect()
}

/// Other Load Profile metrics retain their collected rates and original names.
pub fn load_profile_rate(a: &AWR, name: &str) -> Option<f64> {
    if let Some(metric) = DbLoadMetric::from_name(name) {
        return db_load_rate(a, metric).map(|m| m.per_second);
    }
    if !available(a, "load_profile", !a.load_profile.is_empty()) {
        return None;
    }
    a.load_profile
        .iter()
        .find(|r| r.stat_name.eq_ignore_ascii_case(name))
        .map(|r| r.per_second)
        .filter(|v| v.is_finite())
}

#[derive(Default, Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct TargetSourceCounts {
    pub time_model: usize,
    pub load_profile: usize,
    pub unavailable: usize,
}

pub fn db_load_sources(
    c: &AWRSCollection,
    range: &(u64, u64),
) -> BTreeMap<String, TargetSourceCounts> {
    [DbLoadMetric::DbTime, DbLoadMetric::DbCpu]
        .into_iter()
        .map(|metric| {
            let mut counts = TargetSourceCounts::default();
            for a in selected(c, range) {
                match db_load_rate(a, metric).map(|m| m.source) {
                    Some("time_model") => counts.time_model += 1,
                    Some(_) => counts.load_profile += 1,
                    None => counts.unavailable += 1,
                }
            }
            (metric.name().into(), counts)
        })
        .collect()
}

pub fn timestamp(text: &str) -> Option<NaiveDateTime> {
    if let Ok(t) = DateTime::parse_from_rfc3339(text.trim()) {
        return Some(t.naive_utc());
    }
    [
        "%d-%b-%y %H:%M:%S",
        "%d-%b-%Y %H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
        "%d-%b-%y %H:%M",
    ]
    .iter()
    .find_map(|f| NaiveDateTime::parse_from_str(text.trim(), f).ok())
}

pub fn seconds(awr: &AWR) -> Option<f64> {
    let begin = timestamp(&awr.snap_info.begin_snap_time)?;
    let end = timestamp(&awr.snap_info.end_snap_time)?;
    let s = (end - begin).num_microseconds()? as f64 / 1e6;
    (s.is_finite() && s > 0.0).then_some(s)
}

pub fn selected<'a>(c: &'a AWRSCollection, range: &(u64, u64)) -> Vec<&'a AWR> {
    c.awrs
        .iter()
        .filter(|a| a.snap_info.begin_snap_id >= range.0 && a.snap_info.end_snap_id <= range.1)
        .collect()
}

pub fn available(a: &AWR, domain: &str, present: bool) -> bool {
    a.data_availability.get(domain).copied().unwrap_or(present) && present
}

pub fn host_cpu_available(a: &AWR) -> bool {
    let h = &a.host_cpu;
    // Legacy lab adapters supplied CPU count plus zero percentages. Those are unknown.
    let sum = h.pct_user + h.pct_system + h.pct_idle + h.pct_wio;
    available(
        a,
        "host_cpu",
        sum.is_finite()
            && (95.0..=105.0).contains(&sum)
            && [h.pct_user, h.pct_system, h.pct_idle, h.pct_wio]
                .iter()
                .all(|v| (0.0..=100.0).contains(v)),
    )
}

pub fn host_cpu_json(a: &AWR) -> serde_json::Value {
    if host_cpu_available(a) {
        serde_json::json!(a.host_cpu)
    } else {
        serde_json::Value::Null
    }
}

/// Complete observed series only: a missing statistic/window is never an observed zero.
/// Gauges retain their level; accumulated interval deltas are divided by wall seconds.
pub fn instance_rates(c: &AWRSCollection, range: &(u64, u64)) -> BTreeMap<String, Vec<f64>> {
    let awrs = selected(c, range);
    let names: BTreeSet<_> = awrs
        .iter()
        .flat_map(|a| a.instance_stats.iter().map(|s| s.statname.clone()))
        .collect();
    names
        .into_iter()
        .filter_map(|name| {
            let values: Option<Vec<f64>> = awrs
                .iter()
                .map(|a| {
                    if !available(a, "instance_stats", !a.instance_stats.is_empty()) {
                        return None;
                    }
                    let v = a.instance_stats.iter().find(|s| s.statname == name)?.total as f64;
                    if name.ends_with(" current") {
                        Some(v)
                    } else {
                        Some(v / seconds(a)?)
                    }
                })
                .collect();
            values.map(|v| (name, v))
        })
        .collect()
}

/// Convert dense legacy interval vectors to rates. Missing exposure excludes the series.
pub fn rates(
    c: &AWRSCollection,
    range: &(u64, u64),
    data: &BTreeMap<String, Vec<f64>>,
) -> BTreeMap<String, Vec<f64>> {
    let awrs = selected(c, range);
    let Some(exposure): Option<Vec<f64>> = awrs.iter().map(|a| seconds(a)).collect() else {
        return BTreeMap::new();
    };
    data.iter()
        .filter_map(|(name, series)| {
            if series.len() != exposure.len() || series.iter().any(|x| !x.is_finite() || *x < 0.0) {
                return None;
            }
            Some((
                name.clone(),
                series.iter().zip(&exposure).map(|(x, s)| x / s).collect(),
            ))
        })
        .collect()
}

/// A wholly uncollected domain in any window cannot be a measured regression zero.
/// Within observed TOP lists, omitted members retain the documented censored proxy.
pub fn domain_rates(
    c: &AWRSCollection,
    range: &(u64, u64),
    data: &BTreeMap<String, Vec<f64>>,
    domain: &str,
) -> BTreeMap<String, Vec<f64>> {
    if selected(c, range).iter().any(|a| {
        !available(
            a,
            domain,
            match domain {
                "foreground_wait_events" => !a.foreground_wait_events.is_empty(),
                "sql_elapsed_time" => !a.sql_elapsed_time.is_empty(),
                "sql_cpu_time" => !a.sql_cpu_time.is_empty(),
                "time_model_stats" => !a.time_model_stats.is_empty(),
                _ => true,
            },
        )
    }) {
        return BTreeMap::new();
    }
    rates(c, range, data)
}

pub fn statistic_unit(name: &str) -> &'static str {
    if name.ends_with(" current") {
        return "gauge";
    }
    match crate::staticdata::classify_stat_unit_group(name) {
        crate::staticdata::StatUnitGroup::Volume => "bytes/s",
        crate::staticdata::StatUnitGroup::Counter => "events/s",
        crate::staticdata::StatUnitGroup::Time => "native time units/s",
        _ => "native units/s",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target_window() -> AWR {
        let mut a = AWR::default();
        a.snap_info.begin_snap_time = "13-Sep-26 10:00:00".into();
        a.snap_info.end_snap_time = "13-Sep-26 10:01:00".into();
        for (name, seconds) in [("DB time", 13.32), ("DB CPU", 12.94)] {
            a.time_model_stats.push(crate::awr::TimeModelStats {
                stat_name: name.into(),
                time_s: seconds,
                ..Default::default()
            });
            let mut lp = crate::awr::LoadProfile::default();
            lp.stat_name = format!("{name}(s):");
            lp.per_second = 0.2;
            a.load_profile.push(lp);
        }
        a
    }

    #[test]
    fn time_model_target_preserves_precision_zero_and_original_input() {
        let mut a = target_window();
        let measured = db_load_rate(&a, DbLoadMetric::DbTime).unwrap();
        assert_eq!(measured.source, "time_model");
        assert!((measured.per_second - 0.222).abs() < 1e-15);
        assert_eq!(a.load_profile[0].per_second, 0.2);
        assert!((load_profile_rate(&a, "DB TIME(s)").unwrap() - 0.222).abs() < 1e-15);
        a.snap_info.end_snap_time = "13-Sep-26 10:00:30".into();
        assert!((db_load_rate(&a, DbLoadMetric::DbTime).unwrap().per_second - 0.444).abs() < 1e-15);
        a.time_model_stats[0].time_s = 0.0;
        assert_eq!(
            db_load_rate(&a, DbLoadMetric::DbTime).unwrap().per_second,
            0.0
        );
        assert!(db_load_rate(&a, DbLoadMetric::DbCpu).unwrap().per_second > 0.0);
        assert!(DbLoadMetric::from_name("background CPU time").is_none());
    }

    #[test]
    fn target_fallback_is_per_metric_and_respects_availability_and_exposure() {
        let mut a = target_window();
        a.time_model_stats.remove(0);
        assert_eq!(
            db_load_rate(&a, DbLoadMetric::DbTime).unwrap().source,
            "load_profile"
        );
        assert_eq!(
            db_load_rate(&a, DbLoadMetric::DbCpu).unwrap().source,
            "time_model"
        );
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            let mut invalid = target_window();
            invalid.time_model_stats[0].time_s = bad;
            assert_eq!(
                db_load_rate(&invalid, DbLoadMetric::DbTime)
                    .unwrap()
                    .per_second,
                0.2
            );
        }
        for end in ["invalid", "13-Sep-26 10:00:00", "13-Sep-26 09:59:59"] {
            let mut invalid = target_window();
            invalid.snap_info.end_snap_time = end.into();
            assert_eq!(
                db_load_rate(&invalid, DbLoadMetric::DbTime).unwrap().source,
                "load_profile"
            );
            invalid.load_profile.clear();
            assert!(db_load_rate(&invalid, DbLoadMetric::DbTime).is_none());
        }
        a.data_availability.insert("load_profile".into(), false);
        assert!(db_load_rate(&a, DbLoadMetric::DbTime).is_none());
        assert_eq!(
            db_load_rate(&a, DbLoadMetric::DbCpu).unwrap().source,
            "time_model"
        );
        a.data_availability.insert("time_model_stats".into(), false);
        assert!(db_load_rate(&a, DbLoadMetric::DbCpu).is_none());
        a.data_availability.insert("load_profile".into(), true);
        assert_eq!(
            db_load_rate(&a, DbLoadMetric::DbCpu).unwrap().source,
            "load_profile"
        );
        a.load_profile[1].per_second = -1.0;
        assert!(db_load_rate(&a, DbLoadMetric::DbCpu).is_none());
    }

    #[test]
    fn unknown_targets_keep_snapshot_alignment_and_are_not_gradient_zeroes() {
        let a = target_window();
        let mut b = a.clone();
        b.time_model_stats.clear();
        b.load_profile.clear();
        let mut c = a.clone();
        c.time_model_stats.clear();
        let collection = AWRSCollection {
            awrs: vec![a, b, c],
            db_instance_information: Default::default(),
            initialization_parameters: Default::default(),
            sql_text: Default::default(),
            nmon: None,
        };
        let y = db_load_series(&collection, &(0, u64::MAX), DbLoadMetric::DbTime);
        assert_eq!(y.len(), 3);
        assert!(y[1].is_nan());
        let counts = db_load_sources(&collection, &(0, u64::MAX));
        let counts = &counts["DB time"];
        assert_eq!(
            (counts.time_model, counts.load_profile, counts.unavailable),
            (1, 1, 1)
        );
        let fit = crate::gradient::build_db_time_gradient_section(
            &y,
            &BTreeMap::from([("counter".into(), vec![1.0, 2.0, 3.0])]),
            30.0,
            Some(0.1),
            0.2,
            5000,
            1e-6,
            "counter",
            10,
        );
        assert!(fit.unwrap_err().contains("finite"));
    }
    #[test]
    fn timestamp_formats_and_invalid_exposure() {
        for (b, e, s) in [
            ("13-Sep-26 10:00:00", "13-Sep-26 10:30:00", 1800.0),
            ("2026-09-13T10:00:00.100Z", "2026-09-13T10:00:00.350Z", 0.25),
        ] {
            let mut a = AWR::default();
            a.snap_info.begin_snap_time = b.into();
            a.snap_info.end_snap_time = e.into();
            assert_eq!(seconds(&a), Some(s));
            a.snap_info.end_snap_time = b.into();
            assert_eq!(seconds(&a), None);
        }
        assert!(timestamp("unknown").is_none());
    }
    #[test]
    fn placeholder_cpu_is_unknown_but_measured_idle_is_valid() {
        let mut a = AWR::default();
        a.host_cpu.cpus = 10;
        assert!(!host_cpu_available(&a));
        a.host_cpu.pct_idle = 100.0;
        assert!(host_cpu_available(&a));
        a.data_availability.insert("host_cpu".into(), false);
        assert_eq!(host_cpu_json(&a), serde_json::Value::Null);
    }
}

#[cfg(test)]
pub(crate) mod replay_tests {
    use super::*;
    use crate::awr::load_awrs_collection_from_json_str;

    pub(crate) fn native_target_collection() -> AWRSCollection {
        #[derive(serde::Deserialize)]
        struct Window {
            snap_info: crate::awr::SnapInfo,
            load_profile: Vec<crate::awr::LoadProfile>,
            time_model_stats: Vec<crate::awr::TimeModelStats>,
            instance_stats: Vec<crate::awr::InstanceStats>,
            data_availability: std::collections::HashMap<String, bool>,
        }
        let windows: Vec<Window> = serde_json::from_str(include_str!(
            "../../test_support/fixtures/empty_calories/native_scan_targets.json"
        ))
        .unwrap();
        AWRSCollection {
            awrs: windows
                .into_iter()
                .map(|w| {
                    let mut a = AWR::default();
                    a.snap_info = w.snap_info;
                    a.load_profile = w.load_profile;
                    a.time_model_stats = w.time_model_stats;
                    a.instance_stats = w.instance_stats;
                    a.data_availability = w.data_availability;
                    a
                })
                .collect(),
            db_instance_information: Default::default(),
            initialization_parameters: Default::default(),
            sql_text: Default::default(),
            nmon: None,
        }
    }

    /// Exercise the same normalized inputs and target families as the CLI without
    /// writing HTML or invoking a model. Shared by the classic/MCP contract replay.
    pub(crate) fn standard_report(data: &str) -> (AWRSCollection, crate::reasonings::ReportForAI) {
        use clap::Parser;
        let collection = load_awrs_collection_from_json_str(data).unwrap();
        let args = crate::Args::parse_from(["jas-min"]);
        let range = (0, u64::MAX);
        let rates = instance_rates(&collection, &range);
        let db_time = db_load_series(&collection, &range, DbLoadMetric::DbTime);
        let db_cpu = db_load_series(&collection, &range, DbLoadMetric::DbCpu);
        let fit = |y: &[f64], select: fn(&str) -> bool| {
            let features = rates
                .iter()
                .filter(|(name, _)| select(name))
                .map(|(name, values)| (name.clone(), values.clone()))
                .collect();
            crate::gradient::build_db_time_gradient_section(
                y,
                &features,
                args.ridge_lambda,
                args.en_lambda,
                args.en_alpha,
                args.en_max_iter,
                args.en_tol,
                "statistic_rate_counter",
                10,
            )
        };
        let labels = collection
            .awrs
            .iter()
            .map(|a| a.snap_info.begin_snap_time.clone())
            .collect::<Vec<_>>();
        let report = crate::reasonings::ReportForAI {
            db_load_sources: db_load_sources(&collection, &range),
            db_time_gradient_instance_stats_counters: Some(
                fit(&db_time, crate::staticdata::is_counter_stat).unwrap(),
            ),
            db_cpu_gradient_instance_stats: Some(
                fit(&db_cpu, crate::staticdata::is_cpu_stat).unwrap(),
            ),
            db_time_degradation_report: crate::degradation::build_db_time_degradation_report(
                &collection,
                &range,
                &labels,
                &db_time,
                &db_cpu,
                &Default::default(),
                &Default::default(),
                &Default::default(),
                &args,
            ),
            ..Default::default()
        };
        (collection, report)
    }

    #[test]
    fn native_short_windows_recover_scan_signal_from_existing_time_model() {
        let collection = native_target_collection();
        let before = serde_json::to_string(&collection).unwrap();
        let (_, report) = standard_report(&before);
        assert_eq!(report.db_load_sources["DB time"].time_model, 24);
        assert_eq!(report.db_load_sources["DB CPU"].time_model, 24);
        assert_eq!(serde_json::to_string(&collection).unwrap(), before);
        let section = report
            .db_time_gradient_instance_stats_counters
            .as_ref()
            .unwrap();
        for (model, expected_rank) in [("ridge", 2), ("huber", 2), ("quantile95", 5)] {
            let scan = section.model_rankings[model]
                .iter()
                .find(|r| r.event_name == "table scan blocks gotten")
                .unwrap();
            assert_eq!(scan.active_rank, Some(expected_rank), "{model}");
            assert!(scan.gradient_coef > 0.0);
        }
        // The classic payload carries the same recovered signal and source coverage.
        let classic = crate::reasonings::gradient_prompt_value(&report);
        assert_eq!(classic["db_load_sources"]["DB time"]["time_model"], 24);
        assert!(
            classic["db_time_gradient_instance_stats_counters"]["ridge_top"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["event_name"] == "table scan blocks gotten")
        );
    }
    #[test]
    fn equal_rates_survive_unequal_windows_and_missing_stats_are_excluded() {
        let mut c = load_awrs_collection_from_json_str(include_str!(
            "../../test_support/fixtures/empty_calories/scan_degradation.json"
        ))
        .unwrap();
        c.awrs.truncate(2);
        for (i, a) in c.awrs.iter_mut().enumerate() {
            a.snap_info.begin_snap_time = "2026-09-13T10:00:00Z".into();
            a.snap_info.end_snap_time = format!("2026-09-13T10:0{}:00Z", i + 1);
            for s in &mut a.instance_stats {
                s.total = 600 * (i as u64 + 1);
            }
        }
        let rate = instance_rates(&c, &(0, u64::MAX));
        assert_eq!(rate["table scan blocks gotten"], vec![10.0, 10.0]);
        let timeline = crate::ai_tools::dispatch_tool_call_value(
            "get_metric_time_series",
            &serde_json::json!({"kind":"instance_stat", "name":"table scan blocks gotten", "field":"per_second"}),
            &c,
            "unused",
        );
        assert_eq!(timeline["series"][0]["value"], 10.0);
        assert_eq!(timeline["series"][1]["value"], 10.0);
        for a in &mut c.awrs {
            let mut gauge = a.instance_stats[0].clone();
            gauge.statname = "logons current".into();
            gauge.total = 7;
            a.instance_stats.push(gauge);
        }
        let gauge = crate::ai_tools::dispatch_tool_call_value(
            "get_metric_time_series",
            &serde_json::json!({"kind":"instance_stat", "name":"logons current", "field":"per_second"}),
            &c,
            "unused",
        );
        assert_eq!(gauge["series"][0]["value"], 7.0);
        assert_eq!(gauge["series"][1]["value"], 7.0);
        c.awrs[1]
            .instance_stats
            .retain(|s| s.statname != "table scan blocks gotten");
        assert!(!instance_rates(&c, &(0, u64::MAX)).contains_key("table scan blocks gotten"));
        c.awrs[1].snap_info.end_snap_time = "invalid".into();
        let remaining = instance_rates(&c, &(0, u64::MAX));
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining["logons current"], vec![7.0, 7.0]);
    }
    #[test]
    fn recorded_scan_and_migration_are_observed_without_fabricating_segment_proof() {
        for (name, data) in [
            (
                "scan",
                include_str!("../../test_support/fixtures/empty_calories/scan_degradation.json"),
            ),
            (
                "migr",
                include_str!("../../test_support/fixtures/empty_calories/migr_degradation.json"),
            ),
        ] {
            let c = load_awrs_collection_from_json_str(data).unwrap();
            let range = (0, u64::MAX);
            let rates = instance_rates(&c, &range);
            for key in ["table scan blocks gotten", "table scan rows gotten"] {
                assert!(crate::staticdata::is_counter_stat(key));
                assert_eq!(rates[key].len(), 15);
            }
            let report = crate::access_path::build(
                &c,
                &range,
                crate::degradation::detected(&c, &range),
                Default::default(),
            );
            assert_eq!(report.evidence_level, "degradation_detected");
            assert_eq!(report.coverage["host_cpu"]["observed_windows"], 0);
            assert!(report.segment_candidates.is_empty());
            assert!(report.sql_costs.is_empty());
            let continued = report
                .instance_signals
                .iter()
                .find(|v| v["name"] == "table fetch continued row")
                .unwrap();
            assert_eq!(continued["material_activity"], name == "migr");
            assert_eq!(continued["mechanism_confirmed"], false);
            let target = c
                .awrs
                .iter()
                .map(|a| a.load_profile[0].per_second)
                .collect::<Vec<_>>();
            let features = rates
                .into_iter()
                .filter(|(k, _)| crate::staticdata::is_counter_stat(k))
                .collect();
            let gradients = crate::gradient::build_db_time_gradient_section(
                &target,
                &features,
                30.0,
                Some(0.1),
                0.2,
                1000,
                1e-6,
                "counter rates",
                10,
            )
            .unwrap();
            for ranking in gradients.model_rankings.values() {
                for key in ["table scan blocks gotten", "table scan rows gotten"] {
                    assert!(ranking.iter().any(|v| v.event_name == key));
                }
            }
        }
    }
}
