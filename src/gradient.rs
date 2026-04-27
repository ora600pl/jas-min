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
                        DbTimeGradientSection,
                        VifDiagnostic,
                        CollinearGroupImpact};

use prettytable::{Table, Row, Cell, format, Attr};
use colored::*;
use crate::tools::*;
use rayon::prelude::*;
use std::time::Instant;


/// Named time series: event_name/stat_name/sqlid -> Vec<sample_value>
/// Each Vec must have the same length as DB Time series.
pub type EventSeriesMap = BTreeMap<String, Vec<f64>>;

/// Named vector: event_name/stat_name/sqlid -> scalar_value (coef, impact, mean, std, MAD, etc.)
pub type EventScalarMap = BTreeMap<String, f64>;

#[derive(Debug, Clone)]
pub struct EventImpact {
    pub event_name: String,
    /// Regression coefficient on standardized Δ
    pub gradient_coef: f64,

    /// Legacy/typical impact = |coef| * MAD(Δx)
    /// Measures contribution during *typical* variability.
    pub impact: f64,
    /// Signed version of `impact` (preserves direction).
    pub signed_impact: f64,

    /// Active impact = |coef| * P90(|Δx|)
    /// Measures contribution when the predictor is *actively moving*.
    /// **Primary metric for DB tuning prioritization.**
    pub impact_active: f64,
    /// Signed active impact (positive = true bottleneck contributor).
    pub signed_impact_active: f64,

    /// Peak impact = |coef| * P99(|Δx|)
    /// Measures worst-case single-snapshot contribution.
    pub impact_peak: f64,

    /// Share of total active impact across all predictors in this ranking.
    /// Range: [0.0, 1.0]. Computed after ranking is built.
    pub impact_share: f64,
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

    /// Variance Inflation Factor for each predictor
    pub vif_by_event: EventScalarMap,
    /// Grouped impacts for collinear predictor clusters
    /// Each entry: (group_member_names, group_impact, group_coef)
    pub collinear_groups: Vec<(Vec<String>, f64, f64)>,
}

//This will be usable for task collection to make parallel threads with Rayon
#[derive(Debug)]
enum RegressionResult {
    Ridge(Result<EventScalarMap, String>),
    ElasticNet(EventScalarMap),
    Huber(EventScalarMap),
    Quantile95(EventScalarMap),
}

fn compute_abs_percentile_by_event(
    series_by_event: &EventSeriesMap,
    p: f64,
) -> EventScalarMap {
    series_by_event.iter()
        .map(|(name, series)| (name.clone(), abs_percentile(series, p)))
        .collect()
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

    let db_time_delta_raw = compute_time_deltas(db_time_series);
    // Center target variable (implicit intercept)
    let y_mean = db_time_delta_raw.iter().sum::<f64>() / db_time_delta_raw.len() as f64;
    let db_time_delta: Vec<f64> = db_time_delta_raw.iter().map(|&y| y - y_mean).collect();
    let event_delta_by_event = compute_event_deltas(event_series)?;
    let event_delta_mean_by_event = compute_mean_by_event(&event_delta_by_event);
    let event_delta_std_by_event = compute_std_by_event(&event_delta_by_event, &event_delta_mean_by_event);
    let event_delta_standardized_by_event =
        standardize_by_event(&event_delta_by_event, &event_delta_mean_by_event, &event_delta_std_by_event);
    let event_delta_mad_by_event = compute_mad_by_event(&event_delta_by_event);
    let event_delta_p90_by_event = compute_abs_percentile_by_event(&event_delta_by_event, 0.90);
    let event_delta_p99_by_event = compute_abs_percentile_by_event(&event_delta_by_event, 0.99);

 
    let tasks: Vec<u8> = vec![0, 1, 2, 3]; //4 tasks - 4 models

    // Compute Huber delta from median residuals (intercept-only model)
    // This makes Huber independent of Ridge quality
    let y_median_val = median(&db_time_delta);
    let median_residuals: Vec<f64> = db_time_delta.iter().map(|&y| y - y_median_val).collect();
    let huber_delta = (1.345 * mad(&median_residuals)).max(1e-6);

    let start = Instant::now(); //for counting duration of models computation
    //parallel regression calculation
    let results: Vec<RegressionResult> = tasks.par_iter().map(|&task_id| {
            match task_id {
                0 => RegressionResult::Ridge(ridge_regression_map(
                    &event_delta_standardized_by_event,
                    &db_time_delta,
                    ridge_lambda,
                )),
                1 => RegressionResult::ElasticNet(elastic_net_coordinate_descent_map(
                    &event_delta_standardized_by_event,
                    &db_time_delta,
                    elastic_net_lambda,
                    elastic_net_alpha,
                    elastic_net_max_iter,
                    elastic_net_tol,
                )),
                2 => RegressionResult::Huber(huber_regression_map(
                    &event_delta_standardized_by_event,
                    &db_time_delta,
                    huber_delta,
                    100,
                    elastic_net_tol,
                    ridge_lambda,
                )),
                3 => RegressionResult::Quantile95(quantile_regression_irls_map(
                    &event_delta_standardized_by_event,
                    &db_time_delta,
                    0.95,
                    200,
                    elastic_net_tol,
                    ridge_lambda,
                )),
                _ => unreachable!(),
            }
        }).collect();

    
    //Unpacking results
    let mut ridge_gradient_by_event = None;
    let mut elastic_net_gradient_by_event = None;
    let mut huber_gradient_by_event = None;
    let mut quantile95_gradient_by_event = None;

    for result in results {
        match result {
            RegressionResult::Ridge(r) => ridge_gradient_by_event = Some(r?),
            RegressionResult::ElasticNet(m) => elastic_net_gradient_by_event = Some(m),
            RegressionResult::Huber(m) => huber_gradient_by_event = Some(m),
            RegressionResult::Quantile95(m) => quantile95_gradient_by_event = Some(m),
        }
    }

    let ridge_gradient_by_event = ridge_gradient_by_event.unwrap();
    let elastic_net_gradient_by_event = elastic_net_gradient_by_event.unwrap();
    let huber_gradient_by_event = huber_gradient_by_event.unwrap();
    let quantile95_gradient_by_event = quantile95_gradient_by_event.unwrap();

    let ridge_ranking = build_ranking(&ridge_gradient_by_event, &event_delta_mad_by_event, &event_delta_p90_by_event, &event_delta_p99_by_event);
    let elastic_net_ranking = build_ranking(&elastic_net_gradient_by_event, &event_delta_mad_by_event, &event_delta_p90_by_event, &event_delta_p99_by_event);
    let huber_ranking = build_ranking(&huber_gradient_by_event, &event_delta_mad_by_event, &event_delta_p90_by_event, &event_delta_p99_by_event);
    let quantile95_ranking = build_ranking(&quantile95_gradient_by_event, &event_delta_mad_by_event, &event_delta_p90_by_event, &event_delta_p99_by_event);

    // VIF diagnostics
    let vif_by_event = compute_vif(&event_delta_standardized_by_event);
    for (event, vif) in &vif_by_event {
        if *vif > 10.0 {
            println!("  ⚠️  VIF({}) = {:.1} — severe multicollinearity", event, vif);
        }
    }
    let end = Instant::now(); //for counting duration of models computation
    let duration = end.duration_since(start);

    println!(" [Elapsed: {}ms]", duration.as_millis());

    // Grouped impacts for collinear clusters
    let collinear_groups = compute_grouped_impacts(
        &event_delta_standardized_by_event,
        &event_delta_by_event,
        &db_time_delta,
        &vif_by_event,
        10.0,  // VIF threshold
        0.8,   // correlation threshold for grouping
    );

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
        vif_by_event,
        collinear_groups,
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

fn compute_std_by_event(
    series_by_event: &EventSeriesMap,
    mean_by_event: &EventScalarMap,
) -> EventScalarMap {
    let mut std_by_event = BTreeMap::new();
    for (event_name, series) in series_by_event.iter() {
        let mean_val = *mean_by_event.get(event_name).unwrap_or(&0.0);
        let mut var = 0.0;
        for v in series.iter() {
            let d = *v - mean_val;
            var += d * d;
        }
        let n = series.len() as f64;
        // Bessel's correction: N-1 for sample variance
        let denom = if n > 1.0 { n - 1.0 } else { 1.0 };
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
        let mad_val = mad(series);
        mad_by_event.insert(event_name.clone(), mad_val);
    }
    mad_by_event
}

/* =========================================================================================
   Ridge regression (map-based)
   ========================================================================================= */

fn ridge_regression_map (
    standardized_event_deltas: &EventSeriesMap,
    db_time_delta: &[f64],
    lambda: f64,
) -> Result<EventScalarMap, String> {

    println!("  -> Building Ridge regression");

    let n = db_time_delta.len();
    let event_names: Vec<String> = standardized_event_deltas.keys().cloned().collect();
    let p = event_names.len();

    if p == 0 || n == 0 {
        return Err("Empty input for Ridge regression.".into());
    }

    for (name, series) in standardized_event_deltas.iter() {
        if series.len() != n {
            return Err(format!(
                "Standardized delta length mismatch for '{}': got {}, expected {}.",
                name,
                series.len(),
                n
            ));
        }
    }

    // Build X'X + lambda*I  and  X'y  using dense arrays
    let mut xtx: Vec<Vec<f64>> = vec![vec![0.0; p]; p];
    let mut xty: Vec<f64> = vec![0.0; p];

    for t in 0..n {
        let yt = db_time_delta[t];
        for j in 0..p {
            let xj = standardized_event_deltas[&event_names[j]][t];
            xty[j] += xj * yt;
            for k in j..p {
                let xk = standardized_event_deltas[&event_names[k]][t];
                let val = xj * xk;
                xtx[j][k] += val;
                if j != k {
                    xtx[k][j] += val;
                }
            }
        }
    }

    // Ridge penalty on diagonal
    for j in 0..p {
        xtx[j][j] += lambda;
    }

    let beta = solve_dense_linear_system(&xtx, &xty);

    Ok(event_names.into_iter().zip(beta).collect())
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

    println!("  -> Building Elastic Net regression");

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
    println!("  -> Building Huber robust regression");

    let n = y.len();
    let event_names: Vec<String> = x_by_event.keys().cloned().collect();
    let p = event_names.len();
    if p == 0 || n == 0 {
        return BTreeMap::new();
    }

    // Pre-materialize columns
    let columns: Vec<&[f64]> = event_names.iter()
        .map(|name| x_by_event[name].as_slice())
        .collect();

    let mut beta: Vec<f64> = vec![0.0; p];

    // Pre-allocate, reuse
    let mut xtwx: Vec<f64> = vec![0.0; p * p];
    let mut xtwy: Vec<f64> = vec![0.0; p];
    let mut weights: Vec<f64> = vec![1.0; n];

    for _iter in 0..max_iter {
        let beta_old = beta.clone();

        // Compute Huber weights
        for t in 0..n {
            let mut pred = 0.0;
            for j in 0..p {
                pred += beta[j] * columns[j][t];
            }
            let r = (y[t] - pred).abs();
            weights[t] = if r <= delta { 1.0 } else { delta / r.max(1e-15) };
        }

        xtwx.iter_mut().for_each(|v| *v = 0.0);
        xtwy.iter_mut().for_each(|v| *v = 0.0);

        for t in 0..n {
            let w = weights[t];
            let wyt = w * y[t];
            for j in 0..p {
                let xj = columns[j][t];
                let wxj = w * xj;
                xtwy[j] += xj * wyt;
                for k in j..p {
                    xtwx[j * p + k] += wxj * columns[k][t];
                }
            }
        }

        for j in 0..p {
            for k in (j + 1)..p {
                xtwx[k * p + j] = xtwx[j * p + k];
            }
            xtwx[j * p + j] += ridge_penalty;
        }

        solve_dense_linear_system_flat(&mut xtwx, &mut xtwy, p, &mut beta);

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
    ridge_penalty: f64,
) -> EventScalarMap {
    println!("  -> Building Quantile regression tau={}", tau);

    let n = y.len();
    let event_names: Vec<String> = x_by_event.keys().cloned().collect();
    let p = event_names.len();
    if p == 0 || n == 0 {
        return BTreeMap::new();
    }

    // Pre-materialize columns ONCE — eliminate all BTreeMap lookups
    let columns: Vec<&[f64]> = event_names.iter()
        .map(|name| x_by_event[name].as_slice())
        .collect();

    let mut beta: Vec<f64> = vec![0.0; p];
    let eps = 1e-6;
    let q_ridge = (ridge_penalty * 0.01).max(1e-6);

    // Pre-allocate matrices ONCE, reuse across iterations
    let mut xtwx: Vec<f64> = vec![0.0; p * p];  // flat layout for cache
    let mut xtwy: Vec<f64> = vec![0.0; p];
    let mut weights: Vec<f64> = vec![1.0; n];

    for _iter in 0..max_iter {
        let beta_old = beta.clone();

        // Compute weights
        for t in 0..n {
            let mut pred = 0.0;
            for j in 0..p {
                pred += beta[j] * columns[j][t];
            }
            let r = y[t] - pred;
            let abs_r = r.abs().max(eps);
            weights[t] = if r >= 0.0 { tau / abs_r } else { (1.0 - tau) / abs_r };
        }

        // Zero out — much faster than reallocating
        xtwx.iter_mut().for_each(|v| *v = 0.0);
        xtwy.iter_mut().for_each(|v| *v = 0.0);

        // Build X'WX (symmetric, upper triangle) + X'Wy
        for t in 0..n {
            let w = weights[t];
            let wyt = w * y[t];
            for j in 0..p {
                let xj = columns[j][t];
                let wxj = w * xj;
                xtwy[j] += xj * wyt;
                // Upper triangle only, flat indexing
                for k in j..p {
                    xtwx[j * p + k] += wxj * columns[k][t];
                }
            }
        }

        // Mirror upper triangle to lower + add ridge
        for j in 0..p {
            for k in (j + 1)..p {
                xtwx[k * p + j] = xtwx[j * p + k];
            }
            xtwx[j * p + j] += q_ridge;
        }

        // Solve in-place (reusing xtwx as augmented matrix)
        solve_dense_linear_system_flat(&mut xtwx, &mut xtwy, p, &mut beta);

        let max_change: f64 = beta.iter().zip(beta_old.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        if max_change < tol {
            break;
        }
    }

    event_names.into_iter().zip(beta).collect()
}

/* =========================================================================================
   Dense linear system solver with partial pivoting
   ========================================================================================= */
/// Solves Ax = b in-place. 
/// `a` is p×p flat row-major (DESTROYED during solve).
/// `b` is the RHS (DESTROYED, becomes scratch).
/// `x` receives the solution.
fn solve_dense_linear_system_flat(a: &mut [f64], b: &mut [f64], p: usize, x: &mut [f64]) {
    if p == 0 { return; }

    // We need the augmented column, but instead of building [A|b],
    // we keep b separate and apply the same row operations.

    // Forward elimination with partial pivoting
    for col in 0..p {
        // Find pivot
        let mut max_row = col;
        let mut max_val = a[col * p + col].abs();
        for row in (col + 1)..p {
            let val = a[row * p + col].abs();
            if val > max_val {
                max_val = val;
                max_row = row;
            }
        }

        // Swap rows in a and b
        if max_row != col {
            for j in 0..p {
                a.swap(col * p + j, max_row * p + j);
            }
            b.swap(col, max_row);
        }

        let pivot = a[col * p + col];
        if pivot.abs() < 1e-15 { continue; }

        for row in (col + 1)..p {
            let factor = a[row * p + col] / pivot;
            for j in col..p {
                a[row * p + j] -= factor * a[col * p + j];
            }
            b[row] -= factor * b[col];
        }
    }

    // Back substitution
    for i in (0..p).rev() {
        let mut sum = b[i];
        for j in (i + 1)..p {
            sum -= a[i * p + j] * x[j];
        }
        x[i] = if a[i * p + i].abs() > 1e-15 { sum / a[i * p + i] } else { 0.0 };
    }
}

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

/* =========================================================================================
   Variance Inflation Factor (VIF) diagnostics
   ========================================================================================= */

/// Compute Variance Inflation Factor for each predictor.
/// VIF_j = 1 / (1 - R²_j), where R²_j is from regressing x_j on all other x's.
/// VIF > 10 indicates severe multicollinearity.
pub fn compute_vif(x_by_event: &EventSeriesMap) -> EventScalarMap {
    println!("  -> Computing VIF");
    let event_names: Vec<String> = x_by_event.keys().cloned().collect();
    let p = event_names.len();
    let n = x_by_event
        .values()
        .next()
        .map(|v| v.len())
        .unwrap_or(0);

    if p <= 1 || n < p + 1 {
        return event_names.iter().map(|e| (e.clone(), 1.0)).collect();
    }

    let vif_entries: Vec<(String, f64)> = event_names.par_iter().map(|target_name| {
        let y_j: &[f64] = &x_by_event[target_name];

        let other_names: Vec<&String> = event_names
            .iter()
            .filter(|name| *name != target_name)
            .collect();
        let q = other_names.len();

        let mut xtx: Vec<Vec<f64>> = vec![vec![0.0; q]; q];
        let mut xty: Vec<f64> = vec![0.0; q];
        let mut yty: f64 = 0.0;
        let mut y_sum: f64 = 0.0;

        for t in 0..n {
            let yt = y_j[t];
            y_sum += yt;
            yty += yt * yt;
            for a in 0..q {
                let xa = x_by_event[other_names[a]][t];
                xty[a] += xa * yt;
                for b in a..q {
                    let xb = x_by_event[other_names[b]][t];
                    let val = xa * xb;
                    xtx[a][b] += val;
                    if a != b {
                        xtx[b][a] += val;
                    }
                }
            }
        }
        // Tiny ridge for numerical stability in VIF auxiliary regressions
        for a in 0..q {
            xtx[a][a] += 1e-8;
        }

        let beta = solve_dense_linear_system(&xtx, &xty);

        let y_mean_val = y_sum / n as f64;
        let ss_tot = yty - n as f64 * y_mean_val * y_mean_val;

        let mut ss_res = 0.0;
        for t in 0..n {
            let mut pred = 0.0;
            for a in 0..q {
                pred += beta[a] * x_by_event[other_names[a]][t];
            }
            let r = y_j[t] - pred;
            ss_res += r * r;
        }

        let r_squared = if ss_tot > 1e-15 {
            (1.0 - ss_res / ss_tot).max(0.0)
        } else {
            0.0
        };
        let vif = if r_squared < 1.0 - 1e-15 {
            1.0 / (1.0 - r_squared)
        } else {
            1e6
        };

        (target_name.clone(), vif)
    }).collect();

    vif_entries.into_iter().collect()
}

/// Groups collinear events (VIF > threshold) by pairwise correlation,
/// then computes a single univariate coefficient for each group's combined signal.
pub fn compute_grouped_impacts(
    event_delta_standardized: &EventSeriesMap,
    event_delta_raw: &EventSeriesMap,
    db_time_delta: &[f64],
    vif_by_event: &EventScalarMap,
    vif_threshold: f64,
    corr_threshold: f64,
) -> Vec<(Vec<String>, f64, f64)> {
    // Returns: Vec<(group_member_names, group_impact, group_coef)>

    let high_vif: Vec<String> = vif_by_event
        .iter()
        .filter(|(_, &v)| v > vif_threshold)
        .map(|(name, _)| name.clone())
        .collect();

    if high_vif.is_empty() {
        return vec![];
    }

    let n = db_time_delta.len();
    if n == 0 {
        return vec![];
    }

    // Greedy correlation-based clustering
    let mut groups: Vec<HashSet<String>> = Vec::new();

    for event in &high_vif {
        let series_a = match event_delta_standardized.get(event) {
            Some(s) => s,
            None => continue,
        };
        let mut found_group = false;

        for group in groups.iter_mut() {
            let representative = group.iter().next().unwrap();
            let series_b = match event_delta_standardized.get(representative) {
                Some(s) => s,
                None => continue,
            };

            let nf = series_a.len() as f64;
            if nf == 0.0 {
                continue;
            }
            let mean_a: f64 = series_a.iter().sum::<f64>() / nf;
            let mean_b: f64 = series_b.iter().sum::<f64>() / nf;
            let mut cov = 0.0;
            let mut var_a = 0.0;
            let mut var_b = 0.0;
            for i in 0..series_a.len() {
                let da = series_a[i] - mean_a;
                let db = series_b[i] - mean_b;
                cov += da * db;
                var_a += da * da;
                var_b += db * db;
            }
            let corr = if var_a > 0.0 && var_b > 0.0 {
                cov / (var_a * var_b).sqrt()
            } else {
                0.0
            };

            if corr.abs() > corr_threshold {
                group.insert(event.clone());
                found_group = true;
                break;
            }
        }

        if !found_group {
            let mut new_group = HashSet::new();
            new_group.insert(event.clone());
            groups.push(new_group);
        }
    }

    let mut results: Vec<(Vec<String>, f64, f64)> = Vec::new();

    for group in &groups {
        if group.len() < 2 {
            continue;
        }

        // Sum raw deltas within group
        let mut combined: Vec<f64> = vec![0.0; n];
        for event_name in group {
            if let Some(raw_series) = event_delta_raw.get(event_name) {
                for t in 0..n.min(raw_series.len()) {
                    combined[t] += raw_series[t];
                }
            }
        }

        // Univariate regression: beta = cov(combined, y) / var(combined)
        let mean_c: f64 = combined.iter().sum::<f64>() / n as f64;
        let mean_y: f64 = db_time_delta.iter().sum::<f64>() / n as f64;
        let mut cov_cy = 0.0;
        let mut var_c = 0.0;
        for t in 0..n {
            let dc = combined[t] - mean_c;
            let dy = db_time_delta[t] - mean_y;
            cov_cy += dc * dy;
            var_c += dc * dc;
        }
        let group_coef = if var_c > 0.0 { cov_cy / var_c } else { 0.0 };
        let group_mad = mad(&combined);
        let group_impact = group_coef.abs() * group_mad;

        let mut names: Vec<String> = group.iter().cloned().collect();
        names.sort();
        results.push((names, group_impact, group_coef));
    }

    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    results
}

fn build_ranking(
    coef_by_event: &EventScalarMap,
    mad_by_event: &EventScalarMap,
    p90_by_event: &EventScalarMap,
    p99_by_event: &EventScalarMap,
) -> Vec<EventImpact> {
    let mut ranking: Vec<EventImpact> = coef_by_event.iter().map(|(event_name, coef)| {
        let mad_val = *mad_by_event.get(event_name).unwrap_or(&0.0);
        let p90_val = *p90_by_event.get(event_name).unwrap_or(&0.0);
        let p99_val = *p99_by_event.get(event_name).unwrap_or(&0.0);

        let abs_coef = coef.abs();

        EventImpact {
            event_name: event_name.clone(),
            gradient_coef: *coef,
            impact: abs_coef * mad_val,
            signed_impact: *coef * mad_val,
            impact_active: abs_coef * p90_val,
            signed_impact_active: *coef * p90_val,
            impact_peak: abs_coef * p99_val,
            impact_share: 0.0, // fill below
        }
    }).collect();

    // Compute share of active impact (only over positive contributors —
    // negative coefs are usually confounders, not bottlenecks)
    let total_positive_active: f64 = ranking.iter()
        .filter(|r| r.gradient_coef > 0.0)
        .map(|r| r.impact_active)
        .sum();

    if total_positive_active > 1e-15 {
        for r in ranking.iter_mut() {
            if r.gradient_coef > 0.0 {
                r.impact_share = r.impact_active / total_positive_active;
            }
        }
    }

    // Primary sort: signed_impact_active DESC
    // This puts real bottlenecks (positive coef × active magnitude) on top,
    // and suppressors (negative coef) at the bottom.
    ranking.sort_by(|a, b| {
        b.signed_impact_active
            .partial_cmp(&a.signed_impact_active)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
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
        .filter(|i| i.impact >= 0.0 && i.gradient_coef > 0.0)
        .map(|i| i.event_name.clone())
        .collect();

    let en_set: HashSet<String> = section.elastic_net_top.iter()
        .take(top_n)
        .filter(|i| i.impact >= 0.0 && i.gradient_coef > 0.0)
        .map(|i| i.event_name.clone())
        .collect();

    let huber_set: HashSet<String> = section.huber_top.iter()
        .take(top_n)
        .filter(|i| i.impact >= 0.0 && i.gradient_coef > 0.0)
        .map(|i| i.event_name.clone())
        .collect();

    let q95_set: HashSet<String> = section.quantile95_top.iter()
        .take(top_n)
        .filter(|i| i.impact >= 0.0 && i.gradient_coef > 0.0)
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
        let ridge_impact = section.ridge_top.iter() 
                                               .find_map(|r| (&r.event_name == event).then(|| r.impact_active))
                                               .unwrap_or(0.0);
        let en_impact = section.elastic_net_top.iter() 
                                               .find_map(|r| (&r.event_name == event).then(|| r.impact_active))
                                               .unwrap_or(0.0);
        let huber_impact = section.huber_top.iter() 
                                               .find_map(|r| (&r.event_name == event).then(|| r.impact_active))
                                               .unwrap_or(0.0);
        let q95_impact = section.quantile95_top.iter() 
                                               .find_map(|r| (&r.event_name == event).then(|| r.impact_active))
                                               .unwrap_or(0.0);
        let combined_impact = ridge_impact + en_impact + huber_impact + q95_impact;

        let (classification, priority) = if in_ridge && in_en && in_huber && in_q95 {
            // All 4 models agree
            ("CONFIRMED_BOTTLENECK", 0)
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
            combined_impact: combined_impact,
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
        Cell::new("Combined Impact").with_style(Attr::Bold),
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
            Cell::new(&format!("{:.2}",c.combined_impact)),
            Cell::new(&desc),
        ]));
        }
        
    }


    let mut html = table_to_html_string(&table, &format!("Cross-Model Triangulation: {}", section_label), &["Event/Stat/SQL", "Classification", "Ridge", "EN", "Huber", "Q95", "Combined Impact","Description"]);
    html = format!(r#"<div>{html}</div>"#);

    /* Removing description before printing on screen, because it doesn't look good */
    for row in table.row_iter_mut() {
        row.remove_cell(6);
    }

    for table_line in table.to_string().lines() {
        make_notes!(logfile_name, args.quiet, 0, "{}\n", table_line);
    }

    html
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
                impact_active: x.impact_active,
                impact_peak: x.impact_peak,
                impact_share: x.impact_share,
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
        vif_diagnostics: Vec::new(),
        collinear_group_impacts: Vec::new(),
    };

    //Compute cross-model triangulation
    section.cross_model_classifications = cross_model_classify(&section, 50);

    // VIF diagnostics
    section.vif_diagnostics = gradient_result.vif_by_event.iter()
        .filter(|(_, &vif)| vif > 5.0)  // Only report VIF > 5 (moderate+)
        .map(|(name, &vif)| {
            let interpretation = if vif > 100.0 {
                "SEVERE_COLLINEARITY".to_string()
            } else if vif > 10.0 {
                "HIGH_COLLINEARITY".to_string()
            } else {
                "MODERATE_COLLINEARITY".to_string()
            };
            VifDiagnostic {
                event_name: name.clone(),
                vif,
                interpretation,
            }
        })
        .collect();
    section.vif_diagnostics.sort_by(|a, b| b.vif.partial_cmp(&a.vif).unwrap());

    // Collinear group impacts
    section.collinear_group_impacts = gradient_result.collinear_groups.iter()
        .map(|(names, impact, coef)| {
            CollinearGroupImpact {
                group_members: names.clone(),
                combined_impact: *impact,
                combined_coef: *coef,
            }
        })
        .collect();

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

    // VIF diagnostics table
    if !section.vif_diagnostics.is_empty() {
        make_notes!(logfile_name, args.quiet, 0, "{}",
            "\n-- VIF Diagnostics (Multicollinearity) --\n".bold().bright_yellow());
        let mut vif_table = Table::new();
        vif_table.set_titles(Row::new(vec![
            Cell::new("Event/Stat/SQL").with_style(Attr::Bold),
            Cell::new("VIF").with_style(Attr::Bold),
            Cell::new("Interpretation").with_style(Attr::Bold),
        ]));
        for vd in &section.vif_diagnostics {
            vif_table.add_row(Row::new(vec![
                Cell::new(&vd.event_name),
                Cell::new(&format!("{:.1}", vd.vif)),
                Cell::new(&vd.interpretation),
            ]));
        }
        for table_line in vif_table.to_string().lines() {
            make_notes!(logfile_name, args.quiet, 0, "{}\n", table_line);
        }
        let vif_html = table_to_html_string(
            &vif_table,
            "VIF Diagnostics (Multicollinearity)",
            &["Event/Stat/SQL", "VIF", "Interpretation"],
        );
        gradient_html.push_str(&format!(r#"<div class="cross-model">{}</div>"#, vif_html));
    }

    // Collinear group impacts table
    if !section.collinear_group_impacts.is_empty() {
        make_notes!(logfile_name, args.quiet, 0, "{}",
            "\n-- Collinear Group Impacts --\n".bold().bright_yellow());
        let mut grp_table = Table::new();
        grp_table.set_titles(Row::new(vec![
            Cell::new("Group Members").with_style(Attr::Bold),
            Cell::new("Combined Coef").with_style(Attr::Bold),
            Cell::new("Combined Impact").with_style(Attr::Bold),
        ]));
        for g in &section.collinear_group_impacts {
            grp_table.add_row(Row::new(vec![
                Cell::new(&g.group_members.join(" + ")),
                Cell::new(&format!("{:+.6}", g.combined_coef)),
                Cell::new(&format!("{:.6}", g.combined_impact)),
            ]));
        }
        for table_line in grp_table.to_string().lines() {
            make_notes!(logfile_name, args.quiet, 0, "{}\n", table_line);
        }
        let grp_html = table_to_html_string(
            &grp_table,
            "Collinear Group Impacts",
            &["Group Members", "Combined Coef", "Combined Impact"],
        );
        gradient_html.push_str(&format!(r#"<div class="cross-model">{}</div>"#, grp_html));
    }

    gradient_html
}

pub fn print_top_items_table(
    title: &str,
    items: &[GradientTopItem],
    logfile_name: &str,
    args: &Args,
) -> String {
    let mut table = Table::new();
    table.set_titles(Row::new(vec![
        Cell::new("#").with_style(Attr::Bold),
        Cell::new("Wait Event/Statistic").with_style(Attr::Bold),
        Cell::new("Coef").with_style(Attr::Bold),
        Cell::new("Active Impact (P90)").with_style(Attr::Bold),
        Cell::new("Peak Impact (P99)").with_style(Attr::Bold),
        Cell::new("Share %").with_style(Attr::Bold),
        Cell::new("Typical Impact (MAD)").with_style(Attr::Bold),
    ]));
    for (idx, item) in items.iter().enumerate() {
        table.add_row(Row::new(vec![
            Cell::new(&format!("{}", idx + 1)),
            Cell::new(&item.event_name),
            Cell::new(&format!("{} {:+.6}",
                if item.gradient_coef > 0.0 { "↑" } else { "↓" },
                item.gradient_coef
            )),
            Cell::new(&format!("{:.6}", item.impact_active)),
            Cell::new(&format!("{:.6}", item.impact_peak)),
            Cell::new(&format!("{:.1}%", item.impact_share * 100.0)),
            Cell::new(&format!("{:.6}", item.impact)),
        ]));
    }
    make_notes!(logfile_name, args.quiet, 0, "{}",
        format!("{} table (Top {})\n", title, items.len()).bright_black());
    for table_line in table.to_string().lines() {
        make_notes!(logfile_name, args.quiet, 0, "{}\n", table_line);
    }
    let mut html = table_to_html_string(
        &table,
        title,
        &["#", "Wait Event/Statistic", "Coef", "Active Impact (P90)",
          "Peak Impact (P99)", "Share %", "Typical Impact (MAD)"],
    );
    html = format!(r#"<div>{html}</div>"#);
    html
}


pub struct GradientSectionSpec<'a> {
    /// Target time series (e.g. DB Time or DB CPU)
    pub target: &'a [f64],
    /// Feature series – already filtered/ready
    pub features: BTreeMap<String, Vec<f64>>,
    /// Label passed to `build_db_time_gradient_section`
    pub label: String,
    /// Whether this is a wait-events section (affects table rendering)
    pub is_events: bool,
    /// Human-readable name for logs
    pub display_name: String,
}

pub fn run_gradient_section(
    spec: &GradientSectionSpec,
    ridge_lambda: f64,
    elastic_net_lambda: f64,
    elastic_net_alpha: f64,
    elastic_net_max_iter: usize,
    elastic_net_tol: f64,
    logfile_name: &str,
    args: &Args,
) -> (Option<DbTimeGradientSection>, String) {
    match build_db_time_gradient_section(
        spec.target,
        &spec.features,
        ridge_lambda,
        elastic_net_lambda,
        elastic_net_alpha,
        elastic_net_max_iter,
        elastic_net_tol,
        &spec.label,
    ) {
        Ok(section) => {
            make_notes!(
                logfile_name, false, 1,
                "\n\n{}",
                format!("{} attached to ReportForAI", spec.display_name)
                    .bold().green()
            );
            let html = print_db_time_gradient_tables(
                &section, spec.is_events, logfile_name, args,
            );
            (Some(section), html)
        }
        Err(err) => {
            make_notes!(
                logfile_name, false, 1,
                "\n\n{} skipped: {}", spec.display_name, err
            );
            (None, String::new())
        }
    }
}

/// Specification of a single gradient section to be rendered in HTML
pub struct GradientHtmlSection {
    /// Section heading displayed in HTML (e.g. "DB Time vs Wait Events")
    pub heading: String,
    /// Rendered HTML of this section (output from `print_db_time_gradient_tables`)
    pub html: String,
}

/// Generic function that builds a complete HTML file from gradient sections.
///
/// # Arguments
/// * `title` - page title (e.g. "DB Time Gradient Analyzes" or "DB CPU Gradient Analyzes")
/// * `main_heading` - main heading displayed on the page
/// * `sections` - vector of gradient sections to render (in order)
///
/// # Returns
/// Complete HTML document as a `String`.
pub fn build_gradient_html(
    title: &str,
    main_heading: &str,
    sections: Vec<GradientHtmlSection>,
) -> String {
    // Combine all non-empty sections into a single HTML block
    let sections_html: String = sections
        .into_iter()
        .filter(|s| !s.html.trim().is_empty()) // skip empty sections
        .map(|s| {
            format!(
                r#"                <p><span style="font-size:15px;font-weight:bold;">{}</span></p>
                {}
"#,
                s.heading, s.html
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title}</title>
    <style>
        body {{ font-family: Arial, sans-serif; }}
        .content {{ font-size: 14px; }}
        table {{
            width: 30%;
            border-collapse: collapse;
            margin-top: 20px;
        }}
        th, td {{
            border: 1px solid black;
            padding: 8px;
            text-align: center;
        }}
        th {{
            background-color: #632e4f;
            color: white;
            cursor: pointer;
            user-select: none;
            position: relative;
        }}
        th:hover {{
            background-color: #7a3a62;
        }}
        th.sort-asc::after {{
            content: " \25B2";
            font-size: 0.8em;
        }}
        th.sort-desc::after {{
            content: " \25BC";
            font-size: 0.8em;
        }}
        tr:nth-child(even) {{
            background-color: #f2f2f2;
        }}
        td:first-child {{
            text-align: right;
            font-weight: bold;
        }}
        .tables-grid {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(720px, 1fr));
            gap: 20px;
            align-items: start;
        }}
        .tables-grid table {{
            width: 100%;
            margin-top: 0;
            table-layout: fixed;      /* kluczowe: nie rozpychaj się zawartością */
            word-break: break-word;   /* długie liczby mogą się łamać */
        }}
        .tables-grid td,
        .tables-grid th {{
            overflow-wrap: anywhere;  /* żeby liczby mogły się złamać */
            font-size: 12px;          /* odrobinę mniejsze, więcej się mieści */
        }}
        .cross-model table {{
            width: 100%;
            margin-top: 20px;
        }}
        .cross-model p {{
            font-weight: bold;
            font-size: 16px;
            margin-top: 40px;
        }}
    </style>
    <script>
        // Generic sorter that works for every table on the page.
        // Numeric values (including negative and scientific notation) are sorted numerically,
        // otherwise locale-aware string comparison is used.
        function gradientSortTable(headerCell) {{
            const table = headerCell.closest('table');
            if (!table) return;

            // Determine the clicked column index within its own row
            const headerRow = headerCell.parentElement;
            const headers = Array.from(headerRow.children);
            const columnIndex = headers.indexOf(headerCell);
            if (columnIndex < 0) return;

            // Pick the tbody (or fallback to the table itself if missing)
            const tbody = table.tBodies[0] || table;
            const rows = Array.from(tbody.querySelectorAll('tr')).filter(r => {{
                // Skip rows that are actually header rows inside tbody
                return r.querySelector('td') !== null;
            }});
            if (rows.length === 0) return;

            // Toggle sort direction based on the previous state stored on the header
            const currentDir = headerCell.getAttribute('data-sort-dir');
            const ascending = currentDir !== 'asc';

            // Clear sort indicators on all headers of this table
            table.querySelectorAll('th').forEach(th => {{
                th.removeAttribute('data-sort-dir');
                th.classList.remove('sort-asc', 'sort-desc');
            }});

            // Helper: try to parse a cell's text as a number
            const parseValue = (text) => {{
                if (text === null || text === undefined) return {{ num: NaN, str: '' }};
                const trimmed = String(text).trim();
                if (trimmed === '' || trimmed === '-' || trimmed === 'N/A') {{
                    return {{ num: NaN, str: trimmed }};
                }}
                // Remove thousands separators and percent signs before numeric parsing
                const cleaned = trimmed.replace(/,/g, '').replace(/%$/, '');
                const num = Number(cleaned);
                return {{ num: isNaN(num) ? NaN : num, str: trimmed }};
            }};

            // Sort the rows using the chosen column
            rows.sort((rowA, rowB) => {{
                const cellA = rowA.children[columnIndex];
                const cellB = rowB.children[columnIndex];
                const textA = cellA ? cellA.innerText : '';
                const textB = cellB ? cellB.innerText : '';
                const a = parseValue(textA);
                const b = parseValue(textB);

                let cmp;
                if (!isNaN(a.num) && !isNaN(b.num)) {{
                    cmp = a.num - b.num;
                }} else {{
                    cmp = a.str.localeCompare(b.str, undefined, {{ numeric: true, sensitivity: 'base' }});
                }}
                return ascending ? cmp : -cmp;
            }});

            // Re-append rows in the new order
            rows.forEach(r => tbody.appendChild(r));

            // Save the new direction indicator on the clicked header
            headerCell.setAttribute('data-sort-dir', ascending ? 'asc' : 'desc');
            headerCell.classList.add(ascending ? 'sort-asc' : 'sort-desc');
        }}

        // Attach click handlers to every table header on page load
        document.addEventListener('DOMContentLoaded', function() {{
            document.querySelectorAll('table th').forEach(th => {{
                th.addEventListener('click', function() {{
                    gradientSortTable(th);
                }});
            }});
        }});
    </script>
</head>
<body>
    <div class="content">
        <p><a href="https://github.com/ora600pl/jas-min" target="_blank">
            <img src="https://raw.githubusercontent.com/rakustow/jas-min/main/img/jasmin_LOGO_white.png"
                 width="150" alt="JAS-MIN" onerror="this.style.display='none';"/>
        </a></p>
        <p><span style="font-size:20px;font-weight:bold;">{main_heading}</span></p>
{sections_html}
    </div>
</body>
</html>
"#
    )
}