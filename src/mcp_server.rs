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
use anyhow::{bail, Context, Result};
use dashmap::DashMap;
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, GetPromptRequestParams,
        GetPromptResponse, GetPromptResult, Implementation, ListPromptsResult, ListToolsResult,
        Prompt, PromptArgument, PromptMessage, Role, ServerCapabilities, ServerInfo, Tool,
        ToolAnnotations,
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
    future::Future,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio_util::sync::CancellationToken;

const MCP_ANALYSIS_SCHEMA_VERSION: &str = "2026-08-03.1";
const SEED_EVIDENCE_ID: &str = "SEED-E0001";
const DEFAULT_GUIDANCE_LIMIT_CHARS: usize = 8 * 1024;

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
    arguments: Value,
    result: Value,
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
            include_evidence_appendix: true,
            include_guidance_appendix: true,
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
    details: String,
    evidence_refs: Vec<String>,
    guidance_refs: Vec<String>,
    recommendations: Vec<Recommendation>,
}

#[derive(Debug, Clone, Serialize)]
struct ReportAssessment {
    assessment: String,
    status: String,
    conclusion: String,
    evidence_refs: Vec<String>,
    guidance_refs: Vec<String>,
}

struct AnalysisSession {
    config: ReportConfig,
    evidence: BTreeMap<String, EvidenceRecord>,
    evidence_cache: HashMap<String, String>,
    guidance_refs: BTreeSet<String>,
    findings: BTreeMap<String, ReportFinding>,
    assessments: BTreeMap<String, ReportAssessment>,
    next_evidence: u64,
    next_finding: u64,
    report_revision: u64,
}

impl AnalysisSession {
    fn new(seed: Value, config: ReportConfig) -> Self {
        let seed_record = EvidenceRecord {
            evidence_id: SEED_EVIDENCE_ID.to_string(),
            tool_name: "initial_case_seed".to_string(),
            arguments: json!({}),
            result: seed,
        };
        Self {
            config,
            evidence: BTreeMap::from([(SEED_EVIDENCE_ID.to_string(), seed_record)]),
            evidence_cache: HashMap::new(),
            guidance_refs: BTreeSet::new(),
            findings: BTreeMap::new(),
            assessments: BTreeMap::new(),
            next_evidence: 2,
            next_finding: 1,
            report_revision: 0,
        }
    }
}

/// Immutable parsed data plus explicitly keyed conversational report sessions.
#[derive(Clone)]
pub struct AnalysisRuntime {
    collection: Arc<AWRSCollection>,
    report: Arc<ReportForAI>,
    stem: Arc<String>,
    security_level: usize,
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
    ) -> Self {
        Self {
            collection: Arc::new(collection),
            report: Arc::new(report),
            stem: Arc::new(stem),
            security_level,
            guidance: Arc::new(GuidanceLibrary::load()),
            sessions: Arc::new(DashMap::new()),
            sequence: Arc::new(AtomicU64::new(1)),
        }
    }

    fn new_analysis(&self, arguments: &Value) -> Value {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let analysis_id = format!(
            "A-{}-{sequence:04}",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        );
        let seed = mcp_bootstrap_seed(&self.report);
        let mut config = ReportConfig::default();
        if let Some(language) = arguments.get("language").and_then(Value::as_str) {
            config.language = bounded_string(language, 16);
        }
        if let Some(audience) = arguments.get("audience").and_then(Value::as_str) {
            if matches!(audience, "technical" | "management" | "mixed") {
                config.audience = audience.to_string();
            }
        }
        let session = AnalysisSession::new(seed.clone(), config.clone());
        self.sessions
            .insert(analysis_id.clone(), Arc::new(Mutex::new(session)));
        let dataset_manifest = self.dataset_manifest();
        let attachments = dataset_manifest
            .get("attachments")
            .cloned()
            .unwrap_or(Value::Null);
        let recommended_calls = recommended_next_calls(
            &self.collection.db_instance_information.platform,
            &attachments,
            dataset_manifest.get("date_from").and_then(Value::as_str),
            dataset_manifest.get("date_to").and_then(Value::as_str),
        );

        json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "seed_evidence_id": SEED_EVIDENCE_ID,
            "instruction": "Use this analysis_id in every subsequent JAS-MIN tool call. Build competing hypotheses, obtain narrow evidence, consult guidance only for detected symptoms, and finish through the report tools.",
            "focus": arguments.get("focus").cloned().unwrap_or(Value::Null),
            "dataset_manifest": dataset_manifest,
            "available_calculations": calculation_catalog(),
            "case_seed": seed,
            "triage_preview": mcp_triage_preview(&self.report),
            "diagnostic_guidance": self.guidance.catalog_json(),
            "quality_gates": quality_gates(&self.collection.db_instance_information.platform),
            "report_contract": report_contract(&config),
            "recommended_next_calls": recommended_calls
        })
    }

    fn dataset_manifest(&self) -> Value {
        let first = self.collection.awrs.first();
        let last = self.collection.awrs.last();
        let date_from = first.and_then(|awr| oracle_snapshot_date(&awr.snap_info.begin_snap_time));
        let date_to = last.and_then(|awr| oracle_snapshot_date(&awr.snap_info.end_snap_time));
        json!({
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
            "attachments": self.attachment_inventory()
        })
    }

    fn attachment_inventory(&self) -> Value {
        let directory = PathBuf::from(format!("{}_attachments", self.stem));
        let aix_directory = directory.join("AIX");
        json!({
            "directory_present": directory.is_dir(),
            "execution_plans": count_extension(&directory, "xplan"),
            "child_cursor_reason_files": count_suffix(&directory, ".shared_cursor_reasons"),
            "alert_logs": count_name_contains(&directory, "alert"),
            "aix_files": count_regular_files(&aix_directory),
            "aix_directory_present": aix_directory.is_dir()
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
        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "dataset_manifest": self.dataset_manifest(),
            "available_calculations": calculation_catalog(),
            "diagnostic_guidance": self.guidance.catalog_json(),
            "report_contract": report_contract(&state.config)
        }))
    }

    fn execute_evidence_tool(
        &self,
        name: &str,
        arguments: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let analysis_id = Self::analysis_id(arguments)?.to_string();
        let session = self.session(&analysis_id)?;
        let mut clean_arguments = arguments.clone();
        clean_arguments.remove("analysis_id");
        let clean_value = Value::Object(clean_arguments.clone());
        let result = if name == "get_precomputed_analysis" {
            dispatch_precomputed_analysis(&clean_value, &self.report)
        } else {
            dispatch_tool_call_value(name, &clean_value, &self.collection, self.stem.as_str())
        };
        if result.get("error").is_some() {
            return Err(result);
        }

        let cache_key = format!("{name}:{}", canonical_json(&clean_value));
        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        if let Some(existing_id) = state.evidence_cache.get(&cache_key).cloned() {
            if let Some(record) = state.evidence.get(&existing_id) {
                return Ok(json!({
                    "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
                    "analysis_id": analysis_id,
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
                arguments: clean_value,
                result: result.clone(),
            },
        );
        Ok(json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "evidence_id": evidence_id,
            "tool_name": name,
            "cached": false,
            "result": result
        }))
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
        if let Some(matches) = result.get_mut("matches").and_then(Value::as_array_mut) {
            for matched in matches {
                if let Some(section_id) = matched.get("section_id").and_then(Value::as_str) {
                    let reference = format!("GUIDE-{section_id}");
                    if let Some(object) = matched.as_object_mut() {
                        object.insert("guidance_ref".to_string(), json!(reference));
                    }
                    references.push(reference);
                }
            }
        }
        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        state.guidance_refs.extend(references.iter().cloned());
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
        let details = optional_string(arguments, "details", 16_000);
        let evidence_refs = string_array(arguments, "evidence_refs", 32, 32)?;
        let guidance_refs = string_array(arguments, "guidance_refs", 16, 64)?;
        let recommendations = parse_recommendations(arguments.get("recommendations"))?;

        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        validate_references(&state, &evidence_refs, &guidance_refs)?;
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
                details,
                evidence_refs,
                guidance_refs,
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
        let evidence_refs = string_array(arguments, "evidence_refs", 32, 32)?;
        let guidance_refs = string_array(arguments, "guidance_refs", 16, 64)?;
        let mut state = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "analysis session lock is poisoned"))?;
        validate_references(&state, &evidence_refs, &guidance_refs)?;
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
                evidence_refs,
                guidance_refs,
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
        let report_document = json!({
            "schema_version": MCP_ANALYSIS_SCHEMA_VERSION,
            "analysis_id": analysis_id,
            "revision": state.report_revision,
            "generated_at": chrono::Utc::now().to_rfc3339(),
            "dataset": self.dataset_manifest(),
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
        }
        if matches!(state.config.output_format.as_str(), "json" | "both") {
            output["report"] = report_document;
        }
        Ok(output)
    }

    fn call_tool(
        &self,
        name: &str,
        arguments: Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        match name {
            "start_performance_analysis" => self.new_analysis(&Value::Object(arguments)).pipe(Ok),
            "get_analysis_catalog" => self.catalog_for_session(&arguments),
            "get_precomputed_analysis" => self.execute_evidence_tool(name, &arguments),
            "get_diagnostic_guidance" => self.diagnostic_guidance(&arguments),
            "configure_report" => self.configure_report(&arguments),
            "record_finding" => self.record_finding(&arguments),
            "set_report_assessment" => self.set_assessment(&arguments),
            "get_report_status" => self.report_status(&arguments),
            "finalize_report" => self.finalize_report(&arguments),
            other => self.execute_evidence_tool(other, &arguments),
        }
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, function: impl FnOnce(Self) -> T) -> T {
        function(self)
    }
}

impl<T> Pipe for T {}

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
        .with_instructions(
            "Call start_performance_analysis before every investigation and pass its analysis_id to all later tools. Use narrow evidence calls and compare peaks with quiet baselines. Diagnostic guidance is methodology, never observed evidence. On AIX, obtain entitlement evidence before a CPU-pressure conclusion. Distinguish latency from workload volume, correlation from causation, and unknown from absent. Store findings with evidence_refs, complete all mandatory assessments, check get_report_status, and finish through finalize_report.",
        )
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(
            self.tools.as_ref().clone(),
        )))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|tool| tool.name == name).cloned()
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<CallToolResponse, McpError>> + Send + '_ {
        async move {
            let name = request.name.to_string();
            let arguments = request.arguments.unwrap_or_default();
            let result = match self.runtime.call_tool(&name, arguments) {
                Ok(value) => CallToolResult::structured(value),
                Err(value) => CallToolResult::structured_error(value),
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
                    "Investigate {focus} using the JAS-MIN MCP server. Begin with start_performance_analysis, then use the returned analysis_id for all evidence calls. Form competing hypotheses and falsify them with timelines, snapshots, SQL text, plans, child-cursor reasons, alert log and AIX evidence when available. Fetch reasonings.txt guidance only for concrete symptoms and never cite it as measurement evidence. Store evidence-backed findings, complete every mandatory assessment, validate report status and finalize the stable report. Write finding content in {language}."
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
    println!("✅ JAS-MIN MCP ready at {}", endpoint.url());
    println!("   Parsed collection and statistical analysis are retained in memory.");
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
    let mut tools = tools_schema(runtime.stem.as_str())
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|definition| openai_definition_to_mcp(definition, true, true))
        .collect::<Vec<_>>();
    tools.extend(mcp_control_definitions().iter().filter_map(|definition| {
        let name = definition.pointer("/function/name")?.as_str()?;
        let requires_analysis = name != "start_performance_analysis";
        let read_only = matches!(
            name,
            "get_analysis_catalog"
                | "get_precomputed_analysis"
                | "get_diagnostic_guidance"
                | "get_report_status"
        );
        openai_definition_to_mcp(definition, requires_analysis, read_only)
    }));
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

fn openai_definition_to_mcp(
    definition: &Value,
    requires_analysis_id: bool,
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
            "start_performance_analysis",
            "Mandatory first call. Creates an explicit analysis session and returns the statistical capability catalog, dataset manifest, compact high-signal seed, diagnostic quality gates and stable report contract.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
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
            "Creates or replaces one evidence-backed report finding. Evidence and guidance references must have been obtained in this analysis session.",
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
                    "details": {"type": "string"},
                    "evidence_refs": {"type": "array", "items": {"type": "string"}},
                    "guidance_refs": {"type": "array", "items": {"type": "string"}},
                    "recommendations": {"type": "array", "items": {"type": "object", "additionalProperties": false, "properties": {"owner": {"type": "string", "enum": ["DBA", "Developer", "Management"]}, "priority": {"type": "string", "enum": ["immediate", "high", "medium", "low"]}, "action": {"type": "string"}}, "required": ["owner", "priority", "action"]}}
                },
                "required": ["category", "title", "severity", "confidence", "conclusion", "evidence_refs"]
            }),
        ),
        function_definition(
            "set_report_assessment",
            "Records one mandatory final assessment. Non-unknown conclusions must cite measurement evidence from this session.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "assessment": {"type": "string", "enum": REQUIRED_ASSESSMENTS},
                    "status": {"type": "string", "enum": ["proven", "not_proven", "unknown"]},
                    "conclusion": {"type": "string"},
                    "evidence_refs": {"type": "array", "items": {"type": "string"}},
                    "guidance_refs": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["assessment", "status", "conclusion", "evidence_refs"]
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
        {"id": "timeline_and_baseline_comparison", "outputs": ["metric series", "SQL timeline", "wait timeline", "snapshot comparison", "wait histogram"], "access": ["get_metric_time_series", "get_sql_timeline", "get_wait_event_timeline", "compare_snapshots", "get_wait_event_histogram"], "caveat": "Always pair SNAP_ID with timestamp and compare a peak with a representative quiet baseline."}
    ])
}

fn quality_gates(platform: &str) -> Value {
    json!([
        {"gate": "cpu_pressure", "required": platform.to_lowercase().contains("aix"), "rule": "On AIX, inspect Entc%, physc/pc, entitled capacity, capped/shared mode and temporal alignment before deciding CPU pressure."},
        {"gate": "disk_quality", "required": true, "rule": "Separate measured latency from I/O request volume; inspect LGWR, DBWR, buffer-cache and direct-I/O evidence."},
        {"gate": "application_and_commit_policy", "required": true, "rule": "Do not infer bad application design or commit policy from high executions or waits alone; verify transaction, redo, latency and direct anti-pattern evidence."},
        {"gate": "sql_tuning", "required": true, "rule": "Inspect SQL text, timeline and available plans before concrete SQL tuning recommendations."},
        {"gate": "cursor_contention", "required": true, "rule": "Use child-cursor reasons and parse/reload/invalidation evidence before explaining cursor proliferation or mutex contention."},
        {"gate": "parameter_changes", "required": true, "rule": "A parameter recommendation requires its observed current value and a causal performance rationale; missing means unknown."}
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
        .get("alert_logs")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > 0
    {
        calls.push(json!({"tool": "get_alertlog_errors", "reason": "correlate Oracle errors and incidents with snapshot evidence"}));
    }
    Value::Array(calls)
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
        "extension_policy": "Core sections cannot be removed. Detail may be changed per category and evidence/guidance appendices are optional."
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
        "ready_to_finalize": ready,
        "findings": state.findings.len(),
        "evidence_records": state.evidence.len(),
        "guidance_refs": state.guidance_refs.len(),
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

    output.push_str("## 1. Executive Summary\n\n");
    let mut leading = state.findings.values().collect::<Vec<_>>();
    leading.sort_by_key(|finding| severity_rank(&finding.severity));
    if leading.is_empty() {
        output.push_str("No evidence-backed findings have been recorded.\n\n");
    } else {
        for finding in leading.into_iter().take(5) {
            output.push_str(&format!(
                "- **{}** [{} / {}]: {} {}\n",
                finding.title,
                finding.severity,
                finding.confidence,
                finding.conclusion,
                inline_refs(&finding.evidence_refs, &finding.guidance_refs)
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
                "### {} [{} / {}]\n\n{} {}\n\n",
                finding.title,
                finding.severity,
                finding.confidence,
                finding.conclusion,
                inline_refs(&finding.evidence_refs, &finding.guidance_refs)
            ));
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
                "- **{} — {}**: {} {}\n",
                assessment.replace('_', " "),
                value.status,
                value.conclusion,
                inline_refs(&value.evidence_refs, &value.guidance_refs)
            ));
        } else {
            output.push_str(&format!(
                "- **{} — UNKNOWN**: assessment not completed.\n",
                assessment.replace('_', " ")
            ));
        }
    }
    output.push('\n');

    if state.config.include_evidence_appendix {
        output.push_str("## Appendix A. Evidence Register\n\n");
        for record in state.evidence.values() {
            output.push_str(&format!(
                "- `{}` — `{}` with arguments `{}`\n",
                record.evidence_id, record.tool_name, record.arguments
            ));
        }
        output.push('\n');
    }
    if state.config.include_guidance_appendix {
        output.push_str("## Appendix B. Diagnostic Guidance Consulted\n\n");
        if state.guidance_refs.is_empty() {
            output.push_str("No external diagnostic guidance was consulted.\n\n");
        } else {
            for reference in &state.guidance_refs {
                output.push_str(&format!(
                    "- `{reference}` — methodology only, not measurement evidence\n"
                ));
            }
            output.push('\n');
        }
    }
    output.push_str("Generated by JAS-MIN · https://github.com/ora600pl/jas-min · expert performance tuning at ora-600.pl\n");
    output
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
        .filter(|reference| !state.guidance_refs.contains(*reference))
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

fn inline_refs(evidence: &[String], guidance: &[String]) -> String {
    let mut references = evidence
        .iter()
        .map(|reference| format!("`{reference}`"))
        .collect::<Vec<_>>();
    references.extend(guidance.iter().map(|reference| format!("`{reference}`")));
    if references.is_empty() {
        String::new()
    } else {
        format!("[{}]", references.join(", "))
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

fn count_name_contains(directory: &Path, needle: &str) -> usize {
    read_files(directory)
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.to_lowercase().contains(needle))
        })
        .count()
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
    use crate::awr::{DBInstance, SnapInfo, AWR};
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
        AnalysisRuntime {
            collection: Arc::new(collection),
            report: Arc::new(ReportForAI::default()),
            stem: Arc::new("nonexistent-test-dataset".to_string()),
            security_level: 0,
            guidance: Arc::new(GuidanceLibrary::default()),
            sessions: Arc::new(DashMap::new()),
            sequence: Arc::new(AtomicU64::new(1)),
        }
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
        let start = tools
            .iter()
            .find(|tool| tool.name == "start_performance_analysis")
            .unwrap();
        assert!(start.input_schema["properties"]
            .get("analysis_id")
            .is_none());
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
        assert_eq!(
            final_report["report"]["section_index"]
                .as_array()
                .unwrap()
                .len(),
            11
        );
        assert_eq!(final_report["draft"], false);
    }
}
