use crate::awr::{AWRSCollection, AWR};
use crate::debug_note;
use crate::measurements;
use crate::reasonings::{
    DbTimeDegradationDomainSummary, DbTimeDegradationFinding, DbTimeDegradationReport,
};
use crate::tools::{get_safe_filename, jasmin_brand_banner_html, mad, median};
use crate::Args;
use html_escape::{encode_double_quoted_attribute, encode_text};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

const MIN_RECENT_SAMPLES: usize = 2;
const MAX_RECENT_SAMPLES: usize = 48;
const MIN_BASELINE_SAMPLES: usize = 3;
const MIN_DB_TIME_Z: f64 = 3.0;
const MIN_DB_TIME_DELTA_PCT: f64 = 25.0;
const STRONG_DB_TIME_DELTA_PCT: f64 = 100.0;
const RECENT_OVER_BASELINE_P90_FACTOR: f64 = 1.25;
const MIN_FINDING_Z: f64 = 2.5;
const MIN_FINDING_DELTA_PCT: f64 = 20.0;
const STRONG_FINDING_DELTA_PCT: f64 = 100.0;
const MIN_FINDING_CORR: f64 = 0.30;

// A named metric time series: SQL_ID / wait event / stat name -> one value per snapshot.
// All series scored by this module must be aligned with the DB Time vector.
type SeriesMap = BTreeMap<String, Vec<f64>>;

pub fn build_db_time_degradation_report(
    collection: &AWRSCollection,
    snap_range: &(u64, u64),
    x_vals: &[String],
    db_time: &[f64],
    db_cpu: &[f64],
    wait_events: &SeriesMap,
    sql_elapsed: &SeriesMap,
    instance_stats: &HashMap<String, Vec<f64>>,
    args: &Args,
) -> Option<DbTimeDegradationReport> {
    debug_note!(
        "Building DB Time degradation report: snap_range={}-{}, db_time_samples={}, db_cpu_samples={}, waits={}, sqls={}, stats={}",
        snap_range.0,
        snap_range.1,
        db_time.len(),
        db_cpu.len(),
        wait_events.len(),
        sql_elapsed.len(),
        instance_stats.len()
    );
    if db_time.len() < MIN_BASELINE_SAMPLES + MIN_RECENT_SAMPLES || x_vals.len() != db_time.len() {
        debug_note!(
            "Skipping DB Time degradation report: insufficient or unaligned samples (labels={}, db_time={})",
            x_vals.len(),
            db_time.len()
        );
        return None;
    }

    let (baseline, degraded) = split_windows(db_time.len());
    if baseline.len() < MIN_BASELINE_SAMPLES || degraded.len() < MIN_RECENT_SAMPLES {
        debug_note!(
            "Skipping DB Time degradation report: baseline_samples={}, recent_samples={}",
            baseline.len(),
            degraded.len()
        );
        return None;
    }

    let db_time_stats = match compare_windows(db_time, &baseline, &degraded) {
        Some(stats) => stats,
        None => {
            debug_note!("Skipping DB Time degradation report: window comparison unavailable");
            return None;
        }
    };
    let db_cpu_stats = compare_windows(db_cpu, &baseline, &degraded).unwrap_or_default();

    let instance_rates = measurements::instance_rates(collection, snap_range);
    let wait_rates = measurements::domain_rates(
        collection,
        snap_range,
        wait_events,
        "foreground_wait_events",
    );
    let load_profile = load_profile_series(collection, snap_range, db_time.len());
    let time_model = measurements::domain_rates(
        collection,
        snap_range,
        &time_model_series(collection, snap_range, db_time.len()),
        "time_model_stats",
    );
    let mut sql_elapsed_wide = sql_elapsed_series(collection, snap_range, db_time.len());
    for (sql_id, series) in sql_elapsed {
        sql_elapsed_wide.insert(sql_id.clone(), series.clone());
    }

    let sql_elapsed_wide = measurements::domain_rates(
        collection,
        snap_range,
        &sql_elapsed_wide,
        "sql_elapsed_time",
    );

    // Keep independent top-N lists per domain. No cross-unit ranking or additive cost.
    let per_domain_limit = args.top_gradient.max(10);
    let mut findings = Vec::new();
    findings.extend(top_findings(
        score_domain(
            "SQL elapsed time",
            &sql_elapsed_wide,
            db_time,
            &baseline,
            &degraded,
        ),
        per_domain_limit,
    ));
    findings.extend(top_findings(
        score_domain(
            "Foreground wait events",
            &wait_rates,
            db_time,
            &baseline,
            &degraded,
        ),
        per_domain_limit,
    ));
    findings.extend(top_findings(
        score_domain(
            "Instance statistics",
            &instance_rates,
            db_time,
            &baseline,
            &degraded,
        ),
        per_domain_limit,
    ));
    findings.extend(top_findings(
        score_domain("Time model", &time_model, db_time, &baseline, &degraded),
        per_domain_limit,
    ));
    findings.extend(top_findings(
        score_domain("Load profile", &load_profile, db_time, &baseline, &degraded),
        per_domain_limit,
    ));

    // Domain order is lexical; ranks have meaning only inside their own domain.
    findings.sort_by(|a, b| {
        a.domain
            .cmp(&b.domain)
            .then(a.domain_rank.cmp(&b.domain_rank))
    });
    let dominant_domains = summarize_domains(&findings);
    let is_degradation_detected = is_db_time_degraded(&db_time_stats);
    let verdict = if is_degradation_detected {
        format!(
            "DB Time degradation detected. Change: {:.1}% ({:.3} -> {:.3} s/s), robust z-score {:.2}, recent peak {:.3} s/s.",
            db_time_stats.delta_pct,
            db_time_stats.baseline_avg,
            db_time_stats.degraded_avg,
            db_time_stats.robust_z_score,
            db_time_stats.degraded_peak
        )
    } else {
        format!(
            "No statistically strong DB Time degradation detected. Change: {:.1}%, robust z-score {:.2}, recent peak {:.3} s/s.",
            db_time_stats.delta_pct, db_time_stats.robust_z_score, db_time_stats.degraded_peak
        )
    };

    debug_note!(
        "DB Time degradation report ready: detected={}, delta_pct={:.2}, findings={}, domains={}",
        is_degradation_detected,
        db_time_stats.delta_pct,
        findings.len(),
        dominant_domains.len()
    );

    Some(DbTimeDegradationReport {
        is_degradation_detected,
        verdict,
        baseline_start: x_vals[*baseline.first().unwrap()].clone(),
        baseline_end: x_vals[*baseline.last().unwrap()].clone(),
        degraded_start: x_vals[*degraded.first().unwrap()].clone(),
        degraded_end: x_vals[*degraded.last().unwrap()].clone(),
        baseline_samples: baseline.len(),
        degraded_samples: degraded.len(),
        db_time_baseline_avg: db_time_stats.baseline_avg,
        db_time_degraded_avg: db_time_stats.degraded_avg,
        db_time_delta_avg: db_time_stats.delta_avg,
        db_time_delta_pct: db_time_stats.delta_pct,
        db_time_robust_z_score: db_time_stats.robust_z_score,
        db_cpu_baseline_avg: db_cpu_stats.baseline_avg,
        db_cpu_degraded_avg: db_cpu_stats.degraded_avg,
        db_cpu_delta_avg: db_cpu_stats.delta_avg,
        db_cpu_delta_pct: db_cpu_stats.delta_pct,
        dominant_domains,
        findings,
    })
}

pub fn find_degraded_sqls_for_analysis(
    collection: &AWRSCollection,
    snap_range: &(u64, u64),
    _limit: usize,
) -> Vec<(String, String)> {
    // This is an early, SQL-only pass used before the main report builds SQL plots/tables.
    // It reuses the same baseline-vs-recent math as the full degradation report, then returns
    // SQL_IDs that should be promoted into the normal TOP SQL analysis pipeline.
    let db_time = db_time_series(collection, snap_range);
    debug_note!(
        "Scanning degraded SQLs: snap_range={}-{}, db_time_samples={}",
        snap_range.0,
        snap_range.1,
        db_time.len()
    );
    if db_time.len() < MIN_BASELINE_SAMPLES + MIN_RECENT_SAMPLES {
        debug_note!("Skipping degraded SQL scan: insufficient DB Time samples");
        return Vec::new();
    }

    let (baseline, degraded) = split_windows(db_time.len());
    if baseline.len() < MIN_BASELINE_SAMPLES || degraded.len() < MIN_RECENT_SAMPLES {
        debug_note!(
            "Skipping degraded SQL scan: baseline_samples={}, recent_samples={}",
            baseline.len(),
            degraded.len()
        );
        return Vec::new();
    }

    let Some(_) = compare_windows(&db_time, &baseline, &degraded) else {
        debug_note!("Skipping degraded SQL scan: window comparison unavailable");
        return Vec::new();
    };

    let sql_elapsed = sql_elapsed_series(collection, snap_range, db_time.len());
    let sql_modules = sql_module_map(collection, snap_range);

    // Do not cap this list with --top-gradient. The goal here is not presentation ranking;
    // it is coverage: every SQL_ID identified as degraded should be available to the regular
    // SQL analysis pipeline, otherwise the detailed pages/tables/gradients can miss it.
    let sql_elapsed =
        measurements::domain_rates(collection, snap_range, &sql_elapsed, "sql_elapsed_time");
    let mut sql_findings = score_domain(
        "SQL elapsed time",
        &sql_elapsed,
        &db_time,
        &baseline,
        &degraded,
    );
    sql_findings.sort_by(|a, b| {
        b.change_score
            .partial_cmp(&a.change_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                b.robust_z_score
                    .partial_cmp(&a.robust_z_score)
                    .unwrap_or(Ordering::Equal)
            })
    });

    let degraded_sqls = sql_findings
        .into_iter()
        .map(|f| {
            let module = sql_modules.get(&f.name).cloned().unwrap_or_default();
            (f.name, module)
        })
        .collect::<Vec<_>>();
    debug_note!(
        "Degraded SQL scan completed: sql_count={}",
        degraded_sqls.len()
    );
    degraded_sqls
}

pub fn build_db_time_degradation_html(report: &DbTimeDegradationReport) -> String {
    let brand = jasmin_brand_banner_html();
    let mut domain_rows = String::new();
    let mut domain_options = String::from(r#"<option value="">All domains</option>"#);
    for d in &report.dominant_domains {
        domain_rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{:.3}</td></tr>",
            encode_text(&d.domain),
            d.findings_count,
            d.max_change_score
        ));
        domain_options.push_str(&format!(
            r#"<option value="{}">{}</option>"#,
            encode_text(&d.domain),
            encode_text(&d.domain)
        ));
    }

    let mut finding_rows = String::new();
    for f in &report.findings {
        let name_html = linked_finding_name(f);
        finding_rows.push_str(&format!(
            "<tr data-domain=\"{}\"><td>{}</td><td>{}</td><td>{:.3}</td><td>{:.3}</td><td>{:.3}</td><td>{:.1}%</td><td>{:.2}</td><td>{:.2}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            encode_text(&f.domain),
            encode_text(&f.domain),
            name_html,
            f.baseline_avg,
            f.degraded_avg,
            f.delta_avg,
            f.delta_pct,
            f.robust_z_score,
            f.correlation_with_db_time,
            encode_text(&f.unit),
            encode_text(&f.severity),
            encode_text(&f.evidence)
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>DB Time Degradation</title>
    <style>
        body {{ font-family: Arial, sans-serif; color: #222; }}
        .content {{ font-size: 14px; max-width: 1500px; margin: 0 auto; }}
        .summary {{ border-left: 5px solid #c52228; background: #fbf1f2; padding: 14px 18px; margin: 18px 0; }}
        table {{ width: 100%; border-collapse: collapse; margin-top: 20px; table-layout: fixed; }}
        th, td {{ border: 1px solid black; padding: 8px; text-align: center; overflow-wrap: anywhere; font-size: 12px; }}
        th {{ background-color: #111111; color: white; cursor: pointer; user-select: none; }}
        th:hover {{ background-color: #c52228; }}
        tr:nth-child(even) {{ background-color: #f2f2f2; }}
        td:nth-child(2), td:nth-child(11) {{ text-align: left; }}
        .verdict {{ font-size: 18px; font-weight: bold; }}
        .table-controls {{ display: flex; align-items: center; gap: 10px; margin: 12px 0 0; }}
        .table-controls label {{ font-weight: bold; }}
        .table-controls select {{ min-width: 220px; padding: 6px 8px; border: 1px solid #999; border-radius: 4px; background: white; }}
    </style>
    <script>
        function sortTable(headerCell) {{
            const table = headerCell.closest('table');
            const idx = Array.from(headerCell.parentElement.children).indexOf(headerCell);
            const tbody = table.tBodies[0];
            const rows = Array.from(tbody.rows);
            const asc = headerCell.getAttribute('data-sort-dir') !== 'asc';
            table.querySelectorAll('th').forEach(th => th.removeAttribute('data-sort-dir'));
            rows.sort((a, b) => {{
                const av = a.cells[idx]?.innerText.trim() || '';
                const bv = b.cells[idx]?.innerText.trim() || '';
                const an = Number(av.replace('%', ''));
                const bn = Number(bv.replace('%', ''));
                const cmp = !Number.isNaN(an) && !Number.isNaN(bn) ? an - bn : av.localeCompare(bv, undefined, {{ numeric: true }});
                return asc ? cmp : -cmp;
            }});
            rows.forEach(r => tbody.appendChild(r));
            headerCell.setAttribute('data-sort-dir', asc ? 'asc' : 'desc');
        }}
        function filterDegradedParameters() {{
            const select = document.getElementById('domain-filter');
            const selectedDomain = select ? select.value : '';
            document.querySelectorAll('#degraded-parameters-table tbody tr').forEach(row => {{
                const rowDomain = row.getAttribute('data-domain') || '';
                row.style.display = selectedDomain === '' || rowDomain === selectedDomain ? '' : 'none';
            }});
        }}
        document.addEventListener('DOMContentLoaded', () => {{
            document.querySelectorAll('th').forEach(th => th.addEventListener('click', () => sortTable(th)));
            const domainFilter = document.getElementById('domain-filter');
            if (domainFilter) {{
                domainFilter.addEventListener('change', filterDegradedParameters);
            }}
        }});
    </script>
</head>
<body>
<div class="content">
    {brand}
    <h2>DB Time Degradation Report</h2>
    <div class="summary">
        <div class="verdict">{}</div>
        <p><strong>Baseline:</strong> {} - {} ({} samples)<br>
        <strong>Degraded window:</strong> {} - {} ({} samples)</p>
        <p><strong>DB Time:</strong> {:.3} -> {:.3} s/s, delta {:.3} ({:.1}%), robust z-score {:.2}<br>
        <strong>DB CPU:</strong> {:.3} -> {:.3} s/s, delta {:.3} ({:.1}%)</p>
    </div>

    <h3>Changed Domains</h3><p>Ranked within each domain by dimensionless change score. Metrics overlap and must not be summed. No percentage of DB Time or recoverable time is estimated.</p>
    <table>
        <thead><tr><th>Domain</th><th>Findings</th><th>Maximum change score (dimensionless)</th></tr></thead>
        <tbody>{}</tbody>
    </table>

    <h3>Degraded Parameters</h3>
    <div class="table-controls">
        <label for="domain-filter">Domain</label>
        <select id="domain-filter">{}</select>
    </div>
    <table id="degraded-parameters-table">
        <thead><tr><th>Domain</th><th>Name</th><th>Baseline avg</th><th>Recent avg</th><th>Delta</th><th>Delta %</th><th>Robust z</th><th>Corr DB Time</th><th>Unit</th><th>Severity</th><th>Evidence</th></tr></thead>
        <tbody>{}</tbody>
    </table>
</div>
</body>
</html>"#,
        encode_text(&report.verdict),
        encode_text(&report.baseline_start),
        encode_text(&report.baseline_end),
        report.baseline_samples,
        encode_text(&report.degraded_start),
        encode_text(&report.degraded_end),
        report.degraded_samples,
        report.db_time_baseline_avg,
        report.db_time_degraded_avg,
        report.db_time_delta_avg,
        report.db_time_delta_pct,
        report.db_time_robust_z_score,
        report.db_cpu_baseline_avg,
        report.db_cpu_degraded_avg,
        report.db_cpu_delta_avg,
        report.db_cpu_delta_pct,
        domain_rows,
        domain_options,
        finding_rows
    )
}

fn linked_finding_name(finding: &DbTimeDegradationFinding) -> String {
    let name = encode_text(&finding.name);
    match finding.domain.as_str() {
        "SQL elapsed time" => format!(
            r#"<a href="../sqlid/sqlid_{}.html" target="_blank" class="nav-link" style="font-weight: bold">{}</a>"#,
            encode_double_quoted_attribute(&finding.name),
            name
        ),
        "Foreground wait events" => {
            let file_name = get_safe_filename(finding.name.clone(), "fg".to_string());
            format!(
                r#"<a href="../{}" target="_blank" class="nav-link" style="font-weight: bold">{}</a>"#,
                encode_double_quoted_attribute(&file_name),
                name
            )
        }
        _ => name.to_string(),
    }
}

#[derive(Default)]
struct WindowComparison {
    baseline_avg: f64,
    degraded_avg: f64,
    degraded_median: f64,
    degraded_peak: f64,
    baseline_p90: f64,
    delta_avg: f64,
    delta_pct: f64,
    robust_z_score: f64,
}

pub(crate) fn split_windows(len: usize) -> (Vec<usize>, Vec<usize>) {
    // The recent window represents the suspected degradation period. A 25% tail works well
    // for "last few days vs previous week" reports while the hard cap prevents long inputs
    // from diluting the recent signal with too many older snapshots.
    let mut recent_len = ((len as f64) * 0.25).ceil() as usize;
    recent_len = recent_len.clamp(MIN_RECENT_SAMPLES, MAX_RECENT_SAMPLES);
    if len.saturating_sub(recent_len) < MIN_BASELINE_SAMPLES {
        recent_len = len.saturating_sub(MIN_BASELINE_SAMPLES);
    }
    let split = len - recent_len;
    ((0..split).collect(), (split..len).collect())
}

fn compare_windows(
    series: &[f64],
    baseline: &[usize],
    degraded: &[usize],
) -> Option<WindowComparison> {
    if series.iter().any(|v| !v.is_finite()) || series.len() <= *degraded.last()? {
        return None;
    }
    let baseline_values: Vec<f64> = baseline.iter().map(|&i| series[i]).collect();
    let degraded_values: Vec<f64> = degraded.iter().map(|&i| series[i]).collect();
    let baseline_avg = avg(&baseline_values);
    let degraded_avg = avg(&degraded_values);
    let baseline_p90 = percentile(&baseline_values, 0.90);
    let degraded_peak = degraded_values.iter().copied().fold(0.0, f64::max);
    let delta_avg = degraded_avg - baseline_avg;
    // Percent delta captures level shifts that users see on charts. It complements robust
    // z-score because a noisy baseline can make z-score modest even when the average doubled.
    let delta_pct = if baseline_avg.abs() < 1e-9 {
        if delta_avg > 0.0 {
            100.0
        } else {
            0.0
        }
    } else {
        delta_avg / baseline_avg.abs() * 100.0
    };
    let baseline_median = median(&baseline_values);
    let baseline_mad = mad(&baseline_values);
    let degraded_median = median(&degraded_values);
    // Robust z-score uses median and MAD instead of mean/stddev:
    //   z = (recent_median - baseline_median) / (1.4826 * baseline_MAD)
    // 1.4826 rescales MAD to be comparable to standard deviation under a normal distribution.
    // This makes the test less sensitive to isolated baseline spikes.
    let robust_z_score = if baseline_mad <= 1e-9 {
        if delta_avg > 0.0 {
            99.0
        } else {
            0.0
        }
    } else {
        (degraded_median - baseline_median) / (1.4826 * baseline_mad)
    };
    Some(WindowComparison {
        baseline_avg,
        degraded_avg,
        degraded_median,
        degraded_peak,
        baseline_p90,
        delta_avg,
        delta_pct,
        robust_z_score,
    })
}

fn score_domain(
    domain: &str,
    series_map: &SeriesMap,
    db_time: &[f64],
    baseline: &[usize],
    degraded: &[usize],
) -> Vec<DbTimeDegradationFinding> {
    let mut findings = Vec::new();
    for (name, series) in series_map {
        if series.len() != db_time.len() {
            continue;
        }
        let Some(stats) = compare_windows(series, baseline, degraded) else {
            continue;
        };
        if stats.delta_avg <= 0.0 {
            continue;
        }
        let corr = safe_pearson(series, db_time);
        if !is_degraded_finding(&stats, corr) {
            continue;
        }
        // Unit-invariant change evidence, never an accounting share or a time saving.
        let score = stats.robust_z_score.max(0.0).min(99.0)
            + (1.0 + stats.delta_pct.max(0.0) / 100.0).ln() * corr.max(0.0);
        let severity = classify_severity(stats.robust_z_score, stats.delta_pct, corr);
        let unit = match domain {
            "SQL elapsed time" | "Foreground wait events" | "Time model" => "s/s".to_string(),
            "Instance statistics" => format!("{} ({})", measurements::statistic_unit(name), name),
            _ => format!("reported units/s ({})", name),
        };
        let evidence = format!(
            "avg {:.3} -> {:.3} {}; robust z {:.2}; corr(DB Time) {:.2}; change score {:.2}; association only",
            stats.baseline_avg, stats.degraded_avg, unit, stats.robust_z_score, corr, score
        );
        findings.push(DbTimeDegradationFinding {
            domain: domain.to_string(),
            name: name.clone(),
            baseline_avg: stats.baseline_avg,
            degraded_avg: stats.degraded_avg,
            delta_avg: stats.delta_avg,
            delta_pct: stats.delta_pct,
            robust_z_score: stats.robust_z_score,
            correlation_with_db_time: corr,
            change_score: score,
            unit,
            domain_rank: 0,
            severity,
            evidence,
        });
    }
    findings
}

fn avg(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let idx = ((sorted.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize;
    sorted[idx]
}

fn is_db_time_degraded(stats: &WindowComparison) -> bool {
    // DB Time degradation is detected by a hybrid rule:
    // 1. robust statistical shift (MAD-based z-score),
    // 2. very strong average level shift, or
    // 3. recent tail breakout above the historical baseline p90.
    //
    // The OR combination is intentional. In real AWR series the baseline may already contain
    // some bursts, which inflates MAD and can hide an obvious chart-level degradation if only
    // z-score is used.
    let robust_signal =
        stats.robust_z_score >= MIN_DB_TIME_Z && stats.delta_pct >= MIN_DB_TIME_DELTA_PCT;
    let strong_level_shift =
        stats.delta_pct >= STRONG_DB_TIME_DELTA_PCT && stats.degraded_avg > stats.baseline_avg;
    let recent_tail_breakout = stats.delta_pct >= MIN_DB_TIME_DELTA_PCT
        && stats.degraded_peak > stats.baseline_p90 * RECENT_OVER_BASELINE_P90_FACTOR;

    robust_signal || strong_level_shift || recent_tail_breakout
}

fn is_degraded_finding(stats: &WindowComparison, corr: f64) -> bool {
    // A parameter is considered degraded if it has its own robust shift, a very strong level
    // shift, a moderate shift correlated with DB Time, or a recent tail breakout. This allows
    // bursty SQL/wait patterns to be captured even when their median remains relatively low.
    let robust_signal = stats.robust_z_score >= MIN_FINDING_Z;
    let strong_level_shift =
        stats.delta_pct >= STRONG_FINDING_DELTA_PCT && stats.degraded_avg > stats.baseline_avg;
    let moderate_correlated_shift =
        stats.delta_pct >= MIN_FINDING_DELTA_PCT && corr.abs() >= MIN_FINDING_CORR;
    let tail_breakout = stats.delta_pct >= MIN_FINDING_DELTA_PCT
        && stats.degraded_peak > stats.baseline_p90 * RECENT_OVER_BASELINE_P90_FACTOR;

    robust_signal || strong_level_shift || moderate_correlated_shift || tail_breakout
}

fn safe_pearson(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.len() < 2 {
        return 0.0;
    }
    let mean_a = avg(a);
    let mean_b = avg(b);
    let mut num = 0.0;
    let mut den_a = 0.0;
    let mut den_b = 0.0;
    // Pearson correlation:
    //   r = cov(a,b) / (std(a) * std(b))
    // We compute the numerator and denominator directly so constant series return 0 instead
    // of panicking in the ndarray helper used elsewhere in the project.
    for (&x, &y) in a.iter().zip(b.iter()) {
        let dx = x - mean_a;
        let dy = y - mean_b;
        num += dx * dy;
        den_a += dx * dx;
        den_b += dy * dy;
    }
    let den = (den_a * den_b).sqrt();
    if den <= 1e-12 {
        0.0
    } else {
        (num / den).clamp(-1.0, 1.0)
    }
}

fn classify_severity(z: f64, delta_pct: f64, corr: f64) -> String {
    // Statistical change severity does not establish materiality or mechanism.
    if z >= 6.0 && delta_pct >= 100.0 && corr >= 0.5 {
        "high"
    } else if z >= 2.5 || delta_pct >= 25.0 {
        "medium"
    } else {
        "low"
    }
    .to_string()
}

fn summarize_domains(findings: &[DbTimeDegradationFinding]) -> Vec<DbTimeDegradationDomainSummary> {
    let mut by_domain: BTreeMap<String, DbTimeDegradationDomainSummary> = BTreeMap::new();
    for f in findings {
        let entry =
            by_domain
                .entry(f.domain.clone())
                .or_insert_with(|| DbTimeDegradationDomainSummary {
                    domain: f.domain.clone(),
                    findings_count: 0,
                    max_change_score: 0.0,
                });
        entry.findings_count += 1;
        entry.max_change_score = entry.max_change_score.max(f.change_score);
    }
    by_domain.into_values().collect()
}

fn top_findings(
    mut findings: Vec<DbTimeDegradationFinding>,
    limit: usize,
) -> Vec<DbTimeDegradationFinding> {
    // Unit-invariant rank within a domain; materiality is evaluated separately.
    findings.sort_by(|a, b| {
        b.change_score
            .partial_cmp(&a.change_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                b.robust_z_score
                    .partial_cmp(&a.robust_z_score)
                    .unwrap_or(Ordering::Equal)
            })
    });
    for (i, f) in findings.iter_mut().enumerate() {
        f.domain_rank = i + 1;
    }
    findings.truncate(limit);
    findings
}

fn load_profile_series(
    collection: &AWRSCollection,
    snap_range: &(u64, u64),
    expected_len: usize,
) -> SeriesMap {
    let mut names = BTreeMap::new();
    for awr in filtered_awrs(&collection.awrs, snap_range) {
        for lp in &awr.load_profile {
            names.entry(lp.stat_name.clone()).or_insert_with(Vec::new);
        }
    }
    fill_series(
        names,
        expected_len,
        |awr, name| measurements::load_profile_rate(awr, name).unwrap_or(f64::NAN),
        collection,
        snap_range,
    )
}

fn time_model_series(
    collection: &AWRSCollection,
    snap_range: &(u64, u64),
    expected_len: usize,
) -> SeriesMap {
    let mut names = BTreeMap::new();
    for awr in filtered_awrs(&collection.awrs, snap_range) {
        for tm in &awr.time_model_stats {
            names.entry(tm.stat_name.clone()).or_insert_with(Vec::new);
        }
    }
    fill_series(
        names,
        expected_len,
        |awr, name| {
            awr.time_model_stats
                .iter()
                .find(|tm| tm.stat_name == name)
                .filter(|_| measurements::available(awr, "time_model_stats", true))
                .map(|tm| tm.time_s)
                .unwrap_or(f64::NAN)
        },
        collection,
        snap_range,
    )
}

fn db_time_series(collection: &AWRSCollection, snap_range: &(u64, u64)) -> Vec<f64> {
    measurements::db_load_series(collection, snap_range, measurements::DbLoadMetric::DbTime)
}

fn sql_elapsed_series(
    collection: &AWRSCollection,
    snap_range: &(u64, u64),
    expected_len: usize,
) -> SeriesMap {
    let mut names = BTreeMap::new();
    for awr in filtered_awrs(&collection.awrs, snap_range) {
        for sql in &awr.sql_elapsed_time {
            names.entry(sql.sql_id.clone()).or_insert_with(Vec::new);
        }
    }
    fill_series(
        names,
        expected_len,
        |awr, sql_id| {
            awr.sql_elapsed_time
                .iter()
                .filter(|sql| sql.sql_id == sql_id)
                .map(|sql| sql.elapsed_time_s)
                .sum()
        },
        collection,
        snap_range,
    )
}

fn sql_module_map(collection: &AWRSCollection, snap_range: &(u64, u64)) -> HashMap<String, String> {
    let mut modules = HashMap::new();
    for awr in filtered_awrs(&collection.awrs, snap_range) {
        for sql in &awr.sql_elapsed_time {
            modules
                .entry(sql.sql_id.clone())
                .or_insert_with(|| sql.sql_module.clone());
        }
    }
    modules
}

fn fill_series<F>(
    mut names: SeriesMap,
    expected_len: usize,
    value_for: F,
    collection: &AWRSCollection,
    snap_range: &(u64, u64),
) -> SeriesMap
where
    F: Fn(&AWR, &str) -> f64,
{
    for awr in filtered_awrs(&collection.awrs, snap_range) {
        for series in names.values_mut() {
            series.push(0.0);
        }
        for (name, series) in names.iter_mut() {
            if let Some(last) = series.last_mut() {
                *last = value_for(awr, name);
            }
        }
    }
    names.retain(|_, v| v.len() == expected_len && v.iter().any(|x| x.abs() > 1e-9));
    names
}

fn filtered_awrs<'a>(awrs: &'a [AWR], snap_range: &(u64, u64)) -> impl Iterator<Item = &'a AWR> {
    let (begin, end) = *snap_range;
    awrs.iter()
        .filter(move |awr| awr.snap_info.begin_snap_id >= begin && awr.snap_info.end_snap_id <= end)
}

pub(crate) fn detected(c: &AWRSCollection, range: &(u64, u64)) -> bool {
    let series = db_time_series(c, range);
    if series.len() < 5 {
        return false;
    }
    let (b, r) = split_windows(series.len());
    compare_windows(&series, &b, &r).is_some_and(|s| is_db_time_degraded(&s))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changing_counter_unit_cannot_create_a_db_time_share_or_change_rank() {
        let x = vec![10.0, 11.0, 9.0, 10.0, 12.0, 10.0, 30.0, 32.0];
        let y = vec![1.0, 1.1, 0.9, 1.0, 1.2, 1.0, 3.0, 3.2];
        let (b, r) = split_windows(x.len());
        let base = score_domain(
            "Instance statistics",
            &BTreeMap::from([("counter".into(), x.clone())]),
            &y,
            &b,
            &r,
        );
        let scaled = score_domain(
            "Instance statistics",
            &BTreeMap::from([("counter".into(), x.iter().map(|x| x * 1e6).collect())]),
            &y,
            &b,
            &r,
        );
        assert!((base[0].change_score - scaled[0].change_score).abs() < 1e-8);
        let value = serde_json::to_value(&scaled[0]).unwrap();
        assert!(value.get("estimated_db_time_delta_share").is_none());
        let html = build_db_time_degradation_html(&DbTimeDegradationReport {
            findings: scaled,
            ..Default::default()
        });
        assert!(!html.contains("999.0%"));
        assert!(!html.contains("DB Time delta share"));
        assert!(html.contains("Unit"));
    }
    #[test]
    fn domain_summaries_do_not_sum_counter_deltas() {
        let findings = vec![
            DbTimeDegradationFinding {
                domain: "Instance statistics".into(),
                delta_avg: 1e9,
                change_score: 4.0,
                ..Default::default()
            },
            DbTimeDegradationFinding {
                domain: "Time model".into(),
                delta_avg: 0.1,
                change_score: 10.0,
                ..Default::default()
            },
        ];
        let value = serde_json::to_value(summarize_domains(&findings)).unwrap();
        assert!(value[0].get("total_positive_delta").is_none());
        assert_eq!(value[0]["max_change_score"], 4.0);
    }
}
