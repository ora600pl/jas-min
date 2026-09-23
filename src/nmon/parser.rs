use super::model::{NmonMetadata, NmonMetricDescriptor};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct ParsedNmonFile {
    pub path: PathBuf,
    pub metadata: NmonMetadata,
    pub interval_seconds: Option<u64>,
    pub timezone: Option<String>,
    pub samples: BTreeMap<NaiveDateTime, BTreeMap<String, f64>>,
    pub descriptors: BTreeMap<String, NmonMetricDescriptor>,
    pub unsupported_sections: BTreeSet<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct SectionHeader {
    description: String,
    columns: Vec<String>,
}

pub(crate) fn parse_file(path: &Path) -> Result<ParsedNmonFile, String> {
    let file = File::open(path)
        .map_err(|error| format!("cannot open NMON file '{}': {error}", path.display()))?;
    parse_reader(path, BufReader::new(file))
}

fn parse_reader(path: &Path, reader: impl BufRead) -> Result<ParsedNmonFile, String> {
    let mut raw_metadata = BTreeMap::<String, String>::new();
    let mut headers = HashMap::<String, SectionHeader>::new();
    let mut timestamp_by_id = HashMap::<String, NaiveDateTime>::new();
    let mut last_timestamp_in_file = None;
    let mut samples_by_id = BTreeMap::<String, BTreeMap<String, f64>>::new();
    let mut descriptors = BTreeMap::<String, NmonMetricDescriptor>::new();
    let mut descriptor_by_column = HashMap::<(String, usize), String>::new();
    let mut unsupported_sections = BTreeSet::new();
    let mut warnings = Vec::new();
    let mut interval_seconds = None;
    let mut timezone = None;

    for (zero_based, line_result) in reader.lines().enumerate() {
        let line_number = zero_based + 1;
        let line = line_result.map_err(|error| {
            format!(
                "cannot read NMON file '{}' at line {line_number}: {error}",
                path.display()
            )
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
        let Some(section) = fields.first().filter(|value| !value.is_empty()) else {
            continue;
        };

        match *section {
            "AAA" => {
                if fields.len() >= 3 {
                    let key = fields[1].to_string();
                    let value = fields[2..].join(",");
                    raw_metadata.entry(key.clone()).or_insert(value.clone());
                    if key.eq_ignore_ascii_case("interval") {
                        interval_seconds = value.trim().parse::<u64>().ok();
                    }
                    if matches!(
                        key.to_ascii_lowercase().as_str(),
                        "timezone" | "time_zone" | "tz"
                    ) {
                        timezone = nonempty(value);
                    }
                }
            }
            "ZZZZ" => match parse_timestamp_record(&fields) {
                Ok((sample_id, timestamp)) => {
                    if last_timestamp_in_file.is_some_and(|previous| timestamp < previous) {
                        push_warning(
                            &mut warnings,
                            format!(
                                "{}:{line_number}: ZZZZ timestamp {timestamp} is earlier than the preceding timestamp",
                                path.display()
                            ),
                        );
                    }
                    last_timestamp_in_file = Some(timestamp);
                    if let Some(existing) = timestamp_by_id.insert(sample_id.clone(), timestamp) {
                        if existing != timestamp {
                            push_warning(
                                &mut warnings,
                                format!(
                                    "{}:{line_number}: sample {sample_id} maps to both {existing} and {timestamp}",
                                    path.display()
                                ),
                            );
                        }
                    }
                }
                Err(message) => push_warning(
                    &mut warnings,
                    format!("{}:{line_number}: {message}", path.display()),
                ),
            },
            _ if is_supported(section) => {
                if fields.get(1).is_some_and(|value| is_sample_id(value)) {
                    if fields.len() < 3 {
                        push_warning(
                            &mut warnings,
                            format!(
                                "{}:{line_number}: malformed {section} sample record",
                                path.display()
                            ),
                        );
                        continue;
                    }
                    let Some(header) = headers.get(*section) else {
                        push_warning(
                            &mut warnings,
                            format!(
                                "{}:{line_number}: {section} sample has no section header",
                                path.display()
                            ),
                        );
                        continue;
                    };
                    let values = &fields[2..];
                    if values.len() != header.columns.len() {
                        push_warning(
                            &mut warnings,
                            format!(
                                "{}:{line_number}: {section} has {} values but its header has {} columns",
                                path.display(),
                                values.len(),
                                header.columns.len()
                            ),
                        );
                    }
                    let sample = samples_by_id.entry(fields[1].to_string()).or_default();
                    for (index, raw_value) in values.iter().enumerate() {
                        let Some(column) = header.columns.get(index) else {
                            break;
                        };
                        if raw_value.is_empty() {
                            continue;
                        }
                        let Ok(value) = raw_value.parse::<f64>() else {
                            push_warning(
                                &mut warnings,
                                format!(
                                    "{}:{line_number}: non-numeric {section}/{column} value '{raw_value}'",
                                    path.display()
                                ),
                            );
                            continue;
                        };
                        if !value.is_finite() {
                            continue;
                        }
                        let cache_key = (section.to_string(), index);
                        let key = if let Some(key) = descriptor_by_column.get(&cache_key) {
                            key.clone()
                        } else {
                            let (key, descriptor) = metric_descriptor(section, header, column);
                            descriptors.insert(key.clone(), descriptor);
                            descriptor_by_column.insert(cache_key, key.clone());
                            key
                        };
                        sample.insert(key, value);
                    }
                } else if fields.len() >= 3 {
                    headers.insert(
                        section.to_string(),
                        SectionHeader {
                            description: fields[1].to_string(),
                            columns: fields[2..]
                                .iter()
                                .map(|value| value.trim().to_string())
                                .collect(),
                        },
                    );
                } else {
                    push_warning(
                        &mut warnings,
                        format!(
                            "{}:{line_number}: malformed {section} header",
                            path.display()
                        ),
                    );
                }
            }
            _ => {
                if !section.starts_with("BB") && !section.starts_with("TOP") {
                    unsupported_sections.insert(section.to_string());
                }
            }
        }
    }

    if timestamp_by_id.is_empty() {
        return Err(format!(
            "NMON file '{}' has no valid ZZZZ timestamp records",
            path.display()
        ));
    }

    let mut samples = BTreeMap::<NaiveDateTime, BTreeMap<String, f64>>::new();
    for (sample_id, sample) in samples_by_id {
        let Some(timestamp) = timestamp_by_id.get(&sample_id).copied() else {
            push_warning(
                &mut warnings,
                format!(
                    "{}: sample {sample_id} has data but no ZZZZ timestamp mapping",
                    path.display()
                ),
            );
            continue;
        };
        if let Some(existing) = samples.get_mut(&timestamp) {
            push_warning(
                &mut warnings,
                format!(
                    "{}: multiple sample identifiers map to timestamp {timestamp}",
                    path.display()
                ),
            );
            for (key, value) in sample {
                match existing.get(&key) {
                    Some(previous) if (*previous - value).abs() <= f64::EPSILON => {}
                    Some(previous) => {
                        return Err(format!(
                            "{}: conflicting values for {key} at {timestamp}: {previous} and {value}",
                            path.display()
                        ));
                    }
                    None => {
                        existing.insert(key, value);
                    }
                }
            }
        } else {
            samples.insert(timestamp, sample);
        }
    }

    if samples.is_empty() {
        return Err(format!(
            "NMON file '{}' has timestamps but no supported numeric samples",
            path.display()
        ));
    }

    let metadata = metadata_from_aaa(raw_metadata);
    Ok(ParsedNmonFile {
        path: path.to_path_buf(),
        metadata,
        interval_seconds,
        timezone,
        samples,
        descriptors,
        unsupported_sections,
        warnings,
    })
}

fn parse_timestamp_record(fields: &[&str]) -> Result<(String, NaiveDateTime), String> {
    if fields.len() < 4 || !is_sample_id(fields[1]) {
        return Err("malformed ZZZZ record".to_string());
    }
    let time = NaiveTime::parse_from_str(fields[2], "%H:%M:%S")
        .map_err(|_| format!("invalid ZZZZ time '{}'", fields[2]))?;
    let date = NaiveDate::parse_from_str(&fields[3].to_ascii_uppercase(), "%d-%b-%Y")
        .map_err(|_| format!("invalid ZZZZ date '{}'", fields[3]))?;
    Ok((fields[1].to_string(), date.and_time(time)))
}

fn is_sample_id(value: &str) -> bool {
    value
        .strip_prefix('T')
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
}

fn is_supported(section: &str) -> bool {
    section == "CPU_ALL"
        || section.strip_prefix("CPU").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit())
        })
        || matches!(
            section,
            "LPAR"
                | "MEM"
                | "MEMNEW"
                | "MEMUSE"
                | "VM"
                | "PAGE"
                | "PAGING"
                | "PROC"
                | "NET"
                | "NETPACKET"
        )
        || section.starts_with("DISK")
}

fn metric_descriptor(
    section: &str,
    header: &SectionHeader,
    column: &str,
) -> (String, NmonMetricDescriptor) {
    let (domain, entity, source_name) = if section.starts_with("DISK") {
        (
            "disk".to_string(),
            Some(column.to_string()),
            section.to_string(),
        )
    } else if section == "PAGING" {
        (
            "paging_space".to_string(),
            Some(column.to_string()),
            section.to_string(),
        )
    } else if matches!(section, "NET" | "NETPACKET") {
        let (entity, metric) = column
            .split_once('-')
            .map(|(entity, metric)| (Some(entity.to_string()), metric.to_string()))
            .unwrap_or((None, column.to_string()));
        ("network".to_string(), entity, metric)
    } else if section != "CPU_ALL" && section.starts_with("CPU") {
        (
            "cpu".to_string(),
            Some(section.to_string()),
            column.to_string(),
        )
    } else {
        (
            domain_for_section(section).to_string(),
            None,
            column.to_string(),
        )
    };
    let mut key_parts = vec![slug(&domain)];
    if let Some(entity) = entity.as_ref() {
        key_parts.push(slug(entity));
    }
    if section.starts_with("DISK") || section == "PAGING" {
        key_parts.push(slug(section));
    } else {
        key_parts.push(slug(&source_name));
    }
    let unit = infer_unit(section, &header.description, column);
    (
        key_parts.join("."),
        NmonMetricDescriptor {
            section: section.to_string(),
            source_name,
            source_label: if section.starts_with("DISK") || section == "PAGING" {
                header.description.clone()
            } else {
                column.to_string()
            },
            domain,
            entity,
            unit,
        },
    )
}

fn domain_for_section(section: &str) -> &'static str {
    match section {
        "CPU_ALL" => "cpu_all",
        "LPAR" => "lpar",
        "MEM" | "MEMNEW" | "MEMUSE" => "memory",
        "VM" | "PAGE" => "vm",
        "PROC" => "process",
        _ => "other",
    }
}

fn infer_unit(section: &str, description: &str, column: &str) -> Option<String> {
    let combined = format!("{description} {column}").to_ascii_lowercase();
    if combined.contains("msec/xfer") {
        Some("ms/xfer".to_string())
    } else if combined.contains("kb/s") {
        Some("KB/s".to_string())
    } else if combined.contains("(mb)") || description.contains(" MB ") {
        Some("MB".to_string())
    } else if combined.contains('%') || section == "DISKBUSY" {
        Some("%".to_string())
    } else if combined.contains("reads/s")
        || combined.contains("writes/s")
        || section == "DISKRIO"
        || section == "DISKWIO"
    {
        Some("operations/s".to_string())
    } else if combined.contains("transfers per second") || section == "DISKXFER" {
        Some("transfers/s".to_string())
    } else {
        None
    }
}

fn slug(value: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            separator = false;
        } else if character == '%' {
            if !slug.ends_with("pct") {
                if !slug.is_empty() && !slug.ends_with('_') {
                    slug.push('_');
                }
                slug.push_str("pct");
            }
            separator = false;
        } else if !separator && !slug.is_empty() {
            slug.push('_');
            separator = true;
        }
    }
    slug.trim_matches('_').to_string()
}

fn metadata_from_aaa(source_metadata: BTreeMap<String, String>) -> NmonMetadata {
    let host = lookup(&source_metadata, "host").or_else(|| lookup(&source_metadata, "NodeName"));
    let os_version = lookup(&source_metadata, "AIX");
    let nmon_version = lookup(&source_metadata, "version");
    let architecture = lookup(&source_metadata, "hardware");
    let subprocessor_mode = lookup(&source_metadata, "SubprocessorMode");
    let mut lpar = super::model::NmonLparMetadata {
        subprocessor_mode,
        ..Default::default()
    };
    if let Some(number_name) = lookup(&source_metadata, "LPARNumberName") {
        let mut parts = number_name.splitn(2, ',');
        lpar.partition_id = parts.next().and_then(|value| value.trim().parse().ok());
        lpar.partition_name = parts.next().and_then(|value| nonempty(value.to_string()));
    }
    NmonMetadata {
        host,
        os_name: os_version.as_ref().map(|_| "AIX".to_string()),
        os_version,
        nmon_version,
        architecture,
        lpar,
        memory: Default::default(),
        source_metadata,
    }
}

fn lookup(values: &BTreeMap<String, String>, key: &str) -> Option<String> {
    values
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .and_then(|(_, value)| nonempty(value.clone()))
}

fn nonempty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn push_warning(warnings: &mut Vec<String>, warning: String) {
    const MAX_WARNINGS: usize = 100;
    if warnings.len() < MAX_WARNINGS {
        warnings.push(warning);
    } else if warnings.len() == MAX_WARNINGS {
        warnings.push("additional parser warnings were suppressed".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(input: &str) -> ParsedNmonFile {
        parse_reader(Path::new("synthetic.nmon"), Cursor::new(input)).unwrap()
    }

    #[test]
    fn maps_zzzz_and_parses_core_sections_without_treating_missing_as_zero() {
        let parsed = parse(
            "AAA,host,aix1\nAAA,AIX,7.3.1.0\nAAA,interval,30\n\
             CPU_ALL,CPU Total aix1,User%,Sys%,Wait%,Idle%\n\
             LPAR,Logical Partition aix1,PhysicalCPU,virtualCPUs,logicalCPUs,entitled,SharedCPU,Capped\n\
             MEM,Memory aix1,Real free(MB),Real total(MB)\n\
             VM,Virtual Memory aix1,pgin,pgout\n\
             PROC,Processes aix1,Runnable,Swap-in\n\
             DISKREAD,Disk Read KB/s aix1,hdisk0,hdisk1\n\
             NET,Network I/O aix1,en0-read-KB/s,en0-write-KB/s\n\
             CPU_ALL,T0001,10,5,,85\n\
             LPAR,T0001,1.5,4,16,2.0,1,0\n\
             MEM,T0001,1024,8192\nVM,T0001,3,4\nPROC,T0001,7,0\n\
             DISKREAD,T0001,100,200\nNET,T0001,10,20\n\
             ZZZZ,T0001,01:02:03,10-SEP-2026\n",
        );
        let timestamp =
            NaiveDateTime::parse_from_str("2026-09-10 01:02:03", "%Y-%m-%d %H:%M:%S").unwrap();
        assert_eq!(parsed.samples.len(), 1);
        let sample = &parsed.samples[&timestamp];
        assert_eq!(sample["cpu_all.user_pct"], 10.0);
        assert!(!sample.contains_key("cpu_all.wait_pct"));
        assert_eq!(sample["disk.hdisk1.diskread"], 200.0);
        assert_eq!(sample["network.en0.read_kb_s"], 10.0);
        assert_eq!(parsed.metadata.host.as_deref(), Some("aix1"));
        assert_eq!(parsed.interval_seconds, Some(30));
    }

    #[test]
    fn ignores_unknown_sections_and_reports_malformed_supported_records_once() {
        let parsed = parse(
            "AAA,host,aix1\nCPU_ALL,CPU Total aix1,User%\nCPU_ALL,T0001,oops\n\
             UNKNOWN,T0001,9\nZZZZ,T0001,00:00:00,10-SEP-2026\n",
        );
        assert!(parsed.unsupported_sections.contains("UNKNOWN"));
        assert_eq!(parsed.samples.len(), 1);
        assert_eq!(parsed.warnings.len(), 1);
    }
}
