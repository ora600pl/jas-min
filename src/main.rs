#![allow(dead_code, unused)]
use std::io::Read;
use std::io::Write;
use std::str;
use std::fs;
use dotenv::dotenv;
use colored::*;
use clap::Parser;

mod awr;
mod analyze;
mod idleevents;
mod reasonings;
use crate::reasonings::{backend_ai};

///This tool will parse STATSPACK or AWR report into JSON format which can be used by visualization tool of your choice.
///The assumption is that text file is a STATSPACK report and HTML is AWR, but it tries to parse AWR report also. 
/// It was tested only against 19c reports
/// The tool is under development and it has a lot of bugs, so please test it and don't hasitate to suggest some code changes :)
#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
struct Args {
    ///Parse a single text or html file
    #[clap(long, default_value="NO")]
    file: String,

    ///Parse whole directory of files
	#[clap(short, long, default_value="NO")]
    directory: String,

	///Draw a plot? 
	#[clap(short, long, default_value_t=1)]
    plot: u8,

	 ///Write output to nondefault file? Default is directory_name.json
	 #[clap(short, long, default_value="NO")]
	 outfile: String,

	 ///Ratio of DB CPU / DB TIME
	 #[clap(short, long, default_value_t=0.666)]
	 time_cpu_ratio: f64,

	 ///Filter only for DBTIME greater than (if zero the filter is not effective)
	 #[clap(short, long, default_value_t=0.0)]
	 filter_db_time: f64,

	 ///Analyze provided JSON file
	 #[clap(short, long, default_value="NO")]
	 json_file: String,

	 ///Filter snapshots, based on dates in format BEGIN-END
	 #[clap(short, long, default_value="0-666666666")]
	 snap_range: String,

	 ///Should I be quiet? This mode suppresses terminal output but still writes to log file
	 #[clap(short, long)]
     quiet: bool,

	 ///Use AI model to interpret collected statistics and describe them. 
	 ///Environment variable [OPENAI_API_KEY | GEMINI_API_KEY] should be set to your personal API key 
	 ///The parameter should be set to the value in format: VENDOR:MODEL_NAME:LANGUAGE_CODE (for example openai:gpt-4-turbo:PL or google:gemini-2.0-flash:PL)
	 #[clap(short, long, default_value="NO")]
	 ai: String,

	/// Launches the backend agent used by the JASMIN Assistant. Configuration details such as API keys and the selected PORT number are loaded from the .env file
	 #[clap(short, long)]
	 backend_assistant: bool,
}

fn main() {
	let mut 
	reportfile: String = "".to_string();
	let args = Args::parse(); 
	println!("{}{}","JAS-MIN v".bright_yellow(),env!("CARGO_PKG_VERSION").bright_yellow());
	if args.file != "NO" {
		let awr_doc = awr::parse_awr_report(&args.file, false).unwrap();
		println!("{}", awr_doc);
	} else if args.directory != "NO" {
		let awr_doc = awr::parse_awr_dir(args.clone()).unwrap();
		let mut fname = format!("{}.json", &args.directory);
		reportfile = format!("{}.txt", &args.directory);
		if args.outfile != "NO" {
			fname = args.outfile;
		}
		let mut f = fs::File::create(fname).unwrap();
		f.write_all(awr_doc.as_bytes()).unwrap();
		
	} else if args.json_file != "NO" {
		awr::prarse_json_file(args.clone());
		let file_and_ext: Vec<&str> = args.json_file.split('.').collect();
    	reportfile = format!("{}.txt", file_and_ext[0]);
	}
	if args.backend_assistant {
		dotenv().ok();
		let bckend_port = std::env::var("PORT").expect("You have to set backend PORT value in .env");
		println!("{}",r#"==== STARTING ASISTANT BACKEND ==="#.bright_cyan());
		println!("🤖 Starting JAS-MIN Assistant Backend on http://loclahost:{}",bckend_port);
		println!("📁 Report File: {}",reportfile.clone());
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(backend_ai(reportfile));
    }
	
}
