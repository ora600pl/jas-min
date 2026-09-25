# Reproducible test fixtures

These small fixtures are tracked separately from ignored, local customer data in
`tests/` and `test_runs/`. No DNV or USK4 measurements are included here.

- `empty_calories/hints_native.json`: reduced 61-window Oracle EmptyCalories lab
  collection. Retains SQL `9paxwp1pabugh`, ERP_SPARSE/ERP_DENSE logical reads,
  scan counters, DB time/CPU, and source availability. Other Top-N sections are
  emptied. Values and time boundaries are unchanged; filenames are basenames.
- `empty_calories/native_scan_targets.json`: 24 windows from the same lab's
  `jasmin_only/native_scan_prefix.json`, retaining `snap_info`, `load_profile`,
  `time_model_stats`, `instance_stats`, and `data_availability` without edits.
- `empty_calories/scan_counterexamples.json`: recorded calibration trials, copied
  from `jasmin_only/evidence/calibration.json` without edits.
- `empty_calories/{scan,migr}_degradation.json`: copies of the original controlled
  15-window lab replays. Their synthetic temporal structure is unchanged.
- `empty_calories/segment_scope.html` and `initialization_parameters.json`:
  restored synthetic parser regression cases (scoped/legacy segments and five
  parameter table variants). These are not recorded customer reports.
- `instance_efficiency.json`: replacement synthetic HTML/text parser cases for
  paired metrics, nested labels, zero values and section boundaries. The old
  untracked instance-efficiency report corpus was not recovered; missing-value
  tests remain in Rust as well.

`lab-source-sha256.json` identifies the source lab files before reduction.
Run the complete default suite with `cargo test --offline`.
