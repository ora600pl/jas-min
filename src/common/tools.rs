use crate::awr::GetStats;
use crate::debug_note;
use base64::{engine::general_purpose, Engine as _};
use chrono::Local;
use html_escape::{encode_double_quoted_attribute, encode_text};
use ndarray::{iter, Array1, Array2};
use ndarray_stats::histogram::Grid;
use ndarray_stats::interpolate::Linear;
use ndarray_stats::{CorrelationExt, QuantileExt};
use noisy_float::types::N64;
use prettytable::{Cell, Row, Table};
use pulldown_cmark::{html, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use regex::Regex;
use serde::Serialize;
use serde_json::{Number, Value};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs::File;
use std::io::{stdout, BufRead, BufReader, BufWriter, Write as Write2};
use std::{collections::HashMap, collections::HashSet, env, fs, path::Path};
use tokio::sync::oneshot;

pub(crate) use crate::report::html::*;

/// Bonferroni-corrected correlation significance threshold.
/// Returns the minimum |r| that is significant at family-wise alpha
/// after correcting for num_tests independent tests.
pub fn bonferroni_significance_threshold(num_tests: usize, alpha: f64, sample_size: usize) -> f64 {
    if num_tests == 0 || sample_size < 4 {
        return 0.5; // fallback
    }
    let corrected_alpha = alpha / num_tests as f64;
    // Use Fisher z-transform approximation
    let z = normal_quantile(1.0 - corrected_alpha / 2.0);
    let df = (sample_size as f64 - 2.0).max(1.0);
    // t = z (large sample approximation), r = t / sqrt(t^2 + df)
    let r_critical = z / (z * z + df).sqrt();
    r_critical
}

/// Abramowitz & Stegun approximation for normal quantile
fn normal_quantile(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    if p == 0.5 {
        return 0.0;
    }

    let (work_p, negate) = if p > 0.5 { (1.0 - p, false) } else { (p, true) };
    let t = (-2.0 * work_p.ln()).sqrt();
    let c0 = 2.515517;
    let c1 = 0.802853;
    let c2 = 0.010328;
    let d1 = 1.432788;
    let d2 = 0.189269;
    let d3 = 0.001308;
    let result = t - (c0 + c1 * t + c2 * t * t) / (1.0 + d1 * t + d2 * t * t + d3 * t * t * t);
    if negate {
        -result
    } else {
        result
    }
}

pub fn get_timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

//Calculate pearson correlation of 2 vectors and return simple result
pub fn pearson_correlation_2v(vec1: &Vec<f64>, vec2: &Vec<f64>) -> f64 {
    let rows: usize = 2;
    let cols: usize = vec1.len();

    let mut data: Vec<f64> = Vec::new();
    data.extend(vec1);
    data.extend(vec2);

    let a: ndarray::ArrayBase<ndarray::OwnedRepr<f64>, ndarray::Dim<[usize; 2]>> =
        Array2::from_shape_vec((rows, cols), data).unwrap();
    let crr = a.pearson_correlation().unwrap();

    crr.row(0)[1]
}

pub fn mean(data: Vec<f64>) -> Option<f64> {
    let sum: f64 = data.iter().sum::<f64>() as f64;
    let count: usize = data.len();

    match count {
        positive if positive > 0 => Some(sum / count as f64),
        _ => None,
    }
}

pub fn std_deviation(data: Vec<f64>) -> Option<f64> {
    match (mean(data.clone()), data.len()) {
        (Some(data_mean), count) if count > 0 => {
            let variance: f64 = data
                .iter()
                .map(|value| {
                    let diff: f64 = data_mean - (*value as f64);

                    diff * diff
                })
                .sum::<f64>()
                / count as f64;

            Some(variance.sqrt())
        }
        _ => None,
    }
}

pub fn median(data: &[f64]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut tmp = data.to_vec();
    let mid = tmp.len() / 2;
    tmp.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
    if tmp.len() % 2 == 1 {
        tmp[mid]
    } else {
        let lower_max = tmp[..mid].iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (lower_max + tmp[mid]) * 0.5
    }
}

pub fn mad(data: &[f64]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let med = median(data);
    let deviations: Vec<f64> = data.iter().map(|x| (x - med).abs()).collect();
    median(&deviations)
}

pub fn mad_with_median(data: &[f64], med: f64) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let deviations: Vec<f64> = data.iter().map(|x| (x - med).abs()).collect();
    median(&deviations)
}

pub fn get_safe_filename(name: String, category: String) -> String {
    // Replace invalid characters for filenames (e.g., slashes or spaces)
    let safe_event_name: String = name
        .replace("/", "_")
        .replace(" ", "_")
        .replace(":", "")
        .replace("*", "_");
    let mut file_name: String = String::new();
    if category == "fg".to_string() {
        file_name = format!("fg/fg_{}.html", safe_event_name);
    } else if category == "bg".to_string() {
        file_name = format!("bg/bg_{}.html", safe_event_name);
    } else if category == "inst_stat".to_string() {
        file_name = format!("stats/stat_{}.html", safe_event_name);
    }
    file_name
}

pub fn get_statistics(data: Vec<f64>) -> Option<GetStats> {
    if data.is_empty() {
        return None;
    }

    let samples = data.len() as u64;
    let arr = Array1::from(data);

    // Calculate basic stats using ndarray methods
    let min = *arr.min().unwrap();
    let max = *arr.max().unwrap();
    let mean = arr.mean().unwrap();
    let variance = arr.var(0.0);
    let std_dev = arr.std(0.0);

    // Convert to noisy_float for quantile calculations
    let mut arr_n64: Array1<N64> = arr.mapv(N64::new);

    // Calculate quartiles
    let q1 = arr_n64
        .quantile_axis_mut(ndarray::Axis(0), N64::new(0.25), &Linear)
        .unwrap()
        .into_scalar()
        .raw();

    let median = arr_n64
        .quantile_axis_mut(ndarray::Axis(0), N64::new(0.5), &Linear)
        .unwrap()
        .into_scalar()
        .raw();

    let q3 = arr_n64
        .quantile_axis_mut(ndarray::Axis(0), N64::new(0.75), &Linear)
        .unwrap()
        .into_scalar()
        .raw();

    // Calculate theoretical fence boundaries
    let iqr = q3 - q1;
    let lower_fence_boundary = q1 - 1.5 * iqr;
    let upper_fence_boundary = q3 + 1.5 * iqr;

    // Match Plotly's behavior: fences are the min/max data points within boundaries
    let lower_fence = arr
        .iter()
        .filter(|&&x| x >= lower_fence_boundary)
        .min_by(|a, b| a.partial_cmp(b).unwrap())
        .copied()
        .unwrap_or(min);

    let upper_fence = arr
        .iter()
        .filter(|&&x| x <= upper_fence_boundary)
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .copied()
        .unwrap_or(max);

    let round2 = |x: f64| (x * 100.0).round() / 100.0;

    Some(GetStats {
        samples,
        min: round2(min),
        lower_fence: round2(lower_fence),
        q1: round2(q1),
        mean: round2(mean),
        median: round2(median),
        q3: round2(q3),
        upper_fence: round2(upper_fence),
        max: round2(max),
        variance: round2(variance),
        std_dev: round2(std_dev),
    })
}

pub async fn spinning_beer(mut done: oneshot::Receiver<()>) {
    let frames = ["🍺", "🍻", "🍺", "🍻"];
    let mut i = 0;
    while done.try_recv().is_err() {
        print!("\r{}", frames[i % frames.len()]);
        stdout().flush().unwrap();
        i += 1;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    println!("\r✅ Got response!");
}

/// Rough token estimate: ~4 chars per token.
/// Works fine as a budget guardrail for JSON-heavy prompts.
pub fn estimate_tokens_from_str(s: &str) -> usize {
    (s.chars().count() + 3) / 4
}

pub fn round_json_floats(value: &mut Value, decimal_places: u32) {
    let factor = 10_f64.powi(decimal_places as i32);

    match value {
        Value::Array(values) => {
            for value in values {
                round_json_floats(value, decimal_places);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                round_json_floats(value, decimal_places);
            }
        }
        Value::Number(number) if number.is_f64() => {
            if let Some(float_value) = number.as_f64() {
                let mut rounded = (float_value * factor).round() / factor;
                if rounded == -0.0 {
                    rounded = 0.0;
                }
                if let Some(rounded_number) = Number::from_f64(rounded) {
                    *number = rounded_number;
                }
            }
        }
        _ => {}
    }
}

pub fn rounded_json_for_toon(mut value: Value) -> Value {
    round_json_floats(&mut value, 3);
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_summary_headings_and_explicit_finding_links_survive_conversion() {
        let html = render_markdown_html_document(
            "# Report\n\n## 1. Executive Summary\n\n**1. Cursor issue [high / high]** — Two waiters.\n\n**Mechanism:** Holder parsing.\n\n[Detail](#cursor-case)\n\n### Cursor evidence {#cursor-case}\n\nExact values.\n\n### Duplicate {#cursor-case}\n",
            "", "", HashMap::new(),
        );
        let document = scraper::Html::parse_document(&html);
        let select = |s| scraper::Selector::parse(s).unwrap();
        assert_eq!(document.select(&select("h3")).count(), 3);
        assert_eq!(document.select(&select("h3#cursor-case")).count(), 1);
        assert_eq!(document.select(&select("h3#cursor-case-2")).count(), 1);
        assert!(document
            .select(&select("p"))
            .any(|node| node.text().collect::<String>() == "Mechanism: Holder parsing."));
        assert!(!html.contains("p:has(> strong:first-child)"));
        assert!(html.contains("revealFragment"));
    }

    #[test]
    fn classic_navigation_links_only_existing_reports_without_iframes_or_placeholders() {
        let root = std::env::temp_dir().join(format!(
            "jas-min-classic-navigation-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(root.join("stats")).unwrap();
        std::fs::write(root.join("jasmin_main.html"), b"main").unwrap();
        std::fs::write(root.join("stats/jasmin_highlight.html"), b"profile").unwrap();

        let html = render_markdown_html_document(
            "# Oracle Performance Analysis\n\nBody.",
            "node-1.html_reports",
            &root.to_string_lossy(),
            HashMap::new(),
        );

        assert!(html.contains(
            "href=\"node-1.html_reports/jasmin_main.html\" target=\"_blank\" rel=\"noopener\""
        ));
        assert!(html.contains("node-1.html_reports/stats/jasmin_highlight.html"));
        assert!(!html.contains("jasmin_highlight2.html"));
        assert!(!html.contains("<iframe"));
        assert!(unresolved_report_placeholder_for_test(&html).is_none());

        std::fs::remove_file(root.join("stats/jasmin_highlight.html")).unwrap();
        std::fs::remove_file(root.join("jasmin_main.html")).unwrap();
        std::fs::remove_dir(root.join("stats")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn report_renderer_applies_readable_audit_layout_and_severity_classes() {
        let html = render_markdown_html_document(
            "# Oracle Performance Analysis\n\n## Wait Events\n\n### Cursor contention [critical / high]\n\nEvidence.\n\n| SQL ID | Finding |\n|---|---|\n| abc123 | Wide diagnostic evidence that remains scrollable |\n\n### Storage is healthy [informational / high]\n\nEvidence.\n\n## 11. Prioritized Actions and Mandatory Assessments\n\n- Fix the proven issue.\n\n### Mandatory Assessments\n\n- CPU pressure assessed.",
            "",
            "",
            HashMap::new(),
        );

        assert!(html.contains("<nav class=\"toc\" aria-label=\"Report contents\">"));
        assert!(html.contains("class=\"report-title\""));
        assert!(html.contains("class=\"section-title\""));
        assert!(html.contains("class=\"finding-title severity-critical\""));
        assert!(html.contains("class=\"finding-title severity-informational\""));
        assert!(html.contains("grid-template-columns: minmax(240px, 300px)"));
        assert!(html.contains("@media print"));
        assert!(html.contains(".actions-section-title + ul"));
        assert!(html.contains("class=\"section-title actions-section-title\""));
        assert!(html.contains("class=\"subsection-title assessment-list-title\""));
        assert!(html.contains("class=\"table-scroll\" tabindex=\"0\""));
        assert!(html.contains("overflow-x: auto"));
        assert!(html.contains("width: max-content"));
        assert!(html.contains("className = \"table-sort-button\""));
        assert!(html.contains("header.setAttribute(\"aria-sort\", \"none\")"));
        assert!(html.contains("tableSortNumber"));
        assert!(html.contains("tableSortDate"));
        assert!(!html.contains("#section-26 + ul"));
        assert!(html.contains("<header class=\"brand-banner\">"));
        assert!(html.contains("ORACLE PERFORMANCE EVIDENCE"));
        assert!(html.contains("data:image/png;base64,"));
        assert!(html.contains("--high: #c52228"));
        assert!(!html.contains("jasmin_LOGO_white.png"));
    }

    fn unresolved_report_placeholder_for_test(value: &str) -> Option<&'static str> {
        [
            "{load_profile}",
            "{load_profile2}",
            "{jasmin_main}",
            "{lp}",
            "{lp2}",
            "{jm}",
        ]
        .into_iter()
        .find(|placeholder| value.contains(placeholder))
    }

    #[test]
    fn round_json_floats_rounds_nested_float_values_only() {
        let mut value = json!({
            "float": 23.54664676448,
            "integer": 23,
            "text": "23.54664676448",
            "nested": [{ "negative_zero": -0.0001 }]
        });

        round_json_floats(&mut value, 3);

        assert_eq!(value["float"], json!(23.547));
        assert_eq!(value["integer"], json!(23));
        assert_eq!(value["text"], json!("23.54664676448"));
        assert_eq!(value["nested"][0]["negative_zero"], json!(0.0));
    }
}

/// Builds the "combined string" that we actually send (base prompt + capsule + input JSON).
pub fn estimate_request_tokens(
    base_user_prompt_str: &str,
    capsule_json_str: &str,
    input_json: &serde_json::Value,
) -> usize {
    let input_str =
        serde_json::to_string(&rounded_json_for_toon(input_json.clone())).unwrap_or_default();
    let combined = format!(
        "{}\n{}\nINPUT:\n{}",
        base_user_prompt_str, capsule_json_str, input_str
    );
    estimate_tokens_from_str(&combined)
}

/// Generic helper: given a sorted Vec<T>, find the maximum prefix length that fits into budget.
/// `wrap` is responsible for putting the slice into the JSON shape used by the section.
pub fn max_prefix_that_fits<T: Serialize>(
    items: &[T],
    base_user_prompt_str: &str,
    capsule_json_str: &str,
    budget_tokens: usize,
    wrap: impl Fn(&[T]) -> serde_json::Value,
) -> usize {
    if items.is_empty() || budget_tokens < 256 {
        return 0;
    }

    let fits = |k: usize| -> bool {
        if k == 0 {
            return true;
        }
        let v = wrap(&items[..k]);
        estimate_request_tokens(base_user_prompt_str, capsule_json_str, &v) <= budget_tokens
    };

    if !fits(1) {
        return 0;
    }

    let mut lo = 1usize;
    let mut hi = items.len();

    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    lo
}

/// Extracts table names from Oracle SQL text.
///
/// Handles: FROM, JOIN, INTO, UPDATE, MERGE INTO, DELETE [FROM]
pub fn extract_tables_from_sql(sql: &str) -> Vec<String> {
    let mut tables: HashSet<String> = HashSet::new();

    // Normalize: collapse whitespace, remove newlines
    let normalized = sql.replace('\n', " ").replace('\r', " ").replace('\t', " ");

    // Remove single-line comments (-- ...)
    let re_single_comment = Regex::new(r"--[^\n]*").unwrap();
    let normalized = re_single_comment.replace_all(&normalized, " ").to_string();

    // Remove multi-line comments (/* ... */)
    let re_multi_comment = Regex::new(r"/\*[\s\S]*?\*/").unwrap();
    let normalized = re_multi_comment.replace_all(&normalized, " ").to_string();

    // Remove string literals ('...')
    let re_strings = Regex::new(r"'[^']*'").unwrap();
    let normalized = re_strings.replace_all(&normalized, " ").to_string();

    // Collapse multiple spaces
    let re_spaces = Regex::new(r"\s+").unwrap();
    let normalized = re_spaces.replace_all(&normalized, " ").to_string();

    let upper = normalized.to_uppercase();

    // NOTE: DELETE supports optional FROM: DELETE table ... and DELETE FROM table ...
    let table_pattern = Regex::new(
        r"(?i)\b(?:FROM|JOIN|UPDATE|INTO|MERGE\s+INTO|DELETE(?:\s+FROM)?)\s+([A-Z_$#][A-Z0-9_$#]*(?:\.[A-Z_$#][A-Z0-9_$#]*)*)(?:\s+(?:AS\s+)?[A-Z_][A-Z0-9_]*)?"
    ).unwrap();

    // Pseudo-tables and system schemas to exclude
    let exclude: HashSet<&str> = [
        "DUAL",
        "SYS",
        "SYSTEM",
        "SELECT",
        "VALUES",
        "SET",
        "WHERE",
        "AND",
        "OR",
        "ON",
        "USING",
        "TABLE",
        "INDEX",
        "VIEW",
        "BEGIN",
        "END",
        "DECLARE",
        "EXCEPTION",
        "LOOP",
        "IF",
        "THEN",
        "ELSE",
        "ELSIF",
        "RETURN",
        "NULL",
        "NOT",
        "IN",
        "EXISTS",
        "PARTITION",
        "SUBPARTITION",
        "LATERAL",
        "XMLTABLE",
        "JSON_TABLE",
        // FOR UPDATE [SKIP LOCKED | NOWAIT | WAIT n] clause tokens
        "SKIP",
        "LOCKED",
        "NOWAIT",
        "WAIT",
        "FOR",
    ]
    .iter()
    .cloned()
    .collect();

    for cap in table_pattern.captures_iter(&upper) {
        let table_name = cap[1].trim().to_string();

        let base_name = table_name.split('.').last().unwrap_or(&table_name);
        if exclude.contains(base_name) {
            continue;
        }
        if table_name.starts_with("SYS.") || table_name.starts_with("SYSTEM.") {
            continue;
        }
        if !table_name.contains('.') && table_name.len() <= 1 {
            continue;
        }

        tables.insert(table_name);
    }

    let mut result: Vec<String> = tables.into_iter().collect();
    result.sort();
    result
}

/// Given a set of SQL_IDs, the sql_text map, and a lookup function,
/// returns a deduplicated sorted list of table names found across all matching SQLs.
pub fn find_tables_for_sql_ids(
    sql_ids: &HashSet<String>,
    sql_text: &HashMap<String, String>,
) -> Vec<String> {
    let mut all_tables: HashSet<String> = HashSet::new();

    for sql_id in sql_ids {
        if let Some(text) = sql_text.get(sql_id) {
            let tables = extract_tables_from_sql(text);
            all_tables.extend(tables);
        }
    }

    let mut result: Vec<String> = all_tables.into_iter().collect();
    result.sort();
    result
}

/// Compute Z-Score normalization for a vector: (x - mean) / stddev.
/// Returns a vector of the same length. If stddev is 0 or NaN, returns zeros
/// (the series is constant — nothing to normalize).
pub fn z_score_normalize(values: &[f64]) -> Vec<f64> {
    // Use only finite values to compute mean/stddev, but preserve NaN positions in output.
    let finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() {
        return vec![0.0; values.len()];
    }
    let avg = mean(finite.clone()).unwrap_or(0.0);
    let sd = std_deviation(finite).unwrap_or(0.0);
    if sd.abs() < f64::EPSILON || !sd.is_finite() {
        return vec![0.0; values.len()];
    }
    values
        .iter()
        .map(|v| {
            if v.is_finite() {
                (v - avg) / sd
            } else {
                f64::NAN
            }
        })
        .collect()
}

pub fn robust_z_score(values: &[f64]) -> Vec<f64> {
    if values.is_empty() {
        return Vec::new();
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    let mut deviations: Vec<f64> = values.iter().map(|v| (v - median).abs()).collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = deviations[deviations.len() / 2];
    if mad.abs() < f64::EPSILON {
        return vec![0.0; values.len()];
    }
    let scale = 1.4826 * mad;
    values.iter().map(|v| (v - median) / scale).collect()
}

pub fn robust_minmax_0_100(values: &[f64], low_pct: f64, high_pct: f64) -> Vec<f64> {
    let finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();

    if finite.is_empty() {
        return vec![0.0; values.len()];
    }

    let lo = percentile(&finite, low_pct);
    let hi = percentile(&finite, high_pct);

    if !lo.is_finite() || !hi.is_finite() || (hi - lo).abs() < f64::EPSILON {
        return vec![0.0; values.len()];
    }

    values
        .iter()
        .map(|v| {
            if !v.is_finite() {
                f64::NAN
            } else {
                let scaled = ((*v - lo) / (hi - lo)) * 100.0;
                scaled.clamp(0.0, 100.0)
            }
        })
        .collect()
}

pub fn percentile(values: &[f64], pct: f64) -> f64 {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();

    if sorted.is_empty() {
        return f64::NAN;
    }

    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let pct = pct.clamp(0.0, 100.0);
    let rank = (pct / 100.0) * ((sorted.len() - 1) as f64);

    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;

    if lower == upper {
        sorted[lower]
    } else {
        let weight = rank - lower as f64;
        sorted[lower] * (1.0 - weight) + sorted[upper] * weight
    }
}

pub fn log1p_robust_minmax_0_100(values: &[f64], low_pct: f64, high_pct: f64) -> Vec<f64> {
    let transformed: Vec<f64> = values
        .iter()
        .map(|v| {
            if v.is_finite() && *v >= 0.0 {
                v.ln_1p()
            } else if v.is_finite() {
                // For safety. Most Oracle counters should not be negative anyway.
                0.0
            } else {
                f64::NAN
            }
        })
        .collect();

    robust_minmax_0_100(&transformed, low_pct, high_pct)
}

/// Percentile of absolute values — robust measure of "active magnitude".
/// Uses linear interpolation between order statistics.
/// Returns 0.0 for empty input.
pub fn abs_percentile(series: &[f64], p: f64) -> f64 {
    if series.is_empty() {
        return 0.0;
    }
    let mut abs_vals: Vec<f64> = series
        .iter()
        .map(|v| v.abs())
        .filter(|v| v.is_finite())
        .collect();
    if abs_vals.is_empty() {
        return 0.0;
    }
    abs_vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = abs_vals.len();
    let rank = p.clamp(0.0, 1.0) * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = rank - lo as f64;
    abs_vals[lo] * (1.0 - frac) + abs_vals[hi] * frac
}
