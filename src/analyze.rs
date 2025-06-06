use crate::awr::{WaitEvents, HostCPU, LoadProfile, SQLCPUTime, SQLIOTime, SQLGets, SQLReads, AWR, AWRSCollection};

use plotly::color::NamedColor;
use plotly::{Plot, Histogram, BoxPlot, Scatter, HeatMap};
use plotly::common::{ColorBar, Mode, Title, Visible, Line, Orientation, Anchor, Marker, ColorScale, ColorScalePalette};
use plotly::box_plot::{BoxMean,BoxPoints};
use plotly::layout::{Axis, GridPattern, Layout, LayoutGrid, Legend, RowOrder, TraceOrder, ModeBar, HoverMode, RangeMode};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::format;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;

use colored::*;
use dotenv::dotenv;
use open::*;

use ndarray::Array2;
use ndarray_stats::CorrelationExt;
use ndarray_stats::histogram::Grid;
use regex::*;
use crate::{anomalies, Args};
use crate::reasonings::{chat_gpt, gemini};
use crate::anomalies::*;

use crate::make_notes;
use prettytable::{Table, Row, Cell, format, Attr};
use rayon::prelude::*;

struct TopStats {
    events: BTreeMap<String, u8>,
    bgevents: BTreeMap<String, u8>,
    sqls:   BTreeMap<String, String>,   // <SQL_ID, Module>
    stat_names: BTreeMap<String, u8>,
    event_anomalies_mad: HashMap<String, Vec<(String,f64)>>,
    bgevent_anomalies_mad: HashMap<String, Vec<(String,f64)>>,
    sql_elapsed_time_anomalies_mad: HashMap<String, Vec<(String,f64)>>,
}



//We don't want to plot everything, because it would cause to much trouble 
//we need to find only essential wait events and SQLIDs 
fn find_top_stats(awrs: &Vec<AWR>, db_time_cpu_ratio: f64, filter_db_time: f64, snap_range: String, logfile_name: &str, args: &Args) -> TopStats {
    let mut event_names: BTreeMap<String, u8> = BTreeMap::new();
    let mut bgevent_names: BTreeMap<String, u8> = BTreeMap::new();
    let mut sql_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut stat_names: BTreeMap<String, u8> = BTreeMap::new();
    //so we scan the AWR data
    make_notes!(&logfile_name, false, 
        "==== DBCPU/DBTime ratio analysis ====\nPeaks are being analyzed based on specified ratio (default 0.666).\nThe ratio is beaing calculated as DB CPU / DB Time.\nThe lower the ratio the more sessions are waiting for resources other than CPU.\nIf DB CPU = 2 and DB Time = 8 it means that on AVG 8 actice sessions are working but only 2 of them are actively working on CPU.\nCurrent ratio used to find peak periods is {}\n\n", db_time_cpu_ratio);
        
    
    let mut full_window_size = ((args.mad_window_size as f32 / 100.0 ) * awrs.len() as f32) as usize; // Default is 20% of probes
    if full_window_size % 2 == 1 {
        full_window_size = full_window_size + 1;
    }
    make_notes!(&logfile_name, false, 
                "==== Median Absolute Deviation ====\n\tMAD threshold = {}\n\tMAD window size={}% ({} of probes out of {})\n\n", args.mad_threshold, args.mad_window_size, full_window_size, awrs.len());
    
    for awr in awrs {
        let snap_filter: Vec<&str> = snap_range.split("-").collect::<Vec<&str>>();
        let f_begin_snap: u64 = u64::from_str(snap_filter[0]).unwrap();
        let f_end_snap: u64 = u64::from_str(snap_filter[1]).unwrap();

        if awr.snap_info.begin_snap_id >= f_begin_snap && awr.snap_info.end_snap_id <= f_end_snap {
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
                make_notes!(&logfile_name, false, "Analyzing a peak in {} ({}) for ratio: [{:.2}/{:.2}] = {:.2}\n", awr.file_name, awr.snap_info.begin_snap_time, cputime, dbtime, (cputime/dbtime));
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
            for stats in &awr.key_instance_stats {
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

    let top: TopStats = TopStats {events: event_names, 
                                  bgevents: bgevent_names, 
                                  sqls: sql_ids, 
                                  stat_names: stat_names, 
                                  event_anomalies_mad: event_anomalies,
                                  bgevent_anomalies_mad: bgevent_anomalies,
                                  sql_elapsed_time_anomalies_mad: sql_anomalies,};
    top
}

//Calculate pearson correlation of 2 vectors and return simple result
fn pearson_correlation_2v(vec1: &Vec<f64>, vec2: &Vec<f64>) -> f64 {
    let rows: usize = 2;
    let cols: usize = vec1.len();

    let mut data: Vec<f64> = Vec::new();
    data.extend(vec1);
    data.extend(vec2);
    
    let a: ndarray::ArrayBase<ndarray::OwnedRepr<f64>, ndarray::Dim<[usize; 2]>> = Array2::from_shape_vec((rows, cols), data).unwrap();
    let crr = a.pearson_correlation().unwrap();

    crr.row(0)[1]
}

fn mean(data: Vec<f64>) -> Option<f64> {
    let sum: f64 = data.iter().sum::<f64>() as f64;
    let count: usize = data.len();

    match count {
        positive if positive > 0 => Some(sum / count as f64),
        _ => None,
    }
}

fn std_deviation(data: Vec<f64>) -> Option<f64> {
    match (mean(data.clone()), data.len()) {
        (Some(data_mean), count) if count > 0 => {
            let variance: f64 = data.iter().map(|value| {
                let diff: f64 = data_mean - (*value as f64);

                diff * diff
            }).sum::<f64>() / count as f64;

            Some(variance.sqrt())
        },
        _ => None
    }
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

// Generate plots for top events
fn generate_events_plotfiles(awrs: &Vec<AWR>, top_events: &BTreeMap<String, u8>, is_fg: bool, snap_range: &str, dirpath: &str) {
    let snap_filter: Vec<&str> = snap_range.split("-").collect::<Vec<&str>>();
    let f_begin_snap: u64 = u64::from_str(snap_filter[0]).unwrap();
    let f_end_snap: u64 = u64::from_str(snap_filter[1]).unwrap();
    
    // Detect number and type of buckets
    // Ensure that there is at least one AWR with fg or bg event and they are explicitly checked
    assert!(!awrs.is_empty(),"generate_events_plotfiles: No AWR data available.");

    let first_wait_events = if is_fg {
        &awrs[0].foreground_wait_events
    } else {
        &awrs[0].background_wait_events
    };

    assert!(!first_wait_events.is_empty(),
        "generate_events_plotfiles: No {} wait events in first AWR snapshot.", if is_fg { "foreground" } else { "background" }
    );
    // Make colors consistent across buckets 
    let color_palette = vec![
        "#00E399", "#2FD900", "#E3E300", "#FFBF00", "#FF8000",
        "#FF4000", "#FF0000", "#B22222", "#8B0000", "#4B0082",
        "#8A2BE2", "#1E90FF"
    ];
    // To cover buckets dynamic across db versions - Extract bucket names from the first event's histogram_ms 
    let hist_buckets: Vec<String> = first_wait_events[0].waitevent_histogram_ms.keys().cloned().collect();
    // Assign colors from the palette to detected buckets
    let mut bucket_colors: HashMap<String, String> = HashMap::new();
    for (i, bucket) in hist_buckets.iter().enumerate() {
        let color = color_palette.get(i % color_palette.len()).unwrap();
        bucket_colors.insert(bucket.clone(), color.to_string());
    }
    // Group Events by Name and by needed data (ename(db_time, total_wait, waits,histogram values by bucket, heatmap)
    struct HeatmapEntry { //Group Events by Snap Time and histograms
        snap_time: String,
        histogram: BTreeMap<String, f32>,
    }
    struct EventStats {
        pct_dbtime: Vec<f64>,
        total_wait_time_s: Vec<f64>,
        waits: Vec<u64>,
        histogram_by_bucket: BTreeMap<String, Vec<f32>>,
        histogram_heatmap: Vec<HeatmapEntry>
    }
    let mut data_by_event: HashMap<String, EventStats> = HashMap::new();
    
    // Let's gather all needed data
    for awr in awrs {
        if awr.snap_info.begin_snap_id >= f_begin_snap && awr.snap_info.end_snap_id <= f_end_snap {
            let snap_time: String = format!("{} ({})", awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id);
        
            let events = if is_fg {
                &awr.foreground_wait_events
            } else {
                &awr.background_wait_events
            };

            for event in events {
                if !top_events.contains_key(&event.event) {
                    continue;
                }
                // Initilize events map
                let entry = data_by_event
                    .entry(event.event.clone())
                    .or_insert_with(|| EventStats {
                        pct_dbtime: Vec::new(),
                        total_wait_time_s: Vec::new(),
                        waits: Vec::new(),
                        histogram_by_bucket: BTreeMap::new(),
                        histogram_heatmap: Vec::new(),
                    });
                // Gather data by Event Name
                entry.pct_dbtime.push(event.pct_dbtime);
                entry.total_wait_time_s.push(event.total_wait_time_s);
                entry.waits.push(event.waits);
                for (bucket, value) in &event.waitevent_histogram_ms {
                    entry.histogram_by_bucket
                        .entry(bucket.clone())
                        .or_insert_with(Vec::new)
                        .push(*value);
                }
                entry.histogram_heatmap.push(HeatmapEntry {
                    snap_time: snap_time.clone(),
                    histogram: event.waitevent_histogram_ms.clone(),
                });
            }
        }
    }

    // Build plots for each event and save it as separate file
    for (event, entry) in data_by_event {
        let mut plot: Plot = Plot::new();
        let event_name = format!("{}", &event);
    
        // Add HeatMap plot (from embedded heatmap entries)
        if !entry.histogram_heatmap.is_empty() {
            let x_labels: Vec<String> = entry.histogram_heatmap.iter().map(|e| e.snap_time.clone()).collect();
    
            let mut z_matrix: Vec<Vec<f32>> = Vec::new();
            for bucket in &hist_buckets {
                let row: Vec<f32> = entry.histogram_heatmap
                    .iter()
                    .map(|h| *h.histogram.get(bucket).unwrap_or(&0.0))
                    .collect();
                z_matrix.push(row);
            }
    
            let heatmap = HeatMap::new(x_labels.clone(), hist_buckets.clone(), z_matrix)
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
            let event_total_wait = Scatter::new(x_labels.clone(), entry.total_wait_time_s.clone())
                .mode(Mode::Lines)
                .name("Total Wait Time (s)")
                .x_axis("x1")
                .y_axis("y2");

            plot.add_trace(event_total_wait);

            // Add Wait Count trace
            let event_wait_count = Scatter::new(x_labels.clone(), entry.waits.clone())
                .mode(Mode::Lines)
                .name("Wait Count")
                .x_axis("x1")
                .y_axis("y3");

            plot.add_trace(event_wait_count);
        }

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
            .title(&format!("'<b>{}</b>'", event))
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
                    .domain(&[0.65, 0.83])
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
        
        // Replace invalid characters for filenames (e.g., slashes or spaces)
        let safe_event_name: String = event.replace("/", "_").replace(" ", "_").replace(":","").replace("*","_");
        let mut file_name: String = String::new();
        if is_fg {
            file_name = format!("{}/fg_{}.html", dirpath, safe_event_name);
        } else {
            file_name = format!("{}/bg_{}.html", dirpath, safe_event_name);
        }

        // Save the plot as an HTML file
        let path: &Path = Path::new(&file_name);
        //plot.save(path).expect("Failed to save plot to file");
        plot.write_html(path);
    }
    if is_fg {
        println!("Saved plots for Foreground events to '{}/fg_*'", dirpath);
    } else {
        println!("Saved plots for Background events to '{}/bg_*'", dirpath);
    }
}

// Generate TOP SQLs html subpages with plots - To Be Developed
fn generate_sqls_plotfiles(awrs: &Vec<AWR>, top_stats: &TopStats, snap_range: &str, dirpath: &str){
    let snap_filter: Vec<&str> = snap_range.split("-").collect::<Vec<&str>>();
    let f_begin_snap: u64 = u64::from_str(snap_filter[0]).unwrap();
    let f_end_snap: u64 = u64::from_str(snap_filter[1]).unwrap();
    
    struct SQLStats{
        execs: Vec<u64>,            // Number of Executions
        ela_exec_s: Vec<f64>,       // Elapsed Time (s) per Execution
        ela_pct_total: Vec<f64>,    // Elapsed Time as a percentage of Total DB time
        pct_cpu: Vec<f64>,          // CPU Time as a percentage of Elapsed Time
        pct_io: Vec<f64>,           // User I/O Time as a percentage of Elapsed Time
        cpu_time_exec_s: Vec<f64>,  // CPU Time (s) per Execution
        cpu_t_pct_total: Vec<f64>,  // CPU Time as a percentage of Total DB CPU
        io_time_exec_s: Vec<f64>,   // User I/O Wait Time (s) per Execution
        io_pct_total: Vec<f64>,     // User I/O Time as a percentage of Total User I/O Wait time
        gets_per_exec: Vec<f64>,    // Number of Buffer Gets per Execution
        gets_pct_total: Vec<f64>,   // Buffer Gets as a percentage of Total Buffer Gets
        phy_r_exec: Vec<f64>,       // Number of Physical Reads per Execution
        phy_r_pct_total: Vec<f64>   // Physical Reads as a percentage of Total Disk Reads
    }
    let mut sqls_by_stats: HashMap<String, SQLStats> = HashMap::new();

    let x_vals: Vec<String> = awrs
        .iter()
        .filter(|awr| awr.snap_info.begin_snap_id >= f_begin_snap && awr.snap_info.end_snap_id <= f_end_snap)
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
                if awr.snap_info.begin_snap_id >= f_begin_snap && awr.snap_info.end_snap_id <= f_end_snap {
                    let mut sql_found: bool = false;
                    // Elapsed Time
                    if let Some(sql_et) = awr.sql_elapsed_time.iter().find(|e| &e.sql_id == sql_id) {
                        stats.execs.push(sql_et.executions);
                        stats.ela_exec_s.push(sql_et.elpased_time_exec_s);
                        stats.ela_pct_total.push(sql_et.pct_total);
                        stats.pct_cpu.push(sql_et.pct_cpu);
                        stats.pct_io.push(sql_et.pct_io);
                        sql_found = true;
                    } else {
                        //stats.execs.push(0);
                        stats.ela_exec_s.push(0.0);
                        stats.ela_pct_total.push(0.0);
                        //stats.pct_cpu.push(0.0);
                        //stats.pct_io.push(0.0);
                    }
    
                    // CPU Time
                    if let Some(sql_cpu) = awr.sql_cpu_time.get(sql_id) {
                        stats.cpu_time_exec_s.push(sql_cpu.cpu_time_exec_s);
                        stats.cpu_t_pct_total.push(sql_cpu.pct_total);
                        if !sql_found{
                            stats.execs.push(sql_cpu.executions);
                            stats.pct_cpu.push(sql_cpu.pct_cpu);
                            stats.pct_io.push(sql_cpu.pct_io);
                            sql_found = true;
                        }
                    } else {
                        stats.cpu_time_exec_s.push(0.0);
                        stats.cpu_t_pct_total.push(0.0);
                    }
    
                    // IO Time
                    if let Some(sql_io) = awr.sql_io_time.get(sql_id) {
                        stats.io_time_exec_s.push(sql_io.io_time_exec_s);
                        stats.io_pct_total.push(sql_io.pct_total);
                        if !sql_found{
                            stats.execs.push(sql_io.executions);
                            stats.pct_cpu.push(sql_io.pct_cpu);
                            stats.pct_io.push(sql_io.pct_io);
                            sql_found = true;
                        }
                    } else {
                        stats.io_time_exec_s.push(0.0);
                        stats.io_pct_total.push(0.0);
                    }
    
                    // Gets
                    if let Some(sql_gets) = awr.sql_gets.get(sql_id) {
                        stats.gets_per_exec.push(sql_gets.gets_per_exec);
                        stats.gets_pct_total.push(sql_gets.pct_total);
                        if !sql_found{
                            stats.execs.push(sql_gets.executions);
                            stats.pct_cpu.push(sql_gets.pct_cpu);
                            stats.pct_io.push(sql_gets.pct_io);
                            sql_found = true;
                        }
                    } else {
                        stats.gets_per_exec.push(0.0);
                        stats.gets_pct_total.push(0.0);
                    }
    
                    // Reads
                    if let Some(sql_reads) = awr.sql_reads.get(sql_id) {
                        stats.phy_r_exec.push(sql_reads.reads_per_exec);
                        stats.phy_r_pct_total.push(sql_reads.pct_total);
                        if !sql_found{
                            stats.execs.push(sql_reads.executions);
                            stats.pct_cpu.push(sql_reads.cpu_time_pct);
                            stats.pct_io.push(sql_reads.pct_io);
                            sql_found = true;
                        }
                    } else {
                        stats.phy_r_exec.push(0.0);
                        stats.phy_r_pct_total.push(0.0);
                    }
                    if !sql_found {
                        stats.execs.push(0);
                        stats.pct_cpu.push(0.0);
                        stats.pct_io.push(0.0);
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
            .x_axis("x1")
            .y_axis("y4")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_gets_per_exec);

        let sql_phy_r_exec = Scatter::new(x_vals.clone(), stats.phy_r_exec.clone())
            .mode(Mode::Markers)
            .name("# Physical Reads")
            .x_axis("x1")
            .y_axis("y4")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_phy_r_exec);

        let sql_ela_pct_total = Scatter::new(x_vals.clone(), stats.ela_pct_total.clone())
            .mode(Mode::Lines)
            .name("% Ela Time as DB Time")
            .x_axis("x1")
            .y_axis("y3");
        sql_plot.add_trace(sql_ela_pct_total);

        let sql_pct_cpu = Scatter::new(x_vals.clone(), stats.pct_cpu.clone())
            .mode(Mode::Lines)
            .name("% CPU of Ela")
            .x_axis("x1")
            .y_axis("y3");
        sql_plot.add_trace(sql_pct_cpu);

        // IO % of Ela
        let sql_pct_io = Scatter::new(x_vals.clone(), stats.pct_io.clone())
            .mode(Mode::Lines)
            .name("% IO of Ela")
            .x_axis("x1")
            .y_axis("y3");
        sql_plot.add_trace(sql_pct_io);

        // CPU Time as % of DB CPU
        let sql_cpu_t_pct_total = Scatter::new(x_vals.clone(), stats.cpu_t_pct_total.clone())
            .mode(Mode::Markers)
            .name("% CPU Time as DB CPU")
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_cpu_t_pct_total);

        // IO Time as % of Total IO Wait
        let sql_io_pct_total = Scatter::new(x_vals.clone(), stats.io_pct_total.clone())
            .mode(Mode::Markers)
            .name("% IO Time as DB IO Wait")
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_io_pct_total);

        // Buffer Gets as % of Total
        let sql_gets_pct_total = Scatter::new(x_vals.clone(), stats.gets_pct_total.clone())
            .mode(Mode::Markers)
            .name("% Gets as Total Gets")
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_gets_pct_total);

        // Physical Reads as % of Total Disk Reads
        let sql_phy_r_pct_total = Scatter::new(x_vals.clone(), stats.phy_r_pct_total.clone())
            .mode(Mode::Markers)
            .name("% Phys Reads as Total Disk Reads")
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_phy_r_pct_total);

        let sql_ela_exec_s = Scatter::new(x_vals.clone(), stats.ela_exec_s.clone())
            .mode(Mode::Lines)
            .name("(s) Elapsed Time per Exec")
            .x_axis("x1")
            .y_axis("y2");
        sql_plot.add_trace(sql_ela_exec_s);
    
        let sql_cpu_time_exec_s = Scatter::new(x_vals.clone(), stats.cpu_time_exec_s.clone())
            .mode(Mode::Markers)
            .name("(s) CPU Time")
            .x_axis("x1")
            .y_axis("y2")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_cpu_time_exec_s);
        
        let sql_io_time_exec_s = Scatter::new(x_vals.clone(), stats.io_time_exec_s.clone())
            .mode(Mode::Markers)
            .name("(s) User I/O Wait Time")
            .x_axis("x1")
            .y_axis("y2")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_io_time_exec_s);
        
        let sql_exec = Scatter::new(x_vals.clone(), stats.execs.clone())
            .mode(Mode::Lines)
            .name("# Executions")
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
            let file_name: String = format!("{}/sqlid_{}.html", dirpath, &sql);
            let path: &Path = Path::new(&file_name);
            sql_plot.write_html(path);
    }
    println!("Saved plots for SQLs to '{}/sqlid_*'", dirpath);

}



pub fn plot_to_file(collection: AWRSCollection, fname: String, args: Args) {
    let db_time_cpu_ratio: f64 = args.time_cpu_ratio;
    let filter_db_time: f64 = args.filter_db_time;
    let snap_range: String = args.snap_range.clone();
    
    let file_len = fname.len();
    let logfile_name: String = format!("{}.txt", &fname[0..file_len-5]); //cut .html from file name and add .txt
    let logfile_path = Path::new(&logfile_name);
    println!("Starting output capture to: {}", logfile_path.display() );
    if logfile_path.exists() { //remove logfile if it exists - the notes made by JAS-MIN has to be created each time
        fs::remove_file(&logfile_path).unwrap();
    }

    let mut y_vals_dbtime: Vec<f64> = Vec::new();
    let mut y_vals_dbcpu: Vec<f64> = Vec::new();
    let mut y_vals_events: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_bgevents: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_logons: Vec<f64> = Vec::new();
    let mut y_vals_calls: Vec<f64> = Vec::new();
    let mut y_vals_execs: Vec<f64> = Vec::new();
    let mut y_vals_trans: Vec<f64> = Vec::new();
    let mut y_vals_redosize: Vec<f64> = Vec::new();
    let mut y_vals_parses: Vec<f64> = Vec::new();
    let mut y_vals_hparses: Vec<f64> = Vec::new();
    let mut y_vals_cpu_user: Vec<f64> = Vec::new();
    let mut y_vals_cpu_load: Vec<f64> = Vec::new();
    let mut y_vals_redo_switches: Vec<f64> = Vec::new();
    let mut y_excessive_commits: Vec<f64> = Vec::new();
    let mut y_cleanout_ktugct: Vec<f64> = Vec::new();
    let mut y_cleanout_cr: Vec<f64> = Vec::new();
    let mut y_read_mb: Vec<f64> = Vec::new();
    let mut y_write_mb: Vec<f64> = Vec::new();

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
    /********************************************/

    /*HashMap for calculating instance stats correlation*/
    let mut instance_stats: HashMap<String, Vec<f64>> = HashMap::new();
    /****************************************************/

    let mut x_vals: Vec<String> = Vec::new();
    
    // === ANALYZING ===
    println!("{}","\n==== ANALYZING ===".bright_cyan());
    let top_stats: TopStats = find_top_stats(&collection.awrs, db_time_cpu_ratio, filter_db_time, snap_range.clone(), &logfile_name, &args);
    
    let mut is_logfilesync_high: bool = false;
    
    // Extract the parent directory and generate FG Events html plots

    //This will be empty if -d or -j specified without whole path
    let dir_path: &str = Path::new(&fname).parent().unwrap_or(Path::new("")).to_str().unwrap_or("");
    let mut html_dir: String = String::new(); //variable for html files directory
    let mut stripped_fname: &str = fname.as_str(); //without whole path to a file this will be just a file name
    if dir_path.len() == 0 {
        html_dir = format!("{}_reports", &fname); 
    } else { //if the whole path was specified we are extracting file name and seting html_dir properly 
        stripped_fname = Path::new(&fname).file_name().unwrap().to_str().unwrap();
        html_dir = format!("{}/{}_reports", &dir_path, &stripped_fname);
    }
    
    println!("{}","\n==== CREATING PLOTS ===".bright_cyan());
    fs::create_dir(&html_dir).unwrap_or_default();
    generate_events_plotfiles(&collection.awrs, &top_stats.events, true, &snap_range, &html_dir);
    generate_events_plotfiles(&collection.awrs, &top_stats.bgevents, false, &snap_range, &html_dir);
    generate_sqls_plotfiles(&collection.awrs, &top_stats, &snap_range, &html_dir);
    let fname: String = format!("{}/jasmin_{}", &html_dir, &stripped_fname); //new file name path for main report

    /* ------ Preparing data ------ */
    println!("\n{}","==== PREPARING RESULTS ===".bright_cyan());
    for awr in &collection.awrs {
        let snap_filter: Vec<&str> = snap_range.split("-").collect::<Vec<&str>>();
        let f_begin_snap: u64 = u64::from_str(snap_filter[0]).unwrap();
        let f_end_snap: u64 = u64::from_str(snap_filter[1]).unwrap();

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
                    } else if lp.stat_name.starts_with("User logons") || (is_statspack && lp.stat_name.starts_with("Logons")) {
                        y_vals_logons.push(lp.per_second*60.0*60.0);
                    } else if lp.stat_name.starts_with("Executes") {
                        y_vals_execs.push(lp.per_second);
                    } else if lp.stat_name.starts_with("Transactions") {
                            y_vals_trans.push(lp.per_second);
                    } else if lp.stat_name.starts_with("Redo size") {
                        y_vals_redosize.push(lp.per_second as f64/1024.0/1024.0);
                    } else if lp.stat_name.starts_with("Parses") {
                        y_vals_parses.push(lp.per_second);
                    } else if lp.stat_name.starts_with("Hard parses") {
                        y_vals_hparses.push(lp.per_second);
                    } else if lp.stat_name.starts_with("Physical read") {
                        y_read_mb.push(lp.per_second*collection.db_instance_information.db_block_size as f64/1024.0/1024.0);
                    } else if lp.stat_name.starts_with("Physical write") {
                        y_write_mb.push(lp.per_second*collection.db_instance_information.db_block_size as f64/1024.0/1024.0);
                    }
            }
                // ----- Host CPU
                if awr.host_cpu.pct_user < 0.0 {
                    y_vals_cpu_user.push(0.0);
                } else {
                    y_vals_cpu_user.push(awr.host_cpu.pct_user);
                }
                y_vals_cpu_load.push(100.0-awr.host_cpu.pct_idle);

                // ----- Additionally plot Redo Log Switches
                y_vals_redo_switches.push(awr.redo_log.per_hour);

                let mut calls: u64 = 0;
                let mut commits: u64 = 0;
                let mut rollbacks: u64 = 0;
                let mut cleanout_ktugct: u64 = 0;
                let mut cleanout_cr: u64 = 0;
                let mut excessive_commit: f64 = 0.0;

                for activity in &awr.key_instance_stats {
                    let mut v: &mut Vec<f64> = instance_stats.get_mut(&activity.statname).unwrap();
                    v[x_vals.len()-1] = activity.total as f64;

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

    //make_notes!(&logfile_name, false, "{}\n","Load Profile and Top Stats");
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

    // ------ Ploting and reporting starts ----------
    make_notes!(&logfile_name, args.quiet, "\n{}\n\n","==== Statistical Computation RESULTS ===".bright_cyan());

    let mut plot_main: Plot = Plot::new();
    let mut plot_highlight: Plot = Plot::new();

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
    let calls_trace = Scatter::new(x_vals.clone(), y_vals_calls)
                                                    .mode(Mode::LinesText)
                                                    .name("User Calls")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let logons_trace = Scatter::new(x_vals.clone(), y_vals_logons)
                                                    .mode(Mode::LinesText)
                                                    .name("Logons")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let exec_trace = Scatter::new(x_vals.clone(), y_vals_execs.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("Executes")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let parses_trace = Scatter::new(x_vals.clone(), y_vals_parses)
                                                    .mode(Mode::LinesText)
                                                    .name("Parses")
                                                    .x_axis("x1")
                                                    .y_axis("y2");
    let hparses_trace = Scatter::new(x_vals.clone(), y_vals_hparses)
                                                    .mode(Mode::LinesText)
                                                    .name("Hard Parses")
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
    let cpu_load_box_plot  = BoxPlot::new(y_vals_cpu_load)
                                                    //.mode(Mode::LinesText)
                                                    .name("CPU Load")
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
                                                    .name("Read MB/s")
                                                    .x_axis("x5")
                                                    .y_axis("y5")
                                                    .box_mean(BoxMean::True)
                                                    .show_legend(false)
                                                    .box_points(BoxPoints::All)
                                                    .whisker_width(0.2)
                                                    .marker(Marker::new().color("#c2be4f".to_string()).opacity(0.7).size(2));
    let write_mb_box_plot  = BoxPlot::new(y_write_mb)
                                                    //.mode(Mode::LinesText)
                                                    .name("Write MB/s")
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
        plot_main.add_trace(redo_switches);
        plot_main.add_trace(excessive_commits);
        plot_main.add_trace(cleanout_cr_only);
        plot_main.add_trace(cleanout_ktugct_calls);
    }
    
    plot_main.add_trace(dbtime_trace);
    plot_highlight.add_trace(aas_box_plot);
    plot_main.add_trace(dbcpu_trace);
    plot_main.add_trace(calls_trace);
    plot_main.add_trace(logons_trace);
    plot_main.add_trace(exec_trace);
    plot_highlight.add_trace(exec_box_plot);
    plot_highlight.add_trace(trans_box_plot);
    plot_main.add_trace(parses_trace);
    plot_main.add_trace(hparses_trace);
    plot_main.add_trace(cpu_user);
    plot_main.add_trace(cpu_load);
    plot_highlight.add_trace(cpu_load_box_plot);
    plot_highlight.add_trace(read_mb_box_plot);
    plot_highlight.add_trace(write_mb_box_plot);
    plot_highlight.add_trace(redo_mb_box_plot);

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
    make_notes!(&logfile_name, false, "{}\n","Foreground Wait Events");
    for (key, yv) in &y_vals_events_sorted {
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
        make_notes!(&logfile_name, args.quiet, "\t{: >5}\n", &event_name);

        let correlation_info: String = format!("--- Correlation with DB Time: {:.2}", &corr);
        if corr >= 0.4 || corr <= -0.4 { 
            make_notes!(&logfile_name, args.quiet, "{: >50}", correlation_info.red().bold());
        } else {
            make_notes!(&logfile_name, args.quiet, "{: >50}", correlation_info);
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
        
        make_notes!(&logfile_name, args.quiet, "\t\tMarked as TOP in {:.2}% of probes\n",  (x_n.len() as f64 / x_vals.len() as f64 )* 100.0);
        make_notes!(&logfile_name, args.quiet, "\t\t--- AVG PCT of DB Time: {:>15.2}% \tSTDDEV PCT of DB Time: {:>15.2}%\n", &avg_exec_t, &stddev_exec_t);
        make_notes!(&logfile_name, args.quiet, "\t\t--- AVG Wait Time (s): {:>16.2} \tSTDDEV Wait Time (s): {:>16.2}\n", &avg_exec_s, &stddev_exec_s);
        make_notes!(&logfile_name, args.quiet, "\t\t--- AVG No. executions: {:>15.2} \tSTDDEV No. executions: {:>15.2}\n", &avg_exec_n, &stddev_exec_n);
        make_notes!(&logfile_name, args.quiet, "\t\t--- AVG wait/exec (ms): {:>15.2} \tSTDDEV wait/exec (ms): {:>15.2}\n\n", &avg_wait_per_exec_ms, &stddev_wait_per_exec_ms);
        
        /* Print table of detected anomalies for given event_name (key.1)*/
        let safe_event_name: String = event_name.replace("/", "_").replace(" ", "_").replace(":","").replace("*","_");
        let anomaly_id = format!("mad_fg_{}", &safe_event_name);
        let mut anomalies_flag: bool = false;

        if let Some(anomalies) = top_stats.event_anomalies_mad.get(&key.1) {
            let anomalies_detection_msg = "Detected anomalies using Median Absolute Deviation on the following dates:".to_string().bold().underline().blue();
            anomalies_flag = true;
            make_notes!(&logfile_name, args.quiet, "\t\t{}\n",  anomalies_detection_msg);
            
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
                make_notes!(&logfile_name, args.quiet, "\t\t{}\n", table_line);
            }
        } else {
            let no_anomalies_txt = format!("\t\tNo anomalies detected based on MAD threshold: {}\n", args.mad_threshold);
            anomalies_flag = false;
            make_notes!(&logfile_name, args.quiet, "{}", no_anomalies_txt.green().italic());
        }
        make_notes!(&logfile_name, args.quiet,"\n");
        
        /* FGEVENTS - Generate a row for the Main HTML table */
        table_events.push_str(&format!(
            r#"
            <tr>
                <td><a href="fg_{}.html" target="_blank" class="nav-link" style="font-weight: bold">{}</a></td>
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
    make_notes!(&logfile_name, false, "{}\n","Background Wait Events");
    for (key, yv) in &y_vals_bgevents_sorted {
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
        make_notes!(&logfile_name, args.quiet, "\t{: >5}\n", &event_name);

        let correlation_info: String = format!("--- Correlation with DB Time: {:.2}", &corr);
        if corr >= 0.4 || corr <= -0.4 { 
            make_notes!(&logfile_name, args.quiet, "{: >50}", correlation_info.red().bold());
        } else {
            make_notes!(&logfile_name, args.quiet, "{: >50}", correlation_info);
        }
        make_notes!(&logfile_name, args.quiet, "\t\tMarked as TOP in {:.2}% of probes\n",  (x_n.len() as f64 / x_vals.len() as f64 )* 100.0);
        make_notes!(&logfile_name, args.quiet, "\t\t--- AVG PCT of DB Time: {:>15.2}% \tSTDDEV PCT of DB Time: {:>15.2}%\n", &avg_exec_t, &stddev_exec_t);
        make_notes!(&logfile_name, args.quiet, "\t\t--- AVG Wait Time (s): {:>16.2} \tSTDDEV Wait Time (s): {:>16.2}\n", &avg_exec_s, &stddev_exec_s);
        make_notes!(&logfile_name, args.quiet, "\t\t--- AVG No. executions: {:>15.2} \tSTDDEV No. executions: {:>15.2}\n", &avg_exec_n, &stddev_exec_n);
        make_notes!(&logfile_name, args.quiet, "\t\t--- AVG wait/exec (ms): {:>15.2} \tSTDDEV wait/exec (ms): {:>15.2}\n\n", &avg_wait_per_exec_ms, &stddev_wait_per_exec_ms);
         /* Print table of detected anomalies for given event_name (key.1)*/
        let safe_event_name: String = event_name.replace("/", "_").replace(" ", "_").replace(":","").replace("*","_");
        let anomaly_id = format!("mad_bg_{}", &safe_event_name);
        let mut anomalies_flag: bool = false;

        if let Some(anomalies) = top_stats.bgevent_anomalies_mad.get(&key.1) {
            let anomalies_detection_msg = "Detected anomalies using Median Absolute Deviation on the following dates:".to_string().bold().underline().blue();
            anomalies_flag = true;
            make_notes!(&logfile_name, args.quiet, "\t\t{}\n",  anomalies_detection_msg);

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
                make_notes!(&logfile_name, args.quiet, "\t\t{}\n", table_line);
            }
        } else {
            let no_anomalies_txt = format!("\t\tNo anomalies detected based on MAD threshold: {}\n", args.mad_threshold);
            anomalies_flag = false;
            make_notes!(&logfile_name, args.quiet, "{}", no_anomalies_txt.green().italic());
        }
        make_notes!(&logfile_name, args.quiet,"\n");
        
        /* BGEVENTS - Generate a row for the HTML table */
        table_bgevents.push_str(&format!(
            r#"
            <tr>
                <td><a href="bg_{}.html" target="_blank" class="nav-link" style="font-weight: bold">{}</a></td>
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

    //println!("{}","SQLs");
    make_notes!(&logfile_name, false, "TOP SQLs by Elapsed time (SQL_ID or OLD_HASH_VALUE presented)");

    for (key,yv) in y_vals_sqls_sorted {
        let sql_trace = Scatter::new(x_vals.clone(), yv.clone())
                                                        .mode(Mode::LinesText)
                                                        .name(key.1.clone())
                                                        .x_axis("x1")
                                                        .y_axis("y5").visible(Visible::LegendOnly);
        plot_main.add_trace(sql_trace);
        
        let sql_id: String = key.1.clone();
        /* Correlation calc */
        let corr: f64 = pearson_correlation_2v(&y_vals_dbtime, &yv);
        // Print Correlation considered high enough to mark it
        let top_sections: HashMap<String, f64> = report_top_sql_sections(&sql_id, &collection.awrs);
        make_notes!(&logfile_name, args.quiet,
            "\n\t{: >5} \t {}\n",
            &sql_id.bold(),
            format!("Other Top Sections: {}",
                &top_sections.iter().map(|(key, value)| format!("{} [{:.2}%]", key, value)).collect::<Vec<String>>().join(" | ")
            ).italic(),
        );

        let correlation_info: String = format!("--- Correlation with DB Time: {:.2}", &corr);
        if corr >= 0.4 || corr <= -0.4 { 
            //print!("{: >49}", correlation_info.red().bold());
            make_notes!(&logfile_name, args.quiet, "{: >49}", correlation_info.red().bold());
        } else {
            //print!("{: >49}", correlation_info);
            make_notes!(&logfile_name, args.quiet, "{: >49}", correlation_info);
        }

        /* Calculate STDDEV and AVG for sqls executions number */
        let x: Vec<f64> = y_vals_sqls_exec_n.get(&key.1.clone()).unwrap().clone();
        let avg_exec_n: f64 = mean(x.clone()).unwrap();
        let stddev_exec_n: f64 = std_deviation(x).unwrap();

        /* Calculate STDDEV and AVG for sqls time per execution */
        let x: Vec<f64> = y_vals_sqls_exec_t.get(&key.1.clone()).unwrap().clone();
        let avg_exec_t: f64 = mean(x.clone()).unwrap();
        let stddev_exec_t: f64 = std_deviation(x.clone()).unwrap();

        /* Calculate STDDEV and AVG for sqls time */
        let x_s: Vec<f64> = y_vals_sqls_exec_s.get(&key.1.clone()).unwrap().clone();
        let avg_exec_s: f64 = mean(x_s.clone()).unwrap();
        let stddev_exec_s: f64 = std_deviation(x_s).unwrap();

        make_notes!(&logfile_name, args.quiet, "{: >24}{:.2}% of probes\n", "Marked as TOP in ", (x.len() as f64 / x_vals.len() as f64 )* 100.0);
        make_notes!(&logfile_name, args.quiet, "{: >35} {: <16.2} \tSTDDEV Ela by Exec: {:.2}\n", "--- AVG Ela by Exec:", avg_exec_t, stddev_exec_t);
        make_notes!(&logfile_name, args.quiet, "{: >36} {: <16.2} \tSTDDEV Ela Time   : {:.2}\n", "--- AVG Ela Time (s):", avg_exec_s, stddev_exec_s);
        make_notes!(&logfile_name, args.quiet, "{: >38} {: <14.2} \tSTDDEV No. executions:  {:.2}\n", "--- AVG No. executions:", avg_exec_n, stddev_exec_n);
        make_notes!(&logfile_name, args.quiet, "{: >23} {} \n", "MODULE: ", top_stats.sqls.get(&sql_id).unwrap().blue());

        
        /* Print table of detected anomalies for given SQL_ID (key.1)*/
        let anomaly_id = format!("mad_{}", &sql_id);
        let mut anomalies_flag: bool = false;

        if let Some(anomalies) = top_stats.sql_elapsed_time_anomalies_mad.get(&key.1) {
            let anomalies_detection_msg = "Detected anomalies using Median Absolute Deviation on the following dates:".to_string().bold().underline().blue();
            anomalies_flag = true;
            make_notes!(&logfile_name, args.quiet, "\t\t{}\n",  anomalies_detection_msg);

            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("Date"),
                Cell::new("MAD Score"),
                Cell::new("Elapsed Time (s)"),
                Cell::new("Executions"),
                Cell::new("Ela time / exec (s)")
            ]));

            for (i,a) in anomalies.iter().enumerate() {
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
                make_notes!(&logfile_name, args.quiet, "\t\t{}\n", table_line);
            }
        } else {
            let no_anomalies_txt = format!("\t\tNo anomalies detected based on MAD threshold: {}\n", args.mad_threshold);
            anomalies_flag = false;
            make_notes!(&logfile_name, args.quiet, "{}", no_anomalies_txt.green().italic());
        }
        make_notes!(&logfile_name, args.quiet,"\n");
        
        /* SQLs - Generate a row for the HTML table */
        table_sqls.push_str(&format!(
            r#"
            <tr>
                <td><a href="sqlid_{}.html" target="_blank" class="nav-link" style="font-weight: bold">{}</a></td>
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
                avg_exec_t, stddev_exec_t,  // PCT of DB Time
                avg_exec_n, stddev_exec_n,  // Execution times
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
        let sql_corr_txt_header = "\t\t\tCorrelations of SQL with wait events\n".to_string();
        make_notes!(&logfile_name, args.quiet, "{}", sql_corr_txt_header.bold().blue());
        for (key,ev) in &y_vals_events_sorted {
            let crr = pearson_correlation_2v(&yv, &ev);
            let corr_text = format!("{: >32} | {: <32} : {:.2}", "+".to_string(), key.1.clone(), crr);
            if crr >= 0.4 || crr <= - 0.4 { //Correlation considered high enough to mark it
                sql_corr_txt.push(format!(r#"<span style="color:red; font-weight:bold;">{}</span>"#, corr_text));
                make_notes!(&logfile_name, args.quiet, "{}\n", corr_text.red().bold());
            } else {
                sql_corr_txt.push(corr_text.clone());
                make_notes!(&logfile_name, args.quiet, "{}\n", corr_text);
            }
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
                    <p><span style="font-size:20px;font-weight:bold;width:100%;text-align:center;">{sql_id}</span></p>
                    <p><span style="color:blue;font-weight:bold;">Module:<br></span>{module}</p>
                    <p><span style="color:blue;font-weight:bold;">Other Top Sections:<br></span> {top_section}</p>
                    <p><span style="color:blue;font-weight:bold;">Correlations:<br></span>{sql_corr_txt}</p>
                </div>
            </body>
            </html>
            "#,
            sql_id = sql_id,
            module=top_stats.sqls.get(&sql_id).unwrap(),
            top_section=top_sections.iter().map(|(key, value)| format!("<span class=\"bold\">{}:</span> {:.2}%", key, value)).collect::<Vec<String>>().join("<br>"),
            sql_corr_txt = sql_corr_txt.join("<br>")
        );

        // Insert this into already existing sqlid_*.html file
        let filename: String = format!("{}/sqlid_{}.html", &html_dir,sql_id);
        let mut sql_file: String = fs::read_to_string(&filename)
            .expect(&format!("Failed to read file: {}", filename));
        sql_file = sql_file.replace(
            "<body>",
            &format!("<body>\n{}\n",sqlid_html_content)
        );
        if let Err(e) = fs::write(&filename, sql_file) {
            eprintln!("Error writing file {}: {}", filename, e);
        }
    }

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
                    <th onclick="sortTable('sqls-table',5)" style="cursor: pointer;">Correlation of DBTime</th>
                    <th onclick="sortTable('sqls-table',6)" style="cursor: pointer;">TOP in % Probes</th>
                    <th onclick="sortTable('sqls-table',7)" style="cursor: pointer;">Anomalies</th>
                </tr>
            </thead>
            <tbody>
            {}
            </tbody>
        </table>
        "#,
        table_sqls
    );


    /* Load Profile Anomalies detection and report */
    make_notes!(&logfile_name, args.quiet, "\nLoad Profile Anomalies detection using Median Absolute Deviation threshold: {}\n", args.mad_threshold);
    make_notes!(&logfile_name, args.quiet, "-----------------------------------------------------------------------------------\n");
    
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
            
            let anomalies_str = format!("\tAnomalies detected for \"{}\", AVG value per second is: {:.2}", stat_name, mean_per_s).red();
            make_notes!(&logfile_name, args.quiet, "\n\n{}\n", anomalies_str);
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
                make_notes!(&logfile_name, args.quiet, "\t\t{}\n", table_line);
            }
        } else {
            let no_anomalies_str = format!("\tNo anomalies detected for \"{}\", AVG value per second iss: {:.2}", stat_name, mean_per_s).green();
            make_notes!(&logfile_name, args.quiet, "\n\n{}\n", no_anomalies_str);
        }
    }
    /***********************************************/


    // STATISTICS Correltation to DBTime

    println!("\n{}","Statistics");
    make_notes!(&logfile_name, args.quiet, "\n\n");
    make_notes!(&logfile_name, args.quiet, "-----------------------------------------------------------------------------------\n");
    make_notes!(&logfile_name, args.quiet, "Correlation of instance statatistics with DB Time for values >= 0.5 and <= -0.5\n\n\n");
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
                <p><span>JAS-MIN<br></span><span style="font-size:20px;font-weight:bold;">Correlation of Instance Statistics with DB Time for Values >= 0.5 and <= -0.5</span></p>
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
    let stats_corr_filename: String = format!("{}/statistics_corr.html", &html_dir);
    if let Err(e) = fs::write(&stats_corr_filename, table_stat_corr) {
        eprintln!("Error writing file {}: {}", stats_corr_filename, e);
    }
    for (k,v) in sorted_correlation {
        make_notes!(&logfile_name, args.quiet, "\t{: >64} : {:.2}\n", &k.1, v);
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

    /****************   Report anomalies summary ************/
    println!("{}","Anomalies Summary");
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
        report_anomalies_summary(&mut anomalies_summary, &args, &logfile_name)
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
            .title("CPU Util (%)")
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
        .height(450)
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

    println!("{}","Generating Plots");
    plot_main.set_layout(layout_main);
    plot_highlight.set_layout(layout_highlight);
    // plot_main.use_local_plotly();
    // plot_highlight.use_local_plotly();
    plot_main.write_html(fname.clone());
    plot_highlight.write_html(format!("{}/jasmin_highlight.html", &html_dir));
    
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
    
    dotenv().ok();
    let mut bckend_port: String = String::new();
    if args.backend_assistant {
        bckend_port = std::env::var("PORT").expect("You have to set backend PORT value in .env");
    }
    
    let jasmin_html_scripts: String = format!(
        r#"
        <script src="https://cdn.jsdelivr.net/npm/marked/marked.min.js"></script>
        <script>
        const messages = document.getElementById('messages');
        const input = document.getElementById('user-input');
        const sendBtn = document.getElementById('send-btn');
        messages.innerHTML += `<div class="message ai-msg" style="color: grey;">Example of questions to JAS-MIN Assistant:<br>Summarise Rreport<br>What is the most important Wait Event</div>`;
        async function sendMessage() {{
            const userMsg = input.value.trim();
            if (userMsg === '') return;
            messages.innerHTML += `<div class="message user-msg">${{userMsg}}</div>`;
            messages.scrollTop = messages.scrollHeight;
            input.value = '';
            try {{
                const response = await fetch('http://localhost:{}/api/chat', {{
                    method: 'POST',
                    headers: {{ 'Content-Type': 'application/json' }},
                    body: JSON.stringify({{ message: userMsg }})
                }});
                const data = await response.json();
                messages.innerHTML += `<div class="message ai-msg">${{marked.parse(data.reply)}}</div>`;
                messages.scrollTop = messages.scrollHeight;
            }} catch (error) {{
                messages.innerHTML += `<div class="message ai-msg">Error retrieving response.</div>`;
                messages.scrollTop = messages.scrollHeight;
            }}
        }}
        input.addEventListener('keydown', function(event) {{
            if (event.key === 'Enter') {{
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
        .expect("Failed to read Main JAS-MIN HTML file");
    let mut highlight_html: String = fs::read_to_string(format!("{}/jasmin_highlight.html", &html_dir))
        .expect("Failed to read HighLight JAS-MIN HTML file");
    
    //Prepare HighLight Section to be pasted into Main HTML file
    highlight_html = highlight_html.replace("plotly-html-element","highlight-html-element");
    fs::write(format!("{}/jasmin_highlight.html", &html_dir),&highlight_html);
    highlight_html = highlight_html
                    .lines() // Iterate over lines
                    .skip_while(|line| !line.contains("<div id=\"highlight-html-element\"")) // Skip lines until found
                    .take_while(|line| !line.contains("</script>")) // Keep only lines before `</script>`
                    .collect::<Vec<&str>>() // Collect remaining lines into a Vec
                    .join("\n"); // Convert back into a String
    // Inject CSS styles into Main HTML
    const STYLE_CSS: &str = include_str!("../src/style.css");
    plotly_html = plotly_html.replace(
        "<head>",
        &format!("<head>\n<title>JAS-MIN</title>\n{}",STYLE_CSS)
    );
    // Inject Buttons and Tables into Main HTML
    plotly_html = plotly_html.replace(
        "<body>",
        &format!("<body>\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}",
            db_instance_info_html,
            "<button id=\"show-events-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Wait Events</span><span>TOP Wait Events</span></button>",
            "<button id=\"show-sqls-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Wait SQLs</span><span>TOP Wait SQLs</span></button>",
            "<button id=\"show-bgevents-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Backgrd Events</span><span>TOP Backgrd Events</span></button>",
            "<button id=\"show-anomalies-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">Anomalies Summary</span><span>Anomalies Summary</span></button>",
            format!(
                "<a href=\"{}\" target=\"_blank\" style=\"text-decoration: none;\">
                    <button id=\"show-stat_corr-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">STATS Correlation</span><span>STATS Correlation</span></button>
                </a>", "statistics_corr.html"
            ),
            "<button id=\"show-JASMINAI-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">JAS-MIN Assistant</span><span>JAS-MIN Assistant</span></button>",
            jasmin_assistant,
            event_table_html,
            bgevent_table_html,
            anomalies_summary_html,
            sqls_table_html,
            jasmin_html_scripts)
    );
    // Inject HighLight section into Main HTML
    plotly_html = plotly_html.replace(
        "<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">", 
        &format!("{}\n\t\t\t\t</script>\n<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">",
                highlight_html)
    );
    // Write the updated HTML back to the file
    fs::write(&fname, plotly_html)
        .expect("Failed to write updated Plotly HTML file");
    println!("{}","\n==== DONE ===".bright_cyan());
    println!("{}{}\n","JAS-MIN Report saved to: ",&fname);

    open::that(fname);
}
