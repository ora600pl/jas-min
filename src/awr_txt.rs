#![allow(dead_code, unused)]

use std::env;
use std::fs;
use serde::{Deserialize, Serialize};
use regex::Regex;
use std::collections::HashMap;
use substring::Substring;
use oracle::Connection;

const RE_FIELD: &str = "^(\\s|~|-)*((\\w|/|:|-|\\*|\\(|\\)|\\+)+\\s?)+";
const SQL_ID: &str = "([a-z]+|[0-9]+){13}$";
const OLD_HASH_VALUE: &str = "[0-9]{10}$";

#[derive(Serialize, Deserialize)]
struct Definitions {
	section_start: String,
	section_end: String,
	field_re: String,
	field_no: Vec<i32>,
	target_table: String,
}

#[derive(Clone)]
struct DefinitionsR {
	section_start: String,
	section_end: String,
	field_re: Regex,
	field_no: Vec<i32>,
	target_table: String,
}

/*
struct Stats {
	stat_name: String,
	stat_value: u64,
}*/

fn read_definition(fname: &str) -> HashMap<String, Definitions> {
    let def_file = fs::read_to_string(fname).expect(&format!("Something wrong with a file {} ", fname));
    let v_definitions: HashMap<String, Definitions> = serde_json::from_str(&def_file).expect("Wrong JSON format");
    for (key, def) in &v_definitions {
    	println!("Reading definition file {} {} {}", key, key.len(), def.section_start);
    }
    v_definitions
}

fn gather_stats(awr_fname: &str, def: HashMap<String, DefinitionsR>, audit_id: &str) {
    let conn = Connection::connect("awranalyzer", "awr", "//localhost:1521/audits").unwrap();
    conn.execute("alter session set nls_date_format='DD-Mon-YY HH24:MI:SS'", &[]).unwrap();

    let awr_rep = fs::read_to_string(awr_fname).expect(&format!("Couldn't open awr file {}", awr_fname));
    let awr_lines = awr_rep.split("\n").collect::<Vec<&str>>();
    let re_field = Regex::new(RE_FIELD).unwrap();
    let sql_id = Regex::new(SQL_ID).unwrap();
    let old_hash_value = Regex::new(OLD_HASH_VALUE).unwrap();
    let mut gather_stats = false;
    let mut analyze_header = false;
    let mut section_matched = false;
    let mut d: Option<&DefinitionsR> = None;
    let mut sql_txt = "insert into ".to_string();
    let mut field_headers: Vec<&str> = Vec::new();
    let mut begin_snap: String = String::new();
    let mut end_snap: String = String::new();
    let mut section_key: String = "".to_string();

    for line in awr_lines {
		for m in re_field.captures_iter(line) {
			//println!("{}", line);
			if line.contains("Begin Snap:") || line.contains("End Snap:") {
				let fields = line.split_whitespace().collect::<Vec<&str>>();
				if line.contains("Begin Snap:") {
					begin_snap = format!("{} {}", fields[3], fields[4]);
				} else if line.contains("End Snap:") {
					end_snap = format!("{} {}", fields[3], fields[4]);
				}
			}
			if gather_stats && line.starts_with(&d.unwrap().section_end) {
				gather_stats = false;
				section_matched = false;
				analyze_header = false;
				sql_txt = "insert into ".to_string();
				println!("Ended section {}", section_key);
			}

			let key = &m[0].trim().to_string();
			if def.contains_key(key) {
				d = def.get(key);
				section_matched = true;
				println!("Matched in AWR: {} \t file: {}", key, awr_fname);
				section_key = key.to_string();
			}
			if section_matched && line.starts_with(&d.unwrap().section_start) {
				analyze_header = true;
				println!("Found section {} and I will be analyzing header", &d.unwrap().section_start);
			}

			if section_matched && analyze_header && line.contains("---") && !gather_stats {
				gather_stats = true;
				field_headers = line.split_whitespace().collect::<Vec<&str>>();
				println!("Found {} fields in header", field_headers.len());
			} else if gather_stats {
				let mut field_no: i32 = 0;
				let mut found = 0;
				let mut field_data: Vec<i16> = Vec::new();
				let mut stat_name = m[0].to_string().clone();
				let line_fields = line.split_whitespace().collect::<Vec<&str>>();
				let mut found_sqlid = false;
				let mut line_pointer = 0;
				let mut line_pointer_modifier = 0;
				if field_headers[0].contains("~") {
					line_pointer = stat_name.len();
					line_pointer_modifier = 2;
				} else {
					line_pointer = field_headers[0].len();
				}
				if section_key.starts_with("SQL ordered by") && (sql_id.is_match(line_fields.last().unwrap()) || old_hash_value.is_match(line_fields.last().unwrap())) {
					stat_name = line_fields.last().unwrap().to_string();
					found_sqlid = true;
					line_pointer = 0;
				}
				if (section_key.starts_with("SQL ordered by") && found_sqlid) || (!section_key.starts_with("SQL ordered by")) {
					sql_txt = format!("{} {} values ('{}'", sql_txt, d.unwrap().target_table, stat_name.trim());
					for field in &field_headers {
						if (!found_sqlid && field_no > 0) || (found_sqlid && field_no >= 0) {
							let field_data: &str = line.substring(line_pointer+1, line_pointer+field.len()+2);
							line_pointer += field_data.len() + line_pointer_modifier;
							for fno in &d.unwrap().field_no {
								if field_no == *fno && (d.unwrap().field_re.is_match(&field_data.trim()) || field_data.trim() == "") {
									if field_data.trim().len() > 0 && !field_data.contains("N/A") {
										sql_txt = format!("{}, '{}'", sql_txt, field_data.trim().replace(",",""));
									} else {
										sql_txt = format!("{}, {}", sql_txt, "null");
									}
									found += 1;
								}
							}
						}
						field_no += 1;
					}
					if found == d.unwrap().field_no.len() {
						let sql = format!("{}, '{}', '{}', '{}')", sql_txt, begin_snap, end_snap, audit_id);
						print!("{}",sql);
						match conn.execute(&sql, &[]) {
							Ok(x) => println!(" OK "),
							Err(_e) => println!(" => IGNORRED {}", _e)
						}
					}
				}
				sql_txt = "insert into ".to_string();
			}
		}
    }
    conn.commit().unwrap();
}

fn main() {
    let args = env::args().collect::<Vec<String>>();
    println!("Read file {}\n", args[1]);

    let m_def = read_definition(&args[1]);
    let mut m_def_r: HashMap<String, DefinitionsR> = HashMap::new();

    let audit_id = &args[3];

    for (k, v) in &m_def {
		m_def_r.insert(k.to_string(), DefinitionsR{section_start: v.section_start.clone(),
						section_end:   v.section_end.clone(),
						field_re:      Regex::new(&v.field_re).unwrap(),
						field_no:	v.field_no.clone(),
						target_table:  v.target_table.clone()});
    }
    //gather_stats(&args[2], m_def_r);
    for file in fs::read_dir(&args[2]).unwrap() {
		let fname = &file.unwrap().path().display().to_string();
		if (fname.contains("awr") || fname.contains("sp_")) && fname.ends_with(".txt") {
			println!("{}", fname);
			gather_stats(fname, m_def_r.clone(), audit_id);
		}
    }

}
