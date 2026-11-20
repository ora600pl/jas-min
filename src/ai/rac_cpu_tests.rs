use super::*;
use crate::awr::{DBInstance, SQLCPUTime, SnapInfo, TimeModelStats};
use std::collections::HashMap;

const MONITOR: &str = "0000000000001";
const SELECTED: &str = "0000000000002";
const OTHER: &str = "0000000000003";

fn request() -> Request {
    Request {
        project_ids: vec!["project-1".into(), "project-2".into()],
        monitor_module_prefix: "MONITOR".into(),
        selected_sql_ids: vec![SELECTED.into()],
        sql_reduction_pct: 50.0,
    }
}

fn snapshot(id: u64, begin: &str, end: &str, db: f64, monitor: f64, selected: f64) -> AWR {
    let mut a = AWR {
        snap_info: SnapInfo {
            begin_snap_id: id,
            end_snap_id: id + 1,
            begin_snap_time: format!("2025-01-01 {begin}"),
            end_snap_time: format!("2025-01-01 {end}"),
        },
        time_model_stats: vec![TimeModelStats {
            stat_name: "DB CPU".into(),
            time_s: db,
            ..Default::default()
        }],
        ..Default::default()
    };
    for (sql_id, module, cpu) in [
        (MONITOR, "monitor:collector", monitor),
        (SELECTED, "application", selected),
        (OTHER, "application", 1.0),
    ] {
        if cpu > 0.0 {
            a.sql_cpu_time.insert(
                sql_id.into(),
                SQLCPUTime {
                    sql_id: sql_id.into(),
                    sql_module: module.into(),
                    cpu_time_s: cpu,
                    ..Default::default()
                },
            );
        }
    }
    a
}

fn source(n: u8, awrs: Vec<AWR>) -> AWRSCollection {
    AWRSCollection {
        db_instance_information: DBInstance {
            db_id: 1,
            instance_num: n,
            rac: "YES".into(),
            ..Default::default()
        },
        initialization_parameters: HashMap::new(),
        awrs,
        sql_text: HashMap::from([
            (
                MONITOR.to_string(),
                "/* sample */ select 1 from dual".into(),
            ),
            (
                SELECTED.to_string(),
                "-- sample\nwith q as (select 1 as x from dual) select x from q".into(),
            ),
        ]),
        nmon: None,
    }
}

fn inputs() -> (AWRSCollection, AWRSCollection) {
    (
        source(
            1,
            vec![
                snapshot(1, "00:00:00", "00:01:40", 1000., 400., 200.),
                snapshot(2, "00:01:40", "00:03:20", 200., 0., 0.),
            ],
        ),
        source(
            2,
            vec![
                snapshot(1, "00:00:02", "00:02:02", 480., 0., 120.),
                snapshot(2, "00:02:02", "00:03:22", 800., 0., 0.),
            ],
        ),
    )
}

fn compute(a: &AWRSCollection, b: &AWRSCollection) -> ScenarioResult<Value> {
    calculate(&request(), &[("project-1", a), ("project-2", b)])
}

#[test]
fn cluster_peak_uses_coincident_instance_rates_and_model_can_move_the_peak() {
    let (a, mut b) = inputs();
    b.awrs.reverse();
    let result = compute(&a, &b).unwrap();
    assert_eq!(result["paired_intervals"], 2);
    assert_eq!(result["observed_peak"]["active_cpu"], 14.0);
    assert_eq!(result["scenario_peak"]["active_cpu"], 12.0);
    assert_ne!(
        result["scenario_peak"]["begin"],
        result["observed_peak"]["begin"]
    );
    assert_eq!(result["instances"][0]["observed_peak"]["active_cpu"], 10.0);
    assert_eq!(result["instances"][1]["observed_peak"]["active_cpu"], 10.0);
    let rows = &result["intervals"];
    assert_eq!(rows[0]["scenario_active_cpu"], 8.5);
    assert_eq!(rows[0]["instances"][1]["wall_seconds"], 120.);
    assert_eq!(rows[1]["instances"][1]["wall_seconds"], 80.);
    assert_eq!(rows[0]["instances"][0]["wall_seconds"], 100.);
    assert_eq!(rows[0]["saved_cpu_seconds"], 560.);
    assert_eq!(result["saved_cpu_hours"], 560.0 / 3600.0);
    assert_eq!(result["monitor_cpu_hours"], 400.0 / 3600.0);
    assert_eq!(result["selected_sql_cpu_hours"], 320.0 / 3600.0);
    assert_eq!(rows[1]["instances"][0]["selected_top_rows"], 0);
    assert_eq!(rows[1]["instances"][0]["selected_sql_cpu_seconds"], 0.0);
    assert_eq!(result, compute(&a, &b).unwrap());
}

#[test]
fn rejects_discontinuous_duplicate_missing_and_misaligned_intervals() {
    let (a, b) = inputs();
    let mut c = b.clone();
    c.awrs.pop();
    assert!(compute(&a, &c).is_err());
    c = b.clone();
    c.awrs.push(c.awrs[0].clone());
    assert!(compute(&a, &c).is_err());
    c = b.clone();
    c.awrs[0].snap_info.begin_snap_time = "2025-01-01 00:01:01".into();
    assert!(compute(&a, &c).is_err());
    c = b.clone();
    c.awrs[1].snap_info.begin_snap_time = "2025-01-01 00:02:03".into();
    assert!(compute(&a, &c).is_err());
}

#[test]
fn rejects_duplicate_instances_and_cross_database_sources() {
    let (a, mut b) = inputs();
    b.db_instance_information.instance_num = 1;
    assert!(compute(&a, &b).is_err());
    b.db_instance_information.instance_num = 2;
    b.db_instance_information.db_id = 2;
    assert!(compute(&a, &b).is_err());
    b.db_instance_information.db_id = 1;
    b.db_instance_information.rac = "NO".into();
    assert!(compute(&a, &b).is_err());
}

#[test]
fn rejects_unknown_time_model_or_top_capture_and_unseen_selectors() {
    let (a, b) = inputs();
    let mut c = a.clone();
    c.awrs[0]
        .data_availability
        .insert("time_model_stats".into(), false);
    assert!(compute(&c, &b).is_err());
    c = a.clone();
    c.awrs[0]
        .data_availability
        .insert("sql_cpu_time".into(), false);
    assert!(compute(&c, &b).is_err());
    c = a.clone();
    c.awrs[0].sql_cpu_time.clear();
    assert!(compute(&c, &b).is_err());
    let mut args = request();
    args.selected_sql_ids = vec!["0000000000004".into()];
    assert!(calculate(&args, &[("project-1", &a), ("project-2", &b)]).is_err());
    args = request();
    args.monitor_module_prefix = "ABSENT".into();
    assert!(calculate(&args, &[("project-1", &a), ("project-2", &b)]).is_err());
}

#[test]
fn rejects_overlapping_attribution_procedural_parents_and_impossible_cpu() {
    let (a, b) = inputs();
    let mut c = a.clone();
    c.awrs[0].sql_cpu_time.get_mut(SELECTED).unwrap().sql_module = "monitor".into();
    assert!(compute(&c, &b).is_err());
    c = a.clone();
    c.sql_text
        .insert(SELECTED.into(), "/* select */ begin null; end;".into());
    assert!(compute(&c, &b).is_err());
    c = a.clone();
    c.awrs[0].time_model_stats[0].time_s = 450.;
    assert!(compute(&c, &b).is_err());
}

#[test]
fn request_normalizes_selectors_and_rejects_duplicates_and_invalid_fractions() {
    let mut v = json!(request());
    v["project_ids"] = json!(["project-2", "project-1"]);
    v["analysis_id"] = "analysis-1".into();
    v["monitor_module_prefix"] = " monitor ".into();
    let canonical = Request::parse(v.as_object().unwrap()).unwrap();
    assert_eq!(json!(canonical), json!(request()));
    v["sql_reduction_pct"] = 101.into();
    assert!(Request::parse(v.as_object().unwrap()).is_err());
    v["sql_reduction_pct"] = 0.into();
    v["selected_sql_ids"] = json!(["000000000000a", "000000000000A"]);
    assert!(Request::parse(v.as_object().unwrap()).is_err());
}
