use crate::ai_tools::*;
use crate::awr::{
    AWRSCollection, HostCPU, IOStats, LoadProfile, SQLCPUTime, SQLGets, SQLIOTime, SQLReads,
    SegmentStats, WaitEvents, AWR,
};
use crate::{debug_note, tools::*};
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use base64::{engine::general_purpose, Engine as _};
use colored::Colorize;
use reqwest::multipart::{Form, Part};
use reqwest::{multipart, Client};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;
use std::borrow::Cow;
use std::env::Args;
use std::error::Error;
use std::fmt::format;
use std::io::{stdout, Write};
use std::str::FromStr;
use std::{collections::HashMap, collections::HashSet, env, fs, path::Path, sync::Arc};
use tokio::sync::oneshot;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tower_http::cors::{Any, CorsLayer};

fn get_openai_url() -> String {
    env::var("OPENAI_URL").unwrap_or_else(|_| "https://api.openai.com/".to_string())
}

fn stem_from_logfile(logfile_name: &str) -> &str {
    logfile_name.split('.').next().unwrap_or(logfile_name)
}

fn load_profile_for_stem(stem: &str) -> String {
    let json_path = format!("{stem}.html_reports/stats/global_statistics.json");
    fs::read_to_string(&json_path).expect(&format!("Can't open file {}", json_path))
}

fn load_tools_collection(args: &crate::Args) -> AWRSCollection {
    let mut json_file = args.json_file.clone();
    if json_file.is_empty() {
        json_file = format!("{}.json", args.directory);
    }
    let s_json = fs::read_to_string(&json_file).expect(&format!("Can't read {}", json_file));
    serde_json::from_str(&s_json).expect("Wrong AWRSCollection JSON")
}

fn build_model_instructions(
    lang: &str,
    args: &crate::Args,
    events_sqls: &HashMap<&str, HashSet<String>>,
    stem: &str,
    tools_mode: bool,
) -> String {
    let mut spell = format!("{} {}", SPELL, lang);

    if let Some(pr) = private_reasonings() {
        spell = format!("{spell}\n#ADVANCED RULES\n{pr}");
    }

    if !args.url_context_file.is_empty() {
        if let Some(urls) = url_context(&args.url_context_file, events_sqls.clone()) {
            spell = format!("{spell}\n# URL CONTEXT\n{urls}");
        }
    }

    if tools_mode {
        spell.push_str(&tools_mode_instructions(stem));
    }

    spell
}

fn tools_mode_instructions(stem: &str) -> String {
    let xplan_dir = format!("{stem}_attachments");
    let xplan_note = if Path::new(&xplan_dir).is_dir() {
        format!(
            "\nAvailable execution-plan attachment directory: `{}`. \
             If list_available_sql_plans is present, use it to discover SQL_IDs with plans.",
            xplan_dir
        )
    } else {
        String::new()
    };

    format!(
        "\n\n# TOOLS MODE\n\
         You have access to diagnostic tools that fetch detailed AWR/STATSPACK data on demand. \
         Use tools proactively. Do not rely only on the initial summary when a precise tool call can verify or falsify a hypothesis. \
         Start with get_database_load_summary unless the user request is already very narrow. \
         For suspicious snapshots, call list_snapshots, compare_snapshots, top_wait_events_in_snapshot, top_sqls_in_snapshot, get_metric_time_series, get_sql_timeline, or get_wait_event_timeline as needed. \
         For every SQL_ID that materially contributes to DB Time, elapsed time, DB CPU, I/O time, buffer gets, physical reads, anomalous waits, or regression symptoms, call get_sql_text and get_sql_timeline. \
         If execution-plan tools are available, you are expected to use list_available_sql_plans and get_sql_execution_plan for important SQL_IDs before making SQL tuning recommendations. \
         When you fetch an execution plan, produce a dedicated SQL execution plan analysis covering: dominant operations, access paths, join methods and join order, cardinality estimate errors, partition pruning, parallel execution, adaptive plan notes, temp spills/sorts, index usage, and concrete remediation options. \
         Recommendations must be specific and evidence-based: statistics refresh, histograms, extended statistics, SQL rewrite, indexing, partitioning, SQL Plan Management baseline/profile, bind/literal handling, or application-side change. \
         Prefer multiple narrow tool calls over guessing. Stop calling tools only when you have enough evidence to produce the FINAL markdown report following the OUTPUT STRUCTURE.{}",
        xplan_note
    )
}

fn final_synthesis_request() -> &'static str {
    r#"
You have reached the maximum number of allowed tool iterations.

You must now write the final Oracle performance analysis report in Markdown.

Rules:
- Do not request or call any more tools.
- Use the original ReportForAI / AWR / Statspack data already provided.
- Use all tool results already returned in this conversation.
- If some SQL texts or execution plans were not inspected, do not invent their details.
- Focus on evidence-backed findings, impact, root causes, and concrete recommendations.
- Do not mention that the tool budget was exhausted.
- Produce the final Markdown report now.
"#
}

fn estimate_tokens_from_value(value: &Value) -> usize {
    let payload_str = serde_json::to_string(value).unwrap_or_default();
    estimate_tokens_from_str(&payload_str)
}

fn openrouter_bad_response_path(response_file: &str, context: &str) -> String {
    let safe_context = context.replace(' ', "_");
    format!("{response_file}.{safe_context}.bad_response.json")
}

fn parse_openrouter_response_json(
    body: &str,
    response_file: &str,
    context: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    match serde_json::from_str(body) {
        Ok(json) => Ok(json),
        Err(e) => {
            let debug_path = openrouter_bad_response_path(response_file, context);
            let _ = fs::write(&debug_path, body.as_bytes());
            Err(format!(
                "OpenRouter returned malformed JSON during {context}: {e}. \
                 Raw response saved to {debug_path} ({} chars).",
                body.chars().count()
            )
            .into())
        }
    }
}

fn openrouter_payload_tokens(model: &str, messages: &[Value], tools: Option<&Value>) -> usize {
    let mut payload = json!({
        "model": model,
        "messages": messages,
        "reasoning": { "effort": "high" },
        "stream": false
    });

    if let Some(tools) = tools {
        payload["tools"] = tools.clone();
        payload["tool_choice"] = json!("auto");
    }

    estimate_tokens_from_value(&payload)
}

fn gemini_payload_tokens(spell: &str, contents: &[Value], tools: Option<&Value>) -> usize {
    let mut payload = json!({
        "systemInstruction": {
            "parts": [{ "text": format!("### SYSTEM INSTRUCTIONS\n{spell}") }]
        },
        "contents": contents,
        "generationConfig": {
            "thinkingConfig": {
                "thinkingBudget": -1
            }
        }
    });

    if let Some(tools) = tools {
        payload["tools"] = tools.clone();
        payload["toolConfig"] = json!({
            "functionCallingConfig": { "mode": "AUTO" }
        });
    }

    estimate_tokens_from_value(&payload)
}

fn openai_responses_payload_tokens(
    model: &str,
    input_messages: &[Value],
    tools: Option<&Value>,
) -> usize {
    let mut payload = json!({
        "model": model,
        "input": input_messages,
    });

    if let Some(tools) = tools {
        payload["tools"] = tools.clone();
        payload["tool_choice"] = json!("auto");
    }

    estimate_tokens_from_value(&payload)
}

fn compact_openrouter_tool_results_for_budget(
    model: &str,
    messages: &mut Vec<Value>,
    budget_tokens: usize,
) -> usize {
    let mut compacted = 0;

    while openrouter_payload_tokens(model, messages, None) > budget_tokens {
        let largest_tool_message = messages
            .iter()
            .enumerate()
            .filter_map(|(idx, msg)| {
                if msg.get("role").and_then(|v| v.as_str()) != Some("tool") {
                    return None;
                }

                let len = msg
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(|s| s.chars().count())
                    .unwrap_or(0);

                Some((idx, len))
            })
            .max_by_key(|(_, len)| *len);

        let Some((idx, len)) = largest_tool_message else {
            break;
        };

        if len <= 2048 {
            break;
        }

        let original = messages[idx]
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let prefix: String = original.chars().take(2048).collect();
        messages[idx]["content"] = json!(format!(
            "{}\n\n[Tool result truncated by JAS-MIN token budget guard. Original length: {} chars.]",
            prefix, len
        ));
        compacted += 1;
    }

    compacted
}

fn compact_gemini_tool_results_for_budget(
    spell: &str,
    contents: &mut Vec<Value>,
    budget_tokens: usize,
) -> usize {
    let mut compacted = 0;

    while gemini_payload_tokens(spell, contents, None) > budget_tokens {
        let largest_tool_response = contents
            .iter()
            .enumerate()
            .filter_map(|(idx, content)| {
                let response = content.pointer("/parts/0/functionResponse/response")?;
                let len = serde_json::to_string(response)
                    .map(|s| s.chars().count())
                    .unwrap_or(0);
                Some((idx, len))
            })
            .max_by_key(|(_, len)| *len);

        let Some((idx, len)) = largest_tool_response else {
            break;
        };

        if len <= 2048 {
            break;
        }

        let original = contents[idx]
            .pointer("/parts/0/functionResponse/response")
            .and_then(|v| serde_json::to_string(v).ok())
            .unwrap_or_default();
        let prefix: String = original.chars().take(2048).collect();

        if let Some(response) = contents[idx].pointer_mut("/parts/0/functionResponse/response") {
            *response = json!({
                "truncated_by_jasmin_token_budget": true,
                "original_length_chars": len,
                "prefix": prefix
            });
            compacted += 1;
        } else {
            break;
        }
    }

    compacted
}

fn compact_openai_tool_results_for_budget(
    model: &str,
    input_messages: &mut Vec<Value>,
    budget_tokens: usize,
) -> usize {
    let mut compacted = 0;

    while openai_responses_payload_tokens(model, input_messages, None) > budget_tokens {
        let largest_tool_output = input_messages
            .iter()
            .enumerate()
            .filter_map(|(idx, item)| {
                if item.get("type").and_then(|v| v.as_str()) != Some("function_call_output") {
                    return None;
                }

                let len = item
                    .get("output")
                    .and_then(|v| v.as_str())
                    .map(|s| s.chars().count())
                    .unwrap_or(0);

                Some((idx, len))
            })
            .max_by_key(|(_, len)| *len);

        let Some((idx, len)) = largest_tool_output else {
            break;
        };

        if len <= 2048 {
            break;
        }

        let original = input_messages[idx]
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let prefix: String = original.chars().take(2048).collect();
        input_messages[idx]["output"] = json!(format!(
            "{}\n\n[Tool result truncated by JAS-MIN token budget guard. Original length: {} chars.]",
            prefix, len
        ));
        compacted += 1;
    }

    compacted
}

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
    pub ridge_lambda: f64,
    pub elastic_net_lambda: f64,
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
    pub impact_active: f64, // P90-based — primary tuning metric
    pub impact_peak: f64,   // P99-based — worst-case
    pub impact_share: f64,  // % of total active impact
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct DbTimeGradientSection {
    pub settings: GradientSettings,
    pub ridge_top: Vec<GradientTopItem>,
    pub elastic_net_top: Vec<GradientTopItem>,
    pub huber_top: Vec<GradientTopItem>,
    pub quantile95_top: Vec<GradientTopItem>,
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

You receive a **ReportForAI** object (TOON format) containing preprocessed, aggregated 
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
- `instance_stats_pearson_correlation` — instance statistics correlated with DB Time (abs(rho) >= 0.5)
- `load_profile_anomalies` — MAD-detected load profile anomalies
- `anomaly_clusters` — temporally grouped anomalies across multiple domains
- `initialization_parameters` — Oracle instance initialization parameters (name-value pairs). 
  Contains both explicit (user-set) and default parameter values from the analyzed instance.

## Gradient Analysis Sections (Optional)

### DB Time Gradient Sections
Sections `db_time_gradient_fg_wait_events`, `db_time_gradient_instance_stats_[counters,volumes,time]`,
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

### Gradient Impact Metrics (GradientTopItem fields)

Each entry in `ridge_top` / `elastic_net_top` / `huber_top` / `quantile95_top` contains:

- `gradient_coef` — standardized regression coefficient.
  Sensitivity: how DB Time responds to a 1-sigma move of the predictor.
  Sign matters: positive = contributes to DB Time, negative = suppressor/confounder.
  Use for understanding mechanics.

- `impact_active` = abs(coef) * P90(abs(delta_x)) — **PRIMARY TUNING METRIC**.
  Contribution to DB Time when the predictor is actively moving (not during idle periods).
  Expressed in DB Time units. **Use this first when ranking bottlenecks.**

- `impact_peak` = abs(coef) * P99(abs(delta_x)) — worst-case single-snapshot contribution.
  How much DB Time this predictor can add during its most aggressive moments.
  Use for capacity planning and identifying spike causes.

- `impact` = abs(coef) * MAD(delta_x) — legacy/typical impact.
  Contribution during *median* variability. Often near zero for bursty events.
  Use only as comparison baseline (see diagnostic rule below).

- `impact_share` = impact_active / sum(impact_active across positive-coef predictors).
  Range [0.0, 1.0]. Use to communicate relative importance, e.g., 
  'this event explains 23% of DB Time variance'.

**Ranking rule:** Entries are sorted by signed_impact_active descending — positive contributors 
(real bottlenecks) first, suppressors last. When reporting top factors, focus on entries with 
gradient_coef > 0.

**Diagnostic rule — bursty vs systematic behavior:**

Compare `impact_active` and `impact` for each predictor:
- `impact_active` much greater than `impact` (e.g., ratio > 5x): **bursty behavior** — 
  predictor is quiet most of the time but causes large DB Time contribution during activity 
  windows. Investigate batch jobs, scheduled tasks, or commit storms.
- `impact_active` approximately equal to `impact`: **systematic behavior** — predictor 
  contributes steadily. Typical of background I/O or constantly-active workloads.
- `impact_active` much less than `impact`: rarely happens; suggests data quality issues.

**Reporting guidelines:**
- When citing a predictor's contribution to DB Time, quote `impact_active` as the main number.
- Use `impact_share` (as a percentage) to express relative importance.
- Quote `impact_peak` when discussing worst-case behavior or capacity risk.
- Mention the bursty/systematic distinction explicitly when `impact_active` and `impact` diverge.
- NEVER use `impact` (MAD-based) as the primary metric — it systematically underestimates 
  rare-but-severe events (log file sync bursts, lock spikes, etc.).

### Cross-Model Classification Rules

Each gradient section includes `cross_model_classifications` with pre-computed triangulation.
Interpret classifications using this priority hierarchy:

- `CONFIRMED_BOTTLENECK` (all 4 models) — systematic, robust bottleneck. **CRITICAL** priority.
- `CONFIRMED_BOTTLENECK_EN_COLLINEAR` (Ridge + Huber + Q95, not EN) — bottleneck masked by L1 
  collinearity. **CRITICAL — find correlated EN factor.**
- `TAIL_OUTLIER` (Ridge + Q95, not Huber) — extreme snapshots that ARE the worst periods. 
  HIGH priority.
- `TAIL_RISK` (Q95 only, not Ridge) — rare catastrophic spikes. HIGH priority — warn about 
  peak periods.
- `STRONG_CONTRIBUTOR` (Ridge + EN + Huber, not Q95) — reliable systematic contributor. 
  MEDIUM priority.
- `OUTLIER_DRIVEN` (Ridge, not Huber) — few extreme snapshots only. MEDIUM priority — 
  check anomaly_clusters.
- `SPARSE_DOMINANT` (EN, not Ridge) — dominant among correlated group. MEDIUM priority.
- `STABLE_CONTRIBUTOR` (Ridge + Huber, not EN, not Q95) — steady background contributor. 
  LOW-MEDIUM priority.
- `ROBUST_ONLY` (Huber only) — background factor without outliers. LOW priority.
- `MULTI_MODEL_MINOR` (2+ models, no pattern) — minor contributor. LOW priority.
- `SINGLE_MODEL` (1 model only) — low confidence. INFORMATIONAL.

The `combined_impact` field in each classification is the sum of `impact_active` across all 
models where the predictor appears — treat it as the aggregate active-impact signal.

The `combined_peak_impact` field in each classification is the sum of `impact_peak` across all 
models where the predictor appears — treat it as the aggregate peak-impact signal.

### VIF Diagnostics & Collinear Groups

Each gradient section may contain:
- `vif_diagnostics` — Variance Inflation Factor for predictors with VIF > 5.
  - VIF > 10: coefficients are unreliable due to multicollinearity; use group impact instead
  - VIF > 100: extreme collinearity; individual impact values are meaningless
- `collinear_group_impacts` — when collinear predictors are detected, their raw signals
  are summed and a single univariate coefficient is computed for the group.
  - `combined_impact` represents the TRUE impact of the collinear group on DB Time
  - This resolves cases where individual impact is near zero despite high correlation

**When VIF diagnostics are present:**
1. Do NOT report individual `impact_active` values for events with VIF > 10 as meaningful
2. Instead, report the collinear group's `combined_impact`
3. Explain to the reader that individual regression coefficients cannot separate
   the effects of highly correlated events
4. Example: if TX row lock and TM contention have VIF > 800, their individual
   `impact_active` near zero is an artifact — the group impact reveals their true contribution

**Gradient analysis strategy:**
1. Start with CONFIRMED_BOTTLENECK and CONFIRMED_BOTTLENECK_EN_COLLINEAR — highest priority
2. Rank within each classification by `impact_active` (not `impact`)
3. Flag TAIL_RISK and TAIL_OUTLIER items as hidden dangers; quote their `impact_peak` values
4. Cross-reference OUTLIER_DRIVEN with anomaly_clusters for root cause
5. Use SPARSE_DOMINANT to find representative factors in correlated groups
6. For each top predictor, report: `impact_active`, `impact_share` (%), and the 
   bursty-vs-systematic diagnosis derived from `impact_active` vs `impact`
7. Integrate with traditional AWR analysis — gradients explain *why* DB Time changes
8. For DB CPU gradients: cross-reference `db_cpu_gradient_sql_cpu_time` with 
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
- Wait events are symptoms -> trace to SQLs -> segments -> application behavior
- Use correlation data to build causal chains
- **When `tables_associated_with_event_based_on_ash_sql` is present for a wait event, 
  treat it as direct evidence linking the event to specific tables. Cross-reference 
  these tables with segment statistics (logical reads, physical reads, row lock waits, 
  buffer busy waits, etc.) for deeper insight. This SQL-parsed data supplements but 
  does NOT replace statistical reasoning — always validate with segment stats and 
  correlation data.**
- Cross-validate with gradient analysis when available — use `impact_active` to quantify 
  contributions and `impact_share` to express relative importance

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
- Rank findings by business impact (DB Time contribution x frequency)
- When gradient data is available, use `impact_active` x model-confidence (classification tier) 
  as the quantitative ranking signal
- Separate systematic issues from incidents (use the bursty-vs-systematic diagnostic)
- Assign ownership (DBA vs Developer)

# OUTPUT RULES

- **Format**: Markdown with clear sections and subsections, using icons/symbols
- **Precision**: Quote exact values, SQL_IDs, event names, segment names from the data. 
  Never fabricate data. Format wait events and SQL_IDs as inline code.
- **Number formatting**: For very large impact values (>= 1e6), use human-readable suffixes 
  (K/M/G/T) with 2-3 significant digits, e.g., '15.4T' instead of '15441773967434.10'. 
  For `impact_share`, always format as percentage with 1 decimal place (e.g., '23.4%').
- **Temporal**: Always pair SNAP_ID with SNAP_DATE
- **Cross-referencing**: Connect findings across sections
- **MOS Notes**: Include relevant Oracle MOS note IDs when applicable
- **Parameter names**: Format initialization parameter names as inline code 
  (e.g., `optimizer_index_cost_adj`, `_fix_control`)
- **Markdown tables**: When you need to include a literal pipe character inside a table cell, 
  escape it as backslash-pipe so it does not break the table structure.

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
- For each top predictor, report `impact_active`, `impact_share` (%), and the 
  bursty/systematic diagnosis
- Quote `impact_peak` when discussing spike periods or capacity concerns
- For SQL analysis: include a cross-gradient comparison table showing SQL_IDs that appear 
  in db_time_gradient_sql_elapsed_time and/or db_cpu_gradient_sql_cpu_time, with columns:
  SQL_ID, DB Time Classification, DB CPU Classification, Active Impact, Share %, Diagnosis.
  Where Diagnosis is one of: CPU-Dominant, Wait-Dominant, Mixed, or CPU-Only.
## 10. ⚙️ Initialization Parameter Analysis
### 10.1 Parameters Related to Identified Performance Issues
For each finding from sections 2-9 where an initialization parameter is relevant:
- Current value, recommended value, justification, and reference source.
### 10.2 General Parameter Risks & Anti-Patterns
Parameters with risky, deprecated, or suboptimal values independent of current symptoms.
### 10.3 Parameter Change Summary Table
Columns: Parameter, Current Value, Recommended Value, Risk Level, Related Finding, Source.
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

Language style has to be precise, descriptive and professional. 
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

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct UrlContext {
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
    let url_context_data: HashMap<String, Vec<UrlContext>> =
        serde_json::from_str(&r_content.unwrap()).expect("Wrong url file JSON format");
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

async fn upload_file_to_gemini_from_path(
    api_key: &str,
    path: &str,
    file_type: &str,
    file_name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let file_bytes = fs::read(path)?;

    let part = multipart::Part::bytes(file_bytes)
        .file_name(Cow::Owned(file_name.to_string()))
        .mime_str(file_type)?;

    let form = multipart::Form::new().part("file", part);

    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "https://generativelanguage.googleapis.com/upload/v1beta/files?key={}",
            api_key
        ))
        .multipart(form)
        .send()
        .await?;

    if response.status().is_success() {
        let response_text = response.text().await?;
        match serde_json::from_str::<GeminiFileUploadResponse>(&response_text) {
            Ok(file_upload_response) => {
                println!(
                    "✅ {} uploaded! URI: {}",
                    path, file_upload_response.file.uri
                );
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

async fn upload_log_file_gemini(
    api_key: &str,
    log_content: String,
    file_name: String,
) -> Result<String, Box<dyn std::error::Error>> {
    let part = multipart::Part::bytes(log_content.into_bytes())
        .file_name(file_name)
        .mime_str("text/plain")
        .unwrap();

    let form = multipart::Form::new().part("file", part);

    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "https://generativelanguage.googleapis.com/upload/v1beta/files?key={}",
            api_key
        ))
        .multipart(form)
        .send()
        .await
        .unwrap();

    if response.status().is_success() {
        let response_text = response.text().await?;

        match serde_json::from_str::<GeminiFileUploadResponse>(&response_text) {
            Ok(file_upload_response) => {
                println!("✅ File uploaded! URI: {}", file_upload_response.file.uri);
                Ok(file_upload_response.file.uri)
            }
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

fn extract_gemini_text(json: &Value) -> String {
    let Some(parts) = json
        .pointer("/candidates/0/content/parts")
        .and_then(|v| v.as_array())
    else {
        return String::new();
    };

    let mut seen = HashSet::new();
    parts
        .iter()
        .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
        .filter(|t| seen.insert(t.to_string()))
        .collect::<Vec<&str>>()
        .join("\n")
}

fn extract_gemini_function_calls(json: &Value) -> Vec<Value> {
    json.pointer("/candidates/0/content/parts")
        .and_then(|v| v.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("functionCall").cloned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::main]
pub async fn gemini(
    logfile_name: &str,
    vendor_model_lang: Vec<&str>,
    events_sqls: HashMap<&str, HashSet<String>>,
    args: &crate::Args,
    report_for_ai: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let tools_mode = args.tools_mode;
    let mode_label = if tools_mode { "TOOLS" } else { "single-shot" };
    println!(
        "{}{}{}{}{}",
        "=== Consulting Google Gemini (".bright_cyan(),
        mode_label,
        ") model: ".bright_cyan(),
        vendor_model_lang[1],
        " ===".bright_cyan()
    );

    let api_key = env::var("GEMINI_API_KEY").expect("You have to set GEMINI_API_KEY env variable");

    let stem = stem_from_logfile(logfile_name);
    let load_profile = load_profile_for_stem(stem);
    let suffix = if tools_mode { "_tools" } else { "" };
    let response_file = format!("{}_gemini{}.md", logfile_name, suffix);
    let client = Client::new();

    let spell =
        build_model_instructions(vendor_model_lang[2], args, &events_sqls, stem, tools_mode);
    let main_report_uri = upload_log_file_gemini(
        &api_key,
        report_for_ai.to_string(),
        "main_report.toon".to_string(),
    )
    .await
    .unwrap();
    let global_profile_data_uri = upload_log_file_gemini(
        &api_key,
        load_profile,
        "load_profile_statistics.json".to_string(),
    )
    .await
    .unwrap();

    let mut initial_parts = vec![
        json!({
            "fileData": {
                "mimeType": "text/plain",
                "fileUri": main_report_uri
            }
        }),
        json!({
            "fileData": {
                "mimeType": "text/plain",
                "fileUri": global_profile_data_uri
            }
        }),
    ];

    if tools_mode && Path::new(&format!("{stem}_attachments")).is_dir() {
        initial_parts.push(json!({
            "text": "### AVAILABLE ATTACHMENTS\nExecution plan attachments (*.xplan) may be available through tools. Use list_available_sql_plans and get_sql_execution_plan for important SQL_IDs before making SQL tuning recommendations.\n-- END AVAILABLE ATTACHMENTS --"
        }));
    }

    let mut contents: Vec<Value> = vec![json!({
        "role": "user",
        "parts": initial_parts
    })];

    let collection: Option<AWRSCollection> = if tools_mode {
        Some(load_tools_collection(args))
    } else {
        None
    };

    let max_iterations = if tools_mode {
        args.max_tool_iterations
    } else {
        1
    };

    let tools = if tools_mode {
        tools_schema_for_gemini(stem)
    } else {
        json!([])
    };

    let gemini_tool_payload_budget = if tools_mode {
        let initial_payload_tokens = gemini_payload_tokens(&spell, &contents, Some(&tools));
        let budget = initial_payload_tokens.saturating_add(args.tokens_budget);
        println!(
            "Gemini tools token guard: initial payload ~{} tokens, tool headroom {}, stop threshold ~{} tokens.",
            initial_payload_tokens, args.tokens_budget, budget
        );
        budget
    } else {
        args.tokens_budget
    };

    let mut final_content = String::new();
    let mut last_usage: Value = Value::Null;
    let mut last_finish: Value = Value::Null;

    for iteration in 0..max_iterations {
        if tools_mode {
            println!(
                "🔁 Gemini tool loop iteration {}/{}",
                iteration + 1,
                max_iterations
            );
        }

        let is_last_iteration = iteration + 1 == max_iterations;

        let mut payload = json!({
            "systemInstruction": {
                "parts": [{ "text": format!("### SYSTEM INSTRUCTIONS\n{spell}") }]
            },
            "contents": contents.clone(),
            "generationConfig": {
                "thinkingConfig": {
                    "thinkingBudget": -1
                }
            }
        });

        if tools_mode {
            payload["tools"] = tools.clone();
            payload["toolConfig"] = json!({
                "functionCallingConfig": { "mode": "AUTO" }
            });
        }

        if tools_mode {
            let estimated_payload_tokens = estimate_tokens_from_value(&payload);
            println!(
                "Estimated Gemini tool payload tokens: {}/{}",
                estimated_payload_tokens, gemini_tool_payload_budget
            );

            if estimated_payload_tokens > gemini_tool_payload_budget {
                eprintln!(
                    "⚠️ Gemini tools token budget reached before iteration {}/{}. \
                     Running final synthesis without more tool calls.",
                    iteration + 1,
                    max_iterations
                );

                contents.push(json!({
                    "role": "user",
                    "parts": [{ "text": final_synthesis_request() }]
                }));

                let compacted = compact_gemini_tool_results_for_budget(
                    &spell,
                    &mut contents,
                    gemini_tool_payload_budget,
                );

                if compacted > 0 {
                    eprintln!(
                        "⚠️ Compacted {} large Gemini tool result(s) to fit the final synthesis budget.",
                        compacted
                    );
                }

                let final_payload = json!({
                    "systemInstruction": {
                        "parts": [{ "text": format!("### SYSTEM INSTRUCTIONS\n{spell}") }]
                    },
                    "contents": contents.clone(),
                    "generationConfig": {
                        "thinkingConfig": {
                            "thinkingBudget": -1
                        }
                    }
                });
                let final_payload_tokens = estimate_tokens_from_value(&final_payload);

                if final_payload_tokens > gemini_tool_payload_budget {
                    return Err(format!(
                        "Gemini tools token budget exhausted: final synthesis payload is ~{} tokens, budget is ~{} tokens. \
                         Increase --tokens-budget or reduce the initial report/tool scope.",
                        final_payload_tokens, gemini_tool_payload_budget
                    )
                    .into());
                }

                println!(
                    "Estimated Gemini final synthesis tokens: {}/{}",
                    final_payload_tokens, gemini_tool_payload_budget
                );

                let (tx, rx) = oneshot::channel();
                let spinner = tokio::spawn(spinning_beer(rx));

                let final_response = client
                    .post(format!(
                        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
                        vendor_model_lang[1],
                        api_key
                    ))
                    .header("Content-Type", "application/json")
                    .json(&final_payload)
                    .send()
                    .await?;

                let _ = tx.send(());
                let _ = spinner.await;

                if !final_response.status().is_success() {
                    eprintln!("Error during final synthesis: {}", final_response.status());
                    eprintln!("{}", final_response.text().await.unwrap_or_default());
                    break;
                }

                let final_json: Value = final_response.json().await?;
                last_usage = final_json
                    .get("usageMetadata")
                    .cloned()
                    .unwrap_or(Value::Null);
                last_finish = final_json
                    .pointer("/candidates/0/finishReason")
                    .cloned()
                    .unwrap_or(Value::Null);
                final_content = extract_gemini_text(&final_json);
                break;
            }
        }

        debug_note!("Payload for Gemini API is {:?}", payload);

        let (tx, rx) = oneshot::channel();
        let spinner = tokio::spawn(spinning_beer(rx));

        let response = client
            .post(format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
                vendor_model_lang[1], api_key
            ))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        let _ = tx.send(());
        let _ = spinner.await;

        if !response.status().is_success() {
            eprintln!("Error: {}", response.status());
            eprintln!("{}", response.text().await.unwrap_or_default());
            break;
        }

        let json: Value = response.json().await?;
        last_usage = json.get("usageMetadata").cloned().unwrap_or(Value::Null);
        last_finish = json
            .pointer("/candidates/0/finishReason")
            .cloned()
            .unwrap_or(Value::Null);

        if tools_mode {
            let tool_calls = extract_gemini_function_calls(&json);
            if !tool_calls.is_empty() {
                if let Some(model_content) = json.pointer("/candidates/0/content").cloned() {
                    contents.push(model_content);
                }

                for tc in tool_calls {
                    let fn_name = tc
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let parsed_args = tc.get("args").cloned().unwrap_or_else(|| json!({}));

                    println!("🛠  Gemini tool call: {}({})", fn_name, parsed_args);

                    let result_text = dispatch_tool_call(
                        &fn_name,
                        &parsed_args,
                        collection.as_ref().unwrap(),
                        stem,
                    );
                    let result_json: Value = serde_json::from_str(&result_text)
                        .unwrap_or_else(|_| json!({ "result": result_text }));

                    contents.push(json!({
                        "role": "user",
                        "parts": [{
                            "functionResponse": {
                                "name": fn_name,
                                "response": result_json
                            }
                        }]
                    }));
                }

                if is_last_iteration {
                    eprintln!(
                        "⚠️ Tool loop limit reached while model still requested tools. \
                         Running final synthesis pass without tools."
                    );
                    contents.push(json!({
                        "role": "user",
                        "parts": [{ "text": final_synthesis_request() }]
                    }));

                    let compacted = compact_gemini_tool_results_for_budget(
                        &spell,
                        &mut contents,
                        gemini_tool_payload_budget,
                    );

                    if compacted > 0 {
                        eprintln!(
                            "⚠️ Compacted {} large Gemini tool result(s) to fit the final synthesis budget.",
                            compacted
                        );
                    }

                    let final_payload = json!({
                        "systemInstruction": {
                            "parts": [{ "text": format!("### SYSTEM INSTRUCTIONS\n{spell}") }]
                        },
                        "contents": contents.clone(),
                        "generationConfig": {
                            "thinkingConfig": {
                                "thinkingBudget": -1
                            }
                        }
                    });
                    let final_payload_tokens = estimate_tokens_from_value(&final_payload);

                    if final_payload_tokens > gemini_tool_payload_budget {
                        return Err(format!(
                            "Gemini tools token budget exhausted: final synthesis payload is ~{} tokens, budget is ~{} tokens. \
                             Increase --tokens-budget or reduce the initial report/tool scope.",
                            final_payload_tokens, gemini_tool_payload_budget
                        )
                        .into());
                    }

                    println!(
                        "Estimated Gemini final synthesis tokens: {}/{}",
                        final_payload_tokens, gemini_tool_payload_budget
                    );

                    let (tx, rx) = oneshot::channel();
                    let spinner = tokio::spawn(spinning_beer(rx));

                    let final_response = client
                        .post(format!(
                            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
                            vendor_model_lang[1],
                            api_key
                        ))
                        .header("Content-Type", "application/json")
                        .json(&final_payload)
                        .send()
                        .await?;

                    let _ = tx.send(());
                    let _ = spinner.await;

                    if !final_response.status().is_success() {
                        eprintln!("Error during final synthesis: {}", final_response.status());
                        eprintln!("{}", final_response.text().await.unwrap_or_default());
                        break;
                    }

                    let final_json: Value = final_response.json().await?;
                    last_usage = final_json
                        .get("usageMetadata")
                        .cloned()
                        .unwrap_or(Value::Null);
                    last_finish = final_json
                        .pointer("/candidates/0/finishReason")
                        .cloned()
                        .unwrap_or(Value::Null);
                    final_content = extract_gemini_text(&final_json);
                    break;
                }

                continue;
            }
        }

        final_content = extract_gemini_text(&json);
        break;
    }

    if final_content.is_empty() {
        eprintln!("⚠️ Gemini response had no extractable final content");
    } else {
        fs::write(&response_file, final_content.as_bytes())?;
        println!("🍻 Gemini response written to file: {}", &response_file);
        convert_md_to_html_file(&response_file, events_sqls.clone());
        println!(
            "Total tokens: {}\nFinish reason: {}\n",
            last_usage, last_finish
        );
    }

    Ok(())
}

fn extract_chat_message_content(msg: &Value) -> String {
    if let Some(s) = msg.get("content").and_then(|v| v.as_str()) {
        return s.to_string();
    }

    if let Some(arr) = msg.get("content").and_then(|v| v.as_array()) {
        let mut out = String::new();

        for item in arr {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                out.push_str(text);
                out.push('\n');
            } else if let Some(text) = item.get("content").and_then(|v| v.as_str()) {
                out.push_str(text);
                out.push('\n');
            }
        }

        return out.trim().to_string();
    }

    String::new()
}

#[tokio::main]
pub async fn openrouter(
    logfile_name: &str,
    vendor_model_lang: Vec<&str>,
    events_sqls: HashMap<&str, HashSet<String>>,
    args: &crate::Args,
    report_for_ai: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let tools_mode = args.tools_mode;
    let mode_label = if tools_mode { "TOOLS" } else { "single-shot" };
    println!(
        "=== Consulting OpenRouter ({}) model: {} ===",
        mode_label, vendor_model_lang[1]
    );

    let api_key =
        env::var("OPENROUTER_API_KEY").expect("You have to set OPENROUTER_API_KEY env variable");

    let stem = stem_from_logfile(logfile_name);
    let load_profile = load_profile_for_stem(stem);

    let model_name = vendor_model_lang[1].replace("/", "_");
    let suffix = if tools_mode { "_tools" } else { "" };
    let response_file = format!("{}_{}{}.md", logfile_name, model_name, suffix);
    let client = Client::new();

    let spell =
        build_model_instructions(vendor_model_lang[2], args, &events_sqls, stem, tools_mode);

    // --- common - history begins ---
    let mut messages: Vec<Value> = vec![
        json!({ "role": "system", "content": format!("### SYSTEM INSTRUCTIONS\n{}", spell) }),
        json!({ "role": "user", "content": format!(
            "MAIN REPORT (toon/json-as-text):\n```\n{}\n```\n\nGLOBAL PROFILE:\n```json\n{}\n```",
            report_for_ai, load_profile
        )}),
    ];

    let collection: Option<AWRSCollection> = if tools_mode {
        Some(load_tools_collection(args))
    } else {
        None
    };

    let max_iterations = if tools_mode {
        args.max_tool_iterations
    } else {
        1 // without tools we always have one iteration.
    };

    let tools = if tools_mode {
        tools_schema(stem)
    } else {
        json!([])
    };

    let openrouter_tool_payload_budget = if tools_mode {
        let initial_payload_tokens =
            openrouter_payload_tokens(vendor_model_lang[1], &messages, Some(&tools));
        let budget = initial_payload_tokens.saturating_add(args.tokens_budget);
        println!(
            "OpenRouter tools token guard: initial payload ~{} tokens, tool headroom {}, stop threshold ~{} tokens.",
            initial_payload_tokens, args.tokens_budget, budget
        );
        budget
    } else {
        args.tokens_budget
    };

    let mut final_content = String::new();
    let mut last_usage: Value = Value::Null;
    let mut last_finish: String = String::new();

    for iteration in 0..max_iterations {
        if tools_mode {
            println!(
                "🔁 Tool loop iteration {}/{}",
                iteration + 1,
                max_iterations
            );
        }

        let is_last_iteration = iteration + 1 == max_iterations;

        // Payload — if tools mode is on we can add it to payload
        let mut payload = json!({
            "model": vendor_model_lang[1],
            "messages": messages,
            "reasoning": { "effort": "high" },
            "stream": false
        });

        if tools_mode {
            payload["tools"] = tools.clone();
            payload["tool_choice"] = json!("auto");
        }

        if tools_mode {
            let estimated_payload_tokens = estimate_tokens_from_value(&payload);
            println!(
                "Estimated OpenRouter tool payload tokens: {}/{}",
                estimated_payload_tokens, openrouter_tool_payload_budget
            );

            if estimated_payload_tokens > openrouter_tool_payload_budget {
                eprintln!(
                    "⚠️ OpenRouter tools token budget reached before iteration {}/{}. \
                     Running final synthesis without more tool calls.",
                    iteration + 1,
                    max_iterations
                );

                messages.push(json!({
                    "role": "user",
                    "content": final_synthesis_request()
                }));

                let compacted = compact_openrouter_tool_results_for_budget(
                    vendor_model_lang[1],
                    &mut messages,
                    openrouter_tool_payload_budget,
                );

                if compacted > 0 {
                    eprintln!(
                        "⚠️ Compacted {} large tool result(s) to fit the final synthesis budget.",
                        compacted
                    );
                }

                let final_payload = json!({
                    "model": vendor_model_lang[1],
                    "messages": messages,
                    "reasoning": { "effort": "high" },
                    "stream": false
                });
                let final_payload_tokens = estimate_tokens_from_value(&final_payload);

                if final_payload_tokens > openrouter_tool_payload_budget {
                    return Err(format!(
                        "OpenRouter tools token budget exhausted: final synthesis payload is ~{} tokens, budget is ~{} tokens. \
                         Increase --tokens-budget or reduce the initial report/tool scope.",
                        final_payload_tokens, openrouter_tool_payload_budget
                    )
                    .into());
                }

                println!(
                    "Estimated OpenRouter final synthesis tokens: {}/{}",
                    final_payload_tokens, openrouter_tool_payload_budget
                );

                let (tx, rx) = oneshot::channel();
                let spinner = tokio::spawn(spinning_beer(rx));

                let final_resp = client
                    .post("https://openrouter.ai/api/v1/chat/completions")
                    .header("Authorization", format!("Bearer {}", api_key))
                    .header("Content-Type", "application/json")
                    .header("X-Title", "jas-min")
                    .json(&final_payload)
                    .send()
                    .await?;

                let _ = tx.send(());
                let _ = spinner.await;

                if !final_resp.status().is_success() {
                    eprintln!("Error during final synthesis: {}", final_resp.status());
                    eprintln!("{}", final_resp.text().await.unwrap_or_default());
                    break;
                }

                let final_body = final_resp.text().await?;
                let final_json =
                    parse_openrouter_response_json(&final_body, &response_file, "final_synthesis")?;

                let final_choice = &final_json["choices"][0];
                let final_msg = &final_choice["message"];

                last_usage = final_json["usage"].clone();
                last_finish = final_choice["finish_reason"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();

                final_content = extract_chat_message_content(final_msg);

                if final_content.is_empty() {
                    eprintln!("⚠️ Final synthesis message had no extractable content:");
                    eprintln!(
                        "{}",
                        serde_json::to_string_pretty(final_msg)
                            .unwrap_or_else(|_| final_msg.to_string())
                    );
                }

                break;
            }
        }

        debug_note!("Payload for AI is {:?}", payload);

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

        let _ = tx.send(());
        let _ = spinner.await;

        if !resp.status().is_success() {
            eprintln!("Error: {}", resp.status());
            eprintln!("{}", resp.text().await.unwrap_or_default());
            break;
        }

        let body = resp.text().await?;
        let json = parse_openrouter_response_json(&body, &response_file, "tool_loop")?;

        let choice = &json["choices"][0];
        let msg = &choice["message"];

        last_usage = json["usage"].clone();
        last_finish = choice["finish_reason"].as_str().unwrap_or("").to_string();

        // --- TOOLS: is model calling any tools? ---
        if tools_mode {
            if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                if !tool_calls.is_empty() {
                    messages.push(msg.clone()); // keep assistant tool-call message in history

                    for tc in tool_calls {
                        let tc_id = tc["id"].as_str().unwrap_or("").to_string();

                        let fn_name = tc["function"]["name"].as_str().unwrap_or("").to_string();

                        let raw_args = tc["function"]["arguments"].as_str().unwrap_or("{}");

                        let parsed_args: Value =
                            serde_json::from_str(raw_args).unwrap_or_else(|_| json!({}));

                        println!("🛠  Tool call: {}({})", fn_name, parsed_args);

                        let result = dispatch_tool_call(
                            &fn_name,
                            &parsed_args,
                            collection.as_ref().unwrap(),
                            stem,
                        );

                        let result_text = serde_json::to_string(&result).unwrap_or_else(|_| {
                            "{\"error\":\"failed to serialize tool result\"}".to_string()
                        });

                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": tc_id,
                            "content": result_text
                        }));
                    }

                    if is_last_iteration {
                        eprintln!(
                            "⚠️ Tool loop limit reached while model still requested tools. \
                         Running final synthesis pass without tools."
                        );

                        messages.push(json!({
                            "role": "user",
                            "content": final_synthesis_request()
                        }));

                        let compacted = compact_openrouter_tool_results_for_budget(
                            vendor_model_lang[1],
                            &mut messages,
                            openrouter_tool_payload_budget,
                        );

                        if compacted > 0 {
                            eprintln!(
                                "⚠️ Compacted {} large tool result(s) to fit the final synthesis budget.",
                                compacted
                            );
                        }

                        let final_payload = json!({
                            "model": vendor_model_lang[1],
                            "messages": messages,
                            "reasoning": { "effort": "high" },
                            "stream": false
                        });
                        let final_payload_tokens = estimate_tokens_from_value(&final_payload);

                        if final_payload_tokens > openrouter_tool_payload_budget {
                            return Err(format!(
                                "OpenRouter tools token budget exhausted: final synthesis payload is ~{} tokens, budget is ~{} tokens. \
                                 Increase --tokens-budget or reduce the initial report/tool scope.",
                                final_payload_tokens, openrouter_tool_payload_budget
                            )
                            .into());
                        }

                        println!(
                            "Estimated OpenRouter final synthesis tokens: {}/{}",
                            final_payload_tokens, openrouter_tool_payload_budget
                        );

                        debug_note!("Final synthesis payload for AI is {:?}", final_payload);

                        let (tx, rx) = oneshot::channel();
                        let spinner = tokio::spawn(spinning_beer(rx));

                        let final_resp = client
                            .post("https://openrouter.ai/api/v1/chat/completions")
                            .header("Authorization", format!("Bearer {}", api_key))
                            .header("Content-Type", "application/json")
                            .header("X-Title", "jas-min")
                            .json(&final_payload)
                            .send()
                            .await?;

                        let _ = tx.send(());
                        let _ = spinner.await;

                        if !final_resp.status().is_success() {
                            eprintln!("Error during final synthesis: {}", final_resp.status());
                            eprintln!("{}", final_resp.text().await.unwrap_or_default());
                            break;
                        }

                        let final_body = final_resp.text().await?;
                        let final_json = parse_openrouter_response_json(
                            &final_body,
                            &response_file,
                            "final_synthesis",
                        )?;

                        let final_choice = &final_json["choices"][0];
                        let final_msg = &final_choice["message"];

                        last_usage = final_json["usage"].clone();
                        last_finish = final_choice["finish_reason"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();

                        final_content = extract_chat_message_content(final_msg);

                        if final_content.is_empty() {
                            eprintln!("⚠️ Final synthesis message had no extractable content:");
                            eprintln!(
                                "{}",
                                serde_json::to_string_pretty(final_msg)
                                    .unwrap_or_else(|_| final_msg.to_string())
                            );
                        }

                        break;
                    }

                    continue; // next round, because tools were called
                }
            }
        }

        // --- No tool called or single-shot -> final answer ---
        final_content = extract_chat_message_content(msg);

        if final_content.is_empty() {
            eprintln!("⚠️ Assistant message had no extractable final content:");
            eprintln!(
                "{}",
                serde_json::to_string_pretty(msg).unwrap_or_else(|_| msg.to_string())
            );
        }

        break;
    }

    fs::write(&response_file, final_content.as_bytes())?;
    println!("🍻 OpenRouter response written to file: {}", &response_file);
    convert_md_to_html_file(&response_file, events_sqls.clone());
    println!(
        "Total tokens: {}\nFinish reason: {}\n",
        last_usage, last_finish
    );

    Ok(())
}

fn tools_schema_for_openai_responses(stem: &str) -> Value {
    let tools = tools_schema(stem);

    let Some(arr) = tools.as_array() else {
        return json!([]);
    };

    let converted: Vec<Value> = arr
        .iter()
        .filter_map(|tool| {
            let function = tool.get("function")?;
            let name = function.get("name")?.clone();
            let description = function
                .get("description")
                .cloned()
                .unwrap_or_else(|| json!(""));
            let parameters = function.get("parameters").cloned().unwrap_or_else(|| {
                json!({
                    "type": "object",
                    "properties": {}
                })
            });

            Some(json!({
                "type": "function",
                "name": name,
                "description": description,
                "parameters": parameters,
                "strict": false
            }))
        })
        .collect();

    json!(converted)
}

fn tools_schema_for_gemini(stem: &str) -> Value {
    let tools = tools_schema(stem);

    let Some(arr) = tools.as_array() else {
        return json!([]);
    };

    let declarations: Vec<Value> = arr
        .iter()
        .filter_map(|tool| {
            let function = tool.get("function")?;
            let name = function.get("name")?.clone();
            let description = function
                .get("description")
                .cloned()
                .unwrap_or_else(|| json!(""));
            let parameters = function.get("parameters").cloned().unwrap_or_else(|| {
                json!({
                    "type": "object",
                    "properties": {}
                })
            });

            Some(json!({
                "name": name,
                "description": description,
                "parameters": parameters
            }))
        })
        .collect();

    json!([{ "functionDeclarations": declarations }])
}

fn extract_openai_responses_text(json: &Value) -> String {
    if let Some(s) = json.get("output_text").and_then(|v| v.as_str()) {
        return s.to_string();
    }

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
}

#[tokio::main]
pub async fn openai_gpt(
    logfile_name: &str,
    vendor_model_lang: Vec<&str>,
    events_sqls: HashMap<&str, HashSet<String>>,
    args: &crate::Args,
    report_for_ai: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let tools_mode = args.tools_mode;
    let mode_label = if tools_mode { "TOOLS" } else { "single-shot" };
    println!(
        "{}{}{}{}{}",
        "=== Consulting OpenAI (".bright_cyan(),
        mode_label,
        ") model: ".bright_cyan(),
        vendor_model_lang[1],
        " ===".bright_cyan()
    );

    let api_key = env::var("OPENAI_API_KEY").expect("You have to set OPENAI_API_KEY env variable");

    let stem = stem_from_logfile(logfile_name);
    let load_profile = load_profile_for_stem(stem);

    let suffix = if tools_mode { "_tools" } else { "" };
    let response_file = format!("{}_{}{}.md", logfile_name, vendor_model_lang[1], suffix);

    let spell =
        build_model_instructions(vendor_model_lang[2], args, &events_sqls, stem, tools_mode);

    let mut input_messages = vec![json!({
        "role": "system",
        "content": [
            { "type": "input_text", "text": spell }
        ]
    })];

    let mut report_payload = vec![
        json!({"type":"input_text", "text":
            format!("### ATTACHED REPORT\n{report_for_ai}\n-- END ATTACHED REPORT --")
        }),
        json!({
            "type": "input_text",
            "text": format!("### LOAD PROFILE STATISTICS JSON\n{}\n-- END JSON --", load_profile)
        }),
    ];

    if tools_mode && Path::new(&format!("{stem}_attachments")).is_dir() {
        report_payload.push(json!({
            "type": "input_text",
            "text": "### AVAILABLE ATTACHMENTS\nExecution plan attachments (*.xplan) may be available through tools. Use list_available_sql_plans and get_sql_execution_plan for important SQL_IDs before making SQL tuning recommendations.\n-- END AVAILABLE ATTACHMENTS --"
        }));
    }

    input_messages.push(json!({
        "role": "user",
        "content": report_payload
    }));

    let collection: Option<AWRSCollection> = if tools_mode {
        Some(load_tools_collection(args))
    } else {
        None
    };

    let max_iterations = if tools_mode {
        args.max_tool_iterations
    } else {
        1
    };

    let tools = if tools_mode {
        tools_schema_for_openai_responses(stem)
    } else {
        json!([])
    };

    let openai_tool_payload_budget = if tools_mode {
        let initial_payload_tokens =
            openai_responses_payload_tokens(vendor_model_lang[1], &input_messages, Some(&tools));
        let budget = initial_payload_tokens.saturating_add(args.tokens_budget);
        println!(
            "OpenAI tools token guard: initial payload ~{} tokens, tool headroom {}, stop threshold ~{} tokens.",
            initial_payload_tokens, args.tokens_budget, budget
        );
        budget
    } else {
        args.tokens_budget
    };

    let client = Client::new();
    let mut final_content = String::new();
    let mut last_usage: Value = Value::Null;
    let mut last_finish: Value = Value::Null;

    for iteration in 0..max_iterations {
        if tools_mode {
            println!(
                "🔁 OpenAI tool loop iteration {}/{}",
                iteration + 1,
                max_iterations
            );
        }

        let is_last_iteration = iteration + 1 == max_iterations;

        let mut payload = json!({
            "model": vendor_model_lang[1],
            "input": input_messages,
        });

        if tools_mode {
            payload["tools"] = tools.clone();
            payload["tool_choice"] = json!("auto");
        }

        let estimated_payload_tokens = estimate_tokens_from_value(&payload);
        if tools_mode {
            println!(
                "Estimated OpenAI tool payload tokens: {}/{}",
                estimated_payload_tokens, openai_tool_payload_budget
            );

            if estimated_payload_tokens > openai_tool_payload_budget {
                eprintln!(
                    "⚠️ OpenAI tools token budget reached before iteration {}/{}. \
                     Running final synthesis without more tool calls.",
                    iteration + 1,
                    max_iterations
                );

                input_messages.push(json!({
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": final_synthesis_request() }
                    ]
                }));

                let compacted = compact_openai_tool_results_for_budget(
                    vendor_model_lang[1],
                    &mut input_messages,
                    openai_tool_payload_budget,
                );

                if compacted > 0 {
                    eprintln!(
                        "⚠️ Compacted {} large OpenAI tool result(s) to fit the final synthesis budget.",
                        compacted
                    );
                }

                let final_payload = json!({
                    "model": vendor_model_lang[1],
                    "input": input_messages,
                });
                let final_payload_tokens = estimate_tokens_from_value(&final_payload);

                if final_payload_tokens > openai_tool_payload_budget {
                    return Err(format!(
                        "OpenAI tools token budget exhausted: final synthesis payload is ~{} tokens, budget is ~{} tokens. \
                         Increase --tokens-budget or reduce the initial report/tool scope.",
                        final_payload_tokens, openai_tool_payload_budget
                    )
                    .into());
                }

                println!(
                    "Estimated OpenAI final synthesis tokens: {}/{}",
                    final_payload_tokens, openai_tool_payload_budget
                );

                let (tx, rx) = oneshot::channel();
                let spinner = tokio::spawn(spinning_beer(rx));

                let final_response = client
                    .post(format!("{}v1/responses", get_openai_url()))
                    .bearer_auth(&api_key)
                    .header("Content-Type", "application/json")
                    .json(&final_payload)
                    .send()
                    .await?;

                let _ = tx.send(());
                let _ = spinner.await;

                if !final_response.status().is_success() {
                    eprintln!("Error during final synthesis: {}", final_response.status());
                    eprintln!("{}", final_response.text().await.unwrap_or_default());
                    break;
                }

                let final_json: Value = final_response.json().await?;
                last_usage = final_json.get("usage").cloned().unwrap_or(Value::Null);
                last_finish = final_json
                    .pointer("/output/0/finish_reason")
                    .cloned()
                    .or_else(|| final_json.get("finish_reason").cloned())
                    .unwrap_or(Value::Null);
                final_content = extract_openai_responses_text(&final_json);
                break;
            }
        } else {
            println!(
                "The whole estimated number of tokens is: {}",
                estimated_payload_tokens
            );
        }
        debug_note!("Payload for OpenAI Responses API is {:?}", payload);

        let (tx, rx) = oneshot::channel();
        let spinner = tokio::spawn(spinning_beer(rx));

        let response = client
            .post(format!("{}v1/responses", get_openai_url()))
            .bearer_auth(&api_key)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        let _ = tx.send(());
        let _ = spinner.await;

        if !response.status().is_success() {
            eprintln!("Error: {}", response.status());
            eprintln!("{}", response.text().await.unwrap_or_default());
            break;
        }

        let json: Value = response.json().await?;
        last_usage = json.get("usage").cloned().unwrap_or(Value::Null);
        last_finish = json
            .pointer("/output/0/finish_reason")
            .cloned()
            .or_else(|| json.get("finish_reason").cloned())
            .unwrap_or(Value::Null);

        if tools_mode {
            let mut tool_calls: Vec<Value> = vec![];
            if let Some(output_arr) = json.get("output").and_then(|o| o.as_array()) {
                for item in output_arr {
                    if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                        tool_calls.push(item.clone());
                    }
                }

                // The Responses API requires passing model output items back,
                // including reasoning items. Tiny detail, massive debugging party.
                input_messages.extend(output_arr.iter().cloned());
            }

            if !tool_calls.is_empty() {
                for tc in tool_calls {
                    let call_id = tc
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let fn_name = tc
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let raw_args = tc.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
                    let parsed_args: Value = serde_json::from_str(raw_args).unwrap_or(json!({}));

                    println!("🛠  OpenAI tool call: {}({})", fn_name, parsed_args);

                    let result = dispatch_tool_call(
                        &fn_name,
                        &parsed_args,
                        collection.as_ref().unwrap(),
                        stem,
                    );

                    input_messages.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": result
                    }));
                }

                if is_last_iteration {
                    eprintln!(
                        "⚠️ Tool loop limit reached while model still requested tools. \
                         Running final synthesis pass without tools."
                    );

                    input_messages.push(json!({
                        "role": "user",
                        "content": [
                            { "type": "input_text", "text": final_synthesis_request() }
                        ]
                    }));

                    let compacted = compact_openai_tool_results_for_budget(
                        vendor_model_lang[1],
                        &mut input_messages,
                        openai_tool_payload_budget,
                    );

                    if compacted > 0 {
                        eprintln!(
                            "⚠️ Compacted {} large OpenAI tool result(s) to fit the final synthesis budget.",
                            compacted
                        );
                    }

                    let final_payload = json!({
                        "model": vendor_model_lang[1],
                        "input": input_messages,
                    });
                    let final_payload_tokens = estimate_tokens_from_value(&final_payload);

                    if final_payload_tokens > openai_tool_payload_budget {
                        return Err(format!(
                            "OpenAI tools token budget exhausted: final synthesis payload is ~{} tokens, budget is ~{} tokens. \
                             Increase --tokens-budget or reduce the initial report/tool scope.",
                            final_payload_tokens, openai_tool_payload_budget
                        )
                        .into());
                    }

                    println!(
                        "Estimated OpenAI final synthesis tokens: {}/{}",
                        final_payload_tokens, openai_tool_payload_budget
                    );

                    debug_note!(
                        "Final synthesis payload for OpenAI Responses API is {:?}",
                        final_payload
                    );

                    let (tx, rx) = oneshot::channel();
                    let spinner = tokio::spawn(spinning_beer(rx));

                    let final_response = client
                        .post(format!("{}v1/responses", get_openai_url()))
                        .bearer_auth(&api_key)
                        .header("Content-Type", "application/json")
                        .json(&final_payload)
                        .send()
                        .await?;

                    let _ = tx.send(());
                    let _ = spinner.await;

                    if !final_response.status().is_success() {
                        eprintln!("Error during final synthesis: {}", final_response.status());
                        eprintln!("{}", final_response.text().await.unwrap_or_default());
                        break;
                    }

                    let final_json: Value = final_response.json().await?;
                    last_usage = final_json.get("usage").cloned().unwrap_or(Value::Null);
                    last_finish = final_json
                        .pointer("/output/0/finish_reason")
                        .cloned()
                        .or_else(|| final_json.get("finish_reason").cloned())
                        .unwrap_or(Value::Null);
                    final_content = extract_openai_responses_text(&final_json);
                    break;
                }

                continue;
            }
        }

        final_content = extract_openai_responses_text(&json);
        break;
    }

    if final_content.is_empty() {
        eprintln!("⚠️  No final content produced");
    } else {
        fs::write(&response_file, final_content.as_bytes())?;
        println!("🧠 OpenAI response written to file: {}", &response_file);
        convert_md_to_html_file(&response_file, events_sqls);
        println!("Total tokens (OpenAI): {}", last_usage);
        println!("Finish reason: {}", last_finish);
    }

    Ok(())
}
