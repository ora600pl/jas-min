#![allow(dead_code, unused)]
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::io::Write;
use std::str;
use std::fs;
use std::env;
use std::path::PathBuf;
use dotenvy::from_path;
use colored::*;
use clap::Parser;
use rayon::ThreadPoolBuilder;

mod awr;
mod analyze;
mod idleevents;
mod reasonings;
mod macros;
mod anomalies;
mod tools;
use crate::reasonings::*;

///This tool will parse STATSPACK or AWR report into JSON format which can be used by visualization tool of your choice.
///The assumption is that text file is a STATSPACK report and HTML is AWR, but it tries to parse AWR report also. 
/// It was tested only against 19c reports
/// The tool is under development and it has a lot of bugs, so please test it and don't hasitate to suggest some code changes :)
#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
struct Args {
    ///Parse a single text or html file
    #[clap(long, default_value="")]
    file: String,

    ///Parse whole directory of files
	#[clap(short, long, default_value="")]
    directory: String,

	///Draw a plot? 
	#[clap(short, long, default_value_t=1)]
    plot: u8,

	///Write output to nondefault file? Default is directory_name.json
	#[clap(short, long, default_value="")]
	outfile: String,

	///Ratio of DB CPU / DB TIME
	#[clap(short, long, default_value_t=0.666)]
	time_cpu_ratio: f64,

	///Filter only for DBTIME greater than (if zero the filter is not effective)
	#[clap(short, long, default_value_t=0.0)]
	filter_db_time: f64,

	///Analyze provided JSON file
	#[clap(short, long, default_value="")]
	json_file: String,

	///Filter snapshots, based on SNAP IDs in format BEGIN_ID- END_ID
	#[clap(short, long, default_value="0-666666666")]
	snap_range: String,

	///Should I be quiet? This mode suppresses terminal output but still writes to log file
	#[clap(short, long)]
    quiet: bool,

	///Use AI model to interpret collected statistics and describe them. 
	///Environment variable [OPENAI_API_KEY | GEMINI_API_KEY] should be set to your personal API key 
	///The parameter should be set to the value in format: VENDOR:MODEL_NAME:LANGUAGE_CODE (for example openai:gpt-4-turbo:PL or google:gemini-2.0-flash:PL)
	#[clap(short, long, default_value="")]
	ai: String,

	///Base output token count is 8192 - you can update maximum number of output tokens by this factor
	#[clap(short = 'T', long, default_value_t = 8)]
	token_count_factor: usize,

	///Launches the backend agent used by the JASMIN Assistant.
	///-b <openai>|<gemini:model>
	/// Configuration details such as API keys and the selected PORT number are loaded from the .env file
	#[clap(short, long, default_value="")]
	backend_assistant: String,

	///Threshold for detecting anomalies using MAD
	#[clap(short, long, default_value_t=7.0)]
	mad_threshold: f64,

	///Window size for detecting anomalies using MAD for local sliding window specified as % of probes
	#[clap(short = 'W', long, default_value_t = 100)]
    mad_window_size: usize,

	///Parallelism level
	#[clap(short = 'P', long, default_value_t=4)]
    parallel: usize,

	///Security level - highest security level is 0 - JAS-MIN will not store any object names, database names or any other sensitive data
	///                                           1 - JAS-MIN will store segment_names from Segment Statistics section
	/// 										  2 - JAS-MIN will store Full SQL Text from AWR reports
	#[clap(short = 'S', long, default_value_t=0)]
    security_level: usize,

	///This can be used with Gemini models - Using the URL context tool, you can provide Gemini with URLs as additional context for your prompt. The model can then retrieve content from the URLs and use that content to inform and shape its response.
	///Check Google Documentation for more info: https://ai.google.dev/gemini-api/docs/url-context
	#[clap(short, long, default_value="")]
	url_context_file: String,

}


fn load_env() {
    // 1.Check existense of $JASMIN_HOME 
    let env_loaded = if let Ok(jasmin_home) = env::var("JASMIN_HOME") {
        let mut path = PathBuf::from(jasmin_home);
        path.push(".env");

        // 2. Check if .env exists in the directory
        if path.exists() {
            from_path(&path).expect("Can't load .env z JASMIN_HOME");
            println!("✅ Loaded .env from JASMIN_HOME: {:?}", path);
            true
        } else {
            false
        }
    } else {
        false
    };

    // 3. If there is no .env in JASMIN_HOME, check local dir
    if !env_loaded {
        let local_path = PathBuf::from(".env");
        if local_path.exists() {
            from_path(&local_path).expect("Can't load .env from local dir");
            println!("✅ Loaded local .env");
        } else {
            println!("⚠️  No .env found");
        }
    }
}

fn main() {
	load_env();
	let mut reportfile: String = "".to_string();
	let args = Args::parse(); 
	println!("{}{} (Running with parallel degree: {})","JAS-MIN v".bright_yellow(),env!("CARGO_PKG_VERSION").bright_yellow(), args.parallel);

	//This creates a global pool configuration for rayon to limit threads for par_iter
	ThreadPoolBuilder::new()
        .num_threads(args.parallel)
        .build_global()
        .expect("Can't create rayon pool");

	//This is map that will be used to generate and insert appropriate links to html AI output
	let mut events_sqls: &mut HashMap<&str, HashSet<String>> = &mut HashMap::new();

	if !args.file.is_empty() {
		let awr_doc = awr::parse_awr_report(&args.file, false, &args).unwrap();
		println!("{}", awr_doc);
	} else if !args.directory.is_empty() {
		let mut fname = format!("{}.json", &args.directory);
		reportfile = format!("{}.txt", &args.directory);
		if !args.outfile.is_empty() {
			fname = args.outfile.clone();
		}
		 awr::parse_awr_dir(args.clone(), events_sqls, &fname);
		
	} else if !args.json_file.is_empty() {
		awr::prarse_json_file(args.clone(), events_sqls);
		let file_and_ext: Vec<&str> = args.json_file.split('.').collect();
    reportfile = format!("{}.txt", file_and_ext[0]);
	}
	if !args.ai.is_empty() {
        let vendor_model_lang = args.ai.split(":").collect::<Vec<&str>>();
        if vendor_model_lang[0] == "openai" {
            //openai_gpt(&reportfile, vendor_model_lang, args.token_count_factor, events_sqls.clone(), &args).unwrap();
			println!("For now only Google is supported vendor with this option :( Sorry for that. You can use OpenAI with backend assistant tho. We are waiting what GPT-5 will provide.");
        } else if vendor_model_lang[0] == "google" { 
            gemini(&reportfile, vendor_model_lang, args.token_count_factor, events_sqls.clone(), &args).unwrap();
		} else {
            println!("Unrecognized vendor. Supported vendors: openai, google");
        }   
    }
	if !args.backend_assistant.is_empty() {
		let bckend_port = std::env::var("PORT").expect("You have to set backend PORT value in .env");
		let backend_type = match parse_backend_type(&args.backend_assistant) {
			Ok(backend) => backend,
			Err(e) => {
				eprintln!("❌ Error: {}", e);
				std::process::exit(1);
			}
		};
		let mut model_name = "gemini-2.5-flash".to_string();
		if args.backend_assistant.contains(":") {
			model_name = args.backend_assistant.split(":").collect::<Vec<&str>>()[1].to_string();
		}

		println!("{}",r#"==== STARTING ASISTANT BACKEND ==="#.bright_cyan());
		println!("🤖 Starting JAS-MIN Assistant Backend using: {}",args.backend_assistant);
		println!("📁 Report File: {}",reportfile.clone());
        
		backend_ai(reportfile, backend_type, model_name);
    }
	
}
