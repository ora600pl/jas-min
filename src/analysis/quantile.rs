//! L2-regularized quantile regression with an unpenalized intercept.
//!
//! Minimize mean(pinball_tau(y_standardized - a - X beta)) + lambda/2 * ||beta||².
//! ADMM splits the residual from the regression fit. Both primal and dual
//! residuals must pass their tolerances (Boyd et al., 2011, section 3.3).
//! Target scaling is reversed on output; lambda is independent of target units.
use nalgebra::{DMatrix, DVector};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantileDiagnostics {
    pub method: String,
    pub status: String,
    pub converged: bool,
    pub iterations: usize,
    pub max_iterations: usize,
    pub tau: f64,
    pub lambda: f64,
    pub tolerance: f64,
    pub target_scale: f64,
    /// Intercept in original target units, for centered/standardized predictors.
    pub intercept: f64,
    pub objective_standardized: f64,
    pub dual_objective_standardized: f64,
    pub duality_gap: f64,
    pub primal_residual: f64,
    pub primal_tolerance: f64,
    pub dual_residual: f64,
    pub dual_tolerance: f64,
}

pub const Q95_LAMBDA: f64 = 0.0005;
pub const Q95_MAX_ITER: usize = 20_000;
pub const Q95_TOL: f64 = 1e-6;

// A feasible dual gives a lower bound on the global minimum of this convex
// objective. Project -rho*u onto [tau-1, tau] with sum=0 (free intercept).
fn objective_bounds(
    x: &DMatrix<f64>,
    y: &DVector<f64>,
    beta: &DVector<f64>,
    dual: &DVector<f64>,
    rho: f64,
    lambda: f64,
    tau: f64,
) -> (f64, f64) {
    let n = y.len() as f64;
    let alpha = -rho * dual;
    let mut lo = alpha.min() - tau;
    let mut hi = alpha.max() - (tau - 1.0);
    for _ in 0..80 {
        let shift = (lo + hi) * 0.5;
        let sum: f64 = alpha
            .iter()
            .map(|a| (a - shift).clamp(tau - 1.0, tau))
            .sum();
        if sum > 0.0 {
            lo = shift;
        } else {
            hi = shift;
        }
    }
    let feasible = alpha.map(|a| (a - (lo + hi) * 0.5).clamp(tau - 1.0, tau));
    let moment = x.transpose() * &feasible / n;
    let lower =
        y.dot(&feasible) / n - moment.iter().skip(1).map(|v| v * v).sum::<f64>() / (2.0 * lambda);
    let errors = y - x * beta;
    let upper = errors
        .iter()
        .map(|r| if *r >= 0.0 { tau * r } else { (tau - 1.0) * r })
        .sum::<f64>()
        / n
        + 0.5 * lambda * beta.iter().skip(1).map(|b| b * b).sum::<f64>();
    (upper, lower)
}

pub fn fit(
    columns: &BTreeMap<String, Vec<f64>>,
    target: &[f64],
    tau: f64,
    lambda: f64,
    max_iter: usize,
    tol: f64,
) -> Result<(BTreeMap<String, f64>, QuantileDiagnostics), String> {
    let n = target.len();
    let p = columns.len();
    if n < 2
        || p == 0
        || !(0.0..1.0).contains(&tau)
        || tau == 0.0
        || !lambda.is_finite()
        || lambda <= 0.0
        || max_iter == 0
        || !tol.is_finite()
        || tol <= 0.0
        || target.iter().any(|v| !v.is_finite())
        || columns
            .values()
            .any(|c| c.len() != n || c.iter().any(|v| !v.is_finite()))
    {
        return Err("Invalid quantile regression input or solver settings".into());
    }
    let mean = target.iter().sum::<f64>() / n as f64;
    let scale = (target.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    if !mean.is_finite() || !scale.is_finite() {
        return Err("Quantile target scaling overflow".into());
    }
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let y = DVector::from_iterator(n, target.iter().map(|v| (v - mean) / scale));
    let mut x = DMatrix::from_element(n, p + 1, 1.0);
    for (j, column) in columns.values().enumerate() {
        for (i, value) in column.iter().enumerate() {
            x[(i, j + 1)] = *value;
        }
    }
    let gram = x.transpose() * &x;
    let mut rho = 1.0;
    let factor = |rho: f64| {
        let mut matrix = gram.clone();
        // The intercept (column zero) is never penalized.
        for j in 1..=p {
            matrix[(j, j)] += n as f64 * lambda / rho;
        }
        matrix
            .cholesky()
            .ok_or_else(|| "Quantile linear solve failed".to_string())
    };
    let mut cholesky = factor(rho)?;
    let mut beta = DVector::zeros(p + 1);
    let mut residual = y.clone();
    let mut dual = DVector::zeros(n);
    let mut info = QuantileDiagnostics {
        method: "admm_pinball_l2_scaled_target_v1".into(),
        status: "iteration_limit".into(),
        converged: false,
        iterations: 0,
        max_iterations: max_iter,
        tau,
        lambda,
        tolerance: tol,
        target_scale: scale,
        intercept: mean,
        objective_standardized: 0.0,
        dual_objective_standardized: 0.0,
        duality_gap: 0.0,
        primal_residual: 0.0,
        primal_tolerance: 0.0,
        dual_residual: 0.0,
        dual_tolerance: 0.0,
    };
    for iteration in 1..=max_iter {
        beta = cholesky.solve(&(x.transpose() * (&y - &residual - &dual)));
        let prediction = &x * &beta;
        let previous = residual.clone();
        for i in 0..n {
            let value = y[i] - prediction[i] - dual[i];
            residual[i] = if value > tau / rho {
                value - tau / rho
            } else if value < (tau - 1.0) / rho {
                value - (tau - 1.0) / rho
            } else {
                0.0
            };
        }
        let primal = &prediction + &residual - &y;
        dual += &primal;
        info.iterations = iteration;
        info.primal_residual = primal.norm();
        info.dual_residual = (x.transpose() * (&residual - previous)).norm() * rho;
        info.primal_tolerance =
            tol * ((n as f64).sqrt() + prediction.norm().max(residual.norm()).max(y.norm()));
        info.dual_tolerance =
            tol * (((p + 1) as f64).sqrt() + rho * (x.transpose() * &dual).norm());
        if !beta.iter().chain(dual.iter()).all(|v| v.is_finite()) {
            return Err("Non-finite quantile regression iterate".into());
        }
        if info.primal_residual <= info.primal_tolerance
            && info.dual_residual <= info.dual_tolerance
        {
            let (upper, lower) = objective_bounds(&x, &y, &beta, &dual, rho, lambda, tau);
            if upper - lower <= tol * (1.0 + upper.abs()) {
                info.converged = true;
                info.status = "converged".into();
                break;
            }
        }
        // Bounded residual balancing. Rescale the scaled dual to preserve the
        // unscaled multiplier when rho changes; refactor only on actual changes.
        if iteration % 50 == 0 {
            let next = if info.primal_residual > 10.0 * info.dual_residual {
                (rho * 2.0).min(1024.0)
            } else if info.dual_residual > 10.0 * info.primal_residual {
                (rho / 2.0).max(1e-4)
            } else {
                rho
            };
            if next != rho {
                dual *= rho / next;
                rho = next;
                cholesky = factor(rho)?;
            }
        }
    }
    let (upper, lower) = objective_bounds(&x, &y, &beta, &dual, rho, lambda, tau);
    info.objective_standardized = upper;
    info.dual_objective_standardized = lower;
    info.duality_gap = (upper - lower).max(0.0);
    info.intercept = mean + beta[0] * scale;
    let coefficients = columns
        .keys()
        .enumerate()
        .map(|(j, name)| (name.clone(), beta[j + 1] * scale))
        .collect();
    Ok((coefficients, info))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_independent_constrained_quadratic_reference() {
        // SciPy SLSQP, epigraph formulation with u,v >= 0 and
        // a + X*b + u - v = standardized(y); ftol=1e-13.
        // This is a different optimizer, not a replay of ADMM iterations.
        let x: Vec<f64> = (0..30).map(|i| (i as f64 * 1.3).sin()).collect();
        let z: Vec<f64> = (0..30).map(|i| (i as f64 * 0.7).cos()).collect();
        let y: Vec<f64> = (0..30)
            .map(|i| 2.0 * x[i] - 0.7 * z[i] + if i % 7 == 0 { 5.0 } else { -0.3 })
            .collect();
        let (b, d) = fit(
            &BTreeMap::from([("x".into(), x), ("z".into(), z)]),
            &y,
            0.95,
            0.05,
            Q95_MAX_ITER,
            1e-8,
        )
        .unwrap();
        assert!(d.converged, "{d:?}");
        assert!((b["x"] - 1.56634501).abs() < 1e-5);
        assert!((b["z"] + 0.85125537).abs() < 1e-5);
        assert!((d.intercept - 5.166589596459151).abs() < 1e-5);
        assert!((d.objective_standardized - 0.11162828248909537).abs() < 1e-7);
        assert!(d.duality_gap < 1e-8 * (1.0 + d.objective_standardized));
        assert!(d.dual_objective_standardized <= d.objective_standardized + 1e-12);
    }
    #[test]
    fn skewed_centered_noise_needs_a_free_intercept() {
        let x = BTreeMap::from([(
            "x".into(),
            (0..40).map(|i| if i < 20 { -1.0 } else { 1.0 }).collect(),
        )]);
        let y: Vec<_> = (0..40)
            .map(|i| if i % 20 < 18 { -1.0 } else { 9.0 })
            .collect();
        let (b, d) = fit(&x, &y, 0.95, Q95_LAMBDA, Q95_MAX_ITER, 1e-8).unwrap();
        assert!(d.converged, "{d:?}");
        assert!(b["x"].abs() < 1e-6);
        assert!((d.intercept - 9.0).abs() < 1e-5);
    }
    #[test]
    fn target_units_shift_and_duplicate_samples_preserve_the_fit() {
        let x = BTreeMap::from([(
            "x".into(),
            (0..80).map(|i| (i as f64 * 1.3).sin()).collect::<Vec<_>>(),
        )]);
        let y: Vec<_> = x["x"]
            .iter()
            .enumerate()
            .map(|(i, v)| 1.7 * v + if i % 11 == 0 { 4.0 } else { -0.4 })
            .collect();
        let (b, d) = fit(&x, &y, 0.95, 0.05, Q95_MAX_ITER, 1e-8).unwrap();
        let scaled: Vec<_> = y.iter().map(|v| v * 1000.0 + 231.0).collect();
        let (bs, ds) = fit(&x, &scaled, 0.95, 0.05, Q95_MAX_ITER, 1e-8).unwrap();
        assert!(d.converged && ds.converged);
        assert!((bs["x"] / 1000.0 - b["x"]).abs() < 1e-6);
        assert!(((ds.intercept - 231.0) / 1000.0 - d.intercept).abs() < 1e-6);
        // Population target scaling and mean loss preserve duplicated fits.
        let xx = BTreeMap::from([("x".into(), x["x"].repeat(2))]);
        let (bd, dd) = fit(&xx, &y.repeat(2), 0.95, 0.05, Q95_MAX_ITER, 1e-8).unwrap();
        assert!(dd.converged);
        assert!((bd["x"] - b["x"]).abs() < 1e-5);
    }
    #[test]
    fn iteration_limit_is_not_convergence() {
        let x = BTreeMap::from([("x".into(), vec![-1.0, 0.0, 1.0, 2.0])]);
        let (_, d) = fit(&x, &[0.0, 1.0, 9.0, 3.0], 0.95, Q95_LAMBDA, 1, 1e-12).unwrap();
        assert!(!d.converged);
        assert_eq!(d.status, "iteration_limit");
        assert_eq!(d.iterations, 1);
    }
}
