## Interpret scan and row-continuation signals in the existing analysis

Use the existing DB Time/DB CPU gradients, DB Time degradation, instance-statistic
correlations and SQL timelines to form actionable hypotheses. Do not require a separate
access-path report, SQL-attributed counters or structural measurements before suggesting
a possible mechanism. Those measurements confirm or reject a hypothesis later.

When scan/continuation statistics are prominent, or DB Time/DB CPU and logical reads
increase together, inspect `table scan blocks gotten`, `table scan rows gotten`,
`table fetch continued row`, `consistent gets` and `session logical reads` in the same
baseline/recent windows. Where available, use `get_precomputed_analysis` for `full_gradients`
(DB Time counter and DB CPU instance-statistic families, exact `contributor`) and
`db_time_degradation`; use `get_metric_time_series(kind="instance_stat", name=...,
field="per_second")` plus raw totals for materiality. In classic API mode, use supplied
gradients and the available timeline tools. Without tools, use the supplied evidence and state which comparison is unavailable. Outside TOP, a zero Elastic Net
coefficient, a missing predictor or an unconverged Q95 fit does not establish absence
of the mechanism. Preserve counterevidence and collinearity limits.

- Increased scan work and logical reads, especially rising SQL buffer gets/execution
  and elapsed/execution at comparable useful work, support a hypothesis of inefficient
  scanning. Empty or sparsely populated blocks below the high water mark are one
  possible explanation. Compare execution volume, actual data growth, selectivity,
  access plans and cache/concurrency changes as competing explanations. Increased
  instance activity alone cannot distinguish more work from a more expensive scan.
- Material `table fetch continued row` activity aligned with rising logical reads,
  SQL cost and DB CPU/DB Time supports a row chaining or migration hypothesis. A tiny
  absolute count with a large percentage change or a TAIL_RISK/ROBUST_ONLY label is
  context, not a priority diagnosis. Chaining and migration cannot be distinguished
  from this counter alone. Cached extra block visits can cost CPU without increased
  physical reads or prominent I/O waits; their absence does not rule the hypothesis out.
- `table scan rows gotten` counts scan activity, not live rows or useful application
  work. Neither blocks/rows gotten nor an aggregate query's ROWS_PROCESSED establishes
  empty-block percentage or cost per useful row. Normalize interval totals by actual
  wall seconds; compare SQL costs using observed executions and equivalent work.
  Missing observations, TOP-list omissions and unavailable host CPU are unknown, not zero.

Put a concise conclusion in the existing gradient/degradation synthesis or SQL finding:
observed symptom and aligned values -> plausible mechanism and alternatives -> affected
SQL/object candidates if supported -> one targeted confirmation step and success criterion.
For scanning, check the candidate segment's space below HWM against useful data and its
actual scan plan. For continuation, obtain SQL/session attribution and inspect chained or
migrated rows in the implicated object. Instance counters and SQL text alone do not
attribute a mechanism to a segment; FS4 is not an empty-block count. Label an unconfirmed
mechanism as a hypothesis with the missing proof, rather than suppressing a useful lead.
Do not automatically recommend MOVE, SHRINK or REBUILD: choose a remedy after confirmation
and verify lower gets/elapsed per equivalent work with a controlled before/after comparison.
No separate empty-blocks section or exhaustive counter dump is required.
