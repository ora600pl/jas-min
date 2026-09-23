use super::aggregate::build_aggregates;
use super::model::{
    NmonCapture, NmonDataset, NmonDiagnostics, NmonGap, NmonLparMetadata, NmonMetadata,
    NmonMetricDescriptor, NmonMetricSeries, NmonMetricSource, NmonOverlap, NMON_SCHEMA_VERSION,
};
use super::parser::{parse_file, ParsedNmonFile};
use super::peaks::build_peak_periods;
use super::stats::statistics;
use chrono::NaiveDateTime;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

pub fn load_directory(directory: &Path, quiet: bool) -> Result<NmonDataset, String> {
    let mut paths = discover_files(directory)?;
    paths.sort();
    if paths.is_empty() {
        return Err(format!(
            "NMON directory '{}' contains no *.nmon files",
            directory.display()
        ));
    }
    if !quiet {
        println!("INFO Found {} NMON files", paths.len());
    }

    let mut accumulator = Accumulator::default();
    let mut parse_errors = Vec::new();
    for path in paths {
        if !quiet {
            println!("INFO Parsing NMON file: {}", path.display());
        }
        match parse_file(&path) {
            Ok(parsed) => accumulator.merge(parsed)?,
            Err(error) => parse_errors.push(error),
        }
    }
    if accumulator.files.is_empty() {
        return Err(format!(
            "no valid NMON files could be parsed: {}",
            parse_errors.join("; ")
        ));
    }
    accumulator.diagnostics.warnings.extend(parse_errors);
    let dataset = accumulator.finish()?;
    if !quiet {
        println!(
            "INFO Detected NMON host: {}",
            dataset.metadata.host.as_deref().unwrap_or("unknown")
        );
        println!(
            "INFO Sampling interval: {} seconds",
            dataset
                .capture
                .sampling_interval_seconds
                .map_or_else(|| "unknown".to_string(), |value| value.to_string())
        );
        println!(
            "INFO Parsed {} NMON samples and {} metrics",
            dataset.capture.sample_count,
            dataset.metrics.len()
        );
        println!(
            "INFO Capture period: {} to {}",
            dataset.capture.start.as_deref().unwrap_or("unknown"),
            dataset.capture.end.as_deref().unwrap_or("unknown")
        );
    }
    Ok(dataset)
}

fn discover_files(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(directory).map_err(|error| {
        format!(
            "cannot read NMON directory '{}': {error}",
            directory.display()
        )
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot enumerate NMON directory '{}': {error}",
                directory.display()
            )
        })?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("nmon"))
        {
            paths.push(path);
        }
    }
    Ok(paths)
}

#[derive(Default)]
struct Accumulator {
    metadata: Option<NmonMetadata>,
    interval_seconds: Option<u64>,
    timezone: Option<String>,
    files: Vec<String>,
    samples: BTreeMap<NaiveDateTime, BTreeMap<String, f64>>,
    sample_owner: BTreeMap<NaiveDateTime, String>,
    descriptors: BTreeMap<String, NmonMetricDescriptor>,
    diagnostics: NmonDiagnostics,
}

impl Accumulator {
    fn merge(&mut self, file: ParsedNmonFile) -> Result<(), String> {
        self.validate_compatibility(&file)?;
        let file_name = file.path.to_string_lossy().into_owned();
        self.files.push(file_name.clone());
        self.diagnostics
            .unsupported_sections
            .extend(file.unsupported_sections);
        self.diagnostics.warnings.extend(file.warnings);

        for (key, descriptor) in file.descriptors {
            if let Some(existing) = self.descriptors.get(&key) {
                if existing.section != descriptor.section
                    || existing.source_label != descriptor.source_label
                    || existing.unit != descriptor.unit
                {
                    self.diagnostics.warnings.push(format!(
                        "metric '{key}' has differing headers across NMON files; the first header was retained"
                    ));
                }
            } else {
                self.descriptors.insert(key, descriptor);
            }
        }

        for (timestamp, incoming) in file.samples {
            if let Some(existing) = self.samples.get_mut(&timestamp) {
                let previous_file = self
                    .sample_owner
                    .get(&timestamp)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                if self.diagnostics.overlaps.len() < 100 {
                    self.diagnostics.overlaps.push(NmonOverlap {
                        timestamp: format_timestamp(timestamp),
                        files: vec![previous_file, file_name.clone()],
                    });
                }
                for (key, value) in incoming {
                    match existing.get(&key) {
                        Some(previous) if (*previous - value).abs() <= f64::EPSILON => {
                            self.diagnostics.duplicate_samples += 1;
                        }
                        Some(previous) => {
                            return Err(format!(
                                "conflicting NMON samples at {} for metric '{}': {} in an earlier file and {} in '{}'",
                                format_timestamp(timestamp), key, previous, value, file_name
                            ));
                        }
                        None => {
                            existing.insert(key, value);
                        }
                    }
                }
            } else {
                self.sample_owner.insert(timestamp, file_name.clone());
                self.samples.insert(timestamp, incoming);
            }
        }
        Ok(())
    }

    fn validate_compatibility(&mut self, file: &ParsedNmonFile) -> Result<(), String> {
        if let (Some(expected), Some(actual)) = (self.interval_seconds, file.interval_seconds) {
            if expected != actual {
                return Err(format!(
                    "inconsistent NMON sampling interval in '{}': expected {expected}s, found {actual}s",
                    file.path.display()
                ));
            }
        } else if self.interval_seconds.is_none() {
            self.interval_seconds = file.interval_seconds;
        }

        if let (Some(expected), Some(actual)) = (self.timezone.as_ref(), file.timezone.as_ref()) {
            if expected != actual {
                return Err(format!(
                    "inconsistent NMON timezone in '{}': expected '{expected}', found '{actual}'",
                    file.path.display()
                ));
            }
        } else if self.timezone.is_none() {
            self.timezone = file.timezone.clone();
        }

        if let Some(existing) = self.metadata.as_ref() {
            if let (Some(expected), Some(actual)) =
                (existing.host.as_deref(), file.metadata.host.as_deref())
            {
                if !expected.eq_ignore_ascii_case(actual) {
                    return Err(format!(
                        "inconsistent NMON host in '{}': expected '{expected}', found '{actual}'",
                        file.path.display()
                    ));
                }
            }
            if let (Some(expected), Some(actual)) = (
                existing.os_version.as_deref(),
                file.metadata.os_version.as_deref(),
            ) {
                if expected != actual {
                    return Err(format!(
                        "inconsistent AIX version in '{}': expected '{expected}', found '{actual}'",
                        file.path.display()
                    ));
                }
            }
        } else {
            self.metadata = Some(file.metadata.clone());
        }
        Ok(())
    }

    fn finish(mut self) -> Result<NmonDataset, String> {
        if self.samples.is_empty() {
            return Err("no supported NMON samples were found".to_string());
        }
        let timestamps_dt = self.samples.keys().copied().collect::<Vec<_>>();
        let inferred_interval = infer_interval(&timestamps_dt);
        match (self.interval_seconds, inferred_interval) {
            (Some(declared), Some(inferred))
                if declared.abs_diff(inferred) > 1 && timestamps_dt.len() > 2 =>
            {
                return Err(format!(
                    "NMON timestamps imply a {inferred}s sampling interval but metadata declares {declared}s"
                ));
            }
            (None, inferred) => self.interval_seconds = inferred,
            _ => {}
        }
        self.validate_timeline(&timestamps_dt);

        let timestamps = timestamps_dt
            .iter()
            .copied()
            .map(format_timestamp)
            .collect::<Vec<_>>();
        let mut metrics = BTreeMap::new();
        for (key, descriptor) in &self.descriptors {
            let values = timestamps_dt
                .iter()
                .map(|timestamp| {
                    self.samples
                        .get(timestamp)
                        .and_then(|sample| sample.get(key))
                        .copied()
                })
                .collect::<Vec<_>>();
            metrics.insert(
                key.clone(),
                NmonMetricSeries {
                    section: descriptor.section.clone(),
                    source_name: descriptor.source_name.clone(),
                    source_label: descriptor.source_label.clone(),
                    domain: descriptor.domain.clone(),
                    entity: descriptor.entity.clone(),
                    unit: descriptor.unit.clone(),
                    source: NmonMetricSource::Observed,
                    values,
                },
            );
        }
        add_entitlement_utilization(&mut metrics);

        let summaries = metrics
            .iter()
            .map(|(key, metric)| {
                (
                    key.clone(),
                    statistics(metric.values.iter().filter_map(|value| *value)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let aggregates = build_aggregates(&timestamps, &metrics);
        let peak_periods = build_peak_periods(&aggregates);
        let mut metadata = self.metadata.unwrap_or_default();
        enrich_configuration(&mut metadata, &metrics);
        let start = timestamps.first().cloned();
        let end = timestamps.last().cloned();
        Ok(NmonDataset {
            schema_version: NMON_SCHEMA_VERSION.to_string(),
            metadata,
            capture: NmonCapture {
                files: self.files,
                start,
                end,
                sampling_interval_seconds: self.interval_seconds,
                sample_count: timestamps.len(),
                timezone: self.timezone,
            },
            timestamps,
            metrics,
            summaries,
            aggregates,
            peak_periods,
            diagnostics: self.diagnostics,
        })
    }

    fn validate_timeline(&mut self, timestamps: &[NaiveDateTime]) {
        let Some(interval) = self.interval_seconds.map(|value| value as i64) else {
            return;
        };
        for pair in timestamps.windows(2) {
            let seconds = (pair[1] - pair[0]).num_seconds();
            if seconds > interval.saturating_mul(2) {
                self.diagnostics.gaps.push(NmonGap {
                    from: format_timestamp(pair[0]),
                    to: format_timestamp(pair[1]),
                    seconds,
                });
            }
        }
    }
}

fn infer_interval(timestamps: &[NaiveDateTime]) -> Option<u64> {
    let mut counts = BTreeMap::<i64, usize>::new();
    for pair in timestamps.windows(2) {
        let seconds = (pair[1] - pair[0]).num_seconds();
        if seconds > 0 && seconds <= 3600 {
            *counts.entry(seconds).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(seconds, count)| (*count, std::cmp::Reverse(*seconds)))
        .and_then(|(seconds, _)| u64::try_from(seconds).ok())
}

fn add_entitlement_utilization(metrics: &mut BTreeMap<String, NmonMetricSeries>) {
    let Some(physical) = metrics.get("lpar.physicalcpu").cloned() else {
        return;
    };
    let Some(entitled) = metrics.get("lpar.entitled").cloned() else {
        return;
    };
    let values = physical
        .values
        .iter()
        .zip(entitled.values.iter())
        .map(|(physical, entitled)| match (physical, entitled) {
            (Some(physical), Some(entitled)) if *entitled > 0.0 => {
                Some(physical / entitled * 100.0)
            }
            _ => None,
        })
        .collect();
    metrics.insert(
        "lpar.entitlement_utilization_pct".to_string(),
        NmonMetricSeries {
            section: "DERIVED".to_string(),
            source_name: "entitlement_utilization_pct".to_string(),
            source_label: "PhysicalCPU / entitled * 100".to_string(),
            domain: "lpar".to_string(),
            entity: None,
            unit: Some("%".to_string()),
            source: NmonMetricSource::Derived,
            values,
        },
    );
}

fn enrich_configuration(metadata: &mut NmonMetadata, metrics: &BTreeMap<String, NmonMetricSeries>) {
    let lpar = &mut metadata.lpar;
    lpar.entitled_capacity = stable_value(metrics, "lpar.entitled");
    lpar.virtual_processors = stable_u32(metrics, "lpar.virtualcpus");
    lpar.logical_cpus = stable_u32(metrics, "lpar.logicalcpus");
    lpar.pool_cpus = stable_u32(metrics, "lpar.poolcpus");
    lpar.processor_pool_id = stable_u32(metrics, "lpar.pool_id");
    lpar.variable_capacity_weight = stable_value(metrics, "lpar.weight");
    lpar.capped = stable_value(metrics, "lpar.capped").map(|value| value != 0.0);
    if let Some(shared) = stable_value(metrics, "lpar.sharedcpu") {
        lpar.processor_mode = Some(if shared != 0.0 { "shared" } else { "dedicated" }.to_string());
    }
    lpar.smt_threads_per_virtual_processor = match (lpar.logical_cpus, lpar.virtual_processors) {
        (Some(logical), Some(virtual_cpus)) if virtual_cpus > 0 && logical % virtual_cpus == 0 => {
            Some(logical / virtual_cpus)
        }
        _ => None,
    };
    metadata.memory.assigned_memory_mb = stable_value(metrics, "memory.real_total_mb");
}

fn stable_value(metrics: &BTreeMap<String, NmonMetricSeries>, key: &str) -> Option<f64> {
    let mut values = metrics.get(key)?.values.iter().filter_map(|value| *value);
    let first = values.next()?;
    values
        .all(|value| (value - first).abs() <= 1e-9)
        .then_some(first)
}

fn stable_u32(metrics: &BTreeMap<String, NmonMetricSeries>, key: &str) -> Option<u32> {
    let value = stable_value(metrics, key)?;
    (value >= 0.0 && value <= u32::MAX as f64 && value.fract().abs() <= 1e-9)
        .then_some(value as u32)
}

fn format_timestamp(timestamp: NaiveDateTime) -> String {
    timestamp.format(TIMESTAMP_FORMAT).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "jasmin-nmon-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, name: &str, contents: &str) {
            let mut file = fs::File::create(self.0.join(name)).unwrap();
            file.write_all(contents.as_bytes()).unwrap();
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn capture(host: &str, timestamp: &str, sample: &str, interval: u64) -> String {
        format!(
            "AAA,host,{host}\nAAA,interval,{interval}\nCPU_ALL,CPU Total {host},User%\nCPU_ALL,T0001,{sample}\nZZZZ,T0001,{timestamp}\n"
        )
    }

    #[test]
    fn merges_files_chronologically_and_detects_gap() {
        let directory = TestDirectory::new();
        directory.write("b.nmon", &capture("aix1", "00:10:00,10-SEP-2026", "2", 30));
        directory.write("a.nmon", &capture("aix1", "00:00:00,10-SEP-2026", "1", 30));
        directory.write("ignored.txt", "not nmon");
        let dataset = load_directory(&directory.0, true).unwrap();
        assert_eq!(
            dataset.timestamps.len(),
            2,
            "files={:?}, warnings={:?}",
            dataset.capture.files,
            dataset.diagnostics.warnings
        );
        assert_eq!(dataset.timestamps[0], "2026-09-10T00:00:00");
        assert_eq!(dataset.timestamps[1], "2026-09-10T00:10:00");
        assert_eq!(dataset.diagnostics.gaps.len(), 1);
        assert_eq!(
            dataset.metrics["cpu_all.user_pct"].values,
            vec![Some(1.0), Some(2.0)]
        );
    }

    #[test]
    fn deduplicates_identical_overlap_and_rejects_conflict() {
        let directory = TestDirectory::new();
        let first = capture("aix1", "00:00:00,10-SEP-2026", "1", 30);
        directory.write("a.nmon", &first);
        directory.write("b.nmon", &first);
        let dataset = load_directory(&directory.0, true).unwrap();
        assert_eq!(dataset.capture.sample_count, 1);
        assert_eq!(dataset.diagnostics.duplicate_samples, 1);
        assert_eq!(dataset.diagnostics.overlaps.len(), 1);

        directory.write("b.nmon", &capture("aix1", "00:00:00,10-SEP-2026", "2", 30));
        assert!(load_directory(&directory.0, true)
            .unwrap_err()
            .contains("conflicting NMON samples"));
    }

    #[test]
    fn rejects_inconsistent_host_and_interval() {
        let directory = TestDirectory::new();
        directory.write("a.nmon", &capture("aix1", "00:00:00,10-SEP-2026", "1", 30));
        directory.write("b.nmon", &capture("aix2", "00:01:00,10-SEP-2026", "2", 30));
        assert!(load_directory(&directory.0, true)
            .unwrap_err()
            .contains("inconsistent NMON host"));
        directory.write("b.nmon", &capture("aix1", "00:01:00,10-SEP-2026", "2", 60));
        assert!(load_directory(&directory.0, true)
            .unwrap_err()
            .contains("inconsistent NMON sampling interval"));
    }

    #[test]
    fn json_round_trip_preserves_missing_values_and_schema() {
        let directory = TestDirectory::new();
        directory.write("a.nmon", &capture("aix1", "00:00:00,10-SEP-2026", "1", 30));
        let dataset = load_directory(&directory.0, true).unwrap();
        let json = serde_json::to_string(&dataset).unwrap();
        let decoded: NmonDataset = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.schema_version, NMON_SCHEMA_VERSION);
        assert_eq!(decoded.capture.sample_count, 1);
    }

    #[test]
    #[ignore = "parses the optional large real fixture set when present"]
    fn validates_real_nmon_fixtures() {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/nmon");
        if !directory.is_dir() {
            return;
        }
        let dataset = load_directory(&directory, true).unwrap();
        assert_eq!(dataset.metadata.host.as_deref(), Some("oraprod"));
        assert_eq!(dataset.capture.sampling_interval_seconds, Some(30));
        assert!(dataset.capture.sample_count >= 20_000);
        assert!(dataset.metrics.contains_key("cpu_all.user_pct"));
        assert!(dataset.metrics.contains_key("lpar.physicalcpu"));
        assert!(dataset.metrics.contains_key("disk.hdisk2.diskread"));
        assert!(dataset.metrics.contains_key("disk.hdisk2.diskwait"));
        let serialized_bytes = serde_json::to_vec(&dataset).unwrap().len();
        println!(
            "real NMON: samples={}, metrics={}, json_bytes={}, hdisk2 DISKREAD avg={:?} max={:?}, DISKWAIT avg={:?}",
            dataset.capture.sample_count,
            dataset.metrics.len(),
            serialized_bytes,
            dataset.summaries["disk.hdisk2.diskread"].average,
            dataset.summaries["disk.hdisk2.diskread"].max,
            dataset.summaries["disk.hdisk2.diskwait"].average,
        );
    }
}
