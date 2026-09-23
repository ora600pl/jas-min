use super::model::NmonDataset;
use serde_json::{json, Value};

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1_000;

pub fn configuration(dataset: &NmonDataset) -> Value {
    json!({
        "schema_version": dataset.schema_version,
        "metadata": dataset.metadata,
        "capture": dataset.capture,
        "diagnostics": {
            "warning_count": dataset.diagnostics.warnings.len(),
            "duplicate_samples": dataset.diagnostics.duplicate_samples,
            "overlap_count": dataset.diagnostics.overlaps.len(),
            "gap_count": dataset.diagnostics.gaps.len(),
            "unsupported_sections": dataset.diagnostics.unsupported_sections,
        }
    })
}

pub fn overview(dataset: &NmonDataset) -> Value {
    let selected = [
        "cpu_all.user_pct",
        "cpu_all.sys_pct",
        "cpu_all.wait_pct",
        "cpu_all.busy",
        "lpar.physicalcpu",
        "lpar.entitlement_utilization_pct",
        "process.runnable",
        "vm.pgin",
        "vm.pgout",
        "vm.pgsin",
        "vm.pgsout",
    ];
    let summaries = selected
        .iter()
        .filter_map(|key| {
            dataset
                .summaries
                .get(*key)
                .map(|summary| ((*key).to_string(), json!(summary)))
        })
        .collect::<serde_json::Map<_, _>>();
    let disk_devices = dataset
        .metrics
        .values()
        .filter(|metric| metric.domain == "disk")
        .filter_map(|metric| metric.entity.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let network_interfaces = dataset
        .metrics
        .values()
        .filter(|metric| metric.domain == "network")
        .filter_map(|metric| metric.entity.clone())
        .collect::<std::collections::BTreeSet<_>>();
    json!({
        "schema_version": dataset.schema_version,
        "host": dataset.metadata.host,
        "lpar": dataset.metadata.lpar,
        "memory": dataset.metadata.memory,
        "capture": dataset.capture,
        "metric_count": dataset.metrics.len(),
        "disk_devices": disk_devices,
        "network_interfaces": network_interfaces,
        "selected_summaries": summaries,
        "next_steps": [
            "list_host_metrics",
            "get_host_metric_summary",
            "get_host_peak_periods",
            "get_host_metric_time_series"
        ]
    })
}

pub fn list_metrics(dataset: &NmonDataset, args: &Value) -> Value {
    let domain = args.get("domain").and_then(Value::as_str);
    let entity = args.get("entity").and_then(Value::as_str);
    let section = args.get("section").and_then(Value::as_str);
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = bounded_limit(args);
    let rows = dataset
        .metrics
        .iter()
        .filter(|(_, metric)| domain.is_none_or(|value| metric.domain == value))
        .filter(|(_, metric)| entity.is_none_or(|value| metric.entity.as_deref() == Some(value)))
        .filter(|(_, metric)| section.is_none_or(|value| metric.section == value))
        .map(|(key, metric)| {
            json!({
                "metric_key": key,
                "domain": metric.domain,
                "entity": metric.entity,
                "section": metric.section,
                "source_name": metric.source_name,
                "source_label": metric.source_label,
                "unit": metric.unit,
                "source": metric.source,
                "summary": dataset.summaries.get(key),
            })
        })
        .collect::<Vec<_>>();
    let total = rows.len();
    json!({
        "total": total,
        "offset": offset,
        "limit": limit,
        "metrics": rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>()
    })
}

pub fn metric_summary(dataset: &NmonDataset, args: &Value) -> Value {
    let Some(key) = args.get("metric_key").and_then(Value::as_str) else {
        return error("INVALID_ARGUMENT", "metric_key is required");
    };
    let Some(metric) = dataset.metrics.get(key) else {
        return error("METRIC_NOT_FOUND", &format!("unknown NMON metric '{key}'"));
    };
    json!({
        "metric_key": key,
        "metadata": {
            "domain": metric.domain,
            "entity": metric.entity,
            "section": metric.section,
            "source_name": metric.source_name,
            "source_label": metric.source_label,
            "unit": metric.unit,
            "source": metric.source,
        },
        "summary": dataset.summaries.get(key),
        "peak_period_count": dataset.peak_periods.iter().filter(|peak| peak.metric_key == key).count(),
    })
}

pub fn peak_periods(dataset: &NmonDataset, args: &Value) -> Value {
    let metric_key = args.get("metric_key").and_then(Value::as_str);
    let duration = args.get("duration_seconds").and_then(Value::as_u64);
    let limit = bounded_limit(args);
    let rows = dataset
        .peak_periods
        .iter()
        .filter(|peak| metric_key.is_none_or(|key| peak.metric_key == key))
        .filter(|peak| duration.is_none_or(|seconds| peak.duration_seconds == seconds))
        .take(limit)
        .collect::<Vec<_>>();
    json!({
        "metric_key": metric_key,
        "duration_seconds": duration,
        "limit": limit,
        "peak_periods": rows,
    })
}

pub fn time_series(dataset: &NmonDataset, args: &Value) -> Value {
    let Some(key) = args.get("metric_key").and_then(Value::as_str) else {
        return error("INVALID_ARGUMENT", "metric_key is required");
    };
    let Some(metric) = dataset.metrics.get(key) else {
        return error("METRIC_NOT_FOUND", &format!("unknown NMON metric '{key}'"));
    };
    let from = args.get("from").and_then(Value::as_str);
    let to = args.get("to").and_then(Value::as_str);
    let resolution = args
        .get("resolution")
        .and_then(Value::as_str)
        .unwrap_or("raw");
    let limit = bounded_limit(args);
    if resolution == "raw" {
        let rows = dataset
            .timestamps
            .iter()
            .zip(metric.values.iter())
            .filter(|(timestamp, _)| in_range(timestamp, from, to))
            .filter_map(|(timestamp, value)| {
                value.map(|value| json!({"timestamp": timestamp, "value": value}))
            })
            .take(limit)
            .collect::<Vec<_>>();
        return json!({
            "metric_key": key,
            "resolution": "raw",
            "unit": metric.unit,
            "limit": limit,
            "samples": rows,
        });
    }
    let window = match resolution {
        "5m" => &dataset.aggregates.five_minutes,
        "15m" => &dataset.aggregates.fifteen_minutes,
        "1h" => &dataset.aggregates.one_hour,
        _ => return error("INVALID_ARGUMENT", "resolution must be raw, 5m, 15m, or 1h"),
    };
    let Some(series) = window.metrics.get(key) else {
        return error(
            "AGGREGATES_NOT_FOUND",
            &format!("no aggregates for '{key}'"),
        );
    };
    let rows = window
        .starts
        .iter()
        .enumerate()
        .filter(|(_, start)| in_range(start, from, to))
        .map(|(index, start)| {
            json!({
                "start": start,
                "duration_seconds": window.duration_seconds,
                "count": series.count.get(index).copied().unwrap_or_default(),
                "min": series.min.get(index).copied().flatten(),
                "average": series.average.get(index).copied().flatten(),
                "p95": series.p95.get(index).copied().flatten(),
                "max": series.max.get(index).copied().flatten(),
            })
        })
        .take(limit)
        .collect::<Vec<_>>();
    json!({
        "metric_key": key,
        "resolution": resolution,
        "unit": metric.unit,
        "limit": limit,
        "buckets": rows,
    })
}

pub fn disk_validation(dataset: &NmonDataset, args: &Value) -> Value {
    let device = args.get("device").and_then(Value::as_str);
    let rows = dataset
        .metrics
        .iter()
        .filter(|(_, metric)| metric.domain == "disk")
        .filter(|(_, metric)| device.is_none_or(|value| metric.entity.as_deref() == Some(value)))
        .map(|(key, metric)| {
            json!({
                "device": metric.entity,
                "metric": metric.section,
                "metric_key": key,
                "source_label": metric.source_label,
                "unit": metric.unit,
                "statistics": dataset.summaries.get(key),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "device": device,
        "rows": rows,
        "note": "Metric names are the original NMON section identities for direct comparison with NMONVisualizer."
    })
}

fn bounded_limit(args: &Value) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT)
}

fn in_range(timestamp: &str, from: Option<&str>, to: Option<&str>) -> bool {
    from.is_none_or(|from| timestamp >= from) && to.is_none_or(|to| timestamp <= to)
}

fn error(code: &str, message: &str) -> Value {
    json!({"error": message, "error_code": code})
}
