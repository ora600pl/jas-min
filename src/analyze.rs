use crate::awr::{AWRS, AWR, LoadProfile, HostCPU};
use plotly::color::NamedColor;
use plotly::Scatter;
use plotly::common::{ColorBar, Mode, Title, Visible};
use plotly::Plot;
use plotly::layout::{Axis, GridPattern, Layout, LayoutGrid, Legend, RowOrder, TraceOrder, ModeBar, HoverMode};
use std::collections::{BTreeMap, HashMap};
use std::fs;

use ndarray::Array2;
use ndarray_stats::CorrelationExt;
 
struct TopStats {
    events: BTreeMap<String, u8>,
    sqls:   BTreeMap<String, u8>,
    stat_names: BTreeMap<String, u8>,
}

//We don't want to plot everything, because it would cause to much trouble 
//we need to find only essential wait events and SQLIDs 
fn find_top_stats(awrs: Vec<AWRS>, db_time_cpu_ratio: f64, filter_db_time: f64) -> TopStats {
    let mut event_names: BTreeMap<String, u8> = BTreeMap::new();
    let mut sql_ids: BTreeMap<String, u8> = BTreeMap::new();
    let mut stat_names: BTreeMap<String, u8> = BTreeMap::new();
    //so we scan the AWR data
    for awr in awrs {
        let mut dbtime: f64 = 0.0;
        let mut cputime: f64 = 0.0;
        //We want to find dbtime and cputime because based on their delta we will base our decisions 
        for lp in awr.awr_doc.load_profile {
            if lp.stat_name.starts_with("DB Time") || lp.stat_name.starts_with("DB time") {
                dbtime = lp.per_second;
                
            } else if lp.stat_name.starts_with("DB CPU") {
                cputime = lp.per_second;
            }
        }
        //If proportion of cputime and dbtime is less then db_time_cpu_ratio (default 0.666) than we want to find out what might be the problem 
        //because it means that Oracle spent some time waiting on wait events and not working on CPU
        if dbtime > 0.0 && cputime > 0.0 && cputime/dbtime < db_time_cpu_ratio && (filter_db_time==0.0 || dbtime>filter_db_time){
            println!("Analyzing a peak in {} ({}) for ratio: [{:.2}/{:.2}] = {:.2}", awr.file_name, awr.awr_doc.snap_info.begin_snap_time, cputime, dbtime, (cputime/dbtime));
            let mut events = awr.awr_doc.foreground_wait_events;
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
            let mut sqls = awr.awr_doc.sql_elapsed_time;
            sqls.sort_by_key(|s| s.elapsed_time_s as i64);
            let l = sqls.len();  
            if l > 5 {
                for i in 1..5 {
                    sql_ids.entry(sqls[l-i].sql_id.clone()).or_insert(1);
                }
            } else if l >= 1 && l <=5 {
                for i in 0..l {
                    sql_ids.entry(sqls[i].sql_id.clone()).or_insert(1);
                }
            }
        }
        for stats in awr.awr_doc.key_instance_stats {
            stat_names.entry(stats.statname.clone()).or_insert(1);
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

fn correlation_of(what1: String, what2: String, vec1: Vec<f64>, vec2: Vec<f64>) {
    
    let crr = pearson_correlation_2v(&vec1, &vec2);
    if crr >= 0.4 || crr <= - 0.4 { //Correlation considered high enough to mark it
        println!("\x1b[2m{: >32} | {: <32} : {:.2}\x1b[0m", what1, what2, crr);
    } else {
        println!("{: >32} | {: <32} : {:.2}", what1, what2, crr);
    }
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

pub fn plot_to_file(awrs: Vec<AWRS>, fname: String, db_time_cpu_ratio: f64, filter_db_time: f64) {
    let mut y_vals_dbtime: Vec<f64> = Vec::new();
    let mut y_vals_dbcpu: Vec<f64> = Vec::new();
    let mut y_vals_events: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_logons: Vec<f64> = Vec::new();
    let mut y_vals_calls: Vec<f64> = Vec::new();
    let mut y_vals_execs: Vec<f64> = Vec::new();
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

    let top_stats = find_top_stats(awrs.clone(), db_time_cpu_ratio, filter_db_time);
    let top_events = top_stats.events;
    let top_sqls = top_stats.sqls;
    let all_stats = top_stats.stat_names;
    let mut is_excessive_commits: bool = false;
    
    println!("Correlations:\n-------------------------");

    for awr in awrs {
        let xval = format!("{} ({})", awr.awr_doc.snap_info.begin_snap_time, awr.awr_doc.snap_info.begin_snap_id);
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
                is_excessive_commits = true;
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
       for event in awr.awr_doc.foreground_wait_events { 
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
       for sqls in awr.awr_doc.sql_elapsed_time {
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
       for lp in awr.awr_doc.load_profile {
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
            }
       }
        // ----- Host CPU
        if awr.awr_doc.host_cpu.pct_user < 0.0 {
             y_vals_cpu_user.push(0.0);
        } else {
             y_vals_cpu_user.push(awr.awr_doc.host_cpu.pct_user);
        }
        y_vals_cpu_load.push(100.0-awr.awr_doc.host_cpu.pct_idle);

        // ----- Additionally plot Redo Log Switches
        y_vals_redo_switches.push(awr.awr_doc.redo_log.per_hour);

        let mut calls: u64 = 0;
        let mut commits: u64 = 0;
        let mut rollbacks: u64 = 0;
        let mut cleanout_ktugct: u64 = 0;
        let mut cleanout_cr: u64 = 0;
        let mut excessive_commit = 0.0;

        for activity in awr.awr_doc.key_instance_stats {
            let mut v = instance_stats.get_mut(&activity.statname).unwrap();
            v[x_vals.len()-1] = activity.total as f64;

            // ----- Excessive Commits - plot if 'log file sync' is in top events
            if is_excessive_commits {
               
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
       if is_excessive_commits {
            y_excessive_commits.push(excessive_commit);
            /* This is for printing delayed block cleanouts when log file sync is present*/
            y_cleanout_ktugct.push(cleanout_ktugct as f64);
            y_cleanout_cr.push(cleanout_cr as f64);
       }
    }

    let mut plot = Plot::new();

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

    let exec_trace = Scatter::new(x_vals.clone(), y_vals_execs)
                                                    .mode(Mode::LinesText)
                                                    .name("Executes")
                                                    .x_axis("x1")
                                                    .y_axis("y2");

    let cpu_user = Scatter::new(x_vals.clone(), y_vals_cpu_user)
                                                    .mode(Mode::LinesText)
                                                    .name("CPU User")
                                                    .x_axis("x1")
                                                    .y_axis("y4");
    
    let cpu_load = Scatter::new(x_vals.clone(), y_vals_cpu_load)
                                                    .mode(Mode::LinesText)
                                                    .name("CPU Load")
                                                    .x_axis("x1")
                                                    .y_axis("y4");
    if is_excessive_commits{
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
        plot.add_trace(redo_switches);
        plot.add_trace(excessive_commits);
        plot.add_trace(cleanout_cr_only);
        plot.add_trace(cleanout_ktugct_calls);
    }

    
    plot.add_trace(dbtime_trace);
    plot.add_trace(dbcpu_trace);
    plot.add_trace(calls_trace);
    plot.add_trace(logons_trace);
    plot.add_trace(exec_trace);
    plot.add_trace(cpu_user);
    plot.add_trace(cpu_load);

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
    let y_vals_events_sorted2 = y_vals_events_sorted.clone();

    // WAIT EVENTS Correlation and AVG/STDDEV calcution, print and feed table used for HTML
    let mut table_events = String::new();

    for (key, yv) in y_vals_events_sorted {
        let event_trace = Scatter::new(x_vals.clone(), yv.clone())
                                                        .mode(Mode::LinesText)
                                                        .name(key.1.clone())
                                                        .x_axis("x1")
                                                        .y_axis("y3").visible(Visible::LegendOnly);
        plot.add_trace(event_trace);
        let event_name = key.1.clone();
        /* Correlation calc */
        let corr = pearson_correlation_2v(&y_vals_dbtime.clone(), &yv.clone());
        // Print Correlation considered high enough to mark it
        if corr >= 0.4 || corr <= -0.4 { 
            println!("\x1b[31m{: >32} | {: <32} : {:.2}\x1b[0m", "Correlation of DB Time".to_string(), &event_name, &corr);
        } else {
            println!("{: >32} | {: <32} : {:.2}", "Correlation of DB Time".to_string(), &event_name, &corr);
        }
        /* STDDEV/AVG Calculations */
        let x_n = y_vals_events_n.get(&event_name).unwrap().clone();
        let avg_exec_n = mean(x_n.clone()).unwrap();
        let stddev_exec_n = std_deviation(x_n).unwrap();

        let x_t = y_vals_events_t.get(&event_name).unwrap().clone();
        let avg_exec_t = mean(x_t.clone()).unwrap();
        let stddev_exec_t = std_deviation(x_t).unwrap();

        let x_s = y_vals_events_s.get(&event_name).unwrap().clone();
        let avg_exec_s = mean(x_s.clone()).unwrap();
        let stddev_exec_s = std_deviation(x_s).unwrap();

        let avg_wait_per_exec_ms = (avg_exec_s / avg_exec_n) * 1000.0;
        let stddev_wait_per_exec_ms = (stddev_exec_s / stddev_exec_n) * 1000.0;
        // Print calculations:
        println!("\t\t --- AVG PCT of DB Time: {: <16.2} \tSTDDEV PCT of DB Time: {:.2}", &avg_exec_t, &stddev_exec_t);
        println!("\t\t --- AVG Wait Time (s):  {: <16.2} \tSTDDEV Wait Time (s):  {:.2}", &avg_exec_s, &stddev_exec_s);
        println!("\t\t --- AVG exec times:     {: <16.2} \tSTDDEV exec times:     {:.2}", &avg_exec_n, &stddev_exec_n);
        println!("\t\t --- AVG wait/exec (ms): {: <16.2} \tSTDDEV wait/exec (ms): {:.2}\n", &avg_wait_per_exec_ms, &stddev_wait_per_exec_ms);
        /* Generate a row for the HTML table */
        table_events.push_str(&format!(
            r#"
            <tr>
                <td>{}</td>
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
            event_name,
            avg_exec_t, stddev_exec_t,  // PCT of DB Time
            avg_exec_s, stddev_exec_s,  // Wait Time (s)
            avg_exec_n, stddev_exec_n,  // Execution times
            avg_wait_per_exec_ms, stddev_wait_per_exec_ms,  // Wait per exec (ms)
            corr
        ));
    }
    let table_html = format!(
        r#"
        <table id="data-table">
            <thead>
                <tr>
                    <th>Event Name</th>
                    <th>AVG % of DBTime</th>
                    <th>STDDEV % of DBTime</th>
                    <th>AVG Wait Time (s)</th>
                    <th>STDDEV Wait Time (s)</th>
                    <th>AVG Exec Times</th>
                    <th>STDDEV Exec Times</th>
                    <th>AVG Wait per Exec (ms)</th>
                    <th>STDDEV Wait per Exec (ms)</th>
                    <th>Correlation of DBTime</th>
                </tr>
            </thead>
            <tbody>
            {}
            </tbody>
        </table>
        <script>
            const button = document.getElementById('show-table-button');
            const table = document.getElementById('data-table');
            button.addEventListener('click', () => {{
                if (table.style.display === 'none' || table.style.display === '') {{
                    table.style.display = 'table'; // Show the table
                    button.textContent = 'TOP Wait Events'; // Update button text
                }} else {{
                    table.style.display = 'none'; // Hide the table
                    button.textContent = 'TOP Wait Events'; // Update button text
                }}
            }});
        </script>
        "#,
        table_events
    );

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
    for (key,yv) in y_vals_sqls_sorted {
        let sql_trace = Scatter::new(x_vals.clone(), yv.clone())
                                                        .mode(Mode::LinesText)
                                                        .name(key.1.clone())
                                                        .x_axis("x1")
                                                        .y_axis("y5").visible(Visible::LegendOnly);
        plot.add_trace(sql_trace);
        correlation_of("\n\tCorrelation of DB Time\t".to_string(), key.1.clone(), y_vals_dbtime.clone(), yv.clone());
        
        /* Calculate STDDEV and AVG for sqls executions number */
        let x = y_vals_sqls_exec_n.get(&key.1.clone()).unwrap().clone();
        let avg_exec_n = mean(x.clone()).unwrap();
        let stddev_exec_n = std_deviation(x).unwrap();

        /* Calculate STDDEV and AVG for sqls time per execution */
        let x = y_vals_sqls_exec_t.get(&key.1.clone()).unwrap().clone();
        let avg_exec_t = mean(x.clone()).unwrap();
        let stddev_exec_t = std_deviation(x).unwrap();

        println!("\t\t --- AVG Ela by Exec: {:.2} \tSTDDEV Ela by Exec: {:.2}", avg_exec_t, stddev_exec_t);
        println!("\t\t --- AVG exec times:  {:.2} \tSTDDEV exec times:  {:.2}", avg_exec_n, stddev_exec_n);
        
        for (key,ev) in &y_vals_events_sorted2 {
            correlation_of("+".to_string(), key.1.clone(), yv.clone(), ev.clone());
        }
    }
    
    report_instance_stats_cor(instance_stats, y_vals_dbtime);

    let layout = Layout::new()
        .height(1000)
        .y_axis5(Axis::new()
            .anchor("x1")
            .domain(&[0.8, 1.0])
            .title(Title::new("SQL Elapsed Time"))
            .visible(true)
        )
        .y_axis4(Axis::new()
            .anchor("x1")
            .domain(&[0.6, 0.8])
            .range(vec![0.,])
            .title(Title::new("CPU Util (%)"))
        )
        .y_axis3(Axis::new()
            .anchor("x1")
            .domain(&[0.4, 0.6])
            .title(Title::new("Wait Events (s)"))
        )
        .y_axis2(Axis::new()
            .anchor("x1")
            .domain(&[0.2, 0.4])
            .range(vec![0.,])
            .title(Title::new("#"))
        )
        .y_axis(Axis::new()
            .anchor("x1")
            .domain(&[0., 0.2])
            .title(Title::new("(s/s)"))
        )
        .hover_mode(HoverMode::X);
    plot.set_layout(layout);
    plot.use_local_plotly();
    plot.write_html(fname.clone());
    plot.show();
    // Modify HTML and inject Additional sections - Buttons, Tables, etc
    let mut plotly_html = fs::read_to_string(&fname)
        .expect("Failed to read Plotly HTML file");

    // Inject the table HTML before the closing </body> tag
    plotly_html = plotly_html.replace("<head>", &format!("<head>\n{}", "<title>JAS-MIN</title>
    <style>
        #data-table {
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

        table thead tr {
            background-color: #009876;
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
    </style>"));
    plotly_html = plotly_html.replace("<body>",&format!("<body>\n\t{}","<button id=\"show-table-button\">TOP Wait Events</button>"));
    plotly_html = plotly_html.replace("<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">", &format!("{}\n<div id=\"plotly-html-element\" class=\"plotly-graph-div\" style=\"height:100%; width:100%;\">", table_html));

    // Write the updated HTML back to the file
    fs::write(&fname, plotly_html)
        .expect("Failed to write updated Plotly HTML file");
}

  
