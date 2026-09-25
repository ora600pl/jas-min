use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use crate::reasonings::ReportForAI;

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct LoadProfile {
    pub stat_name: String,
    pub per_second: f64,
    pub(crate) per_transaction: f64,
    //pub begin_snap_time: String,
}
#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct InstanceEfficiency {
    //Target is 100%
    pub eff_stat: String,
    pub eff_pct: Option<f32>,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct RedoLog {
    pub stat_name: String,
    pub per_hour: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct DBInstance {
    //db_name: String,
    pub db_id: u64,
    //instance_name: String,
    pub instance_num: u8,
    pub startup_time: String,
    pub release: String,
    pub rac: String,
    pub platform: String,
    pub cpus: u16,
    pub cores: u16,
    pub sockets: u8,
    pub memory: u16,
    pub db_block_size: u16,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct WaitClasses {
    pub(crate) wait_class: String,
    pub(crate) waits: u64,
    pub(crate) total_wait_time_s: f64,
    pub(crate) avg_wait_ms: f64,
    pub(crate) db_time_pct: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct HostCPU {
    pub cpus: u32,
    pub cores: u32,
    pub sockets: u8,
    pub load_avg_begin: f64,
    pub load_avg_end: f64,
    pub pct_user: f64,
    pub pct_system: f64,
    pub pct_wio: f64,
    pub pct_idle: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct TimeModelStats {
    pub stat_name: String,
    pub time_s: f64,
    pub pct_dbtime: f64,
    //begin_snap_time: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct WaitEvents {
    pub event: String,
    pub waits: u64,
    pub total_wait_time_s: f64,
    pub avg_wait: f64,
    pub pct_dbtime: f64,
    //begin_snap_time: String,
    pub waitevent_histogram_ms: BTreeMap<String, f32>,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct SQLElapsedTime {
    pub sql_id: String,
    pub elapsed_time_s: f64,
    pub executions: u64,
    pub elpased_time_exec_s: f64,
    pub pct_total: f64,
    pub pct_cpu: f64,
    pub pct_io: f64,
    pub sql_module: String,
    pub sql_type: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct SQLCPUTime {
    pub sql_id: String,
    pub cpu_time_s: f64,
    pub executions: u64,
    pub cpu_time_exec_s: f64,
    pub pct_total: f64,
    pub pct_cpu: f64,
    pub pct_io: f64,
    pub sql_module: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct SQLIOTime {
    pub sql_id: String,
    pub io_time_s: f64,
    pub executions: u64,
    pub io_time_exec_s: f64,
    pub pct_total: f64,
    pub pct_cpu: f64,
    pub pct_io: f64,
    pub(crate) sql_module: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct SQLGets {
    pub sql_id: String,
    pub buffer_gets: f64,
    pub executions: u64,
    pub gets_per_exec: f64,
    pub pct_total: f64,
    pub pct_cpu: f64,
    pub pct_io: f64,
    pub(crate) sql_module: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct SQLReads {
    pub sql_id: String,
    pub physical_reads: f64,
    pub executions: u64,
    pub reads_per_exec: f64,
    pub pct_total: f64,
    pub cpu_time_pct: f64, //in Statspack it is CPU Time - in AWR it is PCT CPU
    pub pct_io: f64,       //doesn't exists in statspack
    pub(crate) sql_module: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct SnapInfo {
    pub begin_snap_id: u64,
    pub end_snap_id: u64,
    pub begin_snap_time: String,
    pub end_snap_time: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct InstanceStats {
    pub statname: String,
    pub total: u64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct IOStats {
    pub reads_data: f64, // in MB
    pub reads_req_s: f64,
    pub reads_data_s: f64, // in MB
    pub writes_data: f64,  // in MB
    pub writes_req_s: f64,
    pub writes_data_s: f64, // in MB
    pub waits_count: u64,
    pub avg_time: Option<f64>, // in ms
}

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct GetStats {
    pub samples: u64,
    pub min: f64,
    pub lower_fence: f64,
    pub q1: f64,
    pub mean: f64,
    pub median: f64,
    pub q3: f64,
    pub upper_fence: f64,
    pub max: f64,
    pub variance: f64,
    pub std_dev: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct DictionaryCache {
    pub statname: String,
    pub get_requests: u64,
    pub final_usage: u64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct LibraryCache {
    pub statname: String,
    pub get_requests: u64,
    pub get_pct_miss: f64,
    pub pin_requests: u64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct LatchActivity {
    pub statname: String,
    pub get_requests: u64,
    pub get_pct_miss: f64,
    pub wait_time: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct SegmentStats {
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub pdb_name: Option<String>,
    #[serde(default)]
    pub con_id: Option<u32>,
    #[serde(default)]
    pub subobject_name: Option<String>,
    pub obj: u64,
    pub objd: u64,
    pub object_name: String,
    pub object_type: String,
    pub stat_name: String,
    pub stat_vlalue: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct TopSQLWithTopEvents {
    pub sql_id: String,
    pub plan_hash_value: u64,
    pub executions: u64,
    pub pct_activity: f64,
    pub event_name: String,
    pub pct_event: f64,
    pub top_row_source: String,
    pub pct_row_source: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct AWR {
    /// False means not collected, even when a legacy numeric field contains zero.
    #[serde(default)]
    pub data_availability: HashMap<String, bool>,
    #[serde(default)]
    pub access_path_observations: Vec<crate::access_path::SqlObservation>,
    pub file_name: String,
    pub snap_info: SnapInfo,
    pub(crate) status: String,
    pub load_profile: Vec<LoadProfile>,
    pub instance_efficiency: Vec<InstanceEfficiency>,
    pub redo_log: RedoLog,
    pub(crate) wait_classes: Vec<WaitClasses>,
    pub host_cpu: HostCPU,
    pub time_model_stats: Vec<TimeModelStats>,
    pub foreground_wait_events: Vec<WaitEvents>,
    pub background_wait_events: Vec<WaitEvents>,
    pub sql_elapsed_time: Vec<SQLElapsedTime>,
    pub sql_cpu_time: HashMap<String, SQLCPUTime>,
    pub sql_io_time: HashMap<String, SQLIOTime>,
    pub sql_gets: HashMap<String, SQLGets>,
    pub sql_reads: HashMap<String, SQLReads>,
    pub top_sql_with_top_events: HashMap<String, TopSQLWithTopEvents>,
    pub instance_stats: Vec<InstanceStats>,
    pub dictionary_cache: Vec<DictionaryCache>,
    pub io_stats_byfunc: HashMap<String, IOStats>,
    pub library_cache: Vec<LibraryCache>,
    pub latch_activity: Vec<LatchActivity>,
    pub segment_stats: HashMap<String, Vec<SegmentStats>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AWRSCollection {
    pub db_instance_information: DBInstance,
    pub initialization_parameters: HashMap<String, String>,
    pub awrs: Vec<AWR>,
    pub sql_text: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nmon: Option<crate::nmon::NmonDataset>,
}

/// Keeps the detailed collection and its compact AI summary together.
///
/// MCP needs both views after parsing: `AWRSCollection` serves narrow evidence
/// queries while `ReportForAI` provides the precomputed statistical seed.
pub struct ParsedAnalysis {
    pub collection: AWRSCollection,
    pub report_for_ai: ReportForAI,
}
