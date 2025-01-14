#![allow(dead_code, unused)]
use std::io::Read;
use std::io::Write;
use std::str;
use std::fs;
use clap::Parser;

mod awr;
mod analyze;
mod idleevents;

///This tool will parse STATSPACK or AWR report into JSON format which can be used by visualization tool of your choice.
///The assumption is that text file is a STATSPACK report and HTML is AWR, but it tries to parse AWR report also. 
/// It was tested only against 19c reports
/// The tool is under development and it has a lot of bugs, so please test it and don't hasitate to suggest some code changes :)
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    ///Run in server mode - you can parse files via GET/POST methods. HTTP will listen on 6751 port by default
    #[clap(short, long, default_value="0.0.0.0:6751")]
    server: String,

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
}


fn main() {

	let args = Args::parse(); 

	if args.file != "NO" {
		let awr_doc = awr::parse_awr_report(&args.file, false).unwrap();
		println!("{}", awr_doc);
	} else if args.directory != "NO" {
		let awr_doc = awr::parse_awr_dir(&args.directory, args.plot, args.time_cpu_ratio, args.filter_db_time).unwrap();
		let mut fname = format!("{}.json", &args.directory);
		if args.outfile != "NO" {
			fname = args.outfile;
		}
		let mut f = fs::File::create(fname).unwrap();
		f.write_all(awr_doc.as_bytes()).unwrap();
		
	} else if args.json_file != "NO" {
		awr::prarse_json_file(args.json_file, args.time_cpu_ratio, args.filter_db_time);
	}
}
