#![allow(dead_code, unused)]
#[macro_use]
use rouille::*;
use std::io::Read;
use std::str;
use std::fs;
use clap::Parser;


mod awr;
mod ostask;

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
    #[clap(short, long, default_value="NO")]
    file: String,

    ///Parse whole directory of files
	#[clap(short, long, default_value="NO")]
    directory: String,
}


fn main() {

	let args = Args::parse(); 

	if args.file == "NO" && args.directory == "NO" {

		println!("Now listening on {}", &args.server);

		rouille::start_server(args.server, move |request| {
			router!(request,
				(GET) (/) => {
					rouille::Response::text("AWR/Statspack to JSON parser")
				},
				(POST) (/) => {
					rouille::Response::text("AWR/Statspack to JSON parser")
				},
				(GET) (/parse/{fname: String}) => {
					let awr_doc = match awr::parse_awr_report(&fname, false) {
						Ok(awr) => awr,
						Err(e) => format!("{{\"status\": \"ERROR\", \"msg\": \"Problem with a file {} - error: {}\"}}", &fname, e),
					};
					rouille::Response::text(awr_doc)
				},
				(GET) (/parsedir/{fname: String}) => {
					let awr_doc = match awr::parse_awr_dir(&fname) {
						Ok(awr) => awr,
						Err(e) => format!("{{\"status\": \"ERROR\", \"msg\": \"Problem with a file {} - error: {}\"}}", &fname, e),
					};
					rouille::Response::text(awr_doc)
				},
				(POST) (/parse) => {
					let mut data = request.data().expect("{{\"status\": \"ERROR\", \"msg\": \"Can't extract data from request\"}}");
					let mut buf = Vec::new();
					match data.read_to_end(&mut buf) {
						Ok(_) => (),
						Err(_) => return Response::text("{{\"status\": \"ERROR\", \"msg\": \"Can't read body from request\"}}")
					};
					println!("raw request string {:#?}", str::from_utf8(&buf));
					println!("raw request object {:#?}", request);
					let body: String = str::from_utf8(&buf).unwrap().to_string();
					
					let awr_doc = match awr::parse_awr_report(&body, true) {
						Ok(awr) => awr,
						Err(e) => format!("{{\"status\": \"ERROR\", \"msg\": \"Problem with a request - error: {}\"}}", e),
					};

					rouille::Response::text(awr_doc)
				},
				(POST) (/ostask) => {
					let mut data = request.data().expect("{{\"status\": \"ERROR\", \"msg\": \"Can't extract data from request\"}}");
					let mut buf = Vec::new();
					match data.read_to_end(&mut buf) {
						Ok(_) => (),
						Err(_) => return Response::text("{{\"status\": \"ERROR\", \"msg\": \"Can't read body from request\"}}")
					};
					println!("raw request string {:#?}", str::from_utf8(&buf));
					println!("raw request object {:#?}", request);
					let body: String = str::from_utf8(&buf).unwrap().to_string();
					let response: String = ostask::execute_command(body);
					rouille::Response::text(response)
				},
				_ => rouille::Response::empty_404()
			)
		});
	} else if args.file != "NO" {
		let awr_doc = awr::parse_awr_report(&args.file, false).unwrap();
		println!("{}", awr_doc);
	} else if args.directory != "NO" {
		let awr_doc = awr::parse_awr_dir(&args.directory).unwrap();
		println!("{}", awr_doc);
	}
}
