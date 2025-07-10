use colored::Colorize;
use reqwest::{Client, multipart};
use reqwest::multipart::{Form, Part};
use serde_json::json;
use std::env::Args;
use std::fmt::format;
use std::{env, fs, collections::HashMap, sync::Arc, path::Path};
use axum::{routing::post, Router, Json, extract::State, http::StatusCode, response::IntoResponse};
use dotenv::dotenv;
use serde::{Deserialize, Serialize};
use tower_http::cors::{CorsLayer, Any};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use qdrant_client::qdrant::{Condition, Filter, SearchParamsBuilder, SearchPointsBuilder, SearchPoints};
use qdrant_client::Qdrant;


static SPELL: &str =   "You are a sarcastic as shit Oracle Database performance tuning expert and assistant.
                        You are analyzing report file containing summarized statistics from parsed AWR reports from long period of time. 
                        Based on received input (like AWR, STATSPACK reports), you can describe the current database performance profile, 
                        spot potential bottlenecks, suggest the heaviest wait events impacting database performance, and identify SQL IDs that require further performance analysis. 
                        Highlight which statistics are crucial to understanding the current performance situation.
                        Write answear in language:";

static SPELL_RAG: &str = "Udziel odpowiedzi **tylko na podstawie poniższych dokumentów**.
                          Nie używaj własnej wiedzy. Zacytuj źródła. 
                          Jeśli brakuje danych, powiedz to wprost.:\n\n";


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



fn private_reasoninings() -> Option<String> {
    let r_content = fs::read_to_string("reasonings.txt");
    if r_content.is_err() {
        return None;
    }
    let r_content = r_content.unwrap();
    Some(r_content)
}

/* Embeddings based on OpenAI model - later it will have to be based on choosen model provided as argument */
async fn embed_text(text: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let api_key = env::var("OPENAI_API_KEY").unwrap();
    let client = Client::new();
    let res = client
        .post("https://api.openai.com/v1/embeddings")
        .bearer_auth(api_key)
        .json(&serde_json::json!({
            "model": "text-embedding-3-small",
            "input": text
        }))
        .send()
        .await.unwrap();

    let json: serde_json::Value = res.json().await?;
    let vector = json["data"][0]["embedding"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect();
    Ok(vector)
}

/* Function for retrievieng context from QDRANT database */
async fn retrieve_context(query_embedding: &[f32]) -> Result<String, Box<dyn std::error::Error>> {
    /* Setting QDRANT client based on QDRANT_URL variable */
    let client = Qdrant::from_url(&env::var("QDRANT_URL")
                         .expect("No QDRANT_URL in .env"))
                         .skip_compatibility_check().build().unwrap(); 

    /* determining collection anme */
    let collection = env::var("COLLECTION_NAME").expect("No COLLECTION_NAME in .env");

    /* search result based on vector search */
    let search_result = client.search_points(SearchPointsBuilder::new(collection.clone(), 
                                          query_embedding.to_vec(), 
                                          1).with_payload(true).build()).await.unwrap();

    /* Result points */
    let points = search_result.result;

    /* building context based on returned points */
    let mut context = String::new();
    for point in points {
        let payload = point.payload;
        if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
            context.push_str("- ");
            context.push_str(text);
            context.push('\n');
        }
    }

    Ok(context)
}

pub async fn chat_gpt(logfile_name: &str, vendor_model_lang: Vec<&str>, token_count_factor: usize) -> Result<(), Box<dyn std::error::Error>> {

    println!("{}{}{}","=== Consulting ChatGPT model: ".bright_cyan(), vendor_model_lang[1]," ===".bright_cyan());
    let api_key = env::var("OPENAI_API_KEY").expect("You have to set OPENAI_API_KEY env variable");
    let client = Client::new();

    let mut spell: String = format!("{} {}", SPELL, vendor_model_lang[2]);
    let pr = private_reasoninings();
    if pr.is_some() {
        spell = format!("{}\n{}", spell, pr.unwrap());
    }

    // === 0. Create temporary assistant dynamically ===
    let assistant_create_resp = client
        .post("https://api.openai.com/v1/assistants")
        .bearer_auth(&api_key)
        .header("OpenAI-Beta", "assistants=v2")
        .json(&json!({
            "model": vendor_model_lang[1], // e.g. gpt-4.1-2025-04-14
            "name": "Temp Oracle Assistant",
            "instructions": spell,
            "tools": [{ "type": "file_search" }]
        }))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let assistant_id: &str;
    if let Some(aid) = assistant_create_resp["id"].as_str() {
        assistant_id = aid;
        println!("🎩 Temporary assistant created: {}", assistant_id);
    } else {
        eprintln!("❌ Thread creation failed:\n{}", assistant_create_resp);
        return Ok(());
    }
    

    let log_content = fs::read_to_string(logfile_name).expect(&format!("Can't open file {}", logfile_name));
    let response_file = format!("{}_openai.md", logfile_name);

   

    // === 1. Upload pliku ===
    // prepart multipart form
    let part = multipart::Part::bytes(log_content.into_bytes())
        .file_name("performance_report.txt") 
        .mime_str("text/plain").unwrap(); //  MIME type as text

    // Create multipart form
    let form = multipart::Form::new().text("purpose", "assistants").part("file", part);

    let upload_resp = client
        .post("https://api.openai.com/v1/files")
        .bearer_auth(&api_key)
        .multipart(form)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    let file_id = upload_resp["id"].as_str().unwrap();
    println!("✅ File uploaded: {}", file_id);

    // === 2. Create thread ===
    let thread_resp_text = client
        .post("https://api.openai.com/v1/threads")
        .bearer_auth(&api_key)
        .header("OpenAI-Beta", "assistants=v2")
        .json(&json!({}))
        .send()
        .await?
        .text()
        .await?; 

    let thread_id: &str;
    let thread_json: serde_json::Value = serde_json::from_str(&thread_resp_text)?;
    if let Some(tid) = thread_json.get("id").and_then(|v| v.as_str()) {
        thread_id = tid;
        println!("🧵 Thread created: {}", thread_id);
    } else {
        eprintln!("❌ Thread creation failed:\n{}", thread_resp_text);
        return Ok(());
    }

    // === 3. Add user message ===
    let message_resp = client
        .post(format!("https://api.openai.com/v1/threads/{}/messages", thread_id))
        .bearer_auth(&api_key)
        .header("OpenAI-Beta", "assistants=v2")
        .json(&json!({
            "role": "user",
            "content": spell,
            "attachments": [{
                "file_id": file_id,
                "tools": [{ "type": "file_search" }]
            }]
        }))
        .send()
        .await?;
    println!("📩 Message sent");

    // === 3.5 Wait for vector store to process the file ===
    println!("⏳ Waiting for file processing to complete...");
    let mut attempts = 0;
    let max_attempts = 30;

    loop {
        let thread_url = format!("https://api.openai.com/v1/threads/{}", thread_id);
        let thread_res = client
            .get(&thread_url)
            .bearer_auth(&api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .send()
            .await?;

        if !thread_res.status().is_success() {
            eprintln!("Failed to get thread status: {}", thread_res.status());
            break;
        }

        let thread_data = thread_res.json::<serde_json::Value>().await?;
        if let Some(tool_resources) = thread_data.get("tool_resources") {
            if let Some(file_search) = tool_resources.get("file_search") {
                if let Some(vector_store_ids) = file_search.get("vector_store_ids") {
                    if let Some(first_id) = vector_store_ids.as_array().and_then(|arr| arr.get(0)).and_then(|v| v.as_str()) {
                        // Check vector store status
                        let url = format!("https://api.openai.com/v1/vector_stores/{}", first_id);
                        let res = client
                            .get(&url)
                            .bearer_auth(&api_key)
                            .header("OpenAI-Beta", "assistants=v2")
                            .send()
                            .await?;
                        if !res.status().is_success() {
                            eprintln!("Failed to get vector store status: {}", res.status());
                            break;
                        }

                        let json_res = res.json::<serde_json::Value>().await?;
                        let status = json_res["status"].as_str().unwrap_or("unknown");
                        println!("📊 Vector store status: {} (attempt {}/{})", status, attempts + 1, max_attempts);

                        if status == "completed" {
                            println!("✅ Vector store processing completed!");
                            break;
                        } else if status == "failed" {
                            eprintln!("❌ Vector store processing failed!");
                            break;
                        }
                    }
                }
            }
        }

        if attempts >= max_attempts {
            eprintln!("❌ File processing timeout after {} attempts", max_attempts);
            break;
        }

        sleep(Duration::from_secs(3)).await;
        attempts += 1;
    }

    // === 4. Run assistant ===
    let run_resp = client
        .post(format!("https://api.openai.com/v1/threads/{}/runs", thread_id))
        .bearer_auth(&api_key)
        .header("OpenAI-Beta", "assistants=v2")
        .json(&json!({
            "assistant_id": assistant_id,
            "tools": [{ "type": "file_search" }]
        }))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    let run_id = run_resp["id"].as_str().unwrap();
    println!("🏃 Run started: {}", run_id);

    // === 5. Poll status until completed ===
    loop {
        let status_resp = client
            .get(format!("https://api.openai.com/v1/threads/{}/runs/{}", thread_id, run_id))
            .bearer_auth(&api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let status = status_resp["status"].as_str().unwrap();
        println!("⏳ Status: {}", status);
        if status == "completed" {
            break;
        } else if status == "failed" {
            panic!("❌ Run failed");
        }
        sleep(Duration::from_secs(2)).await;
    }

    // === 6. Read response ===
    let messages_resp = client
        .get(format!("https://api.openai.com/v1/threads/{}/messages", thread_id))
        .bearer_auth(&api_key)
        .header("OpenAI-Beta", "assistants=v2")
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let messages = messages_resp["data"].as_array().unwrap();
    for msg in messages {
    if msg["role"].as_str().unwrap_or("") == "assistant" {
        if let Some(content) = msg["content"][0]["text"]["value"].as_str() {
            fs::write(&response_file, content.as_bytes())?;
            println!("🍻 OpenAI response written to file: {}", response_file);
        }
    }
}

    // === 7. Delete temporary assistant ===
    let delete_resp = client
        .delete(format!("https://api.openai.com/v1/assistants/{}", assistant_id))
        .bearer_auth(&api_key)
        .header("OpenAI-Beta", "assistants=v2")
        .send()
        .await?;

    if delete_resp.status().is_success() {
        println!("🗑️ Temporary assistant deleted: {}", assistant_id);
    } else {
        eprintln!("⚠️ Failed to delete assistant: {}", delete_resp.status());
    }

    Ok(())
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


pub async fn gemini(logfile_name: &str, vendor_model_lang: Vec<&str>, token_count_factor: usize) -> Result<(), Box<dyn std::error::Error>> {

    println!("{}{}{}","=== Consulting Google Gemini model: ".bright_cyan(), vendor_model_lang[1]," ===".bright_cyan());
    let api_key = env::var("GEMINI_API_KEY").expect("You have to set GEMINI_API_KEY env variable");

    let log_content = fs::read_to_string(logfile_name).expect(&format!("Can't open file {}", logfile_name));

    let response_file = format!("{}_gemini.md", logfile_name);

    let client = Client::new();

    let mut spell: String = format!("{} {}", SPELL, vendor_model_lang[2]);
    let pr = private_reasoninings();
    if pr.is_some() {
        spell = format!("{}\n\n# Additional insights: {}", spell, pr.unwrap());
    }
    
    /* Code for future RAG -> RAG */
    // if let Ok(qdrant_url) = env::var("QDRANT_URL") {
    //     let embedding = match embed_text(&spell).await {
    //         Ok(e) => e,
    //         Err(err) => return Err(err),
    //     };
    //     let context = match retrieve_context(&embedding).await {
    //         Ok(c) => c,
    //         Err(err) => return Err(err),           
    //     };
    //     spell = format!("{}{}{}", SPELL_RAG, context, spell);
    // }
    
    // println!("Your final prompt is: \n {}", spell);
    // println!(" <=============== RESPONSE ===============> \n\n");
    /* ****************************** */

    let file_uri = upload_log_file_gemini(&api_key, log_content).await.unwrap();

    let payload = json!({
            "contents": [{
                "parts": [
                    { "text": spell }, // Twój prompt/polecenie
                    {
                        "fileData": {
                            "mimeType": "text/plain",
                            "fileUri": file_uri
                        }
                    }
                ]
            }],
            "generationConfig": {
                "maxOutputTokens": 8192*token_count_factor
            }
            });

    let response = client
            .post(format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}", vendor_model_lang[1], api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await.unwrap();

    if response.status().is_success() {
        let json: serde_json::Value = response.json().await.unwrap();
        let response = json["candidates"][0]["content"]["parts"][0]["text"].as_str().unwrap();
        //println!("{}", response);
        //println!("Parts: {}", json["candidates"][0]["content"]["parts"].as_array().iter().len());
        fs::write(&response_file, response.as_bytes());
        println!("🍻 Gemini response written to file: {}", response_file);
    } else {
        eprintln!("Error: {}", response.status());
        eprintln!("{}", response.text().await.unwrap());
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
        let pr = private_reasoninings();
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
pub async fn backend_ai(reportfile: String, backend_type: BackendType, model_name: String) -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    
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
    backend_mut.initialize(reportfile).await?;
    
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
        btype = args.split(":").collect::<Vec<&str>>()[0]; //for gemini you have to specify model - for example gemini:gemini-2.5-flash
    }
    match btype {
        "openai" => Ok(BackendType::OpenAI),
        "gemini" => Ok(BackendType::Gemini),
        _ => Err(format!("Backend must be 'openai' or 'gemini' -> found: {}",args)),
    }
}