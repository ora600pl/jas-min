// ============================================================================
// ai_tools.rs
// ----------------------------------------------------------------------------
// Tool-calling support for JAS-MIN's OpenRouter / OpenAI-compatible integration.
//
// This module exposes:
//   * `jasmin_tools_schema()`  — OpenAI/OpenRouter-compatible tool schema
//   * `dispatch_tool_call()`   — executes a tool call against an AWRSCollection
//
// Design goals:
//   * give the model narrow, useful diagnostic probes instead of dumping whole AWRs;
//   * keep returned payloads bounded and deterministic enough for logs/diffs;
//   * never panic on malformed model arguments; return JSON errors instead;
//   * preserve backward-compatible aliases for older prompt/tool names.
// ============================================================================

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::fs;

use crate::awr::{AWR, AWRSCollection};

const JASMIN_TOOLS_SCHEMA_VERSION: &str = "2026-05-20.1";
const DEFAULT_LIMIT: usize = 50;
const DEFAULT_TOP_N: usize = 10;
const MAX_LIMIT: usize = 500;
const MAX_TOP_N: usize = 100;
const DEFAULT_XPLAN_LIMIT: usize = 100;
const MAX_XPLAN_BYTES: usize = 512 * 1024;

// ----------------------------------------------------------------------------
// Tool schema (sent to the LLM)
// ----------------------------------------------------------------------------

/// Returns the full tool catalog in OpenAI/OpenRouter function-calling format.
///
/// Keep descriptions explicit: the model uses them as its routing table. Yes,
/// apparently we now write documentation for probabilistic parrots. Here we are.
pub fn tools_schema(stem: &str) -> Value {
    let mut tools = json!([
        // ====================================================================
        // 0. GLOBAL OVERVIEW / TRIAGE
        // ====================================================================
        {
            "type": "function",
            "function": {
                "name": "get_database_load_summary",
                "description": "Returns a compact global summary of the whole AWR/STATSPACK collection: snapshot range, DB Time/DB CPU totals, busiest snapshots, top foreground waits and metadata counts. Use this as the first tool call before deciding where to drill down.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "top_n": {
                            "type": "integer",
                            "description": "Number of busiest snapshots and top events to return, default 10, max 100"
                        }
                    }
                }
            }
        },

        // ====================================================================
        // 1. POINT LOOKUPS
        // ====================================================================
        {
            "type": "function",
            "function": {
                "name": "get_snapshot_details",
                "description": "Returns detailed AWR/STATSPACK data for one snapshot. Prefer passing 'sections' to limit output. Full snapshot output may be large and should only be used when narrower tools were insufficient.",
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
                            "description": "Optional: limit the output to selected sections. Omit only when you truly need the whole snapshot."
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
                "description": "Returns the full SQL text for a given SQL_ID when available. Use this after identifying a problematic SQL_ID to inspect tables, joins, predicates and hints.",
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
                "description": "Returns values of one or more Oracle initialization parameters. Parameters are stored at collection level, not per snapshot.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "names": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Explicit list of parameter names, underscore params supported"
                        },
                        "pattern": {
                            "type": "string",
                            "description": "Optional case-insensitive substring filter, e.g. 'optimizer_'"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum parameters returned for pattern searches, default 100, max 500"
                        }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_db_instance_info",
                "description": "Returns DB instance hardware/version metadata: DB ID, release, RAC mode, platform, CPUs/cores/sockets, memory, db_block_size. Useful for sizing reasoning.",
                "parameters": { "type": "object", "properties": {} }
            }
        },

        // ====================================================================
        // 2. AGGREGATIONS
        // ====================================================================
        {
            "type": "function",
            "function": {
                "name": "list_snapshots",
                "description": "Lists snapshots in a range with DB Time, DB CPU, DB CPU/DB Time ratio, host CPU idle/user % and the top foreground wait event. Use this to find quiet baselines, peaks, and neighboring snapshots.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "from_snap_id": { "type": "integer" },
                        "to_snap_id":   { "type": "integer" },
                        "limit":        { "type": "integer", "description": "Max snapshots returned, default 50, max 500" }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "top_sqls_in_snapshot",
                "description": "Returns top-N SQLs in a snapshot ranked by elapsed time, CPU time, I/O time, buffer gets or physical reads.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "snap_id": { "type": "integer" },
                        "metric":  {
                            "type": "string",
                            "enum": ["elapsed_time", "cpu_time", "io_time", "buffer_gets", "physical_reads"]
                        },
                        "top_n": { "type": "integer", "description": "default 10, max 100" }
                    },
                    "required": ["snap_id", "metric"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "top_wait_events_in_snapshot",
                "description": "Returns top-N foreground or background wait events in a snapshot with their millisecond-bucket histograms.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "snap_id": { "type": "integer" },
                        "kind":    { "type": "string", "enum": ["foreground", "background"] },
                        "top_n":   { "type": "integer", "description": "default 10, max 100" },
                        "rank_by": {
                            "type": "string",
                            "enum": ["pct_dbtime", "total_wait_time_s", "avg_wait", "waits"],
                            "description": "default pct_dbtime"
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
                "description": "Returns top segments in a snapshot for a chosen segment statistic category. Use this to identify hot objects behind I/O, logical reads, row lock waits or buffer busy waits.",
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
                        "top_n": { "type": "integer", "description": "default 10, max 100" }
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
                        "top_n":   { "type": "integer", "description": "default 10, max 100" }
                    },
                    "required": ["snap_id"]
                }
            }
        },

        // ====================================================================
        // 3. SEARCH
        // ====================================================================
        {
            "type": "function",
            "function": {
                "name": "search_sql_text",
                "description": "Searches collected SQL text for a case-insensitive substring. Returns matching SQL_IDs with snippets. Useful to locate DMLs touching a table, queries using a hint, or SQLs containing a predicate.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "needle": { "type": "string", "description": "case-insensitive substring" },
                        "limit":  { "type": "integer", "description": "max matches, default 20, max 500" }
                    },
                    "required": ["needle"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_sqls_touching_object",
                "description": "Searches SQL text for references to a table, index, view or object name. Use this when segment statistics point to a hot object and you need candidate SQL_IDs touching it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "object_name": { "type": "string" },
                        "limit": { "type": "integer", "description": "default 30, max 500" }
                    },
                    "required": ["object_name"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_snapshots_with_event",
                "description": "Finds snapshots where a given wait event was significant, using case-insensitive event matching. Returns per-snapshot stats. Use to localize when an event becomes hot.",
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
                "description": "Finds all snapshots where a SQL_ID appears in any SQL section: elapsed/cpu/io/gets/reads/top_with_events. Reports per-snapshot impact.",
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
                "description": "Returns SQL_IDs whose module name matches a case-insensitive substring. Uses SQL sections where module is publicly available. Useful to connect application code paths with database load.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "module_pattern": { "type": "string" },
                        "limit":          { "type": "integer", "description": "default 50, max 500" }
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
                "description": "Returns a time series across all snapshots for a load profile metric, instance statistic, wait event, time model statistic, host CPU field or I/O function metric. Use list_available_metrics first if exact names are uncertain.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["load_profile", "instance_stat", "wait_event_fg", "wait_event_bg", "time_model", "host_cpu", "io_stats_byfunc"]
                        },
                        "name":  { "type": "string", "description": "stat / event / function name. For host_cpu use 'host_cpu'." },
                        "field": {
                            "type": "string",
                            "description": "For wait events: pct_dbtime|total_wait_time_s|avg_wait|waits. For host_cpu: pct_user|pct_system|pct_wio|pct_idle|load_avg_begin|load_avg_end|cpus|cores|sockets. For io_stats_byfunc: reads_data|reads_req_s|reads_data_s|writes_data|writes_req_s|writes_data_s|waits_count|avg_time."
                        }
                    },
                    "required": ["kind", "name"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_wait_event_timeline",
                "description": "Returns a timeline for one wait event across snapshots, including waits, total wait time, average wait, %DB time and histogram. Use after identifying a suspicious wait event.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "event_name": { "type": "string" },
                        "kind": { "type": "string", "enum": ["foreground", "background"] }
                    },
                    "required": ["event_name", "kind"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_sql_timeline",
                "description": "Returns a per-snapshot timeline for one SQL_ID across elapsed time, CPU time, I/O time, buffer gets, physical reads and top wait event information. Use this to determine whether a SQL statement is persistently expensive or only spikes during specific snapshots.",
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
                "name": "compare_snapshots",
                "description": "Compares two snapshots side-by-side, reporting significant deltas in load profile, foreground waits, top SQL elapsed time, latches, host CPU and I/O. Use to contrast an anomalous period against a baseline.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "snap_id_a": { "type": "integer", "description": "baseline/good snapshot" },
                        "snap_id_b": { "type": "integer", "description": "anomaly/bad snapshot" },
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
                "description": "Returns the millisecond-bucket histogram for a wait event in a specific snapshot. Matching is case-insensitive. Use to distinguish many fast waits from fewer slow waits.",
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
        // 5. META / DISCOVERY
        // ====================================================================
        {
            "type": "function",
            "function": {
                "name": "list_available_metrics",
                "description": "Lists metric/event/stat names available for a given kind. Call this before get_metric_time_series if exact names are uncertain. Output is limited to protect context size, because apparently infinite lists are bad for thinking.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["load_profile", "instance_stat", "wait_event_fg", "wait_event_bg", "time_model", "io_stats_byfunc", "latch", "library_cache", "dictionary_cache", "init_parameter", "sql_id", "segment_category"]
                        },
                        "pattern": { "type": "string", "description": "Optional case-insensitive substring filter" },
                        "limit": { "type": "integer", "description": "default 200, max 500" }
                    },
                    "required": ["kind"]
                }
            }
        }
    ]);
    // If an attachments directory exists and contains *.xplan files, publish
    // SQL plan tools. The model can then discover available SQL_ID plans and
    // fetch a concrete plan before making SQL tuning recommendations.
    let attachments_dir = attachments_dir_for_stem(stem);
    if !list_xplan_files(&attachments_dir).is_empty() {
        tools.as_array_mut().expect("tools must be a JSON array").push(json!({
            "type": "function",
            "function": {
                "name": "list_available_sql_plans",
                "description": "Lists SQL_IDs for which execution plan attachments (*.xplan) are available. Use this early when analyzing top SQL, SQL elapsed time, SQL CPU, SQL I/O, suspicious waits, plan instability, performance regressions, or when deciding which SQL execution plans need deeper analysis.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Optional case-insensitive filter. Usually a SQL_ID or part of a filename."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum plans returned, default 100, max 500."
                        }
                    }
                }
            }
        }));

        tools.as_array_mut().expect("tools must be a JSON array").push(json!({
            "type": "function",
            "function": {
                "name": "get_sql_execution_plan",
                "description": "Returns the text execution plan from an attachment file, typically <SQL_ID>.xplan. Use this for every SQL_ID that materially contributes to DB Time, DB CPU, elapsed time, I/O time, buffer gets, physical reads, regressions, or suspicious wait events. Strongly prefer this tool before making claims about access paths, join methods, cardinality estimates, partition pruning, index usage, full scans, adaptive plans, bind sensitivity, or SQL tuning recommendations.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "sql_id": {
                            "type": "string",
                            "description": "SQL_ID whose execution plan should be read. If filename is 7ud94ccmpaz8u.xplan, pass 7ud94ccmpaz8u."
                        }
                    },
                    "required": ["sql_id"]
                }
            }
        }));

        println!("✅ Found execution plans in {}", attachments_dir.display());
    }    
    tools
}

// ----------------------------------------------------------------------------
// Dispatcher
// ----------------------------------------------------------------------------

/// Routes a tool call from the LLM to the matching implementation and returns a
/// JSON-encoded string suitable for a `role: "tool"` chat message.
pub fn dispatch_tool_call(name: &str, args: &Value, collection: &AWRSCollection, stem: &str) -> String {
    let result: Value = match name {
        // Global overview
        "get_database_load_summary" => tool_get_database_load_summary(args, collection),

        // Point lookups
        "get_snapshot_details" => tool_get_snapshot_details(args, collection),
        "get_snapshot_summary" => tool_get_snapshot_summary(args, collection),
        "get_sql_text" => tool_get_sql_text(args, collection),
        "get_init_parameter" => tool_get_init_parameter(args, collection),
        "get_db_instance_info" => json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "db_instance_information": collection.db_instance_information
        }),
        "list_available_sql_plans" => tool_list_available_sql_plans(args, stem),
        "get_sql_execution_plan" => tool_get_sql_execution_plan(args, stem),

        // Aggregations
        "list_snapshots" | "list_snapshots_in_range" => tool_list_snapshots(args, collection),
        "top_sqls_in_snapshot" => tool_top_sqls_in_snapshot(args, collection),
        "top_wait_events_in_snapshot" => tool_top_wait_events(args, collection),
        "top_segments_in_snapshot" => tool_top_segments(args, collection),
        "top_latches_in_snapshot" => tool_top_latches(args, collection),

        // Search
        "search_sql_text" => tool_search_sql_text(args, collection),
        "find_sqls_touching_object" => tool_find_sqls_touching_object(args, collection),
        "find_snapshots_with_event" => tool_find_snapshots_with_event(args, collection),
        "find_snapshots_with_sql" => tool_find_snapshots_with_sql(args, collection),
        "find_sqls_by_module" => tool_find_sqls_by_module(args, collection),

        // Time-series & compare
        "get_metric_time_series" => tool_get_metric_time_series(args, collection),
        "get_wait_event_timeline" => tool_get_wait_event_timeline(args, collection),
        "get_sql_timeline" => tool_get_sql_timeline(args, collection),
        "compare_snapshots" => tool_compare_snapshots(args, collection),
        "get_wait_event_histogram" => tool_get_wait_event_histogram(args, collection),

        // Meta
        "list_available_metrics" => tool_list_available_metrics(args, collection),

        other => json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("Unknown tool: {}", other)
        }),
    };

    serde_json::to_string(&result).unwrap_or_else(|e| {
        json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("Serialization failed: {}", e)
        })
        .to_string()
    })
}

// ----------------------------------------------------------------------------
// Shared helpers
// ----------------------------------------------------------------------------

fn find_awr(collection: &AWRSCollection, snap_id: u64) -> Option<&AWR> {
    collection.awrs.iter().find(|a| a.snap_info.begin_snap_id == snap_id)
}

fn arg_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key)?.as_u64()
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)?.as_str()
}

fn arg_f64(args: &Value, key: &str) -> Option<f64> {
    args.get(key)?.as_f64()
}

fn arg_usize(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(default)
}

fn arg_limit(args: &Value, key: &str, default: usize, max: usize) -> usize {
    arg_usize(args, key, default).min(max)
}

fn cmp_desc(a: f64, b: f64) -> std::cmp::Ordering {
    b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
}

fn attachments_dir_for_stem(stem: &str) -> PathBuf {
    // Current JAS-MIN convention used by report generation.
    // Example: report "node1.html" -> "node1_attachments".
    PathBuf::from(format!("{stem}_attachments"))
}

fn is_safe_sql_id(sql_id: &str) -> bool {
    let s = sql_id.trim();

    !s.is_empty()
        && s.len() <= 30
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '$')
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains("..")
}

fn list_xplan_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext.eq_ignore_ascii_case("xplan"))
                        .unwrap_or(false)
            })
            .collect(),
        Err(_) => Vec::new(),
    };

    files.sort();
    files
}

fn sql_id_from_xplan_path(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

fn db_time_of(awr: &AWR) -> f64 {
    awr.time_model_stats
        .iter()
        .find(|t| t.stat_name.eq_ignore_ascii_case("DB time"))
        .map(|t| t.time_s)
        .unwrap_or(0.0)
}

fn db_cpu_of(awr: &AWR) -> f64 {
    awr.time_model_stats
        .iter()
        .find(|t| t.stat_name.eq_ignore_ascii_case("DB CPU"))
        .map(|t| t.time_s)
        .unwrap_or(0.0)
}

fn error_missing_arg(name: &str) -> Value {
    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "error": format!("{} is required", name)
    })
}

fn snapshot_header(awr: &AWR) -> Value {
    let db_time = db_time_of(awr);
    let db_cpu = db_cpu_of(awr);
    json!({
        "snap_id": awr.snap_info.begin_snap_id,
        "begin_snap_time": awr.snap_info.begin_snap_time,
        "end_snap_time": awr.snap_info.end_snap_time,
        "file_name": awr.file_name,
        "db_time_s": db_time,
        "db_cpu_s": db_cpu,
        "db_cpu_dbtime_ratio": if db_time > 0.0 { db_cpu / db_time } else { 0.0 },
        "host_cpu_cpus": awr.host_cpu.cpus,
        "host_cpu_cores": awr.host_cpu.cores,
        "host_cpu_sockets": awr.host_cpu.sockets,
        "host_cpu_load_avg_begin": awr.host_cpu.load_avg_begin,
        "host_cpu_load_avg_end": awr.host_cpu.load_avg_end,
        "host_cpu_pct_user": awr.host_cpu.pct_user,
        "host_cpu_pct_system": awr.host_cpu.pct_system,
        "host_cpu_pct_wio": awr.host_cpu.pct_wio,
        "host_cpu_pct_idle": awr.host_cpu.pct_idle
    })
}

// ============================================================================
// 0. GLOBAL OVERVIEW / TRIAGE
// ============================================================================

fn tool_get_database_load_summary(args: &Value, c: &AWRSCollection) -> Value {
    let top_n = arg_limit(args, "top_n", DEFAULT_TOP_N, MAX_TOP_N);

    let mut snapshots: Vec<Value> = c
        .awrs
        .iter()
        .map(|a| {
            let db_time = db_time_of(a);
            let db_cpu = db_cpu_of(a);
            let top_fg = a
                .foreground_wait_events
                .iter()
                .max_by(|x, y| cmp_desc(y.pct_dbtime, x.pct_dbtime))
                .map(|e| {
                    json!({
                        "event": e.event,
                        "pct_dbtime": e.pct_dbtime,
                        "total_wait_time_s": e.total_wait_time_s,
                        "avg_wait_ms": e.avg_wait,
                        "waits": e.waits
                    })
                })
                .unwrap_or(Value::Null);

            json!({
                "snap_id": a.snap_info.begin_snap_id,
                "begin_snap_time": a.snap_info.begin_snap_time,
                "end_snap_time": a.snap_info.end_snap_time,
                "db_time_s": db_time,
                "db_cpu_s": db_cpu,
                "db_cpu_dbtime_ratio": if db_time > 0.0 { db_cpu / db_time } else { 0.0 },
                "host_cpu_cpus": a.host_cpu.cpus,
                "host_cpu_cores": a.host_cpu.cores,
                "host_cpu_sockets": a.host_cpu.sockets,
                "host_cpu_load_avg_begin": a.host_cpu.load_avg_begin,
                "host_cpu_load_avg_end": a.host_cpu.load_avg_end,
                "host_cpu_pct_user": a.host_cpu.pct_user,
                "host_cpu_pct_system": a.host_cpu.pct_system,
                "host_cpu_pct_wio": a.host_cpu.pct_wio,
                "host_cpu_pct_idle": a.host_cpu.pct_idle,
                "top_fg_event": top_fg
            })
        })
        .collect();

    snapshots.sort_by(|a, b| {
        let av = a["db_time_s"].as_f64().unwrap_or(0.0);
        let bv = b["db_time_s"].as_f64().unwrap_or(0.0);
        cmp_desc(av, bv)
    });

    let total_db_time_s: f64 = c.awrs.iter().map(db_time_of).sum();
    let total_db_cpu_s: f64 = c.awrs.iter().map(db_cpu_of).sum();

    let mut global_waits: HashMap<String, (f64, u64)> = HashMap::new();
    for a in &c.awrs {
        for e in &a.foreground_wait_events {
            let entry = global_waits.entry(e.event.clone()).or_insert((0.0, 0));
            entry.0 += e.total_wait_time_s;
            entry.1 += e.waits;
        }
    }

    let mut wait_rows: Vec<Value> = global_waits
        .into_iter()
        .map(|(event, (total_wait_time_s, waits))| {
            json!({
                "event": event,
                "total_wait_time_s": total_wait_time_s,
                "waits": waits
            })
        })
        .collect();

    wait_rows.sort_by(|a, b| {
        let av = a["total_wait_time_s"].as_f64().unwrap_or(0.0);
        let bv = b["total_wait_time_s"].as_f64().unwrap_or(0.0);
        cmp_desc(av, bv)
    });
    wait_rows.truncate(top_n);

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "db_instance_information": c.db_instance_information,
        "snapshot_count": c.awrs.len(),
        "first_snapshot": c.awrs.first().map(snapshot_header),
        "last_snapshot": c.awrs.last().map(snapshot_header),
        "total_db_time_s": total_db_time_s,
        "total_db_cpu_s": total_db_cpu_s,
        "global_db_cpu_dbtime_ratio": if total_db_time_s > 0.0 { total_db_cpu_s / total_db_time_s } else { 0.0 },
        "sql_text_count": c.sql_text.len(),
        "init_parameter_count": c.initialization_parameters.len(),
        "busiest_snapshots_by_db_time": snapshots.into_iter().take(top_n).collect::<Vec<_>>(),
        "top_foreground_waits_by_total_time": wait_rows
    })
}

// ============================================================================
// 1. POINT LOOKUPS
// ============================================================================

fn tool_get_snapshot_summary(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = match arg_u64(args, "snap_id") {
        Some(v) => v,
        None => return error_missing_arg("snap_id"),
    };
    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("snap_id {} not found", snap_id)
        }),
    };

    let mut top_waits = awr.foreground_wait_events.clone();
    top_waits.sort_by(|a, b| cmp_desc(a.pct_dbtime, b.pct_dbtime));

    let mut top_sql_elapsed = awr.sql_elapsed_time.clone();
    top_sql_elapsed.sort_by(|a, b| cmp_desc(a.elapsed_time_s, b.elapsed_time_s));

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "snapshot": snapshot_header(awr),
        "top_foreground_waits": top_waits.into_iter().take(DEFAULT_TOP_N).collect::<Vec<_>>(),
        "top_sql_elapsed": top_sql_elapsed.into_iter().take(DEFAULT_TOP_N).collect::<Vec<_>>(),
        "load_profile": awr.load_profile,
        "host_cpu": awr.host_cpu
    })
}

fn tool_get_snapshot_details(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = match arg_u64(args, "snap_id") {
        Some(v) => v,
        None => return error_missing_arg("snap_id"),
    };
    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("snap_id {} not found", snap_id)
        }),
    };

    let sections: Option<HashSet<String>> = args
        .get("sections")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());

    let include = |key: &str| sections.as_ref().map_or(true, |s| s.contains(key));

    let mut out = json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "snap_info": awr.snap_info,
        "file_name": awr.file_name,
        "note": if sections.is_some() {
            "Output limited to requested sections."
        } else {
            "Full snapshot output. Prefer section-limited calls for large reports."
        }
    });

    if include("load_profile") {
        out["load_profile"] = json!(awr.load_profile);
    }
    if include("instance_efficiency") {
        out["instance_efficiency"] = json!(awr.instance_efficiency);
    }
    if include("host_cpu") {
        out["host_cpu"] = json!(awr.host_cpu);
    }
    if include("time_model") {
        out["time_model"] = json!(awr.time_model_stats);
    }
    if include("foreground_waits") {
        out["foreground_waits"] = json!(awr.foreground_wait_events);
    }
    if include("background_waits") {
        out["background_waits"] = json!(awr.background_wait_events);
    }
    if include("sql_elapsed") {
        out["sql_elapsed"] = json!(awr.sql_elapsed_time);
    }
    if include("sql_cpu") {
        out["sql_cpu"] = json!(awr.sql_cpu_time);
    }
    if include("sql_io") {
        out["sql_io"] = json!(awr.sql_io_time);
    }
    if include("sql_gets") {
        out["sql_gets"] = json!(awr.sql_gets);
    }
    if include("sql_reads") {
        out["sql_reads"] = json!(awr.sql_reads);
    }
    if include("top_sql_with_top_events") {
        out["top_sql_with_top_events"] = json!(awr.top_sql_with_top_events);
    }
    if include("instance_stats") {
        out["instance_stats"] = json!(awr.instance_stats);
    }
    if include("io_stats_byfunc") {
        out["io_stats_byfunc"] = json!(awr.io_stats_byfunc);
    }
    if include("dictionary_cache") {
        out["dictionary_cache"] = json!(awr.dictionary_cache);
    }
    if include("library_cache") {
        out["library_cache"] = json!(awr.library_cache);
    }
    if include("latch_activity") {
        out["latch_activity"] = json!(awr.latch_activity);
    }
    if include("segment_stats") {
        out["segment_stats"] = json!(awr.segment_stats);
    }
    if include("redo_log") {
        out["redo_log"] = json!(awr.redo_log);
    }

    out
}

fn tool_get_sql_text(args: &Value, c: &AWRSCollection) -> Value {
    let sql_id = match arg_str(args, "sql_id") {
        Some(v) => v,
        None => return error_missing_arg("sql_id"),
    };

    match c.sql_text.get(sql_id) {
        Some(text) => json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "sql_id": sql_id,
            "sql_text": text
        }),
        None => json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "sql_id": sql_id,
            "sql_text": null,
            "note": "SQL text was not collected for this SQL_ID or is not present in source reports."
        }),
    }
}

fn tool_get_init_parameter(args: &Value, c: &AWRSCollection) -> Value {
    let limit = arg_limit(args, "limit", 100, MAX_LIMIT);
    let mut result = serde_json::Map::new();

    if let Some(names) = args.get("names").and_then(|v| v.as_array()) {
        for name in names.iter().filter_map(|v| v.as_str()) {
            let value = c
                .initialization_parameters
                .get(name)
                .cloned()
                .unwrap_or_else(|| "<not present>".to_string());
            result.insert(name.to_string(), json!(value));
        }
    }

    if let Some(pat) = arg_str(args, "pattern") {
        let pat_lc = pat.to_lowercase();
        let mut matches: Vec<(&String, &String)> = c
            .initialization_parameters
            .iter()
            .filter(|(k, _)| k.to_lowercase().contains(&pat_lc))
            .collect();
        matches.sort_by(|a, b| a.0.cmp(b.0));

        for (k, v) in matches.into_iter().take(limit) {
            result.insert(k.clone(), json!(v));
        }
    }

    let total_returned = result.len();
    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "scope": "collection_level",
        "note": "Initialization parameters are stored at collection level, not per snapshot.",
        "parameters": result,
        "total_returned": total_returned,
        "total_available": c.initialization_parameters.len(),
        "limit": limit
    })
}

fn tool_get_sql_execution_plan(args: &Value, stem: &str) -> Value {
    let sql_id = match arg_str(args, "sql_id") {
        Some(v) if is_safe_sql_id(v) => v.trim(),
        Some(v) => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": "Invalid sql_id. Only simple SQL_ID-like filenames are accepted.",
                "sql_id": v
            });
        }
        None => return error_missing_arg("sql_id"),
    };

    let attachments_dir = attachments_dir_for_stem(stem);
    let path = attachments_dir.join(format!("{sql_id}.xplan"));

    if !path.is_file() {
        return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "sql_id": sql_id,
            "filename": path.display().to_string(),
            "plan_text": null,
            "note": "Execution plan attachment was not found."
        });
    }

    let metadata = fs::metadata(&path).ok();
    let size_bytes = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

    let mut text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "sql_id": sql_id,
                "filename": path.display().to_string(),
                "error": format!("Failed to read xplan file: {}", e)
            });
        }
    };

    let truncated = text.len() > MAX_XPLAN_BYTES;
    if truncated {
        text.truncate(MAX_XPLAN_BYTES);
        text.push_str("\n\n-- JAS-MIN NOTE: execution plan output truncated because it exceeded the tool payload limit.\n");
    }

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "sql_id": sql_id,
        "filename": path.file_name().and_then(|f| f.to_str()).unwrap_or("").to_string(),
        "size_bytes": size_bytes,
        "truncated": truncated,
        "max_bytes": MAX_XPLAN_BYTES,
        "analysis_guidance": [
            "Identify the dominant operations by cost, cardinality, bytes, elapsed time evidence, and likely row-source impact.",
            "Check access paths: full scans, index range scans, index unique scans, skip scans, bitmap operations, partition pruning.",
            "Check join methods and join order: nested loops, hash joins, merge joins, cartesian joins, bloom filters.",
            "Compare estimated rows with actual rows if A-Rows/Starts are present; highlight cardinality estimation errors.",
            "Look for expensive sorts, temp spills, remote operations, adaptive plan notes, bind peeking/sensitivity, dynamic sampling, SQL plan directives, parallel execution, and partition-related issues.",
            "Produce concrete recommendations: stats refresh, histograms, extended stats, SQL rewrite, indexing, partitioning, SPM baseline/profile, or bind/literal handling."
        ],
        "plan_text": text
    })
}

fn tool_list_available_sql_plans(args: &Value, stem: &str) -> Value {
    let attachments_dir = attachments_dir_for_stem(stem);
    let pattern = arg_str(args, "pattern").map(|s| s.to_lowercase());
    let limit = arg_limit(args, "limit", DEFAULT_XPLAN_LIMIT, MAX_LIMIT);

    let mut plans: Vec<Value> = list_xplan_files(&attachments_dir)
        .into_iter()
        .filter_map(|path| {
            let filename = path.file_name()?.to_str()?.to_string();
            let filename_lc = filename.to_lowercase();

            if let Some(pattern) = &pattern {
                if !filename_lc.contains(pattern) {
                    return None;
                }
            }

            let sql_id = sql_id_from_xplan_path(&path)?;
            let size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

            Some(json!({
                "sql_id": sql_id,
                "filename": filename,
                "size_bytes": size_bytes
            }))
        })
        .collect();

    plans.sort_by(|a, b| {
        a["sql_id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["sql_id"].as_str().unwrap_or(""))
    });

    let total_matches = plans.len();
    plans.truncate(limit);

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "available": attachments_dir.is_dir(),
        "attachments_dir": attachments_dir.display().to_string(),
        "pattern": pattern,
        "total_matches": total_matches,
        "returned": plans.len(),
        "limit": limit,
        "sql_ids_xplan": plans,
        "usage_hint": "For SQL_IDs that are material to DB Time, DB CPU, elapsed time, I/O, buffer gets, physical reads, or regressions, call get_sql_execution_plan and include a dedicated plan analysis with recommendations."
    })
}


// ============================================================================
// 2. AGGREGATIONS
// ============================================================================

fn tool_list_snapshots(args: &Value, c: &AWRSCollection) -> Value {
    let from = arg_u64(args, "from_snap_id").unwrap_or(0);
    let to = arg_u64(args, "to_snap_id").unwrap_or(u64::MAX);
    let limit = arg_limit(args, "limit", DEFAULT_LIMIT, MAX_LIMIT);

    let list: Vec<Value> = c
        .awrs
        .iter()
        .filter(|a| {
            let id = a.snap_info.begin_snap_id;
            id >= from && id <= to
        })
        .take(limit)
        .map(|a| {
            let top_fg = a
                .foreground_wait_events
                .iter()
                .max_by(|x, y| cmp_desc(y.pct_dbtime, x.pct_dbtime))
                .map(|e| json!({ "event": e.event, "pct_dbtime": e.pct_dbtime }))
                .unwrap_or(Value::Null);

            let mut header = snapshot_header(a);
            header["top_fg_event"] = top_fg;
            header
        })
        .collect();

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "count": list.len(),
        "limit": limit,
        "snapshots": list
    })
}

fn tool_top_sqls_in_snapshot(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = match arg_u64(args, "snap_id") {
        Some(v) => v,
        None => return error_missing_arg("snap_id"),
    };
    let metric = arg_str(args, "metric").unwrap_or("elapsed_time");
    let top_n = arg_limit(args, "top_n", DEFAULT_TOP_N, MAX_TOP_N);

    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("snap_id {} not found", snap_id)
        }),
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
        other => return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("Unknown metric '{}'", other)
        }),
    };

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "snap_id": snap_id,
        "metric": metric,
        "top_n": top_n,
        "items": items
    })
}

fn tool_top_wait_events(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = match arg_u64(args, "snap_id") {
        Some(v) => v,
        None => return error_missing_arg("snap_id"),
    };
    let kind = arg_str(args, "kind").unwrap_or("foreground");
    let top_n = arg_limit(args, "top_n", DEFAULT_TOP_N, MAX_TOP_N);
    let rank_by = arg_str(args, "rank_by").unwrap_or("pct_dbtime");

    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("snap_id {} not found", snap_id)
        }),
    };

    let mut events = if kind == "background" {
        awr.background_wait_events.clone()
    } else {
        awr.foreground_wait_events.clone()
    };

    events.sort_by(|a, b| {
        let (x, y) = match rank_by {
            "total_wait_time_s" => (a.total_wait_time_s, b.total_wait_time_s),
            "avg_wait" => (a.avg_wait, b.avg_wait),
            "waits" => (a.waits as f64, b.waits as f64),
            _ => (a.pct_dbtime, b.pct_dbtime),
        };
        cmp_desc(x, y)
    });

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "snap_id": snap_id,
        "kind": kind,
        "rank_by": rank_by,
        "top_n": top_n,
        "items": events.into_iter().take(top_n).collect::<Vec<_>>()
    })
}

fn tool_top_segments(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = match arg_u64(args, "snap_id") {
        Some(v) => v,
        None => return error_missing_arg("snap_id"),
    };
    let category = match arg_str(args, "category") {
        Some(v) => v,
        None => return error_missing_arg("category"),
    };
    let top_n = arg_limit(args, "top_n", DEFAULT_TOP_N, MAX_TOP_N);

    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("snap_id {} not found", snap_id)
        }),
    };

    match awr.segment_stats.get(category) {
        Some(segs) => {
            let mut v = segs.clone();
            v.sort_by(|a, b| cmp_desc(a.stat_vlalue, b.stat_vlalue));
            json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "snap_id": snap_id,
                "category": category,
                "top_n": top_n,
                "items": v.into_iter().take(top_n).collect::<Vec<_>>()
            })
        }
        None => json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("Category '{}' not found in this snapshot", category),
            "available_categories": awr.segment_stats.keys().cloned().collect::<Vec<_>>()
        }),
    }
}

fn tool_top_latches(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = match arg_u64(args, "snap_id") {
        Some(v) => v,
        None => return error_missing_arg("snap_id"),
    };
    let rank_by = arg_str(args, "rank_by").unwrap_or("wait_time");
    let top_n = arg_limit(args, "top_n", DEFAULT_TOP_N, MAX_TOP_N);

    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("snap_id {} not found", snap_id)
        }),
    };

    let mut v = awr.latch_activity.clone();
    v.sort_by(|a, b| {
        let (x, y) = match rank_by {
            "pct_miss" => (a.get_pct_miss, b.get_pct_miss),
            "get_requests" => (a.get_requests as f64, b.get_requests as f64),
            _ => (a.wait_time, b.wait_time),
        };
        cmp_desc(x, y)
    });

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "snap_id": snap_id,
        "rank_by": rank_by,
        "top_n": top_n,
        "items": v.into_iter().take(top_n).collect::<Vec<_>>()
    })
}

// ============================================================================
// 3. SEARCH
// ============================================================================

fn tool_search_sql_text(args: &Value, c: &AWRSCollection) -> Value {
    let needle_raw = match arg_str(args, "needle") {
        Some(n) => n,
        None => return error_missing_arg("needle"),
    };
    let needle = needle_raw.to_lowercase();
    let limit = arg_limit(args, "limit", 20, MAX_LIMIT);

    let mut matches: Vec<Value> = c
        .sql_text
        .iter()
        .filter(|(_, text)| text.to_lowercase().contains(&needle))
        .map(|(sql_id, text)| {
            let snippet: String = text.chars().take(500).collect();
            json!({
                "sql_id": sql_id,
                "snippet": snippet,
                "full_len": text.len()
            })
        })
        .collect();

    matches.sort_by(|a, b| {
        a["sql_id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["sql_id"].as_str().unwrap_or(""))
    });

    let total_matches = matches.len();
    matches.truncate(limit);

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "needle": needle_raw,
        "total_matches": total_matches,
        "returned": matches.len(),
        "limit": limit,
        "matches": matches
    })
}

fn tool_find_sqls_touching_object(args: &Value, c: &AWRSCollection) -> Value {
    let object_name = match arg_str(args, "object_name") {
        Some(s) => s,
        None => return error_missing_arg("object_name"),
    };
    let mut rewritten = args.clone();
    rewritten["needle"] = json!(object_name);
    let mut result = tool_search_sql_text(&rewritten, c);
    result["object_name"] = json!(object_name);
    result["note"] = json!("This is a SQL text substring search; it may miss dynamic SQL, synonyms, quoted variants or object references absent from collected SQL text.");
    result
}

fn tool_find_snapshots_with_event(args: &Value, c: &AWRSCollection) -> Value {
    let event_name = match arg_str(args, "event_name") {
        Some(n) => n,
        None => return error_missing_arg("event_name"),
    };
    let kind = arg_str(args, "kind").unwrap_or("foreground");
    let min_pct = arg_f64(args, "min_pct_dbtime").unwrap_or(1.0);

    let hits: Vec<Value> = c
        .awrs
        .iter()
        .filter_map(|a| {
            let events = if kind == "background" {
                &a.background_wait_events
            } else {
                &a.foreground_wait_events
            };
            events
                .iter()
                .find(|e| e.event.eq_ignore_ascii_case(event_name) && e.pct_dbtime >= min_pct)
                .map(|e| {
                    json!({
                        "snap_id": a.snap_info.begin_snap_id,
                        "begin_snap_time": a.snap_info.begin_snap_time,
                        "end_snap_time": a.snap_info.end_snap_time,
                        "event": e.event,
                        "waits": e.waits,
                        "total_wait_time_s": e.total_wait_time_s,
                        "avg_wait_ms": e.avg_wait,
                        "pct_dbtime": e.pct_dbtime
                    })
                })
        })
        .collect();

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "event_name": event_name,
        "kind": kind,
        "min_pct_dbtime": min_pct,
        "hit_count": hits.len(),
        "snapshots": hits
    })
}

fn tool_find_snapshots_with_sql(args: &Value, c: &AWRSCollection) -> Value {
    let sql_id = match arg_str(args, "sql_id") {
        Some(s) => s,
        None => return error_missing_arg("sql_id"),
    };

    let hits: Vec<Value> = c
        .awrs
        .iter()
        .filter_map(|a| {
            let mut sections = serde_json::Map::new();

            if let Some(s) = a.sql_elapsed_time.iter().find(|s| s.sql_id == sql_id) {
                sections.insert(
                    "sql_elapsed".to_string(),
                    json!({
                        "elapsed_time_s": s.elapsed_time_s,
                        "executions": s.executions,
                        "elapsed_time_exec_s": s.elpased_time_exec_s,
                        "pct_total": s.pct_total,
                        "pct_cpu": s.pct_cpu,
                        "pct_io": s.pct_io,
                        "module": s.sql_module,
                        "sql_type": s.sql_type
                    }),
                );
            }
            if let Some(s) = a.sql_cpu_time.get(sql_id) {
                sections.insert(
                    "sql_cpu".to_string(),
                    json!({
                        "cpu_time_s": s.cpu_time_s,
                        "executions": s.executions,
                        "cpu_time_exec_s": s.cpu_time_exec_s,
                        "pct_total": s.pct_total,
                        "pct_cpu": s.pct_cpu,
                        "pct_io": s.pct_io,
                        "module": s.sql_module
                    }),
                );
            }
            if let Some(s) = a.sql_io_time.get(sql_id) {
                sections.insert(
                    "sql_io".to_string(),
                    json!({
                        "io_time_s": s.io_time_s,
                        "executions": s.executions,
                        "io_time_exec_s": s.io_time_exec_s,
                        "pct_total": s.pct_total,
                        "pct_cpu": s.pct_cpu,
                        "pct_io": s.pct_io
                    }),
                );
            }
            if let Some(s) = a.sql_gets.get(sql_id) {
                sections.insert(
                    "sql_gets".to_string(),
                    json!({
                        "buffer_gets": s.buffer_gets,
                        "executions": s.executions,
                        "gets_per_exec": s.gets_per_exec,
                        "pct_total": s.pct_total,
                        "pct_cpu": s.pct_cpu,
                        "pct_io": s.pct_io
                    }),
                );
            }
            if let Some(s) = a.sql_reads.get(sql_id) {
                sections.insert(
                    "sql_reads".to_string(),
                    json!({
                        "physical_reads": s.physical_reads,
                        "executions": s.executions,
                        "reads_per_exec": s.reads_per_exec,
                        "pct_total": s.pct_total,
                        "cpu_time_pct": s.cpu_time_pct,
                        "pct_io": s.pct_io
                    }),
                );
            }
            if let Some(s) = a.top_sql_with_top_events.get(sql_id) {
                sections.insert(
                    "top_sql_with_top_events".to_string(),
                    json!({
                        "event_name": s.event_name,
                        "pct_event": s.pct_event,
                        "pct_activity": s.pct_activity,
                        "top_row_source": s.top_row_source,
                        "pct_row_source": s.pct_row_source,
                        "executions": s.executions,
                        "plan_hash_value": s.plan_hash_value
                    }),
                );
            }

            if sections.is_empty() {
                None
            } else {
                Some(json!({
                    "snap_id": a.snap_info.begin_snap_id,
                    "begin_snap_time": a.snap_info.begin_snap_time,
                    "end_snap_time": a.snap_info.end_snap_time,
                    "sections": sections
                }))
            }
        })
        .collect();

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "sql_id": sql_id,
        "hit_count": hits.len(),
        "sql_text_available": c.sql_text.contains_key(sql_id),
        "snapshots": hits
    })
}

fn tool_find_sqls_by_module(args: &Value, c: &AWRSCollection) -> Value {
    let module_pattern = match arg_str(args, "module_pattern") {
        Some(s) => s.to_lowercase(),
        None => return error_missing_arg("module_pattern"),
    };
    let limit = arg_limit(args, "limit", DEFAULT_LIMIT, MAX_LIMIT);

    let mut seen: HashSet<String> = HashSet::new();
    let mut matches: Vec<Value> = Vec::new();

    for a in &c.awrs {
        for s in &a.sql_elapsed_time {
            if s.sql_module.to_lowercase().contains(&module_pattern) && seen.insert(s.sql_id.clone()) {
                matches.push(json!({
                    "sql_id": s.sql_id,
                    "module": s.sql_module,
                    "first_seen_snap_id": a.snap_info.begin_snap_id,
                    "source_section": "sql_elapsed"
                }));
            }
        }
        for s in a.sql_cpu_time.values() {
            if s.sql_module.to_lowercase().contains(&module_pattern) && seen.insert(s.sql_id.clone()) {
                matches.push(json!({
                    "sql_id": s.sql_id,
                    "module": s.sql_module,
                    "first_seen_snap_id": a.snap_info.begin_snap_id,
                    "source_section": "sql_cpu"
                }));
            }
        }
    }

    matches.sort_by(|a, b| {
        a["sql_id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["sql_id"].as_str().unwrap_or(""))
    });

    let total_matches = matches.len();
    matches.truncate(limit);

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "module_pattern": module_pattern,
        "total_matches": total_matches,
        "returned": matches.len(),
        "limit": limit,
        "matches": matches,
        "note": "Search uses SQL sections where sql_module is publicly available in current parser structs."
    })
}

// ============================================================================
// 4. TIME-SERIES / COMPARE
// ============================================================================

fn tool_get_metric_time_series(args: &Value, c: &AWRSCollection) -> Value {
    let kind = match arg_str(args, "kind") {
        Some(k) => k,
        None => return error_missing_arg("kind"),
    };
    let name = match arg_str(args, "name") {
        Some(n) => n,
        None => return error_missing_arg("name"),
    };
    let field = arg_str(args, "field").unwrap_or("value");

    let mut unsupported_note: Option<String> = None;

    let series: Vec<Value> = c
        .awrs
        .iter()
        .filter_map(|a| {
            let value = match kind {
                "load_profile" => a
                    .load_profile
                    .iter()
                    .find(|lp| lp.stat_name.eq_ignore_ascii_case(name))
                    .map(|lp| lp.per_second),

                "instance_stat" => a
                    .instance_stats
                    .iter()
                    .find(|s| s.statname.eq_ignore_ascii_case(name))
                    .map(|s| s.total as f64),

                "wait_event_fg" => a
                    .foreground_wait_events
                    .iter()
                    .find(|e| e.event.eq_ignore_ascii_case(name))
                    .map(|e| match field {
                        "total_wait_time_s" => e.total_wait_time_s,
                        "avg_wait" => e.avg_wait,
                        "waits" => e.waits as f64,
                        _ => e.pct_dbtime,
                    }),

                "wait_event_bg" => a
                    .background_wait_events
                    .iter()
                    .find(|e| e.event.eq_ignore_ascii_case(name))
                    .map(|e| match field {
                        "total_wait_time_s" => e.total_wait_time_s,
                        "avg_wait" => e.avg_wait,
                        "waits" => e.waits as f64,
                        _ => e.pct_dbtime,
                    }),

                "time_model" => a
                    .time_model_stats
                    .iter()
                    .find(|t| t.stat_name.eq_ignore_ascii_case(name))
                    .map(|t| match field {
                        "pct_dbtime" => t.pct_dbtime,
                        _ => t.time_s,
                    }),

                "host_cpu" => match field {
                    "pct_user" | "value" => Some(a.host_cpu.pct_user),
                    "pct_system" => Some(a.host_cpu.pct_system),
                    "pct_wio" => Some(a.host_cpu.pct_wio),
                    "pct_idle" => Some(a.host_cpu.pct_idle),
                    "load_avg_begin" => Some(a.host_cpu.load_avg_begin),
                    "load_avg_end" => Some(a.host_cpu.load_avg_end),
                    "cpus" => Some(a.host_cpu.cpus as f64),
                    "cores" => Some(a.host_cpu.cores as f64),
                    "sockets" => Some(a.host_cpu.sockets as f64),
                    _ => Some(a.host_cpu.pct_user),
                },

                "io_stats_byfunc" => a.io_stats_byfunc.get(name).map(|io| match field {
                    "reads_data" => io.reads_data,
                    "reads_req_s" => io.reads_req_s,
                    "reads_data_s" => io.reads_data_s,
                    "writes_data" => io.writes_data,
                    "writes_req_s" => io.writes_req_s,
                    "writes_data_s" => io.writes_data_s,
                    "waits_count" => io.waits_count as f64,
                    "avg_time" => io.avg_time.unwrap_or(0.0),
                    _ => io.reads_data_s,
                }),

                _ => None,
            };

            value.map(|v| {
                json!({
                    "snap_id": a.snap_info.begin_snap_id,
                    "begin_snap_time": a.snap_info.begin_snap_time,
                    "end_snap_time": a.snap_info.end_snap_time,
                    "value": v
                })
            })
        })
        .collect();

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "kind": kind,
        "name": name,
        "field": field,
        "points": series.len(),
        "series": series,
        "note": unsupported_note
    })
}

fn tool_get_wait_event_timeline(args: &Value, c: &AWRSCollection) -> Value {
    let event_name = match arg_str(args, "event_name") {
        Some(s) => s,
        None => return error_missing_arg("event_name"),
    };
    let kind = arg_str(args, "kind").unwrap_or("foreground");

    let rows: Vec<Value> = c
        .awrs
        .iter()
        .filter_map(|a| {
            let events = if kind == "background" {
                &a.background_wait_events
            } else {
                &a.foreground_wait_events
            };

            events
                .iter()
                .find(|e| e.event.eq_ignore_ascii_case(event_name))
                .map(|e| {
                    json!({
                        "snap_id": a.snap_info.begin_snap_id,
                        "begin_snap_time": a.snap_info.begin_snap_time,
                        "end_snap_time": a.snap_info.end_snap_time,
                        "event": e.event,
                        "waits": e.waits,
                        "total_wait_time_s": e.total_wait_time_s,
                        "avg_wait_ms": e.avg_wait,
                        "pct_dbtime": e.pct_dbtime,
                        "histogram": e.waitevent_histogram_ms
                    })
                })
        })
        .collect();

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "event_name": event_name,
        "kind": kind,
        "points": rows.len(),
        "timeline": rows
    })
}

fn tool_get_sql_timeline(args: &Value, c: &AWRSCollection) -> Value {
    let sql_id = match arg_str(args, "sql_id") {
        Some(s) => s,
        None => return error_missing_arg("sql_id"),
    };

    let rows: Vec<Value> = c
        .awrs
        .iter()
        .filter_map(|a| {
            let elapsed = a.sql_elapsed_time.iter().find(|s| s.sql_id == sql_id);
            let cpu = a.sql_cpu_time.get(sql_id);
            let io = a.sql_io_time.get(sql_id);
            let gets = a.sql_gets.get(sql_id);
            let reads = a.sql_reads.get(sql_id);
            let top_event = a.top_sql_with_top_events.get(sql_id);

            if elapsed.is_none()
                && cpu.is_none()
                && io.is_none()
                && gets.is_none()
                && reads.is_none()
                && top_event.is_none()
            {
                return None;
            }

            Some(json!({
                "snap_id": a.snap_info.begin_snap_id,
                "begin_snap_time": a.snap_info.begin_snap_time,
                "end_snap_time": a.snap_info.end_snap_time,
                "elapsed": elapsed.map(|s| json!({
                    "elapsed_time_s": s.elapsed_time_s,
                    "executions": s.executions,
                    "elapsed_time_exec_s": s.elpased_time_exec_s,
                    "pct_total": s.pct_total,
                    "pct_cpu": s.pct_cpu,
                    "pct_io": s.pct_io,
                    "module": s.sql_module,
                    "sql_type": s.sql_type
                })),
                "cpu": cpu.map(|s| json!({
                    "cpu_time_s": s.cpu_time_s,
                    "executions": s.executions,
                    "cpu_time_exec_s": s.cpu_time_exec_s,
                    "pct_total": s.pct_total,
                    "pct_cpu": s.pct_cpu,
                    "pct_io": s.pct_io,
                    "module": s.sql_module
                })),
                "io": io.map(|s| json!({
                    "io_time_s": s.io_time_s,
                    "executions": s.executions,
                    "io_time_exec_s": s.io_time_exec_s,
                    "pct_total": s.pct_total,
                    "pct_cpu": s.pct_cpu,
                    "pct_io": s.pct_io
                })),
                "gets": gets.map(|s| json!({
                    "buffer_gets": s.buffer_gets,
                    "executions": s.executions,
                    "gets_per_exec": s.gets_per_exec,
                    "pct_total": s.pct_total,
                    "pct_cpu": s.pct_cpu,
                    "pct_io": s.pct_io
                })),
                "reads": reads.map(|s| json!({
                    "physical_reads": s.physical_reads,
                    "executions": s.executions,
                    "reads_per_exec": s.reads_per_exec,
                    "pct_total": s.pct_total,
                    "cpu_time_pct": s.cpu_time_pct,
                    "pct_io": s.pct_io
                })),
                "top_event": top_event.map(|s| json!({
                    "event_name": s.event_name,
                    "pct_event": s.pct_event,
                    "pct_activity": s.pct_activity,
                    "top_row_source": s.top_row_source,
                    "pct_row_source": s.pct_row_source,
                    "executions": s.executions,
                    "plan_hash_value": s.plan_hash_value
                }))
            }))
        })
        .collect();

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "sql_id": sql_id,
        "points": rows.len(),
        "timeline": rows,
        "sql_text_available": c.sql_text.contains_key(sql_id)
    })
}

fn tool_compare_snapshots(args: &Value, c: &AWRSCollection) -> Value {
    let snap_a = match arg_u64(args, "snap_id_a") {
        Some(v) => v,
        None => return error_missing_arg("snap_id_a"),
    };
    let snap_b = match arg_u64(args, "snap_id_b") {
        Some(v) => v,
        None => return error_missing_arg("snap_id_b"),
    };

    let a = match find_awr(c, snap_a) {
        Some(a) => a,
        None => return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("snap_id_a {} not found", snap_a)
        }),
    };
    let b = match find_awr(c, snap_b) {
        Some(b) => b,
        None => return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("snap_id_b {} not found", snap_b)
        }),
    };

    let focus: HashSet<String> = args
        .get("focus")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_else(|| {
            ["load_profile", "waits", "sqls", "latches", "host_cpu", "io"]
                .iter()
                .map(|s| s.to_string())
                .collect()
        });

    let mut out = json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "snap_a": snapshot_header(a),
        "snap_b": snapshot_header(b)
    });

    if focus.contains("load_profile") {
        let map_a: HashMap<&str, f64> = a
            .load_profile
            .iter()
            .map(|lp| (lp.stat_name.as_str(), lp.per_second))
            .collect();
        let mut deltas: Vec<Value> = b
            .load_profile
            .iter()
            .filter_map(|lp| {
                let va = *map_a.get(lp.stat_name.as_str()).unwrap_or(&0.0);
                let vb = lp.per_second;
                let d = vb - va;
                if d.abs() > 0.0 {
                    Some(json!({
                        "stat_name": lp.stat_name,
                        "a_per_sec": va,
                        "b_per_sec": vb,
                        "delta": d
                    }))
                } else {
                    None
                }
            })
            .collect();
        deltas.sort_by(|x, y| {
            let xd = x["delta"].as_f64().unwrap_or(0.0).abs();
            let yd = y["delta"].as_f64().unwrap_or(0.0).abs();
            cmp_desc(xd, yd)
        });
        deltas.truncate(15);
        out["load_profile_deltas"] = json!(deltas);
    }

    if focus.contains("waits") {
        let map_a: HashMap<&str, f64> = a
            .foreground_wait_events
            .iter()
            .map(|e| (e.event.as_str(), e.pct_dbtime))
            .collect();
        let mut deltas: Vec<Value> = b
            .foreground_wait_events
            .iter()
            .map(|e| {
                let va = *map_a.get(e.event.as_str()).unwrap_or(&0.0);
                json!({
                    "event": e.event,
                    "a_pct_dbtime": va,
                    "b_pct_dbtime": e.pct_dbtime,
                    "delta_pct_dbtime": e.pct_dbtime - va,
                    "b_total_wait_time_s": e.total_wait_time_s,
                    "b_waits": e.waits,
                    "b_avg_wait_ms": e.avg_wait
                })
            })
            .collect();
        deltas.sort_by(|x, y| {
            let xd = x["delta_pct_dbtime"].as_f64().unwrap_or(0.0).abs();
            let yd = y["delta_pct_dbtime"].as_f64().unwrap_or(0.0).abs();
            cmp_desc(xd, yd)
        });
        deltas.truncate(15);
        out["fg_wait_event_deltas"] = json!(deltas);
    }

    if focus.contains("sqls") {
        let map_a: HashMap<&str, f64> = a
            .sql_elapsed_time
            .iter()
            .map(|s| (s.sql_id.as_str(), s.elapsed_time_s))
            .collect();
        let map_b: HashMap<&str, f64> = b
            .sql_elapsed_time
            .iter()
            .map(|s| (s.sql_id.as_str(), s.elapsed_time_s))
            .collect();
        let all_keys: HashSet<&str> = map_a.keys().chain(map_b.keys()).copied().collect();

        let mut deltas: Vec<Value> = all_keys
            .into_iter()
            .map(|sql_id| {
                let va = *map_a.get(sql_id).unwrap_or(&0.0);
                let vb = *map_b.get(sql_id).unwrap_or(&0.0);
                json!({
                    "sql_id": sql_id,
                    "a_elapsed_s": va,
                    "b_elapsed_s": vb,
                    "delta_elapsed_s": vb - va,
                    "sql_text_available": c.sql_text.contains_key(sql_id)
                })
            })
            .collect();
        deltas.sort_by(|x, y| {
            let xd = x["delta_elapsed_s"].as_f64().unwrap_or(0.0).abs();
            let yd = y["delta_elapsed_s"].as_f64().unwrap_or(0.0).abs();
            cmp_desc(xd, yd)
        });
        deltas.truncate(20);
        out["sql_elapsed_deltas"] = json!(deltas);
    }

    if focus.contains("latches") {
        let map_a: HashMap<&str, f64> = a
            .latch_activity
            .iter()
            .map(|l| (l.statname.as_str(), l.wait_time))
            .collect();
        let mut deltas: Vec<Value> = b
            .latch_activity
            .iter()
            .map(|l| {
                let va = *map_a.get(l.statname.as_str()).unwrap_or(&0.0);
                json!({
                    "latch": l.statname,
                    "a_wait_s": va,
                    "b_wait_s": l.wait_time,
                    "delta_wait": l.wait_time - va,
                    "b_get_requests": l.get_requests,
                    "b_get_pct_miss": l.get_pct_miss
                })
            })
            .collect();
        deltas.sort_by(|x, y| {
            let xd = x["delta_wait"].as_f64().unwrap_or(0.0).abs();
            let yd = y["delta_wait"].as_f64().unwrap_or(0.0).abs();
            cmp_desc(xd, yd)
        });
        deltas.truncate(15);
        out["latch_deltas"] = json!(deltas);
    }

    if focus.contains("host_cpu") {
        out["host_cpu"] = json!({
            "a": a.host_cpu,
            "b": b.host_cpu
        });
    }

    if focus.contains("io") {
        out["io_stats_byfunc"] = json!({
            "a": a.io_stats_byfunc,
            "b": b.io_stats_byfunc
        });
    }

    out
}

fn tool_get_wait_event_histogram(args: &Value, c: &AWRSCollection) -> Value {
    let snap_id = match arg_u64(args, "snap_id") {
        Some(v) => v,
        None => return error_missing_arg("snap_id"),
    };
    let event = match arg_str(args, "event_name") {
        Some(v) => v,
        None => return error_missing_arg("event_name"),
    };
    let kind = arg_str(args, "kind").unwrap_or("foreground");

    let awr = match find_awr(c, snap_id) {
        Some(a) => a,
        None => return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("snap_id {} not found", snap_id)
        }),
    };

    let events = if kind == "background" {
        &awr.background_wait_events
    } else {
        &awr.foreground_wait_events
    };

    match events.iter().find(|e| e.event.eq_ignore_ascii_case(event)) {
        Some(e) => json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "snap_id": snap_id,
            "event": e.event,
            "kind": kind,
            "histogram": e.waitevent_histogram_ms,
            "summary": {
                "waits": e.waits,
                "total_wait_time_s": e.total_wait_time_s,
                "avg_wait_ms": e.avg_wait,
                "pct_dbtime": e.pct_dbtime
            }
        }),
        None => json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("Event '{}' not found in {} waits of snapshot {}", event, kind, snap_id)
        }),
    }
}

// ============================================================================
// 5. META / DISCOVERY
// ============================================================================

fn tool_list_available_metrics(args: &Value, c: &AWRSCollection) -> Value {
    let kind = match arg_str(args, "kind") {
        Some(k) => k,
        None => return error_missing_arg("kind"),
    };
    let pattern = arg_str(args, "pattern").map(|p| p.to_lowercase());
    let limit = arg_limit(args, "limit", 200, MAX_LIMIT);

    let mut names: HashSet<String> = HashSet::new();

    match kind {
        "load_profile" => {
            for a in &c.awrs {
                for lp in &a.load_profile {
                    names.insert(lp.stat_name.clone());
                }
            }
        }
        "instance_stat" => {
            for a in &c.awrs {
                for s in &a.instance_stats {
                    names.insert(s.statname.clone());
                }
            }
        }
        "wait_event_fg" => {
            for a in &c.awrs {
                for e in &a.foreground_wait_events {
                    names.insert(e.event.clone());
                }
            }
        }
        "wait_event_bg" => {
            for a in &c.awrs {
                for e in &a.background_wait_events {
                    names.insert(e.event.clone());
                }
            }
        }
        "time_model" => {
            for a in &c.awrs {
                for t in &a.time_model_stats {
                    names.insert(t.stat_name.clone());
                }
            }
        }
        "io_stats_byfunc" => {
            for a in &c.awrs {
                for k in a.io_stats_byfunc.keys() {
                    names.insert(k.clone());
                }
            }
        }
        "latch" => {
            for a in &c.awrs {
                for l in &a.latch_activity {
                    names.insert(l.statname.clone());
                }
            }
        }
        "library_cache" => {
            for a in &c.awrs {
                for l in &a.library_cache {
                    names.insert(l.statname.clone());
                }
            }
        }
        "dictionary_cache" => {
            for a in &c.awrs {
                for d in &a.dictionary_cache {
                    names.insert(d.statname.clone());
                }
            }
        }
        "init_parameter" => {
            for k in c.initialization_parameters.keys() {
                names.insert(k.clone());
            }
        }
        "sql_id" => {
            for k in c.sql_text.keys() {
                names.insert(k.clone());
            }
            for a in &c.awrs {
                for s in &a.sql_elapsed_time {
                    names.insert(s.sql_id.clone());
                }
                for k in a.sql_cpu_time.keys() {
                    names.insert(k.clone());
                }
                for k in a.sql_io_time.keys() {
                    names.insert(k.clone());
                }
                for k in a.sql_gets.keys() {
                    names.insert(k.clone());
                }
                for k in a.sql_reads.keys() {
                    names.insert(k.clone());
                }
                for k in a.top_sql_with_top_events.keys() {
                    names.insert(k.clone());
                }
            }
        }
        "segment_category" => {
            for a in &c.awrs {
                for k in a.segment_stats.keys() {
                    names.insert(k.clone());
                }
            }
        }
        other => return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "error": format!("Unknown kind '{}'", other)
        }),
    }

    let mut filtered: Vec<String> = names
        .into_iter()
        .filter(|n| match &pattern {
            Some(p) => n.to_lowercase().contains(p),
            None => true,
        })
        .collect();
    filtered.sort();

    let total_matches = filtered.len();
    filtered.truncate(limit);

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "kind": kind,
        "pattern": pattern,
        "total_matches": total_matches,
        "returned": filtered.len(),
        "limit": limit,
        "names": filtered
    })
}
