use std::collections::{BTreeMap, HashSet};
use crate::make_notes;
use crate::Args;
use crate::reasonings::{StatisticsDescription,
                        TopPeaksSelected,
                        MadAnomaliesEvents,
                        MadAnomaliesSQL,
                        TopForegroundWaitEvents,
                        TopBackgroundWaitEvents,
                        PctOfTimesThisSQLFoundInOtherTopSections,
                        WaitEventsWithStrongCorrelation,
                        WaitEventsFromASH,
                        TopSQLsByElapsedTime,
                        StatsSummary,
                        IOStatsByFunctionSummary,
                        LatchActivitySummary,
                        Top10SegmentStats,
                        InstanceStatisticCorrelation,
                        LoadProfileAnomalies,
                        AnomalyDescription,
                        AnomlyCluster,
                        ReportForAI,
                        AppState,
                        GradientSettings,
                        GradientTopItem,
                        CrossModelClassification,
                        DbTimeGradientSection};

use prettytable::{Table, Row, Cell, format, Attr};
use colored::*;
use crate::tools::*;


/// Named time series: event_name/stat_name/sqlid -> Vec<sample_value>
/// Each Vec must have the same length as DB Time series.
pub type EventSeriesMap = BTreeMap<String, Vec<f64>>;

/// Named vector: event_name/stat_name/sqlid -> scalar_value (coef, impact, mean, std, MAD, etc.)
pub type EventScalarMap = BTreeMap<String, f64>;

#[derive(Debug, Clone)]
pub struct EventImpact {
    pub event_name: String,
    /// Regression coefficient for standardized Δ(event/stat/sql)
    pub gradient_coef: f64,
    /// impact = |coef| * MAD(raw Δ(event/stat/sql))
    pub impact: f64,
}

/// Full result package
#[derive(Debug, Clone)]
pub struct DbTimeGradientResult {
    /// Ridge coefficients: event -> coef
    pub ridge_gradient_by_event: EventScalarMap,
    /// Elastic Net coefficients: event -> coef
    pub elastic_net_gradient_by_event: EventScalarMap,
    //Huber robust regression coefficients
    pub huber_gradient_by_event: EventScalarMap,
    //Quantile regression (tau=0.95) coefficients
    pub quantile95_gradient_by_event: EventScalarMap,

    /// Ridge impact ranking (sorted by impact descending)
    pub ridge_ranking: Vec<EventImpact>,
    /// Elastic Net impact ranking (sorted by impact descending)
    pub elastic_net_ranking: Vec<EventImpact>,
    //Huber impact ranking
    pub huber_ranking: Vec<EventImpact>,
    //Quantile 95 impact ranking
    pub quantile95_ranking: Vec<EventImpact>,

    /// Δ(wait event/statistic value/sqlid exec time) standardization stats
    pub event_delta_mean_by_event: EventScalarMap,
    pub event_delta_std_by_event: EventScalarMap,

    /// MAD(raw Δ(wait event/statistic value/sqlid exec time))
    pub event_delta_mad_by_event: EventScalarMap,
}

pub fn compute_db_time_gradient(
    db_time_series: &[f64],
    event_series: &EventSeriesMap,
    ridge_lambda: f64,
    elastic_net_lambda: f64,
    elastic_net_alpha: f64,
    elastic_net_max_iter: usize,
    elastic_net_tol: f64,
) -> Result<DbTimeGradientResult, String> {
    if db_time_series.len() < 3 {
        return Err("DB Time series must have at least 3 samples.".into());
    }
    if event_series.is_empty() {
        return Err("event_series is empty.".into());
    }
    if ridge_lambda < 0.0 || elastic_net_lambda < 0.0 {
        return Err("Regularization lambdas must be >= 0.".into());
    }
    if !(0.0..=1.0).contains(&elastic_net_alpha) {
        return Err("Elastic Net alpha must be in [0, 1].".into());
    }

    let time_len = db_time_series.len();

    for (event_name, series) in event_series.iter() {
        if series.len() != time_len {
            return Err(format!(
                "Event '{event_name}' has length {}, expected {} (same as DB Time).",
                series.len(),
                time_len
            ));
        }
    }

    let db_time_delta = compute_time_deltas(db_time_series);
    let event_delta_by_event = compute_event_deltas(event_series)?;
    let event_delta_mean_by_event = compute_mean_by_event(&event_delta_by_event);
    let event_delta_std_by_event = compute_std_by_event(&event_delta_by_event, &event_delta_mean_by_event);
    let event_delta_standardized_by_event =
        standardize_by_event(&event_delta_by_event, &event_delta_mean_by_event, &event_delta_std_by_event);
    let event_delta_mad_by_event = compute_mad_by_event(&event_delta_by_event);

    // Ridge
    let ridge_gradient_by_event = ridge_regression_map(
        &event_delta_standardized_by_event,
        &db_time_delta,
        ridge_lambda,
    )?;

    // Elastic Net
    let elastic_net_gradient_by_event = elastic_net_coordinate_descent_map(
        &event_delta_standardized_by_event,
        &db_time_delta,
        elastic_net_lambda,
        elastic_net_alpha,
        elastic_net_max_iter,
        elastic_net_tol,
    );

    //Huber robust regression
    let ridge_residuals = compute_residuals_map(&ridge_gradient_by_event, &event_delta_standardized_by_event, &db_time_delta);
    let huber_delta = 1.345 * mad_of_slice(&ridge_residuals);
    let huber_gradient_by_event = huber_regression_map(
        &event_delta_standardized_by_event,
        &db_time_delta,
        huber_delta,
        100,
        elastic_net_tol,
        ridge_lambda,
    );

    //Quantile regression tau=0.95
    let quantile95_gradient_by_event = quantile_regression_irls_map(
        &event_delta_standardized_by_event,
        &db_time_delta,
        0.95,
        200,
        elastic_net_tol,
    );

    let ridge_ranking = build_ranking(&ridge_gradient_by_event, &event_delta_mad_by_event);
    let elastic_net_ranking = build_ranking(&elastic_net_gradient_by_event, &event_delta_mad_by_event);
    let huber_ranking = build_ranking(&huber_gradient_by_event, &event_delta_mad_by_event);
    let quantile95_ranking = build_ranking(&quantile95_gradient_by_event, &event_delta_mad_by_event);

    Ok(DbTimeGradientResult {
        ridge_gradient_by_event,
        elastic_net_gradient_by_event,
        huber_gradient_by_event,
        quantile95_gradient_by_event,
        ridge_ranking,
        elastic_net_ranking,
        huber_ranking,
        quantile95_ranking,
        event_delta_mean_by_event,
        event_delta_std_by_event,
        event_delta_mad_by_event,
    })
}

/* =========================================================================================
   Core computations
   ========================================================================================= */

fn compute_time_deltas(series: &[f64]) -> Vec<f64> {
    let mut deltas = Vec::with_capacity(series.len().saturating_sub(1));
    for t in 0..series.len() - 1 {
        deltas.push(series[t + 1] - series[t]);
    }
    deltas
}

fn compute_event_deltas(event_series: &EventSeriesMap) -> Result<EventSeriesMap, String> {
    let mut deltas = BTreeMap::new();
    for (event_name, series) in event_series.iter() {
        if series.len() < 2 {
            return Err(format!("Wait event '{event_name}' must have at least 2 samples."));
        }
        deltas.insert(event_name.clone(), compute_time_deltas(series));
    }
    Ok(deltas)
}

fn compute_mean_by_event(series_by_event: &EventSeriesMap) -> EventScalarMap {
    let mut mean_by_event = BTreeMap::new();
    for (event_name, series) in series_by_event.iter() {
        let mean = if series.is_empty() {
            0.0
        } else {
            series.iter().sum::<f64>() / (series.len() as f64)
        };
        mean_by_event.insert(event_name.clone(), mean);
    }
    mean_by_event
}

fn compute_std_by_event(series_by_event: &EventSeriesMap, mean_by_event: &EventScalarMap) -> EventScalarMap {
    let mut std_by_event = BTreeMap::new();
    for (event_name, series) in series_by_event.iter() {
        let mean = *mean_by_event.get(event_name).unwrap_or(&0.0);
        let mut var = 0.0;
        for v in series.iter() {
            let d = *v - mean;
            var += d * d;
        }
        let denom = (series.len() as f64).max(1.0);
        let mut std = (var / denom).sqrt();
        if !std.is_finite() || std == 0.0 {
            std = 1e-12;
        }
        std_by_event.insert(event_name.clone(), std);
    }
    std_by_event
}

fn standardize_by_event(
    raw_by_event: &EventSeriesMap,
    mean_by_event: &EventScalarMap,
    std_by_event: &EventScalarMap,
) -> EventSeriesMap {
    let mut standardized = BTreeMap::new();
    for (event_name, series) in raw_by_event.iter() {
        let mean = *mean_by_event.get(event_name).unwrap_or(&0.0);
        let std = *std_by_event.get(event_name).unwrap_or(&1e-12);
        let mut out = Vec::with_capacity(series.len());
        for v in series.iter() {
            out.push((*v - mean) / std);
        }
        standardized.insert(event_name.clone(), out);
    }
    standardized
}

fn compute_mad_by_event(series_by_event: &EventSeriesMap) -> EventScalarMap {
    let mut mad_by_event = BTreeMap::new();
    for (event_name, series) in series_by_event.iter() {
        let mad = mad(series);
        mad_by_event.insert(event_name.clone(), mad);
    }
    mad_by_event
}

//Compute residuals for a given coefficient map
fn compute_residuals_map(
    coefs: &EventScalarMap,
    x_by_event: &EventSeriesMap,
    y: &[f64],
) -> Vec<f64> {
    let n = y.len();
    let mut residuals = y.to_vec();
    for (event_name, series) in x_by_event.iter() {
        let coef = *coefs.get(event_name).unwrap_or(&0.0);
        if coef != 0.0 {
            for t in 0..n {
                residuals[t] -= coef * series[t];
            }
        }
    }
    residuals
}

//MAD of a slice
fn mad_of_slice(values: &[f64]) -> f64 {
    if values.is_empty() { return 1.0; }
    let med = median(values);
    let mut deviations: Vec<f64> = values.iter().map(|v| (v - med).abs()).collect();
    let result = median_in_place(&mut deviations);
    if result == 0.0 { 1.0 } else { result }
}

/* =========================================================================================
   Ridge regression (map-based)
   ========================================================================================= */

fn ridge_regression_map(
    standardized_event_deltas: &EventSeriesMap,
    db_time_delta: &[f64],
    lambda: f64,
) -> Result<EventScalarMap, String> {
    let sample_count = db_time_delta.len();
    for (event_name, series) in standardized_event_deltas.iter() {
        if series.len() != sample_count {
            return Err(format!(
                "Standardized delta series length mismatch for '{event_name}': got {}, expected {}.",
                series.len(), sample_count
            ));
        }
    }
    let rows = build_rows_by_time(standardized_event_deltas, sample_count)?;
    let (mut a_matrix, mut b_vector) = build_normal_equations(&rows, db_time_delta);
    for event_name in standardized_event_deltas.keys() {
        let diag = a_matrix
            .entry(event_name.clone())
            .or_insert_with(BTreeMap::new)
            .entry(event_name.clone())
            .or_insert(0.0);
        *diag += lambda;
    }
    gaussian_elimination_solve(&mut a_matrix, &mut b_vector)
}

fn build_rows_by_time(
    standardized_event_deltas: &EventSeriesMap,
    sample_count: usize,
) -> Result<Vec<BTreeMap<String, f64>>, String> {
    let mut rows: Vec<BTreeMap<String, f64>> = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        rows.push(BTreeMap::new());
    }
    for (event_name, series) in standardized_event_deltas.iter() {
        if series.len() != sample_count {
            return Err(format!("Series length mismatch in build_rows_by_time for '{event_name}'."));
        }
        for (t, value) in series.iter().enumerate() {
            rows[t].insert(event_name.clone(), *value);
        }
    }
    Ok(rows)
}

fn build_normal_equations(
    rows: &[BTreeMap<String, f64>],
    y: &[f64],
) -> (BTreeMap<String, BTreeMap<String, f64>>, BTreeMap<String, f64>) {
    let mut a: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    let mut b: BTreeMap<String, f64> = BTreeMap::new();
    for (t, row) in rows.iter().enumerate() {
        let yt = y[t];
        for (event_i, xi) in row.iter() {
            *b.entry(event_i.clone()).or_insert(0.0) += xi * yt;
        }
        for (event_i, xi) in row.iter() {
            let row_i = a.entry(event_i.clone()).or_insert_with(BTreeMap::new);
            for (event_j, xj) in row.iter() {
                *row_i.entry(event_j.clone()).or_insert(0.0) += xi * xj;
            }
        }
    }
    (a, b)
}

fn gaussian_elimination_solve(
    a: &mut BTreeMap<String, BTreeMap<String, f64>>,
    b: &mut BTreeMap<String, f64>,
) -> Result<EventScalarMap, String> {
    let event_names: Vec<String> = a.keys().cloned().collect();
    for pivot_name in event_names.iter() {
        let pivot = get_matrix_value(a, pivot_name, pivot_name);
        if pivot.abs() < 1e-18 || !pivot.is_finite() {
            return Err(format!("Singular or invalid pivot for '{pivot_name}' in Ridge solve."));
        }
        let pivot_row = a.get_mut(pivot_name).ok_or_else(|| format!("Missing row for '{pivot_name}'"))?;
        for (_col_name, val) in pivot_row.iter_mut() {
            *val /= pivot;
        }
        let pivot_b = b.get(pivot_name).copied().unwrap_or(0.0) / pivot;
        b.insert(pivot_name.clone(), pivot_b);
        for other_name in event_names.iter() {
            if other_name == pivot_name { continue; }
            let factor = get_matrix_value(a, other_name, pivot_name);
            if factor == 0.0 { continue; }
            let pivot_row_snapshot = a.get(pivot_name).cloned()
                .ok_or_else(|| format!("Missing pivot row snapshot for '{pivot_name}'"))?;
            let other_row = a.get_mut(other_name)
                .ok_or_else(|| format!("Missing row for '{other_name}'"))?;
            for (col_name, pivot_val) in pivot_row_snapshot.iter() {
                let entry = other_row.entry(col_name.clone()).or_insert(0.0);
                *entry -= factor * pivot_val;
            }
            let b_other = b.get(other_name).copied().unwrap_or(0.0);
            let b_pivot = b.get(pivot_name).copied().unwrap_or(0.0);
            b.insert(other_name.clone(), b_other - factor * b_pivot);
            if let Some(v) = other_row.get_mut(pivot_name) {
                if v.abs() < 1e-15 { *v = 0.0; }
            }
        }
    }
    Ok(b.clone())
}

fn get_matrix_value(a: &BTreeMap<String, BTreeMap<String, f64>>, row: &str, col: &str) -> f64 {
    a.get(row).and_then(|r| r.get(col)).copied().unwrap_or(0.0)
}

/* =========================================================================================
   Elastic Net coordinate descent (map-based)
   ========================================================================================= */

fn elastic_net_coordinate_descent_map(
    standardized_event_deltas: &EventSeriesMap,
    db_time_delta: &[f64],
    lambda: f64,
    alpha: f64,
    max_iter: usize,
    tol: f64,
) -> EventScalarMap {
    let sample_count = db_time_delta.len();
    let mut coef_by_event: EventScalarMap = standardized_event_deltas
        .keys().map(|k| (k.clone(), 0.0)).collect();
    let mut residual = db_time_delta.to_vec();
    let mut feature_norm_by_event: EventScalarMap = BTreeMap::new();
    for (event_name, series) in standardized_event_deltas.iter() {
        let mut s = 0.0;
        for v in series.iter() { s += v * v; }
        let mut norm = s / (sample_count as f64).max(1.0);
        if norm == 0.0 || !norm.is_finite() { norm = 1e-12; }
        feature_norm_by_event.insert(event_name.clone(), norm);
    }
    let l1_penalty = lambda * alpha;
    let l2_penalty = lambda * (1.0 - alpha);
    for _ in 0..max_iter {
        let mut max_change: f64 = 0.0;
        for (event_name, feature_series) in standardized_event_deltas.iter() {
            let old_coef = *coef_by_event.get(event_name).unwrap_or(&0.0);
            if old_coef != 0.0 {
                for (t, x_t) in feature_series.iter().enumerate() {
                    residual[t] += x_t * old_coef;
                }
            }
            let mut correlation = 0.0;
            for (t, x_t) in feature_series.iter().enumerate() {
                correlation += x_t * residual[t];
            }
            correlation /= (sample_count as f64).max(1.0);
            let norm = *feature_norm_by_event.get(event_name).unwrap_or(&1e-12);
            let new_coef = soft_threshold(correlation, l1_penalty) / (norm + l2_penalty);
            coef_by_event.insert(event_name.clone(), new_coef);
            if new_coef != 0.0 {
                for (t, x_t) in feature_series.iter().enumerate() {
                    residual[t] -= x_t * new_coef;
                }
            }
            max_change = max_change.max((new_coef - old_coef).abs());
        }
        if max_change < tol { break; }
    }
    coef_by_event
}

fn soft_threshold(value: f64, threshold: f64) -> f64 {
    if value > threshold { value - threshold }
    else if value < -threshold { value + threshold }
    else { 0.0 }
}

/* =========================================================================================
   Huber robust regression via IRLS (map-based)
   ========================================================================================= */

fn huber_regression_map(
    x_by_event: &EventSeriesMap,
    y: &[f64],
    delta: f64,
    max_iter: usize,
    tol: f64,
    ridge_penalty: f64,
) -> EventScalarMap {
    let n = y.len();
    let event_names: Vec<String> = x_by_event.keys().cloned().collect();
    let p = event_names.len();
    if p == 0 || n == 0 {
        return BTreeMap::new();
    }

    let mut beta: Vec<f64> = vec![0.0; p];

    for _iter in 0..max_iter {
        let beta_old = beta.clone();

        // Compute residuals and Huber weights
        let mut weights: Vec<f64> = vec![1.0; n];
        for t in 0..n {
            let mut pred = 0.0;
            for (j, name) in event_names.iter().enumerate() {
                pred += beta[j] * x_by_event[name][t];
            }
            let r = (y[t] - pred).abs();
            weights[t] = if r <= delta { 1.0 } else { delta / r.max(1e-15) };
        }

        // Solve WLS: (X'WX + ridge*I) beta = X'Wy
        let mut xtwx: Vec<Vec<f64>> = vec![vec![0.0; p]; p];
        let mut xtwy: Vec<f64> = vec![0.0; p];

        for t in 0..n {
            let w = weights[t];
            for j in 0..p {
                let xj = x_by_event[&event_names[j]][t];
                xtwy[j] += w * xj * y[t];
                for k in 0..p {
                    xtwx[j][k] += w * xj * x_by_event[&event_names[k]][t];
                }
            }
        }
        for j in 0..p { xtwx[j][j] += ridge_penalty; }
        beta = solve_dense_linear_system(&xtwx, &xtwy);

        let max_change: f64 = beta.iter().zip(beta_old.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        if max_change < tol { break; }
    }

    event_names.into_iter().zip(beta).collect()
}

/* =========================================================================================
   Quantile regression via IRLS (map-based, tau=0.95)
   ========================================================================================= */

fn quantile_regression_irls_map(
    x_by_event: &EventSeriesMap,
    y: &[f64],
    tau: f64,
    max_iter: usize,
    tol: f64,
) -> EventScalarMap {
    let n = y.len();
    let event_names: Vec<String> = x_by_event.keys().cloned().collect();
    let p = event_names.len();
    if p == 0 || n == 0 {
        return BTreeMap::new();
    }

    let mut beta: Vec<f64> = vec![0.0; p];
    let eps = 1e-6; // avoid division by zero

    for _iter in 0..max_iter {
        let beta_old = beta.clone();

        // Compute residuals and asymmetric weights
        let mut weights: Vec<f64> = vec![1.0; n];
        for t in 0..n {
            let mut pred = 0.0;
            for (j, name) in event_names.iter().enumerate() {
                pred += beta[j] * x_by_event[name][t];
            }
            let r = y[t] - pred;
            let abs_r = r.abs().max(eps);
            weights[t] = if r >= 0.0 { tau / abs_r } else { (1.0 - tau) / abs_r };
        }

        // Solve WLS: (X'WX + small_ridge*I) beta = X'Wy
        let mut xtwx: Vec<Vec<f64>> = vec![vec![0.0; p]; p];
        let mut xtwy: Vec<f64> = vec![0.0; p];

        for t in 0..n {
            let w = weights[t];
            for j in 0..p {
                let xj = x_by_event[&event_names[j]][t];
                xtwy[j] += w * xj * y[t];
                for k in 0..p {
                    xtwx[j][k] += w * xj * x_by_event[&event_names[k]][t];
                }
            }
        }
        // Small ridge for numerical stability
        for j in 0..p { xtwx[j][j] += 1e-8; }
        beta = solve_dense_linear_system(&xtwx, &xtwy);

        let max_change: f64 = beta.iter().zip(beta_old.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        if max_change < tol { break; }
    }

    event_names.into_iter().zip(beta).collect()
}

/* =========================================================================================
   Dense linear system solver with partial pivoting
   ========================================================================================= */

fn solve_dense_linear_system(a: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
    let n = b.len();
    if n == 0 { return vec![]; }

    // Build augmented matrix
    let mut aug: Vec<Vec<f64>> = a.iter().enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.push(b[i]);
            r
        }).collect();

    // Forward elimination with partial pivoting
    for col in 0..n {
        let max_row = (col..n)
            .max_by(|&a, &b| aug[a][col].abs().partial_cmp(&aug[b][col].abs()).unwrap())
            .unwrap();
        aug.swap(col, max_row);

        let pivot = aug[col][col];
        if pivot.abs() < 1e-15 { continue; }

        for row in (col + 1)..n {
            let factor = aug[row][col] / pivot;
            for j in col..=n {
                aug[row][j] -= factor * aug[col][j];
            }
        }
    }

    // Back substitution
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = aug[i][n];
        for j in (i + 1)..n {
            sum -= aug[i][j] * x[j];
        }
        x[i] = if aug[i][i].abs() > 1e-15 { sum / aug[i][i] } else { 0.0 };
    }
    x
}

/* =========================================================================================
   Ranking / reporting helpers
   ========================================================================================= */

fn build_ranking(
    coef_by_event: &EventScalarMap,
    mad_by_event: &EventScalarMap,
) -> Vec<EventImpact> {
    let mut ranking = Vec::new();
    for (event_name, coef) in coef_by_event.iter() {
        let mad = *mad_by_event.get(event_name).unwrap_or(&0.0);
        ranking.push(EventImpact {
            event_name: event_name.clone(),
            gradient_coef: *coef,
            impact: coef.abs() * mad,
        });
    }
    ranking.sort_by(|a, b| b.impact.partial_cmp(&a.impact).unwrap());
    ranking
}

/* =========================================================================================
   Cross-model triangulation / classification
   ========================================================================================= */

/// Classify events/stats/SQLs by cross-referencing all 4 model rankings.
/// Returns a list of classifications sorted by confidence (confirmed bottlenecks first).
pub fn cross_model_classify(
    section: &DbTimeGradientSection,
    top_n: usize,
) -> Vec<CrossModelClassification> {
    let ridge_set: HashSet<String> = section.ridge_top.iter()
        .take(top_n)
        .filter(|i| i.impact > 0.0 && i.gradient_coef > 0.0)
        .map(|i| i.event_name.clone())
        .collect();

    let en_set: HashSet<String> = section.elastic_net_top.iter()
        .take(top_n)
        .filter(|i| i.impact > 0.0 && i.gradient_coef > 0.0)
        .map(|i| i.event_name.clone())
        .collect();

    let huber_set: HashSet<String> = section.huber_top.iter()
        .take(top_n)
        .filter(|i| i.impact > 0.0 && i.gradient_coef > 0.0)
        .map(|i| i.event_name.clone())
        .collect();

    let q95_set: HashSet<String> = section.quantile95_top.iter()
        .take(top_n)
        .filter(|i| i.impact > 0.0 && i.gradient_coef > 0.0)
        .map(|i| i.event_name.clone())
        .collect();

    let all_events: HashSet<String> = ridge_set.iter()
        .chain(en_set.iter())
        .chain(huber_set.iter())
        .chain(q95_set.iter())
        .cloned()
        .collect();

    let mut results: Vec<CrossModelClassification> = Vec::new();

    for event in &all_events {
        let in_ridge = ridge_set.contains(event);
        let in_en = en_set.contains(event);
        let in_huber = huber_set.contains(event);
        let in_q95 = q95_set.contains(event);

        let model_count = [in_ridge, in_en, in_huber, in_q95].iter().filter(|&&b| b).count();

        let (classification, priority) = if in_ridge && in_en && in_huber && in_q95 {
            // All 4 models agree
            ("CONFIRMED_BOTTLENECK", 1)
        } else if in_ridge && in_huber && in_q95 && !in_en {
            // Ridge + Huber + Q95 but NOT EN → EN zeroed it due to collinearity with another 
            // dominant event. Still very high confidence since 3 independent models agree 
            // including the tail-risk model.
            ("CONFIRMED_BOTTLENECK_EN_COLLINEAR", 1)
        } else if in_ridge && in_en && in_huber && !in_q95 {
            // Strong across average behavior, not dominant in tail
            ("STRONG_CONTRIBUTOR", 2)
        } else if in_ridge && in_huber && !in_en && !in_q95 {
            // Stable systematic contributor, but EN dropped it (collinear) 
            // and not a tail risk
            ("STABLE_CONTRIBUTOR", 3)
        } else if in_q95 && !in_ridge {
            // Worst-case only — hidden danger
            ("TAIL_RISK", 4)
        } else if in_q95 && in_ridge && !in_huber {
            // High in Ridge and Q95 but NOT in Huber → impact comes from extreme 
            // snapshots that also happen to be the worst ones
            ("TAIL_OUTLIER", 4)
        } else if in_ridge && !in_huber {
            // In Ridge but Huber disagrees → outlier-driven
            ("OUTLIER_DRIVEN", 5)
        } else if in_en && !in_ridge {
            // EN selected it but Ridge didn't rank it high → sparse dominant
            ("SPARSE_DOMINANT", 6)
        } else if in_huber && !in_ridge && !in_en {
            // Only visible when outliers are removed
            ("ROBUST_ONLY", 7)
        } else if model_count >= 2 {
            // At least 2 models, but doesn't match any specific pattern above
            ("MULTI_MODEL_MINOR", 8)
        } else {
            ("SINGLE_MODEL", 9)
        };

        let description = match classification {
            "CONFIRMED_BOTTLENECK" => 
                "Present in ALL 4 models (Ridge, ElasticNet, Huber, Q95). Highest confidence — \
                 systematic, robust bottleneck affecting both average and worst-case DB Time.",
            "CONFIRMED_BOTTLENECK_EN_COLLINEAR" => 
                "Present in Ridge, Huber, and Q95 but NOT in ElasticNet. Very high confidence — \
                 3 independent models agree. ElasticNet likely zeroed it due to collinearity with \
                 another correlated event. Treat as confirmed bottleneck; check EN for which \
                 correlated event was selected instead.",
            "STRONG_CONTRIBUTOR" => 
                "Present in Ridge, ElasticNet, and Huber but not Q95. Reliable systematic \
                 contributor to DB Time, but not especially dominant in tail/worst-case scenarios.",
            "STABLE_CONTRIBUTOR" => 
                "Present in Ridge and Huber (both agree = robust finding) but absent from \
                 ElasticNet (collinearity) and Q95 (not a tail driver). A steady, moderate \
                 contributor to DB Time.",
            "TAIL_RISK" => 
                "Present in Quantile95 but NOT in Ridge. Usually behaves fine but causes \
                 catastrophic DB Time spikes in the worst 5% of snapshots. Investigate \
                 specific peak periods.",
            "TAIL_OUTLIER" => 
                "Present in Ridge and Q95 but NOT in Huber. Impact is concentrated in \
                 extreme snapshots that are also the worst-performing ones. A high-severity \
                 outlier problem — find and fix those specific periods.",
            "OUTLIER_DRIVEN" => 
                "Present in Ridge but NOT in Huber (outlier-resistant). Its apparent impact \
                 is driven by a few extreme snapshots, not systematic behavior. Examine \
                 those specific snapshots.",
            "SPARSE_DOMINANT" => 
                "Present in ElasticNet but NOT in Ridge top. One of a small number of truly \
                 dominant factors selected by L1 sparsity. May be correlated with other \
                 contributors that Ridge spreads weight across.",
            "ROBUST_ONLY" => 
                "Present only in Huber. Stable background contributor visible only when \
                 outliers are downweighted. Low priority but worth monitoring.",
            "MULTI_MODEL_MINOR" => 
                "Appeared in at least 2 models but with no clear dominant pattern. Minor \
                 contributor worth noting.",
            _ => 
                "Appeared in only one model with low confidence.",
        };

        results.push(CrossModelClassification {
            event_name: event.clone(),
            classification: classification.to_string(),
            description: Some(description.to_string()),
            in_ridge,
            in_elastic_net: in_en,
            in_huber,
            in_quantile95: in_q95,
            priority: priority as u8,
        });
    }

    results.sort_by_key(|r| r.priority);
    results
}

/// Print cross-model classification as a console table and return HTML.
pub fn print_cross_model_table(
    classifications: &[CrossModelClassification],
    section_label: &str,
    logfile_name: &str,
    args: &Args,
) -> String {
    make_notes!(logfile_name, args.quiet, 0, "\n{}\n",
        format!("-- Cross-Model Triangulation: {} --", section_label).bold().bright_magenta());

    let mut table = Table::new();
    table.set_titles(Row::new(vec![
        Cell::new("Event/Stat/SQL").with_style(Attr::Bold),
        Cell::new("Classification").with_style(Attr::Bold),
        Cell::new("Ridge").with_style(Attr::Bold),
        Cell::new("EN").with_style(Attr::Bold),
        Cell::new("Huber").with_style(Attr::Bold),
        Cell::new("Q95").with_style(Attr::Bold),
        Cell::new("Description").with_style(Attr::Bold),
    ]));

    for c in classifications {
        let yn = |b: bool| -> &str { if b { "Yes" } else { "-" } };
        if let Some(desc) = c.description.clone() {
            table.add_row(Row::new(vec![
            Cell::new(&c.event_name),
            Cell::new(&c.classification),
            Cell::new(yn(c.in_ridge)),
            Cell::new(yn(c.in_elastic_net)),
            Cell::new(yn(c.in_huber)),
            Cell::new(yn(c.in_quantile95)),
            Cell::new(&desc),
        ]));
        }
        
    }

    for table_line in table.to_string().lines() {
        make_notes!(logfile_name, args.quiet, 0, "{}\n", table_line);
    }

    

    let mut html = table_to_html_string(&table, &format!("Cross-Model Triangulation: {}", section_label), &["Event/Stat/SQL", "Classification", "Ridge", "EN", "Huber", "Q95", "Description"]);
    html = format!(r#"<div>{html}</div>"#);
    html
}

/* =========================================================================================
   Robust statistics helpers (MAD / median)
   ========================================================================================= */

fn mad(values: &[f64]) -> f64 {
    if values.is_empty() { return 0.0; }
    let med = median(values);
    let mut deviations: Vec<f64> = values.iter().map(|v| (v - med).abs()).collect();
    median_in_place(&mut deviations)
}

fn median(values: &[f64]) -> f64 {
    let mut tmp = values.to_vec();
    median_in_place(&mut tmp)
}

fn median_in_place(values: &mut [f64]) -> f64 {
    if values.is_empty() { return 0.0; }
    let mid = values.len() / 2;
    values.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        let lower_max = values[..mid].iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (lower_max + values[mid]) * 0.5
    }
}

/* =========================================================================================
   build_db_time_gradient_section — now includes Huber + Q95 + cross-model
   ========================================================================================= */

pub fn build_db_time_gradient_section(
    db_time_series: &[f64],
    event_series: &BTreeMap<String, Vec<f64>>,
    ridge_lambda: f64,
    elastic_net_lambda: f64,
    elastic_net_alpha: f64,
    elastic_net_max_iter: usize,
    elastic_net_tol: f64,
    units_desc: &str,
) -> Result<DbTimeGradientSection, String> {
    println!("\n\nBuilding gradient for {units_desc} - {} stats", event_series.len());
    if db_time_series.len() < 3 {
        return Err("Not enough DB Time samples (need >= 3).".into());
    }
    if event_series.is_empty() {
        return Err("No events provided (empty map).".into());
    }
    let expected_len = db_time_series.len();
    for (event_name, series) in event_series.iter() {
        if series.len() != expected_len {
            return Err(format!(
                "Event '{}' length mismatch: got {}, expected {}.",
                event_name, series.len(), expected_len
            ));
        }
    }

    let gradient_result = compute_db_time_gradient(
        db_time_series, event_series,
        ridge_lambda, elastic_net_lambda, elastic_net_alpha,
        elastic_net_max_iter, elastic_net_tol,
    )?;

    let make_top = |ranking: &[EventImpact], filter_zero: bool| -> Vec<GradientTopItem> {
        ranking.iter()
            .filter(|x| !filter_zero || x.gradient_coef != 0.0)
            .take(50)
            .map(|x| GradientTopItem {
                event_name: x.event_name.clone(),
                gradient_coef: x.gradient_coef,
                impact: x.impact,
            })
            .collect()
    };

    let ridge_top = make_top(&gradient_result.ridge_ranking, false);
    let elastic_net_top = make_top(&gradient_result.elastic_net_ranking, true);
    let huber_top = make_top(&gradient_result.huber_ranking, false);
    let quantile95_top = make_top(&gradient_result.quantile95_ranking, false);

    let mut section = DbTimeGradientSection {
        settings: GradientSettings {
            ridge_lambda,
            elastic_net_lambda,
            elastic_net_alpha,
            elastic_net_max_iter,
            elastic_net_tol,
            input_wait_event_unit: units_desc.to_string(),
            input_db_time_unit: "db_time_per_second".to_string(),
        },
        ridge_top,
        elastic_net_top,
        huber_top,
        quantile95_top,
        cross_model_classifications: Vec::new(),
    };

    //Compute cross-model triangulation
    section.cross_model_classifications = cross_model_classify(&section, 20);

    Ok(section)
}

/* =========================================================================================
   Print functions
   ========================================================================================= */

pub fn print_db_time_gradient_tables(section: &DbTimeGradientSection, print_settings: bool, logfile_name: &str, args: &Args) -> String {
    make_notes!(logfile_name, args.quiet, 0, "\n{} \n\t- {}", "==== DB TIME GRADIENT (Ridge / Elastic Net / Huber / Quantile95) ====".bold().bright_cyan(), section.settings.input_wait_event_unit);

    if print_settings {
        let mut settings_table = Table::new();
        settings_table.set_titles(Row::new(vec![
            Cell::new("Setting").with_style(Attr::Bold),
            Cell::new("Value").with_style(Attr::Bold),
        ]));
        settings_table.add_row(Row::new(vec![Cell::new("ridge_lambda"), Cell::new(&format!("{:.6}", section.settings.ridge_lambda))]));
        settings_table.add_row(Row::new(vec![Cell::new("elastic_net_lambda"), Cell::new(&format!("{:.6}", section.settings.elastic_net_lambda))]));
        settings_table.add_row(Row::new(vec![Cell::new("elastic_net_alpha"), Cell::new(&format!("{:.6}", section.settings.elastic_net_alpha))]));
        settings_table.add_row(Row::new(vec![Cell::new("elastic_net_max_iter"), Cell::new(&format!("{}", section.settings.elastic_net_max_iter))]));
        settings_table.add_row(Row::new(vec![Cell::new("elastic_net_tol"), Cell::new(&format!("{:.6e}", section.settings.elastic_net_tol))]));
        settings_table.add_row(Row::new(vec![Cell::new("input_event_unit"), Cell::new(&section.settings.input_wait_event_unit)]));
        settings_table.add_row(Row::new(vec![Cell::new("input_db_time_unit"), Cell::new(&section.settings.input_db_time_unit)]));
        make_notes!(logfile_name, args.quiet, 0, "{}", "\n-- Settings --".bold().bright_white());
        for table_line in settings_table.to_string().lines() {
            make_notes!(logfile_name, args.quiet, 0, "{}\n", table_line);
        }
    }

    let mut gradient_html = r#"<div class="tables-grid">"#.to_string();
    // Ridge
    make_notes!(logfile_name, args.quiet, 0, "{}", "\n-- Ridge TOP --\n".bold().bright_white());
    let r_html = print_top_items_table("Ridge", &section.ridge_top, logfile_name, args);
    gradient_html += &format!(r#"<div>{}</div>"#, r_html);

    // Elastic Net
    make_notes!(logfile_name, args.quiet, 0, "{}", "\n-- Elastic Net TOP --\n".bold().bright_white());
    let en_nonzero: Vec<GradientTopItem> = section.elastic_net_top.iter()
        .cloned().filter(|x| x.gradient_coef != 0.0).collect();
    if en_nonzero.is_empty() {
        make_notes!(logfile_name, args.quiet, 0, "{}", "Elastic Net produced no non-zero coefficients.\n".yellow());
    } else {
        let en_html = print_top_items_table("ElasticNet", &en_nonzero, logfile_name, args);
        gradient_html += &format!(r#"<div>{}</div>"#, en_html);
    }

    //Huber
    make_notes!(logfile_name, args.quiet, 0, "{}", "\n-- Huber Robust TOP --\n".bold().bright_white());
    let huber_html = print_top_items_table("Huber", &section.huber_top, logfile_name, args);
    gradient_html += &format!(r#"<div>{}</div>"#, huber_html);

    //Quantile 95
    make_notes!(logfile_name, args.quiet, 0, "{}", "\n-- Quantile 95 TOP (worst 5% of snapshots) --\n".bold().bright_white());
    let q95_html = print_top_items_table("Quantile95", &section.quantile95_top, logfile_name, args);
    gradient_html += &format!(r#"<div>{}</div>"#, q95_html);

    gradient_html.push_str("</div>");
    //Cross-model triangulation
    if !section.cross_model_classifications.is_empty() {
        let cross_html = print_cross_model_table(
            &section.cross_model_classifications,
            &section.settings.input_wait_event_unit,
            logfile_name,
            args,
        );
        gradient_html.push_str(&format!(r#"<div class="cross-model">{}</div>"#, cross_html));
    }
    gradient_html
}

pub fn print_top_items_table(title: &str, items: &[GradientTopItem], logfile_name: &str, args: &Args) -> String {
    let mut table = Table::new();
    table.set_titles(Row::new(vec![
        Cell::new("#").with_style(Attr::Bold),
        Cell::new("Wait Event/Statistic").with_style(Attr::Bold),
        Cell::new("Coef").with_style(Attr::Bold),
        Cell::new("Impact").with_style(Attr::Bold),
    ]));
    for (idx, item) in items.iter().enumerate() {
        table.add_row(Row::new(vec![
            Cell::new(&format!("{}", idx + 1)),
            Cell::new(&item.event_name),
            Cell::new(&format!("{:+.6}", item.gradient_coef)),
            Cell::new(&format!("{:.6}", item.impact)),
        ]));
    }
    make_notes!(logfile_name, args.quiet, 0, "{}", format!("{} table (Top {})\n", title, items.len()).bright_black());
    for table_line in table.to_string().lines() {
        make_notes!(logfile_name, args.quiet, 0, "{}\n", table_line);
    }
    let mut html = table_to_html_string(&table, title, &["#", "Wait Event/Statistic", "Coef", "Impact"]);
    html = format!(r#"<div>{html}</div>"#);
    html
}