use std::clone;
use std::collections::HashSet;
use std::env;
use std::env::args;
use std::fs;
use std::str;
use std::result;
use colored::Colorize;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::collections::{BTreeMap, HashMap};
use std::char;
use std::io::{self, Write};
use rayon::prelude::*;
use indicatif::{ProgressBar, ProgressStyle};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
use dashmap::DashMap;

use crate::analyze::plot_to_file;
use crate::idleevents::is_idle;
use crate::Args;

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct LoadProfile {
	pub stat_name: String,
	pub per_second: f64,
	per_transaction: f64,
	//pub begin_snap_time: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct RedoLog {
	pub stat_name: String,
	pub per_hour: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct DBInstance {
	//db_name: String,
	pub db_id: u64,
	//instance_name: String, 
	pub instance_num: u8,
	pub startup_time: String,
	pub release: String,
	pub rac: String,
	pub platform: String,
	pub cpus: u16,
	pub cores: u16,
	pub sockets: u8,
	pub memory: u16,
	pub db_block_size: u16,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct WaitClasses {
	wait_class: String,
	waits: u64,
	total_wait_time_s: f64,
	avg_wait_ms: f64,
	db_time_pct: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct HostCPU {
	pub cpus: u32,
	cores: u32,
	sockets: u8,
	load_avg_begin: f64,
	load_avg_end: f64,
	pub pct_user: f64,
	pct_system: f64,
	pct_wio: f64,
	pub pct_idle: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct TimeModelStats {
	pub stat_name: String,
	pub time_s: f64,
	pub pct_dbtime: f64,
	//begin_snap_time: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct WaitEvents {
	pub event: String,
	pub waits: u64,
	pub total_wait_time_s: f64,
	pub avg_wait: f64,
	pub pct_dbtime: f64,
	//begin_snap_time: String,
	pub waitevent_histogram_ms: BTreeMap<String,f32>,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct SQLElapsedTime {
	pub sql_id: String,
	pub elapsed_time_s: f64,
	pub executions: u64,
	pub elpased_time_exec_s: f64,
	pub pct_total: f64,
	pub pct_cpu: f64, 
	pub pct_io: f64,
	pub sql_module: String,
	pub sql_type: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct SQLCPUTime {
	pub sql_id: String,
	pub cpu_time_s: f64,
	pub executions: u64,
	pub cpu_time_exec_s: f64,
	pub pct_total: f64,
	pub pct_cpu: f64, 
	pub pct_io: f64,
	sql_module: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct SQLIOTime {
	pub sql_id: String,
	pub io_time_s: f64,
	pub executions: u64,
	pub io_time_exec_s: f64,
	pub pct_total: f64,
	pub pct_cpu: f64, 
	pub pct_io: f64,
	sql_module: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct SQLGets {
	pub sql_id: String,
	pub buffer_gets: f64,
	pub executions: u64,
	pub gets_per_exec: f64,
	pub pct_total: f64,
	pub pct_cpu: f64, 
	pub pct_io: f64,
	sql_module: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct SQLReads {
	pub sql_id: String,
	pub physical_reads: f64,
	pub executions: u64,
	pub reads_per_exec: f64,
	pub pct_total: f64,
	pub cpu_time_pct: f64, //in Statspack it is CPU Time - in AWR it is PCT CPU
	pub pct_io: f64, //doesn't exists in statspack
	sql_module: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct SnapInfo {
	pub begin_snap_id: u64,
	pub end_snap_id: u64,
	pub begin_snap_time: String,
	pub end_snap_time: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct KeyInstanceStats {
	pub statname: String,
	pub total: u64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct IOStats {
	pub reads_data: f64,	// in MB
	pub reads_req_s: f64,
	pub reads_data_s: f64,	// in MB
	pub writes_data: f64,	// in MB
	pub writes_req_s: f64,
	pub writes_data_s: f64,	// in MB
	pub waits_count: u64,
	pub avg_time: Option<f64>,		// in ms
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct DictionaryCache {
	pub statname: String, 
	pub get_requests: u64,
	pub final_usage: u64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct LibraryCache {
	pub statname: String, 
	pub get_requests: u64,
	pub get_pct_miss: f64,
	pub pin_requests: u64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct LatchActivity {
	pub statname: String, 
	pub get_requests: u64,
	pub get_pct_miss: f64,
	pub wait_time: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct SegmentStats {
	pub obj: u64,
	pub objd: u64,
	pub object_name: String,
	pub object_type: String,
	pub stat_name: String, 
	pub stat_vlalue: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct TopSQLWithTopEvents {
	pub sql_id: String,
	pub plan_hash_value: u64,
	pub executions: u64,
	pub pct_activity: f64,
	pub event_name: String,
	pub pct_event: f64,
	pub top_row_source: String,
	pub pct_row_source: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct AWR {
	pub file_name: String,
	pub snap_info: SnapInfo,
	status: String,
	pub load_profile: Vec<LoadProfile>,
	pub redo_log: RedoLog,
	wait_classes: Vec<WaitClasses>,
	pub host_cpu: HostCPU,
	pub time_model_stats: Vec<TimeModelStats>,
	pub foreground_wait_events: Vec<WaitEvents>,
	pub background_wait_events: Vec<WaitEvents>,
	pub sql_elapsed_time: Vec<SQLElapsedTime>,
	pub sql_cpu_time: HashMap<String, SQLCPUTime>,
	pub sql_io_time: HashMap<String, SQLIOTime>,
	pub sql_gets: HashMap<String, SQLGets>,
	pub sql_reads: HashMap<String, SQLReads>,
	pub top_sql_with_top_events: HashMap<String, TopSQLWithTopEvents>,
	pub key_instance_stats: Vec<KeyInstanceStats>,
	pub dictionary_cache: Vec<DictionaryCache>,
	pub io_stats_byfunc: HashMap<String,IOStats>,
	pub library_cache: Vec<LibraryCache>,
	pub latch_activity: Vec<LatchActivity>,
	pub segment_stats: HashMap<String, Vec<SegmentStats>>,
} 

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AWRSCollection {
    pub db_instance_information: DBInstance,
    pub awrs: Vec<AWR>,
	pub sql_text: HashMap<String, String>,

}

#[derive(Debug)]
struct SectionIdx {
	begin: usize,
	end: usize,
}

fn find_section_boundries(awr_doc: Vec<&str>, section_start: &str, section_end: &str, fname: &str, cinf: Option<bool> ) -> SectionIdx {
	let cinf = cinf.unwrap_or(false); // true - Ok to continue if Section not found, fasle (default) - do not continue
	let mut awr_iter: std::vec::IntoIter<&str> = awr_doc.into_iter();
	let section_start_trim = &section_start[1..section_start.len()-1];
	let section_end_trim: &str = &section_end[1..section_end.len()-1];
	let section_start = awr_iter.position(|x| x.starts_with(section_start) || x.starts_with(section_start_trim));
	match section_start {
        Some(start) => {
            // Find end position relative to start
            let section_end = awr_iter.position(|x: &str| x.starts_with(section_end) || x.starts_with(section_end_trim));

            match section_end {
                Some(rel_end) => {
                    let end = start + rel_end;  // Adding relative position to start
                    SectionIdx { begin: start, end }
                }
                None => {
                    eprintln!("\n{}: {} End section '{}' not found after start '{}'", "Error".bright_red(),fname.bright_magenta(), section_end_trim, section_start_trim);
                    eprintln!("Debug: {} Start idx: {:?}, End idx: {:?}", fname, start, section_end);
                    panic!("JAS-MIN is quitting");
                }
            }
        }
        None => {
            if !cinf {
				eprintln!("\n{}: {} Section '{}' not found","Error".bright_red(), fname.bright_magenta(), section_start_trim);
				panic!("JAS-MIN is quitting");
			} else {
				eprintln!("\n{}: {} Section '{}' not found but JAS-MIN will continue","Warning".bright_magenta(), fname.bright_magenta(), section_start_trim);
				SectionIdx {begin: 0, end: 0}
			}
		}
    }
}

fn sql_text(table: ElementRef) -> HashMap<String, String> {
	let mut sqls: HashMap<String, String> = HashMap::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() > 1 {
			let sql_id: Vec<&str> =  columns[0].text().collect::<Vec<_>>();
			let sql_id = sql_id[0].trim().to_string();

			let sql_text: Vec<&str> =  columns[1].text().collect::<Vec<_>>();
			let sql_text = sql_text[0].trim().to_string();

			sqls.entry(sql_id).or_insert(sql_text);
		}
	}
	sqls
}

fn top_sql_with_top_events(table: ElementRef) -> HashMap<String, TopSQLWithTopEvents> {
	let mut sqls: HashMap<String, TopSQLWithTopEvents> = HashMap::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() > 8 {
			let sql_id: Vec<&str> =  columns[0].text().collect::<Vec<_>>();
			let sql_id = sql_id[0].trim().to_string();

			let plan_hash_value: Vec<&str> =  columns[1].text().collect::<Vec<_>>();
			let plan_hash_value = u64::from_str(&plan_hash_value[0].trim().replace(",","")).unwrap_or(0);

			let executions: Vec<&str> =  columns[2].text().collect::<Vec<_>>();
			let executions = u64::from_str(&executions[0].trim().replace(",","")).unwrap_or(0);

			let pct_activity: Vec<&str> =  columns[3].text().collect::<Vec<_>>();
			let pct_activity = f64::from_str(&pct_activity[0].trim().replace(",","")).unwrap_or(0.0);

			let event_name: Vec<&str> =  columns[4].text().collect::<Vec<_>>();
			let event_name = event_name[0].trim().to_string();

			let pct_event: Vec<&str> =  columns[5].text().collect::<Vec<_>>();
			let pct_event = f64::from_str(&pct_event[0].trim().replace(",","")).unwrap_or(0.0);


			let top_row_source: Vec<&str> =  columns[6].text().collect::<Vec<_>>();
			let top_row_source = top_row_source[0].trim().to_string();

			let pct_row_source: Vec<&str> =  columns[7].text().collect::<Vec<_>>();
			let pct_row_source = f64::from_str(&pct_row_source[0].trim().replace(",","")).unwrap_or(0.0);

			sqls.entry(sql_id.clone()).or_insert(TopSQLWithTopEvents { sql_id: sql_id, 
																			plan_hash_value: plan_hash_value, 
																			executions: executions, 
																			pct_activity: pct_activity, 
																			event_name: event_name, 
																			pct_event: pct_event, 
																			top_row_source: top_row_source, 
																			pct_row_source: pct_row_source });
		}
	}
	sqls
}

fn segment_stats(table: ElementRef, stat_name: &str, args: &Args) -> Vec<SegmentStats> {
	let mut segment_stats: Vec<SegmentStats> = Vec::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() >= 7 {

			let mut version_modificator = 0; 

			if columns.len() == 7 { //this is for older AWR format (like 11g)
				version_modificator=1;
			}

			let mut segment_name = "#".to_string();
			if args.security_level > 0 {
				let sname = columns[2].text().collect::<Vec<_>>();
			    segment_name = sname[0].trim().to_string();
			}
			
			let segment_type =  columns[4].text().collect::<Vec<_>>();
			let segment_type = segment_type[0].trim().to_string();

			let mut obj = 0;
			let mut objd = 0;
			if version_modificator == 0 {
				let vobj = columns[5].text().collect::<Vec<_>>();
				obj = u64::from_str(&vobj[0].trim().replace(",","")).unwrap_or(0);

				let vobjd = columns[6].text().collect::<Vec<_>>();
				objd = u64::from_str(&vobjd[0].trim().replace(",","")).unwrap_or(0);

			}
			
			let stat_value = columns[7-version_modificator].text().collect::<Vec<_>>();
			let stat_value = f64::from_str(&stat_value[0].trim().replace(",","")).unwrap_or(0.0);

			segment_stats.push(SegmentStats {obj: obj, objd: objd, object_name: segment_name, object_type: segment_type, stat_name: stat_name.to_string(), stat_vlalue: stat_value});
		}
	}

	segment_stats
}

fn dictionary_cache_stats(table: ElementRef) -> Vec<DictionaryCache> {
	let mut dictionary_cache_stats: Vec<DictionaryCache> = Vec::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() >= 7 {
			let stat_name = columns[0].text().collect::<Vec<_>>();
			let stat_name = stat_name[0].trim();

			let get_req = columns[1].text().collect::<Vec<_>>();
			let get_req = u64::from_str(&get_req[0].trim().replace(",","")).unwrap_or(0);

			let final_usage = columns[6].text().collect::<Vec<_>>();
			let final_usage = u64::from_str(&final_usage[0].trim().replace(",","")).unwrap_or(0);

			dictionary_cache_stats.push(DictionaryCache {statname: stat_name.to_string(), get_requests: get_req, final_usage: final_usage});
		}
	}

	dictionary_cache_stats
}

fn dictionary_cache_stats_txt(dictionary_cache_section: Vec<&str>) -> Vec<DictionaryCache> {
	let mut dictionary_cache_stats_txt: Vec<DictionaryCache> = Vec::new();
	for line in dictionary_cache_section {
		if line.len() >= 77 {
			let statname = line[0..25].to_string().trim().to_string();
			let get_requests = u64::from_str(&line[26..38].trim().replace(",",""));
			let final_usage = u64::from_str(&line[69..79].trim().replace(",",""));
			if get_requests.is_ok() && final_usage.is_ok() {
				dictionary_cache_stats_txt.push(DictionaryCache{statname: statname.to_string(), get_requests: get_requests.unwrap() , final_usage: final_usage.unwrap()});
			}
		}
		
	} 
	dictionary_cache_stats_txt
}

fn library_cache_stats(table: ElementRef) -> Vec<LibraryCache> {
	let mut library_cache_stats: Vec<LibraryCache> = Vec::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() >= 7 {
			let stat_name = columns[0].text().collect::<Vec<_>>();
			let stat_name = stat_name[0].trim();

			let get_req = columns[1].text().collect::<Vec<_>>();
			let get_req = u64::from_str(&get_req[0].trim().replace(",","")).unwrap_or(0);

			let pin_req = columns[3].text().collect::<Vec<_>>();
			let pin_req = u64::from_str(&pin_req[0].trim().replace(",","")).unwrap_or(0);

			let get_req_pct_miss = columns[2].text().collect::<Vec<_>>();
			let get_req_pct_miss = f64::from_str(&get_req_pct_miss[0].trim().replace(",","")).unwrap_or(0.0);

			library_cache_stats.push(LibraryCache {statname: stat_name.to_string(), get_requests: get_req, get_pct_miss: get_req_pct_miss, pin_requests: pin_req});
		}
	}

	library_cache_stats
}


fn library_cache_stats_txt(library_cache_stats_section: Vec<&str>) -> Vec<LibraryCache> {
	let mut library_cache_stats_txt: Vec<LibraryCache> = Vec::new();
	for line in library_cache_stats_section {
		if line.len() >= 79 && !line.starts_with(" "){
			let statname = line[0..45].to_string().trim().to_string();
			let get_requests = u64::from_str(&line[45..58].trim().replace(",",""));
			let pct_miss = f64::from_str(&line[59..65].trim().replace(",",""));
			let pin_req = u64::from_str(&line[66..80].trim().replace(",",""));
			if get_requests.is_ok() && pct_miss.is_ok() && pin_req.is_ok() {
				library_cache_stats_txt.push(LibraryCache{statname: statname.to_string(), get_requests: get_requests.unwrap() , get_pct_miss: pct_miss.unwrap(), pin_requests: pin_req.unwrap()});
			}
		}
		
	} 
	library_cache_stats_txt
}

fn latch_activity_stats(table: ElementRef) -> Vec<LatchActivity> {
	let mut latch_activity_stats: Vec<LatchActivity> = Vec::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() >= 7 {
			let stat_name = columns[0].text().collect::<Vec<_>>();
			let stat_name = stat_name[0].trim();

			let get_req = columns[1].text().collect::<Vec<_>>();
			let get_req = u64::from_str(&get_req[0].trim().replace(",","")).unwrap_or(0);

			let wait_time_s = columns[4].text().collect::<Vec<_>>();
			let wait_time_s = f64::from_str(&wait_time_s[0].trim().replace(",","")).unwrap_or(0.0);

			let get_req_pct_miss = columns[2].text().collect::<Vec<_>>();
			let get_req_pct_miss = f64::from_str(&get_req_pct_miss[0].trim().replace(",","")).unwrap_or(0.0);

			latch_activity_stats.push(LatchActivity {statname: stat_name.to_string(), get_requests: get_req, get_pct_miss: get_req_pct_miss, wait_time: wait_time_s});
		}
	}

	latch_activity_stats
}

fn latch_activity_stats_txt(latch_activity_stats_section: Vec<&str>) -> Vec<LatchActivity> {
	let mut latch_activity_stats_txt: Vec<LatchActivity> = Vec::new();
	for line in latch_activity_stats_section {
		if line.len() >= 72 && !line.starts_with(" "){
			let statname = line[0..24].to_string().trim().to_string();
			let get_req = u64::from_str(&line[25..39].trim().replace(",",""));
			let pct_miss = f64::from_str(&line[40..46].trim().replace(",",""));
			let wait_time_s = f64::from_str(&line[54..60].trim().replace(",",""));
			if get_req.is_ok() && pct_miss.is_ok() && wait_time_s.is_ok() {
				latch_activity_stats_txt.push(LatchActivity {statname: statname.to_string(), get_requests: get_req.unwrap(), get_pct_miss: pct_miss.unwrap(), wait_time: wait_time_s.unwrap()});
			}
		}
		
	} 
	latch_activity_stats_txt
}

fn sql_elapsed_time(table: ElementRef) -> Vec<SQLElapsedTime> {
	let mut sql_elapsed_time: Vec<SQLElapsedTime> = Vec::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() >= 9 {
			let sql_id = columns[6].text().collect::<Vec<_>>();
			let sql_id = sql_id[0].trim().to_string();
			
			let elapsed_time_s = columns[0].text().collect::<Vec<_>>();
			let elapsed_time_s = f64::from_str(&elapsed_time_s[0].trim().replace(",","")).unwrap_or(0.0);

			let executions = columns[1].text().collect::<Vec<_>>();
			let executions = u64::from_str(&executions[0].trim().replace(",","")).unwrap_or(0);
			
			let elpased_time_exec_s = columns[2].text().collect::<Vec<_>>();
			let elpased_time_exec_s = f64::from_str(&elpased_time_exec_s[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_total = columns[3].text().collect::<Vec<_>>();
			let pct_total = f64::from_str(&pct_total[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_cpu = columns[4].text().collect::<Vec<_>>();
			let pct_cpu = f64::from_str(&pct_cpu[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_io = columns[5].text().collect::<Vec<_>>();
			let pct_io = f64::from_str(&pct_io[0].trim().replace(",","")).unwrap_or(0.0);

			let sql_module = columns[7].text().collect::<Vec<_>>();
			let sql_module = sql_module[0].trim().to_string();

			let sql_txt =  columns.last().unwrap().text().collect::<Vec<&str>>();
			let sql_txt = sql_txt[0].trim().to_string();
			let mut sql_type = "SELECT";
			if sql_txt.to_uppercase().starts_with("UPDATE") {
				sql_type = "UPDATE";
			} else if sql_txt.to_uppercase().starts_with("DELETE") {
				sql_type = "DELETE";
			} else if sql_txt.to_uppercase().starts_with("INSERT") {
				sql_type = "INSERT";
			} else if sql_txt.to_uppercase().starts_with("MERGE") {
				sql_type = "MERGE";
			} else if sql_txt.to_uppercase().starts_with("BEGIN") || sql_txt.to_uppercase().starts_with("DECLARE") || sql_txt.to_uppercase().starts_with("CALL") {
				sql_type = "PL/SQL";
			}

			sql_elapsed_time.push(SQLElapsedTime { sql_id, elapsed_time_s, executions, elpased_time_exec_s, pct_total, pct_cpu, pct_io, sql_module , sql_type: sql_type.to_string()})
		
		}
	}

	sql_elapsed_time
}

fn sql_ela_time_txt(sql_ela_section: Vec<&str>) -> Vec<SQLElapsedTime> {
	let mut sql_ela_time: Vec<SQLElapsedTime> = Vec::new();
	let mut sql_id_hash = String::new();
	for line in sql_ela_section {
		let fields = line.split_whitespace().collect::<Vec<&str>>();
		if fields.len()>=6 {
			let ela_time = f64::from_str(&fields[0].trim().replace(",",""));
			let executions = u64::from_str(&fields[1].trim().replace(",",""));
			let ela_exec = f64::from_str(&fields[2].trim().replace(",",""));
			let pct_total = f64::from_str(&fields[3].trim().replace(",",""));
			let cpu_time = f64::from_str(&fields[4].trim().replace(",",""));
			let ph_reads = f64::from_str(&fields[5].trim().replace(",",""));
			if fields.len() == 7 && ela_time.is_ok() && executions.is_ok() && ela_exec.is_ok() && pct_total.is_ok() && cpu_time.is_ok() && ph_reads.is_ok() {
				let ela_time = ela_time.unwrap();
				let executions = executions.unwrap();
				let ela_exec = ela_exec.unwrap();
				let pct_total = pct_total.unwrap();
				let cpu_time = cpu_time.unwrap();
				let ph_reads = ph_reads.unwrap();
				sql_id_hash = fields[6].trim().to_string();
				sql_ela_time.push(SQLElapsedTime{sql_id: sql_id_hash.clone(), 
												elapsed_time_s: ela_time, 
												executions: executions, 
												elpased_time_exec_s: ela_exec, 
												pct_total: pct_total, 
												pct_cpu: -1.0, pct_io: -1.0, sql_module: "?".to_string(), sql_type: String::new()});
			}
		}
		if line.starts_with("Module:") && !sql_id_hash.is_empty() {
			let fields = line.split(":").collect::<Vec<&str>>();
			let module = fields[1].trim();
			let mut sql = sql_ela_time.last_mut().unwrap();
			sql.sql_module = module.trim().to_string();
		}
	}
	sql_ela_time
}

fn sql_cpu_time(table: ElementRef) -> HashMap<String,SQLCPUTime> {
	let mut sql_cpu_time: HashMap<String,SQLCPUTime> = HashMap::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() >= 10 {
			let sql_id = columns[7].text().collect::<Vec<_>>();
			let sql_id = sql_id[0].trim().to_string();
			
			let cpu_time_s = columns[0].text().collect::<Vec<_>>();
			let cpu_time_s = f64::from_str(&cpu_time_s[0].trim().replace(",","")).unwrap_or(0.0);

			let executions = columns[1].text().collect::<Vec<_>>();
			let executions = u64::from_str(&executions[0].trim().replace(",","")).unwrap_or(0);
			
			let cpu_time_exec_s = columns[2].text().collect::<Vec<_>>();
			let cpu_time_exec_s = f64::from_str(&cpu_time_exec_s[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_total = columns[3].text().collect::<Vec<_>>();
			let pct_total = f64::from_str(&pct_total[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_cpu = columns[5].text().collect::<Vec<_>>();
			let pct_cpu = f64::from_str(&pct_cpu[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_io = columns[6].text().collect::<Vec<_>>();
			let pct_io = f64::from_str(&pct_io[0].trim().replace(",","")).unwrap_or(0.0);

			let sql_module = columns[8].text().collect::<Vec<_>>();
			let sql_module = sql_module[0].trim().to_string();

			sql_cpu_time.entry(sql_id.clone()).or_insert(SQLCPUTime { sql_id, cpu_time_s, executions, cpu_time_exec_s, pct_total, pct_cpu, pct_io, sql_module });
		}
	}

	sql_cpu_time
}

fn sql_cpu_time_txt(sql_cpu_section: Vec<&str>) -> HashMap<String, SQLCPUTime> {
	let mut sql_cpu_time: HashMap<String, SQLCPUTime> = HashMap::new();
	let mut sql_id_hash = String::new();
	for line in sql_cpu_section {
		let fields = line.split_whitespace().collect::<Vec<&str>>();
		if fields.len()>=6 {
			let cpu_time = f64::from_str(&fields[0].trim().replace(",",""));
			let executions = u64::from_str(&fields[1].trim().replace(",",""));
			let cpu_exec = f64::from_str(&fields[2].trim().replace(",",""));
			let pct_total = f64::from_str(&fields[3].trim().replace(",",""));
			let ela_time = f64::from_str(&fields[4].trim().replace(",",""));
			let buf_gets = f64::from_str(&fields[5].trim().replace(",",""));
			if fields.len() == 7 && cpu_time.is_ok() && ela_time.is_ok() && executions.is_ok() && cpu_exec.is_ok() && pct_total.is_ok() && buf_gets.is_ok() {
				let cpu_time = cpu_time.unwrap();
				let executions = executions.unwrap();
				let cpu_exec = cpu_exec.unwrap();
				let pct_total = pct_total.unwrap();
				let ela_time = ela_time.unwrap();
				let buf_gets = buf_gets.unwrap();
				sql_id_hash = fields[6].trim().to_string();
				sql_cpu_time.entry(sql_id_hash.clone()).or_insert(SQLCPUTime{sql_id: sql_id_hash.clone(), 
												cpu_time_s: cpu_time, 
												executions: executions, 
												cpu_time_exec_s: cpu_exec, 
												pct_total: pct_total, 
												pct_cpu: -1.0, pct_io: -1.0, sql_module: "?".to_string()});
			}
		}
		if line.starts_with("Module:") && !sql_id_hash.is_empty() {
			let fields = line.split(":").collect::<Vec<&str>>();
			let module = fields[1].trim();
			let mut sql = sql_cpu_time.get_mut(&sql_id_hash).unwrap();
			sql.sql_module = module.to_string();
		}
	}
	sql_cpu_time
}

fn sql_io_time(table: ElementRef) -> HashMap<String,SQLIOTime> {
	let mut sql_io_time: HashMap<String,SQLIOTime> = HashMap::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() >= 10 {
			let sql_id = columns[7].text().collect::<Vec<_>>();
			let sql_id = sql_id[0].trim().to_string();
			
			let io_time_s = columns[0].text().collect::<Vec<_>>();
			let io_time_s = f64::from_str(&io_time_s[0].trim().replace(",","")).unwrap_or(0.0);

			let executions = columns[1].text().collect::<Vec<_>>();
			let executions = u64::from_str(&executions[0].trim().replace(",","")).unwrap_or(0);
			
			let io_time_exec_s = columns[2].text().collect::<Vec<_>>();
			let io_time_exec_s = f64::from_str(&io_time_exec_s[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_total = columns[3].text().collect::<Vec<_>>();
			let pct_total = f64::from_str(&pct_total[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_cpu = columns[5].text().collect::<Vec<_>>();
			let pct_cpu = f64::from_str(&pct_cpu[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_io = columns[6].text().collect::<Vec<_>>();
			let pct_io = f64::from_str(&pct_io[0].trim().replace(",","")).unwrap_or(0.0);

			let sql_module = columns[8].text().collect::<Vec<_>>();
			let sql_module = sql_module[0].trim().to_string();

			sql_io_time.entry(sql_id.clone()).or_insert(SQLIOTime { sql_id, io_time_s, executions, io_time_exec_s, pct_total, pct_cpu, pct_io, sql_module });
		}
	}

	sql_io_time
}

fn sql_gets(table: ElementRef) -> HashMap<String,SQLGets> {
	let mut sql_gets: HashMap<String,SQLGets> = HashMap::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() >= 10 {
			let sql_id = columns[7].text().collect::<Vec<_>>();
			let sql_id = sql_id[0].trim().to_string();
			
			let buffer_gets = columns[0].text().collect::<Vec<_>>();
			let buffer_gets = f64::from_str(&buffer_gets[0].trim().replace(",","")).unwrap_or(0.0);

			let executions = columns[1].text().collect::<Vec<_>>();
			let executions = u64::from_str(&executions[0].trim().replace(",","")).unwrap_or(0);
			
			let gets_per_exec = columns[2].text().collect::<Vec<_>>();
			let gets_per_exec = f64::from_str(&gets_per_exec[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_total = columns[3].text().collect::<Vec<_>>();
			let pct_total = f64::from_str(&pct_total[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_cpu = columns[5].text().collect::<Vec<_>>();
			let pct_cpu = f64::from_str(&pct_cpu[0].trim().replace(",",".")).unwrap_or(0.0);

			let pct_io = columns[6].text().collect::<Vec<_>>();
			let pct_io = f64::from_str(&pct_io[0].trim().replace(",",".")).unwrap_or(0.0);
			

			let sql_module = columns[8].text().collect::<Vec<_>>();
			let sql_module = sql_module[0].trim().to_string();

			sql_gets.entry(sql_id.clone()).or_insert(SQLGets { sql_id, buffer_gets, executions, gets_per_exec, pct_total, pct_cpu, pct_io, sql_module });
		}
	}

	sql_gets
}

fn sql_gets_txt(sql_gets_section: Vec<&str>) -> HashMap<String, SQLGets> {
	let mut sql_gets: HashMap<String, SQLGets> = HashMap::new();
	let mut sql_id_hash: String = String::new();
	for line in sql_gets_section {
		let fields = line.split_whitespace().collect::<Vec<&str>>();
		if fields.len()>=6 {
			let buffer_gets = f64::from_str(&fields[0].trim().replace(",",""));
			let executions = u64::from_str(&fields[1].trim().replace(",",""));
			let gets_exec = f64::from_str(&fields[2].trim().replace(",",""));
			let pct_total = f64::from_str(&fields[3].trim().replace(",",""));
			
			if fields.len() == 7 && buffer_gets.is_ok() && gets_exec.is_ok() && executions.is_ok() && pct_total.is_ok() {
				let buffer_gets = buffer_gets.unwrap();
				let executions = executions.unwrap();
				let pct_total = pct_total.unwrap();
				let gets_exec = gets_exec.unwrap();
				sql_id_hash = fields[6].trim().to_string();
				sql_gets.entry(sql_id_hash.clone()).or_insert(SQLGets{sql_id: sql_id_hash.clone(), 
												buffer_gets: buffer_gets, 
												executions: executions, 
												gets_per_exec: gets_exec,
												pct_total: pct_total, 
												pct_cpu: -1.0, pct_io: -1.0, sql_module: "?".to_string()});
			}
		}
		if line.starts_with("Module:") && !sql_id_hash.is_empty() {
			let fields = line.split(":").collect::<Vec<&str>>();
			let module = fields[1].trim();
			let mut sql = sql_gets.get_mut(&sql_id_hash).unwrap();
			sql.sql_module = module.to_string();
		}
	}
	sql_gets
}


fn sql_reads(table: ElementRef) -> HashMap<String,SQLReads> {
	let mut sql_reads: HashMap<String,SQLReads> = HashMap::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() >= 10 {
			let sql_id = columns[7].text().collect::<Vec<_>>();
			let sql_id = sql_id[0].trim().to_string();
			
			let physical_reads = columns[0].text().collect::<Vec<_>>();
			let physical_reads = f64::from_str(&physical_reads[0].trim().replace(",","")).unwrap_or(0.0);

			let executions = columns[1].text().collect::<Vec<_>>();
			let executions = u64::from_str(&executions[0].trim().replace(",","")).unwrap_or(0);
			
			let reads_per_exec = columns[2].text().collect::<Vec<_>>();
			let reads_per_exec = f64::from_str(&reads_per_exec[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_total = columns[3].text().collect::<Vec<_>>();
			let pct_total = f64::from_str(&pct_total[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_cpu = columns[5].text().collect::<Vec<_>>();
			let pct_cpu = f64::from_str(&pct_cpu[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_io = columns[6].text().collect::<Vec<_>>();
			let pct_io = f64::from_str(&pct_io[0].trim().replace(",","")).unwrap_or(0.0);

			let sql_module = columns[8].text().collect::<Vec<_>>();
			let sql_module = sql_module[0].trim().to_string();

			sql_reads.entry(sql_id.clone()).or_insert(SQLReads { sql_id, physical_reads, executions, reads_per_exec, pct_total, cpu_time_pct: pct_cpu, pct_io, sql_module });
		}
	}

	sql_reads
}


fn sql_reads_txt(sql_gets_section: Vec<&str>) -> HashMap<String, SQLReads> {
	let mut sql_reads: HashMap<String, SQLReads> = HashMap::new();
	let mut sql_id_hash: String = String::new();
	for line in sql_gets_section {
		let fields = line.split_whitespace().collect::<Vec<&str>>();
		if fields.len()>=6 {
			let physical_reads = f64::from_str(&fields[0].trim().replace(",",""));
			let executions = u64::from_str(&fields[1].trim().replace(",",""));
			let reads_exec = f64::from_str(&fields[2].trim().replace(",",""));
			let pct_total = f64::from_str(&fields[3].trim().replace(",",""));
			let cpu_time = f64::from_str(&fields[4].trim().replace(",",""));
			
			if fields.len() == 7 && physical_reads.is_ok() && reads_exec.is_ok() && executions.is_ok() && pct_total.is_ok() && cpu_time.is_ok() {
				let physical_reads = physical_reads.unwrap();
				let executions = executions.unwrap();
				let pct_total = pct_total.unwrap();
				let reads_exec = reads_exec.unwrap();
				let cpu_time = cpu_time.unwrap();
				sql_id_hash = fields[6].trim().to_string();
				sql_reads.entry(sql_id_hash.clone()).or_insert(SQLReads{sql_id: sql_id_hash.clone(), 
												physical_reads: physical_reads, 
												executions: executions, 
												reads_per_exec: reads_exec,
												pct_total: pct_total,
												cpu_time_pct:cpu_time, pct_io: -1.0, sql_module: "?".to_string()});
			}
		}
		if line.starts_with("Module:") && !sql_id_hash.is_empty() {
			let fields = line.split(":").collect::<Vec<&str>>();
			let module = fields[1].trim();
			let mut sql = sql_reads.get_mut(&sql_id_hash).unwrap();
			sql.sql_module = module.to_string();
		}
	}
	sql_reads
}

fn waitevent_histogram_ms(table: ElementRef) -> HashMap<String, BTreeMap<String, f32>> {
	let mut histogram: HashMap<String, BTreeMap<String, f32>> = HashMap::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();
	let header_selector = Selector::parse("th").unwrap();
	let mut proper_table = false;
	let mut buckets: Vec<String> = Vec::new();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		let headers = row.select(&header_selector).collect::<Vec<_>>();
		if headers.len() == 10 && !proper_table {
			let header_check = headers[3].text().collect::<Vec<_>>();
			let header_check = header_check[0].trim();
			if header_check == "<1ms" || header_check == "<2ms" {
				proper_table = true;
			}
			for i in 2..10 {
				let h = headers[i].text().collect::<Vec<_>>();
				let h = h[0].trim();
				buckets.push(h.to_string());
			}
		} else if columns.len() == 10 && proper_table {
			let event = columns[0].text().collect::<Vec<_>>();
			let event = event[0].trim();

			for i in 2..10 {
				let pct_time = columns[i].text().collect::<Vec<_>>();
				let pct_time = f32::from_str(&pct_time[0].trim().replace(",","")).unwrap_or(0.0);
				histogram.entry(event.to_string()).or_insert(BTreeMap::new());
				let mut v = histogram.get_mut(event).unwrap();
				let bucket = format!("{}: {}", i-2, buckets[i-2].clone());
				v.entry(bucket).or_insert(pct_time);
			}
		} 
	}

	histogram
}

fn waitevent_histogram_ms_txt(events_histogram_section: Vec<&str>, event_names: HashMap<String, String>) -> HashMap<String, BTreeMap<String, f32>> {
	let mut histogram: HashMap<String, BTreeMap<String, f32>> = HashMap::new();
	for line in events_histogram_section {
		if line.len() > 26 {
			let mut hist_values: BTreeMap<String, f32> = BTreeMap::from([
				("1: <1ms".to_string(), 0.0),
				("2: <2ms".to_string(), 0.0),
				("3: <4ms".to_string(), 0.0),
				("4: <8ms".to_string(), 0.0),
				("5: <16ms".to_string(), 0.0),
				("6: <32ms".to_string(), 0.0),
				("7: <=1s".to_string(), 0.0),
				("8: >1s".to_string(), 0.0),
			]);

			let mut event_name = line[0..26].to_string().trim().to_string();
			if event_names.contains_key(&event_name) {
				event_name = event_names.get(&event_name).unwrap().clone();
				if line.len() >= 37 {
					let pct_val = f32::from_str(&line[33..38].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("1: <1ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 43 {
					let pct_val = f32::from_str(&line[39..44].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("2: <2ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 49 {
					let pct_val = f32::from_str(&line[45..50].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("3: <4ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 55 {
					let pct_val = f32::from_str(&line[51..56].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("4: <8ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 61 {
					let pct_val = f32::from_str(&line[57..62].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("5: <16ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 67 {
					let pct_val = f32::from_str(&line[63..68].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("6: <32ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 73 {
					let pct_val = f32::from_str(&line[69..74].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("7: <=1s").unwrap();
					*x = pct_val;
				}
				if line.len() >= 79 {
					let pct_val = f32::from_str(&line[75..80].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("8: >1s").unwrap();
					*x = pct_val;
				}
				histogram.insert(event_name.clone(), hist_values.clone());
			}
		}
	}
	histogram
}

fn wait_events(table: ElementRef) -> Vec<WaitEvents> {
	let mut wait_events: Vec<WaitEvents> = Vec::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() == 7 {
			let event = columns[0].text().collect::<Vec<_>>();
			let event = event[0].trim();

			let waits = columns[1].text().collect::<Vec<_>>();
			let waits = u64::from_str(&waits[0].trim().replace(",","")).unwrap_or(0);

			let total_wait_time_s = columns[3].text().collect::<Vec<_>>();
			let total_wait_time_s = f64::from_str(&total_wait_time_s[0].trim().replace(",","")).unwrap_or(0.0);

			let avg_wait = columns[4].text().collect::<Vec<_>>();
			let avg_wait = f64::from_str(&avg_wait[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_dbtime = columns[6].text().collect::<Vec<_>>();
			let pct_dbtime = f64::from_str(&pct_dbtime[0].trim().replace(",","")).unwrap_or(0.0);
			if !is_idle(&event) {
				wait_events.push(WaitEvents { event: event.to_string(), waits: waits, total_wait_time_s: total_wait_time_s, avg_wait: avg_wait, pct_dbtime: pct_dbtime, waitevent_histogram_ms: BTreeMap::new() })
			}
		}
	}

	wait_events	
}

fn wait_events_txt(events_section: Vec<&str>) -> Vec<WaitEvents> {
	let mut wait_events: Vec<WaitEvents> = Vec::new();
	
	for line in events_section {
		if line.len() >= 73 {
			//println!("{}", line);
			let statname = line[0..28].to_string().trim().to_string();
			let waits = u64::from_str(&line[29..41].trim().replace(",",""));

			if waits.is_ok() {
				let waits: u64 = waits.unwrap_or(0);
				let mut total_wait_time = f64::from_str(&line[46..57].trim().replace(",","")).unwrap_or(0.0);
				//if total_wait_time == 0.0 {
				//	total_wait_time = f64::from_str(&line[38..54].trim().replace(",","")).unwrap_or(0.0);
				//}
				let avg_wait = f64::from_str(&line[57..64].trim().replace(",","")).unwrap_or(0.0);
				let mut pct_dbtime = 0.0;
				if line.len() > 79 {
					//let mut pct_dbtime_end: usize = 80;
					//if line.len() < pct_dbtime_end {
					//	pct_dbtime_end = line.len();
					//}
					//pct_dbtime = f64::from_str(&line[73..pct_dbtime_end].trim().replace(",","")).unwrap();
					pct_dbtime = f64::from_str(&line[73..80].trim().replace(",","")).unwrap_or(0.0);
				}
				if !is_idle(&statname) {
					wait_events.push(WaitEvents { event: statname, waits: waits, total_wait_time_s: total_wait_time, avg_wait: avg_wait, pct_dbtime: pct_dbtime, waitevent_histogram_ms: BTreeMap::new()})
				}
			}
		}
	}
	wait_events
}	

fn time_model_stats(table: ElementRef) -> Vec<TimeModelStats> {
	let mut time_model_stats: Vec<TimeModelStats> = Vec::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() >= 3 {
			let stat_name = columns[0].text().collect::<Vec<_>>();
			let stat_name = stat_name[0].trim();

			let time_s = columns[1].text().collect::<Vec<_>>();
			let time_s = f64::from_str(&time_s[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_dbtime = columns[2].text().collect::<Vec<_>>();
			let pct_dbtime = f64::from_str(&pct_dbtime[0].trim().replace(",","")).unwrap_or(0.0);

			time_model_stats.push(TimeModelStats {stat_name: stat_name.to_string(), time_s: time_s, pct_dbtime: pct_dbtime});
		}
	}

	time_model_stats
}

fn time_model_stats_txt(time_model_section: Vec<&str>) -> Vec<TimeModelStats> {
	let mut time_model_stats: Vec<TimeModelStats> = Vec::new();
	for line in time_model_section {
		if line.len() >= 66 {
			let statname = line[0..35].to_string().trim().to_string();
			let time_s = f64::from_str(&line[35..56].trim().replace(",",""));
			let pct_dbtime = f64::from_str(&line[56..66].trim().replace(",",""));
			if time_s.is_ok() && pct_dbtime.is_ok() {
				time_model_stats.push(TimeModelStats{stat_name: statname.to_string(), time_s: time_s.unwrap(), pct_dbtime: pct_dbtime.unwrap()});
			}
		} 
		
	} 
	time_model_stats
}

fn host_cpu(table: ElementRef) -> HostCPU {
	let mut host_cpu: HostCPU = HostCPU::default();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
            let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
            if columns.len() == 9 {
				let cpus = columns[0].text().collect::<Vec<_>>();
				let cpus: u32 = u32::from_str(&cpus[0].trim().replace(",","")).unwrap_or(0);

				let cores = columns[1].text().collect::<Vec<_>>();
				let cores: u32 = u32::from_str(&cores[0].trim().replace(",","")).unwrap_or(0);

				let sockets = columns[2].text().collect::<Vec<_>>();
				let sockets: u8 = u8::from_str(&sockets[0].trim().replace(",","")).unwrap_or(0);

				let load_avg_begin =  columns[3].text().collect::<Vec<_>>();
				let load_avg_begin: f64 = f64::from_str(&load_avg_begin[0].trim().replace(",","")).unwrap_or(0.0);

				let load_avg_end = columns[4].text().collect::<Vec<_>>();
				let load_avg_end: f64 = f64::from_str(&load_avg_end[0].trim().replace(",","")).unwrap_or(0.0);

				let pct_user = columns[5].text().collect::<Vec<_>>();
				let pct_user: f64 = f64::from_str(&pct_user[0].trim().replace(",","")).unwrap_or(0.0);

				let pct_system = columns[6].text().collect::<Vec<_>>();
				let pct_system: f64 = f64::from_str(&pct_system[0].trim().replace(",","")).unwrap_or(0.0);

				let pct_wio = columns[7].text().collect::<Vec<_>>();
				let pct_wio: f64 = f64::from_str(&pct_wio[0].trim().replace(",","")).unwrap_or(0.0); 

				let pct_idle = columns[8].text().collect::<Vec<_>>();
				let pct_idle: f64 = f64::from_str(&pct_idle[0].trim().replace(",","")).unwrap_or(0.0); 

				host_cpu = HostCPU{cpus, cores, sockets, load_avg_begin, load_avg_end, pct_user, pct_system, pct_wio, pct_idle};
		} else if columns.len() == 6 {
			let load_avg_begin =  columns[0].text().collect::<Vec<_>>();
				let load_avg_begin: f64 = f64::from_str(&load_avg_begin[0].trim().replace(",","")).unwrap_or(0.0);

				let load_avg_end = columns[1].text().collect::<Vec<_>>();
				let load_avg_end: f64 = f64::from_str(&load_avg_end[0].trim().replace(",","")).unwrap_or(0.0);

				let pct_user = columns[2].text().collect::<Vec<_>>();
				let pct_user: f64 = f64::from_str(&pct_user[0].trim().replace(",","")).unwrap_or(0.0);

				let pct_system = columns[3].text().collect::<Vec<_>>();
				let pct_system: f64 = f64::from_str(&pct_system[0].trim().replace(",","")).unwrap_or(0.0);

				let pct_wio = columns[4].text().collect::<Vec<_>>();
				let pct_wio: f64 = f64::from_str(&pct_wio[0].trim().replace(",","")).unwrap_or(0.0); 

				let pct_idle = columns[5].text().collect::<Vec<_>>();
				let pct_idle: f64 = f64::from_str(&pct_idle[0].trim().replace(",","")).unwrap_or(0.0); 

				host_cpu = HostCPU{cpus: 0, cores: 0, sockets: 0, load_avg_begin, load_avg_end, pct_user, pct_system, pct_wio, pct_idle};
		}
	}

	host_cpu
}

fn host_cpu_txt(lines: Vec<&str>) -> HostCPU {
    let mut host_cpu = HostCPU::default();
    
    for line in lines.iter() {
        // Look for the line with CPUs, Cores, Sockets
        if line.contains("Host CPU") {
            // Extract the numbers from the line
            if let Some(captures) = regex::Regex::new(r"CPUs:\s*(\d+)\s*Cores:\s*(\d+)\s*Sockets:\s*(\d+)")
                                        .unwrap()
                                        .captures(line) 
            {
                host_cpu.cpus = captures.get(1).map_or(0, |m| m.as_str().parse::<u32>().unwrap_or(0));
                host_cpu.cores = captures.get(2).map_or(0, |m| m.as_str().parse::<u32>().unwrap_or(0));
                host_cpu.sockets = captures.get(3).map_or(0, |m| m.as_str().parse::<u8>().unwrap_or(0));
            }
        }

        // Look for the line with load averages and percentages
        if line.trim().starts_with(|c: char| c.is_digit(10)) {
            let columns: Vec<&str> = line.split_whitespace().collect();
            if columns.len() >= 6 {
                host_cpu.load_avg_begin = columns[0].parse::<f64>().unwrap_or(0.0);
                host_cpu.load_avg_end = columns[1].parse::<f64>().unwrap_or(0.0);
                host_cpu.pct_user = columns[2].parse::<f64>().unwrap_or(0.0);
                host_cpu.pct_system = columns[3].parse::<f64>().unwrap_or(0.0);
                host_cpu.pct_idle = columns[4].parse::<f64>().unwrap_or(0.0);
                host_cpu.pct_wio = columns[5].parse::<f64>().unwrap_or(0.0);
            }
        }
    }

    host_cpu
}

fn redo_log_switches(table: ElementRef) -> RedoLog {
	let mut redo_switches = RedoLog::default();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
            let columns: Vec<ElementRef> = row.select(&column_selector).collect::<Vec<_>>();
			if columns.len() == 3 {
				let stat_name = columns[0].text().collect::<Vec<_>>();
				let stat_name = stat_name[0].trim();
				if stat_name.starts_with("log switches (derived)") {
					let per_hour = columns[2].text().collect::<Vec<_>>();
					let per_hour = f64::from_str(&per_hour[0].trim().replace(",","")).unwrap_or(0.0);
					redo_switches.stat_name = stat_name.to_string();
					redo_switches.per_hour = per_hour;
				}	
			}
	}
	redo_switches
}

fn redo_log_switches_txt(line: &str) -> RedoLog {
    // Example: "log switches (derived)                            37     37.00"
    let mut redo_switches = RedoLog::default();
	let parts: Vec<&str> = line.split_whitespace().collect();

    // Assuming the first part is the stat name and the last part is the value
    redo_switches.stat_name = parts[0..3].join(" ");  // Joining the first 3 parts as the stat name
    redo_switches.per_hour = parts.last().unwrap().parse::<f64>().unwrap_or(0.0);  // Parsing the last part as the value
	redo_switches
}
 


fn instance_activity_stats(table: ElementRef) -> Vec<KeyInstanceStats> {
	let mut ias: Vec<KeyInstanceStats> = Vec::new();
	let row_selector = Selector::parse("tr").unwrap();
	let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() == 4 {
			let stat_name = columns[0].text().collect::<Vec<_>>();
			let stat_name = stat_name[0].trim();

			let total = columns[1].text().collect::<Vec<_>>();
			let total = u64::from_str(&total[0].trim().replace(",","")).unwrap_or(0);

			ias.push(KeyInstanceStats { statname: stat_name.to_string(), total: total });

		}
	}
	ias
}

fn instance_activity_stats_txt(inst_stats_section: Vec<&str>) -> Vec<KeyInstanceStats> {
	let mut ias: Vec<KeyInstanceStats> = Vec::new();
	for line in inst_stats_section {
		if line.len() >= 52 {
			let statname = line[0..35].to_string().trim().to_string();
			let total = i64::from_str(&line[35..52].trim().replace(",","")).unwrap_or(-1);
			if total >= 0 {
				ias.push(KeyInstanceStats{statname: statname.clone(), total: total as u64});
			}
		}
	}
	ias
}

fn io_stats_byfunc(table: ElementRef) -> HashMap<String, IOStats> {
	let mut result: HashMap<String, IOStats> = HashMap::new();
	let row_selector = Selector::parse("tr").unwrap();
	let column_selector = Selector::parse("td").unwrap();
	
	fn parse_data_size(s: &str) -> f64 {
		let s = s.trim().replace(",", ".");
		if s.is_empty() {
			return 0.0;
		}
		let (num, unit) = match s.get(..s.len() - 1).zip(s.chars().last()) {
			Some((num, unit)) => (num.trim(), unit),
			None => return 0.0, // fallback if string is too short
		};
		let val: f64 = num.parse().unwrap_or(0.0);
		match unit {
			'K' => val / 1024.0,
			'M' => val,
			'G' => val * 1024.0,
			'T' => val * 1024.0 * 1024.0,
			_ => val,
		}
	}
	
	fn parse_wait_time(s: &str) -> Option<f64> {
		let s = s.replace("&#160;", "").trim().replace(",", "").replace('\u{00A0}', "");
		if s.is_empty() {
			return None;
		}
		if s.ends_with("us") {
			s.trim_end_matches("us").parse::<f64>().ok().map(|v| (v / 1000.0 * 1_000_000.0).round() / 1_000_000.0) //round({:6})
		} else if s.ends_with("ms") {
			s.trim_end_matches("ms").parse::<f64>().ok()
		} else if s.ends_with("ns") {
			s.trim_end_matches("ns").parse::<f64>().ok().map(|v| (v / 1_000_000.0 * 1_000_000.0).round() / 1_000_000.0)
		} else {
			s.parse::<f64>().ok()
		}
	}
	
	fn parse_count(s: &str) -> u64 {
		let s = s.trim().replace(",", ".").to_lowercase();
		let multiplier = match s.chars().last() {
			Some('k') => 1_000.0,
			Some('m') => 1_000_000.0,
			Some('g') => 1_000_000_000.0,
			Some('t') => 1_000_000_000_000.0,
			Some('p') => 1_000_000_000_000_000.0,
			_ => 1.0,
		};
		let number_str = match s.chars().last() {
			Some(c) if "kmgtp".contains(c) => &s[..s.len() - 1],
			_ => &s,
		};
		number_str.parse::<f64>().map(|n| (n * multiplier) as u64).unwrap_or(0)
	}

	for row in table.select(&row_selector) {
		let columns = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() != 9 {
			continue;
		}
		let name = columns[0].text().collect::<String>().trim().to_string();
		if name == "TOTAL:" {
            continue;
        }
		let mut iostats = IOStats::default();
		
		iostats.reads_data = parse_data_size(&columns[1].text().collect::<String>());
		iostats.reads_req_s = columns[2].text().collect::<String>().replace(",", ".").parse().unwrap_or(0.0);
		iostats.reads_data_s = parse_data_size(&columns[3].text().collect::<String>());
		iostats.writes_data = parse_data_size(&columns[4].text().collect::<String>());
		iostats.writes_req_s = columns[5].text().collect::<String>().replace(",", ".").parse().unwrap_or(0.0);
		iostats.writes_data_s = parse_data_size(&columns[6].text().collect::<String>());
		iostats.waits_count = parse_count(&columns[7].text().collect::<String>());
		iostats.avg_time = parse_wait_time(&columns[8].text().collect::<String>());

		result.insert(name,iostats);
	}
	result
}

fn io_stats_byfunc_txt(iostats_section: Vec<&str>) -> HashMap<String, IOStats> {
	let mut result: HashMap<String, IOStats> = HashMap::new();
    
    fn parse_data_size(s: &str) -> f64 {
        let s = s.trim().replace(",", ".");
        if s.is_empty() || s == "." {
            return 0.0;
        }
        let (num, unit) = match s.get(..s.len() - 1).zip(s.chars().last()) {
            Some((num, unit)) => (num.trim(), unit),
            None => return 0.0, // fallback if string is too short
        };
        let val: f64 = num.parse().unwrap_or(0.0);
        match unit {
            'K' => val / 1024.0,
            'M' => val,
            'G' => val * 1024.0,
            'T' => val * 1024.0 * 1024.0,
            _ => val,
        }
    }
    
    fn parse_wait_time(s: &str) -> Option<f64> {
        let s = s.replace("&#160;", "").trim().replace(",", "").replace('\u{00A0}', "");
        if s.is_empty() || s == "." {
            return None;
        }
        if s.ends_with("us") {
            s.trim_end_matches("us").parse::<f64>().ok().map(|v| (v / 1000.0 * 1_000_000.0).round() / 1_000_000.0) //round({:6})
        } else if s.ends_with("ms") {
            s.trim_end_matches("ms").parse::<f64>().ok()
        } else if s.ends_with("ns") {
            s.trim_end_matches("ns").parse::<f64>().ok().map(|v| (v / 1_000_000.0 * 1_000_000.0).round() / 1_000_000.0)
        } else {
            s.parse::<f64>().ok()
        }
    }
    
    fn parse_count(s: &str) -> u64 {
        let s = s.trim().replace(",", ".").to_lowercase();
        if s.is_empty() || s == "." {
            return 0;
        }
        let multiplier = match s.chars().last() {
            Some('k') => 1_000.0,
            Some('m') => 1_000_000.0,
            Some('g') => 1_000_000_000.0,
            Some('t') => 1_000_000_000_000.0,
            Some('p') => 1_000_000_000_000_000.0,
            _ => 1.0,
        };
        let number_str = match s.chars().last() {
            Some(c) if "kmgtp".contains(c) => &s[..s.len() - 1],
            _ => &s,
        };
        number_str.parse::<f64>().map(|n| (n * multiplier) as u64).unwrap_or(0)
    }
    
    fn parse_requests_per_sec(s: &str) -> f64 {
        s.trim().replace(",", ".").parse().unwrap_or(0.0)
    }

	fn extract_columns(line: &str) -> Vec<String> {
		// Split line into columns, handling the fixed-width format
		let mut columns = Vec::new();
		let parts: Vec<&str> = line.split_whitespace().collect();
		
		if parts.is_empty() {
			return columns;
		}
		// Function name (first part, may contain spaces)
		let mut function_name = String::new();
		let mut data_start_idx = 0;
		// Find where numeric data starts
		for (i, part) in parts.iter().enumerate() {
			if is_numeric_or_volume(part) {
				data_start_idx = i;
				break;
			} else {
				if !function_name.is_empty() {
					function_name.push(' ');
				}
				function_name.push_str(part);
			}
		}
		
		columns.push(function_name);
		// Add the remaining data columns
		for part in &parts[data_start_idx..] {
			columns.push(part.to_string());
		}
		// Ensure we have at least 8 columns, pad with empty strings if needed
		while columns.len() < 9 {
			columns.push(String::new());
		}
		columns
	}
	
	fn is_numeric_or_volume(s: &str) -> bool {
		if s.is_empty() || s == "." {
			return true;
		}
		// Check if it's a number with optional volume suffix
		let s = s.trim();
		if s.ends_with(char::is_alphabetic) {
			let (num_part, _) = s.split_at(s.len() - 1);
			num_part.parse::<f64>().is_ok()
		} else {
			s.parse::<f64>().is_ok()
		}
	}
    
    // Find data rows (skip headers and separators)
    let mut in_data_section = false;
    for line in iostats_section {
        let line = line.trim();
        // Skip empty lines and comments
        if line.is_empty() || line.starts_with("->") || line.starts_with("IO Stat") {
            continue;
        }
        // Mark start of data section
        if line.contains("Function") && line.contains("Volume") {
            in_data_section = true;
            continue;
        }
        // Skip separator lines
        if line.starts_with("---") || line.starts_with("===") || line.contains("-----") {
            continue;
        }
        
        // Process data lines
        if in_data_section && !line.is_empty() {
            let columns = extract_columns(line);
            if columns.len() >= 8 { // Minimum required columns
                let name = match columns[0].trim() {
					"Buffer Cache Re" => "Buffer Cache Reads".to_string(),
					other => other.to_string(),
				};
                if name == "TOTAL:" || name.is_empty() {
                    continue;
                }
                
                let mut iostats = IOStats::default();
                iostats.reads_data = parse_data_size(&columns[1]);
                iostats.reads_req_s = parse_requests_per_sec(&columns[2]);
                iostats.reads_data_s = parse_data_size(&columns[3]);
                iostats.writes_data = parse_data_size(&columns[4]);
                iostats.writes_req_s = parse_requests_per_sec(&columns[5]);
                iostats.writes_data_s = parse_data_size(&columns[6]);
                iostats.waits_count = parse_count(&columns[7]);
                iostats.avg_time = if columns.len() > 8 {
                    parse_wait_time(&columns[8])
                } else {
                    None
                };
                result.insert(name, iostats);
            }
        }
    }
    
    result
}


fn wait_classes(table: ElementRef) -> Vec<WaitClasses> {
	let mut wait_classes: Vec<WaitClasses> = Vec::new();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
            let columns = row.select(&column_selector).collect::<Vec<_>>();
            if columns.len() == 6 {
				let wait_class = columns[0].text().collect::<Vec<_>>();
				let wait_class = wait_class[0].trim();

				let waits = columns[1].text().collect::<Vec<_>>();
				let waits = u64::from_str(&waits[0].trim().replace(",","")).unwrap_or(0);

				let total_wait_time = columns[3].text().collect::<Vec<_>>();
				let total_wait_time = f64::from_str(&total_wait_time[0].trim().replace(",","")).unwrap_or(0.0);

				let avg_wait_ms = columns[4].text().collect::<Vec<_>>();
				let avg_wait_ms = f64::from_str(&avg_wait_ms[0].trim().replace(",","")).unwrap_or(0.0);

				let db_time_pct = columns[5].text().collect::<Vec<_>>();
				let db_time_pct = f64::from_str(&db_time_pct[0].trim().replace(",","")).unwrap_or(0.0);

				wait_classes.push(WaitClasses {wait_class: wait_class.to_string(),
								waits: waits,
								total_wait_time_s: total_wait_time,
								avg_wait_ms: avg_wait_ms,
								db_time_pct: db_time_pct,
								});
			}
		}
	wait_classes
}

fn snap_info(table: ElementRef) -> SnapInfo {
	let mut si = SnapInfo::default();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
		let columns = row.select(&column_selector).collect::<Vec<_>>();
        if columns.len() >= 5 {
			let begin_end_snap = columns[0].text().collect::<Vec<_>>();
			let begin_end_snap = begin_end_snap[0].trim();
			
			let begin_end_snap_id = columns[1].text().collect::<Vec<_>>();
			let begin_end_snap_id = u64::from_str(&begin_end_snap_id[0]).unwrap_or(0);

			let begin_end_snap_time = columns[2].text().collect::<Vec<_>>();
			let begin_end_snap_time = begin_end_snap_time[0].trim();

			if begin_end_snap == "Begin Snap:" {
				si.begin_snap_id = begin_end_snap_id;
				si.begin_snap_time = begin_end_snap_time.to_string();
			} else if begin_end_snap == "End Snap:" {
				si.end_snap_id = begin_end_snap_id;
				si.end_snap_time = begin_end_snap_time.to_string();
			}
		}	
	}
	si
}

fn snap_info_txt(snap_section: Vec<&str>) -> SnapInfo {
	let mut si = SnapInfo::default();
	let fields_begin = snap_section[2].split_whitespace().collect::<Vec<&str>>();
	let fields_end = snap_section[3].split_whitespace().collect::<Vec<&str>>();
	let begin_snap = format!("{} {}", fields_begin[3], fields_begin[4]);
	let end_snap = format!("{} {}", fields_end[3], fields_end[4]);

	let begin_snap_id = format!("{}", fields_begin[2]);
	let end_snap_id = format!("{}", fields_end[2]);

	si.begin_snap_id = u64::from_str(&begin_snap_id).unwrap();
	si.end_snap_id = u64::from_str(&end_snap_id).unwrap();
	si.begin_snap_time = begin_snap;
	si.end_snap_time = end_snap;

	si
}

fn load_profile(table: ElementRef) -> Vec<LoadProfile>{
    let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();
    let mut lp: Vec<LoadProfile> = Vec::new();
    for row in table.select(&row_selector) {
		let columns = row.select(&column_selector).collect::<Vec<_>>();
		if columns.len() == 5 {
			let statname = columns[0].text().collect::<Vec<_>>();
			let statname = statname[0].trim();
			
			let per_second = columns[1].text().collect::<Vec<_>>();
			let per_second = f64::from_str(&per_second[0].trim().replace(",","")).unwrap_or(0.0);

			let per_transaction = columns[2].text().collect::<Vec<_>>();
			let per_transaction = f64::from_str(&per_transaction[0].trim().replace(",", "")).unwrap_or(0.0);

			lp.push(LoadProfile{stat_name: statname.to_string(), per_second: per_second, per_transaction: per_transaction});
		}
    }
    lp
}

fn load_profile_txt(load_section: Vec<&str>) -> Vec<LoadProfile> {
	let mut lp: Vec<LoadProfile> = Vec::new();
	for line in load_section {
		let statname_end = line.to_string().find(":");
		if statname_end.is_some() {
			let statname_end = statname_end.unwrap() + 1;
			let statname = line[0..statname_end].to_string().trim().to_string();
			let mut per_second_end = statname_end + 19;
			if per_second_end > line.len() {
				per_second_end = line.len();
			}
			let per_second = f64::from_str(&line[statname_end..per_second_end].trim().replace(",","")).unwrap();
			let mut per_transaction = 0.0;
			if !line.contains("Transactions") { 
				let mut transaction_end = statname_end + 20 + 18;
				if transaction_end > line.len() {
					transaction_end = line.len();
				}
				per_transaction = f64::from_str(&line[statname_end+19..transaction_end].trim().replace(",","")).unwrap();
			} 
			lp.push(LoadProfile{stat_name: statname.to_string(), per_second: per_second, per_transaction: per_transaction});
		}
		
	}
	lp
}

fn instance_info(table: ElementRef, table_type: &str) -> Option<DBInstance> {
    let th_selector = Selector::parse("th").unwrap();
    let tr_selector = Selector::parse("tr").unwrap();
    let td_selector = Selector::parse("td").unwrap();

    let headers: Vec<String> = table.select(&th_selector).map(|h| h.text().collect::<String>().trim().to_string()).collect();

    if table_type == "Info" {
		if headers.len() == 8 {
			if let Some(data_row) = table.select(&tr_selector).nth(1) {
				let cols: Vec<String> = data_row.select(&td_selector).map(|td| td.text().collect::<String>().trim().to_string()).collect();
				if cols.len() >= 7 {
					let mut dbi = DBInstance::default();
					dbi.db_id = u64::from_str(&cols[1]).unwrap_or(0);
					dbi.release = cols[5].clone();
					dbi.rac = cols[6].clone();
					return Some(dbi);
				}
			}
		}
	} else if table_type == "Details" {
		if headers.len() == 5 {
			if let Some(data_row) = table.select(&tr_selector).nth(1) {
				let cols: Vec<String> = data_row.select(&td_selector).map(|td| td.text().collect::<String>().trim().to_string()).collect();
				if cols.len() >= 3 {
					let mut dbi = DBInstance::default();
					dbi.instance_num = u8::from_str(&cols[1]).unwrap_or(0);
					dbi.startup_time = cols[2].clone();
					return Some(dbi);
				}
			}
		}
	} else if table_type == "Host"{
		if headers.len() == 6 {
			if let Some(data_row) = table.select(&tr_selector).nth(1) {
				let cols: Vec<String> = data_row.select(&td_selector).map(|td| td.text().collect::<String>().trim().to_string()).collect();
				if cols.len() >= 6 {
					let mut dbi = DBInstance::default();
					dbi.platform = cols[1].clone();
					dbi.cpus = u16::from_str(&cols[2]).unwrap_or(0);
					dbi.cores = u16::from_str(&cols[3]).unwrap_or(0);
					dbi.sockets = u8::from_str(&cols[4]).unwrap_or(0);
					let mem: f32 = cols[5].parse().unwrap_or(0.0);
					dbi.memory = mem.round() as u16;
					return Some(dbi);
				}
			}
		}
	} else if table_type == "db_block_size"{
		if headers.len() >= 3 {
			for row in table.select(&tr_selector).skip(1) { // Skip header row
				let cols: Vec<String> = row.select(&td_selector)
					.map(|td| td.text().collect::<String>().trim().to_string())
					.collect();
				// Ensure we have at least 2 columns (Parameter Name, Value)
				if cols[0] == "db_block_size" {
						let mut dbi = DBInstance::default();
						dbi.db_block_size = cols[1].parse().unwrap();
						return Some(dbi);
				}
			}
		}
	}
    None
}

fn instance_info_txt(info_section: Vec<&str>) -> DBInstance {
	let mut dbi = DBInstance::default();
	let mut db_info = info_section.first().unwrap().trim();
	let mut host_info = info_section.last().unwrap().trim();
	let db_tokens: Vec<&str> = db_info.split_whitespace().collect();
    if db_tokens.len() >= 7 {
        dbi.db_id = db_tokens[0].parse().unwrap_or_default();
        dbi.instance_num = db_tokens[2].parse().unwrap_or_default();
        dbi.startup_time = format!("{} {}", db_tokens[3], db_tokens[4]);
        dbi.release = db_tokens[5].to_string();
        dbi.rac = db_tokens[6].to_string();
    }
	let host_tokens: Vec<&str> = host_info.split_whitespace().collect();
	if host_tokens.len() >= 8 {
        dbi.platform = host_tokens[1..4].join(" ");
        dbi.cpus = host_tokens[4].parse().unwrap_or_default();
        dbi.cores = host_tokens[5].parse().unwrap_or_default();
        dbi.sockets = host_tokens[6].parse().unwrap_or_default();
        let mem: f32 = host_tokens[7].parse().unwrap_or(0.0);
        dbi.memory = mem.round() as u16;
    }
	dbi
}

fn parse_db_instance_information(fname: String) -> DBInstance {
	let mut db_instance_information = DBInstance::default();
    if fname.ends_with("html") {
        let html = fs::read_to_string(&fname)
            .expect(&format!("Couldn't open awr file {}", fname));
        let doc = Html::parse_document(&html);
        let table_selector = Selector::parse("table").unwrap();

        for table in doc.select(&table_selector) {
            if let Some(summary) = table.value().attr("summary") {
                if summary == "This table displays database instance information" {
                    if let Some(inst_info) = instance_info(table,"Info") {
                        // Merge fields from the first table:
                        db_instance_information.db_id = inst_info.db_id;
                        db_instance_information.release = inst_info.release;
                        db_instance_information.rac = inst_info.rac;
                    }
                    if let Some(inst_details) = instance_info(table,"Details") {
                        // Merge fields from the second table:
                        db_instance_information.instance_num = inst_details.instance_num;
                        db_instance_information.startup_time = inst_details.startup_time;
                    }
                } else if summary == "This table displays host information" {
                    if let Some(host_info) = instance_info(table,"Host") {
                        // Merge fields from the host table:
                        db_instance_information.platform = host_info.platform;
                        db_instance_information.cpus = host_info.cpus;
                        db_instance_information.cores = host_info.cores;
                        db_instance_information.sockets = host_info.sockets;
                        db_instance_information.memory = host_info.memory;
                    }
                } else if summary == "This table displays name and value of the modified initialization parameters" {
					if let Some(block_size) = instance_info(table,"db_block_size") {
						db_instance_information.db_block_size = block_size.db_block_size;
					}
				} else if summary == "This table displays name and value of the initialization parametersmodified by the current container" {
					if let Some(block_size) = instance_info(table,"db_block_size") {
						db_instance_information.db_block_size = block_size.db_block_size;
					}
				}	
            }
        }
	} else if fname.ends_with("txt") {
		let awr_rep = fs::read_to_string(&fname).expect(&format!("Couldn't open awr file {}", fname));
    	let awr_lines = awr_rep.split("\n").collect::<Vec<&str>>();
		let block_size = awr_lines.iter().find(|line| line.starts_with("db_block_size")).unwrap().split_whitespace().last().unwrap();
		let mut instance_info = find_section_boundries(awr_lines.clone(), "Database    DB Id", "Snapshot       Snap Id",&fname, None);
		let mut instance_info_lines: Vec<&str> = Vec::new();
		instance_info_lines.extend_from_slice(&awr_lines[instance_info.begin+2..instance_info.end]);
		db_instance_information = instance_info_txt(instance_info_lines);
		db_instance_information.db_block_size = block_size.parse().unwrap();
	}
	db_instance_information
}

fn parse_awr_report_internal(fname: &str, args: &Args) -> (AWR, HashMap<String, String>) {
	let mut awr: AWR = AWR::default();
	let mut sqls_txt: HashMap<String, String> = HashMap::new();
	if fname.ends_with("html") {

		//println!("Parsing file {}", &fname);
		let html_file = fs::read_to_string(fname);
		let html = html_file.unwrap();

		let doc = Html::parse_document(&html);
		let table_selector = Selector::parse("table").unwrap();
		let row_selector = Selector::parse("tr").unwrap();
		let column_selector = Selector::parse("td").unwrap();


		for element in doc.select(&table_selector) {
			if element.value().attr("summary").unwrap() == "This table displays load profile" {
				awr.load_profile = load_profile(element);
			} else if element.value().attr("summary").unwrap() == "This table displays foreground wait class statistics" {
				awr.wait_classes = wait_classes(element);
			} else if element.value().attr("summary").unwrap() == "This table displays system load statistics" {
				awr.host_cpu = host_cpu(element);
			} else if element.value().attr("summary").unwrap() == "This table displays different time model statistics. For each statistic, time and % of DB time are displayed" {
				awr.time_model_stats = time_model_stats(element);
			} else if element.value().attr("summary").unwrap() == "This table displays Foreground Wait Events and their wait statistics" {
				awr.foreground_wait_events = wait_events(element);
			} else if element.value().attr("summary").unwrap() == "This table displays background wait events statistics" {
				awr.background_wait_events = wait_events(element);
			} else if element.value().attr("summary").unwrap() == "This table displays top SQL by elapsed time" {
				awr.sql_elapsed_time = sql_elapsed_time(element);
			} else if element.value().attr("summary").unwrap() == "This table displays top SQL by CPU time" {
				awr.sql_cpu_time = sql_cpu_time(element);
			} else if element.value().attr("summary").unwrap() == "This table displays top SQL by user I/O time" {
				awr.sql_io_time = sql_io_time(element);
			} else if element.value().attr("summary").unwrap() == "This table displays top SQL by buffer gets" {
				awr.sql_gets = sql_gets(element);
			} else if element.value().attr("summary").unwrap() == "This table displays top SQL by physical reads" {
				awr.sql_reads = sql_reads(element);
			} else if element.value().attr("summary").unwrap() == "This table displays snapshot information" {
				awr.snap_info = snap_info(element);
			} else if element.value().attr("summary").unwrap() == "This table displays Instance activity statistics. For each instance, activity total, activity per second, and activity per transaction are displayed" {
				awr.key_instance_stats = instance_activity_stats(element);
			} else if element.value().attr("summary").unwrap() == "This table displays the IO Statistics for different functions. IO stats includes amount of reads and writes, requests per second, data per second, wait count and average wait time" {
				awr.io_stats_byfunc = io_stats_byfunc(element);
			} else if element.value().attr("summary").unwrap() == "This table displays thread activity stats in the instance. For each activity , total number of activity and activity per hour are displayed" {
				awr.redo_log = redo_log_switches(element);
			} else if element.value().attr("summary").unwrap() == "This table displays dictionary cache statistics. Get requests, % misses, scan requests, final usage, etc. are displayed for each cache" {
				awr.dictionary_cache = dictionary_cache_stats(element);
			} else if element.value().attr("summary").unwrap() == "This table displays library cache statistics. Get requests, % misses, pin request, % miss, reloads, etc. are displayed for each library cache namespace" {
				awr.library_cache = library_cache_stats(element);
			} else if element.value().attr("summary").unwrap() == "This table displays latch statistics. Get requests, % get miss, wait time, noWait requests are displayed for each latch" {
				awr.latch_activity = latch_activity_stats(element);
			} else if element.value().attr("summary").unwrap() == "This table displays top segments by row lock waits. Owner, tablespace name, object type, row lock waits, etc. are displayed for each segment" {
				let segment = segment_stats(element, "Row Lock Waits", &args);
				awr.segment_stats.insert("Row Lock Waits".to_string(), segment);
			} else if element.value().attr("summary").unwrap() == "This table displays top segments by logical reads. Owner, tablespace name, object type, logical read, etc. are displayed for each segment" {
				let segment = segment_stats(element, "Logical Reads", &args);
				awr.segment_stats.insert("Logical Reads".to_string(), segment);
			} else if element.value().attr("summary").unwrap() == "This table displays top segments by physical reads. Owner, tablespace name, object type, physical reads, etc. are displayed for each segment" {
				let segment = segment_stats(element, "Reads", &args);
				awr.segment_stats.insert("Physical Reads".to_string(), segment);
			} else if element.value().attr("summary").unwrap() == "This table displays top segments by physical read requests. Owner, tablespace name, object type, physical read requests, etc. are displayed for each segment" {
				let segment = segment_stats(element, "Read Requests", &args);
				awr.segment_stats.insert("Physical Read Requests".to_string(), segment);
			} else if element.value().attr("summary").unwrap() == "This table displays top segments by direct physical reads. Owner, tablespace name, object type, direct reads, etc. are displayed for each segment" {
				let segment = segment_stats(element, "Direct Reads", &args);
				awr.segment_stats.insert("Direct Physical Reads".to_string(), segment);
			} else if element.value().attr("summary").unwrap() == "This table displays top segments by physical writes. Owner, tablespace name, object type, physical writes, etc. are displayed for each segment" {
				let segment = segment_stats(element, "Writes", &args);
				awr.segment_stats.insert("Physical Writes".to_string(), segment);
			} else if element.value().attr("summary").unwrap() == "This table displays top segments by physical write requests. Owner, tablespace name, object type, physical write requests, etc. are displayed for each segment" {
				let segment = segment_stats(element, "Write Requests", &args);
				awr.segment_stats.insert("Physical Write Requests".to_string(), segment);
			} else if element.value().attr("summary").unwrap() == "This table displays top segments by direct physical writes. Owner, tablespace name, object type, direct writes, etc. are displayed for each segment" {
				let segment = segment_stats(element, "Direct Writes", &args);
				awr.segment_stats.insert("Direct Physical Writes".to_string(), segment);
			} else if element.value().attr("summary").unwrap() == "This table displays top segments by buffer busy waits. Owner, tablespace name, object type, buffer busy waits, etc. are displayed for each segment" {
				let segment = segment_stats(element, "Busy Waits", &args);
				awr.segment_stats.insert("Buffer Busy Waits".to_string(), segment);
			} else if element.value().attr("summary").unwrap() == "This table displays top segments by global cache buffer busy waits. Owner, tablespace name, object type, GC buffer busy waits, etc. are displayed for each segment" {
				let segment = segment_stats(element, "GCBusy Waits", &args);
				awr.segment_stats.insert("Global Cache Buffer Busy".to_string(), segment);
			} else if args.security_level>=2 && element.value().attr("summary").unwrap().starts_with("This table displays the text of the SQL") {
				 sqls_txt = sql_text(element);
			} else if element.value().attr("summary").unwrap() == "This table displays the Top SQL by Top Wait Events" {
				awr.top_sql_with_top_events = top_sql_with_top_events(element);
			} else if element.value().attr("summary").unwrap() == "This table displays total number of waits, and information about total wait time, for each wait event" {
				let event_histogram = waitevent_histogram_ms(element);
				if event_histogram.len() > 0 {
					for ev in awr.foreground_wait_events.iter_mut() {
						if event_histogram.contains_key(&ev.event) {
							let histogram = event_histogram.get(&ev.event).unwrap().clone();
							ev.waitevent_histogram_ms = histogram;
						}
					}
					for ev in awr.background_wait_events.iter_mut() {
						if event_histogram.contains_key(&ev.event) {
							let histogram = event_histogram.get(&ev.event).unwrap().clone();
							ev.waitevent_histogram_ms = histogram;
						}
					}
				}
			}
		}
	} else if fname.ends_with("txt") {
		let awr_rep = fs::read_to_string(&fname).expect(&format!("Couldn't open awr file {}", fname));
    	let awr_lines = awr_rep.split("\n").collect::<Vec<&str>>();

		let mut snapshot_index = find_section_boundries(awr_lines.clone(), "Snapshot       Snap Id", "Cache Sizes",&fname, None);
		if snapshot_index.begin == 0 && snapshot_index.end == 0 {
			snapshot_index = find_section_boundries(awr_lines.clone(), "              Snap Id", "Top ADDM Findings",&fname, None);
		}
		let mut snap_info_lines: Vec<&str> = Vec::new();
		snap_info_lines.extend_from_slice(&awr_lines[snapshot_index.begin..snapshot_index.end]);
		awr.snap_info = snap_info_txt(snap_info_lines); 

		let host_cpu_section_start = format!("{}{}", 12u8 as char, "Host CPU");
		let host_cpu_index = find_section_boundries(awr_lines.clone(), &host_cpu_section_start, "Instance CPU",&fname, None);
		if host_cpu_index.begin != 0 && host_cpu_index.end != 0 {
    		let host_cpu_lines: Vec<&str> = awr_lines[host_cpu_index.begin..host_cpu_index.end+2].to_vec();
    		awr.host_cpu = host_cpu_txt(host_cpu_lines);
		}

		// Search for the line containing "log switches (derived)"
        if let Some(line) = awr_lines.iter().find(|&&line| line.contains("log switches (derived)")) {
            awr.redo_log = redo_log_switches_txt(line);
        }
		
		let load_profile_index = find_section_boundries(awr_lines.clone(), "Load Profile", "Instance Efficiency",&fname, None);
		let mut load_profile_lines: Vec<&str> = Vec::new();
		load_profile_lines.extend_from_slice(&awr_lines[load_profile_index.begin+2..load_profile_index.end]);
		awr.load_profile = load_profile_txt(load_profile_lines);

		let time_model_index = find_section_boundries(awr_lines.clone(), "Time Model", "DB time",&fname, None);
		let mut db_time_lines: Vec<&str> = Vec::new();
		db_time_lines.extend_from_slice(&awr_lines[time_model_index.begin+5..time_model_index.end]);
		awr.time_model_stats = time_model_stats_txt(db_time_lines);

		let foreground_even_section_start = format!("{}{}", 12u8 as char, "Foreground Wait Events");
		let foreground_event_index = find_section_boundries(awr_lines.clone(), &foreground_even_section_start, "Background Wait Events",&fname, None);
		let mut foreground_events: Vec<&str> = Vec::new();
		foreground_events.extend_from_slice(&awr_lines[foreground_event_index.begin+8..foreground_event_index.end-1]);
		awr.foreground_wait_events = wait_events_txt(foreground_events);

		let background_even_section_start = format!("{}{}", 12u8 as char, "Background Wait Events");
		let background_event_index = find_section_boundries(awr_lines.clone(), &background_even_section_start, "Wait Events (fg and bg)",&fname, None);
		let mut background_events: Vec<&str> = Vec::new();
		background_events.extend_from_slice(&awr_lines[background_event_index.begin+8..background_event_index.end-1]);
		awr.background_wait_events = wait_events_txt(background_events);

		let sql_cpu_section_start = format!("{}{}", 12u8 as char, "SQL ordered by CPU");
		let sql_cpu_section_end = format!("{}{}", 12u8 as char, "SQL ordered by Elapsed time");
		let sql_cpu_index = find_section_boundries(awr_lines.clone(), &sql_cpu_section_start, &sql_cpu_section_end,&fname, None);
		let mut sql_cpu: Vec<&str> = Vec::new();
		sql_cpu.extend_from_slice(&awr_lines[sql_cpu_index.begin..sql_cpu_index.end]);
		awr.sql_cpu_time = sql_cpu_time_txt(sql_cpu);

		let sql_gets_section_start = format!("{}{}", 12u8 as char, "SQL ordered by Gets");
		let sql_gets_section_end = format!("{}{}", 12u8 as char, "SQL ordered by Reads");
		let sql_gets_index = find_section_boundries(awr_lines.clone(), &sql_cpu_section_start, &sql_cpu_section_end,&fname, None);
		let mut sql_gets: Vec<&str> = Vec::new();
		sql_gets.extend_from_slice(&awr_lines[sql_gets_index.begin..sql_gets_index.end]);
		awr.sql_gets = sql_gets_txt(sql_gets);

		let sql_reads_section_start = format!("{}{}", 12u8 as char, "SQL ordered by Reads");
		let sql_reads_section_end = format!("{}{}", 12u8 as char, "SQL ordered by Executions");
		let sql_reads_index = find_section_boundries(awr_lines.clone(), &sql_cpu_section_start, &sql_cpu_section_end,&fname, None);
		let mut sql_reads: Vec<&str> = Vec::new();
		sql_reads.extend_from_slice(&awr_lines[sql_reads_index.begin..sql_reads_index.end]);
		awr.sql_reads = sql_reads_txt(sql_reads);

		let sql_ela_section_start = format!("{}{}", 12u8 as char, "SQL ordered by Elapsed time");
		let sql_ela_section_end = format!("{}{}", 12u8 as char, "SQL ordered by Gets");
		let mut sql_ela_index = find_section_boundries(awr_lines.clone(), &sql_ela_section_start, &sql_ela_section_end,&fname, None);

		if sql_ela_index.begin == 0 || sql_ela_index.end == 0 {
			let sql_ela_section_start = format!("{}{}", 12u8 as char, "SQL ordered by Elapsed Time");
			let sql_ela_section_end = format!("{}{}", 12u8 as char, "SQL ordered by CPU Time");
			sql_ela_index = find_section_boundries(awr_lines.clone(), &sql_ela_section_start, &sql_ela_section_end,&fname, None);
		}

		let mut sql_ela: Vec<&str> = Vec::new();
		sql_ela.extend_from_slice(&awr_lines[sql_ela_index.begin..sql_ela_index.end]);
		awr.sql_elapsed_time = sql_ela_time_txt(sql_ela);

		let instance_activity_start = format!("{}{}", 12u8 as char, "Instance Activity Stats");
		let instance_activity_end = format!("{}{}", 12u8 as char, "workarea executions - optimal");
		let instance_act_index = find_section_boundries(awr_lines.clone(), &instance_activity_start, &instance_activity_end,&fname, None);
		let mut inst_stats: Vec<&str> = Vec::new();
		inst_stats.extend_from_slice(&awr_lines[instance_act_index.begin..instance_act_index.end+2]);
		awr.key_instance_stats = instance_activity_stats_txt(inst_stats);

		let iostats_summary_start = format!("{}{}", 12u8 as char, "IO Stat by Function - summary");
		let iostats_summary_end = format!("{}{}", 12u8 as char, "IO Stat by Function - detail");
		let iostats_summary_index = find_section_boundries(awr_lines.clone(), &iostats_summary_start, &iostats_summary_end,&fname, Some(true));
		let mut iostats_summary_stats: Vec<&str> = Vec::new();
		iostats_summary_stats.extend_from_slice(&awr_lines[iostats_summary_index.begin..iostats_summary_index.end+2]);
		awr.io_stats_byfunc = io_stats_byfunc_txt(iostats_summary_stats);
		
		let dictionary_cache_start = format!("{}{}", 12u8 as char, "Dictionary Cache Stats");
		let dictionary_cache_end = format!("{}{}", 12u8 as char, "Library Cache Activity");
		let dictionary_cache_index = find_section_boundries(awr_lines.clone(), &dictionary_cache_start, &dictionary_cache_end,&fname, None);
		let mut dictionary_cache: Vec<&str> = Vec::new();
		dictionary_cache.extend_from_slice(&awr_lines[dictionary_cache_index.begin..dictionary_cache_index.end+2]);
		awr.dictionary_cache = dictionary_cache_stats_txt(dictionary_cache);


		let library_cache_start = format!("{}{}", 12u8 as char, "Library Cache Activity");
		let library_cache_end = format!("{}{}", 12u8 as char, "          -------------------------------------------------------------");
		let library_cache_index = find_section_boundries(awr_lines.clone(), &library_cache_start, &library_cache_end,&fname, None);
		let mut library_cache: Vec<&str> = Vec::new();
		library_cache.extend_from_slice(&awr_lines[library_cache_index.begin..library_cache_index.end+2]);
		awr.library_cache = library_cache_stats_txt(library_cache);

		let latch_activity_start = format!("{}{}", 12u8 as char, "Latch Activity");
		let latch_activity_end = format!("{}{}", 12u8 as char, "          -------------------------------------------------------------");
		let latch_activity_index = find_section_boundries(awr_lines.clone(), &latch_activity_start, &latch_activity_end,&fname, None);
		let mut latch_activity: Vec<&str> = Vec::new();
		latch_activity.extend_from_slice(&awr_lines[latch_activity_index.begin..latch_activity_index.end+2]);
		awr.latch_activity = latch_activity_stats_txt(latch_activity);

		let mut event_names: HashMap<String, String> = HashMap::new();
		for ev in &awr.foreground_wait_events {
			if ev.event.len() >= 26 {
				event_names.insert(ev.event[0..26].to_string(), ev.event.clone());
			} else {
				event_names.insert(ev.event.to_string(), ev.event.clone());
			}
		}
		let mut bgevent_names: HashMap<String, String> = HashMap::new();
		for ev in &awr.background_wait_events {
			if ev.event.len() >= 26 {
				bgevent_names.insert(ev.event[0..26].to_string(), ev.event.clone());
			} else {
				bgevent_names.insert(ev.event.to_string(), ev.event.clone());
			}
		}
		let event_histogram_start = format!("{}{}", 12u8 as char, "Wait Event Histogram");
		let event_histogram_end = format!("{}{}", 12u8 as char, "SQL ordered by");
		let event_histogram_index = find_section_boundries(awr_lines.clone(), &event_histogram_start, &event_histogram_end,&fname, None);
		let mut event_hist: Vec<&str> = Vec::new();
		event_hist.extend_from_slice(&awr_lines[event_histogram_index.begin..event_histogram_index.end]);
		let event_histogram = waitevent_histogram_ms_txt(event_hist.clone(), event_names);
		let bgevent_histogram = waitevent_histogram_ms_txt(event_hist, bgevent_names);
		if event_histogram.len() > 0 {
			for ev in awr.foreground_wait_events.iter_mut() {
				if event_histogram.contains_key(&ev.event) {
					let histogram = event_histogram.get(&ev.event).unwrap().clone();
					ev.waitevent_histogram_ms = histogram;
				}
			}
		}
		if bgevent_histogram.len() > 0 {
			for ev in awr.background_wait_events.iter_mut() {
				if bgevent_histogram.contains_key(&ev.event) {
					let histogram = bgevent_histogram.get(&ev.event).unwrap().clone();
					ev.waitevent_histogram_ms = histogram;
				}
			}
		}
	}
	awr.status = "OK".to_string();
	awr.file_name = fname.to_string();
	(awr, sqls_txt)
}



pub fn parse_awr_dir(args: Args, events_sqls: &mut HashMap<&str, HashSet<String>>, file: &str) {
	println!("{}","\n==== PARSING DIRECTORY DATA ===".bright_cyan());
	//let mut awr_vec: Vec<AWR> = Vec::new();
	let mut file_collection: Vec<String> = Vec::new();
	let mut is_instance_info: Option<DBInstance> = None; // to grab DBInstance info from the first file
	for file in fs::read_dir(&args.directory).unwrap() {
		let fname: &String = &file.unwrap().path().display().to_string();
		let file_name = fname.split("/").collect::<Vec<&str>>();
		let file_name = file_name.last().unwrap().to_string();
		if !file_name.starts_with(".") && (file_name.ends_with(".txt") || file_name.ends_with(".html")) {
			if is_instance_info.is_none() {
                is_instance_info = Some(parse_db_instance_information(fname.to_string()));
            }
			file_collection.push(fname.clone()); 
		}
    }

	let pb = ProgressBar::new(file_collection.len() as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({percent}%)")
        .unwrap()
        .progress_chars("##-"));

    let counter = Arc::new(AtomicUsize::new(0));

	/* This will create a separate thread which will display a progress bar - updating progress bar inside a thread is too slow */
	let counter_clone = Arc::clone(&counter); //clone of atomic counter
	let pb_clone = pb.clone(); //clone of progress bar
	let fc = file_collection.clone(); //clone of file_collection
	let update_thread = thread::spawn(move || {
		loop {
			let val = counter_clone.load(Ordering::Relaxed); //check counter value
			pb_clone.set_position(val as u64); //update progress bar

			if val >= fc.len() {
				break; //if counter is bigger or equal than the number of files you can stop displaying this
			}
			thread::sleep(Duration::from_millis(100)); // update display every 100ms
		}
	});
	/************************************************************/

	//This is save HashMap which will be filled with SQLText if appropriate Security Level is being set
	let sqls_txt = Arc::new(DashMap::<String, String>::new());

    let mut awr_vec: Vec<AWR> = file_collection
        .par_iter()
        .map_init( //initialize variables for each thread
            || (Arc::clone(&counter), Arc::clone(&sqls_txt)), //initializied will be counter as cloned value for each thread
            |(counter, s), f| { //map operator is initialized clone of counter and file name
                let (result, sqls) = parse_awr_report_internal(f, &args); //each thread is processing one file
				if !sqls.is_empty() {
					for (sqlid, sqltxt) in sqls {
						s.entry(sqlid).or_insert(sqltxt);
					}
				}
				counter.fetch_add(1, Ordering::Relaxed); //increment counter
                result
            },
        )
        .collect(); //collect result into collection of awrs

	update_thread.join().unwrap(); //wait for thread updating progress bar to finish
    pb.finish_with_message("Finished parsing! 🎉");

	println!("");

	awr_vec.sort_by_key(|a| a.snap_info.begin_snap_id);

    let fg_events: HashSet<String> = awr_vec
										.iter()
										.flat_map(|a| a.foreground_wait_events.clone())
										.map(|e| e.event)
										.collect();

	let bg_events: HashSet<String> = awr_vec
										.iter()
										.flat_map(|a| a.background_wait_events.clone())
										.map(|e| e.event)
										.collect();

	let sqls: HashSet<String> = awr_vec
								.iter()
								.flat_map(|a| a.sql_elapsed_time.clone())
								.map(|s| s.sql_id)
								.collect();

	events_sqls.insert("FG", fg_events);
	events_sqls.insert("BG", bg_events);
	events_sqls.insert("SQL", sqls);

	/* Collect sqls txt map from Arc */
	let dash = Arc::try_unwrap(sqls_txt)
    	.expect("Other Arc clones still exist");

	let sql_txt_final: HashMap<_, _> = dash
		.into_iter()
		.collect();

	/* ************************* */

    let collection = AWRSCollection {
        db_instance_information: is_instance_info.unwrap_or_default(),
        awrs: awr_vec,
		sql_text: sql_txt_final,
    };
    let json_str = serde_json::to_string_pretty(&collection).unwrap();
	let mut f = fs::File::create(file).unwrap();
		f.write_all(json_str.as_bytes()).unwrap();
    if args.plot > 0 {
        let html_fname = format!("{}.html", &args.directory);
        plot_to_file(collection, html_fname, args.clone());
    }
    //Ok(json_str)
}

pub fn parse_awr_report(data: &str, json_data: bool, args: &Args) -> Result<String, std::io::Error> {
	let mut fname: String = "nofile.html".to_string();
	if json_data {
		let cmd_data: Result<HashMap<String, String>, serde_json::Error> = serde_json::from_str(&data);
    	let cmd_data: HashMap<String, String> = match cmd_data {
			Ok(c) => c,
			Err(e) => return Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
		};
		if cmd_data.contains_key("file_name") {
			println!("{:?}", cmd_data);
			fname = cmd_data["file_name"].clone();
		}
	
	} else {
		fname = data.to_string();
	}
	println!("Try to parsee a file: {}", &fname);
	let awr = parse_awr_report_internal(&fname, &args);
	
    let awr_doc: String = serde_json::to_string_pretty(&awr).unwrap();
	Ok(awr_doc)
}

pub fn prarse_json_file(args: Args, events_sqls: &mut HashMap<&str, HashSet<String>>) {
	println!("{}","\n==== PARSING JSON DATA ===".bright_cyan());
	//fname: String, db_time_cpu_ratio: f64, filter_db_time: f64, snap_range: String
	let json_file = fs::read_to_string(&args.json_file).expect(&format!("Something wrong with a file {} ", &args.json_file));
	let mut collection: AWRSCollection = serde_json::from_str(&json_file).expect("Wrong JSON format");
	collection.awrs.clone().sort_by_key(|a| a.snap_info.begin_snap_id);
	println!("{} samples found",collection.awrs.len());
    let file_and_ext: Vec<&str> = args.json_file.split('.').collect();
    let html_fname = format!("{}.html", file_and_ext[0]);
	let fg_events: HashSet<String> = collection.awrs
										.iter()
										.flat_map(|a| a.foreground_wait_events.clone())
										.map(|e| e.event)
										.collect();

	let bg_events: HashSet<String> = collection.awrs
										.iter()
										.flat_map(|a| a.background_wait_events.clone())
										.map(|e| e.event)
										.collect();

	let sqls: HashSet<String> = collection.awrs
								.iter()
								.flat_map(|a| a.sql_elapsed_time.clone())
								.map(|s| s.sql_id)
								.collect();

	events_sqls.insert("FG", fg_events);
	events_sqls.insert("BG", bg_events);
	events_sqls.insert("SQL", sqls);
	plot_to_file(collection, html_fname, args.clone());
}
