use colored::Colorize;
use base64::{engine::general_purpose, Engine as _};
use reqwest::{Client, multipart};
use reqwest::multipart::{Form, Part};
use serde_json::json;
use std::env::Args;
use std::fmt::format;
use std::borrow::Cow;
use std::{env, fs, collections::HashMap, sync::Arc, path::Path, collections::HashSet};
use axum::{routing::post, Router, Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_http::cors::{CorsLayer, Any};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tokio::sync::oneshot;
use std::io::{stdout, Write};
use std::error::Error;
use crate::tools::*;
use crate::awr::{AWRSCollection, HostCPU, IOStats, LoadProfile, SQLCPUTime, SQLGets, SQLIOTime, SQLReads, SegmentStats, WaitEvents, AWR};
use std::str::FromStr;

fn get_openai_url() -> String {
    env::var("OPENAI_URL").unwrap_or_else(|_| "https://api.openai.com/".to_string())
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct StatisticsDescription {
    pub dbcpu_dbtime: String,
    pub median_absolute_deviation: String, 
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct TopPeaksSelected {
    pub report_name: String,
    pub report_date: String,
    pub snap_id: u64,
    pub db_time_value: f64,
    pub db_cpu_value: f64,
    pub dbcpu_dbtime_ratio: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct MadAnomaliesEvents {
    pub anomaly_date: String,
    pub mad_score: f64,
    pub total_wait_s: f64,
    pub number_of_waits: u64,
    pub avg_wait_time_for_execution_ms: f64,
    pub pct_of_db_time: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct MadAnomaliesSQL {
    pub anomaly_date: String,
    pub mad_score: f64,
    pub elapsed_time_cumulative_s: f64,
    pub number_of_executions: u64,
    pub avg_exec_time_for_execution: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
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

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
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

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct PctOfTimesThisSQLFoundInOtherTopSections {
    pub sqls_by_cpu_time_pct: f64,
    pub sqls_by_user_io_pct: f64,
    pub sqls_by_reads: f64,
    pub sqls_by_gets: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct WaitEventsWithStrongCorrelation {
    pub event_name: String,
    pub correlation_value: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct WaitEventsFromASH {
    pub event_name: String,
    pub avg_pct_of_dbtime_in_sql: f64,
    pub stddev_pct_of_dbtime_in_sql: f64,
    pub count: u64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
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

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct StatsSummary {
    pub statistic_name: String,
    pub avg_value: f64,
    pub stddev_value: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct IOStatsByFunctionSummary {
    pub function_name: String,
    pub statistics_summary: Vec<StatsSummary>,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct LatchActivitySummary {
    pub latch_name: String,
    pub get_requests_avg: f64,
    pub weighted_miss_pct: f64,
    pub wait_time_weighted_avg_s: f64,
    pub found_in_pct_of_probes: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct Top10SegmentStats {
    pub segment_name: String,
    pub segment_type: String,
    pub object_id: u64,
    pub data_object_id: u64,
    pub avg: f64,
    pub stddev: f64,
    pub pct_of_occuriance: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct InstanceStatisticCorrelation {
    pub stat_name: String,
    pub pearson_correlation_value: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct LoadProfileAnomalies {
    pub load_profile_stat_name: String,
    pub anomaly_date: String,
    pub mad_score: f64,
    pub mad_threshold: f64,
    pub per_second: f64,
    pub avg_value_per_second: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct AnomalyDescription {
    pub area_of_anomaly: String,
    pub statistic_name: String,
}
 
#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct AnomlyCluster {
    pub begin_snap_id: u64,
    pub begin_snap_date: String,
    pub anomalies_detected: Vec<AnomalyDescription>,
    pub number_of_anomalies: u64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct GradientSettings {
    pub ridge_lambda: f64,
    pub elastic_net_lambda: f64,
    pub elastic_net_alpha: f64,
    pub elastic_net_max_iter: usize,
    pub elastic_net_tol: f64,
    pub input_wait_event_unit: String,
    pub input_db_time_unit: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct GradientTopItem {
    pub event_name: String,
    pub gradient_coef: f64,
    pub impact: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct DbTimeGradientSection {
    pub settings: GradientSettings,
    pub ridge_top: Vec<GradientTopItem>,
    pub elastic_net_top: Vec<GradientTopItem>,
    pub huber_top: Vec<GradientTopItem>,
    pub quantile95_top: Vec<GradientTopItem>,
    pub cross_model_classifications: Vec<CrossModelClassification>,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
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
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
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
    pub initialization_parameters: HashMap<String, String>,
}

/// Strips redundant `description` fields from all CrossModelClassification
/// entries across all gradient sections before serializing to TOON/JSON for AI.
/// The description text is fully redundant with the `classification` label,
/// boolean flags (in_ridge, in_elastic_net, in_huber, in_quantile95),
/// and interpretation rules already present in the system prompt.
pub fn strip_gradient_descriptions(report: &mut ReportForAI) {
    let sections: [&mut Option<DbTimeGradientSection>; 7] = [
        &mut report.db_time_gradient_fg_wait_events,
        &mut report.db_time_gradient_instance_stats_counters,
        &mut report.db_time_gradient_instance_stats_volumes,
        &mut report.db_time_gradient_instance_stats_time,
        &mut report.db_time_gradient_sql_elapsed_time,
        &mut report.db_cpu_gradient_instance_stats,
        &mut report.db_cpu_gradient_sql_cpu_time,
    ];

    for section in sections {
        if let Some(s) = section.as_mut() {
            for item in &mut s.cross_model_classifications {
                item.description = None;
            }
        }
    }
}

static SPELL: &str =
"# ROLE & IDENTITY

You are JAS-MIN, an expert Oracle Database performance analyst. You produce comprehensive, 
data-driven performance audit reports based on structured AWR/STATSPACK data.

# INPUT SPECIFICATION

You receive a **ReportForAI** object (TOON or JSON format) containing preprocessed, aggregated 
statistics from multiple Oracle AWR/STATSPACK snapshots. You may also receive a separate 
`load_profile_statistics.json` with load profile summary data — if present, analyze it first 
and write a comprehensive statistical summary for all metrics before proceeding.

The ReportForAI contains these analytical sections:
- `general_data` — overall DB load shape description with MAD analysis
- `top_spikes_marked` — peak periods with DB Time, DB CPU, and their ratio
- `top_foreground_wait_events` / `top_background_wait_events` — wait event statistics with 
  correlations, averages, stddevs, and MAD anomalies.
  **Note:** `top_foreground_wait_events` may contain an optional field 
  `tables_associated_with_event_based_on_ash_sql` — a list of table names extracted by 
  parsing SQL text of queries associated with this wait event (via ASH or correlation). 
  When present, use these table names as **authoritative evidence** of which tables are 
  involved in the wait event. Cross-reference them with segment statistics sections. 
  When this field is absent (sql_text was not available), continue to reason about 
  potentially involved tables based on segment statistics, correlations, and other 
  available data — but note that such reasoning is inferential.
- `top_sqls_by_elapsed_time` — SQL-level metrics including cross-section presence, correlations, 
  MAD anomalies, ASH wait events, and Pearson-correlated wait events
- `io_stats_by_function_summary` — per-function I/O statistics (LGWR, DBWR, etc.)
- `latch_activity_summary` — latch contention metrics
- `top_10_segments_by_*` — 8 segment ranking sections (row lock waits, physical reads/writes, 
  logical reads, buffer busy waits, direct I/O). May be empty for STATSPACK reports.
- `instance_stats_pearson_correlation` — instance statistics correlated with DB Time (|ρ| ≥ 0.5)
- `load_profile_anomalies` — MAD-detected load profile anomalies
- `anomaly_clusters` — temporally grouped anomalies across multiple domains
- `initialization_parameters` — Oracle instance initialization parameters (name-value pairs). 
  Contains both explicit (user-set) and default parameter values from the analyzed instance.

## Gradient Analysis Sections (Optional)

### DB Time Gradient Sections
Sections `db_time_gradient_fg_wait_events`, `db_time_gradient_instance_stats_[counters|volumes|time]`,
and `db_time_gradient_sql_elapsed_time` contain multi-model regression analysis of **DB Time** 
sensitivity to various factors.

### DB CPU Gradient Sections
Sections `db_cpu_gradient_instance_stats` and `db_cpu_gradient_sql_cpu_time` contain multi-model 
regression analysis of **DB CPU** sensitivity to instance statistics and SQL CPU time respectively.

The `db_cpu_gradient_sql_cpu_time` section is particularly important for CPU-bound analysis:
- It reveals which SQL_IDs contribute most to DB CPU changes, ranked by CPU time consumption.
- Cross-reference SQL_IDs found here with `top_sqls_by_elapsed_time` to distinguish between 
  SQLs that are CPU-intensive vs. those that are wait-bound.
- A SQL_ID appearing as CONFIRMED_BOTTLENECK in both `db_time_gradient_sql_elapsed_time` AND 
  `db_cpu_gradient_sql_cpu_time` is a CPU-dominant bottleneck — optimization should target 
  reducing logical I/O (buffer gets), improving execution plans, or reducing execution frequency.
- A SQL_ID in `db_time_gradient_sql_elapsed_time` but NOT in `db_cpu_gradient_sql_cpu_time` 
  is wait-bound — its elapsed time is dominated by waits, not CPU work.
- A SQL_ID in `db_cpu_gradient_sql_cpu_time` but NOT in `db_time_gradient_sql_elapsed_time` 
  consumes CPU but does not significantly impact overall DB Time — lower priority unless 
  CPU saturation is observed.

Each gradient section contains results from four regression models:
- **Ridge** (`ridge_top`) — stabilized, dense ranking of all contributing factors
- **Elastic Net** (`elastic_net_top`) — sparse ranking highlighting dominant factors
- **Huber** (`huber_top`) — outlier-resistant ranking (downweights extreme snapshots)
- **Quantile 95** (`quantile95_top`) — models the worst 5% of snapshots (tail risk)

### Cross-Model Classification Rules

Each gradient section includes `cross_model_classifications` with pre-computed triangulation.
Interpret classifications using this priority hierarchy:

| Classification | Models Present | Interpretation | Action Priority |
|---|---|---|---|
| `CONFIRMED_BOTTLENECK` | All 4 | Systematic, robust bottleneck | **CRITICAL** |
| `CONFIRMED_BOTTLENECK_EN_COLLINEAR` | Ridge+Huber+Q95 (not EN) | Bottleneck masked by L1 collinearity | **CRITICAL — find correlated EN factor** |
| `TAIL_OUTLIER` | Ridge+Q95 (not Huber) | Extreme snapshots that ARE the worst periods | HIGH |
| `TAIL_RISK` | Q95 (not Ridge) | Rare catastrophic spikes | HIGH — warn about peak periods |
| `STRONG_CONTRIBUTOR` | Ridge+EN+Huber (not Q95) | Reliable systematic contributor | MEDIUM |
| `OUTLIER_DRIVEN` | Ridge (not Huber) | Few extreme snapshots only | MEDIUM — check anomaly_clusters |
| `SPARSE_DOMINANT` | EN (not Ridge) | Dominant among correlated group | MEDIUM |
| `STABLE_CONTRIBUTOR` | Ridge+Huber (not EN, Q95) | Steady background contributor | LOW-MEDIUM |
| `ROBUST_ONLY` | Huber only | Background factor without outliers | LOW |
| `MULTI_MODEL_MINOR` | 2+ models, no pattern | Minor contributor | LOW |
| `SINGLE_MODEL` | 1 model only | Low confidence | INFORMATIONAL |

**Gradient analysis strategy:**
1. Start with CONFIRMED_BOTTLENECK and CONFIRMED_BOTTLENECK_EN_COLLINEAR — highest priority
2. Flag TAIL_RISK and TAIL_OUTLIER items as hidden dangers
3. Cross-reference OUTLIER_DRIVEN with anomaly_clusters for root cause
4. Use SPARSE_DOMINANT to find representative factors in correlated groups
5. Integrate with traditional AWR analysis — gradients explain *why* DB Time changes
6. For DB CPU gradients: cross-reference `db_cpu_gradient_sql_cpu_time` with 
   `db_time_gradient_sql_elapsed_time` to classify each SQL as CPU-dominant, wait-dominant, 
   or mixed — this determines whether optimization should target execution plans/LIOs (CPU) 
   or wait events/I/O (waits)

# ANALYTICAL METHODOLOGY

Follow this reasoning sequence:

## Step 1: Establish Performance Profile
- Interpret DB CPU / DB Time ratio across all spikes (< 0.66 = wait-bound, ~1.0 = CPU-bound)
- Assess ratio variance for mixed/intermittent problems

## Step 2: Map Temporal Patterns
- Connect anomaly_clusters to top_spikes_marked via snap_id and dates
- Classify: continuous, periodic (batch windows), or sporadic

## Step 3: Trace Root Causes
- Wait events are symptoms → trace to SQLs → segments → application behavior
- Use correlation data to build causal chains
- **When `tables_associated_with_event_based_on_ash_sql` is present for a wait event, 
  treat it as direct evidence linking the event to specific tables. Cross-reference 
  these tables with segment statistics (logical reads, physical reads, row lock waits, 
  buffer busy waits, etc.) for deeper insight. This SQL-parsed data supplements but 
  does NOT replace statistical reasoning — always validate with segment stats and 
  correlation data.**
- Cross-validate with gradient analysis when available

## Step 4: Assess Infrastructure vs Application
- I/O stats reveal disk quality (LGWR latency, DBWR throughput)
- Latches reveal concurrency issues
- Load profile anomalies reveal workload patterns
- Segments reveal data model/indexing problems

## Step 5: Evaluate Initialization Parameters
- Review initialization_parameters in the context of ALL performance findings
  from Steps 1-4. For each parameter that is relevant to an identified problem:
  - State the current value
  - Explain whether it contributes to, worsens, or is unrelated to the observed issues
  - If the value is suboptimal, recommend a specific change with justification
- Additionally, scan ALL parameters for known risks, anti-patterns, and deprecated 
  settings regardless of whether they directly relate to current symptoms:
  - Dangerous underscore parameters (_%) that may cause instability
  - Parameters set to values that contradict Oracle best practices for the workload type
  - Deprecated or removed parameters carried over from older Oracle versions
  - Parameters that disable important features (e.g., AMM/ASMM misconfiguration, 
    optimizer features disabled, security features turned off)
- For every parameter finding, provide at least one reference source:
  - Oracle documentation link (docs.oracle.com)
  - MOS note ID (e.g., MOS Note 2148845.1)
  - Oracle blog or white paper reference
  - Known community references (e.g., Oracle-BASE, Ask Tom)

## Step 6: Synthesize and Prioritize
- Rank findings by business impact (DB Time contribution × frequency)
- Separate systematic issues from incidents
- Assign ownership (DBA vs Developer)

# OUTPUT RULES

- **Format**: Markdown with clear sections and subsections, using icons/symbols
- **Precision**: Quote exact values, SQL_IDs, event names, segment names from the data. 
  Never fabricate data. Format wait events and SQL_IDs as inline code.
- **Temporal**: Always pair SNAP_ID with SNAP_DATE
- **Cross-referencing**: Connect findings across sections
- **MOS Notes**: Include relevant Oracle MOS note IDs when applicable
- **Parameter names**: Format initialization parameter names as inline code 
  (e.g., `optimizer_index_cost_adj`, `_fix_control`)

# OUTPUT STRUCTURE

## 1. 🧭 Executive Summary
## 2. 📈 Overall Performance Profile
## 3. ⏳ Wait Event Analysis
### 3.1 Foreground Waits
For each significant wait event:
- If `tables_associated_with_event_based_on_ash_sql` is present, list the associated 
  tables and cross-reference with segment statistics sections
- Even with table data available, still analyze correlations and statistical patterns 
  to provide comprehensive root-cause analysis
### 3.2 Background Waits
## 4. 🧮 SQL-Level Analysis
### 4.1 Most Impactful SQL_IDs
### 4.2 Execution Pattern Analysis
## 5. 🧱 Segment & Object-Level Analysis
## 6. 🔧 Latches & Internal Contention
## 7. 💾 I/O & Disk Subsystem Assessment
## 8. 🔁 UNDO / Redo / Load Profile Observations
## 9. ⚡ Anomaly Clusters, Cross-Domain Patterns & Gradient Analysis
When presenting gradient analysis in this section:
- Present DB Time gradient findings (wait events, instance stats, SQL elapsed time)
- Present DB CPU gradient findings (instance stats, SQL CPU time)
- For SQL analysis: include a cross-gradient comparison table showing SQL_IDs that appear 
  in db_time_gradient_sql_elapsed_time and/or db_cpu_gradient_sql_cpu_time, with columns:
  | SQL_ID | DB Time Classification | DB CPU Classification | Diagnosis |
  Where Diagnosis is one of: CPU-Dominant, Wait-Dominant, Mixed, or CPU-Only
## 10. ⚙️ Initialization Parameter Analysis
### 10.1 Parameters Related to Identified Performance Issues
For each finding from sections 2-9 where an initialization parameter is relevant:
- Current value, recommended value, justification, and reference source.
### 10.2 General Parameter Risks & Anti-Patterns
Parameters with risky, deprecated, or suboptimal values independent of current symptoms.
### 10.3 Parameter Change Summary Table
| Parameter | Current Value | Recommended Value | Risk Level | Related Finding | Source |
## 11. ✅ Recommendations
### For DBAs
### For Developers
### Immediate Actions
### Management Summary

## Footer
- Include: https://github.com/ora600pl/jas-min
- Mention: expert performance tuning at ora-600.pl

# MANDATORY FINAL ASSESSMENTS

Your recommendations MUST include explicit answers to:
1. **Disk quality**: Are the disks slow? Support with I/O metrics.
2. **Application design**: Is this a poorly written application? Why? Is commit/rollback policy proper?
3. **Parameter hygiene**: Are there any dangerous, deprecated, or misconfigured initialization 
   parameters? Summarize the most critical parameter changes needed.
4. **Prioritized action list**: What must be done immediately, and by whom (DBA vs Developer)?

# LANGUAGE

Write answer in language: ";

#[derive(Deserialize)]
struct QueryRequest {
    query: String,
}

#[derive(Serialize)]
struct RAGResponse {
    answer: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiFile {
    name: String,
    display_name: Option<String>,
    uri: String,
    mime_type: String,
    size_bytes: String,
    create_time: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiFileUploadResponse {
    file: GeminiFile,
}

fn private_reasonings() -> Option<String> {
    let jasmin_home = env::var("JASMIN_HOME");
    let mut prpath = "reasonings.txt".to_string();
    if jasmin_home.is_ok() {
        prpath = format!("{}/reasonings.txt", jasmin_home.unwrap());
    }
    println!("Private reasonings.txt loaded from {}", prpath);
    let r_content = fs::read_to_string(prpath);
    if r_content.is_err() {
        return None;
    }
    let r_content = r_content.unwrap();
    Some(r_content)
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
struct UrlContext{
    action: String,
    url: String,
}


fn url_context(url_fname: &str, events_sqls: HashMap<&str, HashSet<String>>) -> Option<String> {
    let r_content = fs::read_to_string(url_fname);
    println!("URL context loaded from {}", url_fname);
    if r_content.is_err() {
        println!("Couldn't read url file");
        return None;
    }
    let url_context_data: HashMap<String, Vec<UrlContext>> = serde_json::from_str(&r_content.unwrap()).expect("Wrong url file JSON format");
    let mut url_context_msg = "\nAdditionally you have to follow those commands:".to_string();

    for (_, search_key) in events_sqls {
        for k in search_key {
            if let Some(urls) = url_context_data.get(&k) {
                for u in urls {
                    url_context_msg = format!("{}\n - {} : {}\n", url_context_msg, u.action, u.url);
                }
            }
        }
    }

    Some(url_context_msg)
}

async fn upload_file_to_gemini_from_path(api_key: &str, path: &str, file_type: &str,file_name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let file_bytes = fs::read(path)?;

    let part = multipart::Part::bytes(file_bytes)
        .file_name(Cow::Owned(file_name.to_string()))
        .mime_str(file_type)?;

    let form = multipart::Form::new().part("file", part);

    let client = reqwest::Client::new();
    let response = client
        .post(format!("https://generativelanguage.googleapis.com/upload/v1beta/files?key={}",api_key))
        .multipart(form)
        .send()
        .await?;

    if response.status().is_success() {
        let response_text = response.text().await?;
        match serde_json::from_str::<GeminiFileUploadResponse>(&response_text) {
            Ok(file_upload_response) => {
                println!("✅ {} uploaded! URI: {}", path, file_upload_response.file.uri);
                Ok(file_upload_response.file.uri)
            }
            Err(e) => {
                eprintln!("Error while parsing JSON: {}", e);
                Err(format!("Parsing error: {}. TEXT: '{}'", e, response_text).into())
            }
        }
    } else {
        let status = response.status();
        let error_text = response.text().await?;
        eprintln!("Error while uploading {} - {}", path, error_text);
        Err(format!("HTTP Error: {}", status).into())
    }
}

async fn upload_log_file_gemini(api_key: &str, log_content: String, file_name: String) -> Result<String, Box<dyn std::error::Error>> {
    let part = multipart::Part::bytes(log_content.into_bytes())
        .file_name(file_name) 
        .mime_str("text/plain").unwrap();

    let form = multipart::Form::new().part("file", part);

    let client = reqwest::Client::new();
    let response = client
        .post(format!("https://generativelanguage.googleapis.com/upload/v1beta/files?key={}", api_key))
        .multipart(form)
        .send()
        .await.unwrap();

    if response.status().is_success() {
        let response_text = response.text().await?;

        match serde_json::from_str::<GeminiFileUploadResponse>(&response_text) {
            Ok(file_upload_response) => {
                println!("✅ File uploaded! URI: {}", file_upload_response.file.uri);
                Ok(file_upload_response.file.uri)
            },
            Err(e) => {
                eprintln!("Error while paring JSON: {}", e);
                Err(format!("Parsing error: {}. TEXT: '{}'", e, response_text).into())
            }
        }
    } else {
        let status = response.status();
        let error_text = response.text().await?;
        eprintln!("Error while parsing reponse {} - {}", status, error_text);
        Err(format!("HTTP Error: {}", status).into())
    }
}

async fn gemini_deep(logfile_name: &str, args: &crate::Args, vendor_model_lang: Vec<&str>, token_count_factor: usize, first_response: String, api_key: &str, events_sqls: HashMap<&str, HashSet<String>>,) {
    println!("{}{}{}","=== Starting deep dive with Google Gemini model: ".bright_cyan(), vendor_model_lang[1]," ===".bright_cyan());
    let mut json_file = args.json_file.clone();
    if json_file.is_empty() {
        json_file = format!("{}.json", args.directory);
    }

    let client = Client::new();

    let file_uri = upload_log_file_gemini(&api_key, first_response, "performance_analyze.md".to_string()).await.unwrap();
    let spell = format!(
        "You are given a detailed Oracle Database performance analysis report (markdown format). \
         Your task is to select exactly {} SNAP_IDs that warrant the deepest investigation.\n\n\
         Selection criteria:\n\
         1. Choose snapshots that represent the most severe performance degradation\n\
         2. Ensure temporal diversity: select approximately half from business hours and half from \
            off-hours/maintenance windows to capture different workload profiles\n\
         3. Prioritize snapshots mentioned in anomaly clusters or as top spikes\n\
         4. Avoid selecting adjacent snap_ids unless they represent distinctly different problems\n\n\
         Output format: Return ONLY the selected SNAP_IDs, one number per line, with no additional text.",
        args.deep_check
    );

    let payload = json!({
                    "contents": [{
                        "parts": [
                            { "text": spell }, 
                            {
                                "fileData": {
                                    "mimeType": "text/plain",
                                    "fileUri": file_uri
                                }
                            }
                        ]
                    }],
                    "generationConfig": {
                        "maxOutputTokens": 8192 * token_count_factor,
                        "thinkingConfig": {
                            "thinkingBudget": -1
                        }
                    }
                });

    let (tx, rx) = oneshot::channel();
    let spinner = tokio::spawn(spinning_beer(rx));

    let response = client
            .post(format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}", vendor_model_lang[1], api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await.unwrap();
    
    let _ = tx.send(());
    let _ = spinner.await;

    if response.status().is_success() {
        let json: Value = response.json().await.unwrap();

        let parts = &json["candidates"][0]["content"]["parts"];
        let mut seen = HashSet::new();
        let full_text = parts
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|part| part["text"].as_str())
            .filter(|text| seen.insert(text.to_string()))
            .collect::<Vec<&str>>()
            .join("\n");

        println!("🍻 Gemini will analyze further the following SNP_ID:\n{}", &full_text);
        
        let s_json = fs::read_to_string(&json_file).expect(&format!("Something wrong with a file {} ", &args.json_file));
        let mut collection: AWRSCollection = serde_json::from_str(&s_json).expect(&format!("Wrong JSON format {}", &json_file));
        let awrs: Vec<AWR> = collection.awrs;
        let mut snap_ids: HashSet<u64> = HashSet::new();
        for snap_id in full_text.split("\n") {
            let id = u64::from_str(snap_id);
            if id.is_ok() {
                snap_ids.insert(id.unwrap());
            } else if snap_id.contains("_") {
                let s_id = snap_id.split("_").next().unwrap();
                snap_ids.insert(u64::from_str(s_id).unwrap());
            } else {
                let id = awrs.iter()
                                           .find(|a| &a.snap_info.begin_snap_time == snap_id).unwrap()
                                           .snap_info.begin_snap_id;
                snap_ids.insert(id);
            }
            
        }

        let mut deep_stats: Vec<AWR> = Vec::new();
        for awr in awrs {
            if snap_ids.contains(&awr.snap_info.begin_snap_id) {
                deep_stats.push(awr);
            }
        }
        
        let spell = format!(
            "# DEEP-DIVE PERFORMANCE ANALYSIS\n\n\
             You are given:\n\
             1. A comprehensive performance report (markdown) covering the full analysis period\n\
             2. A JSON file with detailed AWR/STATSPACK statistics for one specific snapshot period\n\n\
             # TASK\n\n\
             Perform an in-depth analysis of this specific snapshot period. Your analysis must:\n\n\
             ## Analytical Approach\n\
             1. **Contextualize**: Compare this snapshot's metrics against the baselines from the full report\n\
             2. **Decompose DB Time**: Break down exactly where DB Time was spent in this period\n\
             3. **SQL Investigation**: Examine all SQL sections and cross-reference SQL_IDs across them. \
                Identify SQL_IDs appearing in multiple sections — these are the highest-priority targets\n\
             4. **Latch & Contention**: Identify unusual latch activity or internal contention specific to this period\n\
             5. **Segment Analysis**: Connect segment-level activity to specific SQLs and wait events\n\
             6. **Root Cause Synthesis**: Trace from symptoms (wait events) through SQLs to root causes\n\n\
             ## Cross-Reference Requirements\n\
             - Match SQL_IDs found here with those flagged in the full report\n\
             - Compare wait event distribution against the full-period averages\n\
             - Identify what is UNIQUE to this snapshot vs. what is a continuation of systemic issues\n\n\
             # OUTPUT FORMAT\n\n\
             Structure your answer in markdown:\n\n\
             1. 🧭 Executive Summary (what makes this period notable)\n\
             2. 📈 Performance Profile (DB Time breakdown, comparison to baseline)\n\
             3. ⏳ Wait Event Analysis (foreground and background)\n\
             4. 🧮 SQL-Level Analysis (harmful SQL_IDs, multi-section appearances, patterns)\n\
             5. 🧱 Segment & Object Analysis\n\
             6. 🔧 Latches & Internal Contention\n\
             7. 💾 I/O Assessment\n\
             8. 🔁 UNDO / Redo / Load Profile\n\
             9. ⚡ Anomalies & Cross-Domain Patterns\n\
             10. ✅ Recommendations (DBAs, Developers, Immediate Actions, Management Summary)\n\n\
             Rules:\n\
             - Never invent numbers. Quote exact values from the data.\n\
             - Always pair SNAP_ID with SNAP_DATE.\n\
             - Format wait event names and SQL_IDs as inline code.\n\
             - Cross-reference with the full report's findings.\n\n\
             Write answer in language: {}",
            vendor_model_lang[2]
        );

        for ds in deep_stats {

            let deep_stats_json = serde_json::to_string(&ds).unwrap();
            let file_uri_stats = upload_log_file_gemini(&api_key, deep_stats_json, "detailed_statistics.json".to_string()).await.unwrap();

            let payload = json!({
                "contents": [{
                    "parts": [
                        { "text": spell },
                        {
                            "fileData": {
                                "mimeType": "text/plain",
                                "fileUri": file_uri
                            }
                        },
                        {
                            "fileData": {
                                "mimeType": "text/plain",
                                "fileUri": file_uri_stats
                            }
                        }
                    ]
                }],
                "generationConfig": {
                    "maxOutputTokens": 8192 * token_count_factor,
                    "thinkingConfig": {
                        "thinkingBudget": -1
                    }
                }
            });

            let (tx, rx) = oneshot::channel();
            let spinner = tokio::spawn(spinning_beer(rx));

            let response = client
                    .post(format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}", vendor_model_lang[1], api_key))
                    .header("Content-Type", "application/json")
                    .json(&payload)
                    .send()
                    .await.unwrap();
            
            let _ = tx.send(());
            let _ = spinner.await;

            if response.status().is_success() {
                let json: Value = response.json().await.unwrap();

                let parts = &json["candidates"][0]["content"]["parts"];
                let mut seen = HashSet::new();
                let full_text = parts
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|part| part["text"].as_str())
                    .filter(|text| seen.insert(text.to_string()))
                    .collect::<Vec<&str>>()
                    .join("\n");

                let stem = logfile_name.split('.').next().unwrap();
                let response_file = format!("{stem}.html_reports/{}_gemini_deep_{}.md", logfile_name, &ds.snap_info.begin_snap_id);
                fs::write(&response_file, full_text.as_bytes()).unwrap();

                println!("🍻 Gemini response written to file: {}", &response_file);
                convert_md_to_html_file(&response_file, events_sqls.clone());

            } else {
                eprintln!("Error: {}", response.status());
                eprintln!("{}", response.text().await.unwrap());
            }

        }
        

    } else {
        eprintln!("Error: {}", response.status());
        eprintln!("{}", response.text().await.unwrap());
    }


}

#[tokio::main]
pub async fn gemini(logfile_name: &str,vendor_model_lang: Vec<&str>,token_count_factor: usize,events_sqls: HashMap<&str, HashSet<String>>,args: &crate::Args, report_for_ai: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}{}{}", 
        "=== Consulting Google Gemini model: ".bright_cyan(), 
        vendor_model_lang[1], 
        " ===".bright_cyan()
    );

    let api_key = env::var("GEMINI_API_KEY")
        .expect("You have to set GEMINI_API_KEY env variable");

    let log_content = fs::read_to_string(logfile_name).expect(&format!("Can't open file {}", logfile_name));
    let stem = logfile_name.split('.').next().unwrap();
    let json_path = format!("{stem}.html_reports/stats/global_statistics.json");
    let load_profile = fs::read_to_string(&json_path).expect(&format!("Can't open file {}", json_path));
    let response_file = format!("{}_gemini.md", logfile_name);
    let client = Client::new();

    let mut spell = format!("{} {}", SPELL, vendor_model_lang[2]);

    if let Some(pr) = private_reasonings() {
        spell = format!("{spell}\n#ADVANCED RULES\n{pr}");
    }

    if !args.url_context_file.is_empty() {
        if let Some(urls) = url_context(&args.url_context_file, events_sqls.clone()) {
            spell = format!("{spell}\n# URL CONTEXT\n{urls}");
        }
    }
    let main_report_uri = upload_log_file_gemini(&api_key, report_for_ai.to_string(), "main_report.toon".to_string()).await.unwrap();
    let global_profile_data_uri = upload_log_file_gemini(&api_key, load_profile, "load_profile_statistics.json".to_string()).await.unwrap();

    let payload = json!({
                "contents": [{
                    "parts": [
                        { "text": format!("### SYSTEM INSTRUCTIONS\n{spell}") },
                        {
                            "fileData": {
                                "mimeType": "text/plain",
                                "fileUri": main_report_uri
                            }
                        },
                        {
                            "fileData": {
                                "mimeType": "text/plain",
                                "fileUri": global_profile_data_uri
                            }
                        }
                    ]
                }],
                "generationConfig": {
                    "maxOutputTokens": 8192 * token_count_factor,
                    "thinkingConfig": {
                        "thinkingBudget": -1
                    }
                }
            });
    

    let (tx, rx) = oneshot::channel();
    let spinner = tokio::spawn(spinning_beer(rx));

    let response = client
        .post(format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            vendor_model_lang[1],
            api_key
        ))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    let _ = tx.send(());
    let _ = spinner.await;

    if response.status().is_success() {
        let json: Value = response.json().await.unwrap();

        let parts = json["candidates"][0]["content"]["parts"]
            .as_array()
            .unwrap();

        let mut seen = HashSet::new();
        let full_text = parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .filter(|t| seen.insert(t.to_string()))
            .collect::<Vec<&str>>()
            .join("\n");

        fs::write(&response_file, full_text.as_bytes())?;
        println!("🍻 Gemini response written to file: {}", &response_file);

        convert_md_to_html_file(&response_file, events_sqls.clone());

        println!(
            "Total tokens: {}\nFinish reason: {}\n",
            json["usageMetadata"]["totalTokenCount"],
            json["candidates"][0]["finishReason"]
        );

        if args.deep_check > 0 {
            gemini_deep(
                logfile_name,
                &args,
                vendor_model_lang,
                token_count_factor,
                full_text,
                &api_key,
                events_sqls,
            )
            .await;
        }
    } else {
        eprintln!("Error: {}", response.status());
        eprintln!("{}", response.text().await.unwrap());
    }

    Ok(())
}

#[tokio::main]
pub async fn openrouter(
    logfile_name: &str,
    vendor_model_lang: Vec<&str>,
    token_count_factor: usize,
    events_sqls: HashMap<&str, HashSet<String>>,
    args: &crate::Args,
    report_for_ai: &str,
) -> Result<(), Box<dyn std::error::Error>> {

    println!("=== Consulting OpenRouter model: {} ===", vendor_model_lang[1]);

    let api_key = env::var("OPENROUTER_API_KEY")
        .expect("You have to set OPENROUTER_API_KEY env variable");

    let stem = logfile_name.split('.').next().unwrap();
    let json_path = format!("{stem}.html_reports/stats/global_statistics.json");
    let load_profile = fs::read_to_string(&json_path)
        .expect(&format!("Can't open file {}", json_path));

    let model_name = vendor_model_lang[1].replace("/", "_");
    let response_file = format!("{}_{}.md", logfile_name, model_name);
    let client = Client::new();

    let mut spell = format!("{} {}", SPELL, vendor_model_lang[2]);
    if let Some(pr) = private_reasonings() {
        spell = format!("{spell}\n#ADVANCED RULES\n{pr}");
    }
    if !args.url_context_file.is_empty() {
        if let Some(urls) = url_context(&args.url_context_file, events_sqls.clone()) {
            spell = format!("{spell}\n# URL CONTEXT\n{urls}");
        }
    }

    let payload = json!({
        "model": vendor_model_lang[1],
        "messages": [
            { "role": "system", "content": format!("### SYSTEM INSTRUCTIONS\n{spell}") },
            { "role": "user", "content": format!(
                "MAIN REPORT (toon/json-as-text):\n```\n{}\n```\n\nGLOBAL PROFILE:\n```json\n{}\n```",
                report_for_ai, load_profile
            )}
        ],
        "reasoning": { "effort": "high" },
        "stream": false
    });

    let (tx, rx) = oneshot::channel();
    let spinner = tokio::spawn(spinning_beer(rx));

    let resp = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .header("X-Title", "jas-min")
        .json(&payload)
        .send()
        .await?;

    if resp.status().is_success() {
        println!("Waiting for OpenRouter to stream full response - please wait...");
        let body = resp.text().await?;

        let _ = tx.send(());
        let _ = spinner.await;

        let json: Value = serde_json::from_str(&body)?;
        let content = json["choices"][0]["message"]["content"]
            .as_str().unwrap_or("")
            .to_string();

        fs::write(&response_file, content.as_bytes())?;
        println!("🍻 OpenRouter response written to file: {}", &response_file);

        convert_md_to_html_file(&response_file, events_sqls.clone());

        println!(
            "Total tokens: {}\nFinish reason: {}\n",
            json["usage"]["total_tokens"],
            json["choices"][0]["finish_reason"]
        );
    } else {
        eprintln!("Error: {}", resp.status());
        eprintln!("{}", resp.text().await.unwrap_or_default());
    }

    Ok(())
}

#[tokio::main]
pub async fn openai_gpt(logfile_name: &str, vendor_model_lang: Vec<&str>, token_count_factor: usize, events_sqls: HashMap<&str, HashSet<String>>, args: &crate::Args, report_for_ai: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}{}{}", "=== Consulting OpenAI model: ".bright_cyan(), vendor_model_lang[1], " ===".bright_cyan());

    let api_key = env::var("OPENAI_API_KEY")
        .expect("You have to set OPENAI_API_KEY env variable");

    let log_content = fs::read_to_string(logfile_name)
        .expect(&format!("Can't open file {}", logfile_name));

    let stem = logfile_name.split('.').collect::<Vec<&str>>()[0];
    let path = format!("{stem}.html_reports/stats/global_statistics.json");
    let load_profile = fs::read_to_string(&path).expect(&format!("Can't open file {}", path));

    let response_file = format!("{}_{}.md", logfile_name, vendor_model_lang[1]);

    let mut spell: String = format!("{} {}", SPELL, vendor_model_lang[2]);
    let rag_context = private_reasonings();

    if !args.url_context_file.is_empty() {
        if let Some(urls) = url_context(&args.url_context_file, events_sqls.clone()) {
            spell = format!("{spell}\n{urls}");
        }
    }
    
    let mut input_messages = vec![
        json!({
            "role": "system",
            "content": [
                { "type": "input_text", "text": spell }
            ]
        }),
    ];

    if let Some(rag) = rag_context {
        input_messages.push(json!({
            "role": "user",
            "content": [
                { "type": "input_text", "text":
                    format!("### RAG CONTEXT\n{rag}\n-- END RAG CONTEXT --")
                }
            ]
        }));
    }
    
    let mut log_and_images = vec![
        json!({"type":"input_text", "text":
            format!("### ATTACHED REPORT\n{report_for_ai}\n-- END ATTACHED REPORT --")
        }),
    ];

    log_and_images.push(json!({
        "type": "input_text",
        "text": format!("### LOAD PROFILE STATISTICS JSON\n{}\n-- END JSON --",load_profile
        )
    }));
    
    input_messages.push(json!({
        "role": "user",
        "content": log_and_images
    }));

    let payload = json!({
        "model": vendor_model_lang[1],
        "input": input_messages,
    });

    let payload_str = serde_json::to_string(&payload).unwrap();
    println!("The whole estimated number of tokens is: {}", estimate_tokens_from_str(&payload_str));

    let client = Client::new();

    let (tx, rx) = oneshot::channel();
    let spinner = tokio::spawn(spinning_beer(rx));

    let response = client
        .post(format!("{}v1/responses", get_openai_url()))
        .bearer_auth(api_key)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    let _ = tx.send(());
    let _ = spinner.await;

    if response.status().is_success() {
        let json: Value = response.json().await?;

        let full_text = if let Some(s) = json.get("output_text").and_then(|v| v.as_str()) {
            s.to_string()
        } else {
            let mut seen = std::collections::HashSet::<String>::new();
            let mut chunks: Vec<String> = vec![];
            if let Some(output_arr) = json.get("output").and_then(|o| o.as_array()) {
                for item in output_arr {
                    if let Some(content_arr) = item.get("content").and_then(|c| c.as_array()) {
                        for c in content_arr {
                            if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
                                if seen.insert(t.to_string()) {
                                    chunks.push(t.to_string());
                                }
                            } else if let Some(t) = c.get("output_text").and_then(|t| t.as_str()) {
                                if seen.insert(t.to_string()) {
                                    chunks.push(t.to_string());
                                }
                            }
                        }
                    }
                }
            }
            chunks.join("\n")
        };

        fs::write(&response_file, full_text.as_bytes())?;
        println!("🧠 OpenAI response written to file: {}", &response_file);

        convert_md_to_html_file(&response_file, events_sqls);

        if let Some(usage) = json.get("usage") {
            println!("Total tokens (OpenAI): {}", usage);
        }
        if let Some(finish) = json.pointer("/output/0/finish_reason").or_else(|| json.get("finish_reason")) {
            println!("Finish reason: {}", finish);
        }
    } else {
        eprintln!("Error: {}", response.status());
        eprintln!("{}", response.text().await.unwrap_or_default());
    }

    Ok(())
}

// ###########################
// JASMIN Assistant Backend
// ###########################
#[derive(Deserialize)]
struct UserMessage {
    message: String,
}

#[derive(Serialize)]
struct AIResponse {
    reply: String,
}

#[derive(Clone, Debug)]
pub enum BackendType {
    OpenAI,
    Gemini,
}

#[async_trait::async_trait]
trait AIBackend: Send + Sync {
    async fn initialize(&mut self, toon_str: String) -> anyhow::Result<()>;
    async fn send_message(&self, message: &str) -> anyhow::Result<String>;
}

struct OpenAIBackend {
    client: reqwest::Client,
    api_key: String,
    assistant_id: String,
    thread_id: Option<String>,
}

impl OpenAIBackend {
    fn new(api_key: String, assistant_id: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            assistant_id,
            thread_id: None,
        }
    }

    async fn create_thread_with_file(&self, toon_str: String) -> anyhow::Result<String> {
        let file_bytes = toon_str.into_bytes();
        let file_name = "jasmin_report.txt";

        let file_part = Part::bytes(file_bytes)
            .file_name(file_name)
            .mime_str("text/plain")?;
        let form = Form::new()
            .part("file", file_part)
            .text("purpose", "assistants");
        
        let upload_res = self.client
            .post(format!("{}v1/files", get_openai_url()))
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;
        
        let file_id = upload_res.get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Failed to upload file: {:?}", upload_res))?;
        
        println!("✅ File uploaded with ID: {}", file_id);

        let thread_res = self.client
            .post(format!("{}v1/threads", get_openai_url()))
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .json(&serde_json::json!({
                "messages": [{
                    "role": "user",
                    "content": "Uploading file with performance report"
                }]
            }))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;
        
        let thread_id = thread_res.get("id")
            .and_then(|id| id.as_str())
            .ok_or_else(|| anyhow::anyhow!("Failed to create thread: {:?}", thread_res))?;
        
        println!("✅ Thread created: {}", thread_id);

        let message_url = format!("{}v1/threads/{}/messages", get_openai_url(), thread_id);
        let file_msg_res = self.client
            .post(&message_url)
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .json(&serde_json::json!({
                "role": "user",
                "content": "Please analyze the attached performance report.",
                "attachments": [{
                    "file_id": file_id,
                    "tools": [{ "type": "file_search" }]
                }]
            }))
            .send()
            .await?;

        if !file_msg_res.status().is_success() {
            let status = file_msg_res.status();
            let err_text = file_msg_res.text().await?;
            eprintln!("Attach file failed. Status: {}, Response body: {}", status, err_text);
            return Err(anyhow::anyhow!("Failed to attach file to thread."));
        }

        println!("📎 File attached to thread.");

        println!("⏳ Waiting for file processing to complete...");
        self.wait_for_file_processing(&thread_id).await?;
        
        println!("✅ File processing completed! Thread is ready for use.");
        Ok(thread_id.to_string())
    }

    async fn wait_for_file_processing(&self, thread_id: &str) -> anyhow::Result<()> {
        let mut attempts = 0;
        let max_attempts = 30;
        
        loop {
            let thread_url = format!("{}v1/threads/{}", get_openai_url(), thread_id);
            let thread_res = self.client
                .get(&thread_url)
                .bearer_auth(&self.api_key)
                .header("OpenAI-Beta", "assistants=v2")
                .send()
                .await?;
                
            if !thread_res.status().is_success() {
                return Err(anyhow::anyhow!("Failed to get thread details"));
            }
            
            let thread_data = thread_res.json::<serde_json::Value>().await?;
            
            if let Some(tool_resources) = thread_data.get("tool_resources") {
                if let Some(file_search) = tool_resources.get("file_search") {
                    if let Some(vector_store_ids) = file_search.get("vector_store_ids") {
                        if let Some(vector_stores) = vector_store_ids.as_array() {
                            if let Some(vs_id) = vector_stores.first().and_then(|v| v.as_str()) {
                                match self.check_vector_store_status(vs_id).await? {
                                    status if status == "completed" => {
                                        println!("✅ Vector store processing completed!");
                                        return Ok(());
                                    }
                                    status if status == "failed" => {
                                        return Err(anyhow::anyhow!("Vector store processing failed"));
                                    }
                                    status => {
                                        println!("📊 Vector store status: {} (attempt {}/{})", status, attempts + 1, max_attempts);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            
            if attempts >= max_attempts {
                return Err(anyhow::anyhow!("File processing timeout after {} attempts", max_attempts));
            }
            
            sleep(Duration::from_secs(5)).await;
            attempts += 1;
        }
    }

    async fn check_vector_store_status(&self, vector_store_id: &str) -> anyhow::Result<String> {
        let url = format!("{}v1/vector_stores/{}", get_openai_url(), vector_store_id);
        
        let res = self.client
            .get(&url)
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .send()
            .await?;
        
        if !res.status().is_success() {
            return Err(anyhow::anyhow!("Failed to check vector store status"));
        }
        
        let json_res = res.json::<serde_json::Value>().await?;
        
        let status = json_res["status"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing status in vector store response"))?;
        
        Ok(status.to_string())
    }

    async fn create_message(&self, thread_id: &str, content: &str) -> anyhow::Result<()> {
        let url = format!("{}v1/threads/{}/messages", get_openai_url(), thread_id);

        let mut body = HashMap::new();
        body.insert("role", "user");
        body.insert("content", content);

        self.client.post(&url)
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .json(&body)
            .send().await?
            .error_for_status()?;

        Ok(())
    }

    async fn run_assistant(&self, thread_id: &str) -> anyhow::Result<String> {
        let url = format!("{}v1/threads/{}/runs", get_openai_url(), thread_id);

        let mut body = HashMap::new();
        body.insert("assistant_id", &self.assistant_id);

        let res = self.client.post(&url)
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .json(&body)
            .send().await?;
            
        if !res.status().is_success() {
            let status = res.status();
            let error_text = res.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow::anyhow!("API request failed with status {}: {}", status, error_text));
        }
        
        let json_res = res.json::<serde_json::Value>().await?;
        let run_id = json_res["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing or invalid 'id' field in response: {}", json_res))?
            .to_string();

        Ok(run_id)
    }

    async fn wait_for_completion(&self, thread_id: &str, run_id: &str) -> anyhow::Result<()> {
        loop {
            let url = format!("{}v1/threads/{}/runs/{}", get_openai_url(), thread_id, run_id);
            let res = self.client
                .get(&url)
                .bearer_auth(&self.api_key)
                .header("OpenAI-Beta", "assistants=v2")
                .send().await?
                .json::<serde_json::Value>().await?;

            let status = res.get("status").and_then(|s| s.as_str());

            match status {
                Some("completed") => return Ok(()),
                Some("failed") | Some("cancelled") | Some("expired") => {
                    return Err(anyhow::anyhow!("Run failed or was cancelled/expired:\n {:?}", res))
                },
                Some(_) => {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                },
                None => {
                    return Err(anyhow::anyhow!("Missing 'status' field in response:\n {:?}", res));
                }
            }
        }
    }

    async fn get_reply(&self, thread_id: &str) -> anyhow::Result<String> {
        let url = format!("{}v1/threads/{}/messages", get_openai_url(), thread_id);

        let res = self.client
            .get(&url)
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        if let Some(reply) = res["data"][0]["content"][0]["text"]["value"].as_str() {
            Ok(reply.to_string())
        } else {
            Err(anyhow::anyhow!("Failed to extract assistant reply: {:?}", res))
        }
    }
}

#[async_trait::async_trait]
impl AIBackend for OpenAIBackend {
    async fn initialize(&mut self, file_path: String) -> anyhow::Result<()> {
        let thread_id = self.create_thread_with_file(file_path).await?;
        self.thread_id = Some(thread_id);
        Ok(())
    }

    async fn send_message(&self, message: &str) -> anyhow::Result<String> {
        let thread_id = self.thread_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Thread not initialized"))?;

        self.create_message(thread_id, message).await?;
        let run_id = self.run_assistant(thread_id).await?;
        self.wait_for_completion(thread_id, &run_id).await?;
        self.get_reply(thread_id).await
    }
}

struct GeminiBackend {
    client: reqwest::Client,
    api_key: String,
    model: String,
    conversation_history: Vec<GeminiMessage>,
    file_content: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct GeminiMessage {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum GeminiPart {
    Text { text: String },
    InlineData { inline_data: InlineData },
}

#[derive(Clone, Serialize, Deserialize)]
struct InlineData {
    mime_type: String,
    data: String,
}

impl GeminiBackend {
    fn new(api_key: String, gemini_model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model: gemini_model, 
            conversation_history: Vec::new(),
            file_content: None,
        }
    }

    async fn send_to_gemini(&self, messages: &[GeminiMessage]) -> anyhow::Result<String> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );

        let body = serde_json::json!({
            "contents": messages,
            "generationConfig": {
                "temperature": 0.7,
                "maxOutputTokens": 8192,
            }
        });

        let res = self.client
            .post(&url)
            .json(&body)
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status();
            let error_text = res.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow::anyhow!("Gemini API request failed with status {}: {}", status, error_text));
        }

        let json_res = res.json::<serde_json::Value>().await?;
        
        if let Some(candidates) = json_res["candidates"].as_array() {
            if let Some(first_candidate) = candidates.first() {
                if let Some(content) = first_candidate["content"]["parts"][0]["text"].as_str() {
                    return Ok(content.to_string());
                }
            }
        }

        Err(anyhow::anyhow!("Failed to extract response from Gemini: {:?}", json_res))
    }
}

#[async_trait::async_trait]
impl AIBackend for GeminiBackend {
    async fn initialize(&mut self, toon_str: String) -> anyhow::Result<()> {
        self.file_content = Some(toon_str.clone());
        
        let mut spell: String = format!("{}", SPELL);
        let pr = private_reasonings();
        if pr.is_some() {
            spell = format!("{}\n# ADVANCED RULES: {}", spell, pr.unwrap());
        }

        let initial_message = GeminiMessage {
            role: "user".to_string(),
            parts: vec![
                GeminiPart::Text {
                    text: format!(
                        "# INITIALIZATION\n\n\
                         I am providing you with an Oracle Database performance audit report generated \
                         by JAS-MIN. This report contains aggregated AWR/STATSPACK statistics including \
                         wait events, SQL analysis, I/O metrics, segment statistics, anomaly detection, \
                         and gradient-based regression analysis.\n\n\
                         ## Your Role\n\
                         {}\n\n\
                         ## Instructions\n\
                         1. Ingest and understand the complete report below\n\
                         2. Be prepared to answer detailed questions about any aspect of the data\n\
                         3. When answering questions, always reference specific values, SQL_IDs, \
                            event names, and snap_ids from the report\n\
                         4. Detect the language of each question and respond in that same language\n\n\
                         ## Report Content\n\
                         ```\n{}\n```",
                        spell, toon_str
                    )
                }
            ],
        };
        
        self.conversation_history.push(initial_message.clone());
        
        let response = self.send_to_gemini(&self.conversation_history).await?;
        
        self.conversation_history.push(GeminiMessage {
            role: "model".to_string(),
            parts: vec![GeminiPart::Text { text: response.clone() }],
        });
        
        println!("✅ Gemini initialized with file content");
        Ok(())
    }

    async fn send_message(&self, message: &str) -> anyhow::Result<String> {
        let mut messages = self.conversation_history.clone();
        
        messages.push(GeminiMessage {
            role: "user".to_string(),
            parts: vec![GeminiPart::Text { text: message.to_string() }],
        });
        
        let response = self.send_to_gemini(&messages).await?;
        
        Ok(response)
    }
}

pub struct AppState {
    backend: Arc<Mutex<Box<dyn AIBackend>>>,
}

#[tokio::main]
pub async fn backend_ai(reportfile: String, backend_type: BackendType, model_name: String, toon_str: String) -> anyhow::Result<()> {    
    let backend: Box<dyn AIBackend> = match backend_type {
        BackendType::OpenAI => {
            let api_key = env::var("OPENAI_API_KEY")
                .expect("You have to set OPENAI_API_KEY variable in .env");
            let assistant_id = env::var("OPENAI_ASST_ID")
                .expect("You have to set OPENAI_ASST_ID variable in .env");
            Box::new(OpenAIBackend::new(api_key, assistant_id))
        },
        BackendType::Gemini => {
            let api_key = env::var("GEMINI_API_KEY")
                .expect("You have to set GEMINI_API_KEY variable in .env");
            
            Box::new(GeminiBackend::new(api_key, model_name))
        },
    };
    
    let backend_port = env::var("PORT").unwrap_or("3000".to_string());
    
    let mut backend_mut = backend;
    if let Err(e) = backend_mut.initialize(toon_str).await {
        eprintln!("❌ Backend initialization failed: {:?}", e);
        return Err(e);
    }
    
    let state = Arc::new(AppState {
        backend: Arc::new(Mutex::new(backend_mut)),
    });

    let app = Router::new()
        .route("/api/chat", post(chat_handler))
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", backend_port)).await?;
    println!("🚀 Server running on http://127.0.0.1:{}", backend_port);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn chat_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UserMessage>,
) -> impl IntoResponse {
    let backend = state.backend.lock().await;
    
    match backend.send_message(&payload.message).await {
        Ok(reply) => (StatusCode::OK, Json(AIResponse { reply })).into_response(),
        Err(err) => {
            eprintln!("Error processing message: {:?}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to process message").into_response()
        }
    }
}

pub fn parse_backend_type(args: &str) -> Result<BackendType, String> {
    let mut btype = args; 
    if args.contains(":") {
        btype = args.split(":").collect::<Vec<&str>>()[0];
    }
    match btype {
        "openai" => Ok(BackendType::OpenAI),
        "google" => Ok(BackendType::Gemini),
        _ => Err(format!("Backend must be 'openai' or 'google' -> found: {}",args)),
    }
}