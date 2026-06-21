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

use chrono::{NaiveDate, NaiveDateTime};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use toon::encode;

use crate::awr::{AWRSCollection, AWR};

const JASMIN_TOOLS_SCHEMA_VERSION: &str = "2026-05-31.1";
const DEFAULT_LIMIT: usize = 50;
const DEFAULT_TOP_N: usize = 10;
const MAX_LIMIT: usize = 500;
const MAX_TOP_N: usize = 100;
const DEFAULT_XPLAN_LIMIT: usize = 100;
const MAX_XPLAN_BYTES: usize = 512 * 1024;
const DEFAULT_ALERTLOG_LIMIT: usize = 200;
const MAX_ALERTLOG_LIMIT: usize = 1000;
const DEFAULT_AIX_FILE_LIMIT: usize = 100;
const DEFAULT_AIX_RECORD_LIMIT: usize = 200;
const MAX_AIX_FILE_BYTES: usize = 512 * 1024;
const MAX_AIX_SUMMARY_BYTES_PER_FILE: usize = 2 * 1024 * 1024;
const MAX_AIX_RECURSION_DEPTH: usize = 4;

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
    if !list_alertlog_files(&attachments_dir).is_empty() {
        tools.as_array_mut().expect("tools must be a JSON array").push(json!({
            "type": "function",
            "function": {
                "name": "get_alertlog_errors",
                "description": "Parses the Oracle alert.log attachment into a compact structured error stream and returns matching rows as TOON. Use this when the main report mentions parse errors, ORA/TNS errors, incidents, failures, warnings, disconnects, emergency flushes, redo/log allocation problems, or any suspicion that alert.log may contain missing context. Always call this for relevant date ranges before concluding that a reported error symptom has no supporting evidence.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "date_from": {
                            "type": "string",
                            "description": "Start date inclusive, format YYYY-MM-DD. Omit to start from the beginning of alert.log."
                        },
                        "date_to": {
                            "type": "string",
                            "description": "End date inclusive, format YYYY-MM-DD. Omit to read through the end of alert.log."
                        },
                        "code_pattern": {
                            "type": "string",
                            "description": "Optional case-insensitive substring filter for error code/type, e.g. ORA-00904, TNS-, PARSE_ERROR, WARNING."
                        },
                        "include_parse_error_details": {
                            "type": "boolean",
                            "description": "When true, includes parsed details from WARNING: too many parse errors blocks: ORA code, SQL hash, SQL ID, username, application and action."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum rows returned after filtering, default 200, max 1000."
                        }
                    }
                }
            }
        }));

        println!(
            "✅ Found alert.log candidates in {}",
            attachments_dir.display()
        );
    }
    let aix_dir = aix_dir_for_stem(stem);
    if !list_aix_files(&aix_dir).is_empty() {
        tools.as_array_mut().expect("tools must be a JSON array").push(json!({
            "type": "function",
            "function": {
                "name": "list_aix_os_attachments",
                "description": "Lists AIX operating-system attachment files collected under <stem>_attachments/AIX, typically from the oraix collector. If these tools are present, use them before deciding whether an AIX database host is CPU-bound.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Optional case-insensitive filter over relative path or filename, e.g. lparstat, nmon, vmstat, sar, topas, entc."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum files returned, default 100, max 500."
                        }
                    }
                }
            }
        }));

        tools.as_array_mut().expect("tools must be a JSON array").push(json!({
            "type": "function",
            "function": {
                "name": "get_aix_os_attachment",
                "description": "Returns a bounded text excerpt from one AIX OS attachment under <stem>_attachments/AIX. Use this to inspect raw oraix evidence such as lparstat, nmon, sar, vmstat, topas or processor entitlement details.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "relative_path": {
                            "type": "string",
                            "description": "Relative path returned by list_aix_os_attachments. Path traversal and absolute paths are rejected."
                        },
                        "max_bytes": {
                            "type": "integer",
                            "description": "Maximum bytes to return, default 65536, max 524288."
                        }
                    },
                    "required": ["relative_path"]
                }
            }
        }));

        tools.as_array_mut().expect("tools must be a JSON array").push(json!({
            "type": "function",
            "function": {
                "name": "get_aix_cpu_entitlement_summary",
                "description": "Scans AIX OS attachments for CPU entitlement evidence such as Entc%, %entc, physc/pc, entitled capacity/ec, user/sys/wait/idle/busy. On AIX LPARs this is required before classifying the system as CPU-bound because AWR Host CPU %CPU can look low while Entc% is saturated.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Optional case-insensitive filter over relative path or filename. Use it to narrow to lparstat/nmon/sar/vmstat files."
                        },
                        "max_files": {
                            "type": "integer",
                            "description": "Maximum matching files to scan, default 100, max 500."
                        },
                        "limit_records": {
                            "type": "integer",
                            "description": "Maximum parsed sample records returned for inspection, default 200, max 500."
                        }
                    }
                }
            }
        }));

        println!("✅ Found AIX OS attachments in {}", aix_dir.display());
    }
    tools
}

// ----------------------------------------------------------------------------
// Dispatcher
// ----------------------------------------------------------------------------

/// Routes a tool call from the LLM to the matching implementation and returns a
/// JSON-encoded string suitable for a `role: "tool"` chat message.
pub fn dispatch_tool_call(
    name: &str,
    args: &Value,
    collection: &AWRSCollection,
    stem: &str,
) -> String {
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
        "get_alertlog_errors" => tool_get_alertlog_errors(args, stem),
        "list_aix_os_attachments" => tool_list_aix_os_attachments(args, stem),
        "get_aix_os_attachment" => tool_get_aix_os_attachment(args, stem),
        "get_aix_cpu_entitlement_summary" => tool_get_aix_cpu_entitlement_summary(args, stem),

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
    collection
        .awrs
        .iter()
        .find(|a| a.snap_info.begin_snap_id == snap_id)
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

fn aix_dir_for_stem(stem: &str) -> PathBuf {
    attachments_dir_for_stem(stem).join("AIX")
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

fn list_alertlog_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                if !path.is_file() {
                    return false;
                }
                let Some(filename) = path.file_name().and_then(|f| f.to_str()) else {
                    return false;
                };
                let filename_lc = filename.to_lowercase();
                if !filename_lc.contains("alert") {
                    return false;
                }
                match path.extension().and_then(|ext| ext.to_str()) {
                    Some(ext) => matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "log" | "txt" | "trc" | "out"
                    ),
                    None => true,
                }
            })
            .collect(),
        Err(_) => Vec::new(),
    };

    files.sort_by(|a, b| {
        let size_a = fs::metadata(a).map(|m| m.len()).unwrap_or(0);
        let size_b = fs::metadata(b).map(|m| m.len()).unwrap_or(0);
        size_b.cmp(&size_a).then_with(|| a.cmp(b))
    });
    files
}

fn collect_files_recursive(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > MAX_AIX_RECURSION_DEPTH {
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_file() {
            files.push(path);
        } else if file_type.is_dir() {
            collect_files_recursive(&path, depth + 1, files);
        }
    }
}

fn list_aix_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files_recursive(dir, 0, &mut files);
    files.sort();
    files
}

fn relative_display_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_safe_relative_path(relative_path: &str) -> bool {
    let trimmed = relative_path.trim();
    if trimmed.is_empty() {
        return false;
    }

    let path = Path::new(trimmed);
    if path.is_absolute() {
        return false;
    }

    path.components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn read_limited_lossy(path: &Path, max_bytes: usize) -> Result<(String, u64, bool), String> {
    let mut bytes = fs::read(path).map_err(|e| format!("Failed to read file: {}", e))?;
    let size_bytes = bytes.len() as u64;
    let truncated = bytes.len() > max_bytes;
    if truncated {
        bytes.truncate(max_bytes);
    }

    Ok((
        String::from_utf8_lossy(&bytes).to_string(),
        size_bytes,
        truncated,
    ))
}

fn clean_aix_line(line: &str) -> String {
    let mut cleaned = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            while let Some(next) = chars.next() {
                if next.is_ascii_alphabetic() || next == '@' || next == '~' {
                    break;
                }
            }
            continue;
        }

        if ch.is_control() {
            if ch == '\t' {
                cleaned.push(' ');
            }
            continue;
        }

        cleaned.push(ch);
    }

    cleaned
}

fn split_aix_fields(line: &str) -> Vec<String> {
    let cleaned = clean_aix_line(line);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let parts: Vec<&str> = if trimmed.contains(',') {
        trimmed.split(',').collect()
    } else {
        trimmed.split_whitespace().collect()
    };

    parts
        .into_iter()
        .map(|part| {
            part.trim()
                .trim_matches('"')
                .trim_matches('\'')
                .trim()
                .to_string()
        })
        .filter(|part| !part.is_empty())
        .collect()
}

fn normalize_aix_key(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn parse_aix_number(value: &str) -> Option<f64> {
    let cleaned = value
        .trim()
        .trim_matches('%')
        .trim_matches('[')
        .trim_matches(']')
        .trim_matches(',')
        .trim_matches(';')
        .trim_matches(':')
        .trim_matches('=');

    if cleaned.is_empty() {
        return None;
    }

    cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[derive(Debug, Clone, Default, Serialize)]
struct AixCpuObservation {
    source_file: String,
    line_number: usize,
    timestamp_hint: Option<String>,
    entc_pct: Option<f64>,
    cpu_busy_pct: Option<f64>,
    cpu_user_pct: Option<f64>,
    cpu_sys_pct: Option<f64>,
    cpu_wait_pct: Option<f64>,
    cpu_idle_pct: Option<f64>,
    physc: Option<f64>,
    entitled_capacity: Option<f64>,
    raw_sample: String,
}

fn apply_aix_metric(obs: &mut AixCpuObservation, key: &str, value: f64) -> bool {
    let normalized = normalize_aix_key(key);
    if normalized.is_empty() {
        return false;
    }

    if normalized.contains("entc") {
        obs.entc_pct = Some(value);
    } else if normalized == "physc"
        || normalized == "pc"
        || (normalized.contains("physicalcpu") && !normalized.contains("percentage"))
        || normalized.contains("physicalprocessor")
    {
        obs.physc = Some(value);
    } else if normalized == "ec" {
        obs.entc_pct = Some(value);
    } else if normalized == "ent"
        || normalized == "entitled"
        || normalized.contains("entitledcapacity")
        || normalized.contains("entitlement")
    {
        obs.entitled_capacity = Some(value);
    } else if normalized.contains("user") || normalized == "usr" || normalized == "us" {
        obs.cpu_user_pct = Some(value);
    } else if normalized == "kern" || normalized == "kernel" {
        obs.cpu_sys_pct = Some(value);
    } else if normalized == "sys"
        || normalized == "sy"
        || normalized.ends_with("sys")
        || normalized.contains("system")
    {
        obs.cpu_sys_pct = Some(value);
    } else if normalized.contains("wait") || normalized == "wio" || normalized == "wa" {
        obs.cpu_wait_pct = Some(value);
    } else if normalized.contains("idle") || normalized == "id" {
        obs.cpu_idle_pct = Some(value);
    } else if normalized == "busy" || normalized == "lbusy" || normalized.contains("cpubusy") {
        obs.cpu_busy_pct = Some(value);
    } else {
        return false;
    }

    true
}

fn finalize_aix_observation(mut obs: AixCpuObservation) -> Option<AixCpuObservation> {
    if obs.entc_pct.is_none() {
        if let (Some(physc), Some(entitled_capacity)) = (obs.physc, obs.entitled_capacity) {
            if entitled_capacity > 0.0 {
                obs.entc_pct = Some((physc / entitled_capacity) * 100.0);
            }
        }
    }

    if obs.cpu_busy_pct.is_none() {
        if let Some(idle) = obs.cpu_idle_pct {
            if (0.0..=100.0).contains(&idle) {
                obs.cpu_busy_pct = Some(100.0 - idle);
            }
        }
    }

    let has_metric = obs.entc_pct.is_some()
        || obs.cpu_busy_pct.is_some()
        || obs.cpu_user_pct.is_some()
        || obs.cpu_sys_pct.is_some()
        || obs.cpu_wait_pct.is_some()
        || obs.cpu_idle_pct.is_some()
        || obs.physc.is_some()
        || obs.entitled_capacity.is_some();

    has_metric.then_some(obs)
}

fn looks_like_aix_header(fields: &[String]) -> bool {
    let mut has_metric_label = false;
    let mut has_non_numeric = false;

    for field in fields {
        if parse_aix_number(field).is_none() {
            has_non_numeric = true;
        }

        let normalized = normalize_aix_key(field);
        if normalized.contains("entc")
            || normalized == "physc"
            || normalized == "pc"
            || normalized == "ec"
            || normalized.contains("entitledcapacity")
            || normalized.contains("user")
            || normalized == "usr"
            || normalized == "sys"
            || normalized.contains("system")
            || normalized.contains("wait")
            || normalized == "wio"
            || normalized.contains("idle")
            || normalized == "busy"
            || normalized == "lbusy"
            || normalized.contains("cpubusy")
        {
            has_metric_label = true;
        }
    }

    has_metric_label && has_non_numeric
}

fn timestamp_hint_from_fields(fields: &[String]) -> Option<String> {
    let time_re = Regex::new(r"^\d{1,2}:\d{2}(?::\d{2})?$").expect("valid time regex");
    let date_re =
        Regex::new(r"^\d{4}-\d{2}-\d{2}$|^\d{1,2}/\d{1,2}/\d{2,4}$").expect("valid date regex");

    let mut hints = Vec::new();
    for field in fields.iter().take(6) {
        if time_re.is_match(field) || date_re.is_match(field) || field.starts_with('T') {
            hints.push(field.clone());
        }
    }

    if hints.is_empty() {
        None
    } else {
        Some(hints.join(" "))
    }
}

fn parse_aix_row_with_header(
    source_file: &str,
    line_number: usize,
    line: &str,
    header: &[String],
    fields: &[String],
) -> Option<AixCpuObservation> {
    if fields.is_empty() || header.is_empty() {
        return None;
    }

    let mut obs = AixCpuObservation {
        source_file: source_file.to_string(),
        line_number,
        timestamp_hint: timestamp_hint_from_fields(fields),
        raw_sample: trim_sample(line, 240),
        ..Default::default()
    };

    let mut matched = false;
    for (idx, key) in header.iter().enumerate() {
        let Some(value_field) = fields.get(idx) else {
            continue;
        };
        let Some(value) = parse_aix_number(value_field) else {
            continue;
        };
        matched |= apply_aix_metric(&mut obs, key, value);
    }

    matched
        .then_some(())
        .and_then(|_| finalize_aix_observation(obs))
}

fn parse_aix_key_value_line(
    source_file: &str,
    line_number: usize,
    line: &str,
    fields: &[String],
) -> Option<AixCpuObservation> {
    if fields.is_empty() {
        return None;
    }

    let mut obs = AixCpuObservation {
        source_file: source_file.to_string(),
        line_number,
        timestamp_hint: timestamp_hint_from_fields(fields),
        raw_sample: trim_sample(line, 240),
        ..Default::default()
    };

    let mut matched = false;
    for idx in 0..fields.len() {
        let field = &fields[idx];

        if let Some((key, value)) = field.split_once('=').or_else(|| field.split_once(':')) {
            if let Some(number) = parse_aix_number(value) {
                matched |= apply_aix_metric(&mut obs, key, number);
                continue;
            }
        }

        if let Some(next) = fields.get(idx + 1) {
            if let Some(number) = parse_aix_number(next) {
                matched |= apply_aix_metric(&mut obs, field, number);
            }
        }
    }

    matched
        .then_some(())
        .and_then(|_| finalize_aix_observation(obs))
}

fn parse_topas_cpu_line(
    source_file: &str,
    line_number: usize,
    line: &str,
) -> Option<AixCpuObservation> {
    let fields = split_aix_fields(line);
    if fields.is_empty() {
        return None;
    }

    let starts_with_total = fields
        .first()
        .map(|field| field.eq_ignore_ascii_case("total"))
        .unwrap_or(false);
    let starts_with_whitespace = line
        .chars()
        .next()
        .map(|ch| ch.is_whitespace())
        .unwrap_or(false);

    if !starts_with_total && !starts_with_whitespace {
        return None;
    }

    let numeric_values: Vec<f64> = fields
        .iter()
        .filter_map(|field| parse_aix_number(field))
        .collect();

    if numeric_values.len() < 6 {
        return None;
    }

    let values = &numeric_values[0..6];

    let user = values[0];
    let sys = values[1];
    let wait = values[2];
    let idle = values[3];
    let physc = values[4];
    let entc = values[5];
    let pct_sum = user + sys + wait + idle;

    if !(0.0..=100.0).contains(&user)
        || !(0.0..=100.0).contains(&sys)
        || !(0.0..=100.0).contains(&wait)
        || !(0.0..=100.0).contains(&idle)
        || !(80.0..=120.0).contains(&pct_sum)
        || !(0.0..=64.0).contains(&physc)
        || !(0.0..=200.0).contains(&entc)
    {
        return None;
    }

    finalize_aix_observation(AixCpuObservation {
        source_file: source_file.to_string(),
        line_number,
        timestamp_hint: timestamp_hint_from_fields(&fields),
        entc_pct: Some(entc),
        cpu_busy_pct: Some(user + sys + wait),
        cpu_user_pct: Some(user),
        cpu_sys_pct: Some(sys),
        cpu_wait_pct: Some(wait),
        cpu_idle_pct: Some(idle),
        physc: Some(physc),
        entitled_capacity: None,
        raw_sample: trim_sample(line, 240),
    })
}

fn aix_stat(values: &[f64]) -> Value {
    let mut finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    finite.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    if finite.is_empty() {
        return Value::Null;
    }

    let count = finite.len();
    let min = finite[0];
    let max = finite[count - 1];
    let avg = finite.iter().sum::<f64>() / count as f64;
    let p95_idx = ((count as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(count - 1);

    json!({
        "count": count,
        "min": min,
        "avg": avg,
        "p95": finite[p95_idx],
        "max": max
    })
}

fn aix_threshold_counts(values: &[f64]) -> Value {
    let finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    let count = finite.len();

    if count == 0 {
        return Value::Null;
    }

    let count_ge = |threshold: f64| finite.iter().filter(|v| **v >= threshold).count();
    let pct = |n: usize| (n as f64 / count as f64) * 100.0;

    let ge_90 = count_ge(90.0);
    let ge_95 = count_ge(95.0);
    let ge_98 = count_ge(98.0);

    json!({
        "sample_count": count,
        "ge_90_count": ge_90,
        "ge_90_pct": pct(ge_90),
        "ge_95_count": ge_95,
        "ge_95_pct": pct(ge_95),
        "ge_98_count": ge_98,
        "ge_98_pct": pct(ge_98)
    })
}

fn stat_value(stats: &Value, key: &str) -> Option<f64> {
    stats.get(key).and_then(|v| v.as_f64())
}

fn parse_date_arg(args: &Value, key: &str) -> Result<Option<NaiveDate>, String> {
    match arg_str(args, key) {
        Some(value) if !value.trim().is_empty() => {
            NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
                .map(Some)
                .map_err(|_| format!("{key} must use YYYY-MM-DD format"))
        }
        _ => Ok(None),
    }
}

fn timestamp_from_alertlog_line(line: &str, ts_re: &Regex) -> Option<NaiveDateTime> {
    let caps = ts_re.captures(line)?;
    let date = caps.name("date")?.as_str();
    let hour = caps.name("hour")?.as_str();
    let minute = caps.name("minute")?.as_str();
    let second = caps.name("second")?.as_str();
    NaiveDateTime::parse_from_str(
        &format!("{date} {hour}:{minute}:{second}"),
        "%Y-%m-%d %H:%M:%S",
    )
    .ok()
}

fn ora_description(code: i64) -> &'static str {
    match code {
        904 => "invalid identifier",
        942 => "table or view does not exist",
        6550 => "PL/SQL compilation error",
        1722 => "invalid number",
        1403 => "no data found",
        1008 => "not all variables bound",
        1036 => "illegal variable name/number",
        _ => "",
    }
}

fn format_ora_code(code: i64) -> String {
    format!("ORA-{code:05}")
}

#[derive(Debug, Clone, Serialize)]
struct AlertlogEvent {
    timestamp: String,
    date: String,
    code: String,
    description: String,
    occurrences: u64,
    total: u64,
    sample: String,
}

#[derive(Debug, Clone, Serialize)]
struct AlertlogParseDetail {
    timestamp: String,
    date: String,
    code: String,
    description: String,
    count: u64,
    sql_hash: String,
    sqlid: String,
    username: String,
    application: String,
    action: String,
    sample: String,
}

#[derive(Debug, Clone, Serialize)]
struct AlertlogPayload {
    schema_version: String,
    source_file: String,
    date_from: Option<String>,
    date_to: Option<String>,
    code_pattern: Option<String>,
    truncated: bool,
    returned_events: usize,
    returned_parse_error_details: usize,
    matching_events_total: usize,
    matching_parse_error_details_total: usize,
    events: Vec<AlertlogEvent>,
    parse_error_details: Vec<AlertlogParseDetail>,
}

#[derive(Debug)]
struct PendingAlertlogParseDetail {
    timestamp: NaiveDateTime,
    count: u64,
    sql_hash: String,
    code: String,
    description: String,
    ospid: String,
    sqlid: String,
    username: String,
    application: String,
    action: String,
    sample: String,
    lines_seen: usize,
}

impl PendingAlertlogParseDetail {
    fn to_detail(&self) -> AlertlogParseDetail {
        AlertlogParseDetail {
            timestamp: self.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
            date: self.timestamp.date().to_string(),
            code: self.code.clone(),
            description: self.description.clone(),
            count: self.count,
            sql_hash: self.sql_hash.clone(),
            sqlid: self.sqlid.clone(),
            username: self.username.clone(),
            application: self.application.clone(),
            action: self.action.clone(),
            sample: self.sample.clone(),
        }
    }
}

fn trim_sample(line: &str, max_chars: usize) -> String {
    line.trim().chars().take(max_chars).collect()
}

fn detect_alertlog_event(
    line: &str,
    ora_re: &Regex,
    tns_re: &Regex,
    too_many_parse_errors_re: &Regex,
    parse_error_re: &Regex,
    special_patterns: &[(&str, Regex)],
) -> Option<(String, String, u64)> {
    let lower = line.to_lowercase();
    if !(lower.contains("ora-")
        || lower.contains("tns-")
        || lower.contains("fatal ni connect error")
        || lower.contains("cannot allocate new log")
        || lower.contains("private strand flush not complete")
        || lower.contains("emergency flush")
        || lower.contains("warning")
        || lower.contains("error")
        || lower.contains("incident")
        || lower.contains("failed")
        || lower.contains("terminating"))
    {
        return None;
    }

    if let Some(caps) = too_many_parse_errors_re.captures(line) {
        let count = caps
            .name("count")
            .and_then(|m| m.as_str().parse::<u64>().ok())
            .unwrap_or(1);
        return Some((
            "WARNING_TOO_MANY_PARSE_ERRORS".to_string(),
            format!("Oracle reported too many parse errors, count={count}"),
            count,
        ));
    }

    if let Some(caps) = parse_error_re.captures(line) {
        let code_num = caps
            .name("code")
            .and_then(|m| m.as_str().parse::<i64>().ok())
            .unwrap_or(0);
        return Some((
            format!("PARSE_ERROR_{}", format_ora_code(code_num)),
            ora_description(code_num).to_string(),
            1,
        ));
    }

    if let Some(caps) = ora_re.captures(line) {
        let code = caps.name("code")?.as_str().to_uppercase();
        return Some((code.clone(), line.trim().to_string(), 1));
    }

    if let Some(caps) = tns_re.captures(line) {
        let code = caps.name("code")?.as_str().to_uppercase();
        return Some((code.clone(), line.trim().to_string(), 1));
    }

    for (event_type, pattern) in special_patterns {
        if pattern.is_match(line) {
            return Some((event_type.to_string(), line.trim().to_string(), 1));
        }
    }

    None
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
        None => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": format!("snap_id {} not found", snap_id)
            })
        }
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
        None => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": format!("snap_id {} not found", snap_id)
            })
        }
    };

    let sections: Option<HashSet<String>> =
        args.get("sections").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

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

fn tool_list_aix_os_attachments(args: &Value, stem: &str) -> Value {
    let aix_dir = aix_dir_for_stem(stem);
    let pattern = arg_str(args, "pattern").map(|s| s.trim().to_lowercase());
    let limit = arg_limit(args, "limit", DEFAULT_AIX_FILE_LIMIT, MAX_LIMIT);

    let mut files: Vec<Value> = list_aix_files(&aix_dir)
        .into_iter()
        .filter_map(|path| {
            let relative_path = relative_display_path(&aix_dir, &path);
            let filename = path.file_name()?.to_str()?.to_string();
            let haystack = format!(
                "{} {}",
                relative_path.to_lowercase(),
                filename.to_lowercase()
            );

            if let Some(pattern) = &pattern {
                if !haystack.contains(pattern) {
                    return None;
                }
            }

            let size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            Some(json!({
                "relative_path": relative_path,
                "filename": filename,
                "size_bytes": size_bytes
            }))
        })
        .collect();

    files.sort_by(|a, b| {
        a["relative_path"]
            .as_str()
            .unwrap_or("")
            .cmp(b["relative_path"].as_str().unwrap_or(""))
    });

    let total_matches = files.len();
    files.truncate(limit);

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "available": aix_dir.is_dir(),
        "aix_dir": aix_dir.display().to_string(),
        "pattern": pattern,
        "total_matches": total_matches,
        "returned": files.len(),
        "limit": limit,
        "files": files,
        "usage_hint": "If the database platform is AIX, inspect these OS attachments before CPU-bound conclusions. Prefer get_aix_cpu_entitlement_summary first, then get_aix_os_attachment for the most relevant raw files."
    })
}

fn tool_get_aix_os_attachment(args: &Value, stem: &str) -> Value {
    let relative_path = match arg_str(args, "relative_path") {
        Some(v) if is_safe_relative_path(v) => v.trim(),
        Some(v) => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": "Invalid relative_path. Only simple relative paths under the AIX attachment directory are accepted.",
                "relative_path": v
            });
        }
        None => return error_missing_arg("relative_path"),
    };

    let max_bytes = arg_limit(args, "max_bytes", 64 * 1024, MAX_AIX_FILE_BYTES);
    let aix_dir = aix_dir_for_stem(stem);
    let path = aix_dir.join(relative_path);

    if !path.is_file() {
        return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "available": aix_dir.is_dir(),
            "aix_dir": aix_dir.display().to_string(),
            "relative_path": relative_path,
            "text": null,
            "error": "AIX attachment file was not found."
        });
    }

    let (text, size_bytes, truncated) = match read_limited_lossy(&path, max_bytes) {
        Ok(v) => v,
        Err(e) => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "aix_dir": aix_dir.display().to_string(),
                "relative_path": relative_path,
                "error": e
            });
        }
    };

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "aix_dir": aix_dir.display().to_string(),
        "relative_path": relative_path,
        "size_bytes": size_bytes,
        "truncated": truncated,
        "max_bytes": max_bytes,
        "text": text,
        "usage_hint": "Use this raw AIX OS evidence to verify CPU entitlement, capped/shared LPAR behavior, physc/pc usage, and Entc% before declaring the system CPU-bound."
    })
}

fn tool_get_aix_cpu_entitlement_summary(args: &Value, stem: &str) -> Value {
    let aix_dir = aix_dir_for_stem(stem);
    let pattern = arg_str(args, "pattern").map(|s| s.trim().to_lowercase());
    let max_files = arg_limit(args, "max_files", DEFAULT_AIX_FILE_LIMIT, MAX_LIMIT);
    let limit_records = arg_limit(args, "limit_records", DEFAULT_AIX_RECORD_LIMIT, MAX_LIMIT);

    let mut candidate_files: Vec<PathBuf> = list_aix_files(&aix_dir)
        .into_iter()
        .filter(|path| {
            let relative_path = relative_display_path(&aix_dir, path);
            let haystack = relative_path.to_lowercase();
            pattern
                .as_ref()
                .map(|pattern| haystack.contains(pattern))
                .unwrap_or(true)
        })
        .collect();
    candidate_files.truncate(max_files);

    let mut observations = Vec::new();
    let mut scanned_files = Vec::new();
    let mut truncated_files = Vec::new();

    for path in &candidate_files {
        let relative_path = relative_display_path(&aix_dir, path);
        let is_topas_file = relative_path.to_lowercase().contains("topas");
        let (text, size_bytes, truncated) =
            match read_limited_lossy(path, MAX_AIX_SUMMARY_BYTES_PER_FILE) {
                Ok(v) => v,
                Err(_) => continue,
            };

        scanned_files.push(json!({
            "relative_path": relative_path,
            "size_bytes": size_bytes,
            "truncated": truncated
        }));
        if truncated {
            truncated_files.push(relative_display_path(&aix_dir, path));
        }

        let mut header: Option<Vec<String>> = None;
        for (idx, line) in text.split(|c| c == '\n' || c == '\r').enumerate() {
            let line_number = idx + 1;
            let clean_line = clean_aix_line(line);
            let fields = split_aix_fields(&clean_line);
            if fields.is_empty() {
                continue;
            }

            if is_topas_file {
                if let Some(obs) = parse_topas_cpu_line(&relative_path, line_number, &clean_line) {
                    observations.push(obs);
                    continue;
                }
            }

            if looks_like_aix_header(&fields) {
                if let Some(obs) =
                    parse_aix_key_value_line(&relative_path, line_number, &clean_line, &fields)
                {
                    observations.push(obs);
                    continue;
                }
                header = Some(fields.clone());
                continue;
            }

            if let Some(header_fields) = &header {
                if let Some(obs) = parse_aix_row_with_header(
                    &relative_path,
                    line_number,
                    &clean_line,
                    header_fields,
                    &fields,
                ) {
                    observations.push(obs);
                    continue;
                }
            }

            if let Some(obs) =
                parse_aix_key_value_line(&relative_path, line_number, &clean_line, &fields)
            {
                observations.push(obs);
            }
        }
    }

    let entc_values: Vec<f64> = observations.iter().filter_map(|o| o.entc_pct).collect();
    let busy_values: Vec<f64> = observations.iter().filter_map(|o| o.cpu_busy_pct).collect();
    let user_values: Vec<f64> = observations.iter().filter_map(|o| o.cpu_user_pct).collect();
    let sys_values: Vec<f64> = observations.iter().filter_map(|o| o.cpu_sys_pct).collect();
    let wait_values: Vec<f64> = observations.iter().filter_map(|o| o.cpu_wait_pct).collect();
    let idle_values: Vec<f64> = observations.iter().filter_map(|o| o.cpu_idle_pct).collect();
    let physc_values: Vec<f64> = observations.iter().filter_map(|o| o.physc).collect();
    let entitled_values: Vec<f64> = observations
        .iter()
        .filter_map(|o| o.entitled_capacity)
        .collect();

    let entc_stats = aix_stat(&entc_values);
    let busy_stats = aix_stat(&busy_values);
    let max_entc = stat_value(&entc_stats, "max");
    let p95_entc = stat_value(&entc_stats, "p95");
    let avg_entc = stat_value(&entc_stats, "avg");
    let max_busy = stat_value(&busy_stats, "max");

    let entc_pressure = aix_threshold_counts(&entc_values);

    let cpu_bound_assessment = if let Some(max_entc) = max_entc {
        if max_entc >= 98.0 || p95_entc.unwrap_or(0.0) >= 95.0 {
            "AIX CPU entitlement/physical-capacity saturation is confirmed. Treat the host as CPU-bound or CPU-constrained even if AWR Host CPU %CPU/idle looks lower, the LPAR is uncapped, or the shared pool has theoretical spare capacity. Entc% and physc/pc are the controlling evidence."
        } else if max_entc >= 90.0 || p95_entc.unwrap_or(0.0) >= 85.0 {
            "AIX CPU entitlement saturation is likely. Treat the host as CPU-bound or CPU-constrained unless a narrower time-window proves otherwise; do not dismiss this because AWR Host CPU %CPU is lower."
        } else if avg_entc.unwrap_or(0.0) >= 75.0 || max_busy.unwrap_or(0.0) >= 85.0 {
            "AIX CPU pressure is possible but not conclusively saturated. Correlate Entc%, physc/pc, busy/idle and DB CPU timelines before declaring CPU-bound."
        } else {
            "Parsed Entc% does not show LPAR entitlement saturation. Do not call the system CPU-bound from AIX entitlement data alone."
        }
    } else if max_busy.unwrap_or(0.0) >= 85.0 {
        "High CPU busy was parsed, but Entc% was not found. On AIX this is insufficient for a final CPU-bound decision; ask for Entc%, physc/pc, EC, capped/uncapped and shared/dedicated LPAR details."
    } else {
        "No Entc%/LPAR entitlement metric was parsed. On AIX, do not conclude CPU-bound from DB CPU/DB Time or AWR Host CPU %CPU alone; ask for Entc%, physc/pc, EC, capped/uncapped and shared/dedicated LPAR details."
    };

    let total_observations = observations.len();
    let records: Vec<AixCpuObservation> = observations.into_iter().take(limit_records).collect();
    let records_value = serde_json::to_value(&records).unwrap_or_else(|_| json!([]));
    let records_toon = encode(&records_value, None);

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "available": aix_dir.is_dir(),
        "aix_dir": aix_dir.display().to_string(),
        "pattern": pattern,
        "scanned_file_count": scanned_files.len(),
        "scanned_files": scanned_files,
        "truncated_files": truncated_files,
        "total_observations": total_observations,
        "returned_records": records.len(),
        "stats": {
            "entc_pct": entc_stats,
            "cpu_busy_pct": busy_stats,
            "cpu_user_pct": aix_stat(&user_values),
            "cpu_sys_pct": aix_stat(&sys_values),
            "cpu_wait_pct": aix_stat(&wait_values),
            "cpu_idle_pct": aix_stat(&idle_values),
            "physc": aix_stat(&physc_values),
            "entitled_capacity": aix_stat(&entitled_values)
        },
        "entc_pressure": entc_pressure,
        "cpu_bound_assessment": cpu_bound_assessment,
        "decision_guardrail": "On AIX, high Entc%/%entc/ec or physc/pc near entitled/max capacity overrides apparently comfortable AWR Host CPU idle. Do not conclude 'not CPU-bound' from uncapped mode, shared-pool spare capacity, or AWR %CPU alone.",
        "format": "TOON",
        "aix_cpu_records_toon": records_toon,
        "usage_hint": "For AIX, Entc%/%entc/ec is required evidence for CPU-bound decisions. If Entc% is high (commonly >= 90%, and especially p95 >= 95% or max >= 98%), report CPU entitlement/physical-capacity pressure even when AWR Host CPU idle is nonzero. If Entc% is absent, derive it from physc/pc divided by entitled capacity when possible; otherwise ask for AIX LPAR entitlement details before final CPU-bound classification."
    })
}

fn tool_get_alertlog_errors(args: &Value, stem: &str) -> Value {
    let attachments_dir = attachments_dir_for_stem(stem);
    let alertlogs = list_alertlog_files(&attachments_dir);
    let Some(path) = alertlogs.first() else {
        return json!({
            "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
            "available": false,
            "attachments_dir": attachments_dir.display().to_string(),
            "error": "No alert.log-like attachment found."
        });
    };

    let date_from = match parse_date_arg(args, "date_from") {
        Ok(v) => v,
        Err(e) => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": e
            })
        }
    };
    let date_to = match parse_date_arg(args, "date_to") {
        Ok(v) => v,
        Err(e) => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": e
            })
        }
    };
    if let (Some(from), Some(to)) = (date_from, date_to) {
        if from > to {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": "date_from cannot be later than date_to"
            });
        }
    }

    let code_pattern = arg_str(args, "code_pattern")
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let include_parse_error_details = args
        .get("include_parse_error_details")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let limit = arg_limit(args, "limit", DEFAULT_ALERTLOG_LIMIT, MAX_ALERTLOG_LIMIT);

    let ts_re = Regex::new(
        r"^(?P<date>\d{4}-\d{2}-\d{2})T(?P<hour>\d{2}):(?P<minute>\d{2}):(?P<second>\d{2})(?:\.\d+)?(?:[+-]\d{2}:\d{2}|Z)?",
    )
    .expect("valid timestamp regex");
    let ora_re = Regex::new(r"(?i)\b(?P<code>ORA-\d{4,5})\b").expect("valid ORA regex");
    let tns_re = Regex::new(r"(?i)\b(?P<code>TNS-\d{5})\b").expect("valid TNS regex");
    let too_many_parse_errors_re =
        Regex::new(r"(?i)\bWARNING:\s*too many parse errors,\s*count=(?P<count>\d+)\b")
            .expect("valid parse warning regex");
    let parse_error_re =
        Regex::new(r"(?i)\bPARSE\s+ERROR:\s*ospid=(?P<ospid>\d+),\s*error=(?P<code>\d+)\b")
            .expect("valid parse error regex");
    let sql_hash_re =
        Regex::new(r"(?i)\bSQL hash=(?P<sql_hash>0x[0-9a-f]+)\b").expect("valid sql hash regex");
    let sqlid_re = Regex::new(r"(?i)\bsqlid=(?P<sqlid>[0-9a-z]+)\b").expect("valid sqlid regex");
    let username_re =
        Regex::new(r"(?i)\.\.\.Current username=(?P<username>\S+)").expect("valid username regex");
    let app_action_re =
        Regex::new(r"(?i)\.\.\.Application:\s*(?P<application>.*?)\s+Action:\s*(?P<action>.*)$")
            .expect("valid application/action regex");
    let special_patterns = vec![
        ("FATAL_NI_CONNECT_ERROR", Regex::new(r"(?i)\bFatal NI connect error\b").unwrap()),
        ("THREAD_CANNOT_ALLOCATE_NEW_LOG", Regex::new(r"(?i)\bThread \d+ cannot allocate new log\b").unwrap()),
        ("PRIVATE_STRAND_FLUSH_NOT_COMPLETE", Regex::new(r"(?i)\bPrivate strand flush not complete\b").unwrap()),
        (
            "ASH_EMERGENCY_FLUSH",
            Regex::new(r"(?i)\bASH\) performed an emergency flush\b|\bActive Session History \(ASH\) performed an emergency flush\b").unwrap(),
        ),
        ("WARNING", Regex::new(r"(?i)\bWARNING\b").unwrap()),
        ("ERROR", Regex::new(r"(?i)\bERROR\b").unwrap()),
        ("INCIDENT", Regex::new(r"(?i)\bincident\b").unwrap()),
        (
            "PROCESS_TERMINATING",
            Regex::new(r"(?i)\bterminating (?:the )?instance\b|\bterminating process\b").unwrap(),
        ),
        ("FAILED", Regex::new(r"(?i)\bfailed\b").unwrap()),
    ];

    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "source_file": path.display().to_string(),
                "error": format!("Failed to open alertlog attachment: {}", e)
            })
        }
    };

    let mut current_ts: Option<NaiveDateTime> = None;
    let mut suppress_parse_error_detail_lines: usize = 0;
    let mut pending_parse: Option<PendingAlertlogParseDetail> = None;
    let mut events = Vec::new();
    let mut parse_details = Vec::new();
    let mut matching_events_total = 0usize;
    let mut matching_parse_error_details_total = 0usize;

    let mut flush_pending = |pending: &mut Option<PendingAlertlogParseDetail>,
                             parse_details: &mut Vec<AlertlogParseDetail>,
                             matching_total: &mut usize| {
        if let Some(detail) = pending.take() {
            let detail = detail.to_detail();
            let matches_pattern = code_pattern
                .as_ref()
                .map(|p| {
                    detail.code.to_lowercase().contains(p)
                        || detail.description.to_lowercase().contains(p)
                })
                .unwrap_or(true);
            if include_parse_error_details && matches_pattern {
                *matching_total += 1;
                if parse_details.len() < limit {
                    parse_details.push(detail);
                }
            }
        }
    };

    use std::io::{BufRead, BufReader};
    for raw_line in BufReader::new(file).lines() {
        let line = match raw_line {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(ts) = timestamp_from_alertlog_line(&line, &ts_re) {
            flush_pending(
                &mut pending_parse,
                &mut parse_details,
                &mut matching_parse_error_details_total,
            );
            current_ts = Some(ts);
            suppress_parse_error_detail_lines = 0;
            continue;
        }

        let Some(ts) = current_ts else {
            continue;
        };
        let current_day = ts.date();
        if date_from.map(|d| current_day < d).unwrap_or(false)
            || date_to.map(|d| current_day > d).unwrap_or(false)
        {
            continue;
        }

        if let Some(caps) = too_many_parse_errors_re.captures(&line) {
            flush_pending(
                &mut pending_parse,
                &mut parse_details,
                &mut matching_parse_error_details_total,
            );
            let count = caps
                .name("count")
                .and_then(|m| m.as_str().parse::<u64>().ok())
                .unwrap_or(1);
            let sql_hash = sql_hash_re
                .captures(&line)
                .and_then(|c| c.name("sql_hash").map(|m| m.as_str().to_lowercase()))
                .unwrap_or_else(|| "UNKNOWN".to_string());
            pending_parse = Some(PendingAlertlogParseDetail {
                timestamp: ts,
                count,
                sql_hash,
                code: "UNKNOWN".to_string(),
                description: String::new(),
                ospid: "UNKNOWN".to_string(),
                sqlid: "UNKNOWN".to_string(),
                username: "UNKNOWN".to_string(),
                application: "UNKNOWN".to_string(),
                action: "UNKNOWN".to_string(),
                sample: trim_sample(&line, 240),
                lines_seen: 0,
            });
            suppress_parse_error_detail_lines = 8;
        }

        if let Some(pending) = pending_parse.as_mut() {
            pending.lines_seen += 1;
            if let Some(caps) = parse_error_re.captures(&line) {
                let code_num = caps
                    .name("code")
                    .and_then(|m| m.as_str().parse::<i64>().ok())
                    .unwrap_or(0);
                pending.ospid = caps
                    .name("ospid")
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| "UNKNOWN".to_string());
                pending.code = format_ora_code(code_num);
                pending.description = ora_description(code_num).to_string();
            }
            if let Some(caps) = sqlid_re.captures(&line) {
                pending.sqlid = caps
                    .name("sqlid")
                    .map(|m| m.as_str().to_lowercase())
                    .unwrap_or_else(|| "UNKNOWN".to_string());
            }
            if let Some(caps) = username_re.captures(&line) {
                pending.username = caps
                    .name("username")
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_else(|| "UNKNOWN".to_string());
            }
            if let Some(caps) = app_action_re.captures(&line) {
                pending.application = caps
                    .name("application")
                    .map(|m| m.as_str().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "UNKNOWN".to_string());
                pending.action = caps
                    .name("action")
                    .map(|m| m.as_str().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "UNKNOWN".to_string());
            }
            if pending.lines_seen >= 6 || app_action_re.is_match(&line) {
                flush_pending(
                    &mut pending_parse,
                    &mut parse_details,
                    &mut matching_parse_error_details_total,
                );
            }
        }

        if let Some((code, description, total)) = detect_alertlog_event(
            &line,
            &ora_re,
            &tns_re,
            &too_many_parse_errors_re,
            &parse_error_re,
            &special_patterns,
        ) {
            if code.starts_with("PARSE_ERROR_ORA_")
                && suppress_parse_error_detail_lines > 0
                && !include_parse_error_details
            {
                suppress_parse_error_detail_lines -= 1;
                continue;
            }

            let matches_pattern = code_pattern
                .as_ref()
                .map(|p| code.to_lowercase().contains(p) || description.to_lowercase().contains(p))
                .unwrap_or(true);
            if matches_pattern {
                matching_events_total += 1;
                if events.len() < limit {
                    events.push(AlertlogEvent {
                        timestamp: ts.format("%Y-%m-%d %H:%M:%S").to_string(),
                        date: current_day.to_string(),
                        code,
                        description,
                        occurrences: 1,
                        total,
                        sample: trim_sample(&line, 180),
                    });
                }
            }
        }

        if suppress_parse_error_detail_lines > 0 {
            suppress_parse_error_detail_lines -= 1;
        }
    }
    flush_pending(
        &mut pending_parse,
        &mut parse_details,
        &mut matching_parse_error_details_total,
    );

    let truncated = matching_events_total > events.len()
        || matching_parse_error_details_total > parse_details.len();
    let payload = AlertlogPayload {
        schema_version: JASMIN_TOOLS_SCHEMA_VERSION.to_string(),
        source_file: path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("")
            .to_string(),
        date_from: date_from.map(|d| d.to_string()),
        date_to: date_to.map(|d| d.to_string()),
        code_pattern: code_pattern.clone(),
        truncated,
        returned_events: events.len(),
        returned_parse_error_details: parse_details.len(),
        matching_events_total,
        matching_parse_error_details_total,
        events,
        parse_error_details: parse_details,
    };
    let payload_value = serde_json::to_value(&payload).unwrap_or_else(|_| json!({}));
    let alertlog_errors_toon = encode(&payload_value, None);

    json!({
        "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
        "available": true,
        "attachments_dir": attachments_dir.display().to_string(),
        "source_file": payload.source_file,
        "date_from": payload.date_from,
        "date_to": payload.date_to,
        "code_pattern": payload.code_pattern,
        "truncated": payload.truncated,
        "returned_events": payload.returned_events,
        "returned_parse_error_details": payload.returned_parse_error_details,
        "matching_events_total": payload.matching_events_total,
        "matching_parse_error_details_total": payload.matching_parse_error_details_total,
        "format": "TOON",
        "alertlog_errors_toon": alertlog_errors_toon,
        "usage_hint": "Use alertlog_errors_toon as the authoritative structured alert.log evidence for this date range. For parse-error investigations, call again with include_parse_error_details=true and the narrowest relevant date range."
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
        None => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": format!("snap_id {} not found", snap_id)
            })
        }
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
        other => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": format!("Unknown metric '{}'", other)
            })
        }
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
        None => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": format!("snap_id {} not found", snap_id)
            })
        }
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
        None => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": format!("snap_id {} not found", snap_id)
            })
        }
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
        None => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": format!("snap_id {} not found", snap_id)
            })
        }
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
            if s.sql_module.to_lowercase().contains(&module_pattern)
                && seen.insert(s.sql_id.clone())
            {
                matches.push(json!({
                    "sql_id": s.sql_id,
                    "module": s.sql_module,
                    "first_seen_snap_id": a.snap_info.begin_snap_id,
                    "source_section": "sql_elapsed"
                }));
            }
        }
        for s in a.sql_cpu_time.values() {
            if s.sql_module.to_lowercase().contains(&module_pattern)
                && seen.insert(s.sql_id.clone())
            {
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
        None => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": format!("snap_id_a {} not found", snap_a)
            })
        }
    };
    let b = match find_awr(c, snap_b) {
        Some(b) => b,
        None => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": format!("snap_id_b {} not found", snap_b)
            })
        }
    };

    let focus: HashSet<String> = args
        .get("focus")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
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
        None => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": format!("snap_id {} not found", snap_id)
            })
        }
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
        other => {
            return json!({
                "schema_version": JASMIN_TOOLS_SCHEMA_VERSION,
                "error": format!("Unknown kind '{}'", other)
            })
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aix_parser_reads_lparstat_entitlement_columns() {
        let header = split_aix_fields("%user %sys %wait %idle physc %entc lbusy app vcsw phint");
        let fields = split_aix_fields("10.0 5.0 0.0 85.0 3.8 95.0 40.0 0.2 123 4");

        let obs = parse_aix_row_with_header("lparstat.out", 2, "", &header, &fields)
            .expect("expected lparstat observation");

        assert_eq!(obs.entc_pct, Some(95.0));
        assert_eq!(obs.physc, Some(3.8));
        assert_eq!(obs.cpu_user_pct, Some(10.0));
        assert_eq!(obs.cpu_sys_pct, Some(5.0));
        assert_eq!(obs.cpu_idle_pct, Some(85.0));
    }

    #[test]
    fn aix_parser_reads_key_value_metrics_and_rejects_unsafe_paths() {
        let fields = split_aix_fields("Entc%=92.5 physc=3.4 ent=4.0 idle=70.0");

        let obs = parse_aix_key_value_line("oraix.txt", 1, "", &fields)
            .expect("expected key-value observation");

        assert_eq!(obs.entc_pct, Some(92.5));
        assert_eq!(obs.physc, Some(3.4));
        assert_eq!(obs.entitled_capacity, Some(4.0));
        assert_eq!(obs.cpu_busy_pct, Some(30.0));

        assert!(is_safe_relative_path("host1/lparstat.out"));
        assert!(!is_safe_relative_path("../lparstat.out"));
        assert!(!is_safe_relative_path("/tmp/lparstat.out"));
    }

    #[test]
    fn aix_parser_reads_vmstat_pc_ec_columns() {
        let header = split_aix_fields("r b avm fre re pi po fr sr cy in sy cs us sy id wa pc ec");
        let fields = split_aix_fields(
            "36 0 104145618 2648516 0 0 0 0 0 0 638 43926 18364 31 42 28 0 8.93 89.3",
        );

        let obs = parse_aix_row_with_header("vmstat.out", 5, "", &header, &fields)
            .expect("expected vmstat observation");

        assert_eq!(obs.cpu_user_pct, Some(31.0));
        assert_eq!(obs.cpu_sys_pct, Some(42.0));
        assert_eq!(obs.cpu_idle_pct, Some(28.0));
        assert_eq!(obs.cpu_busy_pct, Some(72.0));
        assert_eq!(obs.physc, Some(8.93));
        assert_eq!(obs.entc_pct, Some(89.3));
    }

    #[test]
    fn aix_parser_reads_nmon_lpar_rows() {
        let header = split_aix_fields("LPAR,Logical Partition di-ora-prd,PhysicalCPU,virtualCPUs,logicalCPUs,poolCPUs,entitled,weight,PoolIdle,usedAllCPU%,usedPoolCPU%,SharedCPU,Capped,EC_User%,EC_Sys%,EC_Wait%,EC_Idle%,VP_User%,VP_Sys%,VP_Wait%,VP_Idle%,Folded,Pool_id");
        let fields = split_aix_fields("LPAR,T0001,9.81,10,40,28,10.00,172,0,20.4,35.0,1,0,30.0,43.0,0.0,27.0,30.0,43.0,0.0,27.0,0,1");

        let obs = parse_aix_row_with_header("nmon.nmon", 500, "", &header, &fields)
            .expect("expected nmon LPAR observation");

        assert_eq!(obs.physc, Some(9.81));
        assert_eq!(obs.entitled_capacity, Some(10.0));
        assert!((obs.entc_pct.unwrap_or(0.0) - 98.1).abs() < 0.0001);
        assert_eq!(obs.cpu_user_pct, Some(30.0));
        assert_eq!(obs.cpu_sys_pct, Some(43.0));
        assert_eq!(obs.cpu_wait_pct, Some(0.0));
        assert_eq!(obs.cpu_idle_pct, Some(27.0));
    }

    #[test]
    fn aix_parser_reads_topas_entc_with_terminal_codes() {
        let header = split_aix_fields("\u{1b}[;7mCPU User% Kern% Wait% Idle% Physc Entc%");
        let fields = split_aix_fields("Total 29.1 45.0 0.3 25.6 9.85 98.5292924");

        let obs = parse_aix_row_with_header("topas.out", 10, "", &header, &fields)
            .expect("expected topas observation");

        assert_eq!(obs.cpu_user_pct, Some(29.1));
        assert_eq!(obs.cpu_sys_pct, Some(45.0));
        assert_eq!(obs.cpu_wait_pct, Some(0.3));
        assert_eq!(obs.cpu_idle_pct, Some(25.6));
        assert_eq!(obs.physc, Some(9.85));
        assert_eq!(obs.entc_pct, Some(98.5292924));
    }

    #[test]
    fn aix_fixture_dir_exposes_entitlement_when_env_is_set() {
        let Ok(aix_dir) = std::env::var("JASMIN_AIX_FIXTURE_DIR") else {
            return;
        };
        let aix_dir = PathBuf::from(aix_dir);
        let attachments_dir = aix_dir
            .parent()
            .expect("fixture AIX dir should have parent attachments dir");
        let stem_path = attachments_dir
            .to_string_lossy()
            .strip_suffix("_attachments")
            .expect("fixture parent should be named <stem>_attachments")
            .to_string();

        let result = tool_get_aix_cpu_entitlement_summary(
            &json!({
                "pattern": "vmstat",
                "limit_records": 5
            }),
            &stem_path,
        );

        assert!(
            result["total_observations"].as_u64().unwrap_or(0) > 0,
            "expected observations from fixture: {result}"
        );
        assert!(
            result["stats"]["entc_pct"]["max"].as_f64().unwrap_or(0.0) > 90.0,
            "expected Entc%/ec from fixture: {result}"
        );

        let topas_result = tool_get_aix_cpu_entitlement_summary(
            &json!({
                "pattern": "topas",
                "limit_records": 5
            }),
            &stem_path,
        );

        assert!(
            topas_result["stats"]["entc_pct"]["max"]
                .as_f64()
                .unwrap_or(0.0)
                > 98.0,
            "expected Entc% from topas fixture: {topas_result}"
        );
    }
}
