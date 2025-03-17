use colored::Colorize;
use reqwest::Client;
use serde_json::json;
use std::{env, fs};

static SPELL: &str =   "Assuming that correlation in the log is Pearson correlation, suggest which wait events are the heaviest for database performance, 
                        which SQL IDs would require further performance analyze and which statistics are crucial for performance problems. 
                        Explain the meaning of wait events and statistics that are problematic for performance in your opinion.
                        Divide wait events to foreground and background wait event sections.
                        Summerize information about each wait event, SQL_ID and statistic you point out.
                        For SQL_ID you have to summerize information about other top sections and point out correlated wait events. 
                        Compare AVG and STDDEV values for SQLs and wait events and interpret it. 
                        For SQL_ID point out SQLs that have small execution time for single execution but are executed many times, causing performance issues. 
                        Print different sections for SQLs that are causing perfromance problems because of number of executions and diffent section for SQLs that are just executing slowly. 
                        Format answear pretty, assuming 180 characters per line - it has to look nice in terminal.
                        Write answear in language:";

#[tokio::main]
pub async fn chat_gpt(logfile_name: &str, vendor_model_lang: Vec<&str>) -> Result<(), Box<dyn std::error::Error>> {

    println!("{}{}{}","=== Consulting ChatGPT model: ".bright_cyan(), vendor_model_lang[1]," ===".bright_cyan());
    let api_key = env::var("OPENAI_API_KEY").expect("You have to set OPENAI_API_KEY env variable");

    let log_content = fs::read_to_string(logfile_name).expect(&format!("Can't open file {}", logfile_name));

    let client = Client::new();

    let spell: String = format!("{} {}", SPELL, vendor_model_lang[2]);

    
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
            "max_tokens": 3000
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

    let spell: String = format!("{} {}", SPELL, vendor_model_lang[2]);
    
    let response = client
        .post(format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}", vendor_model_lang[1], api_key))
        .header("Content-Type", "application/json")
        .json(&json!({
            "contents": [{
                "parts": [{
                    "text": format!("{} \n {}", spell, log_content)
                }]
            }],
            "safetySettings": [
                {
                    "category": "HARM_CATEGORY_HARASSMENT",
                    "threshold": "BLOCK_NONE"
                },
                {
                    "category": "HARM_CATEGORY_HATE_SPEECH",
                    "threshold": "BLOCK_NONE"
                },
                {
                    "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                    "threshold": "BLOCK_NONE"
                },
                {
                    "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                    "threshold": "BLOCK_NONE"
                }
            ]
        }))
        .send()
        .await.unwrap();

        if response.status().is_success() {
            let json: serde_json::Value = response.json().await.unwrap();
            println!("{}", json["candidates"][0]["content"]["parts"][0]["text"].as_str().unwrap());
        } else {
            println!("Błąd: {}", response.status());
        }

    Ok(())
}