use crate::awr::{AWRS, AWR, LoadProfile, HostCPU};
use plotly::color::NamedColor;
use plotly::Scatter;
use plotly::common::{ColorBar, Mode, Visible};
use plotly::Plot;
use plotly::layout::{Axis, GridPattern, Layout, LayoutGrid, Legend, RowOrder, TraceOrder, ModeBar, HoverMode};
use std::collections::BTreeMap;

use ndarray::Array2;
use ndarray_stats::CorrelationExt;
 
struct TopStats {
    events: BTreeMap<String, u8>,
    sqls:   BTreeMap<String, u8>,
}

//We don't want to plot everything, because it would cause to much trouble 
//we need to find only essential wait events and SQLIDs 
fn find_top_stats(awrs: Vec<AWRS>, db_time_cpu_ratio: f64, filter_db_time: f64) -> TopStats {
    let mut event_names: BTreeMap<String, u8> = BTreeMap::new();
    let mut sql_ids: BTreeMap<String, u8> = BTreeMap::new();
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
            //We are registering only TOP5 from each snap
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
    }
    let top = TopStats {events: event_names, sqls: sql_ids,};
    top
}

fn correlation_of(what1: String, what2: String, vec1: Vec<f64>, vec2: Vec<f64>) {
    let rows = 2;
    let cols = vec1.len();

    let mut data = Vec::new();
    data.extend(vec1);
    data.extend(vec2);
    
    let a = Array2::from_shape_vec((rows, cols), data).unwrap();
    let crr = a.pearson_correlation().unwrap();
    if crr.row(0)[1] >= 0.4 { //Correlation considered high enough to mark it
        println!("\x1b[2m{} | {: <32} : \t {:.2}\x1b[0m", what1, what2, crr.row(0)[1]);
    } else {
        println!("{} | {: <32} : \t {:.2}", what1, what2, crr.row(0)[1]);
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


pub fn plot_to_file(awrs: Vec<AWRS>, fname: String, db_time_cpu_ratio: f64, filter_db_time: f64) {
    let mut y_vals_dbtime: Vec<f64> = Vec::new();
    let mut y_vals_dbcpu: Vec<f64> = Vec::new();
    let mut y_vals_events: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls_exec_t: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls_exec_n: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_logons: Vec<f64> = Vec::new();
    let mut y_vals_calls: Vec<f64> = Vec::new();
    let mut y_vals_execs: Vec<f64> = Vec::new();
    let mut y_vals_cpu: Vec<f64> = Vec::new();

    let mut x_vals: Vec<String> = Vec::new();

    let top_stats = find_top_stats(awrs.clone(), db_time_cpu_ratio, filter_db_time);
    let top_events = top_stats.events;
    let top_sqls = top_stats.sqls;

    println!("Correlations:\n-------------------------");

    for awr in awrs {
        let xval = format!("{} ({})", awr.awr_doc.snap_info.begin_snap_time, awr.awr_doc.snap_info.begin_snap_id);
        x_vals.push(xval.clone());

        //We have to fill the whole data traces for wait events and SQLs with 0 to be sure that chart won't be moved to one side
        for (sql, _) in &top_sqls {
            y_vals_sqls.entry(sql.to_string()).or_insert(Vec::new());
            y_vals_sqls_exec_t.entry(sql.to_string()).or_insert(Vec::new());
            y_vals_sqls_exec_n.entry(sql.to_string()).or_insert(Vec::new());
            let mut v = y_vals_sqls.get_mut(sql).unwrap();
            v.push(0.0);
            // let mut v = y_vals_sqls_exec_t.get_mut(sql).unwrap();
            // v.push(0.0);
            // let mut v = y_vals_sqls_exec_n.get_mut(sql).unwrap();
            // v.push(0.0);
        } 
        for (event, _) in &top_events {
            y_vals_events.entry(event.to_string()).or_insert(Vec::new());
            let mut v = y_vals_events.get_mut(event).unwrap();
            v.push(0.0);
        }

        //Than we can set the current value of the vector to the desired one, if the event is in TOP section in that snap id
       for event in awr.awr_doc.foreground_wait_events { 
            if top_events.contains_key(&event.event) {
                let mut v = y_vals_events.get_mut(&event.event).unwrap();
                v[x_vals.len()-1] = event.total_wait_time_s;
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

       if awr.awr_doc.host_cpu.pct_user < 0.0 {
            y_vals_cpu.push(0.0);
       } else {
            y_vals_cpu.push(awr.awr_doc.host_cpu.pct_user);
       }
       
    }

    let dbtime_trace = Scatter::new(x_vals.clone(), y_vals_dbtime.clone())
                                                    .mode(Mode::LinesMarkersText)
                                                    .name("DB Time (s/s)")
                                                    .x_axis("x1")
                                                    .y_axis("y1");

    let dbcpu_trace = Scatter::new(x_vals.clone(), y_vals_dbcpu)
                                                    .mode(Mode::LinesMarkersText)
                                                    .name("DB CPU (s/s)")
                                                    .x_axis("x1")
                                                    .y_axis("y1");

    let calls_trace = Scatter::new(x_vals.clone(), y_vals_calls)
                                                    .mode(Mode::LinesMarkersText)
                                                    .name("User Calls")
                                                    .x_axis("x1")
                                                    .y_axis("y2");

    let logons_trace = Scatter::new(x_vals.clone(), y_vals_logons)
                                                    .mode(Mode::LinesMarkersText)
                                                    .name("Logons")
                                                    .x_axis("x1")
                                                    .y_axis("y2");

    let exec_trace = Scatter::new(x_vals.clone(), y_vals_execs)
                                                    .mode(Mode::LinesMarkersText)
                                                    .name("Executes")
                                                    .x_axis("x1")
                                                    .y_axis("y2");

    let cpu_trace = Scatter::new(x_vals.clone(), y_vals_cpu)
                                                    .mode(Mode::LinesMarkersText)
                                                    .name("User CPU")
                                                    .x_axis("x1")
                                                    .y_axis("y4");

    let mut plot = Plot::new();
    plot.add_trace(dbtime_trace);
    plot.add_trace(dbcpu_trace);
    plot.add_trace(calls_trace);
    plot.add_trace(logons_trace);
    plot.add_trace(exec_trace);
    plot.add_trace(cpu_trace);

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

    for (key,yv) in y_vals_events_sorted {
        let event_trace = Scatter::new(x_vals.clone(), yv.clone())
                                                        .mode(Mode::LinesMarkers)
                                                        .name(key.1.clone())
                                                        .x_axis("x1")
                                                        .y_axis("y3");
        plot.add_trace(event_trace);
        //We are going to show correlation of each event with DB Time
        correlation_of("Correlation of DB Time".to_string(), key.1.clone(), y_vals_dbtime.clone(), yv.clone());
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
    for (key,yv) in y_vals_sqls_sorted {
        let sql_trace = Scatter::new(x_vals.clone(), yv.clone())
                                                        .mode(Mode::LinesMarkers)
                                                        .name(key.1.clone())
                                                        .x_axis("x1")
                                                        .y_axis("y5").visible(Visible::LegendOnly);
        plot.add_trace(sql_trace);
        correlation_of("Correlation of DB Time".to_string(), key.1.clone(), y_vals_dbtime.clone(), yv.clone());
        
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
            correlation_of("\t+ ".to_string(), key.1.clone(), yv.clone(), ev.clone());
        }
    }

    let layout = Layout::new()
        .height(1000)
        .y_axis(Axis::new().anchor("x1").domain(&[0., 0.2]))
        .y_axis2(Axis::new().anchor("x1").domain(&[0.2, 0.4]))
        .y_axis3(Axis::new().anchor("x1").domain(&[0.4, 0.6]))
        .y_axis4(Axis::new().anchor("x1").domain(&[0.6, 0.8]))
        .y_axis5(Axis::new().anchor("x1").domain(&[0.8, 1.0]))
        .hover_mode(HoverMode::XUnified);

    plot.set_layout(layout);
    
    plot.use_local_plotly();
    plot.write_html(fname);
    plot.show();

}

  