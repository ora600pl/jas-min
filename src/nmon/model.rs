use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const NMON_SCHEMA_VERSION: &str = "1.0";

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonDataset {
    pub schema_version: String,
    pub metadata: NmonMetadata,
    pub capture: NmonCapture,
    pub timestamps: Vec<String>,
    pub metrics: BTreeMap<String, NmonMetricSeries>,
    pub summaries: BTreeMap<String, NmonStatistics>,
    pub aggregates: NmonAggregates,
    pub peak_periods: Vec<NmonPeakPeriod>,
    pub diagnostics: NmonDiagnostics,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonMetadata {
    pub host: Option<String>,
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub nmon_version: Option<String>,
    pub architecture: Option<String>,
    pub lpar: NmonLparMetadata,
    pub memory: NmonMemoryMetadata,
    /// Original AAA values are retained so version-specific metadata is not lost.
    pub source_metadata: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonLparMetadata {
    pub partition_id: Option<u32>,
    pub partition_name: Option<String>,
    pub processor_mode: Option<String>,
    pub capped: Option<bool>,
    pub entitled_capacity: Option<f64>,
    pub virtual_processors: Option<u32>,
    pub logical_cpus: Option<u32>,
    pub pool_cpus: Option<u32>,
    pub processor_pool_id: Option<u32>,
    pub variable_capacity_weight: Option<f64>,
    pub subprocessor_mode: Option<String>,
    pub smt_threads_per_virtual_processor: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonMemoryMetadata {
    pub assigned_memory_mb: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonCapture {
    pub files: Vec<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub sampling_interval_seconds: Option<u64>,
    pub sample_count: usize,
    pub timezone: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonMetricSeries {
    pub section: String,
    pub source_name: String,
    pub source_label: String,
    pub domain: String,
    pub entity: Option<String>,
    pub unit: Option<String>,
    pub source: NmonMetricSource,
    pub values: Vec<Option<f64>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NmonMetricSource {
    #[default]
    Observed,
    Derived,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct NmonStatistics {
    pub count: usize,
    pub min: Option<f64>,
    pub average: Option<f64>,
    pub median: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
    pub max: Option<f64>,
    pub standard_deviation: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonAggregates {
    pub five_minutes: NmonAggregateWindow,
    pub fifteen_minutes: NmonAggregateWindow,
    pub one_hour: NmonAggregateWindow,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonAggregateWindow {
    pub duration_seconds: u64,
    pub starts: Vec<String>,
    pub metrics: BTreeMap<String, NmonAggregateSeries>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonAggregateSeries {
    pub count: Vec<usize>,
    pub min: Vec<Option<f64>>,
    pub average: Vec<Option<f64>>,
    pub p95: Vec<Option<f64>>,
    pub max: Vec<Option<f64>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonPeakPeriod {
    pub metric_key: String,
    pub duration_seconds: u64,
    pub start: String,
    pub end: String,
    pub statistics: NmonStatistics,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonGap {
    pub from: String,
    pub to: String,
    pub seconds: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonOverlap {
    pub timestamp: String,
    pub files: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NmonDiagnostics {
    pub warnings: Vec<String>,
    pub unsupported_sections: BTreeSet<String>,
    pub duplicate_samples: usize,
    pub overlaps: Vec<NmonOverlap>,
    pub gaps: Vec<NmonGap>,
}

#[derive(Debug, Clone)]
pub(crate) struct NmonMetricDescriptor {
    pub section: String,
    pub source_name: String,
    pub source_label: String,
    pub domain: String,
    pub entity: Option<String>,
    pub unit: Option<String>,
}
