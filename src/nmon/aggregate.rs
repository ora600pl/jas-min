use super::model::{NmonAggregateSeries, NmonAggregateWindow, NmonAggregates, NmonMetricSeries};
use super::stats::statistics;
use chrono::{DateTime, NaiveDateTime, Utc};
use std::collections::BTreeMap;

const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

pub fn build_aggregates(
    timestamps: &[String],
    metrics: &BTreeMap<String, NmonMetricSeries>,
) -> NmonAggregates {
    NmonAggregates {
        five_minutes: aggregate_window(timestamps, metrics, 5 * 60),
        fifteen_minutes: aggregate_window(timestamps, metrics, 15 * 60),
        one_hour: aggregate_window(timestamps, metrics, 60 * 60),
    }
}

fn aggregate_window(
    timestamps: &[String],
    metrics: &BTreeMap<String, NmonMetricSeries>,
    window_seconds: i64,
) -> NmonAggregateWindow {
    let mut groups = BTreeMap::<i64, Vec<usize>>::new();
    for (index, timestamp) in timestamps.iter().enumerate() {
        let Ok(timestamp) = NaiveDateTime::parse_from_str(timestamp, TIMESTAMP_FORMAT) else {
            continue;
        };
        let epoch = timestamp.and_utc().timestamp();
        let bucket_start = epoch.div_euclid(window_seconds) * window_seconds;
        groups.entry(bucket_start).or_default().push(index);
    }
    let starts = groups
        .keys()
        .filter_map(|epoch| DateTime::<Utc>::from_timestamp(*epoch, 0))
        .map(|timestamp| timestamp.naive_utc().format(TIMESTAMP_FORMAT).to_string())
        .collect::<Vec<_>>();
    let sample_indices = groups.into_values().collect::<Vec<_>>();
    let metrics = metrics
        .iter()
        .map(|(key, metric)| {
            let mut series = NmonAggregateSeries::default();
            for indices in &sample_indices {
                let stats = statistics(
                    indices
                        .iter()
                        .filter_map(|index| metric.values.get(*index).copied().flatten()),
                );
                series.count.push(stats.count);
                series.min.push(stats.min);
                series.average.push(stats.average);
                series.p95.push(stats.p95);
                series.max.push(stats.max);
            }
            (key.clone(), series)
        })
        .collect();
    NmonAggregateWindow {
        duration_seconds: window_seconds as u64,
        starts,
        metrics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nmon::model::{NmonMetricSeries, NmonMetricSource};

    #[test]
    fn buckets_on_clock_boundaries_for_all_required_windows() {
        let timestamps = vec![
            "2026-09-10T10:04:30".to_string(),
            "2026-09-10T10:05:00".to_string(),
            "2026-09-10T10:14:30".to_string(),
            "2026-09-10T10:15:00".to_string(),
            "2026-09-10T11:00:00".to_string(),
        ];
        let metric = NmonMetricSeries {
            section: "CPU_ALL".to_string(),
            source_name: "User%".to_string(),
            source_label: "User%".to_string(),
            domain: "cpu_all".to_string(),
            entity: None,
            unit: Some("%".to_string()),
            source: NmonMetricSource::Observed,
            values: vec![Some(1.0), Some(2.0), None, Some(4.0), Some(5.0)],
        };
        let result = build_aggregates(
            &timestamps,
            &BTreeMap::from([("cpu_all.user_pct".to_string(), metric)]),
        );
        assert_eq!(result.five_minutes.starts.len(), 5);
        assert_eq!(result.fifteen_minutes.starts.len(), 3);
        assert_eq!(result.one_hour.starts.len(), 2);
        assert_eq!(result.five_minutes.starts[0], "2026-09-10T10:00:00");
        assert_eq!(result.five_minutes.starts[1], "2026-09-10T10:05:00");
        assert_eq!(result.five_minutes.metrics["cpu_all.user_pct"].count[1], 1);
        assert_eq!(result.five_minutes.metrics["cpu_all.user_pct"].count[2], 0);
    }
}
