use super::model::{NmonAggregateWindow, NmonAggregates, NmonPeakPeriod, NmonStatistics};

const TOP_PERIODS_PER_DURATION: usize = 3;

pub fn build_peak_periods(aggregates: &NmonAggregates) -> Vec<NmonPeakPeriod> {
    let mut peaks = Vec::new();
    append_window(&mut peaks, &aggregates.five_minutes);
    append_window(&mut peaks, &aggregates.fifteen_minutes);
    append_window(&mut peaks, &aggregates.one_hour);
    peaks.sort_by(|left, right| {
        left.metric_key
            .cmp(&right.metric_key)
            .then(left.duration_seconds.cmp(&right.duration_seconds))
            .then_with(|| {
                right
                    .statistics
                    .average
                    .unwrap_or(f64::NEG_INFINITY)
                    .total_cmp(&left.statistics.average.unwrap_or(f64::NEG_INFINITY))
            })
            .then(left.start.cmp(&right.start))
    });
    peaks
}

fn append_window(output: &mut Vec<NmonPeakPeriod>, window: &NmonAggregateWindow) {
    for (metric_key, series) in &window.metrics {
        let mut candidates = series
            .average
            .iter()
            .enumerate()
            .filter_map(|(index, average)| average.map(|average| (index, average)))
            .collect::<Vec<_>>();
        candidates.sort_by(|(left_index, left), (right_index, right)| {
            right.total_cmp(left).then(left_index.cmp(right_index))
        });
        for (index, _) in candidates.into_iter().take(TOP_PERIODS_PER_DURATION) {
            let Some(start) = window.starts.get(index) else {
                continue;
            };
            let end = chrono::NaiveDateTime::parse_from_str(start, "%Y-%m-%dT%H:%M:%S")
                .ok()
                .and_then(|start| {
                    start.checked_add_signed(chrono::Duration::seconds(
                        window.duration_seconds as i64,
                    ))
                })
                .map(|end| end.format("%Y-%m-%dT%H:%M:%S").to_string())
                .unwrap_or_else(|| start.clone());
            output.push(NmonPeakPeriod {
                metric_key: metric_key.clone(),
                duration_seconds: window.duration_seconds,
                start: start.clone(),
                end,
                statistics: NmonStatistics {
                    count: series.count.get(index).copied().unwrap_or_default(),
                    min: series.min.get(index).copied().flatten(),
                    average: series.average.get(index).copied().flatten(),
                    p95: series.p95.get(index).copied().flatten(),
                    max: series.max.get(index).copied().flatten(),
                    ..Default::default()
                },
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nmon::model::NmonAggregateSeries;
    use std::collections::BTreeMap;

    #[test]
    fn selects_sustained_non_overlapping_top_buckets_deterministically() {
        let window = NmonAggregateWindow {
            duration_seconds: 300,
            starts: (0..5)
                .map(|index| format!("2026-09-10T10:{:02}:00", index * 5))
                .collect(),
            metrics: BTreeMap::from([(
                "cpu_all.user_pct".to_string(),
                NmonAggregateSeries {
                    count: vec![10; 5],
                    min: vec![Some(1.0); 5],
                    average: vec![Some(1.0), Some(9.0), Some(8.0), Some(7.0), Some(6.0)],
                    p95: vec![Some(1.0); 5],
                    max: vec![Some(1.0); 5],
                },
            )]),
        };
        let peaks = build_peak_periods(&NmonAggregates {
            five_minutes: window,
            ..Default::default()
        });
        assert_eq!(peaks.len(), 3);
        assert_eq!(peaks[0].statistics.average, Some(9.0));
        assert_eq!(peaks[1].statistics.average, Some(8.0));
        assert_eq!(peaks[2].statistics.average, Some(7.0));
    }
}
