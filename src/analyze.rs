use crate::awr::{ForegroundWaitEvents, HostCPU, LoadProfile, SQLCPUTime, SQLIOTime, SQLGets, SQLReads, AWR, AWRSCollection};
use plotly::color::NamedColor;
use plotly::{Plot, Histogram, BoxPlot, Scatter};
use plotly::common::{ColorBar, Mode, Title, Visible, Line, Orientation, Anchor, Marker};
use plotly::box_plot::{BoxMean,BoxPoints};
use plotly::layout::{Axis, GridPattern, Layout, LayoutGrid, Legend, RowOrder, TraceOrder, ModeBar, HoverMode, RangeMode};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use colored::*;
use open::*;
use std::str::FromStr;


use ndarray::Array2;
use ndarray_stats::CorrelationExt;
use ndarray_stats::histogram::Grid;

struct TopStats {
    events: BTreeMap<String, u8>,
    sqls:   BTreeMap<String, String>,
    stat_names: BTreeMap<String, u8>,
}

//We don't want to plot everything, because it would cause to much trouble 
//we need to find only essential wait events and SQLIDs 
fn find_top_stats(awrs: Vec<AWR>, db_time_cpu_ratio: f64, filter_db_time: f64, snap_range: String) -> TopStats {
    let mut event_names: BTreeMap<String, u8> = BTreeMap::new();
    let mut sql_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut stat_names: BTreeMap<String, u8> = BTreeMap::new();
    //so we scan the AWR data
    for awr in awrs {
        let snap_filter = snap_range.split("-").collect::<Vec<&str>>();
        let f_begin_snap = u64::from_str(snap_filter[0]).unwrap();
        let f_end_snap = u64::from_str(snap_filter[1]).unwrap();

        if awr.snap_info.begin_snap_id >= f_begin_snap && awr.snap_info.end_snap_id <= f_end_snap {
            let mut dbtime: f64 = 0.0;
            let mut cputime: f64 = 0.0;
            //We want to find dbtime and cputime because based on their delta we will base our decisions 
            for lp in awr.load_profile {
                if lp.stat_name.starts_with("DB Time") || lp.stat_name.starts_with("DB time") {
                    dbtime = lp.per_second;
                    
                } else if lp.stat_name.starts_with("DB CPU") {
                    cputime = lp.per_second;
                }
            }
            //If proportion of cputime and dbtime is less then db_time_cpu_ratio (default 0.666) than we want to find out what might be the problem 
            //because it means that Oracle spent some time waiting on wait events and not working on CPU
            if dbtime > 0.0 && cputime > 0.0 && cputime/dbtime < db_time_cpu_ratio && (filter_db_time==0.0 || dbtime>filter_db_time){
                println!("Analyzing a peak in {} ({}) for ratio: [{:.2}/{:.2}] = {:.2}", awr.file_name, awr.snap_info.begin_snap_time, cputime, dbtime, (cputime/dbtime));
                let mut events = awr.foreground_wait_events;
                //I'm sorting events by total wait time, to get the longest waits at the end
                events.sort_by_key(|e| e.total_wait_time_s as i64);
                let l = events.len();
                //We are registering only TOP10 from each snap
                if l > 10 {
                    for i in 1..10 {
                        event_names.entry(events[l-i].event.clone()).or_insert(1);
                    }
                }
                //And the same with SQLs
                let mut sqls = awr.sql_elapsed_time;
                sqls.sort_by_key(|s| s.elapsed_time_s as i64);
                let l = sqls.len();  
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
            for stats in awr.key_instance_stats {
                stat_names.entry(stats.statname.clone()).or_insert(1);
            }
        }
    }
    let top = TopStats {events: event_names, sqls: sql_ids, stat_names: stat_names,};
    top
}

//Calculate pearson correlation of 2 vectors and return simple result
fn pearson_correlation_2v(vec1: &Vec<f64>, vec2: &Vec<f64>) -> f64 {
    let rows = 2;
    let cols = vec1.len();

    let mut data = Vec::new();
    data.extend(vec1);
    data.extend(vec2);
    
    let a: ndarray::ArrayBase<ndarray::OwnedRepr<f64>, ndarray::Dim<[usize; 2]>> = Array2::from_shape_vec((rows, cols), data).unwrap();
    let crr = a.pearson_correlation().unwrap();

    crr.row(0)[1]
}

fn mean(data: Vec<f64>) -> Option<f64> {
    let sum = data.iter().sum::<f64>() as f64;
    let count = data.len();

    match count {
        positive if positive > 0 => Some(sum / count as f64),
        _ => None,
    }
}

fn std_deviation(data: Vec<f64>) -> Option<f64> {
    match (mean(data.clone()), data.len()) {
        (Some(data_mean), count) if count > 0 => {
            let variance = data.iter().map(|value| {
                let diff = data_mean - (*value as f64);

                diff * diff
            }).sum::<f64>() / count as f64;

            Some(variance.sqrt())
        },
        _ => None
    }
}

fn report_top_sql_sections(sqlid: &str, awrs: &Vec<AWR>) -> HashMap<String, f64> {
    let probe_size = awrs.len() as f64;

    let mut sql_io_time = 0.0;
    let mut sql_gets = 0.0;

    let mut is_statspack = false;
    
    if awrs[0].file_name.ends_with(".txt") {
        is_statspack = true;
    }

    //Filter HashMaps of SQL ordered by CPU Time to find how many times the given sqlid was marked in top section
    let sql_cpu: Vec<&HashMap<String,SQLCPUTime>> = awrs.iter().map(|awr| &awr.sql_cpu_time)
                                                  .filter(|sql| sql.contains_key(sqlid)).collect(); 

    let sql_cpu_count = sql_cpu.len() as f64;

    //Filter HashMaps of SQL ordered by User IO to find how many times the given sqlid was marked in top section
    let sql_io: Vec<&HashMap<String, SQLIOTime>> = awrs.iter().map(|awr| &awr.sql_io_time)
                                                 .filter(|sql|sql.contains_key(sqlid)).collect();
    let sql_io_count = sql_io.len() as f64;

     //Filter HashMaps of SQL ordered by GETS to find how many times the given sqlid was marked in top section
     let sql_gets: Vec<&HashMap<String, SQLGets>> = awrs.iter().map(|awr| &awr.sql_gets)
                                                    .filter(|sql|sql.contains_key(sqlid)).collect();
    let sql_gets_count = sql_gets.len() as f64;

     //Filter HashMaps of SQL ordered by READS to find how many times the given sqlid was marked in top section
     let sql_reads: Vec<&HashMap<String, SQLReads>> = awrs.iter().map(|awr| &awr.sql_reads)
                                                    .filter(|sql|sql.contains_key(sqlid)).collect();
    let sql_reads_count = sql_reads.len() as f64;

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

fn report_instance_stats_cor(instance_stats: HashMap<String, Vec<f64>>, dbtime_vec: Vec<f64>) {
    println!("\n");
    println!("-----------------------------------------------------------------------------------");
    println!("Correlation of instance statatistics with DB Time for values >= 0.5 and <= -0.5\n\n");

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
    for (k,v) in sorted_correlation {
        println!("\t{: >64} : {:.2}", &k.1, v);
    }
}

// Filter and generate histogram for top events
fn generate_fgevents_plotfiles(awrs: &Vec<AWR>, top_events: &BTreeMap<String, u8>, dirpath: &str) {
    
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

    // Filter ForegroundWaitEvents based on top_events
    let filtered_events: Vec<&ForegroundWaitEvents> = awrs
        .iter()
        .flat_map(|awr| &awr.foreground_wait_events)
        .filter(|event| top_events.contains_key(&event.event))
        .collect();

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
        let mut plot = Plot::new();
        let event_name = format!("{}",&event);
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
            let default_color = "#000000".to_string(); // Store default in a variable
            let color = bucket_colors.get(&bucket).unwrap_or(&default_color);
            let bucket_name = format!("{}", &bucket);

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

        let layout = Layout::new()
            .title(Title::new(&format!("'{}'", event)))
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
                    .title(Title::new("% DBTime"))
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
                    .title(Title::new("% Wait Event ms"))
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
        let safe_event_name = event.replace("/", "_").replace(" ", "_").replace(":","");
        let file_name = format!("{}/{}.html", dirpath, safe_event_name);

        // Save the plot as an HTML file
        let path = Path::new(&file_name);
        //plot.save(path).expect("Failed to save plot to file");
        plot.write_html(path);
    }
    println!("Saved plots for events to '{}'", dirpath);
}

pub fn plot_to_file(collection: AWRSCollection, fname: String, db_time_cpu_ratio: f64, filter_db_time: f64, snap_range: String) {
    let mut y_vals_dbtime: Vec<f64> = Vec::new();
    let mut y_vals_dbcpu: Vec<f64> = Vec::new();
    let mut y_vals_events: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_logons: Vec<f64> = Vec::new();
    let mut y_vals_calls: Vec<f64> = Vec::new();
    let mut y_vals_execs: Vec<f64> = Vec::new();
    let mut y_vals_trans: Vec<f64> = Vec::new();
    let mut y_vals_parses: Vec<f64> = Vec::new();
    let mut y_vals_hparses: Vec<f64> = Vec::new();
    let mut y_vals_cpu_user: Vec<f64> = Vec::new();
    let mut y_vals_cpu_load: Vec<f64> = Vec::new();
    let mut y_vals_redo_switches: Vec<f64> = Vec::new();
    let mut y_excessive_commits: Vec<f64> = Vec::new();
    let mut y_cleanout_ktugct: Vec<f64> = Vec::new();
    let mut y_cleanout_cr: Vec<f64> = Vec::new();

    /*Variables used for statistics computations*/
    let mut y_vals_events_n: BTreeMap<String, Vec<f64>> = BTreeMap::new(); 
    let mut y_vals_events_t: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_events_s: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls_exec_t: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls_exec_n: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    /********************************************/

    /*HashMap for calculating instance stats correlation*/
    let mut instance_stats: HashMap<String, Vec<f64>> = HashMap::new();
    /****************************************************/

    let mut x_vals: Vec<String> = Vec::new();

    let top_stats = find_top_stats(collection.awrs.clone(), db_time_cpu_ratio, filter_db_time, snap_range.clone());
    let top_events = top_stats.events;
    let top_sqls = top_stats.sqls;
    let all_stats = top_stats.stat_names;
    let mut is_logfilesync_high: bool = false;
    
    // Extract the parent directory and generate FG Events html plots

    //This will be empty if -d or -j specified without whole path
    let dir_path = Path::new(&fname).parent().unwrap_or(Path::new("")).to_str().unwrap_or("");
    let mut html_dir = String::new(); //variable for html files directory
    let mut stripped_fname = fname.as_str(); //without whole path to a file this will be just a file name
    if dir_path.len() == 0 {
        html_dir = format!("{}_reports", &fname); 
    } else { //if the whole path was specified we are extracting file name and seting html_dir properly 
        stripped_fname = Path::new(&fname).file_name().unwrap().to_str().unwrap();
        html_dir = format!("{}/{}_reports", &dir_path, &stripped_fname);
    }
    
    fs::create_dir(&html_dir).unwrap_or_default();
    generate_fgevents_plotfiles(&collection.awrs, &top_events,&html_dir);
    let fname = format!("{}/{}", html_dir, &stripped_fname); //new file name path for main report

    /* ------ Preparing data ------ */
    for awr in &collection.awrs {
        let snap_filter = snap_range.split("-").collect::<Vec<&str>>();
        let f_begin_snap = u64::from_str(snap_filter[0]).unwrap();
        let f_end_snap = u64::from_str(snap_filter[1]).unwrap();

        if awr.snap_info.begin_snap_id >= f_begin_snap && awr.snap_info.end_snap_id <= f_end_snap {

                let xval = format!("{} ({})", awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id);
                x_vals.push(xval.clone());

                //We have to fill the whole data traces for stats, wait events and SQLs with 0 to be sure that chart won't be moved to one side
                for (sql, _) in &top_sqls {
                    y_vals_sqls.entry(sql.to_string()).or_insert(Vec::new());
                    y_vals_sqls_exec_t.entry(sql.to_string()).or_insert(Vec::new());
                    y_vals_sqls_exec_n.entry(sql.to_string()).or_insert(Vec::new());
                    let mut v = y_vals_sqls.get_mut(sql).unwrap();
                    v.push(0.0);
                } 

                for (event, _) in &top_events {
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

                for (statname, _) in &all_stats {
                    instance_stats.entry(statname.to_string()).or_insert(Vec::new());
                    let mut v = instance_stats.get_mut(statname).unwrap();
                    v.push(0.0);
                }

                //Than we can set the current value of the vector to the desired one, if the event is in TOP section in that snap id
            for event in &awr.foreground_wait_events { 
                    if top_events.contains_key(&event.event) {
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
            //Same with SQLs
            for sqls in &awr.sql_elapsed_time {
                    if top_sqls.contains_key(&sqls.sql_id) {
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
                    } else if lp.stat_name.starts_with("Parses") {
                        y_vals_parses.push(lp.per_second);
                    } else if lp.stat_name.starts_with("Hard parses") {
                        y_vals_hparses.push(lp.per_second);
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
                let mut excessive_commit = 0.0;

                for activity in &awr.key_instance_stats {
                    let mut v = instance_stats.get_mut(&activity.statname).unwrap();
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

    //I want to stort wait events by most heavy ones across the whole period
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
    println!("Statistical Computations:\n-------------------------");

    let mut plot_main = Plot::new();
    let mut plot_highlight = Plot::new();

    let dbtime_trace = Scatter::new(x_vals.clone(), y_vals_dbtime.clone())
                                                    .mode(Mode::LinesText)
                                                    .name("DB Time (s/s)")
                                                    .x_axis("x1")
                                                    .y_axis("y1");
    let aas_box_plot  = BoxPlot::new(y_vals_dbtime.clone())
                                                    .name("AAS")
                                                    .x_axis("x2")
                                                    .y_axis("y2")
                                                    .box_mean(BoxMean::True)
                                                    .show_legend(false)
                                                    .box_points(BoxPoints::All)
                                                    .whisker_width(0.2)
                                                    .marker(Marker::new().color("#2d9c57".to_string()).opacity(0.7).size(2));

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
                                                    .marker(Marker::new().color("#6302eb".to_string()).opacity(0.7).size(2));
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

    // WAIT EVENTS Correlation and AVG/STDDEV calculation, print and feed table used for HTML
    let mut table_events = String::new();
    let mut table_sqls = String::new();

    for (key, yv) in &y_vals_events_sorted {
        let event_trace = Scatter::new(x_vals.clone(), yv.clone())
                                                        .mode(Mode::LinesText)
                                                        .name(key.1.clone())
                                                        .x_axis("x1")
                                                        .y_axis("y3").visible(Visible::LegendOnly);
        plot_main.add_trace(event_trace);
        let event_name = key.1.clone();
        /* Correlation calc */
        let corr = pearson_correlation_2v(&y_vals_dbtime, &yv);
        // Print Correlation considered high enough to mark it
        println!("\t{: >5}", &event_name);
        let correlation_info = format!("--- Correlation with DB Time: {:.2}", &corr);
        if corr >= 0.4 || corr <= -0.4 { 
            print!("{: >50}", correlation_info.red().bold());
        } else {
            print!("{: >50}", correlation_info);
        }
        /* STDDEV/AVG Calculations */
        let x_n = y_vals_events_n.get(&event_name).unwrap().clone();
        let avg_exec_n = mean(x_n.clone()).unwrap();
        let stddev_exec_n = std_deviation(x_n.clone()).unwrap();

        let x_t = y_vals_events_t.get(&event_name).unwrap().clone();
        let avg_exec_t = mean(x_t.clone()).unwrap();
        let stddev_exec_t = std_deviation(x_t).unwrap();

        let x_s = y_vals_events_s.get(&event_name).unwrap().clone();
        let avg_exec_s = mean(x_s.clone()).unwrap();
        let stddev_exec_s = std_deviation(x_s).unwrap();

        let avg_wait_per_exec_ms = (avg_exec_s / avg_exec_n) * 1000.0;
        let stddev_wait_per_exec_ms = (stddev_exec_s / stddev_exec_n) * 1000.0;
        // Print calculations:
        
        println!("\t\tMarked as TOP in {:.2}% of probes",  (x_n.len() as f64 / x_vals.len() as f64 )* 100.0);
        println!("{: >39}  {: <16.2} \tSTDDEV PCT of DB Time: {:.2}",   "--- AVG PCT of DB Time:", &avg_exec_t, &stddev_exec_t);
        println!("{: >38}  {: <16.2} \tSTDDEV Wait Time (s):  {:.2}",   "--- AVG Wait Time (s):",  &avg_exec_s, &stddev_exec_s);
        println!("{: >40}  {: <16.2} \tSTDDEV exec times:     {:.2}",   "--- AVG exec times:     ", &avg_exec_n, &stddev_exec_n);
        println!("{: >39}  {: <16.2} \tSTDDEV wait/exec (ms): {:.2}\n", "--- AVG wait/exec (ms):", &avg_wait_per_exec_ms, &stddev_wait_per_exec_ms);
        
        /* EVENTS - Generate a row for the HTML table */
        let safe_event_name = event_name.replace("/", "_").replace(" ", "_").replace(":","");
        table_events.push_str(&format!(
            r#"
            <tr>
                <td><a href="{}.html" target="_blank">{}</a></td>
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
    let event_table_html = format!(
        r#"
        <table id="events-table">
            <thead>
                <tr>
                    <th onclick="sortTable('events-table',0)" style="cursor: pointer;">Event Name</th>
                    <th onclick="sortTable('events-table',1)" style="cursor: pointer;">AVG % of DBTime</th>
                    <th onclick="sortTable('events-table',2)" style="cursor: pointer;">STDDEV % of DBTime</th>
                    <th onclick="sortTable('events-table',3)" style="cursor: pointer;">AVG Wait Time (s)</th>
                    <th onclick="sortTable('events-table',4)" style="cursor: pointer;">STDDEV Wait Time (s)</th>
                    <th onclick="sortTable('events-table',5)" style="cursor: pointer;">AVG Exec Times</th>
                    <th onclick="sortTable('events-table',6)" style="cursor: pointer;">STDDEV Exec Times</th>
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

    for (key,yv) in y_vals_sqls_sorted {
        let sql_trace = Scatter::new(x_vals.clone(), yv.clone())
                                                        .mode(Mode::LinesText)
                                                        .name(key.1.clone())
                                                        .x_axis("x1")
                                                        .y_axis("y5").visible(Visible::LegendOnly);
        plot_main.add_trace(sql_trace);
        
        let sql_id = key.1.clone();
        /* Correlation calc */
        let corr = pearson_correlation_2v(&y_vals_dbtime, &yv);
        // Print Correlation considered high enough to mark it
        let top_sections = report_top_sql_sections(&sql_id, &collection.awrs);
        println!(
            "\n\t{: >5} \t {}",
            &sql_id.bold(),
            format!("Other Top Sections: {}",
                &top_sections.iter().map(|(key, value)| format!("{} [{:.2}%]", key, value)).collect::<Vec<String>>().join(" | ")
            ).italic(),
        );

        let correlation_info = format!("--- Correlation with DB Time: {:.2}", &corr);
        if corr >= 0.4 || corr <= -0.4 { 
            print!("{: >49}", correlation_info.red().bold());
        } else {
            print!("{: >49}", correlation_info);
        }

        /* Calculate STDDEV and AVG for sqls executions number */
        let x = y_vals_sqls_exec_n.get(&key.1.clone()).unwrap().clone();
        let avg_exec_n = mean(x.clone()).unwrap();
        let stddev_exec_n = std_deviation(x).unwrap();

        /* Calculate STDDEV and AVG for sqls time per execution */
        let x = y_vals_sqls_exec_t.get(&key.1.clone()).unwrap().clone();
        let avg_exec_t = mean(x.clone()).unwrap();
        let stddev_exec_t = std_deviation(x.clone()).unwrap();
        
        println!("{: >24}{:.2}% of probes", "Marked as TOP in ", (x.len() as f64 / x_vals.len() as f64 )* 100.0);
        println!("{: >35} {: <16.2} \tSTDDEV Ela by Exec: {:.2}", "--- AVG Ela by Exec:", avg_exec_t, stddev_exec_t);
        println!("{: >34} {: <16.2} \tSTDDEV exec times:  {:.2}", "--- AVG exec times:", avg_exec_n, stddev_exec_n);
        println!("{: >23} {} ", "MODULE: ", top_sqls.get(&sql_id).unwrap().blue());

        /* SQLs - Generate a row for the HTML table */
        table_sqls.push_str(&format!(
            r#"
            <tr>
                <td><a href="{}.html" target="_blank">{}</a></td>
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
        for (key,ev) in &y_vals_events_sorted {
            let crr = pearson_correlation_2v(&yv, &ev);
            let corr_text = format!("{: >32} | {: <32} : {:.2}", "+".to_string(), key.1.clone(), crr);
            if crr >= 0.4 || crr <= - 0.4 { //Correlation considered high enough to mark it
                sql_corr_txt.push(format!(r#"<span style="color:red; font-weight:bold;">{}</span>"#, corr_text));
                println!("{}",corr_text.red().bold());
            } else {
                sql_corr_txt.push(corr_text.clone());
                println!("{}", corr_text);
            }
        }

        let filename = format!("{}/{}.html", html_dir,sql_id);
        // Format the content as HTML
        let html_content = format!(
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
            module=top_sqls.get(&sql_id).unwrap(),
            top_section=top_sections.iter().map(|(key, value)| format!("<span class=\"bold\">{}:</span> {:.2}%", key, value)).collect::<Vec<String>>().join("<br>"),
            sql_corr_txt = sql_corr_txt.join("<br>")
        );

        // Write to the file
        if let Err(e) = fs::write(&filename, html_content) {
            eprintln!("Error writing file {}: {}", filename, e);
        }
    }

    let sqls_table_html = format!(
        r#"
        <table id="sqls-table"">
            <thead>
                <tr>
                    <th onclick="sortTable('sqls-table',0)" style="cursor: pointer;">SQL ID</th>
                    <th onclick="sortTable('sqls-table',1)" style="cursor: pointer;">AVG Ela by Exec</th>
                    <th onclick="sortTable('sqls-table',2)" style="cursor: pointer;">STDDEV Ela by Exec</th>
                    <th onclick="sortTable('sqls-table',3)" style="cursor: pointer;">AVG exec times</th>
                    <th onclick="sortTable('sqls-table',4)" style="cursor: pointer;">STDDEV exec times</th>
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

    
    report_instance_stats_cor(instance_stats, y_vals_dbtime);

    let layout_main = Layout::new()
        .height(1200)
        .grid(
            LayoutGrid::new()
                .rows(5)
                .columns(1),
                //.row_order(Grid::TopToBottom),
        )
        //.legend(Legend::new()
            //.y_anchor(Anchor::Top)
        //    .x(1.02)
        //    .y(0.5)
        //)
        .y_axis5(Axis::new()
            .anchor("x1")
            .domain(&[0.82, 1.0])
            .title(Title::new("SQL Elapsed Time"))
            .visible(true)
            .zero_line(true)
            .range_mode(RangeMode::ToZero)
        )
        .y_axis4(Axis::new()
            .anchor("x1")
            .domain(&[0.615, 0.815])
            .range(vec![0.,])
            .title(Title::new("CPU Util (%)"))
            .zero_line(true)
            .range(vec![0.,])
            .range_mode(RangeMode::ToZero)
        )
        .y_axis3(Axis::new()
            .anchor("x1")
            .domain(&[0.41, 0.61])
            .title(Title::new("Wait Events (s)"))
            .zero_line(true)
            .range(vec![0.,])
            .range_mode(RangeMode::ToZero)
        )
        .y_axis2(Axis::new()
            .anchor("x1")
            .domain(&[0.205, 0.405])
            .range(vec![0.,])
            .title(Title::new("#"))
            .zero_line(true)
            .range_mode(RangeMode::ToZero)
        )
        .y_axis(Axis::new()
            .anchor("x1")
            .domain(&[0., 0.2])
            .title(Title::new("(s/s)"))
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

        let layout_highlight = Layout::new()
            .height(450)
            .grid(
                LayoutGrid::new()
                    .rows(1)
                    .columns(1),
                    //.row_order(Grid::TopToBottom),
            )
            .hover_mode(HoverMode::X)
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
    
    plot_main.set_layout(layout_main);
    plot_highlight.set_layout(layout_highlight);
    plot_main.use_local_plotly();
    plot_highlight.use_local_plotly();
    plot_main.write_html(fname.clone());
    plot_highlight.write_html(format!("{}/jasmin_highlight.html", html_dir));
    //plot.show();

    let db_instance_info_html = format!(
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
        env!("CARGO_PKG_VERSION")
    );
    
    let jasmin_html_scripts = format!(
        r#"
        <script>//JAS-MIN scripts
        function toggleTable(buttonId, tableId, buttonText) {{
            const button = document.getElementById(buttonId);
            const table = document.getElementById(tableId);
        
            button.addEventListener('click', () => {{
                if (table.style.display === 'none' || table.style.display === '') {{
                    table.style.display = 'table';
                    button.textContent = buttonText;
                    button.style.backgroundColor = '#272727';
                    button.style.color = 'white';
                    button.style.fontWeight = 'bold';
                    button.style.border = '1px solid #ccc';
                }} else {{
                    table.style.display = 'none';
                    button.textContent = buttonText;
                    button.style.backgroundColor = '#e0e0e0';
                    button.style.color = 'black';
                    button.style.fontWeight = 'normal';
                    button.style.border = '1px solid #ccc';
                }}
            }});
        }}
        toggleTable('show-events-button', 'events-table', 'TOP Wait Events');
        toggleTable('show-sqls-button', 'sqls-table', 'TOP SQLs');
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
        </script>"#
    );

    // Open plot_main HTML to inject Additional sections - Buttons, Tables, etc
    let mut plotly_html = fs::read_to_string(&fname)
        .expect("Failed to read Main JAS-MIN HTML file");
    let mut highlight_html = fs::read_to_string(format!("{}/jasmin_highlight.html", html_dir))
        .expect("Failed to read HighLight JAS-MIN HTML file");
    
    //Prepare HighLight Section to be pasted into Main HTML file
    highlight_html = highlight_html.replace("plotly-html-element","highlight-html-element");
    fs::write(format!("{}/jasmin_highlight.html", html_dir),&highlight_html);
    highlight_html = highlight_html
                    .lines() // Iterate over lines
                    .skip_while(|line| !line.contains("<div id=\"highlight-html-element\"")) // Skip lines until found
                    .take_while(|line| !line.contains("</script>")) // Keep only lines before `</script>`
                    .collect::<Vec<&str>>() // Collect remaining lines into a Vec
                    .join("\n"); // Convert back into a String
    // Inject the table HTML before the closing </body> tag
    plotly_html = plotly_html.replace(
        "<head>",
        &format!("<head>\n{}", 
    "<title>JAS-MIN</title>
    <style>
        #events-table {
            display: none;
        } 
        #sqls-table {
            display: none;
        }
        button {
            padding: 10px 20px;
            font-size: 16px;
            cursor: pointer;
        }
        table {
            margin-top: 20px;
            border-collapse: collapse;
            width: 100%;
            min-width: 400px;
            font-family: 'Helvetica', 'Arial', sans-serif;
            font-size: 0.9em;
            box-shadow: 0 0 20px rgba(0, 0, 0, 0.15);
        }
        #db-instance-info{
            margin-bottom: 20px;
            font-family: 'Helvetica', 'Arial', sans-serif;
            font-size: 0.9em;
            background-color: #ffd9d9;
            display: flex;
        }
        table thead tr {
            background-color: #009876;
            color: #ffffff;
            text-align: center;
        }
        #sqls-table thead tr {
            background-color: #006f98;
            color: #ffffff;
            text-align: center;
        }
        table th,
        table td {
            padding: 12px 15px;
            text-align: center;
        }
        table tbody tr {
            border-bottom: 1px solid #dddddd;
        }
        table tbody tr:nth-of-type(even) {
            background-color: #f3f3f3;
        }
        table tbody tr:last-of-type {
            border-bottom: 2px solid #009876;
        }
        #sqls-table tbody tr:last-of-type {
            border-bottom: 2px solid #006f98;
        }        
    </style>"
                )
    );

    plotly_html = plotly_html.replace(
        "<body>",
        &format!("<body>\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}",
            db_instance_info_html,
            "<button id=\"show-events-button\">TOP Wait Events</button>",
            "<button id=\"show-sqls-button\">TOP SQLs</button>",
            event_table_html,
            sqls_table_html,
            jasmin_html_scripts)
    );
    plotly_html = plotly_html.replace(
        "<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">", 
        &format!("{}\n\t\t\t\t</script>\n<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">",
                highlight_html)
    );

    // Write the updated HTML back to the file
    fs::write(&fname, plotly_html)
        .expect("Failed to write updated Plotly HTML file");

    open::that(fname);
}

//<div id="highlight-html-element" class="plotly-graph-div" style="height:100%; width:100%;"></div>