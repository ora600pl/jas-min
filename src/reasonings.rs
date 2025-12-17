use colored::Colorize;
use base64::{engine::general_purpose, Engine as _};
use reqwest::{Client, multipart};
use reqwest::multipart::{Form, Part};
use serde_json::json;
use std::env::Args;
use std::fmt::format;
use std::borrow::Cow;
use std::{env, fs, collections::HashMap, sync::Arc, path::Path, collections::HashSet};
use axum::{routing::post, Router, Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_http::cors::{CorsLayer, Any};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tokio::sync::oneshot;
use std::io::{stdout, Write};
use std::error::Error;
use crate::tools::*;
use crate::awr::{AWRSCollection, HostCPU, IOStats, LoadProfile, SQLCPUTime, SQLGets, SQLIOTime, SQLReads, SegmentStats, WaitEvents, AWR};
use std::str::FromStr;

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct StatisticsDescription {
    pub dbcpu_dbtime: String,
    pub median_absolute_deviation: String, 
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct TopPeaksSelected {
    pub report_name: String,
    pub report_date: String,
    pub snap_id: u64,
    pub db_time_value: f64,
    pub db_cpu_value: f64,
    pub dbcpu_dbtime_ratio: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct MadAnomaliesEvents {
    pub anomaly_date: String,
    pub mad_score: f64,
    pub total_wait_s: f64,
    pub number_of_waits: u64,
    pub avg_wait_time_for_execution_ms: f64,
    pub pct_of_db_time: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct MadAnomaliesSQL {
    pub anomaly_date: String,
    pub mad_score: f64,
    pub elapsed_time_cumulative_s: f64,
    pub number_of_executions: u64,
    pub avg_exec_time_for_execution: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct TopForegroundWaitEvents {
    pub event_name: String,
    pub correlation_with_db_time: f64,
    pub marked_as_top_in_pct_of_probes: f64,
    pub avg_pct_of_dbtime: f64,
    pub stddev_pct_of_db_time: f64,
    pub avg_wait_time_s: f64,
    pub stddev_wait_time_s: f64,
    pub avg_number_of_executions: f64,
    pub stddev_number_of_executions: f64,
    pub avg_wait_for_execution_ms: f64,
    pub stddev_wait_for_execution_ms: f64,
    pub median_absolute_deviation_anomalies: Vec<MadAnomaliesEvents>,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct TopBackgroundWaitEvents {
    pub event_name: String,
    pub correlation_with_db_time: f64,
    pub marked_as_top_in_pct_of_probes: f64,
    pub avg_pct_of_dbtime: f64,
    pub stddev_pct_of_db_time: f64,
    pub avg_wait_time_s: f64,
    pub stddev_wait_time_s: f64,
    pub avg_number_of_executions: f64,
    pub stddev_number_of_executions: f64,
    pub avg_wait_for_execution_ms: f64,
    pub stddev_wait_for_execution_ms: f64,
    pub median_absolute_deviation_anomalies: Vec<MadAnomaliesEvents>,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct PctOfTimesThisSQLFoundInOtherTopSections {
    pub sqls_by_cpu_time_pct: f64,
    pub sqls_by_user_io_pct: f64,
    pub sqls_by_reads: f64,
    pub sqls_by_gets: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct WaitEventsWithStrongCorrelation {
    pub event_name: String,
    pub correlation_value: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct WaitEventsFromASH {
    pub event_name: String,
    pub avg_pct_of_dbtime_in_sql: f64,
    pub stddev_pct_of_dbtime_in_sql: f64,
    pub count: u64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct TopSQLsByElapsedTime {
    pub sql_id: String,
    pub module: String,
    pub sql_type: String,
    pub pct_of_time_sql_was_found_in_other_top_sections: PctOfTimesThisSQLFoundInOtherTopSections,
    pub correlation_with_db_time: f64,
    pub marked_as_top_in_pct_of_probes: f64,
    pub avg_elapsed_time_by_exec: f64,
    pub stddev_elapsed_time_by_exec: f64,
    pub avg_cpu_time_by_exec: f64,
    pub stddev_cpu_time_by_exec: f64,
    pub avg_elapsed_time_cumulative_s: f64,
    pub stddev_elapsed_time_cumulative_s: f64,
    pub avg_cpu_time_cumulative_s: f64,
    pub stddev_cpu_time_cumulative_s: f64,
    pub avg_number_of_executions: f64,
    pub stddev_number_of_executions: f64,
    pub median_absolute_deviation_anomalies: Vec<MadAnomaliesSQL>,
    pub wait_events_with_strong_pearson_correlation: Vec<WaitEventsWithStrongCorrelation>,
    pub wait_events_found_in_ash_sections_for_this_sql: Vec<WaitEventsFromASH>,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct StatsSummary {
    pub statistic_name: String,
    pub avg_value: f64,
    pub stddev_value: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct IOStatsByFunctionSummary {
    pub function_name: String,
    pub statistics_summary: Vec<StatsSummary>,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct LatchActivitySummary {
    pub latch_name: String,
    pub get_requests_avg: f64,
    pub weighted_miss_pct: f64,
    pub wait_time_weighted_avg_s: f64,
    pub found_in_pct_of_probes: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct Top10SegmentStats {
    pub segment_name: String,
    pub segment_type: String,
    pub object_id: u64,
    pub data_object_id: u64,
    pub avg: f64,
    pub stddev: f64,
    pub pct_of_occuriance: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct InstanceStatisticCorrelation {
    pub stat_name: String,
    pub pearson_correlation_value: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct LoadProfileAnomalies {
    pub load_profile_stat_name: String,
    pub anomaly_date: String,
    pub mad_score: f64,
    pub mad_threshold: f64,
    pub per_second: f64,
    pub avg_value_per_second: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct AnomalyDescription {
    pub area_of_anomaly: String,
    pub statistic_name: String,
}
 
#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct AnomlyCluster {
    pub begin_snap_id: u64,
    pub begin_snap_date: String,
    pub anomalies_detected: Vec<AnomalyDescription>,
    pub number_of_anomalies: u64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct ReportForAI {
    pub general_data: StatisticsDescription,
    pub top_spikes_marked: Vec<TopPeaksSelected>,
    pub top_foreground_wait_events: Vec<TopForegroundWaitEvents>,
    pub top_background_wait_events: Vec<TopBackgroundWaitEvents>,
    pub top_sqls_by_elapsed_time: Vec<TopSQLsByElapsedTime>,
    pub io_stats_by_function_summary: Vec<IOStatsByFunctionSummary>,
    pub latch_activity_summary: Vec<LatchActivitySummary>,
    pub top_10_segments_by_row_lock_waits: Vec<Top10SegmentStats>,
    pub top_10_segments_by_physical_writes: Vec<Top10SegmentStats>,
    pub top_10_segments_by_physical_write_requests: Vec<Top10SegmentStats>,
    pub top_10_segments_by_physical_read_requests: Vec<Top10SegmentStats>,
    pub top_10_segments_by_logical_reads: Vec<Top10SegmentStats>,
    pub top_10_segments_by_direct_physical_writes: Vec<Top10SegmentStats>,
    pub top_10_segments_by_direct_physical_reads: Vec<Top10SegmentStats>,
    pub top_10_segments_by_buffer_busy_waits: Vec<Top10SegmentStats>,
    pub instance_stats_pearson_correlation: Vec<InstanceStatisticCorrelation>,
    pub load_profile_anomalies: Vec<LoadProfileAnomalies>,
    pub anomaly_clusters: Vec<AnomlyCluster>,
}


static SPELL: &str =   
"Your name is JAS-MIN. You are a professional as shit Oracle Database performance tuning expert and assistant but don't mention it.
You are analyzing JSON file containing summarized statistics from parsed AWR reports from long period of time. 
You are an Oracle Database performance expert.

You receive main input object called **ReportForAI**, encoded as TOON (Token-Oriented Object Notation) or pure JSON.
This TOON/JSON is a preprocessed, structured representation of an Oracle performance audit report (AWR/Statspack family).

If you receive load_profile_statistics.json, containing load profile summary for the database, analyze them first and write comprehensive summary for all metrics with as many statistical insights as possible.

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
   - Each entry: segment_name, segment_type, object_id, data_object_id, avg, stddev, pct_of_occuriance. (segment_name might be missing due to security reasons)

9. instance_stats_pearson_correlation
   - Equivalent to:
     'Instance Statistics: Correlation with DB Time for |ρ| ≥ 0.5'.
   - Each entry: stat_name, pearson_correlation_value.
   - Includes statistics like 'user logons cumulative', 'user logouts cumulative', UNDO-related stats, chained rows, etc.

10. load_profile_anomalies
    - Each anomaly: load_profile_stat_name, anomaly_date, mad_score, mad_threshold, per_second.

11. anomaly_clusters
    - Each cluster:
      - begin_snap_id, begin_snap_date
      - anomalies_detected: array of anomalies:
        - area_of_anomaly
        - statistic_name

============================================================
CORE GUIDELINES

- Analyze the WHOLE TOON/JSON report and perform the most comprehensive analysis you can.
  Do NOT ask user if they want something: assume they want EVERY relevant insight you can discover.
- Do not hallucinate.
- Show all important numbers. Do not invent values.
  Quote numbers, IDs (SQL_IDs), segment names, and metric names exactly as they appear in JSON.
- Summarize:
  - Overall performance profile of the system.
  - Each logical area (wait events, SQLs, IO, latches, segments, anomalies).
- When describing overall performance profile:
  - Explain the DBCPU/DBTIME ratio and its meaning.
  - Use real values from:
    - top_spikes_marked (dbcpu_dbtime_ratio, db_time_value, db_cpu_value)
    - general_data (textual description about MAD and DBCPU/DBTIME).
- When you mention dates:
  - Try to associate them explicitly with snap_id (via top_spikes_marked or anomaly_clusters.begin_snap_id).
- If you know any relevant Oracle MOS notes for the issues you identify:
  - Provide links to those MOS notes.
- Answer in **markdown** format.
- The answer should be divided into **clear sections and subsections**, starting with icons/symbols.
  Use fine-grained structure, not a giant monolithic block.
- Finish with a **summary and overall recommendations** that MUST include:
  - Comment on the quality of disks based on IO-related metrics:
    - Are the disks slow or not?
  - Comment on application design:
    - Is this likely a poorly written application and why?
  - A list of actions that should be taken **immediately**,
    and how they should be addressed to **developers** and/or **DBAs**.
  - Summary for management written in less technical slang.
============================================================
OVERALL OUTPUT FORMAT

Your answer MUST be in **markdown** and should be structured at least as:

1. 🧭 Executive Summary
2. 📈 Overall Performance Profile
3. ⏳ Wait Event Analysis
   - Foreground waits
   - Background waits
4. 🧮 SQL-Level Analysis
   - Most harmful SQL_IDs
   - Short-but-frequent vs long-running SQLs
5. 🧱 Segment & Object-Level Analysis (TOP 10 Segments)
6. 🔧 Latches & Internal Contention
7. 💾 IO & Disk Subsystem Assessment
8. 🔁 UNDO / Redo / Load Profile Observations
9. ⚡ Anomaly Clusters & Cross-Domain Patterns
10. ✅ Recommendations
    - For DBAs
    - For Developers
    - Immediate actions
    - Summary for management 

In each section:
- Use real values from ReportForAI TOON/JSON.
- Do not invent numbers or entities.
- Cross-reference:
  - snap_ids
  - dates
  - SQL_IDs
  - wait event names
  - segment names
  - instance statistics
- Always show both: SNAP_ID and SNAP_DATE when you mention problematic periods.
- Always write wait events names and SQL_IDs as code (example: `db file sequential read`, `7wbv18dfuc66z`)

Finish with:
- Clear statement about disk quality ('Are disks slow?').
- Clear statement about application design quality.
- Clear, actionable list of what should be done next by DBAs and developers.
- Add link to github: https://github.com/ora600pl/jas-min - this is your source code.
- Suggest that good performance tuning experts are at ora-600.pl
- Write answer in language: ";

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

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
struct UrlContext{
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
    let url_context_data: HashMap<String, Vec<UrlContext>> = serde_json::from_str(&r_content.unwrap()).expect("Wrong url file JSON format");
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

async fn upload_file_to_gemini_from_path(api_key: &str, path: &str, file_type: &str,file_name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let file_bytes = fs::read(path)?;   // renamed

    let part = multipart::Part::bytes(file_bytes)
        .file_name(Cow::Owned(file_name.to_string()))
        .mime_str(file_type)?;

    let form = multipart::Form::new().part("file", part);

    let client = reqwest::Client::new();
    let response = client
        .post(format!("https://generativelanguage.googleapis.com/upload/v1beta/files?key={}",api_key))
        .multipart(form)
        .send()
        .await?;

    if response.status().is_success() {
        let response_text = response.text().await?;
        match serde_json::from_str::<GeminiFileUploadResponse>(&response_text) {
            Ok(file_upload_response) => {
                println!("✅ {} uploaded! URI: {}", path, file_upload_response.file.uri);
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

async fn upload_log_file_gemini(api_key: &str, log_content: String, file_name: String) -> Result<String, Box<dyn std::error::Error>> {
    // prepart multipart form
    let part = multipart::Part::bytes(log_content.into_bytes())
        .file_name(file_name) 
        .mime_str("text/plain").unwrap(); //  MIME type as text

    // Create multipart form
    let form = multipart::Form::new().part("file", part);

    let client = reqwest::Client::new();
    let response = client
        .post(format!("https://generativelanguage.googleapis.com/upload/v1beta/files?key={}", api_key))
        .multipart(form)
        .send()
        .await.unwrap();

    if response.status().is_success() {
        let response_text = response.text().await?;

        match serde_json::from_str::<GeminiFileUploadResponse>(&response_text) {
            Ok(file_upload_response) => {
                println!("✅ File uploaded! URI: {}", file_upload_response.file.uri);
                Ok(file_upload_response.file.uri)
            },
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


async fn spinning_beer(mut done: oneshot::Receiver<()>) {
    let frames = ["🍺", "🍻", "🍺", "🍻"];
    let mut i = 0;
    while done.try_recv().is_err() {
        print!("\r{}", frames[i % frames.len()]);
        stdout().flush().unwrap();
        i += 1;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    println!("\r✅ Got response!");
}

async fn gemini_deep(logfile_name: &str, args: &crate::Args, vendor_model_lang: Vec<&str>, token_count_factor: usize, first_response: String, api_key: &str, events_sqls: HashMap<&str, HashSet<String>>,) {
    println!("{}{}{}","=== Starting deep dive with Google Gemini model: ".bright_cyan(), vendor_model_lang[1]," ===".bright_cyan());
    let mut json_file = args.json_file.clone();
    if json_file.is_empty() {
        json_file = format!("{}.json", args.directory);
    }

    let client = Client::new();

    let file_uri = upload_log_file_gemini(&api_key, first_response, "performance_analyze.md".to_string()).await.unwrap();
    let spell = format!("You were given a detailed performance analyze of Oracle database. Your task is to answear ONLY with a list of TOP {} SNAP_IDs that should be analyzed in more detail. Anwear only with those numbers representing SNAP_ID - one number in a line.", args.deep_check);

    let payload = json!({
                    "contents": [{
                        "parts": [
                            { "text": spell }, 
                            {
                                "fileData": {
                                    "mimeType": "text/plain",
                                    "fileUri": file_uri
                                }
                            }
                        ]
                    }],
                    "generationConfig": {
                        "maxOutputTokens": 8192 * token_count_factor,
                        "thinkingConfig": {
                            "thinkingBudget": -1
                        }
                    }
                });

    let (tx, rx) = oneshot::channel();
    let spinner = tokio::spawn(spinning_beer(rx));

    let response = client
            .post(format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}", vendor_model_lang[1], api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await.unwrap();
    
    let _ = tx.send(()); //stop spinner
    let _ = spinner.await;

    if response.status().is_success() {
        let json: Value = response.json().await.unwrap();

        // Integrate all parts, keeping only first unique occurrences
        let parts = &json["candidates"][0]["content"]["parts"];
        let mut seen = HashSet::new();
        let full_text = parts
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|part| part["text"].as_str())
            .filter(|text| seen.insert(text.to_string()))
            .collect::<Vec<&str>>()
            .join("\n");

        println!("🍻 Gemini will analyze further the following SNP_ID:\n{}", &full_text);
        
        let s_json = fs::read_to_string(&json_file).expect(&format!("Something wrong with a file {} ", &args.json_file));
        let mut collection: AWRSCollection = serde_json::from_str(&s_json).expect(&format!("Wrong JSON format {}", &json_file));
        //println!("Total amount of tokens drank by Google: {}\nFinished reason was: {}\n", &json["usageMetadata"]["totalTokenCount"], &json["candidates"][0]["finishReason"]);
        let awrs: Vec<AWR> = collection.awrs;
        let mut snap_ids: HashSet<u64> = HashSet::new();
        for snap_id in full_text.split("\n") {
            let id = u64::from_str(snap_id);
            if id.is_ok() {
                snap_ids.insert(id.unwrap());
            } else if snap_id.contains("_") {
                let s_id = snap_id.split("_").next().unwrap();
                snap_ids.insert(u64::from_str(s_id).unwrap());
            } else { //if id is not ok it means that model returned data instead of SNAP_ID as a number, so we have match date to snap_id
                let id = awrs.iter()
                                           .find(|a| &a.snap_info.begin_snap_time == snap_id).unwrap()
                                           .snap_info.begin_snap_id;
                snap_ids.insert(id);
            }
            
        }

        let mut deep_stats: Vec<AWR> = Vec::new();
        for awr in awrs {
            if snap_ids.contains(&awr.snap_info.begin_snap_id) {
                deep_stats.push(awr);
            }
        }
        
        let spell = format!("You were given a detailed performance report of Oracle Database (written in markdown format) and JSON file containing one of the TOP periods with the detailed statistics. 
Perform a deep analyzes of those statistics. Return a detailed report, showing what you have found. 
Suggest performance improvments. Detect impactfull statistics and latches.
Investigate all SQL sections and crosscheck SQL_IDs to find patterns of bad SQL executions.
Highlight SQL_IDs that are reported in many different sections.
============================================================
OVERALL OUTPUT FORMAT

Your answer MUST be in **markdown** and should be structured at least as:

1. 🧭 Executive Summary
2. 📈 Overall Performance Profile
3. ⏳ Wait Event Analysis
   - Foreground waits
   - Background waits
4. 🧮 SQL-Level Analysis
   - Most harmful SQL_IDs
   - Short-but-frequent vs long-running SQLs
5. 🧱 Segment & Object-Level Analysis (TOP 10 Segments)
6. 🔧 Latches & Internal Contention
7. 💾 IO & Disk Subsystem Assessment
8. 🔁 UNDO / Redo / Load Profile Observations
9. ⚡ Anomaly Clusters & Cross-Domain Patterns
10. ✅ Recommendations
    - For DBAs
    - For Developers
    - Immediate actions
    - Summary for management 

In each section:
- Do not invent numbers or entities.
- Cross-reference with performance report delivered in markdown format:
  - snap_ids
  - dates
  - SQL_IDs
  - wait event names
  - segment names
  - instance statistics
- Always show both: SNAP_ID and SNAP_DATE when you mention problematic periods.
- Always write wait events names and SQL_IDs as code (example: `db file sequential read`, `7wbv18dfuc66z`)
Write answer in language: {}", vendor_model_lang[2]);

        for ds in deep_stats {

            let deep_stats_json = serde_json::to_string(&ds).unwrap();
            let file_uri_stats = upload_log_file_gemini(&api_key, deep_stats_json, "detailed_statistics.json".to_string()).await.unwrap();

            let payload = json!({
                "contents": [{
                    "parts": [
                        { "text": spell },
                        {
                            "fileData": {
                                "mimeType": "text/plain",
                                "fileUri": file_uri
                            }
                        },
                        {
                            "fileData": {
                                "mimeType": "text/plain",
                                "fileUri": file_uri_stats
                            }
                        }
                    ]
                }],
                "generationConfig": {
                    "maxOutputTokens": 8192 * token_count_factor,
                    "thinkingConfig": {
                        "thinkingBudget": -1
                    }
                }
            });

            let (tx, rx) = oneshot::channel();
            let spinner = tokio::spawn(spinning_beer(rx));

            let response = client
                    .post(format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}", vendor_model_lang[1], api_key))
                    .header("Content-Type", "application/json")
                    .json(&payload)
                    .send()
                    .await.unwrap();
            
            let _ = tx.send(()); //stop spinner
            let _ = spinner.await;

            if response.status().is_success() {
                let json: Value = response.json().await.unwrap();

                // Integrate all parts, keeping only first unique occurrences
                let parts = &json["candidates"][0]["content"]["parts"];
                let mut seen = HashSet::new();
                let full_text = parts
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|part| part["text"].as_str())
                    .filter(|text| seen.insert(text.to_string()))
                    .collect::<Vec<&str>>()
                    .join("\n");

                let stem = logfile_name.split('.').next().unwrap();
                let response_file = format!("{stem}.html_reports/{}_gemini_deep_{}.md", logfile_name, &ds.snap_info.begin_snap_id);
                fs::write(&response_file, full_text.as_bytes()).unwrap();

                println!("🍻 Gemini response written to file: {}", &response_file);
                convert_md_to_html_file(&response_file, events_sqls.clone());
                //println!("{}", full_text);

            } else {
                eprintln!("Error: {}", response.status());
                eprintln!("{}", response.text().await.unwrap());
            }

        }
        

    } else {
        eprintln!("Error: {}", response.status());
        eprintln!("{}", response.text().await.unwrap());
    }


}

#[tokio::main]
pub async fn gemini(logfile_name: &str,vendor_model_lang: Vec<&str>,token_count_factor: usize,events_sqls: HashMap<&str, HashSet<String>>,args: &crate::Args, report_for_ai: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}{}{}", 
        "=== Consulting Google Gemini model: ".bright_cyan(), 
        vendor_model_lang[1], 
        " ===".bright_cyan()
    );

    let api_key = env::var("GEMINI_API_KEY")
        .expect("You have to set GEMINI_API_KEY env variable");

    // -----------------------------
    // LOAD JSON main report + JSON REPORT global stats report
    // -----------------------------
    let log_content = fs::read_to_string(logfile_name).expect(&format!("Can't open file {}", logfile_name));
    let stem = logfile_name.split('.').next().unwrap();
    let json_path = format!("{stem}.html_reports/stats/global_statistics.json");
    let load_profile = fs::read_to_string(&json_path).expect(&format!("Can't open file {}", json_path));
    let response_file = format!("{}_gemini.md", logfile_name);
    let client = Client::new();

    // -----------------------------
    // SYSTEM SPELL + ADVANCED RULES
    // -----------------------------
    let mut spell = format!("{} {}", SPELL, vendor_model_lang[2]);

    if let Some(pr) = private_reasonings() {
        spell = format!("{spell}\n#ADVANCED RULES\n{pr}");
    }

    if !args.url_context_file.is_empty() {
        if let Some(urls) = url_context(&args.url_context_file, events_sqls.clone()) {
            spell = format!("{spell}\n# URL CONTEXT\n{urls}");
        }
    }
    let main_report_uri = upload_log_file_gemini(&api_key, report_for_ai.to_string(), "main_report.toon".to_string()).await.unwrap();
    let global_profile_data_uri = upload_log_file_gemini(&api_key, load_profile, "load_profile_statistics.json".to_string()).await.unwrap();
    // -----------------------------
    // BUILD FINAL PAYLOAD
    // -----------------------------
    let payload = json!({
                "contents": [{
                    "parts": [
                        { "text": format!("### SYSTEM INSTRUCTIONS\n{spell}") },
                        {
                            "fileData": {
                                "mimeType": "text/plain",
                                "fileUri": main_report_uri
                            }
                        },
                        {
                            "fileData": {
                                "mimeType": "text/plain",
                                "fileUri": global_profile_data_uri
                            }
                        }
                    ]
                }],
                "generationConfig": {
                    "maxOutputTokens": 8192 * token_count_factor,
                    "thinkingConfig": {
                        "thinkingBudget": -1
                    }
                }
            });
    

    // -----------------------------
    // SEND REQUEST
    // -----------------------------
    let (tx, rx) = oneshot::channel();
    let spinner = tokio::spawn(spinning_beer(rx));

    let response = client
        .post(format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            vendor_model_lang[1],
            api_key
        ))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    let _ = tx.send(());
    let _ = spinner.await;

    // -----------------------------
    // HANDLE RESPONSE
    // -----------------------------
    if response.status().is_success() {
        let json: Value = response.json().await.unwrap();

        let parts = json["candidates"][0]["content"]["parts"]
            .as_array()
            .unwrap();

        let mut seen = HashSet::new();
        let full_text = parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .filter(|t| seen.insert(t.to_string()))
            .collect::<Vec<&str>>()
            .join("\n");

        fs::write(&response_file, full_text.as_bytes())?;
        println!("🍻 Gemini response written to file: {}", &response_file);

        convert_md_to_html_file(&response_file, events_sqls.clone());

        println!(
            "Total tokens: {}\nFinish reason: {}\n",
            json["usageMetadata"]["totalTokenCount"],
            json["candidates"][0]["finishReason"]
        );

        if args.deep_check > 0 {
            gemini_deep(
                logfile_name,
                &args,
                vendor_model_lang,
                token_count_factor,
                full_text,
                &api_key,
                events_sqls,
            )
            .await;
        }
    } else {
        eprintln!("Error: {}", response.status());
        eprintln!("{}", response.text().await.unwrap());
    }

    Ok(())
}

#[tokio::main]
pub async fn openrouter(
    logfile_name: &str,
    vendor_model_lang: Vec<&str>, // np. ["openrouter", "anthropic/claude-3.5-sonnet", "pl"]
    token_count_factor: usize,
    events_sqls: HashMap<&str, HashSet<String>>,
    args: &crate::Args,
    report_for_ai: &str,
) -> Result<(), Box<dyn std::error::Error>> {

    println!("=== Consulting OpenRouter model: {} ===", vendor_model_lang[1]);

    let api_key = env::var("OPENROUTER_API_KEY")
        .expect("You have to set OPENROUTER_API_KEY env variable");

    let stem = logfile_name.split('.').next().unwrap();
    let json_path = format!("{stem}.html_reports/stats/global_statistics.json");
    let load_profile = fs::read_to_string(&json_path)
        .expect(&format!("Can't open file {}", json_path));

    let response_file = format!("{}_openrouter.md", logfile_name);
    let client = Client::new();

    // SYSTEM SPELL + ADVANCED RULES 
    let mut spell = format!("{} {}", SPELL, vendor_model_lang[2]);
    if let Some(pr) = private_reasonings() {
        spell = format!("{spell}\n#ADVANCED RULES\n{pr}");
    }
    if !args.url_context_file.is_empty() {
        if let Some(urls) = url_context(&args.url_context_file, events_sqls.clone()) {
            spell = format!("{spell}\n# URL CONTEXT\n{urls}");
        }
    }

    // OpenRouter payload
    let payload = json!({
        "model": vendor_model_lang[1],
        "messages": [
            { "role": "system", "content": format!("### SYSTEM INSTRUCTIONS\n{spell}") },
            { "role": "user", "content": format!(
                "MAIN REPORT (toon/json-as-text):\n```\n{}\n```\n\nGLOBAL PROFILE:\n```json\n{}\n```",
                report_for_ai, load_profile
            )}
        ],
        "max_tokens": 8192 * token_count_factor,
        "reasoning": { "effort": "high" },
        "stream": false
    });

    let (tx, rx) = oneshot::channel();
    let spinner = tokio::spawn(spinning_beer(rx));

    let resp = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .header("X-Title", "jas-min")
        .json(&payload)
        .send()
        .await?;

    let _ = tx.send(());
    let _ = spinner.await;

    if resp.status().is_success() {
        println!("Processing response... (it may take a bit)");
        let body = resp.text().await?;

        let json: Value = serde_json::from_str(&body)?;
        let content = json["choices"][0]["message"]["content"]
            .as_str().unwrap_or("")
            .to_string();

        fs::write(&response_file, content.as_bytes())?;
        println!("🍻 OpenRouter response written to file: {}", &response_file);

        convert_md_to_html_file(&response_file, events_sqls.clone());

        println!(
            "Total tokens: {}\nFinish reason: {}\n",
            json["usage"]["total_tokens"],
            json["choices"][0]["finish_reason"]
        );
    } else {
        eprintln!("Error: {}", resp.status());
        eprintln!("{}", resp.text().await.unwrap_or_default());
    }

    Ok(())
}

#[tokio::main]
pub async fn openai_gpt(logfile_name: &str, vendor_model_lang: Vec<&str>, token_count_factor: usize, events_sqls: HashMap<&str, HashSet<String>>, args: &crate::Args, report_for_ai: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}{}{}", "=== Consulting OpenAI model: ".bright_cyan(), vendor_model_lang[1], " ===".bright_cyan());

    let api_key = env::var("OPENAI_API_KEY")
        .expect("You have to set OPENAI_API_KEY env variable");

    // inputs
    let log_content = fs::read_to_string(logfile_name)
        .expect(&format!("Can't open file {}", logfile_name));

    let stem = logfile_name.split('.').collect::<Vec<&str>>()[0];
    let path = format!("{stem}.html_reports/stats/global_statistics.json");
    let load_profile = fs::read_to_string(&path).expect(&format!("Can't open file {}", path));

    let response_file = format!("{}_gpt5.md", logfile_name);

    // build spell
    let mut spell: String = format!("{} {}", SPELL, vendor_model_lang[2]);
    let rag_context = private_reasonings();

    if !args.url_context_file.is_empty() {
        if let Some(urls) = url_context(&args.url_context_file, events_sqls.clone()) {
            spell = format!("{spell}\n{urls}");
        }
    }
    
    let mut input_messages = vec![
        json!({
            "role": "system",
            "content": [
                { "type": "input_text", "text": spell }
            ]
        }),
    ];

    if let Some(rag) = rag_context {
        input_messages.push(json!({
            "role": "user",
            "content": [
                { "type": "input_text", "text":
                    format!("### RAG CONTEXT\n{rag}\n-- END RAG CONTEXT --")
                }
            ]
        }));
    }
    
    let mut log_and_images = vec![
        json!({"type":"input_text", "text":
            format!("### ATTACHED REPORT\n{report_for_ai}\n-- END ATTACHED REPORT --")
        }),
    ];

    log_and_images.push(json!({
        "type": "input_text",
        "text": format!("### LOAD PROFILE STATISTICS JSON\n{}\n-- END JSON --",load_profile
        )
    }));
    
    input_messages.push(json!({
        "role": "user",
        "content": log_and_images
    }));
        
    // tune max_output_tokens
    let max_output_tokens = 8192 * token_count_factor;

    let payload = json!({
        "model": vendor_model_lang[1], // e.g., "gpt-5"
        "input": input_messages,
        "max_output_tokens": 8192 * token_count_factor,
    });

    let client = Client::new();

    // spinner
    let (tx, rx) = oneshot::channel();
    let spinner = tokio::spawn(spinning_beer(rx));

    let response = client
        .post("https://api.openai.com/v1/responses")
        .bearer_auth(api_key)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    let _ = tx.send(()); // stop spinner
    let _ = spinner.await;

    if response.status().is_success() {
        let json: Value = response.json().await?;

        let full_text = if let Some(s) = json.get("output_text").and_then(|v| v.as_str()) {
            s.to_string()
        } else {
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
                            // Some responses nest text as {"type":"output_text","text": "..."}
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
        };

        fs::write(&response_file, full_text.as_bytes())?;
        println!("🧠 GPT-5 response written to file: {}", &response_file);

        convert_md_to_html_file(&response_file, events_sqls);

        // Best-effort token & finish info if present
        if let Some(usage) = json.get("usage") {
            println!("Total tokens (OpenAI): {}", usage);
        }
        if let Some(finish) = json.pointer("/output/0/finish_reason").or_else(|| json.get("finish_reason")) {
            println!("Finish reason: {}", finish);
        }
    } else {
        eprintln!("Error: {}", response.status());
        eprintln!("{}", response.text().await.unwrap_or_default());
    }

    Ok(())
}

// ###########################
// JASMIN Assistant Backend
// ###########################
#[derive(Deserialize)]
struct UserMessage {
    message: String,
}

#[derive(Serialize)]
struct AIResponse {
    reply: String,
}

// Backend type enum
#[derive(Clone, Debug)]
pub enum BackendType {
    OpenAI,
    Gemini,
}

// Trait for AI backends
#[async_trait::async_trait]
trait AIBackend: Send + Sync {
    async fn initialize(&mut self, toon_str: String) -> anyhow::Result<()>;
    async fn send_message(&self, message: &str) -> anyhow::Result<String>;
}

// OpenAI implementation
struct OpenAIBackend {
    client: reqwest::Client,
    api_key: String,
    assistant_id: String,
    thread_id: Option<String>,
}

impl OpenAIBackend {
    fn new(api_key: String, assistant_id: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            assistant_id,
            thread_id: None,
        }
    }

    async fn create_thread_with_file(&self, toon_str: String) -> anyhow::Result<String> {
        // === Step 1: Read file content ===
        let file_bytes = toon_str.into_bytes();
        // let file_name = Path::new(&file_path)
        //     .file_name()
        //     .unwrap_or_else(|| std::ffi::OsStr::new("jasmin_report.txt"))
        //     .to_string_lossy()
        //     .to_string();
        let file_name = "jasmin_report.txt";

        // === Step 2: Upload file to OpenAI ===
        let file_part = Part::bytes(file_bytes)
            .file_name(file_name)
            .mime_str("text/plain")?;
        let form = Form::new()
            .part("file", file_part)
            .text("purpose", "assistants");
        
        let upload_res = self.client
            .post("https://api.openai.com/v1/files")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;
        
        let file_id = upload_res.get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Failed to upload file: {:?}", upload_res))?;
        
        println!("✅ File uploaded with ID: {}", file_id);

        // === Step 3: Create thread with intro message ===
        let thread_res = self.client
            .post("https://api.openai.com/v1/threads")
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .json(&serde_json::json!({
                "messages": [{
                    "role": "user",
                    "content": "Uploading file with performance report"
                }]
            }))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;
        
        let thread_id = thread_res.get("id")
            .and_then(|id| id.as_str())
            .ok_or_else(|| anyhow::anyhow!("Failed to create thread: {:?}", thread_res))?;
        
        println!("✅ Thread created: {}", thread_id);

        // === Step 4: Attach file to thread ===
        let message_url = format!("https://api.openai.com/v1/threads/{}/messages", thread_id);
        let file_msg_res = self.client
            .post(&message_url)
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .json(&serde_json::json!({
                "role": "user",
                "content": "Please analyze the attached performance report.",
                "attachments": [{
                    "file_id": file_id,
                    "tools": [{ "type": "file_search" }]
                }]
            }))
            .send()
            .await?;

        if !file_msg_res.status().is_success() {
            let status = file_msg_res.status();
            let err_text = file_msg_res.text().await?;
            eprintln!("Attach file failed. Status: {}, Response body: {}", status, err_text);
            return Err(anyhow::anyhow!("Failed to attach file to thread."));
        }

        println!("📎 File attached to thread.");

        // === Step 5: Wait for vector store to process the file ===
        println!("⏳ Waiting for file processing to complete...");
        self.wait_for_file_processing(&thread_id).await?;
        
        println!("✅ File processing completed! Thread is ready for use.");
        Ok(thread_id.to_string())
    }

    async fn wait_for_file_processing(&self, thread_id: &str) -> anyhow::Result<()> {
        let mut attempts = 0;
        let max_attempts = 30;
        
        loop {
            let thread_url = format!("https://api.openai.com/v1/threads/{}", thread_id);
            let thread_res = self.client
                .get(&thread_url)
                .bearer_auth(&self.api_key)
                .header("OpenAI-Beta", "assistants=v2")
                .send()
                .await?;
                
            if !thread_res.status().is_success() {
                return Err(anyhow::anyhow!("Failed to get thread details"));
            }
            
            let thread_data = thread_res.json::<serde_json::Value>().await?;
            
            if let Some(tool_resources) = thread_data.get("tool_resources") {
                if let Some(file_search) = tool_resources.get("file_search") {
                    if let Some(vector_store_ids) = file_search.get("vector_store_ids") {
                        if let Some(vector_stores) = vector_store_ids.as_array() {
                            if let Some(vs_id) = vector_stores.first().and_then(|v| v.as_str()) {
                                match self.check_vector_store_status(vs_id).await? {
                                    status if status == "completed" => {
                                        println!("✅ Vector store processing completed!");
                                        return Ok(());
                                    }
                                    status if status == "failed" => {
                                        return Err(anyhow::anyhow!("Vector store processing failed"));
                                    }
                                    status => {
                                        println!("📊 Vector store status: {} (attempt {}/{})", status, attempts + 1, max_attempts);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            
            if attempts >= max_attempts {
                return Err(anyhow::anyhow!("File processing timeout after {} attempts", max_attempts));
            }
            
            sleep(Duration::from_secs(5)).await;
            attempts += 1;
        }
    }

    async fn check_vector_store_status(&self, vector_store_id: &str) -> anyhow::Result<String> {
        let url = format!("https://api.openai.com/v1/vector_stores/{}", vector_store_id);
        
        let res = self.client
            .get(&url)
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .send()
            .await?;
        
        if !res.status().is_success() {
            return Err(anyhow::anyhow!("Failed to check vector store status"));
        }
        
        let json_res = res.json::<serde_json::Value>().await?;
        
        let status = json_res["status"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing status in vector store response"))?;
        
        Ok(status.to_string())
    }

    async fn create_message(&self, thread_id: &str, content: &str) -> anyhow::Result<()> {
        let url = format!("https://api.openai.com/v1/threads/{}/messages", thread_id);

        let mut body = HashMap::new();
        body.insert("role", "user");
        body.insert("content", content);

        self.client.post(&url)
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .json(&body)
            .send().await?
            .error_for_status()?;

        Ok(())
    }

    async fn run_assistant(&self, thread_id: &str) -> anyhow::Result<String> {
        let url = format!("https://api.openai.com/v1/threads/{}/runs", thread_id);

        let mut body = HashMap::new();
        body.insert("assistant_id", &self.assistant_id);

        let res = self.client.post(&url)
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .json(&body)
            .send().await?;
            
        if !res.status().is_success() {
            let status = res.status();
            let error_text = res.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow::anyhow!("API request failed with status {}: {}", status, error_text));
        }
        
        let json_res = res.json::<serde_json::Value>().await?;
        let run_id = json_res["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing or invalid 'id' field in response: {}", json_res))?
            .to_string();

        Ok(run_id)
    }

    async fn wait_for_completion(&self, thread_id: &str, run_id: &str) -> anyhow::Result<()> {
        loop {
            let url = format!("https://api.openai.com/v1/threads/{}/runs/{}", thread_id, run_id);
            let res = self.client
                .get(&url)
                .bearer_auth(&self.api_key)
                .header("OpenAI-Beta", "assistants=v2")
                .send().await?
                .json::<serde_json::Value>().await?;

            let status = res.get("status").and_then(|s| s.as_str());

            match status {
                Some("completed") => return Ok(()),
                Some("failed") | Some("cancelled") | Some("expired") => {
                    return Err(anyhow::anyhow!("Run failed or was cancelled/expired:\n {:?}", res))
                },
                Some(_) => {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                },
                None => {
                    return Err(anyhow::anyhow!("Missing 'status' field in response:\n {:?}", res));
                }
            }
        }
    }

    async fn get_reply(&self, thread_id: &str) -> anyhow::Result<String> {
        let url = format!("https://api.openai.com/v1/threads/{}/messages", thread_id);

        let res = self.client
            .get(&url)
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        if let Some(reply) = res["data"][0]["content"][0]["text"]["value"].as_str() {
            Ok(reply.to_string())
        } else {
            Err(anyhow::anyhow!("Failed to extract assistant reply: {:?}", res))
        }
    }
}

#[async_trait::async_trait]
impl AIBackend for OpenAIBackend {
    async fn initialize(&mut self, file_path: String) -> anyhow::Result<()> {
        let thread_id = self.create_thread_with_file(file_path).await?;
        self.thread_id = Some(thread_id);
        Ok(())
    }

    async fn send_message(&self, message: &str) -> anyhow::Result<String> {
        let thread_id = self.thread_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Thread not initialized"))?;

        self.create_message(thread_id, message).await?;
        let run_id = self.run_assistant(thread_id).await?;
        self.wait_for_completion(thread_id, &run_id).await?;
        self.get_reply(thread_id).await
    }
}

// Gemini implementation
struct GeminiBackend {
    client: reqwest::Client,
    api_key: String,
    model: String,
    conversation_history: Vec<GeminiMessage>,
    file_content: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct GeminiMessage {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum GeminiPart {
    Text { text: String },
    InlineData { inline_data: InlineData },
}

#[derive(Clone, Serialize, Deserialize)]
struct InlineData {
    mime_type: String,
    data: String,
}

impl GeminiBackend {
    fn new(api_key: String, gemini_model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model: gemini_model, 
            conversation_history: Vec::new(),
            file_content: None,
        }
    }

    async fn send_to_gemini(&self, messages: &[GeminiMessage]) -> anyhow::Result<String> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );

        let body = serde_json::json!({
            "contents": messages,
            "generationConfig": {
                "temperature": 0.7,
                "maxOutputTokens": 8192,
            }
        });

        let res = self.client
            .post(&url)
            .json(&body)
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status();
            let error_text = res.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow::anyhow!("Gemini API request failed with status {}: {}", status, error_text));
        }

        let json_res = res.json::<serde_json::Value>().await?;
        
        // Extract response text
        if let Some(candidates) = json_res["candidates"].as_array() {
            if let Some(first_candidate) = candidates.first() {
                if let Some(content) = first_candidate["content"]["parts"][0]["text"].as_str() {
                    return Ok(content.to_string());
                }
            }
        }

        Err(anyhow::anyhow!("Failed to extract response from Gemini: {:?}", json_res))
    }
}

#[async_trait::async_trait]
impl AIBackend for GeminiBackend {
    async fn initialize(&mut self, toon_str: String) -> anyhow::Result<()> {
        // Read file content
        //let file_content = fs::read_to_string(&file_path)?;
        self.file_content = Some(toon_str.clone());
        
        let mut spell: String = format!("{}", SPELL);
        let pr = private_reasonings();
        if pr.is_some() {
            spell = format!("{}\n# ADVANCED RULES: {}", spell, pr.unwrap());
        }
        // Initialize conversation with file content
        let initial_message = GeminiMessage {
            role: "user".to_string(),
            parts: vec![
                GeminiPart::Text { 
                    text: format!("I'm uploading a performance report.\n{} detect from question.\nPlease analyze it and be ready to answer questions about it.\n\nReport content:\n{}", spell, toon_str)
                }
            ],
        };
        
        self.conversation_history.push(initial_message.clone());
        
        // Get initial response
        let response = self.send_to_gemini(&self.conversation_history).await?;
        
        // Add assistant response to history
        self.conversation_history.push(GeminiMessage {
            role: "model".to_string(),
            parts: vec![GeminiPart::Text { text: response.clone() }],
        });
        
        println!("✅ Gemini initialized with file content");
        Ok(())
    }

    async fn send_message(&self, message: &str) -> anyhow::Result<String> {
        let mut messages = self.conversation_history.clone();
        
        // Add user message
        messages.push(GeminiMessage {
            role: "user".to_string(),
            parts: vec![GeminiPart::Text { text: message.to_string() }],
        });
        
        // Send to Gemini
        let response = self.send_to_gemini(&messages).await?;
        
        Ok(response)
    }
}

// App state with dynamic backend
pub struct AppState {
    backend: Arc<Mutex<Box<dyn AIBackend>>>,
}

// Main backend function
#[tokio::main]
pub async fn backend_ai(reportfile: String, backend_type: BackendType, model_name: String, toon_str: String) -> anyhow::Result<()> {    
    let backend: Box<dyn AIBackend> = match backend_type {
        BackendType::OpenAI => {
            let api_key = env::var("OPENAI_API_KEY")
                .expect("You have to set OPENAI_API_KEY variable in .env");
            let assistant_id = env::var("OPENAI_ASST_ID")
                .expect("You have to set OPENAI_ASST_ID variable in .env");
            Box::new(OpenAIBackend::new(api_key, assistant_id))
        },
        BackendType::Gemini => {
            let api_key = env::var("GEMINI_API_KEY")
                .expect("You have to set GEMINI_API_KEY variable in .env");
            
            Box::new(GeminiBackend::new(api_key, model_name))
        },
    };
    
    let backend_port = env::var("PORT").unwrap_or("3000".to_string());
    
    // Initialize backend with file
    let mut backend_mut = backend;
    if let Err(e) = backend_mut.initialize(toon_str).await {
        eprintln!("❌ Backend initialization failed: {:?}", e);
        return Err(e);
    }
    
    let state = Arc::new(AppState {
        backend: Arc::new(Mutex::new(backend_mut)),
    });

    let app = Router::new()
        .route("/api/chat", post(chat_handler))
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", backend_port)).await?;
    println!("🚀 Server running on http://127.0.0.1:{}", backend_port);
    axum::serve(listener, app).await?;
    Ok(())
}

// Unified chat handler
async fn chat_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UserMessage>,
) -> impl IntoResponse {
    let backend = state.backend.lock().await;
    
    match backend.send_message(&payload.message).await {
        Ok(reply) => (StatusCode::OK, Json(AIResponse { reply })).into_response(),
        Err(err) => {
            eprintln!("Error processing message: {:?}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to process message").into_response()
        }
    }
}

// Parse command line arguments
pub fn parse_backend_type(args: &str) -> Result<BackendType, String> {
    let mut btype = args; 
    if args.contains(":") {
        btype = args.split(":").collect::<Vec<&str>>()[0]; //for google you have to specify model - for example google:gemini-2.5-flash
    }
    match btype {
        "openai" => Ok(BackendType::OpenAI),
        "google" => Ok(BackendType::Gemini),
        _ => Err(format!("Backend must be 'openai' or 'google' -> found: {}",args)),
    }
}
