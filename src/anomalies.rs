use std::collections::{HashMap, HashSet, BTreeMap};
use std::path::Path;
use std::fs::File;
use std::io::{self, Write};
use crate::awr::{WaitEvents, HostCPU, LoadProfile, SQLCPUTime, SQLIOTime, SQLGets, SQLReads, AWR, AWRSCollection};
use crate::Args;
use prettytable::{Table, Row, Cell, format, Attr};
use rayon::prelude::*;
use crate::make_notes;
use colored::*;
use open::*; 
use crate::tools::*; 
use crate::reasonings::{StatisticsDescription,TopPeaksSelected,MadAnomaliesEvents,MadAnomaliesSQL,TopForegroundWaitEvents,TopBackgroundWaitEvents,PctOfTimesThisSQLFoundInOtherTopSections,WaitEventsWithStrongCorrelation,WaitEventsFromASH,TopSQLsByElapsedTime,StatsSummary,IOStatsByFunctionSummary,LatchActivitySummary,Top10SegmentStats,InstanceStatisticCorrelation,LoadProfileAnomalies,AnomalyDescription,AnomlyCluster,ReportForAI,AppState};


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
                    .flat_map(|awr| awr.instance_stats.iter())
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
                        .instance_stats
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

fn get_time_model_map_vectors(awrs: &Vec<AWR>) -> HashMap<String, Vec<f64>> {
    //Create list of all statistics
    let all_stats: HashSet<String> = awrs
                    .iter()
                    .flat_map(|awr| awr.time_model_stats.iter())
                    .map(|l| l.stat_name.clone())
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
                        .time_model_stats
                        .iter()
                        .map(|l| (&l.stat_name, l.time_s as f64))
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

//Median Absolute Deviation for anomalies detection in Time Model stats
pub fn detect_time_model_anomalies_mad(awrs: &Vec<AWR>, args: &Args) -> HashMap<String, Vec<(String,f64)>> {
    let stats_map_vectors = get_time_model_map_vectors(awrs);    
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

pub fn save_anomalies_to_csv(
    anomalies_summary: &BTreeMap<(u64, String), BTreeMap<String, Vec<String>>>,
    output_dir: impl AsRef<Path>,
    ) -> io::Result<()> {

    /*
     Saves anomalies summary data to CSV files.
     
     Creates:
     1. A summary CSV with snap_id, snap_date, and total count
     2. Individual detailed CSVs for each snap_id with full anomaly information
    
     # Arguments
     * `anomalies_summary` - The map containing anomaly data
     * `output_dir` - Directory where CSV files will be saved
    
     # Returns
     Result indicating success or error
     */

    let output_dir = output_dir.as_ref();
    
    // Path to subdirectory for anomalies CSV files
    let dirpath = output_dir.with_extension("html_reports").join("jasmin/anomalies");

    // Create the subdirectory if it doesn't exist
    std::fs::create_dir_all(&dirpath)?;

    // Save the summary CSV file
    save_summary_csv(anomalies_summary, &dirpath)?;

    // Save individual detailed CSV files for each snap_id
    save_detailed_csv_files(anomalies_summary, &dirpath)?;
    println!("Detailed CSV files for {} anomalies saved successfully",anomalies_summary.len());

    Ok(())
}


fn save_summary_csv(
    anomalies_summary: &BTreeMap<(u64, String), BTreeMap<String, Vec<String>>>,
    output_dir: &Path,
    ) -> io::Result<()> {
    ///
    /// Saves a summary CSV containing snap_id, snap_date, and total anomaly count
    /// 
    let summary_path = output_dir.join("anomalies_reference.csv");
    let mut file = File::create(summary_path)?;

    // Write CSV header
    writeln!(file, "BEGIN_SNAP_ID,BEGIN_SNAP_DATE,COUNT")?;

    // Write data rows
    for ((snap_id, snap_date), anomalies_map) in anomalies_summary {
        // Calculate total count of anomalies for this snapshot
        let total_count: usize = anomalies_map
            .values()
            .map(|details| details.len())
            .sum();

        writeln!(file, "{},{},{}", snap_id, snap_date, total_count)?;
    }

    Ok(())
}


fn save_detailed_csv_files(
    anomalies_summary: &BTreeMap<(u64, String), BTreeMap<String, Vec<String>>>,
    output_dir: &Path,
    ) -> io::Result<()> {
    ///
    /// Saves detailed CSV files, one per snap_id, containing all anomaly details
    /// 
    for ((snap_id, snap_date), anomalies_map) in anomalies_summary {
        let filename = format!("{}.csv", snap_id);
        let detail_path = output_dir.join(filename);
        let mut file = File::create(detail_path)?;

        // Write CSV header
        writeln!(file, "BEGIN_SNAP_ID,BEGIN_SNAP_DATE,COUNT,ANOMALY_SUMMARY")?;

        // Collect all anomaly lines for this snapshot
        let mut anomaly_lines: Vec<String> = Vec::new();
        for (anomaly_type, details) in anomalies_map {
            for detail in details {
                // Escape the detail string for CSV format
                let escaped_detail = escape_csv_field(&format!("{}: {}", anomaly_type, detail));
                anomaly_lines.push(escaped_detail);
            }
        }

        // Write one row per anomaly
        for anomaly_line in &anomaly_lines {
            writeln!(
                file,
                "{},{},{},{}",
                snap_id,
                snap_date,
                anomaly_lines.len(),
                anomaly_line
            )?;
        }
    }

    Ok(())
}

/// Escapes a CSV field by wrapping it in quotes if it contains special characters
fn escape_csv_field(field: &str) -> String {
    // If the field contains commas, quotes, or newlines, wrap it in quotes
    // and escape any internal quotes by doubling them
    if field.contains(',') || field.contains('"') || field.contains('\n') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

pub fn report_anomalies_summary(anomalies_summary: &mut BTreeMap<(u64, String), BTreeMap<String, Vec<String>>>, args: &Args, logfile_name: &str, report_for_ai: &mut ReportForAI) -> String {
    
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
        let mut anomaly_data: Vec<AnomalyDescription> = Vec::new();

        for (anomaly_type, details) in anomalies_map {
            for detail in details {
                all_lines.push(format!("{}: {}", anomaly_type, detail));
                anomaly_data.push(AnomalyDescription {area_of_anomaly: anomaly_type.clone(), statistic_name: detail.clone()});
            }
        }

        let c_begin_snap_id = Cell::new(&snap_id.to_string());
        let c_begin_snap_date = Cell::new(snap_date);
        let c_anomalie_details = Cell::new(&all_lines.join("\n"));
        let c_anomalie_count = Cell::new(&all_lines.len().to_string());

        report_for_ai.anomaly_clusters.push(AnomlyCluster { begin_snap_id: *snap_id, 
                                                            begin_snap_date: snap_date.clone(), 
                                                            anomalies_detected: anomaly_data, 
                                                            number_of_anomalies: all_lines.len() as u64 });

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
    make_notes!(logfile_name, args.quiet, 0, "\n\n");
    make_notes!(logfile_name, false, 2, "{}\n", "Anomalies summary for each date from all sections where anomaly was detected".yellow());
    for table_line in table.to_string().lines() {
        make_notes!(logfile_name, args.quiet, 0, "{}\n", table_line);
    }
    if let Err(e) = save_anomalies_to_csv(anomalies_summary, &args.directory) {
        eprintln!("Failed to save CSV files: {}", e);
    }

    html_table
}