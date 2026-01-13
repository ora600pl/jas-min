use std::collections::BTreeMap;
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
                        DbTimeGradientSection};

use prettytable::{Table, Row, Cell, format, Attr};
use colored::*;


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

    /// Ridge impact ranking (sorted by impact descending)
    pub ridge_ranking: Vec<EventImpact>,
    /// Elastic Net impact ranking (sorted by impact descending)
    pub elastic_net_ranking: Vec<EventImpact>,

    /// Δ(wait event/statistic value/sqlid exec time) standardization stats (used internally; helpful for debugging)
    pub event_delta_mean_by_event: EventScalarMap,
    pub event_delta_std_by_event: EventScalarMap,

    /// MAD(raw Δ(wait event/statistic value/sqlid exec time)) (useful for “real-world” impact explanation)
    pub event_delta_mad_by_event: EventScalarMap,
}

/// Compute a “numerical gradient” of DB Time with respect to wait events using:
/// - time differences: ΔDBTime and Δ(wait events or stat values or sqlid exec times)
/// - Ridge regression (stability)
/// - Elastic Net regression (stability + sparsity)
///
/// Notes:
/// - We work on deltas to focus on changes rather than levels.
/// - We standardize Δ(wait events) by (value - mean) / std per event.
///   This is recommended for fair regularization behavior in Ridge/EN.
pub fn compute_db_time_gradient(
    db_time_series: &[f64],
    event_series: &EventSeriesMap,
    ridge_lambda: f64,
    elastic_net_lambda: f64,
    elastic_net_alpha: f64,
    elastic_net_max_iter: usize,
    elastic_net_tol: f64,
) -> Result<DbTimeGradientResult, String> {
    // ------------------------------------------------------------
    // 1) Validate inputs
    // ------------------------------------------------------------
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

    // ------------------------------------------------------------
    // 2) Compute ΔDBTime (positional by time is fine and intended)
    // ------------------------------------------------------------
    let db_time_delta = compute_time_deltas(db_time_series);

    // ------------------------------------------------------------
    // 3) Compute Δ(wait events) as a name-keyed map
    // ------------------------------------------------------------
    let event_delta_by_event = compute_event_deltas(event_series)?;

    // ------------------------------------------------------------
    // 4) Compute per-event standardization stats for Δ(wait events)
    // ------------------------------------------------------------
    let event_delta_mean_by_event = compute_mean_by_event(&event_delta_by_event);
    let event_delta_std_by_event = compute_std_by_event(&event_delta_by_event, &event_delta_mean_by_event);

    // Standardize deltas: (delta - mean) / std, per event name
    let event_delta_standardized_by_event =
        standardize_by_event(&event_delta_by_event, &event_delta_mean_by_event, &event_delta_std_by_event);

    // MAD of raw deltas (robust “typical movement” per event)
    let event_delta_mad_by_event = compute_mad_by_event(&event_delta_by_event);

    // ------------------------------------------------------------
    // 5) Ridge regression in fully map-based form:
    //    We solve: (X'X + λI) w = X'y
    //    where columns are wait events (by name) and rows are time samples.
    // ------------------------------------------------------------
    let ridge_gradient_by_event = ridge_regression_map(
        &event_delta_standardized_by_event,
        &db_time_delta,
        ridge_lambda,
    )?;

    // ------------------------------------------------------------
    // 6) Elastic Net via coordinate descent (map-based):
    //    Returns event -> coef, with many zeros possible.
    // ------------------------------------------------------------
    let elastic_net_gradient_by_event = elastic_net_coordinate_descent_map(
        &event_delta_standardized_by_event,
        &db_time_delta,
        elastic_net_lambda,
        elastic_net_alpha,
        elastic_net_max_iter,
        elastic_net_tol,
    );

    // ------------------------------------------------------------
    // 7) Build impact rankings:
    //    impact(event) = |coef(event)| * MAD(raw Δevent)
    // ------------------------------------------------------------
    let ridge_ranking = build_ranking(&ridge_gradient_by_event, &event_delta_mad_by_event);
    let elastic_net_ranking = build_ranking(&elastic_net_gradient_by_event, &event_delta_mad_by_event);

    Ok(DbTimeGradientResult {
        ridge_gradient_by_event,
        elastic_net_gradient_by_event,
        ridge_ranking,
        elastic_net_ranking,
        event_delta_mean_by_event,
        event_delta_std_by_event,
        event_delta_mad_by_event,
    })
}

/* =========================================================================================
   Core computations (all wait-event operations are name-keyed)
   ========================================================================================= */

/// Compute time deltas for a single series: delta[t] = series[t+1] - series[t]
fn compute_time_deltas(series: &[f64]) -> Vec<f64> {
    let mut deltas = Vec::with_capacity(series.len().saturating_sub(1));
    for t in 0..series.len() - 1 {
        deltas.push(series[t + 1] - series[t]);
    }
    deltas
}

/// Compute deltas for all wait events: event_name -> Vec<delta>
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

/// Compute mean per event (on the series provided).
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

/// Compute standard deviation per event (population std) using provided means.
/// Guards against zero/NaN by using a small epsilon.
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

        // If an event never changes, std becomes 0; keep it non-zero to avoid division by zero.
        if !std.is_finite() || std == 0.0 {
            std = 1e-12;
        }

        std_by_event.insert(event_name.clone(), std);
    }

    std_by_event
}

/// Standardize each event series: (value - mean(event)) / std(event)
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

/// Compute MAD per event: MAD = median(|x - median(x)|)
fn compute_mad_by_event(series_by_event: &EventSeriesMap) -> EventScalarMap {
    let mut mad_by_event = BTreeMap::new();

    for (event_name, series) in series_by_event.iter() {
        let mad = mad(series);
        mad_by_event.insert(event_name.clone(), mad);
    }

    mad_by_event
}

/* =========================================================================================
   Ridge regression (map-based, no positional wait-event indexing)
   ========================================================================================= */

/// Ridge regression solver using named features (events) only.
/// We build normal equations in map form:
///   A = X'X + λI
///   b = X'y
/// and solve A w = b with Gaussian elimination operating on BTreeMap keys.
///
/// Inputs:
/// - standardized_event_deltas: event -> Vec<standardized_delta>
/// - db_time_delta: Vec<ΔDBTime>
/// Returns:
/// - event -> coef
fn ridge_regression_map(
    standardized_event_deltas: &EventSeriesMap,
    db_time_delta: &[f64],
    lambda: f64,
) -> Result<EventScalarMap, String> {
    // Validate consistent lengths
    let sample_count = db_time_delta.len();
    for (event_name, series) in standardized_event_deltas.iter() {
        if series.len() != sample_count {
            return Err(format!(
                "Standardized delta series length mismatch for '{event_name}': got {}, expected {}.",
                series.len(),
                sample_count
            ));
        }
    }

    // Build row-wise representation to accumulate X'X and X'y without any positional event indexing.
    // Each row is: event_name -> value at time t
    let rows = build_rows_by_time(standardized_event_deltas, sample_count)?;

    // Compute XtX (A) and Xty (b) in fully name-keyed maps.
    let (mut a_matrix, mut b_vector) = build_normal_equations(&rows, db_time_delta);

    // Add λI (ridge penalty) to diagonal entries
    for event_name in standardized_event_deltas.keys() {
        let diag = a_matrix
            .entry(event_name.clone())
            .or_insert_with(BTreeMap::new)
            .entry(event_name.clone())
            .or_insert(0.0);
        *diag += lambda;
    }

    // Solve the system A w = b
    gaussian_elimination_solve(&mut a_matrix, &mut b_vector)
}

/// Build rows-by-time: Vec< BTreeMap<event_name, value_at_t> >
/// This enables accumulating outer products without ever referring to “feature i”.
fn build_rows_by_time(
    standardized_event_deltas: &EventSeriesMap,
    sample_count: usize,
) -> Result<Vec<BTreeMap<String, f64>>, String> {
    let mut rows: Vec<BTreeMap<String, f64>> = Vec::with_capacity(sample_count);

    // Initialize empty maps for each time row
    for _ in 0..sample_count {
        rows.push(BTreeMap::new());
    }

    // Fill rows: for each event, assign its value at each time into the row map
    for (event_name, series) in standardized_event_deltas.iter() {
        if series.len() != sample_count {
            return Err(format!(
                "Series length mismatch in build_rows_by_time for '{event_name}'."
            ));
        }
        for (t, value) in series.iter().enumerate() {
            rows[t].insert(event_name.clone(), *value);
        }
    }

    Ok(rows)
}

/// Build normal equations:
/// A = X'X, b = X'y
/// using row-wise maps.
/// No positional wait-event logic is needed.
fn build_normal_equations(
    rows: &[BTreeMap<String, f64>],
    y: &[f64],
) -> (BTreeMap<String, BTreeMap<String, f64>>, BTreeMap<String, f64>) {
    let mut a: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    let mut b: BTreeMap<String, f64> = BTreeMap::new();

    for (t, row) in rows.iter().enumerate() {
        let yt = y[t];

        // b_i += x_i * y
        for (event_i, xi) in row.iter() {
            *b.entry(event_i.clone()).or_insert(0.0) += xi * yt;
        }

        // A_ij += x_i * x_j
        for (event_i, xi) in row.iter() {
            let row_i = a.entry(event_i.clone()).or_insert_with(BTreeMap::new);
            for (event_j, xj) in row.iter() {
                *row_i.entry(event_j.clone()).or_insert(0.0) += xi * xj;
            }
        }
    }

    (a, b)
}

/// Solve A w = b using Gaussian elimination on BTreeMap keys.
/// This is deterministic because BTreeMap key order is sorted.
///
/// WARNING:
/// - This is not optimized for huge N; it is designed for clarity and debugability.
fn gaussian_elimination_solve(
    a: &mut BTreeMap<String, BTreeMap<String, f64>>,
    b: &mut BTreeMap<String, f64>,
) -> Result<EventScalarMap, String> {
    let event_names: Vec<String> = a.keys().cloned().collect();

    // Forward elimination
    for pivot_name in event_names.iter() {
        let pivot = get_matrix_value(a, pivot_name, pivot_name);
        if pivot.abs() < 1e-18 || !pivot.is_finite() {
            return Err(format!("Singular or invalid pivot for '{pivot_name}' in Ridge solve."));
        }

        // Normalize pivot row
        let pivot_row = a.get_mut(pivot_name).ok_or_else(|| format!("Missing row for '{pivot_name}'"))?;
        for (_col_name, val) in pivot_row.iter_mut() {
            *val /= pivot;
        }
        let pivot_b = b.get(pivot_name).copied().unwrap_or(0.0) / pivot;
        b.insert(pivot_name.clone(), pivot_b);

        // Eliminate pivot column from all other rows
        for other_name in event_names.iter() {
            if other_name == pivot_name {
                continue;
            }
            let factor = get_matrix_value(a, other_name, pivot_name);
            if factor == 0.0 {
                continue;
            }

            // row_other = row_other - factor * row_pivot
            let pivot_row_snapshot = a
                .get(pivot_name)
                .cloned()
                .ok_or_else(|| format!("Missing pivot row snapshot for '{pivot_name}'"))?;

            let other_row = a
                .get_mut(other_name)
                .ok_or_else(|| format!("Missing row for '{other_name}'"))?;

            for (col_name, pivot_val) in pivot_row_snapshot.iter() {
                let entry = other_row.entry(col_name.clone()).or_insert(0.0);
                *entry -= factor * pivot_val;
            }

            // b_other = b_other - factor * b_pivot
            let b_other = b.get(other_name).copied().unwrap_or(0.0);
            let b_pivot = b.get(pivot_name).copied().unwrap_or(0.0);
            b.insert(other_name.clone(), b_other - factor * b_pivot);

            // Clean up numerical noise in the pivot column
            if let Some(v) = other_row.get_mut(pivot_name) {
                if v.abs() < 1e-15 {
                    *v = 0.0;
                }
            }
        }
    }

    // At this point A should be ~Identity and b is the solution.
    // Return solution as a name-keyed map.
    Ok(b.clone())
}

/// Helper to read A[row][col] with default 0.0 when missing.
fn get_matrix_value(a: &BTreeMap<String, BTreeMap<String, f64>>, row: &str, col: &str) -> f64 {
    a.get(row).and_then(|r| r.get(col)).copied().unwrap_or(0.0)
}

/* =========================================================================================
   Elastic Net coordinate descent (map-based, no positional wait-event indexing)
   ========================================================================================= */

/// Elastic Net coordinate descent using name-keyed features only.
/// Returns event -> coef, with possible zeros.
///
/// Objective (conceptually):
///   (1/2m)||y - Xw||^2 + λ[ α||w||_1 + (1-α)/2 ||w||^2 ]
///
fn elastic_net_coordinate_descent_map(
    standardized_event_deltas: &EventSeriesMap,
    db_time_delta: &[f64],
    lambda: f64,
    alpha: f64,
    max_iter: usize,
    tol: f64,
) -> EventScalarMap {
    let sample_count = db_time_delta.len();

    // Coefficients start at 0 for all events
    let mut coef_by_event: EventScalarMap = standardized_event_deltas
        .keys()
        .map(|k| (k.clone(), 0.0))
        .collect();

    // Residual = y - Xw, and w starts as zero => residual = y
    let mut residual = db_time_delta.to_vec();

    // Precompute feature norms: (1/m) * sum x^2
    let mut feature_norm_by_event: EventScalarMap = BTreeMap::new();
    for (event_name, series) in standardized_event_deltas.iter() {
        let mut s = 0.0;
        for v in series.iter() {
            s += v * v;
        }
        let mut norm = s / (sample_count as f64).max(1.0);
        if norm == 0.0 || !norm.is_finite() {
            norm = 1e-12;
        }
        feature_norm_by_event.insert(event_name.clone(), norm);
    }

    let l1_penalty = lambda * alpha;
    let l2_penalty = lambda * (1.0 - alpha);

    for _ in 0..max_iter {
        let mut max_change: f64 = 0.0;

        for (event_name, feature_series) in standardized_event_deltas.iter() {
            let old_coef = *coef_by_event.get(event_name).unwrap_or(&0.0);

            // Restore residual: residual += x_j * old_coef
            if old_coef != 0.0 {
                for (t, x_t) in feature_series.iter().enumerate() {
                    residual[t] += x_t * old_coef;
                }
            }

            // Compute correlation: (1/m) * x_j^T residual
            let mut correlation = 0.0;
            for (t, x_t) in feature_series.iter().enumerate() {
                correlation += x_t * residual[t];
            }
            correlation /= (sample_count as f64).max(1.0);

            // Coordinate update
            let norm = *feature_norm_by_event.get(event_name).unwrap_or(&1e-12);
            let new_coef = soft_threshold(correlation, l1_penalty) / (norm + l2_penalty);

            // Update coefficient
            coef_by_event.insert(event_name.clone(), new_coef);

            // Update residual: residual -= x_j * new_coef
            if new_coef != 0.0 {
                for (t, x_t) in feature_series.iter().enumerate() {
                    residual[t] -= x_t * new_coef;
                }
            }

            max_change = max_change.max((new_coef - old_coef).abs());
        }

        if max_change < tol {
            break;
        }
    }

    coef_by_event
}

/// Soft-thresholding operator used by L1 penalty:
/// S(z, γ) = sign(z) * max(|z|-γ, 0)
fn soft_threshold(value: f64, threshold: f64) -> f64 {
    if value > threshold {
        value - threshold
    } else if value < -threshold {
        value + threshold
    } else {
        0.0
    }
}

/* =========================================================================================
   Ranking / reporting helpers
   ========================================================================================= */

/// Build a sorted (descending) impact ranking from coef map and MAD map:
/// impact(event) = |coef(event)| * MAD(event)
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
   Robust statistics helpers (MAD / median)
   ========================================================================================= */

fn mad(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let med = median(values);

    let mut deviations: Vec<f64> = values.iter().map(|v| (v - med).abs()).collect();
    median_in_place(&mut deviations)
}

fn median(values: &[f64]) -> f64 {
    let mut tmp = values.to_vec();
    median_in_place(&mut tmp)
}

/// Median computed in-place using select_nth_unstable (reorders the vector).
fn median_in_place(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mid = values.len() / 2;
    values.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());

    if values.len() % 2 == 1 {
        values[mid]
    } else {
        // For even length, average of the two middle values:
        // the lower middle is max of the left partition
        let lower_max = values[..mid]
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        (lower_max + values[mid]) * 0.5
    }
}


/// Compute DB Time gradient section for ReportForAI.
/// - Uses y_vals_dbtime as DB Time series (aligned to snaps)
/// - Uses y_vals_events as wait-event series (aligned, zero-filled)
/// - Converts wait series to ms per sample
/// - Returns a compact Top10 summary for Ridge and Elastic Net
///
/// This function never exposes positional mapping for wait events.
/// It only consumes/produces name-keyed maps or named lists.
pub fn build_db_time_gradient_section (
    db_time_series: &[f64],
    event_series: &BTreeMap<String, Vec<f64>>,
    ridge_lambda: f64,
    elastic_net_lambda: f64,
    elastic_net_alpha: f64,
    elastic_net_max_iter: usize,
    elastic_net_tol: f64,
    units_desc: &str
) -> Result<DbTimeGradientSection, String> {
    // ------------------------------------------------------------
    // 1) Validate aligned lengths (DB Time vs every wait event series)
    // ------------------------------------------------------------
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
                event_name,
                series.len(),
                expected_len
            ));
        }
    }

    // ------------------------------------------------------------
    // 2) Run gradient estimation (Ridge + Elastic Net)
    // ------------------------------------------------------------
    let gradient_result = compute_db_time_gradient(
        db_time_series,
        &event_series,
        ridge_lambda,
        elastic_net_lambda,
        elastic_net_alpha,
        elastic_net_max_iter,
        elastic_net_tol,
    )?;

    // ------------------------------------------------------------
    // 3) Convert to compact AI section (Top50)
    //    - Ridge: take Top50 by impact
    //    - Elastic Net: skip zeros, then take Top50
    // ------------------------------------------------------------
    let ridge_top: Vec<GradientTopItem> = gradient_result
        .ridge_ranking
        .iter()
        .take(50)
        .map(|x| GradientTopItem {
            event_name: x.event_name.clone(),
            gradient_coef: x.gradient_coef,
            impact: x.impact,
        })
        .collect();

    let elastic_net_top: Vec<GradientTopItem> = gradient_result
        .elastic_net_ranking
        .iter()
        .filter(|x| x.gradient_coef != 0.0) // avoid dumping zeros
        .take(50)
        .map(|x| GradientTopItem {
            event_name: x.event_name.clone(),
            gradient_coef: x.gradient_coef,
            impact: x.impact,
        })
        .collect();

    Ok(DbTimeGradientSection {
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
    })
}

/// Print DB Time gradient summary (Ridge + Elastic Net) in nice console tables.
///
/// This is intended for quick debugging / sanity-checking during report generation.
/// It uses only name-keyed values (event_name), never positional indexing.
///
/// Expected input:
/// - section.ridge_top / section.elastic_net_top: already TopN lists prepared for AI
pub fn print_db_time_gradient_tables(section: &DbTimeGradientSection, print_settings: bool, logfile_name: &str, args: &Args) {
    make_notes!(logfile_name, args.quiet, 0, "\n{} \n\t- {}", "==== DB TIME GRADIENT (Ridge / Elastic Net) ====".bold().bright_cyan(), section.settings.input_wait_event_unit);

    // -----------------------------
    // Settings table
    // -----------------------------
    if print_settings {
        let mut settings_table = Table::new();
        settings_table.set_titles(Row::new(vec![
            Cell::new("Setting").with_style(Attr::Bold),
            Cell::new("Value").with_style(Attr::Bold),
        ]));

        settings_table.add_row(Row::new(vec![
            Cell::new("ridge_lambda"),
            Cell::new(&format!("{:.6}", section.settings.ridge_lambda)),
        ]));
        settings_table.add_row(Row::new(vec![
            Cell::new("elastic_net_lambda"),
            Cell::new(&format!("{:.6}", section.settings.elastic_net_lambda)),
        ]));
        settings_table.add_row(Row::new(vec![
            Cell::new("elastic_net_alpha"),
            Cell::new(&format!("{:.6}", section.settings.elastic_net_alpha)),
        ]));
        settings_table.add_row(Row::new(vec![
            Cell::new("elastic_net_max_iter"),
            Cell::new(&format!("{}", section.settings.elastic_net_max_iter)),
        ]));
        settings_table.add_row(Row::new(vec![
            Cell::new("elastic_net_tol"),
            Cell::new(&format!("{:.6e}", section.settings.elastic_net_tol)),
        ]));
        settings_table.add_row(Row::new(vec![
            Cell::new("input_event_unit"),
            Cell::new(&section.settings.input_wait_event_unit),
        ]));
        settings_table.add_row(Row::new(vec![
            Cell::new("input_db_time_unit"),
            Cell::new(&section.settings.input_db_time_unit),
        ]));

        make_notes!(logfile_name, args.quiet, 0, "{}", "\n-- Settings --".bold().bright_white());

        for table_line in settings_table.to_string().lines() {
            make_notes!(logfile_name, args.quiet, 0, "{}\n", table_line);
        }
    }

    // -----------------------------
    // Ridge TOP table
    // -----------------------------
    make_notes!(logfile_name, args.quiet, 0, "{}", "\n-- Ridge TOP --\n".bold().bright_white());
    print_top_items_table("Ridge", &section.ridge_top, logfile_name, args);

    // -----------------------------
    // Elastic Net TOP table
    // -----------------------------
    println!("{}", "\n-- Elastic Net TOP --\n".bold().bright_white());
    // Elastic Net: in practice you usually want to hide zero coefficients
    let en_nonzero: Vec<crate::reasonings::GradientTopItem> = section
        .elastic_net_top
        .iter()
        .cloned()
        .filter(|x| x.gradient_coef != 0.0)
        .collect();

    if en_nonzero.is_empty() {
        make_notes!(logfile_name, args.quiet, 0, "{}", "Elastic Net produced no non-zero coefficients (try smaller lambda or smaller alpha)."
            .yellow());
    } else {
        print_top_items_table("ElasticNet", &en_nonzero, logfile_name, args);
    }
}

/// Helper: prints Top-N list as a prettytable.
/// Rows are rank-ordered as provided by the caller.
pub fn print_top_items_table(title: &str, items: &[GradientTopItem], logfile_name: &str, args: &Args) {
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

    // Small title banner for clarity in logs
    make_notes!(logfile_name, args.quiet, 0, "{}", format!("{} table (Top {})\n", title, items.len()).bright_black());
    for table_line in table.to_string().lines() {
            make_notes!(logfile_name, args.quiet, 0, "{}\n", table_line);
        }
}