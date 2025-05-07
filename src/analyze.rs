use crate::awr::{WaitEvents, HostCPU, LoadProfile, SQLCPUTime, SQLIOTime, SQLGets, SQLReads, AWR, AWRSCollection};

use plotly::color::NamedColor;
use plotly::{Plot, Histogram, BoxPlot, Scatter};
use plotly::common::{ColorBar, Mode, Title, Visible, Line, Orientation, Anchor, Marker};
use plotly::box_plot::{BoxMean,BoxPoints};
use plotly::layout::{Axis, GridPattern, Layout, LayoutGrid, Legend, RowOrder, TraceOrder, ModeBar, HoverMode, RangeMode};

use std::collections::{BTreeMap, HashMap};
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


struct TopStats {
    events: BTreeMap<String, u8>,
    bgevents: BTreeMap<String, u8>,
    sqls:   BTreeMap<String, String>,
    stat_names: BTreeMap<String, u8>,
    event_anomalies_mad: HashMap<String, Vec<(String,f64)>>,
    bgevent_anomalies_mad: HashMap<String, Vec<(String,f64)>>,
}



//We don't want to plot everything, because it would cause to much trouble 
//we need to find only essential wait events and SQLIDs 
fn find_top_stats(awrs: Vec<AWR>, db_time_cpu_ratio: f64, filter_db_time: f64, snap_range: String, logfile_name: &str, args: &Args) -> TopStats {
    let mut event_names: BTreeMap<String, u8> = BTreeMap::new();
    let mut bgevent_names: BTreeMap<String, u8> = BTreeMap::new();
    let mut sql_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut stat_names: BTreeMap<String, u8> = BTreeMap::new();
    //so we scan the AWR data
    make_notes!(&logfile_name, false, 
        "==== DBCPU/DBTime ratio analysis ====\nPeaks are being analyzed based on specified ratio (default 0.666).\nThe ratio is beaing calculated as DB CPU / DB Time.\nThe lower the ratio the more sessions are waiting for resources other than CPU.\nIf DB CPU = 2 and DB Time = 8 it means that on AVG 8 actice sessions are working but only 2 of them are actively working on CPU.\nCurrent ratio used to find peak periods is {}\n\n", db_time_cpu_ratio);
        
    for awr in &awrs {
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
                    for i in 1..10 {
                        event_names.entry(events[fg_length-i].event.clone()).or_insert(1);
                    }
                }
                if bg_length > 10 {
                    for i in 1..10 {
                        bgevent_names.entry(bgevents[bg_length-i].event.clone()).or_insert(1);
                    }
                }
                //And the same with SQLs
                let mut sqls: Vec<crate::awr::SQLElapsedTime> = awr.sql_elapsed_time.clone();
                sqls.sort_by_key(|s| s.elapsed_time_s as i64);
                let l: usize = sqls.len();  
                if l > 5 {
                    for i in 1..5 {
                        sql_ids.entry(sqls[l-i].sql_id.clone()).or_insert(sqls[l-i].sql_module.clone());
                    }
                } else if l >= 1 && l <=5 {
                    for i in 0..l {
                        sql_ids.entry(sqls[i].sql_id.clone()).or_insert(sqls[i].sql_module.clone());
                    }
                }
            }
            for stats in &awr.key_instance_stats {
                stat_names.entry(stats.statname.clone()).or_insert(1);
            }
        }
    }
    
    let event_anomalies = detect_event_anomalies_mad(&awrs, &args, "FOREGROUND");
    for a in &event_anomalies {
        event_names.entry(a.0.to_string()).or_insert(1);
    }
    let bgevent_anomalies = detect_event_anomalies_mad(&awrs, &args, "BACKGROUND");
    for a in &bgevent_anomalies {
        bgevent_names.entry(a.0.to_string()).or_insert(1);
    }

    let top: TopStats = TopStats {events: event_names, 
                                  bgevents: bgevent_names, 
                                  sqls: sql_ids, 
                                  stat_names: stat_names, 
                                  event_anomalies_mad: event_anomalies,
                                  bgevent_anomalies_mad: bgevent_anomalies,};
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

// Filter and generate histogram for top events
fn generate_events_plotfiles(awrs: &Vec<AWR>, top_events: &BTreeMap<String, u8>, is_fg: bool, dirpath: &str) {
    
    //Make colors consistent across buckets 
    let bucket_colors: HashMap<String, String> = HashMap::from([
        ("0: <512us".to_string(), "#00E399".to_string()),  // Ocean Green
        ("1: <1ms".to_string(), "#2FD900".to_string()),    // Green
        ("2: <2ms".to_string(), "#E3E300".to_string()),    // Yellow
        ("3: <4ms".to_string(), "#FFBF00".to_string()),    // Amber
        ("4: <8ms".to_string(), "#FF8000".to_string()),    // Orange
        ("5: <16ms".to_string(), "#FF4000".to_string()),   // Tomato
        ("6: <32ms".to_string(), "#FF0000".to_string()),   // Red
        ("7: <=1s".to_string(), "#B22222".to_string()),    // Red
        ("7: >=32ms".to_string(), "#B22222".to_string()),  // Firebrick
        ("8: >1s".to_string(), "#8B0000".to_string())      // Dark Red
    ]);
    
    let mut filtered_events: Vec<&WaitEvents> = Vec::<&WaitEvents>::new();
    if is_fg {
        filtered_events = awrs // Filter Foreground WaitEvents based on top_events
            .iter()
            .flat_map(|awr| &awr.foreground_wait_events)
            .filter(|event| top_events.contains_key(&event.event))
            .collect();
    } else {
        filtered_events = awrs // Filter Background WaitEvents based on top_events
            .iter()
            .flat_map(|awr| &awr.background_wait_events)
            .filter(|event| top_events.contains_key(&event.event))
            .collect();
    }

    // Group data by event
    let mut data_by_event: HashMap<String, (Vec<f64>, BTreeMap<String, Vec<f32>>)> = HashMap::new();
    for event in filtered_events {
        let entry = data_by_event.entry(event.event.clone()).or_insert_with(|| (Vec::new(), BTreeMap::new()));
        entry.0.push(event.pct_dbtime); // Add pct_dbtime
        for (bucket, value) in &event.waitevent_histogram_ms { // Add waitevent_histogram_ms
            entry.1.entry(bucket.clone()).or_insert_with(Vec::new).push(*value);
        }
    }

    // Create the histogram for each event and save it as separate file
    for (event, (pct_dbtime_values, histogram_data)) in data_by_event {
        let mut plot: Plot = Plot::new();
        let event_name: String = format!("{}",&event);
        //Add Histogram for DBTime
        let dbt_histogram = Histogram::new(pct_dbtime_values.clone())
           .name(&event_name)
           .legend_group(&event_name)
            //.n_bins_x(100) // Number of bins
            .x_axis("x1")
            .y_axis("y1")
            .show_legend(true);
        plot.add_trace(dbt_histogram);

        // Add Box Plot for DBTime
        let dbt_box_plot = BoxPlot::new_xy(pct_dbtime_values.clone(),vec![event.clone();pct_dbtime_values.clone().len()])
            .name("")
            .legend_group(&event_name)
            .box_mean(BoxMean::True)
            .orientation(Orientation::Horizontal)
            .x_axis("x1")
            .y_axis("y2")
            .marker(Marker::new().color("#e377c2".to_string()).opacity(0.7))
            .show_legend(false);
        plot.add_trace(dbt_box_plot);

        // Add Bar Plots for Histogram Buckets
        for (bucket, values) in histogram_data {
            let default_color: String = "#000000".to_string(); // Store default in a variable
            let color: &String = bucket_colors.get(&bucket).unwrap_or(&default_color);
            let bucket_name: String = format!("{}", &bucket);

            let ms_bucket_histogram = Histogram::new(values.clone())
                .name(&bucket_name)
                .legend_group(&bucket_name)
                .auto_bin_x(true)
                //.n_bins_x(50) // Number of bins
                .x_axis("x2")
                .y_axis("y3")
                .marker(Marker::new().color(color.clone()).opacity(0.7))
                .show_legend(true);
            plot.add_trace(ms_bucket_histogram);

            let ms_bucket_box_plot = BoxPlot::new_xy(values.clone(),vec![bucket.clone();values.clone().len()])// Use values for y-axis, // Use bucket names for x-axis
                .name("")
                .legend_group(&bucket_name)
                .box_mean(BoxMean::True)
                .orientation(Orientation::Horizontal)
                .x_axis("x2")
                .y_axis("y4")
                .marker(Marker::new().color(color.clone()).opacity(0.7))
                .show_legend(false);
            plot.add_trace(ms_bucket_box_plot);
        }

        let layout: Layout = Layout::new()
            .title(&format!("'{}'", event))
            .height(900)
            .bar_gap(0.0)
            .bar_mode(plotly::layout::BarMode::Overlay)
            .grid(
                LayoutGrid::new()
                    .rows(4)
                    .columns(1),
                    //.row_order(Grid::TopToBottom),
            )
            //.legend(
            //    Legend::new()
            //        .x(0.0)
            //        .x_anchor(Anchor::Left),
            //)
            .x_axis(
                Axis::new()
                    .title("% DBTime")
                    .domain(&[0.0, 1.0])
                    .anchor("y1")
                    .range(vec![0.,])
                    .show_grid(true),
            )
            .y_axis(
                Axis::new()
                    .domain(&[0.0, 0.3])
                    .anchor("x1")
                    .range(vec![0.,]),
            )
            .y_axis2(
                Axis::new()
                    .domain(&[0.3, 0.35])
                    .anchor("x1")
                    .range(vec![0.,])
                    .show_tick_labels(false),
            )
            .x_axis2(
                Axis::new()
                    .title("% Wait Event ms")
                    .domain(&[0.0, 1.0])
                    .anchor("y3")
                    .range(vec![0.,])
                    .show_grid(true),
            )
            .y_axis3(
                Axis::new()
                    .domain(&[0.45, 0.8])
                    .anchor("x2")
                    .range(vec![0.,]),
            )
            .y_axis4(
                Axis::new()
                    .domain(&[0.83, 1.0])
                    .anchor("x2")
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
    /********************************************/

    /*HashMap for calculating instance stats correlation*/
    let mut instance_stats: HashMap<String, Vec<f64>> = HashMap::new();
    /****************************************************/

    let mut x_vals: Vec<String> = Vec::new();
    
    // === ANALYZING ===
    println!("{}","\n==== ANALYZING ===".bright_cyan());
    let top_stats: TopStats = find_top_stats(collection.awrs.clone(), db_time_cpu_ratio, filter_db_time, snap_range.clone(), &logfile_name, &args);
    
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
    generate_events_plotfiles(&collection.awrs, &top_stats.events, true, &html_dir);
    generate_events_plotfiles(&collection.awrs, &top_stats.bgevents, false,&html_dir);
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
    let mut table_events: String = String::new();
    let mut table_bgevents: String = String::new();
    let mut table_sqls: String = String::new();
    let mut table_stat_corr: String = String::new();

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
        make_notes!(&logfile_name, args.quiet, "{: >39} {: <16.2} \tSTDDEV PCT of DB Time: {:.2}\n",   "--- AVG PCT of DB Time:", &avg_exec_t, &stddev_exec_t);
        make_notes!(&logfile_name, args.quiet, "{: >38}  {: <16.2} \tSTDDEV Wait Time (s):  {:.2}\n",   "--- AVG Wait Time (s):",  &avg_exec_s, &stddev_exec_s);
        make_notes!(&logfile_name, args.quiet, "{: >40}{: <8.2}  \t\tSTDDEV No. executions: {:.2}\n",   "--- AVG No. executions: ", &avg_exec_n, &stddev_exec_n);
        make_notes!(&logfile_name, args.quiet, "{: >39} {: <16.2} \tSTDDEV wait/exec (ms): {:.2}\n\n", "--- AVG wait/exec (ms):", &avg_wait_per_exec_ms, &stddev_wait_per_exec_ms);
        
        /* Print table of detected anomalies for given event_name (key.1)*/
        if let Some(anomalies) = top_stats.event_anomalies_mad.get(&key.1) {
            let anomalies_detection_msg = "Detected anomalies using Median Absolute Deviation on the following dates:".to_string().bold().underline().blue();
            make_notes!(&logfile_name, args.quiet, "\t\t{}\n",  anomalies_detection_msg);

            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("Date"),
                Cell::new("MAD Score"),
                Cell::new("Total Wait (s)"),
                Cell::new("Waits"),
                Cell::new("AVG Wait (ms)")
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
                let c_avg_wait_ms = Cell::new(&format!("{:.3}", wait_event.total_wait_time_s/(wait_event.waits as f64)*1000.0));

                table.add_row(Row::new(vec![c_event,c_mad_score,c_total_wait_s,c_waits, c_avg_wait_ms]));
            }
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, "\t\t{}\n", table_line);
            }
        }

        /* FGEVENTS - Generate a row for the HTML table */
        let safe_event_name: String = event_name.replace("/", "_").replace(" ", "_").replace(":","").replace("*","_");
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
            </tr>
            "#,
            safe_event_name,
            event_name,
            avg_exec_t, stddev_exec_t,  // PCT of DB Time
            avg_exec_s, stddev_exec_s,  // Wait Time (s)
            avg_exec_n, stddev_exec_n,  // Execution times
            avg_wait_per_exec_ms, stddev_wait_per_exec_ms,  // Wait per exec (ms)
            corr,
            (x_n.len() as f64 / x_vals.len() as f64 )* 100.0
        ));
    }
    
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
        make_notes!(&logfile_name, args.quiet, "{: >39} {: <16.2} \tSTDDEV PCT of DB Time: {:.2}\n",   "--- AVG PCT of DB Time:", &avg_exec_t, &stddev_exec_t);
        make_notes!(&logfile_name, args.quiet, "{: >38}  {: <16.2} \tSTDDEV Wait Time (s):  {:.2}\n",   "--- AVG Wait Time (s):",  &avg_exec_s, &stddev_exec_s);
        make_notes!(&logfile_name, args.quiet, "{: >40}{: <8.2}  \t\tSTDDEV No. executions: {:.2}\n",   "--- AVG No. executions: ", &avg_exec_n, &stddev_exec_n);
        make_notes!(&logfile_name, args.quiet, "{: >39} {: <16.2} \tSTDDEV wait/exec (ms): {:.2}\n\n", "--- AVG wait/exec (ms):", &avg_wait_per_exec_ms, &stddev_wait_per_exec_ms);
        
         /* Print table of detected anomalies for given event_name (key.1)*/
         if let Some(anomalies) = top_stats.bgevent_anomalies_mad.get(&key.1) {
            let anomalies_detection_msg = "Detected anomalies using Median Absolute Deviation on the following dates:".to_string().bold().underline().blue();
            make_notes!(&logfile_name, args.quiet, "\t\t{}\n",  anomalies_detection_msg);

            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("Date"),
                Cell::new("MAD Score"),
                Cell::new("Total Wait (s)"),
                Cell::new("Waits"),
                Cell::new("AVG Wait (ms)")
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

                table.add_row(Row::new(vec![c_event,c_mad_score,c_total_wait_s,c_waits, c_avg_wait_ms]));
            }
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, "\t\t{}\n", table_line);
            }
        }
        
        /* BGEVENTS - Generate a row for the HTML table */
        let safe_event_name: String = event_name.replace("/", "_").replace(" ", "_").replace(":","").replace("*","_");
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
            </tr>
            "#,
            safe_event_name,
            event_name,
            avg_exec_t, stddev_exec_t,  // PCT of DB Time
            avg_exec_s, stddev_exec_s,  // Wait Time (s)
            avg_exec_n, stddev_exec_n,  // Execution times
            avg_wait_per_exec_ms, stddev_wait_per_exec_ms,  // Wait per exec (ms)
            corr,
            (x_n.len() as f64 / x_vals.len() as f64 )* 100.0
        ));
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

        make_notes!(&logfile_name, args.quiet, "{: >24}{:.2}% of probes\n", "Marked as TOP in ", (x.len() as f64 / x_vals.len() as f64 )* 100.0);
        make_notes!(&logfile_name, args.quiet, "{: >35} {: <16.2} \tSTDDEV Ela by Exec: {:.2}\n", "--- AVG Ela by Exec:", avg_exec_t, stddev_exec_t);
        make_notes!(&logfile_name, args.quiet, "{: >38} {: <14.2} \tSTDDEV No. executions:  {:.2}\n", "--- AVG No. executions:", avg_exec_n, stddev_exec_n);
        make_notes!(&logfile_name, args.quiet, "{: >23} {} \n", "MODULE: ", top_stats.sqls.get(&sql_id).unwrap().blue());

        /* SQLs - Generate a row for the HTML table */
        table_sqls.push_str(&format!(
            r#"
            <tr>
                <td><a href="{}.html" target="_blank" class="nav-link" style="font-weight: bold">{}</a></td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
                <td>{:.2}</td>
            </tr>
            "#,
                &sql_id, &sql_id,
                avg_exec_t, stddev_exec_t,  // PCT of DB Time
                avg_exec_n, stddev_exec_n,  // Execution times
                corr,
                (x.len() as f64 / x_vals.len() as f64 )* 100.0
            )
        );
        
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

        let filename: String = format!("{}/{}.html", &html_dir,sql_id);
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
                    <p><span style="font-size:20px;font-weight:bold">{sql_id}</span></p>
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

        // Write to the file
        if let Err(e) = fs::write(&filename, sqlid_html_content) {
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
                </tr>
            </thead>
            <tbody>
            {}
            </tbody>
        </table>
        "#,
        table_sqls
    );

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

    let layout_main: Layout = Layout::new()
        .height(1200)
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
        .y_axis5(Axis::new()
            .anchor("x1")
            .domain(&[0.82, 1.0])
            .title("SQL Elapsed Time")
            .visible(true)
            .zero_line(true)
            .range_mode(RangeMode::ToZero)
        )
        .y_axis4(Axis::new()
            .anchor("x1")
            .domain(&[0.615, 0.815])
            .range(vec![0.,])
            .title("CPU Util (%)")
            .zero_line(true)
            .range(vec![0.,])
            .range_mode(RangeMode::ToZero)
        )
        .y_axis3(Axis::new()
            .anchor("x1")
            .domain(&[0.41, 0.61])
            .title("Wait Events (s)")
            .zero_line(true)
            .range(vec![0.,])
            .range_mode(RangeMode::ToZero)
        )
        .y_axis2(Axis::new()
            .anchor("x1")
            .domain(&[0.205, 0.405])
            .range(vec![0.,])
            .title("#")
            .zero_line(true)
            .range_mode(RangeMode::ToZero)
        )
        .y_axis(Axis::new()
            .anchor("x1")
            .domain(&[0., 0.2])
            .title("(s/s)")
            .zero_line(true)
            .range_mode(RangeMode::ToZero)
        )
        .x_axis(
            Axis::new()
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
            <span><strong>DB ID:</strong> {}</span>
            <span> <strong>&nbsp;&nbsp;&nbsp;Platform:</strong> {}</span>
            <span> <strong>&nbsp;&nbsp;&nbsp;Release:</strong> {}</span>
            <span> <strong>&nbsp;&nbsp;&nbsp;Startup Time:</strong> {}</span>
            <span> <strong>&nbsp;&nbsp;&nbsp;RAC:</strong> {}</span>
            <span> <strong>&nbsp;&nbsp;&nbsp;Instance Number:</strong> {}</span>
            <span> <strong>&nbsp;&nbsp;&nbsp;CPUs:</strong> {}</span>
            <span> <strong>&nbsp;&nbsp;&nbsp;Cores:</strong> {}</span>
            <span> <strong>&nbsp;&nbsp;&nbsp;Sockets:</strong> {} </span>
            <span> <strong>&nbsp;&nbsp;&nbsp;Memory (G):</strong> {} </span>
            <span style=\"margin-left: auto;\"> <strong>JAS-MIN</strong> v{}&nbsp;&nbsp;&nbsp</span>
            <br>
            <span style=\"width: 100%; text-align: center;\"><strong>Snap range:</strong> {} - {}</span>
    </div>",
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
        env!("CARGO_PKG_VERSION"),
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
        toggleTable('show-JASMINAI-button', 'chat-container');
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
        &format!("<body>\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}",
            db_instance_info_html,
            "<button id=\"show-events-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Wait Events</span><span>TOP Wait Events</span></button>",
            "<button id=\"show-sqls-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Wait SQLs</span><span>TOP Wait SQLs</span></button>",
            "<button id=\"show-bgevents-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Backgrd Events</span><span>TOP Backgrd Events</span></button>",
            format!(
                "<a href=\"{}\" target=\"_blank\" style=\"text-decoration: none;\">
                    <button id=\"show-stat_corr-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">STATS Correlation</span><span>STATS Correlation</span></button>
                </a>", "statistics_corr.html"
            ),
            "<button id=\"show-JASMINAI-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">JAS-MIN Assistant</span><span>JAS-MIN Assistant</span></button>",
            jasmin_assistant,
            event_table_html,
            bgevent_table_html,
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

    if args.ai != "NO" {
        let vendor_model_lang = args.ai.split(":").collect::<Vec<&str>>();
        if vendor_model_lang[0] == "openai" {
            chat_gpt(&logfile_name, vendor_model_lang).unwrap();
        } else if vendor_model_lang[0] == "google" {
            gemini(&logfile_name, vendor_model_lang).unwrap();
        } else {
            println!("Unrecognized vendor. Supported vendors: openai, google");
        }
        
    };
}
