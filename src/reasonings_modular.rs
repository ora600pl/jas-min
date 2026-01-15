use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{cmp::Ordering, env, fs};
use reqwest::Client;
use tokio::sync::oneshot;
use crate::tools::*;
use crate::reasonings::{StatisticsDescription,TopPeaksSelected,MadAnomaliesEvents,MadAnomaliesSQL,TopForegroundWaitEvents,TopBackgroundWaitEvents,PctOfTimesThisSQLFoundInOtherTopSections,WaitEventsWithStrongCorrelation,WaitEventsFromASH,TopSQLsByElapsedTime,StatsSummary,IOStatsByFunctionSummary,LatchActivitySummary,Top10SegmentStats,InstanceStatisticCorrelation,LoadProfileAnomalies,AnomalyDescription,AnomlyCluster,ReportForAI,AppState};
use toon::encode;


/// =====================
/// Output: SectionNotes
/// =====================
/// This is what the model MUST return in modular steps (JSON only).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SectionNotes {
    pub section: String,
    pub highlights: Vec<Highlight>,
    pub risks: Vec<Risk>,
    pub recommendations: Vec<Recommendation>,
    pub notes: String,
}

impl Default for SectionNotes {
    fn default() -> Self {
        Self {
            section: String::new(),
            highlights: Vec::new(),
            risks: Vec::new(),
            recommendations: Vec::new(),
            notes: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Highlight {
    pub title: String,
    pub evidence: Vec<String>,
    pub snap_refs: Vec<SnapRef>,
    pub entities: Entities,
}

// Add Default impl so #[serde(default)] can fill missing fields cleanly.
impl Default for Highlight {
    fn default() -> Self {
        Self {
            title: String::new(),
            evidence: Vec::new(),
            snap_refs: Vec::new(),
            entities: Entities::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapRef {
    pub snap_id: Option<u64>,
    pub date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Entities {
    pub sql_ids: Vec<String>,
    pub wait_events: Vec<String>,
    pub segments: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Risk {
    pub risk: String,
    pub why: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub audience: String, // "DBA" | "DEV" | "MGMT"
    pub action: String,
    pub why: String,
    pub evidence: Vec<String>,
}

/// =====================
/// Modular sections enum
/// =====================
#[derive(Debug, Clone, Copy)]
pub enum Section {
    Baseline,
    WaitEventsForeground,
    WaitEventsBackground,
    SqlElapsedTime,
    IoStatsByFunction,
    Latches,

    SegmentsRowLockWaits,
    SegmentsPhysicalWrites,
    SegmentsPhysicalWriteRequests,
    SegmentsPhysicalReadRequests,
    SegmentsLogicalReads,
    SegmentsDirectPhysicalWrites,
    SegmentsDirectPhysicalReads,
    SegmentsBufferBusyWaits,

    InstanceStatsCorrelation,
    LoadProfileAnomalies,
    AnomalyClusters,

    ComposeFinal,

    GradientWaitEvents,
    GradientStatsCounters,
    GradientStatsVolume,
    GradientStatsTime,
    GradientSQLs,
}

/// =====================
/// Config
/// =====================
#[derive(Debug, Clone)]
pub struct ModularLlmConfig {
    pub lang: String,                  // "pl" or "en"
    pub top_spikes_n: usize,            // e.g. 10
    pub temperature: f64,              // e.g. 0.2
    pub max_tokens_per_call: usize,     // e.g. 4096
    pub enable_reasoning_prompt: bool,  // adds "reasoning mode" instructions
    // Trimming knobs to keep chunks small:
    pub waits_top_n: usize,             // e.g. 20
    pub sqls_top_n: usize,              // e.g. 30
    pub anomalies_top_n: usize,         // e.g. 50 (load_profile anomalies)
    pub mad_per_item_top_n: usize,      // e.g. 5 (MAD anomalies per event/sql)
    pub tokens_budget: usize,           // e.g. 131072 (like for openai/gpt-oss-20b)
    pub use_openrouter: bool,
}


/// One client type that can talk either to LM Studio (local) or OpenRouter (remote).
pub enum ChatClient {
    Local(LocalOpenAiCompatClient),
    OpenRouter(OpenRouterClient),
}

impl ChatClient {
    /// Factory: choose backend by a boolean flag (minimal code changes).
    pub fn new(use_openrouter: bool, model_hint: &str, temperature: f64, max_tokens: usize) -> Result<Self, Box<dyn std::error::Error>> {
        if use_openrouter {
            Ok(Self::OpenRouter(OpenRouterClient::from_env(model_hint, temperature, max_tokens)?))
        } else {
            Ok(Self::Local(LocalOpenAiCompatClient::from_env(model_hint, temperature, max_tokens)))
        }
    }

    pub async fn chat_content(&self, system: &str, user: &str) -> Result<String, Box<dyn std::error::Error>> {
        match self {
            ChatClient::Local(c) => c.chat_content(system, user).await,
            ChatClient::OpenRouter(c) => c.chat_content(system, user).await,
        }
    }
}

/// ---------------------
/// LM Studio client (OpenAI-compatible)
/// ---------------------
pub struct LocalOpenAiCompatClient {
    http: Client,
    base_url: String, // e.g. http://localhost:1234/v1
    model: String,    // from LOCAL_MODEL or hint
    temperature: f64,
    max_tokens: usize,
    api_key: String,
}

impl LocalOpenAiCompatClient {
    pub fn from_env(model_hint: &str, temperature: f64, max_tokens: usize) -> Self {
        let base_url = env::var("LOCAL_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:1234/v1/chat/completions".to_string());

        let model = env::var("LOCAL_MODEL")
            .unwrap_or_else(|_| model_hint.to_string());

        let api_key = env::var("LOCAL_API_KEY").unwrap_or("X".to_string());

        Self {
            http: Client::new(),
            base_url,
            model,
            temperature,
            max_tokens,
            api_key,
        }
    }

    pub async fn chat_content(&self, system: &str, user: &str) -> Result<String, Box<dyn std::error::Error>> {
        let url =  self.base_url.trim_end_matches('/');

        let payload = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "temperature": self.temperature,
            //"max_tokens": self.max_tokens,
            "stream": false
        });

        //let resp = self.http.post(url).json(&payload).send().await?;
        let resp = self.http
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("X-Title", "jas-min")
            .json(&payload)
            .send()
            .await?;
        
        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            return Err(format!("Local LLM HTTP {}: {}", status, text).into());
        }

        let v: Value = serde_json::from_str(&text)?;
        Ok(v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string())
    }
}

/// ---------------------
/// OpenRouter client (OpenAI-like but with its own rules)
/// ---------------------
pub struct OpenRouterClient {
    http: Client,
    api_key: String,
    model: String,      // e.g. "anthropic/claude-3.5-sonnet" or "nvidia/nemotron-3-nano"
    temperature: f64,
    max_tokens: usize,
}

impl OpenRouterClient {
    pub fn from_env(model_hint: &str, temperature: f64, max_tokens: usize) -> Result<Self, Box<dyn std::error::Error>> {
        let api_key = env::var("OPENROUTER_API_KEY")
            .map_err(|_| "Missing OPENROUTER_API_KEY env var")?;

        // Allow overriding model via env, else use hint passed by code.
        let model = env::var("OPENROUTER_MODEL").unwrap_or_else(|_| model_hint.to_string());

        Ok(Self {
            http: Client::new(),
            api_key,
            model,
            temperature,
            max_tokens,
        })
    }

    pub async fn chat_content(&self, system: &str, user: &str) -> Result<String, Box<dyn std::error::Error>> {
        let url = "https://openrouter.ai/api/v1/chat/completions";

        let payload = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "temperature": self.temperature,
            //"max_tokens": self.max_tokens,
            "stream": false
            // "reasoning": { "effort": "high" }
        });

        let resp = self.http
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("X-Title", "jas-min")
            .json(&payload)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            return Err(format!("OpenRouter HTTP {}: {}", status, text).into());
        }

        let v: Value = serde_json::from_str(&text)?;
        Ok(v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string())
    }
}


/// =====================
/// Prompt building 
/// =====================

fn system_master_prompt(lang: &str) -> String {
    format!(
r#"Your name is JAS-MIN. You are a professional Oracle Database performance tuning expert and assistant but don't mention it.
You are analyzing JSON file containing summarized statistics from parsed AWR reports from long period of time.
You are an Oracle Database performance expert.

You receive main input object called ReportForAI, encoded as TOON (Token-Oriented Object Notation) or pure JSON.
This TOON/JSON is a preprocessed, structured representation of an Oracle performance audit report (AWR/Statspack family).

If you receive load_profile_statistics.json, containing load profile summary for the database, analyze them first and write comprehensive summary for all metrics with as many statistical insights as possible.

You may also receive a section called db_time_gradient_fg_wait_events, db_time_gradient_instance_stats_[counters|volumes|time] or db_time_gradient_sql_elapsed_time.

This section represents a numerical gradient of DB Time with respect to wait events, key instance statistics and sql elapsed time,
estimated using linear regression on time deltas.

Important interpretation rules:
- Gradient coefficients represent local sensitivity, not global causality.
- Ridge regression provides a stabilized, dense view of contributing wait events, sqls or statistics.
- Elastic Net provides a sparse view, highlighting dominant or representative wait events, sqls or statistics.
- Wait events, sqls or statistics. appearing in both Ridge and Elastic Net rankings should be treated as strong contributors.
- Absence from Elastic Net does NOT mean irrelevance; it may indicate correlation with other elements.

Use those sections to support, not replace, traditional AWR-based reasoning.

============================================================
INPUT FORMAT: ReportForAI (TOON or JSON)

ReportForAI contains the following main sections (mapping from classical AWR-style report):

1. general_data
   - Summary description of overall DB load shape and Median Absolute Deviation analysis.
   - Contains textual description of DBCPU/DBTIME ratio behavior across peaks.

2. top_spikes_marked
   - Array of peaks with:
     - report_name, report_date, snap_id
     - db_time_value, db_cpu_value
     - dbcpu_dbtime_ratio
   - Use this to understand how DBCPU/DBTIME behaves across time and to identify problematic periods.

3. top_foreground_wait_events
   - For each event you have:
     - correlation_with_db_time
     - avg_pct_of_dbtime, stddev_pct_of_db_time
     - avg_wait_time_s, stddev_wait_time_s
     - avg_number_of_executions, stddev_number_of_executions
     - avg_wait_for_execution_ms, stddev_wait_for_execution_ms
     - median_absolute_deviation_anomalies (array of MAD anomalies with anomaly_date, mad_score, total_wait_s, number_of_waits, avg_wait_time_for_execution_ms, pct_of_db_time).

4. top_background_wait_events
   - Same structure as foreground; treat them separately, but correlate with foreground waits.

5. top_sqls_by_elapsed_time
   - For each SQL_ID you have:
     - module, sql_type
     - pct_of_time_sql_was_found_in_other_top_sections (CPU, User I/O, Reads, Gets)
     - correlation_with_db_time, marked_as_top_in_pct_of_probes
     - avg_elapsed_time_by_exec, stddev_elapsed_time_by_exec
     - avg_cpu_time_by_exec, stddev_cpu_time_by_exec
     - avg_elapsed_time_cumulative_s, stddev_elapsed_time_cumulative_s
     - avg_cpu_time_cumulative_s, stddev_cpu_time_cumulative_s
     - avg_number_of_executions, stddev_number_of_executions
     - median_absolute_deviation_anomalies (MAD anomalies per SQL)
     - wait_events_with_strong_pearson_correlation (array of: event_name, correlation_value)
     - wait_events_found_in_ash_sections_for_this_sql (array of: event_name, avg_pct_of_dbtime_in_sql, stddev_pct_of_dbtime_in_sql, count)

6. io_stats_by_function_summary
   - For each function_name (e.g. LGWR, DBWR) you have statistics_summary (statistic_name, avg_value, stddev_value).
   - Use especially LGWR stats and any per-function wait times.

7. latch_activity_summary
   - latch_name, get_requests_avg, weighted_miss_pct, wait_time_weighted_avg_s, found_in_pct_of_probes.

8. TOP 10 Segments sections (this section can be empty if this is a statspack based report)
   - Each of these corresponds to a 'TOP 10 Segments by ...' AWR section:
     - top_10_segments_by_row_lock_waits
     - top_10_segments_by_physical_writes
     - top_10_segments_by_physical_write_requests
     - top_10_segments_by_physical_read_requests
     - top_10_segments_by_logical_reads
     - top_10_segments_by_direct_physical_writes
     - top_10_segments_by_direct_physical_reads
     - top_10_segments_by_buffer_busy_waits
   - Each entry: segment_name, segment_type, object_id, data_object_id, avg, stddev, pct_of_occuriance.

9. instance_stats_pearson_correlation
   - Equivalent to:
     'Instance Statistics: Correlation with DB Time for |ρ| ≥ 0.5'.
   - Each entry: stat_name, pearson_correlation_value.

10. load_profile_anomalies
    - Each anomaly: load_profile_stat_name, anomaly_date, mad_score, mad_threshold, per_second, avg_value_per_second.

11. anomaly_clusters
    - Each cluster:
      - begin_snap_id, begin_snap_date
      - anomalies_detected: array of anomalies:
        - area_of_anomaly
        - statistic_name
      - number_of_anomalies

12. db_time_gradient_fg_wait_events (optional):
    - settings:
        - ridge_lambda
        - elastic_net_lambda
        - elastic_net_alpha
        - elastic_net_max_iter
        - elastic_net_tol
    - ridge_top:
        - list of wait events ranked by impact using Ridge regression
    - elastic_net_top:
        - list of wait events ranked by impact using Elastic Net (may be sparse)

    This section summarizes which wait events most strongly influence changes in DB Time.

13. db_time_gradient_instance_stats_counters (optional):
    - settings:
        - ridge_lambda
        - elastic_net_lambda
        - elastic_net_alpha
        - elastic_net_max_iter
        - elastic_net_tol
    - ridge_top:
        - list of statistic counters ranked by impact using Ridge regression
    - elastic_net_top:
        - list of statistic counters ranked by impact using Elastic Net (may be sparse)

    This section summarizes which key instance stats counters most strongly influence changes in DB Time.

14. db_time_gradient_instance_stats_volumes (optional):
    - settings:
        - ridge_lambda
        - elastic_net_lambda
        - elastic_net_alpha
        - elastic_net_max_iter
        - elastic_net_tol
    - ridge_top:
        - list of volume statistics (like bytes, blocks etc.) ranked by impact using Ridge regression
    - elastic_net_top:
        - list of volume statistics (like bytes, blocks etc.) ranked by impact using Elastic Net (may be sparse)

    This section summarizes which key instance stats (volume means here bytes, blocks, etc.) most strongly influence changes in DB Time.

15. db_time_gradient_instance_stats_time (optional):
    - settings:
        - ridge_lambda
        - elastic_net_lambda
        - elastic_net_alpha
        - elastic_net_max_iter
        - elastic_net_tol
    - ridge_top:
        - list of time statistics (like seconds) ranked by impact using Ridge regression
    - elastic_net_top:
        - list of time statistics (like seconds) ranked by impact using Elastic Net (may be sparse)

    This section summarizes which key instance stats time (like seconds, etc.) most strongly influence changes in DB Time.

16. db_time_gradient_sql_elapsed_time (optional):
    - settings:
        - ridge_lambda
        - elastic_net_lambda
        - elastic_net_alpha
        - elastic_net_max_iter
        - elastic_net_tol
    - ridge_top:
        - list of SQL_IDs (elapsed time) ranked by impact using Ridge regression
    - elastic_net_top:
        - list of SQL_IDs (elapsed time) ranked by impact using Elastic Net (may be sparse)

    This section summarizes which SQL_IDs most strongly influence changes in DB Time.

============================================================
CORE GUIDELINES

- Modular mode: you will often receive only ONE section of ReportForAI at a time.
- Use CONTEXT CAPSULE (if provided) for:
  - timeline anchoring
  - DBCPU/DBTIME interpretation
- Analyze the provided chunk as comprehensively as possible.
- Do NOT hallucinate. Do NOT invent numbers or entities.
- Show important numbers; quote IDs and names exactly as in input.
- When you mention dates, associate them with snap_id when possible.
- When db_time_gradient_instance_stats_* and/or db_time_gradient_fg_wait_events is present:
    - Cross-check gradient results with AWR wait event dominance.
    - Highlight wait events that appear in both Ridge and Elastic Net rankings.
    - Use gradient results to explain *why* DB Time increases, not only *what* increases.
    - Prefer Elastic Net for short actionable lists, and Ridge for broader context.

============================================================
OUTPUT RULES

A) For modular steps:
OUTPUT FORMAT FOR MODULAR STEPS
- Output MUST be pure MARKDOWN (no JSON).
- Start with a level-1 heading using the section name exactly, for example:
  # SECTION: sql_elapsed_time
- Use bullet points and short subsections.
- Include a short "Evidence" subsection with the exact values you used.
- Include a "Recommendations" subsection split into DBA vs DEV vs MGMT if applicable.
- Do NOT include <think> blocks. Do NOT include code fences.

Language: {lang}
"#)
}

fn reasoning_mode_block(enable: bool) -> &'static str {
    if !enable { return ""; }
    r#"
# REASONING MODE
Use deep, multi-step reasoning internally.
Prefer correctness over brevity.
Verify conclusions against provided data.
Do not reveal chain-of-thought. Output only the required markdown as requested.
"#
}

/// Builds a rich user prompt per section.
/// Important: we keep the "big manual" in SYSTEM and keep USER focused on the step + requirements.
fn user_prompt_for_section(section: Section, capsule_json: &str, section_input_json: &str) -> String {
    match section {
        Section::Baseline => format!(r#"
Return SectionNotes JSON.

Section: baseline_profile

Analyze:
- general_data
- top_spikes_marked

Requirements:
- Explain DBCPU/DBTIME ratio meaning and behavior (CPU-bound vs wait-bound) using exact ratios and peak values.
- Use real values from top_spikes_marked: dbcpu_dbtime_ratio, db_time_value, db_cpu_value.
- Identify problematic periods (top 10 by db_time_value if possible). Include snap_id + report_date.
- Do NOT invent anything.

INPUT:
{section_input_json}
"#),

        Section::WaitEventsForeground => format!(r#"
Return SectionNotes JSON.

Section: wait_events_foreground

CONTEXT CAPSULE (timeline anchor):
{capsule_json}

Analyze ONLY this section:
INPUT:
{section_input_json}

Requirements:
- Rank top events by avg_pct_of_dbtime (top 5) and show exact values.
- For each top event include: correlation_with_db_time, avg_pct_of_dbtime, avg_wait_time_s, avg_wait_for_execution_ms, marked_as_top_in_pct_of_probes.
- If MAD anomalies exist: list up to 5 with anomaly_date, mad_score, pct_of_db_time.
- Cross-reference anomaly dates with spikes using context capsule when possible.
- Provide DBA + DEV actions backed by evidence.
"#),

        Section::WaitEventsBackground => format!(r#"
Return SectionNotes JSON.

Section: wait_events_background

CONTEXT CAPSULE (timeline anchor):
{capsule_json}

Analyze ONLY this section:
INPUT:
{section_input_json}

Requirements:
- Rank top events by avg_pct_of_dbtime (top 5) and show exact values.
- For each top event include: correlation_with_db_time, avg_wait_time_s, avg_wait_for_execution_ms, marked_as_top_in_pct_of_probes.
- Correlate patterns with foreground waits if event names suggest redo/IO/DBWR/LGWR mechanisms.
- Provide DBA actions (I/O, redo, background pressure) and DEV actions if applicable.
"#),

        Section::SqlElapsedTime => format!(r#"
Return SectionNotes JSON.

Section: sql_elapsed_time

CONTEXT CAPSULE (timeline anchor):
{capsule_json}

Analyze ONLY this section:
INPUT:
{section_input_json}

Requirements:
- Identify most harmful SQL_IDs (top 5) using cumulative and per-exec metrics (use exact numbers).
- Split into "short-but-frequent" vs "long-running" using avg_number_of_executions and avg_elapsed_time_by_exec.
- For each SQL_ID include: module, sql_type, correlation_with_db_time, marked_as_top_in_pct_of_probes.
- Use wait_events_with_strong_pearson_correlation and ASH waits if present.
- Include MAD anomalies (up to 5) with anomaly_date and mad_score.
- Recommendations must be split DBA vs DEV and backed by evidence.
"#),

        Section::IoStatsByFunction => format!(r#"
Return SectionNotes JSON.

Section: io_stats_by_function

CONTEXT CAPSULE (timeline anchor):
{capsule_json}

Analyze ONLY this section:
INPUT:
{section_input_json}

Requirements:
- Highlight LGWR/DBWR related stats and any storage/redo-related signals (evidence-based).
- Call out high stddev patterns (instability/bursts).
- Provide DBA recommendations regarding redo logs, storage latency, and IO scheduling only if supported by metric names/values.
"#),

        Section::Latches => format!(r#"
Return SectionNotes JSON.

Section: latches

CONTEXT CAPSULE (timeline anchor):
{capsule_json}

Analyze ONLY this section:
INPUT:
{section_input_json}

Requirements:
- Identify top latches by weighted_miss_pct and wait_time_weighted_avg_s.
- Include found_in_pct_of_probes and get_requests_avg.
- Give likely contention causes and evidence-based DBA/DEV actions.
"#),

        // Segments: same pattern, only data differs.
        Section::SegmentsRowLockWaits => segment_prompt("segments_row_lock_waits", capsule_json, section_input_json),
        Section::SegmentsPhysicalWrites => segment_prompt("segments_physical_writes", capsule_json, section_input_json),
        Section::SegmentsPhysicalWriteRequests => segment_prompt("segments_physical_write_requests", capsule_json, section_input_json),
        Section::SegmentsPhysicalReadRequests => segment_prompt("segments_physical_read_requests", capsule_json, section_input_json),
        Section::SegmentsLogicalReads => segment_prompt("segments_logical_reads", capsule_json, section_input_json),
        Section::SegmentsDirectPhysicalWrites => segment_prompt("segments_direct_physical_writes", capsule_json, section_input_json),
        Section::SegmentsDirectPhysicalReads => segment_prompt("segments_direct_physical_reads", capsule_json, section_input_json),
        Section::SegmentsBufferBusyWaits => segment_prompt("segments_buffer_busy_waits", capsule_json, section_input_json),

        //Gradient: same pattern, only data differs. 
        Section::GradientWaitEvents => gradient_prompt("db_time_gradient_fg_wait_events", capsule_json, section_input_json),
        Section::GradientStatsCounters => gradient_prompt("db_time_gradient_instance_stats_counters", capsule_json, section_input_json),
        Section::GradientStatsVolume => gradient_prompt("db_time_gradient_instance_stats_volumes", capsule_json, section_input_json),
        Section::GradientStatsTime => gradient_prompt("db_time_gradient_instance_stats_time", capsule_json, section_input_json),
        Section::GradientSQLs => gradient_prompt("db_time_gradient_sql_elapsed_time", capsule_json, section_input_json),
        
        Section::InstanceStatsCorrelation => format!(r#"
Return SectionNotes JSON.

Section: instance_stats_correlation

CONTEXT CAPSULE (timeline anchor):
{capsule_json}

Analyze ONLY this section:
INPUT:
{section_input_json}

Requirements:
- List top correlations by |pearson_correlation_value| (top 10).
- Highlight logons/logouts/UNDO/chained rows patterns explicitly if present.
- Provide interpretations and DBA/DEV actions backed by evidence.
"#),

        Section::LoadProfileAnomalies => format!(r#"
Return SectionNotes JSON.

Section: load_profile_anomalies

CONTEXT CAPSULE (timeline anchor):
{capsule_json}

Analyze ONLY this section:
INPUT:
{section_input_json}

Requirements:
- Rank anomalies by mad_score (top 10) and show: stat, anomaly_date, mad_score, mad_threshold, per_second, avg_value_per_second.
- Interpret likely operational meaning (burst, storm) and give evidence-based actions.
"#),

        Section::AnomalyClusters => format!(r#"
Return SectionNotes JSON.

Section: anomaly_clusters

CONTEXT CAPSULE (timeline anchor):
{capsule_json}

Analyze ONLY this section:
INPUT:
{section_input_json}

Requirements:
- Summarize clusters: begin_snap_id + begin_snap_date + number_of_anomalies.
- Detect repeating patterns across area_of_anomaly.
- Provide cross-domain hypotheses only if clusters suggest it (do not guess).
"#),

        Section::ComposeFinal => format!(r#"
You will receive an array of SectionNotes JSON objects produced from separate modular analyses.

Task:
- Produce final report in MARKDOWN with structure:
1. 🧭 Executive Summary
2. 📈 Overall Performance Profile
3. ⏳ Wait Event Analysis (Foreground/Background)
4. 🧮 SQL-Level Analysis
5. 🧱 Segment & Object-Level Analysis
6. 🔧 Latches & Internal Contention
7. 💾 IO & Disk Subsystem Assessment
8. 🔁 UNDO / Redo / Load Profile Observations
9. ⚡ Anomaly Clusters, Cross-Domain Patterns & Gradient Analyzes
10. ✅ Recommendations (DBA/DEV/Immediate/Management)

General Rules:
- Do not invent numbers. Use only evidence already present in SectionNotes.
- Always show both SNAP_ID and SNAP_DATE when referencing periods (if present in notes).
- Finish with explicit statements:
  - Disk quality: are disks slow?
  - Comment on application design:
    - Is this likely a poorly written application and why?
    - Is commit/rollback policy proper?
  - Immediate actions for DBAs and Developers.
  - Summary for management (less technical).
- Add link to github: https://github.com/ora600pl/jas-min
- Suggest that good performance tuning experts are at ora-600.pl

WAIT EVENTS DETAILED RULES
Special handling: **'log file sync'** and **'log file parallel write'**:
   - If these are significant in foreground or background events:
     - Check if they appear in anomaly_clusters (area_of_anomaly "WAIT" or "STAT" with names matching these waits).
     - Analyze whether the cause is:
       a) A slow IO subsystem:
          - Use:
            - avg_wait_for_execution_ms and associated MAD anomalies for these waits.
            - io_stats_by_function_summary for LGWR:
              - Look for statistics like "Wait Avg Time", write/IO stats for LGWR.
          - If single-wait times (AVG wait/exec ms and LGWR wait times) are HIGH,
            explain that IO subsystem may be slow.
       b) Excessive redo generation with relatively FAST IO:
          - If single-wait times for LGWR are around ~1 ms or low,
            but these waits still dominate:
            - Use load_profile_anomalies and any redo-related stats to conclude that
              the total amount of redo is huge rather than IO latency being slow.
          - Explain how redo multiplexing can multiply physical writes (if hinted by stats).
     - Clearly state these conclusions in your answer.

INPUT:
{section_input_json}
"#),
    }
}

/// Helper prompt template for gradient sections.
fn gradient_prompt(section_name: &str, capsule_json: &str, section_input_json: &str) -> String {
    format!(r#"
Return SectionNotes JSON.

Section: {section_name}

CONTEXT CAPSULE (timeline anchor):
{capsule_json}

Analyze ONLY this section:
INPUT:
{section_input_json}

Requirements:
  - Interpret Ridge and Elastic Net regretion input data based on the following information:
    - Gradient coefficients represent local sensitivity, not global causality.
    - Ridge regression provides a stabilized, dense view of contributing wait events, sqls or statistics.
    - Elastic Net provides a sparse view, highlighting dominant or representative wait events, sqls or statistics.
    - Wait events, sqls or statistics. appearing in both Ridge and Elastic Net rankings should be treated as strong contributors.
    - Absence from Elastic Net does NOT mean irrelevance; it may indicate correlation with other elements.
"#)
}

/// Helper prompt template for segment sections.
fn segment_prompt(section_name: &str, capsule_json: &str, section_input_json: &str) -> String {
    format!(r#"
Return SectionNotes JSON.

Section: {section_name}

CONTEXT CAPSULE (timeline anchor):
{capsule_json}

Analyze ONLY this section:
INPUT:
{section_input_json}

Requirements:
- Rank top segments by avg and pct_of_occuriance (show exact values).
- If segment_name is missing, use object_id/data_object_id as primary identifiers.
- Provide DBA/DEV actions only if supported by evidence (hot blocks, row lock hotspots, IO hotspots, indexing/partitioning).
"#)
}

/// =====================
/// Trimming utilities (reduce chunk sizes)
/// =====================

fn to_pretty_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".to_string())
}



/// =====================
/// Output sanitization (models love to add junk)
/// =====================

/// Removes model "thinking" blocks like <think>...</think> and similar tags.
fn strip_think_blocks(input: &str) -> String {
    let mut s = input.to_string();
    for tag in ["think", "analysis", "reasoning", "THINK", "ANALYSIS", "REASONING"] {
        loop {
            let open = format!("<{}>", tag);
            let close = format!("</{}>", tag);

            let start = match s.find(&open) {
                Some(i) => i,
                None => break,
            };
            let end = match s.find(&close) {
                Some(j) => j + close.len(),
                None => {
                    // Best-effort: if unclosed, remove from opening tag to end.
                    s.replace_range(start.., "");
                    break;
                }
            };

            if end > start && end <= s.len() {
                s.replace_range(start..end, "");
            } else {
                break;
            }
        }
    }
    s
}

/// Removes common code fences that some models keep adding.
fn strip_code_fences(input: &str) -> String {
    input
        .replace("```markdown", "")
        .replace("```md", "")
        .replace("```", "")
        .trim()
        .to_string()
}

/// Ensures the output starts with the expected header.
/// If the model forgot it, we prepend it.
fn ensure_section_header(section: Section, md: &str) -> String {
    let expected = format!("# SECTION: {}", section_name(section));
    let trimmed = md.trim();

    if trimmed.starts_with(&expected) {
        trimmed.to_string()
    } else {
        format!("{expected}\n\n{trimmed}")
    }
}

/// Sanitizes model output into clean markdown for storage/compose step.
fn sanitize_markdown(section: Section, raw: &str) -> String {
    let s1 = strip_think_blocks(raw);
    let s2 = strip_code_fences(&s1);
    ensure_section_header(section, &s2)
}

/// Keep only top N wait events by avg_pct_of_dbtime and top MAD anomalies per event.
fn trim_foreground_waits(mut v: Vec<TopForegroundWaitEvents>, waits_top_n: usize, mad_top_n: usize) -> Vec<TopForegroundWaitEvents> {
    v.sort_by(|a, b| b.avg_pct_of_dbtime.partial_cmp(&a.avg_pct_of_dbtime).unwrap_or(Ordering::Equal));
    v.truncate(waits_top_n);

    for e in &mut v {
        e.median_absolute_deviation_anomalies
            .sort_by(|a, b| b.mad_score.partial_cmp(&a.mad_score).unwrap_or(Ordering::Equal));
        e.median_absolute_deviation_anomalies.truncate(mad_top_n);
    }
    v
}

fn trim_foreground_waits_by_budget(
    mut v: Vec<TopForegroundWaitEvents>,
    base_user_prompt_str: &str,
    capsule_json_str: &str,
    budget_tokens: usize,
) -> Vec<TopForegroundWaitEvents> {
    // Sort by importance (same as before).
    v.sort_by(|a, b| b.avg_pct_of_dbtime.partial_cmp(&a.avg_pct_of_dbtime).unwrap_or(std::cmp::Ordering::Equal));

    // Step A: for each event, trim MAD list to the maximum that still *could* fit reasonably.
    // We do a local per-event trim using a mini-budget approach:
    // - Build JSON with only this one event
    // - Find max MAD prefix that fits inside budget when combined with prompt+capsule.
    for e in &mut v {
        e.median_absolute_deviation_anomalies
            .sort_by(|a, b| b.mad_score.partial_cmp(&a.mad_score).unwrap_or(std::cmp::Ordering::Equal));

        let mad = &e.median_absolute_deviation_anomalies;

        // Find max MAD anomalies for THIS event under overall budget.
        // Wrap shape matches what your section sends.
        let k = max_prefix_that_fits(
            mad,
            base_user_prompt_str,
            capsule_json_str,
            budget_tokens,
            |slice| json!({
                "top_foreground_wait_events": [{
                    "event_name": e.event_name,
                    "correlation_with_db_time": e.correlation_with_db_time,
                    "marked_as_top_in_pct_of_probes": e.marked_as_top_in_pct_of_probes,
                    "avg_pct_of_dbtime": e.avg_pct_of_dbtime,
                    "stddev_pct_of_db_time": e.stddev_pct_of_db_time,
                    "avg_wait_time_s": e.avg_wait_time_s,
                    "stddev_wait_time_s": e.stddev_wait_time_s,
                    "avg_number_of_executions": e.avg_number_of_executions,
                    "stddev_number_of_executions": e.stddev_number_of_executions,
                    "avg_wait_for_execution_ms": e.avg_wait_for_execution_ms,
                    "stddev_wait_for_execution_ms": e.stddev_wait_for_execution_ms,
                    "median_absolute_deviation_anomalies": slice
                }]
            })
        );

        e.median_absolute_deviation_anomalies.truncate(k);
    }

    // Step B: now pick maximum number of events (prefix) that fits.
    let n = max_prefix_that_fits(
        &v,
        base_user_prompt_str,
        capsule_json_str,
        budget_tokens,
        |slice| json!({ "top_foreground_wait_events": slice })
    );

    v.truncate(n);
    v
}

fn trim_background_waits(mut v: Vec<TopBackgroundWaitEvents>, waits_top_n: usize, mad_top_n: usize) -> Vec<TopBackgroundWaitEvents> {
    v.sort_by(|a, b| b.avg_pct_of_dbtime.partial_cmp(&a.avg_pct_of_dbtime).unwrap_or(Ordering::Equal));
    v.truncate(waits_top_n);

    for e in &mut v {
        e.median_absolute_deviation_anomalies
            .sort_by(|a, b| b.mad_score.partial_cmp(&a.mad_score).unwrap_or(Ordering::Equal));
        e.median_absolute_deviation_anomalies.truncate(mad_top_n);
    }
    v
}

fn trim_background_waits_by_budget(
    mut v: Vec<TopBackgroundWaitEvents>,
    base_user_prompt_str: &str,
    capsule_json_str: &str,
    budget_tokens: usize,
) -> Vec<TopBackgroundWaitEvents> {
    v.sort_by(|a, b| b.avg_pct_of_dbtime.partial_cmp(&a.avg_pct_of_dbtime).unwrap_or(std::cmp::Ordering::Equal));

    for e in &mut v {
        e.median_absolute_deviation_anomalies
            .sort_by(|a, b| b.mad_score.partial_cmp(&a.mad_score).unwrap_or(std::cmp::Ordering::Equal));

        let mad = &e.median_absolute_deviation_anomalies;

        let k = max_prefix_that_fits(
            mad,
            base_user_prompt_str,
            capsule_json_str,
            budget_tokens,
            |slice| json!({
                "top_background_wait_events": [{
                    "event_name": e.event_name,
                    "correlation_with_db_time": e.correlation_with_db_time,
                    "marked_as_top_in_pct_of_probes": e.marked_as_top_in_pct_of_probes,
                    "avg_pct_of_dbtime": e.avg_pct_of_dbtime,
                    "stddev_pct_of_db_time": e.stddev_pct_of_db_time,
                    "avg_wait_time_s": e.avg_wait_time_s,
                    "stddev_wait_time_s": e.stddev_wait_time_s,
                    "avg_number_of_executions": e.avg_number_of_executions,
                    "stddev_number_of_executions": e.stddev_number_of_executions,
                    "avg_wait_for_execution_ms": e.avg_wait_for_execution_ms,
                    "stddev_wait_for_execution_ms": e.stddev_wait_for_execution_ms,
                    "median_absolute_deviation_anomalies": slice
                }]
            })
        );

        e.median_absolute_deviation_anomalies.truncate(k);
    }

    let n = max_prefix_that_fits(
        &v,
        base_user_prompt_str,
        capsule_json_str,
        budget_tokens,
        |slice| json!({ "top_background_wait_events": slice })
    );

    v.truncate(n);
    v
}

/// Keep only top N SQLs by avg_elapsed_time_cumulative_s and top MAD anomalies per SQL.
fn trim_sqls(mut v: Vec<TopSQLsByElapsedTime>, sqls_top_n: usize, mad_top_n: usize) -> Vec<TopSQLsByElapsedTime> {
    v.sort_by(|a, b| b.avg_elapsed_time_cumulative_s.partial_cmp(&a.avg_elapsed_time_cumulative_s).unwrap_or(Ordering::Equal));
    v.truncate(sqls_top_n);

    for s in &mut v {
        s.median_absolute_deviation_anomalies
            .sort_by(|a, b| b.mad_score.partial_cmp(&a.mad_score).unwrap_or(Ordering::Equal));
        s.median_absolute_deviation_anomalies.truncate(mad_top_n);
    }
    v
}

fn trim_sqls_by_budget(
    mut v: Vec<TopSQLsByElapsedTime>,
    base_user_prompt_str: &str,
    capsule_json_str: &str,
    budget_tokens: usize,
) -> Vec<TopSQLsByElapsedTime> {
    v.sort_by(|a, b| b.avg_elapsed_time_cumulative_s.partial_cmp(&a.avg_elapsed_time_cumulative_s).unwrap_or(std::cmp::Ordering::Equal));

    for s in &mut v {
        s.median_absolute_deviation_anomalies
            .sort_by(|a, b| b.mad_score.partial_cmp(&a.mad_score).unwrap_or(std::cmp::Ordering::Equal));

        let mad = &s.median_absolute_deviation_anomalies;

        let k = max_prefix_that_fits(
            mad,
            base_user_prompt_str,
            capsule_json_str,
            budget_tokens,
            |slice| json!({
                "top_sqls_by_elapsed_time": [{
                    "sql_id": s.sql_id,
                    "module": s.module,
                    "sql_type": s.sql_type,
                    "pct_of_time_sql_was_found_in_other_top_sections": s.pct_of_time_sql_was_found_in_other_top_sections,
                    "correlation_with_db_time": s.correlation_with_db_time,
                    "marked_as_top_in_pct_of_probes": s.marked_as_top_in_pct_of_probes,
                    "avg_elapsed_time_by_exec": s.avg_elapsed_time_by_exec,
                    "stddev_elapsed_time_by_exec": s.stddev_elapsed_time_by_exec,
                    "avg_cpu_time_by_exec": s.avg_cpu_time_by_exec,
                    "stddev_cpu_time_by_exec": s.stddev_cpu_time_by_exec,
                    "avg_elapsed_time_cumulative_s": s.avg_elapsed_time_cumulative_s,
                    "stddev_elapsed_time_cumulative_s": s.stddev_elapsed_time_cumulative_s,
                    "avg_cpu_time_cumulative_s": s.avg_cpu_time_cumulative_s,
                    "stddev_cpu_time_cumulative_s": s.stddev_cpu_time_cumulative_s,
                    "avg_number_of_executions": s.avg_number_of_executions,
                    "stddev_number_of_executions": s.stddev_number_of_executions,
                    "median_absolute_deviation_anomalies": slice,
                    "wait_events_with_strong_pearson_correlation": s.wait_events_with_strong_pearson_correlation,
                    "wait_events_found_in_ash_sections_for_this_sql": s.wait_events_found_in_ash_sections_for_this_sql
                }]
            })
        );

        s.median_absolute_deviation_anomalies.truncate(k);
    }

    let n = max_prefix_that_fits(
        &v,
        base_user_prompt_str,
        capsule_json_str,
        budget_tokens,
        |slice| json!({ "top_sqls_by_elapsed_time": slice })
    );

    v.truncate(n);
    v
}

/// Keep top N load profile anomalies by mad_score.
fn trim_load_profile_anomalies(mut v: Vec<LoadProfileAnomalies>, top_n: usize) -> Vec<LoadProfileAnomalies> {
    v.sort_by(|a, b| b.mad_score.partial_cmp(&a.mad_score).unwrap_or(Ordering::Equal));
    v.truncate(top_n);
    v
}

fn trim_load_profile_anomalies_by_budget(
    mut v: Vec<LoadProfileAnomalies>,
    base_user_prompt_str: &str,
    capsule_json_str: &str,
    budget_tokens: usize,
) -> Vec<LoadProfileAnomalies> {
    v.sort_by(|a, b| b.mad_score.partial_cmp(&a.mad_score).unwrap_or(std::cmp::Ordering::Equal));

    let n = max_prefix_that_fits(
        &v,
        base_user_prompt_str,
        capsule_json_str,
        budget_tokens,
        |slice| json!({ "load_profile_anomalies": slice })
    );

    v.truncate(n);
    v
}

fn trim_anomaly_clusters(
    mut clusters: Vec<AnomlyCluster>,
    top_clusters: usize
) -> Vec<AnomlyCluster> {
    // Sort by declared number_of_anomalies (descending).
    clusters.sort_by(|a, b| b.number_of_anomalies.cmp(&a.number_of_anomalies));

    // Keep only top N clusters.
    clusters.truncate(top_clusters);

    clusters
}

/// Selects as many clusters as possible under a token budget.
/// Does NOT truncate anomalies inside a cluster.
fn trim_clusters_by_token_budget(
    mut clusters: Vec<AnomlyCluster>,
    capsule_json_str: &str,        // already serialized capsule (small but included)
    base_user_prompt_str: &str,    // the user prompt template for the section (without INPUT json)
    budget_tokens: usize,          // max tokens we allow for this step input (rough)
) -> Vec<AnomlyCluster> {

    // 1) Sort clusters by importance (here: number_of_anomalies desc)
    clusters.sort_by(|a, b| b.number_of_anomalies.cmp(&a.number_of_anomalies));

    // 2) Fast path: if empty or budget absurdly small
    if clusters.is_empty() || budget_tokens < 256 {
        return Vec::new();
    }

    // Helper: estimate tokens for "capsule + prompt + input_json"
    let fits = |k: usize| -> bool {
        let slice = &clusters[..k];
        let input_json = serde_json::json!({ "anomaly_clusters": slice });
        let input_str = serde_json::to_string(&input_json).unwrap_or_default();

        // Estimate tokens for what we actually send (prompt text + capsule + input JSON)
        let combined = format!(
            "{}\n{}\nINPUT:\n{}",
            base_user_prompt_str,
            capsule_json_str,
            input_str
        );

        estimate_tokens_from_str(&combined) <= budget_tokens
    };

    // 3) If even 1 cluster doesn't fit, return empty.
    if !fits(1) {
        return Vec::new();
    }

    // 4) Binary search for maximum k that still fits.
    let mut lo = 1usize;
    let mut hi = clusters.len();

    while lo < hi {
        let mid = (lo + hi + 1) / 2; // upper mid
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    clusters.truncate(lo);
    clusters
}

/// =====================
/// Context capsule
/// =====================

#[derive(Debug, Clone, Serialize)]
struct ContextCapsule<'a> {
    general_data: &'a StatisticsDescription,
    top_spikes_marked_top_n: Vec<&'a TopPeaksSelected>,
}

fn build_capsule<'a>(report: &'a ReportForAI, top_n: usize) -> ContextCapsule<'a> {
    let mut spikes: Vec<&TopPeaksSelected> = report.top_spikes_marked.iter().collect();
    spikes.sort_by(|a, b| b.db_time_value.partial_cmp(&a.db_time_value).unwrap_or(Ordering::Equal));
    spikes.truncate(top_n);

    ContextCapsule {
        general_data: &report.general_data,
        top_spikes_marked_top_n: spikes,
    }
}

fn section_name(section: Section) -> &'static str {
    match section {
        Section::Baseline => "baseline",
        Section::WaitEventsForeground => "wait_events_foreground",
        Section::WaitEventsBackground => "wait_events_background",
        Section::SqlElapsedTime => "sql_elapsed_time",
        Section::IoStatsByFunction => "io_stats_by_function",
        Section::Latches => "latches",
        Section::SegmentsRowLockWaits => "segments_row_lock_waits",
        Section::SegmentsPhysicalWrites => "segments_physical_writes",
        Section::SegmentsPhysicalWriteRequests => "segments_physical_write_requests",
        Section::SegmentsPhysicalReadRequests => "segments_physical_read_requests",
        Section::SegmentsLogicalReads => "segments_logical_reads",
        Section::SegmentsDirectPhysicalWrites => "segments_direct_physical_writes",
        Section::SegmentsDirectPhysicalReads => "segments_direct_physical_reads",
        Section::SegmentsBufferBusyWaits => "segments_buffer_busy_waits",
        Section::InstanceStatsCorrelation => "instance_stats_correlation",
        Section::LoadProfileAnomalies => "load_profile_anomalies",
        Section::AnomalyClusters => "anomaly_clusters",
        Section::ComposeFinal => "compose_final",
        Section::GradientWaitEvents => "db_time_gradient_fg_wait_events",
        Section::GradientStatsCounters => "db_time_gradient_instance_stats_counters",
        Section::GradientStatsVolume => "db_time_gradient_instance_stats_volumes",
        Section::GradientStatsTime => "db_time_gradient_instance_stats_time",
        Section::GradientSQLs => "db_time_gradient_sql_elapsed_time"
    }
}


/// =====================
/// Main modular pipeline
/// =====================
#[tokio::main]
pub async fn analyze_report_modular_lmstudio(
    report: &ReportForAI,
    cfg: &ModularLlmConfig,
    default_model_hint: &str,
) -> Result<(Vec<(Section, String)>, String), Box<dyn std::error::Error>> {

    // Build client from env, using the configured max tokens/temperature.
    let client = ChatClient::new(cfg.use_openrouter, default_model_hint, cfg.temperature, cfg.max_tokens_per_call)?;

    // Full system prompt with all descriptions preserved.
    let system = format!(
        "{}{}",
        system_master_prompt(&cfg.lang),
        reasoning_mode_block(cfg.enable_reasoning_prompt),
    );

    // Build a small context capsule used by most steps.
    let capsule = build_capsule(report, cfg.top_spikes_n);
    let capsule_json = to_pretty_json(&capsule);

    // Helper to run one section and parse SectionNotes JSON.
    async fn run_section(
        client: &ChatClient,
        system: &str,
        section: Section,
        capsule_json: &str,
        input_json: Value,
    ) -> Result<String, Box<dyn std::error::Error>> {

        //let input_pretty = serde_json::to_string_pretty(&input_json).unwrap_or_else(|_| "{}".to_string());
        let input_pretty = encode(&input_json, None); //TOON string for minimizing tokens

        let user = user_prompt_for_section(section, capsule_json, &input_pretty);        
        println!("Analyzing secion: {:?}", section);

        let (tx, rx) = oneshot::channel();
        let spinner = tokio::spawn(spinning_beer(rx));
        
        let raw = client.chat_content(system, &user).await?;
        // Sanitize (strip think/code fences, enforce header)
        let md = sanitize_markdown(section, &raw);

        // Persist section output for debugging / audit trail.
        //let output_prefix = "lmstudio";
        //let path = format!("{}.section.{}.md", output_prefix, section_name(section));
        //fs::write(&path, md.as_bytes())?;
        
        let _ = tx.send(()); //stop spinner
        let _ = spinner.await;
        
        Ok(md)

    }

    // 0) Baseline (general_data + full spikes; spikes are not huge typically)
    let baseline_input = json!({
        "general_data": report.general_data,
        "top_spikes_marked": report.top_spikes_marked
    });
    let baseline_notes = run_section(&client, &system, Section::Baseline, "", baseline_input).await?;

    let mut notes: Vec<(Section, String)> = vec![(Section::Baseline, baseline_notes)];

    // 1) Foreground waits (trimmed)
    let base_user_prompt_str = user_prompt_for_section(Section::WaitEventsForeground, &capsule_json, "{}");  
    let fg_waits = trim_foreground_waits_by_budget(
        report.top_foreground_wait_events.clone(),
        &base_user_prompt_str,
        &capsule_json,
        cfg.tokens_budget,
    );

    notes.push((Section::WaitEventsForeground,run_section(
        &client, &system, Section::WaitEventsForeground, &capsule_json,
        json!({ "top_foreground_wait_events": fg_waits })
    ).await?));

    // 2) Background waits (trimmed)
    let base_user_prompt_str = user_prompt_for_section(Section::WaitEventsBackground, &capsule_json, "{}");  
    let bg_waits = trim_background_waits_by_budget(
        report.top_background_wait_events.clone(),
        &base_user_prompt_str,
        &capsule_json,
        cfg.tokens_budget,
    );
    notes.push((Section::WaitEventsBackground,run_section(
        &client, &system, Section::WaitEventsBackground, &capsule_json,
        json!({ "top_background_wait_events": bg_waits })
    ).await?));

    // 3) SQLs (trimmed)
    let base_user_prompt_str = user_prompt_for_section(Section::SqlElapsedTime, &capsule_json, "{}");  
    let sqls = trim_sqls_by_budget(
        report.top_sqls_by_elapsed_time.clone(),
        &base_user_prompt_str,
        &capsule_json,
        cfg.tokens_budget,
    );
    notes.push((Section::SqlElapsedTime,run_section(
        &client, &system, Section::SqlElapsedTime, &capsule_json,
        json!({ "top_sqls_by_elapsed_time": sqls })
    ).await?));

    // 4) IO stats (usually not enormous; no trimming by default)
    notes.push((Section::IoStatsByFunction,run_section(
        &client, &system, Section::IoStatsByFunction, &capsule_json,
        json!({ "io_stats_by_function_summary": report.io_stats_by_function_summary })
    ).await?));

    // 5) Latches
    notes.push((Section::Latches,run_section(
        &client, &system, Section::Latches, &capsule_json,
        json!({ "latch_activity_summary": report.latch_activity_summary })
    ).await?));

    // 6) Segments (these are "top 10" each, so usually safe)
    notes.push((Section::SegmentsRowLockWaits,run_section(&client, &system, Section::SegmentsRowLockWaits, &capsule_json,
        json!({ "top_10_segments_by_row_lock_waits": report.top_10_segments_by_row_lock_waits })
    ).await?));

    notes.push((Section::SegmentsPhysicalWrites,run_section(&client, &system, Section::SegmentsPhysicalWrites, &capsule_json,
        json!({ "top_10_segments_by_physical_writes": report.top_10_segments_by_physical_writes })
    ).await?));

    notes.push((Section::SegmentsPhysicalWriteRequests,run_section(&client, &system, Section::SegmentsPhysicalWriteRequests, &capsule_json,
        json!({ "top_10_segments_by_physical_write_requests": report.top_10_segments_by_physical_write_requests })
    ).await?));

    notes.push((Section::SegmentsPhysicalReadRequests,run_section(&client, &system, Section::SegmentsPhysicalReadRequests, &capsule_json,
        json!({ "top_10_segments_by_physical_read_requests": report.top_10_segments_by_physical_read_requests })
    ).await?));

    notes.push((Section::SegmentsLogicalReads,run_section(&client, &system, Section::SegmentsLogicalReads, &capsule_json,
        json!({ "top_10_segments_by_logical_reads": report.top_10_segments_by_logical_reads })
    ).await?));

    notes.push((Section::SegmentsDirectPhysicalWrites,run_section(&client, &system, Section::SegmentsDirectPhysicalWrites, &capsule_json,
        json!({ "top_10_segments_by_direct_physical_writes": report.top_10_segments_by_direct_physical_writes })
    ).await?));

    notes.push((Section::SegmentsDirectPhysicalReads,run_section(&client, &system, Section::SegmentsDirectPhysicalReads, &capsule_json,
        json!({ "top_10_segments_by_direct_physical_reads": report.top_10_segments_by_direct_physical_reads })
    ).await?));

    notes.push((Section::SegmentsBufferBusyWaits,run_section(&client, &system, Section::SegmentsBufferBusyWaits, &capsule_json,
        json!({ "top_10_segments_by_buffer_busy_waits": report.top_10_segments_by_buffer_busy_waits })
    ).await?));

    // 7) Instance correlation
    notes.push((Section::InstanceStatsCorrelation,run_section(
        &client, &system, Section::InstanceStatsCorrelation, &capsule_json,
        json!({ "instance_stats_pearson_correlation": report.instance_stats_pearson_correlation })
    ).await?));

    // 8) Load profile anomalies (trimmed)
    let base_user_prompt_str = user_prompt_for_section(Section::LoadProfileAnomalies, &capsule_json, "{}");  
    let lp_anoms = trim_load_profile_anomalies_by_budget(
        report.load_profile_anomalies.clone(),
        &base_user_prompt_str,
        &capsule_json,
        cfg.tokens_budget,
    );
    notes.push((Section::LoadProfileAnomalies,run_section(
        &client, &system, Section::LoadProfileAnomalies, &capsule_json,
        json!({ "load_profile_anomalies": lp_anoms })
    ).await?));

    // 9) Anomaly clusters
    let section_budget_tokens = cfg.tokens_budget; 

    // You already have capsule_json as string.
    let capsule_json_str = &capsule_json;

    // Build a base prompt string for this section WITHOUT embedding the huge JSON yet.
    // Keep it stable, so the estimator matches what you send.
    let base_user_prompt_str = r#"Section: anomaly_clusters

    CONTEXT CAPSULE (timeline anchor):
    "#;

    // Now trim clusters by budget (no truncation inside clusters).
    let clusters_trimmed = trim_clusters_by_token_budget(
        report.anomaly_clusters.clone(),
        capsule_json_str,
        base_user_prompt_str,
        section_budget_tokens,
    );

    // Send only those clusters.
    notes.push((Section::AnomalyClusters,run_section(
        &client, &system, Section::AnomalyClusters, capsule_json_str,
        serde_json::json!({ "anomaly_clusters": clusters_trimmed })
    ).await?));

    //Add gradient sections
    notes.push((Section::GradientWaitEvents,run_section(
        &client, &system, Section::GradientWaitEvents, capsule_json_str,
        serde_json::json!({ "db_time_gradient_fg_wait_events": report.db_time_gradient_fg_wait_events })
    ).await?));

    notes.push((Section::GradientStatsCounters,run_section(
        &client, &system, Section::GradientStatsCounters, capsule_json_str,
        serde_json::json!({ "db_time_gradient_instance_stats_counters": report.db_time_gradient_instance_stats_counters })
    ).await?));

    notes.push((Section::GradientStatsVolume,run_section(
        &client, &system, Section::GradientStatsVolume, capsule_json_str,
        serde_json::json!({ "db_time_gradient_instance_stats_volumes": report.db_time_gradient_instance_stats_volumes })
    ).await?));

    notes.push((Section::GradientStatsTime,run_section(
        &client, &system, Section::GradientStatsTime, capsule_json_str,
        serde_json::json!({ "db_time_gradient_instance_stats_time": report.db_time_gradient_instance_stats_time })
    ).await?));

    notes.push((Section::GradientSQLs,run_section(
        &client, &system, Section::GradientSQLs, capsule_json_str,
        serde_json::json!({ "db_time_gradient_sql_elapsed_time": report.db_time_gradient_sql_elapsed_time })
    ).await?));

   // Build a single markdown bundle for the composer step.
    let mut bundle = String::new();
    for (sec, md) in &notes {
        bundle.push_str("\n\n============================================================\n");
        bundle.push_str(&format!("===BEGIN_SECTION name={}===\n", section_name(*sec)));
        bundle.push_str(md);
        bundle.push_str("\n===END_SECTION===\n");
    }

    // Compose final report
    println!("Composing final report...");
    let composer_system = system_master_prompt(&cfg.lang);
    let composer_user = user_prompt_for_section(
        Section::ComposeFinal,
        "",
        &bundle
    );

    let final_raw = client.chat_content(&composer_system, &composer_user).await?;
    let final_md = strip_code_fences(&strip_think_blocks(&final_raw));

    Ok((notes, final_md))
}

/// Writes outputs to files:
/// - <base_name>.section_notes.json
/// - <base_name>.final.md
pub fn write_outputs(base_name: &str, final_md: &str) -> Result<(), Box<dyn std::error::Error>> {
    let notes_path = format!("{base_name}.section_notes.json");
    let md_path = format!("{base_name}.final.md");

    fs::write(&md_path, final_md.as_bytes())?;

    Ok(())
}