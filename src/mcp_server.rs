//! Streamable HTTP MCP server for interactive JAS-MIN investigations.
//!
//! The server deliberately keeps Oracle measurements, diagnostic guidance and
//! report state separate. Measurements receive evidence IDs, guidance receives
//! methodology references, and the report builder accepts only references that
//! were observed in the same explicit analysis session.

use crate::ai_tools::{dispatch_tool_call_value, tools_schema};
use crate::awr::AWRSCollection;
use crate::local_agent::{build_case_seed, dispatch_precomputed_analysis, GuidanceLibrary};
use crate::reasonings::ReportForAI;
use crate::tools::render_markdown_html_document;
use anyhow::{bail, Context, Result};
use dashmap::DashMap;
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

const MCP_ANALYSIS_SCHEMA_VERSION: &str = "2026-08-22.5";
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
    "io",
    "parameters",
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
}

#[derive(Debug, Clone, Serialize)]
struct ReportFinding {
    finding_id: String,
    category: String,
    title: String,
    severity: String,
    confidence: String,
    conclusion: String,
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

struct AnalysisSession {
    project_ids: Vec<String>,
    config: ReportConfig,
    evidence: BTreeMap<String, EvidenceRecord>,
    evidence_cache: HashMap<String, String>,
    guidance: BTreeMap<String, GuidanceRecord>,
    findings: BTreeMap<String, ReportFinding>,
    assessments: BTreeMap<String, ReportAssessment>,
    next_evidence: u64,
    next_finding: u64,
    report_revision: u64,
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
            next_evidence: 2,
            next_finding: 1,
            report_revision: 0,
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
        let evidence_summary = required_string(arguments, "evidence_summary", 4_000)?;
        let details = optional_string(arguments, "details", 16_000);
        let evidence_refs = string_array(arguments, "evidence_refs", 32, 32)?;
        let guidance_refs = string_array(arguments, "guidance_refs", 16, 64)?;
        let recommendations = parse_recommendations(arguments.get("recommendations"))?;

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
                evidence_summary,
                details,
                evidence_refs,
                guidance_refs,
                guidance_quotes,
                recommendations,
            },
        );
        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "finding_id": finding_id,
            "findings_total": state.findings.len(),
            "message": "Evidence-backed finding stored. Reuse finding_id to replace it after deeper investigation."
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
        Ok(report_status_value(&analysis_id, &state))
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
        let status = report_status_value(&analysis_id, &state);
        if !allow_incomplete && status.get("ready_to_finalize") != Some(&Value::Bool(true)) {
            return Err(json!({
                "error_code": "REPORT_INCOMPLETE",
                "message": "The report contract is incomplete. Add the missing categories/assessments or explicitly request allow_incomplete=true for a draft.",
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
            "mandatory_assessments": state.assessments,
            "coverage": status
        });
        let markdown = render_markdown(&report_document, &state);
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
        let project_ids = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?
            .project_ids
            .clone();
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
        let report_links = single_project
            .as_ref()
            .map(|project| {
                project
                    .report_links
                    .iter()
                    .map(|(kind, names)| (kind.as_str(), names.clone()))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let html = render_markdown_html_document(
            markdown,
            report_directory_reference,
            &report_directory.to_string_lossy(),
            report_links,
        );
        validate_resolved_html_navigation(&html)?;

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
            "This server has {} loaded performance project(s). Call list_performance_projects first when more than one project is available, then call start_performance_analysis with the intended project_ids. Pass analysis_id to every later tool and project_id to project-specific evidence calls in comparative sessions. Use compare_project_metric and compare_project_sql for normalized cross-project evidence. Use narrow evidence calls and compare peaks with quiet baselines. Diagnostic guidance is methodology, never observed evidence. On AIX, obtain entitlement evidence before a CPU-pressure conclusion. Distinguish latency from workload volume, correlation from causation, and unknown from absent. Store findings with evidence_refs plus a reader-facing evidence_summary containing exact values. In comparative prose, label every project or instance value explicitly; never use an unlabeled X/Y shorthand. Treat a zero-byte attachment as missing coverage, never as a searched-and-clean source or a reader-facing evidence link. Use each alert attachment's observed first/last timestamp instead of assuming it covers the enclosing AWR period. A zero-match literal proves only that exact search/filter; inspect raw context and punctuation/message variants before declaring an event absent. If guidance is applied, include a verbatim guidance quote; the server verifies it against the retrieved text. Complete all mandatory assessments, check get_report_status, and finish through finalize_report. When the user requests HTML, configure Markdown output, finalize the stable Markdown report first, then pass that exact Markdown to convert_markdown_to_html. Comparative HTML must expose active source-report links for every selected project. In reader-facing findings, link each material wait-event name and SQL_ID directly to every existing project-specific detail report, with explicit instance/project labels when more than one target exists; generic labels such as 'instance 1' alone are insufficient.",
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
        let supports_project_id = name == "get_precomputed_analysis";
        let read_only = matches!(
            name,
            "list_performance_projects"
                | "get_analysis_catalog"
                | "get_precomputed_analysis"
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
            "Creates or replaces one evidence-backed report finding. The human-readable evidence_summary must state the exact supporting values. Evidence and guidance references must have been obtained in this analysis session; every applied guidance reference requires a verified verbatim quotation.",
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
                    "evidence_summary": {"type": "string", "description": "Human-readable evidence basis with exact values, time scope, and project/instance context; never just evidence IDs or tool names."},
                    "details": {"type": "string"},
                    "evidence_refs": {"type": "array", "items": {"type": "string"}},
                    "guidance_refs": {"type": "array", "items": {"type": "string"}},
                    "guidance_quotes": {"type": "array", "items": {"type": "object", "additionalProperties": false, "properties": {"guidance_ref": {"type": "string"}, "quote": {"type": "string", "description": "Contiguous verbatim excerpt from the retrieved guidance section."}}, "required": ["guidance_ref", "quote"]}},
                    "recommendations": {"type": "array", "items": {"type": "object", "additionalProperties": false, "properties": {"owner": {"type": "string", "enum": ["DBA", "Developer", "Management"]}, "priority": {"type": "string", "enum": ["immediate", "high", "medium", "low"]}, "action": {"type": "string"}}, "required": ["owner", "priority", "action"]}}
                },
                "required": ["category", "title", "severity", "confidence", "conclusion", "evidence_summary", "evidence_refs"]
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
            "Checks stable-section coverage, mandatory assessments, actions, evidence and guidance before final report generation.",
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
            "Validates a complete 11-section JAS-MIN Markdown report, applies the mandatory responsive audit presentation shared with classic AI mode, and creates a new HTML file directly in the JAS-MIN working directory. The presentation includes sticky navigation, severity markers, finding/action cards, styled tables and plans, mobile reflow, and print CSS. Finalize Markdown first and pass it unchanged. Existing files are never overwritten and the server does not open a browser.",
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
        {"gate": "sql_tuning", "required": true, "rule": "Inspect SQL text, timeline and available plans before concrete SQL tuning recommendations."},
        {"gate": "cursor_contention", "required": true, "rule": "Use child-cursor reasons and parse/reload/invalidation evidence before explaining cursor proliferation or mutex contention."},
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
        json!({"tool": "get_precomputed_analysis", "arguments": {"section": "db_time_degradation"}, "reason": "establish whether the recent window degraded"}),
        json!({"tool": "get_precomputed_analysis", "arguments": {"section": "full_gradients"}, "reason": "rank competing contributors and inspect collinearity"}),
        json!({"tool": "list_snapshots", "reason": "select peaks and quiet baselines"}),
    ];
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
        calls.push(json!({"tool": "get_alertlog_errors", "reason": "correlate Oracle errors and incidents with snapshot evidence"}));
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
        "required_assessments": REQUIRED_ASSESSMENTS,
        "human_citation_policy": {
            "evidence_summary_required": true,
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
            "presentation": "The shared renderer always applies the responsive ORA-600-aligned audit layout: a self-contained vector JAS-MIN wordmark, white/black/red palette, sticky report navigation, severity-marked findings derived from [severity / confidence], card treatment for executive findings and actions, styled evidence tables and plans, accessible focus states, and a print layout."
        }
    })
}

fn report_status_value(analysis_id: &str, state: &AnalysisSession) -> Value {
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
    let ready = missing_categories.is_empty()
        && missing_assessments.is_empty()
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
        "guidance_refs": state.guidance.len(),
        "recommendation_actions": actions,
        "present_categories": present_categories,
        "missing_required_categories": missing_categories,
        "completed_assessments": state.assessments.keys().collect::<Vec<_>>(),
        "missing_assessments": missing_assessments,
        "next_step": if ready { "Call finalize_report." } else { "Collect missing evidence, store findings and complete mandatory assessments." }
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

fn render_markdown(document: &Value, state: &AnalysisSession) -> String {
    let mut output = String::new();
    output.push_str("# Oracle Performance Analysis\n\n");
    output.push_str(&format!(
        "Analysis `{}` · revision {} · language `{}` · audience `{}`\n\n",
        document["analysis_id"].as_str().unwrap_or("unknown"),
        document["revision"].as_u64().unwrap_or(0),
        state.config.language,
        state.config.audience
    ));
    output.push_str(&format!(
        "Projects: {}\n\n",
        state
            .project_ids
            .iter()
            .map(|project_id| format!("`{project_id}`"))
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
        for finding in leading {
            output.push_str(&format!(
                "- **{}** [{} / {}]: {}\n",
                finding.title, finding.severity, finding.confidence, finding.conclusion
            ));
        }
        output.push('\n');
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
        let findings = state
            .findings
            .values()
            .filter(|finding| finding.category == category)
            .collect::<Vec<_>>();
        if findings.is_empty() {
            output.push_str("No evidence-backed findings were recorded for this section.\n\n");
            continue;
        }
        let detail = state
            .config
            .detail_overrides
            .get(category)
            .map(String::as_str)
            .unwrap_or(&state.config.detail_level);
        for finding in findings {
            output.push_str(&format!(
                "### {} [{} / {}]\n\n{}\n\n**Evidence basis:** {}\n\n",
                finding.title,
                finding.severity,
                finding.confidence,
                finding.conclusion,
                finding.evidence_summary
            ));
            output.push_str(&render_guidance_quotes(&finding.guidance_quotes, state));
            if state.config.include_evidence_appendix && !finding.evidence_refs.is_empty() {
                output.push_str(&format!(
                    "**Technical provenance:** {}\n\n",
                    evidence_links(&finding.evidence_refs)
                ));
            }
            if detail != "compact" && !finding.details.is_empty() {
                output.push_str(&finding.details);
                output.push_str("\n\n");
            }
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
    actions.sort_by_key(|(_, action)| priority_rank(&action.priority));
    if actions.is_empty() {
        output.push_str("No prioritized actions were recorded.\n\n");
    } else {
        for (finding, action) in actions {
            output.push_str(&format!(
                "- **{} / {}**: {} _(from {})_\n",
                action.owner, action.priority, action.action, finding.finding_id
            ));
        }
        output.push('\n');
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
        for evidence_id in cited {
            let Some(record) = state.evidence.get(&evidence_id) else {
                continue;
            };
            output.push_str(&format!(
                "<a id=\"evidence-{}\"></a>- **{} — {}**{}\n",
                evidence_anchor(&record.evidence_id),
                record.evidence_id,
                humanize_identifier(&record.tool_name),
                evidence_scope(record)
            ));
        }
        output.push('\n');
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

fn render_source_report_links(document: &Value) -> String {
    let Some(datasets) = document.get("datasets").and_then(Value::as_array) else {
        return String::new();
    };
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
            entries.push(format!("- **{project_id}:** {}", links.join(" · ")));
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
    if !lines
        .iter()
        .any(|line| *line == "# Oracle Performance Analysis")
    {
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
            Ok(Recommendation {
                owner,
                priority,
                action,
            })
        })
        .collect()
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
    use crate::reasonings::{MadAnomaliesEvents, TopForegroundWaitEvents};

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
                "evidence_summary".to_string(),
                json!("The cited source would need to contain the measured latency."),
            ),
            ("evidence_refs".to_string(), json!(["E-9999"])),
        ]);
        let error = runtime.call_tool("record_finding", finding).unwrap_err();
        assert_eq!(error["error_code"], "UNKNOWN_EVIDENCE_REF");
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

        for (index, category) in REQUIRED_REPORT_CATEGORIES.iter().enumerate() {
            let recommendations = if index == 0 {
                json!([{"owner": "DBA", "priority": "high", "action": "Validate the change in a controlled window."}])
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
}
