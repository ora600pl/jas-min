// ============================================================================
// jasmin_tools.rs
// ----------------------------------------------------------------------------
// Tool-calling support for JAS-MIN's OpenRouter integration.
//
// This module exposes:
//   * `jasmin_tools_schema()`  — OpenAI/OpenRouter-compatible tool schema
//   * `dispatch_tool_call()`   — executes a tool call against an AWRSCollection
//
// The toolset is organized into four conceptual categories:
//
//   1. POINT LOOKUPS       — fetch a specific object (snapshot, SQL text, parameter)
//   2. AGGREGATIONS        — top-N rankings within a snapshot or globally
//   3. SEARCH              — find patterns (event in snapshots, SQL by module, etc.)
//   4. TIME-SERIES/COMPARE — evolution over time, snapshot diffs
//   5. META                — discovery helpers ("what metrics are available?")
//
// All tools return JSON-serializable values. Errors are returned as
// `{ "error": "<message>" }` rather than panicking, so the model can recover.
// ============================================================================

use std::collections::{HashMap, HashSet};
use serde_json::{json, Value};

use crate::awr::{AWR, AWRSCollection};

// ----------------------------------------------------------------------------
// Tool schema (sent to the LLM)
// ----------------------------------------------------------------------------

/// Returns the full tool catalog in OpenAI/OpenRouter function-calling format.
/// The LLM uses these descriptions to decide which tool to call and with which
/// arguments. Keep descriptions concise but explicit about when each tool is
/// the right choice.
pub fn jasmin_tools_schema() -> Value {
    json!([
        // ====================================================================
        // 1. POINT LOOKUPS — fetch a specific object
        // ====================================================================
        {
            "type": "function",
            "function": {
                "name": "get_snapshot_details",
                "description": "Returns the FULL detailed AWR/STATSPACK data for one snapshot: \
                                load profile, wait events (FG/BG with histograms), all SQL sections, \
                                instance stats, latches, segments, I/O stats, time model, host CPU. \
                                Use this when you need to drill into one specific anomalous period.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "snap_id": {
                            "type": "integer",
                            "description": "begin_snap_id of the target snapshot"
                        },
                        "sections": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": [
                                    "load_profile", "instance_efficiency", "host_cpu",
                                    "time_model", "foreground_waits", "background_waits",
                                    "sql_elapsed", "sql_cpu", "sql_io", "sql_gets", "sql_reads",
                                    "top_sql_with_top_events", "instance_stats",
                                    "io_stats_byfunc", "dictionary_cache", "library_cache",
                                    "latch_activity", "segment_stats", "redo_log"
                                ]
                            },
                            "description": "Optional: limit the output to selected sections. \
                                            Omit this argument to receive ALL sections."
                        }
                    },
                    "required": ["snap_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_sql_text",
                "description": "Returns the full SQL text for a given SQL_ID (when available). \
                                Use this when you need to understand the actual query behind a \
                                problematic SQL_ID — identify tables, joins, predicates, anti-patterns.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "sql_id": { "type": "string" }
                    },
                    "required": ["sql_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_init_parameter",
                "description": "Returns values of one or more Oracle initialization parameters. \
                                Use this to verify exact values of parameters you suspect are misconfigured.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "names": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Explicit list of parameter names (underscore params supported)"
                        },
                        "pattern": {
                            "type": "string",
                            "description": "Optional case-insensitive substring filter, e.g. 'optimizer_'"
                        }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_db_instance_info",
                "description": "Returns DB instance hardware/version metadata: db_id, release, RAC mode, \
                                platform, CPUs/cores/sockets, memory, db_block_size. Useful for sizing reasoning.",
                "parameters": { "type": "object", "properties": {} }
            }
        },

        // ====================================================================
        // 2. AGGREGATIONS — top-N within a snapshot
        // ====================================================================
        {
            "type": "function",
            "function": {
                "name": "list_snapshots",
                "description": "Lists snapshots in a range with DB Time, DB CPU, host CPU idle % and the \
                                top foreground wait event. Use this to scan a date range, find quiet \
                                baseline snapshots, or inspect neighbors of an anomaly.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "from_snap_id": { "type": "integer" },
                        "to_snap_id":   { "type": "integer" },
                        "limit":        { "type": "integer", "description": "Max snapshots returned (default 50)" }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "top_sqls_in_snapshot",
                "description": "Returns top-N SQLs in a snapshot ranked by a chosen metric.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "snap_id": { "type": "integer" },
                        "metric":  {
                            "type": "string",
                            "enum": ["elapsed_time", "cpu_time", "io_time", "buffer_gets", "physical_reads"]
                        },
                        "top_n": { "type": "integer", "description": "default 10" }
                    },
                    "required": ["snap_id", "metric"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "top_wait_events_in_snapshot",
                "description": "Returns top-N foreground or background wait events in a snapshot with their \
                                millisecond-bucket histograms.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "snap_id": { "type": "integer" },
                        "kind":    { "type": "string", "enum": ["foreground", "background"] },
                        "top_n":   { "type": "integer", "description": "default 10" },
                        "rank_by": {
                            "type": "string",
                            "enum": ["pct_dbtime", "total_wait_time_s", "avg_wait", "waits"],
                            "description": "default 'pct_dbtime'"
                        }
                    },
                    "required": ["snap_id", "kind"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "top_segments_in_snapshot",
                "description": "Returns top segments in a snapshot for a chosen statistic category.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "snap_id":  { "type": "integer" },
                        "category": {
                            "type": "string",
                            "enum": [
                                "Logical Reads", "Physical Reads", "Physical Read Requests",
                                "Direct Physical Reads", "Physical Writes", "Physical Write Requests",
                                "Direct Physical Writes", "Row Lock Waits",
                                "Buffer Busy Waits", "Global Cache Buffer Busy"
                            ]
                        },
                        "top_n": { "type": "integer", "description": "default 10" }
                    },
                    "required": ["snap_id", "category"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "top_latches_in_snapshot",
                "description": "Returns top latches in a snapshot ranked by wait_time, pct_miss, or get_requests.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "snap_id": { "type": "integer" },
                        "rank_by": { "type": "string", "enum": ["wait_time", "pct_miss", "get_requests"] },
                        "top_n":   { "type": "integer", "description": "default 10" }
                    },
                    "required": ["snap_id"]
                }
            }
        },

        // ====================================================================
        // 3. SEARCH — discover patterns
        // ====================================================================
        {
            "type": "function",
            "function": {
                "name": "search_sql_text",
                "description": "Searches collected SQL text for a case-insensitive substring. \
                                Returns matching SQL_IDs with a short snippet. Useful to locate all DMLs \
                                touching a specific table, queries using a particular hint, etc.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "needle": { "type": "string", "description": "case-insensitive substring" },
                        "limit":  { "type": "integer", "description": "max matches (default 20)" }
                    },
                    "required": ["needle"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_snapshots_with_event",
                "description": "Finds snapshots where a given wait event was significant (>= min_pct_dbtime). \
                                Returns per-snapshot stats. Use to localize when an event becomes hot.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "event_name":     { "type": "string" },
                        "kind":           { "type": "string", "enum": ["foreground", "background"] },
                        "min_pct_dbtime": { "type": "number", "description": "default 1.0" }
                    },
                    "required": ["event_name", "kind"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_snapshots_with_sql",
                "description": "Finds all snapshots where a SQL_ID appears in ANY of the SQL sections \
                                (elapsed/cpu/io/gets/reads/top_with_events). Reports per-snapshot impact.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "sql_id": { "type": "string" }
                    },
                    "required": ["sql_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_sqls_by_module",
                "description": "Returns SQL_IDs whose module name matches a case-insensitive substring. \
                                Use to connect application code paths with database load.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "module_pattern": { "type": "string" },
                        "limit":          { "type": "integer", "description": "default 50" }
                    },
                    "required": ["module_pattern"]
                }
            }
        },

        // ====================================================================
        // 4. TIME-SERIES / COMPARE
        // ====================================================================
        {
            "type": "function",
            "function": {
                "name": "get_metric_time_series",
                "description": "Returns a time series across all snapshots for a given metric. Use this to \
                                understand how a load-profile metric, instance statistic, or wait event \
                                evolves over the analysis window.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["load_profile", "instance_stat", "wait_event_fg", "wait_event_bg",
                                     "time_model", "host_cpu", "io_stats_byfunc"]
                        },
                        "name":  { "type": "string", "description": "stat / event / function name" },
                        "field": {
                            "type": "string",
                            "description": "For wait events: 'pct_dbtime'|'total_wait_time_s'|'avg_wait'|'waits'. \
                                            For host_cpu: 'pct_user'|'pct_system'|'pct_idle'|'pct_wio'. \
                                            For io_stats_byfunc: 'reads_data_s'|'writes_data_s'|'avg_time'|etc."
                        }
                    },
                    "required": ["kind", "name"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "compare_snapshots",
                "description": "Compares two snapshots side-by-side, reporting the most significant deltas \
                                in load profile, wait events, top SQLs and latches. Use to contrast an \
                                anomalous period against a baseline.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "snap_id_a": { "type": "integer", "description": "baseline ('good') snapshot" },
                        "snap_id_b": { "type": "integer", "description": "anomaly ('bad') snapshot" },
                        "focus": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": ["load_profile", "waits", "sqls", "latches", "host_cpu", "io"]
                            },
                            "description": "Optional: restrict comparison to specified areas"
                        }
                    },
                    "required": ["snap_id_a", "snap_id_b"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_wait_event_histogram",
                "description": "Returns the millisecond-bucket histogram (<1ms..>1s) for a wait event in a \
                                specific snapshot. Use to distinguish fast cache hits from slow physical waits.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "snap_id":    { "type": "integer" },
                        "event_name": { "type": "string" },
                        "kind":       { "type": "string", "enum": ["foreground", "background"] }
                    },
                    "required": ["snap_id", "event_name", "kind"]
                }
            }
        },

        // ====================================================================
        // 5. META — discovery
        // ====================================================================
        {
            "type": "function",
            "function": {
                "name": "list_available_metrics",
                "description": "Lists all metric/event/stat names available for a given kind. Call this \
                                BEFORE get_metric_time_series if you're not sure of exact names.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["load_profile", "instance_stat", "wait_event_fg", "wait_event_bg",
                                     "time_model", "io_stats_byfunc", "latch", "library_cache",
                                     "dictionary_cache", "init_parameter", "sql_id", "segment_category"]
                        },
                        "pattern": { "type": "string", "description": "Optional case-insensitive substring filter" }
                    },
                    "required": ["kind"]
                }
            }
        }
    ])
}

// ----------------------------------------------------------------------------
// Dispatcher — routes a tool call to the proper implementation
// ----------------------------------------------------------------------------

/// Routes the tool call coming from the LLM to the matching implementation
/// and returns the result as a JSON-encoded string (the format expected by the
/// `role: "tool"` message in the chat completion API).
pub fn dispatch_tool_call(
    name: &str,
    args: &Value,
    collection: &AWRSCollection,
) -> String {
    let result: Value = match name {
        // Point lookups
        "get_snapshot_details"   => tool_get_snapshot_details(args, collection),
        "get_sql_text"           => tool_get_sql_text(args, collection),
        "get_init_parameter"     => tool_get_init_parameter(args, collection),
        "get_db_instance_info"   => json!(collection.db_instance_information),

        // Aggregations
        "list_snapshots"              => tool_list_snapshots(args, collection),
        "top_sqls_in_snapshot"        => tool_top_sqls_in_snapshot(args, collection),
        "top_wait_events_in_snapshot" => tool_top_wait_events(args, collection),
        "top_segments_in_snapshot"    => tool_top_segments(args, collection),
        "top_latches_in_snapshot"     => tool_top_latches(args, collection),

        // Search
        "search_sql_text"           => tool_search_sql_text(args, collection),
        "find_snapshots_with_event" => tool_find_snapshots_with_event(args, collection),
        "find_snapshots_with_sql"   => tool_find_snapshots_with_sql(args, collection),
        "find_sqls_by_module"       => tool_find_sqls_by_module(args, collection),

        // Time-series & compare
        "get_metric_time_series"    => tool_get_metric_time_series(args, collection),
        "compare_snapshots"         => tool_compare_snapshots(args, collection),
        "get_wait_event_histogram"  => tool_get_wait_event_histogram(args, collection),

        // Meta
        "list_available_metrics"    => tool_list_available_metrics(args, collection),

        other => json!({ "error": format!("Unknown tool: {}", other) }),
    };

    serde_json::to_string(&result).unwrap_or_else(|e|
        json!({ "error": format!("Serialization failed: {}", e) }).to_string()
    )
}

// ----------------------------------------------------------------------------
// Shared helpers
// ----------------------------------------------------------------------------

/// Locates a single AWR snapshot by its `begin_snap_id`.
fn find_awr<'a>(collection: &'a AWRSCollection, snap_id: u64) -> Option<&'a AWR> {
    collection.awrs.iter().find(|a| a.snap_info.begin_snap_id == snap_id)
}

// Convenience argument extractors — keep tool implementations terse.
fn arg_u64(args: &Value, key: &str) -> Option<u64> { args.get(key)?.as_u64() }
fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> { args.get(key)?.as_str() }
fn arg_f64(args: &Value, key: &str) -> Option<f64> { args.get(key)?.as_f64() }
fn arg_usize(args: &Value, key: &str, default: usize) -> usize {
    args.get(key).and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(default)
}

/// Descending float comparator used for ranking by Vec::sort_by.
fn cmp_desc(a: f64, b: f64) -> std::cmp::Ordering {
    b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
}

/// Looks up DB Time (seconds) from the time_model_stats of an AWR snapshot.
fn db_time_of(awr: &AWR) -> f64 {
    awr.time_model_stats.iter()
        .find(|t| t.stat_name.eq_ignore_ascii_case("DB time"))
        .map(|t| t.time_s).unwrap_or(0.0)
}

/// Looks up DB CPU (seconds) from the time_model_stats of an AWR snapshot.
fn db_cpu_of(awr: &AWR) -> f64 {
    awr.time_model_stats.iter()
        .find(|t| t.stat_name.eq_ignore_ascii_case("DB CPU"))
        .map(|t| t.time_s).unwrap_or(0.0)
}

// ============================================================================
// 1. POINT-LOOKUP IMPLEMENTATIONS
// ============================================================================

/// Returns the full per-section content of a single AWR snapshot.
/// If `sections` is provided, only those sections are included in the response,
/// which keeps the payload returned to the LLM as small as possible.
fn tool_get_snapshot_details(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = match arg_u64(args, "snap_id") {
        Some(v) => v,
        None => return json!({ "error": "snap_id is required" }),
    };
    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({ "error": format!("snap_id {} not found", snap_id) }),
    };

    // Optional filter set; absence means "include everything".
    let sections: Option<HashSet<String>> = args.get("sections")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());

    let include = |key: &str| sections.as_ref().map_or(true, |s| s.contains(key));

    let mut out = json!({
        "snap_info": awr.snap_info,
        "file_name": awr.file_name,
    });

    if include("load_profile")            { out["load_profile"]            = json!(awr.load_profile); }
    if include("instance_efficiency")     { out["instance_efficiency"]     = json!(awr.instance_efficiency); }
    if include("host_cpu")                { out["host_cpu"]                = json!(awr.host_cpu); }
    if include("time_model")              { out["time_model"]              = json!(awr.time_model_stats); }
    if include("foreground_waits")        { out["foreground_waits"]        = json!(awr.foreground_wait_events); }
    if include("background_waits")        { out["background_waits"]        = json!(awr.background_wait_events); }
    if include("sql_elapsed")             { out["sql_elapsed"]             = json!(awr.sql_elapsed_time); }
    if include("sql_cpu")                 { out["sql_cpu"]                 = json!(awr.sql_cpu_time); }
    if include("sql_io")                  { out["sql_io"]                  = json!(awr.sql_io_time); }
    if include("sql_gets")                { out["sql_gets"]                = json!(awr.sql_gets); }
    if include("sql_reads")               { out["sql_reads"]               = json!(awr.sql_reads); }
    if include("top_sql_with_top_events") { out["top_sql_with_top_events"] = json!(awr.top_sql_with_top_events); }
    if include("instance_stats")          { out["instance_stats"]          = json!(awr.instance_stats); }
    if include("io_stats_byfunc")         { out["io_stats_byfunc"]         = json!(awr.io_stats_byfunc); }
    if include("dictionary_cache")        { out["dictionary_cache"]        = json!(awr.dictionary_cache); }
    if include("library_cache")           { out["library_cache"]           = json!(awr.library_cache); }
    if include("latch_activity")          { out["latch_activity"]          = json!(awr.latch_activity); }
    if include("segment_stats")           { out["segment_stats"]           = json!(awr.segment_stats); }
    if include("redo_log")                { out["redo_log"]                = json!(awr.redo_log); }

    out
}

/// Returns the full SQL text for a given SQL_ID, or a null result with a hint
/// when the text was not collected (e.g. security_level < 2).
fn tool_get_sql_text(args: &Value, c: &AWRSCollection) -> Value {
    let sql_id = match arg_str(args, "sql_id") {
        Some(v) => v,
        None => return json!({ "error": "sql_id is required" }),
    };
    match c.sql_text.get(sql_id) {
        Some(text) => json!({ "sql_id": sql_id, "sql_text": text }),
        None => json!({
            "sql_id": sql_id,
            "sql_text": null,
            "note": "SQL text was not collected for this SQL_ID (security_level < 2 or not present in source reports)"
        }),
    }
}

/// Returns initialization parameter values either by explicit name list,
/// by case-insensitive substring `pattern`, or both.
fn tool_get_init_parameter(args: &Value, c: &AWRSCollection) -> Value {
    let mut result = serde_json::Map::new();

    // Explicit names
    if let Some(names) = args.get("names").and_then(|v| v.as_array()) {
        for name in names.iter().filter_map(|v| v.as_str()) {
            let value = c.initialization_parameters.get(name)
                .cloned()
                .unwrap_or_else(|| "<not present>".to_string());
            result.insert(name.to_string(), json!(value));
        }
    }

    // Substring pattern
    if let Some(pat) = arg_str(args, "pattern") {
        let pat_lc = pat.to_lowercase();
        for (k, v) in &c.initialization_parameters {
            if k.to_lowercase().contains(&pat_lc) {
                result.insert(k.clone(), json!(v));
            }
        }
    }

    json!({
        "parameters": result,
        "total_returned": result.len(),
        "total_available": c.initialization_parameters.len()
    })
}

// ============================================================================
// 2. AGGREGATION IMPLEMENTATIONS
// ============================================================================

/// Returns a compact list of snapshots in a range with quick-look metrics.
fn tool_list_snapshots(args: &Value, c: &AWRSCollection) -> Value {
    let from  = arg_u64(args, "from_snap_id").unwrap_or(0);
    let to    = arg_u64(args, "to_snap_id").unwrap_or(u64::MAX);
    let limit = arg_usize(args, "limit", 50);

    let list: Vec<Value> = c.awrs.iter()
        .filter(|a| {
            let id = a.snap_info.begin_snap_id;
            id >= from && id <= to
        })
        .take(limit)
        .map(|a| {
            let db_time = db_time_of(a);
            let db_cpu  = db_cpu_of(a);
            // Top foreground event by %DB time for a quick "what hurt the most" hint
            let top_fg = a.foreground_wait_events.iter()
                .max_by(|x, y| x.pct_dbtime.partial_cmp(&y.pct_dbtime).unwrap_or(std::cmp::Ordering::Equal))
                .map(|e| json!({ "event": e.event, "pct_dbtime": e.pct_dbtime }))
                .unwrap_or(Value::Null);

            json!({
                "snap_id":             a.snap_info.begin_snap_id,
                "begin_snap_time":     a.snap_info.begin_snap_time,
                "end_snap_time":       a.snap_info.end_snap_time,
                "db_time_s":           db_time,
                "db_cpu_s":            db_cpu,
                "db_cpu_dbtime_ratio": if db_time > 0.0 { db_cpu / db_time } else { 0.0 },
                "host_cpu_pct_idle":   a.host_cpu.pct_idle,
                "host_cpu_pct_user":   a.host_cpu.pct_user,
                "top_fg_event":        top_fg,
            })
        })
        .collect();

    json!({ "count": list.len(), "snapshots": list })
}

/// Returns the top-N SQLs in a snapshot ranked by the chosen metric.
fn tool_top_sqls_in_snapshot(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = arg_u64(args, "snap_id").unwrap_or(0);
    let metric  = arg_str(args, "metric").unwrap_or("elapsed_time");
    let top_n   = arg_usize(args, "top_n", 10);

    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({ "error": format!("snap_id {} not found", snap_id) }),
    };

    let items: Vec<Value> = match metric {
        "elapsed_time" => {
            let mut v = awr.sql_elapsed_time.clone();
            v.sort_by(|a, b| cmp_desc(a.elapsed_time_s, b.elapsed_time_s));
            v.into_iter().take(top_n).map(|s| json!(s)).collect()
        }
        "cpu_time" => {
            let mut v: Vec<_> = awr.sql_cpu_time.values().cloned().collect();
            v.sort_by(|a, b| cmp_desc(a.cpu_time_s, b.cpu_time_s));
            v.into_iter().take(top_n).map(|s| json!(s)).collect()
        }
        "io_time" => {
            let mut v: Vec<_> = awr.sql_io_time.values().cloned().collect();
            v.sort_by(|a, b| cmp_desc(a.io_time_s, b.io_time_s));
            v.into_iter().take(top_n).map(|s| json!(s)).collect()
        }
        "buffer_gets" => {
            let mut v: Vec<_> = awr.sql_gets.values().cloned().collect();
            v.sort_by(|a, b| cmp_desc(a.buffer_gets, b.buffer_gets));
            v.into_iter().take(top_n).map(|s| json!(s)).collect()
        }
        "physical_reads" => {
            let mut v: Vec<_> = awr.sql_reads.values().cloned().collect();
            v.sort_by(|a, b| cmp_desc(a.physical_reads, b.physical_reads));
            v.into_iter().take(top_n).map(|s| json!(s)).collect()
        }
        other => return json!({ "error": format!("Unknown metric '{}'", other) }),
    };

    json!({ "snap_id": snap_id, "metric": metric, "items": items })
}

/// Returns top-N wait events (foreground or background) within a single snapshot.
fn tool_top_wait_events(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = arg_u64(args, "snap_id").unwrap_or(0);
    let kind    = arg_str(args, "kind").unwrap_or("foreground");
    let top_n   = arg_usize(args, "top_n", 10);
    let rank_by = arg_str(args, "rank_by").unwrap_or("pct_dbtime");

    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({ "error": format!("snap_id {} not found", snap_id) }),
    };

    let mut events = if kind == "background" {
        awr.background_wait_events.clone()
    } else {
        awr.foreground_wait_events.clone()
    };

    events.sort_by(|a, b| {
        let (x, y) = match rank_by {
            "total_wait_time_s" => (a.total_wait_time_s, b.total_wait_time_s),
            "avg_wait"          => (a.avg_wait,          b.avg_wait),
            "waits"             => (a.waits as f64,      b.waits as f64),
            _                   => (a.pct_dbtime,        b.pct_dbtime),
        };
        cmp_desc(x, y)
    });

    let items: Vec<Value> = events.into_iter().take(top_n).map(|e| json!(e)).collect();
    json!({ "snap_id": snap_id, "kind": kind, "rank_by": rank_by, "items": items })
}

/// Returns top-N segments by a given category (e.g. "Logical Reads") in a snapshot.
fn tool_top_segments(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id  = arg_u64(args, "snap_id").unwrap_or(0);
    let category = arg_str(args, "category").unwrap_or("");
    let top_n    = arg_usize(args, "top_n", 10);

    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({ "error": format!("snap_id {} not found", snap_id) }),
    };

    match awr.segment_stats.get(category) {
        Some(segs) => {
            let mut v = segs.clone();
            v.sort_by(|a, b| cmp_desc(a.stat_vlalue, b.stat_vlalue));
            json!({
                "snap_id":  snap_id,
                "category": category,
                "items":    v.into_iter().take(top_n).collect::<Vec<_>>()
            })
        }
        None => json!({
            "error": format!("Category '{}' not found in this snapshot", category),
            "available_categories": awr.segment_stats.keys().collect::<Vec<_>>()
        }),
    }
}

/// Returns top-N latches in a snapshot by wait_time, pct_miss, or get_requests.
fn tool_top_latches(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = arg_u64(args, "snap_id").unwrap_or(0);
    let rank_by = arg_str(args, "rank_by").unwrap_or("wait_time");
    let top_n   = arg_usize(args, "top_n", 10);

    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({ "error": format!("snap_id {} not found", snap_id) }),
    };

    let mut v = awr.latch_activity.clone();
    v.sort_by(|a, b| {
        let (x, y) = match rank_by {
            "pct_miss"     => (a.get_pct_miss,           b.get_pct_miss),
            "get_requests" => (a.get_requests as f64,    b.get_requests as f64),
            _              => (a.wait_time,              b.wait_time),
        };
        cmp_desc(x, y)
    });

    json!({
        "snap_id": snap_id,
        "rank_by": rank_by,
        "items":   v.into_iter().take(top_n).collect::<Vec<_>>()
    })
}

// ============================================================================
// 3. SEARCH IMPLEMENTATIONS
// ============================================================================

/// Searches SQL text for a case-insensitive substring and returns short snippets.
fn tool_search_sql_text(args: &Value, c: &AWRSCollection) -> Value {
    let needle = match arg_str(args, "needle") {
        Some(n) => n.to_lowercase(),
        None => return json!({ "error": "needle is required" }),
    };
    let limit = arg_usize(args, "limit", 20);

    let matches: Vec<Value> = c.sql_text.iter()
        .filter(|(_, text)| text.to_lowercase().contains(&needle))
        .take(limit)
        .map(|(sql_id, text)| {
            // Trim to first 300 chars to keep payloads small
            let snippet: String = text.chars().take(300).collect();
            json!({
                "sql_id":   sql_id,
                "snippet":  snippet,
                "full_len": text.len()
            })
        })
        .collect();

    json!({
        "needle":       needle,
        "match_count":  matches.len(),
        "matches":      matches
    })
}

/// Finds snapshots in which a given wait event reached at least `min_pct_dbtime`.
fn tool_find_snapshots_with_event(args: &Value, c: &AWRSCollection) -> Value {
    let event_name = match arg_str(args, "event_name") {
        Some(n) => n,
        None => return json!({ "error": "event_name is required" }),
    };
    let kind    = arg_str(args, "kind").unwrap_or("foreground");
    let min_pct = arg_f64(args, "min_pct_dbtime").unwrap_or(1.0);

    let hits: Vec<Value> = c.awrs.iter().filter_map(|a| {
        let events = if kind == "background" {
            &a.background_wait_events
        } else {
            &a.foreground_wait_events
        };
        events.iter()
            .find(|e| e.event == event_name && e.pct_dbtime >= min_pct)
            .map(|e| json!({
                "snap_id":           a.snap_info.begin_snap_id,
                "begin_snap_time":   a.snap_info.begin_snap_time,
                "waits":             e.waits,
                "total_wait_time_s": e.total_wait_time_s,
                "avg_wait_ms":       e.avg_wait,
                "pct_dbtime":        e.pct_dbtime,
            }))
    }).collect();

    json!({
        "event_name":     event_name,
        "kind":           kind,
        "min_pct_dbtime": min_pct,
        "hit_count":      hits.len(),
        "snapshots":      hits
    })
}

/// Finds all snapshots in which a given SQL_ID appears in any of the SQL sections,
/// with a per-snapshot summary of where it appeared and the impact in that section.
fn tool_find_snapshots_with_sql(args: &Value, c: &AWRSCollection) -> Value {
    let sql_id = match arg_str(args, "sql_id") {
        Some(s) => s,
        None => return json!({ "error": "sql_id is required" }),
    };

    let hits: Vec<Value> = c.awrs.iter().filter_map(|a| {
        // Build a per-snapshot fact record describing which sections list this SQL_ID
        let mut sections = serde_json::Map::new();

        if let Some(s) = a.sql_elapsed_time.iter().find(|s| s.sql_id == sql_id) {
            sections.insert("sql_elapsed".to_string(), json!({
                "elapsed_time_s": s.elapsed_time_s,
                "executions":     s.executions,
                "pct_total":      s.pct_total
            }));
        }
        if let Some(s) = a.sql_cpu_time.get(sql_id) {
            sections.insert("sql_cpu".to_string(), json!({
                "cpu_time_s": s.cpu_time_s,
                "executions": s.executions,
                "pct_total":  s.pct_total
            }));
        }
        if let Some(s) = a.sql_io_time.get(sql_id) {
            sections.insert("sql_io".to_string(), json!({
                "io_time_s":  s.io_time_s,
                "executions": s.executions,
                "pct_total":  s.pct_total
            }));
        }
        if let Some(s) = a.sql_gets.get(sql_id) {
            sections.insert("sql_gets".to_string(), json!({
                "buffer_gets":   s.buffer_gets,
                "executions":    s.executions,
                "gets_per_exec": s.gets_per_exec,
                "pct_total":     s.pct_total
            }));
        }
        if let Some(s) = a.sql_reads.get(sql_id) {
            sections.insert("sql_reads".to_string(), json!({
                "physical_reads": s.physical_reads,
                "executions":     s.executions,
                "reads_per_exec": s.reads_per_exec,
                "pct_total":      s.pct_total
            }));
        }
        if let Some(s) = a.top_sql_with_top_events.get(sql_id) {
            sections.insert("top_sql_with_top_events".to_string(), json!({
                "event_name":   s.event_name,
                "pct_event":    s.pct_event,
                "pct_activity": s.pct_activity,
                "executions":   s.executions
            }));
        }

        if sections.is_empty() {
            None
        } else {
            Some(json!({
                "snap_id":         a.snap_info.begin_snap_id,
                "begin_snap_time": a.snap_info.begin_snap_time,
                "appearances":     sections
            }))
        }
    }).collect();

    json!({
        "sql_id":          sql_id,
        "snapshot_count":  hits.len(),
        "snapshots":       hits
    })
}

/// Returns SQL_IDs whose module name matches a case-insensitive substring,
/// aggregated across all snapshots so the LLM sees the global picture.
fn tool_find_sqls_by_module(args: &Value, c: &AWRSCollection) -> Value {
    let pattern = match arg_str(args, "module_pattern") {
        Some(p) => p.to_lowercase(),
        None => return json!({ "error": "module_pattern is required" }),
    };
    let limit = arg_usize(args, "limit", 50);

    // For each SQL_ID, accumulate distinct modules and a count of appearances
    let mut accum: HashMap<String, (HashSet<String>, u64)> = HashMap::new();

    for a in &c.awrs {
        for s in &a.sql_elapsed_time {
            if s.sql_module.to_lowercase().contains(&pattern) {
                let entry = accum.entry(s.sql_id.clone()).or_insert((HashSet::new(), 0));
                entry.0.insert(s.sql_module.clone());
                entry.1 += 1;
            }
        }
    }

    let mut rows: Vec<Value> = accum.into_iter().map(|(sql_id, (modules, count))| {
        json!({
            "sql_id":            sql_id,
            "modules":           modules.into_iter().collect::<Vec<_>>(),
            "snapshot_hit_count": count
        })
    }).collect();

    // Sort by global frequency so the most relevant items come first
    rows.sort_by(|a, b| {
        let bn = b["snapshot_hit_count"].as_u64().unwrap_or(0);
        let an = a["snapshot_hit_count"].as_u64().unwrap_or(0);
        bn.cmp(&an)
    });
    rows.truncate(limit);

    json!({
        "module_pattern": pattern,
        "matches":        rows.len(),
        "items":          rows
    })
}

// ============================================================================
// 4. TIME-SERIES & COMPARE IMPLEMENTATIONS
// ============================================================================

/// Builds a time series of a single metric across all snapshots.
/// Supports load profile, instance stats, wait events (FG/BG), time model,
/// host CPU and IO stats by function.
fn tool_get_metric_time_series(args: &Value, c: &AWRSCollection) -> Value {
    let kind  = arg_str(args, "kind").unwrap_or("");
    let name  = match arg_str(args, "name") {
        Some(n) => n,
        None => return json!({ "error": "name is required" }),
    };
    let field = arg_str(args, "field").unwrap_or("pct_dbtime");

    let series: Vec<Value> = c.awrs.iter().filter_map(|a| {
        let value: Option<f64> = match kind {
            "load_profile" => a.load_profile.iter()
                .find(|lp| lp.stat_name == name)
                .map(|lp| lp.per_second),

            "instance_stat" => a.instance_stats.iter()
                .find(|s| s.statname == name)
                .map(|s| s.total as f64),

            "wait_event_fg" | "wait_event_bg" => {
                let events = if kind == "wait_event_bg" {
                    &a.background_wait_events
                } else {
                    &a.foreground_wait_events
                };
                events.iter().find(|e| e.event == name).map(|e| match field {
                    "total_wait_time_s" => e.total_wait_time_s,
                    "avg_wait"          => e.avg_wait,
                    "waits"             => e.waits as f64,
                    _                   => e.pct_dbtime,
                })
            }

            "time_model" => a.time_model_stats.iter()
                .find(|t| t.stat_name == name)
                .map(|t| match field {
                    "pct_dbtime" => t.pct_dbtime,
                    _            => t.time_s,
                }),

            "host_cpu" => match field {
                "pct_system" => Some(a.host_cpu.pct_system),
                "pct_idle"   => Some(a.host_cpu.pct_idle),
                "pct_wio"    => Some(a.host_cpu.pct_wio),
                _            => Some(a.host_cpu.pct_user),
            },

            "io_stats_byfunc" => a.io_stats_byfunc.get(name).map(|io| match field {
                "reads_data"    => io.reads_data,
                "reads_req_s"   => io.reads_req_s,
                "reads_data_s"  => io.reads_data_s,
                "writes_data"   => io.writes_data,
                "writes_req_s"  => io.writes_req_s,
                "writes_data_s" => io.writes_data_s,
                "waits_count"   => io.waits_count as f64,
                "avg_time"      => io.avg_time.unwrap_or(0.0),
                _               => io.reads_data_s,
            }),

            _ => None,
        };

        value.map(|v| json!({
            "snap_id":         a.snap_info.begin_snap_id,
            "begin_snap_time": a.snap_info.begin_snap_time,
            "value":           v,
        }))
    }).collect();

    json!({
        "kind":   kind,
        "name":   name,
        "field":  field,
        "points": series.len(),
        "series": series
    })
}

/// Compares two snapshots and surfaces the largest deltas in load profile,
/// foreground wait events, top SQLs and latches. `focus` may narrow the scope.
fn tool_compare_snapshots(args: &Value, c: &AWRSCollection) -> Value {
    let snap_a = arg_u64(args, "snap_id_a").unwrap_or(0);
    let snap_b = arg_u64(args, "snap_id_b").unwrap_or(0);

    let a = match find_awr(c, snap_a) {
        Some(a) => a, None => return json!({ "error": format!("snap_id_a {} not found", snap_a) }),
    };
    let b = match find_awr(c, snap_b) {
        Some(b) => b, None => return json!({ "error": format!("snap_id_b {} not found", snap_b) }),
    };

    // If no focus is given, compare all areas
    let focus: HashSet<String> = args.get("focus")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_else(|| ["load_profile","waits","sqls","latches","host_cpu","io"]
            .iter().map(|s| s.to_string()).collect());

    let mut out = json!({
        "snap_a": { "snap_id": snap_a, "begin_snap_time": a.snap_info.begin_snap_time },
        "snap_b": { "snap_id": snap_b, "begin_snap_time": b.snap_info.begin_snap_time },
    });

    // --- Load profile deltas (per second) ---
    if focus.contains("load_profile") {
        let map_a: HashMap<&str, f64> = a.load_profile.iter()
            .map(|lp| (lp.stat_name.as_str(), lp.per_second)).collect();
        let mut deltas: Vec<Value> = b.load_profile.iter().filter_map(|lp| {
            let va = *map_a.get(lp.stat_name.as_str()).unwrap_or(&0.0);
            let vb = lp.per_second;
            let d  = vb - va;
            if d.abs() > 0.0 {
                Some(json!({
                    "stat_name": lp.stat_name,
                    "a_per_sec": va,
                    "b_per_sec": vb,
                    "delta":     d
                }))
            } else {
                None
            }
        }).collect();
        deltas.sort_by(|x, y| {
            let xd = x["delta"].as_f64().unwrap_or(0.0).abs();
            let yd = y["delta"].as_f64().unwrap_or(0.0).abs();
            cmp_desc(xd, yd)
        });
        deltas.truncate(15);
        out["load_profile_deltas"] = json!(deltas);
    }

    // --- Foreground wait event deltas (% DB time) ---
    if focus.contains("waits") {
        let map_a: HashMap<&str, f64> = a.foreground_wait_events.iter()
            .map(|e| (e.event.as_str(), e.pct_dbtime)).collect();
        let mut deltas: Vec<Value> = b.foreground_wait_events.iter().map(|e| {
            let va = *map_a.get(e.event.as_str()).unwrap_or(&0.0);
            json!({
                "event":             e.event,
                "a_pct_dbtime":      va,
                "b_pct_dbtime":      e.pct_dbtime,
                "delta_pct_dbtime":  e.pct_dbtime - va
            })
        }).collect();
        deltas.sort_by(|x, y| {
            let xd = x["delta_pct_dbtime"].as_f64().unwrap_or(0.0).abs();
            let yd = y["delta_pct_dbtime"].as_f64().unwrap_or(0.0).abs();
            cmp_desc(xd, yd)
        });
        deltas.truncate(15);
        out["fg_wait_event_deltas"] = json!(deltas);
    }

    // --- SQL deltas by elapsed time ---
    if focus.contains("sqls") {
        let map_a: HashMap<&str, f64> = a.sql_elapsed_time.iter()
            .map(|s| (s.sql_id.as_str(), s.elapsed_time_s)).collect();
        let map_b: HashMap<&str, f64> = b.sql_elapsed_time.iter()
            .map(|s| (s.sql_id.as_str(), s.elapsed_time_s)).collect();

        // Union of all SQL_IDs from both snapshots
        let all_keys: HashSet<&str> = map_a.keys().chain(map_b.keys()).copied().collect();
        let mut deltas: Vec<Value> = all_keys.into_iter().map(|sql_id| {
            let va = *map_a.get(sql_id).unwrap_or(&0.0);
            let vb = *map_b.get(sql_id).unwrap_or(&0.0);
            json!({
                "sql_id":          sql_id,
                "a_elapsed_s":     va,
                "b_elapsed_s":     vb,
                "delta_elapsed_s": vb - va
            })
        }).collect();
        deltas.sort_by(|x, y| {
            let xd = x["delta_elapsed_s"].as_f64().unwrap_or(0.0).abs();
            let yd = y["delta_elapsed_s"].as_f64().unwrap_or(0.0).abs();
            cmp_desc(xd, yd)
        });
        deltas.truncate(20);
        out["sql_elapsed_deltas"] = json!(deltas);
    }

    // --- Latch wait_time deltas ---
    if focus.contains("latches") {
        let map_a: HashMap<&str, f64> = a.latch_activity.iter()
            .map(|l| (l.statname.as_str(), l.wait_time)).collect();
        let mut deltas: Vec<Value> = b.latch_activity.iter().map(|l| {
            let va = *map_a.get(l.statname.as_str()).unwrap_or(&0.0);
            json!({
                "latch":      l.statname,
                "a_wait_s":   va,
                "b_wait_s":   l.wait_time,
                "delta_wait": l.wait_time - va
            })
        }).collect();
        deltas.sort_by(|x, y| {
            let xd = x["delta_wait"].as_f64().unwrap_or(0.0).abs();
            let yd = y["delta_wait"].as_f64().unwrap_or(0.0).abs();
            cmp_desc(xd, yd)
        });
        deltas.truncate(15);
        out["latch_deltas"] = json!(deltas);
    }

    // --- Host CPU side-by-side ---
    if focus.contains("host_cpu") {
        out["host_cpu"] = json!({
            "a": a.host_cpu,
            "b": b.host_cpu,
        });
    }

    // --- IO summary side-by-side (key functions) ---
    if focus.contains("io") {
        out["io_stats_byfunc"] = json!({
            "a": a.io_stats_byfunc,
            "b": b.io_stats_byfunc,
        });
    }

    out
}

/// Returns the millisecond-bucket histogram for a wait event in a specific snapshot.
fn tool_get_wait_event_histogram(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = arg_u64(args, "snap_id").unwrap_or(0);
    let event   = arg_str(args, "event_name").unwrap_or("");
    let kind    = arg_str(args, "kind").unwrap_or("foreground");

    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({ "error": format!("snap_id {} not found", snap_id) }),
    };

    let events = if kind == "background" {
        &awr.background_wait_events
    } else {
        &awr.foreground_wait_events
    };

    match events.iter().find(|e| e.event == event) {
        Some(e) => json!({
            "snap_id":   snap_id,
            "event":     event,
            "kind":      kind,
            "histogram": e.waitevent_histogram_ms,
            "summary": {
                "waits":             e.waits,
                "total_wait_time_s": e.total_wait_time_s,
                "avg_wait_ms":       e.avg_wait,
                "pct_dbtime":        e.pct_dbtime,
            }
        }),
        None => json!({ "error": format!("Event '{}' not found in {} waits of snapshot {}",
                                         event, kind, snap_id) }),
    }
}

// ============================================================================
// 5. META — discovery
// ============================================================================

/// Lists all available metric/event/stat names of a given kind. Use the
/// optional `pattern` to filter by case-insensitive substring. This lets the
/// LLM discover correct names BEFORE calling `get_metric_time_series`.
fn tool_list_available_metrics(args: &Value, c: &AWRSCollection) -> Value {
    let kind    = arg_str(args, "kind").unwrap_or("");
    let pattern = arg_str(args, "pattern").map(|p| p.to_lowercase());

    // Collect unique names across all snapshots
    let mut names: HashSet<String> = HashSet::new();

    match kind {
        "load_profile" => for a in &c.awrs {
            for lp in &a.load_profile { names.insert(lp.stat_name.clone()); }
        },
        "instance_stat" => for a in &c.awrs {
            for s in &a.instance_stats { names.insert(s.statname.clone()); }
        },
        "wait_event_fg" => for a in &c.awrs {
            for e in &a.foreground_wait_events { names.insert(e.event.clone()); }
        },
        "wait_event_bg" => for a in &c.awrs {
            for e in &a.background_wait_events { names.insert(e.event.clone()); }
        },
        "time_model" => for a in &c.awrs {
            for t in &a.time_model_stats { names.insert(t.stat_name.clone()); }
        },
        "io_stats_byfunc" => for a in &c.awrs {
            for k in a.io_stats_byfunc.keys() { names.insert(k.clone()); }
        },
        "latch" => for a in &c.awrs {
            for l in &a.latch_activity { names.insert(l.statname.clone()); }
        },
        "library_cache" => for a in &c.awrs {
            for l in &a.library_cache { names.insert(l.statname.clone()); }
        },
        "dictionary_cache" => for a in &c.awrs {
            for d in &a.dictionary_cache { names.insert(d.statname.clone()); }
        },
        "init_parameter" => for k in c.initialization_parameters.keys() {
            names.insert(k.clone());
        },
        "sql_id" => for k in c.sql_text.keys() {
            names.insert(k.clone());
        },
        "segment_category" => for a in &c.awrs {
            for k in a.segment_stats.keys() { names.insert(k.clone()); }
        },
        other => return json!({ "error": format!("Unknown kind '{}'", other) }),
    }

    // Optional filtering and sorted output for determinism
    let mut filtered: Vec<String> = names.into_iter()
        .filter(|n| match &pattern {
            Some(p) => n.to_lowercase().contains(p),
            None => true,
        })
        .collect();
    filtered.sort();

    json!({
        "kind":    kind,
        "pattern": pattern,
        "count":   filtered.len(),
        "names":   filtered
    })
}