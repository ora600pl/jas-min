use super::model::NmonStatistics;

pub fn statistics(values: impl IntoIterator<Item = f64>) -> NmonStatistics {
    let mut values = values
        .into_iter()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    let count = values.len();
    if count == 0 {
        return NmonStatistics::default();
    }
    let sum = values.iter().sum::<f64>();
    let average = sum / count as f64;
    let variance = values
        .iter()
        .map(|value| {
            let delta = value - average;
            delta * delta
        })
        .sum::<f64>()
        / count as f64;
    NmonStatistics {
        count,
        min: values.first().copied(),
        average: Some(average),
        median: percentile(&values, 0.50),
        p95: percentile(&values, 0.95),
        p99: percentile(&values, 0.99),
        max: values.last().copied(),
        standard_deviation: Some(variance.sqrt()),
    }
}

fn percentile(sorted: &[f64], quantile: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    if sorted.len() == 1 {
        return sorted.first().copied();
    }
    let position = quantile.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        Some(sorted[lower])
    } else {
        let fraction = position - lower as f64;
        Some(sorted[lower] + (sorted[upper] - sorted[lower]) * fraction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates_statistics_and_ignores_missing_values() {
        let stats = statistics([1.0, 2.0, 3.0, 4.0]);
        assert_eq!(stats.count, 4);
        assert_eq!(stats.min, Some(1.0));
        assert_eq!(stats.average, Some(2.5));
        assert_eq!(stats.median, Some(2.5));
        assert_eq!(stats.max, Some(4.0));
        assert!((stats.p95.unwrap_or_default() - 3.85).abs() < 1e-9);
        assert!((stats.p99.unwrap_or_default() - 3.97).abs() < 1e-9);
        assert!((stats.standard_deviation.unwrap_or_default() - 1.1180339887).abs() < 1e-9);
        assert_eq!(statistics(Vec::<f64>::new()).count, 0);
    }
}
