use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{cmp::Ordering, env, fs};
use reqwest::Client;
use tokio::sync::oneshot;
use crate::{debug_note, tools::*};
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

        debug_note!("Local client url: {} and key with len {} chars", base_url, api_key.len());

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

        debug_note!("Payload for local client created - asking for response from url {}", url);

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

        debug_note!("Response status: {}", status);

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

        debug_note!("OpenRouter client with key len = {} chars", api_key.len());

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

        debug_note!("Payload for OpenRouter client created - asking for response from url {}", url);

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

        debug_note!("Response status: {}", status);

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
r#"# ROLE & IDENTITY

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
  correlations, averages, stddevs, and MAD anomalies
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

Write answer in language: ", {lang}
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
    debug_note!("Created client object");
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