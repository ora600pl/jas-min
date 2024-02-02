use std::clone;
use std::env;
use std::fs;
use std::str;
use std::result;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::collections::HashMap;
use std::char;


#[derive(Default,Serialize, Deserialize, Debug)]
struct LoadProfile {
	stat_name: String,
	per_second: f64,
	per_transaction: f64,
	begin_snap_time: String,
}

#[derive(Default,Serialize, Deserialize, Debug)]
struct DBInstance {
	db_name: String,
	db_id: u64,
	instance_name: String, 
	instance_num: u8,
	startup_time: String,
	release: String,
	rac: String,
}

#[derive(Default,Serialize, Deserialize, Debug)]
struct WaitClasses {
	wait_class: String,
	waits: u64,
	total_wait_time_s: u64,
	avg_wait_ms: f64,
	db_time_pct: f64,
}

#[derive(Default,Serialize, Deserialize, Debug)]
struct HostCPU {
	cpus: u32,
	cores: u32,
	sockets: u8,
	load_avg_begin: f64,
	load_avg_end: f64,
	pct_user: f64,
	pct_system: f64,
	pct_wio: f64,
	pct_idle: f64,
}

#[derive(Default,Serialize, Deserialize, Debug)]
struct TimeModelStats {
	stat_name: String,
	time_s: f64,
	pct_dbtime: f64,
	begin_snap_time: String,
}

#[derive(Default,Serialize, Deserialize, Debug)]
struct ForegroundWaitEvents {
	event: String,
	waits: u64,
	total_wait_time_s: u64,
	avg_wait: f64,
	pct_dbtime: f64,
	begin_snap_time: String,
}

#[derive(Default,Serialize, Deserialize, Debug)]
struct SQLElapsedTime {
	sql_id: String,
	elapsed_time_s: f64,
	executions: u64,
	elpased_time_exec_s: f64,
	pct_total: f64,
	pct_cpu: f64, 
	pct_io: f64,
	sql_module: String,
}

#[derive(Default,Serialize, Deserialize, Debug)]
struct SQLCPUTime {
	sql_id: String,
	cpu_time_s: f64,
	executions: u64,
	cpu_time_exec_s: f64,
	pct_total: f64,
	pct_cpu: f64, 
	pct_io: f64,
	sql_module: String,
}

#[derive(Default,Serialize, Deserialize, Debug)]
struct SQLIOTime {
	sql_id: String,
	io_time_s: f64,
	executions: u64,
	io_time_exec_s: f64,
	pct_total: f64,
	pct_cpu: f64, 
	pct_io: f64,
	sql_module: String,
}

#[derive(Default,Serialize, Deserialize, Debug)]
struct SnapInfo {
	begin_snap_id: u32,
	end_snap_id: u32,
	begin_snap_time: String,
	end_snap_time: String,
}

#[derive(Default,Serialize, Deserialize, Debug)]
struct KeyInstanceStats {
	statname: String,
	total: u64,
}

#[derive(Default,Serialize, Deserialize, Debug)]
struct AWR {
	status: String,
	load_profile: Vec<LoadProfile>,
	db_instance_information: DBInstance,
	wait_classes: Vec<WaitClasses>,
	host_cpu: HostCPU,
	time_model_stats: Vec<TimeModelStats>,
	foreground_wait_events: Vec<ForegroundWaitEvents>,
	sql_elapsed_time: Vec<SQLElapsedTime>,
	sql_cpu_time: Vec<SQLCPUTime>,
	sql_io_time: Vec<SQLIOTime>,
	key_instance_stats: Vec<KeyInstanceStats>,
	snap_info: SnapInfo,
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

fn foreground_events_txt(foreground_events_section: Vec<&str>) -> Vec<ForegroundWaitEvents> {
	let mut fg: Vec<ForegroundWaitEvents> = Vec::new();
	for line in foreground_events_section {
		//println!("{}", line);
		if line.len() >= 73 {
			let statname = line[0..28].to_string().trim().to_string();
			let waits = u64::from_str(&line[29..41].trim().replace(",",""));

			if waits.is_ok() {

				let waits: u64 = waits.unwrap_or(0);
				let mut total_wait_time = u64::from_str(&line[46..57].trim().replace(",","")).unwrap_or(0);
				if total_wait_time == 0 {
					total_wait_time = u64::from_str(&line[38..54].trim().replace(",","")).unwrap_or(0);
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
				fg.push(ForegroundWaitEvents { event: statname, waits: waits, total_wait_time_s: total_wait_time, avg_wait: avg_wait, pct_dbtime: pct_dbtime, begin_snap_time: "".to_string() })
			
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
			let total_wait_time_s = u64::from_str(&total_wait_time_s[0].trim().replace(",","")).unwrap_or(0);

			let avg_wait = columns[4].text().collect::<Vec<_>>();
			let avg_wait = f64::from_str(&avg_wait[0].trim().replace(",","")).unwrap_or(0.0);

			let pct_dbtime = columns[6].text().collect::<Vec<_>>();
			let pct_dbtime = f64::from_str(&pct_dbtime[0].trim().replace(",","")).unwrap_or(0.0);

			foreground_wait_events.push(ForegroundWaitEvents { event: event.to_string(), waits: waits, total_wait_time_s: total_wait_time_s, avg_wait: avg_wait, pct_dbtime: pct_dbtime, begin_snap_time: "".to_string() })
			
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
				let total_wait_time = u64::from_str(&total_wait_time[0].trim().replace(",","")).unwrap_or(0);

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
			} else if element.value().attr("summary").unwrap() == "This table displays Key Instance activity statistics. For each instance, activity total, activity per second, and activity per transaction are displayed" {
				awr.key_instance_stats = instance_activity_stats(element);
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
		let sql_ela_index = find_section_boundries(awr_lines.clone(), &sql_ela_section_start, &sql_ela_section_end);
		let mut sql_ela: Vec<&str> = Vec::new();
		sql_ela.extend_from_slice(&awr_lines[sql_ela_index.begin..sql_ela_index.end]);
		awr.sql_elapsed_time = sql_ela_time_txt(sql_ela);

		let instance_activity_start = format!("{}{}", 12u8 as char, "Instance Activity Stats");
		let instance_activity_end = format!("{}{}", 12u8 as char, "workarea executions - optimal");
		let instance_act_index = find_section_boundries(awr_lines.clone(), &instance_activity_start, &instance_activity_end);
		let mut inst_stats: Vec<&str> = Vec::new();
		inst_stats.extend_from_slice(&awr_lines[instance_act_index.begin..instance_act_index.end+2]);
		awr.key_instance_stats = instance_activity_stats_txt(inst_stats);


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


pub fn parse_awr_dir(dirname: &str) -> Result<String, std::io::Error> {
	#[derive(Default,Serialize, Deserialize, Debug)]
	struct AWRS {
		file_name: String,
		awr_doc: AWR,
	}
	let mut awrs: Vec<AWRS> = Vec::new();
	for file in fs::read_dir(dirname).unwrap() {
		let fname = &file.unwrap().path().display().to_string();
		if (fname.contains("awr") || fname.contains("sp_")) {
			//println!("{}", fname);
			awrs.push(AWRS{file_name: fname.clone(), awr_doc: parse_awr_report_internal(fname.to_string())});
		}
    }
	let awr_doc: String = serde_json::to_string_pretty(&awrs).unwrap();
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
