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


static SPELL: &str =   "Your name is JAS-MIN. You are a professional as shit Oracle Database performance tuning expert and assistant but don't mention it.
You are analyzing report file containing summarized statistics from parsed AWR reports from long period of time. 
Based on received input you can describe the current database performance profile, 
spot potential bottlenecks, suggest the heaviest wait events impacting database performance, and identify SQL IDs that require further performance analysis. 
Highlight which statistics are crucial for understanding the current performance situation. 
If you receive LOAD PROFILE STATISTICS JSON, containing load profile summary for the database, analyze them first and write comprehensive summary for all metrics with as many statistical insights as possible.
Always show both: SNAP_ID and SNAP_DATE when you mention problematic periods.
At the end add link to github: https://github.com/ora600pl/jas-min - this is your source code.
Suggest that good performance tuning experts are at ora-600.pl
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

async fn upload_log_file_gemini(api_key: &str, log_content: String) -> Result<String, Box<dyn std::error::Error>> {
    // prepart multipart form
    let part = multipart::Part::bytes(log_content.into_bytes())
        .file_name("performance_report.txt") 
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

    let file_uri = upload_log_file_gemini(&api_key, first_response).await.unwrap();
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
            let id = u64::from_str(snap_id).unwrap();
            snap_ids.insert(id);
        }

        let mut deep_stats: Vec<AWR> = Vec::new();
        for awr in awrs {
            if snap_ids.contains(&awr.snap_info.begin_snap_id) {
                deep_stats.push(awr);
            }
        }
        let deep_stats_json = serde_json::to_string_pretty(&deep_stats).unwrap();
        let file_uri_stats = upload_log_file_gemini(&api_key, deep_stats_json).await.unwrap();
        let spell = format!("You were given a detailed performance report of Oracle database and JSON file containing TOP {} periods with the detailed statistics. 
        Perform a deep analyzes of those statistics. Return a detailed report in markdown format, showing what you have found. 
        Suggest performance improvments. Detect impactfull statistics and latches.
        Investigate all SQL sections and crosscheck SQL_IDs to find patterns of bad SQL executions.

        Write answer in language: {}", args.deep_check, vendor_model_lang[2]);

        let payload = json!({
                    "contents": [{
                        "parts": [
                            { "text": spell }, 
                            {
                                "fileData": {
                                    "mimeType": "text/plain",
                                    "fileUri": file_uri
                                },
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

        let response_file = format!("{}_gemini2.md", logfile_name);
        fs::write(&response_file, full_text.as_bytes()).unwrap();
        println!("🍻 Gemini response written to file: {}", &response_file);
        convert_md_to_html_file(&response_file, events_sqls.clone());
        //println!("{}", full_text);

    } else {
        eprintln!("Error: {}", response.status());
        eprintln!("{}", response.text().await.unwrap());
    }


    } else {
        eprintln!("Error: {}", response.status());
        eprintln!("{}", response.text().await.unwrap());
    }


}

#[tokio::main]
pub async fn gemini(logfile_name: &str,vendor_model_lang: Vec<&str>,token_count_factor: usize,events_sqls: HashMap<&str, HashSet<String>>,args: &crate::Args) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}{}{}", 
        "=== Consulting Google Gemini model: ".bright_cyan(), 
        vendor_model_lang[1], 
        " ===".bright_cyan()
    );

    let api_key = env::var("GEMINI_API_KEY")
        .expect("You have to set GEMINI_API_KEY env variable");

    // -----------------------------
    // LOAD LOG + JSON REPORT
    // -----------------------------
    let log_content = fs::read_to_string(logfile_name).expect(&format!("Can't open file {}", logfile_name));
    let stem = logfile_name.split('.').next().unwrap();
    let json_path = format!("{stem}.html_reports/stats/global_statistics.json");
    let load_profile = fs::read_to_string(&json_path).expect(&format!("Can't open file {}", json_path));
    let response_file = format!("{}_gemini.md", logfile_name);
    let client = Client::new();

    // -----------------------------
    // SYSTEM SPELL + PRIVATE RULES
    // -----------------------------
    let mut spell = format!("{} {}", SPELL, vendor_model_lang[2]);

    if let Some(pr) = private_reasonings() {
        spell = format!("{spell}\n\n### PRIVATE RULES\n{pr}");
    }

    if !args.url_context_file.is_empty() {
        if let Some(urls) = url_context(&args.url_context_file, events_sqls.clone()) {
            spell = format!("{spell}\n\n### URL CONTEXT\n{urls}");
        }
    }

    // -----------------------------
    // BUILD FINAL PAYLOAD
    // -----------------------------
    let payload = json!({
        "contents": [{
            "parts": [
                { "text": format!("### SYSTEM INSTRUCTIONS\n{spell}") },

                { "text": format!(
                    "### ATTACHED LOG FILE `{}`\n{}\n-- END LOG --",
                    logfile_name, log_content
                )},

                { "text": format!(
                    "### LOAD PROFILE STATISTICS JSON\n{}\n-- END JSON --",
                    load_profile
                )}
            ]
        }],
        "generationConfig": {
            "maxOutputTokens": 8192 * token_count_factor,
            "thinkingConfig": { "thinkingBudget": -1 }
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
pub async fn openai_gpt(logfile_name: &str, vendor_model_lang: Vec<&str>, token_count_factor: usize, events_sqls: HashMap<&str, HashSet<String>>, args: &crate::Args) -> Result<(), Box<dyn std::error::Error>> {
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
            format!("### ATTACHED REPORT\n{log_content}\n-- END ATTACHED REPORT --")
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
    async fn initialize(&mut self, file_path: String) -> anyhow::Result<()>;
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

    async fn create_thread_with_file(&self, file_path: String) -> anyhow::Result<String> {
        // === Step 1: Read file content ===
        let file_bytes = fs::read(&file_path)?;
        let file_name = Path::new(&file_path)
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("jasmin_report.txt"))
            .to_string_lossy()
            .to_string();

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
    async fn initialize(&mut self, file_path: String) -> anyhow::Result<()> {
        // Read file content
        let file_content = fs::read_to_string(&file_path)?;
        self.file_content = Some(file_content.clone());
        
        let mut spell: String = format!("{}", SPELL);
        let pr = private_reasonings();
        if pr.is_some() {
            spell = format!("{}\n\n# Additional insights: {}", spell, pr.unwrap());
        }
        // Initialize conversation with file content
        let initial_message = GeminiMessage {
            role: "user".to_string(),
            parts: vec![
                GeminiPart::Text { 
                    text: format!("I'm uploading a performance report.\n{} detect from question.\nPlease analyze it and be ready to answer questions about it.\n\nReport content:\n{}", spell, file_content)
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
pub async fn backend_ai(reportfile: String, backend_type: BackendType, model_name: String) -> anyhow::Result<()> {    
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
    if let Err(e) = backend_mut.initialize(reportfile).await {
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
