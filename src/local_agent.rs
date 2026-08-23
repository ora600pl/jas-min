use crate::ai_tools::{dispatch_tool_call, tools_schema};
use crate::awr::{load_awrs_collection_from_json_str, AWRSCollection};
use crate::debug_note;
use crate::reasonings::{DbTimeGradientSection, ReportForAI};
use crate::tools::estimate_tokens_from_str;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

const LOCAL_AGENT_SCHEMA_VERSION: &str = "2026-08-23.3";
const DEFAULT_MAX_TOOL_RESULT_CHARS: usize = 16 * 1024;
const DEFAULT_CONTEXT_HIGH_WATER_PCT: usize = 72;
const DEFAULT_TOOL_OUTPUT_TOKENS: usize = 3_072;
const DEFAULT_CHECKPOINT_OUTPUT_TOKENS: usize = 4_096;
const DEFAULT_FINAL_OUTPUT_TOKENS: usize = 12_288;
const DEFAULT_TOKEN_ESTIMATE_SAFETY_FACTOR: f64 = 2.0;
const DEFAULT_MAX_GUIDANCE_CHARS: usize = 8 * 1024;
const MAX_FINAL_CONTINUATIONS: usize = 2;
const SEED_EVIDENCE_ID: &str = "SEED-E0001";

#[derive(Debug, Clone)]
pub struct LocalAgentConfig {
    pub model: String,
    pub language: String,
    pub context_tokens: usize,
    pub max_tool_iterations: usize,
    pub max_tool_result_chars: usize,
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: usize,
    pub tool_output_tokens: usize,
    pub checkpoint_output_tokens: usize,
    pub final_output_tokens: usize,
    pub token_estimate_safety_factor: f64,
    pub max_guidance_chars: usize,
}

impl LocalAgentConfig {
    pub fn from_args(model: &str, language: &str, args: &crate::Args) -> Self {
        Self {
            model: env::var("LOCAL_MODEL").unwrap_or_else(|_| model.to_string()),
            language: language.to_string(),
            context_tokens: env_usize("LOCAL_CONTEXT_TOKENS", args.tokens_budget).max(16_384),
            max_tool_iterations: args.max_tool_iterations.max(1),
            max_tool_result_chars: env_usize(
                "LOCAL_MAX_TOOL_RESULT_CHARS",
                DEFAULT_MAX_TOOL_RESULT_CHARS,
            ),
            // Qwen 3.6 defaults to top_k=20. Gemma users can retain its
            // recommended top_k=64 with LOCAL_TOP_K=64.
            temperature: env_f64("LOCAL_TEMPERATURE", 1.0),
            top_p: env_f64("LOCAL_TOP_P", 0.95),
            top_k: env_usize(
                "LOCAL_TOP_K",
                if model.to_lowercase().contains("qwen") {
                    20
                } else {
                    64
                },
            ),
            tool_output_tokens: env_usize("LOCAL_TOOL_OUTPUT_TOKENS", DEFAULT_TOOL_OUTPUT_TOKENS),
            checkpoint_output_tokens: env_usize(
                "LOCAL_CHECKPOINT_OUTPUT_TOKENS",
                DEFAULT_CHECKPOINT_OUTPUT_TOKENS,
            ),
            final_output_tokens: env_usize(
                "LOCAL_FINAL_OUTPUT_TOKENS",
                DEFAULT_FINAL_OUTPUT_TOKENS,
            ),
            token_estimate_safety_factor: env_f64(
                "LOCAL_TOKEN_ESTIMATE_SAFETY_FACTOR",
                DEFAULT_TOKEN_ESTIMATE_SAFETY_FACTOR,
            )
            .max(1.0),
            max_guidance_chars: env_usize("LOCAL_MAX_GUIDANCE_CHARS", DEFAULT_MAX_GUIDANCE_CHARS)
                .max(1_024),
        }
    }

    fn closing_output_tokens(&self, session: u8) -> usize {
        if session == 1 {
            self.checkpoint_output_tokens
        } else {
            self.final_output_tokens
        }
        .clamp(1_024, self.context_tokens / 4)
    }

    fn high_water_tokens(&self) -> usize {
        let ratio_limit = self
            .context_tokens
            .saturating_mul(DEFAULT_CONTEXT_HIGH_WATER_PCT)
            / 100;
        let hard_limit = self
            .context_tokens
            .saturating_sub(self.final_output_tokens)
            .saturating_sub(4_096);
        ratio_limit.min(hard_limit).max(8_192)
    }

    fn conservative_token_estimate(&self, raw_estimate: usize) -> usize {
        (raw_estimate as f64 * self.token_estimate_safety_factor).ceil() as usize
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceRecord {
    pub evidence_id: String,
    pub session: u8,
    pub tool_name: String,
    pub arguments: Value,
    pub cached: bool,
    pub result: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct GuidanceRecord {
    pub guidance_ref: String,
    pub session: u8,
    pub arguments: Value,
    pub cached: bool,
    pub section_ids: Vec<String>,
    pub result: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentUsageRecord {
    pub session: u8,
    pub phase: String,
    pub round: usize,
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub finish_reason: String,
}

#[derive(Debug, Clone)]
pub struct LocalAgentOutcome {
    pub final_markdown: String,
    pub investigation_checkpoint: Value,
    pub evidence: Vec<EvidenceRecord>,
    pub guidance: Vec<GuidanceRecord>,
    pub usage: Vec<AgentUsageRecord>,
}

#[derive(Debug, Clone)]
struct GuidanceSection {
    section_id: String,
    title: String,
    content: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GuidanceLibrary {
    source_path: Option<PathBuf>,
    sections: Vec<GuidanceSection>,
}

impl GuidanceLibrary {
    pub(crate) fn load() -> Self {
        let path = reasonings_path();
        match fs::read_to_string(&path) {
            Ok(content) => {
                let library = Self::from_text(path.clone(), &content);
                println!(
                    "Diagnostic guidance loaded from {}: {} sections, {} chars",
                    path.display(),
                    library.sections.len(),
                    content.chars().count()
                );
                library
            }
            Err(error) => {
                println!(
                    "Diagnostic guidance unavailable at {}: {}",
                    path.display(),
                    error
                );
                Self::default()
            }
        }
    }

    fn from_text(path: PathBuf, content: &str) -> Self {
        let mut sections = Vec::new();
        let mut current: Option<GuidanceSection> = None;

        for line in content.lines() {
            if let Some((section_id, title, is_subsection)) = parse_guidance_heading(line) {
                if let Some(section) = current.take() {
                    sections.push(section);
                }
                if is_subsection {
                    current = Some(GuidanceSection {
                        content: format!("{section_id} {title}\n"),
                        section_id,
                        title,
                    });
                }
                continue;
            }
            if let Some(section) = current.as_mut() {
                section.content.push_str(line);
                section.content.push('\n');
            }
        }
        if let Some(section) = current {
            sections.push(section);
        }

        Self {
            source_path: Some(path),
            sections,
        }
    }

    pub(crate) fn is_available(&self) -> bool {
        !self.sections.is_empty()
    }

    pub(crate) fn catalog(&self) -> String {
        if self.sections.is_empty() {
            return "No external diagnostic guidance library is available.".to_string();
        }
        let entries = self
            .sections
            .iter()
            .map(|section| format!("{} {}", section.section_id, section.title))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "External diagnostic guidance is available through `get_diagnostic_guidance`. Guidance is methodology, never observed evidence. Fetch only sections relevant to a detected symptom and verify every TRIGGER with database evidence.\n{entries}"
        )
    }

    fn prompt_notice(&self) -> String {
        if self.is_available() {
            format!(
                "A local diagnostic guidance library is available through `get_diagnostic_guidance` ({} indexed sections). Query it by a concrete Oracle symptom. Guidance is methodology, never observed evidence.",
                self.sections.len()
            )
        } else {
            "No external diagnostic guidance library is available.".to_string()
        }
    }

    pub(crate) fn query(&self, arguments: &Value, max_chars: usize) -> Value {
        let topic = arguments
            .get("topic")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let max_sections = arguments
            .get("max_sections")
            .and_then(Value::as_u64)
            .unwrap_or(3)
            .clamp(1, 5) as usize;
        if topic.is_empty() {
            return json!({
                "error": "topic must contain a section id or diagnostic symptom",
                "available_sections": self.sections.len()
            });
        }

        let normalized_topic = normalize_guidance_id(topic);
        let topic_lower = topic.to_lowercase();
        let topic_tokens = guidance_tokens(topic);
        let mut ranked = self
            .sections
            .iter()
            .filter_map(|section| {
                let normalized_id = normalize_guidance_id(&section.section_id);
                let title_lower = section.title.to_lowercase();
                let content_lower = section.content.to_lowercase();
                let title_tokens = guidance_tokens(&section.title)
                    .into_iter()
                    .collect::<HashSet<_>>();
                let content_tokens = guidance_tokens(&section.content)
                    .into_iter()
                    .collect::<HashSet<_>>();
                let mut score = 0usize;
                if normalized_topic == normalized_id {
                    score += 10_000;
                }
                if title_lower.contains(&topic_lower) {
                    score += 1_000;
                } else if content_lower.contains(&topic_lower) {
                    score += 250;
                }
                for token in &topic_tokens {
                    if title_tokens.contains(token) {
                        score += 40;
                    } else if content_tokens.contains(token) {
                        score += 5;
                    }
                }
                (score > 0).then_some((score, section))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|(score_a, section_a), (score_b, section_b)| {
            score_b
                .cmp(score_a)
                .then_with(|| section_a.section_id.cmp(&section_b.section_id))
        });

        let mut remaining = max_chars.max(1_024);
        let mut matches = Vec::new();
        for (score, section) in ranked.into_iter().take(max_sections) {
            if remaining == 0 {
                break;
            }
            let original_chars = section.content.chars().count();
            let take = original_chars.min(remaining);
            let text = section.content.chars().take(take).collect::<String>();
            remaining = remaining.saturating_sub(take);
            matches.push(json!({
                "section_id": section.section_id,
                "title": section.title,
                "match_score": score,
                "text": text,
                "truncated": take < original_chars
            }));
        }

        if matches.is_empty() {
            return json!({
                "error": "no diagnostic guidance matched the topic",
                "topic": topic,
                "hint": "Use an exact section id from the guidance catalog or a concrete Oracle wait/statistic name."
            });
        }
        json!({
            "source_file": self.source_path.as_ref().and_then(|path| path.file_name()).map(|name| name.to_string_lossy().to_string()),
            "topic": topic,
            "methodology_only": true,
            "mandatory_rule": "Guidance is not evidence. Verify every trigger and required indicator using seed or tool evidence before accepting a diagnosis or action.",
            "matches": matches
        })
    }

    /// Returns a compact machine-readable catalog for MCP bootstrap responses.
    pub(crate) fn catalog_json(&self) -> Value {
        json!({
            "available": self.is_available(),
            "section_count": self.sections.len(),
            "sections": self.sections.iter().map(|section| json!({
                "section_id": section.section_id,
                "guidance_ref": format!("GUIDE-{}", section.section_id),
                "title": section.title
            })).collect::<Vec<_>>()
        })
    }
}

fn reasonings_path() -> PathBuf {
    env::var("JASMIN_HOME")
        .map(|home| Path::new(&home).join("reasonings.txt"))
        .unwrap_or_else(|_| PathBuf::from("reasonings.txt"))
}

fn parse_guidance_heading(line: &str) -> Option<(String, String, bool)> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix('§')?;
    let id_end = rest
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .unwrap_or(rest.len());
    let raw_id = rest[..id_end].trim_end_matches('.');
    if raw_id.is_empty()
        || !raw_id.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
    {
        return None;
    }
    let title = rest[id_end..].trim().to_string();
    let section_id = format!("§{raw_id}");
    Some((section_id, title, raw_id.contains('.')))
}

fn normalize_guidance_id(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('§')
        .trim_end_matches('.')
        .to_lowercase()
}

fn guidance_tokens(value: &str) -> Vec<String> {
    value
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() >= 3)
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone)]
struct ChatTurn {
    message: Value,
    prompt_tokens: usize,
    completion_tokens: usize,
    finish_reason: String,
}

struct LocalChatClient {
    http: Client,
    endpoint: String,
    api_key: String,
    cfg: LocalAgentConfig,
}

impl LocalChatClient {
    fn new(cfg: LocalAgentConfig) -> Self {
        let configured =
            env::var("LOCAL_BASE_URL").unwrap_or_else(|_| "http://localhost:1234/v1".to_string());
        Self {
            http: Client::new(),
            endpoint: normalize_chat_endpoint(&configured),
            api_key: env::var("LOCAL_API_KEY").unwrap_or_else(|_| "lm-studio".to_string()),
            cfg,
        }
    }

    async fn complete(
        &self,
        messages: &[Value],
        tools: Option<&Value>,
        tool_choice: Option<&str>,
        response_format: Option<Value>,
        max_tokens: usize,
        enable_thinking: bool,
        temperature: Option<f64>,
    ) -> Result<ChatTurn, Box<dyn std::error::Error>> {
        let mut payload = json!({
            "model": self.cfg.model,
            "messages": messages,
            "temperature": temperature.unwrap_or(self.cfg.temperature),
            "top_p": self.cfg.top_p,
            "top_k": self.cfg.top_k,
            "max_tokens": max_tokens,
            "stream": false,
            "enable_thinking": enable_thinking
        });

        if let Some(tools) = tools {
            payload["tools"] = tools.clone();
            payload["tool_choice"] = json!(tool_choice.unwrap_or("auto"));
        }
        if let Some(response_format) = response_format {
            payload["response_format"] = response_format;
        }

        debug_note!(
            "Local model request prepared: endpoint='{}', model='{}', messages={}, tools={}, max_tokens={}, thinking={}, payload_bytes={}",
            self.endpoint,
            self.cfg.model,
            messages.len(),
            tools.and_then(Value::as_array).map_or(0, Vec::len),
            max_tokens,
            enable_thinking,
            payload.to_string().len()
        );

        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .header("Content-Type", "application/json")
            .header("X-Title", "jas-min-local-agent")
            .json(&payload)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        debug_note!(
            "Local model response received: status={}, body_bytes={}",
            status,
            body.len()
        );
        if !status.is_success() {
            return Err(format!("LM Studio HTTP {}: {}", status, body).into());
        }

        let value: Value = serde_json::from_str(&body)
            .map_err(|e| format!("LM Studio returned malformed JSON: {e}; body: {body}"))?;
        let message = value
            .pointer("/choices/0/message")
            .cloned()
            .ok_or("LM Studio response has no choices[0].message")?;

        debug_note!(
            "Local model response parsed: prompt_tokens={}, completion_tokens={}, finish_reason='{}'",
            value
                .pointer("/usage/prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            value
                .pointer("/usage/completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            value
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                .unwrap_or("")
        );

        Ok(ChatTurn {
            message,
            prompt_tokens: value
                .pointer("/usage/prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            completion_tokens: value
                .pointer("/usage/completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            finish_reason: value
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
    }

    async fn detect_model_context_tokens(&self) -> Option<usize> {
        let origin = self
            .endpoint
            .strip_suffix("/v1/chat/completions")
            .unwrap_or(&self.endpoint);
        let response = self
            .http
            .get(format!("{origin}/api/v0/models"))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .ok()?;
        debug_note!(
            "Local model context preflight response: endpoint='{}', status={}",
            origin,
            response.status()
        );
        if !response.status().is_success() {
            return None;
        }
        let catalog: Value = response.json().await.ok()?;
        find_model_context_tokens(&catalog, &self.cfg.model)
    }
}

#[derive(Default)]
struct EvidenceStore {
    records: Vec<EvidenceRecord>,
    guidance_records: Vec<GuidanceRecord>,
    cache: HashMap<String, Value>,
    session_evidence_cache: HashMap<String, String>,
    session_guidance_cache: HashMap<String, String>,
}

impl EvidenceStore {
    fn execute(
        &mut self,
        session: u8,
        tool_name: &str,
        arguments: &Value,
        report: &ReportForAI,
        collection: &AWRSCollection,
        guidance_library: &GuidanceLibrary,
        stem: &str,
        max_chars: usize,
        max_guidance_chars: usize,
    ) -> String {
        debug_note!(
            "Local agent tool dispatch: session={}, name='{}', argument_count={}",
            session,
            tool_name,
            arguments.as_object().map_or(0, serde_json::Map::len)
        );
        let cache_key = format!("{}:{}", tool_name, canonical_json(arguments));
        let session_cache_key = format!("{session}:{cache_key}");
        if tool_name == "get_diagnostic_guidance" {
            if let Some(guidance_ref) = self.session_guidance_cache.get(&session_cache_key) {
                debug_note!(
                    "Local agent reused session guidance: session={}, name='{}', reference='{}'",
                    session,
                    tool_name,
                    guidance_ref
                );
                return serde_json::to_string(&json!({
                    "guidance_ref": guidance_ref,
                    "cached": true,
                    "duplicate_in_session": true,
                    "result_omitted": true,
                    "methodology_only": true,
                    "not_evidence": true,
                    "instruction": "Use the guidance result already returned under this guidance_ref."
                }))
                .unwrap_or_default();
            }
        } else if let Some(evidence_id) = self.session_evidence_cache.get(&session_cache_key) {
            debug_note!(
                "Local agent reused session evidence: session={}, name='{}', evidence_id='{}'",
                session,
                tool_name,
                evidence_id
            );
            return serde_json::to_string(&json!({
                "evidence_id": evidence_id,
                "cached": true,
                "duplicate_in_session": true,
                "result_omitted": true,
                "tool": tool_name,
                "instruction": "Use the evidence result already returned under this evidence_id."
            }))
            .unwrap_or_default();
        }

        let (raw_result, cached) = if let Some(value) = self.cache.get(&cache_key) {
            (value.clone(), true)
        } else {
            let raw = match tool_name {
                "get_precomputed_analysis" => dispatch_precomputed_analysis(arguments, report),
                "get_diagnostic_guidance" => guidance_library.query(arguments, max_guidance_chars),
                _ => {
                    let text = dispatch_tool_call(tool_name, arguments, collection, stem);
                    serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
                }
            };
            self.cache.insert(cache_key, raw.clone());
            (raw, false)
        };

        if tool_name == "get_diagnostic_guidance" {
            let guidance_number = self
                .guidance_records
                .iter()
                .filter(|record| record.session == session)
                .count()
                + 1;
            let guidance_ref = format!("S{}-G{:04}", session, guidance_number);
            let section_ids = raw_result
                .get("matches")
                .and_then(Value::as_array)
                .map(|matches| {
                    matches
                        .iter()
                        .filter_map(|item| item.get("section_id").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let bounded_result = bound_json_result(&raw_result, max_chars);
            self.guidance_records.push(GuidanceRecord {
                guidance_ref: guidance_ref.clone(),
                session,
                arguments: arguments.clone(),
                cached,
                section_ids: section_ids.clone(),
                result: raw_result,
            });
            self.session_guidance_cache
                .insert(session_cache_key, guidance_ref.clone());
            debug_note!(
                "Local agent stored guidance: session={}, reference='{}', cached={}, sections={}",
                session,
                guidance_ref,
                cached,
                section_ids.len()
            );
            return serde_json::to_string(&json!({
                "guidance_ref": guidance_ref,
                "section_ids": section_ids,
                "cached": cached,
                "tool": tool_name,
                "methodology_only": true,
                "not_evidence": true,
                "result": bounded_result
            }))
            .unwrap_or_else(|error| json!({ "error": error.to_string() }).to_string());
        }

        let session_record_number = self
            .records
            .iter()
            .filter(|record| record.session == session)
            .count()
            + 1;
        let evidence_id = format!("S{}-E{:04}", session, session_record_number);
        let bounded_result = bound_json_result(&raw_result, max_chars);
        let raw_result_bytes = raw_result.to_string().len();
        self.records.push(EvidenceRecord {
            evidence_id: evidence_id.clone(),
            session,
            tool_name: tool_name.to_string(),
            arguments: arguments.clone(),
            cached,
            result: raw_result,
        });
        self.session_evidence_cache
            .insert(session_cache_key, evidence_id.clone());

        debug_note!(
            "Local agent stored evidence: session={}, name='{}', evidence_id='{}', cached={}, result_bytes={}",
            session,
            tool_name,
            evidence_id,
            cached,
            raw_result_bytes
        );

        serde_json::to_string(&json!({
            "evidence_id": evidence_id,
            "cached": cached,
            "tool": tool_name,
            "arguments": arguments,
            "result": bounded_result
        }))
        .unwrap_or_else(|e| json!({ "error": e.to_string() }).to_string())
    }
}

#[tokio::main]
pub async fn analyze_report_local_agent(
    report: &ReportForAI,
    args: &crate::Args,
    report_name: &str,
    model: &str,
    language: &str,
) -> Result<LocalAgentOutcome, Box<dyn std::error::Error>> {
    let mut cfg = LocalAgentConfig::from_args(model, language, args);
    let collection = load_collection(args)?;
    let stem = stem_from_report_name(report_name);
    let seed = build_case_seed(report);
    let guidance_library = GuidanceLibrary::load();
    let guidance_catalog = guidance_library.prompt_notice();
    let tools = local_tools_schema(&stem, guidance_library.is_available());
    let preflight_client = LocalChatClient::new(cfg.clone());
    debug_note!(
        "Starting local agent analysis: report='{}', model='{}', language='{}', snapshots={}, configured_context={}, max_tool_iterations={}",
        report_name,
        cfg.model,
        cfg.language,
        collection.awrs.len(),
        cfg.context_tokens,
        cfg.max_tool_iterations
    );
    if let Some(detected_context) = preflight_client.detect_model_context_tokens().await {
        if detected_context < cfg.context_tokens {
            println!(
                "LM Studio model context detected as {}; lowering configured context from {}",
                detected_context, cfg.context_tokens
            );
            cfg.context_tokens = detected_context.max(16_384);
        }
    } else {
        println!(
            "LM Studio context preflight unavailable; using configured context {} (override with LOCAL_CONTEXT_TOKENS)",
            cfg.context_tokens
        );
    }
    let client = LocalChatClient::new(cfg.clone());
    let mut evidence_store = EvidenceStore::default();
    evidence_store.records.push(EvidenceRecord {
        evidence_id: SEED_EVIDENCE_ID.to_string(),
        session: 0,
        tool_name: "initial_case_seed".to_string(),
        arguments: json!({}),
        cached: false,
        result: seed.clone(),
    });
    let mut usage = Vec::new();

    println!(
        "=== Local investigator: model={}, context={}, high-water={} ===",
        cfg.model,
        cfg.context_tokens,
        cfg.high_water_tokens()
    );

    let checkpoint = run_investigation_session(
        &client,
        1,
        investigator_system_prompt(&cfg.language, &guidance_catalog),
        investigator_user_prompt(&seed),
        &tools,
        report,
        &collection,
        &guidance_library,
        &stem,
        &cfg,
        &mut evidence_store,
        &mut usage,
        checkpoint_prompt(),
    )
    .await?;
    debug_note!(
        "Local agent investigator session completed: evidence={}, guidance={}, usage_records={}",
        evidence_store.records.len(),
        evidence_store.guidance_records.len(),
        usage.len()
    );
    write_local_agent_progress(
        report_name,
        &checkpoint,
        &evidence_store.records,
        &evidence_store.guidance_records,
        &usage,
    )?;

    let reviewer_seed = json!({
        "case_seed": seed,
        "session_1_checkpoint": checkpoint
    });
    let final_analysis_result = run_investigation_session(
        &client,
        2,
        reviewer_system_prompt(&cfg.language, &guidance_catalog),
        reviewer_user_prompt(&reviewer_seed),
        &tools,
        report,
        &collection,
        &guidance_library,
        &stem,
        &cfg,
        &mut evidence_store,
        &mut usage,
        final_report_prompt(&cfg.language),
    )
    .await;
    write_local_agent_progress(
        report_name,
        &checkpoint,
        &evidence_store.records,
        &evidence_store.guidance_records,
        &usage,
    )?;
    let final_analysis = final_analysis_result?;

    let final_markdown = final_analysis
        .get("markdown")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or("Final local-agent response does not contain markdown")?;

    debug_note!(
        "Local agent analysis completed: markdown_bytes={}, evidence={}, guidance={}, usage_records={}",
        final_markdown.len(),
        evidence_store.records.len(),
        evidence_store.guidance_records.len(),
        usage.len()
    );

    Ok(LocalAgentOutcome {
        final_markdown,
        investigation_checkpoint: checkpoint,
        evidence: evidence_store.records,
        guidance: evidence_store.guidance_records,
        usage,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_investigation_session(
    client: &LocalChatClient,
    session: u8,
    system_prompt: String,
    initial_prompt: String,
    tools: &Value,
    report: &ReportForAI,
    collection: &AWRSCollection,
    guidance_library: &GuidanceLibrary,
    stem: &str,
    cfg: &LocalAgentConfig,
    evidence_store: &mut EvidenceStore,
    usage: &mut Vec<AgentUsageRecord>,
    closing_prompt: String,
) -> Result<Value, Box<dyn std::error::Error>> {
    debug_note!(
        "Starting local investigation session: session={}, max_rounds={}, context={}, high_water={}",
        session,
        cfg.max_tool_iterations,
        cfg.context_tokens,
        cfg.high_water_tokens()
    );
    let base_system_prompt = system_prompt.clone();
    let base_initial_prompt = initial_prompt.clone();
    let mut messages = vec![
        json!({ "role": "system", "content": system_prompt }),
        json!({ "role": "user", "content": initial_prompt }),
    ];
    let mut observed_token_ratio = cfg.token_estimate_safety_factor;

    for round in 0..cfg.max_tool_iterations {
        let round_tools = tools_for_round(tools, round);
        let raw_estimate = estimate_chat_request_tokens(&messages, Some(&round_tools));
        let estimated = calibrated_token_estimate(raw_estimate, observed_token_ratio);
        if estimated >= cfg.high_water_tokens() {
            debug_note!(
                "Local investigation reached high-water: session={}, round={}, estimated_tokens={}, high_water={}",
                session,
                round + 1,
                estimated,
                cfg.high_water_tokens()
            );
            println!(
                "Session {} reached context high-water before round {}: ~{}/{} tokens",
                session,
                round + 1,
                estimated,
                cfg.high_water_tokens()
            );
            break;
        }

        println!(
            "Session {} tool round {}/{} (~{} conservative prompt tokens; raw estimate {})",
            session,
            round + 1,
            cfg.max_tool_iterations,
            estimated,
            raw_estimate
        );
        let turn = client
            .complete(
                &messages,
                Some(&round_tools),
                Some("auto"),
                None,
                cfg.tool_output_tokens,
                true,
                None,
            )
            .await?;
        update_observed_token_ratio(&mut observed_token_ratio, raw_estimate, turn.prompt_tokens);
        usage.push(AgentUsageRecord {
            session,
            phase: "tool_loop".to_string(),
            round: round + 1,
            prompt_tokens: turn.prompt_tokens,
            completion_tokens: turn.completion_tokens,
            finish_reason: turn.finish_reason.clone(),
        });

        let tool_calls = extract_tool_calls(&turn.message);
        if tool_calls.is_empty() {
            debug_note!(
                "Local investigation turn returned no tool calls: session={}, round={}, finish_reason='{}'",
                session,
                round + 1,
                turn.finish_reason
            );
            if round == 0 {
                messages.push(turn.message);
                messages.push(json!({
                    "role": "user",
                    "content": "The investigation has no tool evidence yet. Call at least one narrow diagnostic tool now. Do not write the report."
                }));
                continue;
            }
            break;
        }

        messages.push(turn.message);
        for (call_id, tool_name, arguments) in tool_calls {
            println!("  tool: {}({})", tool_name, arguments);
            debug_note!(
                "Local investigation requested tool: session={}, round={}, name='{}'",
                session,
                round + 1,
                tool_name
            );
            let output = evidence_store.execute(
                session,
                &tool_name,
                &arguments,
                report,
                collection,
                guidance_library,
                stem,
                cfg.max_tool_result_chars,
                cfg.max_guidance_chars,
            );
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": output
            }));
        }

        if turn
            .prompt_tokens
            .saturating_add(turn.completion_tokens)
            .saturating_add(cfg.closing_output_tokens(session))
            .saturating_add(4_096)
            >= cfg.context_tokens
        {
            println!(
                "Session {} stopping tool loop after round {} based on actual LM Studio usage ({})",
                session,
                round + 1,
                turn.prompt_tokens + turn.completion_tokens
            );
            break;
        }
    }

    let _mandatory_outputs = ensure_mandatory_evidence(
        session,
        evidence_store,
        report,
        collection,
        guidance_library,
        stem,
        cfg,
        tools,
    );
    let _guidance_outputs = ensure_relevant_guidance(
        session,
        evidence_store,
        report,
        collection,
        guidance_library,
        stem,
        cfg,
    );
    let coverage = build_coverage_summary(session, evidence_store, collection);
    debug_note!(
        "Local investigation closing context prepared: session={}, evidence={}, guidance={}, coverage_bytes={}",
        session,
        evidence_store.records.iter().filter(|record| record.session == session).count(),
        evidence_store.guidance_records.iter().filter(|record| record.session == session).count(),
        coverage.to_string().len()
    );
    messages = build_compact_closing_messages(
        &base_system_prompt,
        &base_initial_prompt,
        session,
        evidence_store,
        &coverage,
        &closing_prompt,
        cfg,
    );
    let qwen_structured_checkpoint = session == 1 && cfg.model.to_lowercase().contains("qwen");
    let closing_tools =
        (session == 1 && !qwen_structured_checkpoint).then(checkpoint_submission_tool);
    let closing_response_format = qwen_structured_checkpoint.then(checkpoint_response_format);
    let raw_estimate = estimate_chat_request_tokens(&messages, closing_tools.as_ref());
    let estimated = calibrated_token_estimate(raw_estimate, observed_token_ratio);
    let closing_output_tokens = cfg.closing_output_tokens(session);
    if estimated.saturating_add(closing_output_tokens) > cfg.context_tokens {
        return Err(format!(
            "Session {} cannot fit its closing response: conservative input estimate {}, output reserve {}, context {}",
            session,
            estimated,
            closing_output_tokens,
            cfg.context_tokens
        )
        .into());
    }

    let turn = client
        .complete(
            &messages,
            closing_tools.as_ref(),
            (session == 1 && !qwen_structured_checkpoint).then_some("required"),
            closing_response_format,
            closing_output_tokens,
            false,
            Some(0.2),
        )
        .await?;
    usage.push(AgentUsageRecord {
        session,
        phase: if session == 1 {
            "checkpoint".to_string()
        } else {
            "final_report".to_string()
        },
        round: cfg.max_tool_iterations + 1,
        prompt_tokens: turn.prompt_tokens,
        completion_tokens: turn.completion_tokens,
        finish_reason: turn.finish_reason.clone(),
    });

    if session == 2 {
        let mut content = extract_message_content(&turn.message);
        if content.trim().is_empty() {
            return Err("Session 2 returned an empty final Markdown report".into());
        }
        let mut final_turn = turn;
        for continuation in 0..MAX_FINAL_CONTINUATIONS {
            if final_turn.finish_reason != "length" {
                break;
            }
            messages.push(final_turn.message.clone());
            messages.push(json!({
                "role": "user",
                "content": "Continue the report exactly where it stopped. Do not repeat headings or prior text. Complete all remaining required sections and the required footer."
            }));
            let continuation_tokens = (closing_output_tokens / 2).max(2_048);
            let continuation_raw = estimate_chat_request_tokens(&messages, None);
            let continuation_estimated =
                calibrated_token_estimate(continuation_raw, observed_token_ratio);
            if continuation_estimated.saturating_add(continuation_tokens) > cfg.context_tokens {
                return Err(format!(
                    "Session 2 report was truncated and continuation {} cannot fit safely in context",
                    continuation + 1
                )
                .into());
            }
            final_turn = client
                .complete(
                    &messages,
                    None,
                    None,
                    None,
                    continuation_tokens,
                    false,
                    Some(0.2),
                )
                .await?;
            usage.push(AgentUsageRecord {
                session,
                phase: "final_report_continuation".to_string(),
                round: cfg.max_tool_iterations + 2 + continuation,
                prompt_tokens: final_turn.prompt_tokens,
                completion_tokens: final_turn.completion_tokens,
                finish_reason: final_turn.finish_reason.clone(),
            });
            let continuation_content = extract_message_content(&final_turn.message);
            if continuation_content.trim().is_empty() {
                return Err("Session 2 returned an empty final report continuation".into());
            }
            content = merge_continuation(&content, &continuation_content);
        }
        if final_turn.finish_reason == "length" {
            return Err(
                "Session 2 exhausted continuation attempts; refusing to publish a truncated report"
                    .into(),
            );
        }
        debug_note!(
            "Local reviewer session completed: markdown_bytes={}, finish_reason='{}'",
            content.len(),
            final_turn.finish_reason
        );
        return Ok(json!({
            "markdown": content,
            "unresolved_limitations": Vec::<&str>::new(),
            "evidence_backed": true
        }));
    }

    let first_checkpoint = if turn.finish_reason == "length" {
        Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "checkpoint reached token limit",
        )))
    } else {
        extract_checkpoint(&turn.message).and_then(validate_checkpoint)
    };
    match first_checkpoint {
        Ok(value) => Ok(value),
        Err(first_error) => {
            eprintln!(
                "Session {} structured response was invalid ({}; finish_reason={}); retrying compact serialization",
                session, first_error, turn.finish_reason
            );
            messages.push(json!({
                "role": "user",
                "content": "The previous serialization failed. Return the smallest complete JSON object allowed by the schema. Use short strings, no repetition, no commentary and no Markdown fence."
            }));
            let retry = client
                .complete(
                    &messages,
                    None,
                    None,
                    Some(checkpoint_response_format()),
                    closing_output_tokens,
                    false,
                    Some(0.0),
                )
                .await?;
            usage.push(AgentUsageRecord {
                session,
                phase: if session == 1 {
                    "checkpoint_retry".to_string()
                } else {
                    "final_report_retry".to_string()
                },
                round: cfg.max_tool_iterations + 2,
                prompt_tokens: retry.prompt_tokens,
                completion_tokens: retry.completion_tokens,
                finish_reason: retry.finish_reason.clone(),
            });
            let retry_content = extract_message_content(&retry.message);
            let retry_checkpoint = if retry.finish_reason == "length" {
                Err(serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "checkpoint retry reached token limit",
                )))
            } else {
                parse_json_content(&retry_content).and_then(validate_checkpoint)
            };
            match retry_checkpoint {
                Ok(value) => Ok(value),
                Err(retry_error) => {
                    eprintln!(
                        "Session 1 compact checkpoint remained invalid ({}); continuing with a recovered checkpoint",
                        retry_error
                    );
                    Ok(recover_checkpoint(
                        &retry_content,
                        evidence_store,
                        &first_error.to_string(),
                        &retry_error.to_string(),
                    ))
                }
            }
        }
    }
}

fn build_compact_closing_messages(
    system_prompt: &str,
    initial_prompt: &str,
    session: u8,
    evidence_store: &EvidenceStore,
    coverage: &Value,
    closing_prompt: &str,
    cfg: &LocalAgentConfig,
) -> Vec<Value> {
    let session_records = evidence_store
        .records
        .iter()
        .filter(|record| record.session == session)
        .collect::<Vec<_>>();
    let per_evidence_chars = if session_records.is_empty() {
        2_048
    } else {
        (cfg.max_tool_result_chars.saturating_mul(2) / session_records.len()).clamp(2_048, 6_144)
    };
    let evidence = session_records
        .into_iter()
        .map(|record| {
            json!({
                "evidence_id": record.evidence_id,
                "tool": record.tool_name,
                "arguments": record.arguments,
                "cached": record.cached,
                "result": bound_json_result(&record.result, per_evidence_chars)
            })
        })
        .collect::<Vec<_>>();
    let guidance = evidence_store
        .guidance_records
        .iter()
        .filter(|record| record.session == session)
        .map(|record| {
            json!({
                "guidance_ref": record.guidance_ref,
                "section_ids": record.section_ids,
                "methodology_only": true,
                "result": bound_json_result(&record.result, 3_072)
            })
        })
        .collect::<Vec<_>>();
    let compact_case_file = json!({
        "session": session,
        "database_evidence": evidence,
        "diagnostic_guidance_not_evidence": guidance,
        "coverage": coverage
    });

    vec![
        json!({ "role": "system", "content": system_prompt }),
        json!({ "role": "user", "content": initial_prompt }),
        json!({
            "role": "user",
            "content": format!(
                "COMPACT ORCHESTRATOR CASE FILE\nThis replaces the verbose tool transcript. Full results remain in the audit store; use only the bounded facts below. Guidance is methodology, not evidence.\n{}",
                serde_json::to_string(&compact_case_file).unwrap_or_default()
            )
        }),
        json!({
            "role": "user",
            "content": format!(
                "{closing_prompt}\n\nTreat `available_not_inspected` differently from `unavailable` or `unknown`. Do not claim data is absent merely because it was not inspected."
            )
        }),
    ]
}

fn calibrated_token_estimate(raw_estimate: usize, observed_ratio: f64) -> usize {
    (raw_estimate as f64 * observed_ratio.max(1.0)).ceil() as usize
}

fn update_observed_token_ratio(observed_ratio: &mut f64, raw_estimate: usize, actual: usize) {
    if raw_estimate > 0 && actual > 0 {
        let measured = actual as f64 / raw_estimate as f64;
        *observed_ratio = observed_ratio.max(measured * 1.05);
    }
}

fn validate_checkpoint(value: Value) -> Result<Value, serde_json::Error> {
    let valid = value.get("claims").and_then(Value::as_array).is_some()
        && value
            .get("unresolved_questions")
            .and_then(Value::as_array)
            .is_some()
        && value
            .get("next_session_priorities")
            .and_then(Value::as_array)
            .is_some();
    if valid {
        Ok(value)
    } else {
        Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "checkpoint is missing required arrays",
        )))
    }
}

fn merge_continuation(prefix: &str, continuation: &str) -> String {
    let prefix = prefix.trim_end();
    let continuation = continuation.trim_start();
    let max_overlap = prefix
        .chars()
        .count()
        .min(continuation.chars().count())
        .min(512);
    for overlap in (8..=max_overlap).rev() {
        let suffix = prefix.chars().rev().take(overlap).collect::<String>();
        let suffix = suffix.chars().rev().collect::<String>();
        let head = continuation.chars().take(overlap).collect::<String>();
        if suffix == head {
            return format!(
                "{}{}",
                prefix,
                continuation.chars().skip(overlap).collect::<String>()
            );
        }
    }
    format!("{prefix}\n{continuation}")
}

#[allow(clippy::too_many_arguments)]
fn ensure_mandatory_evidence(
    session: u8,
    evidence_store: &mut EvidenceStore,
    report: &ReportForAI,
    collection: &AWRSCollection,
    guidance_library: &GuidanceLibrary,
    stem: &str,
    cfg: &LocalAgentConfig,
    tools: &Value,
) -> Vec<String> {
    if session != 2 {
        return Vec::new();
    }
    let called = |name: &str, store: &EvidenceStore| {
        store
            .records
            .iter()
            .any(|record| record.session == session && record.tool_name == name)
    };
    let tool_available = |name: &str| {
        tools.as_array().is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.pointer("/function/name").and_then(Value::as_str) == Some(name))
        })
    };
    let haystack = evidence_store
        .records
        .iter()
        .filter(|record| record.session == session || record.session == 0)
        .map(|record| serde_json::to_string(&record.result).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    let precomputed_sections = evidence_store
        .records
        .iter()
        .filter(|record| {
            record.session == session && record.tool_name == "get_precomputed_analysis"
        })
        .filter_map(|record| {
            record
                .arguments
                .get("section")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<HashSet<_>>();
    let mut requests = Vec::new();
    if collection
        .db_instance_information
        .platform
        .to_lowercase()
        .contains("aix")
        && tool_available("get_aix_cpu_entitlement_summary")
        && !called("get_aix_cpu_entitlement_summary", evidence_store)
    {
        requests.push((
            "get_aix_cpu_entitlement_summary",
            json!({"max_files": 20, "limit_records": 100}),
        ));
    }
    if (haystack.contains("cursor: pin") || haystack.contains("library cache pin"))
        && !collection.initialization_parameters.is_empty()
        && !called("get_init_parameter", evidence_store)
    {
        requests.push((
            "get_init_parameter",
            json!({
                "names": [
                    "session_cached_cursors",
                    "open_cursors",
                    "cursor_sharing",
                    "_cursor_obsolete_threshold"
                ]
            }),
        ));
    }
    if tool_available("list_available_sql_plans")
        && !called("list_available_sql_plans", evidence_store)
    {
        requests.push(("list_available_sql_plans", json!({"limit": 100})));
    }
    if tool_available("list_available_child_cursor_reasons")
        && !called("list_available_child_cursor_reasons", evidence_store)
    {
        requests.push(("list_available_child_cursor_reasons", json!({"limit": 100})));
    }
    if (haystack.contains("enq: tx") || haystack.contains("row lock"))
        && !called("top_segments_in_snapshot", evidence_store)
        && !precomputed_sections.contains("segment_hotspots")
    {
        requests.push((
            "get_precomputed_analysis",
            json!({"section": "segment_hotspots", "limit": 10}),
        ));
    }
    if !precomputed_sections.contains("io_summary") {
        requests.push((
            "get_precomputed_analysis",
            json!({"section": "io_summary", "limit": 20}),
        ));
    }
    if !precomputed_sections.contains("load_profile_anomalies") {
        requests.push((
            "get_precomputed_analysis",
            json!({"section": "load_profile_anomalies", "limit": 20}),
        ));
    }

    requests
        .into_iter()
        .map(|(tool_name, arguments)| {
            evidence_store.execute(
                session,
                tool_name,
                &arguments,
                report,
                collection,
                guidance_library,
                stem,
                cfg.max_tool_result_chars.min(6_144),
                cfg.max_guidance_chars,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn ensure_relevant_guidance(
    session: u8,
    evidence_store: &mut EvidenceStore,
    report: &ReportForAI,
    collection: &AWRSCollection,
    guidance_library: &GuidanceLibrary,
    stem: &str,
    cfg: &LocalAgentConfig,
) -> Vec<String> {
    if !guidance_library.is_available() {
        return Vec::new();
    }
    let haystack = evidence_store
        .records
        .iter()
        .filter(|record| record.session == session || record.session == 0)
        .map(|record| serde_json::to_string(&record.result).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    let candidates = [
        (&["cursor: pin", "cursor pin"][..], "cursor pin contention"),
        (&["enq: tx", "row lock"][..], "enq TX row lock contention"),
        (
            &["log file sync", "log file parallel write"][..],
            "log file sync",
        ),
        (&["db file sequential read"][..], "db file sequential read"),
        (&["db file scattered read"][..], "db file scattered read"),
        (
            &["direct path read", "direct path write"][..],
            "direct path I/O",
        ),
        (
            &["buffer busy waits", "read by other session"][..],
            "buffer contention",
        ),
        (&["latch:", "latch free"][..], "latch contention"),
        (&["undo", "snapshot too old"][..], "UNDO"),
    ];
    let mut outputs = Vec::new();
    for (needles, topic) in candidates {
        if outputs.len() >= 2 || !needles.iter().any(|needle| haystack.contains(needle)) {
            continue;
        }
        let output = evidence_store.execute(
            session,
            "get_diagnostic_guidance",
            &json!({"topic": topic, "max_sections": 1}),
            report,
            collection,
            guidance_library,
            stem,
            cfg.max_tool_result_chars.min(4_096),
            cfg.max_guidance_chars.min(4_096),
        );
        outputs.push(output);
    }
    outputs
}

fn build_coverage_summary(
    session: u8,
    evidence_store: &EvidenceStore,
    collection: &AWRSCollection,
) -> Value {
    let calls = evidence_store
        .records
        .iter()
        .filter(|record| record.session == session)
        .map(|record| record.tool_name.as_str())
        .collect::<HashSet<_>>();
    let inspected = |names: &[&str]| names.iter().any(|name| calls.contains(name));
    let status = |is_inspected: bool, available: Option<bool>| {
        if is_inspected {
            "inspected"
        } else {
            match available {
                Some(true) => "available_not_inspected",
                Some(false) => "unavailable",
                None => "unknown",
            }
        }
    };
    let awr_available = !collection.awrs.is_empty();
    json!({
        "session": session,
        "database_overview": status(inspected(&["get_database_load_summary", "get_db_instance_info"]), Some(awr_available)),
        "snapshot_details": status(inspected(&["get_snapshot_details", "compare_snapshots", "list_snapshots"]), Some(awr_available)),
        "wait_events": status(inspected(&["top_wait_events_in_snapshot", "get_wait_event_timeline", "get_wait_event_histogram"]), Some(awr_available)),
        "sql_metrics": status(inspected(&["top_sqls_in_snapshot", "get_sql_timeline", "find_snapshots_with_sql"]), Some(awr_available)),
        "sql_text": status(inspected(&["get_sql_text", "search_sql_text"]), Some(!collection.sql_text.is_empty())),
        "execution_plans": status(inspected(&["list_available_sql_plans", "get_sql_execution_plan"]), None),
        "child_cursor_reasons": status(inspected(&["list_available_child_cursor_reasons", "get_child_cursor_reasons"]), None),
        "segments": status(inspected(&["top_segments_in_snapshot", "find_sqls_touching_object"]), Some(awr_available)),
        "latches": status(inspected(&["top_latches_in_snapshot"]), Some(awr_available)),
        "io_redo_load_profile": status(inspected(&["get_snapshot_details", "get_metric_time_series", "get_precomputed_analysis"]), Some(awr_available)),
        "initialization_parameters": status(inspected(&["get_init_parameter"]), Some(!collection.initialization_parameters.is_empty())),
        "aix_entitlement": status(inspected(&["get_aix_cpu_entitlement_summary"]), None),
        "diagnostic_guidance": if evidence_store.guidance_records.iter().any(|record| record.session == session) { "inspected" } else if guidance_records_available(evidence_store) { "available_not_inspected" } else { "unknown" },
        "evidence_refs": evidence_store.records.iter().filter(|record| record.session == session).map(|record| record.evidence_id.clone()).collect::<Vec<_>>(),
        "guidance_refs": evidence_store.guidance_records.iter().filter(|record| record.session == session).map(|record| record.guidance_ref.clone()).collect::<Vec<_>>()
    })
}

fn guidance_records_available(evidence_store: &EvidenceStore) -> bool {
    !evidence_store.guidance_records.is_empty()
}

fn recover_checkpoint(
    raw_content: &str,
    evidence_store: &EvidenceStore,
    first_error: &str,
    retry_error: &str,
) -> Value {
    let evidence_refs = evidence_store
        .records
        .iter()
        .filter(|record| record.session == 1)
        .map(|record| record.evidence_id.clone())
        .collect::<Vec<_>>();
    let inspected_entities = evidence_store
        .records
        .iter()
        .filter(|record| record.session == 1)
        .map(|record| {
            format!(
                "{}: {}({})",
                record.evidence_id, record.tool_name, record.arguments
            )
        })
        .collect::<Vec<_>>();
    let guidance_refs = evidence_store
        .guidance_records
        .iter()
        .filter(|record| record.session == 1)
        .map(|record| record.guidance_ref.clone())
        .collect::<Vec<_>>();

    json!({
        "claims": [{
            "claim_id": "RECOVERED-CHECKPOINT",
            "conclusion": "The model checkpoint was truncated; Session 2 must independently verify all candidate conclusions from the preserved prefix and evidence records.",
            "confidence": "unknown",
            "evidence_summary": "Tool evidence is intact, but the narrative checkpoint is only a bounded raw prefix.",
            "evidence_refs": evidence_refs,
            "guidance_refs": guidance_refs,
            "counterevidence_refs": []
        }],
        "consulted_guidance_refs": guidance_refs,
        "rejected_hypotheses": [],
        "unresolved_questions": ["Reconstruct and falsify Session 1 candidate conclusions using the original seed and cited evidence."],
        "inspected_entities": inspected_entities,
        "coverage_gaps": ["Session 1 checkpoint JSON was incomplete due to repetitive model output."],
        "next_session_priorities": ["Read the recovered raw prefix, then re-query narrow tools before accepting any conclusion."],
        "serialization_recovered": true,
        "serialization_errors": { "first": first_error, "retry": retry_error },
        "raw_model_checkpoint_prefix": raw_content.chars().take(8_000).collect::<String>()
    })
}

pub fn write_local_agent_outputs(
    base_name: &str,
    outcome: &LocalAgentOutcome,
) -> Result<(), Box<dyn std::error::Error>> {
    debug_note!(
        "Writing local agent outputs: base='{}', markdown_bytes={}, evidence={}, guidance={}, usage_records={}",
        base_name,
        outcome.final_markdown.len(),
        outcome.evidence.len(),
        outcome.guidance.len(),
        outcome.usage.len()
    );
    fs::write(
        format!("{base_name}.final.md"),
        outcome.final_markdown.as_bytes(),
    )?;
    fs::write(
        format!("{base_name}.local_agent.checkpoint.json"),
        serde_json::to_vec_pretty(&outcome.investigation_checkpoint)?,
    )?;
    fs::write(
        format!("{base_name}.local_agent.evidence.json"),
        serde_json::to_vec_pretty(&outcome.evidence)?,
    )?;
    fs::write(
        format!("{base_name}.local_agent.guidance.json"),
        serde_json::to_vec_pretty(&outcome.guidance)?,
    )?;
    fs::write(
        format!("{base_name}.local_agent.usage.json"),
        serde_json::to_vec_pretty(&outcome.usage)?,
    )?;
    debug_note!("Local agent outputs written: base='{}'", base_name);
    Ok(())
}

fn write_local_agent_progress(
    base_name: &str,
    checkpoint: &Value,
    evidence: &[EvidenceRecord],
    guidance: &[GuidanceRecord],
    usage: &[AgentUsageRecord],
) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(
        format!("{base_name}.local_agent.checkpoint.json"),
        serde_json::to_vec_pretty(checkpoint)?,
    )?;
    fs::write(
        format!("{base_name}.local_agent.evidence.json"),
        serde_json::to_vec_pretty(evidence)?,
    )?;
    fs::write(
        format!("{base_name}.local_agent.guidance.json"),
        serde_json::to_vec_pretty(guidance)?,
    )?;
    fs::write(
        format!("{base_name}.local_agent.usage.json"),
        serde_json::to_vec_pretty(usage)?,
    )?;
    Ok(())
}

pub(crate) fn build_case_seed(report: &ReportForAI) -> Value {
    let mut by_db_time = report.top_spikes_marked.clone();
    by_db_time.sort_by(|a, b| {
        b.db_time_value
            .partial_cmp(&a.db_time_value)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut by_cpu_ratio = report.top_spikes_marked.clone();
    by_cpu_ratio.sort_by(|a, b| {
        a.dbcpu_dbtime_ratio
            .partial_cmp(&b.dbcpu_dbtime_ratio)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut selected_snapshots = HashSet::new();
    let mut spikes = Vec::new();
    for spike in by_db_time
        .into_iter()
        .take(8)
        .chain(by_cpu_ratio.into_iter().take(4))
    {
        if selected_snapshots.insert(spike.snap_id) {
            spikes.push(spike);
        }
    }

    json!({
        "evidence_id": SEED_EVIDENCE_ID,
        "schema_version": LOCAL_AGENT_SCHEMA_VERSION,
        "ratio_definition": {
            "field": "db_cpu_to_db_time_ratio",
            "formula": "DB CPU / DB Time",
            "warning": "This is not DB Time / DB CPU. The historical ReportForAI field is named dbcpu_dbtime_ratio."
        },
        "performance_peaks": spikes,
        "performance_peaks_total": report.top_spikes_marked.len(),
        "db_time_degradation": compact_degradation(report),
        "gradients": {
            "db_time_foreground_wait_events": compact_gradient(report.db_time_gradient_fg_wait_events.as_ref()),
            "db_time_instance_stats_counters": compact_gradient(report.db_time_gradient_instance_stats_counters.as_ref()),
            "db_time_instance_stats_volumes": compact_gradient(report.db_time_gradient_instance_stats_volumes.as_ref()),
            "db_time_instance_stats_time": compact_gradient(report.db_time_gradient_instance_stats_time.as_ref()),
            "db_time_sql_elapsed_time": compact_gradient(report.db_time_gradient_sql_elapsed_time.as_ref()),
            "db_cpu_instance_stats": compact_gradient(report.db_cpu_gradient_instance_stats.as_ref()),
            "db_cpu_sql_cpu_time": compact_gradient(report.db_cpu_gradient_sql_cpu_time.as_ref()),
            "custom_wait_events": compact_gradient(report.custom_gradient_wait_events.as_ref()),
            "custom_instance_stats": compact_gradient(report.custom_gradient_instance_stats.as_ref())
        }
    })
}

fn compact_gradient(section: Option<&DbTimeGradientSection>) -> Value {
    let Some(section) = section else {
        return Value::Null;
    };
    json!({
        "settings": section.settings,
        "counts": {
            "cross_model_classifications": section.cross_model_classifications.len(),
            "vif_diagnostics": section.vif_diagnostics.len(),
            "collinear_group_impacts": section.collinear_group_impacts.len(),
            "ridge": section.ridge_top.len(),
            "elastic_net": section.elastic_net_top.len(),
            "huber": section.huber_top.len(),
            "quantile95": section.quantile95_top.len()
        },
        "cross_model_classifications": section.cross_model_classifications.iter().take(4).collect::<Vec<_>>(),
        "vif_diagnostics_omitted_from_seed": section.vif_diagnostics.len(),
        "collinear_group_impacts": section.collinear_group_impacts.iter().take(2).collect::<Vec<_>>(),
        "ridge_top": section.ridge_top.iter().take(3).collect::<Vec<_>>(),
        "elastic_net_top": section.elastic_net_top.iter().take(3).collect::<Vec<_>>(),
        "huber_top": section.huber_top.iter().take(3).collect::<Vec<_>>(),
        "quantile95_top": section.quantile95_top.iter().take(3).collect::<Vec<_>>()
    })
}

/// Return the requested analytical detail rather than the deliberately tiny
/// bootstrap preview. `full_gradients` previously reused `compact_gradient`,
/// which silently capped model rankings at three rows and triangulation at four
/// rows regardless of the caller's `limit` argument.
fn detailed_gradient(section: Option<&DbTimeGradientSection>, limit: usize) -> Value {
    let Some(section) = section else {
        return Value::Null;
    };
    let limited = |count: usize| count.min(limit);
    json!({
        "settings": section.settings,
        "counts": {
            "cross_model_classifications": section.cross_model_classifications.len(),
            "vif_diagnostics": section.vif_diagnostics.len(),
            "collinear_group_impacts": section.collinear_group_impacts.len(),
            "ridge": section.ridge_top.len(),
            "elastic_net": section.elastic_net_top.len(),
            "huber": section.huber_top.len(),
            "quantile95": section.quantile95_top.len()
        },
        "returned": {
            "cross_model_classifications": limited(section.cross_model_classifications.len()),
            "vif_diagnostics": limited(section.vif_diagnostics.len()),
            "collinear_group_impacts": limited(section.collinear_group_impacts.len()),
            "ridge": limited(section.ridge_top.len()),
            "elastic_net": limited(section.elastic_net_top.len()),
            "huber": limited(section.huber_top.len()),
            "quantile95": limited(section.quantile95_top.len())
        },
        "cross_model_classifications": section.cross_model_classifications.iter().take(limit).collect::<Vec<_>>(),
        "vif_diagnostics": section.vif_diagnostics.iter().take(limit).collect::<Vec<_>>(),
        "collinear_group_impacts": section.collinear_group_impacts.iter().take(limit).collect::<Vec<_>>(),
        "ridge_top": section.ridge_top.iter().take(limit).collect::<Vec<_>>(),
        "elastic_net_top": section.elastic_net_top.iter().take(limit).collect::<Vec<_>>(),
        "huber_top": section.huber_top.iter().take(limit).collect::<Vec<_>>(),
        "quantile95_top": section.quantile95_top.iter().take(limit).collect::<Vec<_>>()
    })
}

fn detailed_gradients(report: &ReportForAI, limit: usize) -> Value {
    json!({
        "db_time_foreground_wait_events": detailed_gradient(report.db_time_gradient_fg_wait_events.as_ref(), limit),
        "db_time_instance_stats_counters": detailed_gradient(report.db_time_gradient_instance_stats_counters.as_ref(), limit),
        "db_time_instance_stats_volumes": detailed_gradient(report.db_time_gradient_instance_stats_volumes.as_ref(), limit),
        "db_time_instance_stats_time": detailed_gradient(report.db_time_gradient_instance_stats_time.as_ref(), limit),
        "db_time_sql_elapsed_time": detailed_gradient(report.db_time_gradient_sql_elapsed_time.as_ref(), limit),
        "db_cpu_instance_stats": detailed_gradient(report.db_cpu_gradient_instance_stats.as_ref(), limit),
        "db_cpu_sql_cpu_time": detailed_gradient(report.db_cpu_gradient_sql_cpu_time.as_ref(), limit),
        "custom_wait_events": detailed_gradient(report.custom_gradient_wait_events.as_ref(), limit),
        "custom_instance_stats": detailed_gradient(report.custom_gradient_instance_stats.as_ref(), limit)
    })
}

fn compact_degradation(report: &ReportForAI) -> Value {
    let Some(degradation) = report.db_time_degradation_report.as_ref() else {
        return Value::Null;
    };
    json!({
        "is_degradation_detected": degradation.is_degradation_detected,
        "verdict": degradation.verdict,
        "baseline_start": degradation.baseline_start,
        "baseline_end": degradation.baseline_end,
        "degraded_start": degradation.degraded_start,
        "degraded_end": degradation.degraded_end,
        "baseline_samples": degradation.baseline_samples,
        "degraded_samples": degradation.degraded_samples,
        "db_time_baseline_avg": degradation.db_time_baseline_avg,
        "db_time_degraded_avg": degradation.db_time_degraded_avg,
        "db_time_delta_avg": degradation.db_time_delta_avg,
        "db_time_delta_pct": degradation.db_time_delta_pct,
        "db_time_robust_z_score": degradation.db_time_robust_z_score,
        "db_cpu_baseline_avg": degradation.db_cpu_baseline_avg,
        "db_cpu_degraded_avg": degradation.db_cpu_degraded_avg,
        "db_cpu_delta_avg": degradation.db_cpu_delta_avg,
        "db_cpu_delta_pct": degradation.db_cpu_delta_pct,
        "dominant_domains": degradation.dominant_domains,
        "findings_total": degradation.findings.len(),
        "findings": degradation.findings.iter().take(10).collect::<Vec<_>>()
    })
}

fn local_tools_schema(stem: &str, guidance_available: bool) -> Value {
    let mut schema = tools_schema(stem);
    if let Some(tools) = schema.as_array_mut() {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "get_precomputed_analysis",
                "description": "Fetches one bounded, precomputed ReportForAI analytical section. Use it to inspect aggregate evidence that is not present in the initial gradient/degradation seed before drilling into raw snapshots.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "section": {
                            "type": "string",
                            "enum": [
                                "foreground_waits", "background_waits", "top_sqls",
                                "io_summary", "latches", "segment_hotspots",
                                "instance_stat_correlations", "load_profile_anomalies",
                                "anomaly_clusters", "initialization_parameters",
                                "full_gradients", "db_time_degradation", "performance_peaks"
                            ]
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum top-level rows, default 20, max 100"
                        }
                    },
                    "required": ["section"]
                }
            }
        }));
        if guidance_available {
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": "get_diagnostic_guidance",
                    "description": "Retrieves a small relevant section from the local reasonings.txt Oracle diagnostic knowledge base. Call it for methodology after detecting a concrete symptom. Its output is guidance, not evidence: verify every trigger with database tools before making a claim.",
                    "parameters": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "topic": {
                                "type": "string",
                                "description": "Exact catalog section id such as §1.1 or a concrete symptom such as log file sync, row lock contention, physical read storm, SQL stability, or initialization parameters."
                            },
                            "max_sections": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 5,
                                "description": "Maximum matching sections; default 3. Prefer 1 for an exact section id."
                            }
                        },
                        "required": ["topic"]
                    }
                }
            }));
        }
    }
    schema
}

fn tools_for_round(full_schema: &Value, round: usize) -> Value {
    const TRIAGE: &[&str] = &[
        "get_database_load_summary",
        "get_db_instance_info",
        "list_snapshots",
        "get_snapshot_details",
        "top_sqls_in_snapshot",
        "top_wait_events_in_snapshot",
        "get_sql_text",
        "get_precomputed_analysis",
        "get_diagnostic_guidance",
        "list_aix_os_attachments",
        "get_aix_cpu_entitlement_summary",
    ];
    const INVESTIGATION: &[&str] = &[
        "get_database_load_summary",
        "get_db_instance_info",
        "list_snapshots",
        "get_snapshot_details",
        "top_sqls_in_snapshot",
        "top_wait_events_in_snapshot",
        "top_segments_in_snapshot",
        "top_latches_in_snapshot",
        "get_sql_text",
        "get_init_parameter",
        "get_wait_event_timeline",
        "get_sql_timeline",
        "compare_snapshots",
        "get_wait_event_histogram",
        "list_available_sql_plans",
        "get_sql_execution_plan",
        "list_available_child_cursor_reasons",
        "get_child_cursor_reasons",
        "get_precomputed_analysis",
        "get_diagnostic_guidance",
        "list_aix_os_attachments",
        "get_aix_os_attachment",
        "get_aix_cpu_entitlement_summary",
    ];

    let allowed = if round == 0 {
        Some(TRIAGE)
    } else if round == 1 {
        Some(INVESTIGATION)
    } else {
        None
    };
    let Some(allowed) = allowed else {
        return full_schema.clone();
    };
    let allowed = allowed.iter().copied().collect::<HashSet<_>>();
    Value::Array(
        full_schema
            .as_array()
            .into_iter()
            .flatten()
            .filter(|tool| {
                tool.pointer("/function/name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| allowed.contains(name))
            })
            .cloned()
            .collect(),
    )
}

pub(crate) fn dispatch_precomputed_analysis(args: &Value, report: &ReportForAI) -> Value {
    let section = args.get("section").and_then(Value::as_str).unwrap_or("");
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 100) as usize;
    let value = match section {
        "foreground_waits" => json!(report
            .top_foreground_wait_events
            .iter()
            .take(limit)
            .collect::<Vec<_>>()),
        "background_waits" => json!(report
            .top_background_wait_events
            .iter()
            .take(limit)
            .collect::<Vec<_>>()),
        "top_sqls" => json!(report
            .top_sqls_by_elapsed_time
            .iter()
            .take(limit)
            .collect::<Vec<_>>()),
        "io_summary" => json!(report
            .io_stats_by_function_summary
            .iter()
            .take(limit)
            .collect::<Vec<_>>()),
        "latches" => json!(report
            .latch_activity_summary
            .iter()
            .take(limit)
            .collect::<Vec<_>>()),
        "segment_hotspots" => json!({
            "row_lock_waits": report.top_10_segments_by_row_lock_waits,
            "physical_writes": report.top_10_segments_by_physical_writes,
            "physical_write_requests": report.top_10_segments_by_physical_write_requests,
            "physical_read_requests": report.top_10_segments_by_physical_read_requests,
            "logical_reads": report.top_10_segments_by_logical_reads,
            "direct_physical_writes": report.top_10_segments_by_direct_physical_writes,
            "direct_physical_reads": report.top_10_segments_by_direct_physical_reads,
            "buffer_busy_waits": report.top_10_segments_by_buffer_busy_waits
        }),
        "instance_stat_correlations" => json!(report
            .instance_stats_pearson_correlation
            .iter()
            .take(limit)
            .collect::<Vec<_>>()),
        "load_profile_anomalies" => json!(report
            .load_profile_anomalies
            .iter()
            .take(limit)
            .collect::<Vec<_>>()),
        "anomaly_clusters" => json!(report
            .anomaly_clusters
            .iter()
            .take(limit)
            .collect::<Vec<_>>()),
        "initialization_parameters" => {
            let mut parameters = report.initialization_parameters.iter().collect::<Vec<_>>();
            parameters.sort_by(|a, b| a.0.cmp(b.0));
            json!(parameters
                .into_iter()
                .take(limit)
                .collect::<HashMap<_, _>>())
        }
        "full_gradients" => detailed_gradients(report, limit),
        "db_time_degradation" => compact_degradation(report),
        "performance_peaks" => json!(report
            .top_spikes_marked
            .iter()
            .take(limit)
            .collect::<Vec<_>>()),
        _ => {
            return json!({
                "error": "unknown precomputed section",
                "requested_section": section
            })
        }
    };
    json!({
        "schema_version": LOCAL_AGENT_SCHEMA_VERSION,
        "section": section,
        "limit": limit,
        "data": value
    })
}

fn investigator_system_prompt(language: &str, guidance_catalog: &str) -> String {
    format!(
        r#"You are JAS-MIN Investigator, an expert Oracle Database performance diagnostician.

You receive only a compact, high-signal seed: gradient analyses, DB Time degradation and DB CPU/DB Time ratios for performance peaks. Detailed AWR/STATSPACK evidence is available through read-only tools.

DIAGNOSTIC GUIDANCE AVAILABILITY:
{guidance_catalog}

Your job in this session is evidence collection, not report writing:
1. Form several competing hypotheses from the seed.
2. Use tools proactively to confirm or falsify them.
3. Prefer narrow calls and compare bad snapshots with a quiet baseline.
4. Trace wait -> SQL -> execution plan/object -> workload or infrastructure when evidence permits; preserve correlation and direct ASH attribution as association evidence rather than causality.
5. Inspect SQL text, timeline and plan applicability before SQL tuning recommendations. BEGIN/DECLARE/CALL entry points are PL/SQL and have no expected top-level row-source plan; profile the PL/SQL unit and inspect its inner SQL instead of requesting DBMS_XPLAN recapture.
6. Check I/O evidence before deciding disks are slow.
7. Check redo/commit evidence before judging commit policy.
8. Inspect relevant initialization parameters; do not scan or recommend parameters without a performance rationale.
9. On AIX, call get_db_instance_info and get_aix_cpu_entitlement_summary before any CPU-bound conclusion.
10. Distinguish correlation/gradient sensitivity from causation. Respect VIF and collinear groups.
11. For a material symptom, fetch only the relevant diagnostic guidance section, then verify its TRIGGER and indicators with database tools. Guidance references (`S1-G...`) describe methodology and can never replace evidence references (`SEED-E0001`, `S1-E...`).

Inference guardrails:
- Bind count, column count or a wide SQL statement does not prove rows per batch, transaction size, hard parsing, or parse rate.
- DML volume, execution count and cursor-pin waits do not prove cursor proliferation, missing batching, missing indexes, or poor application design. Those require direct supporting evidence.
- A segment statistic named `Row Lock Waits` is a counter/metric for that segment, not a measured number of blocked rows unless the tool explicitly says so.
- `enq: TX` does not by itself prove an incorrect commit policy. `log file sync` must be assessed with latency, rate and redo/transaction volume.
- A long SQL text or cursor pin wait does not prove hard-parse pressure without parse/reload/invalidation evidence.
- Never recommend a changed initialization parameter unless its current value was inspected. Otherwise phrase it as a conditional diagnostic check.
- A parameter reported as not present in collected data is UNKNOWN, not proof that it is unset or using a particular default.
- High I/O operation count is not a bottleneck when its measured latency and DB Time share are small; distinguish workload volume from material performance impact.
- Extreme percentage gradients with a near-zero baseline are sensitivity flags, not independent confirmation.
- State `available but not inspected`, `unavailable`, and `unknown` precisely; they are not synonyms.

Every important claim must cite evidence_id values and include exact supporting values. Cite `{SEED_EVIDENCE_ID}` only for facts present in the initial seed, and cite tool-returned IDs for facts learned through tools. When applying a diagnostic rule, also record its guidance_ref separately. Never cite guidance as proof and never cite one evidence item for facts it does not contain. Explicitly record unknowns instead of guessing. Do not expose private chain-of-thought. Work in language: {language}."#
    )
}

fn reviewer_system_prompt(language: &str, guidance_catalog: &str) -> String {
    format!(
        r#"You are JAS-MIN Reviewer, a skeptical senior Oracle performance engineer.

You receive the original compact seed and a structured checkpoint from another investigation session. Do not merely rewrite or endorse it. Try to falsify every material conclusion, search for alternative explanations, verify temporal alignment, and obtain fresh evidence through tools. Re-query important evidence because the prior raw conversation is intentionally unavailable.

DIAGNOSTIC GUIDANCE AVAILABILITY:
{guidance_catalog}

Mandatory review gates:
- CPU-bound versus wait-bound, including AIX entitlement caveat.
- Disk latency versus high I/O/redo volume.
- SQL impact, SQL text, plans when available, and execution pattern.
- Application and commit/rollback policy: state UNKNOWN if evidence is insufficient.
- Initialization parameters related to proven findings.
- Contradictions between gradient, degradation, snapshot and timeline evidence.
- Guidance trigger validation: re-fetch important referenced guidance and reject diagnoses whose trigger or required indicators are not demonstrated by session-2 evidence.

Inference guardrails:
- Bind or column counts do not establish batch size or transaction boundaries.
- DML volume and cursor-pin waits do not establish cursor proliferation, missing batching/indexes, hot blocks, or poor application design without direct evidence.
- Report segment `Row Lock Waits` values as the named segment statistic, never as a count of blocked rows.
- Cursor pin waits and SQL width do not establish hard parsing without parse/reload/invalidation measurements.
- `enq: TX` does not establish commit policy; `log file sync` requires latency, rate and redo/transaction context.
- Parameter changes require the observed current value and a proven causal rationale; otherwise recommend only a conditional check.
- `not present in collected data` means UNKNOWN, never Oracle default/unset.
- Do not call I/O volume the problem merely because the operation count is high when its DB Time share is negligible.
- Treat very large percentage gradients from near-zero baselines as unstable sensitivity indicators.
- Never describe available-but-uninspected data as absent. If a plan catalog was inspected, distinguish an unavailable plan for a specific SQL_ID from no plans at all.
- DB CPU/DB Time describes database workload composition; on AIX it cannot prove that host/LPAR CPU pressure is absent. Entitlement evidence controls that conclusion.
- Do not compare configured/logical CPU count directly with entitlement to judge sizing; use Entc%, physc, capped/shared mode and temporal alignment.

Use `{SEED_EVIDENCE_ID}` for seed facts and evidence_id references from this session for tool-derived facts. Guidance references (`S2-G...`) may explain diagnostic methodology but are never factual proof. Never attach an evidence ID to a claim that its stored result does not support. On AIX, CPU capacity or spare-capacity conclusions are UNKNOWN unless this session obtained `get_aix_cpu_entitlement_summary`; configured CPUs are not entitlement. Never invent MOS notes, measurements, SQL text, object names or parameter values. Do not expose private chain-of-thought. Produce analysis in language: {language}."#
    )
}

fn investigator_user_prompt(seed: &Value) -> String {
    format!(
        "Investigate this Oracle performance case. Begin by calling tools; do not write the final report in this phase.\n\nCASE_SEED:\n{}",
        serde_json::to_string(seed).unwrap_or_default()
    )
}

fn reviewer_user_prompt(seed: &Value) -> String {
    format!(
        "Audit and improve the previous investigation. Begin with fresh tool evidence and focus on falsification and missing coverage.\n\nREVIEW_INPUT:\n{}",
        serde_json::to_string(seed).unwrap_or_default()
    )
}

fn checkpoint_prompt() -> String {
    r#"Stop requesting tools. Return a compact investigation checkpoint conforming to the supplied JSON schema.

For every claim include a concise evidence_summary with exact values, evidence_refs and separate guidance_refs. Confidence must reflect evidence, never guidance alone. Record all consulted_guidance_refs, rejected hypotheses, unresolved questions, inspected entities, coverage gaps and priorities for an independent second session. This checkpoint is a factual case file, not chain-of-thought and not the final report."#
        .to_string()
}

fn final_report_prompt(language: &str) -> String {
    format!(
        r#"Stop requesting tools and produce the final evidence-backed Oracle performance report directly as Markdown in {language}. Do not wrap the report in JSON and do not use a Markdown code fence.

Required Markdown structure:
1. Executive Summary
2. Overall Performance Profile and DB Time degradation
3. Wait Events
4. SQL-Level Analysis, including plan findings when plans were available
5. Segments and Objects
6. Latches and Internal Contention
7. I/O and Disk Assessment
8. UNDO, Redo and Load Profile
9. Gradient and anomaly synthesis
10. Relevant Initialization Parameters
11. Prioritized Actions: DBA, Developers, Immediate, Management

Explicitly answer:
- Are disks slow, or is the problem volume? Cite measurements.
- Is the application poorly written? Answer UNKNOWN unless direct evidence demonstrates a concrete anti-pattern; high DML, execution counts, waits or segment activity alone are insufficient.
- Is commit/rollback policy proper? Say UNKNOWN when not provable.
- Is CPU pressure present? Apply the AIX entitlement rule when relevant.
- Which actions have the highest business impact and who owns them?

Use exact values and cite evidence IDs inline. Cite `{SEED_EVIDENCE_ID}` for gradient/degradation/peak facts and session-2 IDs only for facts actually present in those tool results. When a recommendation applies diagnostic guidance, cite the relevant `S2-G...` reference alongside—not instead of—the supporting evidence. On AIX, state CPU capacity as UNKNOWN unless session 2 collected entitlement evidence. State unresolved limitations. Never invent references or facts. End with https://github.com/ora600pl/jas-min and mention expert performance tuning at ora-600.pl."#
    )
}

fn checkpoint_response_format() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "jasmin_investigation_checkpoint",
            "strict": true,
            "schema": checkpoint_object_schema()
        }
    })
}

fn checkpoint_submission_tool() -> Value {
    json!([{
        "type": "function",
        "function": {
            "name": "submit_investigation_checkpoint",
            "description": "Submit the complete compact factual checkpoint for the independent review session. This is a transfer object, not a final report.",
            "parameters": checkpoint_object_schema()
        }
    }])
}

fn checkpoint_object_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "claims": {
                "type": "array",
                "maxItems": 6,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "claim_id": { "type": "string", "maxLength": 48 },
                        "conclusion": { "type": "string", "maxLength": 280 },
                        "confidence": { "type": "string", "enum": ["high", "medium", "low", "unknown"] },
                        "evidence_summary": { "type": "string", "maxLength": 500 },
                        "evidence_refs": { "type": "array", "maxItems": 10, "items": { "type": "string", "maxLength": 24 } },
                        "guidance_refs": { "type": "array", "maxItems": 6, "items": { "type": "string", "maxLength": 24 } },
                        "counterevidence_refs": { "type": "array", "maxItems": 6, "items": { "type": "string", "maxLength": 24 } }
                    },
                    "required": ["claim_id", "conclusion", "confidence", "evidence_summary", "evidence_refs", "guidance_refs", "counterevidence_refs"]
                }
            },
            "consulted_guidance_refs": { "type": "array", "maxItems": 8, "items": { "type": "string", "maxLength": 24 } },
            "rejected_hypotheses": { "type": "array", "maxItems": 8, "items": { "type": "string", "maxLength": 240 } },
            "unresolved_questions": { "type": "array", "maxItems": 8, "items": { "type": "string", "maxLength": 240 } },
            "inspected_entities": { "type": "array", "maxItems": 16, "items": { "type": "string", "maxLength": 240 } },
            "coverage_gaps": { "type": "array", "maxItems": 10, "items": { "type": "string", "maxLength": 240 } },
            "next_session_priorities": { "type": "array", "maxItems": 8, "items": { "type": "string", "maxLength": 240 } }
        },
        "required": ["claims", "consulted_guidance_refs", "rejected_hypotheses", "unresolved_questions", "inspected_entities", "coverage_gaps", "next_session_priorities"]
    })
}

fn extract_tool_calls(message: &Value) -> Vec<(String, String, Value)> {
    message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .enumerate()
                .filter_map(|(idx, call)| {
                    let name = call.pointer("/function/name")?.as_str()?.to_string();
                    let id = call
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("local-call-{idx}"));
                    let raw_arguments = call.pointer("/function/arguments")?;
                    let arguments = match raw_arguments {
                        Value::String(text) => {
                            serde_json::from_str(text).unwrap_or_else(|_| json!({}))
                        }
                        Value::Object(_) => raw_arguments.clone(),
                        _ => json!({}),
                    };
                    Some((id, name, arguments))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn extract_message_content(message: &Value) -> String {
    let content = match message.get("content") {
        Some(Value::String(content)) => content.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .or_else(|| part.get("content"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    if !content.trim().is_empty() {
        return content;
    }
    message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn extract_checkpoint(message: &Value) -> Result<Value, serde_json::Error> {
    if let Some((_, _, arguments)) = extract_tool_calls(message)
        .into_iter()
        .find(|(_, name, _)| name == "submit_investigation_checkpoint")
    {
        return Ok(arguments);
    }
    parse_json_content(&extract_message_content(message))
}

fn parse_json_content(content: &str) -> Result<Value, serde_json::Error> {
    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str(trimmed)
}

fn bound_json_result(value: &Value, max_chars: usize) -> Value {
    let serialized = serde_json::to_string(value).unwrap_or_default();
    if serialized.chars().count() <= max_chars {
        return value.clone();
    }
    let prefix: String = serialized.chars().take(max_chars).collect();
    json!({
        "truncated": true,
        "original_chars": serialized.chars().count(),
        "prefix": prefix,
        "instruction": "Call a narrower tool, lower limit, or request specific snapshot sections."
    })
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            let fields = keys
                .into_iter()
                .map(|key| format!("{}:{}", key, canonical_json(&map[key])))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{fields}}}")
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => value.to_string(),
    }
}

fn estimate_chat_request_tokens(messages: &[Value], tools: Option<&Value>) -> usize {
    let mut value = json!({ "messages": messages });
    if let Some(tools) = tools {
        value["tools"] = tools.clone();
    }
    estimate_tokens_from_str(&serde_json::to_string(&value).unwrap_or_default())
}

fn normalize_chat_endpoint(configured: &str) -> String {
    let base = configured.trim_end_matches('/');
    if base.ends_with("/v1/chat/completions") {
        base.to_string()
    } else if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

fn find_model_context_tokens(catalog: &Value, model: &str) -> Option<usize> {
    match catalog {
        Value::Array(items) => items
            .iter()
            .find_map(|item| find_model_context_tokens(item, model)),
        Value::Object(map) => {
            let id_matches = ["id", "model", "model_key", "identifier"]
                .iter()
                .filter_map(|key| map.get(*key).and_then(Value::as_str))
                .any(|id| id == model || id.ends_with(model) || model.ends_with(id));
            if id_matches {
                for key in [
                    "loaded_context_length",
                    "context_length",
                    "max_context_length",
                ] {
                    if let Some(tokens) = map.get(key).and_then(Value::as_u64) {
                        return Some(tokens as usize);
                    }
                }
            }
            map.values()
                .find_map(|value| find_model_context_tokens(value, model))
        }
        _ => None,
    }
}

fn load_collection(args: &crate::Args) -> Result<AWRSCollection, Box<dyn std::error::Error>> {
    let path = if !args.json_file().is_empty() {
        args.json_file().to_string()
    } else if !args.outfile.is_empty() {
        args.outfile.clone()
    } else {
        format!("{}.json", args.directory())
    };
    let data = fs::read_to_string(&path)
        .map_err(|e| format!("Cannot load AWR collection for local tools from {path}: {e}"))?;
    load_awrs_collection_from_json_str(&data)
        .map_err(|e| format!("Invalid AWR collection JSON in {path}: {e}").into())
}

fn stem_from_report_name(report_name: &str) -> String {
    report_name
        .split('.')
        .next()
        .unwrap_or(report_name)
        .to_string()
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_f64(name: &str, default: f64) -> f64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awr::DBInstance;
    use crate::reasonings::TopPeaksSelected;

    #[test]
    fn endpoint_accepts_origin_v1_and_full_url() {
        assert_eq!(
            normalize_chat_endpoint("http://localhost:1234"),
            "http://localhost:1234/v1/chat/completions"
        );
        assert_eq!(
            normalize_chat_endpoint("http://localhost:1234/v1/"),
            "http://localhost:1234/v1/chat/completions"
        );
        assert_eq!(
            normalize_chat_endpoint("http://localhost:1234/v1/chat/completions"),
            "http://localhost:1234/v1/chat/completions"
        );
    }

    #[test]
    fn case_seed_contains_only_initial_high_signal_sections() {
        let mut report = ReportForAI::default();
        report.top_spikes_marked.push(TopPeaksSelected {
            snap_id: 42,
            db_time_value: 120.0,
            db_cpu_value: 30.0,
            dbcpu_dbtime_ratio: 0.25,
            ..Default::default()
        });
        let seed = build_case_seed(&report);
        assert!(seed.get("performance_peaks").is_some());
        assert_eq!(seed["evidence_id"], SEED_EVIDENCE_ID);
        assert!(seed.get("db_time_degradation").is_some());
        assert!(seed.get("gradients").is_some());
        assert!(seed.get("top_foreground_wait_events").is_none());
        assert_eq!(seed["performance_peaks"][0]["snap_id"], 42);
    }

    #[test]
    fn case_seed_caps_peaks_and_preserves_low_cpu_ratio_examples() {
        let mut report = ReportForAI::default();
        for index in 0..30 {
            report.top_spikes_marked.push(TopPeaksSelected {
                snap_id: index,
                db_time_value: 1_000.0 - index as f64,
                db_cpu_value: 100.0,
                dbcpu_dbtime_ratio: if index == 29 { 0.001 } else { 0.5 },
                ..Default::default()
            });
        }
        let seed = build_case_seed(&report);
        let peaks = seed["performance_peaks"].as_array().unwrap();
        assert!(peaks.len() <= 12);
        assert!(peaks.iter().any(|peak| peak["snap_id"] == 29));
    }

    #[test]
    fn oversized_tool_result_remains_valid_json() {
        let value = json!({ "rows": ["x".repeat(1_000)] });
        let bounded = bound_json_result(&value, 100);
        assert_eq!(bounded["truncated"], true);
        assert!(serde_json::to_string(&bounded).is_ok());
    }

    #[test]
    fn canonical_json_ignores_object_key_order() {
        let a = json!({ "b": 2, "a": 1 });
        let b = json!({ "a": 1, "b": 2 });
        assert_eq!(canonical_json(&a), canonical_json(&b));
    }

    #[test]
    fn tool_call_arguments_accept_string_and_object() {
        let message = json!({
            "tool_calls": [
                {"id":"a", "function":{"name":"one", "arguments":"{\"x\":1}"}},
                {"id":"b", "function":{"name":"two", "arguments":{"y":2}}}
            ]
        });
        let calls = extract_tool_calls(&message);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].2["x"], 1);
        assert_eq!(calls[1].2["y"], 2);
    }

    #[test]
    fn structured_content_falls_back_to_qwen_reasoning_content() {
        let message = json!({
            "content": "",
            "reasoning_content": "{\"claims\":[]}"
        });
        assert_eq!(extract_message_content(&message), "{\"claims\":[]}");
        let normal = json!({
            "content": "visible",
            "reasoning_content": "hidden"
        });
        assert_eq!(extract_message_content(&normal), "visible");
    }

    #[test]
    fn checkpoint_is_extracted_from_forced_tool_arguments() {
        let checkpoint = json!({
            "claims": [],
            "consulted_guidance_refs": [],
            "rejected_hypotheses": [],
            "unresolved_questions": [],
            "inspected_entities": [],
            "coverage_gaps": [],
            "next_session_priorities": []
        });
        let message = json!({
            "tool_calls": [{
                "id": "checkpoint",
                "function": {
                    "name": "submit_investigation_checkpoint",
                    "arguments": checkpoint.to_string()
                }
            }]
        });
        assert_eq!(extract_checkpoint(&message).unwrap(), checkpoint);
    }

    #[test]
    fn checkpoint_schema_bounds_local_model_serialization() {
        let schema = checkpoint_object_schema();
        assert_eq!(schema["properties"]["claims"]["maxItems"], 6);
        assert_eq!(
            schema["properties"]["claims"]["items"]["properties"]["evidence_summary"]["maxLength"],
            500
        );
    }

    #[test]
    fn tool_catalog_expands_by_investigation_round() {
        let full = local_tools_schema("test", true);
        let triage = tools_for_round(&full, 0);
        let investigation = tools_for_round(&full, 1);
        assert!(triage.as_array().unwrap().len() < investigation.as_array().unwrap().len());
        assert!(investigation.as_array().unwrap().len() <= full.as_array().unwrap().len());
        let has =
            |schema: &Value, name: &str| {
                schema.as_array().unwrap().iter().any(|tool| {
                    tool.pointer("/function/name").and_then(Value::as_str) == Some(name)
                })
            };
        assert!(has(&triage, "get_database_load_summary"));
        assert!(!has(&triage, "get_init_parameter"));
        assert!(has(&investigation, "get_init_parameter"));
    }

    #[test]
    fn continuation_overlap_is_not_duplicated() {
        assert_eq!(
            merge_continuation("alpha shared continuation", "shared continuation omega"),
            "alpha shared continuation omega"
        );
    }

    #[test]
    fn model_context_is_found_in_lm_studio_catalog_shapes() {
        let catalog = json!({
            "data": [
                {"id": "other", "max_context_length": 4096},
                {
                    "id": "google/gemma-4-26b-a4b",
                    "loaded_context_length": 116736,
                    "max_context_length": 262144
                }
            ]
        });
        assert_eq!(
            find_model_context_tokens(&catalog, "google/gemma-4-26b-a4b"),
            Some(116_736)
        );
    }

    #[test]
    fn guidance_parser_builds_subsection_catalog_and_exact_lookup() {
        let content = r#"============================================================
§1  WAIT EVENT DIAGNOSTIC PATTERNS
============================================================
§1.1  LOG FILE SYNC / LOG FILE PARALLEL WRITE
TRIGGER: log file sync is significant.
ACTION: Compare LGWR latency and commit volume.

§1.2  CURSOR PIN CONTENTION
TRIGGER: cursor: pin S wait on X is significant.
ACTION: Inspect parsing and invalidation.
"#;
        let library = GuidanceLibrary::from_text(PathBuf::from("reasonings.txt"), content);
        assert_eq!(library.sections.len(), 2);
        assert!(library.catalog().contains("§1.1 LOG FILE SYNC"));

        let result = library.query(&json!({"topic": "1.1", "max_sections": 1}), 8_000);
        assert_eq!(result["matches"][0]["section_id"], "§1.1");
        assert_eq!(result["methodology_only"], true);
    }

    #[test]
    fn guidance_search_routes_concrete_oracle_symptom() {
        let content = r#"§1.1 LOG FILE SYNC
TRIGGER: log file sync is significant. Compare LGWR latency.
§1.2 ROW LOCK CONTENTION
TRIGGER: enq: TX - row lock contention is significant.
"#;
        let library = GuidanceLibrary::from_text(PathBuf::from("reasonings.txt"), content);
        let result = library.query(&json!({"topic": "enq TX row lock"}), 8_000);
        assert_eq!(result["matches"][0]["section_id"], "§1.2");
    }

    #[test]
    fn guidance_search_does_not_treat_log_as_logon() {
        let content = r#"§1.1 LOG FILE SYNC
TRIGGER: log file sync. Compare redo and commit behavior.
§3.2 LOGON STORMS
TRIGGER: user logons and connection creation spike.
"#;
        let library = GuidanceLibrary::from_text(PathBuf::from("reasonings.txt"), content);
        let result = library.query(
            &json!({"topic": "log redo commit", "max_sections": 2}),
            8_000,
        );
        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches[0]["section_id"], "§1.1");
        assert!(matches.iter().all(|entry| entry["section_id"] != "§3.2"));
    }

    #[test]
    fn configured_guidance_file_is_routable_when_present() {
        let path = reasonings_path();
        if !path.is_file() {
            return;
        }
        let content = fs::read_to_string(&path).unwrap();
        let library = GuidanceLibrary::from_text(path, &content);
        assert!(library.sections.len() >= 11);
        let result = library.query(
            &json!({"topic": "log file sync", "max_sections": 1}),
            16_384,
        );
        assert_eq!(result["matches"][0]["section_id"], "§1.1");
        assert!(result["matches"][0]["text"]
            .as_str()
            .unwrap()
            .contains("EXCESSIVE COMMIT FREQUENCY"));
    }

    #[test]
    fn guidance_tool_is_only_advertised_when_library_exists() {
        let with_guidance = local_tools_schema("test", true);
        let without_guidance = local_tools_schema("test", false);
        let has_tool = |schema: &Value| {
            schema.as_array().is_some_and(|tools| {
                tools.iter().any(|tool| {
                    tool.pointer("/function/name")
                        == Some(&Value::String("get_diagnostic_guidance".to_string()))
                })
            })
        };
        assert!(has_tool(&with_guidance));
        assert!(!has_tool(&without_guidance));
    }

    #[test]
    fn guidance_tool_result_is_not_recorded_as_database_evidence() {
        let library = GuidanceLibrary::from_text(
            PathBuf::from("reasonings.txt"),
            "§8.2 STORAGE QUALITY ASSESSMENT\nTRIGGER: Verify I/O latency.\n",
        );
        let collection = AWRSCollection {
            db_instance_information: DBInstance::default(),
            initialization_parameters: HashMap::new(),
            awrs: Vec::new(),
            sql_text: HashMap::new(),
        };
        let mut store = EvidenceStore::default();
        let output = store.execute(
            1,
            "get_diagnostic_guidance",
            &json!({"topic": "§8.2", "max_sections": 1}),
            &ReportForAI::default(),
            &collection,
            &library,
            "test",
            32_768,
            16_384,
        );
        let output: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["guidance_ref"], "S1-G0001");
        assert_eq!(output["not_evidence"], true);
        assert!(output.get("evidence_id").is_none());
        assert!(store.records.is_empty());
        assert_eq!(store.guidance_records.len(), 1);
    }

    #[test]
    fn duplicate_tool_call_in_one_session_reuses_short_reference() {
        let collection = AWRSCollection {
            db_instance_information: DBInstance::default(),
            initialization_parameters: HashMap::new(),
            awrs: Vec::new(),
            sql_text: HashMap::new(),
        };
        let mut store = EvidenceStore::default();
        let first = store.execute(
            1,
            "get_db_instance_info",
            &json!({}),
            &ReportForAI::default(),
            &collection,
            &GuidanceLibrary::default(),
            "test",
            32_768,
            8_192,
        );
        let second = store.execute(
            1,
            "get_db_instance_info",
            &json!({}),
            &ReportForAI::default(),
            &collection,
            &GuidanceLibrary::default(),
            "test",
            32_768,
            8_192,
        );
        let first: Value = serde_json::from_str(&first).unwrap();
        let second: Value = serde_json::from_str(&second).unwrap();
        assert_eq!(first["evidence_id"], second["evidence_id"]);
        assert_eq!(second["result_omitted"], true);
        assert_eq!(store.records.len(), 1);
    }

    #[test]
    fn closing_context_bounds_each_evidence_result() {
        let mut store = EvidenceStore::default();
        for index in 0..8 {
            store.records.push(EvidenceRecord {
                evidence_id: format!("S2-E{index:04}"),
                session: 2,
                tool_name: "test_tool".to_string(),
                arguments: json!({"index": index}),
                cached: false,
                result: json!({"large": "x".repeat(20_000)}),
            });
        }
        let cfg = LocalAgentConfig {
            model: "test".to_string(),
            language: "EN".to_string(),
            context_tokens: 32_768,
            max_tool_iterations: 2,
            max_tool_result_chars: 8_192,
            temperature: 0.0,
            top_p: 1.0,
            top_k: 20,
            tool_output_tokens: 2_048,
            checkpoint_output_tokens: 2_048,
            final_output_tokens: 4_096,
            token_estimate_safety_factor: 2.0,
            max_guidance_chars: 4_096,
        };
        let messages =
            build_compact_closing_messages("system", "seed", 2, &store, &json!({}), "close", &cfg);
        let serialized = serde_json::to_string(&messages).unwrap();
        assert!(serialized.len() < 40_000);
        assert!(serialized.contains("truncated"));
    }

    #[test]
    fn mandatory_evidence_inspects_relevant_parameters_and_sql_attachment_catalogs() {
        let collection = AWRSCollection {
            db_instance_information: DBInstance::default(),
            initialization_parameters: HashMap::from([(
                "session_cached_cursors".to_string(),
                "100".to_string(),
            )]),
            awrs: Vec::new(),
            sql_text: HashMap::new(),
        };
        let mut store = EvidenceStore::default();
        store.records.push(EvidenceRecord {
            evidence_id: SEED_EVIDENCE_ID.to_string(),
            session: 0,
            tool_name: "initial_case_seed".to_string(),
            arguments: json!({}),
            cached: false,
            result: json!({"symptom": "cursor: pin S wait on X"}),
        });
        let tools = json!([
            {"type":"function","function":{"name":"get_init_parameter","parameters":{"type":"object"}}},
            {"type":"function","function":{"name":"list_available_sql_plans","parameters":{"type":"object"}}},
            {"type":"function","function":{"name":"list_available_child_cursor_reasons","parameters":{"type":"object"}}}
        ]);
        let cfg = LocalAgentConfig {
            model: "test".to_string(),
            language: "EN".to_string(),
            context_tokens: 32_768,
            max_tool_iterations: 2,
            max_tool_result_chars: 8_192,
            temperature: 0.0,
            top_p: 1.0,
            top_k: 20,
            tool_output_tokens: 2_048,
            checkpoint_output_tokens: 2_048,
            final_output_tokens: 4_096,
            token_estimate_safety_factor: 2.0,
            max_guidance_chars: 4_096,
        };
        let outputs = ensure_mandatory_evidence(
            2,
            &mut store,
            &ReportForAI::default(),
            &collection,
            &GuidanceLibrary::default(),
            "missing-test-attachments",
            &cfg,
            &tools,
        );
        assert_eq!(outputs.len(), 5);
        assert!(store
            .records
            .iter()
            .any(|record| { record.session == 2 && record.tool_name == "get_init_parameter" }));
        assert!(store.records.iter().any(|record| {
            record.session == 2 && record.tool_name == "list_available_sql_plans"
        }));
        assert!(store.records.iter().any(|record| {
            record.session == 2 && record.tool_name == "list_available_child_cursor_reasons"
        }));
        let sections = store
            .records
            .iter()
            .filter(|record| record.tool_name == "get_precomputed_analysis")
            .filter_map(|record| record.arguments["section"].as_str())
            .collect::<HashSet<_>>();
        assert!(sections.contains("io_summary"));
        assert!(sections.contains("load_profile_anomalies"));
    }

    #[test]
    fn coverage_distinguishes_available_but_uninspected_data() {
        let collection = AWRSCollection {
            db_instance_information: DBInstance::default(),
            initialization_parameters: HashMap::from([(
                "optimizer_mode".to_string(),
                "ALL_ROWS".to_string(),
            )]),
            awrs: Vec::new(),
            sql_text: HashMap::from([("abc".to_string(), "select 1".to_string())]),
        };
        let coverage = build_coverage_summary(1, &EvidenceStore::default(), &collection);
        assert_eq!(
            coverage["initialization_parameters"],
            "available_not_inspected"
        );
        assert_eq!(coverage["sql_text"], "available_not_inspected");
    }

    #[test]
    fn malformed_checkpoint_can_be_recovered_with_evidence_refs() {
        let mut store = EvidenceStore::default();
        store.records.push(EvidenceRecord {
            evidence_id: "S1-E0001".to_string(),
            session: 1,
            tool_name: "get_sql_text".to_string(),
            arguments: json!({"sql_id": "abc"}),
            cached: false,
            result: json!({"sql_text": "select 1 from dual"}),
        });
        store.guidance_records.push(GuidanceRecord {
            guidance_ref: "S1-G0001".to_string(),
            session: 1,
            arguments: json!({"topic": "§1.1"}),
            cached: false,
            section_ids: vec!["§1.1".to_string()],
            result: json!({"methodology_only": true}),
        });
        let checkpoint = recover_checkpoint("{\"claims\":[", &store, "first", "retry");
        assert_eq!(checkpoint["serialization_recovered"], true);
        assert_eq!(checkpoint["claims"][0]["evidence_refs"][0], "S1-E0001");
        assert_eq!(checkpoint["claims"][0]["guidance_refs"][0], "S1-G0001");
        assert_eq!(checkpoint["consulted_guidance_refs"][0], "S1-G0001");
        assert_eq!(checkpoint["raw_model_checkpoint_prefix"], "{\"claims\":[");
    }

    #[test]
    fn full_gradients_honors_requested_limit_instead_of_bootstrap_caps() {
        let classifications = (0..8)
            .map(|index| crate::reasonings::CrossModelClassification {
                event_name: format!("event-{index}"),
                classification: "CONFIRMED_BOTTLENECK_EN_COLLINEAR".to_string(),
                priority: 1,
                combined_impact: (100 - index) as f64,
                combined_peak_impact: (1000 - index) as f64,
                ..Default::default()
            })
            .collect();
        let report = ReportForAI {
            db_time_gradient_fg_wait_events: Some(crate::reasonings::DbTimeGradientSection {
                cross_model_classifications: classifications,
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = dispatch_precomputed_analysis(
            &json!({"section": "full_gradients", "limit": 7}),
            &report,
        );
        assert_eq!(
            result["data"]["db_time_foreground_wait_events"]["returned"]
                ["cross_model_classifications"],
            7
        );
        assert_eq!(
            result["data"]["db_time_foreground_wait_events"]["cross_model_classifications"]
                .as_array()
                .unwrap()
                .len(),
            7
        );
    }
}
