use colored::Colorize;
use reqwest::Client;
use serde_json::json;
use std::{env, fs};

#[tokio::main]
pub async fn chat_gpt(logfile_name: &str, model_language: String) -> Result<(), Box<dyn std::error::Error>> {

    let model_lang = model_language.split(":").collect::<Vec<&str>>();
    println!("{}{}{}","=== Consulting ChatGPT model: ".bright_cyan(), model_lang[0]," ===".bright_cyan());
    let api_key = env::var("OPENAI_API_KEY").expect("You have to set OPENAI_API_KEY env variable");

    let log_content = fs::read_to_string(logfile_name).expect(&format!("Can't open file {}", logfile_name));

    let client = Client::new();

    let spell: String = format!("Assuming that correlation in the log is Pearson correlation, suggest which wait events are the heaviest for database performance, 
                                 which SQL IDs would require further performance analyze and which statistics are crucial for performance problems. 
                                 Explain the meaning of wait events and statistics that are problematic for performance in your opinion.
                                 Summerize information about each wait event, SQL_ID and statistic you point out.
                                 For SQL_ID you have to summerize information about other top sections. 
                                 Write answear in language: {}", model_lang[1]);

    
    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&json!({
            "model": format!("{}", model_lang[0]),
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