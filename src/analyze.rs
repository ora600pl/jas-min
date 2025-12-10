use crate::awr::{AWRSCollection, HostCPU, IOStats, LoadProfile, SQLCPUTime, SQLGets, SQLIOTime, SQLReads, SegmentStats, WaitEvents, AWR, GetStats};

use serde::{Deserialize, Serialize};

//use axum::http::header;
use execute::generic_array::typenum::True;
use plotly::color::NamedColor;
use plotly::{Plot, Histogram, BoxPlot, Scatter, HeatMap};
use plotly::common::{ColorBar, Mode, Title, Visible, Line, Orientation, Anchor, Marker, ColorScale, ColorScalePalette, HoverInfo, MarkerSymbol};
use plotly::box_plot::{BoxMean,BoxPoints};
use plotly::layout::{Axis, GridPattern, Layout, LayoutGrid, Legend, RowOrder, TraceOrder, ModeBar, HoverMode, RangeMode};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::format;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path,PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use colored::*;
use open::*;

use regex::*;
use crate::{anomalies, Args};
use crate::anomalies::*;

use crate::make_notes;
use prettytable::{Table, Row, Cell, format, Attr};
use rayon::prelude::*;

use crate::tools::*;
use crate::reasonings::{StatisticsDescription,TopPeaksSelected,MadAnomaliesEvents,MadAnomaliesSQL,TopForegroundWaitEvents,TopBackgroundWaitEvents,PctOfTimesThisSQLFoundInOtherTopSections,WaitEventsWithStrongCorrelation,WaitEventsFromASH,TopSQLsByElapsedTime,StatsSummary,IOStatsByFunctionSummary,LatchActivitySummary,Top10SegmentStats,InstanceStatisticCorrelation,LoadProfileAnomalies,AnomalyDescription,AnomlyCluster,ReportForAI,AppState};

struct TopStats {
    events: BTreeMap<String, u8>,
    bgevents: BTreeMap<String, u8>,
    sqls:   BTreeMap<String, String>,   // <SQL_ID, Module>
    stat_names: BTreeMap<String, u8>,
    event_anomalies_mad: HashMap<String, Vec<(String,f64)>>,
    bgevent_anomalies_mad: HashMap<String, Vec<(String,f64)>>,
    sql_elapsed_time_anomalies_mad: HashMap<String, Vec<(String,f64)>>,
}

// Check if snap_range argument is passed correctly
fn parse_snap_range(snap_range: &str) -> Result<(u64, u64), String> {
    let parts: Vec<&str> = snap_range.split('-').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid format for snap_range '{}'. Expected format: BEGIN_ID-END_ID", snap_range));
    }
    let begin = parts[0].parse::<u64>()
        .map_err(|_| format!("Invalid number for BEGIN_ID in '{}'", snap_range))?;
    let end = parts[1].parse::<u64>()
        .map_err(|_| format!("Invalid number for END_ID in '{}'", snap_range))?;

    if begin >= end {
        return Err(format!("BEGIN_ID ({}) must be less than END_ID ({})", begin, end));
    }
    Ok((begin, end))
}

//We don't want to plot everything, because it would cause to much trouble 
//we need to find only essential wait events and SQLIDs 
fn find_top_stats(awrs: &Vec<AWR>, db_time_cpu_ratio: f64, filter_db_time: f64, snap_range: &(u64,u64), logfile_name: &str, args: &Args, report_for_ai: &mut ReportForAI) -> TopStats {
    let mut event_names: BTreeMap<String, u8> = BTreeMap::new();
    let mut bgevent_names: BTreeMap<String, u8> = BTreeMap::new();
    let mut sql_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut stat_names: BTreeMap<String, u8> = BTreeMap::new();

    let mut stats_description = StatisticsDescription::default();

    //so we scan the AWR data
    make_notes!(&logfile_name, false, 1, "{}", "DBCPU/DBTIME RATIO ANALYSIS".bold().green());
    make_notes!(&logfile_name, false, 0,
        "\nPeaks are being analyzed based on specified ratio (default 0.666).\nThe ratio is beaing calculated as DB CPU / DB Time.\nThe lower the ratio the more sessions are waiting for resources other than CPU.\nIf DB CPU = 2 and DB Time = 8 it means that on AVG 8 actice sessions are working but only 2 of them are actively working on CPU.\nCurrent ratio used to find peak periods is {}\n\n", db_time_cpu_ratio);
        
    stats_description.dbcpu_dbtime = format!("DBCPU/DBTIME RATIO ANALYSIS\nPeaks are being analyzed based on specified ratio (default 0.666).\nThe ratio is beaing calculated as DB CPU / DB Time.\nThe lower the ratio the more sessions are waiting for resources other than CPU.\nIf DB CPU = 2 and DB Time = 8 it means that on AVG 8 actice sessions are working but only 2 of them are actively working on CPU.\nCurrent ratio used to find peak periods is {}", db_time_cpu_ratio);
    
    let mut full_window_size = ((args.mad_window_size as f32 / 100.0 ) * awrs.len() as f32) as usize; // Default is 20% of probes
    if full_window_size % 2 == 1 {
        full_window_size = full_window_size + 1;
    }
    make_notes!(&logfile_name, false, 1, "{}", "MEDIAN ABSOLUTE DEVIATION".bold().green());
    make_notes!(&logfile_name, false, 0, 
        "\nMAD threshold = {}\nMAD window size={}% ({} of probes out of {})\n\n", args.mad_threshold, args.mad_window_size, full_window_size, awrs.len());
    
    stats_description.median_absolute_deviation = format!("MAD threshold = {}\nMAD window size={}% ({} of probes out of {})\n\n", args.mad_threshold, args.mad_window_size, full_window_size, awrs.len());
    
    let mut top_spikes: Vec<TopPeaksSelected> = Vec::new();

    for awr in awrs {
        let (f_begin_snap,f_end_snap) = snap_range;

        if awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap {
            let mut dbtime: f64 = 0.0;
            let mut cputime: f64 = 0.0;
            //We want to find dbtime and cputime because based on their delta we will base our decisions 
            for lp in awr.load_profile.clone() {
                if lp.stat_name.starts_with("DB Time") || lp.stat_name.starts_with("DB time") {
                    dbtime = lp.per_second;
                    
                } else if lp.stat_name.starts_with("DB CPU") {
                    cputime = lp.per_second;
                }
            }
            //If proportion of cputime and dbtime is less then db_time_cpu_ratio (default 0.666) than we want to find out what might be the problem 
            //because it means that Oracle spent some time waiting on wait events and not working on CPU

            if dbtime > 0.0 && cputime > 0.0 && cputime/dbtime < db_time_cpu_ratio && (filter_db_time==0.0 || dbtime>filter_db_time){
                //println!("Analyzing a peak in {} ({}) for ratio: [{:.2}/{:.2}] = {:.2}", awr.file_name, awr.snap_info.begin_snap_time, cputime, dbtime, (cputime/dbtime));
                make_notes!(&logfile_name, false, 0, "Analyzing a peak in {} ({}) for ratio: [{:.2}/{:.2}] = {:.2}\n", awr.file_name, awr.snap_info.begin_snap_time, cputime, dbtime, (cputime/dbtime));
                
                top_spikes.push(TopPeaksSelected { report_name: awr.file_name.clone(), report_date: awr.snap_info.begin_snap_time.clone(), snap_id: awr.snap_info.begin_snap_id, db_time_value: dbtime, db_cpu_value: cputime, dbcpu_dbtime_ratio: (cputime/dbtime) });

                let mut events: Vec<WaitEvents> = awr.foreground_wait_events.clone();
                let mut bgevents: Vec<WaitEvents> = awr.background_wait_events.clone();
                //I'm sorting events by total wait time, to get the longest waits at the end
                events.sort_by_key(|e| e.total_wait_time_s as i64);
                bgevents.sort_by_key(|e| e.total_wait_time_s as i64);
                let fg_length: usize = events.len();
                let bg_length: usize = bgevents.len();
                //We are registering only TOP10 from each snap
                if fg_length > 10 {
                    for i in 1..11 {
                        event_names.entry(events[fg_length-i].event.clone()).or_insert(1);
                    }
                }
                if bg_length > 10 {
                    for i in 1..11 {
                        bgevent_names.entry(bgevents[bg_length-i].event.clone()).or_insert(1);
                    }
                }
                //And the same with SQLs
                let mut sqls: Vec<crate::awr::SQLElapsedTime> = awr.sql_elapsed_time.clone();
                sqls.sort_by_key(|s| s.elapsed_time_s as i64);
                let l: usize = sqls.len();  
                if l > 5 {
                    for i in 1..6 {
                        sql_ids.entry(sqls[l-i].sql_id.clone()).or_insert(sqls[l-i].sql_module.clone());
                    }
                } else if l > 1 && l <=5 {
                    for i in 0..=l-1 {
                        sql_ids.entry(sqls[i].sql_id.clone()).or_insert(sqls[i].sql_module.clone());
                    }
                }
            }
            for stats in &awr.instance_stats {
                stat_names.entry(stats.statname.clone()).or_insert(1);
            }
        }
    }
    
    if args.mad_window_size == 100 {
        println!("\n****Detecting anamalies using MAD from all probes****\n");
    } else {
        println!("\n****Detecting anamalies using MAD sliding window****\n");
    }
    let event_anomalies = detect_event_anomalies_mad(&awrs, &args, "FOREGROUND");
    for a in &event_anomalies {
        event_names.entry(a.0.to_string()).or_insert(1);
    }
    let bgevent_anomalies = detect_event_anomalies_mad(&awrs, &args, "BACKGROUND");
    for a in &bgevent_anomalies {
        bgevent_names.entry(a.0.to_string()).or_insert(1);
    }

    let sql_anomalies = detect_sql_anomalies_mad(&awrs, &args, "ELAPSED_TIME");
    for a in &sql_anomalies {
        sql_ids.entry(a.0.to_string()).or_insert(String::new());
    }

    if !args.id_sqls.is_empty(){
        let sqlids: Vec<String> = args.id_sqls.split(',').map(|s| s.trim().to_string()).collect();
        println!("Additional SQLs ID considered: {:?}", sqlids.clone());
        for s in sqlids{
            sql_ids.entry(s).or_insert(String::new());
        }
    }
    
    let top: TopStats = TopStats {events: event_names, 
                                  bgevents: bgevent_names, 
                                  sqls: sql_ids, 
                                  stat_names: stat_names, 
                                  event_anomalies_mad: event_anomalies,
                                  bgevent_anomalies_mad: bgevent_anomalies,
                                  sql_elapsed_time_anomalies_mad: sql_anomalies,};

    report_for_ai.top_spikes_marked = top_spikes;
    report_for_ai.general_data = stats_description;

    top
}


fn report_top_sql_sections(sqlid: &str, awrs: &Vec<AWR>) -> HashMap<String, f64> {
    let probe_size: f64 = awrs.len() as f64;

    let mut sql_io_time: f64 = 0.0;
    let mut sql_gets: f64 = 0.0;

    let mut is_statspack: bool = false;
    
    if awrs[0].file_name.ends_with(".txt") {
        is_statspack = true;
    }

    //Filter HashMaps of SQL ordered by CPU Time to find how many times the given sqlid was marked in top section
    let sql_cpu: Vec<&HashMap<String,SQLCPUTime>> = awrs.iter().map(|awr| &awr.sql_cpu_time)
                                                  .filter(|sql| sql.contains_key(sqlid)).collect(); 

    let sql_cpu_count: f64 = sql_cpu.len() as f64;

    //Filter HashMaps of SQL ordered by User IO to find how many times the given sqlid was marked in top section
    let sql_io: Vec<&HashMap<String, SQLIOTime>> = awrs.iter().map(|awr| &awr.sql_io_time)
                                                 .filter(|sql|sql.contains_key(sqlid)).collect();
    let sql_io_count: f64 = sql_io.len() as f64;

     //Filter HashMaps of SQL ordered by GETS to find how many times the given sqlid was marked in top section
     let sql_gets: Vec<&HashMap<String, SQLGets>> = awrs.iter().map(|awr| &awr.sql_gets)
                                                    .filter(|sql|sql.contains_key(sqlid)).collect();
    let sql_gets_count: f64 = sql_gets.len() as f64;

     //Filter HashMaps of SQL ordered by READS to find how many times the given sqlid was marked in top section
     let sql_reads: Vec<&HashMap<String, SQLReads>> = awrs.iter().map(|awr| &awr.sql_reads)
                                                    .filter(|sql|sql.contains_key(sqlid)).collect();
    let sql_reads_count: f64 = sql_reads.len() as f64;

    let mut top_sections: HashMap<String, f64> = HashMap::new();
    top_sections.insert("SQL CPU".to_string(), sql_cpu_count / probe_size * 100.0);
    top_sections.insert("SQL I/O".to_string(), sql_io_count / probe_size * 100.0);
    top_sections.insert("SQL GETS".to_string(), sql_gets_count / probe_size * 100.0);
    top_sections.insert("SQL READS".to_string(), sql_reads_count / probe_size * 100.0);
    
    // If Statspack modify top_sections accordingly
    if is_statspack {
        top_sections.remove("SQL I/O"); // Remove SQL I/O if Statspack is enabled
    }

    top_sections
}

fn report_instance_stats_cor(instance_stats: HashMap<String, Vec<f64>>, dbtime_vec: Vec<f64>) -> BTreeMap<(i64, String), f64> {
    let mut sorted_correlation: BTreeMap<(i64, String), f64> = BTreeMap::new();

    for (k,v) in instance_stats {
        if v.len() == dbtime_vec.len() {
            let crr = pearson_correlation_2v(&v, &dbtime_vec);
            if crr >= 0.5 || crr <= -0.5 {
                sorted_correlation.insert(((crr * 1000.0) as i64 , k.clone()), crr);
            }
        } else {
            println!("Can't calculate correlation for {} - diff was {}", &k, dbtime_vec.len() - v.len());
        }
    }
    sorted_correlation
}

//Add SQL_IDs found in ASH to event charts
fn merge_ash_sqls_to_events(ash_event_sql_map: HashMap<String, HashSet<String>>, dirpath: &str) {

    for (event, sqls) in ash_event_sql_map {
        let filename = get_safe_filename(event.clone(), "fg".to_string());
        let path = Path::new(&dirpath).join(&filename);
        let mut event_html_content = format!(
            r#"
                <h4 style="color:blue;font-weight:bold;">Wait Event found in ASH for following SQL IDs:</h4>
                <ul>
            "#);
        for sqlid in sqls {
            event_html_content = format!("{}<li><a href=../sqlid/sqlid_{}.html target=_blank style=\"color: black;\">{}</a></li>", event_html_content, sqlid, sqlid);
        }
        event_html_content = format!("{}</ul>", event_html_content);
        if path.exists() {
            let mut event_file: String = fs::read_to_string(&path)
                                            .expect(&format!("Failed to read file: {}", &path.to_string_lossy()));
            event_file = event_file.replace(
                                    "</h2>",
                                    &format!("</h2>\n{}\n",event_html_content));

            if let Err(e) = fs::write(&path, event_file) {
                eprintln!("Error writing file {}: {}", &path.to_string_lossy(), e);
            }
        }
        
    }

}

//Add SQL_IDs found with strong correlation to event charts
fn merge_correlated_sqls_to_events(crr_event_sql_map: HashMap<String, HashMap<String, f64>>, dirpath: &str) {

    for (event, sqls) in crr_event_sql_map {
        let filename = get_safe_filename( event.clone(), "fg".to_string());
        let path = Path::new(&dirpath).join(&filename);
        let mut event_html_content = format!(
            r#"
                <h4 style="color:blue;font-weight:bold;">SQL IDs with strong correlation with this wait event:</h4>
                <ul>
            "#);

        let mut vec_sqls: Vec<_> = sqls.into_iter().collect();
        vec_sqls.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        for (sqlid, crr) in vec_sqls {
            event_html_content = format!("{}<li><a href=../sqlid/sqlid_{}.html target=_blank style=\"color: black;\">{:.2} | {}</a></li>", event_html_content, sqlid, crr, sqlid);
        }
        event_html_content = format!("{}</ul>", event_html_content);
        if path.exists() {
            let mut event_file: String = fs::read_to_string(&path)
                                            .expect(&format!("Failed to read file: {}", &path.to_string_lossy()));
            event_file = event_file.replace(
                                    "</h2>",
                                    &format!("</h2>\n{}\n",event_html_content));

            if let Err(e) = fs::write(&path, event_file) {
                eprintln!("Error writing file {}: {}", &path.to_string_lossy(), e);
            }
        }
        
    }

}

// Generate plots for top events
fn generate_events_plotfiles(awrs: &Vec<AWR>, top_events: &BTreeMap<String, u8>, is_fg: bool, snap_range: &(u64,u64), dirpath: &str) {
    let (f_begin_snap,f_end_snap) = snap_range;
    // Ensure that there is at least one AWR with fg or bg event and they are explicitly checked
    assert!(!awrs.is_empty(),"generate_events_plotfiles: No AWR data available.");

    let mut hist_buckets: Vec<String> = Vec::new();
    let mut bucket_colors: HashMap<String, String> = HashMap::new();
    // Make colors consistent across buckets 
    let color_palette = vec![
        "#00E399", "#2FD900", "#E3E300", "#FFBF00", "#FF8000",
        "#FF4000", "#FF0000", "#B22222", "#8B0000", "#4B0082",
        "#8A2BE2", "#1E90FF"
    ];
    // Group Events by Name and by needed data (ename(db_time, total_wait, waits,histogram values by bucket, heatmap)
    let mut snap_time: Vec<String> = Vec::new();
    struct EventStats {
        pct_dbtime: Vec<Option<f64>>,
        total_wait_time_s: Vec<Option<f64>>,
        waits: Vec<Option<u64>>,
        histogram_by_bucket: BTreeMap<String, Vec<Option<f32>>>,
        heatmap: Vec<Option<BTreeMap<String,f32>>>
    }

    let mut data_by_event: HashMap<String, EventStats> = HashMap::new();
    let mut buckets_found: bool = false;

    for awr in awrs {
        if awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap {
            snap_time.push(format!("{} ({})", awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id));
        
            let events = if is_fg {
                &awr.foreground_wait_events
            } else {
                &awr.background_wait_events
            };

            if events.is_empty(){
                println!("   WARNING: generate_events_plotfiles found empty events {}",awr.snap_info.begin_snap_id);
                continue;
            } else{
                if !buckets_found{
                     // To cover buckets dynamic across db versions - Extract bucket names from the first event's histogram_ms found
                    hist_buckets = events[0].waitevent_histogram_ms.keys().cloned().collect();
                    // Assign colors from the palette to detected buckets
                    for (i, bucket) in hist_buckets.iter().enumerate() {
                        let color = color_palette.get(i % color_palette.len()).unwrap();
                        bucket_colors.insert(bucket.clone(), color.to_string());
                    }
                    buckets_found = true;
                }
                for top_event in top_events {
                    if let Some(event) = events.iter().find(|e| &e.event == top_event.0) {
                        // Initilize events map
                        let entry = data_by_event
                            .entry(top_event.0.clone())
                            .or_insert_with(|| EventStats {
                                pct_dbtime: Vec::new(),
                                total_wait_time_s: Vec::new(),
                                waits: Vec::new(),
                                histogram_by_bucket: BTreeMap::new(),
                                heatmap: Vec::new()
                            });
                        // Gather data by Event Name
                        entry.pct_dbtime.push(Some(event.pct_dbtime));
                        entry.total_wait_time_s.push(Some(event.total_wait_time_s));
                        entry.waits.push(Some(event.waits));
                        for (bucket, value) in &event.waitevent_histogram_ms {
                            entry.histogram_by_bucket
                                .entry(bucket.clone())
                                .or_insert_with(Vec::new)
                                .push(Some(*value));
                        }
                        entry.heatmap.push(Some(event.waitevent_histogram_ms.clone()))
                    } else { // Event does NOT exist in this snapshot → push gaps
                        let entry = data_by_event
                            .entry(top_event.0.clone())
                            .or_insert_with(|| EventStats {
                                pct_dbtime: Vec::new(),
                                total_wait_time_s: Vec::new(),
                                waits: Vec::new(),
                                histogram_by_bucket: BTreeMap::new(),
                                heatmap: Vec::new(),
                            });

                        entry.pct_dbtime.push(None);
                        entry.total_wait_time_s.push(None);
                        entry.waits.push(None);
                        for vals in entry.histogram_by_bucket.values_mut() {
                            vals.push(None);
                        }
                        entry.heatmap.push(None);
                    }
                }
            }
        }
    }

    // Build plots for each event and save it as separate file
    for (event, entry) in data_by_event {
        let mut plot: Plot = Plot::new();
        let event_name = format!("{}", &event);
    
        let mut z_matrix: Vec<Vec<Option<f32>>> = Vec::new();
        for bucket in &hist_buckets {
            let row: Vec<Option<f32>> = entry.heatmap
                .iter()
                .map(|snap_opt| {
                    // For each snapshot, extract this bucket's value
                    match snap_opt {
                        Some(histogram) => {
                            histogram.get(bucket).copied() // Snapshot has data
                        }
                        None => None, // Snapshot missing - no data for any bucket
                    }
                })
                .collect();
            z_matrix.push(row);
        }

        let heatmap = HeatMap::new(snap_time.clone(), hist_buckets.clone(), z_matrix)
            .x_axis("x1")
            .y_axis("y1")
            .hover_on_gaps(true)
            .show_legend(false)
            .show_scale(false)
            .color_scale(ColorScale::Palette(ColorScalePalette::Electric))
            .reverse_scale(true)
            .name("%");

        plot.add_trace(heatmap);
    
        //Add Total Wait Time trace
        let event_total_wait = Scatter::new(snap_time.clone(), entry.total_wait_time_s.clone())
            .mode(Mode::LinesMarkers)
            .marker(Marker::new().opacity(0.5))
            .name("Total Wait Time (s)")
            .x_axis("x1")
            .y_axis("y2");

        plot.add_trace(event_total_wait);

        // Add Wait Count trace
        let event_wait_count = Scatter::new(snap_time.clone(), entry.waits.clone())
            .mode(Mode::LinesMarkers)
            .marker(Marker::new().opacity(0.5))
            .name("Wait Count")
            .x_axis("x1")
            .y_axis("y3");

        plot.add_trace(event_wait_count);

        //Add Event DBTime distribution
        let dbt_histogram = Histogram::new(entry.pct_dbtime.clone())
           .name(&event_name)
           .legend_group(&event_name)
            //.n_bins_x(100) // Number of bins
            .x_axis("x2")
            .y_axis("y4")
            .show_legend(true);
        plot.add_trace(dbt_histogram);
        
        // Add Box Plot for DBTime
        let dbt_box_plot = BoxPlot::new_xy(entry.pct_dbtime.clone(),vec![event.clone();entry.pct_dbtime.clone().len()])
            .name("")
            .legend_group(&event_name)
            .box_mean(BoxMean::True)
            .orientation(Orientation::Horizontal)
            .x_axis("x2")
            .y_axis("y5")
            .marker(Marker::new().color("#e377c2".to_string()).opacity(0.7))
            .show_legend(false);
        plot.add_trace(dbt_box_plot);

        // Add Bar Plots for Histogram Buckets
        for (bucket, values) in &entry.histogram_by_bucket {
            let default_color: String = "#000000".to_string(); // Store default in a variable
            let color: &String = bucket_colors.get(bucket).unwrap_or(&default_color);
            let bucket_name: String = format!("{}", &bucket);

            let ms_bucket_histogram = Histogram::new(values.clone())
                .name(&bucket_name)
                .legend_group(&bucket_name)
                .auto_bin_x(true)
                //.n_bins_x(50) // Number of bins
                .x_axis("x3")
                .y_axis("y6")
                .marker(Marker::new().color(color.clone()).opacity(0.7))
                .show_legend(true);
            plot.add_trace(ms_bucket_histogram);

            let ms_bucket_box_plot = BoxPlot::new_xy(values.clone(),vec![bucket.clone();values.clone().len()])// Use values for y-axis, // Use bucket names for x-axis
                .name("")
                .legend_group(&bucket_name)
                .box_mean(BoxMean::True)
                .orientation(Orientation::Horizontal)
                .x_axis("x3")
                .y_axis("y7")
                .marker(Marker::new().color(color.clone()).opacity(0.7))
                .show_legend(false);
            plot.add_trace(ms_bucket_box_plot);
        }
        
        let layout: Layout = Layout::new()
            //.title(&format!("'<b>{}</b>'", event))
            .height(1800)
            .bar_gap(0.0)
            .bar_mode(plotly::layout::BarMode::Overlay)
            .grid(
                LayoutGrid::new()
                    .rows(7)
                    .columns(1),
            )
            //.legend(
            //    Legend::new()
            //        .x(0.0)
            //        .x_anchor(Anchor::Left),
            //)
            .x_axis(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("y1")
                    .range(vec![0.,])
                    .show_grid(true),
            )
            .y_axis(
                Axis::new()
                    .domain(&[0.0, 0.22])
                    .anchor("x1")
                    .range(vec![0.,]),
            )
            .y_axis2(Axis::new()
                .anchor("x1")
                .domain(&[0.23, 0.33])
                .title("Total Wait (s)")
                .zero_line(true)
                .range(vec![0.,])
                .range_mode(RangeMode::ToZero)
            )
            .y_axis3(Axis::new()
                .anchor("x1")
                .domain(&[0.33, 0.43])
                .title("Wait count #")
                .zero_line(true)
                .range(vec![0.,])
                .range_mode(RangeMode::ToZero)
            )
            .x_axis2(
                Axis::new()
                    .title("% DBTime")
                    .domain(&[0.0, 1.0])
                    .anchor("y4")
                    .range(vec![0.,])
                    .show_grid(true),
            )
            .y_axis4(
                Axis::new()
                    .domain(&[0.48, 0.6])
                    .anchor("x2")
                    .range(vec![0.,]),
            )
            .y_axis5(
                Axis::new()
                    .domain(&[0.6, 0.62])
                    .anchor("x2")
                    .range(vec![0.,])
                    .show_tick_labels(false),
            )
            .x_axis3(
                Axis::new()
                    .title("% Wait Event ms")
                    .domain(&[0.0, 1.0])
                    .anchor("y6")
                    .range(vec![0.,])
                    .show_grid(true),
            )
            .y_axis6(
                Axis::new()
                    .domain(&[0.67, 0.83])
                    .anchor("x3")
                    .range(vec![0.,]),
            )
            .y_axis7(
                Axis::new()
                    .domain(&[0.85, 1.0])
                    .anchor("x3")
                    .range(vec![0.,])
                    .show_tick_labels(false)
                    .show_grid(true),
            );
            plot.set_layout(layout);
        
        let file_name = get_safe_filename(event.clone(), if is_fg{"fg".to_string()}else{"bg".to_string()});

        // Save the plot as an HTML file
        let path = Path::new(&dirpath).join(&file_name);
        //plot.save(path).expect("Failed to save plot to file");
        plot.write_html(&path);
        let mut event_file: String = fs::read_to_string(&path).expect(&format!("Failed to read file: {}", file_name));
        event_file = event_file.replace(
            "<body>",
            &format!("<style>\nbody {{ font-family: Arial, sans-serif; }}.content {{ font-size: 16px; }}\n</style>\n<body>\n\t<h2 style=\"width:100%;text-align:center;\">{}</h2>",event));
        if let Err(e) = fs::write(&path, event_file) {
            eprintln!("Error writing file {}: {}", file_name, e);
        }
    }
    if is_fg {
        println!("Saved plots for Foreground events to '{}/fg/fg_*'", &dirpath);
    } else {
        println!("Saved plots for Background events to '{}/bg/bg_*'", &dirpath);
    }
}

// Generate TOP SQLs html subpages with plots - To Be Developed
fn generate_sqls_plotfiles(awrs: &Vec<AWR>, top_stats: &TopStats, snap_range: &(u64,u64), dirpath: &str){
    let (f_begin_snap,f_end_snap) = snap_range;
    
    struct SQLStats{
        execs: Vec<Option<u64>>,            // Number of Executions
        ela_exec_s: Vec<Option<f64>>,       // Elapsed Time (s) per Execution
        ela_pct_total: Vec<Option<f64>>,    // Elapsed Time as a percentage of Total DB time
        pct_cpu: Vec<Option<f64>>,          // CPU Time as a percentage of Elapsed Time
        pct_io: Vec<Option<f64>>,           // User I/O Time as a percentage of Elapsed Time
        cpu_time_exec_s: Vec<Option<f64>>,  // CPU Time (s) per Execution
        cpu_t_pct_total: Vec<Option<f64>>,  // CPU Time as a percentage of Total DB CPU
        io_time_exec_s: Vec<Option<f64>>,   // User I/O Wait Time (s) per Execution
        io_pct_total: Vec<Option<f64>>,     // User I/O Time as a percentage of Total User I/O Wait time
        gets_per_exec: Vec<Option<f64>>,    // Number of Buffer Gets per Execution
        gets_pct_total: Vec<Option<f64>>,   // Buffer Gets as a percentage of Total Buffer Gets
        phy_r_exec: Vec<Option<f64>>,       // Number of Physical Reads per Execution
        phy_r_pct_total: Vec<Option<f64>>  // Physical Reads as a percentage of Total Disk Reads
    }
    //let mut sqls_by_stats: HashMap<String, SQLStats> = HashMap::new();

    let colors = vec![
        "#1f77b4", // strong blue
        "#ff7f0e", // vivid orange
        "#2ca02c", // medium green
        "#d62728", // bright red
        "#9467bd", // deep purple
        "#8c564b", // warm brown
        "#e377c2", // magenta
        "#7f7f7f", // dark gray
        "#bcbd22", // olive green
        "#17becf", // teal
        "#393b79", // navy blue
        "#ff9896", // salmon red
        "#c49c94", // muted brown
    ];

    let x_vals: Vec<String> = awrs
        .iter()
        .filter(|awr| awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap)
        .map(|awr| format!("{} ({})", awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id))
        .collect();

    let sqls_by_stats: HashMap<String, SQLStats> = top_stats.sqls.par_iter()
        .map(|(sql_id, _)| {
            let mut stats = SQLStats {
                execs: Vec::new(),
                ela_exec_s: Vec::new(),
                ela_pct_total: Vec::new(),
                pct_cpu: Vec::new(),
                pct_io: Vec::new(),
                cpu_time_exec_s: Vec::new(),
                cpu_t_pct_total: Vec::new(),
                io_time_exec_s: Vec::new(),
                io_pct_total: Vec::new(),
                gets_per_exec: Vec::new(),
                gets_pct_total: Vec::new(),
                phy_r_exec: Vec::new(),
                phy_r_pct_total: Vec::new(),
            };

            for awr in awrs {
                if awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap {
                    let mut sql_found: bool = false;
                    // Elapsed Time
                    if let Some(sql_et) = awr.sql_elapsed_time.iter().find(|e| &e.sql_id == sql_id) {
                        stats.execs.push(Some(sql_et.executions));
                        stats.ela_exec_s.push(Some(sql_et.elpased_time_exec_s));
                        stats.ela_pct_total.push(Some(sql_et.pct_total));
                        stats.pct_cpu.push(Some(sql_et.pct_cpu));
                        stats.pct_io.push(Some(sql_et.pct_io));
                        sql_found = true;
                    } else {
                        //stats.execs.push(Some(0));
                        stats.ela_exec_s.push(None);
                        stats.ela_pct_total.push(None);
                        //stats.pct_cpu.push(Some(0.0);
                        //stats.pct_io.push(Some(0.0);
                    }
    
                    // CPU Time
                    if let Some(sql_cpu) = awr.sql_cpu_time.get(sql_id) {
                        stats.cpu_time_exec_s.push(Some(sql_cpu.cpu_time_exec_s));
                        stats.cpu_t_pct_total.push(Some(sql_cpu.pct_total));
                        if !sql_found{
                            stats.execs.push(Some(sql_cpu.executions));
                            stats.pct_cpu.push(Some(sql_cpu.pct_cpu));
                            stats.pct_io.push(Some(sql_cpu.pct_io));
                            sql_found = true;
                        }
                    } else {
                        stats.cpu_time_exec_s.push(None);
                        stats.cpu_t_pct_total.push(None);
                    }
    
                    // IO Time
                    if let Some(sql_io) = awr.sql_io_time.get(sql_id) {
                        stats.io_time_exec_s.push(Some(sql_io.io_time_exec_s));
                        stats.io_pct_total.push(Some(sql_io.pct_total));
                        if !sql_found{
                            stats.execs.push(Some(sql_io.executions));
                            stats.pct_cpu.push(Some(sql_io.pct_cpu));
                            stats.pct_io.push(Some(sql_io.pct_io));
                            sql_found = true;
                        }
                    } else {
                        stats.io_time_exec_s.push(None);
                        stats.io_pct_total.push(None);
                    }
    
                    // Gets
                    if let Some(sql_gets) = awr.sql_gets.get(sql_id) {
                        stats.gets_per_exec.push(Some(sql_gets.gets_per_exec));
                        stats.gets_pct_total.push(Some(sql_gets.pct_total));
                        if !sql_found{
                            stats.execs.push(Some(sql_gets.executions));
                            stats.pct_cpu.push(Some(sql_gets.pct_cpu));
                            stats.pct_io.push(Some(sql_gets.pct_io));
                            sql_found = true;
                        }
                    } else {
                        stats.gets_per_exec.push(None);
                        stats.gets_pct_total.push(None);
                    }
    
                    // Reads
                    if let Some(sql_reads) = awr.sql_reads.get(sql_id) {
                        stats.phy_r_exec.push(Some(sql_reads.reads_per_exec));
                        stats.phy_r_pct_total.push(Some(sql_reads.pct_total));
                        if !sql_found{
                            stats.execs.push(Some(sql_reads.executions));
                            stats.pct_cpu.push(Some(sql_reads.cpu_time_pct));
                            stats.pct_io.push(Some(sql_reads.pct_io));
                            sql_found = true;
                        }
                    } else {
                        stats.phy_r_exec.push(None);
                        stats.phy_r_pct_total.push(None);
                    }
                    if !sql_found {
                        stats.execs.push(None);
                        stats.pct_cpu.push(None);
                        stats.pct_io.push(None);
                    }
                }
            }
    
            (sql_id.clone(), stats)
        })
        .collect();

    for (sql, stats) in sqls_by_stats{
        let mut sql_plot: Plot = Plot::new();
        let sql_id = format!("{}", &sql);

        let sql_gets_per_exec = Scatter::new(x_vals.clone(), stats.gets_per_exec.clone())
            .mode(Mode::Markers)
            .name("# Buffer Gets")
            .marker(Marker::new().color(colors[12]).symbol(MarkerSymbol::StarDiamond).opacity(0.7))
            .x_axis("x1")
            .y_axis("y4")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_gets_per_exec);

        let sql_phy_r_exec = Scatter::new(x_vals.clone(), stats.phy_r_exec.clone())
            .mode(Mode::Markers)
            .name("# Physical Reads")
            .marker(Marker::new().color(colors[11]).symbol(MarkerSymbol::DiamondTall).opacity(0.7))
            .x_axis("x1")
            .y_axis("y4")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_phy_r_exec);

        let sql_ela_pct_total = Scatter::new(x_vals.clone(), stats.ela_pct_total.clone())
            .mode(Mode::LinesMarkers)
            .name("% Ela Time as DB Time")
            .marker(Marker::new().color(colors[10]).opacity(0.5))
            .x_axis("x1")
            .y_axis("y3");
        sql_plot.add_trace(sql_ela_pct_total);

        let sql_pct_cpu = Scatter::new(x_vals.clone(), stats.pct_cpu.clone())
            .mode(Mode::LinesMarkers)
            .name("% CPU of Ela")
            .marker(Marker::new().color(colors[9]).opacity(0.5))
            .x_axis("x1")
            .y_axis("y3");
        sql_plot.add_trace(sql_pct_cpu);

        // IO % of Ela
        let sql_pct_io = Scatter::new(x_vals.clone(), stats.pct_io.clone())
            .mode(Mode::LinesMarkers)
            .name("% IO of Ela")
            .marker(Marker::new().color(colors[8]).opacity(0.5))
            .x_axis("x1")
            .y_axis("y3");
        sql_plot.add_trace(sql_pct_io);

        // CPU Time as % of DB CPU
        let sql_cpu_t_pct_total = Scatter::new(x_vals.clone(), stats.cpu_t_pct_total.clone())
            .mode(Mode::Markers)
            .name("% CPU Time as DB CPU")
            .marker(Marker::new().color(colors[7]).symbol(MarkerSymbol::DiamondTall).opacity(0.7))
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_cpu_t_pct_total);

        // IO Time as % of Total IO Wait
        let sql_io_pct_total = Scatter::new(x_vals.clone(), stats.io_pct_total.clone())
            .mode(Mode::Markers)
            .name("% IO Time as DB IO Wait")
            .marker(Marker::new().color(colors[6]).symbol(MarkerSymbol::StarDiamond).opacity(0.7))
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_io_pct_total);

        // Buffer Gets as % of Total
        let sql_gets_pct_total = Scatter::new(x_vals.clone(), stats.gets_pct_total.clone())
            .mode(Mode::Markers)
            .name("% Gets as Total Gets")
            .marker(Marker::new().color(colors[5]).symbol(MarkerSymbol::Diamond).opacity(0.7))
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_gets_pct_total);

        // Physical Reads as % of Total Disk Reads
        let sql_phy_r_pct_total = Scatter::new(x_vals.clone(), stats.phy_r_pct_total.clone())
            .mode(Mode::Markers)
            .name("% Phys Reads as Total Disk Reads")
            .marker(Marker::new().color(colors[4]).symbol(MarkerSymbol::Square).opacity(0.7))
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_phy_r_pct_total);

        let sql_ela_exec_s = Scatter::new(x_vals.clone(), stats.ela_exec_s.clone())
            .mode(Mode::LinesMarkers)
            .name("(s) Elapsed Time per Exec")
            .marker(Marker::new().color(colors[3]).opacity(0.5))
            .x_axis("x1")
            .y_axis("y2");
        sql_plot.add_trace(sql_ela_exec_s);
    
        let sql_cpu_time_exec_s = Scatter::new(x_vals.clone(), stats.cpu_time_exec_s.clone())
            .mode(Mode::Markers)
            .name("(s) CPU Time")
            .marker(Marker::new().color(colors[2]).symbol(MarkerSymbol::DiamondTall).opacity(0.7))
            .x_axis("x1")
            .y_axis("y2")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_cpu_time_exec_s);
        
        let sql_io_time_exec_s = Scatter::new(x_vals.clone(), stats.io_time_exec_s.clone())
            .mode(Mode::Markers)
            .name("(s) User I/O Wait Time")
            .marker(Marker::new().color(colors[1]).symbol(MarkerSymbol::StarDiamond).opacity(0.7))
            .x_axis("x1")
            .y_axis("y2")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_io_time_exec_s);
        
        let sql_exec = Scatter::new(x_vals.clone(), stats.execs.clone())
            .mode(Mode::LinesMarkers)
            .name("# Executions")
            .marker(Marker::new().color(colors[0]).opacity(0.5))
            .x_axis("x1")
            .y_axis("y1");
        sql_plot.add_trace(sql_exec);

        let sql_layout: Layout = Layout::new()
            //.title(&format!("'<b>{}</b>'", &sql))
            .height(800)
            .hover_mode(HoverMode::X)
            .grid(
                LayoutGrid::new()
                    .rows(1)
                    .columns(1),
            )
            .x_axis(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("y1")
                    .range(vec![0.,])
                    .show_grid(true)
            )
            .y_axis(
                Axis::new()
                    .domain(&[0.0, 0.23])
                    .anchor("x1")
                    .range(vec![0.,])
                    .title("#")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero)
            )
            .y_axis2(
                Axis::new()
                    .domain(&[0.25, 0.48])
                    .anchor("x1")
                    .range(vec![0.,])
                    .title("(s) per Exec")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero)
            )
            .y_axis3(
                Axis::new()
                    .domain(&[0.5, 0.73])
                    .anchor("x1")
                    .range(vec![0.,])
                    .title("%")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero)
            )
            .y_axis4(
                Axis::new()
                    .domain(&[0.75, 1.0])
                    .anchor("x1")
                    .range(vec![0.,])
                    .title("#")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero)
            );
            sql_plot.set_layout(sql_layout);
            let file_name: String = format!("{}/sqlid/sqlid_{}.html", dirpath, &sql);
            let path: &Path = Path::new(&file_name);
            sql_plot.write_html(path);
    }
    println!("Saved plots for SQLs to '{}/sqlid/sqlid_*'", dirpath);

}

// Generate HTML for Instance Efficiency
fn generate_instance_efficiency_plot(awrs: &Vec<AWR>, snap_range: &(u64,u64), dirpath: &str) -> String {    
    let (f_begin_snap,f_end_snap) = snap_range;
    struct InstEffStats{
        stat_name: String,
        stat_pct: Vec<Option<f32>>
    }
    let mut ie_stats_names: Vec<String> = awrs[0]
            .instance_efficiency
            .iter()
            .map(|s| s.eff_stat.clone())
            .collect();

    let mut x_vals: Vec<String> = awrs
            .iter()
            .filter(|awr| awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap)
            .map(|awr| format!("{} ({})", awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id))
            .collect();

    let inst_eff_stats: Vec<InstEffStats> = ie_stats_names.par_iter()
        .map(|ie_name|{
            // Collect values across all matching AWRs in range
            let mut values: Vec<Option<f32>> = Vec::new();
            for awr in awrs {
                if awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap{
                    for ie in &awr.instance_efficiency {
                        if ie.eff_stat == *ie_name {
                            values.push(ie.eff_pct);
                        }
                    }
                }
            }
            InstEffStats {
                stat_name: ie_name.clone(),
                stat_pct: values,
            }
        }).collect();

    // === Create the instance efficiency plot ===
    let mut plot_instance_efficiency = Plot::new();
    let color_palette = vec![
        "#1f77b4", "#ff7f0e", "#2ca02c", "#d62728",
        "#9467bd", "#8c564b", "#e377c2", "#7f7f7f",
        "#bcbd22", "#17becf",
        ];
    for (i,ieplot) in inst_eff_stats.iter().enumerate() {
        let color = color_palette[i % color_palette.len()];
        let trace = Scatter::new(x_vals.clone(), ieplot.stat_pct.clone())
            .mode(Mode::Lines)
            .name(ieplot.stat_name.clone())
            .x_axis("x1")
            .y_axis("y1")
            .marker(Marker::new().color(color))
            .legend_group(&ieplot.stat_name)
            .show_legend(false);
        plot_instance_efficiency.add_trace(trace);

        let histogram = Histogram::new(ieplot.stat_pct.clone())
            .name(ieplot.stat_name.clone())
            .x_axis("x2")
            .y_axis("y2")
            .legend_group(&ieplot.stat_name)
            .marker(Marker::new().color(color).opacity(0.7))
            .show_legend(true);
        plot_instance_efficiency.add_trace(histogram);

        let box_plot = BoxPlot::new_xy(ieplot.stat_pct.clone(),vec![ieplot.stat_name.clone();ieplot.stat_pct.clone().len()])
            .name("")
            .x_axis("x2")
            .y_axis("y3")
            .orientation(Orientation::Horizontal)
            .legend_group(&ieplot.stat_name)
            .box_mean(BoxMean::True)
            .marker(Marker::new().color(color).opacity(0.7))
            .show_legend(false);
        plot_instance_efficiency.add_trace(box_plot);
    }

    // Add layout
    let layout = Layout::new()
        .title("Instance Efficiency %")
        .height(1000)
        .bar_gap(0.0)
        .bar_mode(plotly::layout::BarMode::Overlay)
        .hover_mode(HoverMode::X)
        .grid(
            LayoutGrid::new()
                .rows(3)
                .columns(1),
        )
        .x_axis(
            Axis::new()
                .domain(&[0.0, 1.0])
                .anchor("y1")
                .range(vec![0.,])
                .show_grid(true)
        )
        .y_axis(
            Axis::new()
                .title("Efficiency (%)")
                .domain(&[0.0, 0.3])
                .anchor("x1")
                .range(vec![0.,])
                .zero_line(true)
                .range_mode(RangeMode::ToZero)
        )
        .x_axis2(
            Axis::new()
                .title("Hit %")
                .domain(&[0.0, 1.0])
                .anchor("y2")
                .range(vec![0.,])
                .show_grid(true)
        )
        .y_axis2(
            Axis::new()
                .domain(&[0.35, 0.75])
                .anchor("x2")
                .range(vec![0.,]),
        )
        .y_axis3(
            Axis::new()
                .domain(&[0.8, 1.0])
                .anchor("x2")
                .range(vec![0.,])
                .show_tick_labels(false),
        );

    plot_instance_efficiency.set_layout(layout);
    plot_instance_efficiency.to_inline_html(Some("instance-efficiency-plot"))
}

fn generate_instance_stats_plotfiles(awrs: &Vec<AWR>, snap_range: &(u64,u64), dirpath: &str){    
    let (f_begin_snap,f_end_snap) = snap_range;
    struct InstStats{
        stat_name: String,
        stat_total: Vec<Option<u64>>
    }
    let mut i_stats_names: Vec<String> = awrs[0]
            .instance_stats
            .iter()
            .map(|s| s.statname.clone())
            .collect();

    let mut x_vals: Vec<String> = awrs
            .iter()
            .filter(|awr| awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap)
            .map(|awr| format!("{} ({})", awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id))
            .collect();

    let inst_stats: Vec<InstStats> = i_stats_names.par_iter()
        .map(|i_name|{
            // Collect values across all matching AWRs in range
            let mut values: Vec<Option<u64>> = Vec::new();
            for awr in awrs {
                if awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap{
                    for i in &awr.instance_stats {
                        if i.statname == *i_name {
                            values.push(Some(i.total));
                        }
                    }
                }
            }
            InstStats {
                stat_name: i_name.clone(),
                stat_total: values,
            }
        }).collect();

    // === Create the instance stats plots ===
    for (i,iplot) in inst_stats.iter().enumerate() {
        let mut plot_instance_stat = Plot::new();
        let trace = Scatter::new(x_vals.clone(), iplot.stat_total.clone())
            .mode(Mode::Lines)
            .name(iplot.stat_name.clone())
            .x_axis("x1")
            .y_axis("y1")
            .legend_group(&iplot.stat_name)
            .show_legend(false);
        plot_instance_stat.add_trace(trace);

        let histogram = Histogram::new(iplot.stat_total.clone())
            .name(iplot.stat_name.clone())
            .x_axis("x2")
            .y_axis("y2")
            .legend_group(&iplot.stat_name)
            .marker(Marker::new().opacity(0.7))
            .show_legend(true);
        plot_instance_stat.add_trace(histogram);

        let box_plot = BoxPlot::new_xy(iplot.stat_total.clone(),vec![iplot.stat_name.clone();iplot.stat_total.clone().len()])
            .name("")
            .x_axis("x2")
            .y_axis("y3")
            .orientation(Orientation::Horizontal)
            .legend_group(&iplot.stat_name)
            .box_mean(BoxMean::True)
            .marker(Marker::new().opacity(0.7))
            .show_legend(false);
        plot_instance_stat.add_trace(box_plot);

        // Add layout
        let layout = Layout::new()
            .title(&iplot.stat_name)
            .height(800)
            .bar_gap(0.0)
            .bar_mode(plotly::layout::BarMode::Overlay)
            .hover_mode(HoverMode::X)
            .grid(
                LayoutGrid::new()
                    .rows(3)
                    .columns(1),
            )
            .x_axis(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("y1")
                    .range(vec![0.,])
                    .show_grid(true)
            )
            .y_axis(
                Axis::new()
                    .title("Total Number")
                    .domain(&[0.0, 0.3])
                    .anchor("x1")
                    .range(vec![0.,])
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero)
            )
            .x_axis2(
                Axis::new()
                    .title("Total Number")
                    .domain(&[0.0, 1.0])
                    .anchor("y2")
                    .range(vec![0.,])
                    .show_grid(true)
            )
            .y_axis2(
                Axis::new()
                    .domain(&[0.35, 0.75])
                    .anchor("x2")
                    .range(vec![0.,]),
            )
            .y_axis3(
                Axis::new()
                    .domain(&[0.8, 1.0])
                    .anchor("x2")
                    .range(vec![0.,])
                    .show_tick_labels(false),
            );

        plot_instance_stat.set_layout(layout);
        // Save to HTML
        let file_name = get_safe_filename(iplot.stat_name.clone(),"inst_stat".to_string());
        let path = Path::new(&dirpath).join(&file_name);
        plot_instance_stat.write_html(path);
    }
    println!("Saved plots for Instance Stats to '{}/stats/stats_*'", &dirpath);

}

fn generate_iostats_plotfile(awrs: &Vec<AWR>, snap_range: &(u64,u64), dirpath: &str) -> BTreeMap<String,BTreeMap<String, (f64, f64)>> {
    let (f_begin_snap,f_end_snap) = snap_range;
    
    const IO_FUNCTIONS: [&str; 14] = [
        "RMAN",
        "DBWR",
        "LGWR",
        "ARCH",
        "XDB",
        "Streams AQ",
        "Data Pump",
        "Recovery",
        "Buffer Cache Reads",
        "Direct Reads",
        "Direct Writes",
        "Smart Scan",
        "Archive Manager",
        "Others",
    ];
    struct IOStats{
        reads_data: Vec<f64>,	// in MB
        reads_req_s: Vec<f64>,
        reads_data_s: Vec<f64>,	// in MB
        writes_data: Vec<f64>,	// in MB
        writes_req_s: Vec<f64>,
        writes_data_s: Vec<f64>,	// in MB
        waits_count: Vec<u64>,
        avg_time: Vec<Option<f64>>,		// in ms
    }
    let mut io_stats_byfunc: HashMap<String, IOStats> = HashMap::new();
    for func in IO_FUNCTIONS.iter() {
        io_stats_byfunc.insert(
            func.to_string(), 
            IOStats {
                reads_data: Vec::new(),
                reads_req_s: Vec::new(),
                reads_data_s: Vec::new(),
                writes_data: Vec::new(),
                writes_req_s: Vec::new(),
                writes_data_s: Vec::new(),
                waits_count: Vec::new(),
                avg_time: Vec::new(),
            }
        );
    }
    //let mut functions_to_plot: Vec<String> = Vec::new();
    let mut functions_to_plot: BTreeMap<String,BTreeMap<String, (f64, f64)>> = BTreeMap::new();
    let mut x_vals: Vec<String> = Vec::new();
    let mut plot_iostats_main: Plot = Plot::new();

    for awr in awrs{
        if awr.snap_info.begin_snap_id < *f_begin_snap || awr.snap_info.begin_snap_id > *f_end_snap {
            continue;
        }
        x_vals.push(format!("{} ({})",awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id));
        // Process each IO function
        for func in IO_FUNCTIONS.iter() {
            let func_name = func.to_string();
            let entry = io_stats_byfunc.get_mut(&func_name).unwrap();
            
            if let Some(stat) = awr.io_stats_byfunc.get(*func) {
                // Function exists in this AWR, use its values
                entry.reads_data.push(stat.reads_data);
                entry.reads_req_s.push(stat.reads_req_s);
                entry.reads_data_s.push(stat.reads_data_s);
                entry.writes_data.push(stat.writes_data);
                entry.writes_req_s.push(stat.writes_req_s);
                entry.writes_data_s.push(stat.writes_data_s);
                entry.waits_count.push(stat.waits_count);
                entry.avg_time.push(stat.avg_time);
            } else {
                // Function doesn't exist in this AWR, fill with zeros/None
                entry.reads_data.push(0.0);
                entry.reads_req_s.push(0.0);
                entry.reads_data_s.push(0.0);
                entry.writes_data.push(0.0);
                entry.writes_req_s.push(0.0);
                entry.writes_data_s.push(0.0);
                entry.waits_count.push(0);
                entry.avg_time.push(None);
            }
        }
    }
    
    for func in IO_FUNCTIONS.iter() {
        if let Some(stats) = io_stats_byfunc.get(*func) {
            let mean_reads_data = mean(stats.reads_data.clone()).unwrap_or(0.0);
            let mean_writes_data = mean(stats.writes_data.clone()).unwrap_or(0.0);
            let mean_waits_count = mean(stats.waits_count.iter().map(|&x| x as f64).collect()).unwrap_or(0.0);
            
            // Check if any metric has a non-zero mean
            let has_empty_data =
                mean_reads_data == 0.0 &&
                mean_writes_data == 0.0 &&
                mean_waits_count == 0.0;
                
            if !has_empty_data {
                // Calculate all means and standard deviations
                let mean_reads_req_s = mean(stats.reads_req_s.clone()).unwrap_or(0.0);
                let mean_reads_data_s = mean(stats.reads_data_s.clone()).unwrap_or(0.0);
                let mean_writes_req_s = mean(stats.writes_req_s.clone()).unwrap_or(0.0);
                let mean_writes_data_s = mean(stats.writes_data_s.clone()).unwrap_or(0.0);
                
                let std_reads_data = std_deviation(stats.reads_data.clone()).unwrap_or(0.0);
                let std_reads_req_s = std_deviation(stats.reads_req_s.clone()).unwrap_or(0.0);
                let std_reads_data_s = std_deviation(stats.reads_data_s.clone()).unwrap_or(0.0);
                let std_writes_data = std_deviation(stats.writes_data.clone()).unwrap_or(0.0);
                let std_writes_req_s = std_deviation(stats.writes_req_s.clone()).unwrap_or(0.0);
                let std_writes_data_s = std_deviation(stats.writes_data_s.clone()).unwrap_or(0.0);
                
                let std_waits_count = std_deviation(stats.waits_count.iter().map(|&x| x as f64).collect()).unwrap_or(0.0);
                
                // Handle avg_time (Option<f64>)
                let avg_time_values: Vec<f64> = stats.avg_time.iter().filter_map(|&x| x).collect();
                let mean_avg_time = if !avg_time_values.is_empty() {
                    mean(avg_time_values.clone()).unwrap_or(0.0)
                } else {
                    0.0
                };
                let std_avg_time = if !avg_time_values.is_empty() {
                    std_deviation(avg_time_values).unwrap_or(0.0)
                } else {
                    0.0
                };
                
                let mut stats_map: BTreeMap<String, (f64, f64)> = BTreeMap::new();
                stats_map.insert("reads_data".to_string(), (mean_reads_data, std_reads_data));
                stats_map.insert("reads_req_s".to_string(), (mean_reads_req_s, std_reads_req_s));
                stats_map.insert("reads_data_s".to_string(), (mean_reads_data_s, std_reads_data_s));
                stats_map.insert("writes_data".to_string(), (mean_writes_data, std_writes_data));
                stats_map.insert("writes_req_s".to_string(), (mean_writes_req_s, std_writes_req_s));
                stats_map.insert("writes_data_s".to_string(), (mean_writes_data_s, std_writes_data_s));
                stats_map.insert("waits_count".to_string(), (mean_waits_count, std_waits_count));
                stats_map.insert("avg_time".to_string(), (mean_avg_time, std_avg_time));
                
                // Insert into the main HashMap
                functions_to_plot.insert(func.to_string(), stats_map);
            }
        }
    }

    for func in functions_to_plot.keys(){
        let mut plot_iostats: Plot = Plot::new();
        let reads_data = BoxPlot::new(io_stats_byfunc[func].reads_data.clone())
                                        .name("Reads Data(MB)")
                                        .x_axis("x1")
                                        .y_axis("y1")
                                        .box_mean(BoxMean::True)
                                        .show_legend(false)
                                        .box_points(BoxPoints::All)
                                        .whisker_width(0.2)
                                        .marker(Marker::new().color("#003366".to_string()).opacity(0.7).size(2));
        let reads_req_s = BoxPlot::new(io_stats_byfunc[func].reads_req_s.clone())
                                        .name("ReadsReq/s")
                                        .x_axis("x2")
                                        .y_axis("y2")
                                        .box_mean(BoxMean::True)
                                        .show_legend(false)
                                        .box_points(BoxPoints::All)
                                        .whisker_width(0.2)
                                        .marker(Marker::new().color("#0066CC".to_string()).opacity(0.7).size(2));
        let reads_data_s = BoxPlot::new(io_stats_byfunc[func].reads_data_s.clone())
                                        .name("Reads Data(MB)/s")
                                        .x_axis("x3")
                                        .y_axis("y3")
                                        .box_mean(BoxMean::True)
                                        .show_legend(false)
                                        .box_points(BoxPoints::All)
                                        .whisker_width(0.2)
                                        .marker(Marker::new().color("#66B2FF".to_string()).opacity(0.7).size(2));
        let writes_data = BoxPlot::new(io_stats_byfunc[func].writes_data.clone())
                                        .name("Writes Data(MB)")
                                        .x_axis("x4")
                                        .y_axis("y4")
                                        .box_mean(BoxMean::True)
                                        .show_legend(false)
                                        .box_points(BoxPoints::All)
                                        .whisker_width(0.2)
                                        .marker(Marker::new().color("#800000".to_string()).opacity(0.7).size(2));
        let writes_req_s = BoxPlot::new(io_stats_byfunc[func].writes_req_s.clone())
                                        .name("WritesReq/s")
                                        .x_axis("x5")
                                        .y_axis("y5")
                                        .box_mean(BoxMean::True)
                                        .show_legend(false)
                                        .box_points(BoxPoints::All)
                                        .whisker_width(0.2)
                                        .marker(Marker::new().color("#FF0000".to_string()).opacity(0.7).size(2));
        let writes_data_s = BoxPlot::new(io_stats_byfunc[func].writes_data_s.clone())
                                        .name("Writes Data(MB)/s")
                                        .x_axis("x6")
                                        .y_axis("y6")
                                        .box_mean(BoxMean::True)
                                        .show_legend(false)
                                        .box_points(BoxPoints::All)
                                        .whisker_width(0.2)
                                        .marker(Marker::new().color("#FF6666".to_string()).opacity(0.7).size(2));                                    
        let waits_count = BoxPlot::new(io_stats_byfunc[func].waits_count.clone())
                                        .name("Waits Count")
                                        .x_axis("x7")
                                        .y_axis("y7")
                                        .box_mean(BoxMean::True)
                                        .show_legend(false)
                                        .box_points(BoxPoints::All)
                                        .whisker_width(0.2)
                                        .marker(Marker::new().color("#FF8800".to_string()).opacity(0.7).size(2));
        let avg_time = BoxPlot::new(io_stats_byfunc[func].avg_time.clone())
                                        .name("Avg Time(ms)")
                                        .x_axis("x8")
                                        .y_axis("y8")
                                        .box_mean(BoxMean::True)
                                        .show_legend(false)
                                        .box_points(BoxPoints::All)
                                        .whisker_width(0.2)
                                        .marker(Marker::new().color("#00AA00".to_string()).opacity(0.7).size(2));                        
        let reads_data_scat = Scatter::new(x_vals.clone(), io_stats_byfunc[func].reads_data.clone())
                                    .mode(Mode::Lines)
                                    .name(format!("{} Reads Data",func))
                                    //.marker(Marker::new().color(colors[12]))
                                    .x_axis("x1")
                                    .y_axis("y1");
        let writes_data_scat = Scatter::new(x_vals.clone(), io_stats_byfunc[func].writes_data.clone())
                                    .mode(Mode::Lines)
                                    .name(format!("{} Writes Data",func))
                                    //.marker(Marker::new().color(colors[12]))
                                    .x_axis("x1")
                                    .y_axis("y1");
        let reads_req_s_scat = Scatter::new(x_vals.clone(), io_stats_byfunc[func].reads_req_s.clone())
                                    .mode(Mode::Lines)
                                    .name(format!("{} Reads Req/s",func))
                                    //.marker(Marker::new().color(colors[12]))
                                    .x_axis("x1")
                                    .y_axis("y3");
        let writes_req_s_scat = Scatter::new(x_vals.clone(), io_stats_byfunc[func].writes_req_s.clone())
                                    .mode(Mode::Lines)
                                    .name(format!("{} Writes Req/s",func))
                                    //.marker(Marker::new().color(colors[12]))
                                    .x_axis("x1")
                                    .y_axis("y3");
        let reads_data_s_scat = Scatter::new(x_vals.clone(), io_stats_byfunc[func].reads_data_s.clone())
                                    .mode(Mode::Lines)
                                    .name(format!("{} Reads Data/s",func))
                                    //.marker(Marker::new().color(colors[12]))
                                    .x_axis("x1")
                                    .y_axis("y2");
        let writes_data_s_scat = Scatter::new(x_vals.clone(), io_stats_byfunc[func].writes_data_s.clone())
                                    .mode(Mode::Lines)
                                    .name(format!("{} Writes Data/s",func))
                                    //.marker(Marker::new().color(colors[12]))
                                    .x_axis("x1")
                                    .y_axis("y2");
        let waits_count_scat = Scatter::new(x_vals.clone(), io_stats_byfunc[func].waits_count.clone())
                                    .mode(Mode::Lines)
                                    .name(format!("{} Wait Count",func))
                                    //.marker(Marker::new().color(colors[12]))
                                    .x_axis("x1")
                                    .y_axis("y4");
        let avg_time_scat = Scatter::new(x_vals.clone(), io_stats_byfunc[func].avg_time.clone())
                                    .mode(Mode::Lines)
                                    .name(format!("{} Wait AVG Time",func))
                                    //.marker(Marker::new().color(colors[12]))
                                    .x_axis("x1")
                                    .y_axis("y5");
        plot_iostats.add_trace(reads_data);
        plot_iostats.add_trace(reads_req_s);
        plot_iostats.add_trace(reads_data_s);
        plot_iostats.add_trace(writes_data);
        plot_iostats.add_trace(writes_req_s);
        plot_iostats.add_trace(writes_data_s);
        plot_iostats.add_trace(waits_count);
        plot_iostats.add_trace(avg_time);
        plot_iostats_main.add_trace(reads_data_scat);
        plot_iostats_main.add_trace(writes_data_scat);
        plot_iostats_main.add_trace(reads_data_s_scat);
        plot_iostats_main.add_trace(writes_data_s_scat);
        plot_iostats_main.add_trace(reads_req_s_scat);
        plot_iostats_main.add_trace(writes_req_s_scat);
        plot_iostats_main.add_trace(waits_count_scat);
        plot_iostats_main.add_trace(avg_time_scat);

        let layout_iostats: Layout = Layout::new()
            .height(400)
            .grid(
                LayoutGrid::new()
                    .rows(1)
                    .columns(1),
                    //.row_order(Grid::TopToBottom),
            )
            .hover_mode(HoverMode::X)
            .x_axis8(
                Axis::new()
                        .domain(&[0.91, 1.0])
                        .anchor("y8")
                        .range(vec![0.,])
                        .show_grid(false)
            )
            .y_axis8(
                Axis::new()
                        .domain(&[0.0, 1.0])
                        .anchor("x8")
                        .range(vec![0.,])
                        .range_mode(RangeMode::ToZero)
                        .show_grid(false),
            )
            .x_axis7(
                Axis::new()
                        .domain(&[0.78, 0.87])
                        .anchor("y7")
                        .range(vec![0.,])
                        .show_grid(false)
            )
            .y_axis7(
                Axis::new()
                        .domain(&[0.0, 1.0])
                        .anchor("x7")
                        .range(vec![0.,])
                        .range_mode(RangeMode::ToZero)
                        .show_grid(false),
            )
            .x_axis6(
                Axis::new()
                        .domain(&[0.65, 0.74])
                        .anchor("y6")
                        .range(vec![0.,])
                        .show_grid(false)
            )
            .y_axis6(
                Axis::new()
                        .domain(&[0.0, 1.0])
                        .anchor("x6")
                        .range(vec![0.,])
                        .range_mode(RangeMode::ToZero)
                        .show_grid(false),
            )
            .x_axis5(
                Axis::new()
                        .domain(&[0.52, 0.61])
                        .anchor("y5")
                        .range(vec![0.,])
                        .show_grid(false)
            )
            .y_axis5(
                Axis::new()
                        .domain(&[0.0, 1.0])
                        .anchor("x5")
                        .range(vec![0.,])
                        .range_mode(RangeMode::ToZero)
                        .show_grid(false),
            )
            .x_axis4(
                Axis::new()
                        .domain(&[0.39, 0.48])
                        .anchor("y4")
                        .range(vec![0.,])
                        .show_grid(false)
            )
            .y_axis4(
                Axis::new()
                        .domain(&[0.0, 1.0])
                        .anchor("x4")
                        .range(vec![0.,])
                        .range_mode(RangeMode::ToZero)
                        .show_grid(false),
            )
            .x_axis3(
                Axis::new()
                        .domain(&[0.26, 0.35])
                        .anchor("y3")
                        .range(vec![0.,])
                        .show_grid(false)
            )
            .y_axis3(
                Axis::new()
                        .domain(&[0.0, 1.0])
                        .anchor("x3")
                        .range(vec![0.,])
                        .range_mode(RangeMode::ToZero)
                        .show_grid(false),
            )
            .x_axis2(
                Axis::new()
                        .domain(&[0.13, 0.22])
                        .anchor("y2")
                        .range(vec![0.,])
                        .show_grid(false)
            )
            .y_axis2(
                Axis::new()
                        .domain(&[0.0, 1.0])
                        .anchor("x2")
                        .range(vec![0.,])
                        .range_mode(RangeMode::ToZero)
                        .show_grid(false),
            )
            .x_axis(
                Axis::new()
                        .domain(&[0.0, 0.09])
                        .anchor("y1")
                        .range(vec![0.,])
                        .show_grid(false)
            )
            .y_axis(
                Axis::new()
                        .domain(&[0.0, 1.0])
                        .anchor("x1")
                        .title(func)
                        .range(vec![0.,])
                        .range_mode(RangeMode::ToZero)
                        .show_grid(false),
            );
        plot_iostats.set_layout(layout_iostats);
        let func_name = func.replace(" ","_");
        let file_name: String = format!("{}/iostats/iostats_{}.html", dirpath,func_name);
        let path: &Path = Path::new(&file_name);
        plot_iostats.write_html(path);
    }

    let layout_io_stats_main: Layout = Layout::new()
            .height(800)
            .hover_mode(HoverMode::X)
            .grid(
                LayoutGrid::new()
                    .rows(5)
                    .columns(1),
            )
            .x_axis(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("y5")
                    .range(vec![0.,])
                    .show_grid(true)
            )
            .y_axis(
                Axis::new()
                    .domain(&[0.82, 1.0])
                    .anchor("x1")
                    .range(vec![0.,])
                    .title("MB")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero)
            )
            .y_axis2(
                Axis::new()
                    .domain(&[0.62, 0.80])
                    .anchor("x1")
                    .range(vec![0.,])
                    .title("MB/s")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero)
            )
            .y_axis3(
                Axis::new()
                    .domain(&[0.41, 0.60])
                    .anchor("x1")
                    .range(vec![0.,])
                    .title("Req/s")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero)
            )
            .y_axis4(
                Axis::new()
                    .domain(&[0.21, 0.39])
                    .anchor("x1")
                    .range(vec![0.,])
                    .title("#")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero)
            )
            .y_axis5(
                Axis::new()
                    .domain(&[0.0, 0.19])
                    .anchor("x1")
                    .range(vec![0.,])
                    .title("ms")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero)
            );
            plot_iostats_main.set_layout(layout_io_stats_main);
            let file_name: String = format!("{}/iostats/iostats_zMAIN.html", dirpath);
            let path: &Path = Path::new(&file_name);
            plot_iostats_main.write_html(path);

    println!("Saving plots for IO Stats to '{}/iostats_*'", dirpath);
    functions_to_plot.insert("zMAIN".to_string(),BTreeMap::new());
    functions_to_plot
}

// Get Requests – popularność latcha (jak często był używany).
// Pct Get Miss – jak często proces się “odbijał” od latcha (wskazuje kontencję).
// Avg Slps/Miss – czy procesy musiały iść spać (jeśli > 0 → latch contention naprawdę boli).
// Wait Time (s) – łączny koszt dla systemu (sumaryczna strata czasu).
// NoWait Requests / Pct NoWait Miss – zwykle mniej krytyczne, ale czasem pokazują krótkie zatory.
fn generate_latchstats_plotfiles(awrs: &Vec<AWR>, snap_range: &(u64,u64), dirpath: &str, report_for_ai: &mut ReportForAI) -> Table {
    let (f_begin_snap,f_end_snap) = snap_range;
    #[derive(Default)]
    struct LatchAgg {
        get_requests_sum: u64,
        weighted_miss_pct: f64, // sum(get_requests * get_pct_miss)
        occurrences: f64,
        wait_time_sum: f64,
    }

    let mut latch_stat_rows = String::new(); //for HTML
    let mut latches: Vec<String> = Vec::new();       //latch names
    for lname in &awrs[0].latch_activity{
        latches.push(lname.statname.clone());
    }

    let latch_activity: HashMap<String, LatchAgg> = latches.par_iter()
        .map(|lname| {
            let mut agg: LatchAgg = LatchAgg::default();
            for awr in awrs{
                if awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap {
                    for la in awr.latch_activity.iter().filter(|la| la.statname == *lname) {
                        if la.get_requests > 0 { 
                            agg.get_requests_sum = agg.get_requests_sum.saturating_add(la.get_requests);
                            agg.weighted_miss_pct += (la.get_requests as f64) * la.get_pct_miss;
                            agg.wait_time_sum += (la.get_requests as f64)*la.wait_time;
                            agg.occurrences += 1.0; 
                        }
                    }
                }
            }
            if agg.get_requests_sum > 0 {
                agg.weighted_miss_pct = agg.weighted_miss_pct / (agg.get_requests_sum as f64);
                agg.wait_time_sum = agg.wait_time_sum / (agg.get_requests_sum as f64);
            } else {
                agg.weighted_miss_pct = 0.0;
                agg.wait_time_sum = 0.0;
            }
            (lname.clone(), agg)
        }).collect();

    let mut sorted_latches: Vec<(String, LatchAgg)> = latch_activity.into_iter().collect();
    sorted_latches.sort_by(|a, b| b.1.weighted_miss_pct.partial_cmp(&a.1.weighted_miss_pct).unwrap());
    
    let mut latch_table: Table = Table::new();
    latch_table.set_titles(Row::new(vec![
        Cell::new("Latch").with_style(Attr::Bold),
        Cell::new("Get Req avg").with_style(Attr::Bold),
        Cell::new("Weighted Miss %").with_style(Attr::Bold),
        Cell::new("Wait Time (s) wavg").with_style(Attr::Bold),
        Cell::new("In AWR %").with_style(Attr::Bold),
    ]));

    for (lname, agg) in &sorted_latches {
        let mut latch_activity = LatchActivitySummary::default();
        if agg.weighted_miss_pct > 0.0 {
            latch_table.add_row(Row::new(vec![
                Cell::new(lname),
                Cell::new(&format!("{:.2}",(agg.get_requests_sum as f64/agg.occurrences as f64))),
                Cell::new(&format!("{:.4}",agg.weighted_miss_pct)),
                Cell::new(&format!("{:.2}",agg.wait_time_sum)),
                Cell::new(&format!("{:.2}",(agg.occurrences as f64*100.0/awrs.len() as f64))),
            ]));
            latch_activity.latch_name = lname.clone();
            latch_activity.get_requests_avg = agg.get_requests_sum as f64/agg.occurrences as f64;
            latch_activity.weighted_miss_pct = agg.weighted_miss_pct;
            latch_activity.wait_time_weighted_avg_s = agg.wait_time_sum;
            latch_activity.found_in_pct_of_probes = (agg.occurrences as f64*100.0/awrs.len() as f64);

            report_for_ai.latch_activity_summary.push(latch_activity);

            latch_stat_rows.push_str(&format!(
                r#"<tr>
                    <td>{}</td>
                    <td>{:.2}</td>
                    <td>{:.4}</td>
                    <td>{:.2}</td>
                    <td>{:.2}</td>
                </tr>"#,
                lname,(agg.get_requests_sum as f64/agg.occurrences as f64),agg.weighted_miss_pct,agg.wait_time_sum,(agg.occurrences as f64*100.0/awrs.len() as f64)
            ));
        }
    }
    let table_latch_stat: String = format!(
        r#"
        <table id="latchstat-table">
            <thead>
                <tr style="background-color: #f49758;">
                    <th colspan="5" style="text-align: center; font-weight: bold; color: rgba(125, 0, 63, 10); font-size: 1.1em;">Latch Activity Summary</th>
                </tr>
                <tr style="background-color: #f49758;">
                    <th onclick="sortTable('latchstat-table',0)" style="cursor: pointer;">Latch Name</th>
                    <th onclick="sortTable('latchstat-table',1)" style="cursor: pointer;">Get Req avg</th>
                    <th onclick="sortTable('latchstat-table',2)" style="cursor: pointer;">Weighted Miss %</th>
                    <th onclick="sortTable('latchstat-table',3)" style="cursor: pointer;">Wait Time (s) wavg</th>
                    <th onclick="sortTable('latchstat-table',4)" style="cursor: pointer;">In AWR %</th>
                </tr>
            </thead>
            <tbody>
            {}
            </tbody>
        </table>
        "#,
        latch_stat_rows
    );
    let latch_stats_filename: String = format!("{}/latches/latchstats_activity.html", dirpath);
    if let Err(e) = fs::write(&latch_stats_filename, table_latch_stat) {
        eprintln!("Error writing file {}: {}", latch_stats_filename, e);
    }
    //println!("Saved plots for Latch Activity Stats to '{}/latchstats_activity.html'", dirpath);
    //println!("{}\n", latch_table);
    latch_table
}

fn report_segments_summary(awrs: &Vec<AWR>, args: &Args, logfile_name: &str, dir: &str, raport_for_ai: &mut ReportForAI) -> Vec<String> {

    //It will contain section name and vector for all segment stats from the whole AWR collection
    let mut objects_in_section: BTreeMap<String, Vec<SegmentStats>> = BTreeMap::new();

    for awr in awrs {
        for (section_name, segments) in &awr.segment_stats {
            objects_in_section
                .entry(section_name.clone())
                .or_insert_with(Vec::new)
                .extend(segments.clone());
        }
    }

    let mut sections_toplot: Vec<String> = Vec::new();    
    for (section, objects) in objects_in_section {
        sections_toplot.push(objects[0].stat_name.replace(" ","_"));
        let section_msg = format!("TOP 10 Segments by {} ordered by PCT of occuriance desc. Statstic values computed based on {}\n", section, objects[0].stat_name);
        make_notes!(logfile_name, args.quiet, 0, "\n");
        make_notes!(logfile_name, args.quiet, 2, "{}", section_msg.yellow());
        let mut table = Table::new();
        let mut segment_stat_rows = String::new();

        if args.security_level > 0 { //In security level 0 there is no segment name
            table.set_titles(Row::new(vec![
                Cell::new("Segment Name"),
                Cell::new("Segment Type"),
                Cell::new("Object Id"),
                Cell::new("Data Object Id"),
                Cell::new("AVG"),
                Cell::new("STDDEV"),
                Cell::new("PCT of occuriance")
            ]));
        } else {
            table.set_titles(Row::new(vec![
                Cell::new("Segment Type"),
                Cell::new("Object Id"),
                Cell::new("Data Object Id"),
                Cell::new("AVG"),
                Cell::new("STDDEV"),
                Cell::new("PCT of occuriance")
            ]));
        }
        
        struct SegmentSummary {
            segment_name: String,
            segment_type: String,
            object_id: String,
            data_object_id: String,
            stat_name: String,
            avg: String,
            stddev: String,
            pct: String,
        }

        let mut segment_summary: BTreeMap<(i64, u64, u64), SegmentSummary> = BTreeMap::new();
        //build unique set od object_id, data_object_id from all objects in current section
        let all_ids: HashSet<(u64,u64, String)> = objects
                                      .iter()
                                      .map(|s| (s.obj, s.objd, s.object_name.clone()))
                                      .collect(); 
                
        for id in all_ids { //iterate over each object id
            let mut all_values: Vec<f64> = Vec::new();

            objects
            .iter()
            .for_each(|s| {
                if s.obj == id.0 && s.objd == id.1 && s.object_name == id.2{
                    all_values.push(s.stat_vlalue);
                }
            }); //build vector of statistic values 

            let segment_data = objects
                                         .iter()
                                         .find(|s| s.obj == id.0 && s.objd == id.1 && s.object_name == id.2)
                                         .unwrap(); //get details of the given object id

            let avg = mean(all_values.clone()).unwrap();
            let stddev = std_deviation(all_values.clone()).unwrap();
            let pct = (all_values.len() as f64 / awrs.len() as f64) * 100.0;

            let pct_key: i64 = (pct*-100000.0) as i64; //this will be used to sort BTree over PCT - it will negative number to get the biggest value on top

            segment_summary.insert((pct_key, id.0, id.1), SegmentSummary{
                segment_name: segment_data.object_name.clone(),
                segment_type: segment_data.object_type.clone(),
                object_id: id.0.to_string(),
                data_object_id: id.1.to_string(),
                stat_name: segment_data.stat_name.clone(),
                avg: format!("{:.3}", avg),
                stddev: format!("{:.3}", stddev),
                pct: format!("{:.3}", pct),
            });
            
        }

        //Iterate over top 10 segment statistics
        for (_, s) in segment_summary.iter().take(10) {

            let segment_data = Top10SegmentStats {
                    segment_name: s.segment_name.clone(),
                    segment_type: s.segment_type.clone(),
                    object_id: u64::from_str(&s.object_id).unwrap(),
                    data_object_id: u64::from_str(&s.data_object_id).unwrap(),
                    avg: f64::from_str(&s.avg).unwrap(),
                    stddev: f64::from_str(&s.stddev).unwrap(),
                    pct_of_occuriance: f64::from_str(&s.pct).unwrap(),
                };

            if section == "Buffer Busy Waits" {
                raport_for_ai.top_10_segments_by_buffer_busy_waits.push(segment_data);
            } else if section == "Direct Physical Reads" {
                raport_for_ai.top_10_segments_by_direct_physical_reads.push(segment_data);
            } else if section == "Direct Physical Writes" {
                raport_for_ai.top_10_segments_by_direct_physical_writes.push(segment_data);
            } else if section == "Logical Reads" {
                raport_for_ai.top_10_segments_by_logical_reads.push(segment_data);
            } else if section == "Physical Read Requests" {
                raport_for_ai.top_10_segments_by_physical_read_requests.push(segment_data);
            } else if section == "Physical Write Requests" {
                raport_for_ai.top_10_segments_by_physical_write_requests.push(segment_data);
            } else if section == "Physical Writes" {
                raport_for_ai.top_10_segments_by_physical_writes.push(segment_data);
            } else if section == "Row Lock Waits" {
                raport_for_ai.top_10_segments_by_row_lock_waits.push(segment_data);
            }

            if args.security_level > 0 {
                table.add_row(Row::new(vec![
                    Cell::new(&s.segment_name),
                    Cell::new(&s.segment_type),
                    Cell::new(&s.object_id),
                    Cell::new(&s.data_object_id),
                    Cell::new(&s.avg),
                    Cell::new(&s.stddev),
                    Cell::new(&s.pct)
                ]));
                segment_stat_rows.push_str(&format!(
                    r#"<tr>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                    </tr>"#,
                    &s.segment_name,&s.segment_type,&s.object_id,&s.data_object_id, &s.avg, &s.stddev,&s.pct
                ));
            } else {
                table.add_row(Row::new(vec![
                    Cell::new(&s.segment_type),
                    Cell::new(&s.object_id),
                    Cell::new(&s.data_object_id),
                    Cell::new(&s.avg),
                    Cell::new(&s.stddev),
                    Cell::new(&s.pct)
                ]));
                segment_stat_rows.push_str(&format!(
                    r#"<tr>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                    </tr>"#,
                    &s.segment_type,&s.object_id,&s.data_object_id, &s.avg, &s.stddev,&s.pct
                ));
            }
            
        }

        for table_line in table.to_string().lines() {
            make_notes!(logfile_name, args.quiet, 0, "{}\n", table_line);
        }
        
        if args.security_level > 0 {
            let table_segment_stat: String = format!(
                r#"
                <table id="segstat-{idname}-table" style="display: none">
                    <thead>
                        <tr style="background-color: #3cbdc9;">
                            <th colspan="7" style="text-align: center; font-weight: bold; color: rgba(255, 0, 103, 1); font-size: 1.1em;">TOP 10 Segments by {idname}</th>
                        </tr>
                        <tr style="background-color: #3cbdc9;">
                            <th onclick="sortTable('segstat-{idname}-table',0)" style="cursor: pointer;">Segment Name</th>
                            <th onclick="sortTable('segstat-{idname}-table',1)" style="cursor: pointer;">Segment Type</th>
                            <th onclick="sortTable('segstat-{idname}-table',2)" style="cursor: pointer;">Object Id</th>
                            <th onclick="sortTable('segstat-{idname}-table',3)" style="cursor: pointer;">Data Object Id</th>
                            <th onclick="sortTable('segstat-{idname}-table',4)" style="cursor: pointer;">AVG</th>
                            <th onclick="sortTable('segstat-{idname}-table',5)" style="cursor: pointer;">STDDEV</th>
                            <th onclick="sortTable('segstat-{idname}-table',6)" style="cursor: pointer;">% of occurrence</th>
                        </tr>
                    </thead>
                    <tbody>
                    {rows}
                    </tbody>
                </table>
                "#,
                idname = objects[0].stat_name.replace(" ","_"),
                rows = segment_stat_rows
            );
            let segment_stats_filename: String = format!("{}/segstats/segstats_{}.html", dir, objects[0].stat_name.replace(" ","_"),);
            if let Err(e) = fs::write(&segment_stats_filename, table_segment_stat) {
                eprintln!("Error writing file {}: {}", segment_stats_filename, e);
            }
        } else {
            let table_segment_stat: String = format!(
                r#"
                <table id="segstat-{idname}-table" style="display: none">
                    <thead>
                        <tr style="background-color: #3cbdc9;">
                            <th colspan="6" style="text-align: center; font-weight: bold; color: rgba(255, 0, 103, 1);font-size: 1.1em;">TOP 10 Segments by {idname}</th>
                        </tr>
                        <tr style="background-color: #3cbdc9;">
                            <th onclick="sortTable('segstat-{idname}-table',1)" style="cursor: pointer;">Segment Type</th>
                            <th onclick="sortTable('segstat-{idname}-table',2)" style="cursor: pointer;">Object Id</th>
                            <th onclick="sortTable('segstat-{idname}-table',3)" style="cursor: pointer;">Data Object Id</th>
                            <th onclick="sortTable('segstat-{idname}-table',4)" style="cursor: pointer;">AVG</th>
                            <th onclick="sortTable('segstat-{idname}-table',5)" style="cursor: pointer;">STDDEV</th>
                            <th onclick="sortTable('segstat-{idname}-table',6)" style="cursor: pointer;">% of occurrence</th>
                        </tr>
                    </thead>
                    <tbody>
                    {rows}
                    </tbody>
                </table>
                "#,
                idname = objects[0].stat_name.replace(" ","_"),
                rows = segment_stat_rows
            );
            let segment_stats_filename: String = format!("{}/segstats/segstats_{}.html", dir, objects[0].stat_name.replace(" ","_"),);
            if let Err(e) = fs::write(&segment_stats_filename, table_segment_stat) {
                eprintln!("Error writing file {}: {}", segment_stats_filename, e);
            }
        };
    }
    sections_toplot   
}

pub fn main_report_builder(collection: AWRSCollection, args: Args) -> ReportForAI {
    let mut plot_main: Plot = Plot::new();
    let mut plot_highlight: Plot = Plot::new();
    let mut plot_highlight2: Plot = Plot::new();
    let mut global_statistics = BTreeMap::<String, Option<GetStats>>::new();

    /* Struct filled for AI analyzes in JSON */
    let mut report_for_ai: ReportForAI = ReportForAI::default();
    /* ************************************* */
    
    let db_time_cpu_ratio: f64 = args.time_cpu_ratio;
    let filter_db_time: f64 = args.filter_db_time;
    let snap_range: (u64,u64) = parse_snap_range(&args.snap_range).expect("Invalid snap-range argument");
    
    //Filenames and Paths used to save JAS-MIN files
    let mut logfile_name = PathBuf::from(&args.directory).with_extension("txt").to_string_lossy().into_owned();
    if logfile_name.is_empty() && !&args.json_file.is_empty() {
        if let Some(stem) = PathBuf::from(&args.json_file).file_stem() {
            logfile_name = PathBuf::from(stem).with_extension("txt").to_string_lossy().into_owned();
        } 
    }
    let logfile_path = Path::new(&logfile_name);
    println!("Starting output capture to: {}", logfile_path.display() );
    if logfile_path.exists() { //remove logfile if it exists - the notes made by JAS-MIN has to be created each time
        fs::remove_file(&logfile_path).unwrap();
    }

    let mut html_dir = PathBuf::from(&args.directory).with_extension("html_reports").to_string_lossy().into_owned();
    if html_dir.is_empty() && !&args.json_file.is_empty() {
        if let Some(stem) = PathBuf::from(&args.json_file).file_stem() {
            html_dir = PathBuf::from(stem).with_extension("html_reports").to_string_lossy().into_owned();
        } 
    }
    // Create main <PATH>.html_reports folder
    if let Err(e) = fs::create_dir_all(&html_dir) {
        eprintln!("⚠️ Failed to create base directory {:?}: {}", html_dir, e);
    }
    // Create all required subdirectories dir tree under html
    let subdirs = ["fg", "bg", "latches", "iostats", "segstats", "sqlid", "stats","jasmin/anomalies"];
    for sub in subdirs {
        let path = Path::new(&html_dir).join(sub);
        if let Err(e) = fs::create_dir_all(&path) {
            eprintln!("⚠️ Failed to create directory {:?}: {}", path, e);
        }
    }
    
    // Y-axis
    let mut y_vals_dbtime: Vec<f64> = Vec::new();
    let mut y_vals_dbcpu: Vec<f64> = Vec::new();
    let mut y_vals_events: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_bgevents: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_logons: Vec<u64> = Vec::new();
    let mut y_vals_logouts: Vec<u64> = Vec::new();
    let mut y_vals_calls: Vec<f64> = Vec::new();
    let mut y_vals_execs: Vec<f64> = Vec::new();
    let mut y_vals_trans: Vec<f64> = Vec::new();
    let mut y_vals_redosize: Vec<f64> = Vec::new();
    let mut y_vals_parses: Vec<f64> = Vec::new();
    let mut y_vals_hparses: Vec<f64> = Vec::new();
    let mut y_vals_cpu_user: Vec<f64> = Vec::new();
    let mut y_vals_cpu_load: Vec<f64> = Vec::new();
    let mut y_vals_cpu_count: Vec<u32> = Vec::new();
    let mut y_vals_redo_switches: Vec<f64> = Vec::new();
    let mut y_excessive_commits: Vec<f64> = Vec::new();
    let mut y_cleanout_ktugct: Vec<f64> = Vec::new();
    let mut y_cleanout_cr: Vec<f64> = Vec::new();
    let mut y_read_mb: Vec<f64> = Vec::new();
    let mut y_write_mb: Vec<f64> = Vec::new();
    let mut y_user_commits: Vec<u64> = Vec::new();
    let mut y_user_rollbacks: Vec<u64> = Vec::new();
    let mut y_logical_reads_s: Vec<f64> = Vec::new();
    let mut y_block_changes_s: Vec<f64> = Vec::new();
    let mut y_failed_parse_count: Vec<u64> = Vec::new();
    /*Variables used for statistics computations*/
    let mut y_vals_events_n: BTreeMap<String, Vec<f64>> = BTreeMap::new(); 
    let mut y_vals_events_t: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_events_s: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_bgevents_n: BTreeMap<String, Vec<f64>> = BTreeMap::new(); 
    let mut y_vals_bgevents_t: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_bgevents_s: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls_exec_t: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls_exec_n: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls_exec_s: BTreeMap<String, Vec<f64>> = BTreeMap::new(); //For Elapsed Time AVG STDDEV calculations
    /*HashMap for calculating instance stats correlation*/
    let mut instance_stats: HashMap<String, Vec<f64>> = HashMap::new();
    // X-axis -> snaps
    let mut x_vals: Vec<String> = Vec::new();
    
    println!("{}","\n==== ANALYZING ===".bold().bright_cyan());
    let top_stats: TopStats = find_top_stats(&collection.awrs, db_time_cpu_ratio, filter_db_time, &snap_range, &logfile_name, &args, &mut report_for_ai);  
    let mut is_logfilesync_high: bool = false;
    
    println!("{}","\n==== CREATING PLOTS ===".bold().bright_cyan()); 
    generate_events_plotfiles(&collection.awrs, &top_stats.events, true, &snap_range, &html_dir);
    generate_events_plotfiles(&collection.awrs, &top_stats.bgevents, false, &snap_range, &html_dir);
    generate_sqls_plotfiles(&collection.awrs, &top_stats, &snap_range, &html_dir);
    let instance_eff_plot: String = generate_instance_efficiency_plot(&collection.awrs, &snap_range, &html_dir);
    generate_instance_stats_plotfiles(&collection.awrs, &snap_range, &html_dir);
    let iostats = generate_iostats_plotfile(&collection.awrs, &snap_range, &html_dir);
    let table_latch: Table = generate_latchstats_plotfiles(&collection.awrs, &snap_range, &html_dir, &mut report_for_ai);
    let fname: String = format!("{}/jasmin_main.html", &html_dir); //new file name path for main report
    
    println!("\n{}","==== PREPARING RESULTS ===".bold().bright_cyan());

    for awr in &collection.awrs {
        let (f_begin_snap,f_end_snap) = snap_range;
        if awr.snap_info.begin_snap_id >= f_begin_snap && awr.snap_info.end_snap_id <= f_end_snap {
            let xval: String = format!("{} ({})", awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id);
            x_vals.push(xval.clone());
            //We have to fill the whole data traces for stats, wait events and SQLs with 0 to be sure that chart won't be moved to one side
            
            for (sql, _) in &top_stats.sqls {
                y_vals_sqls.entry(sql.to_string()).or_insert(Vec::new());
                y_vals_sqls_exec_t.entry(sql.to_string()).or_insert(Vec::new());
                y_vals_sqls_exec_n.entry(sql.to_string()).or_insert(Vec::new());
                y_vals_sqls_exec_s.entry(sql.to_string()).or_insert(Vec::new());
                let mut v = y_vals_sqls.get_mut(sql).unwrap();
                v.push(0.0);
            } 

            for (event, _) in &top_stats.events {
                if event.to_string() == "log file sync"{
                    is_logfilesync_high = true;
                }
                y_vals_events.entry(event.to_string()).or_insert(Vec::new());
                y_vals_events_n.entry(event.to_string()).or_insert(Vec::new());
                y_vals_events_t.entry(event.to_string()).or_insert(Vec::new());
                y_vals_events_s.entry(event.to_string()).or_insert(Vec::new());
                let mut v = y_vals_events.get_mut(event).unwrap();
                v.push(0.0);
            }

            for (event, _) in &top_stats.bgevents {
                y_vals_bgevents.entry(event.to_string()).or_insert(Vec::new());
                y_vals_bgevents_n.entry(event.to_string()).or_insert(Vec::new());
                y_vals_bgevents_t.entry(event.to_string()).or_insert(Vec::new());
                y_vals_bgevents_s.entry(event.to_string()).or_insert(Vec::new());
                let mut v = y_vals_bgevents.get_mut(event).unwrap();
                v.push(0.0);
            }

            for (statname, _) in &top_stats.stat_names {
                instance_stats.entry(statname.to_string()).or_insert(Vec::new());
                let mut v = instance_stats.get_mut(statname).unwrap();
                v.push(0.0);
            }

            //Than we can set the current value of the vector to the desired one, if the event is in TOP section in that snap id
            for event in &awr.foreground_wait_events { 
                    if top_stats.events.contains_key(&event.event) {
                        let mut v = y_vals_events.get_mut(&event.event).unwrap();
                        v[x_vals.len()-1] = event.total_wait_time_s;
                        let mut v = y_vals_events_n.get_mut(&event.event).unwrap();
                        v.push(event.waits as f64);
                        let mut v = y_vals_events_t.get_mut(&event.event).unwrap();
                        v.push(event.pct_dbtime);
                        let mut v = y_vals_events_s.get_mut(&event.event).unwrap();
                        v.push(event.total_wait_time_s);
                    }
            }
            for event in &awr.background_wait_events { 
                if top_stats.bgevents.contains_key(&event.event) {
                    let mut v = y_vals_bgevents.get_mut(&event.event).unwrap();
                    v[x_vals.len()-1] = event.total_wait_time_s;
                    let mut v = y_vals_bgevents_n.get_mut(&event.event).unwrap();
                    v.push(event.waits as f64);
                    let mut v = y_vals_bgevents_t.get_mut(&event.event).unwrap();
                    v.push(event.pct_dbtime);
                    let mut v = y_vals_bgevents_s.get_mut(&event.event).unwrap();
                    v.push(event.total_wait_time_s);
                }
            }
            //Same with SQLs
            for sqls in &awr.sql_elapsed_time {
                    if top_stats.sqls.contains_key(&sqls.sql_id) {
                        let mut v = y_vals_sqls.get_mut(&sqls.sql_id).unwrap();
                        v[x_vals.len()-1] = sqls.elapsed_time_s;
                        let mut v = y_vals_sqls_exec_t.get_mut(&sqls.sql_id).unwrap();
                        v.push(sqls.elpased_time_exec_s);
                        let mut v = y_vals_sqls_exec_n.get_mut(&sqls.sql_id).unwrap();
                        v.push(sqls.executions as f64); 
                        let mut v = y_vals_sqls_exec_s.get_mut(&sqls.sql_id).unwrap();
                        v.push(sqls.elapsed_time_s as f64); 
                    }
            }
            let mut is_statspack: bool = false;
            //DB Time and DB CPU are in each snap, so you don't need that kind of precautions
            for lp in &awr.load_profile {
                    if lp.stat_name.starts_with("DB Time") || lp.stat_name.starts_with("DB time") {
                        y_vals_dbtime.push(lp.per_second);
                        if lp.stat_name.starts_with("DB time") {
                            is_statspack = true;
                        }
                    } else if lp.stat_name.starts_with("DB CPU") {
                        y_vals_dbcpu.push(lp.per_second);
                    } else if lp.stat_name.starts_with("User calls") {
                        y_vals_calls.push(lp.per_second);
                    } else if lp.stat_name.starts_with("Executes") {
                        y_vals_execs.push(lp.per_second);
                    } else if lp.stat_name.starts_with("Transactions") {
                            y_vals_trans.push(lp.per_second);
                    } else if lp.stat_name.starts_with("Redo size") {
                        y_vals_redosize.push(lp.per_second as f64/1024.0/1024.0);
                    } else if lp.stat_name.starts_with("Block changes") {
                        y_block_changes_s.push(lp.per_second);
                    } else if lp.stat_name.starts_with("Parses") {
                        y_vals_parses.push(lp.per_second);
                    } else if lp.stat_name.starts_with("Hard parses") {
                        y_vals_hparses.push(lp.per_second);
                    } else if lp.stat_name.starts_with("Physical read") {
                        y_read_mb.push(lp.per_second*collection.db_instance_information.db_block_size as f64/1024.0/1024.0);
                    } else if lp.stat_name.starts_with("Physical write") {
                        y_write_mb.push(lp.per_second*collection.db_instance_information.db_block_size as f64/1024.0/1024.0);
                    } else if lp.stat_name.starts_with("Logical read") {
                        y_logical_reads_s.push(lp.per_second*collection.db_instance_information.db_block_size as f64/1024.0/1024.0);
                    }
            }

            // IO Stats data gathering and preparing them for plotting
            // ----- Host CPU
            if awr.host_cpu.pct_user < 0.0 {
                y_vals_cpu_user.push(0.0);
            } else {
                y_vals_cpu_user.push(awr.host_cpu.pct_user);
            }
            y_vals_cpu_load.push(100.0-awr.host_cpu.pct_idle);
            y_vals_cpu_count.push(awr.host_cpu.cpus);

            // ----- Additionally plot Redo Log Switches
            y_vals_redo_switches.push(awr.redo_log.per_hour);

            let mut calls: u64 = 0;
            let mut commits: u64 = 0;
            let mut rollbacks: u64 = 0;
            let mut cleanout_ktugct: u64 = 0;
            let mut cleanout_cr: u64 = 0;
            let mut excessive_commit: f64 = 0.0;

            for activity in &awr.instance_stats {
                let mut v: &mut Vec<f64> = instance_stats.get_mut(&activity.statname).unwrap();
                v[x_vals.len()-1] = activity.total as f64;

                
                if activity.statname == "user commits" {
                    y_user_commits.push(activity.total);
                } else if activity.statname == "user rollbacks" {
                    y_user_rollbacks.push(activity.total);
                } else if activity.statname == "parse count (failures)" {
                    y_failed_parse_count.push(activity.total);
                } else if activity.statname == "user logons cumulative" {
                    y_vals_logons.push(activity.total);
                } else if activity.statname == "user logouts cumulative"{
                    y_vals_logouts.push(activity.total);
                }
                
                // Plot additional stats if 'log file sync' is in top events
                if is_logfilesync_high {
                
                    if activity.statname == "user calls" {
                        calls = activity.total;
                    } else if activity.statname == "user commits" {
                        commits = activity.total;
                    } else if activity.statname == "user rollbacks" {
                        rollbacks = activity.total;
                    } else if activity.statname.starts_with("cleanout - number of ktugct calls") {
                        cleanout_ktugct = activity.total;
                    } else if activity.statname.starts_with("cleanouts only - consistent read") {
                        cleanout_cr = activity.total;
                    }
                    excessive_commit = if commits + rollbacks > 0 {
                        (calls as f64) / ((commits + rollbacks) as f64)
                        } else {
                            0.0
                    };
                }
            }
            if is_logfilesync_high {
                    y_excessive_commits.push(excessive_commit);
                    /* This is for printing delayed block cleanouts when log file sync is present*/
                    y_cleanout_ktugct.push(cleanout_ktugct as f64);
                    y_cleanout_cr.push(cleanout_cr as f64);
            }
        }
    }

    //make_notes!(&logfile_name, false, 0, "{}\n","Load Profile and Top Stats");
    //println!("{}","Load Profile and Top Stats");
    //I want to sort wait events by most heavy ones across the whole period
    let mut y_vals_events_sorted = BTreeMap::new();
    for (evname, ev) in y_vals_events {
        let mut wait_time = 0;
        for v in &ev {
            if *v > 0.0 {
                wait_time -= *v as i64;
            }
        }
        y_vals_events_sorted.insert((wait_time, evname.clone()), ev.clone());
    }
    //I want to sort wait events by most heavy ones across the whole period
    let mut y_vals_bgevents_sorted = BTreeMap::new();
    for (evname, ev) in y_vals_bgevents {
        let mut wait_time = 0;
        for v in &ev {
            if *v > 0.0 {
                wait_time -= *v as i64;
            }
        }
        y_vals_bgevents_sorted.insert((wait_time, evname.clone()), ev.clone());
    }

    //I want to sort SQL IDs by the number of times they showup in snapshots - for this purpose I'm using BTree with two index keys
    let mut y_vals_sqls_sorted = BTreeMap::new(); 
    for (sqlid, yv) in y_vals_sqls {
        let mut occuriance = 0;
        for v in &yv {
            if *v > 0.0 {
                occuriance -= 1;
            }
        }
        y_vals_sqls_sorted.insert((occuriance, sqlid.clone()), yv.clone());
        
    }
    //Get Global Stats for Lod Profile
    global_statistics.insert("CPU Load".to_string(), get_statistics(y_vals_cpu_load.clone()));
    global_statistics.insert("AAS".to_string(), get_statistics(y_vals_dbtime.clone()));
    global_statistics.insert("Executions/s".to_string(), get_statistics(y_vals_execs.clone()));
    global_statistics.insert("Transactions/s".to_string(), get_statistics(y_vals_trans.clone()));
    global_statistics.insert("Physical Reads MB/s".to_string(), get_statistics(y_read_mb.clone()));
    global_statistics.insert("Physical Writes MB/s".to_string(), get_statistics(y_write_mb.clone()));
    global_statistics.insert("Redo MB/s".to_string(), get_statistics(y_vals_redosize.clone()));
    global_statistics.insert("User Commits/snap".to_string(), get_statistics(y_user_commits.iter().map(|v| *v as f64).collect()));
    global_statistics.insert("User Rollbacks/snap".to_string(),get_statistics(y_user_rollbacks.iter().map(|v| *v as f64).collect()));
    global_statistics.insert("Parses/s".to_string(), get_statistics(y_vals_parses.clone()));
    global_statistics.insert("Hard Parses/s".to_string(), get_statistics(y_vals_hparses.clone()));
    global_statistics.insert("Logical Reads MB/s".to_string(), get_statistics(y_logical_reads_s.clone()));
    global_statistics.insert("Block Changes/s".to_string(), get_statistics(y_block_changes_s.clone()));
    global_statistics.insert("User Calls/s".to_string(), get_statistics(y_vals_calls.clone()));
    fs::write(format!("{}/stats/global_statistics.json",&html_dir),serde_json::to_string(&global_statistics).unwrap());
    
    // ------ Ploting and reporting starts ----------
    make_notes!(&logfile_name, args.quiet, 0, "\n\n");
    make_notes!(&logfile_name, false, 1, "{}\n","STATISTICAL COMPUTATION RESULTS".bold().green());

    let dbtime_trace = Scatter::new(x_vals.clone(), y_vals_dbtime.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("DB Time (s/s)")
                                                    .x_axis("x1")
                                                    .y_axis("y1");
    let dbcpu_trace = Scatter::new(x_vals.clone(), y_vals_dbcpu)
                                                    .mode(Mode::LinesText)
                                                    .name("DB CPU (s/s)")
                                                    .x_axis("x1")
                                                    .y_axis("y1");
    let calls_trace = Scatter::new(x_vals.clone(), y_vals_calls.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("User Calls/s")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let transactions_trace = Scatter::new(x_vals.clone(), y_vals_trans.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("Transactions/s")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let logons_trace = Scatter::new(x_vals.clone(), y_vals_logons)
                                                    .mode(Mode::LinesText)
                                                    .name("User Logons")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let logouts_trace = Scatter::new(x_vals.clone(), y_vals_logouts)
                                                    .mode(Mode::LinesText)
                                                    .name("User Logouts")
                                                    .x_axis("x1")
                                                    .y_axis("y2");                                            
    let commits_trace = Scatter::new(x_vals.clone(), y_user_commits.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("User Commits")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let rollbacks_trace = Scatter::new(x_vals.clone(), y_user_rollbacks.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("User Rollbacks")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let exec_trace = Scatter::new(x_vals.clone(), y_vals_execs.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("Executes/s")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let parses_trace = Scatter::new(x_vals.clone(), y_vals_parses.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("Parses/s")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let hparses_trace = Scatter::new(x_vals.clone(), y_vals_hparses.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("Hard Parses/s")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let blockchanges_trace = Scatter::new(x_vals.clone(), y_block_changes_s.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("Block changes/s")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let redosize_trace = Scatter::new(x_vals.clone(), y_vals_redosize.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("Redo MB/s")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let logicalread_trace = Scatter::new(x_vals.clone(), y_logical_reads_s.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("Logical Read MB/s")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let phyread_trace = Scatter::new(x_vals.clone(), y_read_mb.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("Physical Read MB/s")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let phywrite_trace = Scatter::new(x_vals.clone(), y_write_mb.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("Physical Write MB/s")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let cpu_user = Scatter::new(x_vals.clone(), y_vals_cpu_user)
                                                    .mode(Mode::LinesText)
                                                    .name("CPU User")
                                                    .x_axis("x1")
                                                    .y_axis("y4");
    let cpu_load = Scatter::new(x_vals.clone(), y_vals_cpu_load.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("CPU Load")
                                                    .x_axis("x1")
                                                    .y_axis("y4");
    let cpu_count = Scatter::new(x_vals.clone(), y_vals_cpu_count.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("CPU Count")
                                                    .x_axis("x1")
                                                    .y_axis("y4");
    let cpu_load_box_plot  = BoxPlot::new(y_vals_cpu_load)
                                                    //.mode(Mode::LinesText)
                                                    .name("CPU Load %")
                                                    .x_axis("x1")
                                                    .y_axis("y1")
                                                    .box_mean(BoxMean::True)
                                                    .show_legend(false)
                                                    .box_points(BoxPoints::All)
                                                    .whisker_width(0.2)
                                                    .marker(Marker::new().color("#9c2d2d".to_string()).opacity(0.7).size(2));
    let aas_box_plot  = BoxPlot::new(y_vals_dbtime.clone())
                                                    .name("AAS")
                                                    .x_axis("x2")
                                                    .y_axis("y2")
                                                    .box_mean(BoxMean::True)
                                                    .show_legend(false)
                                                    .box_points(BoxPoints::All)
                                                    .whisker_width(0.2)
                                                    .marker(Marker::new().color("#2d9c57".to_string()).opacity(0.7).size(2));
    let exec_box_plot  = BoxPlot::new(y_vals_execs)
                                                    .name("Exec/s")
                                                    .x_axis("x3")
                                                    .y_axis("y3")
                                                    .box_mean(BoxMean::True)
                                                    .show_legend(false)
                                                    .box_points(BoxPoints::All)
                                                    .whisker_width(0.2)
                                                    .marker(Marker::new().color("#2d5d9c".to_string()).opacity(0.7).size(2));
    let trans_box_plot  = BoxPlot::new(y_vals_trans)
                                                    .name("Trans/s")
                                                    .x_axis("x4")
                                                    .y_axis("y4")
                                                    .box_mean(BoxMean::True)
                                                    .show_legend(false)
                                                    .box_points(BoxPoints::All)
                                                    .whisker_width(0.2)
                                                    .marker(Marker::new().color("#904fc2".to_string()).opacity(0.7).size(2));
    let read_mb_box_plot  = BoxPlot::new(y_read_mb)
                                                    //.mode(Mode::LinesText)
                                                    .name("Phy Reads MB/s")
                                                    .x_axis("x5")
                                                    .y_axis("y5")
                                                    .box_mean(BoxMean::True)
                                                    .show_legend(false)
                                                    .box_points(BoxPoints::All)
                                                    .whisker_width(0.2)
                                                    .marker(Marker::new().color("#c2be4f".to_string()).opacity(0.7).size(2));
    let write_mb_box_plot  = BoxPlot::new(y_write_mb)
                                                    //.mode(Mode::LinesText)
                                                    .name("Phy Writes MB/s")
                                                    .x_axis("x6")
                                                    .y_axis("y6")
                                                    .box_mean(BoxMean::True)
                                                    .show_legend(false)
                                                    .box_points(BoxPoints::All)
                                                    .whisker_width(0.2)
                                                    .marker(Marker::new().color("#c2904f".to_string()).opacity(0.7).size(2));
    let redo_mb_box_plot  = BoxPlot::new(y_vals_redosize)
                                                //.mode(Mode::LinesText)
                                                .name("Redo MB/s")
                                                .x_axis("x7")
                                                .y_axis("y7")
                                                .box_mean(BoxMean::True)
                                                .show_legend(false)
                                                .box_points(BoxPoints::All)
                                                .whisker_width(0.2)
                                                .marker(Marker::new().color("#ad247a".to_string()).opacity(0.7).size(2));
    let user_commits_box_plot  = BoxPlot::new(y_user_commits)
                                                //.mode(Mode::LinesText)
                                                .name("User Commits/snap")
                                                .x_axis("x1")
                                                .y_axis("y1")
                                                .box_mean(BoxMean::True)
                                                .show_legend(false)
                                                .box_points(BoxPoints::All)
                                                .whisker_width(0.2)
                                                .marker(Marker::new().color("#fa3434".to_string()).opacity(0.7).size(2));
    let user_rollbacks_box_plot  = BoxPlot::new(y_user_rollbacks)
                                            //.mode(Mode::LinesText)
                                                .name("User Rollbacks/snap")
                                                .x_axis("x2")
                                                .y_axis("y2")
                                                .box_mean(BoxMean::True)
                                                .show_legend(false)
                                                .box_points(BoxPoints::All)
                                                .whisker_width(0.2)
                                                .marker(Marker::new().color("#11fa7a".to_string()).opacity(0.7).size(2));
    let user_parses_box_plot  = BoxPlot::new(y_vals_parses)
                                            //.mode(Mode::LinesText)
                                                .name("Parses/s")
                                                .x_axis("x3")
                                                .y_axis("y3")
                                                .box_mean(BoxMean::True)
                                                .show_legend(false)
                                                .box_points(BoxPoints::All)
                                                .whisker_width(0.2)
                                                .marker(Marker::new().color("#006aff".to_string()).opacity(0.7).size(2));
    let user_hparses_box_plot  = BoxPlot::new(y_vals_hparses)
                                            //.mode(Mode::LinesText)
                                                .name("Hard Parses/s")
                                                .x_axis("x4")
                                                .y_axis("y4")
                                                .box_mean(BoxMean::True)
                                                .show_legend(false)
                                                .box_points(BoxPoints::All)
                                                .whisker_width(0.2)
                                                .marker(Marker::new().color("#9000ff".to_string()).opacity(0.7).size(2));
    let logical_reads_box_plot  = BoxPlot::new(y_logical_reads_s)
                                            //.mode(Mode::LinesText)
                                                .name("Logical Reads (MB)/s")
                                                .x_axis("x5")
                                                .y_axis("y5")
                                                .box_mean(BoxMean::True)
                                                .show_legend(false)
                                                .box_points(BoxPoints::All)
                                                .whisker_width(0.2)
                                                .marker(Marker::new().color("#f0dc02".to_string()).opacity(0.7).size(2));
    let block_changes_box_plot  = BoxPlot::new(y_block_changes_s)
                                            //.mode(Mode::LinesText)
                                                .name("Block Changes/s")
                                                .x_axis("x6")
                                                .y_axis("y6")
                                                .box_mean(BoxMean::True)
                                                .show_legend(false)
                                                .box_points(BoxPoints::All)
                                                .whisker_width(0.2)
                                                .marker(Marker::new().color("#ff8f1f".to_string()).opacity(0.7).size(2));
    let user_calls_box_plot     = BoxPlot::new(y_vals_calls)
                                            //.mode(Mode::LinesText)
                                                .name("User Calls/s")
                                                .x_axis("x7")
                                                .y_axis("y7")
                                                .box_mean(BoxMean::True)
                                                .show_legend(false)
                                                .box_points(BoxPoints::All)
                                                .whisker_width(0.2)
                                                .marker(Marker::new().color("#FF00CC".to_string()).opacity(0.7).size(2));

    if is_logfilesync_high{
        let redo_switches = Scatter::new(x_vals.clone(), y_vals_redo_switches)
                                                        .mode(Mode::LinesText)
                                                        .name("Log Switches")
                                                        .x_axis("x1")
                                                        .y_axis("y2");
        let excessive_commits = Scatter::new(x_vals.clone(), y_excessive_commits)
                                                        .mode(Mode::LinesText)
                                                        .name("Excessive Commits")
                                                        .x_axis("x1")
                                                        .y_axis("y2");
        let cleanout_ktugct_calls = Scatter::new(x_vals.clone(), y_cleanout_ktugct)
                                                        .mode(Mode::LinesText)
                                                        .name("cleanout - number of ktugct calls")
                                                        .x_axis("x1")
                                                        .y_axis("y2");
        let cleanout_cr_only = Scatter::new(x_vals.clone(), y_cleanout_cr)
                                                        .mode(Mode::LinesText)
                                                        .name("cleanouts only - consistent read gets")
                                                        .x_axis("x1")
                                                        .y_axis("y2");
        let parse_failure_count = Scatter::new(x_vals.clone(), y_failed_parse_count)
                                                        .mode(Mode::LinesText)
                                                        .name("Failed Parses")
                                                        .x_axis("x1")
                                                        .y_axis("y2");
        
        plot_main.add_trace(redo_switches);
        plot_main.add_trace(excessive_commits);
        plot_main.add_trace(cleanout_cr_only);
        plot_main.add_trace(cleanout_ktugct_calls);
        plot_main.add_trace(parse_failure_count);
    }
    
    plot_main.add_trace(dbtime_trace);
    plot_highlight.add_trace(aas_box_plot);
    plot_main.add_trace(dbcpu_trace);
    plot_main.add_trace(calls_trace);
    plot_main.add_trace(transactions_trace);
    plot_main.add_trace(logons_trace);
    plot_main.add_trace(logouts_trace);
    plot_main.add_trace(commits_trace);
    plot_main.add_trace(rollbacks_trace);
    plot_main.add_trace(exec_trace);
    plot_highlight.add_trace(exec_box_plot);
    plot_highlight.add_trace(trans_box_plot);
    plot_main.add_trace(blockchanges_trace);
    plot_main.add_trace(redosize_trace);
    plot_main.add_trace(logicalread_trace);
    plot_main.add_trace(phyread_trace);
    plot_main.add_trace(phywrite_trace);
    plot_main.add_trace(parses_trace);
    plot_main.add_trace(hparses_trace);
    plot_main.add_trace(cpu_user);
    plot_main.add_trace(cpu_load);
    
    
    let first_cpu = y_vals_cpu_count.first(); //Get first value of CPU Count
    if first_cpu.is_some() { // if you found something
        let cpu_is_changing = y_vals_cpu_count.iter()
                                                        .find(|cpu| {
                                                                *cpu != first_cpu.unwrap()
                                                        }); //scan the whole vector to find the first value different - it means that number of cpus changes over time
        if cpu_is_changing.is_some() {
             plot_main.add_trace(cpu_count); //if so - add the plot to the cpu scatter
        }                                              
    }
    
    plot_highlight.add_trace(cpu_load_box_plot);
    plot_highlight.add_trace(read_mb_box_plot);
    plot_highlight.add_trace(write_mb_box_plot);
    plot_highlight.add_trace(redo_mb_box_plot);
    plot_highlight2.add_trace(user_commits_box_plot);
    plot_highlight2.add_trace(user_rollbacks_box_plot);
    plot_highlight2.add_trace(user_parses_box_plot);
    plot_highlight2.add_trace(user_hparses_box_plot);
    plot_highlight2.add_trace(logical_reads_box_plot);
    plot_highlight2.add_trace(block_changes_box_plot);
    plot_highlight2.add_trace(user_calls_box_plot);


    // WAIT EVENTS Correlation and AVG/STDDEV calculation, print and feed table used for HTML
    let mut anomalies_html = String::new();
    let mut table_events: String = String::new();
    let mut table_anomalies: String = String::new();
    let mut table_bgevents: String = String::new();
    let mut table_sqls: String = String::new();
    let mut table_stat_corr: String = String::new();

    //This will hold anomalies summary join table indexed by (begin_snap_id, begin_snap_time) with anomalies value
    // like (42,12-Mar-2025 13:00:00) WAIT:db file sequential read (MAD,AVG,etc...)
    let mut anomalies_summary: BTreeMap<(u64, String), BTreeMap<String, Vec<String>>> = BTreeMap::new();

    //println!("{}","Foreground Wait Events");
    make_notes!(&logfile_name, false, 2, "\n{}\n","Foreground Wait Events".yellow());
    let mut top_fg_events: Vec<TopForegroundWaitEvents> = Vec::new();
    
    for (key, yv) in &y_vals_events_sorted {
        let mut event_data = TopForegroundWaitEvents::default();

        let event_trace = Scatter::new(x_vals.clone(), yv.clone())
                                                        .mode(Mode::LinesText)
                                                        .name(key.1.clone())
                                                        .x_axis("x1")
                                                        .y_axis("y3").visible(Visible::LegendOnly);
        plot_main.add_trace(event_trace);
        let event_name: String = key.1.clone();
        /* Correlation calc */
        let corr: f64 = pearson_correlation_2v(&y_vals_dbtime, &yv);
        // Print Correlation considered high enough to mark it
        make_notes!(&logfile_name, args.quiet, 3, "\t{: >5}\n", &event_name.bold());

        let correlation_info: String = format!("--- Correlation with DB Time: {:.2}", &corr);
        if corr >= 0.4 || corr <= -0.4 { 
            make_notes!(&logfile_name, args.quiet, 0, "{: >50}", correlation_info.red().bold());
        } else {
            make_notes!(&logfile_name, args.quiet, 0, "{: >50}", correlation_info);
        }

        /* STDDEV/AVG Calculations */
        let x_n: Vec<f64> = y_vals_events_n.get(&event_name).unwrap().clone();
        let avg_exec_n: f64 = mean(x_n.clone()).unwrap();
        let stddev_exec_n: f64 = std_deviation(x_n.clone()).unwrap();

        let x_t: Vec<f64> = y_vals_events_t.get(&event_name).unwrap().clone();
        let avg_exec_t: f64 = mean(x_t.clone()).unwrap();
        let stddev_exec_t: f64 = std_deviation(x_t).unwrap();

        let x_s: Vec<f64> = y_vals_events_s.get(&event_name).unwrap().clone();
        let avg_exec_s: f64 = mean(x_s.clone()).unwrap();
        let stddev_exec_s: f64 = std_deviation(x_s).unwrap();

        let avg_wait_per_exec_ms: f64 = (avg_exec_s / avg_exec_n) * 1000.0;
        let stddev_wait_per_exec_ms: f64 = (stddev_exec_s / stddev_exec_n) * 1000.0;
        // Print calculations:
        
        make_notes!(&logfile_name, args.quiet, 0, "\t\tMarked as TOP in {:.2}% of probes\n",  (x_n.len() as f64 / x_vals.len() as f64 )* 100.0);
        make_notes!(&logfile_name, args.quiet, 0, "\t\t--- AVG PCT of DB Time: {:>15.2}% \tSTDDEV PCT of DB Time: {:>15.2}%\n", &avg_exec_t, &stddev_exec_t);
        make_notes!(&logfile_name, args.quiet, 0, "\t\t--- AVG Wait Time (s): {:>16.2} \tSTDDEV Wait Time (s): {:>16.2}\n", &avg_exec_s, &stddev_exec_s);
        make_notes!(&logfile_name, args.quiet, 0, "\t\t--- AVG No. executions: {:>15.2} \tSTDDEV No. executions: {:>15.2}\n", &avg_exec_n, &stddev_exec_n);
        make_notes!(&logfile_name, args.quiet, 0, "\t\t--- AVG wait/exec (ms): {:>15.2} \tSTDDEV wait/exec (ms): {:>15.2}\n\n", &avg_wait_per_exec_ms, &stddev_wait_per_exec_ms);
        
        event_data.event_name = event_name.clone();
        event_data.correlation_with_db_time = corr;
        event_data.marked_as_top_in_pct_of_probes = (x_n.len() as f64 / x_vals.len() as f64 )* 100.0;
        event_data.avg_pct_of_dbtime = avg_exec_t; 
        event_data.stddev_pct_of_db_time = stddev_exec_t;
        event_data.avg_wait_time_s = avg_exec_s;
        event_data.stddev_wait_time_s = stddev_exec_s;
        event_data.avg_number_of_executions = avg_exec_n;
        event_data.stddev_number_of_executions = stddev_exec_n;
        event_data.avg_wait_for_execution_ms = avg_wait_per_exec_ms;
        event_data.stddev_wait_for_execution_ms = stddev_wait_per_exec_ms;

        /* Print table of detected anomalies for given event_name (key.1)*/
        let safe_event_name: String = event_name.replace("/", "_").replace(" ", "_").replace(":","").replace("*","_");
        let anomaly_id = format!("mad_fg_{}", &safe_event_name);
        let mut anomalies_flag: bool = false;

        if let Some(anomalies) = top_stats.event_anomalies_mad.get(&key.1) {
            let mut mad_events: MadAnomaliesEvents = MadAnomaliesEvents::default();
            let anomalies_detection_msg = "Detected anomalies using Median Absolute Deviation on the following dates:".to_string().red();
            anomalies_flag = true;
            make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n",  anomalies_detection_msg);
            
            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                    Cell::new("Date"),
                    Cell::new("MAD Score"),
                    Cell::new("Total Wait (s)"),
                    Cell::new("Waits"),
                    Cell::new("AVG Wait (ms)"),
                    Cell::new("DBTime (%)")
                ]));
            
            for (i,a) in anomalies.iter().enumerate() {
                let c_event = Cell::new(&a.0);
                
                let c_mad_score: Cell = Cell::new(&format!("{:.3}", a.1));
                
                let wait_event = collection.awrs
                                                    .iter()
                                                    .filter(|awr| awr.snap_info.begin_snap_time == a.0)
                                                    .flat_map(|awr| awr.foreground_wait_events.iter())
                                                    .find(|w| w.event == key.1).unwrap();
                
                let c_total_wait_s = Cell::new(&format!("{:.3}", wait_event.total_wait_time_s));
                let c_waits = Cell::new(&format!("{:.3}", wait_event.waits));
                let avg_wait_ms = wait_event.total_wait_time_s/(wait_event.waits as f64)*1000.0;
                let c_avg_wait_ms = Cell::new(&format!("{:.3}", avg_wait_ms));
                let c_dbtime_pct = Cell::new(&format!("{:.2}", wait_event.pct_dbtime));

                mad_events.anomaly_date = a.0.clone();
                mad_events.mad_score = a.1;
                mad_events.total_wait_s = wait_event.total_wait_time_s;
                mad_events.number_of_waits = wait_event.waits;
                mad_events.avg_wait_time_for_execution_ms = avg_wait_ms;
                mad_events.pct_of_db_time = wait_event.pct_dbtime;
                event_data.median_absolute_deviation_anomalies.push(mad_events.clone());

                table.add_row(Row::new(vec![c_event.clone(),c_mad_score.clone(),c_total_wait_s.clone(),c_waits.clone(), c_avg_wait_ms.clone(),c_dbtime_pct.clone()]));
                table_anomalies.push_str(&format!(
                    r#"<tr>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                    </tr>"#,
                    c_event.to_string(),c_mad_score.to_string(),c_total_wait_s.to_string(),c_waits.to_string(), c_avg_wait_ms.to_string(), c_dbtime_pct.to_string()
                ));
    
                let begin_snap_id = collection.awrs
                                                    .iter()
                                                    .find_map(|awr| {
                                                        (awr.snap_info.begin_snap_time == a.0)
                                                            .then(|| awr.snap_info.begin_snap_id)
                                                    }).unwrap();

                //anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()), format!("EVENT:    {}", key.1));
                anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()),"EVENT", &key.1);
            }
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }
        } else {
            let no_anomalies_txt = format!("\t\tNo anomalies detected based on MAD threshold: {}\n", args.mad_threshold);
            anomalies_flag = false;
            make_notes!(&logfile_name, args.quiet, 0, "{}", no_anomalies_txt.green().italic());
        }
        make_notes!(&logfile_name, args.quiet, 0,"\n");

        top_fg_events.push(event_data);
        
        /* FGEVENTS - Generate a row for the Main HTML table */
        table_events.push_str(&format!(
            r#"
            <tr>
                <td><a href="fg/fg_{}.html" target="_blank" class="nav-link" style="font-weight: bold">{}</a></td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{}</td>
            </tr>
            "#,
            safe_event_name,
            event_name,
            avg_exec_t, stddev_exec_t,  // PCT of DB Time
            avg_exec_s, stddev_exec_s,  // Wait Time (s)
            avg_exec_n, stddev_exec_n,  // Execution times
            avg_wait_per_exec_ms, stddev_wait_per_exec_ms,  // Wait per exec (ms)
            corr,
            (x_n.len() as f64 / x_vals.len() as f64 )* 100.0,
            if anomalies_flag {
                format!(r#"<a href="javascript:void(0);" onclick="toggleRow('{}')" class="nav-link" style="font-weight: bold">Yes</a>"#, anomaly_id)
            } else {
                "No".to_string()
            }
        ));
        // Include: the collapsible anomaly table row
        if anomalies_flag {
            table_events.push_str(&format!(
                r#"<tr id="{0}" class="anomaly-row" style="display: none;">
                    <td colspan="12">
                        <table class="inner-anomalies-table">
                            <thead>
                                <tr>
                                    <th onclick="sortInnerTable('{0}',0)" style="cursor: pointer;">Date</th>
                                    <th onclick="sortInnerTable('{0}',1)" style="cursor: pointer;">MAD Score</th>
                                    <th onclick="sortInnerTable('{0}',2)" style="cursor: pointer;">Total Wait (s)</th>
                                    <th onclick="sortInnerTable('{0}',3)" style="cursor: pointer;">Waits</th>
                                    <th onclick="sortInnerTable('{0}',4)" style="cursor: pointer;">AVG Wait (ms)</th>
                                    <th onclick="sortInnerTable('{0}',5)" style="cursor: pointer;">DBTime (%)</th>
                                </tr>
                            </thead>
                            <tbody>
                                {1}
                            </tbody>
                        </table>
                    </td>
                </tr>"#,
                anomaly_id,
                table_anomalies
        ))};
        table_anomalies = "".to_string();
        anomalies_flag = false;
    }
      /* FGEVENTS Anomalies Sub Tables  */
    let event_table_html: String = format!(
        r#"
        <table id="events-table">
            <thead>
                <tr>
                    <th onclick="sortTable('events-table',0)" style="cursor: pointer;">Event Name</th>
                    <th onclick="sortTable('events-table',1)" style="cursor: pointer;">AVG % of DBTime</th>
                    <th onclick="sortTable('events-table',2)" style="cursor: pointer;">STDDEV % of DBTime</th>
                    <th onclick="sortTable('events-table',3)" style="cursor: pointer;">AVG Wait Time (s)</th>
                    <th onclick="sortTable('events-table',4)" style="cursor: pointer;">STDDEV Wait Time (s)</th>
                    <th onclick="sortTable('events-table',5)" style="cursor: pointer;">AVG No. Executions</th>
                    <th onclick="sortTable('events-table',6)" style="cursor: pointer;">STDDEV No. Executions</th>
                    <th onclick="sortTable('events-table',7)" style="cursor: pointer;">AVG Wait per Exec (ms)</th>
                    <th onclick="sortTable('events-table',8)" style="cursor: pointer;">STDDEV Wait per Exec (ms)</th>
                    <th onclick="sortTable('events-table',9)" style="cursor: pointer;">Correlation of DBTime</th>
                    <th onclick="sortTable('events-table',10)" style="cursor: pointer;">TOP in % Probes</th>
                    <th onclick="sortTable('events-table',11)" style="cursor: pointer;">Anomalies</th>
                </tr>
            </thead>
            <tbody>
            {}
            </tbody>
        </table>
        "#,
        table_events
    );

    //println!("{}","Background Wait Events");
    make_notes!(&logfile_name, false, 2, "{}\n","Background Wait Events".yellow());
    let mut top_bg_events: Vec<TopBackgroundWaitEvents> = Vec::new();

    for (key, yv) in &y_vals_bgevents_sorted {
        let mut event_data = TopBackgroundWaitEvents::default();

        let event_name: String = key.1.clone();
        /* Correlation calc */
        let corr: f64 = pearson_correlation_2v(&y_vals_dbtime, &yv);

        /* STDDEV/AVG Calculations */
        let x_n: Vec<f64> = y_vals_bgevents_n.get(&event_name).unwrap().clone();
        let avg_exec_n: f64 = mean(x_n.clone()).unwrap();
        let stddev_exec_n: f64 = std_deviation(x_n.clone()).unwrap();

        let x_t: Vec<f64> = y_vals_bgevents_t.get(&event_name).unwrap().clone();
        let avg_exec_t: f64 = mean(x_t.clone()).unwrap();
        let stddev_exec_t: f64 = std_deviation(x_t).unwrap();

        let x_s: Vec<f64> = y_vals_bgevents_s.get(&event_name).unwrap().clone();
        let avg_exec_s: f64 = mean(x_s.clone()).unwrap();
        let stddev_exec_s: f64 = std_deviation(x_s).unwrap();

        let avg_wait_per_exec_ms: f64 = (avg_exec_s / avg_exec_n) * 1000.0;
        let stddev_wait_per_exec_ms: f64 = (stddev_exec_s / stddev_exec_n) * 1000.0;

        //Print calculations
        make_notes!(&logfile_name, args.quiet, 3, "\t{: >5}\n", &event_name.bold());

        let correlation_info: String = format!("--- Correlation with DB Time: {:.2}", &corr);
        if corr >= 0.4 || corr <= -0.4 { 
            make_notes!(&logfile_name, args.quiet, 0, "{: >50}", correlation_info.red().bold());
        } else {
            make_notes!(&logfile_name, args.quiet, 0, "{: >50}", correlation_info);
        }
        make_notes!(&logfile_name, args.quiet, 0, "\t\tMarked as TOP in {:.2}% of probes\n",  (x_n.len() as f64 / x_vals.len() as f64 )* 100.0);
        make_notes!(&logfile_name, args.quiet, 0, "\t\t--- AVG PCT of DB Time: {:>15.2}% \tSTDDEV PCT of DB Time: {:>15.2}%\n", &avg_exec_t, &stddev_exec_t);
        make_notes!(&logfile_name, args.quiet, 0, "\t\t--- AVG Wait Time (s): {:>16.2} \tSTDDEV Wait Time (s): {:>16.2}\n", &avg_exec_s, &stddev_exec_s);
        make_notes!(&logfile_name, args.quiet, 0, "\t\t--- AVG No. executions: {:>15.2} \tSTDDEV No. executions: {:>15.2}\n", &avg_exec_n, &stddev_exec_n);
        make_notes!(&logfile_name, args.quiet, 0, "\t\t--- AVG wait/exec (ms): {:>15.2} \tSTDDEV wait/exec (ms): {:>15.2}\n\n", &avg_wait_per_exec_ms, &stddev_wait_per_exec_ms);
        
        event_data.event_name = event_name.clone();
        event_data.correlation_with_db_time = corr;
        event_data.marked_as_top_in_pct_of_probes = (x_n.len() as f64 / x_vals.len() as f64 )* 100.0;
        event_data.avg_pct_of_dbtime = avg_exec_t;
        event_data.stddev_pct_of_db_time = stddev_exec_t;
        event_data.avg_wait_time_s = avg_exec_s;
        event_data.stddev_wait_time_s = stddev_exec_s;
        event_data.avg_number_of_executions = avg_exec_n;
        event_data.stddev_number_of_executions = stddev_exec_n;
        event_data.avg_wait_for_execution_ms = avg_wait_per_exec_ms;
        event_data.stddev_wait_for_execution_ms = stddev_wait_per_exec_ms;

        
         /* Print table of detected anomalies for given event_name (key.1)*/
        let safe_event_name: String = event_name.replace("/", "_").replace(" ", "_").replace(":","").replace("*","_");
        let anomaly_id = format!("mad_bg_{}", &safe_event_name);
        let mut anomalies_flag: bool = false;

        if let Some(anomalies) = top_stats.bgevent_anomalies_mad.get(&key.1) {
            let mut mad_events: MadAnomaliesEvents = MadAnomaliesEvents::default();

            let anomalies_detection_msg = "Detected anomalies using Median Absolute Deviation on the following dates:".to_string().red();
            anomalies_flag = true;
            make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n",  anomalies_detection_msg);

            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("Date"),
                Cell::new("MAD Score"),
                Cell::new("Total Wait (s)"),
                Cell::new("Waits"),
                Cell::new("AVG Wait (ms)"),
                Cell::new("DBTime (%)")
            ]));

            for (i,a) in anomalies.iter().enumerate() {
                let c_event = Cell::new(&a.0);
                
                let c_mad_score: Cell = Cell::new(&format!("{:.3}", a.1));
                
                let wait_event = collection.awrs
                                                    .iter()
                                                    .filter(|awr| awr.snap_info.begin_snap_time == a.0)
                                                    .flat_map(|awr| awr.background_wait_events.iter())
                                                    .find(|w| w.event == key.1).unwrap();
                
                let c_total_wait_s = Cell::new(&format!("{:.3}", wait_event.total_wait_time_s));
                let c_waits = Cell::new(&format!("{:.3}", wait_event.waits));
                let c_avg_wait_ms = Cell::new(&format!("{:.3}", wait_event.total_wait_time_s/(wait_event.waits as f64)*1000.0));
                let c_dbtime_pct = Cell::new(&format!("{:.2}", wait_event.pct_dbtime));

                mad_events.anomaly_date = a.0.clone();
                mad_events.mad_score = a.1;
                mad_events.total_wait_s = wait_event.total_wait_time_s;
                mad_events.number_of_waits = wait_event.waits;
                mad_events.avg_wait_time_for_execution_ms = wait_event.total_wait_time_s/(wait_event.waits as f64)*1000.0;
                mad_events.pct_of_db_time = wait_event.pct_dbtime;
                event_data.median_absolute_deviation_anomalies.push(mad_events.clone());
                
                
                table.add_row(Row::new(vec![c_event.clone(),c_mad_score.clone(),c_total_wait_s.clone(),c_waits.clone(), c_avg_wait_ms.clone(), c_dbtime_pct.clone()]));
                table_anomalies.push_str(&format!(
                    r#"<tr>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                    </tr>"#,
                    c_event.to_string(),c_mad_score.to_string(),c_total_wait_s.to_string(),c_waits.to_string(), c_avg_wait_ms.to_string(), c_dbtime_pct.to_string()
                ));

                let begin_snap_id = collection.awrs
                                                    .iter()
                                                    .find_map(|awr| {
                                                        (awr.snap_info.begin_snap_time == a.0)
                                                            .then(|| awr.snap_info.begin_snap_id)
                                                    }).unwrap();

                //anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()), format!("BGEVENT:  {}", key.1));
                anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()),"BGEVENT", &key.1);

            }
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }
        } else {
            let no_anomalies_txt = format!("\t\tNo anomalies detected based on MAD threshold: {}\n", args.mad_threshold);
            anomalies_flag = false;
            make_notes!(&logfile_name, args.quiet, 0, "{}", no_anomalies_txt.green().italic());
        }
        make_notes!(&logfile_name, args.quiet, 0,"\n");

        top_bg_events.push(event_data);
        
        /* BGEVENTS - Generate a row for the HTML table */
        table_bgevents.push_str(&format!(
            r#"
            <tr>
                <td><a href="bg/bg_{}.html" target="_blank" class="nav-link" style="font-weight: bold">{}</a></td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{}</td>
            </tr>
            "#,
            safe_event_name,
            event_name,
            avg_exec_t, stddev_exec_t,  // PCT of DB Time
            avg_exec_s, stddev_exec_s,  // Wait Time (s)
            avg_exec_n, stddev_exec_n,  // Execution times
            avg_wait_per_exec_ms, stddev_wait_per_exec_ms,  // Wait per exec (ms)
            corr,
            (x_n.len() as f64 / x_vals.len() as f64 )* 100.0,
            if anomalies_flag {
                format!(r#"<a href="javascript:void(0);" onclick="toggleRow('{}')" class="nav-link" style="font-weight: bold">Yes</a>"#, anomaly_id)
            } else {
                "No".to_string()
            }
        ));
        // Include: the collapsible anomaly table row
        if anomalies_flag {
            table_bgevents.push_str(&format!(
                r#"<tr id="{0}" class="anomaly-row" style="display: none;">
                    <td colspan="12">
                        <table class="inner-anomalies-table">
                            <thead>
                                <tr>
                                    <th onclick="sortInnerTable('{0}',0)" style="cursor: pointer;">Date</th>
                                    <th onclick="sortInnerTable('{0}',1)" style="cursor: pointer;">MAD Score</th>
                                    <th onclick="sortInnerTable('{0}',2)" style="cursor: pointer;">Total Wait (s)</th>
                                    <th onclick="sortInnerTable('{0}',3)" style="cursor: pointer;">Waits</th>
                                    <th onclick="sortInnerTable('{0}',4)" style="cursor: pointer;">AVG Wait (ms)</th>
                                    <th onclick="sortInnerTable('{0}',5)" style="cursor: pointer;">DBTime (%)</th>
                                </tr>
                            </thead>
                            <tbody>
                                {1}
                            </tbody>
                        </table>
                    </td>
                </tr>"#,
                anomaly_id,
                table_anomalies
        ))};
        table_anomalies = "".to_string();
        anomalies_flag = false;
    }
    let bgevent_table_html: String = format!(
        r#"
        <table id="bgevents-table">
            <thead>
                <tr>
                    <th onclick="sortTable('bgevents-table',0)" style="cursor: pointer;">Event Name</th>
                    <th onclick="sortTable('bgevents-table',1)" style="cursor: pointer;">AVG % of DBTime</th>
                    <th onclick="sortTable('bgevents-table',2)" style="cursor: pointer;">STDDEV % of DBTime</th>
                    <th onclick="sortTable('bgevents-table',3)" style="cursor: pointer;">AVG Wait Time (s)</th>
                    <th onclick="sortTable('bgevents-table',4)" style="cursor: pointer;">STDDEV Wait Time (s)</th>
                    <th onclick="sortTable('bgevents-table',5)" style="cursor: pointer;">AVG Exec Times</th>
                    <th onclick="sortTable('bgevents-table',6)" style="cursor: pointer;">STDDEV Exec Times</th>
                    <th onclick="sortTable('bgevents-table',7)" style="cursor: pointer;">AVG Wait per Exec (ms)</th>
                    <th onclick="sortTable('bgevents-table',8)" style="cursor: pointer;">STDDEV Wait per Exec (ms)</th>
                    <th onclick="sortTable('bgevents-table',9)" style="cursor: pointer;">Correlation of DBTime</th>
                    <th onclick="sortTable('bgevents-table',10)" style="cursor: pointer;">TOP in % Probes</th>
                    <th onclick="sortTable('bgevents-table',11)" style="cursor: pointer;">Anomalies</th>
                </tr>
            </thead>
            <tbody>
            {}
            </tbody>
        </table>
        "#,
        table_bgevents
    );

    report_for_ai.top_foreground_wait_events = top_fg_events.clone();
    report_for_ai.top_background_wait_events = top_bg_events.clone();

    //println!("{}","SQLs");
    make_notes!(&logfile_name, false, 2, "{}", "TOP SQLs by Elapsed time".yellow());

    let mut ash_event_sql_map: HashMap<String, HashSet<String>> = HashMap::new();
    let mut crr_event_sql_map: HashMap<String, HashMap<String, f64>> = HashMap::new();

    let mut top_sqls: Vec<TopSQLsByElapsedTime> = Vec::new();
    for (key,yv) in y_vals_sqls_sorted {
        let mut sql_data = TopSQLsByElapsedTime::default();

        let sql_trace = Scatter::new(x_vals.clone(), yv.clone())
                                                        .mode(Mode::LinesText)
                                                        .name(key.1.clone())
                                                        .x_axis("x1")
                                                        .y_axis("y5").visible(Visible::LegendOnly);
        plot_main.add_trace(sql_trace);
        
        let sql_id: String = key.1.clone();
        let sql_id_disp = format!("SQL_ID: {}", key.1.clone());
        /* Correlation calc */
        let corr: f64 = pearson_correlation_2v(&y_vals_dbtime, &yv);
        // Print Correlation considered high enough to mark it
        let top_sections: HashMap<String, f64> = report_top_sql_sections(&sql_id, &collection.awrs);
        make_notes!(&logfile_name, args.quiet, 3, "\n\t{: >5}", &sql_id_disp.bold());
        make_notes!(&logfile_name, args.quiet, 0, "\n\t {}\n",
            format!("Other Top Sections: {}",
                &top_sections.iter().map(|(key, value)| format!("{} [{:.2}%]", key, value)).collect::<Vec<String>>().join(" | ")
            ).italic(),
        );

        let correlation_info: String = format!("--- Correlation with DB Time: {:.2}", &corr);
        if corr >= 0.4 || corr <= -0.4 { 
            make_notes!(&logfile_name, args.quiet, 0, "{: >49}", correlation_info.red().bold());
        } else {
            make_notes!(&logfile_name, args.quiet, 0, "{: >49}", correlation_info);
        }

        /* Calculate STDDEV and AVG for sqls executions number */
        let x: Vec<f64> = y_vals_sqls_exec_n.get(&key.1.clone()).unwrap().clone();
        let avg_exec_n: f64 = mean(x.clone()).unwrap_or(0.0);
        let stddev_exec_n: f64 = std_deviation(x).unwrap_or(0.0);

        /* Calculate STDDEV and AVG for sqls time per execution */
        let x: Vec<f64> = y_vals_sqls_exec_t.get(&key.1.clone()).unwrap().clone();
        let avg_exec_t: f64 = mean(x.clone()).unwrap_or(0.0);
        let stddev_exec_t: f64 = std_deviation(x.clone()).unwrap_or(0.0);

        /* Calculate STDDEV and AVG for sqls CPU time per execution */
        let x_c: Vec<f64> = collection.awrs.iter()
                                      .flat_map(|s| s.sql_cpu_time.clone())
                                      .filter(|sql| sql.0 == key.1.clone())
                                      .map(|sqls| sqls.1.cpu_time_exec_s)
                                      .collect();

        let avg_exec_t_cpu: f64 = mean(x_c.clone()).unwrap_or(0.0);
        let stddev_exec_t_cpu: f64 = std_deviation(x_c.clone()).unwrap_or(0.0);

        /* Calculate STDDEV and AVG for sqls time */
        let x_s: Vec<f64> = y_vals_sqls_exec_s.get(&key.1.clone()).unwrap().clone();
        let avg_exec_s: f64 = mean(x_s.clone()).unwrap_or(0.0);
        let stddev_exec_s: f64 = std_deviation(x_s).unwrap_or(0.0);

        /* Calculate STDDEV and AVG for sqls cpu time */
        let x_s: Vec<f64> = collection.awrs.iter()
                                      .flat_map(|s| s.sql_cpu_time.clone())
                                      .filter(|sql| sql.0 == key.1.clone())
                                      .map(|sqls| sqls.1.cpu_time_s)
                                      .collect();

        let avg_exec_cpu: f64 = mean(x_s.clone()).unwrap_or(0.0);
        let stddev_exec_cpu: f64 = std_deviation(x_s).unwrap_or(0.0);
        let mut sql_type = "?".to_string();
        let s = collection.awrs.iter()
                                .flat_map(|a| a.sql_elapsed_time.clone())
                                .find(|s| s.sql_id == sql_id);
        if s.is_some() {
            sql_type = s.unwrap().sql_type;
        }

        make_notes!(&logfile_name, args.quiet, 0, "{: >24}{:.2}% of probes\n", "Marked as TOP in ", (x.len() as f64 / x_vals.len() as f64 )* 100.0);
        make_notes!(&logfile_name, args.quiet, 0, "{: >35} {: <16.2} \tSTDDEV Ela by Exec: {:.2}\n", "--- AVG Ela by Exec:", avg_exec_t, stddev_exec_t);
        make_notes!(&logfile_name, args.quiet, 0, "{: >35} {: <16.2} \tSTDDEV CPU by Exec: {:.2}\n", "--- AVG CPU by Exec:", avg_exec_t_cpu, stddev_exec_t_cpu);
        make_notes!(&logfile_name, args.quiet, 0, "{: >36} {: <16.2} \tSTDDEV Ela Time   : {:.2}\n", "--- AVG Ela Time (s):", avg_exec_s, stddev_exec_s);
        make_notes!(&logfile_name, args.quiet, 0, "{: >36} {: <16.2} \tSTDDEV CPU Time   : {:.2}\n", "--- AVG CPU Time (s):", avg_exec_cpu, stddev_exec_cpu);
        make_notes!(&logfile_name, args.quiet, 0, "{: >38} {: <14.2} \tSTDDEV No. executions:  {:.2}\n", "--- AVG No. executions:", avg_exec_n, stddev_exec_n);
        make_notes!(&logfile_name, args.quiet, 0, "{: >23} {} \n", "MODULE: ", top_stats.sqls.get(&sql_id).unwrap().blue());
        if !sql_type.is_empty() {
            make_notes!(&logfile_name, args.quiet, 0, "{: >23} {}\n\n", "  TYPE: ", sql_type.green());
        }

        sql_data.sql_id = sql_id.clone();
        sql_data.module = top_stats.sqls.get(&sql_id).unwrap().clone();
        sql_data.sql_type = sql_type.clone();
        sql_data.correlation_with_db_time = corr; 
        sql_data.marked_as_top_in_pct_of_probes = (x.len() as f64 / x_vals.len() as f64 )* 100.0;
        sql_data.avg_elapsed_time_by_exec = avg_exec_t;
        sql_data.stddev_elapsed_time_by_exec = stddev_exec_t;
        sql_data.avg_cpu_time_by_exec = avg_exec_t_cpu;
        sql_data.stddev_cpu_time_by_exec = stddev_exec_t_cpu;
        sql_data.avg_elapsed_time_cumulative_s = avg_exec_s;
        sql_data.stddev_elapsed_time_cumulative_s = stddev_exec_s;
        sql_data.avg_cpu_time_cumulative_s = avg_exec_cpu;
        sql_data.stddev_cpu_time_cumulative_s = stddev_exec_cpu;
        sql_data.avg_number_of_executions = avg_exec_n;
        sql_data.stddev_number_of_executions = stddev_exec_n;

        for (k,v) in &top_sections {
            if k == "SQL CPU" {
                sql_data.pct_of_time_sql_was_found_in_other_top_sections.sqls_by_cpu_time_pct = *v;
            } else if k == "SQL I/O" {
                sql_data.pct_of_time_sql_was_found_in_other_top_sections.sqls_by_user_io_pct = *v;
            } else if k == "SQL READS" {
                sql_data.pct_of_time_sql_was_found_in_other_top_sections.sqls_by_reads = *v;
            } else if k == "SQL GETS" {
                sql_data.pct_of_time_sql_was_found_in_other_top_sections.sqls_by_gets = *v;
            }
        }

        /* Print table of detected anomalies for given SQL_ID (key.1)*/
        let anomaly_id = format!("mad_{}", &sql_id);
        let mut anomalies_flag: bool = false;

        if let Some(anomalies) = top_stats.sql_elapsed_time_anomalies_mad.get(&key.1) {
            let anomalies_detection_msg = "Detected anomalies using Median Absolute Deviation on the following dates:".to_string().red();
            anomalies_flag = true;
            make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n",  anomalies_detection_msg);

            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("Date"),
                Cell::new("MAD Score"),
                Cell::new("Elapsed Time (s)"),
                Cell::new("Executions"),
                Cell::new("Ela time / exec (s)")
            ]));

            for (i,a) in anomalies.iter().enumerate() {
                let mut mad_sql = MadAnomaliesSQL::default();

                let c_event = Cell::new(&a.0);
                
                let c_mad_score: Cell = Cell::new(&format!("{:.3}", a.1));
                
                let sql_id = collection.awrs
                                                    .iter()
                                                    .filter(|awr| awr.snap_info.begin_snap_time == a.0)
                                                    .flat_map(|awr| awr.sql_elapsed_time.iter())
                                                    .find(|s| s.sql_id == key.1).unwrap();
                
                let c_elapsed_time = Cell::new(&format!("{:.3}", sql_id.elapsed_time_s));
                let c_executions = Cell::new(&format!("{:.3}", sql_id.executions));
                let c_elapsed_time_exec = Cell::new(&format!("{:.3}", sql_id.elpased_time_exec_s));

                table.add_row(Row::new(vec![c_event.clone(),c_mad_score.clone(),c_elapsed_time.clone(), c_executions.clone(), c_elapsed_time_exec.clone()]));
                
                mad_sql.anomaly_date = a.0.clone();
                mad_sql.mad_score= a.1;
                mad_sql.elapsed_time_cumulative_s = sql_id.elapsed_time_s;
                mad_sql.number_of_executions = sql_id.executions;
                mad_sql.avg_exec_time_for_execution = sql_id.elpased_time_exec_s;
                
                sql_data.median_absolute_deviation_anomalies.push(mad_sql.clone());

                table_anomalies.push_str(&format!(
                    r#"<tr>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                    </tr>"#,
                    c_event.to_string(),c_mad_score.to_string(),c_elapsed_time.to_string(), c_executions.to_string(), c_elapsed_time_exec.to_string()
                ));

                let begin_snap_id = collection.awrs
                                                    .iter()
                                                    .find_map(|awr| {
                                                        (awr.snap_info.begin_snap_time == a.0)
                                                            .then(|| awr.snap_info.begin_snap_id)
                                                    }).unwrap();

                //anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()), format!("SQL:      {}", key.1));
                anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()),"SQL", &key.1);
            }
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }
            
        } else {
            let no_anomalies_txt = format!("\t\tNo anomalies detected based on MAD threshold: {}\n", args.mad_threshold);
            anomalies_flag = false;
            make_notes!(&logfile_name, args.quiet, 0, "{}", no_anomalies_txt.green().italic());
        }
        make_notes!(&logfile_name, args.quiet, 0,"\n");

        /* SQLs - Generate a row for the HTML table */
        table_sqls.push_str(&format!(
            r#"
            <tr>
                <td><a href="sqlid/sqlid_{}.html" target="_blank" class="nav-link" style="font-weight: bold">{}</a></td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{}</td>
            </tr>
            "#,
                &sql_id, &sql_id,
                avg_exec_t, stddev_exec_t,  //Time per exec
                avg_exec_n, stddev_exec_n,  //Number of executions
                avg_exec_s, stddev_exec_s,  //Total execution time
                avg_exec_t_cpu, stddev_exec_t_cpu, //CPU Time by Exec 
                avg_exec_cpu, stddev_exec_cpu, //Total CPU Time
                corr,
                (x.len() as f64 / x_vals.len() as f64 )* 100.0,
                if anomalies_flag {
                    format!(r#"<a href="javascript:void(0);" onclick="toggleRow('{}')" class="nav-link" style="font-weight: bold">Yes</a>"#, anomaly_id)
                } else {
                    "No".to_string()
                }
            )
        );
        // Include: the collapsible anomaly table row
        if anomalies_flag {
            table_sqls.push_str(&format!(
                r#"<tr id="{0}" class="anomaly-row" style="display: none;">
                    <td colspan="8">
                        <table class="inner-anomalies-table">
                            <thead>
                                <tr>
                                    <th onclick="sortInnerTable('{0}',0)" style="cursor: pointer;">Date</th>
                                    <th onclick="sortInnerTable('{0}',1)" style="cursor: pointer;">MAD Score</th>
                                    <th onclick="sortInnerTable('{0}',2)" style="cursor: pointer;">Total Wait (s)</th>
                                    <th onclick="sortInnerTable('{0}',3)" style="cursor: pointer;">Waits</th>
                                    <th onclick="sortInnerTable('{0}',4)" style="cursor: pointer;">AVG Wait (s)</th>
                                </tr>
                            </thead>
                            <tbody>
                                {1}
                            </tbody>
                        </table>
                    </td>
                </tr>"#,
                anomaly_id,
                table_anomalies
        ))};
        table_anomalies = "".to_string();
        anomalies_flag = false;
        
        let mut sql_corr_txt: Vec<String> = Vec::new();
        
        let mut table = Table::new();
        table.set_titles(Row::new(vec![
            Cell::new("Wait Event Name"),
            Cell::new("Pearson correlation coefficient"),
        ]));
        let mut found_strong_events: bool = false;

        for (key,ev) in &y_vals_events_sorted {
            let mut corr_events = WaitEventsWithStrongCorrelation::default();
            let crr = pearson_correlation_2v(&yv, &ev);
            let corr_text = format!("{: >32} | {: <32} : {:.2}", "+".to_string(), key.1.clone(), crr);
            if crr >= 0.333 || crr <= - 0.333 { //Correlation considered high enough to mark it
                sql_corr_txt.push(format!(r#"<span style="color:red; font-weight:bold;">{}</span>"#, corr_text));
                //make_notes!(&logfile_name, args.quiet, 0, "{}\n", corr_text.red().bold());
                let c_event_name = Cell::new(&key.1);
                let c_corr_factor = Cell::new(&format!("{:.2}", crr));
                 table.add_row(Row::new(vec![
                        c_event_name,
                        c_corr_factor,
                 ]));
                 found_strong_events = true;
                 crr_event_sql_map.entry(key.1.clone()).or_insert_with(HashMap::new).insert(sql_id.clone(), crr);
                 corr_events.event_name=key.1.clone();
                 corr_events.correlation_value = crr;
                 sql_data.wait_events_with_strong_pearson_correlation.push(corr_events);
            }
        }
        
        if found_strong_events {
            let sql_corr_txt_header = "\t\tWait events with strong Pearson correlation coefficient factor.\n".to_string();
            make_notes!(&logfile_name, args.quiet, 0, "{}", sql_corr_txt_header.bold().blue());
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }
        }
        

        let mut ash_events: HashMap<String, Vec<f64>> = HashMap::new();
        collection.awrs.iter()
                        .filter(|a| a.top_sql_with_top_events.contains_key(&sql_id))
                        .flat_map(|a| a.top_sql_with_top_events.clone())
                        .for_each(|(s_id, top_event)| {
                            if s_id == sql_id {
                                ash_events.entry(top_event.event_name.clone()).or_insert_with(Vec::new).push(top_event.pct_activity);
                                ash_event_sql_map.entry(top_event.event_name).or_insert_with(HashSet::new).insert(s_id);
                            }
                        });

        let mut ash_events_html = String::new();
        if !ash_events.is_empty() {
            let mut sql_ash = WaitEventsFromASH::default();
            let sql_ash_txt_header = "Wait events actually found in ASH section of AWR reports:\n".to_string();
            make_notes!(&logfile_name, args.quiet, 0, "\n\t\t{}", sql_ash_txt_header.bold().blue());

            
            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("Wait Event Name"),
                Cell::new("AVG % of DB Time in SQL"),
                Cell::new("STDDEV % of DB Time in SQL"),
                Cell::new("Count")
            ]));

            for (evname, pctvalues) in ash_events {
                let avg_pct = mean(pctvalues.clone()).unwrap();
                let stddev_pct = std_deviation(pctvalues.clone()).unwrap();

                let c_event_name = Cell::new(&evname);
                let c_avg_pct = Cell::new(&format!("{:.2}", avg_pct));
                let c_stddev_pct = Cell::new(&format!("{:.2}", stddev_pct));
                let c_count = Cell::new(&format!("{}", pctvalues.len()));
                 table.add_row(Row::new(vec![
                        c_event_name,
                        c_avg_pct,
                        c_stddev_pct,
                        c_count
                 ]));

                sql_ash.event_name = evname.clone();
                sql_ash.avg_pct_of_dbtime_in_sql = avg_pct;
                sql_ash.stddev_pct_of_dbtime_in_sql = stddev_pct;
                sql_ash.count=pctvalues.len() as u64;
                sql_data.wait_events_found_in_ash_sections_for_this_sql.push(sql_ash.clone());
            }

            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }
            
            ash_events_html = table_to_html_string(&table, &sql_ash_txt_header, &["Wait Event Name", "AVG % of DB Time in SQL", "STDDEV % of DB Time in SQL", "Count"]);
        }

        let mut sql_text = format!("Security level {} does not allow gathering SQL text, use level 2 or higher", args.security_level);
        if args.security_level >= 2 {
            sql_text = format!("<code><details><summary>FULL SQL TEXT</summary>{}</details></code>\n</body>",collection.sql_text.get(&sql_id).unwrap())
        }

        // Format the content as HTML
        let sqlid_html_content: String = format!(
            r#"<!DOCTYPE html>
            <html lang="en">
            <head>
                <meta charset="UTF-8">
                <meta name="viewport" content="width=device-width, initial-scale=1.0">
                <title>{sql_id}</title>
                <style>
                    body {{ font-family: Arial, sans-serif; }}
                    .content {{ font-size: 16px; }}
                    .bold {{ font-weight: bold; }}
                    .italic {{ font-style: italic; }}
                </style>
            </head>
            <body>
                <div class="content">
                    <p><h2 style="width:100%;text-align:center;">{sql_id}</h2></p>
                    <p><span style="color:blue;font-weight:bold;">Module:<br></span>{module}</p>
                    <p><span style="color:blue;font-weight:bold;">SQL Text:<br></span>{sql_txt}</p>
                    <p><span style="color:blue;font-weight:bold;">Other Top Sections:<br></span> {top_section}</p>
                    <p><span style="color:blue;font-weight:bold;">Correlations:<br></span>{sql_corr_txt}</p>
                    {ash_table}
                </div>
            "#,
            sql_id = sql_id,
            module=top_stats.sqls.get(&sql_id).unwrap(),
            top_section=top_sections.iter().map(|(key, value)| format!("<span class=\"bold\">{}:</span> {:.2}%", key, value)).collect::<Vec<String>>().join("<br>"),
            sql_corr_txt = sql_corr_txt.join("<br>"),
            sql_txt = sql_text,
            ash_table = ash_events_html
        );

        // Insert this into already existing sqlid_*.html file
        let filename: String = format!("{}/sqlid/sqlid_{}.html", &html_dir,sql_id);
        let mut sql_file: String = fs::read_to_string(&filename)
            .expect(&format!("Failed to read file: {}", filename));
        sql_file = sql_file.replace(
            "<body>",
            &format!("<body>\n{}\n",sqlid_html_content)
        );
        if let Err(e) = fs::write(&filename, sql_file) {
            eprintln!("Error writing file {}: {}", filename, e);
        }

        top_sqls.push(sql_data);
    }

    report_for_ai.top_sqls_by_elapsed_time = top_sqls;

    let sqls_table_html: String = format!(
        r#"
        <table id="sqls-table">
            <thead>
                <tr>
                    <th onclick="sortTable('sqls-table',0)" style="cursor: pointer;">SQL ID</th>
                    <th onclick="sortTable('sqls-table',1)" style="cursor: pointer;">AVG Ela by Exec</th>
                    <th onclick="sortTable('sqls-table',2)" style="cursor: pointer;">STDDEV Ela by Exec</th>
                    <th onclick="sortTable('sqls-table',3)" style="cursor: pointer;">AVG No. executions</th>
                    <th onclick="sortTable('sqls-table',4)" style="cursor: pointer;">STDDEV No. executions</th>
                    <th onclick="sortTable('sqls-table',5)" style="cursor: pointer;">AVG Total Execution Time</th>
                    <th onclick="sortTable('sqls-table',6)" style="cursor: pointer;">STDDEV Total Execution Time</th>
                    <th onclick="sortTable('sqls-table',7)" style="cursor: pointer;">AVG CPU Time by Exec</th>
                    <th onclick="sortTable('sqls-table',8)" style="cursor: pointer;">STDDEV CPU Time by Exec</th>
                    <th onclick="sortTable('sqls-table',9)" style="cursor: pointer;">AVG Total CPU Time</th>
                    <th onclick="sortTable('sqls-table',10)" style="cursor: pointer;">STDDEV Total CPU Time</th>
                    <th onclick="sortTable('sqls-table',11)" style="cursor: pointer;">Correlation of DBTime</th>
                    <th onclick="sortTable('sqls-table',12)" style="cursor: pointer;">TOP in % Probes</th>
                    <th onclick="sortTable('sqls-table',13)" style="cursor: pointer;">Anomalies</th>
                </tr>
            </thead>
            <tbody>
            {}
            </tbody>
        </table>
        "#,
        table_sqls
    );

    /* If ASH data is present, add SQL_ID information to wait event html reports */
    if !ash_event_sql_map.is_empty() {
        merge_ash_sqls_to_events(ash_event_sql_map, &html_dir);
    }

    if !crr_event_sql_map.is_empty() {
        merge_correlated_sqls_to_events(crr_event_sql_map, &html_dir);
    }


    // STATISTICS Correltation to DBTime

    //println!("\n{}","Statistics");
    make_notes!(&logfile_name, false, 0, "\n");
    /* "IO Stats by Function" are goin into report */
    make_notes!(&logfile_name, false, 2,"{}\n",format!("IO Statistics by Function - Summary").yellow());
    // Create the table
    let mut table_iostats = Table::new();

    table_iostats.set_titles(Row::new(vec![
        Cell::new("Function").with_style(Attr::Bold),
        Cell::new("Statistic").with_style(Attr::Bold),
        Cell::new("Mean").with_style(Attr::Bold),
        Cell::new("Std Dev").with_style(Attr::Bold),
    ]));
    
    // Function to convert metric names to readable format
    fn format_metric_name(metric: &str) -> String {
        match metric {
            "reads_data" => "Read Data (MB)".to_string(),
            "reads_req_s" => "Read Requests/sec".to_string(),
            "reads_data_s" => "Read Data (MB)/sec".to_string(),
            "writes_data" => "Write Data (MB)".to_string(),
            "writes_req_s" => "Write Requests/sec".to_string(),
            "writes_data_s" => "Write Data (MB)/sec".to_string(),
            "waits_count" => "Wait Count".to_string(),
            "avg_time" => "Wait Avg Time (ms)".to_string(),
            _ => metric.to_string(), // fallback for unknown metrics
        }
    }

    let mut iostat_summary: Vec<IOStatsByFunctionSummary> = Vec::new();
    // Add data rows
    for (function_name, stats) in &iostats {
        if function_name == "zMAIN"{
            continue;
        }
        let mut iostat = IOStatsByFunctionSummary::default();
        iostat.function_name = function_name.clone();

        let mut stat_metrics: Vec<String> = Vec::new();
        let mut stat_mean: Vec<String> = Vec::new();
        let mut stat_std_dev: Vec<String> = Vec::new();

        for (metric_name, (mean, std_dev)) in stats {
            stat_metrics.push(format_metric_name(&metric_name));
            stat_mean.push(format!("{:.2}", mean));
            stat_std_dev.push(format!("{:.2}", std_dev));
            iostat.statistics_summary.push(StatsSummary{statistic_name: format_metric_name(&metric_name), avg_value: *mean, stddev_value: *std_dev});
        }
        table_iostats.add_row(Row::new(vec![
            Cell::new(&function_name),
            Cell::new(&stat_metrics.join("\n")),
            Cell::new(&stat_mean.join("\n")),
            Cell::new(&stat_std_dev.join("\n")),
        ]));
        iostat_summary.push(iostat);
    }
    make_notes!(&logfile_name, args.quiet, 0, "{}\n", table_iostats);

    make_notes!(&logfile_name, false, 2,"{}\n",format!("Latch Activity Statistics - Summary").yellow());
    make_notes!(&logfile_name, args.quiet, 0, "{}\n", table_latch);

    /******** Report Segment Statistics Summary */
    let segstats = report_segments_summary(&collection.awrs, &args, &logfile_name, &html_dir, &mut report_for_ai);
    /********************************************/

    make_notes!(&logfile_name, args.quiet, 0, "\n\n");
    make_notes!(&logfile_name, false, 2, "{}", "Instance Statistics: Correlation with DB Time for values >= 0.5 and <= -0.5".yellow());
    make_notes!(&logfile_name, args.quiet, 0, "\n\n");
    let mut sorted_correlation = report_instance_stats_cor(instance_stats, y_vals_dbtime);
    let mut stats_table_rows = String::new();
    for ((score, key), value) in sorted_correlation.iter().rev() { // Sort in descending order
        stats_table_rows.push_str(&format!(
            r#"<tr><td>{}</td><td>{:.3}</td></tr>"#,
            key,
            value
        ));
    }
    table_stat_corr = format!(
        r#"<!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="UTF-8">
            <meta name="viewport" content="width=device-width, initial-scale=1.0">
            <title>Correlation of Instance Statistics with DB Time</title>
            <style>
                body {{ font-family: Arial, sans-serif; }}
                .content {{ font-size: 14px; }}
                table {{
                    width: 30%;
                    border-collapse: collapse;
                    margin-top: 20px;
                }}
                th, td {{
                    border: 1px solid black;
                    padding: 8px;
                    text-align: center;
                }}
                th {{
                    background-color: #632e4f;
                    color: white;
                }}
                tr:nth-child(even) {{
                    background-color: #f2f2f2;
                }}
                td:first-child {{
                    text-align: right;
                    font-weight: bold;
                }}
            </style>
            <script>//JAS-MIN scripts
                function sortTable(tableId,columnId) {{
                    var table = document.getElementById(tableId);
                    var tbody = table.getElementsByTagName("tbody")[0];
                    var rows = Array.from(tbody.getElementsByTagName("tr"));
                    var isAscending = table.getAttribute("data-sort-order") !== "asc";
                    table.setAttribute("data-sort-order", isAscending ? "asc" : "desc");
                    rows.sort(function(rowA, rowB){{
                        var cellA = rowA.getElementsByTagName("td")[columnId].innerText.trim();
                        var cellB = rowB.getElementsByTagName("td")[columnId].innerText.trim();
                        var numA = parseFloat(cellA);
                        var numB = parseFloat(cellB);
                        if (!isNaN(numA) && !isNaN(numB)){{
                            return isAscending ? numA - numB : numB - numA;
                        }} else{{
                            return isAscending ? cellA.localeCompare(cellB) : cellB.localeCompare(cellA);
                        }}
                    }});
                    tbody.innerHTML = "";
                    rows.forEach(row => tbody.appendChild(row));
                }}
            </script>
        </head>
        <body>
            <div class="content">
                <p><a href="https://github.com/ora600pl/jas-min" target="_blank">
                <img src="https://raw.githubusercontent.com/rakustow/jas-min/main/img/jasmin_LOGO_white.png" width="150" alt="JAS-MIN" onerror="this.style.display='none';"/>
                </a></p>
                <p><span style="font-size:20px;font-weight:bold;">Correlation of Instance Statistics with DB Time for Values >= 0.5 and <= -0.5</span></p>
                <table id="stats_corr_table" >
                    <thead>
                        <tr>
                            <th onclick="sortTable('stats_corr_table',0)" style="cursor: pointer;">Instance Statistic</th>
                            <th onclick="sortTable('stats_corr_table',1)" style="cursor: pointer;">Correlation</th>
                        </tr>
                    </thead>
                    <tbody>
                        {}
                    </tbody>
                </table>
            </div>
        </body>
        </html>
        "#,
        stats_table_rows
    );

    // Write to the file
    let stats_corr_filename: String = format!("{}/stats/statistics_corr.html", &html_dir);
    if let Err(e) = fs::write(&stats_corr_filename, table_stat_corr) {
        eprintln!("Error writing file {}: {}", stats_corr_filename, e);
    }
    for (k,v) in sorted_correlation {
        make_notes!(&logfile_name, args.quiet, 0, "\t{: >64} : {:.2}\n", &k.1, v);
        report_for_ai.instance_stats_pearson_correlation.push(InstanceStatisticCorrelation{stat_name: k.1.clone(), pearson_correlation_value: v});
    }
    /* Add information about stats anomalies to the summary */
    let stat_anomalies = detect_stats_anomalies_mad(&collection.awrs, &args);
    let all_stats = top_stats.stat_names;
    for s in all_stats {
        if let Some(anomalies) = stat_anomalies.get(&s.0) {
            for a in anomalies {
                let begin_snap_id = collection.awrs
                                                    .iter()
                                                    .find_map(|awr| {
                                                        (awr.snap_info.begin_snap_time == a.0)
                                                            .then(|| awr.snap_info.begin_snap_id)
                                                    }).unwrap();

                //anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()), format!("STAT:     {}", s.0.clone()));
                anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()),"STAT", s.0.clone());

            }
        }
    } 
    /********************************************************/

    /* Add information about Dictionary Cache anomalies to the summary */
    let stat_anomalies = detect_dc_anomalies_mad(&collection.awrs, &args);
    let all_stats: HashSet<String> = collection.awrs
                                    .iter()
                                    .flat_map(|a| a.dictionary_cache.clone())
                                    .map(|dc| dc.statname)
                                    .collect();
    for s in all_stats {
        if let Some(anomalies) = stat_anomalies.get(&s) {
            for a in anomalies {
                let begin_snap_id = collection.awrs
                                                    .iter()
                                                    .find_map(|awr| {
                                                        (awr.snap_info.begin_snap_time == a.0)
                                                            .then(|| awr.snap_info.begin_snap_id)
                                                    }).unwrap();

                anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()),"DC", s.clone());

            }
        }
    } 
    /********************************************************/


    /* Add information about Library Cache anomalies to the summary */
    let stat_anomalies = detect_libcache_anomalies_mad(&collection.awrs, &args);
    let all_stats: HashSet<String> = collection.awrs
                                    .iter()
                                    .flat_map(|a| a.library_cache.clone())
                                    .map(|lc| lc.statname)
                                    .collect();

    for s in all_stats {
        if let Some(anomalies) = stat_anomalies.get(&s) {
            for a in anomalies {
                let begin_snap_id = collection.awrs
                                                    .iter()
                                                    .find_map(|awr| {
                                                        (awr.snap_info.begin_snap_time == a.0)
                                                            .then(|| awr.snap_info.begin_snap_id)
                                                    }).unwrap();

                anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()),"LC", s.clone());

            }
        }
    } 
    /********************************************************/

    /* Add information about Latch Activity anomalies to the summary */
    let stat_anomalies = detect_latch_activity_anomalies_mad(&collection.awrs, &args);
    let all_stats: HashSet<String> = collection.awrs
                                    .iter()
                                    .flat_map(|a| a.latch_activity.clone())
                                    .map(|lc| lc.statname)
                                    .collect();

    for s in all_stats {
        if let Some(anomalies) = stat_anomalies.get(&s) {
            for a in anomalies {
                let begin_snap_id = collection.awrs
                                                    .iter()
                                                    .find_map(|awr| {
                                                        (awr.snap_info.begin_snap_time == a.0)
                                                            .then(|| awr.snap_info.begin_snap_id)
                                                    }).unwrap();

                anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()),"LATCH", s.clone());

            }
        }
    } 
    /********************************************************/


    /* Add information about Time Model anomalies to the summary */
    let stat_anomalies = detect_time_model_anomalies_mad(&collection.awrs, &args);
    let all_stats: HashSet<String> = collection.awrs
                                    .iter()
                                    .flat_map(|a| a.time_model_stats.clone())
                                    .map(|lc| lc.stat_name)
                                    .collect();

    for s in all_stats {
        if let Some(anomalies) = stat_anomalies.get(&s) {
            for a in anomalies {
                let begin_snap_id = collection.awrs
                                                    .iter()
                                                    .find_map(|awr| {
                                                        (awr.snap_info.begin_snap_time == a.0)
                                                            .then(|| awr.snap_info.begin_snap_id)
                                                    }).unwrap();

                anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()),"TM", s.clone());

            }
        }
    } 
    /********************************************************/

    /****************   Report anomalies summary ************/
    //println!("{}","Anomalies Summary");
    make_notes!(&logfile_name, false, 0, "\n\n");
    make_notes!(&logfile_name, false, 1, "{}\n", "ANOMALIES".bold().green());
     /* Load Profile Anomalies detection and report */
    make_notes!(&logfile_name, false, 2, "\n{} {}\n", "Load Profile Anomalies detection using Median Absolute Deviation threshold:".yellow(), args.mad_threshold);
    
    let all_loadprofile: HashSet<String> = collection.awrs
                                                .iter()
                                                .flat_map(|awr| &awr.load_profile)
                                                .map(|l| l.stat_name.clone())
                                                .collect();
    let profile_anomalies = detect_loadprofile_anomalies_mad(&collection.awrs, &args);
    for l in all_loadprofile {
        let stat_name = l.bold();
        let per_second_v: Vec<f64>      = collection.awrs
                                                .iter()
                                                .flat_map(|awr| &awr.load_profile)
                                                .filter(|lp| lp.stat_name == l)
                                                .map(|flp| flp.per_second)
                                                .collect();
        let mean_per_s = mean(per_second_v).unwrap_or(0.0);

        if let Some(anomalies) = profile_anomalies.get(&l) {
            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("Date"),
                Cell::new("MAD Score"),
                Cell::new("MAD Threshold"),
                Cell::new("Per Second"),
                Cell::new("AVG Per Second"),
            ]));
            
            let anomalies_str = format!("\n\tAnomalies detected for \"{}\", AVG value per second is: {:.2}", stat_name, mean_per_s).red();
            make_notes!(&logfile_name, args.quiet, 0, "{}\n", anomalies_str);
            for a in anomalies {
                
                let per_second_this_date = collection.awrs
                                                            .iter()
                                                            .find(|awr| awr.snap_info.begin_snap_time == a.0)
                                                            .unwrap()
                                                            .load_profile
                                                            .iter()
                                                            .find(|lp| lp.stat_name == l)
                                                            .map(|lpf| lpf.per_second)
                                                            .unwrap_or(0.0);

                let c_date = Cell::new(&a.0);
                let c_mad = Cell::new(&format!("{:.2}", a.1));
                let c_per_second = Cell::new(&format!("{}",per_second_this_date));
                let c_avg_per_second = Cell::new(&format!("{:.2}", mean_per_s));
                let c_mad_threshold = Cell::new(&format!("{}", args.mad_threshold));
                table.add_row(Row::new(vec![c_date, c_mad, c_mad_threshold, c_per_second, c_avg_per_second]));

                report_for_ai.load_profile_anomalies.push(LoadProfileAnomalies {
                    load_profile_stat_name: l.clone(),
                    anomaly_date: a.0.clone(),
                    mad_score: a.1,
                    mad_threshold: args.mad_threshold,
                    per_second: per_second_this_date,
                    avg_value_per_second: mean_per_s,
                });

                let begin_snap_id = collection.awrs
                                                    .iter()
                                                    .find_map(|awr| {
                                                        (awr.snap_info.begin_snap_time == a.0)
                                                            .then(|| awr.snap_info.begin_snap_id)
                                                    }).unwrap();

                //anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()), format!("LP:       {}", l.clone()));
                anomalies_join(&mut anomalies_summary, (begin_snap_id, a.0.clone()),"LP", l.clone());

            }
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }
        } else {
            let no_anomalies_str = format!("\tNo anomalies detected for \"{}\", AVG value per second iss: {:.2}", stat_name, mean_per_s).green();
            make_notes!(&logfile_name, args.quiet, 0, "\n{}\n", no_anomalies_str);
        }
    }
    /***********************************************/
    
    let anomalies_summary_html: String = format!(
        r#"
        <table id="anomalies-sum-table">
            <thead>
                <tr>
                    <th onclick="sortTable('anomalies-sum-table',0)" style="cursor: pointer;">BEGIN SNAP ID</th>
                    <th onclick="sortTable('anomalies-sum-table',1)" style="cursor: pointer;">BEGIN SNAP DATE</th>
                    <th onclick="sortTable('anomalies-sum-table',2)" style="cursor: pointer;">Anomalies summary</th>
                    <th onclick="sortTable('anomalies-sum-table',3)" style="cursor: pointer;">Anomalies count</th>
                </tr>
            </thead>
            <tbody>
            {}
            </tbody>
        </table>
        "#,
        report_anomalies_summary(&mut anomalies_summary, &args, &logfile_name, &mut report_for_ai)
    );
    
    let mut snap_dates: Vec<String> = Vec::new();
    let mut anomaly_types: BTreeMap<String, usize> = BTreeMap::new();
    let mut heat_data: HashMap<(String, String), usize> = HashMap::new();

    for ((_snap_id, snap_date), anomaly_map) in &anomalies_summary {
        // Find the actual formatted snap_date in x_vals (e.g., with "(25)" suffix)
        let matching_xval = x_vals.iter().find(|x| x.starts_with(snap_date));
        if let Some(xval_key) = matching_xval {
            for (atype, values) in anomaly_map {
                anomaly_types.entry(atype.clone()).or_insert(0);
                let count = values.len();
                *heat_data.entry((xval_key.clone(), atype.clone())).or_insert(0) += count;
            }
        }
    }

    let anomaly_labels: Vec<String> = anomaly_types.keys().cloned().collect();
    // Build the z matrix
    let z_data: Vec<Vec<usize>> = anomaly_labels
        .iter()
        .map(|atype| {
            x_vals
                .iter()
                .map(|date| *heat_data.get(&(date.clone(), atype.clone())).unwrap_or(&0))
                .collect()
        })
        .collect();

    let anomalies_heatmap = HeatMap::new(x_vals.clone(), anomaly_labels.clone(), z_data)
        .x_axis("x1")
        .y_axis("y6")
        .hover_on_gaps(true)
        .show_legend(false)
        .show_scale(false)
        .color_scale(ColorScale::Palette(ColorScalePalette::Electric))
        .reverse_scale(true)
        .zauto(true)
        .name("#");
    
    plot_main.add_trace(anomalies_heatmap);
    /*************************/
        
    // Prepare Plots LAYOUTS
    let layout_main: Layout = Layout::new()
        .height(1500)
        .grid(
            LayoutGrid::new()
                .rows(5)
                .columns(1),
                //.row_order(Grid::TopToBottom),
        )
        //.legend(Legend::new()
        //    .y_anchor(Anchor::Top)
        //    .x(1.02)
        //    .y(0.5)
        //)
        .y_axis6(Axis::new()
            .anchor("x1")
            .domain(&[0.785, 1.0])
            .title("Anomalies")
            .visible(true)
            .zero_line(true)
            .range_mode(RangeMode::ToZero)
        )
        .y_axis5(Axis::new()
            .anchor("x1")
            .domain(&[0.63, 0.78])
            .title("SQL Elapsed Time")
            .visible(true)
            .zero_line(true)
            //.range_mode(RangeMode::ToZero)
        )
        .y_axis4(Axis::new()
            .anchor("x1")
            .domain(&[0.47, 0.625])
            .range(vec![0.,])
            .title("CPU Util (%/#)")
            .zero_line(true)
            .range(vec![0.,])
            .range_mode(RangeMode::ToZero)
        )
        .y_axis3(Axis::new()
            .anchor("x1")
            .domain(&[0.31, 0.465])
            .title("Wait Events (s)")
            .zero_line(true)
            .range(vec![0.,])
            .range_mode(RangeMode::ToZero)
        )
        .y_axis2(Axis::new()
            .anchor("x1")
            .domain(&[0.155, 0.305])
            .range(vec![0.,])
            .title("#")
            .zero_line(true)
            .range_mode(RangeMode::ToZero)
        )
        .y_axis(Axis::new()
            .anchor("x1")
            .domain(&[0., 0.15])
            .title("(s/s)")
            .zero_line(true)
            .range_mode(RangeMode::ToZero)
        )
        .x_axis(Axis::new()
            .domain(&[0.0, 1.0])
            .anchor("y1")
            .range(vec![0.,])
            .show_grid(true),
        )
        .hover_mode(HoverMode::X);

    let layout_highlight: Layout = Layout::new()
        .height(400)
        .grid(
            LayoutGrid::new()
                .rows(1)
                .columns(6),
                //.row_order(Grid::TopToBottom),
        )
        .hover_mode(HoverMode::X)
        .x_axis7(
            Axis::new()
                    .domain(&[0.9, 1.0])
                    .anchor("y7")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis7(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x7")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis6(
            Axis::new()
                    .domain(&[0.75, 0.85])
                    .anchor("y6")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis6(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x6")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis5(
            Axis::new()
                    .domain(&[0.6, 0.7])
                    .anchor("y5")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis5(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x5")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis4(
            Axis::new()
                    .domain(&[0.45, 0.55])
                    .anchor("y4")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis4(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x4")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis3(
            Axis::new()
                    .domain(&[0.3, 0.4])
                    .anchor("y3")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis3(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x3")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis2(
            Axis::new()
                    .domain(&[0.15, 0.25])
                    .anchor("y2")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis2(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x2")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis(
            Axis::new()
                    .domain(&[0.0, 0.1])
                    .anchor("y1")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x1")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        );
    let layout_highlight2: Layout = Layout::new()
        .height(400)
        .paper_background_color("White")
        .plot_background_color("White")
        .grid(
            LayoutGrid::new()
                .rows(1)
                .columns(1),
                //.row_order(Grid::TopToBottom),
        )
        .hover_mode(HoverMode::X)
        .x_axis7(
            Axis::new()
                    .domain(&[0.9, 1.0])
                    .anchor("y7")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis7(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x7")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis6(
            Axis::new()
                    .domain(&[0.75, 0.85])
                    .anchor("y6")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis6(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x6")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis5(
            Axis::new()
                    .domain(&[0.6, 0.7])
                    .anchor("y5")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis5(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x5")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis4(
            Axis::new()
                    .domain(&[0.45, 0.55])
                    .anchor("y4")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis4(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x4")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis3(
            Axis::new()
                    .domain(&[0.3, 0.4])
                    .anchor("y3")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis3(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x3")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis2(
            Axis::new()
                    .domain(&[0.15, 0.25])
                    .anchor("y2")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis2(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x2")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        )
        .x_axis(
            Axis::new()
                    .domain(&[0.0, 0.1])
                    .anchor("y1")
                    .range(vec![0.,])
                    .show_grid(false)
        )
        .y_axis(
            Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x1")
                    .range(vec![0.,])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
        );


    println!("\n{}","==== GENERATING PLOTS ====".bold().bright_cyan());
    plot_main.set_layout(layout_main);
    plot_highlight.set_layout(layout_highlight);
    plot_highlight2.set_layout(layout_highlight2);
    plot_main.write_html(fname.clone());
    plot_highlight.write_html(format!("{}/stats/jasmin_highlight.html", &html_dir));
    plot_highlight2.write_html(format!("{}/stats/jasmin_highlight2.html", &html_dir));
    
    let first_snap_time: String = collection.awrs.first().unwrap().snap_info.begin_snap_time.clone();
    let last_snap_time: String = collection.awrs.last().unwrap().snap_info.end_snap_time.clone();
    let db_instance_info_html: String = format!(
        "<div id=\"db-instance-info\" style=\"margin-bottom: 20px;\">
            <span style=\"margin-left: auto;\"> <strong>JAS-MIN</strong> v{}&nbsp;&nbsp;&nbsp</span>
            <br>
            <span style=\"width: 100%; text-align: center;\"><strong>DB ID:</strong> {}&nbsp;&nbsp;&nbsp<strong>Platform:</strong> {}&nbsp;&nbsp;&nbsp<strong>Release:</strong> {}&nbsp;&nbsp;&nbsp<strong>Startup Time:</strong> {}&nbsp;&nbsp;&nbsp<strong>RAC:</strong> {}&nbsp;&nbsp;&nbsp<strong>Instance Number:</strong> {}&nbsp;&nbsp;&nbsp<strong>CPUs:</strong> {}&nbsp;&nbsp;&nbsp<strong>Cores:</strong> {}&nbsp;&nbsp;&nbsp<strong>Sockets:</strong> {}&nbsp;&nbsp;&nbsp<strong>Memory (G):</strong> {}</span>
            <br>
            <span style=\"width: 100%; text-align: center;\"><strong>Snap range:</strong> {} - {}</span>
    </div>",
        env!("CARGO_PKG_VERSION"),
        collection.db_instance_information.db_id,
        collection.db_instance_information.platform,
        collection.db_instance_information.release,
        collection.db_instance_information.startup_time,
        collection.db_instance_information.rac,
        collection.db_instance_information.instance_num,
        collection.db_instance_information.cpus,
        collection.db_instance_information.cores,
        collection.db_instance_information.sockets,
        collection.db_instance_information.memory,
        first_snap_time,
        last_snap_time
    );
    
    let mut bckend_port: String = String::new();
    if !args.backend_assistant.is_empty() {
        bckend_port = std::env::var("PORT").expect("You have to set backend PORT value in .env");
    }
    
    let jasmin_html_scripts: String = format!(
        r#"
        <script src="https://cdn.jsdelivr.net/npm/marked/marked.min.js"></script>
        <script> //JAS-MIN Assistant
        const messages = document.getElementById('messages');
        const input = document.getElementById('user-input');
        const sendBtn = document.getElementById('send-btn');
        messages.innerHTML += `<div class="message ai-msg" style="color: grey;">Example of questions to JAS-MIN Assistant:<br>Summarise Report<br>What is the most important Wait Event</div>`;
        function showLoadingIndicator() {{
            const loadingDiv = document.createElement('div');
            loadingDiv.className = 'message ai-msg loading-message';
            loadingDiv.id = 'loading-indicator';
            loadingDiv.innerHTML = '<span class="loading-dots"></span>';
            messages.appendChild(loadingDiv);
            messages.scrollTop = messages.scrollHeight;
            return loadingDiv;
        }}        
        function removeLoadingIndicator() {{
            const loadingDiv = document.getElementById('loading-indicator');
            if (loadingDiv) {{
                loadingDiv.remove();
            }}
        }}
        async function sendMessage() {{
            const userMsg = input.value.trim();
            if (userMsg === '') return;
            input.disabled = true;
            sendBtn.disabled = true;
            messages.innerHTML += `<div class="message user-msg">${{userMsg}}</div>`;
            messages.scrollTop = messages.scrollHeight;
            input.value = '';
            const loadingDiv = showLoadingIndicator();
            try {{
                const response = await fetch('http://localhost:{}/api/chat', {{
                    method: 'POST',
                    headers: {{ 'Content-Type': 'application/json' }},
                    body: JSON.stringify({{ message: userMsg }})
                }});
                const data = await response.json();
                removeLoadingIndicator();
                messages.innerHTML += `<div class="message ai-msg">${{marked.parse(data.reply)}}</div>`;
                messages.scrollTop = messages.scrollHeight;
            }} catch (error) {{
                removeLoadingIndicator();
                messages.innerHTML += `<div class="message ai-msg">Error retrieving response.</div>`;
                messages.scrollTop = messages.scrollHeight;
            }} finally {{
                // Re-enable input and button
                input.disabled = false;
                sendBtn.disabled = false;
                input.focus();
            }}
        }}
        input.addEventListener('keydown', function(event) {{
            if (event.key === 'Enter' && !input.disabled) {{
                sendMessage();
            }}
        }});
        sendBtn.addEventListener('click', sendMessage);
        </script>
        <script>//JAS-MIN scripts
        function toggleTable(buttonId, tableId) {{
            const button = document.getElementById(buttonId);
            const table = document.getElementById(tableId);
        
            button.addEventListener('click', () => {{
                if (table.style.display === 'none' || table.style.display === '') {{
                    table.style.display = 'table';
                    button.classList.add("button-active");
                }} else {{
                    table.style.display = 'none';
                    button.classList.remove("button-active");
                }}
            }});
        }}
        function toggleTableWithCheckbox(checkboxId, tableId) {{
            const checkbox = document.getElementById(checkboxId);
            const table = document.getElementById(tableId);
            if (!checkbox || !table) return;
            checkbox.addEventListener('change', () => {{
                if (checkbox.checked) {{
                    table.style.display = 'table';
                }} else {{
                    table.style.display = 'none';
                }}
            }});
        }}
        toggleTable('show-events-button', 'events-table');
        toggleTable('show-sqls-button', 'sqls-table');
        toggleTable('show-bgevents-button', 'bgevents-table');
        toggleTable('show-anomalies-button', 'anomalies-sum-table');
        toggleTable('show-JASMINAI-button', 'chat-container');
        function sortTable(tableId, columnId) {{
            const table = document.getElementById(tableId);
            if (!table) return;
            const tbody = table.tBodies[0];
            if (!tbody) return;
            const allRows = Array.from(tbody.rows);
            const rowPairs = [];
            for (let i = 0; i < allRows.length; i++) {{
                const mainRow = allRows[i];
                if (mainRow.classList.contains("anomaly-row")) continue;
                const nextRow = allRows[i + 1];
                let anomalyRow = null;
                if (nextRow && nextRow.classList.contains("anomaly-row")) {{
                    anomalyRow = nextRow;
                    i++;
                }}
                rowPairs.push({{ main: mainRow, anomaly: anomalyRow }});
            }}
            const isAscending = table.getAttribute("data-sort-order") !== "asc";
            table.setAttribute("data-sort-order", isAscending ? "asc" : "desc");
            rowPairs.sort((a, b) => {{
                const cellA = a.main.cells[columnId]?.innerText.trim() || "";
                const cellB = b.main.cells[columnId]?.innerText.trim() || "";
                const numA = parseFloat(cellA);
                const numB = parseFloat(cellB);
                if (!isNaN(numA) && !isNaN(numB)) {{
                    return isAscending ? numA - numB : numB - numA;
                }} else {{
                    return isAscending ? cellA.localeCompare(cellB) : cellB.localeCompare(cellA);
                }}
            }});
            tbody.innerHTML = "";
            let visibleIndex = 0;
            rowPairs.forEach(pair => {{
                pair.main.classList.remove("even", "odd");
                pair.main.classList.add(visibleIndex % 2 === 0 ? "even" : "odd");
                tbody.appendChild(pair.main);
                if (pair.anomaly) {{
                    pair.anomaly.style.display = "none";
                    tbody.appendChild(pair.anomaly);
                }}
                visibleIndex++;
            }});
        }}
        function sortInnerTable(tableId,columnId) {{
            var table = document.getElementById(tableId);
            var tbody = table.getElementsByTagName("tbody")[0];
            var rows = Array.from(tbody.getElementsByTagName("tr"));
            var isAscending = table.getAttribute("data-sort-order") !== "asc";
            table.setAttribute("data-sort-order", isAscending ? "asc" : "desc");
            rows.sort(function(rowA, rowB){{
                var cellA = rowA.getElementsByTagName("td")[columnId].innerText.trim();
                var cellB = rowB.getElementsByTagName("td")[columnId].innerText.trim();
                var numA = parseFloat(cellA);
                var numB = parseFloat(cellB);
                if (!isNaN(numA) && !isNaN(numB)){{
                    return isAscending ? numA - numB : numB - numA;
                }} else{{
                    return isAscending ? cellA.localeCompare(cellB) : cellB.localeCompare(cellA);
                }}
            }});
            tbody.innerHTML = "";
            rows.forEach(row => tbody.appendChild(row));
        }}
        function toggleRow(id) {{
            const row = document.getElementById(id);
            if (row.style.display === 'none') {{
                row.style.display = 'table-row';
            }} else {{
                row.style.display = 'none';
            }}
        }}
        function toggleElement(buttonId, elementSelector = null, checkboxSelector = null) {{
            const button = document.getElementById(buttonId);
            const element = elementSelector ? document.getElementById(elementSelector) : null;
            const checkboxContainer = document.getElementById(checkboxSelector);
            if (!button) return;
            button.addEventListener('click', () => {{
                if (element) {{
                    if (element.style.display === 'none' || element.style.display === '') {{
                        element.style.display = 'block';
                        element.style.width = '100%';
                        button.classList.add("button-active");
                        if (checkboxContainer) {{
                            checkboxContainer.style.display = 'block';
                        }}
                        if (window.Plotly) {{
                            window.Plotly.Plots.resize(element);
                        }}
                    }} else {{
                        element.style.display = 'none';
                        button.classList.remove("button-active");
                        if (checkboxContainer) {{
                            checkboxContainer.style.display = 'none';
                        }}
                    }}
                }} else {{
                    if (checkboxContainer) {{
                        if (checkboxContainer.style.display === 'none' || checkboxContainer.style.display === '') {{
                            checkboxContainer.style.display = 'block';
                            button.classList.add("button-active");
                        }} else {{
                            checkboxContainer.style.display = 'none';
                            button.classList.remove("button-active");
                        }}
                    }}
                }}
            }});
        }}
        function toggleElementWithCheckbox(checkboxId, elementId) {{
            const checkbox = document.getElementById(checkboxId);
            const element = document.getElementById(elementId);
            checkbox.addEventListener('change', () => {{
                if (checkbox.checked) {{
                    element.style.display = 'block';
                    element.style.width = '100%';
                    if (window.Plotly) {{
                        window.Plotly.Plots.resize(element);
                    }}
                }} else {{
                    element.style.display = 'none';
                }}
            }});
        }}
        document.addEventListener('DOMContentLoaded', function() {{
            toggleElement('show-iostats-button','iostat_zMAIN-html-element','iocheckbox-container');
            toggleElement('show-segstats-button',null,'segcheckbox-container');
            toggleElement('show-insteff-button','instance-efficiency-plot','');
            toggleElement('show-lpmore-button','highlight2-html-element','');
            toggleTable('show-latchstats-button','latchstat-table');
            const iocheckboxes = document.querySelectorAll('input[type="checkbox"][id$="-iocheckbox"]');
            iocheckboxes.forEach(checkbox => {{
                const checkboxId = checkbox.id;
                const elementId = 'iostat_' + checkboxId.replace('-iocheckbox', '-html-element');
                const element = document.getElementById(elementId);
                if (element) {{
                    toggleElementWithCheckbox(checkboxId, elementId);
                    console.log(`Set up toggle for ${{checkboxId}} -> ${{elementId}}`);
                }} else {{
                    console.warn(`Element '${{elementId}}' not found for checkbox '${{checkboxId}}'`);
                }}
            }});
            const segcheckboxes = document.querySelectorAll('input[type="checkbox"][id$="-segcheckbox"]');
            segcheckboxes.forEach(checkbox => {{
                const checkboxId = checkbox.id;
                const tableId = 'segstat-' + checkboxId.replace('-segcheckbox', '-table');
                const table = document.getElementById(tableId);
                if (table) {{
                    toggleTableWithCheckbox(checkboxId, tableId); // Use dedicated function
                    console.log(`Set up table toggle for ${{checkboxId}} -> ${{tableId}}`);
                }} else {{
                    console.warn(`Table '${{tableId}}' not found for checkbox '${{checkboxId}}'`);
                }}
            }});
        }});
        </script>"#,bckend_port
    );
    let jasmin_assistant: String = format!(
        r#"
        <div> <br> </div>
        <div id="chat-container" style="display: none;">
            <div id="messages"></div>
            <div id="input-area">
                <input type="text" id="user-input" placeholder="message to JAS-MIN..." autofocus/>
                <button id="send-btn">Send</button>
            </div>
        </div>"#
    );
    // Open plot_main HTML to inject Additional sections - Buttons, Tables, etc
    let mut plotly_html: String = fs::read_to_string(&fname)
        .expect("Failed to read jasmin-html file");
    
    const STYLE_CSS: &str = include_str!("../src/style.css");
    plotly_html = plotly_html.replace(
        "<head>",
        &format!("<head>\n<title>JAS-MIN</title>\n{}",STYLE_CSS)
    );
    let jasmin_logo = format!("<p align=\"center\" style=\"margin-bottom: 0px; margin-top: 5px;\"><a href=\"https://github.com/ora600pl/jas-min\" target=\"_blank\">
        <img src=\"https://raw.githubusercontent.com/rakustow/jas-min/main/img/jasmin_LOGO_white.png\" width=\"150\" alt=\"JAS-MIN\" onerror=\"this.style.display='none';\"/>
    </a></p>");
    // Inject Buttons and Tables into Main HTML
    plotly_html = plotly_html.replace(
        "<body>",
        &format!("<body>\n{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}",
            jasmin_logo,
            db_instance_info_html,
            "<button id=\"show-events-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Wait Events</span><span>TOP Wait Events</span></button>",
            "<button id=\"show-sqls-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Wait SQLs</span><span>TOP Wait SQLs</span></button>",
            "<button id=\"show-bgevents-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Backgrd Events</span><span>TOP Backgrd Events</span></button>",
            "<button id=\"show-anomalies-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">Anomalies Summary</span><span>Anomalies Summary</span></button>",
            format!(
                "<a href=\"{}\" target=\"_blank\" style=\"text-decoration: none;\">
                    <button id=\"show-stat_corr-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">STATS Correlation</span><span>STATS Correlation</span></button>
                </a>", "stats/statistics_corr.html"
            ),
            if !args.backend_assistant.is_empty() { 
                format!("{}","<button id=\"show-JASMINAI-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">JAS-MIN Assistant</span><span>JAS-MIN Assistant</span></button>")
            }else{
                format!("{}","<button id=\"show-JASMINAI-button\" class=\"button-JASMIN\" role=\"button\" style=\"display: none;\"><span class=\"text\">JAS-MIN Assistant</span><span>JAS-MIN Assistant</span></button>")},
            jasmin_assistant,
            event_table_html,
            bgevent_table_html,
            anomalies_summary_html,
            sqls_table_html,
            jasmin_html_scripts)
    );
    let highlight_title = "<div><h4 style=\"margin-top: 40px;margin-bottom: 0px; width: 100%; text-align: center;\">Load Profile</h4></div>\n";
    let explorer_title = "<div><h4 style=\"margin-top: 40px;margin-bottom: 0px; width: 100%; text-align: center;\">Stats Explorer</h4></div>\n";
    let insight_title = "<div><h4 style=\"margin-top: 40px;margin-bottom: 0px; width: 100%; text-align: center;\">Performance Insight</h4></div>\n";
    let explorer_button = format!(
        "\n\t{}\t{}\t{}\t{}\n\t<div id=\"iocheckbox-container\" style=\"margin-top: 10px; display: none;\">{}\n\t</div>
    <div id=\"segcheckbox-container\" style=\"margin-top: 10px; display: none;\">\n{}\n\t</div>\n",
        "<button id=\"show-iostats-button\" class=\"button-JASMIN-small\" role=\"button\"><span class=\"text\">IO Stats</span><span>IO Stats</span></button>",
        "<button id=\"show-segstats-button\" class=\"button-JASMIN-small\" role=\"button\"><span class=\"text\">SEGMENTS Stats</span><span>SEGMENTS Stats</span></button>",
        "<button id=\"show-latchstats-button\" class=\"button-JASMIN-small\" role=\"button\"><span class=\"text\">LATCH Stats</span><span>LATCH Stats</span></button>",
        "<button id=\"show-insteff-button\" class=\"button-JASMIN-small\" role=\"button\"><span class=\"text\">INSTANCE Efficiency</span><span>INSTANCE Efficiency</span></button>",
        iostats
            .keys()
            .filter(|&func| func != "zMAIN")
            .map(|func| {
                let sanitized_func = func.replace(" ", "_");
                format!(
                    "\n\t\t<input type=\"checkbox\" id=\"{}-iocheckbox\"><label for=\"{}-iocheckbox\">{}</label>", 
                    sanitized_func, sanitized_func, func
                )
            })
            .collect::<Vec<String>>()
            .join(" "),
        segstats.iter().map(|seg_stat| {
            format!(
                "\t\t<input type=\"checkbox\" id=\"{}-segcheckbox\"><label for=\"{}-segcheckbox\">by {}</label>", 
                seg_stat, seg_stat, seg_stat.replace("_"," ")
            )
        }).collect::<Vec<String>>().join("\n"),
    );
    // Inject Load Profile section into Main HTML
    plotly_html = plotly_html.replace(
        "<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">", 
        &format!("{}{}{}\n\t\t</script>\n{}\n\t\t</script>\n\t</div>\n{}\n{}<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">",
        highlight_title, 
        "<button id=\"show-lpmore-button\" class=\"button-JASMIN-small\" role=\"button\"><span class=\"text\">LP More</span><span>LP More</span></button>",
        plot_highlight.to_inline_html(Some("highlight-html-element")),
        plot_highlight2.to_inline_html(Some("highlight2-html-element")),
        explorer_title,
        explorer_button)
    );

    // Inject Explorer Sections into Main HTML
    for func in iostats.keys(){
        let func_name = func.replace(" ","_");
        let mut stats_explorer_html: String = fs::read_to_string(format!("{}/iostats/iostats_{}.html", &html_dir,func_name))
            .expect("Failed to read iostats file");
        stats_explorer_html = stats_explorer_html.replace("plotly-html-element",&format!("iostat_{}-html-element",func_name));
        fs::write(format!("{}/iostats/iostats_{}.html", &html_dir,func_name),&stats_explorer_html);
        stats_explorer_html = stats_explorer_html
                        .lines() // Iterate over lines
                        .skip_while(|line| !line.contains(&format!("<div id=\"iostat_{}-html-element\"",func_name))) // Skip lines until found
                        .take_while(|line| !line.contains("</script>")) // Keep only lines before `</script>`
                        .collect::<Vec<&str>>() // Collect remaining lines into a Vec
                        .join("\n"); // Convert back into a String
        plotly_html = plotly_html.replace(
                        "<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">", 
                        &format!("{}\n\t\t\t\t</script><div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">",
                        stats_explorer_html)
                    );
    }
    for func in segstats {
        let mut stats_explorer_html: String = fs::read_to_string(format!("{}/segstats/segstats_{}.html", &html_dir,func))
            .expect("Failed to read iostats file");
        plotly_html = plotly_html.replace(
                        "<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">", 
                        &format!("{}\n\t\t\t\t</script><div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">",
                        stats_explorer_html)
                    );
    }

    plotly_html = plotly_html.replace(
        "<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">", 
        &format!("{}\n\t\t\t\t</script><div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">",
        fs::read_to_string(format!("{}/latches/latchstats_activity.html", &html_dir)).expect("Failed to read iostats file"))
    );

    plotly_html = plotly_html.replace(
        "<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">", 
        &format!("{}\n\t\t\t\t</script><div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">",
        instance_eff_plot)
    );
    
    plotly_html = plotly_html.replace(
        "<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">", 
        &format!("\n{}<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">",
        insight_title)
    );

    // Write the updated HTML back to the file
    fs::write(&fname, plotly_html)
        .expect("Failed to write updated Plotly HTML file");
    println!("{}","\n==== DONE ===".bold().bright_cyan());
    println!("{}{}\n","JAS-MIN Report saved to: ",&fname);

    open::that(fname);
    report_for_ai
}
