use super::*;
use clap::Parser;

fn select(count: usize, tied: bool, reverse: bool) -> TopStats {
    let mut awr = AWR::default();
    awr.snap_info.begin_snap_id = 1;
    awr.snap_info.end_snap_id = 2;
    awr.snap_info.begin_snap_time = "2026-09-25 00:00:00".into();
    awr.snap_info.end_snap_time = "2026-09-25 01:00:00".into();
    awr.load_profile = vec![
        LoadProfile {
            stat_name: "DB time".into(),
            per_second: 10.0,
            ..Default::default()
        },
        LoadProfile {
            stat_name: "DB CPU".into(),
            per_second: 1.0,
            ..Default::default()
        },
    ];
    for j in 0..count {
        let i = if reverse { count - j - 1 } else { j };
        let id = format!("sql{i:02}");
        let seconds = if tied { 0.1 } else { (i + 1) as f64 / 100.0 };
        awr.sql_cpu_time.insert(
            id.clone(),
            SQLCPUTime {
                sql_id: id.clone(),
                cpu_time_s: seconds,
                ..Default::default()
            },
        );
        awr.sql_elapsed_time.push(crate::awr::SQLElapsedTime {
            sql_id: id.clone(),
            elapsed_time_s: seconds,
            ..Default::default()
        });
        let event = WaitEvents {
            event: id,
            total_wait_time_s: seconds,
            ..Default::default()
        };
        awr.foreground_wait_events.push(event.clone());
        awr.background_wait_events.push(event);
    }
    let args = Args::parse_from(["jas-min", "--quiet"]);
    let path = std::env::temp_dir().join(format!(
        "jasmin-selection-{}-{count}-{tied}-{reverse}.log",
        std::process::id()
    ));
    let result = find_top_stats(
        &vec![awr],
        0.666,
        0.0,
        &(0, 10),
        path.to_str().unwrap(),
        &args,
        &mut ReportForAI::default(),
    );
    std::fs::remove_file(path).unwrap();
    result
}

#[test]
fn top_selection_preserves_fractional_seconds_and_small_sections() {
    for count in [0, 1, 5, 6, 10, 11] {
        for reverse in [false, true] {
            let selected = select(count, false, reverse);
            let expected_sql: Vec<_> = (count.saturating_sub(5)..count)
                .map(|i| format!("sql{i:02}"))
                .collect();
            let expected_waits: Vec<_> = (count.saturating_sub(10)..count)
                .map(|i| format!("sql{i:02}"))
                .collect();
            assert_eq!(
                selected.sqls_cpu.keys().cloned().collect::<Vec<_>>(),
                expected_sql
            );
            assert_eq!(
                selected.sqls.keys().cloned().collect::<Vec<_>>(),
                expected_sql
            );
            assert_eq!(
                selected.events.keys().cloned().collect::<Vec<_>>(),
                expected_waits
            );
            assert_eq!(
                selected.bgevents.keys().cloned().collect::<Vec<_>>(),
                expected_waits
            );
        }
    }
}

#[test]
fn top_selection_resolves_ties_before_truncation() {
    for i in 0..32 {
        let selected = select(11, true, i % 2 == 0);
        assert_eq!(
            selected.sqls_cpu.keys().cloned().collect::<Vec<_>>(),
            (0..5).map(|i| format!("sql{i:02}")).collect::<Vec<_>>()
        );
        assert_eq!(
            selected.sqls.keys().cloned().collect::<Vec<_>>(),
            selected.sqls_cpu.keys().cloned().collect::<Vec<_>>()
        );
        assert_eq!(
            selected.events.keys().cloned().collect::<Vec<_>>(),
            (0..10).map(|i| format!("sql{i:02}")).collect::<Vec<_>>()
        );
    }
}
