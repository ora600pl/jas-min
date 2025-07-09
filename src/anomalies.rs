use std::collections::{HashMap, HashSet, BTreeMap};
use crate::awr::{WaitEvents, HostCPU, LoadProfile, SQLCPUTime, SQLIOTime, SQLGets, SQLReads, AWR, AWRSCollection};
use crate::Args;
use prettytable::{Table, Row, Cell, format, Attr};
use rayon::prelude::*;
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

fn get_dc_map_vectors(awrs: &Vec<AWR>) -> HashMap<String, Vec<f64>> {
    //Create list of all statistics
    let all_stats: HashSet<String> = awrs
                    .iter()
                    .flat_map(|awr| awr.dictionary_cache.iter())
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
                        .dictionary_cache
                        .iter()
                        .map(|l| (&l.statname, l.get_requests as f64))
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

fn get_libcache_map_vectors(awrs: &Vec<AWR>) -> HashMap<String, Vec<f64>> {
    //Create list of all statistics
    let all_stats: HashSet<String> = awrs
                    .iter()
                    .flat_map(|awr| awr.library_cache.iter())
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
                        .library_cache
                        .iter()
                        .map(|l| (&l.statname, l.pin_requests as f64))
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

fn get_latch_activity_map_vectors(awrs: &Vec<AWR>) -> HashMap<String, Vec<f64>> {
    //Create list of all statistics
    let all_stats: HashSet<String> = awrs
                    .iter()
                    .flat_map(|awr| awr.latch_activity.iter())
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
                        .latch_activity
                        .iter()
                        .map(|l| (&l.statname, l.get_requests as f64))
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

fn detect_anomalies_mad_sliding(awrs: &Vec<AWR>, stats_vector: &HashMap<String, Vec<f64>>,  args: &Args) -> HashMap<String, Vec<(String,f64)>> {
    let mut anomalies: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    //                          event        date   mad => for each event it will collect date of anomaly and value of MAD
    
    //if window is 100% don't use sliding window alghorithm - use normal detection for better performance
    if args.mad_window_size == 100 {
        anomalies = detect_anomalies_mad(awrs, stats_vector, args);
        return anomalies;
    }
    
    let threshold = args.mad_threshold;
    let len = awrs.len();
    let mut full_window_size = ((args.mad_window_size as f32 / 100.0 ) * len as f32) as usize;
    if full_window_size % 2 == 1 {
        full_window_size = full_window_size + 1;
    }
    let half_window_size = full_window_size / 2;

    //For sliding window there will parallel processing using rayon - Global Thread Pool is configured in main.rs
    anomalies = stats_vector
        .par_iter() //parallel iteration
        .map(|(stat_name, values)| { //each thread will process one statistic
            let mut local_anomalies = Vec::new();
            for (i, &val) in values.iter().enumerate() { //For the given statistic process vector values of each snap and define local window
                
                /* Define boundries for the window  */
                let start = if i >= half_window_size {
                                        i - half_window_size
                                    } else {
                                            0
                                    };

                let end = if start + full_window_size <= len {
                                        start + full_window_size
                                } else {
                                    len
                                };
                /* ********************************** */

                let window = &values[start..end]; //local surrounding window

                let local_median = median(window); 
                let local_mad = mad(window, local_median);

                if local_mad == 0.0 {
                    continue; // no scatter - ignore
                }

                let val_mad_check = ((val - local_median).abs()) / local_mad;

                //if anomaly is bigger than threshold - put event name on index corresponding to detected anomaly
                if val_mad_check > threshold && val >= 0.0 { //Don't take into considaration negative values that are placeholders
                    let snap_date = awrs[i].snap_info.begin_snap_time.clone();
                    local_anomalies.push((snap_date, val_mad_check)); //put in vector date of anomalie and value of MAD
                } 
            }
            (stat_name.clone(), local_anomalies) //return statistic name and anomalies
        }).filter(|(_, v)| {!v.is_empty()}) //filter out statistics with empty vectors - it means that no anomalie was detected for this stat
        .collect();

    anomalies    
    
}

fn detect_anomalies_mad(awrs: &Vec<AWR>, stats_vector: &HashMap<String, Vec<f64>>,  args: &Args) -> HashMap<String, Vec<(String,f64)>> {
    let mut anomalies: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    //                          event        date   mad => for each event it will collect date of anomaly and value of MAD
    let threshold = args.mad_threshold;

    for (stat_name, values) in stats_vector {
         let med = median(values);
         let mad_val = mad(values, med);

        if mad_val == 0.0 {
            continue; // no nomalies - just move on
        }

        for (i, &val) in values.iter().enumerate() {
            let val_mad_check = ((val - med).abs()) / mad_val ;

            //if anomaly is bigger than threshold - put event name on index corresponding to detected anomaly
            if val_mad_check > threshold && val >= 0.0 { //Don't take into considaration negative values that are placeholders
                let snap_date = awrs[i].snap_info.begin_snap_time.clone();
                if let Some(a) = anomalies.get_mut(stat_name) {
                    a.push((snap_date, val_mad_check));
                } else {
                    anomalies.insert(stat_name.to_string(), vec![(snap_date, val_mad_check)]);
                }
            } 
        }
    } 

    anomalies
}

//Median Absolute Deviation for anomalies detection in wait events
pub fn detect_event_anomalies_mad(awrs: &Vec<AWR>, args: &Args, bg_or_fg: &str) -> HashMap<String, Vec<(String,f64)>> {
    let event_map_vectors = get_event_map_vectors(awrs, bg_or_fg);
    //println!("Detecting event anomalies");
    let anomalies = detect_anomalies_mad_sliding(awrs, &event_map_vectors, args);
    //println!("Detected event anomalies");
    anomalies
}


//Median Absolute Deviation for anomalies detection in SQLs
pub fn detect_sql_anomalies_mad(awrs: &Vec<AWR>, args: &Args, sql_type: &str) -> HashMap<String, Vec<(String,f64)>> {
    
    let sql_map_vectors = get_sql_map_vectors(awrs, sql_type);
    let anomalies = detect_anomalies_mad_sliding(awrs, &sql_map_vectors, args);
    
    anomalies
}



//Median Absolute Deviation for anomalies detection in Load Profile
pub fn detect_loadprofile_anomalies_mad(awrs: &Vec<AWR>, args: &Args) -> HashMap<String, Vec<(String,f64)>> {
    let loadprofile_map_vectors = get_loadprofile_map_vectors(awrs);
    let anomalies = detect_anomalies_mad_sliding(awrs, &loadprofile_map_vectors, args);
    
    anomalies
}

//Median Absolute Deviation for anomalies detection in Instance Statistics
pub fn detect_stats_anomalies_mad(awrs: &Vec<AWR>, args: &Args) -> HashMap<String, Vec<(String,f64)>> {
    let stats_map_vectors = get_statistics_map_vectors(awrs);    
    let anomalies = detect_anomalies_mad_sliding(awrs, &stats_map_vectors, args);

    anomalies
}

//Median Absolute Deviation for anomalies detection in Dictionary Cache stats
pub fn detect_dc_anomalies_mad(awrs: &Vec<AWR>, args: &Args) -> HashMap<String, Vec<(String,f64)>> {
    let stats_map_vectors = get_dc_map_vectors(awrs);    
    let anomalies = detect_anomalies_mad_sliding(awrs, &stats_map_vectors, args);

    anomalies
}

//Median Absolute Deviation for anomalies detection in Library Cache stats
pub fn detect_libcache_anomalies_mad(awrs: &Vec<AWR>, args: &Args) -> HashMap<String, Vec<(String,f64)>> {
    let stats_map_vectors = get_libcache_map_vectors(awrs);    
    let anomalies = detect_anomalies_mad_sliding(awrs, &stats_map_vectors, args);

    anomalies
}

//Median Absolute Deviation for anomalies detection in Latch Activity stats
pub fn detect_latch_activity_anomalies_mad(awrs: &Vec<AWR>, args: &Args) -> HashMap<String, Vec<(String,f64)>> {
    let stats_map_vectors = get_latch_activity_map_vectors(awrs);    
    let anomalies = detect_anomalies_mad_sliding(awrs, &stats_map_vectors, args);

    anomalies
}

pub fn anomalies_join(
    anomalies_summary: &mut BTreeMap<(u64, String), BTreeMap<String, Vec<String>>>,
    key: (u64, String),
    anomaly_type: &str,
    anomaly_detail: impl Into<String>,
    ){
    let inner_map = anomalies_summary.entry(key).or_insert_with(BTreeMap::new);
    inner_map
        .entry(anomaly_type.to_string())
        .or_insert_with(Vec::new)
        .push(anomaly_detail.into());
}

pub fn report_anomalies_summary(anomalies_summary: &mut BTreeMap<(u64, String), BTreeMap<String, Vec<String>>>, args: &Args, logfile_name: &str) -> String {
    
    let mut table = Table::new();
    table.set_titles(Row::new(vec![
        Cell::new("BEGIN SNAP ID"),
        Cell::new("BEGIN SNAP DATE"),
        Cell::new("Anomaly Summary"),
        Cell::new("Count"),
    ]));

    let mut html_table: String = String::new();

    anomalies_summary.iter().for_each(|((snap_id, snap_date), anomalies_map)| {
        let mut all_lines: Vec<String> = Vec::new();

        for (anomaly_type, details) in anomalies_map {
            for detail in details {
                all_lines.push(format!("{}: {}", anomaly_type, detail));
            }
        }

        let c_begin_snap_id = Cell::new(&snap_id.to_string());
        let c_begin_snap_date = Cell::new(snap_date);
        let c_anomalie_details = Cell::new(&all_lines.join("\n"));
        let c_anomalie_count = Cell::new(&all_lines.len().to_string());

        table.add_row(Row::new(vec![
            c_begin_snap_id,
            c_begin_snap_date,
            c_anomalie_details,
            c_anomalie_count,
        ]));

        html_table.push_str(&format!(
            r#"<tr>
                <td>{}</td>
                <td>{}</td>
                <td style="text-align: left;">{}</td>
                <td>{}</td>
            </tr>"#,
            snap_id,
            snap_date,
            all_lines.join("<br>"),
            all_lines.len()
        ));
    });

    let anomalies_txt = format!(
        "Anomalies summary for each date from all sections where anomaly was detected"
    )
    .blue()
    .underline();

    make_notes!(logfile_name, args.quiet, "\n\n{}\n", anomalies_txt);
    for table_line in table.to_string().lines() {
        make_notes!(logfile_name, args.quiet, "{}\n", table_line);
    }

    html_table
}