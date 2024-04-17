use crate::awr::{AWRS, AWR, LoadProfile};
use plotly::Scatter;
use plotly::common::Mode;
use plotly::Plot;
use plotly::layout::{Axis, GridPattern, Layout, LayoutGrid, Legend, RowOrder, TraceOrder};
use std::collections::HashMap;

struct TopStats {
    events: HashMap<String, u8>,
    sqls:   HashMap<String, u8>,
}

fn find_top_stats(awrs: Vec<AWRS>) -> TopStats {
    let mut event_names: HashMap<String, u8> = HashMap::new();
    let mut sql_ids: HashMap<String, u8> = HashMap::new();
    for awr in awrs {
        let mut dbtime: f64 = 0.0;
        let mut cputime: f64 = 0.0;
        for lp in awr.awr_doc.load_profile {
            if lp.stat_name.starts_with("DB Time") || lp.stat_name.starts_with("DB time") {
                dbtime = lp.per_second;
                
            } else if lp.stat_name.starts_with("DB CPU") {
                cputime = lp.per_second;
            }
        }
        if dbtime > 0.0 && cputime > 0.0 && cputime/dbtime < 0.666 {
            let mut events = awr.awr_doc.foreground_wait_events;
            events.sort_by_key(|e| e.total_wait_time_s);
            let l = events.len();
            for i in 1..5 {
                event_names.entry(events[l-i].event.clone()).or_insert(1);
            }

            let mut sqls = awr.awr_doc.sql_elapsed_time;
            sqls.sort_by_key(|s| s.elapsed_time_s as i64);
            let l = sqls.len(); 
            for i in 1..5 {
                sql_ids.entry(sqls[l-i].sql_id.clone()).or_insert(1);
            }
        }
    }
    let top = TopStats {events: event_names, sqls: sql_ids,};
    top
}

pub fn plot_to_file(awrs: Vec<AWRS>, fname: String) {
    let mut y_vals_dbtime: Vec<f64> = Vec::new();
    let mut y_vals_dbcpu: Vec<f64> = Vec::new();
    let mut y_vals_events: HashMap<String, Vec<u64>> = HashMap::new();
    let mut y_vals_sqls: HashMap<String, Vec<f64>> = HashMap::new();

    let mut x_vals: Vec<String> = Vec::new();

    let top_stats = find_top_stats(awrs.clone());
    let top_events = top_stats.events;
    let top_sqls = top_stats.sqls;

    for awr in awrs {
        let xval = format!("{} ({})", awr.awr_doc.snap_info.begin_snap_time, awr.awr_doc.snap_info.begin_snap_id);
        x_vals.push(xval.clone());

        for (sql, _) in &top_sqls {
            y_vals_sqls.entry(sql.to_string()).or_insert(Vec::new());
            let mut v = y_vals_sqls.get_mut(sql).unwrap();
            v.push(0.0);
        } 

       for lp in awr.awr_doc.load_profile {
            if lp.stat_name.starts_with("DB Time") || lp.stat_name.starts_with("DB time") {
                y_vals_dbtime.push(lp.per_second);
                
                
            } else if lp.stat_name.starts_with("DB CPU") {
                y_vals_dbcpu.push(lp.per_second);
            }
       }

       for event in awr.awr_doc.foreground_wait_events { 
            if top_events.contains_key(&event.event) {
                y_vals_events.entry(event.event.clone()).or_insert(Vec::new());
                let mut v = y_vals_events.get_mut(&event.event).unwrap();
                v.push(event.total_wait_time_s);
            }
       }

       for sqls in awr.awr_doc.sql_elapsed_time {

            if top_sqls.contains_key(&sqls.sql_id) {
                let mut v = y_vals_sqls.get_mut(&sqls.sql_id).unwrap();
                v[x_vals.len()-1] = sqls.elapsed_time_s;
            }
       }
        
    }

    let dbtime_trace = Scatter::new(x_vals.clone(), y_vals_dbtime)
                                                    .mode(Mode::LinesMarkers)
                                                    .name("DB Time (s/s)")
                                                    .x_axis("x1")
                                                    .y_axis("y1");
    let dbcpu_trace = Scatter::new(x_vals.clone(), y_vals_dbcpu)
                                                    .mode(Mode::LinesMarkers)
                                                    .name("DB CPU (s/s)")
                                                    .x_axis("x1")
                                                    .y_axis("y1");
    let mut plot = Plot::new();
    plot.add_trace(dbtime_trace);
    plot.add_trace(dbcpu_trace);

    for (en,yv) in y_vals_events {
        let event_trace = Scatter::new(x_vals.clone(), yv)
                                                        .mode(Mode::LinesMarkers)
                                                        .name(en.clone())
                                                        .x_axis("x1")
                                                        .y_axis("y2");
        plot.add_trace(event_trace);
    }

    for (en,yv) in y_vals_sqls {
        let sql_trace = Scatter::new(x_vals.clone(), yv)
                                                        .mode(Mode::LinesMarkers)
                                                        .name(en.clone())
                                                        .x_axis("x1")
                                                        .y_axis("y3");
        plot.add_trace(sql_trace);
    }

    let layout = Layout::new()
        .height(1000)
        .y_axis(Axis::new().anchor("x1").domain(&[0., 0.33]))
        .y_axis2(Axis::new().anchor("x1").domain(&[0.33, 0.66]))
        .y_axis3(Axis::new().anchor("x1").domain(&[0.66, 1.]))
        ;

    plot.set_layout(layout);

    plot.use_local_plotly();
    plot.write_html(fname);
    plot.show();

}

  