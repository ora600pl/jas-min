use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct StatisticsDescription {
    pub dbcpu_dbtime: String,
    pub median_absolute_deviation: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct TopPeaksSelected {
    pub report_name: String,
    pub report_date: String,
    pub snap_id: u64,
    pub db_time_value: f64,
    pub db_cpu_value: f64,
    pub dbcpu_dbtime_ratio: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct MadAnomaliesEvents {
    pub anomaly_date: String,
    pub mad_score: f64,
    pub total_wait_s: f64,
    pub number_of_waits: u64,
    pub avg_wait_time_for_execution_ms: f64,
    pub pct_of_db_time: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct MadAnomaliesSQL {
    pub anomaly_date: String,
    pub mad_score: f64,
    pub elapsed_time_cumulative_s: f64,
    pub number_of_executions: u64,
    pub avg_exec_time_for_execution: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct TopForegroundWaitEvents {
    pub event_name: String,
    pub correlation_with_db_time: f64,
    pub marked_as_top_in_pct_of_probes: f64,
    pub avg_pct_of_dbtime: f64,
    pub stddev_pct_of_db_time: f64,
    pub avg_wait_time_s: f64,
    pub stddev_wait_time_s: f64,
    pub avg_number_of_executions: f64,
    pub stddev_number_of_executions: f64,
    pub avg_wait_for_execution_ms: f64,
    pub stddev_wait_for_execution_ms: f64,
    pub median_absolute_deviation_anomalies: Vec<MadAnomaliesEvents>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tables_associated_with_event_based_on_ash_sql: Option<Vec<String>>,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct TopBackgroundWaitEvents {
    pub event_name: String,
    pub correlation_with_db_time: f64,
    pub marked_as_top_in_pct_of_probes: f64,
    pub avg_pct_of_dbtime: f64,
    pub stddev_pct_of_db_time: f64,
    pub avg_wait_time_s: f64,
    pub stddev_wait_time_s: f64,
    pub avg_number_of_executions: f64,
    pub stddev_number_of_executions: f64,
    pub avg_wait_for_execution_ms: f64,
    pub stddev_wait_for_execution_ms: f64,
    pub median_absolute_deviation_anomalies: Vec<MadAnomaliesEvents>,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct PctOfTimesThisSQLFoundInOtherTopSections {
    pub sqls_by_cpu_time_pct: f64,
    pub sqls_by_user_io_pct: f64,
    pub sqls_by_reads: f64,
    pub sqls_by_gets: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct WaitEventsWithStrongCorrelation {
    pub event_name: String,
    pub correlation_value: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct WaitEventsFromASH {
    pub event_name: String,
    pub avg_pct_of_dbtime_in_sql: f64,
    pub stddev_pct_of_dbtime_in_sql: f64,
    pub count: u64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct TopSQLsByElapsedTime {
    pub sql_id: String,
    pub module: String,
    pub sql_type: String,
    pub pct_of_time_sql_was_found_in_other_top_sections: PctOfTimesThisSQLFoundInOtherTopSections,
    pub correlation_with_db_time: f64,
    pub marked_as_top_in_pct_of_probes: f64,
    pub avg_elapsed_time_by_exec: f64,
    pub stddev_elapsed_time_by_exec: f64,
    pub avg_cpu_time_by_exec: f64,
    pub stddev_cpu_time_by_exec: f64,
    pub avg_elapsed_time_cumulative_s: f64,
    pub stddev_elapsed_time_cumulative_s: f64,
    pub avg_cpu_time_cumulative_s: f64,
    pub stddev_cpu_time_cumulative_s: f64,
    pub avg_number_of_executions: f64,
    pub stddev_number_of_executions: f64,
    pub median_absolute_deviation_anomalies: Vec<MadAnomaliesSQL>,
    pub wait_events_with_strong_pearson_correlation: Vec<WaitEventsWithStrongCorrelation>,
    pub wait_events_found_in_ash_sections_for_this_sql: Vec<WaitEventsFromASH>,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct StatsSummary {
    pub statistic_name: String,
    pub avg_value: f64,
    pub stddev_value: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct IOStatsByFunctionSummary {
    pub function_name: String,
    pub statistics_summary: Vec<StatsSummary>,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct LatchActivitySummary {
    pub latch_name: String,
    pub get_requests_avg: f64,
    pub weighted_miss_pct: f64,
    pub wait_time_weighted_avg_s: f64,
    pub found_in_pct_of_probes: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct Top10SegmentStats {
    pub segment_name: String,
    pub segment_type: String,
    pub object_id: u64,
    pub data_object_id: u64,
    pub avg: f64,
    pub stddev: f64,
    pub pct_of_occuriance: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct InstanceStatisticCorrelation {
    pub stat_name: String,
    pub pearson_correlation_value: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct LoadProfileAnomalies {
    pub load_profile_stat_name: String,
    pub anomaly_date: String,
    pub mad_score: f64,
    pub mad_threshold: f64,
    pub per_second: f64,
    pub avg_value_per_second: f64,
}

impl LoadProfileAnomalies {
    pub(crate) fn compare_severity(a: &Self, b: &Self) -> std::cmp::Ordering {
        b.mad_score
            .total_cmp(&a.mad_score)
            .then_with(|| a.load_profile_stat_name.cmp(&b.load_profile_stat_name))
            .then_with(|| a.anomaly_date.cmp(&b.anomaly_date))
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct AnomalyDescription {
    pub area_of_anomaly: String,
    pub statistic_name: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct AnomlyCluster {
    pub begin_snap_id: u64,
    pub begin_snap_date: String,
    pub anomalies_detected: Vec<AnomalyDescription>,
    pub number_of_anomalies: u64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct GradientSettings {
    #[serde(default)]
    pub methodology_version: String,
    #[serde(default)]
    pub selection_policy: String,
    #[serde(default)]
    pub top_n_per_metric: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantile95: Option<crate::quantile::QuantileDiagnostics>,
    pub ridge_lambda: f64,
    /// Selected Elastic Net lambda. In automatic mode this is the value chosen
    /// by forward-chaining validation and used for the final full-data fit.
    pub elastic_net_lambda: f64,
    #[serde(default)]
    pub elastic_net_lambda_mode: String,
    #[serde(default)]
    pub elastic_net_lambda_max: f64,
    #[serde(default)]
    pub elastic_net_lambda_ratio: f64,
    #[serde(default)]
    pub elastic_net_cv_folds: usize,
    #[serde(default)]
    pub elastic_net_cv_rule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elastic_net_cv_mean_loss: Option<f64>,
    #[serde(default)]
    pub elastic_net_nonzero_coefficients: usize,
    #[serde(default)]
    pub elastic_net_target_standardized: bool,
    pub elastic_net_alpha: f64,
    pub elastic_net_max_iter: usize,
    pub elastic_net_tol: f64,
    pub input_wait_event_unit: String,
    pub input_db_time_unit: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct GradientTopItem {
    pub event_name: String,
    pub gradient_coef: f64,
    pub impact: f64,        // typical (MAD-based) — legacy, keep for compatibility
    pub impact_active: f64, // P90 over all absolute deltas, including zeros
    pub impact_peak: f64,   // P99, not the maximum
    pub impact_share: f64,  // % of total active impact
    /// Maximum input |delta| contribution; may involve missingness proxies.
    #[serde(default)]
    pub impact_extreme: f64,
    #[serde(default)]
    pub selection_reasons: Vec<String>,
    #[serde(default)]
    pub active_rank: Option<usize>,
    #[serde(default)]
    pub peak_rank: Option<usize>,
    #[serde(default)]
    pub extreme_rank: Option<usize>,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct GradientCoverage {
    pub event_name: String,
    pub samples: usize,
    pub nonzero_deltas: usize,
    pub p90_abs_delta: f64,
    pub p99_abs_delta: f64,
    pub max_abs_delta: f64,
    /// Index of the ending sample of the largest absolute transition.
    pub max_delta_end_index: usize,
    /// None means the source did not provide an observation mask.
    pub observed_samples: Option<usize>,
    pub observed_zero_samples: Option<usize>,
    pub observed_delta_pairs: Option<usize>,
    pub missing_samples: Option<usize>,
    pub input_policy: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct DbTimeGradientSection {
    pub settings: GradientSettings,
    pub ridge_top: Vec<GradientTopItem>,
    pub elastic_net_top: Vec<GradientTopItem>,
    pub huber_top: Vec<GradientTopItem>,
    pub quantile95_top: Vec<GradientTopItem>,
    /// Full signed fits, including zero and negative coefficients. TOP is a view.
    #[serde(default)]
    pub model_rankings: BTreeMap<String, Vec<GradientTopItem>>,
    #[serde(default)]
    pub predictor_coverage: Vec<GradientCoverage>,
    pub cross_model_classifications: Vec<CrossModelClassification>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vif_diagnostics: Vec<VifDiagnostic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collinear_group_impacts: Vec<CollinearGroupImpact>,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct CrossModelClassification {
    pub event_name: String,
    pub classification: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub in_ridge: bool,
    pub in_elastic_net: bool,
    pub in_huber: bool,
    pub in_quantile95: bool,
    pub priority: u8,
    pub combined_impact: f64,
    pub combined_peak_impact: f64,
    #[serde(default)]
    pub combined_extreme_impact: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct VifDiagnostic {
    pub event_name: String,
    pub vif: f64,
    pub interpretation: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct CollinearGroupImpact {
    pub group_members: Vec<String>,
    pub combined_impact: f64,
    pub combined_coef: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct DbTimeDegradationReport {
    pub is_degradation_detected: bool,
    pub verdict: String,
    pub baseline_start: String,
    pub baseline_end: String,
    pub degraded_start: String,
    pub degraded_end: String,
    pub baseline_samples: usize,
    pub degraded_samples: usize,
    pub db_time_baseline_avg: f64,
    pub db_time_degraded_avg: f64,
    pub db_time_delta_avg: f64,
    pub db_time_delta_pct: f64,
    pub db_time_robust_z_score: f64,
    pub db_cpu_baseline_avg: f64,
    pub db_cpu_degraded_avg: f64,
    pub db_cpu_delta_avg: f64,
    pub db_cpu_delta_pct: f64,
    pub dominant_domains: Vec<DbTimeDegradationDomainSummary>,
    pub findings: Vec<DbTimeDegradationFinding>,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct DbTimeDegradationDomainSummary {
    pub domain: String,
    pub findings_count: usize,
    #[serde(default)]
    pub max_change_score: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct DbTimeDegradationFinding {
    pub domain: String,
    pub name: String,
    pub baseline_avg: f64,
    pub degraded_avg: f64,
    pub delta_avg: f64,
    pub delta_pct: f64,
    pub robust_z_score: f64,
    pub correlation_with_db_time: f64,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub change_score: f64,
    #[serde(default)]
    pub domain_rank: usize,
    pub severity: String,
    pub evidence: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct ReportForAI {
    pub general_data: StatisticsDescription,
    pub top_spikes_marked: Vec<TopPeaksSelected>,
    pub top_foreground_wait_events: Vec<TopForegroundWaitEvents>,
    pub top_background_wait_events: Vec<TopBackgroundWaitEvents>,
    pub top_sqls_by_elapsed_time: Vec<TopSQLsByElapsedTime>,
    pub io_stats_by_function_summary: Vec<IOStatsByFunctionSummary>,
    pub latch_activity_summary: Vec<LatchActivitySummary>,
    pub top_10_segments_by_row_lock_waits: Vec<Top10SegmentStats>,
    pub top_10_segments_by_physical_writes: Vec<Top10SegmentStats>,
    pub top_10_segments_by_physical_write_requests: Vec<Top10SegmentStats>,
    pub top_10_segments_by_physical_read_requests: Vec<Top10SegmentStats>,
    pub top_10_segments_by_logical_reads: Vec<Top10SegmentStats>,
    pub top_10_segments_by_direct_physical_writes: Vec<Top10SegmentStats>,
    pub top_10_segments_by_direct_physical_reads: Vec<Top10SegmentStats>,
    pub top_10_segments_by_buffer_busy_waits: Vec<Top10SegmentStats>,
    pub instance_stats_pearson_correlation: Vec<InstanceStatisticCorrelation>,
    pub load_profile_anomalies: Vec<LoadProfileAnomalies>,
    pub anomaly_clusters: Vec<AnomlyCluster>,
    pub db_time_gradient_fg_wait_events: Option<DbTimeGradientSection>,
    pub db_time_gradient_instance_stats_counters: Option<DbTimeGradientSection>,
    pub db_time_gradient_instance_stats_volumes: Option<DbTimeGradientSection>,
    pub db_time_gradient_instance_stats_time: Option<DbTimeGradientSection>,
    pub db_time_gradient_sql_elapsed_time: Option<DbTimeGradientSection>,
    pub db_cpu_gradient_instance_stats: Option<DbTimeGradientSection>,
    pub db_cpu_gradient_sql_cpu_time: Option<DbTimeGradientSection>,
    pub custom_gradient_wait_events: Option<DbTimeGradientSection>,
    pub custom_gradient_instance_stats: Option<DbTimeGradientSection>,
    pub db_time_degradation_report: Option<DbTimeDegradationReport>,
    #[serde(default)]
    pub performance_hints: Option<crate::performance_hints::PerformanceHintsReport>,
    #[serde(default)]
    pub db_load_sources: BTreeMap<String, crate::measurements::TargetSourceCounts>,
    pub initialization_parameters: HashMap<String, String>,
}
