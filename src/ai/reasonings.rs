use crate::ai_tools::*;
use crate::awr::{
    load_awrs_collection_from_json_str, AWRSCollection, HostCPU, IOStats, LoadProfile, SQLCPUTime,
    SQLGets, SQLIOTime, SQLReads, SegmentStats, WaitEvents, AWR,
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
use std::collections::BTreeMap;
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

pub use crate::model::analysis::*;

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
    let mut json_file = args.json_file().to_string();
    if json_file.is_empty() {
        json_file = format!("{}.json", args.directory());
    }
    let s_json = fs::read_to_string(&json_file).expect(&format!("Can't read {}", json_file));
    load_awrs_collection_from_json_str(&s_json).expect("Wrong AWRSCollection JSON")
}

fn build_model_instructions(
    lang: &str,
    args: &crate::Args,
    events_sqls: &HashMap<&str, HashSet<String>>,
    stem: &str,
    tools_mode: bool,
) -> String {
    let mut spell = format!("{} {}\n\n{}", SPELL, lang, ACCESS_PATH_REASONING);

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

    spell.push_str("\n\n");
    spell.push_str(include_str!("../report/assets/report_writing.md"));
    spell
}

fn tools_mode_instructions(stem: &str) -> String {
    let attachments_dir = format!("{stem}_attachments");
    let aix_dir = format!("{attachments_dir}/AIX");
    let xplan_note = if Path::new(&attachments_dir).is_dir() {
        format!(
            "\nAvailable execution-plan attachment directory: `{}`. \
             If list_available_sql_plans is present, use it to discover SQL_IDs with plans. \
             If list_available_child_cursor_reasons is present, use it to discover TOP SQL_IDs \
             for which decoded V$SQL_SHARED_CURSOR.REASON evidence was collected.",
            attachments_dir
        )
    } else {
        String::new()
    };
    let alertlog_note = if Path::new(&attachments_dir).is_dir() {
        format!(
            "\nIf get_alertlog_errors is present, an alert.log-like file was found in `{}`. \
             Use it as additional evidence when the report mentions parse errors, ORA/TNS errors, incidents, warnings, failed operations, disconnects, redo/log allocation issues, or any symptom that may be explained by alert.log messages. \
             Query the narrowest relevant date range and request parse-error details when parse errors are suspected.",
            attachments_dir
        )
    } else {
        String::new()
    };
    let aix_note = if Path::new(&aix_dir).is_dir() {
        format!(
            "\nAIX OS attachment directory is available: `{}`. \
             If AIX tools are present, you MUST call get_db_instance_info and get_aix_cpu_entitlement_summary before deciding whether the system is CPU-bound. \
             On AIX LPARs, AWR Host CPU %CPU and DB CPU/DB Time can be misleading when Entc%/%entc is high. \
             If Entc%/ec is near saturation, do NOT dismiss CPU pressure because the LPAR is uncapped, because AWR Host CPU idle is nonzero, or because the shared pool has theoretical spare capacity. \
             If Entc%/physc/pc/EC/capped/shared-dedicated details are not available from tools, ask the user for those OS details and do not make a final CPU-bound classification.",
            aix_dir
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
         For every SQL_ID that materially contributes to DB Time, elapsed time, DB CPU, I/O time, buffer gets, physical reads, anomalous waits, or regression symptoms, call get_sql_text and get_sql_timeline. When a wait-to-SQL contributor tool is exposed, use it for every material foreground wait and preserve correlation versus direct ASH attribution separately. \
         If execution-plan tools are available, you are expected to use list_available_sql_plans and get_sql_execution_plan for important SQL_IDs before making SQL tuning recommendations. Classify BEGIN/DECLARE/CALL entry points as PL/SQL: a top-level row-source plan is not applicable, so profile the PL/SQL unit and inspect its inner SQL instead of requesting DBMS_XPLAN recapture. \
         If child-cursor reason tools are available, call list_available_child_cursor_reasons and get_child_cursor_reasons before explaining child cursor proliferation, version_count growth, parsing pressure, library cache or cursor mutex contention, optimizer/NLS/bind/authorization mismatches, or plan instability. Treat A/B values as comparison-vector sides, never chronological old/new values. \
         If alert.log tools are available, use get_alertlog_errors to verify error evidence for relevant date ranges, especially before dismissing parse errors or other reported failures as unrelated. \
         If AIX OS tools are available or get_db_instance_info reports an AIX platform, use get_aix_cpu_entitlement_summary before any CPU-bound conclusion; never rely only on %CPU, DB CPU, or DB CPU/DB Time on AIX. High Entc%/%entc/ec or physc/pc near entitlement is CPU entitlement/physical-capacity pressure even on uncapped LPARs and even when AWR Host CPU idle is nonzero. \
         When you fetch an applicable SQL execution plan, produce a dedicated analysis covering: dominant operations, access paths, join methods and join order, cardinality estimate errors, partition pruning, parallel execution, adaptive plan notes, temp spills/sorts, index usage, and concrete remediation options. For a PL/SQL entry point, report instrumentation and inner-SQL coverage instead of inventing plan findings. \
         Recommendations must be specific and evidence-based: statistics refresh, histograms, extended statistics, SQL rewrite, indexing, partitioning, SQL Plan Management baseline/profile, bind/literal handling, or application-side change. \
         Prefer multiple narrow tool calls over guessing. Stop calling tools only when you have enough evidence to produce the FINAL markdown report following the OUTPUT STRUCTURE.{}",
        format!("{xplan_note}{alertlog_note}{aix_note}")
    )
}

fn available_attachments_prompt(stem: &str) -> Option<String> {
    let attachments_dir = format!("{stem}_attachments");
    let aix_dir = format!("{attachments_dir}/AIX");
    let mut lines = Vec::new();

    if Path::new(&attachments_dir).is_dir() {
        lines.push("Execution plan attachments (*.xplan) may be available through tools. Use list_available_sql_plans and get_sql_execution_plan for important SQL_IDs before making SQL tuning recommendations.".to_string());
        lines.push("Decoded V$SQL_SHARED_CURSOR.REASON attachments (*.shared_cursor_reasons) may be available through tools. Use list_available_child_cursor_reasons and get_child_cursor_reasons before explaining child cursor proliferation, parsing pressure, cursor/library-cache contention, or plan instability.".to_string());
    }

    if Path::new(&aix_dir).is_dir() {
        lines.push("AIX OS attachments were found under the AIX subdirectory. If the database platform is AIX, use get_db_instance_info and get_aix_cpu_entitlement_summary before deciding whether the system is CPU-bound. Entc%/%entc/ec is mandatory evidence on shared/capped/uncapped LPARs; low %CPU, nonzero AWR Host CPU idle, uncapped mode, or shared-pool spare capacity do not clear CPU saturation when Entc% is high.".to_string());
    }

    if lines.is_empty() {
        None
    } else {
        Some(format!(
            "### AVAILABLE ATTACHMENTS\n{}\n-- END AVAILABLE ATTACHMENTS --",
            lines.join("\n")
        ))
    }
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
    if body.trim().is_empty() {
        let debug_path = openrouter_bad_response_path(response_file, context);
        let _ = fs::write(&debug_path, body.as_bytes());
        return Err(format!(
            "OpenRouter returned an empty or whitespace-only response during {context}. \
             Raw response saved to {debug_path} ({} chars).",
            body.chars().count()
        )
        .into());
    }

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

const OPENROUTER_REQUEST_ATTEMPTS: usize = 3;

fn openrouter_retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

async fn request_openrouter_json(
    client: &Client,
    api_key: &str,
    payload: &Value,
    response_file: &str,
    context: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut last_error = String::new();
    let mut attempts_made = 0;

    for attempt in 1..=OPENROUTER_REQUEST_ATTEMPTS {
        attempts_made = attempt;
        debug_note!(
            "OpenRouter request attempt: context='{}', attempt={}/{}, payload_bytes={}",
            context,
            attempt,
            OPENROUTER_REQUEST_ATTEMPTS,
            payload.to_string().len()
        );
        let (tx, rx) = oneshot::channel();
        let spinner = tokio::spawn(spinning_beer(rx));

        let response_result = client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("X-Title", "jas-min")
            .json(payload)
            .send()
            .await;

        let _ = tx.send(());
        let _ = spinner.await;

        let response = match response_result {
            Ok(response) => response,
            Err(error) => {
                last_error = format!("OpenRouter transport error during {context}: {error}");
                debug_note!(
                    "OpenRouter transport failure: context='{}', attempt={}, error={}",
                    context,
                    attempt,
                    error
                );
                if attempt < OPENROUTER_REQUEST_ATTEMPTS {
                    eprintln!(
                        "⚠️ {last_error}. Retrying ({}/{})...",
                        attempt + 1,
                        OPENROUTER_REQUEST_ATTEMPTS
                    );
                    sleep(Duration::from_secs(attempt as u64)).await;
                    continue;
                }
                break;
            }
        };

        let status = response.status();
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                last_error =
                    format!("Could not read OpenRouter response body during {context}: {error}");
                if attempt < OPENROUTER_REQUEST_ATTEMPTS {
                    eprintln!(
                        "⚠️ {last_error}. Retrying ({}/{})...",
                        attempt + 1,
                        OPENROUTER_REQUEST_ATTEMPTS
                    );
                    sleep(Duration::from_secs(attempt as u64)).await;
                    continue;
                }
                break;
            }
        };
        debug_note!(
            "OpenRouter response received: context='{}', attempt={}, status={}, body_bytes={}",
            context,
            attempt,
            status,
            body.len()
        );

        if !status.is_success() {
            last_error = format!(
                "OpenRouter returned HTTP {status} during {context}: {}",
                body.trim()
            );
            if openrouter_retryable_status(status) && attempt < OPENROUTER_REQUEST_ATTEMPTS {
                eprintln!(
                    "⚠️ {last_error}. Retrying ({}/{})...",
                    attempt + 1,
                    OPENROUTER_REQUEST_ATTEMPTS
                );
                sleep(Duration::from_secs(attempt as u64)).await;
                continue;
            }
            break;
        }

        let diagnostic_context = if attempt == OPENROUTER_REQUEST_ATTEMPTS {
            context.to_string()
        } else {
            format!("{context}.attempt_{attempt}")
        };

        match parse_openrouter_response_json(&body, response_file, &diagnostic_context) {
            Ok(json) => {
                debug_note!(
                    "OpenRouter response parsed: context='{}', attempt={}",
                    context,
                    attempt
                );
                return Ok(json);
            }
            Err(error) => {
                last_error = error.to_string();
                if attempt < OPENROUTER_REQUEST_ATTEMPTS {
                    eprintln!(
                        "⚠️ {last_error} Retrying ({}/{})...",
                        attempt + 1,
                        OPENROUTER_REQUEST_ATTEMPTS
                    );
                    sleep(Duration::from_secs(attempt as u64)).await;
                    continue;
                }
            }
        }
    }

    debug_note!(
        "OpenRouter request exhausted retries: context='{}', attempts={}, last_error={}",
        context,
        attempts_made,
        last_error
    );
    Err(format!(
        "OpenRouter request failed during {context} after {attempts_made} attempt(s): {last_error}"
    )
    .into())
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
            .filter_map(|(msg_idx, content)| {
                let parts = content.get("parts")?.as_array()?;
                let mut max_part: Option<(usize, usize)> = None; // (part_idx, len)
                for (part_idx, part) in parts.iter().enumerate() {
                    if let Some(response) = part.pointer("/functionResponse/response") {
                        let len = serde_json::to_string(response)
                            .map(|s| s.chars().count())
                            .unwrap_or(0);
                        if max_part.is_none() || len > max_part.unwrap().1 {
                            max_part = Some((part_idx, len));
                        }
                    }
                }
                max_part.map(|(part_idx, len)| (msg_idx, part_idx, len))
            })
            .max_by_key(|&(_, _, len)| len);

        let Some((msg_idx, part_idx, len)) = largest_tool_response else {
            break;
        };

        if len <= 2048 {
            break;
        }

        let original = contents[msg_idx]
            .pointer(&format!("/parts/{}/functionResponse/response", part_idx))
            .and_then(|v| serde_json::to_string(v).ok())
            .unwrap_or_default();
        let prefix: String = original.chars().take(2048).collect();

        if let Some(response) =
            contents[msg_idx].pointer_mut(&format!("/parts/{}/functionResponse/response", part_idx))
        {
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

/// Keep independent TOP unions in the classic API prompt without copying every
/// dense fit four times. MCP/local tools retain the original ReportForAI object.
/// The CLI persists the complete value separately for coefficient inspection.
pub fn gradient_prompt_value(report: &ReportForAI) -> Value {
    let mut value = serde_json::to_value(report).unwrap();
    for section in value.as_object_mut().unwrap().values_mut() {
        let Some(object) = section.as_object_mut() else {
            continue;
        };
        if !object.contains_key("model_rankings") {
            continue;
        }
        let mut names = HashSet::new();
        for key in [
            "ridge_top",
            "elastic_net_top",
            "huber_top",
            "quantile95_top",
        ] {
            if let Some(rows) = object.get(key).and_then(Value::as_array) {
                names.extend(
                    rows.iter()
                        .filter_map(|r| r["event_name"].as_str().map(str::to_string)),
                );
            }
        }
        let counts = object["model_rankings"]
            .as_object()
            .map(|models| {
                models
                    .iter()
                    .map(|(name, rows)| (name.clone(), json!(rows.as_array().map_or(0, Vec::len))))
                    .collect::<serde_json::Map<_, _>>()
            })
            .unwrap_or_default();
        object.remove("model_rankings");
        object.insert("full_fit_counts".into(), json!(counts));
        object.insert(
            "full_fit_source".into(),
            json!("report_for_ai.full.json; prompt retains independent TOP unions only"),
        );
        if let Some(rows) = object
            .get_mut("predictor_coverage")
            .and_then(Value::as_array_mut)
        {
            rows.retain(|r| r["event_name"].as_str().is_some_and(|n| names.contains(n)));
        }
    }
    value
}

pub(crate) const ACCESS_PATH_REASONING: &str = include_str!("access_path_reasoning.md");

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
  When present, these names establish only which tables appear in the collected SQL text.
  Treat them as candidates and require aligned runtime evidence before attributing a wait or
  segment mechanism. Cross-reference them with segment statistics sections.
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
- `db_load_sources` — per-target snapshot counts for Time Model, Load Profile fallback and unavailable values.
- `performance_hints` — deterministic work-growth signals with independent CPU/elapsed support. Inspect signal_kind, time_impact_status, comparisons[].cost_evaluations, trajectory and episode_status. Work-only signals remain useful: nullable cost means no supported time comparison, never zero cost. Global continuation is instance context, not SQL/segment attribution. Physical mechanisms remain alternative explanations, including wider rows and necessary continuation. Persistent-cost observations use absolute observed_metrics/observed_segments with null baseline/comparison and establish neither growth nor stability. In JAS-only mode unresolved physical cause is a complete, valid conclusion; extra database measurements are optional confirmation.
  DB Time/DB CPU rates prefer Time Model seconds divided by actual wall seconds. This avoids
  rounded Load Profile targets. Raw snapshot Load Profile rows retain their collected values.
- `db_time_degradation_report` — baseline-vs-recent statistical degradation report for DB Time.
  `change_score` is dimensionless and ranked only within its domain. `unit` describes each delta.
  Never sum deltas from different metrics, estimate their DB Time share, or call them savings.
  Regression interval totals are normalized by actual wall seconds (gauges retain levels).
  Missing or invalid exposure excludes rate predictors; missing host CPU is unknown.
  Use it to state whether the latest snapshots statistically departed from the prior baseline,
  and to list the SQL IDs, wait events, instance statistics, time-model metrics, and load-profile
  counters that increased together with DB Time.
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

Compare DB Time and DB CPU fits with actual SQL CPU/elapsed measurements and source coverage.
Model-list membership cannot classify a SQL as CPU-bound or wait-bound by itself.

Each section has Ridge, Elastic Net, Huber and Quantile 95 fits. Q95 estimates the conditional
0.95 quantile of target DELTAS using all observations, not a regression on the largest 5% of snapshots.
Gradient v2 fits an unpenalized Q95 intercept and standardizes the target before minimizing
mean pinball loss + lambda/2 * squared coefficient norm. Coefficients and intercept are restored to
original target units. Read `settings.quantile95` for method, lambda, iterations, objective, primal/
dual residuals and convergence. Unconverged Q95 coefficients are diagnostic only and excluded from
TOP selection and cross-model agreement. Old sections without these diagnostics are unverified.

Elastic Net retains its per-family automatic lambda selection: standardized predictor/target deltas,
forward-chaining cross-validation and the one-standard-error rule, unless explicitly overridden.
Read its lambda mode, lambda/lambda_max, CV rule and nonzero count before interpreting omissions.

### Independent ranking dimensions

- `gradient_coef`: original target units per one predictor-delta standard deviation. Positive is an
  association, not proof of a cause; negative and zero coefficients remain available in full fits.
- `impact_active` = abs(coef / stddev(delta_x)) * P90(abs(delta_x)). This percentile INCLUDES zeros.
  Rare severe events can have active impact zero. It is not a conditional-on-activity percentile.
- `impact_peak` uses P99(abs(delta_x)); it is not the maximum. Even P99 can be zero for very rare events.
- `impact_extreme` uses max(abs(delta_x)); inspect the named transition and coverage before interpreting it.
- `impact` uses MAD(delta_x), a typical-variability comparison which can also be zero for bursts.
- `impact_share` is a normalized positive active-magnitude share, NOT explained variance, DB Time share,
  causal attribution, or recoverable CPU. These model magnitudes are not additive resource costs.

Gradient v2 preserves every coefficient in `model_rankings`. The four legacy `*_top` arrays are now
unions of independent positive TOP active/P90, peak/P99 and extreme/max rankings. Read `selection_reasons`
and the three ranks; zero magnitudes do not occupy positive ranking slots. No signal needs positive
P90 to appear in peak or extreme selection. In MCP/local tools, `get_precomputed_analysis` with
section=full_gradients accepts family, contributor (exact SQL_ID/event/statistic), ranking and offset;
use it to distinguish outside-TOP from a fitted zero, a negative coefficient or an absent predictor.
The seed is only a bounded preview. Full source fits remain available through paginated queries.

`predictor_coverage` retains nonzero-delta counts, percentile values and the extreme transition index.
For SQL top lists it also records observed samples, observed zeros, missing samples and pairs with
both endpoints observed. The fit uses a ZERO-FILLED RETAINED-WORK PROXY where rows are absent; these
are not measurements of zero work. Deltas at top-list entry/exit can reflect selection censoring.
Do not interpret those coefficients as an unbiased estimate of complete SQL workload. Verify peaks
against observed SQL rows and aligned DB Time/waits. A missing observation mask means unknown coverage.

### Cross-model synthesis

`in_ridge`, `in_elastic_net`, `in_huber`, `in_quantile95` indicate membership in the independent TOP
union, not coefficient existence. Models share inputs; agreement is not independent causal evidence.
Legacy classification codes (including CONFIRMED_BOTTLENECK and CONFIRMED_BOTTLENECK_EN_COLLINEAR)
remain compatibility labels for selection patterns. They do not prove a bottleneck, collinearity,
incident severity or a specific reason for an omitted Elastic Net coefficient. Use the accompanying
plain-language description and coefficient/coverage lookup. Never assign CRITICAL merely from a label.
`combined_impact` and `combined_peak_impact` sum positive magnitudes from complete eligible fits for
each selected candidate. They are comparison scores, not additive DB Time or potential savings.

VIF and collinear-group fits identify unstable attribution, not true causal group cost. Inspect
correlated features, source overlap and actual incident windows before choosing an intervention.
A group sum can mix units or overlapping work and must not become a resource-accounting claim.

For each family compare recurring work, P99 tails and observed extremes. State what changed together,
where/when it happened, an alternative explanation and a discriminating runtime test. Integrate SQL,
wait, CPU and anomaly evidence without using a model score as proof of the mechanism.

# ANALYTICAL METHODOLOGY

Follow this reasoning sequence:

## Step 1: Establish Performance Profile
- Interpret DB CPU / DB Time ratio across all spikes (< 0.66 = wait-bound, ~1.0 = CPU-bound)
- Assess ratio variance for mixed/intermittent problems
- AIX caveat: if the platform is AIX, do not decide CPU-bound from DB CPU/DB Time or AWR Host CPU %CPU alone. Entc%/%entc/ec, physc/pc, EC, capped/uncapped and shared/dedicated LPAR data are required; if Entc%/ec is high, classify it as CPU entitlement/physical-capacity pressure even when AWR idle is nonzero or the LPAR is uncapped.

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
    debug_note!(
        "Starting Gemini analysis: model='{}', language='{}', mode={}, report_chars={}",
        vendor_model_lang.get(1).copied().unwrap_or(""),
        vendor_model_lang.get(2).copied().unwrap_or(""),
        mode_label,
        report_for_ai.len()
    );
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

    if tools_mode {
        if let Some(note) = available_attachments_prompt(stem) {
            initial_parts.push(json!({
                "text": note
            }));
        }
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
        tools_schema_for_gemini(
            stem,
            collection
                .as_ref()
                .is_some_and(|value| value.nmon.is_some()),
        )
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

        debug_note!(
            "Gemini request prepared: payload_bytes={}, estimated_tokens={}",
            payload.to_string().len(),
            estimate_tokens_from_value(&payload)
        );

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

                let mut responses = Vec::new();
                for tc in tool_calls {
                    let fn_name = tc
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let parsed_args = tc.get("args").cloned().unwrap_or_else(|| json!({}));

                    println!("🛠  Gemini tool call: {}({})", fn_name, parsed_args);
                    debug_note!("Gemini requested diagnostic tool: name='{}'", fn_name);

                    let result_text = dispatch_tool_call(
                        &fn_name,
                        &parsed_args,
                        collection.as_ref().unwrap(),
                        stem,
                    );
                    let result_json: Value = serde_json::from_str(&result_text)
                        .unwrap_or_else(|_| json!({ "result": result_text }));

                    responses.push(json!({
                        "functionResponse": {
                            "name": fn_name,
                            "response": result_json
                        }
                    }));
                }

                contents.push(json!({
                    "role": "user",
                    "parts": responses
                }));

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
        debug_note!("Gemini analysis completed without extractable content");
        fs::write(&response_file, final_content.as_bytes())?;
        return Err("Gemini response had no extractable final report; no HTML generated".into());
    } else {
        fs::write(&response_file, final_content.as_bytes())?;
        let final_content = crate::report_issues::finalize_api_markdown(&final_content)?;
        fs::write(&response_file, final_content.as_bytes())?;
        debug_note!(
            "Gemini analysis output written: path='{}', bytes={}",
            response_file,
            final_content.len()
        );
        println!("🍻 Gemini response written to file: {}", &response_file);
        convert_md_to_html_file(&response_file, events_sqls.clone())?;
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
    debug_note!(
        "Starting OpenRouter analysis: model='{}', language='{}', mode={}, report_chars={}",
        vendor_model_lang.get(1).copied().unwrap_or(""),
        vendor_model_lang.get(2).copied().unwrap_or(""),
        mode_label,
        report_for_ai.len()
    );
    println!(
        "=== Consulting OpenRouter ({}) model: {} ===",
        mode_label, vendor_model_lang[1]
    );

    let api_key = env::var("OPENROUTER_API_KEY")
        .map_err(|_| "You have to set OPENROUTER_API_KEY env variable")?;

    let stem = stem_from_logfile(logfile_name);
    let load_profile = load_profile_for_stem(stem);

    let model_name = vendor_model_lang[1].replace("/", "_");
    let suffix = if tools_mode { "_tools" } else { "" };
    let response_file = format!("{}_{}{}.md", logfile_name, model_name, suffix);
    let client = Client::new();

    let spell =
        build_model_instructions(vendor_model_lang[2], args, &events_sqls, stem, tools_mode);

    // --- common - history begins ---
    let attachment_note = if tools_mode {
        available_attachments_prompt(stem)
            .map(|note| format!("\n\n{note}"))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let mut messages: Vec<Value> = vec![
        json!({ "role": "system", "content": format!("### SYSTEM INSTRUCTIONS\n{}", spell) }),
        json!({ "role": "user", "content": format!(
            "MAIN REPORT (toon/json-as-text):\n```\n{}\n```\n\nGLOBAL PROFILE:\n```json\n{}\n```{}",
            report_for_ai, load_profile, attachment_note
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
        tools_schema(
            stem,
            collection
                .as_ref()
                .is_some_and(|value| value.nmon.is_some()),
        )
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

                let final_json = request_openrouter_json(
                    &client,
                    &api_key,
                    &final_payload,
                    &response_file,
                    "final_synthesis",
                )
                .await?;

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

        debug_note!(
            "OpenRouter request prepared: payload_bytes={}, estimated_tokens={}",
            payload.to_string().len(),
            estimate_tokens_from_value(&payload)
        );

        let json =
            request_openrouter_json(&client, &api_key, &payload, &response_file, "tool_loop")
                .await?;

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
                        debug_note!("OpenRouter requested diagnostic tool: name='{}'", fn_name);

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

                        debug_note!(
                            "OpenRouter final synthesis prepared: payload_bytes={}, estimated_tokens={}",
                            final_payload.to_string().len(),
                            final_payload_tokens
                        );

                        let final_json = request_openrouter_json(
                            &client,
                            &api_key,
                            &final_payload,
                            &response_file,
                            "final_synthesis",
                        )
                        .await?;

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
    let final_content = crate::report_issues::finalize_api_markdown(&final_content)?;
    fs::write(&response_file, final_content.as_bytes())?;
    debug_note!(
        "OpenRouter analysis output written: path='{}', bytes={}, finish_reason='{}'",
        response_file,
        final_content.len(),
        last_finish
    );
    println!("🍻 OpenRouter response written to file: {}", &response_file);
    convert_md_to_html_file(&response_file, events_sqls.clone())?;
    println!(
        "Total tokens: {}\nFinish reason: {}\n",
        last_usage, last_finish
    );

    Ok(())
}

fn tools_schema_for_openai_responses(stem: &str, include_nmon: bool) -> Value {
    let tools = tools_schema(stem, include_nmon);

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

fn tools_schema_for_gemini(stem: &str, include_nmon: bool) -> Value {
    let tools = tools_schema(stem, include_nmon);

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
    debug_note!(
        "Starting OpenAI analysis: model='{}', language='{}', mode={}, report_chars={}",
        vendor_model_lang.get(1).copied().unwrap_or(""),
        vendor_model_lang.get(2).copied().unwrap_or(""),
        mode_label,
        report_for_ai.len()
    );
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

    if tools_mode {
        if let Some(note) = available_attachments_prompt(stem) {
            report_payload.push(json!({
                "type": "input_text",
                "text": note
            }));
        }
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
        tools_schema_for_openai_responses(
            stem,
            collection
                .as_ref()
                .is_some_and(|value| value.nmon.is_some()),
        )
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
        debug_note!(
            "OpenAI Responses request prepared: payload_bytes={}, estimated_tokens={}",
            payload.to_string().len(),
            estimated_payload_tokens
        );

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
                    debug_note!("OpenAI requested diagnostic tool: name='{}'", fn_name);

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
                        "OpenAI final synthesis prepared: payload_bytes={}, estimated_tokens={}",
                        final_payload.to_string().len(),
                        final_payload_tokens
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
        debug_note!("OpenAI analysis completed without final content");
        fs::write(&response_file, final_content.as_bytes())?;
        return Err("OpenAI response had no extractable final report; no HTML generated".into());
    } else {
        fs::write(&response_file, final_content.as_bytes())?;
        let final_content = crate::report_issues::finalize_api_markdown(&final_content)?;
        fs::write(&response_file, final_content.as_bytes())?;
        debug_note!(
            "OpenAI analysis output written: path='{}', bytes={}",
            response_file,
            final_content.len()
        );
        println!("🧠 OpenAI response written to file: {}", &response_file);
        convert_md_to_html_file(&response_file, events_sqls)?;
        println!("Total tokens (OpenAI): {}", last_usage);
        println!("Finish reason: {}", last_finish);
    }

    Ok(())
}

#[cfg(test)]
mod openrouter_response_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn classic_api_receives_hypothesis_policy_with_and_without_tools() {
        use clap::Parser;
        let args = crate::Args::parse_from(["jas-min"]);
        for tools_mode in [false, true] {
            let prompt =
                build_model_instructions("EN", &args, &HashMap::new(), "unused", tools_mode);
            assert!(prompt.contains(ACCESS_PATH_REASONING));
            assert!(!prompt.contains("mandatory evidence gate for empty-block"));
            assert!(!prompt.contains("- `access_path_diagnostics`"));
        }
    }

    #[test]
    fn classic_api_context_preserves_peak_union_without_duplicating_full_fits() {
        let rare = GradientTopItem {
            event_name: "rare".into(),
            impact_active: 0.0,
            impact_peak: 60.0,
            selection_reasons: vec!["peak_p99".into()],
            ..Default::default()
        };
        let report = ReportForAI {
            db_time_gradient_sql_elapsed_time: Some(DbTimeGradientSection {
                ridge_top: vec![rare.clone()],
                model_rankings: BTreeMap::from([("ridge".into(), vec![rare])]),
                predictor_coverage: vec![
                    GradientCoverage {
                        event_name: "rare".into(),
                        missing_samples: Some(90),
                        ..Default::default()
                    },
                    GradientCoverage {
                        event_name: "other".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            ..Default::default()
        };
        let context = gradient_prompt_value(&report);
        let section = &context["db_time_gradient_sql_elapsed_time"];
        assert!(section.get("model_rankings").is_none());
        assert_eq!(section["full_fit_counts"]["ridge"], 1);
        assert_eq!(section["ridge_top"][0]["event_name"], "rare");
        assert_eq!(section["predictor_coverage"].as_array().unwrap().len(), 1);
        assert_eq!(section["predictor_coverage"][0]["missing_samples"], 90);
        assert_eq!(
            report
                .db_time_gradient_sql_elapsed_time
                .unwrap()
                .model_rankings["ridge"]
                .len(),
            1
        );
    }

    #[test]
    fn whitespace_only_openrouter_response_is_identified_and_saved() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let response_file = std::env::temp_dir().join(format!(
            "jas-min-openrouter-response-{}-{unique}.md",
            std::process::id()
        ));
        let response_file = response_file.to_string_lossy().into_owned();
        let body = "\n         \n\n       \n";

        let error = parse_openrouter_response_json(body, &response_file, "tool_loop").unwrap_err();
        let debug_path = openrouter_bad_response_path(&response_file, "tool_loop");

        assert!(error
            .to_string()
            .contains("empty or whitespace-only response"));
        assert_eq!(fs::read_to_string(&debug_path).unwrap(), body);

        let _ = fs::remove_file(debug_path);
    }

    #[test]
    fn valid_openrouter_response_json_is_accepted() {
        let parsed = parse_openrouter_response_json(
            r#"{"choices":[{"message":{"content":"ok"}}]}"#,
            "unused.md",
            "tool_loop",
        )
        .unwrap();

        assert_eq!(parsed["choices"][0]["message"]["content"], "ok");
    }

    #[test]
    fn openrouter_retries_only_transient_http_statuses() {
        assert!(openrouter_retryable_status(
            reqwest::StatusCode::REQUEST_TIMEOUT
        ));
        assert!(openrouter_retryable_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(openrouter_retryable_status(
            reqwest::StatusCode::BAD_GATEWAY
        ));
        assert!(!openrouter_retryable_status(
            reqwest::StatusCode::BAD_REQUEST
        ));
        assert!(!openrouter_retryable_status(
            reqwest::StatusCode::UNAUTHORIZED
        ));
    }
}
