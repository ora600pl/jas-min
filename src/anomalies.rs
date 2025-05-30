use std::collections::{HashMap, HashSet, BTreeMap};
use crate::awr::{WaitEvents, HostCPU, LoadProfile, SQLCPUTime, SQLIOTime, SQLGets, SQLReads, AWR, AWRSCollection};
use crate::Args;
use prettytable::{Table, Row, Cell, format, Attr};
use crate::make_notes;
use colored::*;

fn median(data: &[f64]) -> f64 {
    let mut sorted = data.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let len = sorted.len();
    if len == 0 {
        return 0.0;
    }
    if len % 2 == 0 {
        (sorted[len / 2 - 1] + sorted[len / 2]) / 2.0
    } else {
        sorted[len / 2]
    }
}

fn mad(data: &[f64], med: f64) -> f64 {
    let deviations: Vec<f64> = data.iter().map(|x| (x - med).abs()).collect();
    median(&deviations)
}

fn get_event_map_vectors(awrs: &Vec<AWR>, bg_or_fg: &str) -> HashMap<String, Vec<f64>> {
    //Create list of all events
    let mut all_events: HashSet<String> = HashSet::new();
    if bg_or_fg == "FOREGROUND" {
        all_events = awrs
                    .iter()
                    .flat_map(|awr| awr.foreground_wait_events.iter())
                    .map(|e| e.event.clone())
                    .collect();
    } else if bg_or_fg == "BACKGROUND" {
        all_events = awrs
                    .iter()
                    .flat_map(|awr| awr.background_wait_events.iter())
                    .map(|e| e.event.clone())
                    .collect();
    }

    //This will hold event name and vector of values filled with 0.0 as default value
    let mut event_map: HashMap<String, Vec<f64>> = all_events
                                                    .iter()
                                                    .map(|e| (e.clone(), vec![-1.0; awrs.len()]))
                                                    .collect();

    //we are iterating over AWR
    for (i, awr) in awrs.iter().enumerate() {
        let mut snapshot_map: HashMap<&String, f64> = HashMap::new();

        if bg_or_fg == "FOREGROUND" {
        //Let's create a HashMap from all foreground and background events
            snapshot_map = awr
                            .foreground_wait_events
                            .iter()
                            .map(|e| (&e.event, e.total_wait_time_s))
                            .collect();
        } else if bg_or_fg == "BACKGROUND" {
            snapshot_map = awr
                            .background_wait_events
                            .iter()
                            .map(|e| (&e.event, e.total_wait_time_s))
                            .collect();
        }

        //Let's go through all of the event names
        for event in &all_events {
            //If some event name exists in this snapshot, set actual value in the map, instead of -1.0
            if let Some(&val) = snapshot_map.get(event) {
                event_map.get_mut(event).unwrap()[i] = val;
            }
        }
    }
    event_map
}

fn get_sql_map_vectors(awrs: &Vec<AWR>, sql_type: &str) -> HashMap<String, Vec<f64>> {
    //Create list of all SQLs
    let mut all_sqls: HashSet<String> = HashSet::new();
    if sql_type == "ELAPSED_TIME" {
        all_sqls = awrs
                    .iter()
                    .flat_map(|awr| awr.sql_elapsed_time.iter())
                    .map(|s| s.sql_id.clone())
                    .collect();
    } 

    //This will hold SQL_ID and vector of values filled with -1.0 as default value
    let mut sql_map: HashMap<String, Vec<f64>> = all_sqls
                                                    .iter()
                                                    .map(|e| (e.clone(), vec![-1.0; awrs.len()]))
                                                    .collect();

    //we are iterating over AWR
    for (i, awr) in awrs.iter().enumerate() {
        let mut snapshot_map: HashMap<&String, f64> = HashMap::new();

        if sql_type == "ELAPSED_TIME" {
        //Let's create a HashMap from all foreground and background events
            snapshot_map = awr
                            .sql_elapsed_time
                            .iter()
                            .map(|s| (&s.sql_id, s.elapsed_time_s))
                            .collect();
        } 

        //Let's go through all of the event names
        for sql in &all_sqls {
            //If some event name exists in this snapshot, set actual value in the map, instead of -1.0
            if let Some(&val) = snapshot_map.get(sql) {
                sql_map.get_mut(sql).unwrap()[i] = val;
            }
        }
    }
    sql_map
}

fn get_loadprofile_map_vectors(awrs: &Vec<AWR>) -> HashMap<String, Vec<f64>> {
    //Create list of all SQLs
    let all_loadprofile: HashSet<String> = awrs
                    .iter()
                    .flat_map(|awr| awr.load_profile.iter())
                    .map(|l| l.stat_name.clone())
                    .collect();
    

    //This will hold load profile stat name and vector of values filled with -1.0 as default value
    let mut profile_map: HashMap<String, Vec<f64>> = all_loadprofile
                                                    .iter()
                                                    .map(|e| (e.clone(), vec![-1.0; awrs.len()]))
                                                    .collect();

    //we are iterating over AWR
    for (i, awr) in awrs.iter().enumerate() {
        let mut snapshot_map: HashMap<&String, f64> = HashMap::new();

        
        snapshot_map = awr
                        .load_profile
                        .iter()
                        .map(|l| (&l.stat_name, l.per_second))
                        .collect();
        

        //Let's go through all of the load profile stats
        for l in &all_loadprofile {
            //If some event name exists in this snapshot, set actual value in the map, instead of -1.0
            if let Some(&val) = snapshot_map.get(l) {
                profile_map.get_mut(l).unwrap()[i] = val;
            }
        }
    }
    profile_map
}

fn get_statistics_map_vectors(awrs: &Vec<AWR>) -> HashMap<String, Vec<f64>> {
    //Create list of all statistics
    let all_stats: HashSet<String> = awrs
                    .iter()
                    .flat_map(|awr| awr.key_instance_stats.iter())
                    .map(|l| l.statname.clone())
                    .collect();
    

    //This will hold stat name and vector of values filled with -1.0 as default value
    let mut stats_map: HashMap<String, Vec<f64>> = all_stats
                                                    .iter()
                                                    .map(|e| (e.clone(), vec![-1.0; awrs.len()]))
                                                    .collect();

    //we are iterating over AWR
    for (i, awr) in awrs.iter().enumerate() {
        let mut snapshot_map: HashMap<&String, f64> = HashMap::new();

        snapshot_map = awr
                        .key_instance_stats
                        .iter()
                        .map(|l| (&l.statname, l.total as f64))
                        .collect();
        

        //Let's go through all of the instance stats
        for l in &all_stats {
            //If some stat exists in this snapshot, set actual value in the map, instead of -1.0
            if let Some(&val) = snapshot_map.get(l) {
                stats_map.get_mut(l).unwrap()[i] = val;
            }
        }
    }
    stats_map
}

//Median Absolute Deviation for anomalies detection in wait events
pub fn detect_event_anomalies_mad(awrs: &Vec<AWR>, args: &Args, bg_or_fg: &str) -> HashMap<String, Vec<(String,f64)>> {
    //let mut anomalies: BTreeMap<usize, Vec<(f64,String)>> = BTreeMap::new();
    let mut anomalies: HashMap<String, Vec<(String,f64)>> = HashMap::new();
    //                          event        date   mad => for each event it will collect date of anomaly and value of MAD
    let event_map_vectors = get_event_map_vectors(awrs, bg_or_fg);
    
    let threshold = args.mad_threshold; //Default threshold for MAD 

    for (event, values) in &event_map_vectors {
        let med = median(values);
        let mad_val = mad(values, med);

        if mad_val == 0.0 {
            continue; // no nomalies - just move on
        }

        for (i, &val) in values.iter().enumerate() {
            //if anomaly is bigger than threshold - put event name on index corresponding to detected anomaly
            let val_mad_check = ((val - med).abs()) / mad_val ;
            if val_mad_check > threshold && val >= 0.0 { //Don't take into considaration negative values that are placeholders
                let snap_date = awrs[i].snap_info.begin_snap_time.clone();
                if let Some(a) = anomalies.get_mut(event) {
                    a.push((snap_date, val_mad_check));
                } else {
                    anomalies.insert(event.to_string(), vec![(snap_date, val_mad_check)]);
                }
            } 
        }
    }

    anomalies
}


//Median Absolute Deviation for anomalies detection in SQLs
pub fn detect_sql_anomalies_mad(awrs: &Vec<AWR>, args: &Args, sql_type: &str) -> HashMap<String, Vec<(String,f64)>> {
    //let mut anomalies: BTreeMap<usize, Vec<(f64,String)>> = BTreeMap::new();
    let mut anomalies: HashMap<String, Vec<(String,f64)>> = HashMap::new();
    //                          sql_id        date   mad => for each SQL it will collect date of anomaly and value of MAD
    let sql_map_vectors = get_sql_map_vectors(awrs, sql_type);
    
    let threshold = args.mad_threshold; //Default threshold for MAD 

    for (sql, values) in &sql_map_vectors {
        let med = median(values);
        let mad_val = mad(values, med);

        if mad_val == 0.0 {
            continue; // no nomalies - just move on
        }

        for (i, &val) in values.iter().enumerate() {
            //if anomaly is bigger than threshold - put event name on index corresponding to detected anomaly
            let val_mad_check = ((val - med).abs()) / mad_val ;
            if val_mad_check > threshold && val >= 0.0 { //Don't take into considaration negative values that are placeholders
                let snap_date = awrs[i].snap_info.begin_snap_time.clone();
                if let Some(a) = anomalies.get_mut(sql) {
                    a.push((snap_date, val_mad_check));
                } else {
                    anomalies.insert(sql.to_string(), vec![(snap_date, val_mad_check)]);
                }
            } 
        }
    }

    anomalies
}



//Median Absolute Deviation for anomalies detection in Load Profile
pub fn detect_loadprofile_anomalies_mad(awrs: &Vec<AWR>, args: &Args) -> HashMap<String, Vec<(String,f64)>> {
    //let mut anomalies: BTreeMap<usize, Vec<(f64,String)>> = BTreeMap::new();
    let mut anomalies: HashMap<String, Vec<(String,f64)>> = HashMap::new();
    //                          stat_name    date   mad => for each Load Profile Stat it will collect date of anomaly and value of MAD
    let loadprofile_map_vectors = get_loadprofile_map_vectors(awrs);
    
    let threshold = args.mad_threshold; //Default threshold for MAD 

    for (l, values) in &loadprofile_map_vectors {
        let med = median(values);
        let mad_val = mad(values, med);

        if mad_val == 0.0 {
            continue; // no nomalies - just move on
        }

        for (i, &val) in values.iter().enumerate() {
            //if anomaly is bigger than threshold - put event name on index corresponding to detected anomaly
            let val_mad_check = ((val - med).abs()) / mad_val ;
            if val_mad_check > threshold && val >= 0.0 { //Don't take into considaration negative values that are placeholders
                let snap_date = awrs[i].snap_info.begin_snap_time.clone();
                if let Some(a) = anomalies.get_mut(l) {
                    a.push((snap_date, val_mad_check));
                } else {
                    anomalies.insert(l.to_string(), vec![(snap_date, val_mad_check)]);
                }
            } 
        }
    }

    anomalies
}

//Median Absolute Deviation for anomalies detection in Load Profile
pub fn detect_stats_anomalies_mad(awrs: &Vec<AWR>, args: &Args) -> HashMap<String, Vec<(String,f64)>> {
    //let mut anomalies: BTreeMap<usize, Vec<(f64,String)>> = BTreeMap::new();
    let mut anomalies: HashMap<String, Vec<(String,f64)>> = HashMap::new();
    //                          stat_name    date   mad => for each Stat it will collect date of anomaly and value of MAD
    let stats_map_vectors = get_statistics_map_vectors(awrs);
    
    let threshold = args.mad_threshold * 2.0; //Default threshold for MAD - For statistics I'm doubling the threshold to focus only on realy big anomalies.

    for (l, values) in &stats_map_vectors {
        let med = median(values);
        let mad_val = mad(values, med);

        if mad_val == 0.0 {
            continue; // no nomalies - just move on
        }

        for (i, &val) in values.iter().enumerate() {
            //if anomaly is bigger than threshold - put event name on index corresponding to detected anomaly
            let val_mad_check = ((val - med).abs()) / mad_val ;
            if val_mad_check > threshold && val >= 0.0 { //Don't take into considaration negative values that are placeholders
                let snap_date = awrs[i].snap_info.begin_snap_time.clone();
                if let Some(a) = anomalies.get_mut(l) {
                    a.push((snap_date, val_mad_check));
                } else {
                    anomalies.insert(l.to_string(), vec![(snap_date, val_mad_check)]);
                }
            } 
        }
    }

    anomalies
}

pub fn anomalies_join(anomalies_summary: &mut BTreeMap<(u64, String), Vec<String>>, key: (u64, String), anomalie: String) {
    if let Some(a) = anomalies_summary.get_mut(&key) {
        a.push(anomalie);
    } else {
        if !anomalie.starts_with("STAT:") {
            anomalies_summary.insert(key.clone(), vec![anomalie.clone()]);
        }
    }

}

pub fn report_anomalies_summary(anomalies_summary: BTreeMap<(u64, String), Vec<String>>, args: &Args, logfile_name: &str) {
    
    let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("BEGIN SNAP ID"),
                Cell::new("BEGIN SNAP DATE"),
                Cell::new("Anomalie summary"),
                Cell::new("Count")
            ]));

    anomalies_summary.iter().for_each(|a| {
        let c_begin_snap_id = Cell::new(&format!("{}", a.0.0));
        let c_begin_snap_date = Cell::new(&format!("{}", a.0.1));
        let anomalie_details: String = a.1.join("\n");
        let c_anomalie_details = Cell::new(&anomalie_details);
        let c_anomalie_count = Cell::new(&format!("{}", a.1.iter().count()));
        table.add_row(Row::new(vec![c_begin_snap_id, c_begin_snap_date, c_anomalie_details, c_anomalie_count]));
    });

    let anomalies_txt = format!("Anomalies summary for each date from all secions where anomalie was detected").blue().underline();
    make_notes!(logfile_name, args.quiet, "\n\n{}\n", anomalies_txt);
    for table_line in table.to_string().lines() {
        make_notes!(logfile_name, args.quiet, "{}\n", table_line);
    }
}