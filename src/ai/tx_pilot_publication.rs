//! Client publication for scoped audits. Disk recipes and public files have distinct schemas.
use super::tx_pilot::{private_directory, sync_directory};
use super::{
    audit_workflow::AuditBrief, function_definition, tool_error, AnalysisRuntime, AnalysisSession,
};
use crate::report::i18n::ReportLanguage;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::NaiveDateTime;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use tempfile::{Builder, NamedTempFile};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const ASSET_CHUNK_BYTES: usize = 192 * 1024;
const EVENT: &str = "enq: TX - row lock contention";
const ASSETS: [&str; 3] = ["index.html", "evidence.html", "report.css"];
type ToolResult<T> = Result<T, Value>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    evidence_ref: String,
    instance_label: String,
    first_snap_id: u64,
    last_snap_id: u64,
    source_brief_id: Option<String>,
    source_brief_revision: Option<u64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    analysis_id: String,
    brief_id: String,
    brief_revision: u64,
    title: String,
    owner: String,
    success_criterion: String,
    evidence: Vec<Selection>,
}

// Only these values reach the renderer. Private recipes and raw tool responses do not.
#[derive(Serialize)]
struct PublicReport {
    title: String,
    conclusion: String,
    boundary: String,
    action: String,
    owner: String,
    success_criterion: String,
}

#[derive(Serialize)]
struct Measurement {
    instance_label: String,
    begin: String,
    end: String,
    total_wait_s: f64,
    observed_intervals: usize,
    intervals: Vec<Interval>,
}

#[derive(Serialize)]
struct Interval {
    begin: String,
    end: String,
    wait_s: f64,
}

fn tx_preview_params() -> Value {
    let short = json!({"type":"string","minLength":1});
    let rev = json!({"type":"integer","minimum":1});
    let evidence = json!({"type":"array","minItems":1,"maxItems":32,
        "items":{"type":"object","additionalProperties":false,
        "properties":{"evidence_ref":short,"instance_label":short,
            "first_snap_id":{"type":"integer","minimum":0},"last_snap_id":{"type":"integer","minimum":0},
            "source_brief_id":short,"source_brief_revision":rev},
        "required":["evidence_ref","instance_label","first_snap_id","last_snap_id"]}});
    json!({
        "type":"object","additionalProperties":false,"properties":{
            "brief_id":short,"brief_revision":rev,"title":short,"owner":short,
            "success_criterion":short,"evidence":evidence},
        "required":["brief_id","brief_revision","title","owner","success_criterion","evidence"]
    })
}

pub(super) fn tools() -> Vec<Value> {
    let short = json!({"type":"string","minLength":1});
    let mut definitions = vec![
        function_definition(
            "preview_tx_report",
            "Preview the scoped TX client report. Select the current brief's public conclusion, evidence boundary and next step, and explicitly supply the reader title, owner and success criterion. For each selected foreground TX wait timeline, supply its public instance label and inclusive begin SNAP_ID range. For another project's timeline also supply its own source_brief_id and source_brief_revision. Only complete observed intervals are supported.",
            tx_preview_params(),
        ),
        function_definition(
            "read_tx_report_asset",
            "Read a verified public TX preview asset in chunks, or download the ZIP after this preview was published. Use only the current analysis and preview ID. Offset is a byte offset from the preceding response. The SHA-256 and byte count describe the complete asset.",
            json!({
                "type":"object","additionalProperties":false,"properties":{
                    "preview_id":short,"asset":{"type":"string","enum":["index.html","evidence.html","report.css","report.zip"]},
                    "offset":{"type":"integer","minimum":0}
                },"required":["preview_id","asset"]
            }),
        ),
        function_definition(
            "publish_tx_report",
            "Publish the reviewed TX preview after the user requests the client ZIP. Supply their request and the preview ID from this active analysis. The selected brief revisions, evidence and exact public preview files are revalidated.",
            json!({
                "type":"object","additionalProperties":false,"properties":{
                    "preview_id":short,"user_request":{"type":"string","minLength":1,"maxLength":4000}},"required":["preview_id","user_request"]
            }),
        ),
    ];
    definitions.extend(cpu::tools());
    definitions.extend(combined::tools());
    definitions.extend(client_report_v2::tools());
    definitions
}

fn invalid(message: impl Into<String>) -> Value {
    tool_error("TX_PUBLICATION_INVALID", message)
}

fn io_error(error: impl std::fmt::Display) -> Value {
    tool_error(
        "TX_PUBLICATION_IO",
        format!("Could not read or persist this report edition: {error}"),
    )
}

pub(super) fn public_text(input: &str, limit: usize) -> ToolResult<String> {
    let plain = input.trim();
    if plain.is_empty()
        || plain.len() > limit
        || plain
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r'))
    {
        return Err(invalid(
            "Check the length and content of the selected client copy.",
        ));
    }
    static INTERNAL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let reserved = INTERNAL.get_or_init(|| regex::Regex::new(
        r"(?i)\b(?:MCP|analysis_id|source_brief_revision|companion_\w+|[AE]-\d{4,}(?:T\d+)?|B-\d{4,})\b|(?:file://|/Users/|/home/|/private/|/tmp/|[A-Z]:[\\/]|\.jasmin[/\\])"
    ).expect("static private field pattern"));
    if reserved.is_match(plain) {
        return Err(invalid(
            "Selected client copy contains an internal identifier or filesystem path. Revise the source brief or public label.",
        ));
    }
    Ok(plain.to_owned())
}

fn brief<'a>(session: &'a AnalysisSession, id: &str, revision: u64) -> ToolResult<&'a AuditBrief> {
    let current = session
        .audit
        .as_ref()
        .and_then(|a| a.briefs.get(id))
        .ok_or_else(|| invalid("The source brief is unavailable in this analysis."))?;
    if current.revision != revision || revision == 0 {
        return Err(tool_error(
            "TX_PREVIEW_STALE",
            "The source brief was revised. Select its current revision and prepare a fresh preview.",
        ));
    }
    Ok(current)
}

fn snapshot_time(row: &Value, name: &str) -> ToolResult<NaiveDateTime> {
    let value = row[name].as_str().unwrap_or("").trim();
    [
        "%d-%b-%y %H:%M:%S",
        "%d-%b-%Y %H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
    ]
    .iter()
    .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
    .ok_or_else(|| invalid("The source interval lacks a supported timestamp."))
}

fn measurement(
    session: &AnalysisSession,
    source: &AuditBrief,
    selection: &Selection,
) -> ToolResult<Measurement> {
    let record = session.evidence.get(&selection.evidence_ref)
        .filter(|r| source.evidence_refs.contains(&r.evidence_id)
            && r.project_id.as_deref() == Some(&source.project_id)
            && r.tool_name == "get_wait_event_timeline"
            && r.result["event_name"].as_str().is_some_and(|e| e.eq_ignore_ascii_case(EVENT))
            && r.result["kind"] == "foreground")
        .ok_or_else(|| invalid("Select a registered foreground TX wait timeline cited by this project's source brief."))?;
    let label = public_text(&selection.instance_label, 120)?;
    let expected = selection
        .last_snap_id
        .checked_sub(selection.first_snap_id)
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| invalid("Select a valid inclusive range of begin SNAP_IDs."))?;
    let rows = record.result["timeline"]
        .as_array()
        .ok_or_else(|| invalid("The evidence contains no wait timeline."))?;
    let mut series = BTreeMap::new();
    for row in rows {
        let id = row["snap_id"]
            .as_u64()
            .ok_or_else(|| invalid("The timeline contains an invalid SNAP_ID."))?;
        if !(selection.first_snap_id..=selection.last_snap_id).contains(&id) {
            continue;
        }
        let begin = snapshot_time(row, "begin_snap_time")?;
        let end = snapshot_time(row, "end_snap_time")?;
        let wait_s = row["total_wait_time_s"]
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .ok_or_else(|| invalid("The selected wait time is missing or invalid."))?;
        if end <= begin
            || !row["event"]
                .as_str()
                .is_some_and(|e| e.eq_ignore_ascii_case(EVENT))
        {
            return Err(invalid(
                "The selected interval or event name differs from the TX observation.",
            ));
        }
        if series.insert(id, (begin, end, wait_s)).is_some() {
            return Err(invalid(
                "Multiple observations share a SNAP_ID in this source. Use an instance-specific timeline.",
            ));
        }
    }
    if series.len() as u64 != expected {
        return Err(invalid(
            "The selected window has unobserved TX intervals. Choose a fully observed window; missing waits cannot be summed as zero.",
        ));
    }
    let mut intervals = Vec::with_capacity(series.len());
    let mut total_wait_s = 0.;
    let mut last_end = None;
    for (begin, end, wait_s) in series.into_values() {
        if last_end.is_some_and(|previous| begin != previous) {
            return Err(invalid(
                "The selected snapshot time intervals are not contiguous. Choose one continuously observed window.",
            ));
        }
        last_end = Some(end);
        total_wait_s += wait_s;
        intervals.push(Interval {
            begin: begin.format("%Y-%m-%d %H:%M:%S%.f").to_string(),
            end: end.format("%Y-%m-%d %H:%M:%S%.f").to_string(),
            wait_s,
        });
    }
    if !total_wait_s.is_finite() {
        return Err(invalid(
            "The sum of wait seconds is outside the numeric range.",
        ));
    }
    Ok(Measurement {
        instance_label: label,
        begin: intervals[0].begin.clone(),
        end: intervals.last().unwrap().end.clone(),
        total_wait_s,
        observed_intervals: intervals.len(),
        intervals,
    })
}

fn public_copy(
    main: &AuditBrief,
    title: &str,
    owner: &str,
    success_criterion: &str,
) -> ToolResult<PublicReport> {
    Ok(PublicReport {
        title: public_text(title, 400)?,
        conclusion: public_text(&main.conclusion, 16000)?,
        boundary: public_text(&main.evidence_boundary, 16000)?,
        action: public_text(&main.next_step, 16000)?,
        owner: public_text(owner, 160)?,
        success_criterion: public_text(success_criterion, 4000)?,
    })
}

fn project(
    session: &AnalysisSession,
    recipe: &Recipe,
) -> ToolResult<(PublicReport, Vec<Measurement>)> {
    let main = brief(session, &recipe.brief_id, recipe.brief_revision)?;
    let selected = public_copy(
        main,
        &recipe.title,
        &recipe.owner,
        &recipe.success_criterion,
    )?;
    if recipe.evidence.is_empty() || recipe.evidence.len() > 32 {
        return Err(invalid("Select 1 to 32 instance windows."));
    }
    let mut labels = HashSet::new();
    let mut scopes = HashSet::new();
    let mut includes_main = false;
    let mut measurements = Vec::new();
    for selection in &recipe.evidence {
        let source = match (&selection.source_brief_id, selection.source_brief_revision) {
            (None, None) => main,
            (Some(id), Some(revision)) => brief(session, id, revision)?,
            _ => return Err(invalid("Supply both the source brief and its revision.")),
        };
        includes_main |= source.brief_id == main.brief_id;
        let measure = measurement(session, source, selection)?;
        if !labels.insert(measure.instance_label.to_lowercase())
            || !scopes.insert((
                &source.project_id,
                selection.first_snap_id,
                selection.last_snap_id,
            ))
        {
            return Err(invalid(
                "Each observation needs a distinct window and public label.",
            ));
        }
        measurements.push(measure);
    }
    if !includes_main {
        return Err(invalid(
            "Select a foreground TX measurement from the primary brief.",
        ));
    }
    Ok((selected, measurements))
}

fn number(value: f64, lang: ReportLanguage) -> String {
    let raw = value.to_string();
    let (whole, frac) = raw.split_once('.').unwrap_or((&raw, ""));
    let mut result = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            result.push_str(lang.choose(",", "\u{202f}"));
        }
        result.push(c);
    }
    if !frac.is_empty() {
        result.push_str(lang.choose(".", ","));
        result.push_str(frac);
    }
    result
}

fn esc(s: &str) -> String {
    html_escape::encode_safe(s).into_owned()
}

#[derive(Clone, Copy)]
enum Pages<'a> {
    Standalone,
    Tx(&'a [native_charts::NativeChart]),
    Cpu(&'a [native_charts::NativeChart]),
}

impl<'a> Pages<'a> {
    fn section(self, evidence: bool) -> &'static str {
        match (self, evidence) {
            (Self::Standalone, false) => "index.html",
            (Self::Standalone, true) => "evidence.html",
            (Self::Tx(_), false) => "tx",
            (Self::Tx(_), true) => "tx-evidence",
            (Self::Cpu(_), false) => "cpu",
            (Self::Cpu(_), true) => "cpu-evidence",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Standalone => "",
            Self::Tx(_) => "tx-",
            Self::Cpu(_) => "cpu-",
        }
    }

    fn href(self, evidence: bool, target: Option<&str>) -> String {
        let section = self.section(evidence);
        match (self, target) {
            (Self::Standalone, None) => section.to_owned(),
            (Self::Standalone, Some(id)) => format!("{section}#{id}"),
            (_, None) => format!("#{section}"),
            (_, Some(id)) => format!("#{}{id}", self.prefix()),
        }
    }

    fn headings(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Standalone => ("h1", "h2", "h3"),
            _ => ("h2", "h3", "h4"),
        }
    }

    fn charts(self) -> &'a [native_charts::NativeChart] {
        match self {
            Self::Standalone => &[],
            Self::Tx(charts) | Self::Cpu(charts) => charts,
        }
    }

    fn page(self, lang: ReportLanguage, title: &str, body: &str, evidence: bool) -> String {
        if matches!(self, Self::Standalone) {
            return page(lang, title, body);
        }
        let section = self.section(evidence);
        let topic = self.section(false);
        format!(
            r#"<section class="report-chapter" id="{section}" data-menu-section="{topic}" aria-label="{}">{body}</section>"#,
            esc(title)
        )
    }
}

fn page(lang: ReportLanguage, title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html><html lang="{}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>{}</title><link rel="stylesheet" href="report.css"></head><body><main>{body}</main></body></html>"#,
        lang.choose("en", "pl"),
        esc(title)
    )
}

#[path = "tx_pilot_report_view.rs"]
mod view;

#[path = "native_pilot_charts.rs"]
mod native_charts;

#[path = "combined_pilot_publication.rs"]
mod combined;

#[path = "cpu_pilot_publication.rs"]
mod cpu;

#[path = "pilot_asset_manifest.rs"]
mod asset_manifest;
use asset_manifest::Manifest;

type Files = BTreeMap<String, Vec<u8>>;

fn files(
    report: &PublicReport,
    measures: &[Measurement],
    lang: ReportLanguage,
    pages: Pages,
) -> Files {
    let (decision, evidence) = (pages.section(false), pages.section(true));
    BTreeMap::from([
        (
            decision.into(),
            view::overview(report, measures, lang, pages).into_bytes(),
        ),
        (
            evidence.into(),
            view::evidence(report, measures, lang, pages).into_bytes(),
        ),
        (
            ASSETS[2].into(),
            include_bytes!("tx_pilot_report.css").to_vec(),
        ),
    ])
}

#[path = "client_report_v2.rs"]
pub(super) mod client_report_v2;

trait PublicationRecipe: DeserializeOwned + Serialize {
    const ENTRY_FILE: &'static str = "index.html";
    const ZIP_FILENAME: &'static str;
    const FROZEN_ASSETS: bool = false;
    fn validate_sources(&self, _session: &AnalysisSession) -> ToolResult<()> {
        Ok(())
    }
    fn analysis_id(&self) -> &str;
    fn render(
        &self,
        session: &AnalysisSession,
        lang: ReportLanguage,
        runtime: Option<&AnalysisRuntime>,
    ) -> ToolResult<(Files, Value)>;
}

impl PublicationRecipe for Recipe {
    const ZIP_FILENAME: &'static str = "raport-tx.zip";

    fn analysis_id(&self) -> &str {
        &self.analysis_id
    }

    fn render(
        &self,
        session: &AnalysisSession,
        lang: ReportLanguage,
        _runtime: Option<&AnalysisRuntime>,
    ) -> ToolResult<(Files, Value)> {
        let (report, measures) = project(session, self)?;
        Ok((
            files(&report, &measures, lang, Pages::Standalone),
            self.details(&report, &measures),
        ))
    }
}

impl Recipe {
    fn details(&self, report: &PublicReport, measures: &[Measurement]) -> Value {
        let summaries: Vec<_> = measures
            .iter()
            .map(|m| {
                json!({
                    "instance_label":m.instance_label,"total_wait_s":m.total_wait_s,
                    "observed_intervals":m.observed_intervals,"begin":m.begin,"end":m.end
                })
            })
            .collect();
        json!({"brief_revision": self.brief_revision, "public_report": report, "measurements": summaries})
    }
}

fn hash_files(files: &Files) -> String {
    let mut hash = Sha256::new();
    for (name, contents) in files {
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        hash.update((contents.len() as u64).to_be_bytes());
        hash.update(contents);
    }
    format!("{:x}", hash.finalize())
}

fn digest(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

fn directories(root: &Path, segments: &[&str]) -> ToolResult<PathBuf> {
    let mut path = root.to_owned();
    for segment in segments {
        path.push(segment);
        private_directory(&path)?;
    }
    Ok(path)
}

fn existing_directories(root: &Path, segments: &[&str]) -> ToolResult<PathBuf> {
    let mut path = root.to_owned();
    for segment in segments {
        path.push(segment);
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            return Err(tool_error(
                "TX_ASSET_UNAVAILABLE",
                "The selected report directory is unavailable.",
            ));
        }
    }
    Ok(path)
}

fn regular_bytes(path: &Path) -> io::Result<Vec<u8>> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Expected a regular publication file",
        ));
    }
    fs::read(path)
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create_new(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

const CHART_DIRECTORY: &str = "wykresy";

/// Public paths are either a file at the root or a file directly under the chart directory.
/// Flat paths are still used by standalone reports and earlier frozen editions.
fn validate_public_path(name: &str) -> ToolResult<()> {
    let filename = match name.split_once('/') {
        Some((CHART_DIRECTORY, file)) => file,
        Some(_) => return Err(invalid("Unknown report directory.")),
        None => name,
    };
    if filename == "report.zip"
        || filename.contains("..")
        || !filename.starts_with(|c: char| c.is_ascii_alphanumeric())
        || !filename
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-".contains(&b))
    {
        return Err(invalid("Invalid public report path."));
    }
    public_mime(filename)?;
    Ok(())
}

fn checked_files(folder: &Path, expected: &Files) -> ToolResult<()> {
    let changed = || {
        tool_error(
            "TX_PREVIEW_CHANGED",
            "The preview files differ from their reviewed content. Create a fresh preview in a clean workspace.",
        )
    };
    if !fs::symlink_metadata(folder).is_ok_and(|m| m.is_dir()) {
        return Err(changed());
    }
    let has_charts = expected.keys().any(|name| name.starts_with("wykresy/"));
    let mut verified = 0;
    let mut directories = vec![(folder.to_owned(), "")];
    while let Some((dir, prefix)) = directories.pop() {
        for entry in fs::read_dir(&dir).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name().into_string().map_err(|_| changed())?;
            let kind = entry.file_type().map_err(io_error)?;
            if kind.is_dir() && prefix.is_empty() && name == CHART_DIRECTORY && has_charts {
                directories.push((entry.path(), "wykresy/"));
                continue;
            }
            let path = format!("{prefix}{name}");
            let bytes = expected.get(&path).ok_or_else(changed)?;
            if !kind.is_file() || &regular_bytes(&entry.path()).map_err(io_error)? != bytes {
                return Err(changed());
            }
            verified += 1;
        }
    }
    if verified != expected.len() {
        return Err(changed());
    }
    Ok(())
}

fn prepare(folder: &Path, recipe: &[u8], files: &Files, manifest: Option<&[u8]>) -> ToolResult<()> {
    let parent = folder.parent().expect("preview directory");

    if !folder.exists() {
        let staging = Builder::new()
            .prefix(".preview-")
            .tempdir_in(parent)
            .map_err(io_error)?;
        let public = staging.path().join("public");
        private_directory(&public)?;
        let chart_dir = public.join(CHART_DIRECTORY);
        let has_charts = files.keys().any(|name| name.starts_with("wykresy/"));
        if has_charts {
            private_directory(&chart_dir)?;
        }
        for (name, bytes) in files {
            write_synced(&public.join(name), bytes).map_err(io_error)?;
        }
        if has_charts {
            sync_directory(&chart_dir)?;
        }
        sync_directory(&public)?;
        write_synced(&staging.path().join("recipe.json"), recipe).map_err(io_error)?;
        if let Some(bytes) = manifest {
            write_synced(&staging.path().join("assets.json"), bytes).map_err(io_error)?;
        }
        sync_directory(staging.path())?;
        if let Err(err) = fs::rename(staging.path(), folder) {
            if !folder.is_dir() {
                return Err(io_error(err));
            }
        }
        sync_directory(parent)?;
    }
    if !fs::symlink_metadata(folder).is_ok_and(|m| m.is_dir())
        || regular_bytes(&folder.join("recipe.json")).map_err(io_error)? != recipe
    {
        return Err(tool_error(
            "TX_PREVIEW_CHANGED",
            "The stored preview recipe has changed.",
        ));
    }
    if let Some(bytes) = manifest {
        if regular_bytes(&folder.join("assets.json")).map_err(io_error)? != bytes {
            return Err(tool_error(
                "TX_PREVIEW_CHANGED",
                "The saved asset manifest differs from the reviewed preview.",
            ));
        }
    }
    checked_files(&folder.join("public"), files)
}

fn package(files: &Files) -> ToolResult<Vec<u8>> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for (name, contents) in files {
        zip.start_file(name.as_str(), options).map_err(io_error)?;
        zip.write_all(contents).map_err(io_error)?;
    }
    Ok(zip.finish().map_err(io_error)?.into_inner())
}

fn immutable_file(dir: &Path, filename: &str, bytes: &[u8]) -> ToolResult<PathBuf> {
    let path = dir.join(filename);
    if !path.exists() {
        let mut temp = NamedTempFile::new_in(dir).map_err(io_error)?;
        temp.write_all(bytes)
            .and_then(|()| temp.as_file().sync_all())
            .map_err(io_error)?;
        if let Err(e) = temp.persist_noclobber(&path) {
            if e.error.kind() != io::ErrorKind::AlreadyExists {
                return Err(io_error(e.error));
            }
        }
        sync_directory(dir)?;
    }
    if regular_bytes(&path).map_err(io_error)? != bytes {
        return Err(tool_error(
            "TX_EDITION_CHANGED",
            "An existing publication file differs from this edition. The earlier file was preserved.",
        ));
    }
    Ok(path)
}

fn has_publication_receipt(
    folder: &Path,
    preview_id: &str,
    content_id: &str,
    zip_sha: &str,
) -> bool {
    let Ok(receipts) = existing_directories(folder, &["receipts"]) else {
        return false;
    };
    let Ok(mut entries) = fs::read_dir(receipts) else {
        return false;
    };
    entries.any(|entry| {
        let Ok(entry) = entry else {
            return false;
        };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json")
            || !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= 16 * 1024)
        {
            return false;
        }
        let Ok(bytes) = regular_bytes(&path) else {
            return false;
        };
        if path.file_stem().and_then(|s| s.to_str()) != Some(digest(&bytes).as_str()) {
            return false;
        }
        let Ok(receipt) = serde_json::from_slice::<Value>(&bytes) else {
            return false;
        };
        receipt["preview_id"] == preview_id
            && receipt["content_sha256"] == content_id
            && receipt["zip_sha256"] == zip_sha
            && receipt["user_request"]
                .as_str()
                .is_some_and(|r| !r.trim().is_empty())
    })
}

fn byte_range(size: usize, offset: Option<&Value>) -> ToolResult<(usize, usize)> {
    let start = match offset {
        None => 0,
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| invalid("Asset offset must be a nonnegative byte offset."))?,
    };
    if start >= size {
        return Err(invalid("Asset offset is outside the requested file."));
    }
    Ok((start, start.saturating_add(ASSET_CHUNK_BYTES).min(size)))
}

fn public_mime(name: &str) -> ToolResult<&'static str> {
    match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => Ok("text/html; charset=utf-8"),
        Some("css") => Ok("text/css; charset=utf-8"),
        Some("js") => Ok("text/javascript; charset=utf-8"),
        Some("png") => Ok("image/png"),
        Some("zip") if name == "report.zip" => Ok("application/zip"),
        _ => Err(invalid("Unknown public artifact type.")),
    }
}

fn asset_chunk(name: &str, mime: &str, bytes: &[u8], offset: Option<&Value>) -> ToolResult<Value> {
    let (start, end) = byte_range(bytes.len(), offset)?;
    Ok(
        json!({"filename":name,"content_type":mime,"offset":start,"bytes":bytes.len(),"sha256":digest(bytes),
        "content_base64":STANDARD.encode(&bytes[start..end]),"next_offset":(end<bytes.len()).then_some(end)}),
    )
}

fn read_asset<R: PublicationRecipe>(
    args: &Map<String, Value>,
    files: &Files,
    workspace: &Path,
    folder: &Path,
    preview_id: &str,
    content_id: &str,
) -> ToolResult<Value> {
    let asset = args.get("asset").and_then(Value::as_str).unwrap_or("");
    let offset = args.get("offset");
    if let Some(bytes) = files.get(asset) {
        let mime = public_mime(asset)?;
        return asset_chunk(asset, mime, bytes, offset);
    }
    if asset != "report.zip" {
        return Err(invalid("Asset is outside the public report."));
    }
    let archive = package(files)?;
    if !has_publication_receipt(folder, preview_id, content_id, &digest(&archive)) {
        return Err(tool_error(
            "TX_ZIP_NOT_PUBLISHED",
            "Publish this exact preview before downloading its ZIP.",
        ));
    }
    let edition = existing_directories(workspace, &["wydania", content_id])?;
    let disk = regular_bytes(&edition.join(R::ZIP_FILENAME)).map_err(io_error)?;
    if disk != archive {
        return Err(tool_error(
            "TX_EDITION_CHANGED",
            "The issued ZIP differs from the reviewed report.",
        ));
    }
    asset_chunk(asset, "application/zip", &disk, offset)
}

#[cfg(test)]
pub(super) fn run(
    name: &str,
    args: &Map<String, Value>,
    session: &AnalysisSession,
    workspace: &Path,
    audit_id: &str,
) -> ToolResult<Value> {
    run_with_runtime(name, args, session, workspace, audit_id, None)
}

pub(super) fn run_with_runtime(
    name: &str,
    args: &Map<String, Value>,
    session: &AnalysisSession,
    workspace: &Path,
    audit_id: &str,
    runtime: Option<&AnalysisRuntime>,
) -> ToolResult<Value> {
    match name {
        "preview_client_report" | "read_client_report_asset" | "publish_client_report" => {
            client_report_v2::edition(name, args, session, workspace, audit_id, runtime)
                .map_err(|error| edition_error(error, "CLIENT"))
        }
        "preview_tx_report" | "publish_tx_report" | "read_tx_report_asset" => {
            run_edition::<Recipe>(name, args, session, workspace, audit_id, runtime)
        }
        "preview_cpu_report" | "publish_cpu_report" | "read_cpu_report_asset" => {
            run_edition::<cpu::Recipe>(name, args, session, workspace, audit_id, runtime)
                .map_err(|error| edition_error(error, "CPU"))
        }
        "preview_combined_report" | "publish_combined_report" | "read_combined_report_asset" => {
            run_edition::<combined::Recipe>(name, args, session, workspace, audit_id, runtime)
                .map_err(|error| edition_error(error, "COMBINED"))
        }
        _ => Err(invalid("Unknown publication operation.")),
    }
}

fn edition_error(mut err: Value, kind: &str) -> Value {
    if let Some(suffix) = err["error_code"]
        .as_str()
        .and_then(|code| code.strip_prefix("TX_"))
    {
        err["error_code"] = json!(format!("{kind}_{suffix}"));
    }
    err
}

fn run_edition<R: PublicationRecipe>(
    name: &str,
    args: &Map<String, Value>,
    session: &AnalysisSession,
    workspace: &Path,
    audit_id: &str,
    runtime: Option<&AnalysisRuntime>,
) -> ToolResult<Value> {
    if audit_id.is_empty()
        || !audit_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(invalid("Invalid audit directory identity."));
    }
    let previewing = name.starts_with("preview_");
    let segments = [".jasmin", "previews", audit_id];
    let root = if previewing {
        directories(workspace, &segments)?
    } else {
        existing_directories(workspace, &segments)?
    };
    let publishing = name.starts_with("publish_");
    if publishing {
        let request = args
            .get("user_request")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if request.is_empty() || request.len() > 4000 {
            return Err(invalid(
                "Supply the user's explicit request to issue the ZIP.",
            ));
        }
    }
    let recipe: R = if !previewing {
        let id = args.get("preview_id").and_then(Value::as_str).unwrap_or("");
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("Use the SHA-256 preview_id from this analysis."));
        }
        let preview_dir = root.join(id);
        if !fs::symlink_metadata(&preview_dir).is_ok_and(|m| m.is_dir()) {
            return Err(tool_error(
                "TX_PREVIEW_MISSING",
                "Preview this edition in the current audit first.",
            ));
        }
        let stored = regular_bytes(&preview_dir.join("recipe.json")).map_err(|_| {
            tool_error(
                "TX_PREVIEW_MISSING",
                "Preview this edition in the current audit first.",
            )
        })?;
        let recipe: R = serde_json::from_slice(&stored)
            .map_err(|_| invalid("Invalid saved preview recipe."))?;
        if args.get("analysis_id").and_then(Value::as_str) != Some(recipe.analysis_id()) {
            return Err(tool_error(
                "TX_PREVIEW_STALE",
                "Prepare a new preview after resuming the analysis.",
            ));
        }
        if !publishing && R::FROZEN_ASSETS {
            recipe.validate_sources(session)?;
            return Manifest::read::<R>(&preview_dir, workspace, id, &stored, args);
        }
        recipe
    } else {
        serde_json::from_value(Value::Object(args.clone()))
            .map_err(|e| invalid(format!("Invalid preview selection: {e}")))?
    };
    if recipe.analysis_id().is_empty() {
        return Err(invalid("An active analysis is required."));
    }
    let (files, mut result) = recipe.render(
        session,
        ReportLanguage::new(&session.config.language),
        runtime,
    )?;
    for name in files.keys() {
        validate_public_path(name)?;
    }
    let content_id = hash_files(&files);
    let recipe_bytes = serde_json::to_vec(&recipe).map_err(io_error)?;
    let archive = R::FROZEN_ASSETS.then(|| package(&files)).transpose()?;
    let manifest = archive
        .as_deref()
        .map(|zip| Manifest::make(&files, &content_id, zip))
        .transpose()?;
    let id = if let Some(bytes) = &manifest {
        Manifest::preview_id(&recipe_bytes, bytes)
    } else {
        let mut hash = Sha256::new();
        hash.update(content_id.as_bytes());
        hash.update(&recipe_bytes);
        format!("{:x}", hash.finalize())
    };
    let folder = root.join(&id);
    let public = folder.join("public");
    if !previewing {
        if args["preview_id"] != id {
            return Err(tool_error(
                "TX_PREVIEW_STALE",
                "The current report differs from the reviewed preview.",
            ));
        }
        checked_files(&public, &files)?;
        if !publishing {
            return read_asset::<R>(args, &files, workspace, &folder, &id, &content_id);
        }
    } else {
        prepare(&folder, &recipe_bytes, &files, manifest.as_deref())?;
    }
    result["preview_id"] = json!(id);
    result["index_path"] = json!(public.join(R::ENTRY_FILE));
    result["files"] = if R::FROZEN_ASSETS {
        json!(std::iter::once(R::ENTRY_FILE)
            .chain(
                files
                    .keys()
                    .map(String::as_str)
                    .filter(|&f| f != R::ENTRY_FILE)
            )
            .collect::<Vec<_>>())
    } else {
        json!(ASSETS)
    };
    result["content_sha256"] = json!(content_id);
    if publishing {
        let zip = match archive {
            Some(zip) => zip,
            None => package(&files)?,
        };
        let zip_hash = digest(&zip);
        let edition = directories(workspace, &["wydania", &content_id])?;
        result["zip_path"] = json!(immutable_file(&edition, R::ZIP_FILENAME, &zip)?);
        result["zip_sha256"] = json!(zip_hash);
        result["bytes"] = json!(zip.len());
        let private_receipt = serde_json::to_vec(&json!({
            "preview_id":id,"content_sha256":content_id,"zip_sha256":zip_hash,"bytes":zip.len(),
            "user_request":args["user_request"],"analysis_id":recipe.analysis_id(),"audit_id":audit_id,
        })).map_err(io_error)?;
        let receipts = directories(&folder, &["receipts"])?;
        result["receipt_path"] = json!(immutable_file(
            &receipts,
            &format!("{}.json", digest(&private_receipt)),
            &private_receipt
        )?);
    }
    Ok(result)
}

#[cfg(test)]
#[path = "tx_pilot_publication_tests.rs"]
mod tests;
