//! Private, workspace-backed snapshots for guided investigations.
use super::*;
use serde::{de::DeserializeOwned, Deserialize};
use sha2::{Digest, Sha256};
use std::{fs, io};
use tempfile::NamedTempFile;

type PilotResult<T> = std::result::Result<T, Value>;

#[cfg(not(test))]
pub(super) fn configured_workspace() -> PathBuf {
    let dir = std::env::var_os("JASMIN_AUDIT_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    fs::canonicalize(&dir).unwrap_or(dir)
}

fn storage_error(error: impl std::fmt::Display) -> Value {
    tool_error(
        "AUDIT_STORAGE_ERROR",
        format!("Unable to access the private audit workspace: {error}"),
    )
}

pub(super) fn private_directory(path: &Path) -> PilotResult<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => sync_directory(path.parent().unwrap_or(path)),
        Err(e)
            if e.kind() == io::ErrorKind::AlreadyExists
                && fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) =>
        {
            Ok(())
        }
        Err(e) => Err(storage_error(e)),
    }
}

#[cfg(unix)]
pub(super) fn sync_directory(path: &Path) -> PilotResult<()> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(storage_error)
}
#[cfg(not(unix))]
pub(super) fn sync_directory(_: &Path) -> PilotResult<()> {
    Ok(())
}

fn checked_audit_id(id: &str) -> PilotResult<()> {
    if id.starts_with("audit-")
        && id.len() <= 96
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        Ok(())
    } else {
        Err(tool_error(
            "INVALID_AUDIT_ID",
            "Use the audit_id returned by list_saved_audits",
        ))
    }
}

fn audit_directory(root: &Path, id: &str, create: bool) -> PilotResult<PathBuf> {
    checked_audit_id(id)?;
    let mut path = root.to_owned();
    for component in [".jasmin", "audits", id] {
        path.push(component);
        if create {
            private_directory(&path)?;
        } else if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            return Err(tool_error(
                "SAVED_AUDIT_UNAVAILABLE",
                "The selected private audit directory is unavailable",
            ));
        }
    }
    Ok(path)
}

fn valid_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn immutable_write(dir: &Path, name: &str, bytes: &[u8]) -> PilotResult<()> {
    let mut tmp = NamedTempFile::new_in(dir).map_err(storage_error)?;
    tmp.write_all(bytes).map_err(storage_error)?;
    tmp.as_file().sync_all().map_err(storage_error)?;
    tmp.persist_noclobber(dir.join(name))
        .map_err(|e| storage_error(e.error))?;
    // Commits to the object directory are batched and synchronized before the snapshot.
    Ok(())
}

fn put_object<T: Serialize>(dir: &Path, value: &T) -> PilotResult<String> {
    let bytes = serde_json::to_vec(value).map_err(storage_error)?;
    let digest = sha256(&bytes);
    let path = dir.join(&digest);
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => {
            // An object left by an interrupted writer can safely be reused after verification.
            let original = read_object_bytes(dir, &digest)?;
            if original != bytes {
                return Err(storage_error("content-address conflict"));
            }
        }
        Ok(_) => return Err(storage_error("unexpected object file type")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => immutable_write(dir, &digest, &bytes)?,
        Err(e) => return Err(storage_error(e)),
    }
    Ok(digest)
}

fn read_object_bytes(dir: &Path, digest: &str) -> PilotResult<Vec<u8>> {
    if !valid_digest(digest) {
        return Err(storage_error("invalid object digest"));
    }
    let path = dir.join(digest);
    if !fs::symlink_metadata(&path)
        .map_err(storage_error)?
        .is_file()
    {
        return Err(storage_error("expected a regular archive object"));
    }
    let bytes = fs::read(path).map_err(storage_error)?;
    if sha256(&bytes) != digest {
        return Err(tool_error(
            "AUDIT_SNAPSHOT_CORRUPT",
            "A saved object has a different SHA-256 digest",
        ));
    }
    Ok(bytes)
}

fn read_object<T: DeserializeOwned>(dir: &Path, digest: &str) -> PilotResult<T> {
    serde_json::from_slice(&read_object_bytes(dir, digest)?).map_err(storage_error)
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum PilotKind {
    #[default]
    Tx,
    General,
    Cpu,
    Combined,
}

impl PilotKind {
    fn is_tx(&self) -> bool {
        *self == Self::Tx
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AuditTopic {
    Tx,
    Cpu,
}

impl AuditTopic {
    fn name(self) -> &'static str {
        match self {
            Self::Tx => "tx",
            Self::Cpu => "cpu",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Tx => Self::Cpu,
            Self::Cpu => Self::Tx,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicFocus {
    current_chapter_id: String,
    focus_project_id: Option<String>,
    focus_brief_id: Option<String>,
    last_private_note: Option<String>,
    last_correction: Option<Value>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CombinedDiscussion {
    active_topic: AuditTopic,
    // The active focus lives in the same fields used by chapter edits and resumptions.
    other_focus: TopicFocus,
    brief_topics: BTreeMap<String, AuditTopic>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedAudit {
    // TX snapshots predate this field. Preserve their serialized bytes for checksum verification.
    #[serde(default, skip_serializing_if = "PilotKind::is_tx")]
    pub(super) pilot_kind: PilotKind,
    pub(super) audit_id: String,
    pub(super) revision: u64,
    project_ids: Vec<String>,
    current_chapter_id: String,
    focus_project_id: Option<String>,
    focus_brief_id: Option<String>,
    last_private_note: Option<String>,
    last_correction: Option<Value>,
    updated_at: String,
    checkpoint_digest: String,
    evidence_objects: BTreeMap<String, String>,
    // Idempotency receipts survive restarts. Arguments are hashed without the ephemeral session ID.
    operations: BTreeMap<String, String>,
    parent_snapshot: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    combined: Option<CombinedDiscussion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finalized_report: Option<StoredReport>,
}
#[derive(Clone, Serialize, Deserialize)]
struct StoredReport {
    revision: u64,
    markdown_digest: Option<String>,
}

impl SavedAudit {
    fn summary(&self) -> Value {
        let mut value = json!({"audit_id":self.audit_id,"pilot_kind":self.pilot_kind,"revision":self.revision,"project_ids":self.project_ids,
            "current_chapter_id":self.current_chapter_id,"focus_project_id":self.focus_project_id,
            "updated_at":self.updated_at,"parent_snapshot":self.parent_snapshot});
        if let Some(combined) = &self.combined {
            value["active_topic"] = json!(combined.active_topic);
        }
        value
    }
    pub(super) fn decorate_status(&self, value: &mut Value, workspace: &Path) {
        value["audit_id"] = json!(self.audit_id);
        value["pilot_kind"] = json!(self.pilot_kind);
        value["snapshot"] = json!({"revision":self.revision,"pilot_kind":self.pilot_kind,"persisted":true,"updated_at":self.updated_at});
        let directory = workspace.join(".jasmin/audits").join(&self.audit_id);
        value["snapshot"]["workspace_path"] = json!(workspace);
        value["snapshot"]["path"] = json!(directory.join(snapshot_name(self.revision)));
        value["resume_context"] = json!({"focus_project_id":self.focus_project_id,"focus_brief_id":self.focus_brief_id,
            "last_private_note":self.last_private_note,"last_correction":self.last_correction});
        if let Some(combined) = &self.combined {
            let mut active = value["resume_context"].clone();
            active["current_chapter_id"] = json!(self.current_chapter_id);
            value["active_topic"] = json!(combined.active_topic);
            value["snapshot"]["active_topic"] = json!(combined.active_topic);
            value["topics"][combined.active_topic.name()] = active;
            value["topics"][combined.active_topic.other().name()] = json!(combined.other_focus);
            value["brief_topics"] = json!(combined.brief_topics);
            if let Some(briefs) = value["briefs"].as_array_mut() {
                for brief in briefs {
                    brief["topic"] = json!(brief["brief_id"]
                        .as_str()
                        .and_then(|id| self.topic_for_brief(id)));
                }
            }
        }
    }

    pub(super) fn topic_for_brief(&self, id: &str) -> Option<&'static str> {
        self.combined
            .as_ref()?
            .brief_topics
            .get(id)
            .map(|topic| topic.name())
    }

    pub(super) fn includes_active_brief(&self, id: &str) -> bool {
        self.combined.as_ref().is_none_or(|discussion| {
            discussion.brief_topics.get(id) == Some(&discussion.active_topic)
        })
    }

    fn switch_to(
        &mut self,
        topic: AuditTopic,
        audit: &mut audit_workflow::AuditState,
    ) -> PilotResult<()> {
        let combined = self.combined.as_mut().ok_or_else(combined_required)?;
        if combined.active_topic != topic {
            let other = &mut combined.other_focus;
            std::mem::swap(&mut self.current_chapter_id, &mut other.current_chapter_id);
            std::mem::swap(&mut self.focus_project_id, &mut other.focus_project_id);
            std::mem::swap(&mut self.focus_brief_id, &mut other.focus_brief_id);
            std::mem::swap(&mut self.last_private_note, &mut other.last_private_note);
            std::mem::swap(&mut self.last_correction, &mut other.last_correction);
            combined.active_topic = topic;
            audit
                .current_chapter_id
                .clone_from(&self.current_chapter_id);
        }
        Ok(())
    }

    fn infer_chapter_focus(&mut self, audit: &audit_workflow::AuditState) {
        if self.focus_brief_id.is_none() {
            if let Some(brief) = audit
                .briefs
                .values()
                .filter(|b| {
                    b.chapter_id == audit.current_chapter_id
                        && self
                            .focus_project_id
                            .as_ref()
                            .is_none_or(|p| p == &b.project_id)
                })
                .max_by_key(|b| b.updated_at_revision)
            {
                self.focus_project_id = Some(brief.project_id.clone());
                self.focus_brief_id = Some(brief.brief_id.clone());
            }
        }
    }

    fn update_focus(
        &mut self,
        args: &Map<String, Value>,
        s: &AnalysisSession,
        recorded: Option<&str>,
    ) -> PilotResult<()> {
        let audit = s
            .audit
            .as_ref()
            .ok_or_else(|| tool_error("AUDIT_NOT_STARTED", "Start a guided audit first"))?;
        if self.pilot_kind == PilotKind::General
            && self.current_chapter_id != audit.current_chapter_id
        {
            self.focus_brief_id = None;
        }
        self.current_chapter_id
            .clone_from(&audit.current_chapter_id);
        if let Some(note) = args.get("private_note") {
            let text = note
                .as_str()
                .filter(|v| !v.trim().is_empty() && v.len() <= 16000)
                .ok_or_else(|| {
                    tool_error(
                        "INVALID_PRIVATE_NOTE",
                        "Private note must contain 1..16000 bytes",
                    )
                })?;
            self.last_private_note = Some(text.to_owned());
        }
        let project = args
            .get("focus_project_id")
            .map(|v| v.as_str().unwrap_or(""));
        if let Some(project) = project {
            if !s.project_ids.iter().any(|p| p == project) {
                return Err(tool_error(
                    "PROJECT_NOT_IN_ANALYSIS",
                    "Choose one of the audit's projects",
                ));
            }
            if self.focus_project_id.as_deref() != Some(project) {
                self.focus_brief_id = None;
            }
            self.focus_project_id = Some(project.to_owned());
        }
        if let Some(id) =
            recorded.or_else(|| args.get("focus_brief_id").map(|v| v.as_str().unwrap_or("")))
        {
            let brief = audit.briefs.get(id).ok_or_else(|| {
                tool_error(
                    "UNKNOWN_AUDIT_BRIEF",
                    "Select a saved brief from this audit",
                )
            })?;
            if project.is_some_and(|p| p != brief.project_id) {
                return Err(tool_error(
                    "AUDIT_BRIEF_SCOPE_CHANGED",
                    "The requested project and brief differ",
                ));
            }
            if let Some(combined) = self.combined.as_mut() {
                match combined.brief_topics.get(id) {
                    Some(topic) if *topic != combined.active_topic => {
                        return Err(tool_error(
                            "AUDIT_TOPIC_MISMATCH",
                            "Switch to the brief's topic before editing or focusing it",
                        ));
                    }
                    None if recorded.is_some() => {
                        combined
                            .brief_topics
                            .insert(id.to_owned(), combined.active_topic);
                    }
                    None => {
                        return Err(tool_error(
                            "UNKNOWN_AUDIT_BRIEF",
                            "The brief has no topic in this audit",
                        ));
                    }
                    _ => (),
                }
            }
            self.focus_brief_id = Some(id.to_owned());
            self.focus_project_id = Some(brief.project_id.clone());
            if recorded.is_some() && brief.revision > 1 {
                self.last_correction = Some(json!({"brief_id":id,"brief_revision":brief.revision,
                    "conclusion":brief.conclusion,"private_note":args.get("private_note")}));
            }
        }
        if self.pilot_kind == PilotKind::General {
            self.infer_chapter_focus(audit);
        }
        Ok(())
    }
}

fn snapshot_name(revision: u64) -> String {
    format!("{revision:020}.json")
}
fn latest_revision(dir: &Path) -> PilotResult<u64> {
    let mut latest = 0;
    for entry in fs::read_dir(dir).map_err(storage_error)? {
        let name = entry.map_err(storage_error)?.file_name();
        if let Some(number) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .filter(|n| n.len() == 20 && n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse::<u64>().ok())
        {
            latest = latest.max(number);
        }
    }
    Ok(latest)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskSnapshot {
    version: u32,
    digest: String,
    state: SavedAudit,
}
fn read_snapshot(dir: &Path, id: &str, revision: u64) -> PilotResult<SavedAudit> {
    let path = dir.join(snapshot_name(revision));
    if !fs::symlink_metadata(&path)
        .map_err(storage_error)?
        .is_file()
    {
        return Err(storage_error("invalid snapshot file"));
    }
    let record: DiskSnapshot =
        serde_json::from_reader(BufReader::new(File::open(path).map_err(storage_error)?))
            .map_err(|e| tool_error("AUDIT_SNAPSHOT_CORRUPT", e.to_string()))?;
    let saved = record.state;
    if record.version != 1
        || sha256(&serde_json::to_vec(&saved).map_err(storage_error)?) != record.digest
    {
        return Err(tool_error(
            "AUDIT_SNAPSHOT_CORRUPT",
            "Snapshot format or checksum differs",
        ));
    }
    if saved.audit_id != id || saved.revision != revision || !valid_digest(&saved.checkpoint_digest)
    {
        return Err(tool_error(
            "AUDIT_SNAPSHOT_CORRUPT",
            "Snapshot identity or revision differs",
        ));
    }
    Ok(saved)
}

pub(super) fn list_saved_audits(root: &Path) -> PilotResult<Value> {
    let root = fs::canonicalize(root).map_err(storage_error)?;
    let dir = root.join(".jasmin/audits");
    if !dir.try_exists().map_err(storage_error)? {
        return Ok(json!({"audits":[]}));
    }
    let mut audits = vec![];
    for entry in fs::read_dir(dir).map_err(storage_error)? {
        let entry = entry.map_err(storage_error)?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if checked_audit_id(&id).is_err() || !entry.file_type().map_err(storage_error)?.is_dir() {
            continue;
        }
        let path = audit_directory(&root, &id, false)?;
        let revision = latest_revision(&path)?;
        if revision > 0 {
            match read_snapshot(&path, &id, revision) {
                Ok(saved) => audits.push(saved.summary()),
                Err(e) => audits
                    .push(json!({"audit_id":id,"revision":revision,"unavailable":true,"error":e})),
            }
        }
    }
    audits.sort_by(|a, b| {
        b["updated_at"]
            .as_str()
            .cmp(&a["updated_at"].as_str())
            .then_with(|| a["audit_id"].as_str().cmp(&b["audit_id"].as_str()))
    });
    Ok(json!({"audits":audits}))
}

fn request_operation(
    tool: &str,
    args: &Map<String, Value>,
) -> PilotResult<Option<(String, String)>> {
    let Some(value) = args.get("operation_id") else {
        return Ok(None);
    };
    let key = value
        .as_str()
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 100
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_:.".contains(&b))
        })
        .ok_or_else(|| {
            tool_error(
                "INVALID_OPERATION_ID",
                "Use a unique 1..100 character operation token",
            )
        })?;
    let mut inputs = args.clone();
    inputs.remove("analysis_id");
    inputs.remove("expected_revision");
    Ok(Some((
        key.to_owned(),
        sha256(&serde_json::to_vec(&(tool, inputs)).map_err(storage_error)?),
    )))
}

fn combined_required() -> Value {
    tool_error(
        "COMBINED_PILOT_REQUIRED",
        "Enable or resume a saved combined audit first",
    )
}

fn revision_conflict(current: u64) -> Value {
    json!({"error_code":"AUDIT_REVISION_CONFLICT","current_revision":current,
        "message":"Reload the latest saved discussion before editing it"})
}

fn writer_lock(directory: &Path) -> PilotResult<File> {
    // A per-audit advisory lock also protects independent MCP processes sharing this workspace.
    let lock_path = directory.join("writer.lock");
    if fs::symlink_metadata(&lock_path).is_ok_and(|m| !m.is_file()) {
        return Err(storage_error("unexpected writer lock type"));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let writer = options.open(lock_path).map_err(storage_error)?;
    writer.lock().map_err(storage_error)?;
    Ok(writer)
}

fn reader_lock(directory: &Path) -> PilotResult<File> {
    let path = directory.join("writer.lock");
    if !fs::symlink_metadata(&path)
        .map_err(storage_error)?
        .is_file()
    {
        return Err(storage_error("unexpected audit lock file type"));
    }
    let file = File::open(path).map_err(storage_error)?;
    file.lock_shared().map_err(storage_error)?;
    Ok(file)
}

fn fresh_audit_id(analysis_id: &str) -> String {
    format!(
        "audit-{}",
        analysis_id
            .strip_prefix("A-")
            .unwrap_or(analysis_id)
            .replace('.', "")
    )
}

impl AnalysisRuntime {
    pub(super) fn private_workspace(&self) -> PilotResult<PathBuf> {
        fs::canonicalize(&self.audit_workspace).map_err(storage_error)
    }
    pub(super) fn write_snapshot(
        &self,
        root: &Path,
        s: &AnalysisSession,
        next: &mut SavedAudit,
    ) -> PilotResult<()> {
        let directory = audit_directory(root, &next.audit_id, true)?;
        let objects = directory.join("objects");
        private_directory(&objects)?;
        let _writer = writer_lock(&directory)?;
        let latest = latest_revision(&directory)?;
        if latest != next.revision {
            return Err(revision_conflict(latest));
        }
        next.finalized_report = if s.report_revision > 0 {
            Some(StoredReport {
                revision: s.report_revision,
                markdown_digest: s
                    .finalized_markdown
                    .as_ref()
                    .map(|markdown| put_object(&objects, markdown))
                    .transpose()?,
            })
        } else {
            None
        };
        let checkpoint = self.discussion_checkpoint(s)?;
        next.checkpoint_digest = put_object(&objects, &checkpoint)?;
        for (id, record) in &s.evidence {
            if let Some(digest) = next.evidence_objects.get(id) {
                match fs::symlink_metadata(objects.join(digest)) {
                    Ok(metadata) if metadata.is_file() => continue,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        // The immutable source is still in memory, so a lost object can be reconstructed.
                        if put_object(&objects, record)? != *digest {
                            return Err(storage_error(
                                "retained evidence differs from its committed identity",
                            ));
                        }
                        continue;
                    }
                    Ok(_) => return Err(storage_error("unexpected archived evidence file type")),
                    Err(error) => return Err(storage_error(error)),
                }
            }
            next.evidence_objects
                .insert(id.clone(), put_object(&objects, record)?);
        }
        sync_directory(&objects)?;
        next.revision += 1;
        next.updated_at = chrono::Utc::now().to_rfc3339();
        let digest = sha256(&serde_json::to_vec(&next).map_err(storage_error)?);
        let bytes = serde_json::to_vec(&DiskSnapshot {
            version: 1,
            digest,
            state: next.clone(),
        })
        .map_err(storage_error)?;
        immutable_write(&directory, &snapshot_name(next.revision), &bytes)?;
        sync_directory(&directory).map_err(|_| tool_error("AUDIT_SNAPSHOT_DURABILITY_UNKNOWN",
            "The snapshot was installed but its directory could not be synchronized. Reopen the saved audit and retry with the same operation ID."))
    }

    pub(super) fn enable_pilot(
        &self,
        args: &Map<String, Value>,
        kind: PilotKind,
    ) -> PilotResult<Value> {
        let id = Self::analysis_id(args)?;
        let session = self.session(id)?;
        let mut s = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
        if let Some(saved) = &s.tx_pilot {
            if kind == PilotKind::General || saved.pilot_kind == kind {
                return Ok(self.discussion_status(id, &s));
            }
            if saved.pilot_kind != PilotKind::General {
                return Err(tool_error(
                    "PILOT_KIND_MISMATCH",
                    "Start a separate analysis for this pilot",
                ));
            }
        }
        if matches!(kind, PilotKind::Cpu | PilotKind::Combined) {
            self.validate_rac_projects(&s.project_ids)?;
        }
        if kind == PilotKind::Combined
            && s.audit
                .as_ref()
                .is_some_and(|audit| !audit.briefs.is_empty())
        {
            return Err(tool_error(
                "AUDIT_ALREADY_HAS_BRIEFS",
                "Start a combined audit before recording topic-specific briefs",
            ));
        }
        let root = self.private_workspace()?;
        let previous_chapter = s.audit.as_ref().map(|a| a.current_chapter_id.clone());
        let default_project = s.project_ids.first().cloned();
        let mut saved = s.tx_pilot.clone().unwrap_or_else(|| SavedAudit {
            audit_id: fresh_audit_id(id),
            project_ids: s.project_ids.clone(),
            focus_project_id: if s.project_ids.len() == 1 {
                default_project.clone()
            } else {
                None
            },
            ..SavedAudit::default()
        });
        saved.pilot_kind = kind;
        let audit = s.audit.get_or_insert_with(Default::default);
        if kind == PilotKind::Combined {
            audit.current_chapter_id = "wait_events".into();
            let project = args
                .get("focus_project_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or(default_project);
            saved.focus_project_id.clone_from(&project);
            saved.combined = Some(CombinedDiscussion {
                active_topic: AuditTopic::Tx,
                other_focus: TopicFocus {
                    current_chapter_id: "sql".into(),
                    focus_project_id: project,
                    ..TopicFocus::default()
                },
                brief_topics: BTreeMap::new(),
            });
        }
        if let Err(error) = saved
            .update_focus(args, &s, None)
            .and_then(|()| self.write_snapshot(&root, &s, &mut saved))
        {
            match previous_chapter {
                Some(chapter) => s.audit.as_mut().unwrap().current_chapter_id = chapter,
                None => s.audit = None,
            }
            return Err(error);
        }
        if previous_chapter.is_none() {
            s.finalized_markdown = None;
        }
        s.tx_pilot = Some(saved);
        Ok(self.discussion_status(id, &s))
    }

    pub(super) fn save_general_report_state(&self, s: &mut AnalysisSession) -> PilotResult<()> {
        if let Some(mut saved) = s
            .tx_pilot
            .as_ref()
            .filter(|a| a.pilot_kind == PilotKind::General)
            .cloned()
        {
            self.write_snapshot(&self.private_workspace()?, s, &mut saved)?;
            s.tx_pilot = Some(saved);
        }
        Ok(())
    }

    pub(super) fn switch_audit_topic(&self, args: &Map<String, Value>) -> PilotResult<Value> {
        let topic = match args.get("topic").and_then(Value::as_str) {
            Some("tx") => AuditTopic::Tx,
            Some("cpu") => AuditTopic::Cpu,
            _ => return Err(tool_error("UNKNOWN_AUDIT_TOPIC", "Choose tx or cpu")),
        };
        self.edit_pilot_discussion("switch_audit_topic", args, |s, next| {
            let saved = next.ok_or_else(combined_required)?;
            let audit = s.audit.as_mut().ok_or_else(combined_required)?;
            saved.switch_to(topic, audit)?;
            Ok(None)
        })
    }

    pub(super) fn edit_pilot_discussion(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        edit: impl FnOnce(&mut AnalysisSession, Option<&mut SavedAudit>) -> PilotResult<Option<String>>,
    ) -> PilotResult<Value> {
        let id = Self::analysis_id(args)?;
        let session = self.session(id)?;
        let mut s = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
        if let Some(mut next) = s.tx_pilot.clone() {
            let op = request_operation(tool, args)?;
            if let Some((key, digest)) = &op {
                if let Some(old) = next.operations.get(key) {
                    if old != digest {
                        return Err(tool_error(
                            "AUDIT_OPERATION_CONFLICT",
                            "Operation ID was used with different arguments",
                        ));
                    }
                    return Ok(self.discussion_status(id, &s));
                }
            }
            if args
                .get("expected_revision")
                .is_some_and(|v| v.as_u64() != Some(next.revision))
            {
                return Err(revision_conflict(next.revision));
            }
            let root = self.private_workspace()?;
            let previous = s.audit.clone();
            let markdown = s.finalized_markdown.clone();
            let result = edit(&mut s, Some(&mut next)).and_then(|recorded| {
                next.update_focus(args, &s, recorded.as_deref())?;
                if let Some((key, digest)) = op {
                    next.operations.insert(key, digest);
                }
                self.write_snapshot(&root, &s, &mut next)
            });
            if let Err(e) = result {
                s.audit = previous;
                s.finalized_markdown = markdown;
                return Err(e);
            }
            s.tx_pilot = Some(next);
        } else {
            if tool == "switch_audit_topic" {
                return Err(combined_required());
            }
            if [
                "operation_id",
                "expected_revision",
                "private_note",
                "focus_brief_id",
                "focus_project_id",
            ]
            .iter()
            .any(|k| args.contains_key(*k))
            {
                return Err(tool_error(
                    "TX_PILOT_REQUIRED",
                    "Enable durable storage before saving a private note or guarded mutation",
                ));
            }
            edit(&mut s, None)?;
        }
        Ok(self.discussion_status(id, &s))
    }

    pub(super) fn resume_saved_audit(&self, args: &Map<String, Value>) -> PilotResult<Value> {
        let audit_id = required_string(args, "audit_id", 96)?;
        let root = self.private_workspace()?;
        let dir = audit_directory(&root, &audit_id, false)?;
        let latest = latest_revision(&dir)?;
        let revision = match args.get("revision") {
            Some(v) => v
                .as_u64()
                .filter(|n| *n > 0 && *n <= latest)
                .ok_or_else(|| {
                    tool_error(
                        "INVALID_SNAPSHOT_REVISION",
                        "Choose an existing positive snapshot revision",
                    )
                })?,
            None => latest,
        };
        if revision == 0 {
            return Err(tool_error(
                "SAVED_AUDIT_UNAVAILABLE",
                "No committed snapshot is available",
            ));
        }
        let mut saved = read_snapshot(&dir, &audit_id, revision)?;
        sync_directory(&dir)?;
        let as_general = args
            .get("as_general")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if as_general && (revision != latest || saved.pilot_kind == PilotKind::Combined) {
            return Err(tool_error(
                "INVALID_AUDIT_KIND",
                "Only the latest single-topic or general audit may continue as a general discussion.",
            ));
        }
        let objects = dir.join("objects");
        if !fs::symlink_metadata(&objects)
            .map_err(storage_error)?
            .is_dir()
        {
            return Err(storage_error("invalid archive object directory"));
        }
        let mut checkpoint: audit_workflow::AuditCheckpoint =
            read_object(&objects, &saved.checkpoint_digest)?;
        for (id, key) in &saved.evidence_objects {
            checkpoint
                .evidence
                .insert(id.clone(), read_object(&objects, key)?);
        }
        let keep_review =
            revision == latest && (as_general || saved.pilot_kind == PilotKind::General);
        let finalized = if keep_review {
            saved
                .finalized_report
                .as_ref()
                .map(|r| -> PilotResult<_> {
                    Ok((
                        r.revision,
                        r.markdown_digest
                            .as_ref()
                            .map(|h| read_object::<String>(&objects, h))
                            .transpose()?,
                    ))
                })
                .transpose()?
        } else {
            None
        };
        let restored = self.restore_checkpoint(checkpoint, keep_review)?;
        let id = restored["analysis_id"].as_str().expect("restored analysis");
        let session = self.session(id)?;
        let mut s = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
        if let Some((report_revision, markdown)) = finalized {
            s.report_revision = report_revision;
            s.finalized_markdown = markdown;
        }
        if revision != latest {
            saved.finalized_report = None;
            saved.parent_snapshot = Some(json!({"audit_id":saved.audit_id,"revision":revision}));
            saved.audit_id = fresh_audit_id(id);
            saved.revision = 0;
            saved.evidence_objects.clear();
            saved.operations.clear();
        }
        let migrating = as_general && saved.pilot_kind != PilotKind::General;
        if migrating {
            saved.pilot_kind = PilotKind::General;
            if let Some(audit) = &s.audit {
                saved.infer_chapter_focus(audit);
            }
        }
        let commit = if revision != latest || migrating {
            self.write_snapshot(&root, &s, &mut saved)
        } else {
            // A concurrent writer may have committed a newer chapter during evidence replay.
            latest_revision(&dir).and_then(|tip| {
                if tip == revision {
                    Ok(())
                } else {
                    Err(revision_conflict(tip))
                }
            })
        };
        if let Err(error) = commit {
            drop(s);
            self.sessions.remove(id);
            return Err(error);
        }
        s.tx_pilot = Some(saved);
        Ok(self.discussion_status(id, &s))
    }

    pub(super) fn pilot_publication(
        &self,
        tool: &str,
        args: &Map<String, Value>,
    ) -> PilotResult<Value> {
        let session = self.session(Self::analysis_id(args)?)?;
        let s = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
        let kind = match tool {
            "preview_client_report" | "read_client_report_asset" | "publish_client_report" => {
                PilotKind::General
            }
            "preview_tx_report" | "publish_tx_report" | "read_tx_report_asset" => PilotKind::Tx,
            "preview_cpu_report" | "publish_cpu_report" | "read_cpu_report_asset" => PilotKind::Cpu,
            "preview_combined_report"
            | "publish_combined_report"
            | "read_combined_report_asset" => PilotKind::Combined,
            _ => return Err(tool_error("UNKNOWN_TOOL", "Unknown publication tool")),
        };
        let saved = s.tx_pilot.as_ref().ok_or_else(|| match kind {
            PilotKind::Combined => combined_required(),
            PilotKind::General => tool_error(
                "CLIENT_AUDIT_REQUIRED",
                "Start or restore a general guided audit first",
            ),
            PilotKind::Tx => tool_error(
                "TX_PILOT_REQUIRED",
                "Enable or resume a saved TX pilot first",
            ),
            PilotKind::Cpu => tool_error(
                "CPU_PILOT_REQUIRED",
                "Enable or resume a saved CPU pilot first",
            ),
        })?;
        if saved.pilot_kind != kind {
            return Err(tool_error(
                "PILOT_KIND_MISMATCH",
                "The selected report belongs to another pilot",
            ));
        }
        let root = self.private_workspace()?;
        let dir = audit_directory(&root, &saved.audit_id, false)?;
        let _lock = if matches!(
            tool,
            "read_tx_report_asset"
                | "read_cpu_report_asset"
                | "read_combined_report_asset"
                | "read_client_report_asset"
        ) {
            reader_lock(&dir)?
        } else {
            writer_lock(&dir)?
        };
        let current = latest_revision(&dir)?;
        if current != saved.revision {
            return Err(revision_conflict(current));
        }
        tx_pilot_publication::run_with_runtime(tool, args, &s, &root, &saved.audit_id, Some(self))
    }
}

pub(super) fn add_mutation_fields(properties: &mut Value) {
    properties["operation_id"] =
        json!({"type":"string","description":"Unique retry key for this saved discussion change."});
    properties["private_note"] = json!({"type":"string","maxLength":16000,"description":"Private continuation context, including a correction or unresolved question."});
    properties["expected_revision"] = json!({"type":"integer","minimum":1,"description":"Last observed snapshot.revision for the durable pilot."});
}

pub(super) fn tools() -> Vec<Value> {
    vec![
        function_definition(
            "enable_combined_pilot",
            "Start a durable RAC discussion of TX waits and CPU from the selected instances. Each topic retains its own chapter, project, board and correction. Starts with TX.",
            json!({"type":"object","additionalProperties":false,"properties":{"focus_project_id":{"type":"string"}}}),
        ),
        function_definition(
            "switch_audit_topic",
            "Switch the durable discussion between TX waits and RAC CPU. Restore the selected topic's chapter, project, brief, last correction and note. The committed snapshot preserves both topics.",
            json!({"type":"object","additionalProperties":false,"properties":{"topic":{"type":"string","enum":["tx","cpu"]},"expected_revision":{"type":"integer","minimum":1},"operation_id":{"type":"string"}},"required":["topic"]}),
        ),
        function_definition(
            "enable_tx_pilot",
            "Enable durable TX discussion snapshots in the operator-configured audit workspace. Save on each brief and chapter mutation.",
            json!({"type":"object","additionalProperties":false,"properties":{"focus_project_id":{"type":"string"}}}),
        ),
        function_definition(
            "enable_cpu_pilot",
            "Start durable discussion of a cluster-wide CPU scenario for the selected RAC instances. Save each correction and chapter transition in the audit workspace.",
            json!({"type":"object","additionalProperties":false,"properties":{"focus_project_id":{"type":"string"}}}),
        ),
        function_definition(
            "list_saved_audits",
            "List saved guided and pilot audits available in the configured private workspace.",
            json!({"type":"object","additionalProperties":false,"properties":{}}),
        ),
        function_definition(
            "resume_saved_audit",
            "Restore the saved audit, its evidence and discussion under a fresh analysis ID. The latest general checkpoint retains current editorial state; older revisions create separate branches. Use as_general to continue an existing single-topic audit as a general discussion under the same audit ID.",
            json!({"type":"object","additionalProperties":false,"properties":{"audit_id":{"type":"string"},"revision":{"type":"integer","minimum":1},"as_general":{"type":"boolean","default":false}},"required":["audit_id"]}),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(dir: &Path) -> AnalysisRuntime {
        let mut runtime = super::super::tests::runtime();
        runtime.audit_workspace = dir.to_owned();
        runtime
    }
    fn call(r: &AnalysisRuntime, tool: &str, args: Value) -> PilotResult<Value> {
        r.call_tool(tool, args.as_object().unwrap().clone())
    }
    fn begin(r: &AnalysisRuntime) -> Value {
        let started = call(r, "start_performance_analysis", json!({})).unwrap();
        call(
            r,
            "enable_tx_pilot",
            json!({"analysis_id":started["analysis_id"]}),
        )
        .unwrap()
    }
    fn brief(status: &Value, conclusion: &str, op: &str) -> Value {
        json!({"analysis_id":status["analysis_id"],"project_id":status["project_ids"][0],
            "chapter_id":"wait_events","brief_id":"tx-wait","question":"Which session was sampled?",
            "explanation":"The event was recorded on the waiting side.","conclusion":conclusion,
            "evidence_boundary":"A holder chain is needed to identify the blocking session.",
            "next_step":"Capture the transaction chain.","open_questions":["Which transaction owns the row?"],
            "evidence_refs":[SEED_EVIDENCE_ID],"operation_id":op,"expected_revision":status["snapshot"]["revision"]})
    }

    #[test]
    fn durable_correction_retry_restore_competing_writer_and_branch() {
        let work = tempfile::tempdir().unwrap();
        let first = runtime(work.path());
        let start = begin(&first);
        let initial = call(
            &first,
            "record_audit_brief",
            brief(&start, "An observed contention episode.", "initial"),
        )
        .unwrap();
        let mut correction = brief(&initial, "The measured session is a waiter.", "correction");
        correction["private_note"] = json!("Identify the holder separately.");
        let changed = call(&first, "record_audit_brief", correction.clone()).unwrap();
        assert_eq!(changed["history_count"], 1);
        assert_eq!(changed["snapshot"]["revision"], 3);
        assert_eq!(
            call(&first, "record_audit_brief", correction.clone()).unwrap()["snapshot"],
            changed["snapshot"]
        );
        let mut different = correction;
        different["private_note"] = json!("Another note.");
        assert_eq!(
            call(&first, "record_audit_brief", different).unwrap_err()["error_code"],
            "AUDIT_OPERATION_CONFLICT"
        );
        let moved = call(
            &first,
            "advance_audit",
            json!({"analysis_id":start["analysis_id"],"chapter_id":"sql",
            "expected_revision":3,"private_note":"Compare alternative statements."}),
        )
        .unwrap();
        let another = runtime(work.path());
        let listed = call(&another, "list_saved_audits", json!({})).unwrap();
        assert_eq!(listed["audits"][0]["audit_id"], start["audit_id"]);
        let resumed = call(
            &another,
            "resume_saved_audit",
            json!({"audit_id":start["audit_id"]}),
        )
        .unwrap();
        assert_ne!(resumed["analysis_id"], start["analysis_id"]);
        assert_eq!(resumed["resume_context"], moved["resume_context"]);
        assert_eq!(
            resumed["resume_context"]["last_correction"]["private_note"],
            "Identify the holder separately."
        );
        assert_eq!(moved["revision"], changed["revision"]);
        assert_eq!(resumed["current_chapter_id"], "sql");
        assert_eq!(resumed["briefs"][0]["revision"], 2);
        assert_eq!(resumed["report_authorized"], false);
        let later=call(&another,"advance_audit",json!({"analysis_id":resumed["analysis_id"],"chapter_id":"wait_events","expected_revision":4})).unwrap();
        let stale = call(
            &first,
            "advance_audit",
            json!({"analysis_id":start["analysis_id"],"chapter_id":"io","expected_revision":4}),
        )
        .unwrap_err();
        assert_eq!(stale["current_revision"], 5);
        assert_eq!(
            call(
                &first,
                "get_audit_status",
                json!({"analysis_id":start["analysis_id"]})
            )
            .unwrap()["current_chapter_id"],
            "sql"
        );
        let fork = call(
            &another,
            "resume_saved_audit",
            json!({"audit_id":start["audit_id"],"revision":initial["snapshot"]["revision"]}),
        )
        .unwrap();
        assert_ne!(fork["audit_id"], start["audit_id"]);
        assert_eq!(fork["snapshot"]["revision"], 1);
        assert_eq!(fork["briefs"][0]["revision"], 1);
        let latest = call(
            &another,
            "resume_saved_audit",
            json!({"audit_id":start["audit_id"]}),
        )
        .unwrap();
        assert_eq!(latest["snapshot"], later["snapshot"]);
    }

    fn rac_runtime(dir: &Path) -> AnalysisRuntime {
        let projects = (1..=2)
            .map(|number| {
                let mut project = super::super::tests::comparison_project(
                    &format!("project-{number}"),
                    &[1.0],
                    &[2.0],
                );
                let db = &mut project.collection.db_instance_information;
                db.instance_num = number;
                db.db_id = 7;
                db.rac = "YES".into();
                project
            })
            .collect();
        let mut r = AnalysisRuntime::from_projects(projects).unwrap();
        r.audit_workspace = dir.to_owned();
        r
    }

    #[test]
    fn combined_topics_preserve_owned_briefs_and_focus_across_switch_retry_and_restart() {
        let work = tempfile::tempdir().unwrap();
        let first = rac_runtime(work.path());
        let selected = call(
            &first,
            "start_performance_analysis",
            json!({"project_ids":["project-1","project-2"]}),
        )
        .unwrap();
        let id = &selected["analysis_id"];
        let start = call(
            &first,
            "enable_combined_pilot",
            json!({"analysis_id":id,"focus_project_id":"project-2"}),
        )
        .unwrap();
        assert_eq!(start["active_topic"], "tx");
        assert_eq!(start["current_chapter_id"], "wait_events");
        assert_eq!(start["topics"]["cpu"]["focus_project_id"], "project-2");

        let tx = call(
            &first,
            "record_audit_brief",
            brief(&start, "Measured waits.", "tx-1"),
        )
        .unwrap();
        let mut correction = brief(&tx, "Measured on the waiting side.", "tx-2");
        correction["private_note"] = json!("Capture the transaction holder.");
        let revised = call(&first, "record_audit_brief", correction).unwrap();
        let switched = json!({"analysis_id":id,"topic":"cpu","expected_revision":revised["snapshot"]["revision"],"operation_id":"switch-cpu"});
        let cpu = call(&first, "switch_audit_topic", switched.clone()).unwrap();
        assert_eq!(cpu["snapshot"]["active_topic"], "cpu");
        assert_eq!(cpu["current_chapter_id"], "sql");
        assert!(cpu["resume_context"]["focus_brief_id"].is_null());
        assert!(cpu["resume_context"]["last_private_note"].is_null());
        assert_eq!(cpu["topics"]["tx"]["last_correction"]["brief_revision"], 2);
        assert_eq!(
            call(&first, "switch_audit_topic", switched).unwrap()["snapshot"],
            cpu["snapshot"]
        );
        assert_eq!(
            call(
                &first,
                "record_audit_brief",
                brief(&cpu, "Wrong topic.", "invalid")
            )
            .unwrap_err()["error_code"],
            "AUDIT_TOPIC_MISMATCH"
        );
        assert_eq!(
            call(
                &first,
                "advance_audit",
                json!({"analysis_id":id,"chapter_id":"sql","focus_brief_id":"tx-wait"})
            )
            .unwrap_err()["error_code"],
            "AUDIT_TOPIC_MISMATCH"
        );

        let mut second = brief(&cpu, "A conditional workload reduction.", "cpu-1");
        second["brief_id"] = json!("cluster-scenario");
        second["chapter_id"] = json!("sql");
        second["project_id"] = json!("project-2");
        second["private_note"] = json!("Verify response times.");
        let cpu = call(&first, "record_audit_brief", second).unwrap();
        assert_eq!(
            cpu["brief_topics"],
            json!({"tx-wait":"tx","cluster-scenario":"cpu"})
        );
        assert_eq!(cpu["briefs"][0]["topic"], "cpu");
        assert_eq!(cpu["history_count"], 1);
        let tx = call(
            &first,
            "switch_audit_topic",
            json!({"analysis_id":id,"topic":"tx"}),
        )
        .unwrap();
        assert_eq!(tx["resume_context"], revised["resume_context"]);
        assert_eq!(tx["revision"], cpu["revision"]);
        assert_eq!(
            tx["topics"]["cpu"]["last_private_note"],
            "Verify response times."
        );
        drop(first);

        let fresh = rac_runtime(work.path());
        let listed = call(&fresh, "list_saved_audits", json!({})).unwrap();
        assert_eq!(listed["audits"][0]["active_topic"], "tx");
        let resumed = call(
            &fresh,
            "resume_saved_audit",
            json!({"audit_id":tx["audit_id"]}),
        )
        .unwrap();
        assert_ne!(resumed["analysis_id"], *id);
        assert_eq!(resumed["snapshot"], tx["snapshot"]);
        assert_eq!(resumed["topics"], tx["topics"]);
        assert_eq!(resumed["briefs"], tx["briefs"]);
        let cpu = call(
            &fresh,
            "switch_audit_topic",
            json!({"analysis_id":resumed["analysis_id"],"topic":"cpu"}),
        )
        .unwrap();
        assert_eq!(cpu["current_chapter_id"], "sql");
        assert_eq!(cpu["resume_context"]["focus_project_id"], "project-2");
        assert_eq!(cpu["resume_context"]["focus_brief_id"], "cluster-scenario");
        assert_eq!(
            cpu["resume_context"]["last_private_note"],
            "Verify response times."
        );
    }

    #[test]
    fn legacy_snapshots_keep_their_checksum_and_report_kind_after_restore() {
        let work = tempfile::tempdir().unwrap();
        let r = runtime(work.path());
        let started = begin(&r);
        let args = json!({"analysis_id":started["analysis_id"]});
        let dir =
            audit_directory(work.path(), started["audit_id"].as_str().unwrap(), false).unwrap();
        let snapshot: Value =
            serde_json::from_reader(File::open(dir.join(snapshot_name(1))).unwrap()).unwrap();
        assert!(snapshot["state"].get("pilot_kind").is_none());
        assert_eq!(started["snapshot"]["pilot_kind"], "tx");
        assert_eq!(
            call(&r, "enable_cpu_pilot", args.clone()).unwrap_err()["error_code"],
            "PILOT_KIND_MISMATCH"
        );
        assert_eq!(
            call(&r, "preview_cpu_report", args).unwrap_err()["error_code"],
            "PILOT_KIND_MISMATCH"
        );
        let resumed = call(
            &runtime(work.path()),
            "resume_saved_audit",
            json!({"audit_id": started["audit_id"]}),
        )
        .unwrap();
        assert_eq!(resumed["pilot_kind"], "tx");
    }

    #[test]
    fn interrupted_write_rolls_back_and_corrupt_tip_can_branch_from_prior_snapshot() {
        let work = tempfile::tempdir().unwrap();
        let r = runtime(work.path());
        let start = begin(&r);
        let dir = audit_directory(work.path(), start["audit_id"].as_str().unwrap(), false).unwrap();
        let objects = dir.join("objects");
        let preserved = dir.join("saved-objects");
        fs::rename(&objects, &preserved).unwrap();
        fs::write(&objects, b"unavailable").unwrap();
        let args = brief(
            &start,
            "An observed contention episode.",
            "retry-after-failure",
        );
        assert!(call(&r, "record_audit_brief", args.clone()).is_err());
        let intact = call(
            &r,
            "get_audit_status",
            json!({"analysis_id":start["analysis_id"]}),
        )
        .unwrap();
        assert_eq!(intact["snapshot"], start["snapshot"]);
        assert_eq!(intact["briefs"].as_array().unwrap().len(), 0);
        fs::remove_file(&objects).unwrap();
        fs::rename(&preserved, &objects).unwrap();
        call(&r, "record_audit_brief", args).unwrap();
        let committed = dir.join(snapshot_name(2));
        let mut document: Value = serde_json::from_reader(File::open(&committed).unwrap()).unwrap();
        document["state"]["last_private_note"] = json!("Modified after commit.");
        fs::write(committed, serde_json::to_vec(&document).unwrap()).unwrap();
        assert_eq!(
            call(
                &r,
                "resume_saved_audit",
                json!({"audit_id":start["audit_id"]})
            )
            .unwrap_err()["error_code"],
            "AUDIT_SNAPSHOT_CORRUPT"
        );
        assert!(call(
            &r,
            "resume_saved_audit",
            json!({"audit_id":start["audit_id"],"revision":1})
        )
        .is_ok());
    }
    #[test]
    fn read_asset_requires_current_workspace_and_never_becomes_measurement_evidence() {
        let work = tempfile::tempdir().unwrap();
        let r = runtime(work.path());
        let tool = build_mcp_tools(&r)
            .into_iter()
            .find(|tool| tool.name == "read_tx_report_asset")
            .unwrap();
        let annotations = tool.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert!(tool.input_schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("analysis_id")));
        let initial = begin(&r);
        let read = json!({"analysis_id":initial["analysis_id"],"preview_id":"0".repeat(64),"asset":"index.html"});
        let before = call(
            &r,
            "export_audit",
            json!({"analysis_id":initial["analysis_id"]}),
        )
        .unwrap()["checkpoint"]
            .clone();
        assert!(call(&r, "read_tx_report_asset", read.clone()).is_err());
        assert!(!work.path().join(".jasmin/previews").exists());
        let after = call(
            &r,
            "export_audit",
            json!({"analysis_id":initial["analysis_id"]}),
        )
        .unwrap()["checkpoint"]
            .clone();
        assert_eq!(before, after);
        let mut forged = after;
        forged["evidence"]["E-0002"] = json!({"evidence_id":"E-0002","tool_name":"read_tx_report_asset",
            "arguments":{},"project_id":null,"result":{}});
        assert_eq!(
            call(&r, "restore_audit", json!({"checkpoint":forged})).unwrap_err()["error_code"],
            "INVALID_CHECKPOINT"
        );
        let other = runtime(work.path());
        let resumed = call(
            &other,
            "resume_saved_audit",
            json!({"audit_id":initial["audit_id"]}),
        )
        .unwrap();
        call(
            &other,
            "advance_audit",
            json!({"analysis_id":resumed["analysis_id"],"chapter_id":"wait_events"}),
        )
        .unwrap();
        assert_eq!(
            call(&r, "read_tx_report_asset", read).unwrap_err()["error_code"],
            "AUDIT_REVISION_CONFLICT"
        );
    }

    #[test]
    #[ignore = "allocates a complete discussion larger than the MCP transport limit"]
    fn disk_snapshot_exceeds_transport_limit_and_preserves_every_correction() {
        let work = tempfile::tempdir().unwrap();
        let r = runtime(work.path());
        let started = call(&r, "start_performance_analysis", json!({})).unwrap();
        let id = started["analysis_id"].as_str().unwrap();
        let scope = call(&r, "start_audit", json!({"analysis_id":id})).unwrap();
        let mut template = brief(&scope, "Initial observation", "seed");
        for key in ["expected_revision", "operation_id"] {
            template.as_object_mut().unwrap().remove(key);
        }
        for key in [
            "question",
            "conclusion",
            "explanation",
            "next_step",
            "evidence_boundary",
        ] {
            template[key] = json!("x".repeat(16000));
        }
        call(&r, "record_audit_brief", template).unwrap();
        {
            // Build a long legal revision history directly so the test measures disk restoration, not hundreds of previews.
            let session = r.session(id).unwrap();
            let mut state = session.lock().unwrap();
            let audit = state.audit.as_mut().unwrap();
            let active = audit.briefs.get_mut("tx-wait").unwrap();
            for _ in 0..850 {
                audit.history.push(active.clone());
                active.revision += 1;
                active.updated_at_revision += 1;
                audit.revision += 1;
            }
        }
        assert_eq!(
            call(&r, "export_audit", json!({"analysis_id":id})).unwrap_err()["error_code"],
            "CHECKPOINT_TOO_LARGE"
        );
        let enabled = call(&r, "enable_tx_pilot", json!({"analysis_id":id})).unwrap();
        assert!(serde_json::to_vec(&enabled).unwrap().len() < 1024 * 1024);
        let key = enabled["audit_id"].clone();
        drop(r);
        let fresh = runtime(work.path());
        let restored = call(&fresh, "resume_saved_audit", json!({"audit_id":key})).unwrap();
        assert_eq!(restored["history_count"], 850);
        assert_eq!(restored["briefs"][0]["revision"], 851);
    }
}
