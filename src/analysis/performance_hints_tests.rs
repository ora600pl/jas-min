use super::*;
use crate::awr::{
    InstanceStats, SQLCPUTime, SQLElapsedTime, SQLGets, TimeModelStats, TopSQLWithTopEvents,
};

pub(super) fn native() -> AWRSCollection {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/empty_calories/hints_native.json"
    ))
    .unwrap()
}
fn run(c: &AWRSCollection) -> PerformanceHintsReport {
    build(c, &(0, u64::MAX), Policy::default())
}
fn set_stat(a: &mut AWR, name: &str, total: u64) {
    a.instance_stats.retain(|s| s.statname != name);
    a.instance_stats.push(InstanceStats {
        statname: name.into(),
        total,
    });
}
fn set_sql(a: &mut AWR, gets: f64, cpu: f64, executions: u64) {
    a.sql_gets.insert("example".into(),serde_json::from_value(json!({"sql_id":"example","buffer_gets":gets*executions as f64,"executions":executions,"gets_per_exec":0,"pct_cpu":0,"pct_io":0,"pct_total":0,"sql_module":"test"})).unwrap());
    a.sql_cpu_time.insert(
        "example".into(),
        SQLCPUTime {
            sql_id: "example".into(),
            cpu_time_s: cpu * executions as f64,
            executions,
            ..Default::default()
        },
    );
    a.sql_elapsed_time = vec![SQLElapsedTime {
        sql_id: "example".into(),
        elapsed_time_s: cpu * 3.0 * executions as f64,
        executions,
        ..Default::default()
    }];
    a.top_sql_with_top_events.insert(
        "example".into(),
        TopSQLWithTopEvents {
            sql_id: "example".into(),
            plan_hash_value: 123,
            top_row_source: "INDEX - RANGE SCAN".into(),
            ..Default::default()
        },
    );
}
fn sample(i: usize) -> AWR {
    let mut a = AWR::default();
    let start = measurements::timestamp("2026-06-15 00:00:00").unwrap();
    a.snap_info.begin_snap_id = i as u64 + 1;
    a.snap_info.end_snap_id = i as u64 + 2;
    a.snap_info.begin_snap_time = (start + chrono::Duration::minutes(i as i64)).to_string();
    a.snap_info.end_snap_time = (start + chrono::Duration::minutes(i as i64 + 1)).to_string();
    for (name, n) in [
        (BLOCKS, 300_000),
        (SHORT, 3000),
        (LONG, 0),
        ("execute count", 3000),
    ] {
        set_stat(&mut a, name, n);
    }
    for name in PATH_COUNTERS {
        set_stat(&mut a, name, 0);
    }
    for name in ["DB time", "DB CPU"] {
        a.time_model_stats.push(TimeModelStats {
            stat_name: name.into(),
            time_s: 12.0,
            ..Default::default()
        });
    }
    set_sql(&mut a, 100.0, 0.01, 100);
    a
}
fn synthetic(n: usize) -> AWRSCollection {
    let mut c = native();
    c.awrs = (0..n).map(sample).collect();
    c.sql_text.clear();
    c
}
fn grow_sql(c: &mut AWRSCollection, indices: &[usize]) {
    for &i in indices {
        set_sql(&mut c.awrs[i], 200.0, 0.02, 100);
    }
}
fn sql_hint(r: &PerformanceHintsReport) -> Option<&PerformanceHint> {
    r.hints.iter().find(|h| {
        h.assessment_scope == "sql:example" && h.assessment_status != "persistent_cost_observation"
    })
}

#[test]
fn native_lab_retains_fragmentation_hypothesis_and_sparse_segment() {
    let r = run(&native());
    assert!(r
        .hints
        .iter()
        .any(|h| h.assessment_scope == "sql:9paxwp1pabugh"));
    assert!(r.hints.iter().any(|h| h
        .possibly_affected_segments
        .iter()
        .any(|s| s.segment.object_name == "ERP_SPARSE")));
    assert!(r.hints.iter().all(|h| h
        .limitations
        .iter()
        .any(|s| s.contains("Data growth") || s.contains("data growth"))));
}
#[test]
fn mixed_paths_and_falling_instance_load_do_not_veto_local_cost() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5, 7]);
    for (i, a) in c.awrs.iter_mut().enumerate() {
        for name in PATH_COUNTERS {
            set_stat(a, name, 100 + i as u64);
        }
        if i >= 4 {
            a.time_model_stats.iter_mut().for_each(|s| s.time_s = 6.0);
            set_stat(a, SHORT, 10000);
        }
    }
    let r = run(&c);
    let h = sql_hint(&r).unwrap();
    assert_eq!(h.assessment_status, "recurrent_hypothesis");
    assert_eq!(r.rule_evaluations[0].excluded_windows, 0);
    assert!(h.mechanisms.is_empty());
    assert!(h
        .alternative_explanations
        .iter()
        .any(|s| s.contains("Sparse table or index")));
}
#[test]
fn volume_only_is_not_cost_growth() {
    let mut c = synthetic(8);
    for a in &mut c.awrs[4..] {
        set_sql(a, 100.0, 0.01, 1000);
        set_stat(a, SHORT, 30000);
        set_stat(a, BLOCKS, 3000000);
    }
    assert!(sql_hint(&run(&c)).is_none());
}
#[test]
fn two_of_three_opportunities_and_single_spike_are_distinct() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4]);
    let r = run(&c);
    assert_eq!(
        sql_hint(&r).unwrap().assessment_status,
        "provisional_hypothesis"
    );
    grow_sql(&mut c, &[6]);
    let r = run(&c);
    assert_eq!(
        sql_hint(&r).unwrap().assessment_status,
        "recurrent_hypothesis"
    );
    let mut c = synthetic(10);
    grow_sql(&mut c, &[4, 9]);
    let r = run(&c);
    assert_eq!(
        sql_hint(&r).unwrap().assessment_status,
        "provisional_hypothesis"
    );
}
#[test]
fn missing_top_is_unknown_but_no_eighty_percent_gate() {
    let mut c = synthetic(12);
    grow_sql(&mut c, &[7, 9]);
    for i in [3, 4, 5, 6, 8, 10, 11] {
        c.awrs[i].sql_gets.clear();
    }
    let r = run(&c);
    let h = sql_hint(&r).unwrap();
    assert_eq!(h.assessment_status, "recurrent_hypothesis");
    let coverage = r.rule_evaluations[0]
        .scope_evaluations
        .iter()
        .find(|s| s.scope == "sql:example")
        .unwrap();
    assert_eq!(coverage.missing_observations, 7);
    assert_eq!(coverage.observed_opportunities, 5);
}
#[test]
fn sql_domains_never_borrow_denominators_or_ignore_masks() {
    let mut a = sample(0);
    a.sql_gets.get_mut("example").unwrap().executions = 0;
    assert!(sql_sample(&a, "example").is_none());
    assert!(sql_value(&a, "example", "sql_cpu_time").is_some());
    a.sql_gets.get_mut("example").unwrap().executions = 1000;
    let s = sql_sample(&a, "example").unwrap();
    assert_eq!(s.work, 10.0);
    assert_eq!(s.cpu, Some(0.01));
    a.data_availability.insert("sql_gets".into(), false);
    assert!(sql_sample(&a, "example").is_none());
    let mut a = sample(0);
    a.sql_elapsed_time.push(a.sql_elapsed_time[0].clone());
    assert!(sql_value(&a, "example", "sql_elapsed_time").is_none());
}
#[test]
fn seasonal_cohorts_compare_days_and_keep_weekends_separate() {
    let mut c = synthetic(7 * 24);
    let start = measurements::timestamp("2026-06-15 00:00:00").unwrap();
    for (i, a) in c.awrs.iter_mut().enumerate() {
        a.snap_info.begin_snap_time = (start + chrono::Duration::hours(i as i64)).to_string();
        a.snap_info.end_snap_time = (start + chrono::Duration::hours(i as i64 + 1)).to_string();
        let day = i / 24;
        let hour = i % 24;
        let cost = if hour < 6 { 500.0 } else { 100.0 };
        set_sql(a, cost, cost / 10000.0, 100);
        if day == 3 || day == 4 {
            set_sql(a, cost * 2.0, cost / 5000.0, 100);
        }
        if day >= 5 {
            set_sql(a, 2000.0, 0.2, 100);
        }
    }
    let r = run(&c);
    let h = sql_hint(&r).unwrap();
    assert!(h
        .comparisons
        .iter()
        .any(|v| v.recurring && v.baseline_sufficient));
    assert!(h
        .comparisons
        .iter()
        .all(|v| v.profile.starts_with("weekday:")));
    assert!(h.comparisons.iter().all(|v| v
        .baseline
        .intervals
        .iter()
        .all(|(_, end)| *end <= v.recent.begin_snap_id)));
}
#[test]
fn baseline_is_frozen_and_fixed_reference_catches_gradual_growth() {
    let mut c = synthetic(30);
    for (i, a) in c.awrs.iter_mut().enumerate() {
        let factor = 1.0 + i as f64 * 0.02;
        set_sql(a, 100.0 * factor, 0.01 * factor, 100);
    }
    let r = run(&c);
    let h = sql_hint(&r).unwrap();
    assert!(h
        .comparisons
        .iter()
        .any(|v| v.baseline_selection == "fixed_comparable_reference"));
    assert!(h
        .comparisons
        .iter()
        .any(|v| v.baseline_selection == "frozen_comparable_reference"));
}
#[test]
fn waiting_retains_local_symptom_with_explicit_counterevidence() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    for a in &mut c.awrs[4..6] {
        a.sql_cpu_time.get_mut("example").unwrap().cpu_time_s = 1.0;
        a.foreground_wait_events.push(crate::awr::WaitEvents {
            event: "library cache pin".into(),
            total_wait_time_s: 11.0,
            ..Default::default()
        });
    }
    let r = run(&c);
    let h = sql_hint(&r).unwrap();
    assert_eq!(h.confidence, "low");
    assert!(h
        .counterevidence
        .iter()
        .any(|s| s.contains("Wait confounder")));
}
#[test]
fn changing_scan_mix_blocks_only_the_instance_ratio() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    for a in &mut c.awrs[4..] {
        set_stat(a, BLOCKS, 1200000);
        set_stat(a, LONG, 3000);
        a.time_model_stats.iter_mut().for_each(|s| s.time_s = 48.0);
    }
    let r = run(&c);
    assert!(sql_hint(&r).is_some());
    assert!(!r.hints.iter().any(|h| h.assessment_scope == "instance"));
}
#[test]
fn plan_changes_lower_confidence_without_discarding_symptom() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    for a in &mut c.awrs[4..] {
        a.top_sql_with_top_events
            .get_mut("example")
            .unwrap()
            .plan_hash_value = 456;
    }
    let r = run(&c);
    let h = sql_hint(&r).unwrap();
    assert_eq!(h.confidence, "low");
    assert!(h.limitations.iter().any(|s| s.contains("plan change")));
}
#[test]
fn invalid_intervals_are_local_exclusions_and_do_not_join_epochs() {
    let mut c = synthetic(16);
    grow_sql(&mut c, &[4, 5, 12, 13]);
    c.awrs[7].snap_info.end_snap_time = "invalid".into();
    let r = run(&c);
    assert_eq!(r.rule_evaluations[0].excluded_windows, 1);
    assert_eq!(
        r.hints
            .iter()
            .filter(|h| h.assessment_scope == "sql:example")
            .count(),
        2
    );
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    c.awrs.push(c.awrs[1].clone());
    let r = run(&c);
    assert_eq!(r.rule_evaluations[0].excluded_windows, 2);
}
#[test]
fn explicit_baseline_bounds_and_policy_validation() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    let p = Policy {
        baseline_snap_range: Some((1, 4)),
        ..Default::default()
    };
    let r = build(&c, &(0, 99), p.clone());
    assert_eq!(
        sql_hint(&r)
            .unwrap()
            .baseline
            .as_ref()
            .unwrap()
            .begin_snap_id,
        1
    );
    assert!(build(&c, &(2, 99), p).hints.is_empty());
    for s in [
        r#"{"consecutive_windows":0}"#,
        r#"{"minimum_baseline_windows":1}"#,
        r#"{"minimum_recurrences":4}"#,
        r#"{"noise_multiplier":-1}"#,
        r#"{"minimum_rowid_fetches":0}"#,
        r#"{"minimum_continuation_delta_per_rowid":0}"#,
        r#"{"minimum_segment_excess_reads_per_second":-1}"#,
    ] {
        assert!(serde_json::from_str::<Policy>(s)
            .unwrap()
            .validate()
            .is_err());
    }
    assert!(serde_json::from_str::<Policy>(r#"{"misspelled":1}"#).is_err());
}
pub(crate) fn material_cost_fixture() -> AWRSCollection {
    let mut c = synthetic(6);
    for a in &mut c.awrs {
        set_sql(a, 200000.0, 1.0, 100);
    }
    // CPU and gets keep their own execution denominators.
    c.awrs[0]
        .sql_cpu_time
        .get_mut("example")
        .unwrap()
        .executions = 50;
    c
}

#[test]
fn persistent_cost_is_an_absolute_observation_not_a_zero_change_comparison() {
    let c = material_cost_fixture();
    let r = run(&c);
    let h = r.hints.iter().find(|h| h.is_cost_observation()).unwrap();
    assert_eq!(h.confidence, "not_applicable");
    assert!(h.baseline.is_none() && h.comparison.is_none());
    assert!(h.comparisons.is_empty() && h.evidence.is_empty() && h.sql_evidence.is_empty());
    assert!(h.mechanisms.is_empty() && h.possibly_affected_segments.is_empty());
    assert_eq!(h.observed_metrics.len(), 2);
    let gets = &h.observed_metrics[0];
    assert_eq!(
        (gets.value, gets.total, gets.exposure),
        (200000.0, 120000000.0, 600.0)
    );
    let cpu = &h.observed_metrics[1];
    assert_eq!(
        (cpu.value, cpu.total, cpu.exposure),
        (600.0 / 550.0, 600.0, 550.0)
    );
    assert_eq!(cpu.period.intervals.len(), 6);
    let value = query(Some(&r), &json!({"scope":"sql:example"}));
    assert!(value["hints"][0]["baseline"].is_null());
    assert!(value["hints"][0]["comparison"].is_null());
    assert!(value["hints"][0]["observed_metrics"][0]
        .get("growth_pct")
        .is_none());
    let html = render_html(&r);
    assert!(html.contains("<details class=access-cost-observations>"));
    assert!(html.contains("200000.0000 gets/execution"));
    assert!(!html.contains("→") && !html.contains("Prove:") && !html.contains("Hint:"));
    assert!(!html.contains("Possibly affected segments:"));
    assert_eq!(r.rule_evaluations[0].status, "assessed_no_growth_signal");
    assert_eq!(index(Some(&r))["hypotheses_total"], 0);
    assert_eq!(index(Some(&r))["cost_observations_total"], 1);
}

#[test]
fn observation_segments_use_only_observed_exposure_and_stay_out_of_affected_segments() {
    let mut c = material_cost_fixture();
    for a in &mut c.awrs {
        a.segment_stats.insert(
            "Logical Reads".into(),
            vec![SegmentStats {
                object_name: "FOLDER".into(),
                object_type: "TABLE".into(),
                obj: 123,
                objd: 456,
                stat_name: "Logical Reads".into(),
                stat_vlalue: 6000.0,
                ..Default::default()
            }],
        );
    }
    c.awrs[0].segment_stats.clear(); // TOP absence must not dilute the rate.
    let mut r = run(&c);
    let dir = std::env::temp_dir().join(format!("jasmin-cost-observation-{}", std::process::id()));
    let attachments = dir.join("input_attachments");
    std::fs::create_dir_all(&attachments).unwrap();
    std::fs::write(attachments.join("example.xplan"), "SQL_ID example, child number 0\nselect * from FOLDER\nPlan hash value: 123\n----------------------------------------------\n| Id | Operation | Name | Rows | Cost |\n----------------------------------------------\n| 0 | SELECT STATEMENT | | 1 | 1 |\n| 1 | TABLE ACCESS FULL | FOLDER | 1 | 1 |\n----------------------------------------------\n").unwrap();
    enrich_with_plans(&mut r, &c, dir.join("input").to_str().unwrap());
    let h = r.hints.iter().find(|h| h.is_cost_observation()).unwrap();
    assert!(h.possibly_affected_segments.is_empty());
    assert_eq!(h.segment_candidates_total, 0);
    let m = &h.observed_segments[0].logical_reads;
    assert_eq!(
        (m.value, m.total, m.exposure, m.period.windows),
        (100.0, 30000.0, 300.0, 5)
    );
    assert_eq!(m.period.intervals[0], (2, 3));
    let html = render_cost_observation(h);
    assert!(html.contains("logical reads/s 100.00"));
    assert!(!html.contains("→") && !html.contains("Prove:"));
    assert!(!html.contains("does not exclude fragmentation"));
    let duplicate = c.awrs[1].segment_stats["Logical Reads"][0].clone();
    c.awrs[1]
        .segment_stats
        .get_mut("Logical Reads")
        .unwrap()
        .push(duplicate);
    enrich_with_plans(&mut r, &c, dir.join("input").to_str().unwrap());
    assert!(r
        .hints
        .iter()
        .find(|h| h.is_cost_observation())
        .unwrap()
        .observed_segments
        .is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn real_cost_growth_precedes_larger_absolute_costs_and_keeps_comparisons() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    let mut r = run(&c);
    let observations = run(&material_cost_fixture());
    r.hints.splice(0..0, observations.hints); // Render correctly even with unsorted input.
    let html = render_html(&r);
    assert!(
        html.find("class=growth-hypothesis").unwrap()
            < html.find("class=cost-observation").unwrap()
    );
    let h = sql_hint(&r).unwrap();
    assert_ne!(
        h.baseline.as_ref().unwrap().intervals,
        h.comparison.as_ref().unwrap().intervals
    );
    assert!(h.evidence.iter().any(|e| e.recent > e.baseline));
    assert!(h.observed_metrics.is_empty());
}
#[test]
fn nonfinite_costs_and_zero_baselines_do_not_produce_nonfinite_json() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    c.awrs[7]
        .sql_cpu_time
        .get_mut("example")
        .unwrap()
        .cpu_time_s = f64::NAN;
    let r = run(&c);
    let serialized = serde_json::to_string(&r).unwrap();
    assert!(!serialized.contains("NaN"));
    assert!(!serialized.contains("Infinity"));
    assert!(growth(&[0.0, 0.0, 0.0], 1.0, 25.0, 0.0001, &Policy::default()).is_some());
}
#[test]
fn pagination_serialization_escaping_and_old_report_defaults() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    let mut r = run(&c);
    let h = sql_hint(&r).unwrap();
    let id = h.hint_id.clone();
    assert_eq!(query(Some(&r), &json!({"hint_id":id}))["total"], 1);
    assert_eq!(query(Some(&r), &json!({"offset":999}))["hints"], json!([]));
    assert_eq!(index(None)["status"], "not_computed");
    r.hints[0].title = "<script>alert(1)</script>".into();
    let html = render_html(&r);
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("Observed evidence"));
    assert!(!html.contains("Prove:"));
    assert!(html.contains(NO_SEGMENTS));
    let mut v = serde_json::to_value(&r).unwrap();
    for h in v["hints"].as_array_mut().unwrap() {
        h.as_object_mut().unwrap().remove("comparisons");
        h.as_object_mut().unwrap().remove("confidence");
        h.as_object_mut().unwrap().remove("observed_metrics");
        h.as_object_mut().unwrap().remove("observed_segments");
    }
    assert!(serde_json::from_value::<PerformanceHintsReport>(v).is_ok());
}
#[test]
fn plan_objects_include_index_and_table_with_explicit_attribution_limits() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    // SQL unit work rises while this plan-linked segment's total activity falls.
    // A global reads/s increase must not be a prerequisite for the SQL candidate.
    for (i, a) in c.awrs.iter_mut().enumerate() {
        a.segment_stats.insert(
            "Logical Reads".into(),
            vec![SegmentStats {
                object_name: "UQ_EVENTS_RECIPIENT".into(),
                object_type: "INDEX".into(),
                obj: 123,
                objd: 456,
                stat_vlalue: if i < 4 { 60000.0 } else { 30000.0 },
                ..Default::default()
            }],
        );
    }
    let mut r = run(&c);
    let dir = std::env::temp_dir().join(format!("jasmin-hint-plans-{}", std::process::id()));
    let stem = dir.join("input");
    let attachments = dir.join("input_attachments");
    std::fs::create_dir_all(&attachments).unwrap();
    std::fs::write(attachments.join("example.xplan"),"SQL_ID example, child number 0\nselect * from EVENTS\nPlan hash value: 123\n----------------------------------------------\n| Id | Operation | Name | Rows | Cost |\n----------------------------------------------\n| 0 | SELECT STATEMENT | | 1 | 1 |\n| 1 | INDEX RANGE SCAN | UQ_EVENTS_RECIPIENT | 1 | 1 |\n| 2 | TABLE ACCESS BY INDEX ROWID | EVENTS | 1 | 1 |\n----------------------------------------------\n").unwrap();
    enrich_with_plans(&mut r, &c, stem.to_str().unwrap());
    let h = sql_hint(&r).unwrap();
    assert!(h
        .plan_objects
        .iter()
        .any(|v| v.object_name == "UQ_EVENTS_RECIPIENT"));
    assert!(h.plan_objects.iter().any(|v| v.object_name == "EVENTS"));
    assert!(h
        .context_evidence
        .iter()
        .any(|v| v.label.contains("does not exclude fragmentation")));
    assert!(h
        .possibly_affected_segments
        .iter()
        .any(|s| s.segment.object_name == "UQ_EVENTS_RECIPIENT"
            && s.logical_reads.recent < s.logical_reads.baseline));
    std::fs::remove_dir_all(dir).unwrap();
}

/// Optional read-only replay of the complete user-provided input; no production
/// data is checked into the test suite. Artifacts are explicitly selected by env.
#[test]
fn production_replay_when_requested() {
    let Ok(path) = std::env::var("JASMIN_HINT_REPLAY_INPUT") else {
        return;
    };
    let text = std::fs::read_to_string(&path).unwrap();
    let c = crate::awr::load_awrs_collection_from_json_str(&text).unwrap();
    let before = serde_json::to_string(&c).unwrap();
    let mut r = run(&c);
    let stem = std::env::var("JASMIN_HINT_REPLAY_STEM")
        .unwrap_or_else(|_| path.trim_end_matches(".json").into());
    enrich_with_plans(&mut r, &c, &stem);
    assert_eq!(before, serde_json::to_string(&c).unwrap());
    assert_eq!(r.rule_evaluations[0].excluded_windows, 0);
    let events = r
        .hints
        .iter()
        .find(|h| h.assessment_scope == "sql:gn3gtqxvucbj8")
        .expect("production EVENTS cost lead retained");
    assert!(!events.comparisons.is_empty());
    let folder = r
        .hints
        .iter()
        .find(|h| h.assessment_scope == "sql:4nux9ys05d5pq")
        .unwrap();
    assert!(folder.is_cost_observation());
    assert!(folder.baseline.is_none() && folder.comparison.is_none());
    assert!(folder.evidence.is_empty() && folder.possibly_affected_segments.is_empty());
    assert_eq!(folder.observed_metrics.len(), 2);
    assert!(!render_cost_observation(folder).contains("→"));
    assert!(!r.hints[0].is_cost_observation());
    let out = std::env::var("JASMIN_HINT_REPLAY_OUTPUT").unwrap();
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(
        format!("{out}/performance_hints.json"),
        serde_json::to_string_pretty(&r).unwrap(),
    )
    .unwrap();
    std::fs::write(format!("{out}/performance_hints.html"), render_html(&r)).unwrap();
    eprintln!(
        "replay: {} windows, {} hints, {} comparisons",
        c.awrs.len(),
        r.hints.len(),
        r.rule_evaluations[0].assessed_comparisons
    );
}

#[test]
fn partial_cpu_history_uses_observed_cost_windows() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[5, 6]);
    c.awrs[1].sql_cpu_time.clear();
    let r = run(&c);
    let h = sql_hint(&r).unwrap();
    let a = &h.comparisons[0];
    assert_eq!(a.cost.as_ref().unwrap().metric, "CPU");
    assert_eq!(a.cost.as_ref().unwrap().baseline_observations, 4);
    assert_eq!(a.cost_baseline_intervals.len(), 4);
}
#[test]
fn same_counterexamples_have_same_physical_interpretation() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../tests/fixtures/empty_calories/scan_counterexamples.json"
    ))
    .unwrap();
    for repeat in 1..=3 {
        let get = |case: &str| {
            cases
                .iter()
                .find(|v| v["case"] == case && v["repeat"] == repeat)
                .unwrap()
        };
        assert_eq!(
            get("scan_live_8000")["metrics"],
            get("scan_deleted_6000_live_2000")["metrics"]
        );
        let mut c = synthetic(8);
        grow_sql(&mut c, &[4, 5]);
        let r = run(&c);
        assert!(sql_hint(&r)
            .unwrap()
            .alternative_explanations
            .iter()
            .any(|s| s.contains("Data growth")));
    }
}
#[test]
fn segment_duplicate_keys_and_changed_dataobj_are_not_merged() {
    let mut c = native();
    let b: Vec<_> = c.awrs[..5].iter().collect();
    let mut recent = c.awrs[15..18].to_vec();
    for a in &mut recent {
        for s in a.segment_stats.values_mut().flatten() {
            s.objd += 1;
        }
    }
    assert!(
        segment_candidates(&b, &recent.iter().collect::<Vec<_>>(), &Policy::default()).is_empty()
    );
    for a in &mut c.awrs {
        if let Some(v) = a.segment_stats.get_mut("Logical Reads") {
            let copies = v.clone();
            v.extend(copies);
        }
    }
    assert!(segment_candidates(
        &c.awrs[..5].iter().collect::<Vec<_>>(),
        &c.awrs[15..18].iter().collect::<Vec<_>>(),
        &Policy::default()
    )
    .is_empty());
}

#[test]
fn scoped_query_returns_only_requested_sql_and_its_coverage() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    let r = run(&c);
    let q = query(Some(&r), &json!({"scope":"sql:example"}));
    assert_eq!(q["total"], 1);
    assert!(q["rule_evaluations"][0]["scope_evaluations"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["scope"] == "sql:example"));
    assert_eq!(query(Some(&r), &json!({"scope":"sql:unknown"}))["total"], 0);
}
#[test]
fn explicit_baseline_never_uses_recent_data_for_an_absent_calendar_profile() {
    let mut c = synthetic(72);
    let start = measurements::timestamp("2026-06-15 00:00:00").unwrap();
    for (i, a) in c.awrs.iter_mut().enumerate() {
        a.snap_info.begin_snap_time = (start + chrono::Duration::hours(i as i64)).to_string();
        a.snap_info.end_snap_time = (start + chrono::Duration::hours(i as i64 + 1)).to_string();
        if i >= 24 {
            set_sql(a, 200.0, 0.02, 100);
        }
    }
    let r = build(
        &c,
        &(0, 99),
        Policy {
            baseline_snap_range: Some((1, 4)),
            ..Default::default()
        },
    );
    assert!(r.hints.iter().flat_map(|h| &h.comparisons).all(|v| v
        .baseline
        .intervals
        .iter()
        .all(|(b, e)| *b >= 1 && *e <= 4)));
}

#[test]
fn masked_ash_does_not_supply_a_persistent_access_path() {
    let mut c = synthetic(6);
    for a in &mut c.awrs {
        set_sql(a, 200000.0, 1.0, 100);
        a.data_availability
            .insert("top_sql_with_top_events".into(), false);
    }
    assert!(run(&c)
        .hints
        .iter()
        .all(|h| h.assessment_status != "persistent_cost_observation"));
}

pub(crate) fn work_only_fixture() -> AWRSCollection {
    let mut c = synthetic(12);
    grow_sql(&mut c, &(4..12).collect::<Vec<_>>());
    for a in &mut c.awrs[..7] {
        a.sql_cpu_time.clear();
        a.sql_elapsed_time.clear();
    }
    c
}

#[test]
fn missing_both_time_domains_preserves_work_reference_and_never_learns_late_time_as_normal() {
    let c = work_only_fixture();
    let r = run(&c);
    let h = sql_hint(&r).unwrap();
    assert_eq!(h.assessment_status, "recurrent_work_inflation");
    assert_eq!(h.comparisons.len(), 8);
    assert!(h.comparisons.iter().all(|a| a.work.baseline == 100.0));
    assert!(h.comparisons.iter().all(|a| a.baseline.end_snap_id <= 5));
    assert!(h.comparisons.iter().all(|a| a.cost.is_none()));
    assert!(h
        .comparisons
        .iter()
        .all(|a| a.cost_evaluations.iter().all(|c| c.baseline.is_none())));
    assert_eq!(h.time_impact_status, "time_comparison_unavailable");
    assert_eq!(h.trajectory.len(), 12);
    let coverage = r.rule_evaluations[0]
        .scope_evaluations
        .iter()
        .find(|e| e.scope == "sql:example")
        .unwrap();
    assert_eq!(
        (
            coverage.observed_opportunities,
            coverage.cpu_observations,
            coverage.elapsed_observations
        ),
        (12, 5, 5)
    );
    let value = serde_json::to_value(h).unwrap();
    assert!(value["comparisons"][0]["cost"].is_null());
    assert!(!render_growth_hint(h).contains("0.000 → 0.000"));
}

#[test]
fn cpu_history_includes_observed_time_even_when_gets_is_missing() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    c.awrs[1].sql_gets.clear();
    let r = run(&c);
    let a = &sql_hint(&r).unwrap().comparisons[0];
    assert_eq!(a.work.baseline_observations, 3);
    assert_eq!(a.cost.as_ref().unwrap().baseline_observations, 4);
    assert!(a.cost_baseline_intervals.contains(&(2, 3)));
    assert!(!a.baseline.intervals.contains(&(2, 3)));
}

#[test]
fn measured_stable_time_is_distinct_from_unavailable_time() {
    let mut c = synthetic(8);
    for a in &mut c.awrs[4..] {
        set_sql(a, 200.0, 0.01, 100);
    }
    let r = run(&c);
    let h = sql_hint(&r).unwrap();
    assert_eq!(h.assessment_status, "recurrent_work_inflation");
    assert_eq!(h.time_impact_status, "no_unconfounded_time_growth");
    assert!(h.comparisons.iter().all(|a| a.cost.is_none()));
    assert!(h.comparisons.iter().all(|a| a
        .cost_evaluations
        .iter()
        .all(|c| c.status == "no_growth" && c.comparison.is_some())));
}

pub(crate) fn continuation_only_fixture() -> AWRSCollection {
    let mut c = synthetic(8);
    for (i, a) in c.awrs.iter_mut().enumerate() {
        set_stat(a, ROWID, 3000);
        set_stat(a, CONTINUED, if i < 4 { 3 } else { 600 });
    }
    c
}

#[test]
fn continuation_is_independent_of_scans_time_and_sql_attribution() {
    let c = continuation_only_fixture();
    let r = run(&c);
    let h = r
        .hints
        .iter()
        .find(|h| h.assessment_scope == CONTINUATION_SCOPE)
        .unwrap();
    assert_eq!(h.signal_kind, "row_continuation_work_inflation");
    assert_eq!(h.assessment_status, "recurrent_work_inflation");
    assert!(h.evidence.iter().all(|e| e.metric == "continued_per_rowid"));
    assert!(
        h.possibly_affected_segments.is_empty()
            && h.sql_evidence.is_empty()
            && h.mechanisms.is_empty()
    );
    assert!(!r.hints.iter().any(|h| h.assessment_scope == "instance"));
    let mut volume = c.clone();
    for (i, a) in volume.awrs.iter_mut().enumerate() {
        set_stat(a, ROWID, if i < 4 { 3000 } else { 30000 });
        set_stat(a, CONTINUED, if i < 4 { 3 } else { 30 });
    }
    assert!(!run(&volume)
        .hints
        .iter()
        .any(|h| h.assessment_scope == CONTINUATION_SCOPE));
    let mut changed_mix = c;
    for a in &mut changed_mix.awrs[4..] {
        set_stat(a, ROWID, 12000);
    }
    assert!(!run(&changed_mix)
        .hints
        .iter()
        .any(|h| h.assessment_scope == CONTINUATION_SCOPE));
}

#[test]
fn decreasing_global_continuation_is_context_and_no_positive_sql_evidence() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    for (i, a) in c.awrs.iter_mut().enumerate() {
        set_stat(a, CONTINUED, if i < 4 { 120 } else { 0 });
    }
    let r = run(&c);
    let h = sql_hint(&r).unwrap();
    assert!(h.evidence.iter().all(|e| e.metric != CONTINUED));
    assert!(h.mechanisms.is_empty());
    assert!(h
        .context_evidence
        .iter()
        .any(|e| e.status == "no_increase" && e.label.contains("not SQL attribution")));
    assert!(h
        .counterevidence
        .iter()
        .any(|e| e.contains("no positive continuation evidence")));
    assert!(render_growth_hint(h).contains("Instance context — not SQL attribution"));
}

#[test]
fn segment_screen_rejects_small_and_activity_only_growth() {
    let mut c = synthetic(8);
    for (i, a) in c.awrs.iter_mut().enumerate() {
        let factor = if i < 4 { 1.0 } else { 3.0 };
        set_stat(a, SHORT, (3000.0 * factor) as u64);
        set_stat(a, "execute count", (3000.0 * factor) as u64);
        a.segment_stats.insert(
            "Logical Reads".into(),
            [
                ("STABLE", if i < 4 { 60000.0 } else { 63840.0 }),
                ("BUSIER", 60000.0 * factor),
                ("EXCESS", if i < 4 { 60000.0 } else { 600000.0 }),
            ]
            .into_iter()
            .enumerate()
            .map(|(j, (name, total))| SegmentStats {
                object_name: name.into(),
                object_type: "TABLE".into(),
                obj: j as u64,
                objd: j as u64,
                stat_vlalue: total,
                ..Default::default()
            })
            .collect(),
        );
    }
    let b = c.awrs[..4].iter().collect::<Vec<_>>();
    let r = c.awrs[4..].iter().collect::<Vec<_>>();
    let candidates = segment_candidates(&b, &r, &Policy::default());
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].segment.object_name, "EXCESS");
    let contexts = segment_assessments(&b, &r, &Policy::default());
    assert_eq!(
        contexts
            .iter()
            .find(|s| s.segment.object_name == "STABLE")
            .unwrap()
            .status,
        "no_material_read_growth"
    );
    assert_eq!(
        contexts
            .iter()
            .find(|s| s.segment.object_name == "BUSIER")
            .unwrap()
            .status,
        "activity_growth_can_explain_reads"
    );
}

#[test]
fn latest_status_and_limits_describe_their_own_period() {
    let mut c = synthetic(10);
    grow_sql(&mut c, &[2, 3, 4, 5, 6]);
    let r = run(&c);
    let h = sql_hint(&r).unwrap();
    assert!(h.comparisons.iter().any(|a| !a.baseline_sufficient));
    assert!(h.comparisons.iter().any(|a| a.baseline_sufficient));
    assert!(!h.limitations.iter().any(|s| s.contains("Short baseline")));
    assert_eq!(h.episode_status, "historical_growth_later_within_reference");
    let html = render_growth_hint(h);
    assert!(html.contains("Historical growth; later work returned within"));
    assert!(html.contains("<svg"));
    c.awrs[9].sql_gets.clear();
    let r = run(&c);
    assert_eq!(
        sql_hint(&r).unwrap().episode_status,
        "historical_growth_latest_work_unavailable"
    );
}

#[test]
fn recorded_counterexamples_reach_detector_without_claiming_physical_cause() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../tests/fixtures/empty_calories/scan_counterexamples.json"
    ))
    .unwrap();
    let make = |before: &str, after: &str| {
        let mut c = synthetic(8);
        for (i, a) in c.awrs.iter_mut().enumerate() {
            let label = if i < 4 { before } else { after };
            let v = cases
                .iter()
                .find(|v| v["case"] == label && v["repeat"] == 1)
                .unwrap();
            for (name, value) in v["metrics"].as_object().unwrap() {
                set_stat(a, name, value.as_u64().unwrap());
            }
        }
        // The recorded live/deleted trials have 20 scans, below the production
        // volume floor. Lower eligibility only in this unit test, not growth gates.
        build(
            &c,
            &(0, u64::MAX),
            Policy {
                minimum_scan_starts: 10.0,
                ..Default::default()
            },
        )
    };
    let nested = make("compact_repeat_2", "compact_repeat_5");
    assert!(!nested
        .hints
        .iter()
        .any(|h| h.assessment_scope == "instance"));
    let live = make("scan_initial_2000", "scan_live_8000");
    let deleted = make("scan_initial_2000", "scan_deleted_6000_live_2000");
    assert_eq!(
        serde_json::to_value(&live).unwrap(),
        serde_json::to_value(&deleted).unwrap()
    );
    let scan = live
        .hints
        .iter()
        .find(|h| h.assessment_scope == "instance")
        .unwrap();
    assert!(scan.mechanisms.is_empty());
    assert!(scan
        .alternative_explanations
        .iter()
        .any(|s| s.contains("wider rows")));
    let projection = make("chain_head_projection", "chain_full_projection");
    let h = projection
        .hints
        .iter()
        .find(|h| h.assessment_scope == CONTINUATION_SCOPE)
        .unwrap();
    assert!(h.evidence[0].recent > 1.0); // Ratio is not bounded to a chained-row percentage.
    assert!(h.mechanisms.is_empty());
    assert!(h
        .alternative_explanations
        .iter()
        .any(|s| s.contains("necessary row continuation")));
}

#[test]
fn reviewed_hourly_lab_replay_when_requested() {
    let Ok(path) = std::env::var("JASMIN_HINT_REVIEW_INPUT") else {
        return;
    };
    let text = std::fs::read_to_string(&path).unwrap();
    let c = crate::awr::load_awrs_collection_from_json_str(&text).unwrap();
    let before = serde_json::to_value(&c).unwrap();
    let mut r = run(&c);
    enrich_with_plans(&mut r, &c, path.trim_end_matches(".json"));
    assert_eq!(before, serde_json::to_value(&c).unwrap());
    let chain = r
        .hints
        .iter()
        .find(|h| h.assessment_scope == "sql:9pq1t4pj14h6p")
        .unwrap();
    assert!(chain.comparisons.len() > 10);
    assert!(chain.comparisons.iter().all(|a| a.work.baseline < 6.0));
    let head = r
        .hints
        .iter()
        .find(|h| h.assessment_scope == "sql:9tskgh4xpvaj5")
        .unwrap();
    assert_eq!(head.assessment_status, "recurrent_work_inflation");
    assert!(head.comparisons.iter().all(|a| a.cost.is_none()));
    let scan = r
        .hints
        .iter()
        .find(|h| h.assessment_scope == "instance")
        .unwrap();
    assert!(scan
        .possibly_affected_segments
        .iter()
        .any(|s| s.segment.object_name == "ERP_SPARSE"));
    assert!(!scan
        .possibly_affected_segments
        .iter()
        .any(|s| s.segment.object_name == "ERP_DENSE"));
    assert!(scan
        .segment_context
        .iter()
        .any(|s| s.segment.object_name == "ERP_DENSE"));
    assert_eq!(
        scan.episode_status,
        "historical_growth_later_within_reference"
    );
    assert!(r
        .hints
        .iter()
        .any(|h| h.assessment_scope == CONTINUATION_SCOPE));
    let sparse = r
        .hints
        .iter()
        .find(|h| h.assessment_scope == "sql:9paxwp1pabugh")
        .unwrap();
    assert!(sparse.mechanisms.is_empty());
    assert!(sparse.evidence.iter().all(|e| e.metric != CONTINUED));
    let packed = r
        .hints
        .iter()
        .find(|h| h.assessment_scope == "sql:bsvuf7rf4tycv")
        .unwrap();
    assert!(!packed
        .limitations
        .iter()
        .any(|s| s.contains("Short baseline")));
    let out = std::env::var("JASMIN_HINT_REVIEW_OUTPUT").unwrap();
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(
        format!("{out}/performance_hints.json"),
        serde_json::to_string_pretty(&r).unwrap(),
    )
    .unwrap();
    std::fs::write(format!("{out}/performance_hints.html"), render_html(&r)).unwrap();
    eprintln!(
        "review replay: {} windows, {} hints; CHAIN {} comparisons; HEAD {} comparisons",
        c.awrs.len(),
        r.hints.len(),
        chain.comparisons.len(),
        head.comparisons.len()
    );
}

#[test]
fn explicit_time_reference_includes_last_baseline_window_without_gets() {
    let mut c = synthetic(8);
    grow_sql(&mut c, &[4, 5]);
    c.awrs[3].sql_gets.clear();
    let r = build(
        &c,
        &(0, 99),
        Policy {
            baseline_snap_range: Some((1, 5)),
            ..Default::default()
        },
    );
    let h = sql_hint(&r).unwrap();
    assert_eq!(h.comparisons[0].work.baseline_observations, 3);
    assert_eq!(
        h.comparisons[0]
            .cost
            .as_ref()
            .unwrap()
            .baseline_observations,
        4
    );
    assert!(h.comparisons[0].cost_baseline_intervals.contains(&(4, 5)));
}
