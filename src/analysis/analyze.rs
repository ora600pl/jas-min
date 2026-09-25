use crate::awr::{
    AWRSCollection, GetStats, HostCPU, IOStats, LoadProfile, SQLCPUTime, SQLGets, SQLIOTime,
    SQLReads, SegmentStats, WaitEvents, AWR,
};
use crate::measurements::{self, DbLoadMetric};
use crate::staticdata::*;

use serde::{Deserialize, Serialize};

//use axum::http::header;
use execute::generic_array::typenum::True;
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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::format;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use colored::*;
use open::*;

use crate::anomalies::AnomalySummaryItem;
use crate::anomalies::*;
use crate::{anomalies, Args};
use regex::*;

use crate::debug_note;
use crate::make_notes;
use prettytable::{format, Attr, Cell, Row, Table};
use rayon::prelude::*;

use crate::reasonings::{
    strip_gradient_descriptions, AnomalyDescription, AnomlyCluster, CollinearGroupImpact,
    DbTimeGradientSection, GradientSettings, GradientTopItem, IOStatsByFunctionSummary,
    InstanceStatisticCorrelation, LatchActivitySummary, LoadProfileAnomalies, MadAnomaliesEvents,
    MadAnomaliesSQL, PctOfTimesThisSQLFoundInOtherTopSections, ReportForAI, StatisticsDescription,
    StatsSummary, Top10SegmentStats, TopBackgroundWaitEvents, TopForegroundWaitEvents,
    TopPeaksSelected, TopSQLsByElapsedTime, VifDiagnostic, WaitEventsFromASH,
    WaitEventsWithStrongCorrelation,
};
use crate::report::classic::*;
use crate::tools::*;

use crate::degradation::{
    build_db_time_degradation_html, build_db_time_degradation_report,
    find_degraded_sqls_for_analysis,
};
use crate::gradient::*;
use crate::gradient::{
    DbTimeGradientResult, EventImpact, EventScalarMap, EventSeriesMap, GradientHtmlSection,
    GradientSectionSpec,
};

use crate::staticdata::StatUnitGroup;

pub(crate) struct TopStats {
    pub(crate) events: BTreeMap<String, u8>,
    pub(crate) bgevents: BTreeMap<String, u8>,
    pub(crate) sqls: BTreeMap<String, String>, // <SQL_ID, Module>
    pub(crate) sqls_cpu: BTreeMap<String, String>, // <SQL_ID, Module>
    pub(crate) stat_names: BTreeMap<String, u8>,
    pub(crate) event_anomalies_mad: HashMap<String, Vec<(String, f64)>>,
    pub(crate) bgevent_anomalies_mad: HashMap<String, Vec<(String, f64)>>,
    pub(crate) sql_elapsed_time_anomalies_mad: HashMap<String, Vec<(String, f64)>>,
}

// Check if snap_range argument is passed correctly
fn parse_snap_range(snap_range: &str) -> Result<(u64, u64), String> {
    let parts: Vec<&str> = snap_range.split('-').collect();
    if parts.len() != 2 {
        return Err(format!(
            "Invalid format for snap_range '{}'. Expected format: BEGIN_ID-END_ID",
            snap_range
        ));
    }
    let begin = parts[0]
        .parse::<u64>()
        .map_err(|_| format!("Invalid number for BEGIN_ID in '{}'", snap_range))?;
    let end = parts[1]
        .parse::<u64>()
        .map_err(|_| format!("Invalid number for END_ID in '{}'", snap_range))?;

    if begin >= end {
        return Err(format!(
            "BEGIN_ID ({}) must be less than END_ID ({})",
            begin, end
        ));
    }
    Ok((begin, end))
}

//We don't want to plot everything, because it would cause to much trouble
//we need to find only essential wait events and SQLIDs
fn find_top_stats(
    awrs: &Vec<AWR>,
    db_time_cpu_ratio: f64,
    filter_db_time: f64,
    snap_range: &(u64, u64),
    logfile_name: &str,
    args: &Args,
    report_for_ai: &mut ReportForAI,
) -> TopStats {
    let mut event_names: BTreeMap<String, u8> = BTreeMap::new();
    let mut bgevent_names: BTreeMap<String, u8> = BTreeMap::new();
    let mut sql_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut sql_ids_cpu: BTreeMap<String, String> = BTreeMap::new();
    let mut stat_names: BTreeMap<String, u8> = BTreeMap::new();

    let mut stats_description = StatisticsDescription::default();

    //so we scan the AWR data
    make_notes!(
        &logfile_name,
        false,
        1,
        "{}",
        "DBCPU/DBTIME RATIO ANALYSIS".bold().green()
    );
    make_notes!(&logfile_name, false, 0,
        "\nPeaks are being analyzed based on specified ratio (default 0.666).\nThe ratio is beaing calculated as DB CPU / DB Time.\nThe lower the ratio the more sessions are waiting for resources other than CPU.\nIf DB CPU = 2 and DB Time = 8 it means that on AVG 8 actice sessions are working but only 2 of them are actively working on CPU.\nCurrent ratio used to find peak periods is {}\n\n", db_time_cpu_ratio);

    stats_description.dbcpu_dbtime = format!("DBCPU/DBTIME RATIO ANALYSIS\nPeaks are being analyzed based on specified ratio (default 0.666).\nThe ratio is beaing calculated as DB CPU / DB Time.\nThe lower the ratio the more sessions are waiting for resources other than CPU.\nIf DB CPU = 2 and DB Time = 8 it means that on AVG 8 actice sessions are working but only 2 of them are actively working on CPU.\nCurrent ratio used to find peak periods is {}", db_time_cpu_ratio);

    let mut full_window_size = ((args.mad_window_size as f32 / 100.0) * awrs.len() as f32) as usize; // Default is 20% of probes
    if full_window_size % 2 == 1 {
        full_window_size = full_window_size + 1;
    }
    make_notes!(
        &logfile_name,
        false,
        1,
        "{}",
        "MEDIAN ABSOLUTE DEVIATION".bold().green()
    );
    make_notes!(
        &logfile_name,
        false,
        0,
        "\nMAD score threshold = 7.0\nMAD top = {}\nMAD window size={}% ({} of probes out of {})\n\n",
        args.mad_top,
        args.mad_window_size,
        full_window_size,
        awrs.len()
    );

    stats_description.median_absolute_deviation = format!(
        "MAD score threshold = 7.0\nMAD top = {}\nMAD window size={}% ({} of probes out of {})\n\n",
        args.mad_top,
        args.mad_window_size,
        full_window_size,
        awrs.len()
    );

    let mut top_spikes: Vec<TopPeaksSelected> = Vec::new();
    let (f_begin_snap, f_end_snap) = snap_range;
    for awr in awrs {
        if awr.snap_info.begin_snap_id >= *f_begin_snap && awr.snap_info.end_snap_id <= *f_end_snap
        {
            let dbtime =
                measurements::db_load_seconds(awr, DbLoadMetric::DbTime).unwrap_or(f64::NAN);
            let cputime =
                measurements::db_load_seconds(awr, DbLoadMetric::DbCpu).unwrap_or(f64::NAN);
            let dbtime_filter = measurements::db_load_rate(awr, DbLoadMetric::DbTime)
                .map(|r| r.per_second)
                .unwrap_or(f64::NAN);
            //If proportion of cputime and dbtime is less then db_time_cpu_ratio (default 0.666) than we want to find out what might be the problem
            //because it means that Oracle spent some time waiting on wait events and not working on CPU

            if dbtime > 0.0
                && cputime > 0.0
                && cputime / dbtime < db_time_cpu_ratio
                && (filter_db_time == 0.0 || dbtime_filter > filter_db_time)
            {
                //println!("Analyzing a peak in {} ({}) for ratio: [{:.2}/{:.2}] = {:.2}", awr.file_name, awr.snap_info.begin_snap_time, cputime, dbtime, (cputime/dbtime));
                make_notes!(
                    &logfile_name,
                    false,
                    0,
                    "Analyzing a peak in {} ({}) for ratio: [{:.2}/{:.2}] = {:.2}\n",
                    awr.file_name,
                    awr.snap_info.begin_snap_time,
                    cputime,
                    dbtime,
                    (cputime / dbtime)
                );

                top_spikes.push(TopPeaksSelected {
                    report_name: awr.file_name.clone(),
                    report_date: awr.snap_info.begin_snap_time.clone(),
                    snap_id: awr.snap_info.begin_snap_id,
                    db_time_value: dbtime,
                    db_cpu_value: cputime,
                    dbcpu_dbtime_ratio: (cputime / dbtime),
                });

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
                        event_names
                            .entry(events[fg_length - i].event.clone())
                            .or_insert(1);
                    }
                }
                if bg_length > 10 {
                    for i in 1..11 {
                        bgevent_names
                            .entry(bgevents[bg_length - i].event.clone())
                            .or_insert(1);
                    }
                }
                //And the same with SQLs
                let mut sqls: Vec<crate::awr::SQLElapsedTime> = awr.sql_elapsed_time.clone();
                sqls.sort_by_key(|s| s.elapsed_time_s as i64);
                let l: usize = sqls.len();
                if l > 5 {
                    for i in 1..6 {
                        sql_ids
                            .entry(sqls[l - i].sql_id.clone())
                            .or_insert(sqls[l - i].sql_module.clone());
                    }
                } else if l > 1 && l <= 5 {
                    for i in 0..=l - 1 {
                        sql_ids
                            .entry(sqls[i].sql_id.clone())
                            .or_insert(sqls[i].sql_module.clone());
                    }
                }

                //And the same with SQLs by CPU
                let mut sqls_cpu: Vec<crate::awr::SQLCPUTime> =
                    awr.sql_cpu_time.iter().map(|s| s.1.clone()).collect();

                sqls_cpu.sort_by_key(|s| s.cpu_time_s as i64);
                let l: usize = sqls_cpu.len();
                if l > 5 {
                    for i in 1..6 {
                        sql_ids_cpu
                            .entry(sqls_cpu[l - i].sql_id.clone())
                            .or_insert(sqls_cpu[l - i].sql_module.clone());
                    }
                } else if l > 1 && l <= 5 {
                    for i in 0..=l - 1 {
                        sql_ids_cpu
                            .entry(sqls_cpu[i].sql_id.clone())
                            .or_insert(sqls_cpu[i].sql_module.clone());
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

    let awrs: Vec<AWR> = awrs
        .clone()
        .iter()
        .filter(|a| {
            a.snap_info.begin_snap_id >= *f_begin_snap && a.snap_info.end_snap_id <= *f_end_snap
        })
        .cloned()
        .collect();

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

    if !args.id_sqls.is_empty() {
        let sqlids: Vec<String> = args
            .id_sqls
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        println!("Additional SQLs ID considered: {:?}", sqlids.clone());
        for s in sqlids {
            sql_ids.entry(s).or_insert(String::new());
        }
    }

    let top: TopStats = TopStats {
        events: event_names,
        bgevents: bgevent_names,
        sqls: sql_ids,
        sqls_cpu: sql_ids_cpu,
        stat_names: stat_names,
        event_anomalies_mad: event_anomalies,
        bgevent_anomalies_mad: bgevent_anomalies,
        sql_elapsed_time_anomalies_mad: sql_anomalies,
    };

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
    let sql_cpu: Vec<&HashMap<String, SQLCPUTime>> = awrs
        .iter()
        .map(|awr| &awr.sql_cpu_time)
        .filter(|sql| sql.contains_key(sqlid))
        .collect();

    let sql_cpu_count: f64 = sql_cpu.len() as f64;

    //Filter HashMaps of SQL ordered by User IO to find how many times the given sqlid was marked in top section
    let sql_io: Vec<&HashMap<String, SQLIOTime>> = awrs
        .iter()
        .map(|awr| &awr.sql_io_time)
        .filter(|sql| sql.contains_key(sqlid))
        .collect();
    let sql_io_count: f64 = sql_io.len() as f64;

    //Filter HashMaps of SQL ordered by GETS to find how many times the given sqlid was marked in top section
    let sql_gets: Vec<&HashMap<String, SQLGets>> = awrs
        .iter()
        .map(|awr| &awr.sql_gets)
        .filter(|sql| sql.contains_key(sqlid))
        .collect();
    let sql_gets_count: f64 = sql_gets.len() as f64;

    //Filter HashMaps of SQL ordered by READS to find how many times the given sqlid was marked in top section
    let sql_reads: Vec<&HashMap<String, SQLReads>> = awrs
        .iter()
        .map(|awr| &awr.sql_reads)
        .filter(|sql| sql.contains_key(sqlid))
        .collect();
    let sql_reads_count: f64 = sql_reads.len() as f64;

    let mut top_sections: HashMap<String, f64> = HashMap::new();
    top_sections.insert("SQL CPU".to_string(), sql_cpu_count / probe_size * 100.0);
    top_sections.insert("SQL I/O".to_string(), sql_io_count / probe_size * 100.0);
    top_sections.insert("SQL GETS".to_string(), sql_gets_count / probe_size * 100.0);
    top_sections.insert(
        "SQL READS".to_string(),
        sql_reads_count / probe_size * 100.0,
    );

    // If Statspack modify top_sections accordingly
    if is_statspack {
        top_sections.remove("SQL I/O"); // Remove SQL I/O if Statspack is enabled
    }

    top_sections
}

fn report_instance_stats_cor(
    instance_stats: HashMap<String, Vec<f64>>,
    dbtime_vec: Vec<f64>,
) -> (BTreeMap<(i64, String), f64>, f64) {
    let mut sorted_correlation: BTreeMap<(i64, String), f64> = BTreeMap::new();
    let num_stats = instance_stats.len();
    let r_threshold = bonferroni_significance_threshold(num_stats, 0.05, dbtime_vec.len());
    // Use max(0.5, r_threshold)
    let effective_threshold = r_threshold.max(0.5);

    for (k, v) in &instance_stats {
        if v.len() == dbtime_vec.len() {
            let crr = pearson_correlation_2v(&v, &dbtime_vec);
            if crr >= effective_threshold || crr <= effective_threshold * -1.0 {
                sorted_correlation.insert(((crr * 1000.0) as i64, k.clone()), crr);
            }
        } else {
            println!(
                "Can't calculate correlation for {} - diff was {}",
                &k,
                dbtime_vec.len() - v.len()
            );
        }
    }
    (sorted_correlation, effective_threshold)
}

//Add SQL_IDs found in ASH to event charts
/// Where the value for a tracked stat comes from in each AWR snapshot.
#[derive(Debug, Clone, Copy)]
pub enum StatSource {
    /// Matches an Oracle statname in `awr.instance_stats` (exact match).
    InstanceStatExact(&'static str),
    /// Matches an Oracle statname in `awr.instance_stats` by prefix.
    InstanceStatPrefix(&'static str),
    /// Matches a load-profile row by `starts_with`. Uses `per_second`.
    LoadProfilePerSec(&'static str),
    /// Matches a load-profile row by `starts_with`. Uses `per_second` multiplied
    /// by the given factor (e.g. block_size / (1024*1024) for MB/s).
    LoadProfilePerSecScaled(&'static str, f64),
    /// Single scalar per snapshot, provided by the caller (e.g. redo log switches).
    Scalar,
    /// Computed per snapshot by a closure (e.g. excessive-commit ratio).
    Computed,
}

/// Stable identifiers for stats tracked by the main report.
/// Adding a new tracked series = 1 variant + 1 spec row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackedStatKey {
    // --- Load Profile ---
    UserCallsPerSec,
    TransactionsPerSec,
    ExecutesPerSec,
    BlockChangesPerSec,
    RedoMbPerSec,
    LogicalReadMbPerSec,
    PhysReadMbPerSec,
    PhysWriteMbPerSec,
    ParsesPerSec,
    HardParsesPerSec,
    // --- Instance stats (cumulative totals) ---
    UserCommits,
    UserRollbacks,
    FailedParseCount,
    UserLogons,
    UserLogouts,
    UserCalls,
    CleanoutKtugct,
    CleanoutCr,
    TableScanRows,
    TableScanBlocks,
    TableFetchRowid,
    TableFetchContRow,
    // --- Derived / external scalars ---
    RedoLogSwitches,
    ExcessiveCommits,
}

pub struct TrackedStatSpec {
    pub key: TrackedStatKey,
    pub display_name: &'static str,
    pub unit: &'static str, // printed in the tooltip after the absolute value
    pub source: StatSource,
    /// When `true`, the stat is only collected/plotted if `log file sync` is a TOP event.
    pub requires_logfilesync: bool,
}

pub fn tracked_stats_specs() -> &'static [TrackedStatSpec] {
    &[
        // --- Load Profile (rates) ---
        TrackedStatSpec {
            key: TrackedStatKey::UserCallsPerSec,
            display_name: "User Calls/s",
            unit: "calls/s",
            source: StatSource::LoadProfilePerSec("User calls"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::TransactionsPerSec,
            display_name: "Transactions/s",
            unit: "tx/s",
            source: StatSource::LoadProfilePerSec("Transactions"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::ExecutesPerSec,
            display_name: "Executes/s",
            unit: "exec/s",
            source: StatSource::LoadProfilePerSec("Executes"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::BlockChangesPerSec,
            display_name: "Block changes/s",
            unit: "blk/s",
            source: StatSource::LoadProfilePerSec("Block changes"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::ParsesPerSec,
            display_name: "Parses/s",
            unit: "parse/s",
            source: StatSource::LoadProfilePerSec("Parses"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::HardParsesPerSec,
            display_name: "Hard Parses/s",
            unit: "hparse/s",
            source: StatSource::LoadProfilePerSec("Hard parses"),
            requires_logfilesync: false,
        },
        // --- Load Profile (scaled to MB/s using block size) ---
        // The scale factor is filled in at runtime (block_size / 1024 / 1024); see build_tracked_stats().
        TrackedStatSpec {
            key: TrackedStatKey::RedoMbPerSec,
            display_name: "Redo MB/s",
            unit: "MB/s",
            source: StatSource::LoadProfilePerSecScaled("Redo size", 1.0 / (1024.0 * 1024.0)),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::LogicalReadMbPerSec,
            display_name: "Logical Read MB/s",
            unit: "MB/s",
            source: StatSource::LoadProfilePerSecScaled(
                "Logical read",
                0.0, /* filled at runtime */
            ),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::PhysReadMbPerSec,
            display_name: "Physical Read MB/s",
            unit: "MB/s",
            source: StatSource::LoadProfilePerSecScaled("Physical read", 0.0),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::PhysWriteMbPerSec,
            display_name: "Physical Write MB/s",
            unit: "MB/s",
            source: StatSource::LoadProfilePerSecScaled("Physical write", 0.0),
            requires_logfilesync: false,
        },
        // --- Instance stats (absolute totals per snap) ---
        TrackedStatSpec {
            key: TrackedStatKey::UserCommits,
            display_name: "User Commits",
            unit: "#",
            source: StatSource::InstanceStatExact("user commits"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::UserRollbacks,
            display_name: "User Rollbacks",
            unit: "#",
            source: StatSource::InstanceStatExact("user rollbacks"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::FailedParseCount,
            display_name: "Failed Parses",
            unit: "#",
            source: StatSource::InstanceStatExact("parse count (failures)"),
            requires_logfilesync: true,
        },
        TrackedStatSpec {
            key: TrackedStatKey::UserLogons,
            display_name: "User Logons",
            unit: "#",
            source: StatSource::InstanceStatExact("user logons cumulative"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::UserLogouts,
            display_name: "User Logouts",
            unit: "#",
            source: StatSource::InstanceStatExact("user logouts cumulative"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::UserCalls,
            display_name: "User Calls (cum.)",
            unit: "#",
            source: StatSource::InstanceStatExact("user calls"),
            requires_logfilesync: true,
        },
        TrackedStatSpec {
            key: TrackedStatKey::CleanoutKtugct,
            display_name: "Cleanout KTUGCT",
            unit: "#",
            source: StatSource::InstanceStatPrefix("cleanout - number of ktugct calls"),
            requires_logfilesync: true,
        },
        TrackedStatSpec {
            key: TrackedStatKey::CleanoutCr,
            display_name: "Cleanouts CR Only",
            unit: "#",
            source: StatSource::InstanceStatPrefix("cleanouts only - consistent read"),
            requires_logfilesync: true,
        },
        TrackedStatSpec {
            key: TrackedStatKey::TableFetchContRow,
            display_name: "Table Fetch Cont Row",
            unit: "#",
            source: StatSource::InstanceStatExact("table fetch continued row"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::TableFetchRowid,
            display_name: "Table Fetch by Rowid",
            unit: "#",
            source: StatSource::InstanceStatExact("table fetch by rowid"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::TableScanBlocks,
            display_name: "Table Scan Blocks",
            unit: "#",
            source: StatSource::InstanceStatExact("table scan blocks gotten"),
            requires_logfilesync: false,
        },
        TrackedStatSpec {
            key: TrackedStatKey::TableScanRows,
            display_name: "Table Scan Rows (scan activity; not live rows)",
            unit: "#",
            source: StatSource::InstanceStatExact("table scan rows gotten"),
            requires_logfilesync: false,
        },
        // --- Externally fed scalars ---
        TrackedStatSpec {
            key: TrackedStatKey::RedoLogSwitches,
            display_name: "Redo Log Switches/h",
            unit: "/h",
            source: StatSource::Scalar,
            requires_logfilesync: true,
        },
        TrackedStatSpec {
            key: TrackedStatKey::ExcessiveCommits,
            display_name: "Excessive Commits",
            unit: "ratio",
            source: StatSource::Computed,
            requires_logfilesync: true,
        },
    ]
}

#[derive(Debug, Clone, Default)]
pub struct TrackedStat {
    pub display_name: String,
    pub unit: String,
    pub values: Vec<f64>,
}

impl TrackedStat {
    pub fn new(display_name: &str, unit: &str) -> Self {
        Self {
            display_name: display_name.to_string(),
            unit: unit.to_string(),
            values: Vec::new(),
        }
    }

    pub fn push(&mut self, v: f64) {
        self.values.push(v);
    }
    pub fn z_scores(&self) -> Vec<f64> {
        z_score_normalize(&self.values)
    }
    pub fn normalized(&self) -> Vec<f64> {
        log1p_robust_minmax_0_100(&self.values, 5.0, 95.0)
    }
    pub fn raw_data(&self) -> Vec<f64> {
        self.values.clone()
    }

    pub fn custom_data(&self) -> Vec<Vec<f64>> {
        let avg = mean(self.values.clone()).unwrap_or(0.0);
        let sd = std_deviation(self.values.clone()).unwrap_or(0.0);
        self.values.iter().map(|v| vec![*v, avg, sd]).collect()
    }

    /// Per-point custom data, one preformatted string per snapshot.
    /// Each row contains "Absolute: X unit<br>Mean: Y unit<br>StdDev: Z unit".
    /// This works around the Rust plotly crate limitation where `customdata`
    /// must be a flat collection of scalars/strings (not a 2D matrix).
    pub fn custom_data_strings(&self) -> Vec<String> {
        let avg = mean(self.values.clone()).unwrap_or(0.0);
        let sd = std_deviation(self.values.clone()).unwrap_or(0.0);
        let unit = &self.unit;

        self.values
            .iter()
            .map(|v| {
                format!(
                    "Absolute: {:.2} {unit}<br>Mean: {:.2} {unit}<br>StdDev: {:.2} {unit}",
                    v,
                    avg,
                    sd,
                    unit = unit
                )
            })
            .collect()
    }
}

/// Build an empty registry containing **all** tracked stats.
/// Filtering by `is_logfilesync_high` happens later, at plot time.
pub fn build_tracked_stats() -> HashMap<TrackedStatKey, TrackedStat> {
    tracked_stats_specs()
        .iter()
        .map(|s| (s.key, TrackedStat::new(s.display_name, s.unit)))
        .collect()
}

/// Look up a spec by key.
pub fn spec_of(key: TrackedStatKey) -> Option<&'static TrackedStatSpec> {
    tracked_stats_specs().iter().find(|s| s.key == key)
}

/// Feed one AWR's instance_stats into the registry.
/// Returns the set of TrackedStatKeys that were matched (useful for side-effects).
pub fn ingest_instance_stats(
    registry: &mut HashMap<TrackedStatKey, TrackedStat>,
    awr: &AWR,
) -> HashMap<TrackedStatKey, f64> {
    let mut matched: HashMap<TrackedStatKey, f64> = HashMap::new();
    for activity in &awr.instance_stats {
        for spec in tracked_stats_specs() {
            let hit = match spec.source {
                StatSource::InstanceStatExact(name) => activity.statname == name,
                StatSource::InstanceStatPrefix(pref) => activity.statname.starts_with(pref),
                _ => false,
            };
            if hit {
                if let Some(ts) = registry.get_mut(&spec.key) {
                    let v = activity.total as f64;
                    ts.push(v);
                    matched.insert(spec.key, v);
                }
            }
        }
    }
    matched
}

/// Feed one AWR's load_profile rows into the registry.
/// `block_size_bytes` is used to finalize MB/s scaling factors.
pub fn ingest_load_profile(
    registry: &mut HashMap<TrackedStatKey, TrackedStat>,
    awr: &AWR,
    block_size_bytes: u64,
) {
    let mb_factor = block_size_bytes as f64 / 1024.0 / 1024.0;

    for lp in &awr.load_profile {
        for spec in tracked_stats_specs() {
            let value_opt = match spec.source {
                StatSource::LoadProfilePerSec(prefix) if lp.stat_name.starts_with(prefix) => {
                    Some(lp.per_second)
                }
                StatSource::LoadProfilePerSecScaled(prefix, factor)
                    if lp.stat_name.starts_with(prefix) =>
                {
                    // For "Physical read/write" and "Logical read" we want MB/s => multiply by block size.
                    // For "Redo size" we want MB/s from raw bytes/s => 1/(1024*1024).
                    let f = match spec.key {
                        TrackedStatKey::RedoMbPerSec => 1.0 / (1024.0 * 1024.0),
                        _ => mb_factor,
                    };
                    Some(lp.per_second * f)
                }
                _ => None,
            };
            if let Some(v) = value_opt {
                if let Some(ts) = registry.get_mut(&spec.key) {
                    ts.push(v);
                }
            }
        }
    }
}

/// Push a pre-computed scalar into the registry.
pub fn push_scalar(
    registry: &mut HashMap<TrackedStatKey, TrackedStat>,
    key: TrackedStatKey,
    value: f64,
) {
    if let Some(ts) = registry.get_mut(&key) {
        ts.push(value);
    }
}

/// Render every tracked stat on y2, honoring the `requires_logfilesync` gate.
fn add_tracked_stat_traces(
    plot: &mut Plot,
    x_vals: &[String],
    tracked_stats: &HashMap<TrackedStatKey, TrackedStat>,
    is_logfilesync_high: bool,
) {
    for spec in tracked_stats_specs() {
        // Honor the "only show when log file sync is high" gate at render time,
        // not at build time — the flag may not be final until we enter plotting.
        if spec.requires_logfilesync && !is_logfilesync_high {
            continue;
        }

        let Some(stat) = tracked_stats.get(&spec.key) else {
            continue;
        };
        if stat.values.is_empty() {
            continue;
        }

        let hover = "<b>%{fullData.name}</b><br>\
             Snap: %{x}<br>\
             Normalized Value: %{y:.2f}<br>\
             %{customdata}\
             <extra></extra>";

        let trace = Scatter::new(x_vals.to_vec(), stat.raw_data())
            .mode(Mode::Lines)
            .name(stat.display_name.clone())
            .custom_data(stat.custom_data_strings())
            .hover_template(hover)
            .x_axis("x1")
            .y_axis("y2")
            .visible(Visible::LegendOnly);

        plot.add_trace(trace);
    }
}

/// Return the raw values of a tracked stat, or an empty slice if missing.
/// Box plots need absolute values (not Z-Scores), so use this accessor.
pub fn raw_values_of(
    tracked_stats: &HashMap<TrackedStatKey, TrackedStat>,
    key: TrackedStatKey,
) -> Vec<f64> {
    tracked_stats
        .get(&key)
        .map(|ts| ts.values.clone())
        .unwrap_or_default()
}

pub fn main_report_builder(
    collection: &AWRSCollection,
    args: Args,
    events_sqls: HashMap<&str, HashSet<String>>,
) -> ReportForAI {
    let mut plot_main: Plot = Plot::new();
    let mut plot_highlight: Plot = Plot::new();
    let mut plot_highlight2: Plot = Plot::new();
    let mut global_statistics = BTreeMap::<String, Option<GetStats>>::new();

    /* Struct filled for AI analyzes in JSON */
    let mut report_for_ai: ReportForAI = ReportForAI::default();
    /* ************************************* */

    let db_time_cpu_ratio: f64 = args.time_cpu_ratio;
    let filter_db_time: f64 = args.filter_db_time;
    let snap_range: (u64, u64) =
        parse_snap_range(&args.snap_range).expect("Invalid snap-range argument");
    debug_note!(
        "Starting main report build: snapshots={}, snap_range={}-{}, directory='{}', json_file='{}'",
        collection.awrs.len(),
        snap_range.0,
        snap_range.1,
        args.directory(),
        args.json_file()
    );

    report_for_ai.db_load_sources = measurements::db_load_sources(&collection, &snap_range);

    //Filenames and Paths used to save JAS-MIN files
    let mut logfile_name = PathBuf::from(args.directory())
        .with_extension("txt")
        .to_string_lossy()
        .into_owned();
    if logfile_name.is_empty() && !args.json_file().is_empty() {
        if let Some(stem) = PathBuf::from(args.json_file()).file_stem() {
            logfile_name = PathBuf::from(stem)
                .with_extension("txt")
                .to_string_lossy()
                .into_owned();
        }
    }
    let logfile_path = Path::new(&logfile_name);
    println!("Starting output capture to: {}", logfile_path.display());
    if logfile_path.exists() {
        //remove logfile if it exists - the notes made by JAS-MIN has to be created each time
        fs::remove_file(&logfile_path).unwrap();
    }

    let mut html_dir = PathBuf::from(args.directory())
        .with_extension("html_reports")
        .to_string_lossy()
        .into_owned();
    if html_dir.is_empty() && !args.json_file().is_empty() {
        if let Some(stem) = PathBuf::from(args.json_file()).file_stem() {
            html_dir = PathBuf::from(stem)
                .with_extension("html_reports")
                .to_string_lossy()
                .into_owned();
        }
    }
    // Create main <PATH>.html_reports folder
    if let Err(e) = fs::create_dir_all(&html_dir) {
        eprintln!("⚠️ Failed to create base directory {:?}: {}", html_dir, e);
    }
    // Create all required subdirectories dir tree under html
    let subdirs = [
        "fg",
        "bg",
        "latches",
        "iostats",
        "segstats",
        "sqlid",
        "stats",
        "jasmin/anomalies",
    ];
    for sub in subdirs {
        let path = Path::new(&html_dir).join(sub);
        if let Err(e) = fs::create_dir_all(&path) {
            eprintln!("⚠️ Failed to create directory {:?}: {}", path, e);
        }
    }
    // Y-axis
    let y_vals_dbtime =
        measurements::db_load_series(&collection, &snap_range, DbLoadMetric::DbTime);
    let y_vals_dbcpu = measurements::db_load_series(&collection, &snap_range, DbLoadMetric::DbCpu);
    let mut y_vals_events: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_bgevents: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_sqls_cpu: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut y_vals_cpu_user: Vec<Option<f64>> = Vec::new();
    let mut y_vals_cpu_load: Vec<Option<f64>> = Vec::new();
    let mut y_vals_cpu_count: Vec<Option<u32>> = Vec::new();
    let mut tracked_stats = build_tracked_stats();

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

    println!("{}", "\n==== ANALYZING ===".bold().bright_cyan());
    let mut top_stats: TopStats = find_top_stats(
        &collection.awrs,
        db_time_cpu_ratio,
        filter_db_time,
        &snap_range,
        &logfile_name,
        &args,
        &mut report_for_ai,
    );
    let degraded_sqls_for_analysis =
        find_degraded_sqls_for_analysis(&collection, &snap_range, args.top_gradient);
    if !degraded_sqls_for_analysis.is_empty() {
        println!(
            "Additional SQLs from DB Time degradation considered: {:?}",
            degraded_sqls_for_analysis
                .iter()
                .map(|(sql_id, _)| sql_id.clone())
                .collect::<Vec<String>>()
        );
        for (sql_id, module) in degraded_sqls_for_analysis {
            top_stats.sqls.entry(sql_id).or_insert(module);
        }
    }
    debug_note!(
        "Top-stat selection completed: fg_waits={}, bg_waits={}, sql_elapsed={}, sql_cpu={}, instance_stats={}",
        top_stats.events.len(),
        top_stats.bgevents.len(),
        top_stats.sqls.len(),
        top_stats.sqls_cpu.len(),
        top_stats.stat_names.len()
    );

    println!("{}", "\n==== CREATING PLOTS ===".bold().bright_cyan());
    generate_events_plotfiles(
        &collection.awrs,
        &top_stats.events,
        true,
        &snap_range,
        &html_dir,
    );
    generate_events_plotfiles(
        &collection.awrs,
        &top_stats.bgevents,
        false,
        &snap_range,
        &html_dir,
    );
    generate_sqls_plotfiles(&collection.awrs, &top_stats, &snap_range, &html_dir);
    let instance_eff_plot: String =
        generate_instance_efficiency_plot(&collection.awrs, &snap_range, &html_dir);
    generate_instance_stats_plotfiles(&collection.awrs, &snap_range, &html_dir);
    let iostats = generate_iostats_plotfile(&collection.awrs, &snap_range, &html_dir);
    let table_latch: Table =
        generate_latchstats_plotfiles(&collection.awrs, &snap_range, &html_dir, &mut report_for_ai);
    let fname: String = format!("{}/jasmin_main.html", &html_dir); //new file name path for main report

    println!("\n{}", "==== PREPARING RESULTS ===".bold().bright_cyan());

    let is_logfilesync_high: bool = top_stats.events.keys().any(|e| e == "log file sync");

    for awr in &collection.awrs {
        let (f_begin_snap, f_end_snap) = snap_range;
        if awr.snap_info.begin_snap_id >= f_begin_snap && awr.snap_info.end_snap_id <= f_end_snap {
            let xval: String = format!(
                "{} ({})",
                awr.snap_info.begin_snap_time, awr.snap_info.begin_snap_id
            );
            x_vals.push(xval.clone());
            //We have to fill the whole data traces for stats, wait events and SQLs with 0 to be sure that chart won't be moved to one side

            for (sql, _) in &top_stats.sqls {
                y_vals_sqls.entry(sql.to_string()).or_insert(Vec::new());
                y_vals_sqls_exec_t
                    .entry(sql.to_string())
                    .or_insert(Vec::new());
                y_vals_sqls_exec_n
                    .entry(sql.to_string())
                    .or_insert(Vec::new());
                y_vals_sqls_exec_s
                    .entry(sql.to_string())
                    .or_insert(Vec::new());
                let mut v = y_vals_sqls.get_mut(sql).unwrap();
                v.push(0.0);
            }

            for (sql, _) in &top_stats.sqls_cpu {
                y_vals_sqls_cpu.entry(sql.to_string()).or_insert(Vec::new());
                let mut v = y_vals_sqls_cpu.get_mut(sql).unwrap();
                v.push(0.0);
            }

            for (event, _) in &top_stats.events {
                y_vals_events.entry(event.to_string()).or_insert(Vec::new());
                y_vals_events_n
                    .entry(event.to_string())
                    .or_insert(Vec::new());
                y_vals_events_t
                    .entry(event.to_string())
                    .or_insert(Vec::new());
                y_vals_events_s
                    .entry(event.to_string())
                    .or_insert(Vec::new());
                let mut v = y_vals_events.get_mut(event).unwrap();
                v.push(0.0);
            }

            for (event, _) in &top_stats.bgevents {
                y_vals_bgevents
                    .entry(event.to_string())
                    .or_insert(Vec::new());
                y_vals_bgevents_n
                    .entry(event.to_string())
                    .or_insert(Vec::new());
                y_vals_bgevents_t
                    .entry(event.to_string())
                    .or_insert(Vec::new());
                y_vals_bgevents_s
                    .entry(event.to_string())
                    .or_insert(Vec::new());
                let mut v = y_vals_bgevents.get_mut(event).unwrap();
                v.push(0.0);
            }

            for (statname, _) in &top_stats.stat_names {
                instance_stats
                    .entry(statname.to_string())
                    .or_insert(Vec::new());
                let mut v = instance_stats.get_mut(statname).unwrap();
                v.push(0.0);
            }

            //Than we can set the current value of the vector to the desired one, if the event is in TOP section in that snap id
            for event in &awr.foreground_wait_events {
                if top_stats.events.contains_key(&event.event) {
                    let mut v = y_vals_events.get_mut(&event.event).unwrap();
                    v[x_vals.len() - 1] = event.total_wait_time_s;
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
                    v[x_vals.len() - 1] = event.total_wait_time_s;
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
                    v[x_vals.len() - 1] = sqls.elapsed_time_s;
                    let mut v = y_vals_sqls_exec_t.get_mut(&sqls.sql_id).unwrap();
                    v.push(sqls.elpased_time_exec_s);
                    let mut v = y_vals_sqls_exec_n.get_mut(&sqls.sql_id).unwrap();
                    v.push(sqls.executions as f64);
                    let mut v = y_vals_sqls_exec_s.get_mut(&sqls.sql_id).unwrap();
                    v.push(sqls.elapsed_time_s as f64);
                }
            }
            for sqls in &awr.sql_cpu_time {
                if top_stats.sqls_cpu.contains_key(&sqls.1.sql_id) {
                    let mut v = y_vals_sqls_cpu.get_mut(&sqls.1.sql_id).unwrap();
                    v[x_vals.len() - 1] = sqls.1.cpu_time_s;
                }
            }

            ingest_load_profile(
                &mut tracked_stats,
                awr,
                collection.db_instance_information.db_block_size as u64,
            );

            // IO Stats data gathering and preparing them for plotting
            // ----- Host CPU
            let cpu_available = crate::measurements::host_cpu_available(awr);
            y_vals_cpu_user.push(cpu_available.then_some(awr.host_cpu.pct_user));
            y_vals_cpu_load.push(cpu_available.then_some(100.0 - awr.host_cpu.pct_idle));
            y_vals_cpu_count
                .push((cpu_available && awr.host_cpu.cpus > 0).then_some(awr.host_cpu.cpus));

            let mut calls: u64 = 0;
            let mut commits: u64 = 0;
            let mut rollbacks: u64 = 0;
            let mut cleanout_ktugct: u64 = 0;
            let mut cleanout_cr: u64 = 0;
            let mut excessive_commit: f64 = 0.0;

            let matched = ingest_instance_stats(&mut tracked_stats, awr);

            for activity in &awr.instance_stats {
                let mut v: &mut Vec<f64> = instance_stats.get_mut(&activity.statname).unwrap();
                v[x_vals.len() - 1] = activity.total as f64;
            }

            // log-file-sync–dependent pushes
            if is_logfilesync_high {
                push_scalar(
                    &mut tracked_stats,
                    TrackedStatKey::RedoLogSwitches,
                    awr.redo_log.per_hour,
                );

                let calls = matched
                    .get(&TrackedStatKey::UserCalls)
                    .copied()
                    .unwrap_or(0.0);
                let commits = matched
                    .get(&TrackedStatKey::UserCommits)
                    .copied()
                    .unwrap_or(0.0);
                let rollbacks = matched
                    .get(&TrackedStatKey::UserRollbacks)
                    .copied()
                    .unwrap_or(0.0);
                let excessive = if commits + rollbacks > 0.0 {
                    calls / (commits + rollbacks)
                } else {
                    0.0
                };
                push_scalar(
                    &mut tracked_stats,
                    TrackedStatKey::ExcessiveCommits,
                    excessive,
                );
            }
        }
    }

    debug_note!(
        "Aligned analysis series built: snapshots={}, db_time={}, db_cpu={}, fg_wait_series={}, bg_wait_series={}, sql_series={}, sql_cpu_series={}, instance_stat_series={}",
        x_vals.len(),
        y_vals_dbtime.len(),
        y_vals_dbcpu.len(),
        y_vals_events.len(),
        y_vals_bgevents.len(),
        y_vals_sqls.len(),
        y_vals_sqls_cpu.len(),
        instance_stats.len()
    );

    //I want to sort wait events by most heavy ones across the whole period
    let mut y_vals_events_sorted = BTreeMap::new();
    for (evname, ev) in y_vals_events.clone() {
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
    for (sqlid, yv) in y_vals_sqls.clone() {
        let mut occuriance = 0;
        for v in &yv {
            if *v > 0.0 {
                occuriance -= 1;
            }
        }
        y_vals_sqls_sorted.insert((occuriance, sqlid.clone()), yv.clone());
    }
    //Get Global Stats for Lod Profile
    global_statistics.insert(
        "CPU Load".to_string(),
        get_statistics(y_vals_cpu_load.iter().flatten().copied().collect()),
    );
    global_statistics.insert(
        "AAS".to_string(),
        get_statistics(
            y_vals_dbtime
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .collect(),
        ),
    );
    global_statistics.insert(
        "Executions/s".to_string(),
        get_statistics(raw_values_of(
            &tracked_stats,
            TrackedStatKey::ExecutesPerSec,
        )),
    );
    global_statistics.insert(
        "Transactions/s".to_string(),
        get_statistics(raw_values_of(
            &tracked_stats,
            TrackedStatKey::TransactionsPerSec,
        )),
    );
    global_statistics.insert(
        "Physical Reads MB/s".to_string(),
        get_statistics(raw_values_of(
            &tracked_stats,
            TrackedStatKey::PhysReadMbPerSec,
        )),
    );
    global_statistics.insert(
        "Physical Writes MB/s".to_string(),
        get_statistics(raw_values_of(
            &tracked_stats,
            TrackedStatKey::PhysWriteMbPerSec,
        )),
    );
    global_statistics.insert(
        "Redo MB/s".to_string(),
        get_statistics(raw_values_of(&tracked_stats, TrackedStatKey::RedoMbPerSec)),
    );
    global_statistics.insert(
        "User Commits/snap".to_string(),
        get_statistics(raw_values_of(&tracked_stats, TrackedStatKey::UserCommits)),
    );
    global_statistics.insert(
        "User Rollbacks/snap".to_string(),
        get_statistics(raw_values_of(&tracked_stats, TrackedStatKey::UserRollbacks)),
    );
    global_statistics.insert(
        "Parses/s".to_string(),
        get_statistics(raw_values_of(&tracked_stats, TrackedStatKey::ParsesPerSec)),
    );
    global_statistics.insert(
        "Hard Parses/s".to_string(),
        get_statistics(raw_values_of(
            &tracked_stats,
            TrackedStatKey::HardParsesPerSec,
        )),
    );
    global_statistics.insert(
        "Logical Reads MB/s".to_string(),
        get_statistics(raw_values_of(
            &tracked_stats,
            TrackedStatKey::LogicalReadMbPerSec,
        )),
    );
    global_statistics.insert(
        "Block Changes/s".to_string(),
        get_statistics(raw_values_of(
            &tracked_stats,
            TrackedStatKey::BlockChangesPerSec,
        )),
    );
    global_statistics.insert(
        "User Calls/s".to_string(),
        get_statistics(raw_values_of(
            &tracked_stats,
            TrackedStatKey::UserCallsPerSec,
        )),
    );
    fs::write(
        format!("{}/stats/global_statistics.json", &html_dir),
        serde_json::to_string(&global_statistics).unwrap(),
    );

    // ------ Ploting and reporting starts ----------
    make_notes!(&logfile_name, args.quiet, 0, "\n\n");
    make_notes!(
        &logfile_name,
        false,
        1,
        "{}\n",
        "STATISTICAL COMPUTATION RESULTS".bold().green()
    );

    let dbtime_trace = Scatter::new(x_vals.clone(), y_vals_dbtime.clone())
        .mode(Mode::LinesText)
        .name("DB Time (s/s)")
        .x_axis("x1")
        .y_axis("y1");

    let dbcpu_trace = Scatter::new(x_vals.clone(), y_vals_dbcpu.clone())
        .mode(Mode::LinesText)
        .name("DB CPU (s/s)")
        .x_axis("x1")
        .y_axis("y1");

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

    let cpu_load_box_plot = BoxPlot::new(y_vals_cpu_load)
        //.mode(Mode::LinesText)
        .name("CPU Load %")
        .x_axis("x1")
        .y_axis("y1")
        .box_mean(BoxMean::True)
        .show_legend(false)
        .box_points(BoxPoints::All)
        .whisker_width(0.2)
        .marker(
            Marker::new()
                .color("#9c2d2d".to_string())
                .opacity(0.7)
                .size(2),
        );

    let aas_box_plot = BoxPlot::new(y_vals_dbtime.clone())
        .name("AAS")
        .x_axis("x2")
        .y_axis("y2")
        .box_mean(BoxMean::True)
        .show_legend(false)
        .box_points(BoxPoints::All)
        .whisker_width(0.2)
        .marker(
            Marker::new()
                .color("#2d9c57".to_string())
                .opacity(0.7)
                .size(2),
        );

    let exec_box_plot = BoxPlot::new(raw_values_of(
        &tracked_stats,
        TrackedStatKey::ExecutesPerSec,
    ))
    .name("Exec/s")
    .x_axis("x3")
    .y_axis("y3")
    .box_mean(BoxMean::True)
    .show_legend(false)
    .box_points(BoxPoints::All)
    .whisker_width(0.2)
    .marker(
        Marker::new()
            .color("#2d5d9c".to_string())
            .opacity(0.7)
            .size(2),
    );

    let trans_box_plot = BoxPlot::new(raw_values_of(
        &tracked_stats,
        TrackedStatKey::TransactionsPerSec,
    ))
    .name("Trans/s")
    .x_axis("x4")
    .y_axis("y4")
    .box_mean(BoxMean::True)
    .show_legend(false)
    .box_points(BoxPoints::All)
    .whisker_width(0.2)
    .marker(
        Marker::new()
            .color("#904fc2".to_string())
            .opacity(0.7)
            .size(2),
    );

    let read_mb_box_plot = BoxPlot::new(raw_values_of(
        &tracked_stats,
        TrackedStatKey::PhysReadMbPerSec,
    ))
    .name("Phy Reads MB/s")
    .x_axis("x5")
    .y_axis("y5")
    .box_mean(BoxMean::True)
    .show_legend(false)
    .box_points(BoxPoints::All)
    .whisker_width(0.2)
    .marker(
        Marker::new()
            .color("#c2be4f".to_string())
            .opacity(0.7)
            .size(2),
    );

    let write_mb_box_plot = BoxPlot::new(raw_values_of(
        &tracked_stats,
        TrackedStatKey::PhysWriteMbPerSec,
    ))
    .name("Phy Writes MB/s")
    .x_axis("x6")
    .y_axis("y6")
    .box_mean(BoxMean::True)
    .show_legend(false)
    .box_points(BoxPoints::All)
    .whisker_width(0.2)
    .marker(
        Marker::new()
            .color("#c2904f".to_string())
            .opacity(0.7)
            .size(2),
    );

    let redo_mb_box_plot =
        BoxPlot::new(raw_values_of(&tracked_stats, TrackedStatKey::RedoMbPerSec))
            .name("Redo MB/s")
            .x_axis("x7")
            .y_axis("y7")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#ad247a".to_string())
                    .opacity(0.7)
                    .size(2),
            );

    let user_commits_box_plot =
        BoxPlot::new(raw_values_of(&tracked_stats, TrackedStatKey::UserCommits))
            .name("User Commits/snap")
            .x_axis("x1")
            .y_axis("y1")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#fa3434".to_string())
                    .opacity(0.7)
                    .size(2),
            );

    let user_rollbacks_box_plot =
        BoxPlot::new(raw_values_of(&tracked_stats, TrackedStatKey::UserRollbacks))
            .name("User Rollbacks/snap")
            .x_axis("x2")
            .y_axis("y2")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#11fa7a".to_string())
                    .opacity(0.7)
                    .size(2),
            );

    let user_parses_box_plot =
        BoxPlot::new(raw_values_of(&tracked_stats, TrackedStatKey::ParsesPerSec))
            .name("Parses/s")
            .x_axis("x3")
            .y_axis("y3")
            .box_mean(BoxMean::True)
            .show_legend(false)
            .box_points(BoxPoints::All)
            .whisker_width(0.2)
            .marker(
                Marker::new()
                    .color("#006aff".to_string())
                    .opacity(0.7)
                    .size(2),
            );

    let user_hparses_box_plot = BoxPlot::new(raw_values_of(
        &tracked_stats,
        TrackedStatKey::HardParsesPerSec,
    ))
    .name("Hard Parses/s")
    .x_axis("x4")
    .y_axis("y4")
    .box_mean(BoxMean::True)
    .show_legend(false)
    .box_points(BoxPoints::All)
    .whisker_width(0.2)
    .marker(
        Marker::new()
            .color("#9000ff".to_string())
            .opacity(0.7)
            .size(2),
    );

    let logical_reads_box_plot = BoxPlot::new(raw_values_of(
        &tracked_stats,
        TrackedStatKey::LogicalReadMbPerSec,
    ))
    .name("Logical Reads (MB)/s")
    .x_axis("x5")
    .y_axis("y5")
    .box_mean(BoxMean::True)
    .show_legend(false)
    .box_points(BoxPoints::All)
    .whisker_width(0.2)
    .marker(
        Marker::new()
            .color("#f0dc02".to_string())
            .opacity(0.7)
            .size(2),
    );

    let block_changes_box_plot = BoxPlot::new(raw_values_of(
        &tracked_stats,
        TrackedStatKey::BlockChangesPerSec,
    ))
    .name("Block Changes/s")
    .x_axis("x6")
    .y_axis("y6")
    .box_mean(BoxMean::True)
    .show_legend(false)
    .box_points(BoxPoints::All)
    .whisker_width(0.2)
    .marker(
        Marker::new()
            .color("#ff8f1f".to_string())
            .opacity(0.7)
            .size(2),
    );

    let user_calls_box_plot = BoxPlot::new(raw_values_of(
        &tracked_stats,
        TrackedStatKey::UserCallsPerSec,
    ))
    .name("User Calls/s")
    .x_axis("x7")
    .y_axis("y7")
    .box_mean(BoxMean::True)
    .show_legend(false)
    .box_points(BoxPoints::All)
    .whisker_width(0.2)
    .marker(
        Marker::new()
            .color("#FF00CC".to_string())
            .opacity(0.7)
            .size(2),
    );

    plot_main.add_trace(dbtime_trace);
    plot_highlight.add_trace(aas_box_plot);
    plot_main.add_trace(dbcpu_trace);
    plot_highlight.add_trace(exec_box_plot);
    plot_highlight.add_trace(trans_box_plot);
    plot_main.add_trace(cpu_user);
    plot_main.add_trace(cpu_load);
    add_tracked_stat_traces(&mut plot_main, &x_vals, &tracked_stats, is_logfilesync_high);

    let first_cpu = y_vals_cpu_count.first(); //Get first value of CPU Count
    if first_cpu.is_some() {
        // if you found something
        let cpu_is_changing = y_vals_cpu_count
            .iter()
            .find(|cpu| *cpu != first_cpu.unwrap()); //scan the whole vector to find the first value different - it means that number of cpus changes over time
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
    let mut anomalies_summary: BTreeMap<(u64, String), BTreeMap<String, Vec<AnomalySummaryItem>>> =
        BTreeMap::new();

    //println!("{}","Foreground Wait Events");
    make_notes!(
        &logfile_name,
        false,
        2,
        "\n{}\n",
        "Foreground Wait Events".yellow()
    );
    let mut top_fg_events: Vec<TopForegroundWaitEvents> = Vec::new();

    for (key, yv) in &y_vals_events_sorted {
        let mut event_data = TopForegroundWaitEvents::default();

        let event_trace = Scatter::new(x_vals.clone(), yv.clone())
            .mode(Mode::LinesText)
            .name(key.1.clone())
            .x_axis("x1")
            .y_axis("y3")
            .visible(Visible::LegendOnly);
        plot_main.add_trace(event_trace);
        let event_name: String = key.1.clone();
        /* Correlation calc */
        let corr: f64 = pearson_correlation_2v(&y_vals_dbtime, &yv);
        let corr = if corr.is_finite() { corr } else { 0.0 };
        // Print Correlation considered high enough to mark it
        make_notes!(
            &logfile_name,
            args.quiet,
            3,
            "\t{: >5}\n",
            &event_name.bold()
        );

        let correlation_info: String = format!("--- Correlation with DB Time: {:.2}", &corr);
        if corr >= 0.4 || corr <= -0.4 {
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "{: >50}",
                correlation_info.red().bold()
            );
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

        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "\t\tMarked as TOP in {:.2}% of probes\n",
            (x_n.len() as f64 / x_vals.len() as f64) * 100.0
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "\t\t--- AVG PCT of DB Time: {:>15.2}% \tSTDDEV PCT of DB Time: {:>15.2}%\n",
            &avg_exec_t,
            &stddev_exec_t
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "\t\t--- AVG Wait Time (s): {:>16.2} \tSTDDEV Wait Time (s): {:>16.2}\n",
            &avg_exec_s,
            &stddev_exec_s
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "\t\t--- AVG No. executions: {:>15.2} \tSTDDEV No. executions: {:>15.2}\n",
            &avg_exec_n,
            &stddev_exec_n
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "\t\t--- AVG wait/exec (ms): {:>15.2} \tSTDDEV wait/exec (ms): {:>15.2}\n\n",
            &avg_wait_per_exec_ms,
            &stddev_wait_per_exec_ms
        );

        event_data.event_name = event_name.clone();
        event_data.correlation_with_db_time = corr;
        event_data.marked_as_top_in_pct_of_probes =
            (x_n.len() as f64 / x_vals.len() as f64) * 100.0;
        event_data.avg_pct_of_dbtime = avg_exec_t;
        event_data.stddev_pct_of_db_time = stddev_exec_t;
        event_data.avg_wait_time_s = avg_exec_s;
        event_data.stddev_wait_time_s = stddev_exec_s;
        event_data.avg_number_of_executions = avg_exec_n;
        event_data.stddev_number_of_executions = stddev_exec_n;
        event_data.avg_wait_for_execution_ms = avg_wait_per_exec_ms;
        event_data.stddev_wait_for_execution_ms = stddev_wait_per_exec_ms;

        /* Print table of detected anomalies for given event_name (key.1)*/
        let safe_event_name: String = event_name
            .replace("/", "_")
            .replace(" ", "_")
            .replace(":", "")
            .replace("*", "_");
        let anomaly_id = format!("mad_fg_{}", &safe_event_name);
        let mut anomalies_flag: bool = false;

        if let Some(anomalies) = top_stats.event_anomalies_mad.get(&key.1) {
            let mut mad_events: MadAnomaliesEvents = MadAnomaliesEvents::default();
            let anomalies_detection_msg =
                "Detected anomalies using Median Absolute Deviation on the following dates:"
                    .to_string()
                    .red();
            anomalies_flag = true;
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "\t\t{}\n",
                anomalies_detection_msg
            );

            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("Date"),
                Cell::new("MAD Score"),
                Cell::new("Total Wait (s)"),
                Cell::new("Waits"),
                Cell::new("AVG Wait (ms)"),
                Cell::new("DBTime (%)"),
            ]));

            for (i, a) in anomalies.iter().enumerate() {
                let c_event = Cell::new(&a.0);

                let c_mad_score: Cell = Cell::new(&format!("{:.3}", a.1));

                let wait_event = collection
                    .awrs
                    .iter()
                    .filter(|awr| awr.snap_info.begin_snap_time == a.0)
                    .flat_map(|awr| awr.foreground_wait_events.iter())
                    .find(|w| w.event == key.1)
                    .unwrap();

                let c_total_wait_s = Cell::new(&format!("{:.3}", wait_event.total_wait_time_s));
                let c_waits = Cell::new(&format!("{:.3}", wait_event.waits));
                let avg_wait_ms = wait_event.total_wait_time_s / (wait_event.waits as f64) * 1000.0;
                let c_avg_wait_ms = Cell::new(&format!("{:.3}", avg_wait_ms));
                let c_dbtime_pct = Cell::new(&format!("{:.2}", wait_event.pct_dbtime));

                mad_events.anomaly_date = a.0.clone();
                mad_events.mad_score = a.1;
                mad_events.total_wait_s = wait_event.total_wait_time_s;
                mad_events.number_of_waits = wait_event.waits;
                mad_events.avg_wait_time_for_execution_ms = avg_wait_ms;
                mad_events.pct_of_db_time = wait_event.pct_dbtime;
                event_data
                    .median_absolute_deviation_anomalies
                    .push(mad_events.clone());

                table.add_row(Row::new(vec![
                    c_event.clone(),
                    c_mad_score.clone(),
                    c_total_wait_s.clone(),
                    c_waits.clone(),
                    c_avg_wait_ms.clone(),
                    c_dbtime_pct.clone(),
                ]));
                table_anomalies.push_str(&format!(
                    r#"<tr>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                    </tr>"#,
                    c_event.to_string(),
                    c_mad_score.to_string(),
                    c_total_wait_s.to_string(),
                    c_waits.to_string(),
                    c_avg_wait_ms.to_string(),
                    c_dbtime_pct.to_string()
                ));

                let begin_snap_id = collection
                    .awrs
                    .iter()
                    .find_map(|awr| {
                        (awr.snap_info.begin_snap_time == a.0).then(|| awr.snap_info.begin_snap_id)
                    })
                    .unwrap();

                anomalies_join(
                    &mut anomalies_summary,
                    (begin_snap_id, a.0.clone()),
                    "EVENT",
                    &key.1,
                    a.1,
                );
            }
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }
        } else {
            let no_anomalies_txt =
                format!("\t\tNo anomalies detected based on MAD score threshold: 7.0\n");
            anomalies_flag = false;
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "{}",
                no_anomalies_txt.green().italic()
            );
        }
        make_notes!(&logfile_name, args.quiet, 0, "\n");

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
        ))
        };
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
    make_notes!(
        &logfile_name,
        false,
        2,
        "{}\n",
        "Background Wait Events".yellow()
    );
    let mut top_bg_events: Vec<TopBackgroundWaitEvents> = Vec::new();

    for (key, yv) in &y_vals_bgevents_sorted {
        let mut event_data = TopBackgroundWaitEvents::default();

        let event_name: String = key.1.clone();
        /* Correlation calc */
        let corr: f64 = pearson_correlation_2v(&y_vals_dbtime, &yv);
        let corr = if corr.is_finite() { corr } else { 0.0 };

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
        make_notes!(
            &logfile_name,
            args.quiet,
            3,
            "\t{: >5}\n",
            &event_name.bold()
        );

        let correlation_info: String = format!("--- Correlation with DB Time: {:.2}", &corr);
        if corr >= 0.4 || corr <= -0.4 {
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "{: >50}",
                correlation_info.red().bold()
            );
        } else {
            make_notes!(&logfile_name, args.quiet, 0, "{: >50}", correlation_info);
        }
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "\t\tMarked as TOP in {:.2}% of probes\n",
            (x_n.len() as f64 / x_vals.len() as f64) * 100.0
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "\t\t--- AVG PCT of DB Time: {:>15.2}% \tSTDDEV PCT of DB Time: {:>15.2}%\n",
            &avg_exec_t,
            &stddev_exec_t
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "\t\t--- AVG Wait Time (s): {:>16.2} \tSTDDEV Wait Time (s): {:>16.2}\n",
            &avg_exec_s,
            &stddev_exec_s
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "\t\t--- AVG No. executions: {:>15.2} \tSTDDEV No. executions: {:>15.2}\n",
            &avg_exec_n,
            &stddev_exec_n
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "\t\t--- AVG wait/exec (ms): {:>15.2} \tSTDDEV wait/exec (ms): {:>15.2}\n\n",
            &avg_wait_per_exec_ms,
            &stddev_wait_per_exec_ms
        );

        event_data.event_name = event_name.clone();
        event_data.correlation_with_db_time = corr;
        event_data.marked_as_top_in_pct_of_probes =
            (x_n.len() as f64 / x_vals.len() as f64) * 100.0;
        event_data.avg_pct_of_dbtime = avg_exec_t;
        event_data.stddev_pct_of_db_time = stddev_exec_t;
        event_data.avg_wait_time_s = avg_exec_s;
        event_data.stddev_wait_time_s = stddev_exec_s;
        event_data.avg_number_of_executions = avg_exec_n;
        event_data.stddev_number_of_executions = stddev_exec_n;
        event_data.avg_wait_for_execution_ms = avg_wait_per_exec_ms;
        event_data.stddev_wait_for_execution_ms = stddev_wait_per_exec_ms;

        /* Print table of detected anomalies for given event_name (key.1)*/
        let safe_event_name: String = event_name
            .replace("/", "_")
            .replace(" ", "_")
            .replace(":", "")
            .replace("*", "_");
        let anomaly_id = format!("mad_bg_{}", &safe_event_name);
        let mut anomalies_flag: bool = false;

        if let Some(anomalies) = top_stats.bgevent_anomalies_mad.get(&key.1) {
            let mut mad_events: MadAnomaliesEvents = MadAnomaliesEvents::default();

            let anomalies_detection_msg =
                "Detected anomalies using Median Absolute Deviation on the following dates:"
                    .to_string()
                    .red();
            anomalies_flag = true;
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "\t\t{}\n",
                anomalies_detection_msg
            );

            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("Date"),
                Cell::new("MAD Score"),
                Cell::new("Total Wait (s)"),
                Cell::new("Waits"),
                Cell::new("AVG Wait (ms)"),
                Cell::new("DBTime (%)"),
            ]));

            for (i, a) in anomalies.iter().enumerate() {
                let c_event = Cell::new(&a.0);

                let c_mad_score: Cell = Cell::new(&format!("{:.3}", a.1));

                let wait_event = collection
                    .awrs
                    .iter()
                    .filter(|awr| awr.snap_info.begin_snap_time == a.0)
                    .flat_map(|awr| awr.background_wait_events.iter())
                    .find(|w| w.event == key.1)
                    .unwrap();

                let c_total_wait_s = Cell::new(&format!("{:.3}", wait_event.total_wait_time_s));
                let c_waits = Cell::new(&format!("{:.3}", wait_event.waits));
                let c_avg_wait_ms = Cell::new(&format!(
                    "{:.3}",
                    wait_event.total_wait_time_s / (wait_event.waits as f64) * 1000.0
                ));
                let c_dbtime_pct = Cell::new(&format!("{:.2}", wait_event.pct_dbtime));

                mad_events.anomaly_date = a.0.clone();
                mad_events.mad_score = a.1;
                mad_events.total_wait_s = wait_event.total_wait_time_s;
                mad_events.number_of_waits = wait_event.waits;
                mad_events.avg_wait_time_for_execution_ms =
                    wait_event.total_wait_time_s / (wait_event.waits as f64) * 1000.0;
                mad_events.pct_of_db_time = wait_event.pct_dbtime;
                event_data
                    .median_absolute_deviation_anomalies
                    .push(mad_events.clone());

                table.add_row(Row::new(vec![
                    c_event.clone(),
                    c_mad_score.clone(),
                    c_total_wait_s.clone(),
                    c_waits.clone(),
                    c_avg_wait_ms.clone(),
                    c_dbtime_pct.clone(),
                ]));
                table_anomalies.push_str(&format!(
                    r#"<tr>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                    </tr>"#,
                    c_event.to_string(),
                    c_mad_score.to_string(),
                    c_total_wait_s.to_string(),
                    c_waits.to_string(),
                    c_avg_wait_ms.to_string(),
                    c_dbtime_pct.to_string()
                ));

                let begin_snap_id = collection
                    .awrs
                    .iter()
                    .find_map(|awr| {
                        (awr.snap_info.begin_snap_time == a.0).then(|| awr.snap_info.begin_snap_id)
                    })
                    .unwrap();

                anomalies_join(
                    &mut anomalies_summary,
                    (begin_snap_id, a.0.clone()),
                    "BGEVENT",
                    &key.1,
                    a.1,
                );
            }
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }
        } else {
            let no_anomalies_txt =
                format!("\t\tNo anomalies detected based on MAD score threshold: 7.0\n");
            anomalies_flag = false;
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "{}",
                no_anomalies_txt.green().italic()
            );
        }
        make_notes!(&logfile_name, args.quiet, 0, "\n");

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
        ))
        };
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
    make_notes!(
        &logfile_name,
        false,
        2,
        "{}",
        "TOP SQLs by Elapsed time".yellow()
    );

    let mut ash_event_sql_map: HashMap<String, HashSet<String>> = HashMap::new();
    let mut crr_event_sql_map: HashMap<String, HashMap<String, f64>> = HashMap::new();

    let mut top_sqls: Vec<TopSQLsByElapsedTime> = Vec::new();
    for (key, yv) in y_vals_sqls_sorted {
        let mut sql_data = TopSQLsByElapsedTime::default();

        let sql_trace = Scatter::new(x_vals.clone(), yv.clone())
            .mode(Mode::LinesText)
            .name(key.1.clone())
            .x_axis("x1")
            .y_axis("y5")
            .visible(Visible::LegendOnly);
        plot_main.add_trace(sql_trace);

        let sql_id: String = key.1.clone();
        let sql_id_disp = format!("SQL_ID: {}", key.1.clone());
        /* Correlation calc */
        let corr: f64 = pearson_correlation_2v(&y_vals_dbtime, &yv);
        let corr = if corr.is_finite() { corr } else { 0.0 };
        // Print Correlation considered high enough to mark it
        let top_sections: HashMap<String, f64> = report_top_sql_sections(&sql_id, &collection.awrs);
        make_notes!(
            &logfile_name,
            args.quiet,
            3,
            "\n\t{: >5}",
            &sql_id_disp.bold()
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "\n\t {}\n",
            format!(
                "Other Top Sections: {}",
                &top_sections
                    .iter()
                    .map(|(key, value)| format!("{} [{:.2}%]", key, value))
                    .collect::<Vec<String>>()
                    .join(" | ")
            )
            .italic(),
        );

        let correlation_info: String = format!("--- Correlation with DB Time: {:.2}", &corr);
        if corr >= 0.4 || corr <= -0.4 {
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "{: >49}",
                correlation_info.red().bold()
            );
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
        let x_c: Vec<f64> = collection
            .awrs
            .iter()
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
        let x_s: Vec<f64> = collection
            .awrs
            .iter()
            .flat_map(|s| s.sql_cpu_time.clone())
            .filter(|sql| sql.0 == key.1.clone())
            .map(|sqls| sqls.1.cpu_time_s)
            .collect();

        let avg_exec_cpu: f64 = mean(x_s.clone()).unwrap_or(0.0);
        let stddev_exec_cpu: f64 = std_deviation(x_s).unwrap_or(0.0);
        let mut sql_type = "?".to_string();
        let s = collection
            .awrs
            .iter()
            .flat_map(|a| a.sql_elapsed_time.clone())
            .find(|s| s.sql_id == sql_id);
        if s.is_some() {
            sql_type = s.unwrap().sql_type;
        }

        debug_note!(
            "Len of X axis is: {}, while {} was marked as TOP in {} probes.",
            x_vals.len(),
            &key.1,
            x.len()
        );

        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "{: >24}{:.2}% of probes\n",
            "Marked as TOP in ",
            (x.len() as f64 / x_vals.len() as f64) * 100.0
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "{: >35} {: <16.2} \tSTDDEV Ela by Exec: {:.2}\n",
            "--- AVG Ela by Exec:",
            avg_exec_t,
            stddev_exec_t
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "{: >35} {: <16.2} \tSTDDEV CPU by Exec: {:.2}\n",
            "--- AVG CPU by Exec:",
            avg_exec_t_cpu,
            stddev_exec_t_cpu
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "{: >36} {: <16.2} \tSTDDEV Ela Time   : {:.2}\n",
            "--- AVG Ela Time (s):",
            avg_exec_s,
            stddev_exec_s
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "{: >36} {: <16.2} \tSTDDEV CPU Time   : {:.2}\n",
            "--- AVG CPU Time (s):",
            avg_exec_cpu,
            stddev_exec_cpu
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "{: >38} {: <14.2} \tSTDDEV No. executions:  {:.2}\n",
            "--- AVG No. executions:",
            avg_exec_n,
            stddev_exec_n
        );
        make_notes!(
            &logfile_name,
            args.quiet,
            0,
            "{: >23} {} \n",
            "MODULE: ",
            top_stats.sqls.get(&sql_id).unwrap().blue()
        );
        if !sql_type.is_empty() {
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "{: >23} {}\n\n",
                "  TYPE: ",
                sql_type.green()
            );
        }

        sql_data.sql_id = sql_id.clone();
        sql_data.module = top_stats.sqls.get(&sql_id).unwrap().clone();
        sql_data.sql_type = sql_type.clone();
        sql_data.correlation_with_db_time = corr;
        sql_data.marked_as_top_in_pct_of_probes = (x.len() as f64 / x_vals.len() as f64) * 100.0;
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

        for (k, v) in &top_sections {
            if k == "SQL CPU" {
                sql_data
                    .pct_of_time_sql_was_found_in_other_top_sections
                    .sqls_by_cpu_time_pct = *v;
            } else if k == "SQL I/O" {
                sql_data
                    .pct_of_time_sql_was_found_in_other_top_sections
                    .sqls_by_user_io_pct = *v;
            } else if k == "SQL READS" {
                sql_data
                    .pct_of_time_sql_was_found_in_other_top_sections
                    .sqls_by_reads = *v;
            } else if k == "SQL GETS" {
                sql_data
                    .pct_of_time_sql_was_found_in_other_top_sections
                    .sqls_by_gets = *v;
            }
        }

        /* Print table of detected anomalies for given SQL_ID (key.1)*/
        let anomaly_id = format!("mad_{}", &sql_id);
        let mut anomalies_flag: bool = false;

        if let Some(anomalies) = top_stats.sql_elapsed_time_anomalies_mad.get(&key.1) {
            let anomalies_detection_msg =
                "Detected anomalies using Median Absolute Deviation on the following dates:"
                    .to_string()
                    .red();
            anomalies_flag = true;
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "\t\t{}\n",
                anomalies_detection_msg
            );

            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("Date"),
                Cell::new("MAD Score"),
                Cell::new("Elapsed Time (s)"),
                Cell::new("Executions"),
                Cell::new("Ela time / exec (s)"),
            ]));

            for (i, a) in anomalies.iter().enumerate() {
                let mut mad_sql = MadAnomaliesSQL::default();

                let c_event = Cell::new(&a.0);

                let c_mad_score: Cell = Cell::new(&format!("{:.3}", a.1));

                let sql_id = collection
                    .awrs
                    .iter()
                    .filter(|awr| awr.snap_info.begin_snap_time == a.0)
                    .flat_map(|awr| awr.sql_elapsed_time.iter())
                    .find(|s| s.sql_id == key.1)
                    .unwrap();

                let c_elapsed_time = Cell::new(&format!("{:.3}", sql_id.elapsed_time_s));
                let c_executions = Cell::new(&format!("{:.3}", sql_id.executions));
                let c_elapsed_time_exec = Cell::new(&format!("{:.3}", sql_id.elpased_time_exec_s));

                table.add_row(Row::new(vec![
                    c_event.clone(),
                    c_mad_score.clone(),
                    c_elapsed_time.clone(),
                    c_executions.clone(),
                    c_elapsed_time_exec.clone(),
                ]));

                mad_sql.anomaly_date = a.0.clone();
                mad_sql.mad_score = a.1;
                mad_sql.elapsed_time_cumulative_s = sql_id.elapsed_time_s;
                mad_sql.number_of_executions = sql_id.executions;
                mad_sql.avg_exec_time_for_execution = sql_id.elpased_time_exec_s;

                sql_data
                    .median_absolute_deviation_anomalies
                    .push(mad_sql.clone());

                table_anomalies.push_str(&format!(
                    r#"<tr>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                    </tr>"#,
                    c_event.to_string(),
                    c_mad_score.to_string(),
                    c_elapsed_time.to_string(),
                    c_executions.to_string(),
                    c_elapsed_time_exec.to_string()
                ));

                let begin_snap_id = collection
                    .awrs
                    .iter()
                    .find_map(|awr| {
                        (awr.snap_info.begin_snap_time == a.0).then(|| awr.snap_info.begin_snap_id)
                    })
                    .unwrap();

                anomalies_join(
                    &mut anomalies_summary,
                    (begin_snap_id, a.0.clone()),
                    "SQL",
                    &key.1,
                    a.1,
                );
            }
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }
        } else {
            let no_anomalies_txt =
                format!("\t\tNo anomalies detected based on MAD score threshold: 7.0\n");
            anomalies_flag = false;
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "{}",
                no_anomalies_txt.green().italic()
            );
        }
        make_notes!(&logfile_name, args.quiet, 0, "\n");

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
        ))
        };
        table_anomalies = "".to_string();
        anomalies_flag = false;

        let mut sql_corr_txt: Vec<String> = Vec::new();

        let mut table = Table::new();
        table.set_titles(Row::new(vec![
            Cell::new("Wait Event Name"),
            Cell::new("Pearson correlation coefficient"),
        ]));
        let mut found_strong_events: bool = false;

        for (key, ev) in &y_vals_events_sorted {
            let mut corr_events = WaitEventsWithStrongCorrelation::default();
            let crr = pearson_correlation_2v(&yv, &ev);
            let corr_text = format!(
                "{: >32} | {: <32} : {:.2}",
                "+".to_string(),
                key.1.clone(),
                crr
            );
            if crr >= 0.333 || crr <= -0.333 {
                //Correlation considered high enough to mark it
                sql_corr_txt.push(format!(
                    r#"<span style="color:red; font-weight:bold;">{}</span>"#,
                    corr_text
                ));
                //make_notes!(&logfile_name, args.quiet, 0, "{}\n", corr_text.red().bold());
                let c_event_name = Cell::new(&key.1);
                let c_corr_factor = Cell::new(&format!("{:.2}", crr));
                table.add_row(Row::new(vec![c_event_name, c_corr_factor]));
                found_strong_events = true;
                crr_event_sql_map
                    .entry(key.1.clone())
                    .or_insert_with(HashMap::new)
                    .insert(sql_id.clone(), crr);
                corr_events.event_name = key.1.clone();
                corr_events.correlation_value = crr;
                sql_data
                    .wait_events_with_strong_pearson_correlation
                    .push(corr_events);
            }
        }

        if found_strong_events {
            let sql_corr_txt_header =
                "\t\tWait events with strong Pearson correlation coefficient factor.\n".to_string();
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "{}",
                sql_corr_txt_header.bold().blue()
            );
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }
        }

        let mut ash_events: HashMap<String, Vec<f64>> = HashMap::new();
        collection
            .awrs
            .iter()
            .filter(|a| a.top_sql_with_top_events.contains_key(&sql_id))
            .flat_map(|a| a.top_sql_with_top_events.clone())
            .for_each(|(s_id, top_event)| {
                if s_id == sql_id {
                    ash_events
                        .entry(top_event.event_name.clone())
                        .or_insert_with(Vec::new)
                        .push(top_event.pct_activity);
                    ash_event_sql_map
                        .entry(top_event.event_name)
                        .or_insert_with(HashSet::new)
                        .insert(s_id);
                }
            });

        let mut ash_events_html = String::new();
        if !ash_events.is_empty() {
            let mut sql_ash = WaitEventsFromASH::default();
            let sql_ash_txt_header =
                "Wait events actually found in ASH section of AWR reports:\n".to_string();
            make_notes!(
                &logfile_name,
                args.quiet,
                0,
                "\n\t\t{}",
                sql_ash_txt_header.bold().blue()
            );

            let mut table = Table::new();
            table.set_titles(Row::new(vec![
                Cell::new("Wait Event Name"),
                Cell::new("AVG % of DB Time in SQL"),
                Cell::new("STDDEV % of DB Time in SQL"),
                Cell::new("Count"),
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
                    c_count,
                ]));

                sql_ash.event_name = evname.clone();
                sql_ash.avg_pct_of_dbtime_in_sql = avg_pct;
                sql_ash.stddev_pct_of_dbtime_in_sql = stddev_pct;
                sql_ash.count = pctvalues.len() as u64;
                sql_data
                    .wait_events_found_in_ash_sections_for_this_sql
                    .push(sql_ash.clone());
            }

            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }

            ash_events_html = table_to_html_string(
                &table,
                &sql_ash_txt_header,
                &[
                    "Wait Event Name",
                    "AVG % of DB Time in SQL",
                    "STDDEV % of DB Time in SQL",
                    "Count",
                ],
            );
        }

        let mut sql_text = format!(
            "Security level {} does not allow gathering SQL text, use level 2 or higher",
            args.security_level
        );
        if args.security_level >= 2 && !collection.sql_text.is_empty() {
            sql_text = format!(
                "<code><details><summary>FULL SQL TEXT</summary>{}</details></code>\n</body>",
                collection
                    .sql_text
                    .get(&sql_id)
                    .unwrap_or(&"SQL NOT FOUND".to_string())
            )
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
            module = top_stats.sqls.get(&sql_id).unwrap(),
            top_section = top_sections
                .iter()
                .map(|(key, value)| format!("<span class=\"bold\">{}:</span> {:.2}%", key, value))
                .collect::<Vec<String>>()
                .join("<br>"),
            sql_corr_txt = sql_corr_txt.join("<br>"),
            sql_txt = sql_text,
            ash_table = ash_events_html
        );

        // Insert this into already existing sqlid_*.html file
        let filename: String = format!("{}/sqlid/sqlid_{}.html", &html_dir, sql_id);
        let mut sql_file: String =
            fs::read_to_string(&filename).expect(&format!("Failed to read file: {}", filename));
        sql_file = sql_file.replace("<body>", &format!("<body>\n{}\n", sqlid_html_content));
        if let Err(e) = fs::write(&filename, sql_file) {
            eprintln!("Error writing file {}: {}", filename, e);
        }

        top_sqls.push(sql_data);
    }

    report_for_ai.top_sqls_by_elapsed_time = top_sqls;

    // --- Enrich foreground wait events with table names from SQL text ---
    if !collection.sql_text.is_empty() {
        // Build reverse map: event_name -> Set<SQL_ID>
        // Sources: ASH data + strong correlation
        let mut event_to_sqls: HashMap<String, HashSet<String>> = HashMap::new();

        for sql_data in &report_for_ai.top_sqls_by_elapsed_time {
            // From ASH
            for ash_event in &sql_data.wait_events_found_in_ash_sections_for_this_sql {
                event_to_sqls
                    .entry(ash_event.event_name.clone())
                    .or_insert_with(HashSet::new)
                    .insert(sql_data.sql_id.clone());
            }
            // From correlation
            for corr_event in &sql_data.wait_events_with_strong_pearson_correlation {
                event_to_sqls
                    .entry(corr_event.event_name.clone())
                    .or_insert_with(HashSet::new)
                    .insert(sql_data.sql_id.clone());
            }
        }

        for event_data in report_for_ai.top_foreground_wait_events.iter_mut() {
            if let Some(sql_ids) = event_to_sqls.get(&event_data.event_name) {
                let tables = find_tables_for_sql_ids(sql_ids, &collection.sql_text);
                if !tables.is_empty() {
                    event_data.tables_associated_with_event_based_on_ash_sql = Some(tables);
                }
            }
        }
    }
    // -----------------

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
    make_notes!(&logfile_name, false, 0, "\n");
    /* "IO Stats by Function" are goin into report */
    make_notes!(
        &logfile_name,
        false,
        2,
        "{}\n",
        format!("IO Statistics by Function - Summary").yellow()
    );
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
        if function_name == "zMAIN" {
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
            iostat.statistics_summary.push(StatsSummary {
                statistic_name: format_metric_name(&metric_name),
                avg_value: *mean,
                stddev_value: *std_dev,
            });
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
    report_for_ai.io_stats_by_function_summary = iostat_summary;

    make_notes!(
        &logfile_name,
        false,
        2,
        "{}\n",
        format!("Latch Activity Statistics - Summary").yellow()
    );
    make_notes!(&logfile_name, args.quiet, 0, "{}\n", table_latch);

    /******** Report Segment Statistics Summary */
    let segstats = report_segments_summary(
        &collection.awrs,
        &args,
        &logfile_name,
        &html_dir,
        &mut report_for_ai,
    );
    /********************************************/

    let mut sorted_correlation =
        report_instance_stats_cor(instance_stats.clone(), y_vals_dbtime.clone());
    let corr_txt = format!(
        "Instance Statistics: Correlation with DB Time for values >= {} and <= -{}",
        sorted_correlation.1, sorted_correlation.1
    );
    make_notes!(&logfile_name, args.quiet, 0, "\n\n");
    make_notes!(&logfile_name, false, 2, "{}", corr_txt.yellow());
    make_notes!(&logfile_name, args.quiet, 0, "\n\n");

    let mut stats_table_rows = String::new();
    for ((score, key), value) in sorted_correlation.0.iter().rev() {
        // Sort in descending order
        stats_table_rows.push_str(&format!(
            r#"<tr><td>{}</td><td>{:.3}</td></tr>"#,
            key, value
        ));
    }
    let brand = jasmin_brand_banner_html();
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
                    background-color: #111111;
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
                {brand}
                <p><span style="font-size:20px;font-weight:bold;">Correlation of Instance Statistics with DB Time for Values >= {} and <= -{}</span></p>
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
        sorted_correlation.1, sorted_correlation.1, stats_table_rows
    );

    // Write to the file
    let stats_corr_filename: String = format!("{}/stats/statistics_corr.html", &html_dir);
    if let Err(e) = fs::write(&stats_corr_filename, table_stat_corr) {
        eprintln!("Error writing file {}: {}", stats_corr_filename, e);
    }
    for (k, v) in sorted_correlation.0 {
        make_notes!(&logfile_name, args.quiet, 0, "\t{: >64} : {:.2}\n", &k.1, v);
        report_for_ai
            .instance_stats_pearson_correlation
            .push(InstanceStatisticCorrelation {
                stat_name: k.1.clone(),
                pearson_correlation_value: v,
            });
    }
    /* Add information about stats anomalies to the summary */
    let stat_anomalies = detect_stats_anomalies_mad(&collection.awrs, &args);
    let all_stats = top_stats.stat_names;
    for s in all_stats {
        if let Some(anomalies) = stat_anomalies.get(&s.0) {
            for a in anomalies {
                let begin_snap_id = collection
                    .awrs
                    .iter()
                    .find_map(|awr| {
                        (awr.snap_info.begin_snap_time == a.0).then(|| awr.snap_info.begin_snap_id)
                    })
                    .unwrap();

                anomalies_join(
                    &mut anomalies_summary,
                    (begin_snap_id, a.0.clone()),
                    "STAT",
                    s.0.clone(),
                    a.1,
                );
            }
        }
    }
    /********************************************************/

    /* Add information about Dictionary Cache anomalies to the summary */
    let stat_anomalies = detect_dc_anomalies_mad(&collection.awrs, &args);
    let all_stats: HashSet<String> = collection
        .awrs
        .iter()
        .flat_map(|a| a.dictionary_cache.clone())
        .map(|dc| dc.statname)
        .collect();
    for s in all_stats {
        if let Some(anomalies) = stat_anomalies.get(&s) {
            for a in anomalies {
                let begin_snap_id = collection
                    .awrs
                    .iter()
                    .find_map(|awr| {
                        (awr.snap_info.begin_snap_time == a.0).then(|| awr.snap_info.begin_snap_id)
                    })
                    .unwrap();

                anomalies_join(
                    &mut anomalies_summary,
                    (begin_snap_id, a.0.clone()),
                    "DC",
                    s.clone(),
                    a.1,
                );
            }
        }
    }
    /********************************************************/

    /* Add information about Library Cache anomalies to the summary */
    let stat_anomalies = detect_libcache_anomalies_mad(&collection.awrs, &args);
    let all_stats: HashSet<String> = collection
        .awrs
        .iter()
        .flat_map(|a| a.library_cache.clone())
        .map(|lc| lc.statname)
        .collect();

    for s in all_stats {
        if let Some(anomalies) = stat_anomalies.get(&s) {
            for a in anomalies {
                let begin_snap_id = collection
                    .awrs
                    .iter()
                    .find_map(|awr| {
                        (awr.snap_info.begin_snap_time == a.0).then(|| awr.snap_info.begin_snap_id)
                    })
                    .unwrap();

                anomalies_join(
                    &mut anomalies_summary,
                    (begin_snap_id, a.0.clone()),
                    "LC",
                    s.clone(),
                    a.1,
                );
            }
        }
    }
    /********************************************************/

    /* Add information about Latch Activity anomalies to the summary */
    let stat_anomalies = detect_latch_activity_anomalies_mad(&collection.awrs, &args);
    let all_stats: HashSet<String> = collection
        .awrs
        .iter()
        .flat_map(|a| a.latch_activity.clone())
        .map(|lc| lc.statname)
        .collect();

    for s in all_stats {
        if let Some(anomalies) = stat_anomalies.get(&s) {
            for a in anomalies {
                let begin_snap_id = collection
                    .awrs
                    .iter()
                    .find_map(|awr| {
                        (awr.snap_info.begin_snap_time == a.0).then(|| awr.snap_info.begin_snap_id)
                    })
                    .unwrap();

                anomalies_join(
                    &mut anomalies_summary,
                    (begin_snap_id, a.0.clone()),
                    "LATCH",
                    s.clone(),
                    a.1,
                );
            }
        }
    }
    /********************************************************/

    /* Add information about Time Model anomalies to the summary */
    let stat_anomalies = detect_time_model_anomalies_mad(&collection.awrs, &args);
    let all_stats: HashSet<String> = collection
        .awrs
        .iter()
        .flat_map(|a| a.time_model_stats.clone())
        .map(|lc| lc.stat_name)
        .collect();

    for s in all_stats {
        if let Some(anomalies) = stat_anomalies.get(&s) {
            for a in anomalies {
                let begin_snap_id = collection
                    .awrs
                    .iter()
                    .find_map(|awr| {
                        (awr.snap_info.begin_snap_time == a.0).then(|| awr.snap_info.begin_snap_id)
                    })
                    .unwrap();

                anomalies_join(
                    &mut anomalies_summary,
                    (begin_snap_id, a.0.clone()),
                    "TM",
                    s.clone(),
                    a.1,
                );
            }
        }
    }
    /********************************************************/

    /****************   Report anomalies summary ************/
    //println!("{}","Anomalies Summary");
    make_notes!(&logfile_name, false, 0, "\n\n");
    make_notes!(&logfile_name, false, 1, "{}\n", "ANOMALIES".bold().green());
    /* Load Profile Anomalies detection and report */
    make_notes!(
        &logfile_name,
        false,
        2,
        "\n{} 7.0, {} {}\n",
        "Load Profile Anomalies detection using Median Absolute Deviation score threshold:"
            .yellow(),
        "MAD top:".yellow(),
        args.mad_top
    );

    let all_loadprofile: HashSet<String> = collection
        .awrs
        .iter()
        .flat_map(|awr| &awr.load_profile)
        .map(|l| l.stat_name.clone())
        .collect();
    let profile_anomalies = detect_loadprofile_anomalies_mad(&collection.awrs, &args);
    for l in all_loadprofile {
        let stat_name = l.bold();
        let per_second_v: Vec<f64> = collection
            .awrs
            .iter()
            .filter_map(|awr| measurements::load_profile_rate(awr, &l))
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

            let anomalies_str = format!(
                "\n\tAnomalies detected for \"{}\", AVG value per second is: {:.2}",
                stat_name, mean_per_s
            )
            .red();
            make_notes!(&logfile_name, args.quiet, 0, "{}\n", anomalies_str);
            for a in anomalies {
                let per_second_this_date = collection
                    .awrs
                    .iter()
                    .find(|awr| awr.snap_info.begin_snap_time == a.0)
                    .and_then(|awr| measurements::load_profile_rate(awr, &l))
                    .unwrap_or(f64::NAN);

                let c_date = Cell::new(&a.0);
                let c_mad = Cell::new(&format!("{:.2}", a.1));
                let c_per_second = Cell::new(&format!("{}", per_second_this_date));
                let c_avg_per_second = Cell::new(&format!("{:.2}", mean_per_s));
                let c_mad_threshold = Cell::new("7.0");
                table.add_row(Row::new(vec![
                    c_date,
                    c_mad,
                    c_mad_threshold,
                    c_per_second,
                    c_avg_per_second,
                ]));

                report_for_ai
                    .load_profile_anomalies
                    .push(LoadProfileAnomalies {
                        load_profile_stat_name: l.clone(),
                        anomaly_date: a.0.clone(),
                        mad_score: a.1,
                        mad_threshold: 7.0,
                        per_second: per_second_this_date,
                        avg_value_per_second: mean_per_s,
                    });

                let begin_snap_id = collection
                    .awrs
                    .iter()
                    .find_map(|awr| {
                        (awr.snap_info.begin_snap_time == a.0).then(|| awr.snap_info.begin_snap_id)
                    })
                    .unwrap();

                anomalies_join(
                    &mut anomalies_summary,
                    (begin_snap_id, a.0.clone()),
                    "LP",
                    l.clone(),
                    a.1,
                );
            }
            for table_line in table.to_string().lines() {
                make_notes!(&logfile_name, args.quiet, 0, "\t\t{}\n", table_line);
            }
        } else {
            let no_anomalies_str = format!(
                "\tNo anomalies detected for \"{}\", AVG value per second iss: {:.2}",
                stat_name, mean_per_s
            )
            .green();
            make_notes!(&logfile_name, args.quiet, 0, "\n{}\n", no_anomalies_str);
        }
    }
    /***********************************************/

    trim_anomalies_summary(&mut anomalies_summary, &args);

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
        report_anomalies_summary(
            &mut anomalies_summary,
            &args,
            &logfile_name,
            &mut report_for_ai
        )
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
                *heat_data
                    .entry((xval_key.clone(), atype.clone()))
                    .or_insert(0) += count;
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
            LayoutGrid::new().rows(5).columns(1),
            //.row_order(Grid::TopToBottom),
        )
        //.legend(Legend::new()
        //    .y_anchor(Anchor::Top)
        //    .x(1.02)
        //    .y(0.5)
        //)
        .y_axis6(
            Axis::new()
                .anchor("x1")
                .domain(&[0.785, 1.0])
                .title("Anomalies")
                .visible(true)
                .zero_line(true)
                .range_mode(RangeMode::ToZero),
        )
        .y_axis5(
            Axis::new()
                .anchor("x1")
                .domain(&[0.63, 0.78])
                .title("SQL Elapsed Time")
                .visible(true)
                .zero_line(true), //.range_mode(RangeMode::ToZero)
        )
        .y_axis4(
            Axis::new()
                .anchor("x1")
                .domain(&[0.47, 0.625])
                .range(vec![0.])
                .title("CPU Util (%/#)")
                .zero_line(true)
                .range(vec![0.])
                .range_mode(RangeMode::ToZero),
        )
        .y_axis3(
            Axis::new()
                .anchor("x1")
                .domain(&[0.31, 0.465])
                .title("Wait Events (s)")
                .zero_line(true)
                .range(vec![0.])
                .range_mode(RangeMode::ToZero),
        )
        .y_axis2(
            Axis::new()
                .anchor("x1")
                .domain(&[0.155, 0.305])
                .range(vec![0.])
                .title("#")
                .zero_line(true)
                .range_mode(RangeMode::ToZero),
        )
        .y_axis(
            Axis::new()
                .anchor("x1")
                .domain(&[0., 0.15])
                .title("(s/s)")
                .zero_line(true)
                .range_mode(RangeMode::ToZero),
        )
        .x_axis(
            Axis::new()
                .domain(&[0.0, 1.0])
                .anchor("y1")
                .range(vec![0.])
                .show_grid(true),
        )
        .hover_mode(HoverMode::X);

    let layout_highlight: Layout = Layout::new()
        .height(400)
        .grid(
            LayoutGrid::new().rows(1).columns(6),
            //.row_order(Grid::TopToBottom),
        )
        .hover_mode(HoverMode::X)
        .x_axis7(
            Axis::new()
                .domain(&[0.9, 1.0])
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
                .domain(&[0.75, 0.85])
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
                .domain(&[0.6, 0.7])
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
                .domain(&[0.45, 0.55])
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
                .domain(&[0.3, 0.4])
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
                .domain(&[0.15, 0.25])
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
                .domain(&[0.0, 0.1])
                .anchor("y1")
                .range(vec![0.])
                .show_grid(false),
        )
        .y_axis(
            Axis::new()
                .domain(&[0.0, 1.0])
                .anchor("x1")
                .range(vec![0.])
                .range_mode(RangeMode::ToZero)
                .show_grid(false),
        );
    let layout_highlight2: Layout = Layout::new()
        .height(400)
        .paper_background_color("White")
        .plot_background_color("White")
        .grid(
            LayoutGrid::new().rows(1).columns(1),
            //.row_order(Grid::TopToBottom),
        )
        .hover_mode(HoverMode::X)
        .x_axis7(
            Axis::new()
                .domain(&[0.9, 1.0])
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
                .domain(&[0.75, 0.85])
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
                .domain(&[0.6, 0.7])
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
                .domain(&[0.45, 0.55])
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
                .domain(&[0.3, 0.4])
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
                .domain(&[0.15, 0.25])
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
                .domain(&[0.0, 0.1])
                .anchor("y1")
                .range(vec![0.])
                .show_grid(false),
        )
        .y_axis(
            Axis::new()
                .domain(&[0.0, 1.0])
                .anchor("x1")
                .range(vec![0.])
                .range_mode(RangeMode::ToZero)
                .show_grid(false),
        );

    println!("\n{}", "==== GENERATING PLOTS ====".bold().bright_cyan());
    plot_main.set_layout(layout_main);
    plot_highlight.set_layout(layout_highlight);
    plot_highlight2.set_layout(layout_highlight2);
    plot_main.write_html(fname.clone());
    plot_highlight.write_html(format!("{}/stats/jasmin_highlight.html", &html_dir));
    plot_highlight2.write_html(format!("{}/stats/jasmin_highlight2.html", &html_dir));

    let first_snap_time: String = collection
        .awrs
        .first()
        .unwrap()
        .snap_info
        .begin_snap_time
        .clone();
    let last_snap_time: String = collection
        .awrs
        .last()
        .unwrap()
        .snap_info
        .end_snap_time
        .clone();
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

    let report_html_scripts: String = format!(
        r#"
        <script>//JAS-MIN scripts
        function toggleTable(buttonId, tableId) {{
            const button = document.getElementById(buttonId);
            const table = document.getElementById(tableId);
            if (!button || !table) return;
        
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
            if (!checkbox || !element) return;
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
        </script>"#
    );

    let db_time_degradation_report = build_db_time_degradation_report(
        &collection,
        &snap_range,
        &x_vals,
        &y_vals_dbtime,
        &y_vals_dbcpu,
        &y_vals_events,
        &y_vals_sqls,
        &instance_stats,
        &args,
    );
    let db_time_degradation_button = if let Some(report) = db_time_degradation_report.as_ref() {
        let degradation_html = build_db_time_degradation_html(report);
        let degradation_filename = format!("{}/stats/db_time_degradation.html", &html_dir);
        if let Err(e) = fs::write(&degradation_filename, degradation_html) {
            eprintln!("Error writing file {}: {}", degradation_filename, e);
            String::new()
        } else {
            "<a href=\"stats/db_time_degradation.html\" target=\"_blank\" style=\"text-decoration: none;\">
                <button id=\"show-dbtime-degradation-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">DB Time Degradation</span><span>DB Time Degradation</span></button>
            </a>"
            .to_string()
        }
    } else {
        String::new()
    };
    report_for_ai.db_time_degradation_report = db_time_degradation_report;

    // Open plot_main HTML to inject Additional sections - Buttons, Tables, etc
    let mut plotly_html: String =
        fs::read_to_string(&fname).expect("Failed to read jasmin-html file");

    let mut nmon_button = String::new();
    if let Some(nmon) = collection.nmon.as_ref() {
        match crate::nmon::render::write_overview(nmon, Path::new(&html_dir)) {
            Ok(()) => {
                nmon_button = "<a href=\"nmon/nmon_overview.html\" target=\"_blank\"><button class=\"button-JASMIN\" role=\"button\"><span class=\"text\">NMON Host</span><span>NMON Host</span></button></a>".to_string();
            }
            Err(error) => eprintln!("⚠️ Failed to build NMON HTML overview: {error}"),
        }
    }

    const STYLE_CSS: &str = include_str!("../report/assets/style.css");
    plotly_html = plotly_html.replace(
        "<head>",
        &format!("<head>\n<title>JAS-MIN</title>\n{}", STYLE_CSS),
    );
    let jasmin_logo = jasmin_brand_banner_html();
    // Inject Buttons and Tables into Main HTML
    plotly_html = plotly_html.replace(
        "<body>",
        &format!("<body>\n{}\n\t{}\n\t<div class=\"jasmin-primary-actions\">\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t</div>\n\t{}\n\t{}\n\t{}\n\t{}\n\t{}\n\t",
            jasmin_logo,
            db_instance_info_html,
            "<button id=\"show-events-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Wait Events</span><span>TOP Wait Events</span></button>",
            "<button id=\"show-sqls-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Wait SQLs</span><span>TOP Wait SQLs</span></button>",
            "<button id=\"show-bgevents-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">TOP Backgrd Events</span><span>TOP Backgrd Events</span></button>",
            "<button id=\"show-anomalies-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">Anomalies Summary</span><span>Anomalies Summary</span></button>",
            format!(
                "<a href=\"stats/statistics_corr.html\" target=\"_blank\" style=\"text-decoration: none;\">
                    <button id=\"show-stat_corr-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">STATS Correlation</span><span>STATS Correlation</span></button>
                </a>
                <a href=\"stats/gradient.html\" target=\"_blank\" style=\"text-decoration: none;\">
                    <button id=\"show-stat_corr-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">DB Time Gradient Analyzes</span><span>DB Time Gradient Analyzes</span></button>
                </a>
                <a href=\"stats/gradient_cpu.html\" target=\"_blank\" style=\"text-decoration: none;\">
                    <button id=\"show-stat_corr-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">DB CPU Gradient Analyzes</span><span>DB CPU Gradient Analyzes</span></button>
                </a>
                <a href=\"stats/performance_hints.html\" target=\"_blank\" style=\"text-decoration: none;\">
                    <button id=\"show-hints-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">HINTS</span><span>HINTS</span></button>
                </a>
                {}
                {}
                {}",
                db_time_degradation_button,
                nmon_button,
                if !args.gradient_custom.is_empty() {
                    format!(
                        "<a href=\"stats/gradient_sqlid.html\" target=\"_blank\" style=\"text-decoration: none;\">
                            <button id=\"show-stat_corr-button\" class=\"button-JASMIN\" role=\"button\"><span class=\"text\">Gradient ({})</span><span>Custom Gradient ({})</span></button>
                        </a>",
                        args.gradient_custom, args.gradient_custom
                    )
                } else {
                    String::new()
                }
            ),
            event_table_html,
            bgevent_table_html,
            anomalies_summary_html,
            sqls_table_html,
            report_html_scripts)
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
    for func in iostats.keys() {
        let func_name = func.replace(" ", "_");
        let mut stats_explorer_html: String =
            fs::read_to_string(format!("{}/iostats/iostats_{}.html", &html_dir, func_name))
                .expect("Failed to read iostats file");
        stats_explorer_html = stats_explorer_html.replace(
            "plotly-html-element",
            &format!("iostat_{}-html-element", func_name),
        );
        fs::write(
            format!("{}/iostats/iostats_{}.html", &html_dir, func_name),
            &stats_explorer_html,
        );
        stats_explorer_html = stats_explorer_html
            .lines() // Iterate over lines
            .skip_while(|line| {
                !line.contains(&format!("<div id=\"iostat_{}-html-element\"", func_name))
            }) // Skip lines until found
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
        let mut stats_explorer_html: String =
            fs::read_to_string(format!("{}/segstats/segstats_{}.html", &html_dir, func))
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

    let ridge_lambda = args.ridge_lambda;
    let elastic_net_lambda = args.en_lambda;
    let elastic_net_alpha = args.en_alpha;
    let elastic_net_max_iter = args.en_max_iter;
    let elastic_net_tol = args.en_tol;

    let gradient_instance_stats = crate::measurements::instance_rates(&collection, &snap_range);
    let gradient_events = crate::measurements::domain_rates(
        &collection,
        &snap_range,
        &y_vals_events,
        "foreground_wait_events",
    );
    let gradient_sql_elapsed = crate::measurements::domain_rates(
        &collection,
        &snap_range,
        &y_vals_sqls,
        "sql_elapsed_time",
    );
    let gradient_sql_cpu = crate::measurements::domain_rates(
        &collection,
        &snap_range,
        &y_vals_sqls_cpu,
        "sql_cpu_time",
    );
    // Define all gradient sections declaratively
    let mut gradient_specs: Vec<(GradientSectionSpec, &str)> = vec![
        // (spec, field_name_tag) — field_name_tag used to dispatch into report_for_ai
        (
            GradientSectionSpec {
                observations: None,
                target: &y_vals_dbtime,
                features: gradient_events
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                label: "event_wait_s_per_s".to_string(),
                is_events: true,
                display_name: "DB TIME GRADIENT for wait events".to_string(),
            },
            "fg_wait_events",
        ),
        (
            GradientSectionSpec {
                observations: None,
                target: &y_vals_dbtime,
                features: gradient_instance_stats
                    .iter()
                    .filter(|(k, _)| is_counter_stat(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                label: "statistic_rate_counter".to_string(),
                is_events: false,
                display_name: "DB TIME GRADIENT for stats counters".to_string(),
            },
            "instance_stats_counters",
        ),
        (
            GradientSectionSpec {
                observations: None,
                target: &y_vals_dbtime,
                features: gradient_instance_stats
                    .iter()
                    .filter(|(k, _)| is_volume_stat(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                label: "statistic_rate_volume".to_string(),
                is_events: false,
                display_name: "DB TIME GRADIENT for stats volumes".to_string(),
            },
            "instance_stats_volumes",
        ),
        (
            GradientSectionSpec {
                observations: None,
                target: &y_vals_dbtime,
                features: gradient_instance_stats
                    .iter()
                    .filter(|(k, _)| is_time_stat(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                label: "statistic_rate_time".to_string(),
                is_events: false,
                display_name: "DB TIME GRADIENT for stats time".to_string(),
            },
            "instance_stats_time",
        ),
        (
            GradientSectionSpec {
                observations: None,
                target: &y_vals_dbtime,
                features: gradient_sql_elapsed
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                label: "SQL_elapsed_s_per_s".to_string(),
                is_events: false,
                display_name: "DB TIME GRADIENT for SQL elapsed time".to_string(),
            },
            "sql_elapsed_time",
        ),
        (
            GradientSectionSpec {
                observations: None,
                target: &y_vals_dbcpu,
                features: gradient_instance_stats
                    .iter()
                    .filter(|(k, _)| is_cpu_stat(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                label: "statistic_rate_cpu".to_string(),
                is_events: false,
                display_name: "CPU TIME GRADIENT for stats".to_string(),
            },
            "cpu_instance_stats",
        ),
        (
            GradientSectionSpec {
                observations: None,
                target: &y_vals_dbcpu,
                features: gradient_sql_cpu
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                label: "SQL_CPU_s_per_s".to_string(),
                is_events: false,
                display_name: "CPU TIME GRADIENT for SQL CPU".to_string(),
            },
            "cpu_sql_cpu_time",
        ),
    ];

    let mut custom_gradient = false;
    if !args.gradient_custom.is_empty() {
        let sql_id_event = args
            .gradient_custom
            .trim()
            .split("=")
            .collect::<Vec<&str>>();
        let mut target_data: Option<&Vec<f64>> = None;
        if sql_id_event[0] == "SQL" {
            target_data = gradient_sql_elapsed.get(sql_id_event[1]);
        } else if sql_id_event[0] == "WAIT" {
            target_data = gradient_events.get(sql_id_event[1]);
        } else {
            println!(
                "WARNING! parameter gradient_custom was wrongly specified as: {}",
                args.gradient_custom
            );
        }
        if let Some(t) = target_data {
            gradient_specs.push((
                GradientSectionSpec {
                    observations: None,
                    target: t,
                    features: gradient_instance_stats
                        .iter()
                        //.filter(|(k, _)| is_in_any_categhory(k))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    label: "custom_stats".to_string(),
                    is_events: false,
                    display_name: "Custom Gradient for stats".to_string(),
                },
                "instance_stats_for_sql_or_event",
            ));

            gradient_specs.push((
                GradientSectionSpec {
                    observations: None,
                    target: t,
                    features: gradient_events
                        .iter()
                        .filter(|(e, _)| *e != sql_id_event[1])
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    label: "custom_events".to_string(),
                    is_events: false,
                    display_name: "Custom Gradiant for events".to_string(),
                },
                "wait_event_for_sql_or_event",
            ));

            custom_gradient = true;
        }
    }

    // Preserve actual AWR membership independently of the zero-filled plotting
    // proxy. An omitted top-list row is not an observed zero execution cost.
    for (spec, tag) in &mut gradient_specs {
        if *tag == "sql_elapsed_time" || *tag == "cpu_sql_cpu_time" {
            let cpu = *tag == "cpu_sql_cpu_time";
            spec.observations = Some(
                spec.features
                    .keys()
                    .map(|sql_id| {
                        let mask = collection
                            .awrs
                            .iter()
                            .filter(|awr| {
                                awr.snap_info.begin_snap_id >= snap_range.0
                                    && awr.snap_info.end_snap_id <= snap_range.1
                            })
                            .map(|awr| {
                                if cpu {
                                    awr.sql_cpu_time.values().any(|row| row.sql_id == *sql_id)
                                } else {
                                    awr.sql_elapsed_time.iter().any(|row| row.sql_id == *sql_id)
                                }
                            })
                            .collect();
                        (sql_id.clone(), mask)
                    })
                    .collect(),
            );
        }
    }

    // Process all sections in a loop
    let mut gradient_results: HashMap<&str, String> = HashMap::new();

    for (spec, tag) in &gradient_specs {
        let (section, html) = run_gradient_section(
            spec,
            ridge_lambda,
            elastic_net_lambda,
            elastic_net_alpha,
            elastic_net_max_iter,
            elastic_net_tol,
            &logfile_name,
            &args,
        );

        // Dispatch into the correct field of report_for_ai
        match *tag {
            "fg_wait_events" => report_for_ai.db_time_gradient_fg_wait_events = section,
            "instance_stats_counters" => {
                report_for_ai.db_time_gradient_instance_stats_counters = section
            }
            "instance_stats_volumes" => {
                report_for_ai.db_time_gradient_instance_stats_volumes = section
            }
            "instance_stats_time" => report_for_ai.db_time_gradient_instance_stats_time = section,
            "sql_elapsed_time" => report_for_ai.db_time_gradient_sql_elapsed_time = section,
            "cpu_instance_stats" => report_for_ai.db_cpu_gradient_instance_stats = section,
            "cpu_sql_cpu_time" => report_for_ai.db_cpu_gradient_sql_cpu_time = section,
            "instance_stats_for_sql_or_event" => {
                report_for_ai.custom_gradient_instance_stats = section
            }
            "wait_event_for_sql_or_event" => report_for_ai.custom_gradient_wait_events = section,
            _ => {}
        }

        gradient_results.insert(tag, html);
    }

    // Extract HTML for template rendering
    let gradient_events = gradient_results
        .remove("fg_wait_events")
        .unwrap_or_default();
    let gradient_stats_cnt = gradient_results
        .remove("instance_stats_counters")
        .unwrap_or_default();
    let gradient_stats_volume = gradient_results
        .remove("instance_stats_volumes")
        .unwrap_or_default();
    let gradient_stats_time = gradient_results
        .remove("instance_stats_time")
        .unwrap_or_default();
    let gradient_sqls = gradient_results
        .remove("sql_elapsed_time")
        .unwrap_or_default();
    let gradient_cpu_stats_all = gradient_results
        .remove("cpu_instance_stats")
        .unwrap_or_default();
    let gradient_cpu_sqls = gradient_results
        .remove("cpu_sql_cpu_time")
        .unwrap_or_default();

    // ---- DB Time gradient page ----
    let db_time_sections = vec![
        GradientHtmlSection {
            heading: "DB Time vs Wait Events".to_string(),
            html: gradient_events,
        },
        GradientHtmlSection {
            heading: "DB Time vs SQLs".to_string(),
            html: gradient_sqls,
        },
        GradientHtmlSection {
            heading: "DB Time vs Statistic Counters".to_string(),
            html: gradient_stats_cnt,
        },
        GradientHtmlSection {
            heading: "DB Time vs Statistic Volumes".to_string(),
            html: gradient_stats_volume,
        },
        GradientHtmlSection {
            heading: "DB Time vs Statistic Time".to_string(),
            html: gradient_stats_time,
        },
    ];

    let gradient_html =
        build_gradient_html("Gradient Analyzes", "Gradient Analyzes", db_time_sections);
    let gradient_html = add_links_to_html(
        gradient_html,
        events_sqls.clone(),
        "..".to_string(),
        html_dir.clone(),
    );
    let gradient_filename: String = format!("{}/stats/gradient.html", &html_dir);
    if let Err(e) = fs::write(&gradient_filename, gradient_html) {
        eprintln!("Error writing file {}: {}", gradient_filename, e);
    }

    // ---- DB CPU gradient page ----
    let db_cpu_sections = vec![
        GradientHtmlSection {
            heading: "DB CPU vs SQLs by CPU Time".to_string(),
            html: gradient_cpu_sqls,
        },
        GradientHtmlSection {
            heading: "DB CPU vs Instance Statistics".to_string(),
            html: gradient_cpu_stats_all,
        },
    ];

    let gradient_html =
        build_gradient_html("Gradient Analyzes", "Gradient Analyzes", db_cpu_sections);
    let gradient_html = add_links_to_html(
        gradient_html,
        events_sqls.clone(),
        "..".to_string(),
        html_dir.clone(),
    );
    let gradient_filename: String = format!("{}/stats/gradient_cpu.html", &html_dir);
    if let Err(e) = fs::write(&gradient_filename, gradient_html) {
        eprintln!("Error writing file {}: {}", gradient_filename, e);
    }

    if custom_gradient {
        // Extract HTML for template rendering
        let sql_instance_stats = gradient_results
            .remove("instance_stats_for_sql_or_event")
            .unwrap_or_default();
        let sql_wait_events = gradient_results
            .remove("wait_event_for_sql_or_event")
            .unwrap_or_default();
        let sql_gradient_sections = vec![
            GradientHtmlSection {
                heading: "Instance Stats".to_string(),
                html: sql_instance_stats,
            },
            GradientHtmlSection {
                heading: "Wait Events".to_string(),
                html: sql_wait_events,
            },
        ];

        let gradient_html = build_gradient_html(
            "Gradient Analyzes",
            "Gradient Analyzes",
            sql_gradient_sections,
        );

        let gradient_html = add_links_to_html(
            gradient_html,
            events_sqls,
            "..".to_string(),
            html_dir.clone(),
        );
        let gradient_filename: String = format!("{}/stats/gradient_sqlid.html", &html_dir);
        if let Err(e) = fs::write(&gradient_filename, gradient_html) {
            eprintln!("Error writing file {}: {}", gradient_filename, e);
        }
    }

    let mut hints = crate::performance_hints::build(
        &collection,
        &snap_range,
        crate::performance_hints::Policy::load(&args.hints_policy)
            .expect("HINTS policy was validated before analysis"),
    );
    // Use the same optional plan attachments as the API. Missing plans retain
    // provisional hints; matched plan hashes add object context, never causality.
    let hints_stem = if args.json_file().is_empty() {
        args.directory().trim_end_matches('/').to_string()
    } else {
        PathBuf::from(args.json_file())
            .with_extension("")
            .to_string_lossy()
            .into_owned()
    };
    crate::performance_hints::enrich_with_plans(&mut hints, collection, &hints_stem);
    fs::write(
        format!("{}/stats/performance_hints.html", &html_dir),
        crate::performance_hints::render_html(&hints),
    )
    .expect("Failed to write HINTS report");
    report_for_ai.performance_hints = Some(hints);

    // Write the updated HTML back to the file
    fs::write(&fname, plotly_html).expect("Failed to write updated Plotly HTML file");
    println!("{}", "\n==== DONE ===".bold().bright_cyan());
    println!("{}{}\n", "JAS-MIN Report saved to: ", &fname);

    open::that(fname);

    /* Clear gradient description to minimalyze token usage */
    strip_gradient_descriptions(&mut report_for_ai);
    /* ***************************************************** */

    report_for_ai.initialization_parameters = collection.initialization_parameters.clone();
    debug_note!(
        "Main report build completed: snapshots={}, serialized_report_bytes={}",
        collection.awrs.len(),
        serde_json::to_vec(&report_for_ai).map_or(0, |value| value.len())
    );
    report_for_ai
}

#[cfg(test)]
mod instance_efficiency_tests {
    use super::*;

    // A metric appearing after the first snapshot must retain all gaps on its time axis.
    #[test]
    fn instance_efficiency_plot_preserves_missing_snapshots() {
        let mut reports = vec![AWR::default(); 4];
        for (index, report) in reports.iter_mut().enumerate() {
            report.snap_info.begin_snap_id = index as u64;
            report.snap_info.end_snap_id = index as u64 + 1;
        }
        for index in [1, 3] {
            reports[index]
                .instance_efficiency
                .push(crate::awr::InstanceEfficiency {
                    eff_stat: "Soft Parse %".into(),
                    eff_pct: Some(90.0 + index as f32),
                });
        }
        let rendered = generate_instance_efficiency_plot(&reports, &(0, 4), "");
        assert!(rendered.contains("Soft Parse %"));
        assert!(rendered.contains("[null,91.0,null,93.0]"));
        let filtered = generate_instance_efficiency_plot(&reports, &(2, 4), "");
        assert!(filtered.contains("[null,93.0]"));
        assert!(!filtered.contains("91.0"));
        assert!(!generate_instance_efficiency_plot(&Vec::new(), &(0, 4), "").is_empty());
    }
}
