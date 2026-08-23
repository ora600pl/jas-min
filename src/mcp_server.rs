//! Streamable HTTP MCP server for interactive JAS-MIN investigations.
//!
//! The server deliberately keeps Oracle measurements, diagnostic guidance and
//! report state separate. Measurements receive evidence IDs, guidance receives
//! methodology references, and the report builder accepts only references that
//! were observed in the same explicit analysis session.

use crate::ai_tools::{dispatch_tool_call_value, tools_schema};
use crate::awr::AWRSCollection;
use crate::local_agent::{build_case_seed, dispatch_precomputed_analysis, GuidanceLibrary};
use crate::reasonings::{CrossModelClassification, DbTimeGradientSection, ReportForAI};
use crate::tools::{get_safe_filename, render_markdown_html_document};
use anyhow::{bail, Context, Result};
use dashmap::DashMap;
use html_escape::encode_text;
use regex::Regex;
use rmcp::{
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult,
        GetPromptRequestParams, GetPromptResponse, GetPromptResult, Implementation,
        ListPromptsResult, ListToolsResult, Prompt, PromptArgument, PromptMessage, ProtocolVersion,
        Role, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    },
    service::{RequestContext, RoleServer},
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData as McpError, ServerHandler,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{File, OpenOptions},
    future::Future,
    io::{BufRead, BufReader, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
use tokio_util::sync::CancellationToken;

const MCP_ANALYSIS_SCHEMA_VERSION: &str = "2026-08-23.3";
const SEED_EVIDENCE_ID: &str = "SEED-E0001";
const DEFAULT_GUIDANCE_LIMIT_CHARS: usize = 8 * 1024;
const MAX_MCP_MARKDOWN_BYTES: usize = 4 * 1024 * 1024;
const MAX_MCP_HTML_FILENAME_BYTES: usize = 200;
const MAX_MCP_LOG_FIELD_CHARS: usize = 160;
const MCP_TOOLS_LIST_TTL_MS: u64 = 300_000;
pub const MAX_MCP_PROJECTS: usize = 32;

static MCP_TOOL_CALL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// One tool-call lifecycle record written to the terminal.
///
/// Only bounded metadata is retained here. Tool arguments and result bodies can
/// contain SQL text, object names, host diagnostics, or a complete report, so
/// they must never be copied into operational logs.
struct McpToolCallLog {
    call_id: u64,
    rpc_id: String,
    tool: String,
    analysis_id: Option<String>,
    request_bytes: usize,
    started_at: Instant,
    finished: bool,
}

impl McpToolCallLog {
    fn start(
        rpc_id: String,
        tool: String,
        analysis_id: Option<String>,
        request_bytes: usize,
    ) -> Self {
        let call = Self {
            call_id: MCP_TOOL_CALL_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            rpc_id,
            tool,
            analysis_id,
            request_bytes,
            started_at: Instant::now(),
            finished: false,
        };
        call.write("START", None, None, None);
        call
    }

    fn succeed(&mut self, response_bytes: usize) {
        self.finished = true;
        self.write(
            "OK",
            Some(self.started_at.elapsed().as_millis()),
            Some(response_bytes),
            None,
        );
    }

    fn fail(&mut self, response_bytes: usize, error_code: &str) {
        self.finished = true;
        self.write(
            "ERROR",
            Some(self.started_at.elapsed().as_millis()),
            Some(response_bytes),
            Some(error_code),
        );
    }

    fn write(
        &self,
        status: &str,
        duration_ms: Option<u128>,
        response_bytes: Option<usize>,
        error_code: Option<&str>,
    ) {
        eprintln!(
            "{}",
            format_mcp_tool_log_line(
                &mcp_log_timestamp(),
                self.call_id,
                &self.rpc_id,
                &self.tool,
                self.analysis_id.as_deref(),
                status,
                self.request_bytes,
                duration_ms,
                response_bytes,
                error_code,
            )
        );
    }
}

impl Drop for McpToolCallLog {
    fn drop(&mut self) {
        if !self.finished {
            self.write(
                "ABORTED",
                Some(self.started_at.elapsed().as_millis()),
                None,
                Some("CALL_DID_NOT_COMPLETE"),
            );
        }
    }
}

fn mcp_log_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn bounded_log_field(value: &str) -> String {
    let mut chars = value.chars();
    let mut bounded = chars
        .by_ref()
        .take(MAX_MCP_LOG_FIELD_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        bounded.push_str("...");
    }
    serde_json::to_string(&bounded).unwrap_or_else(|_| "\"<unavailable>\"".to_string())
}

#[allow(clippy::too_many_arguments)]
fn format_mcp_tool_log_line(
    timestamp: &str,
    call_id: u64,
    rpc_id: &str,
    tool: &str,
    analysis_id: Option<&str>,
    status: &str,
    request_bytes: usize,
    duration_ms: Option<u128>,
    response_bytes: Option<usize>,
    error_code: Option<&str>,
) -> String {
    let analysis_id = analysis_id
        .map(bounded_log_field)
        .unwrap_or_else(|| "null".to_string());
    let duration_ms = duration_ms
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string());
    let response_bytes = response_bytes
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string());
    let error_code = error_code
        .map(bounded_log_field)
        .unwrap_or_else(|| "null".to_string());
    format!(
        "{timestamp} [MCP] status={status} call_id={call_id} rpc_id={} tool={} analysis_id={analysis_id} request_bytes={request_bytes} response_bytes={response_bytes} duration_ms={duration_ms} error_code={error_code}",
        bounded_log_field(rpc_id),
        bounded_log_field(tool),
    )
}

fn serialized_json_size<T: Serialize + ?Sized>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(0, |encoded| encoded.len())
}

const STABLE_MARKDOWN_HEADINGS: &[&str] = &[
    "## 1. Executive Summary",
    "## 2. Overall Performance Profile and DB Time Degradation",
    "## 3. Wait Events",
    "## 4. SQL-Level Analysis",
    "## 5. Segments and Objects",
    "## 6. Latches and Internal Contention",
    "## 7. I/O and Disk Assessment",
    "## 8. UNDO, Redo and Load Profile",
    "## 9. Gradient and Anomaly Synthesis",
    "## 10. Relevant Initialization Parameters",
    "## 11. Prioritized Actions and Mandatory Assessments",
];

const REQUIRED_REPORT_CATEGORIES: &[&str] = &[
    "performance_profile",
    "wait_events",
    "sql",
    "segments",
    "latches",
    "io",
    "undo_redo",
    "gradients_anomalies",
    "parameters",
];

const REQUIRED_STRUCTURED_TABLE_KINDS: &[&str] = &[];

const REQUIRED_PRECOMPUTED_SECTIONS: &[&str] = &[
    "db_time_degradation",
    "foreground_waits",
    "background_waits",
    "top_sqls",
    "segment_hotspots",
    "latches",
    "io_summary",
    "full_gradients",
    "load_profile_anomalies",
    "anomaly_clusters",
];

const PERFORMANCE_PARAMETER_CHECKLIST: &[&str] = &[
    "cpu_count",
    "resource_manager_plan",
    "cluster_database",
    "cluster_interconnects",
    "instance_number",
    "remote_listener",
    "sga_target",
    "sga_max_size",
    "shared_pool_size",
    "db_cache_size",
    "pga_aggregate_target",
    "pga_aggregate_limit",
    "memory_target",
    "memory_max_target",
    "processes",
    "sessions",
    "open_cursors",
    "session_cached_cursors",
    "cursor_sharing",
    "optimizer_mode",
    "optimizer_features_enable",
    "optimizer_dynamic_sampling",
    "optimizer_adaptive_plans",
    "optimizer_adaptive_statistics",
    "parallel_degree_policy",
    "parallel_servers_target",
    "parallel_max_servers",
    "parallel_min_servers",
    "workarea_size_policy",
    "undo_retention",
    "filesystemio_options",
    "db_writer_processes",
    "log_buffer",
    "result_cache_max_size",
    "statistics_level",
    "use_large_pages",
];

const REQUIRED_ASSESSMENTS: &[&str] = &[
    "disk_quality",
    "application_design",
    "commit_policy",
    "cpu_pressure",
    "parameter_hygiene",
];

const REPORT_CATEGORIES: &[&str] = &[
    "performance_profile",
    "wait_events",
    "sql",
    "segments",
    "latches",
    "io",
    "undo_redo",
    "gradients_anomalies",
    "parameters",
    "limitations",
];

/// Parsed form of `--mcp ADDRESS/PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpEndpoint {
    pub address: SocketAddr,
    pub path: String,
}

impl McpEndpoint {
    pub fn url(&self) -> String {
        format!("http://{}{}", self.address, self.path)
    }
}

impl FromStr for McpEndpoint {
    type Err = String;

    fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("MCP endpoint cannot be empty".to_string());
        }
        let raw = raw.strip_prefix("http://").unwrap_or(raw);
        if raw.starts_with("https://") {
            return Err("JAS-MIN MCP provides local HTTP; terminate TLS in a trusted proxy".into());
        }
        let (authority, path) = raw
            .split_once('/')
            .map(|(authority, path)| (authority, format!("/{path}")))
            .unwrap_or((raw, "/mcp".to_string()));
        let address = authority
            .parse::<SocketAddr>()
            .map_err(|error| format!("invalid MCP socket address '{authority}': {error}"))?;
        if !address.ip().is_loopback() {
            return Err(
                "JAS-MIN MCP is loopback-only because parsed reports may contain sensitive SQL and object names"
                    .into(),
            );
        }
        if path == "/" || path.contains("..") || path.chars().any(char::is_whitespace) {
            return Err(format!("invalid MCP endpoint path '{path}'"));
        }
        Ok(Self { address, path })
    }
}

#[derive(Debug, Clone, Serialize)]
struct EvidenceRecord {
    evidence_id: String,
    tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    arguments: Value,
    result: Value,
}

#[derive(Debug, Clone, Serialize)]
struct GuidanceRecord {
    title: String,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct GuidanceQuotation {
    guidance_ref: String,
    quote: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReportConfig {
    output_format: String,
    language: String,
    audience: String,
    detail_level: String,
    detail_overrides: BTreeMap<String, String>,
    include_evidence_appendix: bool,
    include_guidance_appendix: bool,
}

impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            output_format: "both".to_string(),
            language: "EN".to_string(),
            audience: "mixed".to_string(),
            detail_level: "standard".to_string(),
            detail_overrides: BTreeMap::new(),
            // Machine-oriented provenance remains available in structured JSON.
            // Human reports opt in to technical appendices explicitly.
            include_evidence_appendix: false,
            include_guidance_appendix: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct Recommendation {
    owner: String,
    priority: String,
    action: String,
    rationale: String,
    success_criterion: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReportFinding {
    finding_id: String,
    category: String,
    title: String,
    severity: String,
    confidence: String,
    conclusion: String,
    mechanism: String,
    temporal_pattern: String,
    affected_workload: String,
    evidence_limitations: String,
    evidence_summary: String,
    details: String,
    evidence_refs: Vec<String>,
    guidance_refs: Vec<String>,
    guidance_quotes: Vec<GuidanceQuotation>,
    recommendations: Vec<Recommendation>,
}

#[derive(Debug, Clone, Serialize)]
struct ReportAssessment {
    assessment: String,
    status: String,
    conclusion: String,
    evidence_summary: String,
    evidence_refs: Vec<String>,
    guidance_refs: Vec<String>,
    guidance_quotes: Vec<GuidanceQuotation>,
}

#[derive(Debug, Clone, Serialize)]
struct ReportTableRow {
    cells: BTreeMap<String, String>,
    evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ReportTable {
    table_id: String,
    kind: String,
    category: String,
    title: String,
    rows: Vec<ReportTableRow>,
}

const GRADIENT_TABLE_COLUMNS: &[(&str, &str)] = &[
    ("project_id", "Project"),
    ("analysis_family", "Signal family"),
    ("target_metric", "Target metric"),
    ("contributor", "Contributor"),
    ("selection_status", "Gradient coverage"),
    ("method", "Gradient model"),
    ("typical_impact", "Typical impact"),
    ("peak_impact", "Peak impact"),
    ("classification", "Classification"),
    ("corroboration", "Corroboration"),
    ("interpretation", "Interpretation"),
    ("action", "Action"),
];

const ANOMALY_TABLE_COLUMNS: &[(&str, &str)] = &[
    ("project_id", "Project"),
    ("metric", "Metric / event / SQL"),
    ("time_scope", "Anomalous window"),
    ("observed_value", "Observed value"),
    ("baseline", "Baseline"),
    ("deviation", "Deviation / score"),
    ("corroboration", "Corroboration"),
    ("interpretation", "Interpretation"),
    ("action", "Action"),
];

const ANOMALY_CLUSTER_TABLE_COLUMNS: &[(&str, &str)] = &[
    ("project_id", "Project"),
    ("cluster_id", "Cluster"),
    ("time_scope", "Time scope"),
    ("members", "Signals in cluster"),
    ("common_context", "Shared context"),
    ("severity", "Severity"),
    ("interpretation", "Interpretation"),
    ("action", "Action"),
];

const ANALYTIC_SYNTHESIS_TABLE_COLUMNS: &[(&str, &str)] = &[
    ("project_id", "Project"),
    ("entity", "Repeated entity"),
    ("signal_families", "Independent signal families"),
    ("shared_evidence", "Cross-signal evidence"),
    ("hypothesis", "Evidence-backed hypothesis"),
    ("confidence", "Confidence"),
    ("recommended_validation", "Next validation"),
];

const EXECUTION_PLAN_TABLE_COLUMNS: &[(&str, &str)] = &[
    ("project_id", "Project"),
    ("sql_id", "SQL ID"),
    ("plan_hash", "Plan hash"),
    ("plan_status", "Plan status"),
    ("selection_reason", "Why this SQL is in the report"),
    ("workload_evidence", "Measured workload impact"),
    ("temporal_evidence", "Coverage and timing"),
    ("comparative_context", "Comparison context"),
    ("key_operations", "Key operations"),
    ("access_and_joins", "Access paths / joins"),
    ("cardinality", "Cardinality / row-source evidence"),
    ("partition_parallelism", "Partitioning / PX"),
    ("risk", "Finding / risk"),
    ("recommendation_type", "Recommendation type"),
    ("recommendation_rationale", "Recommendation rationale"),
    ("scope_limitations", "What the evidence cannot prove"),
    ("action", "Action"),
    ("success_metric", "Success criterion"),
];

const WAIT_SQL_CONTRIBUTOR_TABLE_COLUMNS: &[(&str, &str)] = &[
    ("project_id", "Project"),
    ("wait_event", "Wait event"),
    ("sql_id", "SQL ID"),
    ("evidence_basis", "Relationship evidence"),
    ("pearson_correlation", "Pearson correlation"),
    ("ash_avg_pct_dbtime_in_sql", "ASH avg % DB Time in SQL"),
    ("ash_samples", "ASH samples"),
    ("sql_type", "SQL type"),
    ("module", "Module"),
    ("workload_context", "Measured SQL workload"),
    ("plan_coverage", "Plan coverage"),
    ("interpretation", "Interpretation"),
    ("action", "Action"),
];

const CHILD_CURSOR_TABLE_COLUMNS: &[(&str, &str)] = &[
    ("project_id", "Project"),
    ("sql_id", "SQL ID"),
    ("child_cursors", "Child cursors"),
    ("direct_reasons", "Direct sharing reasons"),
    ("optimizer_bind_context", "Optimizer / bind / NLS context"),
    ("performance_impact", "Performance impact"),
    ("action", "Action"),
];

const SEGMENT_TABLE_COLUMNS: &[(&str, &str)] = &[
    ("project_id", "Project"),
    ("statistic", "Segment statistic"),
    ("segment_name", "Segment / object"),
    ("segment_type", "Type"),
    ("object_id", "Object ID"),
    ("data_object_id", "Data object ID"),
    ("occurrence_pct", "Occurrence %"),
    ("average", "Average"),
    ("stddev", "Stddev"),
    ("interpretation", "Interpretation"),
    ("action", "Action"),
];

const SEGMENT_SYNTHESIS_TABLE_COLUMNS: &[(&str, &str)] = &[
    ("project_id", "Project"),
    ("segment_name", "Segment / object"),
    ("segment_type", "Type"),
    ("object_id", "Object ID"),
    ("data_object_id", "Data object ID"),
    ("statistics", "Corroborating statistics"),
    ("recurrence", "Recurrence / coverage"),
    ("combined_interpretation", "Combined interpretation"),
    ("action", "Action"),
];

const PARAMETER_TABLE_COLUMNS: &[(&str, &str)] = &[
    ("project_id", "Project"),
    ("parameter", "Parameter"),
    ("observed_value", "Observed value"),
    ("rating", "Rating"),
    ("performance_relevance", "Performance relevance"),
    ("finding", "Assessment"),
    ("action", "Action"),
];

const ALERT_LOG_TABLE_COLUMNS: &[(&str, &str)] = &[
    ("project_id", "Project"),
    ("error_code", "Error / warning"),
    ("event_records", "Event records"),
    ("parse_detail_records", "Parsed detail blocks"),
    ("max_reported_count", "Maximum reported counter"),
    ("first_seen", "First seen"),
    ("last_seen", "Last seen"),
    ("affected_sql_ids", "Affected SQL IDs"),
    ("affected_clients", "Users / applications"),
    ("performance_relevance", "Performance relevance"),
    ("action", "Action"),
];

fn report_table_definition(
    kind: &str,
) -> Option<(&'static str, &'static [(&'static str, &'static str)])> {
    match kind {
        "gradients" => Some(("gradients_anomalies", GRADIENT_TABLE_COLUMNS)),
        "anomalies" => Some(("gradients_anomalies", ANOMALY_TABLE_COLUMNS)),
        "anomaly_clusters" => Some(("gradients_anomalies", ANOMALY_CLUSTER_TABLE_COLUMNS)),
        "analytic_signal_synthesis" => {
            Some(("gradients_anomalies", ANALYTIC_SYNTHESIS_TABLE_COLUMNS))
        }
        "execution_plans" => Some(("sql", EXECUTION_PLAN_TABLE_COLUMNS)),
        "wait_sql_contributors" => Some(("wait_events", WAIT_SQL_CONTRIBUTOR_TABLE_COLUMNS)),
        "child_cursors" => Some(("sql", CHILD_CURSOR_TABLE_COLUMNS)),
        "alert_log_errors" => Some(("sql", ALERT_LOG_TABLE_COLUMNS)),
        "segments" => Some(("segments", SEGMENT_TABLE_COLUMNS)),
        "segment_synthesis" => Some(("segments", SEGMENT_SYNTHESIS_TABLE_COLUMNS)),
        "parameters" => Some(("parameters", PARAMETER_TABLE_COLUMNS)),
        _ => None,
    }
}

struct AnalysisSession {
    project_ids: Vec<String>,
    config: ReportConfig,
    evidence: BTreeMap<String, EvidenceRecord>,
    evidence_cache: HashMap<String, String>,
    guidance: BTreeMap<String, GuidanceRecord>,
    findings: BTreeMap<String, ReportFinding>,
    assessments: BTreeMap<String, ReportAssessment>,
    report_tables: BTreeMap<String, ReportTable>,
    next_evidence: u64,
    next_finding: u64,
    next_table: u64,
    report_revision: u64,
    finalized_markdown: Option<String>,
}

impl AnalysisSession {
    fn new(seed: Value, config: ReportConfig, project_ids: Vec<String>) -> Self {
        let seed_record = EvidenceRecord {
            evidence_id: SEED_EVIDENCE_ID.to_string(),
            tool_name: "initial_case_seed".to_string(),
            project_id: None,
            arguments: json!({}),
            result: seed,
        };
        Self {
            project_ids,
            config,
            evidence: BTreeMap::from([(SEED_EVIDENCE_ID.to_string(), seed_record)]),
            evidence_cache: HashMap::new(),
            guidance: BTreeMap::new(),
            findings: BTreeMap::new(),
            assessments: BTreeMap::new(),
            report_tables: BTreeMap::new(),
            next_evidence: 2,
            next_finding: 1,
            next_table: 1,
            report_revision: 0,
            finalized_markdown: None,
        }
    }
}

/// One fully parsed performance project supplied to the MCP runtime.
pub struct AnalysisProject {
    project_id: String,
    collection: AWRSCollection,
    report: ReportForAI,
    stem: String,
    security_level: usize,
    report_links: HashMap<String, HashSet<String>>,
    html_reports_dir: String,
}

impl AnalysisProject {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_id: String,
        collection: AWRSCollection,
        report: ReportForAI,
        stem: String,
        security_level: usize,
        report_links: HashMap<String, HashSet<String>>,
        html_reports_dir: String,
    ) -> Self {
        Self {
            project_id,
            collection,
            report,
            stem,
            security_level,
            report_links,
            html_reports_dir,
        }
    }
}

#[derive(Clone)]
struct ProjectData {
    project_id: Arc<String>,
    collection: Arc<AWRSCollection>,
    report: Arc<ReportForAI>,
    stem: Arc<String>,
    security_level: usize,
    report_links: Arc<HashMap<String, HashSet<String>>>,
    html_reports_dir: Arc<String>,
}

impl From<AnalysisProject> for ProjectData {
    fn from(project: AnalysisProject) -> Self {
        Self {
            project_id: Arc::new(project.project_id),
            collection: Arc::new(project.collection),
            report: Arc::new(project.report),
            stem: Arc::new(project.stem),
            security_level: project.security_level,
            report_links: Arc::new(project.report_links),
            html_reports_dir: Arc::new(project.html_reports_dir),
        }
    }
}

impl ProjectData {
    fn attachment_inventory(
        &self,
        expected_date_from: Option<&str>,
        expected_date_to: Option<&str>,
    ) -> Value {
        let directory = PathBuf::from(format!("{}_attachments", self.stem));
        let aix_directory = directory.join("AIX");
        let alert_log_paths = files_name_contains(&directory, "alert");
        let alert_logs_nonempty = alert_log_paths
            .iter()
            .filter(|path| std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0))
            .count();
        let alert_log_files = alert_log_paths
            .iter()
            .map(|path| {
                let bytes = std::fs::metadata(path)
                    .map(|metadata| metadata.len())
                    .unwrap_or(0);
                let (first_timestamp, last_timestamp, timestamped_lines) =
                    alert_timestamp_bounds(path);
                let observed_date_from = first_timestamp
                    .as_deref()
                    .and_then(|timestamp| timestamp.get(..10));
                let observed_date_to = last_timestamp
                    .as_deref()
                    .and_then(|timestamp| timestamp.get(..10));
                let coverage_status = alert_coverage_status(
                    bytes,
                    observed_date_from,
                    observed_date_to,
                    expected_date_from,
                    expected_date_to,
                );
                json!({
                    "file": path.file_name().and_then(|value| value.to_str()),
                    "path": path,
                    "bytes": bytes,
                    "empty": bytes == 0,
                    "first_timestamp": first_timestamp,
                    "last_timestamp": last_timestamp,
                    "timestamped_lines": timestamped_lines,
                    "expected_dataset_date_from": expected_date_from,
                    "expected_dataset_date_to": expected_date_to,
                    "coverage_status": coverage_status
                })
            })
            .collect::<Vec<_>>();
        let alert_logs_partial = alert_log_files
            .iter()
            .filter(|entry| {
                entry.get("coverage_status").and_then(Value::as_str)
                    == Some("partial_relative_to_dataset")
            })
            .count();
        let alert_logs_unknown_coverage = alert_log_files
            .iter()
            .filter(|entry| entry.get("coverage_status").and_then(Value::as_str) == Some("unknown"))
            .count();
        json!({
            "directory_present": directory.is_dir(),
            "execution_plans": count_extension(&directory, "xplan"),
            "child_cursor_reason_files": count_suffix(&directory, ".shared_cursor_reasons"),
            "alert_logs": alert_log_paths.len(),
            "alert_logs_nonempty": alert_logs_nonempty,
            "alert_logs_empty": alert_log_paths.len().saturating_sub(alert_logs_nonempty),
            "alert_logs_partial": alert_logs_partial,
            "alert_logs_unknown_coverage": alert_logs_unknown_coverage,
            "alert_log_files": alert_log_files,
            "aix_files": count_regular_files(&aix_directory),
            "aix_directory_present": aix_directory.is_dir()
        })
    }

    fn dataset_manifest(&self) -> Value {
        let first = self.collection.awrs.first();
        let last = self.collection.awrs.last();
        let date_from = first.and_then(|awr| oracle_snapshot_date(&awr.snap_info.begin_snap_time));
        let date_to = last.and_then(|awr| oracle_snapshot_date(&awr.snap_info.end_snap_time));
        let report_directory = PathBuf::from(self.html_reports_dir.as_str());
        let main_report = report_directory.join("jasmin_main.html");
        let load_profile = report_directory.join("stats/jasmin_highlight.html");
        let load_profile_secondary = report_directory.join("stats/jasmin_highlight2.html");
        let main_report_present = main_report.is_file();
        let load_profile_present = load_profile.is_file();
        let load_profile_secondary_present = load_profile_secondary.is_file();
        let attachments = self.attachment_inventory(date_from.as_deref(), date_to.as_deref());
        let mut attachment_quality_warnings = Vec::new();
        if attachments
            .get("alert_logs_empty")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        {
            attachment_quality_warnings.push(
                "One or more alert-log attachments are zero bytes. Treat them as missing coverage; do not describe them as searched-and-clean or link them as reader-facing evidence."
            );
        }
        if attachments
            .get("alert_logs_partial")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        {
            attachment_quality_warnings.push(
                "One or more alert-log attachments do not cover the full dataset date interval. Report each attachment's observed first/last timestamp and scope every alert count to that interval."
            );
        }
        if attachments
            .get("alert_logs_unknown_coverage")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        {
            attachment_quality_warnings.push(
                "One or more non-empty alert-log attachments have no recognized ISO timestamp lines. Their temporal coverage is unknown."
            );
        }
        json!({
            "project_id": self.project_id.as_str(),
            "dataset_stem": self.stem.as_str(),
            "snapshots": self.collection.awrs.len(),
            "begin_snap_id": first.map(|awr| awr.snap_info.begin_snap_id),
            "end_snap_id": last.map(|awr| awr.snap_info.end_snap_id),
            "begin_time": first.map(|awr| awr.snap_info.begin_snap_time.as_str()),
            "end_time": last.map(|awr| awr.snap_info.end_snap_time.as_str()),
            "date_from": date_from,
            "date_to": date_to,
            "database": self.collection.db_instance_information,
            "initialization_parameters": self.collection.initialization_parameters.len(),
            "sql_texts": self.collection.sql_text.len(),
            "security_level": self.security_level,
            "attachments": attachments,
            "attachment_quality_warnings": attachment_quality_warnings,
            "source_reports": {
                "directory": self.html_reports_dir.as_str(),
                "directory_present": report_directory.is_dir(),
                "main": main_report,
                "main_present": main_report_present,
                "load_profile": load_profile,
                "load_profile_present": load_profile_present,
                "load_profile_secondary": load_profile_secondary,
                "load_profile_secondary_present": load_profile_secondary_present
            }
        })
    }
}

/// Immutable parsed projects plus explicitly keyed conversational report sessions.
#[derive(Clone)]
pub struct AnalysisRuntime {
    projects: Arc<BTreeMap<String, Arc<ProjectData>>>,
    guidance: Arc<GuidanceLibrary>,
    sessions: Arc<DashMap<String, Arc<Mutex<AnalysisSession>>>>,
    sequence: Arc<AtomicU64>,
}

impl AnalysisRuntime {
    pub fn new(
        collection: AWRSCollection,
        report: ReportForAI,
        stem: String,
        security_level: usize,
        report_links: HashMap<String, HashSet<String>>,
        html_reports_dir: String,
    ) -> Self {
        let project_id = mcp_project_id_from_stem(&stem);
        Self::from_projects(vec![AnalysisProject::new(
            project_id,
            collection,
            report,
            stem,
            security_level,
            report_links,
            html_reports_dir,
        )])
        .expect("one in-memory MCP project is valid")
    }

    pub fn from_projects(projects: Vec<AnalysisProject>) -> Result<Self> {
        if projects.is_empty() {
            bail!("at least one MCP project is required");
        }
        if projects.len() > MAX_MCP_PROJECTS {
            bail!(
                "{} MCP projects were supplied; the maximum is {MAX_MCP_PROJECTS}",
                projects.len()
            );
        }
        let mut indexed = BTreeMap::new();
        for mut project in projects {
            let project_id = project.project_id.trim();
            if project_id.is_empty() {
                bail!("MCP project_id cannot be empty");
            }
            if project_id.len() > 96 {
                bail!("MCP project_id '{project_id}' exceeds 96 bytes");
            }
            let project_id = project_id.to_string();
            if indexed.contains_key(&project_id) {
                bail!("duplicate MCP project_id '{project_id}'");
            }
            project.project_id = project_id.clone();
            indexed.insert(project_id, Arc::new(ProjectData::from(project)));
        }
        Ok(Self {
            projects: Arc::new(indexed),
            guidance: Arc::new(GuidanceLibrary::load()),
            sessions: Arc::new(DashMap::new()),
            sequence: Arc::new(AtomicU64::new(1)),
        })
    }

    fn selected_project_ids(&self, arguments: &Value) -> std::result::Result<Vec<String>, Value> {
        let singular = arguments
            .get("project_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());
        let plural = arguments.get("project_ids").and_then(Value::as_array);
        if singular.is_some() && plural.is_some() {
            return Err(tool_error(
                "INVALID_PROJECT_SELECTION",
                "Use project_id or project_ids, not both",
            ));
        }
        let requested = if let Some(project_id) = singular {
            vec![project_id.to_string()]
        } else if let Some(project_ids) = plural {
            if project_ids.is_empty() {
                return Err(tool_error(
                    "INVALID_PROJECT_SELECTION",
                    "project_ids cannot be empty",
                ));
            }
            project_ids
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string)
                        .ok_or_else(|| {
                            tool_error(
                                "INVALID_PROJECT_SELECTION",
                                "Every project_ids item must be a non-empty string",
                            )
                        })
                })
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            self.projects.keys().cloned().collect()
        };
        let mut unique = BTreeSet::new();
        for project_id in &requested {
            if !unique.insert(project_id.clone()) {
                return Err(tool_error(
                    "DUPLICATE_PROJECT_ID",
                    format!("project_id '{project_id}' was selected more than once"),
                ));
            }
            if !self.projects.contains_key(project_id) {
                return Err(tool_error(
                    "UNKNOWN_PROJECT",
                    format!("Unknown project_id '{project_id}'. Call list_performance_projects."),
                ));
            }
        }
        Ok(requested)
    }

    fn project(&self, project_id: &str) -> std::result::Result<Arc<ProjectData>, Value> {
        self.projects.get(project_id).cloned().ok_or_else(|| {
            tool_error(
                "UNKNOWN_PROJECT",
                format!("Unknown project_id '{project_id}'. Call list_performance_projects."),
            )
        })
    }

    fn new_analysis(&self, arguments: &Value) -> std::result::Result<Value, Value> {
        let project_ids = self.selected_project_ids(arguments)?;
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let analysis_id = format!(
            "A-{}-{sequence:04}",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        );
        let mut config = ReportConfig::default();
        if let Some(language) = arguments.get("language").and_then(Value::as_str) {
            config.language = bounded_string(language, 16);
        }
        if let Some(audience) = arguments.get("audience").and_then(Value::as_str) {
            if matches!(audience, "technical" | "management" | "mixed") {
                config.audience = audience.to_string();
            }
        }
        let mut project_bootstrap = Vec::with_capacity(project_ids.len());
        for project_id in &project_ids {
            let project = self.project(project_id)?;
            let dataset_manifest = project.dataset_manifest();
            let attachments = dataset_manifest
                .get("attachments")
                .cloned()
                .unwrap_or(Value::Null);
            let recommended_calls = recommended_next_calls(
                &project.collection.db_instance_information.platform,
                &attachments,
                dataset_manifest.get("date_from").and_then(Value::as_str),
                dataset_manifest.get("date_to").and_then(Value::as_str),
            );
            project_bootstrap.push(json!({
                "project_id": project_id,
                "dataset_manifest": dataset_manifest,
                "case_seed": mcp_bootstrap_seed(&project.report),
                "triage_preview": mcp_triage_preview(&project.report),
                "quality_gates": quality_gates(&project.collection.db_instance_information.platform),
                "recommended_next_calls": add_project_id_to_calls(recommended_calls, project_id)
            }));
        }
        let seed = json!({
            "evidence_id": SEED_EVIDENCE_ID,
            "project_ids": project_ids.clone(),
            "projects": project_bootstrap.clone()
        });
        let session = AnalysisSession::new(seed.clone(), config.clone(), project_ids.clone());
        self.sessions
            .insert(analysis_id.clone(), Arc::new(Mutex::new(session)));
        let mut output = json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "seed_evidence_id": SEED_EVIDENCE_ID,
            "project_ids": project_ids.clone(),
            "comparison_mode": project_bootstrap.len() > 1,
            "instruction": if project_bootstrap.len() > 1 {
                "Use this analysis_id for the comparative investigation. Pass project_id to project-specific evidence tools and use the comparison tools for normalized cross-project evidence."
            } else {
                "Use this analysis_id in every subsequent JAS-MIN tool call. Build competing hypotheses, obtain narrow evidence, consult guidance only for detected symptoms, and finish through the report tools."
            },
            "focus": arguments.get("focus").cloned().unwrap_or(Value::Null),
            "projects": project_bootstrap.clone(),
            "available_calculations": calculation_catalog(),
            "case_seed": seed,
            "diagnostic_guidance": self.guidance.catalog_json(),
            "report_contract": report_contract(&config),
            "recommended_comparison_calls": if project_bootstrap.len() > 1 {
                json!([
                    {"tool": "compare_project_metric", "reason": "compare normalized metric distributions between a baseline and candidate project"},
                    {"tool": "compare_project_sql", "reason": "compare the same SQL_ID across projects using per-execution and workload metrics"}
                ])
            } else { Value::Array(Vec::new()) }
        });
        if project_bootstrap.len() == 1 {
            let project = &project_bootstrap[0];
            output["dataset_manifest"] = project["dataset_manifest"].clone();
            output["triage_preview"] = project["triage_preview"].clone();
            output["quality_gates"] = project["quality_gates"].clone();
            output["recommended_next_calls"] = project["recommended_next_calls"].clone();
        }
        Ok(output)
    }

    fn list_projects(&self) -> Value {
        json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "project_count": self.projects.len(),
            "projects": self.projects.values().map(|project| project.dataset_manifest()).collect::<Vec<_>>(),
            "usage": "Call start_performance_analysis with project_ids for a comparative session, or project_id for a single-project session."
        })
    }

    fn session(
        &self,
        analysis_id: &str,
    ) -> std::result::Result<Arc<Mutex<AnalysisSession>>, Value> {
        self.sessions
            .get(analysis_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| tool_error("UNKNOWN_ANALYSIS", format!("Unknown analysis_id '{analysis_id}'. Call start_performance_analysis first.")))
    }

    fn analysis_id(arguments: &Map<String, Value>) -> std::result::Result<&str, Value> {
        arguments
            .get("analysis_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                tool_error(
                    "MISSING_ANALYSIS_ID",
                    "analysis_id is required; call start_performance_analysis first",
                )
            })
    }

    fn catalog_for_session(
        &self,
        arguments: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?;
        let session = self.session(analysis_id)?;
        let state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        let projects = state
            .project_ids
            .iter()
            .filter_map(|project_id| self.projects.get(project_id))
            .map(|project| project.dataset_manifest())
            .collect::<Vec<_>>();
        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "project_ids": state.project_ids,
            "projects": projects,
            "available_calculations": calculation_catalog(),
            "diagnostic_guidance": self.guidance.catalog_json(),
            "report_contract": report_contract(&state.config)
        }))
    }

    fn project_for_session(
        &self,
        session: &Arc<Mutex<AnalysisSession>>,
        arguments: &Map<String, Value>,
    ) -> std::result::Result<Arc<ProjectData>, Value> {
        let selected = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?
            .project_ids
            .clone();
        let requested = arguments
            .get("project_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());
        let project_id = match requested {
            Some(project_id) => project_id,
            None if selected.len() == 1 => &selected[0],
            None => {
                return Err(json!({
                    "error_code": "MISSING_PROJECT_ID",
                    "message": "project_id is required because this analysis contains multiple projects",
                    "available_project_ids": selected
                }))
            }
        };
        if !selected.iter().any(|candidate| candidate == project_id) {
            return Err(json!({
                "error_code": "PROJECT_OUTSIDE_ANALYSIS",
                "message": format!("project_id '{project_id}' is not part of this analysis session"),
                "available_project_ids": selected
            }));
        }
        self.project(project_id)
    }

    fn selected_project(
        &self,
        analysis_id: &str,
        project_id: &str,
    ) -> std::result::Result<(Arc<Mutex<AnalysisSession>>, Arc<ProjectData>), Value> {
        let session = self.session(analysis_id)?;
        let selected = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?
            .project_ids
            .clone();
        if !selected.iter().any(|candidate| candidate == project_id) {
            return Err(json!({
                "error_code": "PROJECT_OUTSIDE_ANALYSIS",
                "message": format!("project_id '{project_id}' is not part of this analysis session"),
                "available_project_ids": selected
            }));
        }
        Ok((session, self.project(project_id)?))
    }

    fn execute_evidence_tool(
        &self,
        name: &str,
        arguments: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let session = self.session(&analysis_id)?;
        let project = self.project_for_session(&session, arguments)?;
        let mut clean_arguments = arguments.clone();
        clean_arguments.remove("analysis_id");
        clean_arguments.remove("project_id");
        let clean_value = Value::Object(clean_arguments.clone());
        let result = if name == "get_precomputed_analysis" {
            dispatch_precomputed_analysis(&clean_value, &project.report)
        } else if name == "get_wait_event_sql_contributors" {
            let event_name = clean_value
                .get("event_name")
                .and_then(Value::as_str)
                .unwrap_or("");
            if event_name.trim().is_empty() {
                return Err(tool_error("MISSING_EVENT_NAME", "event_name is required"));
            }
            let limit = clean_value
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(5) as usize;
            wait_sql_contributors(&project, event_name, limit)
        } else {
            dispatch_tool_call_value(
                name,
                &clean_value,
                &project.collection,
                project.stem.as_str(),
            )
        };
        if result.get("error").is_some() {
            return Err(result);
        }

        let cache_key = format!(
            "{}:{name}:{}",
            project.project_id,
            canonical_json(&clean_value)
        );
        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        if let Some(existing_id) = state.evidence_cache.get(&cache_key).cloned() {
            if let Some(record) = state.evidence.get(&existing_id) {
                return Ok(json!({
                    "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
                    "analysis_id": analysis_id,
                    "project_id": project.project_id.as_str(),
                    "evidence_id": existing_id,
                    "tool_name": name,
                    "cached": true,
                    "result": record.result
                }));
            }
        }

        let evidence_id = format!("E-{:04}", state.next_evidence);
        state.next_evidence += 1;
        state.evidence_cache.insert(cache_key, evidence_id.clone());
        state.evidence.insert(
            evidence_id.clone(),
            EvidenceRecord {
                evidence_id: evidence_id.clone(),
                tool_name: name.to_string(),
                project_id: Some(project.project_id.as_str().to_string()),
                arguments: clean_value,
                result: result.clone(),
            },
        );
        state.finalized_markdown = None;
        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "project_id": project.project_id.as_str(),
            "evidence_id": evidence_id,
            "tool_name": name,
            "cached": false,
            "result": result
        }))
    }

    fn store_comparison_evidence(
        &self,
        analysis_id: &str,
        session: &Arc<Mutex<AnalysisSession>>,
        tool_name: &str,
        arguments: Value,
        result: Value,
    ) -> std::result::Result<Value, Value> {
        let cache_key = format!("{tool_name}:{}", canonical_json(&arguments));
        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        if let Some(existing_id) = state.evidence_cache.get(&cache_key).cloned() {
            if let Some(record) = state.evidence.get(&existing_id) {
                return Ok(json!({
                    "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
                    "analysis_id": analysis_id,
                    "evidence_id": existing_id,
                    "tool_name": tool_name,
                    "cached": true,
                    "result": record.result
                }));
            }
        }
        let evidence_id = format!("E-{:04}", state.next_evidence);
        state.next_evidence += 1;
        state.evidence_cache.insert(cache_key, evidence_id.clone());
        state.evidence.insert(
            evidence_id.clone(),
            EvidenceRecord {
                evidence_id: evidence_id.clone(),
                tool_name: tool_name.to_string(),
                project_id: None,
                arguments,
                result: result.clone(),
            },
        );
        state.finalized_markdown = None;
        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "evidence_id": evidence_id,
            "tool_name": tool_name,
            "cached": false,
            "result": result
        }))
    }

    fn compare_project_metric(
        &self,
        arguments: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let baseline_project_id = required_string(arguments, "baseline_project_id", 96)?;
        let candidate_project_id = required_string(arguments, "candidate_project_id", 96)?;
        if baseline_project_id == candidate_project_id {
            return Err(tool_error(
                "IDENTICAL_COMPARISON_PROJECTS",
                "baseline_project_id and candidate_project_id must differ",
            ));
        }
        let (session, baseline) = self.selected_project(&analysis_id, &baseline_project_id)?;
        let (_, candidate) = self.selected_project(&analysis_id, &candidate_project_id)?;
        let kind = required_string(arguments, "kind", 64)?;
        validate_enum(
            "kind",
            &kind,
            &[
                "load_profile",
                "instance_stat",
                "wait_event_fg",
                "wait_event_bg",
                "time_model",
                "host_cpu",
                "io_stats_byfunc",
            ],
        )?;
        let name = required_string(arguments, "name", 256)?;
        let field = arguments
            .get("field")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("value");
        validate_comparison_metric_field(&kind, field)?;
        let direction = arguments
            .get("direction")
            .and_then(Value::as_str)
            .unwrap_or("neutral");
        validate_enum(
            "direction",
            direction,
            &["neutral", "lower_is_better", "higher_is_better"],
        )?;
        let materiality_pct = arguments
            .get("materiality_pct")
            .and_then(Value::as_f64)
            .unwrap_or(5.0);
        if !(0.0..=1000.0).contains(&materiality_pct) {
            return Err(tool_error(
                "INVALID_MATERIALITY",
                "materiality_pct must be between 0 and 1000",
            ));
        }
        let tool_arguments = json!({"kind": kind, "name": name, "field": field});
        let baseline_series = dispatch_tool_call_value(
            "get_metric_time_series",
            &tool_arguments,
            &baseline.collection,
            baseline.stem.as_str(),
        );
        let candidate_series = dispatch_tool_call_value(
            "get_metric_time_series",
            &tool_arguments,
            &candidate.collection,
            candidate.stem.as_str(),
        );
        let baseline_values = metric_series_values(&baseline_series);
        let candidate_values = metric_series_values(&candidate_series);
        if baseline_values.is_empty() || candidate_values.is_empty() {
            return Err(json!({
                "error_code": "MISSING_COMPARISON_DATA",
                "message": "The requested metric must have observed samples in both projects; missing samples are not converted to zero.",
                "baseline_project_id": baseline_project_id,
                "baseline_points": baseline_values.len(),
                "candidate_project_id": candidate_project_id,
                "candidate_points": candidate_values.len(),
                "kind": kind,
                "name": name,
                "field": field
            }));
        }
        let comparison = compare_numeric_values(
            &baseline_values,
            &candidate_values,
            direction,
            materiality_pct,
        );
        let evidence_arguments = json!({
            "baseline_project_id": baseline_project_id,
            "candidate_project_id": candidate_project_id,
            "kind": kind,
            "name": name,
            "field": field,
            "direction": direction,
            "materiality_pct": materiality_pct
        });
        let result = json!({
            "baseline_project_id": baseline_project_id,
            "candidate_project_id": candidate_project_id,
            "metric": {"kind": kind, "name": name, "field": field},
            "comparison": comparison,
            "baseline_period": compact_project_period(&baseline),
            "candidate_period": compact_project_period(&candidate),
            "coverage_note": "Only observed metric samples are summarized. Missing samples are not zeros. Verify workload mix, snapshot duration, seasonality, and database identity before attributing causality.",
            "series_tools": {
                "baseline": {"tool": "get_metric_time_series", "project_id": baseline_project_id},
                "candidate": {"tool": "get_metric_time_series", "project_id": candidate_project_id}
            }
        });
        self.store_comparison_evidence(
            &analysis_id,
            &session,
            "compare_project_metric",
            evidence_arguments,
            result,
        )
    }

    fn compare_project_sql(
        &self,
        arguments: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let baseline_project_id = required_string(arguments, "baseline_project_id", 96)?;
        let candidate_project_id = required_string(arguments, "candidate_project_id", 96)?;
        if baseline_project_id == candidate_project_id {
            return Err(tool_error(
                "IDENTICAL_COMPARISON_PROJECTS",
                "baseline_project_id and candidate_project_id must differ",
            ));
        }
        let sql_id = required_string(arguments, "sql_id", 32)?.to_ascii_lowercase();
        let materiality_pct = arguments
            .get("materiality_pct")
            .and_then(Value::as_f64)
            .unwrap_or(5.0);
        if !(0.0..=1000.0).contains(&materiality_pct) {
            return Err(tool_error(
                "INVALID_MATERIALITY",
                "materiality_pct must be between 0 and 1000",
            ));
        }
        let (session, baseline) = self.selected_project(&analysis_id, &baseline_project_id)?;
        let (_, candidate) = self.selected_project(&analysis_id, &candidate_project_id)?;
        let baseline_summary = sql_comparison_summary(&baseline, &sql_id);
        let candidate_summary = sql_comparison_summary(&candidate, &sql_id);
        if baseline_summary["snapshots_with_sql"].as_u64().unwrap_or(0) == 0
            || candidate_summary["snapshots_with_sql"]
                .as_u64()
                .unwrap_or(0)
                == 0
        {
            return Err(json!({
                "error_code": "MISSING_SQL_COMPARISON_DATA",
                "message": "The SQL_ID must be observed in both projects; an absent SQL_ID is not treated as zero workload.",
                "sql_id": sql_id,
                "baseline_project_id": baseline_project_id,
                "baseline_snapshots_with_sql": baseline_summary["snapshots_with_sql"],
                "candidate_project_id": candidate_project_id,
                "candidate_snapshots_with_sql": candidate_summary["snapshots_with_sql"]
            }));
        }
        let comparisons = sql_metric_comparisons(
            &baseline.collection,
            &candidate.collection,
            &sql_id,
            materiality_pct,
        );
        let evidence_arguments = json!({
            "baseline_project_id": baseline_project_id,
            "candidate_project_id": candidate_project_id,
            "sql_id": sql_id,
            "materiality_pct": materiality_pct
        });
        let result = json!({
            "sql_id": sql_id,
            "baseline_project_id": baseline_project_id,
            "candidate_project_id": candidate_project_id,
            "baseline": baseline_summary,
            "candidate": candidate_summary,
            "comparisons": comparisons,
            "coverage_note": "Per-execution metrics describe efficiency; totals and executions describe workload volume. Missing SQL samples are excluded rather than converted to zero. Plan hash evidence is limited to snapshots where the SQL appears in top-event data."
        });
        self.store_comparison_evidence(
            &analysis_id,
            &session,
            "compare_project_sql",
            evidence_arguments,
            result,
        )
    }

    fn diagnostic_guidance(
        &self,
        arguments: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let session = self.session(&analysis_id)?;
        let mut clean_arguments = arguments.clone();
        clean_arguments.remove("analysis_id");
        let mut result = self.guidance.query(
            &Value::Object(clean_arguments),
            DEFAULT_GUIDANCE_LIMIT_CHARS,
        );
        if result.get("error").is_some() {
            return Err(result);
        }
        let mut references = Vec::new();
        let mut guidance_records = Vec::new();
        if let Some(matches) = result.get_mut("matches").and_then(Value::as_array_mut) {
            for matched in matches {
                if let Some(section_id) = matched.get("section_id").and_then(Value::as_str) {
                    let reference = format!("GUIDE-{section_id}");
                    let title = matched
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Untitled diagnostic guidance")
                        .to_string();
                    let text = matched
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if let Some(object) = matched.as_object_mut() {
                        object.insert("guidance_ref".to_string(), json!(reference));
                    }
                    guidance_records.push((reference.clone(), GuidanceRecord { title, text }));
                    references.push(reference);
                }
            }
        }
        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        state.guidance.extend(guidance_records);
        state.finalized_markdown = None;
        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "methodology_only": true,
            "guidance_refs": references,
            "result": result
        }))
    }

    fn configure_report(
        &self,
        arguments: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let session = self.session(&analysis_id)?;
        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;

        if let Some(format) = arguments.get("output_format").and_then(Value::as_str) {
            validate_enum("output_format", format, &["markdown", "json", "both"])?;
            state.config.output_format = format.to_string();
        }
        if let Some(language) = arguments.get("language").and_then(Value::as_str) {
            state.config.language = bounded_string(language, 16);
        }
        if let Some(audience) = arguments.get("audience").and_then(Value::as_str) {
            validate_enum("audience", audience, &["technical", "management", "mixed"])?;
            state.config.audience = audience.to_string();
        }
        if let Some(detail) = arguments.get("detail_level").and_then(Value::as_str) {
            validate_enum("detail_level", detail, &["compact", "standard", "deep"])?;
            state.config.detail_level = detail.to_string();
        }
        if let Some(overrides) = arguments.get("detail_overrides").and_then(Value::as_object) {
            let mut parsed = BTreeMap::new();
            for (section, detail) in overrides {
                if !REPORT_CATEGORIES.contains(&section.as_str()) {
                    return Err(tool_error(
                        "INVALID_REPORT_SECTION",
                        format!("Unknown report category '{section}'"),
                    ));
                }
                let Some(detail) = detail.as_str() else {
                    return Err(tool_error(
                        "INVALID_DETAIL_LEVEL",
                        format!("Detail override for '{section}' must be a string"),
                    ));
                };
                validate_enum("detail_level", detail, &["compact", "standard", "deep"])?;
                parsed.insert(section.clone(), detail.to_string());
            }
            state.config.detail_overrides = parsed;
        }
        if let Some(value) = arguments
            .get("include_evidence_appendix")
            .and_then(Value::as_bool)
        {
            state.config.include_evidence_appendix = value;
        }
        if let Some(value) = arguments
            .get("include_guidance_appendix")
            .and_then(Value::as_bool)
        {
            state.config.include_guidance_appendix = value;
        }
        state.finalized_markdown = None;
        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "report_contract": report_contract(&state.config),
            "message": "The stable core section order is server-controlled. Detail and appendices may be changed without losing the report structure."
        }))
    }

    fn record_finding(&self, arguments: &Map<String, Value>) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let session = self.session(&analysis_id)?;
        let category = required_string(arguments, "category", 64)?;
        validate_enum("category", &category, REPORT_CATEGORIES)?;
        let title = required_string(arguments, "title", 240)?;
        let severity = required_string(arguments, "severity", 32)?;
        validate_enum(
            "severity",
            &severity,
            &["critical", "high", "medium", "low", "informational"],
        )?;
        let confidence = required_string(arguments, "confidence", 32)?;
        validate_enum(
            "confidence",
            &confidence,
            &["high", "medium", "low", "unknown"],
        )?;
        let conclusion = required_string(arguments, "conclusion", 2_000)?;
        let mechanism = required_synthesis_string(arguments, "mechanism", 4_000, 32)?;
        let temporal_pattern = required_synthesis_string(arguments, "temporal_pattern", 2_000, 24)?;
        let affected_workload =
            required_synthesis_string(arguments, "affected_workload", 4_000, 24)?;
        let evidence_limitations =
            required_synthesis_string(arguments, "evidence_limitations", 4_000, 24)?;
        let evidence_summary = required_string(arguments, "evidence_summary", 4_000)?;
        validate_distinct_diagnostic_statements(&[
            ("conclusion", &conclusion),
            ("mechanism", &mechanism),
            ("temporal_pattern", &temporal_pattern),
            ("affected_workload", &affected_workload),
            ("evidence_limitations", &evidence_limitations),
            ("evidence_summary", &evidence_summary),
        ])?;
        let details = optional_string(arguments, "details", 16_000);
        let evidence_refs = string_array(arguments, "evidence_refs", 32, 32)?;
        let guidance_refs = string_array(arguments, "guidance_refs", 16, 64)?;
        let recommendations = parse_recommendations(arguments.get("recommendations"))?;
        if category != "limitations" && evidence_refs.is_empty() {
            return Err(tool_error(
                "FINDING_WITHOUT_EVIDENCE",
                "Every analytical finding must cite at least one evidence_ref; only an explicit limitations finding may use an empty array",
            ));
        }

        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        validate_references(&state, &evidence_refs, &guidance_refs)?;
        let guidance_quotes =
            parse_guidance_quotations(arguments.get("guidance_quotes"), &guidance_refs, &state)?;
        let finding_id = arguments
            .get("finding_id")
            .and_then(Value::as_str)
            .filter(|id| state.findings.contains_key(*id))
            .map(str::to_string)
            .unwrap_or_else(|| {
                let id = format!("F-{:04}", state.next_finding);
                state.next_finding += 1;
                id
            });
        state.findings.insert(
            finding_id.clone(),
            ReportFinding {
                finding_id: finding_id.clone(),
                category,
                title,
                severity,
                confidence,
                conclusion,
                mechanism,
                temporal_pattern,
                affected_workload,
                evidence_limitations,
                evidence_summary,
                details,
                evidence_refs,
                guidance_refs,
                guidance_quotes,
                recommendations,
            },
        );
        state.finalized_markdown = None;
        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "finding_id": finding_id,
            "findings_total": state.findings.len(),
            "message": "Evidence-backed finding stored. Reuse finding_id to replace it after deeper investigation."
        }))
    }

    fn record_report_table(
        &self,
        arguments: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let session = self.session(&analysis_id)?;
        let kind = required_string(arguments, "kind", 64)?;
        let Some((category, _)) = report_table_definition(&kind) else {
            return Err(tool_error(
                "INVALID_REPORT_TABLE_KIND",
                format!("Unknown report table kind '{kind}'"),
            ));
        };
        let title = required_string(arguments, "title", 240)?;
        let rows_value = arguments.get("rows").ok_or_else(|| {
            tool_error(
                "MISSING_REPORT_TABLE_ROWS",
                "rows is required and must contain structured analysis rows",
            )
        })?;
        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        let rows = parse_report_table_rows(&kind, rows_value, &state, &self.projects)?;
        let table_id = arguments
            .get("table_id")
            .and_then(Value::as_str)
            .filter(|id| state.report_tables.contains_key(*id))
            .map(str::to_string)
            .unwrap_or_else(|| {
                let id = format!("T-{:04}", state.next_table);
                state.next_table += 1;
                id
            });
        state.report_tables.insert(
            table_id.clone(),
            ReportTable {
                table_id: table_id.clone(),
                kind: kind.clone(),
                category: category.to_string(),
                title,
                rows,
            },
        );
        state.finalized_markdown = None;
        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "table_id": table_id,
            "kind": kind,
            "category": category,
            "tables_total": state.report_tables.len(),
            "message": "Structured analysis table stored. Reuse table_id to replace it after deeper investigation."
        }))
    }

    fn set_assessment(&self, arguments: &Map<String, Value>) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let session = self.session(&analysis_id)?;
        let assessment = required_string(arguments, "assessment", 64)?;
        validate_enum("assessment", &assessment, REQUIRED_ASSESSMENTS)?;
        let status = required_string(arguments, "status", 32)?;
        validate_enum("status", &status, &["proven", "not_proven", "unknown"])?;
        let conclusion = required_string(arguments, "conclusion", 2_000)?;
        let evidence_summary = required_string(arguments, "evidence_summary", 4_000)?;
        let evidence_refs = string_array(arguments, "evidence_refs", 32, 32)?;
        let guidance_refs = string_array(arguments, "guidance_refs", 16, 64)?;
        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        validate_references(&state, &evidence_refs, &guidance_refs)?;
        let guidance_quotes =
            parse_guidance_quotations(arguments.get("guidance_quotes"), &guidance_refs, &state)?;
        if status != "unknown" && evidence_refs.is_empty() {
            return Err(tool_error(
                "ASSESSMENT_WITHOUT_EVIDENCE",
                "A proven or not_proven assessment must cite at least one evidence_ref",
            ));
        }
        state.assessments.insert(
            assessment.clone(),
            ReportAssessment {
                assessment: assessment.clone(),
                status,
                conclusion,
                evidence_summary,
                evidence_refs,
                guidance_refs,
                guidance_quotes,
            },
        );
        state.finalized_markdown = None;
        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "assessment": assessment,
            "assessments_completed": state.assessments.len(),
            "assessments_required": REQUIRED_ASSESSMENTS
        }))
    }

    fn report_status(&self, arguments: &Map<String, Value>) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let session = self.session(&analysis_id)?;
        let state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        Ok(report_status_value(&analysis_id, &state, &self.projects))
    }

    fn finalize_report(&self, arguments: &Map<String, Value>) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let session = self.session(&analysis_id)?;
        let allow_incomplete = arguments
            .get("allow_incomplete")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        let status = report_status_value(&analysis_id, &state, &self.projects);
        if !allow_incomplete && status.get("ready_to_finalize") != Some(&Value::Bool(true)) {
            return Err(json!({
                "error_code": "REPORT_INCOMPLETE",
                "message": "The report contract is incomplete. Satisfy every missing category, assessment, evidence item, structured table kind and structured row, or explicitly request allow_incomplete=true for a draft.",
                "status": status
            }));
        }
        state.report_revision += 1;
        let datasets = state
            .project_ids
            .iter()
            .filter_map(|project_id| self.projects.get(project_id))
            .map(|project| project.dataset_manifest())
            .collect::<Vec<_>>();
        let report_document = json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "revision": state.report_revision,
            "generated_at": chrono::Utc::now().to_rfc3339(),
            "project_ids": state.project_ids,
            "dataset": if datasets.len() == 1 { datasets[0].clone() } else { Value::Null },
            "datasets": datasets,
            "config": state.config,
            "section_index": report_section_index(&state),
            "findings": state.findings.values().collect::<Vec<_>>(),
            "structured_tables": state.report_tables.values().collect::<Vec<_>>(),
            "mandatory_assessments": state.assessments,
            "coverage": status
        });
        let entity_links = ReportEntityLinks::build(&state.project_ids, &self.projects);
        let markdown = render_markdown(&report_document, &state, &entity_links);
        state.finalized_markdown = Some(markdown.clone());
        let mut output = json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "revision": state.report_revision,
            "output_format": state.config.output_format,
            "draft": !status.get("ready_to_finalize").and_then(Value::as_bool).unwrap_or(false)
        });
        if matches!(state.config.output_format.as_str(), "markdown" | "both") {
            output["markdown"] = json!(markdown);
            output["html_export_hint"] = json!(
                "If HTML was requested, pass this exact markdown value to convert_markdown_to_html."
            );
        }
        if matches!(state.config.output_format.as_str(), "json" | "both") {
            output["report"] = report_document;
        }
        Ok(output)
    }

    fn convert_markdown_to_html(
        &self,
        arguments: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let output_directory = std::env::current_dir().map_err(|error| {
            tool_error(
                "WORKING_DIRECTORY_UNAVAILABLE",
                format!("Cannot resolve the JAS-MIN working directory: {error}"),
            )
        })?;
        self.convert_markdown_to_html_in_directory(arguments, &output_directory)
    }

    fn convert_markdown_to_html_in_directory(
        &self,
        arguments: &Map<String, Value>,
        output_directory: &Path,
    ) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let session = self.session(&analysis_id)?;
        let (project_ids, finalized_markdown) = {
            let state = session
                .lock()
                .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
            (state.project_ids.clone(), state.finalized_markdown.clone())
        };
        let markdown = arguments
            .get("markdown")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| tool_error("INVALID_ARGUMENT", "'markdown' is required"))?;
        if markdown.len() > MAX_MCP_MARKDOWN_BYTES {
            return Err(tool_error(
                "MARKDOWN_TOO_LARGE",
                format!(
                    "Markdown input is {} bytes; the maximum is {MAX_MCP_MARKDOWN_BYTES}",
                    markdown.len()
                ),
            ));
        }
        validate_stable_markdown_report(markdown)?;
        let Some(finalized_markdown) = finalized_markdown else {
            return Err(tool_error(
                "REPORT_NOT_FINALIZED",
                "Call finalize_report before converting Markdown to HTML",
            ));
        };
        if markdown != finalized_markdown {
            return Err(tool_error(
                "MARKDOWN_NOT_FINALIZED",
                "The Markdown must exactly match the latest finalize_report output for this analysis session",
            ));
        }

        let single_project = if project_ids.len() == 1 {
            Some(self.project(&project_ids[0])?)
        } else {
            None
        };
        let output_stem = single_project
            .as_ref()
            .map(|project| project.stem.as_str())
            .unwrap_or("jas-min-comparison");
        let filename = html_output_filename(
            arguments.get("output_filename").and_then(Value::as_str),
            output_stem,
            &analysis_id,
        )?;
        let output_path = output_directory.join(&filename);
        let report_directory_reference = single_project
            .as_ref()
            .map(|project| project.html_reports_dir.as_str())
            .unwrap_or("");
        let configured_report_directory = PathBuf::from(report_directory_reference);
        let report_directory = if report_directory_reference.is_empty() {
            output_directory.to_path_buf()
        } else if configured_report_directory.is_absolute() {
            configured_report_directory
        } else {
            output_directory.join(configured_report_directory)
        };
        let linked_report_directories = project_ids
            .iter()
            .filter_map(|project_id| self.projects.get(project_id))
            .map(|project| {
                let configured = PathBuf::from(project.html_reports_dir.as_str());
                let resolved = if configured.is_absolute() {
                    configured
                } else {
                    output_directory.join(configured)
                };
                let directory_present = resolved.is_dir();
                let main_report = resolved.join("jasmin_main.html");
                let main_report_present = main_report.is_file();
                json!({
                    "project_id": project.project_id.as_str(),
                    "directory": resolved,
                    "directory_present": directory_present,
                    "main_report": main_report,
                    "main_report_present": main_report_present
                })
            })
            .collect::<Vec<_>>();
        let html = render_markdown_html_document(
            markdown,
            report_directory_reference,
            &report_directory.to_string_lossy(),
            HashMap::new(),
        );
        validate_resolved_html_navigation(&html)?;
        let validated_local_links = validate_local_html_targets(&html, output_directory)?;

        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .map_err(|error| {
                let code = if error.kind() == std::io::ErrorKind::AlreadyExists {
                    "OUTPUT_EXISTS"
                } else {
                    "HTML_WRITE_FAILED"
                };
                tool_error(
                    code,
                    format!("Cannot create '{}': {error}", output_path.display()),
                )
            })?;
        if let Err(error) = output
            .write_all(html.as_bytes())
            .and_then(|_| output.flush())
        {
            drop(output);
            let _ = std::fs::remove_file(&output_path);
            return Err(tool_error(
                "HTML_WRITE_FAILED",
                format!("Cannot write '{}': {error}", output_path.display()),
            ));
        }

        let linked_report_directory_present = if single_project.is_some() {
            report_directory.is_dir()
        } else {
            linked_report_directories
                .iter()
                .all(|entry| entry["directory_present"] == true)
        };
        let linked_report_directory = single_project
            .as_ref()
            .map(|_| report_directory.to_string_lossy().to_string());

        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "output_filename": filename,
            "output_path": output_path,
            "html_bytes": html.len(),
            "markdown_bytes": markdown.len(),
            "report_structure_validated": true,
            "validated_local_links": validated_local_links,
            "renderer": "JAS-MIN classic AI Markdown renderer",
            "linked_report_directory": linked_report_directory,
            "linked_report_directory_present": linked_report_directory_present,
            "linked_report_directories": linked_report_directories,
            "opened_automatically": false
        }))
    }

    fn call_tool(
        &self,
        name: &str,
        arguments: Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        match name {
            "list_performance_projects" => Ok(self.list_projects()),
            "start_performance_analysis" => self.new_analysis(&Value::Object(arguments)),
            "get_analysis_catalog" => self.catalog_for_session(&arguments),
            "get_precomputed_analysis" => self.execute_evidence_tool(name, &arguments),
            "compare_project_metric" => self.compare_project_metric(&arguments),
            "compare_project_sql" => self.compare_project_sql(&arguments),
            "get_diagnostic_guidance" => self.diagnostic_guidance(&arguments),
            "configure_report" => self.configure_report(&arguments),
            "record_finding" => self.record_finding(&arguments),
            "record_report_table" => self.record_report_table(&arguments),
            "set_report_assessment" => self.set_assessment(&arguments),
            "get_report_status" => self.report_status(&arguments),
            "finalize_report" => self.finalize_report(&arguments),
            "convert_markdown_to_html" => self.convert_markdown_to_html(&arguments),
            other => self.execute_evidence_tool(other, &arguments),
        }
    }
}

fn metric_series_values(result: &Value) -> Vec<f64> {
    result
        .get("series")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|point| point.get("value").and_then(Value::as_f64))
        .filter(|value| value.is_finite())
        .collect()
}

fn validate_comparison_metric_field(kind: &str, field: &str) -> std::result::Result<(), Value> {
    let allowed = match kind {
        "load_profile" | "instance_stat" => &["value"][..],
        "wait_event_fg" | "wait_event_bg" => &[
            "value",
            "pct_dbtime",
            "total_wait_time_s",
            "avg_wait",
            "waits",
        ][..],
        "time_model" => &["value", "time_s", "pct_dbtime"][..],
        "host_cpu" => &[
            "value",
            "pct_user",
            "pct_system",
            "pct_wio",
            "pct_idle",
            "load_avg_begin",
            "load_avg_end",
            "cpus",
            "cores",
            "sockets",
        ][..],
        "io_stats_byfunc" => &[
            "value",
            "reads_data",
            "reads_req_s",
            "reads_data_s",
            "writes_data",
            "writes_req_s",
            "writes_data_s",
            "waits_count",
            "avg_time",
        ][..],
        _ => &[][..],
    };
    if allowed.contains(&field) {
        Ok(())
    } else {
        Err(tool_error(
            "INVALID_COMPARISON_FIELD",
            format!(
                "field '{field}' is not valid for kind '{kind}'; allowed fields: {}",
                allowed.join(", ")
            ),
        ))
    }
}

fn numeric_summary(values: &[f64]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let mut sorted = values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if sorted.is_empty() {
        return Value::Null;
    }
    sorted.sort_by(f64::total_cmp);
    let count = sorted.len();
    let mean = sorted.iter().sum::<f64>() / count as f64;
    let variance = sorted
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / count as f64;
    json!({
        "samples": count,
        "min": sorted[0],
        "mean": mean,
        "median": percentile_sorted(&sorted, 0.50),
        "p95": percentile_sorted(&sorted, 0.95),
        "max": sorted[count - 1],
        "stddev": variance.sqrt()
    })
}

fn percentile_sorted(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = percentile.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let fraction = position - lower as f64;
        sorted[lower] + (sorted[upper] - sorted[lower]) * fraction
    }
}

fn compare_numeric_values(
    baseline_values: &[f64],
    candidate_values: &[f64],
    direction: &str,
    materiality_pct: f64,
) -> Value {
    let baseline = numeric_summary(baseline_values);
    let candidate = numeric_summary(candidate_values);
    let baseline_mean = baseline.get("mean").and_then(Value::as_f64).unwrap_or(0.0);
    let candidate_mean = candidate.get("mean").and_then(Value::as_f64).unwrap_or(0.0);
    let mean_delta = candidate_mean - baseline_mean;
    let mean_delta_pct = if baseline_mean.abs() > f64::EPSILON {
        Some(mean_delta / baseline_mean.abs() * 100.0)
    } else {
        None
    };
    let baseline_median = baseline
        .get("median")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let candidate_median = candidate
        .get("median")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let baseline_p95 = baseline.get("p95").and_then(Value::as_f64).unwrap_or(0.0);
    let candidate_p95 = candidate.get("p95").and_then(Value::as_f64).unwrap_or(0.0);
    let baseline_stddev = baseline
        .get("stddev")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let candidate_stddev = candidate
        .get("stddev")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let pooled_stddev = ((baseline_stddev.powi(2) + candidate_stddev.powi(2)) / 2.0).sqrt();
    let standardized_mean_difference = if pooled_stddev > f64::EPSILON {
        Some(mean_delta / pooled_stddev)
    } else {
        None
    };
    let classification = match (direction, mean_delta_pct) {
        ("neutral", _) => "not_classified",
        (_, None) => "not_classified",
        (_, Some(delta)) if delta.abs() < materiality_pct => "no_material_change",
        ("lower_is_better", Some(delta)) if delta < 0.0 => "improved",
        ("lower_is_better", Some(_)) => "degraded",
        ("higher_is_better", Some(delta)) if delta > 0.0 => "improved",
        ("higher_is_better", Some(_)) => "degraded",
        _ => "not_classified",
    };
    json!({
        "direction": direction,
        "materiality_pct": materiality_pct,
        "classification": classification,
        "baseline": baseline,
        "candidate": candidate,
        "mean_delta": mean_delta,
        "mean_delta_pct": mean_delta_pct,
        "median_delta": candidate_median - baseline_median,
        "p95_delta": candidate_p95 - baseline_p95,
        "standardized_mean_difference": standardized_mean_difference,
        "interpretation_guardrail": if direction == "neutral" {
            "No improvement/degradation label was assigned because metric direction was neutral."
        } else {
            "The label follows the requested direction and materiality threshold; it does not establish causality or equivalent workload mix."
        }
    })
}

fn compact_project_period(project: &ProjectData) -> Value {
    let manifest = project.dataset_manifest();
    json!({
        "project_id": project.project_id.as_str(),
        "snapshots": manifest["snapshots"],
        "begin_snap_id": manifest["begin_snap_id"],
        "end_snap_id": manifest["end_snap_id"],
        "begin_time": manifest["begin_time"],
        "end_time": manifest["end_time"],
        "database": manifest["database"]
    })
}

fn sql_metric_values(collection: &AWRSCollection, sql_id: &str, metric: &str) -> Vec<f64> {
    collection
        .awrs
        .iter()
        .filter_map(|awr| match metric {
            "elapsed_time_per_exec_s" => awr
                .sql_elapsed_time
                .iter()
                .find(|sql| sql.sql_id == sql_id)
                .map(|sql| sql.elpased_time_exec_s),
            "elapsed_time_s" => awr
                .sql_elapsed_time
                .iter()
                .find(|sql| sql.sql_id == sql_id)
                .map(|sql| sql.elapsed_time_s),
            "executions" => awr
                .sql_elapsed_time
                .iter()
                .find(|sql| sql.sql_id == sql_id)
                .map(|sql| sql.executions as f64),
            "cpu_time_per_exec_s" => awr.sql_cpu_time.get(sql_id).map(|sql| sql.cpu_time_exec_s),
            "cpu_time_s" => awr.sql_cpu_time.get(sql_id).map(|sql| sql.cpu_time_s),
            "io_time_per_exec_s" => awr.sql_io_time.get(sql_id).map(|sql| sql.io_time_exec_s),
            "io_time_s" => awr.sql_io_time.get(sql_id).map(|sql| sql.io_time_s),
            "buffer_gets_per_exec" => awr.sql_gets.get(sql_id).map(|sql| sql.gets_per_exec),
            "physical_reads_per_exec" => awr.sql_reads.get(sql_id).map(|sql| sql.reads_per_exec),
            _ => None,
        })
        .filter(|value| value.is_finite())
        .collect()
}

fn sql_comparison_summary(project: &ProjectData, sql_id: &str) -> Value {
    let mut snapshots = BTreeSet::new();
    let mut modules = BTreeSet::new();
    let mut plan_hash_values = BTreeSet::new();
    for awr in &project.collection.awrs {
        if let Some(sql) = awr.sql_elapsed_time.iter().find(|sql| sql.sql_id == sql_id) {
            snapshots.insert(awr.snap_info.begin_snap_id);
            if !sql.sql_module.trim().is_empty() {
                modules.insert(sql.sql_module.clone());
            }
        }
        if let Some(sql) = awr.sql_cpu_time.get(sql_id) {
            snapshots.insert(awr.snap_info.begin_snap_id);
            if !sql.sql_module.trim().is_empty() {
                modules.insert(sql.sql_module.clone());
            }
        }
        if awr.sql_io_time.contains_key(sql_id)
            || awr.sql_gets.contains_key(sql_id)
            || awr.sql_reads.contains_key(sql_id)
        {
            snapshots.insert(awr.snap_info.begin_snap_id);
        }
        if let Some(top) = awr.top_sql_with_top_events.get(sql_id) {
            snapshots.insert(awr.snap_info.begin_snap_id);
            if top.plan_hash_value != 0 {
                plan_hash_values.insert(top.plan_hash_value);
            }
        }
    }
    let metric_names = [
        "elapsed_time_per_exec_s",
        "elapsed_time_s",
        "executions",
        "cpu_time_per_exec_s",
        "cpu_time_s",
        "io_time_per_exec_s",
        "io_time_s",
        "buffer_gets_per_exec",
        "physical_reads_per_exec",
    ];
    let metrics = metric_names
        .iter()
        .map(|metric| {
            (
                (*metric).to_string(),
                numeric_summary(&sql_metric_values(&project.collection, sql_id, metric)),
            )
        })
        .collect::<Map<_, _>>();
    json!({
        "project_id": project.project_id.as_str(),
        "snapshots_total": project.collection.awrs.len(),
        "snapshots_with_sql": snapshots.len(),
        "coverage_pct": if project.collection.awrs.is_empty() { 0.0 } else { snapshots.len() as f64 / project.collection.awrs.len() as f64 * 100.0 },
        "modules": modules,
        "plan_hash_values_from_top_event_rows": plan_hash_values,
        "sql_text_available": project.collection.sql_text.contains_key(sql_id),
        "metrics": metrics
    })
}

fn sql_metric_comparisons(
    baseline: &AWRSCollection,
    candidate: &AWRSCollection,
    sql_id: &str,
    materiality_pct: f64,
) -> Value {
    let definitions = [
        ("elapsed_time_per_exec_s", "lower_is_better"),
        ("cpu_time_per_exec_s", "lower_is_better"),
        ("io_time_per_exec_s", "lower_is_better"),
        ("buffer_gets_per_exec", "lower_is_better"),
        ("physical_reads_per_exec", "lower_is_better"),
        ("elapsed_time_s", "neutral"),
        ("cpu_time_s", "neutral"),
        ("io_time_s", "neutral"),
        ("executions", "neutral"),
    ];
    Value::Array(
        definitions
            .iter()
            .map(|(metric, direction)| {
                let baseline_values = sql_metric_values(baseline, sql_id, metric);
                let candidate_values = sql_metric_values(candidate, sql_id, metric);
                if baseline_values.is_empty() || candidate_values.is_empty() {
                    json!({
                        "metric": metric,
                        "available_in_both": false,
                        "baseline_samples": baseline_values.len(),
                        "candidate_samples": candidate_values.len()
                    })
                } else {
                    json!({
                        "metric": metric,
                        "available_in_both": true,
                        "comparison": compare_numeric_values(&baseline_values, &candidate_values, direction, materiality_pct)
                    })
                }
            })
            .collect(),
    )
}

#[derive(Clone)]
struct JasminMcpServer {
    runtime: AnalysisRuntime,
    tools: Arc<Vec<Tool>>,
}

impl JasminMcpServer {
    fn new(runtime: AnalysisRuntime) -> Self {
        let tools = Arc::new(build_mcp_tools(&runtime));
        Self { runtime, tools }
    }
}

fn tools_list_result(tools: Vec<Tool>, include_cache_hints: bool) -> ListToolsResult {
    let result = ListToolsResult::with_all_items(tools);
    if include_cache_hints {
        // MCP 2026-07-28 requires explicit cache hints on paginated list results.
        // Keep the scope private so a client cannot reuse this server-specific
        // catalog across different users or authorization contexts.
        result
            .with_ttl_ms(MCP_TOOLS_LIST_TTL_MS)
            .with_cache_scope(CacheScope::Private)
    } else {
        result
    }
}

fn supports_tools_list_cache_hints(protocol_version: Option<&ProtocolVersion>) -> bool {
    protocol_version
        .is_some_and(|version| version.as_str() >= ProtocolVersion::V_2026_07_28.as_str())
}

impl ServerHandler for JasminMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(
            Implementation::new("jas-min-mcp", env!("CARGO_PKG_VERSION"))
                .with_title("JAS-MIN Oracle Performance Analysis")
                .with_description(
                    "Interactive evidence-backed analysis of parsed Oracle AWR and STATSPACK data",
                ),
        )
        .with_instructions(format!(
            "This server has {} loaded performance project(s). Call list_performance_projects first when more than one project is available, then call start_performance_analysis with the intended project_ids. Pass analysis_id to every later tool and project_id to project-specific evidence calls in comparative sessions. Use compare_project_metric and compare_project_sql for normalized cross-project evidence. Use narrow evidence calls and compare peaks with quiet baselines. Diagnostic guidance is methodology, never observed evidence. On AIX, obtain entitlement evidence before a CPU-pressure conclusion. Distinguish latency from workload volume, correlation from causation, and unknown from absent. Every finding must synthesize the measured symptom into a mechanism, temporal pattern, named affected workload and explicit evidence limitation; a conclusion plus a table dump is incomplete. Store findings with evidence_refs plus a reader-facing evidence_summary containing exact values. Every recommendation must name an owner and priority, explain why it follows from the finding, and define a measurable success criterion. Complete every stable category. Record gradients, anomalies and anomaly clusters as separate table kinds and add analytic_signal_synthesis when multiple families are available. For every foreground wait reaching 10% DB Time, call get_wait_event_sql_contributors and record the wait-to-SQL relationships; follow the strongest material contributor through SQL text, timeline and plan applicability. Correlation or ASH attribution is association evidence, not blocker/waiter proof. Inspect every supplied execution artifact. Review every unique SQL plan hash, but classify PL/SQL entry points as not_applicable_plsql because a top-level row-source plan is not expected; profile their inner SQL instead of requesting DBMS_XPLAN recapture. Choose an explicit recommendation type with artifact-specific rationale and action; generic 'validate actual rows' prose is rejected. Inspect every child-cursor diagnostic. Parse every non-empty alert attachment with include_parse_error_details=true, reproduce every error_summary code, and cite parse-error evidence in an SQL finding. Record every segment hotspot and a cross-statistic segment_synthesis. Review every collected performance parameter value; missing parameters require no row and only concern/critical ratings are reader-facing. get_report_status lists every missing item and blocks finalization until the deterministic lists are empty. In comparative prose, label every project or instance value explicitly; never use an unlabeled X/Y shorthand. Treat a zero-byte attachment as missing coverage. Use each alert attachment's observed first/last timestamp rather than assuming AWR-period coverage. A zero-match literal proves only that exact filter. If guidance is applied, include a verified verbatim quotation. Complete mandatory assessments and finish through finalize_report. For HTML, finalize Markdown first and pass it unchanged to convert_markdown_to_html. Reader-facing material waits and SQL_IDs must link to every existing project-specific detail report with meaningful instance labels.",
            self.runtime.projects.len()
        ))
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<ListToolsResult, McpError>> + Send + '_ {
        let protocol_version = context.protocol_version();
        let include_cache_hints = supports_tools_list_cache_hints(protocol_version.as_ref());
        std::future::ready(Ok(tools_list_result(
            self.tools.as_ref().clone(),
            include_cache_hints,
        )))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|tool| tool.name == name).cloned()
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<CallToolResponse, McpError>> + Send + '_ {
        async move {
            let name = request.name.to_string();
            let arguments = request.arguments.unwrap_or_default();
            let analysis_id = arguments
                .get("analysis_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            let request_bytes = serialized_json_size(&arguments);
            let mut call_log = McpToolCallLog::start(
                context.id.to_string(),
                name.clone(),
                analysis_id,
                request_bytes,
            );
            let result = match self.runtime.call_tool(&name, arguments) {
                Ok(value) => {
                    call_log.succeed(serialized_json_size(&value));
                    CallToolResult::structured(value)
                }
                Err(value) => {
                    let error_code = value
                        .get("error_code")
                        .and_then(Value::as_str)
                        .unwrap_or("UNKNOWN_TOOL_ERROR");
                    call_log.fail(serialized_json_size(&value), error_code);
                    CallToolResult::structured_error(value)
                }
            };
            Ok(result.into())
        }
    }

    fn list_prompts(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<ListPromptsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListPromptsResult::with_all_items(vec![
            analysis_prompt_definition(),
        ])))
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<GetPromptResponse, McpError>> + Send + '_ {
        std::future::ready(if request.name == "oracle_performance_analysis" {
            let arguments = request.arguments.unwrap_or_default();
            let language = arguments
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or("EN");
            let focus = arguments
                .get("focus")
                .and_then(Value::as_str)
                .unwrap_or("the complete Oracle performance profile");
            Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                Role::User,
                format!(
                    "Investigate {focus} using the JAS-MIN MCP server. Begin with list_performance_projects when multiple projects may be loaded, then call start_performance_analysis with the intended project_ids and use its analysis_id for all evidence calls. In comparative sessions pass project_id to project-specific tools and use compare_project_metric or compare_project_sql for cross-project evidence. Form competing hypotheses and falsify them with timelines, snapshots, SQL text, plans, child-cursor reasons, alert log and AIX evidence when available. Fetch reasonings.txt guidance only for concrete symptoms and never cite it as measurement evidence. Store evidence-backed findings with exact reader-facing evidence summaries instead of exposing raw evidence IDs as prose. Every applied guidance reference requires a verbatim quote from the retrieved section. Complete every mandatory assessment, validate report status and finalize the stable report. Write finding content in {language}. If the user requests HTML, finalize Markdown output first and pass the returned Markdown unchanged to convert_markdown_to_html; ensure comparative output links every source project report."
                ),
            )])
            .with_description("Tool-first Oracle performance investigation workflow")
            .into())
        } else {
            Err(McpError::invalid_params(
                format!("Unknown prompt '{}'", request.name),
                None,
            ))
        })
    }
}

/// Starts the MCP endpoint after JAS-MIN has completed parsing and analysis.
#[tokio::main]
pub async fn run_mcp_server(runtime: AnalysisRuntime, endpoint: McpEndpoint) -> Result<()> {
    let cancellation = CancellationToken::new();
    let factory_runtime = runtime.clone();
    let host = endpoint.address.ip().to_string();
    let port = endpoint.address.port();
    let allowed_hosts = [
        host.clone(),
        format!("{host}:{port}"),
        "localhost".to_string(),
        format!("localhost:{port}"),
    ];
    let allowed_origins = [
        format!("http://{host}:{port}"),
        format!("http://localhost:{port}"),
    ];
    let service: StreamableHttpService<JasminMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(JasminMcpServer::new(factory_runtime.clone())),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default()
                .with_allowed_hosts(allowed_hosts)
                .with_allowed_origins(allowed_origins)
                .with_cancellation_token(cancellation.child_token()),
        );
    let router = axum::Router::new().nest_service(&endpoint.path, service);
    let listener = tokio::net::TcpListener::bind(endpoint.address)
        .await
        .with_context(|| format!("cannot bind MCP endpoint {}", endpoint.url()))?;
    println!(
        "{} [MCP] status=READY endpoint={}",
        mcp_log_timestamp(),
        endpoint.url()
    );
    println!(
        "   {} parsed performance project(s) and their statistical analyses are retained in memory.",
        runtime.projects.len()
    );
    println!("   Tool calls are logged with UTC timestamps, result status, and duration.");
    println!("   Press Ctrl-C to stop the server.");
    axum::serve(listener, router)
        .with_graceful_shutdown({
            let cancellation = cancellation.clone();
            async move {
                let _ = tokio::signal::ctrl_c().await;
                cancellation.cancel();
            }
        })
        .await
        .context("MCP HTTP server failed")?;
    Ok(())
}

fn build_mcp_tools(runtime: &AnalysisRuntime) -> Vec<Tool> {
    let mut evidence_definitions = BTreeMap::new();
    for project in runtime.projects.values() {
        for definition in tools_schema(project.stem.as_str())
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(name) = definition.pointer("/function/name").and_then(Value::as_str) {
                evidence_definitions
                    .entry(name.to_string())
                    .or_insert_with(|| definition.clone());
            }
        }
    }
    let mut tools = evidence_definitions
        .values()
        .filter_map(|definition| openai_definition_to_mcp(definition, true, true, true))
        .collect::<Vec<_>>();
    tools.extend(mcp_control_definitions().iter().filter_map(|definition| {
        let name = definition.pointer("/function/name")?.as_str()?;
        let requires_analysis = !matches!(
            name,
            "list_performance_projects" | "start_performance_analysis"
        );
        let supports_project_id = matches!(
            name,
            "get_precomputed_analysis" | "get_wait_event_sql_contributors"
        );
        let read_only = matches!(
            name,
            "list_performance_projects"
                | "get_analysis_catalog"
                | "get_precomputed_analysis"
                | "get_wait_event_sql_contributors"
                | "get_diagnostic_guidance"
                | "compare_project_metric"
                | "compare_project_sql"
                | "get_report_status"
        );
        openai_definition_to_mcp(
            definition,
            requires_analysis,
            supports_project_id,
            read_only,
        )
    }));
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

fn openai_definition_to_mcp(
    definition: &Value,
    requires_analysis_id: bool,
    supports_project_id: bool,
    read_only: bool,
) -> Option<Tool> {
    let function = definition.get("function")?;
    let name = function.get("name")?.as_str()?.to_string();
    let description = function.get("description")?.as_str()?.to_string();
    let mut input_schema = function
        .get("parameters")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| Map::from_iter([("type".to_string(), json!("object"))]));
    if requires_analysis_id {
        let properties = input_schema
            .entry("properties".to_string())
            .or_insert_with(|| json!({}));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert(
                "analysis_id".to_string(),
                json!({
                    "type": "string",
                    "description": "Explicit session handle returned by start_performance_analysis"
                }),
            );
        }
        let required = input_schema
            .entry("required".to_string())
            .or_insert_with(|| json!([]));
        if let Some(required) = required.as_array_mut() {
            if !required.iter().any(|value| value == "analysis_id") {
                required.push(json!("analysis_id"));
            }
        }
    }
    if supports_project_id {
        let properties = input_schema
            .entry("properties".to_string())
            .or_insert_with(|| json!({}));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert(
                "project_id".to_string(),
                json!({
                    "type": "string",
                    "description": "Project handle returned by list_performance_projects. Required when the analysis contains multiple projects."
                }),
            );
        }
    }
    let output_schema = Arc::new(Map::from_iter([
        ("type".to_string(), json!("object")),
        ("additionalProperties".to_string(), json!(true)),
    ]));
    Some(
        Tool::new(name.clone(), description, Arc::new(input_schema))
            .with_title(name.replace('_', " "))
            .with_raw_output_schema(output_schema)
            .with_annotations(
                ToolAnnotations::new()
                    .read_only(read_only)
                    .destructive(false)
                    .idempotent(read_only)
                    .open_world(false),
            ),
    )
}

fn mcp_control_definitions() -> Vec<Value> {
    vec![
        function_definition(
            "list_performance_projects",
            "Mandatory discovery call when the server contains multiple projects. Returns stable project IDs, time ranges, database identity, sample counts and attachment availability without creating an analysis session.",
            json!({"type": "object", "additionalProperties": false, "properties": {}}),
        ),
        function_definition(
            "start_performance_analysis",
            "Creates an explicit single-project or comparative analysis session and returns project manifests, statistical capabilities, compact seeds, diagnostic quality gates and the stable report contract. With multiple loaded projects, omit selection to analyze all projects or pass project_ids explicitly.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "project_id": {"type": "string", "description": "Select one project. Do not combine with project_ids."},
                    "project_ids": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 32, "uniqueItems": true, "description": "Projects included in one comparative analysis. Omit to select every loaded project."},
                    "focus": {"type": "string", "description": "Optional investigation focus supplied by the user"},
                    "language": {"type": "string", "description": "Preferred report language, default EN"},
                    "audience": {"type": "string", "enum": ["technical", "management", "mixed"], "default": "mixed"}
                }
            }),
        ),
        function_definition(
            "get_analysis_catalog",
            "Returns the calculation catalog, data availability, guidance catalog and report contract for an existing analysis session.",
            json!({"type": "object", "properties": {}}),
        ),
        function_definition(
            "get_precomputed_analysis",
            "Fetches a bounded precomputed ReportForAI section. Use aggregate statistical evidence before drilling into raw snapshots.",
            json!({
                "type": "object",
                "properties": {
                    "section": {"type": "string", "enum": ["foreground_waits", "background_waits", "top_sqls", "io_summary", "latches", "segment_hotspots", "instance_stat_correlations", "load_profile_anomalies", "anomaly_clusters", "initialization_parameters", "full_gradients", "db_time_degradation", "performance_peaks"]},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20}
                },
                "required": ["section"]
            }),
        ),
        function_definition(
            "get_wait_event_sql_contributors",
            "Returns SQL_IDs associated with one foreground wait through positive aligned Pearson correlation and/or direct ASH attribution. Results include exact relationship strength, SQL workload context, plan applicability and attachment coverage. Use it for every material wait before writing a wait finding; correlation is not blocker/waiter proof.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "event_name": {"type": "string", "description": "Exact foreground wait-event name."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 20, "default": 5}
                },
                "required": ["event_name"]
            }),
        ),
        function_definition(
            "get_diagnostic_guidance",
            "Retrieves relevant reasonings.txt sections for a concrete symptom. Guidance is methodology, never measurement evidence; verify every trigger with evidence tools.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "topic": {"type": "string", "description": "Exact section ID such as §1.1 or a concrete Oracle symptom"},
                    "max_sections": {"type": "integer", "minimum": 1, "maximum": 5, "default": 3}
                },
                "required": ["topic"]
            }),
        ),
        function_definition(
            "compare_project_metric",
            "Compares one observed metric distribution between a baseline project and a candidate project. Returns sample coverage, mean, median, p95, standard deviation, absolute and relative deltas, standardized mean difference and an optional direction-aware improvement/degradation label. Missing samples are never converted to zero.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "baseline_project_id": {"type": "string"},
                    "candidate_project_id": {"type": "string"},
                    "kind": {"type": "string", "enum": ["load_profile", "instance_stat", "wait_event_fg", "wait_event_bg", "time_model", "host_cpu", "io_stats_byfunc"]},
                    "name": {"type": "string", "description": "Metric, statistic, event, or I/O function name. For host_cpu use host_cpu."},
                    "field": {"type": "string", "description": "Optional field accepted by get_metric_time_series."},
                    "direction": {"type": "string", "enum": ["neutral", "lower_is_better", "higher_is_better"], "default": "neutral", "description": "Controls only the improved/degraded label. Use neutral when workload volume or metric semantics make direction ambiguous."},
                    "materiality_pct": {"type": "number", "minimum": 0, "maximum": 1000, "default": 5}
                },
                "required": ["baseline_project_id", "candidate_project_id", "kind", "name"]
            }),
        ),
        function_definition(
            "compare_project_sql",
            "Compares the same SQL_ID between baseline and candidate projects. Separates per-execution efficiency metrics from workload totals, reports coverage and observed plan hashes, and never treats missing SQL samples as zero.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "baseline_project_id": {"type": "string"},
                    "candidate_project_id": {"type": "string"},
                    "sql_id": {"type": "string"},
                    "materiality_pct": {"type": "number", "minimum": 0, "maximum": 1000, "default": 5}
                },
                "required": ["baseline_project_id", "candidate_project_id", "sql_id"]
            }),
        ),
        function_definition(
            "configure_report",
            "Sets the session-scoped output format and detail profile while retaining the stable server-controlled core section order.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "output_format": {"type": "string", "enum": ["markdown", "json", "both"], "default": "both"},
                    "language": {"type": "string"},
                    "audience": {"type": "string", "enum": ["technical", "management", "mixed"]},
                    "detail_level": {"type": "string", "enum": ["compact", "standard", "deep"]},
                    "detail_overrides": {"type": "object", "additionalProperties": {"type": "string", "enum": ["compact", "standard", "deep"]}},
                    "include_evidence_appendix": {"type": "boolean"},
                    "include_guidance_appendix": {"type": "boolean"}
                }
            }),
        ),
        function_definition(
            "record_finding",
            "Creates or replaces one evidence-backed report finding. Every finding must connect the measured symptom to a mechanism, temporal pattern and affected workload, then state the evidence boundary. The human-readable evidence_summary must state exact supporting values. Evidence and guidance references must have been obtained in this analysis session; every applied guidance reference requires a verified verbatim quotation.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "finding_id": {"type": "string", "description": "Reuse a returned finding_id to replace the finding"},
                    "category": {"type": "string", "enum": REPORT_CATEGORIES},
                    "title": {"type": "string"},
                    "severity": {"type": "string", "enum": ["critical", "high", "medium", "low", "informational"]},
                    "confidence": {"type": "string", "enum": ["high", "medium", "low", "unknown"]},
                    "conclusion": {"type": "string"},
                    "mechanism": {"type": "string", "description": "How the measured signals connect. Separate a demonstrated mechanism from a hypothesis and do not restate the conclusion."},
                    "temporal_pattern": {"type": "string", "description": "When the symptom occurs: peak snapshots or timestamps, recurrence, baseline direction and whether it is episodic or sustained."},
                    "affected_workload": {"type": "string", "description": "Named SQL_IDs, PL/SQL units, modules, services, objects or business paths affected. State explicitly when attribution is not available."},
                    "evidence_limitations": {"type": "string", "description": "Counterevidence and the precise causal or coverage boundary: what the cited evidence cannot prove and what runtime proof remains necessary."},
                    "evidence_summary": {"type": "string", "description": "Human-readable evidence basis with exact values, time scope, and project/instance context; never just evidence IDs or tool names."},
                    "details": {"type": "string"},
                    "evidence_refs": {"type": "array", "items": {"type": "string"}},
                    "guidance_refs": {"type": "array", "items": {"type": "string"}},
                    "guidance_quotes": {"type": "array", "items": {"type": "object", "additionalProperties": false, "properties": {"guidance_ref": {"type": "string"}, "quote": {"type": "string", "description": "Contiguous verbatim excerpt from the retrieved guidance section."}}, "required": ["guidance_ref", "quote"]}},
                    "recommendations": {"type": "array", "items": {"type": "object", "additionalProperties": false, "properties": {"owner": {"type": "string", "enum": ["DBA", "Developer", "Management"]}, "priority": {"type": "string", "enum": ["immediate", "high", "medium", "low"]}, "action": {"type": "string"}, "rationale": {"type": "string", "description": "Why this action follows from the measured finding and why it has this priority."}, "success_criterion": {"type": "string", "description": "A measurable before/after acceptance criterion including the relevant metric and regression guard."}}, "required": ["owner", "priority", "action", "rationale", "success_criterion"]}}
                },
                "required": ["category", "title", "severity", "confidence", "conclusion", "mechanism", "temporal_pattern", "affected_workload", "evidence_limitations", "evidence_summary", "evidence_refs"]
            }),
        ),
        function_definition(
            "record_report_table",
            "Creates or replaces one structured analysis block. Runtime validation enforces exact columns, project scope, enumerated values, evidence provenance and entity matching. Gradient, anomaly and cluster signals are separate kinds with a mandatory cross-signal synthesis when multiple families exist. Execution-plan rows require a concrete recommendation or evidence-backed no_change outcome. Alert rows reproduce deterministic error_summary values. Parameter rows cover collected values internally; the renderer exposes only concern/critical ratings.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "table_id": {"type": "string", "description": "Reuse a returned table_id to replace the complete table"},
                    "kind": {"type": "string", "enum": ["gradients", "anomalies", "anomaly_clusters", "analytic_signal_synthesis", "execution_plans", "wait_sql_contributors", "child_cursors", "alert_log_errors", "segments", "segment_synthesis", "parameters"]},
                    "title": {"type": "string"},
                    "rows": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 500,
                        "items": {"type": "object", "description": "Exact row fields depend on kind; include every required string field plus a non-empty evidence_refs string array. See report_contract."
                        }
                    }
                },
                "required": ["kind", "title", "rows"]
            }),
        ),
        function_definition(
            "set_report_assessment",
            "Records one mandatory final assessment with a human-readable evidence summary. Non-unknown conclusions must cite measurement evidence from this session; every applied guidance reference requires a verified verbatim quotation.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "assessment": {"type": "string", "enum": REQUIRED_ASSESSMENTS},
                    "status": {"type": "string", "enum": ["proven", "not_proven", "unknown"]},
                    "conclusion": {"type": "string"},
                    "evidence_summary": {"type": "string", "description": "Human-readable basis for the assessment, including exact observed values or the precise missing-data boundary."},
                    "evidence_refs": {"type": "array", "items": {"type": "string"}},
                    "guidance_refs": {"type": "array", "items": {"type": "string"}},
                    "guidance_quotes": {"type": "array", "items": {"type": "object", "additionalProperties": false, "properties": {"guidance_ref": {"type": "string"}, "quote": {"type": "string", "description": "Contiguous verbatim excerpt from the retrieved guidance section."}}, "required": ["guidance_ref", "quote"]}}
                },
                "required": ["assessment", "status", "conclusion", "evidence_summary", "evidence_refs"]
            }),
        ),
        function_definition(
            "get_report_status",
            "Checks every stable category, mandatory assessment, evidence call, supplied plan and child-cursor attachment, alert-log error summary, segment-hotspot row and synthesis, separate analytic signal family and synthesis, and every collected parameter value before finalization. The returned missing lists are the deterministic completion checklist.",
            json!({"type": "object", "properties": {}}),
        ),
        function_definition(
            "finalize_report",
            "Validates the report contract and renders stable Markdown and/or structured JSON. Use allow_incomplete only when the user explicitly requests a draft.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "allow_incomplete": {"type": "boolean", "default": false}
                }
            }),
        ),
        function_definition(
            "convert_markdown_to_html",
            "Validates a complete 11-section JAS-MIN Markdown report and creates a new HTML file with the shared responsive audit presentation. Every table is placed in a keyboard-focusable horizontal overflow region with a visible scrollbar; execution-plan recommendations render as self-contained interactive row-source graphs. Finalize Markdown first and pass it unchanged. Existing files are never overwritten and the server does not open a browser.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "markdown": {
                        "type": "string",
                        "description": "Complete Markdown report returned by finalize_report or authored with the exact stable title and 11 section headings. Maximum 4 MiB."
                    },
                    "output_filename": {
                        "type": "string",
                        "description": "Optional basename created in the JAS-MIN working directory. Path separators are rejected. The .html extension is appended when omitted."
                    }
                },
                "required": ["markdown"]
            }),
        ),
    ]
}

fn function_definition(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters
        }
    })
}

fn analysis_prompt_definition() -> Prompt {
    Prompt::new(
        "oracle_performance_analysis",
        Some("Start a complete tool-first Oracle AWR/STATSPACK performance investigation"),
        Some(vec![
            PromptArgument::new("language")
                .with_description("Language used for finding content, for example EN or PL"),
            PromptArgument::new("focus")
                .with_description("Optional performance symptom or business question"),
        ]),
    )
    .with_title("Oracle performance analysis")
}

/// Builds a bounded first-turn seed; full analytical sections remain available
/// through `get_precomputed_analysis` and the narrow evidence tools.
fn mcp_bootstrap_seed(report: &ReportForAI) -> Value {
    let full = build_case_seed(report);
    let degradation = project_fields(
        full.get("db_time_degradation").unwrap_or(&Value::Null),
        &[
            "is_degradation_detected",
            "verdict",
            "baseline_start",
            "baseline_end",
            "degraded_start",
            "degraded_end",
            "baseline_samples",
            "degraded_samples",
            "db_time_baseline_avg",
            "db_time_degraded_avg",
            "db_time_delta_avg",
            "db_time_delta_pct",
            "db_time_robust_z_score",
            "db_cpu_baseline_avg",
            "db_cpu_degraded_avg",
            "db_cpu_delta_pct",
            "dominant_domains",
            "findings_total",
        ],
    );
    let gradient_highlights = full
        .get("gradients")
        .and_then(Value::as_object)
        .map(|gradients| {
            gradients
                .iter()
                .map(|(name, section)| {
                    let value = if section.is_null() {
                        Value::Null
                    } else {
                        json!({
                            "counts": section.get("counts").cloned().unwrap_or(Value::Null),
                            "top_cross_model": project_array(
                                section.get("cross_model_classifications").unwrap_or(&Value::Null),
                                3,
                                &["event_name", "classification", "priority", "combined_impact", "combined_peak_impact"]
                            ),
                            "top_collinear_group": project_array(
                                section.get("collinear_group_impacts").unwrap_or(&Value::Null),
                                1,
                                &["group_members", "combined_impact"]
                            )
                        })
                    };
                    (name.clone(), value)
                })
                .collect::<Map<String, Value>>()
        })
        .map(Value::Object)
        .unwrap_or(Value::Null);

    json!({
        "evidence_id": SEED_EVIDENCE_ID,
        "ratio_definition": full.get("ratio_definition").cloned().unwrap_or(Value::Null),
        "performance_peaks_total": full.get("performance_peaks_total").cloned().unwrap_or(Value::Null),
        "representative_peaks": project_array(
            full.get("performance_peaks").unwrap_or(&Value::Null),
            6,
            &["report_date", "snap_id", "db_time_value", "db_cpu_value", "dbcpu_dbtime_ratio"]
        ),
        "db_time_degradation": degradation,
        "gradient_highlights": gradient_highlights,
        "drilldown_hint": "Call get_precomputed_analysis for full degradation, gradient, wait, SQL, I/O, latch, anomaly, segment, or parameter evidence."
    })
}

fn mcp_triage_preview(report: &ReportForAI) -> Value {
    json!({
        "available_counts": {
            "foreground_waits": report.top_foreground_wait_events.len(),
            "background_waits": report.top_background_wait_events.len(),
            "top_sqls": report.top_sqls_by_elapsed_time.len(),
            "io_functions": report.io_stats_by_function_summary.len(),
            "latches": report.latch_activity_summary.len(),
            "anomaly_clusters": report.anomaly_clusters.len(),
            "initialization_parameters": report.initialization_parameters.len()
        },
        "foreground_wait_index": project_serializable_rows(
            &report.top_foreground_wait_events,
            5,
            &["event_name", "avg_pct_of_dbtime", "avg_wait_for_execution_ms", "correlation_with_db_time", "marked_as_top_in_pct_of_probes"]
        ),
        "background_wait_index": project_serializable_rows(
            &report.top_background_wait_events,
            3,
            &["event_name", "avg_pct_of_dbtime", "avg_wait_for_execution_ms", "correlation_with_db_time", "marked_as_top_in_pct_of_probes"]
        ),
        "top_sql_index": project_serializable_rows(
            &report.top_sqls_by_elapsed_time,
            5,
            &["sql_id", "module", "sql_type", "avg_elapsed_time_by_exec", "avg_elapsed_time_cumulative_s", "correlation_with_db_time", "marked_as_top_in_pct_of_probes"]
        ),
        "io_function_names": report.io_stats_by_function_summary.iter().take(12).map(|row| row.function_name.as_str()).collect::<Vec<_>>()
    })
}

fn project_serializable_rows<T: Serialize>(rows: &[T], limit: usize, fields: &[&str]) -> Value {
    serde_json::to_value(rows)
        .map(|value| project_array(&value, limit, fields))
        .unwrap_or_else(|_| Value::Array(Vec::new()))
}

fn project_array(value: &Value, limit: usize, fields: &[&str]) -> Value {
    Value::Array(
        value
            .as_array()
            .into_iter()
            .flatten()
            .take(limit)
            .map(|row| project_fields(row, fields))
            .collect(),
    )
}

fn project_fields(value: &Value, fields: &[&str]) -> Value {
    let Some(object) = value.as_object() else {
        return Value::Null;
    };
    Value::Object(
        fields
            .iter()
            .filter_map(|field| {
                object
                    .get(*field)
                    .cloned()
                    .map(|value| ((*field).to_string(), value))
            })
            .collect(),
    )
}

fn calculation_catalog() -> Value {
    json!([
        {"id": "descriptive_statistics", "outputs": ["mean", "median", "standard deviation", "occurrence percentage", "percentiles"], "access": ["get_precomputed_analysis", "list_snapshots"], "caveat": "Magnitude and frequency describe workload shape; they do not establish root cause."},
        {"id": "db_cpu_db_time_ratio", "formula": "DB CPU / DB Time", "access": ["start_performance_analysis", "list_snapshots"], "caveat": "Describes workload composition. It cannot clear AIX LPAR CPU pressure without entitlement data."},
        {"id": "mad_anomalies", "outputs": ["global MAD", "sliding-window MAD", "top anomaly clusters"], "access": ["get_precomputed_analysis(load_profile_anomalies)", "get_precomputed_analysis(anomaly_clusters)"], "caveat": "An anomaly is a temporal deviation, not automatically a bottleneck."},
        {"id": "pearson_correlations", "threshold": "absolute rho >= 0.5 for selected summaries", "access": ["get_precomputed_analysis(instance_stat_correlations)", "get_metric_time_series"], "caveat": "Correlation must be temporally aligned and independently verified."},
        {"id": "multi_model_gradients", "models": ["Ridge", "Elastic Net", "Huber", "Quantile 95"], "primary_metrics": ["impact_active", "impact_peak", "impact_share", "cross_model_classification"], "access": ["get_precomputed_analysis(full_gradients)"], "caveat": "Use VIF and collinear-group evidence; near-zero baselines can create unstable percentage sensitivities."},
        {"id": "db_time_degradation", "method": "baseline versus recent window with robust change statistics", "outputs": ["delta", "delta_pct", "robust_z_score", "estimated_db_time_delta_share"], "access": ["get_precomputed_analysis(db_time_degradation)"], "caveat": "A degraded recent window identifies co-moving contributors, not automatic causality."},
        {"id": "timeline_and_baseline_comparison", "outputs": ["metric series", "SQL timeline", "wait timeline", "snapshot comparison", "wait histogram"], "access": ["get_metric_time_series", "get_sql_timeline", "get_wait_event_timeline", "compare_snapshots", "get_wait_event_histogram"], "caveat": "Always pair SNAP_ID with timestamp and compare a peak with a representative quiet baseline."},
        {"id": "cross_project_comparison", "outputs": ["sample coverage", "mean", "median", "p95", "standard deviation", "relative delta", "standardized mean difference", "direction-aware classification"], "access": ["compare_project_metric", "compare_project_sql"], "caveat": "A statistical change does not prove causality or equivalent workload mix. Missing samples are not zero, and improvement/degradation labels require explicit metric direction."}
    ])
}

fn quality_gates(platform: &str) -> Value {
    json!([
        {"gate": "cpu_pressure", "required": platform.to_lowercase().contains("aix"), "rule": "On AIX, inspect Entc%, physc/pc, entitled capacity, capped/shared mode and temporal alignment before deciding CPU pressure."},
        {"gate": "disk_quality", "required": true, "rule": "Separate measured latency from I/O request volume; inspect LGWR, DBWR, buffer-cache and direct-I/O evidence."},
        {"gate": "application_and_commit_policy", "required": true, "rule": "Do not infer bad application design or commit policy from high executions or waits alone; verify transaction, redo, latency and direct anti-pattern evidence."},
        {"gate": "sql_tuning", "required": true, "rule": "Inspect SQL text, timeline and plan applicability before concrete SQL tuning recommendations. A PL/SQL entry point has no expected top-level row-source plan; profile it and inspect its inner SQL."},
        {"gate": "cursor_contention", "required": true, "rule": "For every material cursor wait, record aligned SQL correlation/ASH contributors, then use holder/waiter, child-cursor and parse/reload/invalidation evidence before assigning causality."},
        {"gate": "parameter_changes", "required": true, "rule": "A parameter recommendation requires its observed current value and a causal performance rationale; missing means unknown."},
        {"gate": "reader_facing_provenance", "required": true, "rule": "Label every comparative value with its project or instance, resolve every report link, link each material wait-event name and SQL_ID directly to every existing project-specific detail report, scope attachment counts to observed first/last timestamps, and treat zero-byte attachments as missing coverage rather than clean evidence."}
    ])
}

fn recommended_next_calls(
    platform: &str,
    attachments: &Value,
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> Value {
    let mut calls = vec![
        json!({"tool": "get_database_load_summary", "reason": "establish the whole-window workload envelope"}),
        json!({"tool": "list_snapshots", "reason": "select peaks and quiet baselines"}),
    ];
    for &section in REQUIRED_PRECOMPUTED_SECTIONS {
        calls.push(json!({
            "tool": "get_precomputed_analysis",
            "arguments": {"section": section, "limit": 100},
            "reason": format!("mandatory report coverage for {section}")
        }));
    }
    calls.push(json!({
        "tool": "get_init_parameter",
        "arguments": {"names": PERFORMANCE_PARAMETER_CHECKLIST},
        "reason": "mandatory exact-value review of the performance parameter checklist"
    }));
    if platform.to_lowercase().contains("aix") {
        let mut arguments = Map::new();
        if let Some(date_from) = date_from {
            arguments.insert("date_from".to_string(), json!(date_from));
        }
        if let Some(date_to) = date_to {
            arguments.insert("date_to".to_string(), json!(date_to));
        }
        calls.push(json!({
            "tool": "get_aix_cpu_entitlement_summary",
            "arguments": arguments,
            "reason": "mandatory AIX CPU-capacity evidence aligned to the AWR interval"
        }));
    }
    if attachments
        .get("execution_plans")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > 0
    {
        calls.push(json!({"tool": "list_available_sql_plans", "reason": "discover plan evidence for material SQL IDs"}));
    }
    if attachments
        .get("child_cursor_reason_files")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > 0
    {
        calls.push(json!({"tool": "list_available_child_cursor_reasons", "reason": "discover direct child-cursor evidence"}));
    }
    if attachments
        .get("alert_logs_nonempty")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > 0
    {
        calls.push(json!({"tool": "get_alertlog_errors", "arguments": {"include_parse_error_details": true, "limit": 1000}, "reason": "mandatory complete alert-log summary, including decoded parse-error context"}));
    }
    Value::Array(calls)
}

fn add_project_id_to_calls(calls: Value, project_id: &str) -> Value {
    Value::Array(
        calls
            .as_array()
            .into_iter()
            .flatten()
            .cloned()
            .map(|mut call| {
                if let Some(object) = call.as_object_mut() {
                    let arguments = object
                        .entry("arguments".to_string())
                        .or_insert_with(|| json!({}));
                    if let Some(arguments) = arguments.as_object_mut() {
                        arguments.insert("project_id".to_string(), json!(project_id));
                    }
                }
                call
            })
            .collect(),
    )
}

fn mcp_project_id_from_stem(stem: &str) -> String {
    let raw = Path::new(stem)
        .file_stem()
        .or_else(|| Path::new(stem).file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("project");
    let mut id = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    let id = id.trim_matches('-');
    if id.is_empty() {
        "project".to_string()
    } else {
        id.chars().take(80).collect()
    }
}

fn oracle_snapshot_date(value: &str) -> Option<String> {
    [
        "%d-%b-%y %H:%M:%S",
        "%d-%b-%Y %H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ]
    .iter()
    .find_map(|format| chrono::NaiveDateTime::parse_from_str(value.trim(), format).ok())
    .map(|date_time| date_time.date().format("%Y-%m-%d").to_string())
}

fn report_contract(config: &ReportConfig) -> Value {
    json!({
        "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
        "config": config,
        "stable_sections": [
            {"number": 1, "id": "executive_summary", "title": "Executive Summary"},
            {"number": 2, "id": "performance_profile", "title": "Overall Performance Profile and DB Time Degradation"},
            {"number": 3, "id": "wait_events", "title": "Wait Events"},
            {"number": 4, "id": "sql", "title": "SQL-Level Analysis"},
            {"number": 5, "id": "segments", "title": "Segments and Objects"},
            {"number": 6, "id": "latches", "title": "Latches and Internal Contention"},
            {"number": 7, "id": "io", "title": "I/O and Disk Assessment"},
            {"number": 8, "id": "undo_redo", "title": "UNDO, Redo and Load Profile"},
            {"number": 9, "id": "gradients_anomalies", "title": "Gradient and Anomaly Synthesis"},
            {"number": 10, "id": "parameters", "title": "Relevant Initialization Parameters"},
            {"number": 11, "id": "recommendations", "title": "Prioritized Actions and Mandatory Assessments"}
        ],
        "required_finding_categories": REQUIRED_REPORT_CATEGORIES,
        "diagnostic_synthesis_policy": {
            "required_fields": ["mechanism", "temporal_pattern", "affected_workload", "evidence_limitations"],
            "narrative_order": "Lead with the diagnosis. Connect symptom to mechanism, named workload and time pattern before the exhaustive structured evidence tables.",
            "causality_boundary": "Correlation, gradient selection and ASH attribution are association evidence. evidence_limitations must state the missing runtime proof and relevant counterevidence.",
            "executive_summary": "The five leading findings include mechanism, workload scope, timing and evidence boundary; a one-sentence register alone is insufficient."
        },
        "recommendation_policy": {
            "required_fields": ["owner", "priority", "action", "rationale", "success_criterion"],
            "rendering": "Group actions by accountable owner and show why the action follows plus a measurable acceptance criterion."
        },
        "required_precomputed_sections": REQUIRED_PRECOMPUTED_SECTIONS,
        "required_structured_table_kinds": REQUIRED_STRUCTURED_TABLE_KINDS,
        "performance_parameter_checklist": PERFORMANCE_PARAMETER_CHECKLIST,
        "artifact_coverage_policy": {
            "execution_plans": "Every supplied .xplan capture must be classified before review. SQL plans require one row per unique plan hash. PL/SQL entry points are not expected to have a top-level row-source plan and use not_applicable_plsql plus inner-SQL/PLSQL profiling guidance, never recapture. A material wait's strongest SQL contributor also requires plan applicability review; a missing attachment is stated explicitly. Actionable recommendations cite observed workload evidence.",
            "wait_sql_contributors": "For every foreground wait that reaches 10% DB Time in any snapshot, call get_wait_event_sql_contributors and record the returned SQL relationships. Positive aligned correlation and ASH attribution are shown separately; neither is presented as blocker/waiter proof. The strongest material SQL is followed through SQL text, timeline and plan applicability when the plan tool is available.",
            "child_cursors": "Every supplied .shared_cursor_reasons attachment must be inspected and represented by a project/SQL_ID row in the child-cursor analysis table.",
            "alert_log_errors": "Every non-empty alert attachment must be parsed with include_parse_error_details=true. Every deterministic error_summary code requires a structured row, and parse-error evidence must be cited by an SQL-level finding before finalization.",
            "segments": "Every object in every non-empty precomputed segment-hotspot category must be represented. Detail is rendered as a separate table per statistic, followed by a required cross-statistic synthesis.",
            "gradients_anomalies": "Full gradients, load-profile anomalies and anomaly clusters are separate table kinds. The five highest-impact cross-model rows per gradient family are mandatory. Every foreground wait reaching 10% DB Time must be represented as triangulated or explicitly material_not_selected. When two or more analytic families exist, cross-signal synthesis is mandatory.",
            "parameters": "Every collected checklist value is reviewed internally. Missing values are not turned into report rows, and the human report renders only concern/critical ratings."
        },
        "structured_table_schemas": {
            "gradients": {
                "category": "gradients_anomalies",
                "required_string_fields": GRADIENT_TABLE_COLUMNS.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
                "selection_status_enum": ["triangulated", "material_not_selected"],
                "coverage_policy": "Top five cross-model classifications per family plus every foreground wait whose maximum snapshot contribution reaches 10% DB Time."
            },
            "anomalies": {
                "category": "gradients_anomalies",
                "required_string_fields": ANOMALY_TABLE_COLUMNS.iter().map(|(key, _)| *key).collect::<Vec<_>>()
            },
            "anomaly_clusters": {
                "category": "gradients_anomalies",
                "required_string_fields": ANOMALY_CLUSTER_TABLE_COLUMNS.iter().map(|(key, _)| *key).collect::<Vec<_>>()
            },
            "analytic_signal_synthesis": {
                "category": "gradients_anomalies",
                "required_string_fields": ANALYTIC_SYNTHESIS_TABLE_COLUMNS.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
                "evidence_policy": "Every row must cite at least two distinct analytic signal families."
            },
            "execution_plans": {
                "category": "sql",
                "required_string_fields": EXECUTION_PLAN_TABLE_COLUMNS.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
                "plan_status_enum": ["analyzed", "analyzed_truncated", "unusable_attachment", "not_applicable_plsql", "missing_attachment"],
                "recommendation_type_enum": ["sql_rewrite", "indexing", "statistics", "partitioning", "parallelism", "plan_management", "binds", "instrumentation", "no_change", "recapture"],
                "workload_evidence_policy": "Every recommendation other than no_change or recapture must cite observed get_sql_timeline, compare_project_sql or top_sqls evidence for the same project and SQL_ID."
            },
            "wait_sql_contributors": {
                "category": "wait_events",
                "required_string_fields": WAIT_SQL_CONTRIBUTOR_TABLE_COLUMNS.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
                "coverage_policy": "Up to five highest-ranked positive correlation/ASH SQL relationships per material foreground wait, preserving exact project and event labels."
            },
            "child_cursors": {
                "category": "sql",
                "required_string_fields": CHILD_CURSOR_TABLE_COLUMNS.iter().map(|(key, _)| *key).collect::<Vec<_>>()
            },
            "alert_log_errors": {
                "category": "sql",
                "required_string_fields": ALERT_LOG_TABLE_COLUMNS.iter().map(|(key, _)| *key).collect::<Vec<_>>()
            },
            "segments": {
                "category": "segments",
                "required_string_fields": SEGMENT_TABLE_COLUMNS.iter().map(|(key, _)| *key).collect::<Vec<_>>()
            },
            "segment_synthesis": {
                "category": "segments",
                "required_string_fields": SEGMENT_SYNTHESIS_TABLE_COLUMNS.iter().map(|(key, _)| *key).collect::<Vec<_>>()
            },
            "parameters": {
                "category": "parameters",
                "required_string_fields": PARAMETER_TABLE_COLUMNS.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
                "rating_enum": ["good", "acceptable", "concern", "critical", "unknown", "not_applicable"]
            },
            "all_rows": {"required_field": "evidence_refs", "evidence_refs_min_items": 1}
        },
        "required_assessments": REQUIRED_ASSESSMENTS,
        "human_citation_policy": {
            "evidence_summary_required": true,
            "diagnostic_synthesis_required": true,
            "recommendation_rationale_and_success_criterion_required": true,
            "raw_evidence_ids_reader_facing": false,
            "guidance_requires_verbatim_quote": true,
            "technical_appendices_default": false,
            "comparative_values_explicitly_labeled": true,
            "empty_attachment_links_reader_facing": false,
            "contextual_wait_and_sql_links_required": true,
            "generic_instance_only_link_labels_accepted": false,
            "unresolved_template_placeholders_accepted": false
        },
        "extension_policy": "Core sections cannot be removed. Detail may be changed per category and evidence/guidance appendices are optional.",
        "html_export": {
            "tool": "convert_markdown_to_html",
            "workflow": "Finalize Markdown first, then pass the exact Markdown to the conversion tool.",
            "write_policy": "Creates a new .html file in the JAS-MIN working directory and never overwrites an existing file.",
            "comparative_navigation": "Links every selected project's main dashboard and load-profile reports; never emits a fake comparison report directory.",
            "classic_navigation": "Publishes verified active links to existing classic source reports instead of embedding unverified iframe paths.",
            "presentation": "The shared renderer always applies the responsive ORA-600-aligned audit layout: self-contained branding, sticky navigation, severity markers, keyboard-focusable horizontal table regions with visible scrollbars, project/instance grouping instead of a repeated Project column, interactive execution-plan graphs, and a print layout."
        }
    })
}

fn state_has_project_evidence(
    state: &AnalysisSession,
    project_id: &str,
    tool_name: &str,
    argument_name: Option<&str>,
    argument_value: Option<&str>,
) -> bool {
    state.evidence.values().any(|record| {
        if record.tool_name != tool_name || record.project_id.as_deref() != Some(project_id) {
            return false;
        }
        match (argument_name, argument_value) {
            (Some(name), Some(value)) => record
                .arguments
                .get(name)
                .and_then(Value::as_str)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(value)),
            _ => true,
        }
    })
}

fn state_has_table_row(
    state: &AnalysisSession,
    kind: &str,
    required_cells: &[(&str, &str)],
) -> bool {
    state
        .report_tables
        .values()
        .filter(|table| table.kind == kind)
        .flat_map(|table| &table.rows)
        .any(|row| {
            required_cells.iter().all(|(key, expected)| {
                row.cells
                    .get(*key)
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
            })
        })
}

const MATERIAL_FOREGROUND_WAIT_PCT_DBTIME: f64 = 10.0;
const REQUIRED_GRADIENT_ROWS_PER_FAMILY: usize = 5;

fn gradient_families(
    report: &ReportForAI,
) -> Vec<(&'static str, &'static str, &DbTimeGradientSection)> {
    [
        (
            "db_time_foreground_wait_events",
            "DB Time",
            report.db_time_gradient_fg_wait_events.as_ref(),
        ),
        (
            "db_time_instance_stats_counters",
            "DB Time",
            report.db_time_gradient_instance_stats_counters.as_ref(),
        ),
        (
            "db_time_instance_stats_volumes",
            "DB Time",
            report.db_time_gradient_instance_stats_volumes.as_ref(),
        ),
        (
            "db_time_instance_stats_time",
            "DB Time",
            report.db_time_gradient_instance_stats_time.as_ref(),
        ),
        (
            "db_time_sql_elapsed_time",
            "DB Time",
            report.db_time_gradient_sql_elapsed_time.as_ref(),
        ),
        (
            "db_cpu_instance_stats",
            "DB CPU",
            report.db_cpu_gradient_instance_stats.as_ref(),
        ),
        (
            "db_cpu_sql_cpu_time",
            "DB CPU",
            report.db_cpu_gradient_sql_cpu_time.as_ref(),
        ),
        (
            "custom_wait_events",
            "Custom target",
            report.custom_gradient_wait_events.as_ref(),
        ),
        (
            "custom_instance_stats",
            "Custom target",
            report.custom_gradient_instance_stats.as_ref(),
        ),
    ]
    .into_iter()
    .filter_map(|(family, target, section)| section.map(|section| (family, target, section)))
    .collect()
}

fn sorted_gradient_classifications(
    section: &DbTimeGradientSection,
) -> Vec<&CrossModelClassification> {
    let mut rows = section
        .cross_model_classifications
        .iter()
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| {
                b.combined_impact
                    .partial_cmp(&a.combined_impact)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                b.combined_peak_impact
                    .partial_cmp(&a.combined_peak_impact)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.event_name.cmp(&b.event_name))
    });
    rows
}

fn material_foreground_waits(project: &ProjectData) -> BTreeMap<String, f64> {
    let mut waits = BTreeMap::<String, f64>::new();
    for awr in &project.collection.awrs {
        for wait in &awr.foreground_wait_events {
            if wait.pct_dbtime >= MATERIAL_FOREGROUND_WAIT_PCT_DBTIME {
                waits
                    .entry(wait.event.clone())
                    .and_modify(|maximum| *maximum = maximum.max(wait.pct_dbtime))
                    .or_insert(wait.pct_dbtime);
            }
        }
    }
    waits
}

fn wait_sql_contributors(project: &ProjectData, event_name: &str, limit: usize) -> Value {
    let mut contributors = Vec::<(f64, f64, String, Value)>::new();
    for sql in &project.report.top_sqls_by_elapsed_time {
        let correlation = sql
            .wait_events_with_strong_pearson_correlation
            .iter()
            .find(|row| row.event_name.eq_ignore_ascii_case(event_name))
            .map(|row| row.correlation_value)
            .filter(|value| *value > 0.0);
        let ash = sql
            .wait_events_found_in_ash_sections_for_this_sql
            .iter()
            .find(|row| row.event_name.eq_ignore_ascii_case(event_name));
        if correlation.is_none() && ash.is_none() {
            continue;
        }

        let plan_result = dispatch_tool_call_value(
            "get_sql_execution_plan",
            &json!({"sql_id": sql.sql_id}),
            &project.collection,
            project.stem.as_str(),
        );
        let plan_coverage = if plan_result.get("plan_text").is_some_and(Value::is_null) {
            "missing_attachment"
        } else {
            match plan_result
                .get("plan_applicability")
                .and_then(Value::as_str)
            {
                Some("not_applicable") => "not_applicable_plsql",
                Some("available") => "available_sql_plan",
                _ => "unavailable_sql_plan",
            }
        };
        let evidence_basis = match (correlation.is_some(), ash.is_some()) {
            (true, true) => "aligned_correlation_and_ash",
            (true, false) => "aligned_correlation_only",
            (false, true) => "ash_attribution_only",
            (false, false) => unreachable!(),
        };
        let ash_avg = ash.map(|row| row.avg_pct_of_dbtime_in_sql);
        let ash_samples = ash.map(|row| row.count);
        let correlation_abs = correlation.unwrap_or(0.0).abs();
        let ash_rank = ash_avg.unwrap_or(0.0);
        contributors.push((
            correlation_abs,
            ash_rank,
            sql.sql_id.clone(),
            json!({
                "sql_id": sql.sql_id,
                "module": sql.module,
                "sql_type": sql.sql_type,
                "evidence_basis": evidence_basis,
                "pearson_correlation": correlation,
                "ash_avg_pct_dbtime_in_sql": ash_avg,
                "ash_stddev_pct_dbtime_in_sql": ash.map(|row| row.stddev_pct_of_dbtime_in_sql),
                "ash_samples": ash_samples,
                "workload": {
                    "marked_as_top_in_pct_of_probes": sql.marked_as_top_in_pct_of_probes,
                    "avg_elapsed_time_per_execution_s": sql.avg_elapsed_time_by_exec,
                    "avg_elapsed_time_cumulative_s": sql.avg_elapsed_time_cumulative_s,
                    "avg_cpu_time_per_execution_s": sql.avg_cpu_time_by_exec,
                    "avg_cpu_time_cumulative_s": sql.avg_cpu_time_cumulative_s,
                    "avg_executions": sql.avg_number_of_executions
                },
                "plan_coverage": plan_coverage,
                "plan_applicability": plan_result.get("plan_applicability").cloned().unwrap_or(Value::Null),
                "available_plan_hashes": plan_result.pointer("/full_file_summary/unique_plan_hashes").cloned().unwrap_or_else(|| json!([])),
                "sql_text_available": project.collection.sql_text.contains_key(&sql.sql_id)
            }),
        ));
    }
    contributors.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                right
                    .1
                    .partial_cmp(&left.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.2.cmp(&right.2))
    });
    contributors.truncate(limit.clamp(1, 20));
    let contributors = contributors
        .into_iter()
        .enumerate()
        .map(|(index, (_, _, _, mut contributor))| {
            let correlation = contributor
                .get("pearson_correlation")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let ash_avg = contributor
                .get("ash_avg_pct_dbtime_in_sql")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            if let Some(object) = contributor.as_object_mut() {
                object.insert("rank".to_string(), json!(index + 1));
                object.insert(
                    "plan_review_required".to_string(),
                    json!(index == 0 && (correlation >= 0.70 || ash_avg >= 5.0)),
                );
            }
            contributor
        })
        .collect::<Vec<_>>();

    json!({
        "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
        "event_name": event_name,
        "relationship_scope": "Positive aligned Pearson correlation of SQL elapsed time with wait-event total time and/or direct ASH event attribution within the precomputed top-SQL set.",
        "causality_warning": "Correlation is not blocker/waiter proof. Validate mutex holder/waiter chains, child cursor and parse evidence at aligned snapshots before assigning root cause.",
        "coverage_limit": "The candidate set is limited to SQL statements retained by JAS-MIN's top-SQL analysis; absence from this list does not prove that no other SQL contributed.",
        "returned": contributors.len(),
        "limit": limit.clamp(1, 20),
        "contributors": contributors
    })
}

#[derive(Debug, Clone)]
struct RequiredGradientRow<'a> {
    family: &'static str,
    contributor: String,
    selection_status: &'static str,
    classification: Option<&'a CrossModelClassification>,
}

fn required_gradient_rows(project: &ProjectData) -> Vec<RequiredGradientRow<'_>> {
    let mut required = BTreeMap::<(String, String), RequiredGradientRow<'_>>::new();
    for (family, _, section) in gradient_families(&project.report) {
        for classification in sorted_gradient_classifications(section)
            .into_iter()
            .take(REQUIRED_GRADIENT_ROWS_PER_FAMILY)
        {
            required.insert(
                (
                    family.to_string(),
                    classification.event_name.to_ascii_lowercase(),
                ),
                RequiredGradientRow {
                    family,
                    contributor: classification.event_name.clone(),
                    selection_status: "triangulated",
                    classification: Some(classification),
                },
            );
        }
    }

    if let Some(section) = project.report.db_time_gradient_fg_wait_events.as_ref() {
        for (wait, _) in material_foreground_waits(project) {
            let classification = section
                .cross_model_classifications
                .iter()
                .find(|row| row.event_name.eq_ignore_ascii_case(&wait));
            required.insert(
                (
                    "db_time_foreground_wait_events".to_string(),
                    wait.to_ascii_lowercase(),
                ),
                RequiredGradientRow {
                    family: "db_time_foreground_wait_events",
                    contributor: classification
                        .map(|row| row.event_name.clone())
                        .unwrap_or(wait),
                    selection_status: if classification.is_some() {
                        "triangulated"
                    } else {
                        "material_not_selected"
                    },
                    classification,
                },
            );
        }
    }
    required.into_values().collect()
}

fn report_status_value(
    analysis_id: &str,
    state: &AnalysisSession,
    projects: &BTreeMap<String, Arc<ProjectData>>,
) -> Value {
    let present_categories = state
        .findings
        .values()
        .map(|finding| finding.category.as_str())
        .collect::<HashSet<_>>();
    let missing_categories = REQUIRED_REPORT_CATEGORIES
        .iter()
        .filter(|category| !present_categories.contains(**category))
        .copied()
        .collect::<Vec<_>>();
    let missing_assessments = REQUIRED_ASSESSMENTS
        .iter()
        .filter(|assessment| !state.assessments.contains_key(**assessment))
        .copied()
        .collect::<Vec<_>>();
    let actions = state
        .findings
        .values()
        .map(|finding| finding.recommendations.len())
        .sum::<usize>();
    let mut required_table_kinds = REQUIRED_STRUCTURED_TABLE_KINDS
        .iter()
        .map(|value| value.to_string())
        .collect::<BTreeSet<_>>();
    let present_table_kinds = state
        .report_tables
        .values()
        .map(|table| table.kind.clone())
        .collect::<BTreeSet<_>>();
    let mut missing_evidence = BTreeSet::new();
    let mut missing_table_rows = BTreeSet::new();

    for project_id in &state.project_ids {
        let Some(project) = projects.get(project_id) else {
            missing_evidence.insert(format!("project:{project_id}:unavailable"));
            continue;
        };

        if !state_has_project_evidence(state, project_id, "get_database_load_summary", None, None) {
            missing_evidence.insert(format!("get_database_load_summary:{project_id}"));
        }
        for &section in REQUIRED_PRECOMPUTED_SECTIONS {
            if !state_has_project_evidence(
                state,
                project_id,
                "get_precomputed_analysis",
                Some("section"),
                Some(section),
            ) {
                missing_evidence.insert(format!("get_precomputed_analysis:{project_id}:{section}"));
            }
        }

        let gradient_available =
            gradient_families(&project.report)
                .iter()
                .any(|(_, _, section)| {
                    !section.cross_model_classifications.is_empty()
                        || !section.ridge_top.is_empty()
                        || !section.elastic_net_top.is_empty()
                        || !section.huber_top.is_empty()
                        || !section.quantile95_top.is_empty()
                });
        let analytic_families = [
            ("gradients", "full_gradients", gradient_available),
            (
                "anomalies",
                "load_profile_anomalies",
                !project.report.load_profile_anomalies.is_empty(),
            ),
            (
                "anomaly_clusters",
                "anomaly_clusters",
                !project.report.anomaly_clusters.is_empty(),
            ),
        ];
        let available_analytic_families = analytic_families
            .iter()
            .filter(|(_, _, available)| *available)
            .count();
        for (kind, section, available) in analytic_families {
            if !available {
                continue;
            }
            required_table_kinds.insert(kind.to_string());
            if kind == "gradients" {
                for required in required_gradient_rows(project) {
                    if !state_has_table_row(
                        state,
                        kind,
                        &[
                            ("project_id", project_id),
                            ("analysis_family", required.family),
                            ("contributor", required.contributor.as_str()),
                            ("selection_status", required.selection_status),
                        ],
                    ) {
                        missing_table_rows.insert(format!(
                            "gradients:{project_id}:{}:{}:{}",
                            required.family, required.selection_status, required.contributor
                        ));
                    }
                }
            } else if !state_has_table_row(state, kind, &[("project_id", project_id)]) {
                missing_table_rows.insert(format!("{kind}:{project_id}:at_least_one"));
            }
            let finding_cites_family = state
                .findings
                .values()
                .filter(|finding| finding.category == "gradients_anomalies")
                .any(|finding| {
                    evidence_record_matches(
                        state,
                        &finding.evidence_refs,
                        "get_precomputed_analysis",
                        project_id,
                        Some("section"),
                        Some(section),
                    )
                });
            if !finding_cites_family {
                missing_evidence.insert(format!("gradient_anomaly_finding:{project_id}:{section}"));
            }
        }
        if available_analytic_families >= 2 {
            required_table_kinds.insert("analytic_signal_synthesis".to_string());
            if !state_has_table_row(
                state,
                "analytic_signal_synthesis",
                &[("project_id", project_id)],
            ) {
                missing_table_rows.insert(format!(
                    "analytic_signal_synthesis:{project_id}:at_least_one"
                ));
            }
        }

        let segment_data = dispatch_precomputed_analysis(
            &json!({"section": "segment_hotspots", "limit": 100}),
            &project.report,
        );
        let mut segment_rows = 0usize;
        if let Some(categories) = segment_data.get("data").and_then(Value::as_object) {
            for (statistic, entries) in categories {
                for entry in entries.as_array().into_iter().flatten() {
                    segment_rows += 1;
                    let object_id = entry
                        .get("object_id")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        .to_string();
                    let data_object_id = entry
                        .get("data_object_id")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        .to_string();
                    if !state_has_table_row(
                        state,
                        "segments",
                        &[
                            ("project_id", project_id),
                            ("statistic", statistic),
                            ("object_id", &object_id),
                            ("data_object_id", &data_object_id),
                        ],
                    ) {
                        missing_table_rows.insert(format!(
                            "segments:{project_id}:{statistic}:{object_id}:{data_object_id}"
                        ));
                    }
                }
            }
        }
        if segment_rows > 0 {
            required_table_kinds.insert("segments".to_string());
            required_table_kinds.insert("segment_synthesis".to_string());
            if !state_has_table_row(state, "segment_synthesis", &[("project_id", project_id)]) {
                missing_table_rows.insert(format!("segment_synthesis:{project_id}:at_least_one"));
            }
            let finding_cites_segments = state
                .findings
                .values()
                .filter(|finding| finding.category == "segments")
                .any(|finding| {
                    evidence_record_matches(
                        state,
                        &finding.evidence_refs,
                        "get_precomputed_analysis",
                        project_id,
                        Some("section"),
                        Some("segment_hotspots"),
                    )
                });
            if !finding_cites_segments {
                missing_evidence.insert(format!("segment_synthesis_finding:{project_id}"));
            }
        }

        for parameter in PERFORMANCE_PARAMETER_CHECKLIST {
            let observed_value = state.evidence.values().find_map(|record| {
                if record.tool_name != "get_init_parameter"
                    || record.project_id.as_deref() != Some(project_id)
                {
                    return None;
                }
                record
                    .result
                    .get("parameters")
                    .and_then(|parameters| parameters.get(*parameter))
                    .and_then(Value::as_str)
            });
            let Some(observed_value) = observed_value else {
                missing_evidence.insert(format!("get_init_parameter:{project_id}:{parameter}"));
                continue;
            };
            if observed_value != "<not present in collected data>" {
                required_table_kinds.insert("parameters".to_string());
                if !state_has_table_row(
                    state,
                    "parameters",
                    &[("project_id", project_id), ("parameter", parameter)],
                ) {
                    missing_table_rows.insert(format!("parameters:{project_id}:{parameter}"));
                }
            }
        }

        let plan_inventory = dispatch_tool_call_value(
            "list_available_sql_plans",
            &json!({"limit": 500}),
            &project.collection,
            project.stem.as_str(),
        );
        let plans = plan_inventory
            .get("sql_ids_xplan")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !plans.is_empty() {
            required_table_kinds.insert("execution_plans".to_string());
            if !state_has_project_evidence(
                state,
                project_id,
                "list_available_sql_plans",
                None,
                None,
            ) {
                missing_evidence.insert(format!("list_available_sql_plans:{project_id}"));
            }
        }
        for plan in &plans {
            let Some(sql_id) = plan.get("sql_id").and_then(Value::as_str) else {
                continue;
            };
            let plan_hashes = plan
                .get("unique_plan_hashes")
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                .unwrap_or_default();
            let expected_hashes = if plan_hashes.is_empty() {
                if plan.get("plan_applicability").and_then(Value::as_str) == Some("not_applicable")
                {
                    vec!["not_applicable"]
                } else {
                    vec!["not_available"]
                }
            } else {
                plan_hashes
            };
            for plan_hash in expected_hashes {
                let evidence_present = state.evidence.values().any(|record| {
                    if record.tool_name != "get_sql_execution_plan"
                        || record.project_id.as_deref() != Some(project_id)
                        || !record
                            .arguments
                            .get("sql_id")
                            .and_then(Value::as_str)
                            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(sql_id))
                    {
                        return false;
                    }
                    let requested = record.arguments.get("plan_hash").and_then(Value::as_str);
                    if matches!(plan_hash, "not_available" | "not_applicable") {
                        requested.is_none()
                    } else {
                        requested.is_some_and(|value| value == plan_hash)
                    }
                });
                if !evidence_present {
                    missing_evidence.insert(format!(
                        "get_sql_execution_plan:{project_id}:{sql_id}:{plan_hash}"
                    ));
                }
                if !state_has_table_row(
                    state,
                    "execution_plans",
                    &[
                        ("project_id", project_id),
                        ("sql_id", sql_id),
                        ("plan_hash", plan_hash),
                    ],
                ) {
                    missing_table_rows
                        .insert(format!("execution_plans:{project_id}:{sql_id}:{plan_hash}"));
                }
            }
        }

        for (event_name, peak_pct_dbtime) in material_foreground_waits(project) {
            if !state_has_project_evidence(
                state,
                project_id,
                "get_wait_event_sql_contributors",
                Some("event_name"),
                Some(&event_name),
            ) {
                missing_evidence.insert(format!(
                    "get_wait_event_sql_contributors:{project_id}:{event_name}"
                ));
            }

            let expected = wait_sql_contributors(project, &event_name, 5);
            let contributors = expected
                .get("contributors")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if contributors.is_empty() {
                continue;
            }
            required_table_kinds.insert("wait_sql_contributors".to_string());
            for contributor in contributors {
                let Some(sql_id) = contributor.get("sql_id").and_then(Value::as_str) else {
                    continue;
                };
                if !state_has_table_row(
                    state,
                    "wait_sql_contributors",
                    &[
                        ("project_id", project_id),
                        ("wait_event", &event_name),
                        ("sql_id", sql_id),
                    ],
                ) {
                    missing_table_rows.insert(format!(
                        "wait_sql_contributors:{project_id}:{event_name}:{sql_id}"
                    ));
                }

                if !contributor
                    .get("plan_review_required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    continue;
                }
                for tool_name in ["get_sql_text", "get_sql_timeline", "get_sql_execution_plan"] {
                    if !state_has_project_evidence(
                        state,
                        project_id,
                        tool_name,
                        Some("sql_id"),
                        Some(sql_id),
                    ) {
                        missing_evidence.insert(format!(
                            "{tool_name}:{project_id}:{sql_id}:material_wait:{event_name}:{peak_pct_dbtime:.2}_pct_dbtime"
                        ));
                    }
                }

                let required_plan_hash =
                    match contributor.get("plan_coverage").and_then(Value::as_str) {
                        Some("missing_attachment") => Some("not_supplied"),
                        Some("not_applicable_plsql") => Some("not_applicable"),
                        Some("unavailable_sql_plan") => Some("not_available"),
                        _ => None,
                    };
                if let Some(plan_hash) = required_plan_hash {
                    required_table_kinds.insert("execution_plans".to_string());
                    if !state_has_table_row(
                        state,
                        "execution_plans",
                        &[
                            ("project_id", project_id),
                            ("sql_id", sql_id),
                            ("plan_hash", plan_hash),
                        ],
                    ) {
                        missing_table_rows.insert(format!(
                            "execution_plans:{project_id}:{sql_id}:{plan_hash}:material_wait:{event_name}"
                        ));
                    }
                }
            }
        }

        let cursor_inventory = dispatch_tool_call_value(
            "list_available_child_cursor_reasons",
            &json!({"limit": 500}),
            &project.collection,
            project.stem.as_str(),
        );
        let cursor_reports = cursor_inventory
            .get("sql_ids_with_child_cursor_reasons")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !cursor_reports.is_empty() {
            required_table_kinds.insert("child_cursors".to_string());
            if !state_has_project_evidence(
                state,
                project_id,
                "list_available_child_cursor_reasons",
                None,
                None,
            ) {
                missing_evidence
                    .insert(format!("list_available_child_cursor_reasons:{project_id}"));
            }
        }
        for cursor_report in cursor_reports {
            let Some(sql_id) = cursor_report.get("sql_id").and_then(Value::as_str) else {
                continue;
            };
            if !state_has_project_evidence(
                state,
                project_id,
                "get_child_cursor_reasons",
                Some("sql_id"),
                Some(sql_id),
            ) {
                missing_evidence.insert(format!("get_child_cursor_reasons:{project_id}:{sql_id}"));
            }
            if !state_has_table_row(
                state,
                "child_cursors",
                &[("project_id", project_id), ("sql_id", sql_id)],
            ) {
                missing_table_rows.insert(format!("child_cursors:{project_id}:{sql_id}"));
            }
        }

        let alert_attachments = project.attachment_inventory(None, None);
        if alert_attachments
            .get("alert_logs_nonempty")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        {
            let expected_alerts = dispatch_tool_call_value(
                "get_alertlog_errors",
                &json!({"include_parse_error_details": true, "limit": 1000}),
                &project.collection,
                project.stem.as_str(),
            );
            let alert_evidence = state.evidence.values().find(|record| {
                record.tool_name == "get_alertlog_errors"
                    && record.project_id.as_deref() == Some(project_id)
                    && record
                        .arguments
                        .get("include_parse_error_details")
                        .and_then(Value::as_bool)
                        == Some(true)
            });
            if alert_evidence.is_none() {
                missing_evidence.insert(format!(
                    "get_alertlog_errors:{project_id}:include_parse_error_details"
                ));
            }
            let expected_summaries = expected_alerts
                .get("error_summary")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !expected_summaries.is_empty() {
                required_table_kinds.insert("alert_log_errors".to_string());
            }
            for summary in &expected_summaries {
                let Some(code) = summary.get("code").and_then(Value::as_str) else {
                    continue;
                };
                if !state_has_table_row(
                    state,
                    "alert_log_errors",
                    &[("project_id", project_id), ("error_code", code)],
                ) {
                    missing_table_rows.insert(format!("alert_log_errors:{project_id}:{code}"));
                }
            }
            let parse_errors_present = expected_summaries.iter().any(|summary| {
                summary
                    .get("code")
                    .and_then(Value::as_str)
                    .is_some_and(|code| {
                        code == "WARNING_TOO_MANY_PARSE_ERRORS" || code.starts_with("PARSE_ERROR_")
                    })
            });
            if parse_errors_present {
                let finding_cites_alert = state
                    .findings
                    .values()
                    .filter(|finding| finding.category == "sql")
                    .any(|finding| {
                        finding.evidence_refs.iter().any(|reference| {
                            state.evidence.get(reference).is_some_and(|record| {
                                record.tool_name == "get_alertlog_errors"
                                    && record.project_id.as_deref() == Some(project_id)
                                    && record
                                        .arguments
                                        .get("include_parse_error_details")
                                        .and_then(Value::as_bool)
                                        == Some(true)
                            })
                        })
                    });
                if !finding_cites_alert {
                    missing_evidence.insert(format!(
                        "sql_finding_with_parse_error_evidence:{project_id}"
                    ));
                }
            }
        }
    }

    let missing_table_kinds = required_table_kinds
        .difference(&present_table_kinds)
        .cloned()
        .collect::<Vec<_>>();
    let ready = missing_categories.is_empty()
        && missing_assessments.is_empty()
        && missing_evidence.is_empty()
        && missing_table_kinds.is_empty()
        && missing_table_rows.is_empty()
        && !state.findings.is_empty()
        && actions > 0;
    json!({
        "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
        "analysis_id": analysis_id,
        "project_ids": state.project_ids,
        "comparison_mode": state.project_ids.len() > 1,
        "ready_to_finalize": ready,
        "findings": state.findings.len(),
        "evidence_records": state.evidence.len(),
        "structured_tables": state.report_tables.len(),
        "guidance_refs": state.guidance.len(),
        "recommendation_actions": actions,
        "present_categories": present_categories,
        "missing_required_categories": missing_categories,
        "required_structured_table_kinds": required_table_kinds,
        "present_structured_table_kinds": present_table_kinds,
        "missing_structured_table_kinds": missing_table_kinds,
        "missing_required_evidence": missing_evidence,
        "missing_structured_table_rows": missing_table_rows,
        "completed_assessments": state.assessments.keys().collect::<Vec<_>>(),
        "missing_assessments": missing_assessments,
        "next_step": if ready { "Call finalize_report." } else { "Collect every listed evidence item, record every structured table row, store all required category findings, and complete mandatory assessments." }
    })
}

fn report_section_index(state: &AnalysisSession) -> Value {
    let definitions = [
        (1, "executive_summary", None),
        (2, "performance_profile", Some("performance_profile")),
        (3, "wait_events", Some("wait_events")),
        (4, "sql", Some("sql")),
        (5, "segments", Some("segments")),
        (6, "latches", Some("latches")),
        (7, "io", Some("io")),
        (8, "undo_redo", Some("undo_redo")),
        (9, "gradients_anomalies", Some("gradients_anomalies")),
        (10, "parameters", Some("parameters")),
        (11, "recommendations_and_assessments", None),
    ];
    Value::Array(
        definitions
            .iter()
            .map(|(number, section_id, category)| {
                let finding_ids = state
                    .findings
                    .values()
                    .filter(|finding| {
                        category
                            .map(|category| finding.category == category)
                            .unwrap_or(*number == 1)
                    })
                    .map(|finding| finding.finding_id.as_str())
                    .collect::<Vec<_>>();
                json!({
                    "number": number,
                    "section_id": section_id,
                    "finding_ids": finding_ids,
                    "table_ids": category.map(|category| {
                        state.report_tables.values()
                            .filter(|table| table.category == category)
                            .map(|table| table.table_id.as_str())
                            .collect::<Vec<_>>()
                    }).unwrap_or_default(),
                    "assessment_ids": if *number == 11 {
                        state.assessments.keys().map(String::as_str).collect::<Vec<_>>()
                    } else {
                        Vec::<&str>::new()
                    }
                })
            })
            .collect(),
    )
}

fn render_markdown(
    document: &Value,
    state: &AnalysisSession,
    entity_links: &ReportEntityLinks,
) -> String {
    let mut output = String::new();
    output.push_str("# Oracle Performance Analysis\n\n");
    output.push_str(&format!(
        "Analysis `{}` · revision {} · language `{}` · audience `{}`\n\n",
        document["analysis_id"].as_str().unwrap_or("unknown"),
        document["revision"].as_u64().unwrap_or(0),
        state.config.language,
        state.config.audience
    ));
    let project_labels = report_project_labels(document);
    output.push_str(&format!(
        "Database instances in scope: {}\n\n",
        state
            .project_ids
            .iter()
            .map(|project_id| project_labels
                .get(project_id)
                .cloned()
                .unwrap_or_else(|| project_id.clone()))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if state.project_ids.len() > 1 {
        output.push_str(
            "Baseline and candidate direction must be stated in each comparative finding and its cited evidence.\n\n",
        );
    }
    output.push_str(&render_source_report_links(document));
    output.push_str("<div class=\"severity-legend\" role=\"note\" aria-label=\"Finding severity legend\"><strong>Finding severity</strong><span class=\"legend-chip legend-critical\">CRITICAL</span><span class=\"legend-chip legend-high\">HIGH</span><span class=\"legend-chip legend-medium\">MEDIUM</span><span class=\"legend-chip legend-info\">INFORMATIONAL</span><span>Confidence remains stated in every finding title.</span></div>\n\n");

    output.push_str("## 1. Executive Summary\n\n");
    let mut leading = state.findings.values().collect::<Vec<_>>();
    leading.sort_by_key(|finding| severity_rank(&finding.severity));
    if leading.is_empty() {
        output.push_str("No evidence-backed findings have been recorded.\n\n");
    } else {
        let leading = leading.into_iter().take(5).collect::<Vec<_>>();
        output.push_str("**At-a-glance finding register:**\n\n");
        output.push_str("| Priority finding | Severity | Confidence |\n");
        output.push_str("|---|---|---|\n");
        for finding in &leading {
            output.push_str(&format!(
                "| {} | {} | {} |\n",
                finding.title.replace('|', "\\|").replace('\n', " "),
                finding.severity,
                finding.confidence
            ));
        }
        output.push('\n');
        for (index, finding) in leading.into_iter().enumerate() {
            output.push_str(&format!(
                "**{}. {} [{} / {}]** — {}\n\n**Mechanism:** {}\n\n**Affected workload:** {}\n\n**Temporal pattern:** {}\n\n**Evidence boundary:** {}\n\n",
                index + 1,
                finding.title,
                finding.severity,
                finding.confidence,
                finding.conclusion,
                finding.mechanism,
                finding.affected_workload,
                finding.temporal_pattern,
                finding.evidence_limitations
            ));
        }
    }

    let sections = [
        (
            2,
            "performance_profile",
            "Overall Performance Profile and DB Time Degradation",
        ),
        (3, "wait_events", "Wait Events"),
        (4, "sql", "SQL-Level Analysis"),
        (5, "segments", "Segments and Objects"),
        (6, "latches", "Latches and Internal Contention"),
        (7, "io", "I/O and Disk Assessment"),
        (8, "undo_redo", "UNDO, Redo and Load Profile"),
        (9, "gradients_anomalies", "Gradient and Anomaly Synthesis"),
        (10, "parameters", "Relevant Initialization Parameters"),
    ];
    for (number, category, title) in sections {
        output.push_str(&format!("## {number}. {title}\n\n"));
        let mut findings = state
            .findings
            .values()
            .filter(|finding| finding.category == category)
            .collect::<Vec<_>>();
        findings.sort_by_key(|finding| severity_rank(&finding.severity));
        if findings.is_empty() {
            output.push_str("No evidence-backed findings were recorded for this section.\n\n");
        }
        let detail = state
            .config
            .detail_overrides
            .get(category)
            .map(String::as_str)
            .unwrap_or(&state.config.detail_level);
        for finding in findings {
            output.push_str(&format!(
                "### {} [{} / {}]\n\n{}\n\n**Diagnostic mechanism:** {}\n\n**Affected workload:** {}\n\n**Temporal pattern:** {}\n\n**Evidence basis:** {}\n\n**Evidence boundary and counterevidence:** {}\n\n",
                finding.title,
                finding.severity,
                finding.confidence,
                finding.conclusion,
                finding.mechanism,
                finding.affected_workload,
                finding.temporal_pattern,
                finding.evidence_summary,
                finding.evidence_limitations
            ));
            output.push_str(&render_finding_entity_shortcuts(
                finding,
                state,
                &project_labels,
                entity_links,
            ));
            output.push_str(&render_guidance_quotes(&finding.guidance_quotes, state));
            if state.config.include_evidence_appendix && !finding.evidence_refs.is_empty() {
                output.push_str(&format!(
                    "**Technical provenance:** {}\n\n",
                    evidence_links(&finding.evidence_refs)
                ));
            }
            if detail != "compact" && !finding.details.is_empty() {
                if detail == "deep" {
                    output.push_str(&finding.details);
                    output.push_str("\n\n");
                } else {
                    output.push_str(&format!(
                        "<details class=\"finding-detail\"><summary>Additional diagnostic context</summary><p>{}</p></details>\n\n",
                        encode_text(&finding.details).replace('\n', "<br>")
                    ));
                }
            }
        }

        let tables = state
            .report_tables
            .values()
            .filter(|table| table.category == category)
            .collect::<Vec<_>>();
        let mut by_kind = BTreeMap::<String, Vec<&ReportTable>>::new();
        for table in tables {
            by_kind.entry(table.kind.clone()).or_default().push(table);
        }
        let mut grouped_tables = by_kind.into_iter().collect::<Vec<_>>();
        grouped_tables.sort_by_key(|(kind, _)| report_table_render_rank(kind));
        if !grouped_tables.is_empty() {
            output.push_str("### Structured technical evidence\n\n");
            output.push_str("The tables below preserve deterministic coverage; the diagnostic findings above remain the decision layer.\n\n");
        }
        for (kind, source_tables) in grouped_tables {
            let merged = ReportTable {
                table_id: format!("rendered-{kind}"),
                kind,
                category: category.to_string(),
                title: source_tables
                    .first()
                    .map(|table| table.title.clone())
                    .unwrap_or_default(),
                rows: source_tables
                    .iter()
                    .flat_map(|table| table.rows.iter().cloned())
                    .collect(),
            };
            output.push_str(&render_report_table(
                &merged,
                state,
                &project_labels,
                entity_links,
            ));
        }
    }

    output.push_str("## 11. Prioritized Actions and Mandatory Assessments\n\n");
    let mut actions = state
        .findings
        .values()
        .flat_map(|finding| {
            finding
                .recommendations
                .iter()
                .map(move |action| (finding, action))
        })
        .collect::<Vec<_>>();
    actions
        .sort_by_key(|(finding, action)| (priority_rank(&action.priority), finding.title.as_str()));
    if actions.is_empty() {
        output.push_str("No prioritized actions were recorded.\n\n");
    } else {
        for owner in ["DBA", "Developer", "Management"] {
            let owned = actions
                .iter()
                .filter(|(_, action)| action.owner == owner)
                .collect::<Vec<_>>();
            if owned.is_empty() {
                continue;
            }
            output.push_str(&format!("### {owner} Actions\n\n"));
            for (finding, action) in owned {
                output.push_str(&format!(
                    "- **{} — {}**  \n  **Why:** {}  \n  **Success criterion:** {}  \n  *Supports: {} ({})*\n",
                    action.priority.to_ascii_uppercase(),
                    action.action,
                    action.rationale,
                    action.success_criterion,
                    finding.title,
                    finding.finding_id
                ));
            }
            output.push('\n');
        }
    }
    output.push_str("### Mandatory Assessments\n\n");
    for assessment in REQUIRED_ASSESSMENTS {
        if let Some(value) = state.assessments.get(*assessment) {
            output.push_str(&format!(
                "- **{} — {}**: {} **Evidence basis:** {}",
                assessment.replace('_', " "),
                value.status,
                value.conclusion,
                value.evidence_summary
            ));
            if state.config.include_evidence_appendix && !value.evidence_refs.is_empty() {
                output.push_str(&format!(
                    " Technical provenance: {}.",
                    evidence_links(&value.evidence_refs)
                ));
            }
            output.push('\n');
            output.push_str(&render_guidance_quotes(&value.guidance_quotes, state));
        } else {
            output.push_str(&format!(
                "- **{} — UNKNOWN**: assessment not completed.\n",
                assessment.replace('_', " ")
            ));
        }
    }
    output.push('\n');

    if state.config.include_evidence_appendix {
        output.push_str("## Appendix A. Technical Evidence Provenance\n\n");
        output.push_str("This optional machine-to-human index lists only cited measurements. The findings above remain authoritative because they state the exact values and interpretation in plain language.\n\n");
        let cited = cited_evidence_refs(state);
        output.push_str(&format!(
            "<details class=\"evidence-appendix\"><summary>Show {} cited evidence records</summary><ul>",
            cited.len()
        ));
        for evidence_id in cited {
            let Some(record) = state.evidence.get(&evidence_id) else {
                continue;
            };
            output.push_str(&format!(
                "<li id=\"evidence-{}\"><strong>{} — {}</strong>{}</li>",
                evidence_anchor(&record.evidence_id),
                record.evidence_id,
                humanize_identifier(&record.tool_name),
                evidence_scope(record)
            ));
        }
        output.push_str("</ul></details>\n\n");
    }
    if state.config.include_guidance_appendix {
        output.push_str("## Appendix B. Diagnostic Guidance Consulted\n\n");
        if state.guidance.is_empty() {
            output.push_str("No external diagnostic guidance was consulted.\n\n");
        } else {
            for (reference, guidance) in &state.guidance {
                output.push_str(&format!(
                    "- **{reference} — {}** — methodology only; quoted verbatim where applied, never used as measurement evidence.\n",
                    guidance.title
                ));
            }
            output.push('\n');
        }
    }
    output.push_str("Generated by JAS-MIN · https://github.com/ora600pl/jas-min · expert performance tuning at ora-600.pl\n");
    output
}

fn markdown_table_cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .replace(['\r', '\n'], "<br>")
        .trim()
        .to_string()
}

fn report_project_labels(document: &Value) -> BTreeMap<String, String> {
    document
        .get("datasets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|dataset| {
            let project_id = dataset.get("project_id")?.as_str()?.to_string();
            let stem = dataset
                .get("dataset_stem")
                .and_then(Value::as_str)
                .and_then(|stem| Path::new(stem).file_name())
                .and_then(|stem| stem.to_str())
                .unwrap_or(project_id.as_str());
            let system = stem
                .split_once("_awr_")
                .map(|(system, _)| system)
                .unwrap_or(stem)
                .to_string();
            let instance = dataset
                .pointer("/database/instance_num")
                .and_then(Value::as_u64)
                .filter(|instance| *instance > 0)
                .map(|instance| format!(" · Oracle instance {instance}"))
                .unwrap_or_default();
            Some((project_id, format!("{system}{instance}")))
        })
        .collect()
}

#[derive(Default)]
struct ReportEntityLinks {
    targets: BTreeMap<(String, String, String), String>,
}

impl ReportEntityLinks {
    fn build(project_ids: &[String], projects: &BTreeMap<String, Arc<ProjectData>>) -> Self {
        let mut links = Self::default();
        for project_id in project_ids {
            let Some(project) = projects.get(project_id) else {
                continue;
            };
            let configured = PathBuf::from(project.html_reports_dir.as_str());
            let root = if configured.is_absolute() {
                configured
            } else {
                std::env::current_dir()
                    .map(|directory| directory.join(&configured))
                    .unwrap_or(configured)
            };
            for (source_kind, names) in project.report_links.iter() {
                let (entity_kind, prefix) = match source_kind.as_str() {
                    "FG" => ("wait", Some("fg")),
                    "BG" => ("background_wait", Some("bg")),
                    "SQL" => ("sql", None),
                    _ => continue,
                };
                for name in names {
                    let target = if let Some(prefix) = prefix {
                        root.join(get_safe_filename(name.clone(), prefix.to_string()))
                    } else {
                        root.join("sqlid").join(format!("sqlid_{name}.html"))
                    };
                    if target.is_file() {
                        links.targets.insert(
                            (
                                project_id.clone(),
                                entity_kind.to_string(),
                                name.to_ascii_lowercase(),
                            ),
                            target.to_string_lossy().to_string(),
                        );
                    }
                }
            }
        }
        links
    }

    fn target(&self, project_id: &str, kind: &str, name: &str) -> Option<&str> {
        self.targets
            .get(&(
                project_id.to_string(),
                kind.to_string(),
                name.to_ascii_lowercase(),
            ))
            .map(String::as_str)
    }

    fn markdown_link(&self, project_id: &str, kind: &str, name: &str) -> String {
        match self.target(project_id, kind, name) {
            Some(target) => format!("[{}](<{}>)", name, target),
            None => format!("`{name}`"),
        }
    }

    fn html_code_link(&self, project_id: &str, kind: &str, name: &str) -> String {
        let target = self.target(project_id, kind, name);
        let name = encode_text(name);
        match target {
            Some(target) => format!(
                "<a class=\"entity-link\" href=\"{}\" target=\"_blank\" rel=\"noopener\"><code>{name}</code></a>",
                html_escape::encode_double_quoted_attribute(target)
            ),
            None => format!("<code>{name}</code>"),
        }
    }
}

fn render_finding_entity_shortcuts(
    finding: &ReportFinding,
    state: &AnalysisSession,
    project_labels: &BTreeMap<String, String>,
    links: &ReportEntityLinks,
) -> String {
    let searchable = format!(
        "{} {} {} {} {} {} {} {}",
        finding.title,
        finding.conclusion,
        finding.mechanism,
        finding.temporal_pattern,
        finding.affected_workload,
        finding.evidence_limitations,
        finding.evidence_summary,
        finding.details
    )
    .to_ascii_lowercase();
    let mut shortcuts = Vec::new();
    for project_id in &state.project_ids {
        let label = project_labels
            .get(project_id)
            .map(String::as_str)
            .unwrap_or(project_id);
        for ((candidate_project, kind, entity), target) in &links.targets {
            if candidate_project != project_id || !searchable.contains(entity) {
                continue;
            }
            let shadowed_by_more_specific_entity =
                links
                    .targets
                    .keys()
                    .any(|(other_project, other_kind, other_entity)| {
                        other_project == candidate_project
                            && other_kind == kind
                            && other_entity.len() > entity.len()
                            && other_entity.contains(entity)
                            && searchable.contains(other_entity)
                    });
            if shadowed_by_more_specific_entity {
                continue;
            }
            let kind_label = match kind.as_str() {
                "sql" => "SQL",
                "wait" => "foreground wait",
                "background_wait" => "background wait",
                _ => "detail",
            };
            shortcuts.push(format!(
                "[{} — {}: {}](<{}>)",
                label, kind_label, entity, target
            ));
        }
    }
    shortcuts.sort();
    shortcuts.dedup();
    shortcuts.truncate(16);
    if shortcuts.is_empty() {
        String::new()
    } else {
        format!(
            "**Interactive evidence shortcuts:** {}\n\n",
            shortcuts.join(" · ")
        )
    }
}

fn report_table_render_rank(kind: &str) -> usize {
    match kind {
        "execution_plans" => 10,
        "wait_sql_contributors" => 10,
        "child_cursors" => 20,
        "alert_log_errors" => 30,
        "segment_synthesis" => 10,
        "segments" => 20,
        "analytic_signal_synthesis" => 10,
        "gradients" => 20,
        "anomalies" => 30,
        "anomaly_clusters" => 40,
        "parameters" => 10,
        _ => 100,
    }
}

fn report_table_display_title<'a>(kind: &str, fallback: &'a str) -> &'a str {
    match kind {
        "execution_plans" => "SQL priorities and execution-plan evidence",
        "wait_sql_contributors" => "SQL contributors aligned with material wait events",
        "child_cursors" => "Cursor-sharing causes and measured SQL impact",
        "alert_log_errors" => "Alert-log errors and parse failures",
        "segments" => "Object hotspots by measured statistic",
        "segment_synthesis" => "Objects recurring across independent statistics",
        "gradients" => "Cross-model contributors by target and signal family",
        "anomalies" => "Largest temporal deviations from each project baseline",
        "anomaly_clusters" => "Peak windows with co-occurring signals",
        "analytic_signal_synthesis" => "Cross-signal hypotheses and required runtime proof",
        "parameters" => "Performance parameters requiring attention",
        _ => fallback,
    }
}

fn report_table_provenance(table: &ReportTable, state: &AnalysisSession) -> String {
    if !state.config.include_evidence_appendix {
        return String::new();
    }
    let references = table
        .rows
        .iter()
        .flat_map(|row| row.evidence_refs.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if references.is_empty() {
        String::new()
    } else {
        format!(
            "**Technical provenance for this analysis block:** {}\n\n",
            evidence_links(&references)
        )
    }
}

fn rendered_report_table_cell(
    kind: &str,
    key: &str,
    row: &ReportTableRow,
    entity_links: &ReportEntityLinks,
) -> String {
    let value = row.cells.get(key).map(String::as_str).unwrap_or("");
    let project_id = row
        .cells
        .get("project_id")
        .map(String::as_str)
        .unwrap_or("");
    match (kind, key) {
        ("child_cursors", "sql_id") => entity_links.markdown_link(project_id, "sql", value),
        ("wait_sql_contributors", "wait_event") => {
            entity_links.markdown_link(project_id, "wait", value)
        }
        ("wait_sql_contributors", "sql_id") => entity_links.markdown_link(project_id, "sql", value),
        ("gradients", "contributor") => {
            let family = row
                .cells
                .get("analysis_family")
                .map(String::as_str)
                .unwrap_or("");
            let entity_kind = if family.contains("wait_events") {
                "wait"
            } else if family.contains("sql_") {
                "sql"
            } else {
                return markdown_table_cell(value);
            };
            entity_links.markdown_link(project_id, entity_kind, value)
        }
        ("alert_log_errors", "affected_sql_ids") => value
            .split(',')
            .map(str::trim)
            .filter(|sql_id| !sql_id.is_empty())
            .map(|sql_id| entity_links.markdown_link(project_id, "sql", sql_id))
            .collect::<Vec<_>>()
            .join(", "),
        ("anomaly_clusters", "members") => {
            let members = value
                .split(';')
                .map(str::trim)
                .filter(|member| !member.is_empty())
                .collect::<Vec<_>>();
            if members.len() > 6 {
                format!(
                    "<details class=\"cell-details\"><summary>{} co-occurring signals</summary>{}</details>",
                    members.len(),
                    encode_text(value)
                )
            } else {
                markdown_table_cell(value)
            }
        }
        _ => markdown_table_cell(value),
    }
}

fn render_plain_report_table(
    kind: &str,
    columns: &[(&str, &str)],
    rows: &[&ReportTableRow],
    entity_links: &ReportEntityLinks,
) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut output = String::new();
    output.push('|');
    for (_, label) in columns {
        output.push(' ');
        output.push_str(label);
        output.push_str(" |");
    }
    output.push('\n');
    output.push('|');
    for _ in columns {
        output.push_str("---|");
    }
    output.push('\n');
    for row in rows {
        output.push('|');
        for (key, _) in columns {
            output.push(' ');
            output.push_str(&rendered_report_table_cell(kind, key, row, entity_links));
            output.push_str(" |");
        }
        output.push('\n');
    }
    output.push('\n');
    output
}

fn plan_graph_evidence<'a>(row: &ReportTableRow, state: &'a AnalysisSession) -> Option<&'a Value> {
    row.evidence_refs.iter().find_map(|reference| {
        let record = state.evidence.get(reference)?;
        (record.tool_name == "get_sql_execution_plan")
            .then(|| record.result.get("plan_graph"))
            .flatten()
    })
}

fn render_plan_review(
    row: &ReportTableRow,
    state: &AnalysisSession,
    label: &str,
    entity_links: &ReportEntityLinks,
) -> String {
    let cell = |key: &str| row.cells.get(key).map(String::as_str).unwrap_or("");
    let sql_id = entity_links.html_code_link(cell("project_id"), "sql", cell("sql_id"));
    let plan_hash = encode_text(cell("plan_hash"));
    let plan_status_raw = cell("plan_status");
    let recommendation_type = encode_text(cell("recommendation_type"));
    let label = encode_text(label);
    let risk = encode_text(cell("risk"));
    let rationale = encode_text(cell("recommendation_rationale"));
    let action = encode_text(cell("action"));
    let status = encode_text(plan_status_raw);
    let selection_reason = encode_text(cell("selection_reason"));
    let workload_evidence = encode_text(cell("workload_evidence"));
    let temporal_evidence = encode_text(cell("temporal_evidence"));
    let comparative_context = encode_text(cell("comparative_context"));
    let limitations = encode_text(cell("scope_limitations"));
    let success_metric = encode_text(cell("success_metric"));
    let artifact_heading = if plan_status_raw == "not_applicable_plsql" {
        format!("SQL {sql_id} · PL/SQL entry point")
    } else if plan_status_raw == "missing_attachment" {
        format!("SQL {sql_id} · execution plan not supplied")
    } else {
        format!("SQL {sql_id} · plan hash <code>{plan_hash}</code>")
    };
    let finding_label = if plan_status_raw == "not_applicable_plsql" {
        "PL/SQL-specific finding"
    } else {
        "Plan-specific finding"
    };
    let status_label = if plan_status_raw == "not_applicable_plsql" {
        "Artifact classification"
    } else {
        "Attachment status"
    };
    let mut output = format!(
        "<section class=\"plan-review\" data-plan-review><header class=\"plan-review-header\"><div><span class=\"plan-scope\">{label}</span><h4>{artifact_heading}</h4></div><span class=\"plan-recommendation-badge\">{recommendation_type}</span></header><dl class=\"sql-context\"><div><dt>Why this SQL is here</dt><dd>{selection_reason}</dd></div><div><dt>Measured workload impact</dt><dd>{workload_evidence}</dd></div><div><dt>Coverage and timing</dt><dd>{temporal_evidence}</dd></div><div><dt>Comparison context</dt><dd>{comparative_context}</dd></div></dl><dl class=\"plan-conclusion\"><div><dt>{finding_label}</dt><dd>{risk}</dd></div><div><dt>Why this recommendation follows</dt><dd>{rationale}</dd></div><div><dt>What this evidence cannot prove</dt><dd>{limitations}</dd></div><div><dt>Concrete action</dt><dd>{action}</dd></div><div><dt>Success criterion</dt><dd>{success_metric}</dd></div><div><dt>{status_label}</dt><dd>{status}</dd></div></dl>"
    );

    let graph = plan_graph_evidence(row, state);
    let operations = graph
        .and_then(|graph| graph.get("operations"))
        .and_then(Value::as_array);
    if operations.is_none_or(Vec::is_empty) {
        let message = match plan_status_raw {
            "not_applicable_plsql" => "A top-level row-source graph is not expected for this PL/SQL entry point. Profile the PL/SQL call and inspect the inner SQL statements that consume DB Time.",
            "missing_attachment" => "No execution-plan attachment was supplied for this material SQL. The report keeps the gap explicit and requires a representative child-cursor plan before plan-shape advice.",
            _ => "A row-source graph could not be extracted from this attachment. The recommendation above remains tied to the cited attachment evidence.",
        };
        output.push_str(&format!(
            "<p class=\"plan-graph-unavailable\">{}</p></section>\n\n",
            encode_text(message)
        ));
        return output;
    }
    let operations = operations.expect("operations were checked");
    let initially_filtered = operations.len() > 30;
    let filter_label = if initially_filtered {
        "Show complete plan"
    } else {
        "Show flagged paths only"
    };
    output.push_str(&format!("<div class=\"plan-toolbar\" aria-label=\"Execution plan graph controls\"><button type=\"button\" data-plan-filter aria-pressed=\"{initially_filtered}\">{filter_label}</button><label>Zoom <input type=\"range\" min=\"75\" max=\"150\" value=\"100\" step=\"5\" data-plan-zoom></label><span>Flagged row-source paths are shown first; expand the complete plan when needed.</span></div><div class=\"plan-canvas\" tabindex=\"0\" role=\"region\" aria-label=\"Interactive execution plan row-source graph\"><div class=\"plan-tree\">"));
    for (index, operation) in operations.iter().enumerate() {
        let id = operation.get("id").and_then(Value::as_u64).unwrap_or(0);
        let depth = operation.get("depth").and_then(Value::as_u64).unwrap_or(0);
        let parent_id = operation
            .get("parent_id")
            .and_then(Value::as_u64)
            .map(|value| value.to_string())
            .unwrap_or_default();
        let has_children = operations
            .iter()
            .any(|candidate| candidate.get("parent_id").and_then(Value::as_u64) == Some(id));
        let on_path = operation
            .get("on_flagged_path")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let flags = operation
            .get("flags")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let severity = operation
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or("informational");
        let operation_name = encode_text(
            operation
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("unknown operation"),
        );
        let object_name = encode_text(
            operation
                .get("object_name")
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        let toggle = if has_children {
            format!("<button type=\"button\" class=\"plan-node-toggle\" data-plan-node-toggle aria-expanded=\"true\" aria-label=\"Collapse descendants of operation {id}\">−</button>")
        } else {
            "<span class=\"plan-node-leaf\" aria-hidden=\"true\">•</span>".to_string()
        };
        let filtered = initially_filtered && !on_path;
        output.push_str(&format!(
            "<article class=\"plan-node plan-node-{severity}\" data-plan-node data-node-id=\"{id}\" data-parent-id=\"{parent_id}\" data-depth=\"{depth}\" data-on-flagged-path=\"{on_path}\" data-filtered=\"{filtered}\" style=\"--plan-depth:{depth}\"><div class=\"plan-node-main\">{toggle}<span class=\"plan-node-id\">{id}</span><strong>{operation_name}</strong>"
        ));
        if !object_name.is_empty() {
            output.push_str(&format!("<code>{object_name}</code>"));
        }
        output.push_str("</div><div class=\"plan-node-metrics\">");
        for (key, label) in [
            ("estimated_rows", "E-Rows"),
            ("actual_rows", "A-Rows"),
            ("starts", "Starts"),
            ("cost", "Cost"),
            ("elapsed_time", "Time"),
            ("temp_space", "Temp"),
        ] {
            if let Some(value) = operation
                .get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            {
                output.push_str(&format!(
                    "<span><b>{label}</b> {}</span>",
                    encode_text(value)
                ));
            }
        }
        output.push_str("</div>");
        if !flags.is_empty() {
            output.push_str("<div class=\"plan-node-flags\">");
            for flag in flags.iter().filter_map(Value::as_str) {
                output.push_str(&format!("<span>{}</span>", encode_text(flag)));
            }
            output.push_str("</div>");
        }
        output.push_str("</article>");
        if index + 1 == operations.len() {
            output.push_str("</div></div>");
        }
    }
    output.push_str("</section>\n\n");
    output
}

fn render_execution_plan_reviews(
    table: &ReportTable,
    state: &AnalysisSession,
    project_labels: &BTreeMap<String, String>,
    entity_links: &ReportEntityLinks,
) -> String {
    let mut output = format!(
        "### {}\n\n",
        report_table_display_title(&table.kind, &table.title)
    );
    let mut no_change = Vec::new();
    for row in &table.rows {
        if row.cells.get("recommendation_type").map(String::as_str) == Some("no_change") {
            no_change.push(row);
            continue;
        }
        let project_id = row
            .cells
            .get("project_id")
            .map(String::as_str)
            .unwrap_or("");
        let label = project_labels
            .get(project_id)
            .map(String::as_str)
            .unwrap_or(project_id);
        output.push_str(&render_plan_review(row, state, label, entity_links));
    }
    if !no_change.is_empty() {
        output.push_str(&format!("<details class=\"plan-coverage\"><summary>{} execution artifact variant(s) reviewed with no plan-change recommendation</summary><ul>", no_change.len()));
        for row in no_change {
            let cell = |key: &str| row.cells.get(key).map(String::as_str).unwrap_or("");
            let sql_link = entity_links.html_code_link(cell("project_id"), "sql", cell("sql_id"));
            let artifact = if cell("plan_status") == "not_applicable_plsql" {
                "PL/SQL entry point".to_string()
            } else {
                format!("plan <code>{}</code>", encode_text(cell("plan_hash")))
            };
            output.push_str(&format!(
                "<li>{} / {}: {} Measured context: {} Outcome: {}</li>",
                sql_link,
                artifact,
                encode_text(cell("selection_reason")),
                encode_text(cell("workload_evidence")),
                encode_text(cell("recommendation_rationale"))
            ));
        }
        output.push_str("</ul></details>\n\n");
    }
    output.push_str(&report_table_provenance(table, state));
    output
}

fn render_report_table(
    table: &ReportTable,
    state: &AnalysisSession,
    project_labels: &BTreeMap<String, String>,
    entity_links: &ReportEntityLinks,
) -> String {
    let Some((_, columns)) = report_table_definition(&table.kind) else {
        return String::new();
    };
    if table.kind == "execution_plans" {
        return render_execution_plan_reviews(table, state, project_labels, entity_links);
    }
    let filtered_rows = table
        .rows
        .iter()
        .filter(|row| {
            table.kind != "parameters"
                || matches!(
                    row.cells.get("rating").map(String::as_str),
                    Some("concern" | "critical")
                )
        })
        .collect::<Vec<_>>();
    if filtered_rows.is_empty() {
        return String::new();
    }
    let mut output = format!(
        "### {}\n\n",
        report_table_display_title(&table.kind, &table.title)
    );
    let project_groups = filtered_rows.iter().fold(
        BTreeMap::<&str, Vec<&ReportTableRow>>::new(),
        |mut groups, row| {
            let project_id = row
                .cells
                .get("project_id")
                .map(String::as_str)
                .unwrap_or("");
            groups.entry(project_id).or_default().push(*row);
            groups
        },
    );
    let visible_columns = columns
        .iter()
        .copied()
        .filter(|(key, _)| {
            *key != "project_id"
                && !(table.kind == "segments" && *key == "statistic")
                && !(table.kind == "gradients" && *key == "analysis_family")
                && !(table.kind == "wait_sql_contributors" && *key == "wait_event")
        })
        .collect::<Vec<_>>();
    for (project_id, rows) in project_groups {
        if state.project_ids.len() > 1 {
            let label = project_labels
                .get(project_id)
                .map(String::as_str)
                .unwrap_or(project_id);
            output.push_str(&format!("#### {}\n\n", label));
        }
        if table.kind == "segments" {
            let statistic_groups = rows.iter().fold(
                BTreeMap::<&str, Vec<&ReportTableRow>>::new(),
                |mut groups, row| {
                    let statistic = row
                        .cells
                        .get("statistic")
                        .map(String::as_str)
                        .unwrap_or("unknown");
                    groups.entry(statistic).or_default().push(*row);
                    groups
                },
            );
            for (statistic, statistic_rows) in statistic_groups {
                output.push_str(&format!("##### {}\n\n", humanize_identifier(statistic)));
                output.push_str(&render_plain_report_table(
                    &table.kind,
                    &visible_columns,
                    &statistic_rows,
                    entity_links,
                ));
            }
        } else if table.kind == "gradients" {
            let family_groups = rows.iter().fold(
                BTreeMap::<&str, Vec<&ReportTableRow>>::new(),
                |mut groups, row| {
                    let family = row
                        .cells
                        .get("analysis_family")
                        .map(String::as_str)
                        .unwrap_or("unknown");
                    groups.entry(family).or_default().push(*row);
                    groups
                },
            );
            for (family, family_rows) in family_groups {
                output.push_str(&format!("##### {}\n\n", humanize_identifier(family)));
                output.push_str(&render_plain_report_table(
                    &table.kind,
                    &visible_columns,
                    &family_rows,
                    entity_links,
                ));
            }
        } else if table.kind == "wait_sql_contributors" {
            let wait_groups = rows.iter().fold(
                BTreeMap::<&str, Vec<&ReportTableRow>>::new(),
                |mut groups, row| {
                    let wait_event = row
                        .cells
                        .get("wait_event")
                        .map(String::as_str)
                        .unwrap_or("unknown");
                    groups.entry(wait_event).or_default().push(*row);
                    groups
                },
            );
            for (wait_event, wait_rows) in wait_groups {
                output.push_str(&format!(
                    "##### {}\n\n",
                    entity_links.markdown_link(project_id, "wait", wait_event)
                ));
                output.push_str(&render_plain_report_table(
                    &table.kind,
                    &visible_columns,
                    &wait_rows,
                    entity_links,
                ));
            }
        } else {
            output.push_str(&render_plain_report_table(
                &table.kind,
                &visible_columns,
                &rows,
                entity_links,
            ));
        }
    }
    output.push_str(&report_table_provenance(table, state));
    output
}

fn render_source_report_links(document: &Value) -> String {
    let Some(datasets) = document.get("datasets").and_then(Value::as_array) else {
        return String::new();
    };
    let labels = report_project_labels(document);
    let mut entries = Vec::new();
    for dataset in datasets {
        let project_id = dataset
            .get("project_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown project");
        let Some(reports) = dataset.get("source_reports") else {
            continue;
        };
        let mut links = Vec::new();
        for (field, label, presence_field) in [
            ("main", "Main JAS-MIN dashboard", "main_present"),
            ("load_profile", "Load profile", "load_profile_present"),
            (
                "load_profile_secondary",
                "Secondary load profile",
                "load_profile_secondary_present",
            ),
        ] {
            if reports.get(presence_field).and_then(Value::as_bool) != Some(true) {
                continue;
            }
            if let Some(path) = reports.get(field).and_then(Value::as_str) {
                links.push(format!("[{label}](<{path}>)"));
            }
        }
        if !links.is_empty() {
            let label = labels
                .get(project_id)
                .map(String::as_str)
                .unwrap_or(project_id);
            entries.push(format!("- **{label}:** {}", links.join(" · ")));
        }
    }
    if entries.is_empty() {
        String::new()
    } else {
        format!(
            "**Interactive source reports:**\n\n{}\n\n",
            entries.join("\n")
        )
    }
}

fn validate_references(
    state: &AnalysisSession,
    evidence_refs: &[String],
    guidance_refs: &[String],
) -> std::result::Result<(), Value> {
    let unknown_evidence = evidence_refs
        .iter()
        .filter(|reference| !state.evidence.contains_key(*reference))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown_evidence.is_empty() {
        return Err(tool_error(
            "UNKNOWN_EVIDENCE_REF",
            format!(
                "Unknown evidence references: {}",
                unknown_evidence.join(", ")
            ),
        ));
    }
    let unknown_guidance = guidance_refs
        .iter()
        .filter(|reference| !state.guidance.contains_key(*reference))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown_guidance.is_empty() {
        return Err(tool_error(
            "UNKNOWN_GUIDANCE_REF",
            format!(
                "Unknown guidance references: {}",
                unknown_guidance.join(", ")
            ),
        ));
    }
    Ok(())
}

fn validate_stable_markdown_report(markdown: &str) -> std::result::Result<(), Value> {
    if let Some(placeholder) = unresolved_report_placeholder(markdown) {
        return Err(tool_error(
            "UNRESOLVED_REPORT_PLACEHOLDER",
            format!("Markdown contains unresolved report placeholder '{placeholder}'"),
        ));
    }
    let lines = markdown.lines().map(str::trim).collect::<Vec<_>>();
    if !lines.contains(&"# Oracle Performance Analysis") {
        return Err(tool_error(
            "INVALID_REPORT_TITLE",
            "Markdown must contain the '# Oracle Performance Analysis' title",
        ));
    }

    let mut positions = Vec::with_capacity(STABLE_MARKDOWN_HEADINGS.len());
    let mut missing = Vec::new();
    for heading in STABLE_MARKDOWN_HEADINGS {
        if let Some(position) = lines.iter().position(|line| line == heading) {
            positions.push(position);
        } else {
            missing.push(*heading);
        }
    }
    if !missing.is_empty() {
        return Err(tool_error(
            "MISSING_REPORT_SECTIONS",
            format!(
                "Markdown is missing required headings: {}",
                missing.join("; ")
            ),
        ));
    }
    if positions.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(tool_error(
            "INVALID_REPORT_SECTION_ORDER",
            "The 11 required report sections must appear in the server-defined order",
        ));
    }
    Ok(())
}

fn unresolved_report_placeholder(value: &str) -> Option<&'static str> {
    [
        "{load_profile}",
        "{load_profile2}",
        "{jasmin_main}",
        "{lp}",
        "{lp2}",
        "{jm}",
    ]
    .into_iter()
    .find(|placeholder| value.contains(placeholder))
}

fn validate_resolved_html_navigation(html: &str) -> std::result::Result<(), Value> {
    if let Some(placeholder) = unresolved_report_placeholder(html) {
        return Err(tool_error(
            "UNRESOLVED_REPORT_PLACEHOLDER",
            format!("Rendered HTML contains unresolved report placeholder '{placeholder}'"),
        ));
    }
    Ok(())
}

fn validate_local_html_targets(
    html: &str,
    output_directory: &Path,
) -> std::result::Result<usize, Value> {
    let href = Regex::new(r#"href=\"([^\"]+)\""#).expect("static href regex is valid");
    let mut checked = 0usize;
    let mut missing = BTreeSet::new();
    for capture in href.captures_iter(html) {
        let raw = capture.get(1).map(|value| value.as_str()).unwrap_or("");
        if raw.is_empty()
            || raw.starts_with('#')
            || raw.starts_with("http://")
            || raw.starts_with("https://")
            || raw.starts_with("mailto:")
            || raw.starts_with("data:")
        {
            continue;
        }
        let decoded = html_escape::decode_html_entities(raw);
        let without_fragment = decoded.split('#').next().unwrap_or("");
        let local = without_fragment
            .strip_prefix("file://")
            .unwrap_or(without_fragment);
        if local.is_empty() {
            continue;
        }
        checked += 1;
        let configured = PathBuf::from(local);
        let resolved = if configured.is_absolute() {
            configured
        } else {
            output_directory.join(configured)
        };
        if !resolved.is_file() {
            missing.insert(resolved.to_string_lossy().to_string());
        }
    }
    if !missing.is_empty() {
        return Err(tool_error(
            "BROKEN_LOCAL_REPORT_LINK",
            format!(
                "Rendered HTML contains local links whose targets do not exist: {}",
                missing.into_iter().collect::<Vec<_>>().join("; ")
            ),
        ));
    }
    Ok(checked)
}

fn html_output_filename(
    requested: Option<&str>,
    dataset_stem: &str,
    analysis_id: &str,
) -> std::result::Result<String, Value> {
    let default_name = || {
        let dataset_name = Path::new(dataset_stem)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("jas-min");
        let mut safe_stem = dataset_name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '-'
                }
            })
            .take(120)
            .collect::<String>();
        while safe_stem.contains("--") {
            safe_stem = safe_stem.replace("--", "-");
        }
        let safe_stem = safe_stem.trim_matches('-');
        let safe_stem = if safe_stem.is_empty() {
            "jas-min"
        } else {
            safe_stem
        };
        format!("{safe_stem}-{}-report.html", analysis_id.to_lowercase())
    };

    let candidate = requested
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(default_name);
    if candidate.len() > MAX_MCP_HTML_FILENAME_BYTES {
        return Err(tool_error(
            "INVALID_OUTPUT_FILENAME",
            format!("output_filename exceeds the {MAX_MCP_HTML_FILENAME_BYTES}-byte limit"),
        ));
    }
    if candidate.starts_with('.')
        || candidate.contains('/')
        || candidate.contains('\\')
        || candidate.chars().any(char::is_control)
    {
        return Err(tool_error(
            "INVALID_OUTPUT_FILENAME",
            "output_filename must be a visible basename without path separators or control characters",
        ));
    }

    let path = Path::new(&candidate);
    match path.extension().and_then(|value| value.to_str()) {
        None => Ok(format!("{candidate}.html")),
        Some(extension) if extension.eq_ignore_ascii_case("html") => {
            let stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if stem.is_empty() {
                Err(tool_error(
                    "INVALID_OUTPUT_FILENAME",
                    "output_filename must contain a name before the .html extension",
                ))
            } else {
                Ok(format!("{stem}.html"))
            }
        }
        Some(_) => Err(tool_error(
            "INVALID_OUTPUT_FILENAME",
            "output_filename must have no extension or use the .html extension",
        )),
    }
}

fn parse_recommendations(value: Option<&Value>) -> std::result::Result<Vec<Recommendation>, Value> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    if items.len() > 24 {
        return Err(tool_error(
            "TOO_MANY_RECOMMENDATIONS",
            "A finding may contain at most 24 recommendations",
        ));
    }
    items
        .iter()
        .map(|item| {
            let Some(object) = item.as_object() else {
                return Err(tool_error(
                    "INVALID_RECOMMENDATION",
                    "Each recommendation must be an object",
                ));
            };
            let owner = required_string(object, "owner", 32)?;
            validate_enum("owner", &owner, &["DBA", "Developer", "Management"])?;
            let priority = required_string(object, "priority", 32)?;
            validate_enum(
                "priority",
                &priority,
                &["immediate", "high", "medium", "low"],
            )?;
            let action = required_string(object, "action", 2_000)?;
            let rationale = required_synthesis_string(object, "rationale", 2_000, 24)?;
            let success_criterion =
                required_synthesis_string(object, "success_criterion", 2_000, 24)?;
            validate_distinct_diagnostic_statements(&[
                ("action", &action),
                ("rationale", &rationale),
                ("success_criterion", &success_criterion),
            ])?;
            Ok(Recommendation {
                owner,
                priority,
                action,
                rationale,
                success_criterion,
            })
        })
        .collect()
}

fn evidence_record_matches(
    state: &AnalysisSession,
    evidence_refs: &[String],
    tool_name: &str,
    project_id: &str,
    argument_name: Option<&str>,
    argument_value: Option<&str>,
) -> bool {
    evidence_refs.iter().any(|reference| {
        let Some(record) = state.evidence.get(reference) else {
            return false;
        };
        if record.tool_name != tool_name || record.project_id.as_deref() != Some(project_id) {
            return false;
        }
        match (argument_name, argument_value) {
            (Some(name), Some(value)) => record
                .arguments
                .get(name)
                .and_then(Value::as_str)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(value)),
            _ => true,
        }
    })
}

fn report_table_numeric_cell_matches(cell: &str, expected: f64) -> bool {
    cell.trim()
        .trim_end_matches('%')
        .parse::<f64>()
        .is_ok_and(|actual| {
            let tolerance = (expected.abs() * 0.0001).max(0.005);
            (actual - expected).abs() <= tolerance
        })
}

fn evidence_supports_sql_priority(
    state: &AnalysisSession,
    evidence_refs: &[String],
    project_id: &str,
    sql_id: &str,
) -> bool {
    evidence_refs.iter().any(|reference| {
        let Some(record) = state.evidence.get(reference) else {
            return false;
        };
        match record.tool_name.as_str() {
            "get_sql_timeline" => {
                record.project_id.as_deref() == Some(project_id)
                    && record
                        .arguments
                        .get("sql_id")
                        .and_then(Value::as_str)
                        .is_some_and(|value| value.eq_ignore_ascii_case(sql_id))
                    && record
                        .result
                        .get("points")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        > 0
            }
            "compare_project_sql" => {
                record
                    .arguments
                    .get("sql_id")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case(sql_id))
                    && ["baseline_project_id", "candidate_project_id"]
                        .iter()
                        .any(|key| {
                            record.arguments.get(*key).and_then(Value::as_str) == Some(project_id)
                        })
            }
            "get_precomputed_analysis" => {
                record.project_id.as_deref() == Some(project_id)
                    && record.arguments.get("section").and_then(Value::as_str) == Some("top_sqls")
                    && record.result.as_array().into_iter().flatten().any(|row| {
                        row.get("sql_id")
                            .and_then(Value::as_str)
                            .is_some_and(|value| value.eq_ignore_ascii_case(sql_id))
                    })
            }
            "get_wait_event_sql_contributors" => {
                record.project_id.as_deref() == Some(project_id)
                    && record
                        .result
                        .get("contributors")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .any(|row| {
                            row.get("sql_id")
                                .and_then(Value::as_str)
                                .is_some_and(|value| value.eq_ignore_ascii_case(sql_id))
                        })
            }
            _ => false,
        }
    })
}

fn parse_report_table_rows(
    kind: &str,
    value: &Value,
    state: &AnalysisSession,
    projects: &BTreeMap<String, Arc<ProjectData>>,
) -> std::result::Result<Vec<ReportTableRow>, Value> {
    let Some((_, columns)) = report_table_definition(kind) else {
        return Err(tool_error(
            "INVALID_REPORT_TABLE_KIND",
            format!("Unknown report table kind '{kind}'"),
        ));
    };
    let Some(input_rows) = value.as_array() else {
        return Err(tool_error(
            "INVALID_REPORT_TABLE_ROWS",
            "rows must be an array",
        ));
    };
    if input_rows.is_empty() || input_rows.len() > 500 {
        return Err(tool_error(
            "INVALID_REPORT_TABLE_ROWS",
            "rows must contain between 1 and 500 entries",
        ));
    }
    let allowed = columns
        .iter()
        .map(|(key, _)| *key)
        .chain(std::iter::once("evidence_refs"))
        .collect::<HashSet<_>>();
    let mut parsed = Vec::with_capacity(input_rows.len());
    let mut unique_keys = HashSet::new();

    for (index, value) in input_rows.iter().enumerate() {
        let Some(object) = value.as_object() else {
            return Err(tool_error(
                "INVALID_REPORT_TABLE_ROW",
                format!("rows[{index}] must be an object"),
            ));
        };
        if let Some(unexpected) = object.keys().find(|key| !allowed.contains(key.as_str())) {
            return Err(tool_error(
                "INVALID_REPORT_TABLE_COLUMN",
                format!(
                    "rows[{index}] contains unsupported field '{unexpected}' for kind '{kind}'"
                ),
            ));
        }
        let mut cells = BTreeMap::new();
        for (key, _) in columns {
            let cell = required_string(object, key, 4_000).map_err(|_| {
                tool_error(
                    "INVALID_REPORT_TABLE_CELL",
                    format!("rows[{index}].{key} must be a non-empty string"),
                )
            })?;
            cells.insert((*key).to_string(), cell);
        }
        let evidence_refs = string_array(object, "evidence_refs", 16, 32)?;
        if evidence_refs.is_empty() {
            return Err(tool_error(
                "REPORT_TABLE_ROW_WITHOUT_EVIDENCE",
                format!("rows[{index}] must cite at least one evidence_ref"),
            ));
        }
        validate_references(state, &evidence_refs, &[])?;
        let project_id = cells.get("project_id").map(String::as_str).unwrap_or("");
        if !state
            .project_ids
            .iter()
            .any(|candidate| candidate == project_id)
        {
            return Err(tool_error(
                "PROJECT_OUTSIDE_ANALYSIS",
                format!("rows[{index}].project_id '{project_id}' is not part of this analysis"),
            ));
        }

        let unique_key = match kind {
            "execution_plans" => format!(
                "{project_id}:{}:{}",
                cells.get("sql_id").map(String::as_str).unwrap_or(""),
                cells.get("plan_hash").map(String::as_str).unwrap_or("")
            ),
            "wait_sql_contributors" => format!(
                "{project_id}:{}:{}",
                cells.get("wait_event").map(String::as_str).unwrap_or(""),
                cells.get("sql_id").map(String::as_str).unwrap_or("")
            ),
            "child_cursors" => format!(
                "{project_id}:{}",
                cells.get("sql_id").map(String::as_str).unwrap_or("")
            ),
            "segments" => format!(
                "{project_id}:{}:{}:{}",
                cells.get("statistic").map(String::as_str).unwrap_or(""),
                cells.get("object_id").map(String::as_str).unwrap_or(""),
                cells
                    .get("data_object_id")
                    .map(String::as_str)
                    .unwrap_or("")
            ),
            "segment_synthesis" => format!(
                "{project_id}:{}:{}",
                cells.get("object_id").map(String::as_str).unwrap_or(""),
                cells
                    .get("data_object_id")
                    .map(String::as_str)
                    .unwrap_or("")
            ),
            "parameters" => format!(
                "{project_id}:{}",
                cells.get("parameter").map(String::as_str).unwrap_or("")
            ),
            "alert_log_errors" => format!(
                "{project_id}:{}",
                cells.get("error_code").map(String::as_str).unwrap_or("")
            ),
            "analytic_signal_synthesis" => format!(
                "{project_id}:{}",
                cells.get("entity").map(String::as_str).unwrap_or("")
            ),
            "gradients" => format!(
                "{project_id}:{}:{}:{}",
                cells
                    .get("analysis_family")
                    .map(String::as_str)
                    .unwrap_or(""),
                cells.get("target_metric").map(String::as_str).unwrap_or(""),
                cells.get("contributor").map(String::as_str).unwrap_or("")
            ),
            "anomalies" => format!(
                "{project_id}:{}:{}",
                cells.get("metric").map(String::as_str).unwrap_or(""),
                cells.get("time_scope").map(String::as_str).unwrap_or("")
            ),
            "anomaly_clusters" => format!(
                "{project_id}:{}",
                cells.get("cluster_id").map(String::as_str).unwrap_or("")
            ),
            _ => unreachable!(),
        }
        .to_ascii_lowercase();
        if !unique_keys.insert(unique_key) {
            return Err(tool_error(
                "DUPLICATE_REPORT_TABLE_ROW",
                format!("rows[{index}] duplicates an earlier table row"),
            ));
        }

        match kind {
            "execution_plans" => {
                let sql_id = cells["sql_id"].as_str();
                validate_enum(
                    "plan_status",
                    &cells["plan_status"],
                    &[
                        "analyzed",
                        "analyzed_truncated",
                        "unusable_attachment",
                        "not_applicable_plsql",
                        "missing_attachment",
                    ],
                )?;
                let plan_hash = cells["plan_hash"].as_str();
                let matching = evidence_refs.iter().find_map(|reference| {
                    let record = state.evidence.get(reference)?;
                    if record.tool_name != "get_sql_execution_plan"
                        || record.project_id.as_deref() != Some(project_id)
                        || !record
                            .arguments
                            .get("sql_id")
                            .and_then(Value::as_str)
                            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(sql_id))
                    {
                        return None;
                    }
                    let requested = record.arguments.get("plan_hash").and_then(Value::as_str);
                    let hash_matches = if plan_hash == "not_applicable" {
                        requested.is_none()
                            && record
                                .result
                                .get("plan_applicability")
                                .and_then(Value::as_str)
                                == Some("not_applicable")
                    } else if plan_hash == "not_supplied" {
                        requested.is_none()
                            && record.result.get("plan_text").is_some_and(Value::is_null)
                    } else if plan_hash == "not_available" {
                        requested.is_none()
                            && record
                                .result
                                .pointer("/full_file_summary/unique_plan_hashes")
                                .and_then(Value::as_array)
                                .is_some_and(Vec::is_empty)
                    } else {
                        requested.is_some_and(|value| value == plan_hash)
                    };
                    hash_matches.then_some(record)
                });
                let Some(record) = matching else {
                    return Err(tool_error(
                        "REPORT_TABLE_EVIDENCE_MISMATCH",
                        format!("rows[{index}] must cite get_sql_execution_plan evidence for project '{project_id}', SQL_ID '{sql_id}', plan hash '{plan_hash}'"),
                    ));
                };
                let expected_status = if record.result.get("plan_text").is_some_and(Value::is_null)
                {
                    "missing_attachment"
                } else if record
                    .result
                    .get("plan_applicability")
                    .and_then(Value::as_str)
                    == Some("not_applicable")
                {
                    "not_applicable_plsql"
                } else if plan_hash == "not_available" {
                    "unusable_attachment"
                } else if record
                    .result
                    .get("truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "analyzed_truncated"
                } else {
                    "analyzed"
                };
                if cells["plan_status"] != expected_status {
                    return Err(tool_error(
                        "REPORT_TABLE_PLAN_STATUS_MISMATCH",
                        format!("rows[{index}].plan_status must be '{expected_status}' for the cited plan evidence"),
                        ));
                }
                validate_enum(
                    "recommendation_type",
                    &cells["recommendation_type"],
                    &[
                        "sql_rewrite",
                        "indexing",
                        "statistics",
                        "partitioning",
                        "parallelism",
                        "plan_management",
                        "binds",
                        "instrumentation",
                        "no_change",
                        "recapture",
                    ],
                )?;
                if expected_status == "unusable_attachment"
                    && cells["recommendation_type"] != "recapture"
                {
                    return Err(tool_error(
                        "REPORT_TABLE_PLAN_RECOMMENDATION_MISMATCH",
                        format!("rows[{index}] must use recommendation_type 'recapture' for an unusable attachment"),
                    ));
                }
                if expected_status == "missing_attachment"
                    && cells["recommendation_type"] != "recapture"
                {
                    return Err(tool_error(
                        "REPORT_TABLE_PLAN_RECOMMENDATION_MISMATCH",
                        format!("rows[{index}] must use recommendation_type 'recapture' when a material SQL plan attachment is missing"),
                    ));
                }
                if expected_status == "not_applicable_plsql"
                    && !matches!(
                        cells["recommendation_type"].as_str(),
                        "instrumentation" | "no_change"
                    )
                {
                    return Err(tool_error(
                        "REPORT_TABLE_PLAN_RECOMMENDATION_MISMATCH",
                        format!("rows[{index}] must use instrumentation or no_change for a PL/SQL entry point; a top-level row-source plan is not applicable"),
                    ));
                }
                if !matches!(
                    expected_status,
                    "unusable_attachment" | "missing_attachment"
                ) && cells["recommendation_type"] == "recapture"
                {
                    return Err(tool_error(
                        "REPORT_TABLE_PLAN_RECOMMENDATION_MISMATCH",
                        format!("rows[{index}] cannot use recommendation_type 'recapture' for a parsed plan"),
                    ));
                }
                let generic_action = cells["action"].to_ascii_lowercase();
                let generic_risk = cells["risk"].to_ascii_lowercase();
                if [
                    "selection_reason",
                    "workload_evidence",
                    "temporal_evidence",
                    "comparative_context",
                    "recommendation_rationale",
                    "scope_limitations",
                    "action",
                    "success_metric",
                ]
                .iter()
                .any(|key| cells[*key].chars().count() < 40)
                    || generic_action == "validate with runtime row-source statistics."
                    || generic_action == "validate actual rows."
                    || generic_risk == "optimizer estimates only."
                {
                    return Err(tool_error(
                        "GENERIC_EXECUTION_PLAN_ANALYSIS",
                        format!("rows[{index}] must contain a plan-specific rationale and concrete action; a generic request to validate row-source statistics is not a tuning recommendation"),
                    ));
                }
                if !matches!(
                    cells["recommendation_type"].as_str(),
                    "no_change" | "recapture"
                ) && !evidence_supports_sql_priority(state, &evidence_refs, project_id, sql_id)
                {
                    return Err(tool_error(
                        "EXECUTION_PLAN_WITHOUT_WORKLOAD_EVIDENCE",
                        format!("rows[{index}] recommends a plan change for SQL_ID '{sql_id}' without cited get_sql_timeline, compare_project_sql or top_sqls evidence proving that the SQL is a workload priority"),
                    ));
                }
            }
            "wait_sql_contributors" => {
                let event_name = cells["wait_event"].as_str();
                let sql_id = cells["sql_id"].as_str();
                let matching = evidence_refs.iter().find_map(|reference| {
                    let record = state.evidence.get(reference)?;
                    if record.tool_name != "get_wait_event_sql_contributors"
                        || record.project_id.as_deref() != Some(project_id)
                        || !record
                            .arguments
                            .get("event_name")
                            .and_then(Value::as_str)
                            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(event_name))
                    {
                        return None;
                    }
                    record
                        .result
                        .get("contributors")
                        .and_then(Value::as_array)?
                        .iter()
                        .find(|contributor| {
                            contributor
                                .get("sql_id")
                                .and_then(Value::as_str)
                                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(sql_id))
                        })
                });
                let Some(contributor) = matching else {
                    return Err(tool_error(
                        "REPORT_TABLE_EVIDENCE_MISMATCH",
                        format!("rows[{index}] must cite get_wait_event_sql_contributors evidence for project '{project_id}', event '{event_name}', SQL_ID '{sql_id}'"),
                    ));
                };
                let expected_plan_coverage = contributor
                    .get("plan_coverage")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                if cells["plan_coverage"] != expected_plan_coverage {
                    return Err(tool_error(
                        "REPORT_TABLE_PLAN_COVERAGE_MISMATCH",
                        format!("rows[{index}].plan_coverage must be '{expected_plan_coverage}' for the cited contributor evidence"),
                    ));
                }
            }
            "child_cursors" => {
                let sql_id = cells["sql_id"].as_str();
                if !evidence_record_matches(
                    state,
                    &evidence_refs,
                    "get_child_cursor_reasons",
                    project_id,
                    Some("sql_id"),
                    Some(sql_id),
                ) {
                    return Err(tool_error(
                        "REPORT_TABLE_EVIDENCE_MISMATCH",
                        format!("rows[{index}] must cite get_child_cursor_reasons evidence for project '{project_id}', SQL_ID '{sql_id}'"),
                    ));
                }
            }
            "segments" | "segment_synthesis" => {
                if kind == "segment_synthesis" {
                    let object_id = cells["object_id"].parse::<u64>().ok();
                    let data_object_id = cells["data_object_id"].parse::<u64>().ok();
                    let matching = evidence_refs.iter().any(|reference| {
                        let Some(record) = state.evidence.get(reference) else {
                            return false;
                        };
                        if record.tool_name != "get_precomputed_analysis"
                            || record.project_id.as_deref() != Some(project_id)
                            || record.arguments.get("section").and_then(Value::as_str)
                                != Some("segment_hotspots")
                        {
                            return false;
                        }
                        record
                            .result
                            .get("data")
                            .and_then(Value::as_object)
                            .into_iter()
                            .flat_map(|categories| categories.values())
                            .flat_map(|entries| entries.as_array().into_iter().flatten())
                            .any(|entry| {
                                entry.get("object_id").and_then(Value::as_u64) == object_id
                                    && entry.get("data_object_id").and_then(Value::as_u64)
                                        == data_object_id
                            })
                    });
                    if !matching {
                        return Err(tool_error(
                            "REPORT_TABLE_EVIDENCE_MISMATCH",
                            format!("rows[{index}] must cite segment_hotspots evidence containing the synthesized object/data-object pair"),
                        ));
                    }
                    parsed.push(ReportTableRow {
                        cells,
                        evidence_refs,
                    });
                    continue;
                }
                let statistic = cells["statistic"].as_str();
                let object_id = cells["object_id"].parse::<u64>().ok();
                let data_object_id = cells["data_object_id"].parse::<u64>().ok();
                let segment_record = evidence_refs.iter().find_map(|reference| {
                    let record = state.evidence.get(reference)?;
                    if record.tool_name != "get_precomputed_analysis"
                        || record.project_id.as_deref() != Some(project_id)
                        || record.arguments.get("section").and_then(Value::as_str)
                            != Some("segment_hotspots")
                    {
                        return None;
                    }
                    Some(record)
                });
                let Some(segment_record) = segment_record else {
                    return Err(tool_error(
                        "REPORT_TABLE_EVIDENCE_MISMATCH",
                        format!("rows[{index}] must cite segment_hotspots evidence for project '{project_id}'"),
                    ));
                };
                let data = segment_record
                    .result
                    .get("data")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        tool_error(
                            "REPORT_TABLE_EVIDENCE_MISMATCH",
                            format!("rows[{index}] cites malformed segment_hotspots evidence"),
                        )
                    })?;
                let no_segment_statistics = data
                    .values()
                    .all(|value| value.as_array().is_none_or(Vec::is_empty));
                if no_segment_statistics {
                    let valid_sentinel = statistic == "no_segment_statistics"
                        && cells["segment_name"] == "not available"
                        && cells["segment_type"] == "unknown"
                        && cells["object_id"] == "0"
                        && cells["data_object_id"] == "0"
                        && cells["occurrence_pct"] == "not available"
                        && cells["average"] == "not available"
                        && cells["stddev"] == "not available";
                    if !valid_sentinel {
                        return Err(tool_error(
                            "REPORT_TABLE_SEGMENT_VALUE_MISMATCH",
                            format!("rows[{index}] must use the deterministic no-segment-data sentinel values"),
                        ));
                    }
                    continue;
                }
                let source = data
                    .get(statistic)
                    .and_then(Value::as_array)
                    .and_then(|entries| {
                        entries.iter().find(|entry| {
                            entry.get("object_id").and_then(Value::as_u64) == object_id
                                && entry.get("data_object_id").and_then(Value::as_u64)
                                    == data_object_id
                        })
                    });
                let Some(source) = source else {
                    return Err(tool_error(
                        "REPORT_TABLE_EVIDENCE_MISMATCH",
                        format!("rows[{index}] must cite the exact segment_hotspots row for project '{project_id}', statistic '{statistic}', object/data object '{}/{}'", cells["object_id"], cells["data_object_id"]),
                    ));
                };
                let source_name = source
                    .get("segment_name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let expected_name = if source_name.is_empty() {
                    "<redacted>"
                } else {
                    source_name
                };
                let exact_strings_match = cells["segment_name"] == expected_name
                    && source
                        .get("segment_type")
                        .and_then(Value::as_str)
                        .is_some_and(|value| value == cells["segment_type"]);
                let numerics_match = [
                    ("occurrence_pct", "pct_of_occuriance"),
                    ("average", "avg"),
                    ("stddev", "stddev"),
                ]
                .iter()
                .all(|(cell_key, source_key)| {
                    source
                        .get(*source_key)
                        .and_then(Value::as_f64)
                        .is_some_and(|value| {
                            report_table_numeric_cell_matches(&cells[*cell_key], value)
                        })
                });
                if !exact_strings_match || !numerics_match {
                    return Err(tool_error(
                        "REPORT_TABLE_SEGMENT_VALUE_MISMATCH",
                        format!("rows[{index}] must reproduce the cited segment name, type, occurrence, average and standard deviation"),
                    ));
                }
            }
            "gradients" | "anomalies" | "anomaly_clusters" => {
                let section = match kind {
                    "gradients" => "full_gradients",
                    "anomalies" => "load_profile_anomalies",
                    "anomaly_clusters" => "anomaly_clusters",
                    _ => unreachable!(),
                };
                if !evidence_record_matches(
                    state,
                    &evidence_refs,
                    "get_precomputed_analysis",
                    project_id,
                    Some("section"),
                    Some(section),
                ) {
                    return Err(tool_error(
                        "REPORT_TABLE_EVIDENCE_MISMATCH",
                        format!(
                            "rows[{index}] must cite {section} evidence for project '{project_id}'"
                        ),
                    ));
                }
                if kind == "gradients" {
                    validate_enum(
                        "selection_status",
                        &cells["selection_status"],
                        &["triangulated", "material_not_selected"],
                    )?;
                    let project = projects.get(project_id).ok_or_else(|| {
                        tool_error(
                            "PROJECT_OUTSIDE_ANALYSIS",
                            format!("Project '{project_id}' is unavailable"),
                        )
                    })?;
                    let family = cells["analysis_family"].as_str();
                    let contributor = cells["contributor"].as_str();
                    let Some((_, expected_target, gradient)) = gradient_families(&project.report)
                        .into_iter()
                        .find(|(candidate, _, _)| *candidate == family)
                    else {
                        return Err(tool_error(
                            "REPORT_TABLE_GRADIENT_FAMILY_MISMATCH",
                            format!("rows[{index}].analysis_family '{family}' is not available for project '{project_id}'"),
                        ));
                    };
                    if cells["target_metric"] != expected_target {
                        return Err(tool_error(
                            "REPORT_TABLE_GRADIENT_TARGET_MISMATCH",
                            format!("rows[{index}].target_metric must be '{expected_target}' for family '{family}'"),
                        ));
                    }
                    let classification = gradient
                        .cross_model_classifications
                        .iter()
                        .find(|row| row.event_name.eq_ignore_ascii_case(contributor));
                    match cells["selection_status"].as_str() {
                        "triangulated" => {
                            let Some(classification) = classification else {
                                return Err(tool_error(
                                    "REPORT_TABLE_GRADIENT_VALUE_MISMATCH",
                                    format!("rows[{index}] claims triangulation for '{contributor}', but the cited family contains no cross-model classification"),
                                ));
                            };
                            if cells["classification"] != classification.classification
                                || !report_table_numeric_cell_matches(
                                    &cells["typical_impact"],
                                    classification.combined_impact,
                                )
                                || !report_table_numeric_cell_matches(
                                    &cells["peak_impact"],
                                    classification.combined_peak_impact,
                                )
                            {
                                return Err(tool_error(
                                    "REPORT_TABLE_GRADIENT_VALUE_MISMATCH",
                                    format!("rows[{index}] must reproduce the cited classification, combined active impact and combined peak impact for '{contributor}'"),
                                ));
                            }
                        }
                        "material_not_selected" => {
                            let is_material = family == "db_time_foreground_wait_events"
                                && material_foreground_waits(project)
                                    .keys()
                                    .any(|wait| wait.eq_ignore_ascii_case(contributor));
                            if classification.is_some() || !is_material {
                                return Err(tool_error(
                                    "REPORT_TABLE_GRADIENT_COVERAGE_MISMATCH",
                                    format!("rows[{index}] may use material_not_selected only for a foreground wait above {MATERIAL_FOREGROUND_WAIT_PCT_DBTIME}% DB Time that has no cross-model classification"),
                                ));
                            }
                        }
                        _ => unreachable!(),
                    }
                }
            }
            "analytic_signal_synthesis" => {
                validate_enum(
                    "confidence",
                    &cells["confidence"],
                    &["high", "medium", "low", "unknown"],
                )?;
                let signal_sections = evidence_refs
                    .iter()
                    .filter_map(|reference| state.evidence.get(reference))
                    .filter(|record| {
                        record.tool_name == "get_precomputed_analysis"
                            && record.project_id.as_deref() == Some(project_id)
                    })
                    .filter_map(|record| record.arguments.get("section").and_then(Value::as_str))
                    .filter(|section| {
                        matches!(
                            *section,
                            "full_gradients" | "load_profile_anomalies" | "anomaly_clusters"
                        )
                    })
                    .collect::<BTreeSet<_>>();
                if signal_sections.len() < 2 {
                    return Err(tool_error(
                        "INSUFFICIENT_ANALYTIC_SYNTHESIS_EVIDENCE",
                        format!("rows[{index}] must cite at least two distinct analytic signal families"),
                    ));
                }
            }
            "alert_log_errors" => {
                let error_code = cells["error_code"].as_str();
                let matching = evidence_refs.iter().any(|reference| {
                    let Some(record) = state.evidence.get(reference) else {
                        return false;
                    };
                    if record.tool_name != "get_alertlog_errors"
                        || record.project_id.as_deref() != Some(project_id)
                    {
                        return false;
                    }
                    record
                        .result
                        .get("error_summary")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .any(|summary| {
                            summary.get("code").and_then(Value::as_str) == Some(error_code)
                                && summary.get("event_records").and_then(Value::as_u64)
                                    == cells["event_records"].parse::<u64>().ok()
                                && summary.get("parse_detail_records").and_then(Value::as_u64)
                                    == cells["parse_detail_records"].parse::<u64>().ok()
                                && summary.get("max_reported_count").and_then(Value::as_u64)
                                    == cells["max_reported_count"].parse::<u64>().ok()
                                && summary.get("first_seen").and_then(Value::as_str)
                                    == Some(cells["first_seen"].as_str())
                                && summary.get("last_seen").and_then(Value::as_str)
                                    == Some(cells["last_seen"].as_str())
                        })
                });
                if !matching {
                    return Err(tool_error(
                        "REPORT_TABLE_ALERT_VALUE_MISMATCH",
                        format!("rows[{index}] must reproduce the cited alert error code, record counts, maximum reported counter and observed time bounds"),
                    ));
                }
            }
            "parameters" => {
                validate_enum(
                    "rating",
                    &cells["rating"],
                    &[
                        "good",
                        "acceptable",
                        "concern",
                        "critical",
                        "unknown",
                        "not_applicable",
                    ],
                )?;
                let parameter = cells["parameter"].as_str();
                if cells["observed_value"] == "<not present in collected data>" {
                    return Err(tool_error(
                        "UNAVAILABLE_PARAMETER_ROW",
                        format!("rows[{index}] must not turn an uncollected parameter into a reader-facing checklist row"),
                    ));
                }
                let matching = evidence_refs.iter().any(|reference| {
                    let Some(record) = state.evidence.get(reference) else {
                        return false;
                    };
                    record.tool_name == "get_init_parameter"
                        && record.project_id.as_deref() == Some(project_id)
                        && record
                            .result
                            .get("parameters")
                            .and_then(|parameters| parameters.get(parameter))
                            .and_then(Value::as_str)
                            .is_some_and(|value| value == cells["observed_value"])
                });
                if !matching {
                    return Err(tool_error(
                        "REPORT_TABLE_EVIDENCE_MISMATCH",
                        format!("rows[{index}] must cite get_init_parameter evidence with the exact observed value for project '{project_id}', parameter '{parameter}'"),
                    ));
                }
            }
            _ => unreachable!(),
        }

        parsed.push(ReportTableRow {
            cells,
            evidence_refs,
        });
    }
    Ok(parsed)
}

fn parse_guidance_quotations(
    value: Option<&Value>,
    guidance_refs: &[String],
    state: &AnalysisSession,
) -> std::result::Result<Vec<GuidanceQuotation>, Value> {
    let items = value.and_then(Value::as_array);
    if guidance_refs.is_empty() {
        if items.is_some_and(|items| !items.is_empty()) {
            return Err(tool_error(
                "GUIDANCE_QUOTE_WITHOUT_REFERENCE",
                "guidance_quotes cannot be supplied without matching guidance_refs",
            ));
        }
        return Ok(Vec::new());
    }
    let Some(items) = items else {
        return Err(tool_error(
            "GUIDANCE_QUOTE_REQUIRED",
            "Every applied guidance_ref requires a verbatim guidance_quotes entry",
        ));
    };
    if items.len() > 16 {
        return Err(tool_error(
            "TOO_MANY_GUIDANCE_QUOTES",
            "A finding or assessment may contain at most 16 guidance quotations",
        ));
    }

    let expected = guidance_refs.iter().cloned().collect::<BTreeSet<_>>();
    let mut observed = BTreeSet::new();
    let mut quotations = Vec::new();
    for item in items {
        let Some(object) = item.as_object() else {
            return Err(tool_error(
                "INVALID_GUIDANCE_QUOTE",
                "Each guidance_quotes entry must be an object",
            ));
        };
        let guidance_ref = required_string(object, "guidance_ref", 64)?;
        let quote = required_string(object, "quote", 2_000)?;
        if !expected.contains(&guidance_ref) {
            return Err(tool_error(
                "GUIDANCE_QUOTE_REFERENCE_MISMATCH",
                format!("Quotation references '{guidance_ref}', which is not in guidance_refs"),
            ));
        }
        if !observed.insert(guidance_ref.clone()) {
            return Err(tool_error(
                "DUPLICATE_GUIDANCE_QUOTE",
                format!("Duplicate quotation for '{guidance_ref}'"),
            ));
        }
        let source = state.guidance.get(&guidance_ref).ok_or_else(|| {
            tool_error(
                "UNKNOWN_GUIDANCE_REF",
                format!("Unknown guidance reference '{guidance_ref}'"),
            )
        })?;
        if !source.text.contains(&quote) {
            return Err(tool_error(
                "GUIDANCE_QUOTE_NOT_VERBATIM",
                format!(
                    "The quotation for '{guidance_ref}' is not a contiguous verbatim excerpt of the retrieved guidance"
                ),
            ));
        }
        quotations.push(GuidanceQuotation {
            guidance_ref,
            quote,
        });
    }
    if observed != expected {
        let missing = expected.difference(&observed).cloned().collect::<Vec<_>>();
        return Err(tool_error(
            "GUIDANCE_QUOTE_REQUIRED",
            format!(
                "Missing verbatim quotation for guidance reference(s): {}",
                missing.join(", ")
            ),
        ));
    }
    Ok(quotations)
}

fn required_string(
    arguments: &Map<String, Value>,
    name: &str,
    max_chars: usize,
) -> std::result::Result<String, Value> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| bounded_string(value, max_chars))
        .ok_or_else(|| tool_error("INVALID_ARGUMENT", format!("'{name}' is required")))
}

fn required_synthesis_string(
    arguments: &Map<String, Value>,
    name: &str,
    max_chars: usize,
    min_chars: usize,
) -> std::result::Result<String, Value> {
    let value = required_string(arguments, name, max_chars)?;
    if value.chars().count() < min_chars {
        return Err(tool_error(
            "DIAGNOSTIC_SYNTHESIS_TOO_VAGUE",
            format!(
                "'{name}' must be a reader-facing diagnostic statement of at least {min_chars} characters; placeholders and bare labels are not accepted"
            ),
        ));
    }
    Ok(value)
}

fn validate_distinct_diagnostic_statements(
    statements: &[(&str, &String)],
) -> std::result::Result<(), Value> {
    let mut observed = HashMap::<String, &str>::new();
    for (name, value) in statements {
        let normalized = value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if let Some(previous) = observed.insert(normalized, name) {
            return Err(tool_error(
                "DUPLICATED_DIAGNOSTIC_STATEMENT",
                format!(
                    "'{name}' duplicates '{previous}'. Each diagnostic field must add a distinct part of the symptom-to-decision narrative"
                ),
            ));
        }
    }
    Ok(())
}

fn optional_string(arguments: &Map<String, Value>, name: &str, max_chars: usize) -> String {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(|value| bounded_string(value, max_chars))
        .unwrap_or_default()
}

fn string_array(
    arguments: &Map<String, Value>,
    name: &str,
    max_items: usize,
    max_chars: usize,
) -> std::result::Result<Vec<String>, Value> {
    let Some(values) = arguments.get(name) else {
        return Ok(Vec::new());
    };
    let Some(values) = values.as_array() else {
        return Err(tool_error(
            "INVALID_ARGUMENT",
            format!("'{name}' must be an array of strings"),
        ));
    };
    if values.len() > max_items {
        return Err(tool_error(
            "INVALID_ARGUMENT",
            format!("'{name}' may contain at most {max_items} values"),
        ));
    }
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(|value| bounded_string(value, max_chars))
                .ok_or_else(|| {
                    tool_error(
                        "INVALID_ARGUMENT",
                        format!("'{name}' must contain only strings"),
                    )
                })
        })
        .collect()
}

fn validate_enum(name: &str, value: &str, allowed: &[&str]) -> std::result::Result<(), Value> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(tool_error(
            "INVALID_ARGUMENT",
            format!("'{name}' must be one of: {}", allowed.join(", ")),
        ))
    }
}

fn bounded_string(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn tool_error(code: &str, message: impl Into<String>) -> Value {
    json!({
        "error_code": code,
        "message": message.into()
    })
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let ordered = object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect::<BTreeMap<_, _>>();
            serde_json::to_string(&ordered).unwrap_or_default()
        }
        Value::Array(values) => {
            let values = values.iter().map(canonical_json).collect::<Vec<_>>();
            serde_json::to_string(&values).unwrap_or_default()
        }
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn evidence_anchor(reference: &str) -> String {
    reference
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn evidence_links(evidence: &[String]) -> String {
    evidence
        .iter()
        .map(|reference| format!("[{}](#evidence-{})", reference, evidence_anchor(reference)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_guidance_quotes(quotations: &[GuidanceQuotation], state: &AnalysisSession) -> String {
    let mut output = String::new();
    for quotation in quotations {
        let title = state
            .guidance
            .get(&quotation.guidance_ref)
            .map(|record| record.title.as_str())
            .unwrap_or("Diagnostic guidance");
        output.push_str(&format!(
            "> **JAS-MIN diagnostic rule ({} — {}):**\n>\n",
            quotation.guidance_ref, title
        ));
        for line in quotation.quote.lines() {
            output.push_str("> “");
            output.push_str(line);
            output.push_str("”\n");
        }
        output.push('\n');
    }
    output
}

fn cited_evidence_refs(state: &AnalysisSession) -> BTreeSet<String> {
    state
        .findings
        .values()
        .flat_map(|finding| finding.evidence_refs.iter())
        .chain(
            state
                .assessments
                .values()
                .flat_map(|assessment| assessment.evidence_refs.iter()),
        )
        .chain(
            state
                .report_tables
                .values()
                .flat_map(|table| table.rows.iter())
                .flat_map(|row| row.evidence_refs.iter()),
        )
        .cloned()
        .collect()
}

fn humanize_identifier(value: &str) -> String {
    let mut words = value
        .split('_')
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if let Some(first) = words.first_mut() {
        if let Some(initial) = first.get_mut(0..1) {
            initial.make_ascii_uppercase();
        }
    }
    words.join(" ")
}

fn evidence_scope(record: &EvidenceRecord) -> String {
    const USEFUL_ARGUMENTS: &[&str] = &[
        "section",
        "snap_id",
        "baseline_snap_id",
        "candidate_snap_id",
        "sql_id",
        "event_name",
        "name",
        "kind",
        "parameter_name",
        "date_from",
        "date_to",
        "limit",
    ];
    let mut parts = Vec::new();
    if let Some(project_id) = record.project_id.as_deref() {
        parts.push(format!("project `{project_id}`"));
    }
    if let Some(arguments) = record.arguments.as_object() {
        for key in USEFUL_ARGUMENTS {
            let Some(value) = arguments.get(*key) else {
                continue;
            };
            let displayed = match value {
                Value::String(value) => value.clone(),
                Value::Number(_) | Value::Bool(_) => value.to_string(),
                _ => continue,
            };
            parts.push(format!("{} `{}`", humanize_identifier(key), displayed));
        }
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        format!(" — {}.", parts.join("; "))
    }
}

fn severity_rank(value: &str) -> usize {
    match value {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        "low" => 3,
        _ => 4,
    }
}

fn priority_rank(value: &str) -> usize {
    match value {
        "immediate" => 0,
        "high" => 1,
        "medium" => 2,
        _ => 3,
    }
}

fn count_extension(directory: &Path, extension: &str) -> usize {
    read_files(directory)
        .iter()
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        })
        .count()
}

fn count_suffix(directory: &Path, suffix: &str) -> usize {
    read_files(directory)
        .iter()
        .filter(|path| path.to_string_lossy().ends_with(suffix))
        .count()
}

fn alert_timestamp_bounds(path: &Path) -> (Option<String>, Option<String>, usize) {
    let Ok(file) = File::open(path) else {
        return (None, None, 0);
    };
    let mut first = None;
    let mut last = None;
    let mut count = 0;
    for line in BufReader::new(file)
        .lines()
        .map_while(std::result::Result::ok)
    {
        let Some(timestamp) = iso_timestamp_prefix(&line) else {
            continue;
        };
        count += 1;
        if first.is_none() {
            first = Some(timestamp.to_string());
        }
        last = Some(timestamp.to_string());
    }
    (first, last, count)
}

fn iso_timestamp_prefix(line: &str) -> Option<&str> {
    let candidate = line.trim_start().split_ascii_whitespace().next()?;
    let bytes = candidate.as_bytes();
    let expected_digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    if bytes.len() < 19
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
        || expected_digits
            .iter()
            .any(|index| !bytes[*index].is_ascii_digit())
    {
        return None;
    }
    Some(candidate)
}

fn alert_coverage_status(
    bytes: u64,
    observed_date_from: Option<&str>,
    observed_date_to: Option<&str>,
    expected_date_from: Option<&str>,
    expected_date_to: Option<&str>,
) -> &'static str {
    if bytes == 0 {
        "missing"
    } else if observed_date_from.is_none() || observed_date_to.is_none() {
        "unknown"
    } else if expected_date_from
        .zip(observed_date_from)
        .is_some_and(|(expected, observed)| observed > expected)
        || expected_date_to
            .zip(observed_date_to)
            .is_some_and(|(expected, observed)| observed < expected)
    {
        "partial_relative_to_dataset"
    } else {
        "covers_dataset_dates"
    }
}

fn files_name_contains(directory: &Path, needle: &str) -> Vec<PathBuf> {
    read_files(directory)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.to_lowercase().contains(needle))
        })
        .collect()
}

fn count_regular_files(directory: &Path) -> usize {
    read_files(directory).len()
}

fn read_files(directory: &Path) -> Vec<PathBuf> {
    let mut paths = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awr::{DBInstance, LoadProfile, SQLElapsedTime, SnapInfo, AWR};
    use crate::reasonings::{
        MadAnomaliesEvents, TopForegroundWaitEvents, TopSQLsByElapsedTime, WaitEventsFromASH,
        WaitEventsWithStrongCorrelation,
    };

    fn runtime() -> AnalysisRuntime {
        let mut awr = AWR::default();
        awr.snap_info = SnapInfo {
            begin_snap_id: 10,
            end_snap_id: 11,
            begin_snap_time: "2026-08-01 10:00".to_string(),
            end_snap_time: "2026-08-01 11:00".to_string(),
        };
        let collection = AWRSCollection {
            db_instance_information: DBInstance {
                platform: "AIX 64-bit".to_string(),
                ..Default::default()
            },
            initialization_parameters: HashMap::new(),
            awrs: vec![awr],
            sql_text: HashMap::new(),
        };
        let mut runtime = AnalysisRuntime::new(
            collection,
            ReportForAI::default(),
            "nonexistent-test-dataset".to_string(),
            0,
            HashMap::new(),
            "nonexistent-test-dataset.html_reports".to_string(),
        );
        runtime.guidance = Arc::new(GuidanceLibrary::default());
        runtime
    }

    fn comparison_project(
        project_id: &str,
        load_values: &[f64],
        sql_elapsed_per_exec: &[f64],
    ) -> AnalysisProject {
        let awrs = load_values
            .iter()
            .enumerate()
            .map(|(index, load_value)| {
                let mut awr = AWR::default();
                awr.snap_info = SnapInfo {
                    begin_snap_id: (index + 1) as u64,
                    end_snap_id: (index + 2) as u64,
                    begin_snap_time: format!("2026-08-{:02} 10:00", index + 1),
                    end_snap_time: format!("2026-08-{:02} 11:00", index + 1),
                };
                let mut load_profile = LoadProfile::default();
                load_profile.stat_name = "User calls".to_string();
                load_profile.per_second = *load_value;
                awr.load_profile.push(load_profile);
                if let Some(per_exec) = sql_elapsed_per_exec.get(index) {
                    awr.sql_elapsed_time.push(SQLElapsedTime {
                        sql_id: "abc123".to_string(),
                        elapsed_time_s: per_exec * 10.0,
                        executions: 10,
                        elpased_time_exec_s: *per_exec,
                        sql_module: "comparison-test".to_string(),
                        sql_type: "SELECT".to_string(),
                        ..Default::default()
                    });
                }
                awr
            })
            .collect();
        AnalysisProject::new(
            project_id.to_string(),
            AWRSCollection {
                db_instance_information: DBInstance::default(),
                initialization_parameters: HashMap::new(),
                awrs,
                sql_text: HashMap::new(),
            },
            ReportForAI::default(),
            format!("nonexistent-{project_id}"),
            0,
            HashMap::new(),
            format!("nonexistent-{project_id}.html_reports"),
        )
    }

    fn comparison_runtime() -> AnalysisRuntime {
        let mut runtime = AnalysisRuntime::from_projects(vec![
            comparison_project("before", &[100.0, 120.0, 110.0], &[2.0, 2.2, 1.8]),
            comparison_project("after", &[70.0, 80.0, 75.0], &[1.0, 1.1, 0.9]),
        ])
        .unwrap();
        runtime.guidance = Arc::new(GuidanceLibrary::default());
        runtime
    }

    #[test]
    fn endpoint_accepts_requested_loopback_shorthand() {
        let endpoint: McpEndpoint = "127.0.0.1:4242/mcp".parse().unwrap();
        assert_eq!(endpoint.address, "127.0.0.1:4242".parse().unwrap());
        assert_eq!(endpoint.path, "/mcp");
        assert_eq!(endpoint.url(), "http://127.0.0.1:4242/mcp");
    }

    #[test]
    fn endpoint_rejects_non_loopback_binding() {
        let error = "0.0.0.0:4242/mcp".parse::<McpEndpoint>().unwrap_err();
        assert!(error.contains("loopback-only"));
    }

    #[test]
    fn tools_list_cache_hints_match_the_modern_protocol_contract() {
        assert!(!supports_tools_list_cache_hints(Some(
            &ProtocolVersion::V_2025_11_25
        )));
        assert!(supports_tools_list_cache_hints(Some(
            &ProtocolVersion::V_2026_07_28
        )));

        let modern = serde_json::to_value(tools_list_result(Vec::new(), true)).unwrap();
        assert_eq!(modern["ttlMs"], MCP_TOOLS_LIST_TTL_MS);
        assert_eq!(modern["cacheScope"], "private");
        assert_eq!(modern["resultType"], "complete");

        let legacy = serde_json::to_value(tools_list_result(Vec::new(), false)).unwrap();
        assert!(legacy.get("ttlMs").is_none());
        assert!(legacy.get("cacheScope").is_none());
    }

    #[test]
    fn tool_log_line_is_single_line_and_omits_payload_content() {
        let line = format_mcp_tool_log_line(
            "2026-08-05T12:00:00.123Z",
            7,
            "rpc\n42",
            "set_report_assessment",
            Some("A-20260805T085728Z-0001"),
            "ERROR",
            512,
            Some(240_001),
            Some(96),
            Some("SESSION\nLOCK"),
        );

        assert_eq!(line.lines().count(), 1);
        assert!(line.contains("status=ERROR call_id=7"));
        assert!(line.contains("rpc_id=\"rpc\\n42\""));
        assert!(line.contains("tool=\"set_report_assessment\""));
        assert!(line.contains("analysis_id=\"A-20260805T085728Z-0001\""));
        assert!(line.contains("duration_ms=240001"));
        assert!(line.contains("error_code=\"SESSION\\nLOCK\""));
        assert!(!line.contains("conclusion"));
        assert!(!line.contains("evidence_refs"));
    }

    #[test]
    fn tool_log_fields_are_bounded() {
        let oversized = "x".repeat(MAX_MCP_LOG_FIELD_CHARS + 50);
        let encoded = bounded_log_field(&oversized);
        assert_eq!(encoded.matches('x').count(), MAX_MCP_LOG_FIELD_CHARS);
        assert!(encoded.ends_with("...\""));
    }

    #[test]
    fn comparative_session_routes_projects_and_records_metric_evidence() {
        let runtime = comparison_runtime();
        let projects = runtime
            .call_tool("list_performance_projects", Map::new())
            .unwrap();
        assert_eq!(projects["project_count"], 2);

        let bootstrap = runtime
            .call_tool("start_performance_analysis", Map::new())
            .unwrap();
        assert_eq!(bootstrap["comparison_mode"], true);
        assert_eq!(bootstrap["project_ids"], json!(["after", "before"]));
        let analysis_id = bootstrap["analysis_id"].as_str().unwrap();

        let ambiguous = runtime
            .call_tool(
                "get_database_load_summary",
                Map::from_iter([("analysis_id".to_string(), json!(analysis_id))]),
            )
            .unwrap_err();
        assert_eq!(ambiguous["error_code"], "MISSING_PROJECT_ID");

        let evidence = runtime
            .call_tool(
                "compare_project_metric",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("baseline_project_id".to_string(), json!("before")),
                    ("candidate_project_id".to_string(), json!("after")),
                    ("kind".to_string(), json!("load_profile")),
                    ("name".to_string(), json!("User calls")),
                    ("direction".to_string(), json!("lower_is_better")),
                ]),
            )
            .unwrap();
        assert_eq!(evidence["evidence_id"], "E-0002");
        assert_eq!(
            evidence["result"]["comparison"]["classification"],
            "improved"
        );
        assert!(
            evidence["result"]["comparison"]["mean_delta_pct"]
                .as_f64()
                .unwrap()
                < 0.0
        );

        let invalid_field = runtime
            .call_tool(
                "compare_project_metric",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("baseline_project_id".to_string(), json!("before")),
                    ("candidate_project_id".to_string(), json!("after")),
                    ("kind".to_string(), json!("host_cpu")),
                    ("name".to_string(), json!("host_cpu")),
                    ("field".to_string(), json!("idle_percent_typo")),
                ]),
            )
            .unwrap_err();
        assert_eq!(invalid_field["error_code"], "INVALID_COMPARISON_FIELD");
    }

    #[test]
    fn sql_comparison_separates_efficiency_from_workload_volume() {
        let runtime = comparison_runtime();
        let bootstrap = runtime
            .call_tool("start_performance_analysis", Map::new())
            .unwrap();
        let analysis_id = bootstrap["analysis_id"].as_str().unwrap();
        let evidence = runtime
            .call_tool(
                "compare_project_sql",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("baseline_project_id".to_string(), json!("before")),
                    ("candidate_project_id".to_string(), json!("after")),
                    ("sql_id".to_string(), json!("ABC123")),
                ]),
            )
            .unwrap();
        assert_eq!(evidence["result"]["sql_id"], "abc123");
        let comparisons = evidence["result"]["comparisons"].as_array().unwrap();
        let elapsed_per_exec = comparisons
            .iter()
            .find(|item| item["metric"] == "elapsed_time_per_exec_s")
            .unwrap();
        assert_eq!(elapsed_per_exec["comparison"]["classification"], "improved");
        let executions = comparisons
            .iter()
            .find(|item| item["metric"] == "executions")
            .unwrap();
        assert_eq!(executions["comparison"]["classification"], "not_classified");
    }

    #[test]
    fn bootstrap_requires_explicit_handle_for_follow_up_calls() {
        let runtime = runtime();
        let bootstrap = runtime
            .call_tool("start_performance_analysis", Map::new())
            .unwrap();
        let analysis_id = bootstrap["analysis_id"].as_str().unwrap();
        assert_eq!(bootstrap["dataset_manifest"]["snapshots"], 1);
        assert_eq!(bootstrap["dataset_manifest"]["date_from"], "2026-08-01");
        assert_eq!(bootstrap["dataset_manifest"]["date_to"], "2026-08-01");
        assert_eq!(
            bootstrap["dataset_manifest"]["database"]["platform"],
            "AIX 64-bit"
        );
        assert!(bootstrap["quality_gates"][0]["required"].as_bool().unwrap());
        let aix_call = bootstrap["recommended_next_calls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|call| call["tool"] == "get_aix_cpu_entitlement_summary")
            .unwrap();
        assert_eq!(aix_call["arguments"]["date_from"], "2026-08-01");

        let missing = runtime
            .call_tool("get_analysis_catalog", Map::new())
            .unwrap_err();
        assert_eq!(missing["error_code"], "MISSING_ANALYSIS_ID");

        let catalog = runtime
            .call_tool(
                "get_analysis_catalog",
                Map::from_iter([("analysis_id".to_string(), json!(analysis_id))]),
            )
            .unwrap();
        assert!(catalog["available_calculations"].as_array().unwrap().len() >= 7);
    }

    #[test]
    fn report_contract_refuses_unreferenced_evidence() {
        let runtime = runtime();
        let bootstrap = runtime
            .call_tool("start_performance_analysis", Map::new())
            .unwrap();
        let analysis_id = bootstrap["analysis_id"].as_str().unwrap();
        let finding = Map::from_iter([
            ("analysis_id".to_string(), json!(analysis_id)),
            ("category".to_string(), json!("io")),
            ("title".to_string(), json!("Disk assessment")),
            ("severity".to_string(), json!("low")),
            ("confidence".to_string(), json!("high")),
            ("conclusion".to_string(), json!("Latency is low.")),
            (
                "mechanism".to_string(),
                json!("Low measured read latency would rule out storage response time as the primary mechanism."),
            ),
            (
                "temporal_pattern".to_string(),
                json!("The cited measurement is intended to cover the complete analyzed snapshot interval."),
            ),
            (
                "affected_workload".to_string(),
                json!("The assessment applies to database I/O consumers in the selected project scope."),
            ),
            (
                "evidence_limitations".to_string(),
                json!("The test checks reference validation and does not prove the storage diagnosis itself."),
            ),
            (
                "evidence_summary".to_string(),
                json!("The cited source would need to contain the measured latency."),
            ),
            ("evidence_refs".to_string(), json!(["E-9999"])),
        ]);
        let error = runtime.call_tool("record_finding", finding).unwrap_err();
        assert_eq!(error["error_code"], "UNKNOWN_EVIDENCE_REF");
    }

    #[test]
    fn report_contract_rejects_shallow_diagnostic_synthesis() {
        let runtime = runtime();
        let bootstrap = runtime
            .call_tool("start_performance_analysis", Map::new())
            .unwrap();
        let analysis_id = bootstrap["analysis_id"].as_str().unwrap();
        let finding = Map::from_iter([
            ("analysis_id".to_string(), json!(analysis_id)),
            ("category".to_string(), json!("wait_events")),
            ("title".to_string(), json!("Material cursor wait")),
            ("severity".to_string(), json!("high")),
            ("confidence".to_string(), json!("medium")),
            (
                "conclusion".to_string(),
                json!("The wait requires diagnostic follow-up."),
            ),
            ("mechanism".to_string(), json!("unknown")),
            (
                "temporal_pattern".to_string(),
                json!("The symptom occurred during one aligned peak snapshot."),
            ),
            (
                "affected_workload".to_string(),
                json!("No SQL attribution was available in the cited test evidence."),
            ),
            (
                "evidence_limitations".to_string(),
                json!("The test evidence does not establish a causal holder and waiter chain."),
            ),
            (
                "evidence_summary".to_string(),
                json!("The seed contains the exact project scope used by this validation test."),
            ),
            ("evidence_refs".to_string(), json!([SEED_EVIDENCE_ID])),
        ]);
        let error = runtime.call_tool("record_finding", finding).unwrap_err();
        assert_eq!(error["error_code"], "DIAGNOSTIC_SYNTHESIS_TOO_VAGUE");
        assert!(error["message"].as_str().unwrap().contains("mechanism"));
    }

    #[test]
    fn report_contract_rejects_copy_pasted_diagnostic_fields() {
        let runtime = runtime();
        let bootstrap = runtime
            .call_tool("start_performance_analysis", Map::new())
            .unwrap();
        let analysis_id = bootstrap["analysis_id"].as_str().unwrap();
        let repeated = "The same generic statement was copied into more than one diagnostic field.";
        let finding = Map::from_iter([
            ("analysis_id".to_string(), json!(analysis_id)),
            ("category".to_string(), json!("latches")),
            ("title".to_string(), json!("Shared-pool pressure")),
            ("severity".to_string(), json!("high")),
            ("confidence".to_string(), json!("medium")),
            ("conclusion".to_string(), json!(repeated)),
            ("mechanism".to_string(), json!(repeated)),
            (
                "temporal_pattern".to_string(),
                json!("The signal occurred in the aligned synthetic peak window."),
            ),
            (
                "affected_workload".to_string(),
                json!("The selected synthetic project is the named workload scope."),
            ),
            (
                "evidence_limitations".to_string(),
                json!("The synthetic evidence cannot establish production causality."),
            ),
            (
                "evidence_summary".to_string(),
                json!("The seed contains the exact synthetic project scope used by this test."),
            ),
            ("evidence_refs".to_string(), json!([SEED_EVIDENCE_ID])),
        ]);
        let error = runtime.call_tool("record_finding", finding).unwrap_err();
        assert_eq!(error["error_code"], "DUPLICATED_DIAGNOSTIC_STATEMENT");
    }

    #[test]
    fn report_gate_requires_every_supplied_plan_and_child_cursor_analysis_row() {
        let test_directory = std::env::temp_dir().join(format!(
            "jas-min-mcp-report-coverage-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let stem = test_directory.join("coverage-project");
        let attachments = PathBuf::from(format!("{}_attachments", stem.display()));
        std::fs::create_dir_all(&attachments).unwrap();
        std::fs::write(
            attachments.join("abc123.xplan"),
            b"Plan hash value: 123\n| Id | Operation | Name |\n",
        )
        .unwrap();
        std::fs::write(
            attachments.join("abc123.shared_cursor_reasons"),
            b"ChildNode<0> NLS Settings mismatch\n",
        )
        .unwrap();

        let runtime = AnalysisRuntime::new(
            AWRSCollection {
                db_instance_information: DBInstance::default(),
                initialization_parameters: HashMap::new(),
                awrs: Vec::new(),
                sql_text: HashMap::new(),
            },
            ReportForAI::default(),
            stem.to_string_lossy().to_string(),
            2,
            HashMap::new(),
            test_directory
                .join("coverage-project.html_reports")
                .to_string_lossy()
                .to_string(),
        );
        let bootstrap = runtime
            .call_tool("start_performance_analysis", Map::new())
            .unwrap();
        let analysis_id = bootstrap["analysis_id"].as_str().unwrap();
        let project_id = bootstrap["project_ids"][0].as_str().unwrap();
        let status = runtime
            .call_tool(
                "get_report_status",
                Map::from_iter([("analysis_id".to_string(), json!(analysis_id))]),
            )
            .unwrap();
        let missing_evidence = status["missing_required_evidence"].as_array().unwrap();
        assert!(missing_evidence.contains(&json!(format!(
            "get_sql_execution_plan:{project_id}:abc123:123"
        ))));
        assert!(missing_evidence.contains(&json!(format!(
            "get_child_cursor_reasons:{project_id}:abc123"
        ))));
        assert!(status["missing_structured_table_kinds"]
            .as_array()
            .unwrap()
            .contains(&json!("execution_plans")));
        assert!(status["missing_structured_table_kinds"]
            .as_array()
            .unwrap()
            .contains(&json!("child_cursors")));

        let plan_evidence = runtime
            .call_tool(
                "get_sql_execution_plan",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("sql_id".to_string(), json!("abc123")),
                    ("plan_hash".to_string(), json!("123")),
                ]),
            )
            .unwrap();
        let cursor_evidence = runtime
            .call_tool(
                "get_child_cursor_reasons",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("sql_id".to_string(), json!("abc123")),
                ]),
            )
            .unwrap();

        let coverage_plan_row = json!({
            "project_id": project_id,
            "sql_id": "abc123",
            "plan_hash": "123",
            "plan_status": "analyzed",
            "selection_reason": "The supplied attachment is reviewed for coverage, but no workload evidence establishes this SQL as a tuning priority.",
            "workload_evidence": "No SQL timeline or top-SQL measurement was observed for this synthetic fixture, so workload impact is not established.",
            "temporal_evidence": "The fixture contains a plan attachment without snapshot coverage or representative execution timing.",
            "comparative_context": "No baseline or second project contains an observed comparison for this synthetic SQL_ID.",
            "key_operations": "TABLE ACCESS FULL",
            "access_and_joins": "Full scan; no join in fixture.",
            "cardinality": "No A-Rows in fixture.",
            "partition_parallelism": "No partition or PX evidence.",
            "risk": "Fixture-only scan.",
            "recommendation_type": "no_change",
            "recommendation_rationale": "The fixture exposes a full scan but contains no workload or runtime row-source evidence that would justify a plan change.",
            "scope_limitations": "A costed full scan alone cannot prove high resource consumption, bad cardinality, or business impact.",
            "action": "Capture DBMS_XPLAN with ALLSTATS LAST for this SQL_ID before deciding whether the scan needs an index or rewrite.",
            "success_metric": "Establish snapshot frequency, DB time contribution, buffer gets per execution, and actual row-source counts before tuning.",
            "evidence_refs": [plan_evidence["evidence_id"]]
        });
        let mut unjustified_action = coverage_plan_row.clone();
        unjustified_action["recommendation_type"] = json!("instrumentation");
        let error = runtime
            .call_tool(
                "record_report_table",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("kind".to_string(), json!("execution_plans")),
                    ("title".to_string(), json!("Execution plan analysis")),
                    ("rows".to_string(), json!([unjustified_action])),
                ]),
            )
            .unwrap_err();
        assert_eq!(
            error["error_code"],
            "EXECUTION_PLAN_WITHOUT_WORKLOAD_EVIDENCE"
        );

        runtime
            .call_tool(
                "record_report_table",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("kind".to_string(), json!("execution_plans")),
                    ("title".to_string(), json!("Execution plan analysis")),
                    ("rows".to_string(), json!([coverage_plan_row])),
                ]),
            )
            .unwrap();
        runtime
            .call_tool(
                "record_report_table",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("kind".to_string(), json!("child_cursors")),
                    ("title".to_string(), json!("Child cursor analysis")),
                    (
                        "rows".to_string(),
                        json!([{
                            "project_id": project_id,
                            "sql_id": "abc123",
                            "child_cursors": "1 fixture node",
                            "direct_reasons": "NLS Settings mismatch",
                            "optimizer_bind_context": "NLS context differs.",
                            "performance_impact": "Requires workload correlation.",
                            "action": "Normalize client NLS settings.",
                            "evidence_refs": [cursor_evidence["evidence_id"]]
                        }]),
                    ),
                ]),
            )
            .unwrap();

        let updated = runtime
            .call_tool(
                "get_report_status",
                Map::from_iter([("analysis_id".to_string(), json!(analysis_id))]),
            )
            .unwrap();
        assert!(!updated["missing_structured_table_rows"]
            .as_array()
            .unwrap()
            .contains(&json!(format!("execution_plans:{project_id}:abc123:123"))));
        assert!(!updated["missing_structured_table_rows"]
            .as_array()
            .unwrap()
            .contains(&json!(format!("child_cursors:{project_id}:abc123"))));

        std::fs::remove_dir_all(test_directory).unwrap();
    }

    #[test]
    fn plsql_execution_artifact_accepts_not_applicable_and_rejects_recapture() {
        let test_directory = std::env::temp_dir().join(format!(
            "jas-min-mcp-plsql-coverage-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let stem = test_directory.join("plsql-project");
        let attachments = PathBuf::from(format!("{}_attachments", stem.display()));
        std::fs::create_dir_all(&attachments).unwrap();
        std::fs::write(
            attachments.join("ffnd3s42wxp77.xplan"),
            b"SQL_ID ffnd3s42wxp77\nBEGIN FFP513_22032019.FORMULA;END;\n",
        )
        .unwrap();

        let runtime = AnalysisRuntime::new(
            AWRSCollection {
                db_instance_information: DBInstance::default(),
                initialization_parameters: HashMap::new(),
                awrs: Vec::new(),
                sql_text: HashMap::new(),
            },
            ReportForAI::default(),
            stem.to_string_lossy().to_string(),
            2,
            HashMap::new(),
            test_directory
                .join("plsql-project.html_reports")
                .to_string_lossy()
                .to_string(),
        );
        let bootstrap = runtime
            .call_tool("start_performance_analysis", Map::new())
            .unwrap();
        let analysis_id = bootstrap["analysis_id"].as_str().unwrap();
        let project_id = bootstrap["project_ids"][0].as_str().unwrap();
        let evidence = runtime
            .call_tool(
                "get_sql_execution_plan",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("sql_id".to_string(), json!("ffnd3s42wxp77")),
                ]),
            )
            .unwrap();

        let base_row = json!({
            "project_id": project_id,
            "sql_id": "ffnd3s42wxp77",
            "plan_hash": "not_applicable",
            "plan_status": "not_applicable_plsql",
            "selection_reason": "The supplied execution artifact is mandatory coverage and contains a PL/SQL entry point rather than a SQL cursor plan.",
            "workload_evidence": "This synthetic fixture provides artifact classification only and does not establish measured production workload impact.",
            "temporal_evidence": "The attachment has no snapshot-aligned runtime coverage, so its execution frequency and duration remain unknown.",
            "comparative_context": "No baseline or candidate execution is available for this synthetic PL/SQL fixture and no difference is claimed.",
            "key_operations": "Not applicable to a PL/SQL entry point.",
            "access_and_joins": "Inner SQL statements were not captured in this fixture.",
            "cardinality": "No top-level row-source cardinality applies to the PL/SQL call.",
            "partition_parallelism": "Unknown until material inner SQL statements are identified.",
            "risk": "Treating this PL/SQL call as a failed SQL plan would prescribe invalid recapture work.",
            "recommendation_type": "no_change",
            "recommendation_rationale": "A top-level SQL execution plan is not applicable; future tuning must identify expensive inner SQL or PL/SQL lines first.",
            "scope_limitations": "The PL/SQL entry point text alone cannot identify the inner statement, line, call path, or resource consumer.",
            "action": "If workload evidence makes this call material, profile it with SQL trace, ASH attribution, DBMS_HPROF, or DBMS_PROFILER and inspect inner SQL plans.",
            "success_metric": "Identify the inner SQL_ID or PL/SQL line responsible for measured DB Time and verify improvement in the same workload window.",
            "evidence_refs": [evidence["evidence_id"]]
        });
        let mut invalid_recapture = base_row.clone();
        invalid_recapture["recommendation_type"] = json!("recapture");
        let error = runtime
            .call_tool(
                "record_report_table",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("kind".to_string(), json!("execution_plans")),
                    ("title".to_string(), json!("Execution artifacts")),
                    ("rows".to_string(), json!([invalid_recapture])),
                ]),
            )
            .unwrap_err();
        assert_eq!(
            error["error_code"],
            "REPORT_TABLE_PLAN_RECOMMENDATION_MISMATCH"
        );

        runtime
            .call_tool(
                "record_report_table",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("kind".to_string(), json!("execution_plans")),
                    ("title".to_string(), json!("Execution artifacts")),
                    ("rows".to_string(), json!([base_row])),
                ]),
            )
            .unwrap();
        let status = runtime
            .call_tool(
                "get_report_status",
                Map::from_iter([("analysis_id".to_string(), json!(analysis_id))]),
            )
            .unwrap();
        assert!(!status["missing_structured_table_rows"]
            .as_array()
            .unwrap()
            .contains(&json!(format!(
                "execution_plans:{project_id}:ffnd3s42wxp77:not_applicable"
            ))));

        std::fs::remove_dir_all(test_directory).unwrap();
    }

    #[test]
    fn report_gate_separates_gradient_anomaly_and_cluster_families() {
        let report = ReportForAI {
            db_time_gradient_fg_wait_events: Some(crate::reasonings::DbTimeGradientSection {
                cross_model_classifications: vec![CrossModelClassification {
                    event_name: "cursor: pin S wait on X".to_string(),
                    classification: "CONFIRMED_BOTTLENECK_EN_COLLINEAR".to_string(),
                    priority: 1,
                    combined_impact: 17_871.0,
                    combined_peak_impact: 281_499.0,
                    in_ridge: true,
                    in_huber: true,
                    in_quantile95: true,
                    ..Default::default()
                }],
                ..Default::default()
            }),
            load_profile_anomalies: vec![crate::reasonings::LoadProfileAnomalies::default()],
            anomaly_clusters: vec![crate::reasonings::AnomlyCluster::default()],
            ..Default::default()
        };
        let runtime = AnalysisRuntime::new(
            AWRSCollection {
                db_instance_information: DBInstance::default(),
                initialization_parameters: HashMap::new(),
                awrs: Vec::new(),
                sql_text: HashMap::new(),
            },
            report,
            "analytic-contract".to_string(),
            0,
            HashMap::new(),
            "analytic-contract.html_reports".to_string(),
        );
        let bootstrap = runtime
            .call_tool("start_performance_analysis", Map::new())
            .unwrap();
        let analysis_id = bootstrap["analysis_id"].as_str().unwrap();
        let project_id = bootstrap["project_ids"][0].as_str().unwrap();
        let status = runtime
            .call_tool(
                "get_report_status",
                Map::from_iter([("analysis_id".to_string(), json!(analysis_id))]),
            )
            .unwrap();
        let kinds = status["required_structured_table_kinds"]
            .as_array()
            .unwrap();
        for kind in [
            "gradients",
            "anomalies",
            "anomaly_clusters",
            "analytic_signal_synthesis",
        ] {
            assert!(kinds.contains(&json!(kind)));
        }
        assert!(!kinds.contains(&json!("gradients_anomalies")));
        let rows = status["missing_structured_table_rows"].as_array().unwrap();
        assert!(rows.contains(&json!(format!(
            "gradients:{project_id}:db_time_foreground_wait_events:triangulated:cursor: pin S wait on X"
        ))));
        assert!(rows.contains(&json!(format!("anomalies:{project_id}:at_least_one"))));
        assert!(rows.contains(&json!(format!(
            "anomaly_clusters:{project_id}:at_least_one"
        ))));
        assert!(rows.contains(&json!(format!(
            "analytic_signal_synthesis:{project_id}:at_least_one"
        ))));
    }

    #[test]
    fn mcp_catalog_adds_analysis_id_to_every_evidence_tool() {
        let runtime = runtime();
        let tools = build_mcp_tools(&runtime);
        let list_snapshots = tools
            .iter()
            .find(|tool| tool.name == "list_snapshots")
            .unwrap();
        assert_eq!(
            list_snapshots.input_schema["properties"]["analysis_id"]["type"],
            "string"
        );
        assert!(list_snapshots.input_schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("analysis_id")));
        assert_eq!(
            list_snapshots.input_schema["properties"]["project_id"]["type"],
            "string"
        );
        let start = tools
            .iter()
            .find(|tool| tool.name == "start_performance_analysis")
            .unwrap();
        assert!(start.input_schema["properties"]
            .get("analysis_id")
            .is_none());
        let projects = tools
            .iter()
            .find(|tool| tool.name == "list_performance_projects")
            .unwrap();
        assert!(projects.input_schema["properties"]
            .get("analysis_id")
            .is_none());
        let compare = tools
            .iter()
            .find(|tool| tool.name == "compare_project_metric")
            .unwrap();
        assert!(compare.input_schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("analysis_id")));
        let html = tools
            .iter()
            .find(|tool| tool.name == "convert_markdown_to_html")
            .unwrap();
        assert!(html.input_schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("analysis_id")));
        assert!(html.input_schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("markdown")));
    }

    #[test]
    fn bootstrap_preview_omits_verbose_anomaly_payloads() {
        let mut report = ReportForAI::default();
        report
            .top_foreground_wait_events
            .push(TopForegroundWaitEvents {
                event_name: "log file sync".to_string(),
                median_absolute_deviation_anomalies: vec![MadAnomaliesEvents::default(); 500],
                ..Default::default()
            });
        let preview = mcp_triage_preview(&report);
        let encoded = serde_json::to_string(&preview).unwrap();
        assert!(encoded.len() < 5_000);
        assert!(!encoded.contains("median_absolute_deviation_anomalies"));
    }

    #[test]
    fn wait_sql_contributors_rank_aligned_correlation_and_expose_missing_plan() {
        let sql_id = "16zny8vayhh40";
        let event_name = "cursor: pin S wait on X";
        let project = ProjectData {
            project_id: Arc::new("node-1".to_string()),
            collection: Arc::new(AWRSCollection {
                db_instance_information: DBInstance::default(),
                initialization_parameters: HashMap::new(),
                awrs: Vec::new(),
                sql_text: HashMap::from([(sql_id.to_string(), "select 1 from dual".to_string())]),
            }),
            report: Arc::new(ReportForAI {
                top_sqls_by_elapsed_time: vec![TopSQLsByElapsedTime {
                    sql_id: sql_id.to_string(),
                    module: "e:PA:cp:pa/PAAPIMPR".to_string(),
                    sql_type: "SELECT".to_string(),
                    marked_as_top_in_pct_of_probes: 8.5,
                    avg_elapsed_time_by_exec: 1.25,
                    avg_cpu_time_by_exec: 0.75,
                    wait_events_with_strong_pearson_correlation: vec![
                        WaitEventsWithStrongCorrelation {
                            event_name: event_name.to_string(),
                            correlation_value: 0.90,
                        },
                    ],
                    wait_events_found_in_ash_sections_for_this_sql: vec![WaitEventsFromASH {
                        event_name: event_name.to_string(),
                        avg_pct_of_dbtime_in_sql: 7.01,
                        stddev_pct_of_dbtime_in_sql: 4.11,
                        count: 60,
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }),
            stem: Arc::new("nonexistent-wait-contributor-dataset".to_string()),
            security_level: 0,
            report_links: Arc::new(HashMap::new()),
            html_reports_dir: Arc::new("nonexistent.html_reports".to_string()),
        };

        let result = wait_sql_contributors(&project, event_name, 5);
        assert_eq!(result["returned"], 1);
        let contributor = &result["contributors"][0];
        assert_eq!(contributor["sql_id"], sql_id);
        assert_eq!(contributor["evidence_basis"], "aligned_correlation_and_ash");
        assert_eq!(contributor["pearson_correlation"], 0.90);
        assert_eq!(contributor["ash_avg_pct_dbtime_in_sql"], 7.01);
        assert_eq!(contributor["ash_samples"], 60);
        assert_eq!(contributor["plan_coverage"], "missing_attachment");
        assert_eq!(contributor["plan_review_required"], true);
    }

    #[test]
    fn finding_shortcuts_prefer_the_longest_wait_name_without_nested_code_markup() {
        let state = AnalysisSession::new(
            json!({}),
            ReportConfig::default(),
            vec!["node-1".to_string()],
        );
        let finding = ReportFinding {
            finding_id: "F-0001".to_string(),
            category: "wait_events".to_string(),
            title: "cursor: pin S wait on X is material".to_string(),
            severity: "critical".to_string(),
            confidence: "high".to_string(),
            conclusion: "The complete wait name identifies the observed symptom.".to_string(),
            mechanism: "The complete mutex wait name preserves the blocking cursor-pin state under investigation.".to_string(),
            temporal_pattern: "The test fixture represents a material wait observed in an aligned snapshot window.".to_string(),
            affected_workload: "The affected SQL workload is represented by project-scoped entity links in this fixture.".to_string(),
            evidence_limitations: "The fixture validates link selection only and does not claim runtime causality.".to_string(),
            evidence_summary: "The complete wait name was present in the measured evidence."
                .to_string(),
            details: String::new(),
            evidence_refs: vec![SEED_EVIDENCE_ID.to_string()],
            guidance_refs: Vec::new(),
            guidance_quotes: Vec::new(),
            recommendations: Vec::new(),
        };
        let links = ReportEntityLinks {
            targets: BTreeMap::from([
                (
                    (
                        "node-1".to_string(),
                        "wait".to_string(),
                        "cursor: pin s".to_string(),
                    ),
                    "/tmp/short-wait.html".to_string(),
                ),
                (
                    (
                        "node-1".to_string(),
                        "wait".to_string(),
                        "cursor: pin s wait on x".to_string(),
                    ),
                    "/tmp/complete-wait.html".to_string(),
                ),
            ]),
        };
        let labels = BTreeMap::from([("node-1".to_string(), "NODE1".to_string())]);

        let rendered = render_finding_entity_shortcuts(&finding, &state, &labels, &links);
        assert!(rendered.contains("/tmp/complete-wait.html"));
        assert!(!rendered.contains("/tmp/short-wait.html"));
        assert!(!rendered.contains('`'));
    }

    #[test]
    fn report_sections_lead_with_diagnostic_synthesis_before_tables() {
        let mut state = AnalysisSession::new(
            json!({}),
            ReportConfig::default(),
            vec!["node-1".to_string()],
        );
        state.findings.insert(
            "F-0001".to_string(),
            ReportFinding {
                finding_id: "F-0001".to_string(),
                category: "parameters".to_string(),
                title: "Parameter governance needs measured review".to_string(),
                severity: "medium".to_string(),
                confidence: "high".to_string(),
                conclusion: "The observed value requires a workload-scoped governance decision."
                    .to_string(),
                mechanism: "The permissive ceiling can hide lifecycle defects but is not a stand-alone bottleneck."
                    .to_string(),
                temporal_pattern: "The value was collected for the complete synthetic project interval."
                    .to_string(),
                affected_workload: "Sessions using the selected project parameter context are the review scope."
                    .to_string(),
                evidence_limitations: "Configuration alone cannot prove the observed wait or establish a safe replacement value."
                    .to_string(),
                evidence_summary: "The synthetic project reports the exact observed parameter value used by this renderer test."
                    .to_string(),
                details: "Additional parameter detail retained for standard rendering.".to_string(),
                evidence_refs: vec![SEED_EVIDENCE_ID.to_string()],
                guidance_refs: Vec::new(),
                guidance_quotes: Vec::new(),
                recommendations: Vec::new(),
            },
        );
        state.report_tables.insert(
            "T-0001".to_string(),
            ReportTable {
                table_id: "T-0001".to_string(),
                kind: "parameters".to_string(),
                category: "parameters".to_string(),
                title: "Parameter evidence".to_string(),
                rows: vec![ReportTableRow {
                    cells: BTreeMap::from([
                        ("project_id".to_string(), "node-1".to_string()),
                        ("parameter".to_string(), "open_cursors".to_string()),
                        ("observed_value".to_string(), "60000".to_string()),
                        ("rating".to_string(), "concern".to_string()),
                        (
                            "performance_relevance".to_string(),
                            "Cursor lifecycle governance.".to_string(),
                        ),
                        (
                            "finding".to_string(),
                            "The value is unusually permissive.".to_string(),
                        ),
                        (
                            "action".to_string(),
                            "Measure session distributions before changing it.".to_string(),
                        ),
                    ]),
                    evidence_refs: vec![SEED_EVIDENCE_ID.to_string()],
                }],
            },
        );
        let document = json!({
            "analysis_id": "A-test",
            "revision": 1,
            "datasets": [{
                "project_id": "node-1",
                "dataset_stem": "NODE1_awr_test",
                "database": {"instance_num": 1}
            }]
        });
        let markdown = render_markdown(&document, &state, &ReportEntityLinks::default());
        let finding_position = markdown
            .find("### Parameter governance needs measured review")
            .unwrap();
        let table_position = markdown.find("### Structured technical evidence").unwrap();
        assert!(finding_position < table_position);
        assert!(markdown.contains("<summary>Additional diagnostic context</summary>"));
        let html = render_markdown_html_document(&markdown, "", "/tmp", HashMap::new());
        assert!(html.contains("class=\"finding-detail\""));
        assert!(html.contains("Additional parameter detail retained for standard rendering."));
    }

    #[test]
    fn source_report_links_are_human_readable_and_clickable() {
        let document = json!({
            "datasets": [{
                "project_id": "node-1",
                "source_reports": {
                    "main": "node-1.html_reports/jasmin_main.html",
                    "main_present": true,
                    "load_profile": "node-1.html_reports/stats/jasmin_highlight.html",
                    "load_profile_present": true,
                    "load_profile_secondary": "node-1.html_reports/stats/jasmin_highlight2.html",
                    "load_profile_secondary_present": false
                }
            }]
        });
        let rendered = render_source_report_links(&document);
        assert!(rendered.contains("Interactive source reports"));
        assert!(
            rendered.contains("[Main JAS-MIN dashboard](<node-1.html_reports/jasmin_main.html>)")
        );
        assert!(
            rendered.contains("[Load profile](<node-1.html_reports/stats/jasmin_highlight.html>)")
        );
        assert!(!rendered.contains("Secondary load profile"));

        let comparative_html = render_markdown_html_document(
            "# Oracle Performance Analysis\n\nComparison body.",
            "",
            "/tmp",
            HashMap::new(),
        );
        assert!(!comparative_html.contains("<iframe"));
        assert!(!comparative_html.contains("/jasmin_main.html"));
    }

    #[test]
    fn comparative_entity_links_resolve_to_each_projects_detail_pages() {
        let root = std::env::temp_dir().join(format!(
            "jas-min-entity-links-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let mut projects = BTreeMap::new();
        for project_id in ["node-1", "node-2"] {
            let report_dir = root.join(format!("{project_id}.html_reports"));
            std::fs::create_dir_all(report_dir.join("fg")).unwrap();
            std::fs::create_dir_all(report_dir.join("sqlid")).unwrap();
            let wait_file = report_dir.join(get_safe_filename(
                "cursor: pin S wait on X".to_string(),
                "fg".to_string(),
            ));
            let sql_file = report_dir.join("sqlid/sqlid_abc123.html");
            std::fs::write(&wait_file, b"wait").unwrap();
            std::fs::write(&sql_file, b"sql").unwrap();
            projects.insert(
                project_id.to_string(),
                Arc::new(ProjectData {
                    project_id: Arc::new(project_id.to_string()),
                    collection: Arc::new(AWRSCollection {
                        db_instance_information: DBInstance::default(),
                        initialization_parameters: HashMap::new(),
                        awrs: Vec::new(),
                        sql_text: HashMap::new(),
                    }),
                    report: Arc::new(ReportForAI::default()),
                    stem: Arc::new(root.join(project_id).to_string_lossy().to_string()),
                    security_level: 0,
                    report_links: Arc::new(HashMap::from([
                        (
                            "FG".to_string(),
                            HashSet::from(["cursor: pin S wait on X".to_string()]),
                        ),
                        ("SQL".to_string(), HashSet::from(["abc123".to_string()])),
                    ])),
                    html_reports_dir: Arc::new(report_dir.to_string_lossy().to_string()),
                }),
            );
        }
        let project_ids = vec!["node-1".to_string(), "node-2".to_string()];
        let links = ReportEntityLinks::build(&project_ids, &projects);
        let first = links.target("node-1", "sql", "abc123").unwrap();
        let second = links.target("node-2", "sql", "abc123").unwrap();
        assert_ne!(first, second);
        assert!(first.ends_with("node-1.html_reports/sqlid/sqlid_abc123.html"));
        assert!(second.ends_with("node-2.html_reports/sqlid/sqlid_abc123.html"));
        assert!(links
            .target("node-1", "wait", "cursor: pin S wait on X")
            .is_some());
        let html = format!(
            "<a href=\"{}\">one</a><a href=\"{}\">two</a>",
            first, second
        );
        assert_eq!(validate_local_html_targets(&html, &root).unwrap(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn human_report_tables_group_instances_and_hide_non_actionable_parameters() {
        let mut state = AnalysisSession::new(
            json!({}),
            ReportConfig::default(),
            vec!["node-1".to_string(), "node-2".to_string()],
        );
        let parameter_row = |project_id: &str, parameter: &str, rating: &str| ReportTableRow {
            cells: BTreeMap::from([
                ("project_id".to_string(), project_id.to_string()),
                ("parameter".to_string(), parameter.to_string()),
                ("observed_value".to_string(), "observed".to_string()),
                ("rating".to_string(), rating.to_string()),
                (
                    "performance_relevance".to_string(),
                    "Material to the tested workload.".to_string(),
                ),
                (
                    "finding".to_string(),
                    "Evidence-backed assessment.".to_string(),
                ),
                (
                    "action".to_string(),
                    "Apply the scoped DBA action.".to_string(),
                ),
            ]),
            evidence_refs: vec![SEED_EVIDENCE_ID.to_string()],
        };
        let table = ReportTable {
            table_id: "T-parameters".to_string(),
            kind: "parameters".to_string(),
            category: "parameters".to_string(),
            title: "Parameters requiring DBA attention".to_string(),
            rows: vec![
                parameter_row("node-1", "sga_target", "good"),
                parameter_row("node-1", "cpu_count", "concern"),
                parameter_row("node-2", "resource_manager_plan", "critical"),
                parameter_row("node-2", "memory_target", "unknown"),
            ],
        };
        let labels = BTreeMap::from([
            (
                "node-1".to_string(),
                "CEBSOFPR1 · Oracle instance 1".to_string(),
            ),
            (
                "node-2".to_string(),
                "CEBSOFPR2 · Oracle instance 2".to_string(),
            ),
        ]);

        let rendered = render_report_table(&table, &state, &labels, &ReportEntityLinks::default());
        assert!(rendered.contains("#### CEBSOFPR1 · Oracle instance 1"));
        assert!(rendered.contains("#### CEBSOFPR2 · Oracle instance 2"));
        assert!(rendered.contains("cpu_count"));
        assert!(rendered.contains("resource_manager_plan"));
        assert!(!rendered.contains("sga_target"));
        assert!(!rendered.contains("memory_target"));
        assert!(!rendered.contains("| Project |"));

        let segment_row = |statistic: &str, object_id: &str| ReportTableRow {
            cells: BTreeMap::from([
                ("project_id".to_string(), "node-1".to_string()),
                ("statistic".to_string(), statistic.to_string()),
                ("segment_name".to_string(), "T_HOT".to_string()),
                ("segment_type".to_string(), "TABLE".to_string()),
                ("object_id".to_string(), object_id.to_string()),
                ("data_object_id".to_string(), object_id.to_string()),
                ("occurrence_pct".to_string(), "50".to_string()),
                ("average".to_string(), "10".to_string()),
                ("stddev".to_string(), "2".to_string()),
                (
                    "interpretation".to_string(),
                    "Repeated hotspot.".to_string(),
                ),
                (
                    "action".to_string(),
                    "Correlate with SQL access.".to_string(),
                ),
            ]),
            evidence_refs: vec![SEED_EVIDENCE_ID.to_string()],
        };
        let segments = ReportTable {
            table_id: "T-segments".to_string(),
            kind: "segments".to_string(),
            category: "segments".to_string(),
            title: "Segment statistics by hotspot family".to_string(),
            rows: vec![
                segment_row("buffer_busy_waits", "1"),
                segment_row("physical_writes", "2"),
            ],
        };
        let segment_rendered =
            render_report_table(&segments, &state, &labels, &ReportEntityLinks::default());
        assert!(segment_rendered.contains("##### Buffer busy waits"));
        assert!(segment_rendered.contains("##### Physical writes"));
        assert!(!segment_rendered.contains("Segment statistic"));

        state.evidence.insert(
            "E-plan".to_string(),
            EvidenceRecord {
                evidence_id: "E-plan".to_string(),
                tool_name: "get_sql_execution_plan".to_string(),
                project_id: Some("node-1".to_string()),
                arguments: json!({"sql_id": "abc123", "plan_hash": "42"}),
                result: json!({
                    "plan_graph": {
                        "operations": [{
                            "id": 0,
                            "parent_id": null,
                            "depth": 0,
                            "operation": "SELECT STATEMENT",
                            "object_name": "",
                            "estimated_rows": "1",
                            "actual_rows": "",
                            "starts": "",
                            "cost": "10",
                            "elapsed_time": "00:00:01",
                            "temp_space": "",
                            "severity": "informational",
                            "flags": [],
                            "on_flagged_path": true
                        }, {
                            "id": 1,
                            "parent_id": 0,
                            "depth": 1,
                            "operation": "TABLE ACCESS FULL",
                            "object_name": "T_BIG",
                            "estimated_rows": "1000",
                            "actual_rows": "",
                            "starts": "",
                            "cost": "10",
                            "elapsed_time": "00:00:01",
                            "temp_space": "",
                            "severity": "medium",
                            "flags": ["full table scan"],
                            "on_flagged_path": true
                        }]
                    }
                }),
            },
        );
        let plan = ReportTable {
            table_id: "T-plan".to_string(),
            kind: "execution_plans".to_string(),
            category: "sql".to_string(),
            title: "Actionable execution-plan recommendations".to_string(),
            rows: vec![ReportTableRow {
                cells: BTreeMap::from([
                    ("project_id".to_string(), "node-1".to_string()),
                    ("sql_id".to_string(), "abc123".to_string()),
                    ("plan_hash".to_string(), "42".to_string()),
                    ("plan_status".to_string(), "analyzed".to_string()),
                    (
                        "selection_reason".to_string(),
                        "Observed high buffer-get demand makes this SQL a tuning priority."
                            .to_string(),
                    ),
                    (
                        "workload_evidence".to_string(),
                        "The SQL consumed 2.3 million buffer gets per execution.".to_string(),
                    ),
                    (
                        "temporal_evidence".to_string(),
                        "Observed across 120 snapshots including business-hour peaks.".to_string(),
                    ),
                    (
                        "comparative_context".to_string(),
                        "The candidate averaged 16 percent more buffer gets than baseline."
                            .to_string(),
                    ),
                    (
                        "key_operations".to_string(),
                        "TABLE ACCESS FULL".to_string(),
                    ),
                    ("access_and_joins".to_string(), "Full scan".to_string()),
                    ("cardinality".to_string(), "Estimated rows only".to_string()),
                    ("partition_parallelism".to_string(), "Serial".to_string()),
                    ("risk".to_string(), "Excessive scan candidate".to_string()),
                    ("recommendation_type".to_string(), "indexing".to_string()),
                    (
                        "recommendation_rationale".to_string(),
                        "The selective predicate has no supporting access path.".to_string(),
                    ),
                    (
                        "scope_limitations".to_string(),
                        "Estimated rows do not prove the runtime row count or final benefit."
                            .to_string(),
                    ),
                    (
                        "action".to_string(),
                        "Test a workload-specific index in a controlled window.".to_string(),
                    ),
                    (
                        "success_metric".to_string(),
                        "Reduce buffer gets per execution without regressing elapsed time."
                            .to_string(),
                    ),
                ]),
                evidence_refs: vec!["E-plan".to_string()],
            }],
        };
        let plan_rendered =
            render_report_table(&plan, &state, &labels, &ReportEntityLinks::default());
        assert!(plan_rendered.contains("class=\"plan-review\" data-plan-review"));
        assert!(plan_rendered.contains("data-plan-filter"));
        assert!(plan_rendered.contains("TABLE ACCESS FULL"));
        assert!(plan_rendered.contains("full table scan"));
        assert!(plan_rendered.contains("Why this SQL is here"));
        assert!(plan_rendered.contains("2.3 million buffer gets"));
        assert!(!plan_rendered.contains("| SQL ID |"));
    }

    #[test]
    fn markdown_merges_same_kind_tables_into_one_semantic_toc_heading() {
        let mut state = AnalysisSession::new(
            json!({}),
            ReportConfig::default(),
            vec!["node-1".to_string()],
        );
        for (index, parameter) in ["cpu_count", "open_cursors"].into_iter().enumerate() {
            state.report_tables.insert(
                format!("T-{index}"),
                ReportTable {
                    table_id: format!("T-{index}"),
                    kind: "parameters".to_string(),
                    category: "parameters".to_string(),
                    title: format!("Parameter table {index}"),
                    rows: vec![ReportTableRow {
                        cells: BTreeMap::from([
                            ("project_id".to_string(), "node-1".to_string()),
                            ("parameter".to_string(), parameter.to_string()),
                            ("observed_value".to_string(), "20".to_string()),
                            ("rating".to_string(), "concern".to_string()),
                            (
                                "performance_relevance".to_string(),
                                "Controls optimizer or cursor resource assumptions.".to_string(),
                            ),
                            (
                                "finding".to_string(),
                                "Requires workload-scoped governance review.".to_string(),
                            ),
                            (
                                "action".to_string(),
                                "Measure usage before changing the observed value.".to_string(),
                            ),
                        ]),
                        evidence_refs: vec![SEED_EVIDENCE_ID.to_string()],
                    }],
                },
            );
        }
        let document = json!({
            "analysis_id": "A-test",
            "revision": 1,
            "datasets": [],
        });
        let markdown = render_markdown(&document, &state, &ReportEntityLinks::default());
        assert_eq!(
            markdown
                .matches("### Performance parameters requiring attention")
                .count(),
            1
        );
        assert!(markdown.contains("cpu_count"));
        assert!(markdown.contains("open_cursors"));
    }

    #[test]
    fn stable_report_rejects_unresolved_navigation_placeholders() {
        let mut markdown = String::from("# Oracle Performance Analysis\n\n");
        for heading in STABLE_MARKDOWN_HEADINGS {
            markdown.push_str(heading);
            markdown.push_str("\n\nBody.\n\n");
        }
        markdown.push_str("{load_profile}\n");

        let error = validate_stable_markdown_report(&markdown).unwrap_err();
        assert_eq!(error["error_code"], "UNRESOLVED_REPORT_PLACEHOLDER");
    }

    #[test]
    fn applied_guidance_requires_a_verbatim_quote() {
        let mut state = AnalysisSession::new(json!({}), ReportConfig::default(), Vec::new());
        state.guidance.insert(
            "GUIDE-§1.2".to_string(),
            GuidanceRecord {
                title: "Cursor diagnostics".to_string(),
                text: "Inspect V$SQL_SHARED_CURSOR before diagnosing child cursor causes."
                    .to_string(),
            },
        );
        let references = vec!["GUIDE-§1.2".to_string()];
        let valid = json!([{
            "guidance_ref": "GUIDE-§1.2",
            "quote": "Inspect V$SQL_SHARED_CURSOR"
        }]);
        assert!(parse_guidance_quotations(Some(&valid), &references, &state).is_ok());

        let invented = json!([{
            "guidance_ref": "GUIDE-§1.2",
            "quote": "Increase every hidden cursor parameter"
        }]);
        let error = parse_guidance_quotations(Some(&invented), &references, &state).unwrap_err();
        assert_eq!(error["error_code"], "GUIDANCE_QUOTE_NOT_VERBATIM");
    }

    #[test]
    fn complete_report_renders_every_stable_section() {
        let runtime = runtime();
        let bootstrap = runtime
            .call_tool("start_performance_analysis", Map::new())
            .unwrap();
        let analysis_id = bootstrap["analysis_id"].as_str().unwrap().to_string();

        runtime
            .call_tool(
                "get_database_load_summary",
                Map::from_iter([("analysis_id".to_string(), json!(analysis_id))]),
            )
            .unwrap();
        for &section in REQUIRED_PRECOMPUTED_SECTIONS {
            runtime
                .call_tool(
                    "get_precomputed_analysis",
                    Map::from_iter([
                        ("analysis_id".to_string(), json!(analysis_id)),
                        ("section".to_string(), json!(section)),
                    ]),
                )
                .unwrap();
        }
        runtime
            .call_tool(
                "get_init_parameter",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("names".to_string(), json!(PERFORMANCE_PARAMETER_CHECKLIST)),
                ]),
            )
            .unwrap();

        for (index, category) in REQUIRED_REPORT_CATEGORIES.iter().enumerate() {
            let recommendations = if index == 0 {
                json!([{
                    "owner": "DBA",
                    "priority": "high",
                    "action": "Validate the change in a controlled window.",
                    "rationale": "The measured finding requires a controlled intervention before production rollout.",
                    "success_criterion": "The target metric improves against the aligned baseline with no regression in waits or errors."
                }])
            } else {
                json!([])
            };
            runtime
                .call_tool(
                    "record_finding",
                    Map::from_iter([
                        ("analysis_id".to_string(), json!(analysis_id)),
                        ("category".to_string(), json!(category)),
                        ("title".to_string(), json!(format!("{category} finding"))),
                        ("severity".to_string(), json!("medium")),
                        ("confidence".to_string(), json!("high")),
                        (
                            "conclusion".to_string(),
                            json!("Verified by the initial statistical seed."),
                        ),
                        (
                            "mechanism".to_string(),
                            json!("The seed measurement identifies the signal path that explains this test finding."),
                        ),
                        (
                            "temporal_pattern".to_string(),
                            json!("The finding is scoped to the single aligned snapshot interval in the test fixture."),
                        ),
                        (
                            "affected_workload".to_string(),
                            json!("The selected test project is the explicitly named workload scope for this finding."),
                        ),
                        (
                            "evidence_limitations".to_string(),
                            json!("The synthetic seed proves renderer coverage only and does not establish production causality."),
                        ),
                        (
                            "evidence_summary".to_string(),
                            json!("The initial seed contains the exact project scope and statistical values used by this test finding."),
                        ),
                        ("evidence_refs".to_string(), json!([SEED_EVIDENCE_ID])),
                        ("recommendations".to_string(), recommendations),
                    ]),
                )
                .unwrap();
        }
        for assessment in REQUIRED_ASSESSMENTS {
            runtime
                .call_tool(
                    "set_report_assessment",
                    Map::from_iter([
                        ("analysis_id".to_string(), json!(analysis_id)),
                        ("assessment".to_string(), json!(assessment)),
                        ("status".to_string(), json!("unknown")),
                        (
                            "conclusion".to_string(),
                            json!(
                                "The available evidence is insufficient for a stronger conclusion."
                            ),
                        ),
                        (
                            "evidence_summary".to_string(),
                            json!("No direct measurement capable of proving this assessment was collected."),
                        ),
                        ("evidence_refs".to_string(), json!([])),
                    ]),
                )
                .unwrap();
        }
        runtime
            .call_tool(
                "configure_report",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("output_format".to_string(), json!("both")),
                ]),
            )
            .unwrap();
        let status = runtime
            .call_tool(
                "get_report_status",
                Map::from_iter([("analysis_id".to_string(), json!(analysis_id))]),
            )
            .unwrap();
        assert_eq!(status["ready_to_finalize"], true);

        let final_report = runtime
            .call_tool(
                "finalize_report",
                Map::from_iter([("analysis_id".to_string(), json!(analysis_id))]),
            )
            .unwrap();
        let markdown = final_report["markdown"].as_str().unwrap();
        for section in 1..=11 {
            assert!(markdown.contains(&format!("## {section}.")));
        }
        assert!(markdown.contains("**Evidence basis:**"));
        assert!(markdown.contains("**Mechanism:**"));
        assert!(markdown.contains("**Diagnostic mechanism:**"));
        assert!(markdown.contains("**Affected workload:**"));
        assert!(markdown.contains("**Temporal pattern:**"));
        assert!(markdown.contains("**Evidence boundary and counterevidence:**"));
        assert!(markdown.contains("### DBA Actions"));
        assert!(markdown.contains("**Why:**"));
        assert!(markdown.contains("**Success criterion:**"));
        assert!(markdown.contains("## 9. Gradient and Anomaly Synthesis"));
        assert!(markdown.contains("## 5. Segments and Objects"));
        assert!(markdown.contains("## 10. Relevant Initialization Parameters"));
        assert!(!markdown.contains("No evidence-backed findings were recorded for this section."));
        assert!(!markdown.contains("`SEED-E0001`"));
        assert!(!markdown.contains("## Appendix A."));
        assert!(!markdown.contains("## Appendix B."));
        assert_eq!(
            final_report["report"]["section_index"]
                .as_array()
                .unwrap()
                .len(),
            11
        );
        assert_eq!(final_report["draft"], false);

        let incomplete = runtime
            .call_tool(
                "convert_markdown_to_html",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    (
                        "markdown".to_string(),
                        json!("# Oracle Performance Analysis\n\n## 1. Executive Summary\n"),
                    ),
                ]),
            )
            .unwrap_err();
        assert_eq!(incomplete["error_code"], "MISSING_REPORT_SECTIONS");

        let edited_markdown = markdown.replacen(
            "Verified by the initial statistical seed.",
            "Edited after finalization.",
            1,
        );
        let edited = runtime
            .call_tool(
                "convert_markdown_to_html",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("markdown".to_string(), json!(edited_markdown)),
                ]),
            )
            .unwrap_err();
        assert_eq!(edited["error_code"], "MARKDOWN_NOT_FINALIZED");

        let unsafe_name = runtime
            .convert_markdown_to_html_in_directory(
                &Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("markdown".to_string(), json!(markdown)),
                    ("output_filename".to_string(), json!("../report.html")),
                ]),
                &std::env::temp_dir(),
            )
            .unwrap_err();
        assert_eq!(unsafe_name["error_code"], "INVALID_OUTPUT_FILENAME");

        let test_directory = std::env::temp_dir().join(format!(
            "jas-min-mcp-html-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir(&test_directory).unwrap();
        let linked_report_directory = test_directory.join("nonexistent-test-dataset.html_reports");
        std::fs::create_dir(&linked_report_directory).unwrap();
        let html_result = runtime
            .convert_markdown_to_html_in_directory(
                &Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("markdown".to_string(), json!(markdown)),
                    ("output_filename".to_string(), json!("MCP-report.HTML")),
                ]),
                &test_directory,
            )
            .unwrap();
        assert_eq!(html_result["output_filename"], "MCP-report.html");
        assert_eq!(html_result["report_structure_validated"], true);
        assert_eq!(html_result["linked_report_directory_present"], true);
        assert_eq!(html_result["opened_automatically"], false);
        let output_path = test_directory.join("MCP-report.html");
        let html = std::fs::read_to_string(&output_path).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<title>JAS-MIN Oracle Performance Analysis</title>"));
        assert!(html.contains("Report contents"));
        assert!(html.contains("class=\"report-title\""));
        assert!(html.contains("class=\"section-title\""));
        assert!(html.contains("Overall Performance Profile and DB Time Degradation"));

        let duplicate = runtime
            .convert_markdown_to_html_in_directory(
                &Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("markdown".to_string(), json!(markdown)),
                    ("output_filename".to_string(), json!("MCP-report.html")),
                ]),
                &test_directory,
            )
            .unwrap_err();
        assert_eq!(duplicate["error_code"], "OUTPUT_EXISTS");
        std::fs::remove_file(output_path).unwrap();
        std::fs::remove_dir(linked_report_directory).unwrap();
        std::fs::remove_dir(test_directory).unwrap();
    }

    #[test]
    fn alert_attachment_inventory_distinguishes_empty_and_partial_coverage() {
        let test_directory = std::env::temp_dir().join(format!(
            "jas-min-mcp-alert-inventory-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir(&test_directory).unwrap();
        let empty = test_directory.join("alert_NODE2.log");
        let populated = test_directory.join("alert_NODE1.log");
        std::fs::write(&empty, []).unwrap();
        std::fs::write(
            &populated,
            b"header\n2026-07-29T00:04:51.035659+02:00\nORA-00918\n2026-08-02T22:59:34.346607+02:00\n",
        )
        .unwrap();

        let matching = files_name_contains(&test_directory, "alert");
        assert_eq!(matching.len(), 2);
        assert_eq!(std::fs::metadata(&empty).unwrap().len(), 0);
        assert!(std::fs::metadata(&populated).unwrap().len() > 0);
        let (first, last, count) = alert_timestamp_bounds(&populated);
        assert_eq!(first.as_deref(), Some("2026-07-29T00:04:51.035659+02:00"));
        assert_eq!(last.as_deref(), Some("2026-08-02T22:59:34.346607+02:00"));
        assert_eq!(count, 2);
        assert_eq!(
            alert_coverage_status(
                std::fs::metadata(&populated).unwrap().len(),
                first.as_deref().and_then(|value| value.get(..10)),
                last.as_deref().and_then(|value| value.get(..10)),
                Some("2026-07-20"),
                Some("2026-08-02"),
            ),
            "partial_relative_to_dataset"
        );
        assert_eq!(
            alert_coverage_status(0, None, None, Some("2026-07-20"), Some("2026-08-02")),
            "missing"
        );
        assert_eq!(
            alert_coverage_status(10, None, None, Some("2026-07-20"), Some("2026-08-02")),
            "unknown"
        );

        std::fs::remove_file(empty).unwrap();
        std::fs::remove_file(populated).unwrap();
        std::fs::remove_dir(test_directory).unwrap();
    }

    #[test]
    fn report_gate_requires_alert_summary_rows_and_parse_error_finding() {
        let test_directory = std::env::temp_dir().join(format!(
            "jas-min-mcp-alert-coverage-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let stem = test_directory.join("alert-coverage");
        let attachments = PathBuf::from(format!("{}_attachments", stem.display()));
        std::fs::create_dir_all(&attachments).unwrap();
        std::fs::write(
            attachments.join("alert_TEST.log"),
            b"2026-08-01T10:00:00.000000+02:00\nWARNING: too many parse errors, count=101 SQL hash=0xabc\nPARSE ERROR: ospid=123, error=904\nsqlid=abc123\n...Current username=APPS\n...Application: JDBC Thin Client Action: query\n",
        )
        .unwrap();
        let runtime = AnalysisRuntime::new(
            AWRSCollection {
                db_instance_information: DBInstance::default(),
                initialization_parameters: HashMap::new(),
                awrs: Vec::new(),
                sql_text: HashMap::new(),
            },
            ReportForAI::default(),
            stem.to_string_lossy().to_string(),
            2,
            HashMap::new(),
            test_directory
                .join("alert-coverage.html_reports")
                .to_string_lossy()
                .to_string(),
        );
        let bootstrap = runtime
            .call_tool("start_performance_analysis", Map::new())
            .unwrap();
        let analysis_id = bootstrap["analysis_id"].as_str().unwrap();
        let project_id = bootstrap["project_ids"][0].as_str().unwrap();
        let initial = runtime
            .call_tool(
                "get_report_status",
                Map::from_iter([("analysis_id".to_string(), json!(analysis_id))]),
            )
            .unwrap();
        assert!(initial["missing_required_evidence"]
            .as_array()
            .unwrap()
            .contains(&json!(format!(
                "get_alertlog_errors:{project_id}:include_parse_error_details"
            ))));
        assert!(initial["missing_structured_table_kinds"]
            .as_array()
            .unwrap()
            .contains(&json!("alert_log_errors")));
        assert!(initial["missing_structured_table_rows"]
            .as_array()
            .unwrap()
            .contains(&json!(format!(
                "alert_log_errors:{project_id}:WARNING_TOO_MANY_PARSE_ERRORS"
            ))));

        let evidence = runtime
            .call_tool(
                "get_alertlog_errors",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("include_parse_error_details".to_string(), json!(true)),
                    ("limit".to_string(), json!(1000)),
                ]),
            )
            .unwrap();
        let evidence_id = evidence["evidence_id"].as_str().unwrap();
        let rows = evidence["result"]["error_summary"]
            .as_array()
            .unwrap()
            .iter()
            .map(|summary| {
                let sql_ids = summary["sql_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                let clients = summary["applications"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                json!({
                    "project_id": project_id,
                    "error_code": summary["code"],
                    "event_records": summary["event_records"].as_u64().unwrap().to_string(),
                    "parse_detail_records": summary["parse_detail_records"].as_u64().unwrap().to_string(),
                    "max_reported_count": summary["max_reported_count"].as_u64().unwrap().to_string(),
                    "first_seen": summary["first_seen"],
                    "last_seen": summary["last_seen"],
                    "affected_sql_ids": if sql_ids.is_empty() { "none identified" } else { &sql_ids },
                    "affected_clients": if clients.is_empty() { "none identified" } else { &clients },
                    "performance_relevance": "Repeated hard-parse failures can consume shared-pool and CPU resources.",
                    "action": "Correct the invalid SQL and correlate the SQL_ID with parse CPU and child-cursor evidence.",
                    "evidence_refs": [evidence_id]
                })
            })
            .collect::<Vec<_>>();
        runtime
            .call_tool(
                "record_report_table",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("kind".to_string(), json!("alert_log_errors")),
                    (
                        "title".to_string(),
                        json!("Alert-log errors requiring attention"),
                    ),
                    ("rows".to_string(), json!(rows)),
                ]),
            )
            .unwrap();
        runtime
            .call_tool(
                "record_finding",
                Map::from_iter([
                    ("analysis_id".to_string(), json!(analysis_id)),
                    ("category".to_string(), json!("sql")),
                    ("title".to_string(), json!("Repeated SQL parse failures")),
                    ("severity".to_string(), json!("high")),
                    ("confidence".to_string(), json!("high")),
                    (
                        "conclusion".to_string(),
                        json!("Oracle recorded repeated parse failures for SQL_ID abc123."),
                    ),
                    (
                        "mechanism".to_string(),
                        json!("Repeated invalid parses create avoidable parse and shared-pool work for the identified SQL."),
                    ),
                    (
                        "temporal_pattern".to_string(),
                        json!("The parse warning and detail block occurred in the attachment at 2026-08-01 10:00:00."),
                    ),
                    (
                        "affected_workload".to_string(),
                        json!("SQL_ID abc123 and its originating client are the directly affected workload path."),
                    ),
                    (
                        "evidence_limitations".to_string(),
                        json!("The alert attachment proves failed parsing but not its share of every cursor-mutex peak."),
                    ),
                    (
                        "evidence_summary".to_string(),
                        json!("The alert attachment contains a too-many-parse-errors warning and an ORA-00904 parse detail block at 2026-08-01 10:00:00."),
                    ),
                    ("evidence_refs".to_string(), json!([evidence_id])),
                ]),
            )
            .unwrap();
        let updated = runtime
            .call_tool(
                "get_report_status",
                Map::from_iter([("analysis_id".to_string(), json!(analysis_id))]),
            )
            .unwrap();
        assert!(!updated["missing_required_evidence"]
            .as_array()
            .unwrap()
            .contains(&json!(format!(
                "get_alertlog_errors:{project_id}:include_parse_error_details"
            ))));
        assert!(!updated["missing_required_evidence"]
            .as_array()
            .unwrap()
            .contains(&json!(format!(
                "sql_finding_with_parse_error_evidence:{project_id}"
            ))));
        assert!(!updated["missing_structured_table_rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row
                .as_str()
                .is_some_and(|row| row.starts_with("alert_log_errors:"))));

        std::fs::remove_dir_all(test_directory).unwrap();
    }
}
