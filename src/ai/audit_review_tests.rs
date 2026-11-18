//! Regression checks for audited narrative scope and portable checkpoint retention.
use super::*;

fn call(
    runtime: &AnalysisRuntime,
    name: &str,
    arguments: Value,
) -> std::result::Result<Value, Value> {
    runtime.call_tool(name, arguments.as_object().unwrap().clone())
}

fn brief(project: &str, chapter: &str, evidence_refs: Vec<String>) -> AuditBrief {
    AuditBrief {
        brief_id: format!("brief-{project}-{chapter}"),
        revision: 1,
        updated_at_revision: 1,
        project_id: project.into(),
        chapter_id: chapter.into(),
        question: "Which observation changes the next decision?".into(),
        explanation:
            "The selected measurement identifies a testable hypothesis, not a proven cause.".into(),
        conclusion: "A comparison with matching work is the next useful measurement.".into(),
        evidence_boundary: "The example does not establish the cause of unsampled executions."
            .into(),
        next_step: "Measure comparable executions before proposing an intervention.".into(),
        open_questions: vec!["Does the cost per completed task change?".into()],
        evidence_refs,
        visual: None,
        ..Default::default()
    }
}

fn example_finding() -> ReportFinding {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../test_support/fixtures/report_readability/cases.json"
    ))
    .unwrap();
    serde_json::from_value(fixture["findings"][0].clone()).unwrap()
}

fn source_state() -> (AnalysisSession, ReportFinding) {
    let mut state = AnalysisSession::new(
        json!({}),
        ReportConfig::default(),
        vec!["project-1".into(), "project-2".into()],
    );
    for (id, project) in [
        ("E-source", "project-1"),
        ("E-supplement", "project-1"),
        ("E-other", "project-2"),
    ] {
        state.evidence.insert(
            id.into(),
            EvidenceRecord {
                evidence_id: id.into(),
                tool_name: "get_database_load_summary".into(),
                project_id: Some(project.into()),
                arguments: json!({}),
                result: json!({"observed":true}),
            },
        );
    }
    let source = brief("project-1", "sql", vec!["E-source".into()]);
    let mut finding = example_finding();
    finding.evidence_refs = vec!["E-source".into()];
    let reader = finding.reader_brief.as_mut().unwrap();
    reader.evidence_refs = finding.evidence_refs.clone();
    reader.source_audit_brief = Some(AuditBriefReference {
        brief_id: source.brief_id.clone(),
        revision: source.revision,
    });
    let mut audit = AuditState::default();
    audit.briefs.insert(source.brief_id.clone(), source);
    state.audit = Some(audit);
    (state, finding)
}

#[test]
fn narrative_source_requires_linkage_only_for_guided_analytical_findings() {
    let (mut state, mut finding) = source_state();
    assert!(validate_narrative_source(&finding, &state).is_ok());
    finding.reader_brief.as_mut().unwrap().source_audit_brief = None;
    assert_eq!(
        validate_narrative_source(&finding, &state).unwrap_err()["error_code"],
        "AUDIT_NARRATIVE_SOURCE_REQUIRED"
    );
    finding.reader_brief = None;
    assert_eq!(
        validate_narrative_source(&finding, &state).unwrap_err()["error_code"],
        "AUDIT_NARRATIVE_SOURCE_REQUIRED"
    );
    finding.category = "limitations".into();
    finding.evidence_refs.clear();
    assert!(validate_narrative_source(&finding, &state).is_ok());
    finding.category = "sql".into();
    state.audit = None;
    assert!(validate_narrative_source(&finding, &state).is_ok());
}

#[test]
fn narrative_source_rejects_wrong_chapter_project_and_missing_source_evidence() {
    let (mut state, mut finding) = source_state();
    finding.evidence_refs.push("E-supplement".into());
    assert!(validate_narrative_source(&finding, &state).is_ok());

    finding.category = "io".into();
    assert_eq!(
        validate_narrative_source(&finding, &state).unwrap_err()["error_code"],
        "AUDIT_NARRATIVE_SCOPE_MISMATCH"
    );
    finding.category = "sql".into();
    finding.evidence_refs = vec!["E-supplement".into()];
    assert_eq!(
        validate_narrative_source(&finding, &state).unwrap_err()["error_code"],
        "AUDIT_NARRATIVE_EVIDENCE_MISSING"
    );

    // A shared seed does not make project-2 measurements evidence for project-1.
    let source = state
        .audit
        .as_mut()
        .unwrap()
        .briefs
        .get_mut("brief-project-1-sql")
        .unwrap();
    source.evidence_refs = vec![SEED_EVIDENCE_ID.into()];
    finding.evidence_refs = vec![SEED_EVIDENCE_ID.into(), "E-other".into()];
    assert_eq!(
        validate_narrative_source(&finding, &state).unwrap_err()["error_code"],
        "AUDIT_NARRATIVE_SCOPE_MISMATCH"
    );
}

#[test]
fn fresh_editorial_review_cannot_publish_a_stale_audit_explanation() {
    let (mut state, finding) = source_state();
    let finding_id = finding.finding_id.clone();
    state.findings.insert(finding_id.clone(), finding);
    {
        let audit = state.audit.as_mut().unwrap();
        audit.revision = 2;
        let source = audit.briefs.get_mut("brief-project-1-sql").unwrap();
        source.revision = 2;
        source.conclusion = "The new comparison contradicts the earlier explanation.".into();
        audit.authorization = Some(ReportAuthorization {
            revision: audit.revision,
            user_request: "Please prepare the report from the revised discussion.".into(),
        });
    }
    let review = EditorialReview {
        preview_id: preview_id(&state),
        checks: [
            "explanation_clear",
            "actions_executable",
            "uncertainty_preserved",
            "evidence_preserved",
            "language_consistent",
        ]
        .into_iter()
        .map(|name| (name.into(), true))
        .collect(),
        notes: "Current preview reviewed; the old source revision must still be rejected.".into(),
    };
    state.audit.as_mut().unwrap().review = Some(review);
    assert_eq!(
        finalization_gate(&state).unwrap_err()["error_code"],
        "STALE_AUDIT_NARRATIVE"
    );
    let source = state
        .findings
        .get_mut(&finding_id)
        .unwrap()
        .reader_brief
        .as_mut()
        .unwrap()
        .source_audit_brief
        .as_mut()
        .unwrap();
    source.revision = 2;
    assert_eq!(
        finalization_gate(&state).unwrap_err()["error_code"],
        "REPORT_EDITORIAL_REVIEW_REQUIRED"
    );
}

fn comparative_runtime() -> AnalysisRuntime {
    let projects = ["project-1", "project-2"]
        .into_iter()
        .map(|project| {
            AnalysisProject::new(
                project.into(),
                AWRSCollection {
                    db_instance_information: Default::default(),
                    initialization_parameters: HashMap::new(),
                    awrs: vec![crate::awr::AWR::default()],
                    sql_text: HashMap::new(),
                    nmon: None,
                },
                ReportForAI::default(),
                format!("missing-{project}"),
                0,
                HashMap::new(),
                format!("missing-{project}.html_reports"),
            )
        })
        .collect();
    let mut runtime = AnalysisRuntime::from_projects(projects).unwrap();
    runtime.guidance = Arc::new(GuidanceLibrary::default());
    runtime
}

fn save_chapters(runtime: &AnalysisRuntime, id: &str, project: &str) {
    for chapter in CHAPTERS {
        let mut arguments = serde_json::to_value(brief(project, chapter.0, vec![])).unwrap();
        arguments.as_object_mut().unwrap().remove("revision");
        arguments
            .as_object_mut()
            .unwrap()
            .remove("updated_at_revision");
        arguments["analysis_id"] = json!(id);
        if chapter.0 == "performance_profile" {
            arguments["hypothesis_gap"] = json!("This capture contains no fitted gradient rows.");
        }
        call(runtime, "record_audit_brief", arguments).unwrap();
    }
}

#[test]
fn comparative_audit_requires_discussion_or_explicit_gaps_for_each_project() {
    let runtime = comparative_runtime();
    let started = call(
        &runtime,
        "start_performance_analysis",
        json!({
            "project_ids":["project-1","project-2"]
        }),
    )
    .unwrap();
    let id = started["analysis_id"].as_str().unwrap();
    call(&runtime, "start_audit", json!({"analysis_id":id})).unwrap();
    save_chapters(&runtime, id, "project-1");
    let status = call(&runtime, "get_audit_status", json!({"analysis_id":id})).unwrap();
    for chapter in status["chapters"].as_array().unwrap() {
        assert_ne!(chapter["status"], "discussed");
        assert_eq!(
            chapter["project_coverage"],
            json!([
                {"project_id":"project-1","discussed":true},
                {"project_id":"project-2","discussed":false}
            ])
        );
    }
    let request = json!({"analysis_id":id,"user_request":"Please prepare the discussed report."});
    let error = call(&runtime, "authorize_report", request.clone()).unwrap_err();
    assert_eq!(error["error_code"], "AUDIT_DISCUSSION_INCOMPLETE");
    assert_eq!(
        error["pending_chapters"].as_array().unwrap().len(),
        CHAPTERS.len()
    );
    assert!(error["pending_chapters"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item.as_str().unwrap().starts_with("project-2:")));
    save_chapters(&runtime, id, "project-2");
    let status = call(&runtime, "authorize_report", request).unwrap();
    assert_eq!(status["report_authorized"], true);
    assert_eq!(status["completeness"]["ready_to_finalize"], false);
}

#[test]
fn checkpoint_preserves_truncated_guidance_from_a_shared_multi_section_budget() {
    let mut runtime = super::super::tests::runtime();
    let guidance = format!(
        "§1.1 Baseline work\n{}\n§1.2 Baseline timing\n{}\n",
        "Measure matched workloads. ".repeat(230),
        "Retain the exact observation scope. ".repeat(210)
    );
    runtime.guidance = Arc::new(GuidanceLibrary::test_from_text(&guidance));
    let started = call(&runtime, "start_performance_analysis", json!({})).unwrap();
    let id = started["analysis_id"].as_str().unwrap();
    call(&runtime, "start_audit", json!({"analysis_id":id})).unwrap();
    let original = call(
        &runtime,
        "get_diagnostic_guidance",
        json!({
            "analysis_id":id,"topic":"Baseline","max_sections":2
        }),
    )
    .unwrap();
    let matches = original["result"]["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[1]["truncated"], true);
    let truncated = matches[1]["text"].as_str().unwrap();
    let full = runtime.guidance.query(
        &json!({"topic":"§1.2","max_sections":1}),
        DEFAULT_GUIDANCE_LIMIT_CHARS,
    );
    assert!(full["matches"][0]["text"].as_str().unwrap().len() > truncated.len());

    let exported = call(&runtime, "export_audit", json!({"analysis_id":id})).unwrap();
    let checkpoint = exported["checkpoint"].clone();
    let restored = call(&runtime, "restore_audit", json!({"checkpoint":checkpoint})).unwrap();
    let restored_export = call(
        &runtime,
        "export_audit",
        json!({"analysis_id":restored["analysis_id"]}),
    )
    .unwrap();
    assert_eq!(
        restored_export["checkpoint"]["guidance"],
        checkpoint["guidance"]
    );
    assert_eq!(restored["report_authorized"], false);
    let mut changed = checkpoint;
    changed["guidance"]["GUIDE-§1.2"]["text"] = json!("Changed methodology prefix.");
    let sessions_before = runtime.sessions.len();
    assert_eq!(
        call(&runtime, "restore_audit", json!({"checkpoint":changed})).unwrap_err()["error_code"],
        "CHECKPOINT_GUIDANCE_CHANGED"
    );
    assert_eq!(runtime.sessions.len(), sessions_before);
}
