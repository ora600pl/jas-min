use colored::Colorize;
use reqwest::{Client};
use reqwest::multipart::{Form, Part};
use serde_json::json;
use std::{env, fs, collections::HashMap, sync::Arc, path::Path};
use axum::{routing::post, Router, Json, extract::State, http::StatusCode, response::IntoResponse};
use dotenv::dotenv;
use serde::{Deserialize, Serialize};
use tower_http::cors::{CorsLayer, Any};
use tokio::sync::Mutex;
use qdrant_client::qdrant::{Condition, Filter, SearchParamsBuilder, SearchPointsBuilder, SearchPoints};
use qdrant_client::Qdrant;


static SPELL: &str =   "Assuming that correlation in the log is Pearson correlation, suggest which wait events are the heaviest for database performance, 
                        which SQL IDs would require further performance analyze and which statistics are crucial for performance problems. 
                        Explain the meaning of wait events and statistics that are problematic for performance in your opinion.
                        Divide wait events to foreground and background wait event sections.
                        Summerize information about each wait event, SQL_ID and statistic you point out.
                        For SQL_ID you have to summerize information about other top sections and point out correlated wait events. 
                        Compare AVG and STDDEV values for SQLs and wait events and interpret it. 
                        For SQL_ID point out SQLs that have small execution time for single execution but are executed many times, causing performance issues. 
                        Print different sections for SQLs that are causing perfromance problems because of number of executions and diffent section for SQLs that are just executing slowly. 
                        Format answear pretty to read it easly in terminal.
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

#[tokio::main]
pub async fn chat_gpt(logfile_name: &str, vendor_model_lang: Vec<&str>) -> Result<(), Box<dyn std::error::Error>> {

    println!("{}{}{}","=== Consulting ChatGPT model: ".bright_cyan(), vendor_model_lang[1]," ===".bright_cyan());
    let api_key = env::var("OPENAI_API_KEY").expect("You have to set OPENAI_API_KEY env variable");

    let log_content = fs::read_to_string(logfile_name).expect(&format!("Can't open file {}", logfile_name));

    let client = Client::new();

    let mut spell: String = format!("{} {}", SPELL, vendor_model_lang[2]);
    let pr = private_reasoninings();
    if pr.is_some() {
        spell = format!("{}\n{}", spell, pr.unwrap());
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
    //     spell = format!("{}DOKUMENTY:\n{}\n\nPytanie: {}", SPELL_RAG, context, spell);
    // }
    
    // println!("Your final prompt is: \n {}", spell);
    // println!(" <=============== RESPONSE ===============> \n\n");

    /* **************************** */

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&json!({
            "model": format!("{}", vendor_model_lang[1]),
            "messages": [
                {"role": "system", "content": "You are an Oracle Database performance tuning expert."},
                {"role": "user", "content": format!("{} \n {}", spell, log_content)}
            ],
            "max_tokens": 4096
        }))
        .send()
        .await.unwrap();

    let json: serde_json::Value = response.json().await.unwrap();

    if json["error"].is_object() {
        println!("{}", json["error"]["message"].as_str().unwrap());
    } else {
        println!("{}", json["choices"][0]["message"]["content"].as_str().unwrap());
    }

    Ok(())
}

#[tokio::main]
pub async fn gemini(logfile_name: &str, vendor_model_lang: Vec<&str>) -> Result<(), Box<dyn std::error::Error>> {

    println!("{}{}{}","=== Consulting Google Gemini model: ".bright_cyan(), vendor_model_lang[1]," ===".bright_cyan());
    let api_key = env::var("GEMINI_API_KEY").expect("You have to set GEMINI_API_KEY env variable");

    let log_content = fs::read_to_string(logfile_name).expect(&format!("Can't open file {}", logfile_name));

    let client = Client::new();

    let mut spell: String = format!("{} {}", SPELL, vendor_model_lang[2]);
    let pr = private_reasoninings();
    if pr.is_some() {
        spell = format!("{}DOKUMENTY:\n{}\n\nPytanie: {}", SPELL_RAG, spell, pr.unwrap());
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
    
    let response = client
        .post(format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}", vendor_model_lang[1], api_key))
        .header("Content-Type", "application/json")
        .json(&json!({
            "contents": [{
                "parts": [{
                    "text": format!("{} \n {}", spell, log_content)
                }]
            }],
            "generationConfig": {
                    "maxOutputTokens": 1000000
            }
        }))
        .send()
        .await.unwrap();

        if response.status().is_success() {
            let json: serde_json::Value = response.json().await.unwrap();
            let response = json["candidates"][0]["content"]["parts"][0]["text"].as_str().unwrap();
            println!("{}", response);
            //println!("Parts: {}", json["candidates"][0]["content"]["parts"].as_array().iter().len());
            //fs::write("response.html", response.as_bytes());
        } else {
            println!("Błąd: {}", response.status());
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

pub struct AppState {
    client: reqwest::Client,
    api_key: String,
    assistant_id: String,
    thread_id: Mutex<Option<String>>,
}

pub async fn backend_ai(reportfile: String) -> anyhow::Result<()> {
    dotenv().ok();
    let api_key = env::var("OPENAI_API_KEY").expect("You have to set OPENAI_API_KEY variable in .env");
    let assistant_id = env::var("OPENAI_ASST_ID").expect("You have to set OPENAI_ASST_ID variable in .env");
    let bckend_port = env::var("PORT").unwrap_or("3000".to_string());

    let state = Arc::new(AppState {
        client: reqwest::Client::new(),
        api_key,
        assistant_id,
        thread_id: Mutex::new(None),
    });
    let thread_id = create_thread_with_file(&state, reportfile).await?;
    // Store thread_id in shared state
    {
        let mut id_lock = state.thread_id.lock().await;
        *id_lock = Some(thread_id);
    }

    let app = Router::new()
        .route("/api/chat", post(chat_handler))
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}",bckend_port)).await.unwrap();
    axum::serve(listener, app).await?;
    Ok(())
}

async fn chat_handler(State(state): State<Arc<AppState>>, Json(payload): Json<UserMessage>) -> impl IntoResponse {
    let thread_id = {
        let id_lock = state.thread_id.lock().await;
        match &*id_lock {
            Some(id) => id.clone(), // clone the String for local use
            None => {
                eprintln!("Thread ID not initialized");
                return (StatusCode::INTERNAL_SERVER_ERROR, "Thread not initialized").into_response();
            }
        }
    };
    
    if let Err(err) = create_message(&state, &thread_id, &payload.message).await {
        eprintln!("Failed to create message: {:?}", err);
        return (StatusCode::INTERNAL_SERVER_ERROR, "Message creation failed").into_response();
    }

    let run_id = match run_assistant(&state, &thread_id).await {
        Ok(id) => id,
        Err(err) => {
            eprintln!("Failed to run assistant: {:?}", err);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Assistant run failed").into_response();
        }
    };

    if let Err(err) = wait_for_completion(&state, &thread_id, &run_id).await {
        eprintln!("Error waiting for completion: {:?}", err);
        return (StatusCode::INTERNAL_SERVER_ERROR, "Completion failed").into_response();
    }

    match get_reply(&state, &thread_id).await {
        Ok(reply) => (StatusCode::OK, Json(AIResponse { reply })).into_response(),
        Err(err) => {
            eprintln!("Failed to get reply: {:?}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, "Reply retrieval failed").into_response()
        }
    }
}

async fn create_message(state: &AppState, thread_id: &str, content: &str) -> anyhow::Result<()> {
    let url = format!("https://api.openai.com/v1/threads/{}/messages", thread_id);

    let spell = content.to_string();

    /* Code for future RAG -> RAG */
    // println!("BEFORE: {}", spell);
    // if let Ok(qdrant_url) = env::var("QDRANT_URL") {
    //     let embedding = embed_text(&content).await.unwrap();
    //     let context =  retrieve_context(&embedding).await.unwrap();
    //     if content.len() >= 10 {
    //         spell = format!("{}DOKUMENTY:\n{}\n\nPytanie: {}", SPELL_RAG, context, spell);
    //     }
    // }
    // println!("AFTER: {}", spell);
    /* ************************* */

    let mut body = HashMap::new();
    body.insert("role", "user");
    body.insert("content", &spell);

    state.client.post(&url)
        .bearer_auth(&state.api_key)
        .header("OpenAI-Beta", "assistants=v2")
        .json(&body)
        .send().await?
        .error_for_status()?;

    Ok(())
}

async fn run_assistant(state: &AppState, thread_id: &str) -> anyhow::Result<String> {
    let url = format!("https://api.openai.com/v1/threads/{}/runs", thread_id);

    let mut body = HashMap::new();
    body.insert("assistant_id", &state.assistant_id);

    let res = state.client.post(&url)
        .bearer_auth(&state.api_key)
        .header("OpenAI-Beta", "assistants=v2")
        .json(&body)
        .send().await?
        .json::<serde_json::Value>().await?;

    Ok(res["id"].as_str().unwrap().to_string())
}

async fn wait_for_completion(state: &AppState, thread_id: &str, run_id: &str) -> anyhow::Result<()> {
    loop {
        let url = format!("https://api.openai.com/v1/threads/{}/runs/{}", thread_id, run_id);
        let res = state.client
            .get(&url)
            .bearer_auth(&state.api_key)
            .header("OpenAI-Beta", "assistants=v2")
            .send().await?
            .json::<serde_json::Value>().await?;

        let status = res.get("status").and_then(|s| s.as_str());

        match status {
            Some("completed") => return Ok(()),
            Some("failed") | Some("cancelled") | Some("expired") => {
                return Err(anyhow::anyhow!("Run failed or was cancelled/expired: {:?}", res))
            },
            Some(_) => {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            },
            None => {
                return Err(anyhow::anyhow!("Missing 'status' field in response: {:?}", res));
            }
        }
    }
}

async fn get_reply(state: &AppState, thread_id: &str) -> anyhow::Result<String> {
    let url = format!("https://api.openai.com/v1/threads/{}/messages", thread_id);

    let res = state.client
        .get(&url)
        .bearer_auth(&state.api_key)
        .header("OpenAI-Beta", "assistants=v2")
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    //println!("Message response: {:#?}", res);
    if let Some(reply) = res["data"][0]["content"][0]["text"]["value"].as_str() {
        //println!("{}",reply);
        return Ok(reply.to_string());
        
    } else {
        Err(anyhow::anyhow!("Failed to extract assistant reply: {:?}", res))
    }
}

pub async fn create_thread_with_file(state: &AppState, file_path: String) -> anyhow::Result<String> {
    // === Step 1: Read file content ===
    let file_bytes = fs::read(&file_path)?;
    let file_name = Path::new(&file_path).file_name().unwrap_or_else(|| std::ffi::OsStr::new("jasmin_report.txt")).to_string_lossy().to_string();

    // === Step 2: Upload file to OpenAI ===
    let file_part = Part::bytes((file_bytes)).file_name(file_name).mime_str("text/plain")?;

    let form = Form::new().part("file", file_part).text("purpose", "assistants");

    let upload_res = state.client
        .post("https://api.openai.com/v1/files")
        .bearer_auth(&state.api_key)
        .multipart(form)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let file_id = upload_res.get("id").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Failed to upload file: {:?}", upload_res))?;

    println!("✅ File uploaded with ID: {}", file_id);

    // === Step 3: Create thread with intro message ===
    let thread_res = state.client
        .post("https://api.openai.com/v1/threads")
        .bearer_auth(&state.api_key)
        .header("OpenAI-Beta", "assistants=v2")
        .json(&json!({
            "messages": [
                {
                    "role": "user",
                    "content": "Uploading file with performance report"
                }
            ]
        }))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let thread_id = thread_res.get("id").and_then(|id| id.as_str())
        .ok_or_else(|| anyhow::anyhow!("Failed to create thread: {:?}", thread_res))?;

    println!("✅ Thread created: {}", thread_id);

    // === Step 4: Attach file to thread ===
    let message_url = format!("https://api.openai.com/v1/threads/{}/messages", thread_id);
    let file_msg_res = state.client
        .post(&message_url)
        .bearer_auth(&state.api_key)
        .header("OpenAI-Beta", "assistants=v2")
        .json(&serde_json::json!({
            "role": "user",
            "content": "Please analyze the attached performance report.",
            "attachments": [
                {
                    "file_id": file_id,
                    "tools": [{ "type": "file_search" }]
                }
            ]
        }))
        .send()
        .await?;
    //println!("Raw response (attach file): {:?}", file_msg_res.status());
    if !file_msg_res.status().is_success() {
        let status = file_msg_res.status(); // ✅ get status first
        let err_text = file_msg_res.text().await?; // consumes file_msg_res
        eprintln!("Attach file failed. Status: {}, Response body: {}",status, err_text);
        return Err(anyhow::anyhow!("Failed to attach file to thread."));
    }

    println!("📎 File attached to thread.");

    Ok(thread_id.to_string())
}