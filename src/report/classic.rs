use crate::analyze::TopStats;
use crate::awr::{
    GetStats, HostCPU, IOStats, LoadProfile, SQLCPUTime, SQLGets, SQLIOTime, SQLReads,
    SegmentStats, WaitEvents, AWR,
};
use crate::reasonings::{
    IOStatsByFunctionSummary, LatchActivitySummary, ReportForAI, StatsSummary, Top10SegmentStats,
};
use crate::staticdata::*;
use crate::tools::*;
use crate::{debug_note, make_notes, Args};

use colored::*;
use open::*;
use plotly::box_plot::{BoxMean, BoxPoints};
use plotly::color::NamedColor;
use plotly::common::{
    Anchor, ColorBar, ColorScale, ColorScalePalette, HoverInfo, Line, Marker, MarkerSymbol, Mode,
    Orientation, Title, Visible,
};
use plotly::layout::{
    Axis, GridPattern, HoverMode, Layout, LayoutGrid, Legend, ModeBar, RangeMode, RowOrder,
    TraceOrder,
};
use plotly::{BoxPlot, HeatMap, Histogram, Plot, Scatter};
use prettytable::{format, Attr, Cell, Row, Table};
use rayon::prelude::*;
use regex::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;

pub(crate) fn merge_ash_sqls_to_events(
    ash_event_sql_map: HashMap<String, HashSet<String>>,
    dirpath: &str,
) {
    for (event, sqls) in ash_event_sql_map {
        let filename = get_safe_filename(event.clone(), "fg".to_string());
        let path = Path::new(&dirpath).join(&filename);
        let mut event_html_content = format!(
            r#"
                <h4 style="color:blue;font-weight:bold;">Wait Event found in ASH for following SQL IDs:</h4>
                <ul>
            "#
        );
        for sqlid in sqls {
            event_html_content = format!("{}<li><a href=../sqlid/sqlid_{}.html target=_blank style=\"color: black;\">{}</a></li>", event_html_content, sqlid, sqlid);
        }
        event_html_content = format!("{}</ul>", event_html_content);
        if path.exists() {
            let mut event_file: String = fs::read_to_string(&path)
                .expect(&format!("Failed to read file: {}", &path.to_string_lossy()));
            event_file = event_file.replace("</h2>", &format!("</h2>\n{}\n", event_html_content));

            if let Err(e) = fs::write(&path, event_file) {
                eprintln!("Error writing file {}: {}", &path.to_string_lossy(), e);
            }
        }
    }
}

//Add SQL_IDs found with strong correlation to event charts
pub(crate) fn merge_correlated_sqls_to_events(
    crr_event_sql_map: HashMap<String, HashMap<String, f64>>,
    dirpath: &str,
) {
    for (event, sqls) in crr_event_sql_map {
        let filename = get_safe_filename(event.clone(), "fg".to_string());
        let path = Path::new(&dirpath).join(&filename);
        let mut event_html_content = format!(
            r#"
                <h4 style="color:blue;font-weight:bold;">SQL IDs with strong correlation with this wait event:</h4>
                <ul>
            "#
        );

        let mut vec_sqls: Vec<_> = sqls.into_iter().collect();
        vec_sqls.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        for (sqlid, crr) in vec_sqls {
            event_html_content = format!("{}<li><a href=../sqlid/sqlid_{}.html target=_blank style=\"color: black;\">{:.2} | {}</a></li>", event_html_content, sqlid, crr, sqlid);
        }
        event_html_content = format!("{}</ul>", event_html_content);
        if path.exists() {
            let mut event_file: String = fs::read_to_string(&path)
                .expect(&format!("Failed to read file: {}", &path.to_string_lossy()));
            event_file = event_file.replace("</h2>", &format!("</h2>\n{}\n", event_html_content));

            if let Err(e) = fs::write(&path, event_file) {
                eprintln!("Error writing file {}: {}", &path.to_string_lossy(), e);
            }
        }
    }
}

// Generate plots for top events
pub(crate) fn generate_events_plotfiles(
    awrs: &Vec<AWR>,
    top_events: &BTreeMap<String, u8>,
    is_fg: bool,
    snap_range: &(u64, u64),
    dirpath: &str,
) {
    let (f_begin_snap, f_end_snap) = snap_range;
    // Ensure that there is at least one AWR with fg or bg event and they are explicitly checked
    assert!(
        !awrs.is_empty(),
        "generate_events_plotfiles: No AWR data available."
    );

    let mut hist_buckets: Vec<String> = Vec::new();
    let mut bucket_colors: HashMap<String, String> = HashMap::new();
    // Make colors consistent across buckets
    let color_palette = vec![
        "#00E399", "#2FD900", "#E3E300", "#FFBF00", "#FF8000", "#FF4000", "#FF0000", "#B22222",
        "#8B0000", "#4B0082", "#8A2BE2", "#1E90FF",
    ];
    // Group Events by Name and by needed data (ename(db_time, total_wait, waits,histogram values by bucket, heatmap)
    let mut snap_time: Vec<String> = Vec::new();
    struct EventStats {
        pct_dbtime: Vec<Option<f64>>,
        total_wait_time_s: Vec<Option<f64>>,
        waits: Vec<Option<u64>>,
        histogram_by_bucket: BTreeMap<String, Vec<Option<f32>>>,
        heatmap: Vec<Option<BTreeMap<String, f32>>>,
    }

    let mut data_by_event: HashMap<String, EventStats> = HashMap::new();
    let mut buckets_found: bool = false;

    for awr in awrs {
        if awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap
        {
            snap_time.push(format!(
                "{} ({})",
                awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id
            ));

            let events = if is_fg {
                &awr.foreground_wait_events
            } else {
                &awr.background_wait_events
            };

            if events.is_empty() {
                println!(
                    "   WARNING: generate_events_plotfiles found empty events {}",
                    awr.snap_info.begin_snap_id
                );
                continue;
            } else {
                if !buckets_found {
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
                        let entry = data_by_event.entry(top_event.0.clone()).or_insert_with(|| {
                            EventStats {
                                pct_dbtime: Vec::new(),
                                total_wait_time_s: Vec::new(),
                                waits: Vec::new(),
                                histogram_by_bucket: BTreeMap::new(),
                                heatmap: Vec::new(),
                            }
                        });
                        // Gather data by Event Name
                        entry.pct_dbtime.push(Some(event.pct_dbtime));
                        entry.total_wait_time_s.push(Some(event.total_wait_time_s));
                        entry.waits.push(Some(event.waits));
                        for (bucket, value) in &event.waitevent_histogram_ms {
                            entry
                                .histogram_by_bucket
                                .entry(bucket.clone())
                                .or_insert_with(Vec::new)
                                .push(Some(*value));
                        }
                        entry
                            .heatmap
                            .push(Some(event.waitevent_histogram_ms.clone()))
                    } else {
                        // Event does NOT exist in this snapshot → push gaps
                        let entry = data_by_event.entry(top_event.0.clone()).or_insert_with(|| {
                            EventStats {
                                pct_dbtime: Vec::new(),
                                total_wait_time_s: Vec::new(),
                                waits: Vec::new(),
                                histogram_by_bucket: BTreeMap::new(),
                                heatmap: Vec::new(),
                            }
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
            let row: Vec<Option<f32>> = entry
                .heatmap
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
        let dbt_box_plot = BoxPlot::new_xy(
            entry.pct_dbtime.clone(),
            vec![event.clone(); entry.pct_dbtime.clone().len()],
        )
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

            let ms_bucket_box_plot =
                BoxPlot::new_xy(values.clone(), vec![bucket.clone(); values.clone().len()]) // Use values for y-axis, // Use bucket names for x-axis
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
            .grid(LayoutGrid::new().rows(7).columns(1))
            .x_axis(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("y1")
                    .range(vec![0.])
                    .show_grid(true),
            )
            .y_axis(
                Axis::new()
                    .domain(&[0.0, 0.22])
                    .anchor("x1")
                    .range(vec![0.]),
            )
            .y_axis2(
                Axis::new()
                    .anchor("x1")
                    .domain(&[0.23, 0.33])
                    .title("Total Wait (s)")
                    .zero_line(true)
                    .range(vec![0.])
                    .range_mode(RangeMode::ToZero),
            )
            .y_axis3(
                Axis::new()
                    .anchor("x1")
                    .domain(&[0.33, 0.43])
                    .title("Wait count #")
                    .zero_line(true)
                    .range(vec![0.])
                    .range_mode(RangeMode::ToZero),
            )
            .x_axis2(
                Axis::new()
                    .title("% DBTime")
                    .domain(&[0.0, 1.0])
                    .anchor("y4")
                    .range(vec![0.])
                    .show_grid(true),
            )
            .y_axis4(
                Axis::new()
                    .domain(&[0.48, 0.6])
                    .anchor("x2")
                    .range(vec![0.]),
            )
            .y_axis5(
                Axis::new()
                    .domain(&[0.6, 0.62])
                    .anchor("x2")
                    .range(vec![0.])
                    .show_tick_labels(false),
            )
            .x_axis3(
                Axis::new()
                    .title("% Wait Event ms")
                    .domain(&[0.0, 1.0])
                    .anchor("y6")
                    .range(vec![0.])
                    .show_grid(true),
            )
            .y_axis6(
                Axis::new()
                    .domain(&[0.67, 0.83])
                    .anchor("x3")
                    .range(vec![0.]),
            )
            .y_axis7(
                Axis::new()
                    .domain(&[0.85, 1.0])
                    .anchor("x3")
                    .range(vec![0.])
                    .show_tick_labels(false)
                    .show_grid(true),
            );
        plot.set_layout(layout);

        let file_name = get_safe_filename(
            event.clone(),
            if is_fg {
                "fg".to_string()
            } else {
                "bg".to_string()
            },
        );

        // Save the plot as an HTML file
        let path = Path::new(&dirpath).join(&file_name);
        //plot.save(path).expect("Failed to save plot to file");
        plot.write_html(&path);
        let mut event_file: String =
            fs::read_to_string(&path).expect(&format!("Failed to read file: {}", file_name));
        event_file = event_file.replace(
            "<body>",
            &format!("<style>\nbody {{ font-family: Arial, sans-serif; }}.content {{ font-size: 16px; }}\n</style>\n<body>\n\t<h2 style=\"width:100%;text-align:center;\">{}</h2>",event));
        if let Err(e) = fs::write(&path, event_file) {
            eprintln!("Error writing file {}: {}", file_name, e);
        }
    }
    if is_fg {
        println!(
            "Saved plots for Foreground events to '{}/fg/fg_*'",
            &dirpath
        );
    } else {
        println!(
            "Saved plots for Background events to '{}/bg/bg_*'",
            &dirpath
        );
    }
}

// Generate TOP SQLs html subpages with plots - To Be Developed
pub(crate) fn generate_sqls_plotfiles(
    awrs: &Vec<AWR>,
    top_stats: &TopStats,
    snap_range: &(u64, u64),
    dirpath: &str,
) {
    let (f_begin_snap, f_end_snap) = snap_range;

    struct SQLStats {
        execs: Vec<Option<u64>>,           // Number of Executions
        ela_exec_s: Vec<Option<f64>>,      // Elapsed Time (s) per Execution
        ela_pct_total: Vec<Option<f64>>,   // Elapsed Time as a percentage of Total DB time
        pct_cpu: Vec<Option<f64>>,         // CPU Time as a percentage of Elapsed Time
        pct_io: Vec<Option<f64>>,          // User I/O Time as a percentage of Elapsed Time
        cpu_time_exec_s: Vec<Option<f64>>, // CPU Time (s) per Execution
        cpu_t_pct_total: Vec<Option<f64>>, // CPU Time as a percentage of Total DB CPU
        io_time_exec_s: Vec<Option<f64>>,  // User I/O Wait Time (s) per Execution
        io_pct_total: Vec<Option<f64>>, // User I/O Time as a percentage of Total User I/O Wait time
        gets_per_exec: Vec<Option<f64>>, // Number of Buffer Gets per Execution
        gets_pct_total: Vec<Option<f64>>, // Buffer Gets as a percentage of Total Buffer Gets
        phy_r_exec: Vec<Option<f64>>,   // Number of Physical Reads per Execution
        phy_r_pct_total: Vec<Option<f64>>, // Physical Reads as a percentage of Total Disk Reads
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
        .filter(|awr| {
            awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap
        })
        .map(|awr| {
            format!(
                "{} ({})",
                awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id
            )
        })
        .collect();

    let mut combined_sqls = top_stats.sqls.clone();
    combined_sqls.extend(top_stats.sqls_cpu.clone());

    let sqls_by_stats: HashMap<String, SQLStats> = combined_sqls
        .par_iter()
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
                if awr.snap_info.begin_snap_id >= *f_begin_snap
                    && awr.snap_info.end_snap_id <= *f_end_snap
                {
                    let mut sql_found: bool = false;
                    // Elapsed Time
                    if let Some(sql_et) = awr.sql_elapsed_time.iter().find(|e| &e.sql_id == sql_id)
                    {
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
                        if !sql_found {
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
                        if !sql_found {
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
                        if !sql_found {
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
                        if !sql_found {
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

    for (sql, stats) in sqls_by_stats {
        let mut sql_plot: Plot = Plot::new();
        let sql_id = format!("{}", &sql);

        let sql_gets_per_exec = Scatter::new(x_vals.clone(), stats.gets_per_exec.clone())
            .mode(Mode::Markers)
            .name("# Buffer Gets")
            .marker(
                Marker::new()
                    .color(colors[12])
                    .symbol(MarkerSymbol::StarDiamond)
                    .opacity(0.7),
            )
            .x_axis("x1")
            .y_axis("y4")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_gets_per_exec);

        let sql_phy_r_exec = Scatter::new(x_vals.clone(), stats.phy_r_exec.clone())
            .mode(Mode::Markers)
            .name("# Physical Reads")
            .marker(
                Marker::new()
                    .color(colors[11])
                    .symbol(MarkerSymbol::DiamondTall)
                    .opacity(0.7),
            )
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
            .marker(
                Marker::new()
                    .color(colors[7])
                    .symbol(MarkerSymbol::DiamondTall)
                    .opacity(0.7),
            )
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_cpu_t_pct_total);

        // IO Time as % of Total IO Wait
        let sql_io_pct_total = Scatter::new(x_vals.clone(), stats.io_pct_total.clone())
            .mode(Mode::Markers)
            .name("% IO Time as DB IO Wait")
            .marker(
                Marker::new()
                    .color(colors[6])
                    .symbol(MarkerSymbol::StarDiamond)
                    .opacity(0.7),
            )
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_io_pct_total);

        // Buffer Gets as % of Total
        let sql_gets_pct_total = Scatter::new(x_vals.clone(), stats.gets_pct_total.clone())
            .mode(Mode::Markers)
            .name("% Gets as Total Gets")
            .marker(
                Marker::new()
                    .color(colors[5])
                    .symbol(MarkerSymbol::Diamond)
                    .opacity(0.7),
            )
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_gets_pct_total);

        // Physical Reads as % of Total Disk Reads
        let sql_phy_r_pct_total = Scatter::new(x_vals.clone(), stats.phy_r_pct_total.clone())
            .mode(Mode::Markers)
            .name("% Phys Reads as Total Disk Reads")
            .marker(
                Marker::new()
                    .color(colors[4])
                    .symbol(MarkerSymbol::Square)
                    .opacity(0.7),
            )
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
            .marker(
                Marker::new()
                    .color(colors[2])
                    .symbol(MarkerSymbol::DiamondTall)
                    .opacity(0.7),
            )
            .x_axis("x1")
            .y_axis("y2")
            .visible(Visible::LegendOnly);
        sql_plot.add_trace(sql_cpu_time_exec_s);

        let sql_io_time_exec_s = Scatter::new(x_vals.clone(), stats.io_time_exec_s.clone())
            .mode(Mode::Markers)
            .name("(s) User I/O Wait Time")
            .marker(
                Marker::new()
                    .color(colors[1])
                    .symbol(MarkerSymbol::StarDiamond)
                    .opacity(0.7),
            )
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
            .grid(LayoutGrid::new().rows(1).columns(1))
            .x_axis(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("y1")
                    .range(vec![0.])
                    .show_grid(true),
            )
            .y_axis(
                Axis::new()
                    .domain(&[0.0, 0.23])
                    .anchor("x1")
                    .range(vec![0.])
                    .title("#")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero),
            )
            .y_axis2(
                Axis::new()
                    .domain(&[0.25, 0.48])
                    .anchor("x1")
                    .range(vec![0.])
                    .title("(s) per Exec")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero),
            )
            .y_axis3(
                Axis::new()
                    .domain(&[0.5, 0.73])
                    .anchor("x1")
                    .range(vec![0.])
                    .title("%")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero),
            )
            .y_axis4(
                Axis::new()
                    .domain(&[0.75, 1.0])
                    .anchor("x1")
                    .range(vec![0.])
                    .title("#")
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero),
            );
        sql_plot.set_layout(sql_layout);
        let file_name: String = format!("{}/sqlid/sqlid_{}.html", dirpath, &sql);
        let path: &Path = Path::new(&file_name);
        sql_plot.write_html(path);
    }
    println!("Saved plots for SQLs to '{}/sqlid/sqlid_*'", dirpath);
}

// Generate HTML for Instance Efficiency
pub(crate) fn generate_instance_efficiency_plot(
    awrs: &Vec<AWR>,
    snap_range: &(u64, u64),
    dirpath: &str,
) -> String {
    let (f_begin_snap, f_end_snap) = snap_range;
    struct InstEffStats {
        stat_name: String,
        stat_pct: Vec<Option<f32>>,
    }
    // Use the same selected snapshots for every series and its time axis.
    let selected: Vec<&AWR> = awrs
        .iter()
        .filter(|awr| {
            awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap
        })
        .collect();
    let mut names = std::collections::BTreeSet::new();
    for awr in &selected {
        for metric in &awr.instance_efficiency {
            names.insert(metric.eff_stat.clone());
        }
    }
    let x_vals: Vec<String> = selected
        .iter()
        .map(|awr| {
            format!(
                "{} ({})",
                awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id
            )
        })
        .collect();
    let inst_eff_stats: Vec<InstEffStats> = names
        .into_iter()
        .map(|name| {
            // Preserve a gap for each absent measurement so later values cannot shift left.
            let values = selected
                .iter()
                .map(|awr| {
                    awr.instance_efficiency
                        .iter()
                        .find(|metric| metric.eff_stat == name)
                        .and_then(|metric| metric.eff_pct)
                })
                .collect();
            InstEffStats {
                stat_name: name,
                stat_pct: values,
            }
        })
        .collect();

    // === Create the instance efficiency plot ===
    let mut plot_instance_efficiency = Plot::new();
    let color_palette = vec![
        "#1f77b4", "#ff7f0e", "#2ca02c", "#d62728", "#9467bd", "#8c564b", "#e377c2", "#7f7f7f",
        "#bcbd22", "#17becf",
    ];
    for (i, ieplot) in inst_eff_stats.iter().enumerate() {
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

        let box_plot = BoxPlot::new_xy(
            ieplot.stat_pct.clone(),
            vec![ieplot.stat_name.clone(); ieplot.stat_pct.clone().len()],
        )
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
        .grid(LayoutGrid::new().rows(3).columns(1))
        .x_axis(
            Axis::new()
                .domain(&[0.0, 1.0])
                .anchor("y1")
                .range(vec![0.])
                .show_grid(true),
        )
        .y_axis(
            Axis::new()
                .title("Efficiency (%)")
                .domain(&[0.0, 0.3])
                .anchor("x1")
                .range(vec![0.])
                .zero_line(true)
                .range_mode(RangeMode::ToZero),
        )
        .x_axis2(
            Axis::new()
                .title("Hit %")
                .domain(&[0.0, 1.0])
                .anchor("y2")
                .range(vec![0.])
                .show_grid(true),
        )
        .y_axis2(
            Axis::new()
                .domain(&[0.35, 0.75])
                .anchor("x2")
                .range(vec![0.]),
        )
        .y_axis3(
            Axis::new()
                .domain(&[0.8, 1.0])
                .anchor("x2")
                .range(vec![0.])
                .show_tick_labels(false),
        );

    plot_instance_efficiency.set_layout(layout);
    plot_instance_efficiency.to_inline_html(Some("instance-efficiency-plot"))
}

pub(crate) fn generate_instance_stats_plotfiles(
    awrs: &Vec<AWR>,
    snap_range: &(u64, u64),
    dirpath: &str,
) {
    let (f_begin_snap, f_end_snap) = snap_range;
    struct InstStats {
        stat_name: String,
        stat_total: Vec<Option<u64>>,
    }
    let mut i_stats_names: Vec<String> = awrs[0]
        .instance_stats
        .iter()
        .map(|s| s.statname.clone())
        .collect();

    let mut x_vals: Vec<String> = awrs
        .iter()
        .filter(|awr| {
            awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap
        })
        .map(|awr| {
            format!(
                "{} ({})",
                awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id
            )
        })
        .collect();

    let inst_stats: Vec<InstStats> = i_stats_names
        .par_iter()
        .map(|i_name| {
            // Collect values across all matching AWRs in range
            let mut values: Vec<Option<u64>> = Vec::new();
            for awr in awrs {
                if awr.snap_info.begin_snap_id >= *f_begin_snap
                    && awr.snap_info.end_snap_id <= *f_end_snap
                {
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
        })
        .collect();

    // === Create the instance stats plots ===
    for (i, iplot) in inst_stats.iter().enumerate() {
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

        let box_plot = BoxPlot::new_xy(
            iplot.stat_total.clone(),
            vec![iplot.stat_name.clone(); iplot.stat_total.clone().len()],
        )
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
            .grid(LayoutGrid::new().rows(3).columns(1))
            .x_axis(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("y1")
                    .range(vec![0.])
                    .show_grid(true),
            )
            .y_axis(
                Axis::new()
                    .title("Total Number")
                    .domain(&[0.0, 0.3])
                    .anchor("x1")
                    .range(vec![0.])
                    .zero_line(true)
                    .range_mode(RangeMode::ToZero),
            )
            .x_axis2(
                Axis::new()
                    .title("Total Number")
                    .domain(&[0.0, 1.0])
                    .anchor("y2")
                    .range(vec![0.])
                    .show_grid(true),
            )
            .y_axis2(
                Axis::new()
                    .domain(&[0.35, 0.75])
                    .anchor("x2")
                    .range(vec![0.]),
            )
            .y_axis3(
                Axis::new()
                    .domain(&[0.8, 1.0])
                    .anchor("x2")
                    .range(vec![0.])
                    .show_tick_labels(false),
            );

        plot_instance_stat.set_layout(layout);
        // Save to HTML
        let file_name = get_safe_filename(iplot.stat_name.clone(), "inst_stat".to_string());
        let path = Path::new(&dirpath).join(&file_name);
        plot_instance_stat.write_html(path);
    }
    println!(
        "Saved plots for Instance Stats to '{}/stats/stats_*'",
        &dirpath
    );
}

pub(crate) fn generate_iostats_plotfile(
    awrs: &Vec<AWR>,
    snap_range: &(u64, u64),
    dirpath: &str,
) -> BTreeMap<String, BTreeMap<String, (f64, f64)>> {
    let (f_begin_snap, f_end_snap) = snap_range;

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
    struct IOStats {
        reads_data: Vec<f64>, // in MB
        reads_req_s: Vec<f64>,
        reads_data_s: Vec<f64>, // in MB
        writes_data: Vec<f64>,  // in MB
        writes_req_s: Vec<f64>,
        writes_data_s: Vec<f64>, // in MB
        waits_count: Vec<u64>,
        avg_time: Vec<Option<f64>>, // in ms
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
            },
        );
    }
    //let mut functions_to_plot: Vec<String> = Vec::new();
    let mut functions_to_plot: BTreeMap<String, BTreeMap<String, (f64, f64)>> = BTreeMap::new();
    let mut x_vals: Vec<String> = Vec::new();
    let mut plot_iostats_main: Plot = Plot::new();

    for awr in awrs {
        if awr.snap_info.begin_snap_id < *f_begin_snap || awr.snap_info.begin_snap_id > *f_end_snap
        {
            continue;
        }
        x_vals.push(format!(
            "{} ({})",
            awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id
        ));
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
            let mean_waits_count =
                mean(stats.waits_count.iter().map(|&x| x as f64).collect()).unwrap_or(0.0);

            // Check if any metric has a non-zero mean
            let has_empty_data =
                mean_reads_data == 0.0 && mean_writes_data == 0.0 && mean_waits_count == 0.0;

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

                let std_waits_count =
                    std_deviation(stats.waits_count.iter().map(|&x| x as f64).collect())
                        .unwrap_or(0.0);

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
                stats_map.insert(
                    "reads_req_s".to_string(),
                    (mean_reads_req_s, std_reads_req_s),
                );
                stats_map.insert(
                    "reads_data_s".to_string(),
                    (mean_reads_data_s, std_reads_data_s),
                );
                stats_map.insert(
                    "writes_data".to_string(),
                    (mean_writes_data, std_writes_data),
                );
                stats_map.insert(
                    "writes_req_s".to_string(),
                    (mean_writes_req_s, std_writes_req_s),
                );
                stats_map.insert(
                    "writes_data_s".to_string(),
                    (mean_writes_data_s, std_writes_data_s),
                );
                stats_map.insert(
                    "waits_count".to_string(),
                    (mean_waits_count, std_waits_count),
                );
                stats_map.insert("avg_time".to_string(), (mean_avg_time, std_avg_time));

                // Insert into the main HashMap
                functions_to_plot.insert(func.to_string(), stats_map);
            }
        }
    }

    for func in functions_to_plot.keys() {
        let mut plot_iostats: Plot = Plot::new();
        let reads_data = BoxPlot::new(io_stats_byfunc[func].reads_data.clone())
            .name("Reads Data(MB)")
            .x_axis("x1")
            .y_axis("y1")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#003366".to_string())
                    .opacity(0.7)
                    .size(2),
            );
        let reads_req_s = BoxPlot::new(io_stats_byfunc[func].reads_req_s.clone())
            .name("ReadsReq/s")
            .x_axis("x2")
            .y_axis("y2")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#0066CC".to_string())
                    .opacity(0.7)
                    .size(2),
            );
        let reads_data_s = BoxPlot::new(io_stats_byfunc[func].reads_data_s.clone())
            .name("Reads Data(MB)/s")
            .x_axis("x3")
            .y_axis("y3")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#66B2FF".to_string())
                    .opacity(0.7)
                    .size(2),
            );
        let writes_data = BoxPlot::new(io_stats_byfunc[func].writes_data.clone())
            .name("Writes Data(MB)")
            .x_axis("x4")
            .y_axis("y4")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#800000".to_string())
                    .opacity(0.7)
                    .size(2),
            );
        let writes_req_s = BoxPlot::new(io_stats_byfunc[func].writes_req_s.clone())
            .name("WritesReq/s")
            .x_axis("x5")
            .y_axis("y5")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#FF0000".to_string())
                    .opacity(0.7)
                    .size(2),
            );
        let writes_data_s = BoxPlot::new(io_stats_byfunc[func].writes_data_s.clone())
            .name("Writes Data(MB)/s")
            .x_axis("x6")
            .y_axis("y6")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#FF6666".to_string())
                    .opacity(0.7)
                    .size(2),
            );
        let waits_count = BoxPlot::new(io_stats_byfunc[func].waits_count.clone())
            .name("Waits Count")
            .x_axis("x7")
            .y_axis("y7")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#FF8800".to_string())
                    .opacity(0.7)
                    .size(2),
            );
        let avg_time = BoxPlot::new(io_stats_byfunc[func].avg_time.clone())
            .name("Avg Time(ms)")
            .x_axis("x8")
            .y_axis("y8")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#00AA00".to_string())
                    .opacity(0.7)
                    .size(2),
            );
        let reads_data_scat =
            Scatter::new(x_vals.clone(), io_stats_byfunc[func].reads_data.clone())
                .mode(Mode::Lines)
                .name(format!("{} Reads Data", func))
                //.marker(Marker::new().color(colors[12]))
                .x_axis("x1")
                .y_axis("y1");
        let writes_data_scat =
            Scatter::new(x_vals.clone(), io_stats_byfunc[func].writes_data.clone())
                .mode(Mode::Lines)
                .name(format!("{} Writes Data", func))
                //.marker(Marker::new().color(colors[12]))
                .x_axis("x1")
                .y_axis("y1");
        let reads_req_s_scat =
            Scatter::new(x_vals.clone(), io_stats_byfunc[func].reads_req_s.clone())
                .mode(Mode::Lines)
                .name(format!("{} Reads Req/s", func))
                //.marker(Marker::new().color(colors[12]))
                .x_axis("x1")
                .y_axis("y3");
        let writes_req_s_scat =
            Scatter::new(x_vals.clone(), io_stats_byfunc[func].writes_req_s.clone())
                .mode(Mode::Lines)
                .name(format!("{} Writes Req/s", func))
                //.marker(Marker::new().color(colors[12]))
                .x_axis("x1")
                .y_axis("y3");
        let reads_data_s_scat =
            Scatter::new(x_vals.clone(), io_stats_byfunc[func].reads_data_s.clone())
                .mode(Mode::Lines)
                .name(format!("{} Reads Data/s", func))
                //.marker(Marker::new().color(colors[12]))
                .x_axis("x1")
                .y_axis("y2");
        let writes_data_s_scat =
            Scatter::new(x_vals.clone(), io_stats_byfunc[func].writes_data_s.clone())
                .mode(Mode::Lines)
                .name(format!("{} Writes Data/s", func))
                //.marker(Marker::new().color(colors[12]))
                .x_axis("x1")
                .y_axis("y2");
        let waits_count_scat =
            Scatter::new(x_vals.clone(), io_stats_byfunc[func].waits_count.clone())
                .mode(Mode::Lines)
                .name(format!("{} Wait Count", func))
                //.marker(Marker::new().color(colors[12]))
                .x_axis("x1")
                .y_axis("y4");
        let avg_time_scat = Scatter::new(x_vals.clone(), io_stats_byfunc[func].avg_time.clone())
            .mode(Mode::Lines)
            .name(format!("{} Wait AVG Time", func))
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
                LayoutGrid::new().rows(1).columns(1),
                //.row_order(Grid::TopToBottom),
            )
            .hover_mode(HoverMode::X)
            .x_axis8(
                Axis::new()
                    .domain(&[0.91, 1.0])
                    .anchor("y8")
                    .range(vec![0.])
                    .show_grid(false),
            )
            .y_axis8(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x8")
                    .range(vec![0.])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
            )
            .x_axis7(
                Axis::new()
                    .domain(&[0.78, 0.87])
                    .anchor("y7")
                    .range(vec![0.])
                    .show_grid(false),
            )
            .y_axis7(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x7")
                    .range(vec![0.])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
            )
            .x_axis6(
                Axis::new()
                    .domain(&[0.65, 0.74])
                    .anchor("y6")
                    .range(vec![0.])
                    .show_grid(false),
            )
            .y_axis6(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x6")
                    .range(vec![0.])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
            )
            .x_axis5(
                Axis::new()
                    .domain(&[0.52, 0.61])
                    .anchor("y5")
                    .range(vec![0.])
                    .show_grid(false),
            )
            .y_axis5(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x5")
                    .range(vec![0.])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
            )
            .x_axis4(
                Axis::new()
                    .domain(&[0.39, 0.48])
                    .anchor("y4")
                    .range(vec![0.])
                    .show_grid(false),
            )
            .y_axis4(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x4")
                    .range(vec![0.])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
            )
            .x_axis3(
                Axis::new()
                    .domain(&[0.26, 0.35])
                    .anchor("y3")
                    .range(vec![0.])
                    .show_grid(false),
            )
            .y_axis3(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x3")
                    .range(vec![0.])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
            )
            .x_axis2(
                Axis::new()
                    .domain(&[0.13, 0.22])
                    .anchor("y2")
                    .range(vec![0.])
                    .show_grid(false),
            )
            .y_axis2(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x2")
                    .range(vec![0.])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
            )
            .x_axis(
                Axis::new()
                    .domain(&[0.0, 0.09])
                    .anchor("y1")
                    .range(vec![0.])
                    .show_grid(false),
            )
            .y_axis(
                Axis::new()
                    .domain(&[0.0, 1.0])
                    .anchor("x1")
                    .title(func)
                    .range(vec![0.])
                    .range_mode(RangeMode::ToZero)
                    .show_grid(false),
            );
        plot_iostats.set_layout(layout_iostats);
        let func_name = func.replace(" ", "_");
        let file_name: String = format!("{}/iostats/iostats_{}.html", dirpath, func_name);
        let path: &Path = Path::new(&file_name);
        plot_iostats.write_html(path);
    }

    let layout_io_stats_main: Layout = Layout::new()
        .height(800)
        .hover_mode(HoverMode::X)
        .grid(LayoutGrid::new().rows(5).columns(1))
        .x_axis(
            Axis::new()
                .domain(&[0.0, 1.0])
                .anchor("y5")
                .range(vec![0.])
                .show_grid(true),
        )
        .y_axis(
            Axis::new()
                .domain(&[0.82, 1.0])
                .anchor("x1")
                .range(vec![0.])
                .title("MB")
                .zero_line(true)
                .range_mode(RangeMode::ToZero),
        )
        .y_axis2(
            Axis::new()
                .domain(&[0.62, 0.80])
                .anchor("x1")
                .range(vec![0.])
                .title("MB/s")
                .zero_line(true)
                .range_mode(RangeMode::ToZero),
        )
        .y_axis3(
            Axis::new()
                .domain(&[0.41, 0.60])
                .anchor("x1")
                .range(vec![0.])
                .title("Req/s")
                .zero_line(true)
                .range_mode(RangeMode::ToZero),
        )
        .y_axis4(
            Axis::new()
                .domain(&[0.21, 0.39])
                .anchor("x1")
                .range(vec![0.])
                .title("#")
                .zero_line(true)
                .range_mode(RangeMode::ToZero),
        )
        .y_axis5(
            Axis::new()
                .domain(&[0.0, 0.19])
                .anchor("x1")
                .range(vec![0.])
                .title("ms")
                .zero_line(true)
                .range_mode(RangeMode::ToZero),
        );
    plot_iostats_main.set_layout(layout_io_stats_main);
    let file_name: String = format!("{}/iostats/iostats_zMAIN.html", dirpath);
    let path: &Path = Path::new(&file_name);
    plot_iostats_main.write_html(path);

    println!("Saving plots for IO Stats to '{}/iostats_*'", dirpath);
    functions_to_plot.insert("zMAIN".to_string(), BTreeMap::new());
    functions_to_plot
}

// Get Requests – latch popularity (how often it was used).
// Pct Get Miss – how often a process failed to acquire the latch (indicates contention).
// Avg Slps/Miss – whether processes had to sleep (values above zero indicate costly contention).
// Wait Time (s) – the total system cost (aggregate time lost).
// NoWait Requests / Pct NoWait Miss – usually less critical, but can expose brief bottlenecks.
pub(crate) fn generate_latchstats_plotfiles(
    awrs: &Vec<AWR>,
    snap_range: &(u64, u64),
    dirpath: &str,
    report_for_ai: &mut ReportForAI,
) -> Table {
    let (f_begin_snap, f_end_snap) = snap_range;
    #[derive(Default)]
    struct LatchAgg {
        get_requests_sum: u64,
        weighted_miss_pct: f64, // sum(get_requests * get_pct_miss)
        occurrences: f64,
        wait_time_sum: f64,
    }

    let mut latch_stat_rows = String::new(); //for HTML
    let mut latches: Vec<String> = Vec::new(); //latch names
    for lname in &awrs[0].latch_activity {
        latches.push(lname.statname.clone());
    }

    let latch_activity: HashMap<String, LatchAgg> = latches
        .par_iter()
        .map(|lname| {
            let mut agg: LatchAgg = LatchAgg::default();
            for awr in awrs {
                if awr.snap_info.begin_snap_id >= *f_begin_snap
                    && awr.snap_info.end_snap_id <= *f_end_snap
                {
                    for la in awr.latch_activity.iter().filter(|la| la.statname == *lname) {
                        if la.get_requests > 0 {
                            agg.get_requests_sum =
                                agg.get_requests_sum.saturating_add(la.get_requests);
                            agg.weighted_miss_pct += (la.get_requests as f64) * la.get_pct_miss;
                            agg.wait_time_sum += (la.get_requests as f64) * la.wait_time;
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
        })
        .collect();

    let mut sorted_latches: Vec<(String, LatchAgg)> = latch_activity.into_iter().collect();
    sorted_latches.sort_by(|a, b| {
        b.1.weighted_miss_pct
            .partial_cmp(&a.1.weighted_miss_pct)
            .unwrap()
    });

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
                Cell::new(&format!(
                    "{:.2}",
                    (agg.get_requests_sum as f64 / agg.occurrences as f64)
                )),
                Cell::new(&format!("{:.4}", agg.weighted_miss_pct)),
                Cell::new(&format!("{:.2}", agg.wait_time_sum)),
                Cell::new(&format!(
                    "{:.2}",
                    (agg.occurrences as f64 * 100.0 / awrs.len() as f64)
                )),
            ]));
            latch_activity.latch_name = lname.clone();
            latch_activity.get_requests_avg = agg.get_requests_sum as f64 / agg.occurrences as f64;
            latch_activity.weighted_miss_pct = agg.weighted_miss_pct;
            latch_activity.wait_time_weighted_avg_s = agg.wait_time_sum;
            latch_activity.found_in_pct_of_probes =
                (agg.occurrences as f64 * 100.0 / awrs.len() as f64);

            report_for_ai.latch_activity_summary.push(latch_activity);

            latch_stat_rows.push_str(&format!(
                r#"<tr>
                    <td>{}</td>
                    <td>{:.2}</td>
                    <td>{:.4}</td>
                    <td>{:.2}</td>
                    <td>{:.2}</td>
                </tr>"#,
                lname,
                (agg.get_requests_sum as f64 / agg.occurrences as f64),
                agg.weighted_miss_pct,
                agg.wait_time_sum,
                (agg.occurrences as f64 * 100.0 / awrs.len() as f64)
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

pub(crate) fn report_segments_summary(
    awrs: &Vec<AWR>,
    args: &Args,
    logfile_name: &str,
    dir: &str,
    raport_for_ai: &mut ReportForAI,
) -> Vec<String> {
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
        sections_toplot.push(objects[0].stat_name.replace(" ", "_"));
        let section_msg = format!("TOP 10 Segments by {} ordered by PCT of occuriance desc. Statstic values computed based on {}\n", section, objects[0].stat_name);
        make_notes!(logfile_name, args.quiet, 0, "\n");
        make_notes!(logfile_name, args.quiet, 2, "{}", section_msg.yellow());
        let mut table = Table::new();
        let mut segment_stat_rows = String::new();

        if args.security_level > 0 {
            //In security level 0 there is no segment name
            table.set_titles(Row::new(vec![
                Cell::new("Segment Name"),
                Cell::new("Segment Type"),
                Cell::new("Object Id"),
                Cell::new("Data Object Id"),
                Cell::new("AVG"),
                Cell::new("STDDEV"),
                Cell::new("PCT of occuriance"),
            ]));
        } else {
            table.set_titles(Row::new(vec![
                Cell::new("Segment Type"),
                Cell::new("Object Id"),
                Cell::new("Data Object Id"),
                Cell::new("AVG"),
                Cell::new("STDDEV"),
                Cell::new("PCT of occuriance"),
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
        let all_ids: HashSet<(u64, u64, String)> = objects
            .iter()
            .map(|s| (s.obj, s.objd, s.object_name.clone()))
            .collect();

        for id in all_ids {
            //iterate over each object id
            let mut all_values: Vec<f64> = Vec::new();

            objects.iter().for_each(|s| {
                if s.obj == id.0 && s.objd == id.1 && s.object_name == id.2 {
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

            let pct_key: i64 = (pct * -100000.0) as i64; //this will be used to sort BTree over PCT - it will negative number to get the biggest value on top

            segment_summary.insert(
                (pct_key, id.0, id.1),
                SegmentSummary {
                    segment_name: segment_data.object_name.clone(),
                    segment_type: segment_data.object_type.clone(),
                    object_id: id.0.to_string(),
                    data_object_id: id.1.to_string(),
                    stat_name: segment_data.stat_name.clone(),
                    avg: format!("{:.3}", avg),
                    stddev: format!("{:.3}", stddev),
                    pct: format!("{:.3}", pct),
                },
            );
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
                raport_for_ai
                    .top_10_segments_by_buffer_busy_waits
                    .push(segment_data);
            } else if section == "Direct Physical Reads" {
                raport_for_ai
                    .top_10_segments_by_direct_physical_reads
                    .push(segment_data);
            } else if section == "Direct Physical Writes" {
                raport_for_ai
                    .top_10_segments_by_direct_physical_writes
                    .push(segment_data);
            } else if section == "Logical Reads" {
                raport_for_ai
                    .top_10_segments_by_logical_reads
                    .push(segment_data);
            } else if section == "Physical Read Requests" {
                raport_for_ai
                    .top_10_segments_by_physical_read_requests
                    .push(segment_data);
            } else if section == "Physical Write Requests" {
                raport_for_ai
                    .top_10_segments_by_physical_write_requests
                    .push(segment_data);
            } else if section == "Physical Writes" {
                raport_for_ai
                    .top_10_segments_by_physical_writes
                    .push(segment_data);
            } else if section == "Row Lock Waits" {
                raport_for_ai
                    .top_10_segments_by_row_lock_waits
                    .push(segment_data);
            }

            if args.security_level > 0 {
                table.add_row(Row::new(vec![
                    Cell::new(&s.segment_name),
                    Cell::new(&s.segment_type),
                    Cell::new(&s.object_id),
                    Cell::new(&s.data_object_id),
                    Cell::new(&s.avg),
                    Cell::new(&s.stddev),
                    Cell::new(&s.pct),
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
                    &s.segment_name,
                    &s.segment_type,
                    &s.object_id,
                    &s.data_object_id,
                    &s.avg,
                    &s.stddev,
                    &s.pct
                ));
            } else {
                table.add_row(Row::new(vec![
                    Cell::new(&s.segment_type),
                    Cell::new(&s.object_id),
                    Cell::new(&s.data_object_id),
                    Cell::new(&s.avg),
                    Cell::new(&s.stddev),
                    Cell::new(&s.pct),
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
                    &s.segment_type, &s.object_id, &s.data_object_id, &s.avg, &s.stddev, &s.pct
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
                idname = objects[0].stat_name.replace(" ", "_"),
                rows = segment_stat_rows
            );
            let segment_stats_filename: String = format!(
                "{}/segstats/segstats_{}.html",
                dir,
                objects[0].stat_name.replace(" ", "_"),
            );
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
                idname = objects[0].stat_name.replace(" ", "_"),
                rows = segment_stat_rows
            );
            let segment_stats_filename: String = format!(
                "{}/segstats/segstats_{}.html",
                dir,
                objects[0].stat_name.replace(" ", "_"),
            );
            if let Err(e) = fs::write(&segment_stats_filename, table_segment_stat) {
                eprintln!("Error writing file {}: {}", segment_stats_filename, e);
            }
        };
    }
    sections_toplot
}
