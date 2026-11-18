//! Guided discussion state is independent from both the screen cache and report coverage.
use super::*;
use serde::Deserialize;
use std::hash::{Hash, Hasher};

#[path = "audit_hypotheses.rs"]
mod hypotheses;
pub(super) use hypotheses::hypothesis_status;
use hypotheses::{HypothesisCheck, HypothesisOutcome, OpeningHypothesis};

const CHAPTERS: &[(&str, &str, &str, &str, &str)] = &[
    ("scope", "Audit scope", "Zakres audytu", "Which workload, period and user symptom are we investigating?", "Jaką operację, okres i objaw zgłoszony przez użytkownika badamy?"),
    ("performance_profile", "Gradients, anomalies and workload", "Gradienty, anomalie i obciążenie", "Which DB Time and DB CPU gradient signals and dated anomalies or clusters match the measured workload? Which two or three hypotheses should we test?", "Które sygnały gradientowe DB Time i DB CPU oraz okna anomalii pokrywają się ze zmierzonym obciążeniem? Jakie dwie lub trzy hipotezy sprawdzimy?"),
    ("wait_events", "Wait events", "Oczekiwania", "Which waits matter, and what workload is associated with them?", "Które oczekiwania są istotne i z jaką pracą się wiążą?"),
    ("sql", "SQL and execution plans", "SQL i plany wykonania", "Which SQL paths explain the measured workload, and what remains unproven?", "Które ścieżki SQL wyjaśniają zmierzoną pracę i czego jeszcze nie potwierdziliśmy?"),
    ("segments", "Segments and objects", "Segmenty i obiekty", "Which objects recur across signals, and can we attribute their cost?", "Które obiekty powtarzają się w pomiarach i czy potrafimy przypisać ich koszt?"),
    ("latches", "Internal contention", "Kontencja wewnętrzna", "What do we know about concurrency, holders and waiters?", "Co wiemy o współbieżności oraz sesjach blokujących i oczekujących?"),
    ("io", "I/O", "Operacje I/O", "Do measurements distinguish storage latency from workload volume?", "Czy pomiary rozróżniają opóźnienie dysków i ilość wykonywanej pracy?"),
    ("undo_redo", "UNDO, redo and commits", "UNDO, redo i zatwierdzanie", "What explains redo and commit costs for this workload?", "Co wyjaśnia koszt redo i zatwierdzania dla badanych operacji?"),
    ("gradients_anomalies", "Hypothesis review", "Weryfikacja hipotez", "Which opening hypotheses were confirmed, rejected or remain unresolved, and which measurements explain each outcome?", "Które hipotezy robocze potwierdziły się, które odpadły, a które pozostają otwarte? Jakie pomiary o tym rozstrzygają?"),
    ("parameters", "Configuration", "Konfiguracja", "Which collected settings need action or an explained exception?", "Które zebrane ustawienia wymagają działania lub uzasadnienia wyjątku?"),
    ("synthesis", "Decisions and next steps", "Wnioski i kolejne kroki", "What can we conclude, what is still open, and what should happen first?", "Co możemy stwierdzić, co pozostaje otwarte i czym należy zająć się najpierw?"),
];

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuditBrief {
    #[serde(default)]
    pub brief_id: String,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub updated_at_revision: u64,
    pub project_id: String,
    pub chapter_id: String,
    pub question: String,
    pub explanation: String,
    pub conclusion: String,
    pub evidence_boundary: String,
    pub next_step: String,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub visual: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opening_hypotheses: Vec<OpeningHypothesis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hypothesis_gap: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hypothesis_checks: Vec<HypothesisCheck>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hypothesis_outcomes: Vec<HypothesisOutcome>,
    // Server-stamped revisions of the opening and checks used when this brief was saved.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hypothesis_basis: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuditState {
    pub revision: u64,
    pub current_chapter_id: String,
    pub briefs: BTreeMap<String, AuditBrief>,
    pub history: Vec<AuditBrief>,
    pub authorization: Option<ReportAuthorization>,
    pub review: Option<EditorialReview>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReportAuthorization {
    pub revision: u64,
    pub user_request: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EditorialReview {
    preview_id: String,
    checks: BTreeMap<String, bool>,
    notes: String,
}

impl Default for AuditState {
    fn default() -> Self {
        Self {
            revision: 1,
            current_chapter_id: "scope".into(),
            briefs: BTreeMap::new(),
            history: vec![],
            authorization: None,
            review: None,
        }
    }
}
impl AuditState {
    fn changed(&mut self) {
        self.revision += 1;
        self.authorization = None;
        self.review = None;
    }
    fn discussed(&self, id: &str, projects: &[String]) -> bool {
        projects
            .iter()
            .all(|project| self.project_discussed(id, project))
    }
    fn project_discussed(&self, chapter: &str, project: &str) -> bool {
        self.briefs
            .values()
            .any(|b| b.chapter_id == chapter && b.project_id == project)
            && hypotheses::chapter_complete(self, chapter, project)
    }
    pub(super) fn authorized(&self) -> bool {
        self.authorization
            .as_ref()
            .is_some_and(|a| a.revision == self.revision)
    }
}
fn chapter_exists(id: &str) -> bool {
    CHAPTERS.iter().any(|c| c.0 == id)
}

fn validate_brief(brief: &AuditBrief, state: &AnalysisSession) -> std::result::Result<(), Value> {
    if !chapter_exists(&brief.chapter_id) {
        return Err(tool_error(
            "UNKNOWN_AUDIT_CHAPTER",
            "Use a chapter_id from get_audit_status",
        ));
    }
    if !state.project_ids.contains(&brief.project_id) {
        return Err(tool_error(
            "PROJECT_NOT_IN_ANALYSIS",
            "The brief must belong to a selected project",
        ));
    }
    for text in [
        &brief.question,
        &brief.explanation,
        &brief.conclusion,
        &brief.evidence_boundary,
        &brief.next_step,
    ] {
        if text.trim().is_empty() || text.len() > 16000 {
            return Err(tool_error(
                "INVALID_AUDIT_BRIEF",
                "Brief fields must contain 1..16000 bytes; preserve the full explanation",
            ));
        }
    }
    if brief.open_questions.len() > 32
        || brief
            .open_questions
            .iter()
            .any(|q| q.trim().is_empty() || q.len() > 2000)
        || brief.evidence_refs.len() > 128
        || brief
            .visual
            .as_ref()
            .is_some_and(|v| !v.is_object() || serialized_json_size(v) > 128 * 1024)
    {
        return Err(tool_error(
            "INVALID_AUDIT_BRIEF",
            "Invalid open questions, evidence references or visual",
        ));
    }
    validate_references(state, &brief.evidence_refs, &[])?;
    if let Some(visual) = &brief.visual {
        crate::report::audit_visuals::validate_visual(visual)
            .map_err(|message| tool_error("INVALID_AUDIT_VISUAL", message))?;
        for panel in visual["panels"].as_array().expect("validated panels") {
            if panel["classification"] == "measured" {
                let source = panel["source"].as_str().expect("validated source");
                let tokens = source
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                    .collect::<HashSet<_>>();
                if !brief
                    .evidence_refs
                    .iter()
                    .any(|reference| tokens.contains(reference.as_str()))
                {
                    return Err(tool_error("AUDIT_VISUAL_EVIDENCE_REQUIRED", "Each measured panel must name a retained evidence reference in its source caption"));
                }
            }
        }
    }

    for reference in &brief.evidence_refs {
        let record = &state.evidence[reference];
        if record
            .project_id
            .as_ref()
            .is_some_and(|p| p != &brief.project_id)
        {
            return Err(tool_error("AUDIT_EVIDENCE_PROJECT_MISMATCH", "Evidence and brief must have the same project; retain comparative scope explicitly"));
        }
    }
    // Empty references represent discussion or a documented coverage gap, never measured proof.
    Ok(())
}

pub(super) fn status_value(analysis_id: &str, state: &AnalysisSession) -> Value {
    let Some(audit) = &state.audit else {
        return json!({"analysis_id":analysis_id,"audit_started":false,"project_ids":state.project_ids});
    };
    let chapters: Vec<Value> = CHAPTERS.iter().map(|(id,title,pl,q,qpl)| json!({
        "chapter_id":id,"title":title,"title_pl":pl,"question":q,"question_pl":qpl,
        "project_coverage":state.project_ids.iter().map(|project|json!({"project_id":project,"discussed":audit.project_discussed(id,project)})).collect::<Vec<_>>(),
        "status": if audit.discussed(id, &state.project_ids) {"discussed"} else if audit.current_chapter_id == *id {"current"} else {"pending"}
    })).collect();
    json!({"analysis_id":analysis_id,"audit_started":true,"project_ids":state.project_ids,
        "revision":audit.revision,"current_chapter_id":audit.current_chapter_id,"chapters":chapters,
        "hypotheses":state.project_ids.iter().map(|p| hypothesis_status(audit,p)).collect::<Vec<_>>(),
        "briefs":audit.briefs.values().collect::<Vec<_>>(),"history_count":audit.history.len(),
        "report_authorized":audit.authorized(),"authorized_revision":audit.authorization.as_ref().map(|a|a.revision),
        "phase":if audit.authorized(){"report_requested"}else{"discussion"},
        "next_step":if audit.authorized(){"Build findings from reviewed briefs, retain all evidence, preview, review and finalize."}else{match audit.current_chapter_id.as_str() {
            "scope" => "Establish the workload, symptom, project and time window before opening hypotheses.",
            "performance_profile" => "Inspect full_gradients, load_profile_anomalies and anomaly_clusters first. Compare their contributors and timestamps with the measured workload and baseline. Record source measurements or specific coverage gaps. Save two or three opening_hypotheses, or explain a missing input in hypothesis_gap.",
            "gradients_anomalies" => "Revisit every opening ID in hypothesis_outcomes. Use confirmed, rejected or unresolved, and explain what the chapter checks established.",
            _ => "Use the project's opening hypotheses. In relevant chapters record hypothesis_checks with the stable ID, observed result or evidence gap. Continue at the auditor's pace."
        }},
        "open_questions":audit.briefs.values().flat_map(|b|b.open_questions.iter().map(move |q|json!({"brief_id":b.brief_id,"question":q}))).collect::<Vec<_>>(),
        "editorial_review_current":audit.review.as_ref().is_some_and(|r|r.preview_id == preview_id(state))})
}

fn preview_id(state: &AnalysisSession) -> String {
    let value = json!({"config":state.config,"evidence":state.evidence,"guidance":state.guidance,"findings":state.findings,
        "tables":state.report_tables,"issues":state.issues,"assessments":state.assessments,
        "audit":state.audit.as_ref().map(|a|json!({"revision":a.revision,"briefs":a.briefs,"authorization":a.authorization}))});
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical_json(&value).hash(&mut hasher);
    format!("preview-{:016x}", hasher.finish())
}

pub(super) fn validate_narrative_source(
    finding: &ReportFinding,
    state: &AnalysisSession,
) -> std::result::Result<(), Value> {
    let Some(audit) = &state.audit else {
        return Ok(());
    };
    let source = finding
        .reader_brief
        .as_ref()
        .and_then(|b| b.source_audit_brief.as_ref());
    let Some(source) = source else {
        if finding.category == "limitations" {
            return Ok(());
        }
        return Err(tool_error(
            "AUDIT_NARRATIVE_SOURCE_REQUIRED",
            format!(
                "Finding {} needs a reader_brief linked to the discussed chapter",
                finding.finding_id
            ),
        ));
    };
    let brief = audit
        .briefs
        .get(&source.brief_id)
        .filter(|b| b.revision == source.revision)
        .ok_or_else(|| {
            tool_error(
                "STALE_AUDIT_NARRATIVE",
                format!(
                    "Review finding {} against the current audit brief",
                    finding.finding_id
                ),
            )
        })?;
    if finding.category != "limitations" && brief.chapter_id != finding.category {
        return Err(tool_error(
            "AUDIT_NARRATIVE_SCOPE_MISMATCH",
            "The narrative must cite its own discussed chapter",
        ));
    }
    if brief
        .evidence_refs
        .iter()
        .any(|r| !finding.evidence_refs.contains(r))
    {
        return Err(tool_error(
            "AUDIT_NARRATIVE_EVIDENCE_MISSING",
            "Retain all source brief evidence in the finding; additional evidence is allowed",
        ));
    }
    let projects = finding
        .evidence_refs
        .iter()
        .filter_map(|r| state.evidence.get(r)?.project_id.as_ref())
        .collect::<BTreeSet<_>>();
    if !projects.is_empty() && !projects.contains(&brief.project_id) {
        return Err(tool_error(
            "AUDIT_NARRATIVE_SCOPE_MISMATCH",
            "Finding evidence does not include the source brief's project",
        ));
    }
    Ok(())
}

pub(super) fn finalization_gate(state: &AnalysisSession) -> std::result::Result<(), Value> {
    let Some(audit) = &state.audit else {
        return Ok(());
    };
    if !audit.authorized() {
        return Err(tool_error("AUDIT_REPORT_NOT_REQUESTED", "Continue the chapter discussion. Call authorize_report only after the user's explicit final request."));
    }
    for finding in state.findings.values() {
        validate_narrative_source(finding, state)?;
    }
    if !audit
        .review
        .as_ref()
        .is_some_and(|r| r.preview_id == preview_id(state))
    {
        return Err(tool_error("REPORT_EDITORIAL_REVIEW_REQUIRED", "Preview the current report and record its editorial review; later changes require another review."));
    }
    Ok(())
}

pub(super) fn document_value(
    analysis_id: &str,
    state: &AnalysisSession,
    projects: &BTreeMap<String, Arc<ProjectData>>,
    revision: u64,
) -> Value {
    let datasets = state
        .project_ids
        .iter()
        .filter_map(|id| projects.get(id))
        .map(|p| p.dataset_manifest())
        .collect::<Vec<_>>();
    json!({"schema_version":MCP_ANALYSIS_SCHEMA_VERSION,"analysis_id":analysis_id,"revision":revision,
        "generated_at":chrono::Utc::now().to_rfc3339(),"project_ids":state.project_ids,
        "dataset":if datasets.len()==1 {datasets[0].clone()}else{Value::Null},"datasets":datasets,"config":state.config,
        "section_index":report_section_index(state),"issues":state.issues.values().collect::<Vec<_>>(),
        "findings":state.findings.values().map(|f|{let mut v=serde_json::to_value(f).unwrap();v["issue_id"]=json!(issue_for_finding(state,&f.finding_id).map(|i|&i.issue_id));v}).collect::<Vec<_>>(),
        "structured_tables":state.report_tables.values().collect::<Vec<_>>(),"mandatory_assessments":state.assessments,
        "evidence":state.evidence,"guidance":state.guidance,"audit":state.audit,
        "coverage":report_status_value(analysis_id,state,projects)})
}

impl AnalysisRuntime {
    pub(super) fn start_audit(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let id = Self::analysis_id(args)?;
        let session = self.session(id)?;
        {
            let mut s = session
                .lock()
                .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
            if s.audit.is_none() {
                s.audit = Some(AuditState::default());
                s.finalized_markdown = None;
            }
        }
        self.audit_status(args)
    }
    pub(super) fn audit_status(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let id = Self::analysis_id(args)?;
        let session = self.session(id)?;
        let s = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
        let mut value = status_value(id, &s);
        value["completeness"] = report_status_value(id, &s, &self.projects);
        Ok(value)
    }
    pub(super) fn record_audit_brief(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let id = Self::analysis_id(args)?;
        let session = self.session(id)?;
        {
            let mut s = session
                .lock()
                .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
            let mut fields = args.clone();
            fields.remove("analysis_id");
            let mut brief: AuditBrief = serde_json::from_value(Value::Object(fields))
                .map_err(|e| tool_error("INVALID_AUDIT_BRIEF", e.to_string()))?;
            validate_brief(&brief, &s)?;
            hypotheses::validate_content(&brief, &s, &self.projects[&brief.project_id])?;
            hypotheses::validate_opening_windows(&brief, &s, &self.projects[&brief.project_id])?;
            let AnalysisSession {
                evidence, audit, ..
            } = &mut *s;
            let audit = audit
                .as_mut()
                .ok_or_else(|| tool_error("AUDIT_NOT_STARTED", "Call start_audit first"))?;
            if audit.history.len() >= 4096
                || (audit.briefs.len() >= 512 && !audit.briefs.contains_key(&brief.brief_id))
            {
                return Err(tool_error(
                    "AUDIT_CAPACITY",
                    "Export this complete audit before starting another; nothing was dropped",
                ));
            }
            if brief.brief_id.is_empty() {
                let mut next = audit.revision;
                loop {
                    brief.brief_id = format!("B-{next:04}");
                    if !audit.briefs.contains_key(&brief.brief_id) {
                        break;
                    }
                    next += 1;
                }
            }
            if brief.brief_id.len() > 64
                || !brief
                    .brief_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err(tool_error("INVALID_AUDIT_BRIEF", "Invalid brief_id"));
            }

            brief.hypothesis_basis = hypotheses::validate_dependencies(&brief, audit, evidence)?;
            if let Some(old) = audit.briefs.get(&brief.brief_id) {
                if old.project_id != brief.project_id || old.chapter_id != brief.chapter_id {
                    return Err(tool_error("AUDIT_BRIEF_SCOPE_CHANGED","Update within the original project and chapter; create a new brief for another scope"));
                }
                brief.revision = old.revision + 1;
                audit.history.push(old.clone());
            } else {
                brief.revision = 1;
            }
            brief.updated_at_revision = audit.revision + 1;
            audit.briefs.insert(brief.brief_id.clone(), brief);
            audit.changed();
            s.finalized_markdown = None;
        }
        self.audit_status(args)
    }
    pub(super) fn advance_audit(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let id = Self::analysis_id(args)?;
        let chapter = required_string(args, "chapter_id", 64)?;
        if !chapter_exists(&chapter) {
            return Err(tool_error("UNKNOWN_AUDIT_CHAPTER", "Use a listed chapter"));
        }
        let session = self.session(id)?;
        {
            let mut s = session
                .lock()
                .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
            let projects = s.project_ids.clone();
            let audit = s
                .audit
                .as_mut()
                .ok_or_else(|| tool_error("AUDIT_NOT_STARTED", "Call start_audit first"))?;
            if args.get("complete_current") == Some(&Value::Bool(true))
                && !audit.discussed(&audit.current_chapter_id, &projects)
            {
                return Err(tool_error("AUDIT_BRIEF_REQUIRED","Save the current explanation or explicit coverage gap before completing the chapter"));
            }
            if audit.current_chapter_id != chapter {
                audit.current_chapter_id = chapter;
                audit.changed();
                s.finalized_markdown = None;
            }
        }
        self.audit_status(args)
    }
    pub(super) fn authorize_report(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let id = Self::analysis_id(args)?;
        let request = required_string(args, "user_request", 4000)?;
        let session = self.session(id)?;
        {
            let mut s = session
                .lock()
                .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
            let projects = s.project_ids.clone();
            let audit = s
                .audit
                .as_mut()
                .ok_or_else(|| tool_error("AUDIT_NOT_STARTED", "Call start_audit first"))?;
            let mut pending = Vec::new();
            for project in &projects {
                for c in CHAPTERS {
                    if !audit.project_discussed(c.0, project) {
                        pending.push(format!("{project}:{}", c.0));
                    }
                }
            }
            if !pending.is_empty() {
                return Err(
                    json!({"error_code":"AUDIT_DISCUSSION_INCOMPLETE","message":"Save the discussed conclusions or explicit coverage gaps before requesting the report.","pending_chapters":pending}),
                );
            }
            audit.authorization = Some(ReportAuthorization {
                revision: audit.revision,
                user_request: request,
            });
            audit.review = None;
            s.finalized_markdown = None;
        }
        self.audit_status(args)
    }
    pub(super) fn preview_report(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let id = Self::analysis_id(args)?;
        let session = self.session(id)?;
        let s = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
        if s.audit.as_ref().is_some_and(|a| !a.authorized()) {
            return Err(tool_error(
                "AUDIT_REPORT_NOT_REQUESTED",
                "The auditor has not requested the report yet",
            ));
        }
        let format = args
            .get("format")
            .and_then(Value::as_str)
            .unwrap_or("markdown");
        validate_enum("format", format, &["markdown", "html", "both"])?;
        let links = ReportEntityLinks::build(&s.project_ids, &self.projects);
        let mut doc = document_value(id, &s, &self.projects, s.report_revision + 1);
        doc["preview_notice"] = json!(if s.config.language.eq_ignore_ascii_case("PL") {
            "Podgląd redakcyjny — raport nie został jeszcze opublikowany."
        } else {
            "Editorial preview — this report has not been published."
        });
        doc["signal_atlas"] = json!(report_signal_adapter::build_signal_atlas(&doc, &s, &links));
        let markdown = render_markdown(&doc, &s, &links);
        let mut result = json!({"analysis_id":id,"preview_id":preview_id(&s),"preview":true,"published":false,"status":doc["coverage"],"editorial_checklist":["explanation_clear","actions_executable","uncertainty_preserved","evidence_preserved","language_consistent"]});
        if format != "html" {
            result["markdown"] = json!(markdown);
        }
        if format != "markdown" {
            result["html"] = json!(render_markdown_html_document(
                &markdown,
                "",
                "",
                HashMap::new()
            ));
        }
        Ok(result)
    }
    pub(super) fn record_report_review(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let id = Self::analysis_id(args)?;
        let session = self.session(id)?;
        let mut s = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
        let mut fields = args.clone();
        fields.remove("analysis_id");
        let review: EditorialReview = serde_json::from_value(Value::Object(fields))
            .map_err(|e| tool_error("INVALID_EDITORIAL_REVIEW", e.to_string()))?;
        if review.preview_id != preview_id(&s) {
            return Err(tool_error(
                "STALE_REPORT_PREVIEW",
                "Preview the current report after the latest change",
            ));
        }
        let expected = [
            "explanation_clear",
            "actions_executable",
            "uncertainty_preserved",
            "evidence_preserved",
            "language_consistent",
        ];
        if review.checks.len() != expected.len()
            || expected
                .iter()
                .any(|k| review.checks.get(*k) != Some(&true))
            || review.notes.trim().is_empty()
            || review.notes.len() > 8000
        {
            return Err(tool_error(
                "EDITORIAL_REVIEW_INCOMPLETE",
                "Read the actual preview and record all five checks plus specific review notes",
            ));
        }
        let audit = s.audit.as_mut().ok_or_else(|| {
            tool_error(
                "AUDIT_NOT_STARTED",
                "Editorial publication gate is part of guided audits",
            )
        })?;
        if !audit.authorized() {
            return Err(tool_error(
                "AUDIT_REPORT_NOT_REQUESTED",
                "Await the explicit final report request",
            ));
        }
        audit.review = Some(review);
        Ok(json!({"analysis_id":id,"review_recorded":true,"preview_id":preview_id(&s)}))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditCheckpoint {
    version: u32,
    schema_version: String,
    project_ids: Vec<String>,
    datasets: Vec<Value>,
    config: ReportConfig,
    evidence: BTreeMap<String, EvidenceRecord>,
    guidance: BTreeMap<String, GuidanceRecord>,
    findings: BTreeMap<String, ReportFinding>,
    issues: BTreeMap<String, ReportIssue>,
    issue_finding_snapshots: BTreeMap<String, BTreeMap<String, Value>>,
    assessments: BTreeMap<String, ReportAssessment>,
    report_tables: BTreeMap<String, ReportTable>,
    audit: AuditState,
}
// JavaScript transports normalize 3600.0 to 3600. Compare exact decimal values,
// never lossy f64 conversions (which would conflate adjacent large integer IDs).
fn decimal_key(number: &serde_json::Number) -> (bool, String, i32) {
    let printed = number.to_string();
    let negative = printed.starts_with('-');
    let unsigned = printed.trim_start_matches('-');
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map(|(m, e)| (m, e.parse::<i32>().unwrap_or(0)))
        .unwrap_or((unsigned, 0));
    let fractional = mantissa.split_once('.').map(|(_, f)| f.len()).unwrap_or(0) as i32;
    let mut digits = mantissa
        .replace('.', "")
        .trim_start_matches('0')
        .to_string();
    if digits.is_empty() {
        return (false, "0".into(), 0);
    }
    let mut scale = exponent - fractional;
    while digits.ends_with('0') {
        digits.pop();
        scale += 1;
    }
    (negative, digits, scale)
}
fn same_json_measurements(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => decimal_key(a) == decimal_key(b),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_json_measurements(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, x)| b.get(key).is_some_and(|y| same_json_measurements(x, y)))
        }
        _ => left == right,
    }
}

fn max_counter<'a>(ids: impl Iterator<Item = &'a String>, prefix: &str) -> u64 {
    ids.filter_map(|id| id.strip_prefix(prefix)?.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1
}
impl AnalysisRuntime {
    pub(super) fn export_audit(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let id = Self::analysis_id(args)?;
        let session = self.session(id)?;
        let s = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
        let audit = s
            .audit
            .clone()
            .ok_or_else(|| tool_error("AUDIT_NOT_STARTED", "Call start_audit first"))?;
        let checkpoint = AuditCheckpoint {
            version: 1,
            schema_version: MCP_ANALYSIS_SCHEMA_VERSION.into(),
            project_ids: s.project_ids.clone(),
            datasets: s
                .project_ids
                .iter()
                .filter_map(|p| self.projects.get(p))
                .map(|p| p.dataset_manifest())
                .collect(),
            config: s.config.clone(),
            evidence: s.evidence.clone(),
            guidance: s.guidance.clone(),
            findings: s.findings.clone(),
            issues: s.issues.clone(),
            issue_finding_snapshots: s.issue_finding_snapshots.clone(),
            assessments: s.assessments.clone(),
            report_tables: s.report_tables.clone(),
            audit,
        };
        if serialized_json_size(&checkpoint) > 64 * 1024 * 1024 {
            return Err(tool_error("CHECKPOINT_TOO_LARGE","Complete checkpoint exceeds the current 64 MiB transport limit; session evidence remains intact"));
        }
        Ok(
            json!({"analysis_id":id,"checkpoint":checkpoint,"persisted":false,
            "instruction":"Save checkpoint as a private JSON file in the configured audit workspace. It includes every evidence payload and discussion revision. This response alone is not a saved file. Restore replays sources and fails if evidence has changed; publication authorization is never restored."}),
        )
    }
    pub(super) fn restore_audit(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, Value> {
        let value = args
            .get("checkpoint")
            .ok_or_else(|| tool_error("INVALID_CHECKPOINT", "checkpoint is required"))?;
        if serialized_json_size(value) > 64 * 1024 * 1024 {
            return Err(tool_error(
                "CHECKPOINT_TOO_LARGE",
                "Maximum checkpoint size is 64 MiB",
            ));
        }
        let saved: AuditCheckpoint = serde_json::from_value(value.clone())
            .map_err(|e| tool_error("INVALID_CHECKPOINT", e.to_string()))?;
        if saved.version != 1 || saved.schema_version != MCP_ANALYSIS_SCHEMA_VERSION {
            return Err(tool_error(
                "CHECKPOINT_VERSION",
                "Checkpoint requires the same analysis schema",
            ));
        }
        let projects = self.selected_project_ids(&json!({"project_ids":saved.project_ids}))?;
        let manifests = projects
            .iter()
            .map(|p| self.projects[p].dataset_manifest())
            .collect::<Vec<_>>();
        if !same_json_measurements(&json!(manifests), &json!(saved.datasets)) {
            return Err(tool_error("CHECKPOINT_DATASET_CHANGED","Selected datasets or source inventories differ; start a new audit and retain the old archive"));
        }
        let created =
            self.new_analysis(&json!({"project_ids":projects,"language":saved.config.language}))?;
        let id = created["analysis_id"].as_str().unwrap().to_string();
        // Rehydrate only into a fresh, isolated session. A failed restore leaves no partial analysis.
        let result = self.restore_audit_into(&id, saved);
        if result.is_err() {
            self.sessions.remove(&id);
        }
        result
    }
    fn restore_audit_into(
        &self,
        id: &str,
        saved: AuditCheckpoint,
    ) -> std::result::Result<Value, Value> {
        let session = self.session(id)?;
        let mut verified = BTreeMap::new();
        let mut reverse_ids = HashMap::new();
        let evidence_tools = build_mcp_tools(self)
            .into_iter()
            .filter(|t| {
                t.annotations
                    .as_ref()
                    .is_some_and(|a| a.read_only_hint == Some(true))
            })
            .map(|t| t.name.to_string())
            .collect::<HashSet<_>>();
        for (key, record) in &saved.evidence {
            if key != &record.evidence_id {
                return Err(tool_error(
                    "INVALID_CHECKPOINT",
                    "Evidence key and identity differ",
                ));
            }
            if key == SEED_EVIDENCE_ID {
                let s = session
                    .lock()
                    .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
                if !same_json_measurements(
                    &serde_json::to_value(&s.evidence[SEED_EVIDENCE_ID]).unwrap(),
                    &serde_json::to_value(record).unwrap(),
                ) {
                    return Err(tool_error(
                        "CHECKPOINT_EVIDENCE_CHANGED",
                        "The initial evidence seed has changed",
                    ));
                }
                verified.insert(key.clone(), s.evidence[SEED_EVIDENCE_ID].clone());
                continue;
            }
            if !evidence_tools.contains(&record.tool_name)
                || matches!(
                    record.tool_name.as_str(),
                    "get_audit_status"
                        | "export_audit"
                        | "preview_report"
                        | "get_report_status"
                        | "get_analysis_catalog"
                        | "list_performance_projects"
                        | "get_diagnostic_guidance"
                )
            {
                return Err(tool_error(
                    "INVALID_CHECKPOINT",
                    "Checkpoint contains a non-measurement tool",
                ));
            }
            if let Some(project) = &record.project_id {
                if !saved.project_ids.contains(project) {
                    return Err(tool_error(
                        "INVALID_CHECKPOINT",
                        "Evidence project is outside the audit",
                    ));
                }
            }
            let mut args = record.arguments.as_object().cloned().ok_or_else(|| {
                tool_error("INVALID_CHECKPOINT", "Evidence arguments must be an object")
            })?;
            args.insert("analysis_id".into(), json!(id));
            if let Some(project) = &record.project_id {
                args.insert("project_id".into(), json!(project));
            }
            let replay = self.call_tool(&record.tool_name, args)?;
            if !replay
                .get("result")
                .is_some_and(|result| same_json_measurements(result, &record.result))
            {
                return Err(tool_error("CHECKPOINT_EVIDENCE_CHANGED",format!("Replayed evidence {} has changed; retain the archive and investigate the new source",key)));
            }
            let new_id = replay["evidence_id"].as_str().ok_or_else(|| {
                tool_error(
                    "INVALID_CHECKPOINT",
                    "Replay did not return evidence identity",
                )
            })?;
            if reverse_ids
                .insert(new_id.to_string(), key.clone())
                .is_some()
            {
                return Err(tool_error(
                    "INVALID_CHECKPOINT",
                    "Duplicate measurement identities",
                ));
            }
            let mut verified_record = record.clone();
            verified_record.result = replay["result"].clone();
            verified.insert(key.clone(), verified_record);
        }
        if !verified.contains_key(SEED_EVIDENCE_ID) {
            return Err(tool_error(
                "INVALID_CHECKPOINT",
                "Initial evidence seed is missing",
            ));
        }
        for (reference, record) in &saved.guidance {
            let topic = reference
                .strip_prefix("GUIDE-")
                .ok_or_else(|| tool_error("INVALID_CHECKPOINT", "Invalid guidance reference"))?;
            self.diagnostic_guidance(
                &serde_json::from_value(json!({"analysis_id":id,"topic":topic,"max_sections":1}))
                    .unwrap(),
            )?;
            let s = session
                .lock()
                .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
            if !s.guidance.get(reference).is_some_and(|g| {
                g.title == record.title
                    && !record.text.is_empty()
                    && g.text.starts_with(&record.text)
            }) {
                return Err(tool_error(
                    "CHECKPOINT_GUIDANCE_CHANGED",
                    "Methodology changed since this checkpoint",
                ));
            }
        }
        {
            let mut s = session
                .lock()
                .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
            for cached in s.evidence_cache.values_mut() {
                if let Some(old) = reverse_ids.get(cached) {
                    *cached = old.clone();
                }
            }
            s.evidence = verified;
            s.next_evidence = max_counter(s.evidence.keys(), "E-");
            s.guidance = saved.guidance;
            s.findings = saved.findings.clone();
            s.report_tables = saved.report_tables.clone();
            s.next_finding = max_counter(s.findings.keys(), "F-");
            s.next_table = max_counter(s.report_tables.keys(), "T-");
        }
        let mut config = serde_json::to_value(&saved.config)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        config.insert("analysis_id".into(), json!(id));
        self.configure_report(&config)?;
        // Re-run the existing validation, including attribution and quotations, rather than trusting archived prose.
        for (key, table) in &saved.report_tables {
            if key != &table.table_id {
                return Err(tool_error("INVALID_CHECKPOINT", "Table identity mismatch"));
            }
            let mut args = serde_json::to_value(table)
                .unwrap()
                .as_object()
                .unwrap()
                .clone();
            args.insert("analysis_id".into(), json!(id));
            self.record_report_table(&args)?;
        }
        for (key, finding) in &saved.findings {
            if key != &finding.finding_id {
                return Err(tool_error(
                    "INVALID_CHECKPOINT",
                    "Finding identity mismatch",
                ));
            }
            let mut args = serde_json::to_value(finding)
                .unwrap()
                .as_object()
                .unwrap()
                .clone();
            args.insert("analysis_id".into(), json!(id));
            self.record_finding(&args)?;
        }
        for (key, assessment) in &saved.assessments {
            if key != &assessment.assessment {
                return Err(tool_error(
                    "INVALID_CHECKPOINT",
                    "Assessment identity mismatch",
                ));
            }
            let mut args = serde_json::to_value(assessment)
                .unwrap()
                .as_object()
                .unwrap()
                .clone();
            args.insert("analysis_id".into(), json!(id));
            self.set_assessment(&args)?;
        }
        for (key, issue) in &saved.issues {
            if key != &issue.issue_id {
                return Err(tool_error("INVALID_CHECKPOINT", "Issue identity mismatch"));
            }
            let mut args = serde_json::to_value(issue)
                .unwrap()
                .as_object()
                .unwrap()
                .clone();
            args.insert("analysis_id".into(), json!(id));
            self.record_issue(&args)?;
        }
        {
            let mut s = session
                .lock()
                .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
            if saved.audit.briefs.len() > 512
                || saved.audit.history.len() > 4096
                || !chapter_exists(&saved.audit.current_chapter_id)
            {
                return Err(tool_error(
                    "INVALID_CHECKPOINT",
                    "Invalid audit discussion state",
                ));
            }
            for (key, brief) in &saved.audit.briefs {
                if key != &brief.brief_id || brief.revision == 0 {
                    return Err(tool_error(
                        "INVALID_CHECKPOINT",
                        "Brief identity/revision mismatch",
                    ));
                }
            }
            // Recheck each hypothesis against the opening and chapter evidence available at that revision.
            let mut replay = AuditState::default();
            let mut revisions: Vec<_> = saved
                .audit
                .history
                .iter()
                .chain(saved.audit.briefs.values())
                .collect();
            revisions.sort_by_key(|b| b.updated_at_revision);
            for brief in revisions {
                validate_brief(brief, &s)?;
                hypotheses::validate_content(brief, &s, &self.projects[&brief.project_id])?;
                if hypotheses::validate_dependencies(brief, &replay, &s.evidence)?
                    != brief.hypothesis_basis
                {
                    return Err(tool_error(
                        "INVALID_CHECKPOINT",
                        "Hypothesis sources do not match the archived discussion revision",
                    ));
                }
                replay.briefs.insert(brief.brief_id.clone(), brief.clone());
            }
            let mut audit = saved.audit;
            audit.authorization = None;
            audit.review = None;
            s.audit = Some(audit);
            s.issue_finding_snapshots = saved.issue_finding_snapshots;
            s.finalized_markdown = None;
        }
        self.audit_status(&serde_json::from_value(json!({"analysis_id":id})).unwrap())
    }
}

pub(super) fn tool_definitions() -> Vec<Value> {
    let simple = json!({"type":"object","additionalProperties":false,"properties":{}});
    let mut brief_props = json!({"brief_id":{"type":"string"},"project_id":{"type":"string"},"chapter_id":{"type":"string"},
        "question":{"type":"string"},"explanation":{"type":"string"},"conclusion":{"type":"string"},"evidence_boundary":{"type":"string"},"next_step":{"type":"string"},
        "open_questions":{"type":"array","items":{"type":"string"}},"evidence_refs":{"type":"array","items":{"type":"string"}},"visual":{"type":"object"}});
    hypotheses::add_tool_fields(&mut brief_props);
    brief_props["evidence_refs"]["description"]=json!("Exact evidence IDs from this analysis and project. Empty means discussion/coverage gap, never measured proof.");
    vec![
        function_definition("start_audit","Start or resume a guided chapter discussion. This does not collect measurements or authorize a report.",simple.clone()),
        function_definition("get_audit_status","Read current chapter, durable session briefs, open questions and independent evidence completeness. Chapters discussed never imply causal proof or permission to publish.",simple.clone()),
        function_definition("record_audit_brief","Save the chapter discussion. In performance_profile submit opening_hypotheses or hypothesis_gap after scope. Use hypothesis_checks in relevant subsequent chapters and hypothesis_outcomes in gradients_anomalies. Reuse the same brief_id for corrections and retain its structured fields. This marks the chapter discussed, not user-approved. Changes revoke report authorization.",json!({"type":"object","additionalProperties":false,"properties":brief_props,"required":["project_id","chapter_id","question","explanation","conclusion","evidence_boundary","next_step","evidence_refs"]})),
        function_definition("advance_audit","Move to the chapter the auditor wants to discuss; allow revisits. Do not advance automatically through the entire audit.",json!({"type":"object","additionalProperties":false,"properties":{"chapter_id":{"type":"string"},"complete_current":{"type":"boolean"}},"required":["chapter_id"]})),
        function_definition("authorize_report","Record the auditor's explicit final request to turn the completed discussion into a report. Quote the actual user request; never infer it from chapter completion. Does not bypass evidence or editorial gates.",json!({"type":"object","additionalProperties":false,"properties":{"user_request":{"type":"string","minLength":1}},"required":["user_request"]})),
        function_definition("export_audit","Export every discussion revision, evidence payload and report state as a portable checkpoint. Save the returned checkpoint in a private configured file; the tool itself does not persist it.",simple.clone()),
        function_definition("restore_audit","Restore an exported checkpoint in a new analysis after verifying the datasets and replaying every evidence source. Changed evidence rejects the restore. Report authorization and editorial review are cleared.",json!({"type":"object","additionalProperties":false,"properties":{"checkpoint":{"type":"object"}},"required":["checkpoint"]})),
        function_definition("preview_report","Render the current report for editorial review without publishing, finalizing or incrementing its revision. Guided audits require the user's final report request first. Completeness gaps remain visible.",json!({"type":"object","additionalProperties":false,"properties":{"format":{"type":"string","enum":["markdown","html","both"],"default":"markdown"}}})),
        function_definition("record_report_review","After reading the actual preview, record semantic checks and specific review notes. Checks cover clarity, executable actions, preserved uncertainty/evidence and consistent language. Any later content change requires a fresh review.",json!({"type":"object","additionalProperties":false,"properties":{"preview_id":{"type":"string"},"checks":{"type":"object","additionalProperties":false,"properties":{"explanation_clear":{"type":"boolean"},"actions_executable":{"type":"boolean"},"uncertainty_preserved":{"type":"boolean"},"evidence_preserved":{"type":"boolean"},"language_consistent":{"type":"boolean"}},"required":["explanation_clear","actions_executable","uncertainty_preserved","evidence_preserved","language_consistent"]},"notes":{"type":"string"}},"required":["preview_id","checks","notes"]})),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn call(
        runtime: &AnalysisRuntime,
        name: &str,
        value: Value,
    ) -> std::result::Result<Value, Value> {
        runtime.call_tool(name, value.as_object().unwrap().clone())
    }
    fn start(runtime: &AnalysisRuntime) -> (String, String) {
        let result = call(
            runtime,
            "start_performance_analysis",
            json!({"language":"PL"}),
        )
        .unwrap();
        let id = result["analysis_id"].as_str().unwrap().to_string();
        let session = runtime.session(&id).unwrap();
        let project = session.lock().unwrap().project_ids[0].clone();
        call(runtime, "start_audit", json!({"analysis_id":id})).unwrap();
        (id, project)
    }
    fn brief(id: &str, project: &str, chapter: &str) -> Value {
        json!({"analysis_id":id,"brief_id":format!("brief-{chapter}"),"project_id":project,"chapter_id":chapter,
        "question":"What do the supplied samples establish?","explanation":"The sampled interval is preserved separately from hypotheses about unsampled work.",
        "conclusion":"The available measurement identifies the next comparison, not a proven cause.","evidence_boundary":"This limited fixture does not contain a complete production audit.",
        "next_step":"Compare the observed interval with a matched baseline before recommending a change.","open_questions":["Does the same workload recur?"],"evidence_refs":[SEED_EVIDENCE_ID]})
    }
    fn discuss_all(runtime: &AnalysisRuntime, id: &str, project: &str) {
        for c in CHAPTERS {
            let mut entry = brief(id, project, c.0);
            if c.0 == "performance_profile" {
                entry["hypothesis_gap"] =
                    json!("No DB Time or DB CPU gradient was available for the sampled interval.");
            }
            call(runtime, "record_audit_brief", entry).unwrap();
        }
    }
    fn authorize(runtime: &AnalysisRuntime, id: &str) {
        call(runtime,"authorize_report",json!({"analysis_id":id,"user_request":"We have discussed the chapters; please create the report now."})).unwrap();
    }
    fn review(runtime: &AnalysisRuntime, id: &str) -> String {
        let preview = call(
            runtime,
            "preview_report",
            json!({"analysis_id":id,"format":"both"}),
        )
        .unwrap();
        assert_eq!(preview["published"], false);
        assert!(preview["html"].as_str().unwrap().contains("<html"));
        let pid = preview["preview_id"].as_str().unwrap().to_string();
        call(runtime,"record_report_review",json!({"analysis_id":id,"preview_id":pid,"checks":{"explanation_clear":true,"actions_executable":true,"uncertainty_preserved":true,"evidence_preserved":true,"language_consistent":true},"notes":"Reviewed the fixture preview: missing coverage is explicit and no intervention is presented as proven."})).unwrap();
        pid
    }
    #[test]
    fn discussion_requires_explicit_request_and_current_review_even_for_drafts() {
        let runtime = super::super::tests::runtime();
        let (id, project) = start(&runtime);
        for name in ["preview_report", "finalize_report"] {
            assert_eq!(
                call(
                    &runtime,
                    name,
                    json!({"analysis_id":id,"allow_incomplete":true})
                )
                .unwrap_err()["error_code"],
                "AUDIT_REPORT_NOT_REQUESTED"
            );
        }
        assert_eq!(
            call(
                &runtime,
                "authorize_report",
                json!({"analysis_id":id,"user_request":"Create it"})
            )
            .unwrap_err()["error_code"],
            "AUDIT_DISCUSSION_INCOMPLETE"
        );
        discuss_all(&runtime, &id, &project);
        assert_eq!(
            call(&runtime, "get_audit_status", json!({"analysis_id":id})).unwrap()
                ["report_authorized"],
            false
        );
        authorize(&runtime, &id);
        assert_eq!(
            call(
                &runtime,
                "finalize_report",
                json!({"analysis_id":id,"allow_incomplete":true})
            )
            .unwrap_err()["error_code"],
            "REPORT_EDITORIAL_REVIEW_REQUIRED"
        );
        review(&runtime, &id);
        assert_eq!(
            call(&runtime, "finalize_report", json!({"analysis_id":id})).unwrap_err()["error_code"],
            "REPORT_INCOMPLETE"
        );
        let report = call(
            &runtime,
            "finalize_report",
            json!({"analysis_id":id,"allow_incomplete":true}),
        )
        .unwrap();
        assert_eq!(report["draft"], true);
        assert!(report["report"]["evidence"].get(SEED_EVIDENCE_ID).is_some());
        let mut revised = brief(&id, &project, "sql");
        revised["conclusion"]=json!("A new comparison contradicts the earlier explanation; the remaining uncertainty is explicit.");
        let status = call(&runtime, "record_audit_brief", revised).unwrap();
        assert_eq!(status["report_authorized"], false);
        assert_eq!(status["history_count"], 1);
        let state = runtime.session(&id).unwrap();
        assert!(state.lock().unwrap().finalized_markdown.is_none());
    }
    #[test]
    fn evidence_and_configuration_changes_require_another_editorial_review() {
        let runtime = super::super::tests::runtime();
        let (id, project) = start(&runtime);
        discuss_all(&runtime, &id, &project);
        authorize(&runtime, &id);
        review(&runtime, &id);
        call(
            &runtime,
            "configure_report",
            json!({"analysis_id":id,"detail_level":"deep"}),
        )
        .unwrap();
        assert_eq!(
            call(
                &runtime,
                "finalize_report",
                json!({"analysis_id":id,"allow_incomplete":true})
            )
            .unwrap_err()["error_code"],
            "REPORT_EDITORIAL_REVIEW_REQUIRED"
        );
        review(&runtime, &id);
        call(
            &runtime,
            "get_database_load_summary",
            json!({"analysis_id":id,"project_id":project}),
        )
        .unwrap();
        assert_eq!(
            call(
                &runtime,
                "finalize_report",
                json!({"analysis_id":id,"allow_incomplete":true})
            )
            .unwrap_err()["error_code"],
            "REPORT_EDITORIAL_REVIEW_REQUIRED"
        );
    }
    #[test]
    fn checkpoint_replays_all_evidence_retains_revisions_and_clears_permission() {
        let runtime = super::super::tests::runtime();
        let (id, project) = start(&runtime);
        let ev = call(
            &runtime,
            "get_database_load_summary",
            json!({"analysis_id":id,"project_id":project}),
        )
        .unwrap();
        discuss_all(&runtime, &id, &project);
        let mut revised = brief(&id, &project, "sql");
        revised["evidence_refs"] = json!([ev["evidence_id"]]);
        call(&runtime, "record_audit_brief", revised).unwrap();
        authorize(&runtime, &id);
        review(&runtime, &id);
        let checkpoint = call(&runtime, "export_audit", json!({"analysis_id":id})).unwrap()
            ["checkpoint"]
            .clone();
        let path = std::env::temp_dir().join(format!("jasmin-audit-checkpoint-{id}.json"));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.write_all(&serde_json::to_vec(&checkpoint).unwrap())
            .unwrap();
        drop(file);
        drop(runtime);
        let runtime = super::super::tests::runtime();
        let checkpoint: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        let restored = call(&runtime, "restore_audit", json!({"checkpoint":checkpoint})).unwrap();
        assert_ne!(restored["analysis_id"], id);
        assert_eq!(restored["report_authorized"], false);
        assert_eq!(restored["history_count"], 1);
        assert_eq!(restored["briefs"].as_array().unwrap().len(), CHAPTERS.len());
        let new_checkpoint = call(
            &runtime,
            "export_audit",
            json!({"analysis_id":restored["analysis_id"]}),
        )
        .unwrap()["checkpoint"]
            .clone();
        assert_eq!(new_checkpoint["evidence"], checkpoint["evidence"]);
        assert_eq!(
            new_checkpoint["audit"]["history"],
            checkpoint["audit"]["history"]
        );
        let count = runtime.sessions.len();
        let mut tampered = checkpoint.clone();
        tampered["evidence"][ev["evidence_id"].as_str().unwrap()]["result"] =
            json!({"invented":true});
        assert_eq!(
            call(&runtime, "restore_audit", json!({"checkpoint":tampered})).unwrap_err()
                ["error_code"],
            "CHECKPOINT_EVIDENCE_CHANGED"
        );
        assert_eq!(runtime.sessions.len(), count);
    }
    #[test]
    fn legacy_checkpoint_can_resume_the_opening_without_losing_existing_chapters() {
        let runtime = super::super::tests::runtime();
        let (id, project) = start(&runtime);
        for (chapter, ..) in CHAPTERS {
            call(
                &runtime,
                "record_audit_brief",
                brief(&id, &project, chapter),
            )
            .unwrap();
        }
        let checkpoint = call(&runtime, "export_audit", json!({"analysis_id":id})).unwrap()
            ["checkpoint"]
            .clone();
        assert!(checkpoint["audit"]["briefs"]["brief-performance_profile"]
            .get("hypothesis_basis")
            .is_none());
        let restored = call(&runtime, "restore_audit", json!({"checkpoint":checkpoint})).unwrap();
        assert_eq!(restored["briefs"].as_array().unwrap().len(), CHAPTERS.len());
        assert_eq!(restored["hypotheses"][0]["opening_hypotheses"], json!([]));
        let resumed = restored["analysis_id"].as_str().unwrap();
        let pending = call(
            &runtime,
            "authorize_report",
            json!({"analysis_id":resumed,"user_request":"Prepare the report."}),
        )
        .unwrap_err();
        assert_eq!(
            pending["pending_chapters"],
            json!([
                format!("{project}:performance_profile"),
                format!("{project}:gradients_anomalies")
            ])
        );
        let mut revised = brief(resumed, &project, "performance_profile");
        revised["hypothesis_gap"] = json!("No fitted gradients are available in this capture.");
        call(&runtime, "record_audit_brief", revised).unwrap();
        authorize(&runtime, resumed);
    }

    #[test]
    fn invalid_refs_and_cross_project_edits_do_not_replace_saved_brief() {
        let runtime = super::super::tests::runtime();
        let (id, project) = start(&runtime);
        let good = brief(&id, &project, "sql");
        call(&runtime, "record_audit_brief", good.clone()).unwrap();
        let before = call(&runtime, "export_audit", json!({"analysis_id":id})).unwrap();
        let mut invalid = good.clone();
        invalid["evidence_refs"] = json!(["E-missing"]);
        assert!(call(&runtime, "record_audit_brief", invalid).is_err());
        let mut wrong = good;
        wrong["project_id"] = json!("another-project");
        assert!(call(&runtime, "record_audit_brief", wrong).is_err());
        assert_eq!(
            before,
            call(&runtime, "export_audit", json!({"analysis_id":id})).unwrap()
        );
    }
    #[test]
    fn project_hypotheses_require_followup_measurements_and_current_scope() {
        let mut runtime = super::super::tests::runtime();
        let data = Arc::make_mut(&mut runtime.projects)
            .values_mut()
            .next()
            .unwrap();
        let data = Arc::make_mut(data);
        let snapshots = &mut Arc::make_mut(&mut data.collection).awrs;
        snapshots[0].load_profile.push(crate::awr::LoadProfile {
            stat_name: "DB time".into(),
            per_second: 2.0,
            ..Default::default()
        });
        snapshots[0]
            .foreground_wait_events
            .push(crate::awr::WaitEvents {
                event: "db file sequential read".into(),
                waits: 2000,
                total_wait_time_s: 8.0,
                avg_wait: 4.0,
                ..Default::default()
            });
        let mut baseline = snapshots[0].clone();
        baseline.snap_info = crate::awr::SnapInfo {
            begin_snap_id: 9,
            end_snap_id: 10,
            begin_snap_time: "2026-08-01 09:00".into(),
            end_snap_time: "2026-08-01 10:00".into(),
        };
        baseline.load_profile[0].per_second = 1.0;
        baseline.foreground_wait_events[0].waits = 1000;
        baseline.foreground_wait_events[0].total_wait_time_s = 4.0;
        snapshots.insert(0, baseline);
        Arc::make_mut(&mut data.report).db_time_gradient_fg_wait_events =
            Some(DbTimeGradientSection {
                ridge_top: vec![crate::reasonings::GradientTopItem {
                    event_name: "db file sequential read".into(),
                    impact_peak: 1.5,
                    ..Default::default()
                }],
                ..Default::default()
            });
        let report = Arc::make_mut(&mut data.report);
        report
            .load_profile_anomalies
            .push(crate::reasonings::LoadProfileAnomalies {
                load_profile_stat_name: "Physical reads".into(),
                anomaly_date: "2026-08-01 10:00".into(),
                per_second: 22.,
                mad_score: 6.,
                ..Default::default()
            });
        report
            .anomaly_clusters
            .push(crate::reasonings::AnomlyCluster {
                begin_snap_id: 10,
                begin_snap_date: "2026-08-01 10:00".into(),
                number_of_anomalies: 2,
                ..Default::default()
            });
        let (id, project) = start(&runtime);
        let save = |entry| call(&runtime, "record_audit_brief", entry);
        let load = call(
            &runtime,
            "get_database_load_summary",
            json!({"analysis_id":id}),
        )
        .unwrap()["evidence_id"]
            .clone();
        let gradient = call(
            &runtime,
            "get_precomputed_analysis",
            json!({"analysis_id":id,"section":"full_gradients"}),
        )
        .unwrap()["evidence_id"]
            .clone();
        let timeline = call(
            &runtime,
            "get_wait_event_timeline",
            json!({"analysis_id":id,"event_name":"db file sequential read","kind":"foreground"}),
        )
        .unwrap();
        assert_eq!(timeline["result"]["timeline"][0]["waits"], 1000);
        assert_eq!(timeline["result"]["timeline"][1]["waits"], 2000);
        let measured = timeline["evidence_id"].clone();
        let scope = brief(&id, &project, "scope");
        let mut profile = brief(&id, &project, "performance_profile");
        profile["evidence_refs"] = json!([load, gradient]);
        profile["opening_hypotheses"] = json!(["Higher demand","Higher elapsed time per read","Changed access path"].into_iter().enumerate().map(|(index,statement)|
            json!({"id":format!("H{}",index+1),"statement":statement,"comparison_window":"Snapshot 10 compared with snapshot 9","gradient_signal":"db file sequential read peak impact 1.5","workload_profile":"2 seconds of DB time per wall second","test":"Compare read count, latency and access path with the matched scope."})).collect::<Vec<_>>());
        assert!(save(profile.clone()).is_err());
        save(scope.clone()).unwrap();
        let mut gap = brief(&id, &project, "performance_profile");
        gap["hypothesis_gap"] = json!("No metrics");
        assert!(save(gap).is_err());
        for section in ["load_profile_anomalies", "anomaly_clusters"] {
            assert!(save(profile.clone())
                .unwrap_err()
                .to_string()
                .contains(section));
            let observed = call(
                &runtime,
                "get_precomputed_analysis",
                json!({"analysis_id":id,"section":section}),
            )
            .unwrap();
            profile["evidence_refs"]
                .as_array_mut()
                .unwrap()
                .push(observed["evidence_id"].clone());
        }
        let mut partial = profile.clone();
        partial["evidence_refs"] = json!([gradient]);
        assert!(save(partial).is_err());
        let mut two = profile.clone();
        two["opening_hypotheses"].as_array_mut().unwrap().pop();
        save(two).unwrap();
        save(profile.clone()).unwrap();

        let mut work = brief(&id, &project, "io");
        work["evidence_refs"] = json!([load, measured]);
        work["hypothesis_checks"] = json!([
            {"id":"H1","reason":"Read demand compared with the snapshot.","evidence_refs":[load,measured]},
            {"id":"H2","reason":"Read latency compared with the snapshot.","evidence_refs":[measured]},
            {"id":"H3","reason":"No matched plan capture.","evidence_refs":[]}
        ]);
        save(work.clone()).unwrap();
        let mut synthesis = brief(&id, &project, "gradients_anomalies");
        synthesis["evidence_refs"] = json!([load, measured]);
        synthesis["hypothesis_outcomes"] = json!([
            {"id":"H1","status":"confirmed","reason":"Read counts increased over the comparison interval.","evidence_refs":[measured]},
            {"id":"H2","status":"rejected","reason":"Read latency stayed constant in the sample.","evidence_refs":[measured]},
            {"id":"H3","status":"unresolved","reason":"A matching plan capture is still missing.","evidence_refs":[]}
        ]);
        let mut incomplete = synthesis.clone();
        incomplete["hypothesis_outcomes"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(save(incomplete).is_err());
        let mut reused = synthesis.clone();
        reused["hypothesis_outcomes"][0]["evidence_refs"] = json!([load]);
        assert!(save(reused).is_err());
        let status = save(synthesis.clone()).unwrap();
        assert_eq!(
            status["hypotheses"][0]["hypothesis_outcomes"],
            synthesis["hypothesis_outcomes"]
        );
        let checkpoint = call(&runtime, "export_audit", json!({"analysis_id":id})).unwrap()
            ["checkpoint"]
            .clone();
        let restored = call(&runtime, "restore_audit", json!({"checkpoint":checkpoint})).unwrap();
        assert_eq!(restored["hypotheses"], status["hypotheses"]);
        work["hypothesis_checks"][0]["reason"] = json!(
            "Read wait counts doubled from 1,000 to 2,000 in these equal-duration snapshots."
        );
        assert_eq!(
            save(work.clone()).unwrap()["hypotheses"][0]["hypothesis_outcomes"],
            json!([])
        );
        save(synthesis.clone()).unwrap();
        profile["opening_hypotheses"][0]["test"] =
            json!("Compare physical reads per execution for matched plans.");
        let revised = save(profile.clone()).unwrap();
        assert_eq!(revised["hypotheses"][0]["hypothesis_checks"], json!([]));
        assert!(save(synthesis.clone()).is_err());
        save(work).unwrap();
        save(synthesis).unwrap();
        let mut revised_scope = scope;
        revised_scope["conclusion"] = json!("The symptom affects a different interval.");
        let moved = save(revised_scope).unwrap();
        assert_eq!(moved["hypotheses"][0]["opening_needs_scope_review"], true);
        assert_eq!(moved["hypotheses"][0]["hypothesis_outcomes"], json!([]));
    }

    #[test]
    fn tool_schemas_expose_restore_without_an_existing_session() {
        let runtime = super::super::tests::runtime();
        let tools = build_mcp_tools(&runtime);
        let restore = tools.iter().find(|t| t.name == "restore_audit").unwrap();
        assert!(!restore
            .input_schema
            .get("properties")
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("analysis_id"));
        for name in [
            "start_audit",
            "record_audit_brief",
            "authorize_report",
            "preview_report",
            "record_report_review",
        ] {
            assert!(tools.iter().any(|t| t.name == name));
        }
    }
}

#[cfg(test)]
#[path = "audit_review_tests.rs"]
mod review_tests;

#[cfg(test)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Node and permission to bind a loopback HTTP port"]
async fn companion_core_transport_integration() {
    let runtime = super::tests::runtime();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = format!("http://{address}/mcp");
    let cancellation = CancellationToken::new();
    let service: StreamableHttpService<JasminMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(JasminMcpServer::new(runtime.clone())),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default()
                .with_allowed_hosts([address.to_string()])
                .with_allowed_origins([format!("http://{address}")])
                .with_cancellation_token(cancellation.child_token()),
        );
    let router = axum::Router::new().nest_service("/mcp", service);
    let stop = cancellation.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .unwrap();
    });
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("node")
            .arg("plugins/jasmin-companion/test/core-integration.mjs")
            .env("JASMIN_AUDIT_TEST_ENDPOINT", endpoint)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    cancellation.cancel();
    server.await.unwrap();
    assert!(
        output.status.success(),
        "Companion/core integration failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
}

#[cfg(test)]
#[test]
fn checkpoint_numeric_comparison_handles_javascript_without_rounding_large_ids() {
    // Gradient values survive a real JSON transport; adjacent measurements still differ.
    let original = json!({"peak": 0.14504457915907235});
    let from_wire: Value = serde_json::from_str(r#"{"peak":0.14504457915907235}"#).unwrap();
    let changed: Value = serde_json::from_str(r#"{"peak":0.14504457915907237}"#).unwrap();
    assert!(same_json_measurements(&original, &from_wire));
    assert!(!same_json_measurements(&original, &changed));
    assert!(same_json_measurements(
        &json!({"seconds":3600.0,"value":1e20}),
        &json!({"seconds":3600,"value":100000000000000000000.0})
    ));
    assert!(!same_json_measurements(
        &json!(9007199254740993u64),
        &json!(9007199254740992u64)
    ));
    assert!(!same_json_measurements(
        &json!(9007199254740993u64),
        &json!(9007199254740992.0)
    ));
    assert!(!same_json_measurements(&Value::Null, &json!(0)));
    assert!(!same_json_measurements(
        &json!({"x":1}),
        &json!({"x":1,"y":0})
    ));
}
