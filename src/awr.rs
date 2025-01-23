use std::clone;
use std::env;
use std::fs;
use std::str;
use std::result;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::collections::{BTreeMap, HashMap};
use std::char;
use crate::analyze::plot_to_file;
use crate::idleevents::is_idle;

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct LoadProfile {
	pub stat_name: String,
	pub per_second: f64,
	per_transaction: f64,
	pub begin_snap_time: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct RedoLog {
	pub stat_name: String,
	pub per_hour: f64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct DBInstance {
	db_name: String,
	db_id: u64,
	instance_name: String, 
	instance_num: u8,
	startup_time: String,
	release: String,
	rac: String,
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
	cpus: u32,
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
	stat_name: String,
	time_s: f64,
	pct_dbtime: f64,
	begin_snap_time: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct ForegroundWaitEvents {
	pub event: String,
	pub waits: u64,
	pub total_wait_time_s: f64,
	pub avg_wait: f64,
	pub pct_dbtime: f64,
	begin_snap_time: String,
	pub waitevent_histogram_ms: HashMap<String,f32>,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct SQLElapsedTime {
	pub sql_id: String,
	pub elapsed_time_s: f64,
	pub executions: u64,
	pub elpased_time_exec_s: f64,
	pct_total: f64,
	pct_cpu: f64, 
	pct_io: f64,
	sql_module: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct SQLCPUTime {
	sql_id: String,
	cpu_time_s: f64,
	executions: u64,
	cpu_time_exec_s: f64,
	pct_total: f64,
	pct_cpu: f64, 
	pct_io: f64,
	sql_module: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct SQLIOTime {
	sql_id: String,
	io_time_s: f64,
	executions: u64,
	io_time_exec_s: f64,
	pct_total: f64,
	pct_cpu: f64, 
	pct_io: f64,
	sql_module: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct SnapInfo {
	pub begin_snap_id: u32,
	end_snap_id: u32,
	pub begin_snap_time: String,
	end_snap_time: String,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct KeyInstanceStats {
	pub statname: String,
	pub total: u64,
}

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct AWR {
	status: String,
	pub load_profile: Vec<LoadProfile>,
	pub redo_log: RedoLog,
	db_instance_information: DBInstance,
	wait_classes: Vec<WaitClasses>,
	pub host_cpu: HostCPU,
	time_model_stats: Vec<TimeModelStats>,
	pub foreground_wait_events: Vec<ForegroundWaitEvents>,
	pub sql_elapsed_time: Vec<SQLElapsedTime>,
	sql_cpu_time: Vec<SQLCPUTime>,
	sql_io_time: Vec<SQLIOTime>,
	pub key_instance_stats: Vec<KeyInstanceStats>,
	pub snap_info: SnapInfo,
} 

#[derive(Default,Serialize, Deserialize, Debug, Clone)]
pub struct AWRS {
	pub file_name: String,
	pub awr_doc: AWR,
} 

#[derive(Debug)]
struct SectionIdx {
	begin: usize,
	end: usize,
}

fn find_section_boundries(awr_doc: Vec<&str>, section_start: &str, section_end: &str) -> SectionIdx {
	let mut awr_iter: std::vec::IntoIter<&str> = awr_doc.into_iter();
	let section_start2 = &section_start[1..section_start.len()-1];
	let section_end2: &str = &section_end[1..section_end.len()-1];
	let section_start = awr_iter.position(|x| x.starts_with(section_start) || x.starts_with(section_start2)).unwrap_or(0);
	let section_end = awr_iter.position(|x: &str| x.starts_with(section_end) || x.starts_with(section_end2)).unwrap_or(0);
	
	let si = SectionIdx{begin: section_start, end: section_start+section_end}; 
	si
}

fn sql_ela_time_txt(sql_ela_section: Vec<&str>) -> Vec<SQLElapsedTime> {
	let mut sql_ela_time: Vec<SQLElapsedTime> = Vec::new();
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
				let sql_id_hash = fields[6].trim().to_string();
				sql_ela_time.push(SQLElapsedTime{sql_id: sql_id_hash, 
												elapsed_time_s: ela_time, 
												executions: executions, 
												elpased_time_exec_s: ela_exec, 
												pct_total: pct_total, 
												pct_cpu: -1.0, pct_io: -1.0, sql_module: "?".to_string()});
			}
		}
	}
	sql_ela_time
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

			sql_elapsed_time.push(SQLElapsedTime { sql_id, elapsed_time_s, executions, elpased_time_exec_s, pct_total, pct_cpu, pct_io, sql_module })
		
		}
	}

	sql_elapsed_time
}

fn sql_cpu_time_txt(sql_cpu_section: Vec<&str>) -> Vec<SQLCPUTime> {
	let mut sql_cpu_time: Vec<SQLCPUTime> = Vec::new();
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
				let sql_id_hash = fields[6].trim().to_string();
				sql_cpu_time.push(SQLCPUTime{sql_id: sql_id_hash, 
												cpu_time_s: cpu_time, 
												executions: executions, 
												cpu_time_exec_s: cpu_exec, 
												pct_total: pct_total, 
												pct_cpu: -1.0, pct_io: -1.0, sql_module: "?".to_string()});
			}
		}
	}
	sql_cpu_time
}


fn sql_cpu_time(table: ElementRef) -> Vec<SQLCPUTime> {
	let mut sql_cpu_time: Vec<SQLCPUTime> = Vec::new();
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

			sql_cpu_time.push(SQLCPUTime { sql_id, cpu_time_s, executions, cpu_time_exec_s, pct_total, pct_cpu, pct_io, sql_module })
		}
	}

	sql_cpu_time
}

fn sql_io_time(table: ElementRef) -> Vec<SQLIOTime> {
	let mut sql_io_time: Vec<SQLIOTime> = Vec::new();
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

			sql_io_time.push(SQLIOTime { sql_id, io_time_s, executions, io_time_exec_s, pct_total, pct_cpu, pct_io, sql_module })
		}
	}

	sql_io_time
}

fn waitevent_histogram_ms(table: ElementRef) -> HashMap<String, HashMap<String, f32>> {
	let mut histogram: HashMap<String, HashMap<String, f32>> = HashMap::new();
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
				histogram.entry(event.to_string()).or_insert(HashMap::new());
				let mut v = histogram.get_mut(event).unwrap();
				v.entry(buckets[i-2].clone()).or_insert(pct_time);
			}
		} 
	}

	histogram
}

fn waitevent_histogram_ms_txt(events_histogram_section: Vec<&str>, event_names: HashMap<String, String>) -> HashMap<String, HashMap<String, f32>> {
	let mut histogram: HashMap<String, HashMap<String, f32>> = HashMap::new();
	for line in events_histogram_section {
		if line.len() > 26 {
			let mut hist_values: HashMap<String, f32> = HashMap::from([
				("<1ms".to_string(), 0.0),
				("<2ms".to_string(), 0.0),
				("<4ms".to_string(), 0.0),
				("<8ms".to_string(), 0.0),
				("<16ms".to_string(), 0.0),
				("<32ms".to_string(), 0.0),
				("<=1s".to_string(), 0.0),
				(">1s".to_string(), 0.0),
			]);

			let mut event_name = line[0..26].to_string().trim().to_string();
			if event_names.contains_key(&event_name) {
				event_name = event_names.get(&event_name).unwrap().clone();
				if line.len() >= 37 {
					let pct_val = f32::from_str(&line[33..38].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("<1ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 43 {
					let pct_val = f32::from_str(&line[39..44].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("<2ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 49 {
					let pct_val = f32::from_str(&line[45..50].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("<4ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 55 {
					let pct_val = f32::from_str(&line[51..56].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("<8ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 61 {
					let pct_val = f32::from_str(&line[57..62].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("<16ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 67 {
					let pct_val = f32::from_str(&line[63..68].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("<32ms").unwrap();
					*x = pct_val;
				}
				if line.len() >= 73 {
					let pct_val = f32::from_str(&line[69..74].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut("<=1s").unwrap();
					*x = pct_val;
				}
				if line.len() >= 79 {
					let pct_val = f32::from_str(&line[75..80].trim().replace(",","")).unwrap_or(0.0);
					let x = hist_values.get_mut(">1s").unwrap();
					*x = pct_val;
				}
				histogram.insert(event_name.clone(), hist_values.clone());
			}
		}
	}
	histogram
}

fn foreground_events_txt(foreground_events_section: Vec<&str>) -> Vec<ForegroundWaitEvents> {
	let mut fg: Vec<ForegroundWaitEvents> = Vec::new();
	for line in foreground_events_section {
		//println!("{}", line);
		if line.len() >= 73 {
			let statname = line[0..28].to_string().trim().to_string();
			let waits = u64::from_str(&line[29..41].trim().replace(",",""));

			if waits.is_ok() {

				let waits: u64 = waits.unwrap_or(0);
				let mut total_wait_time = f64::from_str(&line[46..57].trim().replace(",","")).unwrap_or(0.0);
				if total_wait_time == 0.0 {
					total_wait_time = f64::from_str(&line[38..54].trim().replace(",","")).unwrap_or(0.0);
				}
				let avg_wait = f64::from_str(&line[57..64].trim().replace(",","")).unwrap_or(0.0);
				let mut pct_dbtime = 0.0;
				if line.len() > 73 {
					let mut pct_dbtime_end: usize = 80;
					if line.len() < pct_dbtime_end {
						pct_dbtime_end = line.len();
					}
					pct_dbtime = f64::from_str(&line[73..pct_dbtime_end].trim().replace(",","")).unwrap();
				}
				if !is_idle(&statname) {
					fg.push(ForegroundWaitEvents { event: statname, waits: waits, total_wait_time_s: total_wait_time, avg_wait: avg_wait, pct_dbtime: pct_dbtime, begin_snap_time: "".to_string() , waitevent_histogram_ms: HashMap::new()})
				}
			}
		}
	}
	fg
}	



fn foreground_wait_events(table: ElementRef) -> Vec<ForegroundWaitEvents> {
	let mut foreground_wait_events: Vec<ForegroundWaitEvents> = Vec::new();
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
				foreground_wait_events.push(ForegroundWaitEvents { event: event.to_string(), waits: waits, total_wait_time_s: total_wait_time_s, avg_wait: avg_wait, pct_dbtime: pct_dbtime, begin_snap_time: "".to_string(), waitevent_histogram_ms: HashMap::new() })
			}
		}
	}

	foreground_wait_events	
}

fn time_model_stats_txt(time_model_section: Vec<&str>) -> Vec<TimeModelStats> {
	let mut time_model_stats: Vec<TimeModelStats> = Vec::new();
	for line in time_model_section {
		if line.len() >= 66 {
			let statname = line[0..35].to_string().trim().to_string();
			let time_s = f64::from_str(&line[35..56].trim().replace(",",""));
			let pct_dbtime = f64::from_str(&line[56..66].trim().replace(",",""));
			if time_s.is_ok() && pct_dbtime.is_ok() {
				time_model_stats.push(TimeModelStats{stat_name: statname.to_string(), time_s: time_s.unwrap(), pct_dbtime: pct_dbtime.unwrap(), begin_snap_time: "".to_string()});
			}
		}
		
	} 
	time_model_stats
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

			time_model_stats.push(TimeModelStats {stat_name: stat_name.to_string(), time_s: time_s, pct_dbtime: pct_dbtime, begin_snap_time: "".to_string()});
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

fn snap_info_txt(snap_section: Vec<&str>) -> SnapInfo {
	let mut si = SnapInfo::default();
	let fields_begin = snap_section[2].split_whitespace().collect::<Vec<&str>>();
	let fields_end = snap_section[3].split_whitespace().collect::<Vec<&str>>();
	let begin_snap = format!("{} {}", fields_begin[3], fields_begin[4]);
	let end_snap = format!("{} {}", fields_end[3], fields_end[4]);

	let begin_snap_id = format!("{}", fields_begin[2]);
	let end_snap_id = format!("{}", fields_end[2]);

	si.begin_snap_id = u32::from_str(&begin_snap_id).unwrap();
	si.end_snap_id = u32::from_str(&end_snap_id).unwrap();
	si.begin_snap_time = begin_snap;
	si.end_snap_time = end_snap;

	si
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
			let begin_end_snap_id = u32::from_str(&begin_end_snap_id[0]).unwrap_or(0);

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


fn instance_info(table: ElementRef) -> DBInstance {
	let mut dbi = DBInstance::default();
	let row_selector = Selector::parse("tr").unwrap();
    let column_selector = Selector::parse("td").unwrap();

	for row in table.select(&row_selector) {
        	let columns = row.select(&column_selector).collect::<Vec<_>>();
        	if columns.len() == 7 {
				let dbname = columns[0].text().collect::<Vec<_>>();
				let dbname = dbname[0].trim();

				let dbid = columns[1].text().collect::<Vec<_>>();
				let dbid = u64::from_str(&dbid[0].trim()).unwrap_or(0);

				let iname = columns[2].text().collect::<Vec<_>>();
				let iname = iname[0].trim();

				let inum = columns[3].text().collect::<Vec<_>>();
				let inum = u8::from_str(&inum[0].trim()).unwrap_or(0);

				let startup_time = columns[4].text().collect::<Vec<_>>();
				let startup_time = startup_time[0].trim();

				let release = columns[5].text().collect::<Vec<_>>();
				let release = release[0].trim();

				let israc = columns[6].text().collect::<Vec<_>>();
				let israc = israc[0].trim();

				dbi = DBInstance{db_name: dbname.to_string(), 
						db_id: dbid, 
						instance_name: iname.to_string(), 
						instance_num: inum, 
						startup_time: startup_time.to_string(), 
						release: release.to_string(), 
						rac: israc.to_string()};
		}
	}
	dbi
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
			lp.push(LoadProfile{stat_name: statname.to_string(), per_second: per_second, per_transaction: per_transaction, begin_snap_time: "".to_string()});
		}
		
	}
	lp
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

		lp.push(LoadProfile{stat_name: statname.to_string(), per_second: per_second, per_transaction: per_transaction, begin_snap_time: "".to_string()});
	}
    }
    lp
}

fn parse_awr_report_internal(fname: String) -> AWR {
	let mut awr: AWR = AWR::default();
	if fname.ends_with("html") {

		//println!("Parsing file {}", &fname);
		let html_file = fs::read_to_string(&fname);
		let html = html_file.unwrap();

		let doc = Html::parse_document(&html);
		let table_selector = Selector::parse("table").unwrap();
		let row_selector = Selector::parse("tr").unwrap();
		let column_selector = Selector::parse("td").unwrap();


		for element in doc.select(&table_selector) {
			if element.value().attr("summary").unwrap() == "This table displays load profile" {
				awr.load_profile = load_profile(element);
			} else if element.value().attr("summary").unwrap() == "This table displays database instance information" {
				awr.db_instance_information = instance_info(element);
			} else if element.value().attr("summary").unwrap() == "This table displays foreground wait class statistics" {
				awr.wait_classes = wait_classes(element);
			} else if element.value().attr("summary").unwrap() == "This table displays system load statistics" {
				awr.host_cpu = host_cpu(element);
			} else if element.value().attr("summary").unwrap() == "This table displays different time model statistics. For each statistic, time and % of DB time are displayed" {
				awr.time_model_stats = time_model_stats(element);
			} else if element.value().attr("summary").unwrap() == "This table displays Foreground Wait Events and their wait statistics" {
				awr.foreground_wait_events = foreground_wait_events(element);
			} else if element.value().attr("summary").unwrap() == "This table displays top SQL by elapsed time" {
				awr.sql_elapsed_time = sql_elapsed_time(element);
			} else if element.value().attr("summary").unwrap() == "This table displays top SQL by CPU time" {
				awr.sql_cpu_time = sql_cpu_time(element);
			} else if element.value().attr("summary").unwrap() == "This table displays top SQL by user I/O time" {
				awr.sql_io_time = sql_io_time(element);
			} else if element.value().attr("summary").unwrap() == "This table displays snapshot information" {
				awr.snap_info = snap_info(element);
			} else if element.value().attr("summary").unwrap() == "This table displays Instance activity statistics. For each instance, activity total, activity per second, and activity per transaction are displayed" {
				awr.key_instance_stats = instance_activity_stats(element);
			} else if element.value().attr("summary").unwrap() == "This table displays thread activity stats in the instance. For each activity , total number of activity and activity per hour are displayed" {
				awr.redo_log = redo_log_switches(element);
			} else if element.value().attr("summary").unwrap() == "This table displays total number of waits, and information about total wait time, for each wait event" {
				let event_histogram = waitevent_histogram_ms(element);
				if event_histogram.len() > 0 {
					for ev in awr.foreground_wait_events.iter_mut() {
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
 
		let mut snapshot_index = find_section_boundries(awr_lines.clone(), "Snapshot       Snap Id", "Cache Sizes");
		if snapshot_index.begin == 0 && snapshot_index.end == 0 {
			snapshot_index = find_section_boundries(awr_lines.clone(), "              Snap Id", "Top ADDM Findings");
		}
		let mut snap_info_lines: Vec<&str> = Vec::new();
		snap_info_lines.extend_from_slice(&awr_lines[snapshot_index.begin..snapshot_index.end]);
		awr.snap_info = snap_info_txt(snap_info_lines); 

		let host_cpu_section_start = format!("{}{}", 12u8 as char, "Host CPU");
		let host_cpu_index = find_section_boundries(awr_lines.clone(), &host_cpu_section_start, "Instance CPU");
		if host_cpu_index.begin != 0 && host_cpu_index.end != 0 {
    		let host_cpu_lines: Vec<&str> = awr_lines[host_cpu_index.begin..host_cpu_index.end+2].to_vec();
    		awr.host_cpu = host_cpu_txt(host_cpu_lines);
		}

		// Search for the line containing "log switches (derived)"
        if let Some(line) = awr_lines.iter().find(|&&line| line.contains("log switches (derived)")) {
            awr.redo_log = redo_log_switches_txt(line);
        }
		
		let load_profile_index = find_section_boundries(awr_lines.clone(), "Load Profile", "Instance Efficiency");
		let mut load_profile_lines: Vec<&str> = Vec::new();
		load_profile_lines.extend_from_slice(&awr_lines[load_profile_index.begin+2..load_profile_index.end]);
		awr.load_profile = load_profile_txt(load_profile_lines);

		let time_model_index = find_section_boundries(awr_lines.clone(), "Time Model", "DB time");
		let mut db_time_lines: Vec<&str> = Vec::new();
		db_time_lines.extend_from_slice(&awr_lines[time_model_index.begin+5..time_model_index.end]);
		awr.time_model_stats = time_model_stats_txt(db_time_lines);

		let foreground_even_section_start = format!("{}{}", 12u8 as char, "Foreground Wait Events");
		let foreground_event_index = find_section_boundries(awr_lines.clone(), &foreground_even_section_start, "Background Wait Events");
		let mut foreground_events: Vec<&str> = Vec::new();
		foreground_events.extend_from_slice(&awr_lines[foreground_event_index.begin+8..foreground_event_index.end-3]);
		awr.foreground_wait_events = foreground_events_txt(foreground_events);

		let sql_cpu_section_start = format!("{}{}", 12u8 as char, "SQL ordered by CPU");
		let sql_cpu_section_end = format!("{}{}", 12u8 as char, "SQL ordered by Elapsed time");
		let sql_cpu_index = find_section_boundries(awr_lines.clone(), &sql_cpu_section_start, &sql_cpu_section_end);
		let mut sql_cpu: Vec<&str> = Vec::new();
		sql_cpu.extend_from_slice(&awr_lines[sql_cpu_index.begin..sql_cpu_index.end]);
		awr.sql_cpu_time = sql_cpu_time_txt(sql_cpu);

		let sql_ela_section_start = format!("{}{}", 12u8 as char, "SQL ordered by Elapsed time");
		let sql_ela_section_end = format!("{}{}", 12u8 as char, "SQL ordered by Gets");
		let mut sql_ela_index = find_section_boundries(awr_lines.clone(), &sql_ela_section_start, &sql_ela_section_end);

		if sql_ela_index.begin == 0 || sql_ela_index.end == 0 {
			let sql_ela_section_start = format!("{}{}", 12u8 as char, "SQL ordered by Elapsed Time");
			let sql_ela_section_end = format!("{}{}", 12u8 as char, "SQL ordered by CPU Time");
			sql_ela_index = find_section_boundries(awr_lines.clone(), &sql_ela_section_start, &sql_ela_section_end);
		}

		let mut sql_ela: Vec<&str> = Vec::new();
		sql_ela.extend_from_slice(&awr_lines[sql_ela_index.begin..sql_ela_index.end]);
		awr.sql_elapsed_time = sql_ela_time_txt(sql_ela);

		let instance_activity_start = format!("{}{}", 12u8 as char, "Instance Activity Stats");
		let instance_activity_end = format!("{}{}", 12u8 as char, "workarea executions - optimal");
		let instance_act_index = find_section_boundries(awr_lines.clone(), &instance_activity_start, &instance_activity_end);
		let mut inst_stats: Vec<&str> = Vec::new();
		inst_stats.extend_from_slice(&awr_lines[instance_act_index.begin..instance_act_index.end+2]);
		awr.key_instance_stats = instance_activity_stats_txt(inst_stats);

		let mut event_names: HashMap<String, String> = HashMap::new();
		for ev in &awr.foreground_wait_events {
			if ev.event.len() >= 26 {
				event_names.insert(ev.event[0..26].to_string(), ev.event.clone());
			} else {
				event_names.insert(ev.event.to_string(), ev.event.clone());
			}
		}
		let event_histogram_start = format!("{}{}", 12u8 as char, "Wait Event Histogram");
		let event_histogram_end = format!("{}{}", 12u8 as char, "SQL ordered by");
		let event_histogram_index = find_section_boundries(awr_lines.clone(), &event_histogram_start, &event_histogram_end);
		let mut event_hist: Vec<&str> = Vec::new();
		event_hist.extend_from_slice(&awr_lines[event_histogram_index.begin..event_histogram_index.end+2]);
		let event_histogram = waitevent_histogram_ms_txt(event_hist, event_names);
		println!("{}", event_histogram.len());
		if event_histogram.len() > 0 {
			for ev in awr.foreground_wait_events.iter_mut() {
				if event_histogram.contains_key(&ev.event) {
					let histogram = event_histogram.get(&ev.event).unwrap().clone();
					ev.waitevent_histogram_ms = histogram;
				}
			}
		}
	}
	for lpi in 0..awr.load_profile.len() {
		awr.load_profile[lpi].begin_snap_time = awr.snap_info.begin_snap_time.clone();
	}
	for tmi in 0..awr.time_model_stats.len() {
		awr.time_model_stats[tmi].begin_snap_time = awr.snap_info.begin_snap_time.clone();
	}
	for fwi in 0..awr.foreground_wait_events.len() {
		awr.foreground_wait_events[fwi].begin_snap_time = awr.snap_info.begin_snap_time.clone();
	}
	awr.status = "OK".to_string();
	awr
}


pub fn parse_awr_dir(dirname: &str, plot: u8, db_time_cpu_ratio: f64, filter_db_time: f64) -> Result<String, std::io::Error> {
	
	let mut awrs: Vec<AWRS> = Vec::new();
	for file in fs::read_dir(dirname).unwrap() {
		let fname = &file.unwrap().path().display().to_string();
		if (fname.contains("awr") || fname.contains("sp_")) {
			let file_name = fname.split("/").collect::<Vec<&str>>();
			let file_name = file_name.last().unwrap().to_string();
			awrs.push(AWRS{file_name: file_name.clone(), awr_doc: parse_awr_report_internal(fname.to_string())});
		}
    }
	awrs.sort_by_key(|a| a.awr_doc.snap_info.begin_snap_id);
	let awr_doc: String = serde_json::to_string_pretty(&awrs).unwrap();
	if plot > 0 {
		let html_fname = format!("{}.html", dirname);
		plot_to_file(awrs, html_fname, db_time_cpu_ratio, filter_db_time);
	}
	Ok(awr_doc)
}

pub fn parse_awr_report(data: &str, json_data: bool) -> Result<String, std::io::Error> {
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
	let awr = parse_awr_report_internal(fname);
	
    let awr_doc: String = serde_json::to_string_pretty(&awr).unwrap();
	Ok(awr_doc)
}

pub fn prarse_json_file(fname: String, db_time_cpu_ratio: f64, filter_db_time: f64) {
	let json_file = fs::read_to_string(&fname).expect(&format!("Something wrong with a file {} ", fname));
	let mut awrs: Vec<AWRS> = serde_json::from_str(&json_file).expect("Wrong JSON format");
	let file_and_ext = fname.split(".").collect::<Vec<&str>>();
	let html_fname = format!("{}.html", file_and_ext[0]);
	awrs.sort_by_key(|a| a.awr_doc.snap_info.begin_snap_id);
	plot_to_file(awrs, html_fname, db_time_cpu_ratio, filter_db_time);
}
