# Gradient methodology v2

The earlier output selected TOP rows by signed P90 impact **before** retaining
P99 scores. A predictor changing in less than about 10% of intervals can have
P90 = 0 while carrying a large P99. Its coefficient existed but its result was
discarded. Increasing the MCP limit could not recover it.

## Selection and interpretation

The full signed fits now remain in `model_rankings`, including zero and negative
coefficients. Each `*_top` list is a union of independently selected positive
TOP N active/P90, peak/P99 and extreme/maximum rows. `--top-gradient` controls N
**per metric**, so a model can publish up to 3N rows. Zero magnitudes do not take
ranking slots. `selection_reasons`, `active_rank`, `peak_rank` and `extreme_rank`
explain inclusion. Compact cross-model views interleave these dimensions too.

Given a coefficient on standardized predictor deltas, with target scaling
already reversed:

```
beta_raw = beta_standardized / sample_std(delta_x)
active   = abs(beta_raw) * P90(abs(delta_x))
peak     = abs(beta_raw) * P99(abs(delta_x))
extreme  = abs(beta_raw) * max(abs(delta_x))
```

Percentiles include zeros. P99 is not the maximum; even P99 can be zero for a
very rare signal. Cross-model membership is TOP-union selection, not evidence
that an omitted coefficient is zero. Legacy classification codes are preserved
for compatibility; neither agreement nor omission establishes cause,
collinearity, severity, explained variance or savings. Combined scores sum
positive magnitudes from complete eligible fits, not just retained TOP rows.

## Q95 objective and convergence

Q95 uses all observations to fit a conditional upper quantile of **target
changes**. It does not select the largest 5% of target observations first.
Mean-centering alone cannot supply a quantile-regression intercept.

With centered/sample-standardized predictor deltas X, and target deltas scaled
by their population standard deviation s:

```
y_scaled = (delta_y - mean(delta_y)) / s
minimize mean(pinball_0.95(y_scaled - a - X beta)) + lambda/2 * ||beta||²
```

The intercept a is unpenalized. Reported coefficients are `s * beta`;
the reported intercept is `mean(delta_y) + s * a`, for standardized predictors.
Target population scaling and mean loss preserve the objective when observations
are duplicated. Positive target-unit rescaling leaves the restored fit equivalent.

The dedicated normalized Q95 lambda is 0.0005; it is a fixed regularizer, **not**
a cross-validated optimum and no longer derives from the Ridge argument.
Ridge, Elastic Net and Huber calculations retain their existing objectives.
Elastic Net's automatic forward-chaining lambda selection is unchanged.

ADMM splits the residual, uses a cached Cholesky factorization and bounded
residual balancing. At most 20,000 iterations are allowed. A fit is converged
only when primal and dual residual tests pass **and** a feasible-dual objective
gap is at most `1e-6 * (1 + abs(primal_objective))`. The dual certificate projects
the multiplier onto `[tau-1, tau]` with sum zero, accounting for the free
intercept. See [Boyd et al., sections 3.3–3.4](https://web.stanford.edu/~boyd/papers/admm_distr_stats.html)
for the ADMM residual and stopping framework.

`settings.quantile95` exposes method, status, iteration count/cap, lambda,
target scale, intercept, both objective bounds, gap and residual tolerances.
An unconverged fit remains inspectable, but contributes neither a Q95 TOP
selection nor cross-model agreement. Reports display the status. Missing
diagnostics in older files mean an unverified legacy fit.

## AWR source coverage

SQL plotting/model inputs remain a zero-filled **retained-work proxy**. An
absent AWR TOP row is not a measured zero. A mask separate from numeric values
records actual elapsed-list/CPU-list membership in each snapshot.
`predictor_coverage` reports observed rows, observed zeros, missing rows, pairs
with both endpoints observed, nonzero input transitions, percentiles and the
maximum transition's ending index (zero-based in the aligned series).

This makes censoring explicit; it does not reconstruct omitted SQL work or
make the regression an unbiased complete-workload estimator. Transitions into
or out of TOP-list membership require source/timeline verification. Unknown
masks for other inputs remain unknown, not presumed complete.

## MCP, API and regeneration

MCP schema `2026-09-09.2` and local-agent tools support:

```json
{
  "section": "full_gradients",
  "family": "db_time_sql_elapsed_time",
  "contributor": "exact_sql_id",
  "ranking": "peak",
  "offset": 0,
  "limit": 20
}
```

Supply the normal `analysis_id` and, for multiple projects, `project_id`.
Omit `contributor` to page full rankings. Ranking may be `selection`, `active`,
`peak` or `extreme`. `ranking_pages` gives total/returned/next offset per model;
`full_rankings_available=false` distinguishes a legacy source from an absent
contributor. The legacy `*_top` fields remain bounded preview lists, not pages
of the complete fit. Scoped lookups are retained as normal cited evidence.

The classic API prompt receives the same independent TOP unions, diagnostics
and selected-predictor coverage, without duplicating every dense coefficient.
The CLI saves the unabridged object in `report_for_ai.full.json`; MCP/local tools
keep the unabridged object in memory. Shared AI instructions distinguish all
selection/coverage states and do not equate model shares with explained variance.

Rebuild (`cargo build --release`) and rerun the original AWR/JSON analysis.
Restart an existing MCP process with the new executable to rebuild its
in-memory results. Re-converting an old AI Markdown/HTML document does **not**
refit gradients or repair historical conclusions. Old artifacts stay readable
but must not be relabeled v2.

## Verification

`cargo test` covers independent active/peak/extreme selection, fitted zero versus
absence, exact lookup/paging through the MCP evidence wrapper, classic API
context, observation masks, skewed-noise intercepts, unit changes, duplicated
observations, nonconvergence and an independent constrained-QP reference.

`quantile::tests::matches_independent_constrained_quadratic_reference` compares
ADMM with SciPy SLSQP's epigraph formulation (`a + X beta + u - v = y`,
`u,v >= 0`) at lambda 0.05, tau 0.95. Reference standardized objective:
0.11162828248909537; coefficients: 1.56634501 and -0.85125537; intercept:
5.166589596459151. The deterministic 30-row input is defined in the test.

For a local source replay, the ignored `replay_gradient_fixture` test accepts
`JASMIN_GRADIENT_FIXTURE` (JSON with aligned `target`, named `features`, named
boolean `observations`) and `JASMIN_GRADIENT_OUTPUT`. It exports full results,
an exact-contributor query and generated HTML. The real-data regression assertion
checks that the reported rare SQL survives Q95 selection on the original inputs.
No private source dataset is included in the repository.

### DB Time and DB CPU target precision (2026-09-13)

DB Time/DB CPU targets now prefer the matching `time_model_stats.time_s` divided by
actual snapshot wall seconds. This avoids Load Profile rounding (for example,
`13.32 / 60 = 0.222 s/s` instead of `0.2 s/s`) distorting adjacent target deltas.
Selection is independent for each metric and snapshot. Missing/disabled Time Model,
invalid/nonpositive exposure or invalid timing values fall back to the collected
Load Profile rate. A measured zero remains zero. If neither source is usable, the
aligned target is unavailable and no gradient is fitted across that missing target.

The same selection feeds gradients, degradation, DB-load anomaly comparisons, peak
filters and metric timelines. `ReportForAI.db_load_sources` records per-target counts
of Time Model, Load Profile fallback and unavailable snapshots. Snapshot tools expose
`db_time_rate`/`db_cpu_rate` with `per_second` and `source`; DB-load metric timelines
include `value_source`. Raw Load Profile rows in the input and snapshot detail remain
unchanged. Recompute existing projects; historical rounded-target ranks and coefficients
are not directly comparable with these fits. No model parameter or TOP policy changed.
