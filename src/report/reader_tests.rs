use super::*;
use std::fs;

fn finding(id: &str, category: &str, priority: &str) -> ReportFinding {
    ReportFinding {
        finding_id: id.into(),
        category: category.into(),
        title: format!("Issue {id}"),
        severity: "high".into(),
        confidence: "high".into(),
        reader_brief: None,
        conclusion: "Observed cursor contention.".into(),
        mechanism: "A sampled holder is parsing while two sessions wait.".into(),
        temporal_pattern: "Instance 1, 08-Sep-2026 05:00.".into(),
        affected_workload: "Import service.".into(),
        evidence_limitations: "One sample cannot establish the initiating trigger.".into(),
        evidence_summary: "Two waiters on instance 1; 30 s sampled wait.".into(),
        details: "Full diagnostic context remains available.".into(),
        evidence_refs: vec![SEED_EVIDENCE_ID.into()],
        guidance_refs: vec!["G1".into()],
        guidance_quotes: vec![GuidanceQuotation {
            guidance_ref: "G1".into(),
            quote: "Inspect the holder before changing the shared pool.".into(),
        }],
        recommendations: vec![Recommendation {
            kind: ActionKind::EvidenceCapture,
            owner: "DBA".into(),
            priority: priority.into(),
            action_summary: None,
            steps: Vec::new(),
            decision_rule: None,
            action: "Capture the holder and waiter timeline.".into(),
            rationale: "Identify the initiating event.".into(),
            success_criterion: "Timestamped onset and recovery with the same session identities."
                .into(),
        }],
    }
}

#[test]
fn decision_queue_ranks_actions_and_keeps_evidence_and_limitations() {
    let mut state = AnalysisSession::new(json!({}), ReportConfig::default(), vec![]);
    state.config.issue_grouping = "legacy".into();
    for (id, category, priority) in [
        ("F-1", "sql", "high"),
        ("F-2", "wait_events", "immediate"),
        ("F-3", "limitations", "high"),
    ] {
        state
            .findings
            .insert(id.into(), finding(id, category, priority));
    }
    let markdown = render_markdown(&json!({}), &state, &ReportEntityLinks::default());
    let summary = markdown.split("\n## 2.").next().unwrap();
    assert!(summary.find("Issue F-2").unwrap() < summary.find("Issue F-1").unwrap());
    assert!(summary.contains("Decision boundary:"));
    assert!(!summary.contains("Diagnostic mechanism:"));
    assert!(!summary.contains("At-a-glance finding register"));
    assert_eq!(
        markdown
            .matches("> Inspect the holder before changing the shared pool.")
            .count(),
        1
    );
    let html = render_markdown_html_document(&markdown, "", "", HashMap::new());
    let parsed = scraper::Html::parse_document(&html);
    let sel = |s| scraper::Selector::parse(s).unwrap();
    assert_eq!(parsed.select(&sel("section.decision-card h3")).count(), 3);
    assert_eq!(parsed.select(&sel(".finding-evidence")).count(), 2);
    assert!(parsed
        .select(&sel(".finding-evidence[open]"))
        .next()
        .is_none());
    assert!(parsed.select(&sel("#finding-f-3")).next().is_some());
    assert!(parsed.select(&sel("#guidance-quote-1")).next().is_some());
    let actions = markdown.split("## 11.").nth(1).unwrap();
    assert!(
        actions.find("### IMMEDIATE priority").unwrap()
            < actions.find("### HIGH priority").unwrap()
    );
    // F-1 and F-3 share one complete action, including its acceptance criterion.
    assert_eq!(actions.matches("- **DBA — Capture the holder").count(), 2);
    assert!(actions.contains("[Issue F-3](#finding-f-3)"));
}

#[test]
fn editorial_feedback_does_not_mutate_or_truncate_findings() {
    let mut state = AnalysisSession::new(json!({}), ReportConfig::default(), vec![]);
    let mut long = finding("F-long", "sql", "high");
    long.title = "word ".repeat(13);
    long.evidence_summary = "measured ".repeat(181);
    state.findings.insert(long.finding_id.clone(), long);
    let before = serde_json::to_value(&state.findings).unwrap();
    let review = report_readability_review(&state);
    assert_eq!(review["blocking"], false);
    assert_eq!(review["warnings"].as_array().unwrap().len(), 2);
    assert_eq!(serde_json::to_value(&state.findings).unwrap(), before);
}

/// Replay an archived finalized report without a model call or a running server.
/// Customer fixtures stay outside the repository. Never overwrite a prior export.
#[test]
#[ignore = "requires JASMIN_REPORT_FIXTURE and JASMIN_REPORT_PREVIEW environment paths"]
fn replay_archived_report() {
    let fixture = PathBuf::from(std::env::var("JASMIN_REPORT_FIXTURE").expect("fixture directory"));
    let output =
        PathBuf::from(std::env::var("JASMIN_REPORT_PREVIEW").expect("new output .html path"));
    let base = output.parent().unwrap();
    let read = |name: &str| -> Value {
        serde_json::from_str(&fs::read_to_string(fixture.join(name)).unwrap()).unwrap()
    };
    let document = read("finalized-report.json");
    let mut state = AnalysisSession::new(
        json!({}),
        serde_json::from_value(document["config"].clone()).unwrap(),
        serde_json::from_value(document["project_ids"].clone()).unwrap(),
    );
    for value in document["findings"].as_array().unwrap() {
        let finding: ReportFinding = serde_json::from_value(value.clone()).unwrap();
        state.findings.insert(finding.finding_id.clone(), finding);
    }
    // Optional reviewed migration overlay; never infer groupings/action kinds
    // from SQL IDs or wording when replaying an old customer report.
    if let Ok(path) = std::env::var("JASMIN_REPORT_ISSUES") {
        let overlay: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let issues: Vec<ReportIssue> = serde_json::from_value(overlay["issues"].clone()).unwrap();
        let known = state
            .findings
            .keys()
            .map(|id| report_issues::finding_anchor(id))
            .collect();
        let membership = report_issues::validate_issues(&issues, &known).unwrap();
        assert_eq!(membership.len(), state.findings.len());
        for finding in state.findings.values_mut() {
            let kinds: Vec<ActionKind> =
                serde_json::from_value(overlay["action_kinds"][&finding.finding_id].clone())
                    .unwrap();
            assert_eq!(kinds.len(), finding.recommendations.len());
            for (action, kind) in finding.recommendations.iter_mut().zip(kinds) {
                assert_ne!(kind, ActionKind::Unclassified);
                action.kind = kind;
            }
        }
        for issue in issues {
            state.issue_finding_snapshots.insert(
                issue.issue_id.clone(),
                issue_finding_snapshot(&state, &issue),
            );
            state.issues.insert(issue.issue_id.clone(), issue);
        }
        state.config.issue_grouping = "explicit".into();
    }
    for (key, value) in document["mandatory_assessments"].as_object().unwrap() {
        state
            .assessments
            .insert(key.clone(), serde_json::from_value(value.clone()).unwrap());
    }
    for value in document["structured_tables"].as_array().unwrap() {
        let table: ReportTable = serde_json::from_value(value.clone()).unwrap();
        state.report_tables.insert(table.table_id.clone(), table);
    }
    for value in read("evidence-full.json").as_object().unwrap().values() {
        let Some(evidence_id) = value["evidence_id"].as_str() else {
            continue;
        };
        let record = EvidenceRecord {
            evidence_id: evidence_id.into(),
            tool_name: value["tool_name"].as_str().unwrap().into(),
            project_id: value["project_id"].as_str().map(str::to_string),
            arguments: value.get("arguments").cloned().unwrap_or(json!({})),
            result: value["result"].clone(),
        };
        state.evidence.insert(record.evidence_id.clone(), record);
    }
    for value in read("guidance.json")["result"]["matches"]
        .as_array()
        .unwrap()
    {
        state.guidance.insert(
            value["guidance_ref"].as_str().unwrap().into(),
            GuidanceRecord {
                title: value["title"]
                    .as_str()
                    .unwrap_or("Diagnostic guidance")
                    .into(),
                text: value["text"].as_str().unwrap().into(),
            },
        );
    }
    let mut links = ReportEntityLinks::default();
    for dataset in document["datasets"].as_array().unwrap() {
        let project_id = dataset["project_id"].as_str().unwrap();
        let root = base.join(dataset["source_reports"]["directory"].as_str().unwrap());
        for row in state.report_tables.values().flat_map(|table| &table.rows) {
            for (key, kind, prefix) in [("sql_id", "sql", "sqlid"), ("wait_event", "wait", "fg")] {
                if let Some(name) = row.cells.get(key) {
                    let path = if kind == "sql" {
                        root.join("sqlid").join(format!("sqlid_{name}.html"))
                    } else {
                        root.join(get_safe_filename(name.clone(), prefix.into()))
                    };
                    if path.is_file() {
                        links.targets.insert(
                            (project_id.into(), kind.into(), name.to_ascii_lowercase()),
                            path.to_string_lossy().into(),
                        );
                    }
                }
            }
        }
    }
    let mut markdown = render_markdown(&document, &state, &links);
    // Archived tool responses may omit late calls or their original arguments.
    // Preserve the finalized provenance verbatim; never invent those records.
    let archived = fs::read_to_string(fixture.join("finalized.md")).unwrap();
    let appendix = |text: &str| {
        let start = text.find("<details class=\"evidence-appendix\">").unwrap();
        let end = start + text[start..].find("</details>").unwrap() + "</details>".len();
        start..end
    };
    markdown.replace_range(appendix(&markdown), &archived[appendix(&archived)]);
    let html = render_markdown_html_document(&markdown, "", "", HashMap::new());
    validate_local_html_targets(&html, base).unwrap();
    let parsed = scraper::Html::parse_document(&html);
    let ids = parsed
        .select(&scraper::Selector::parse("[id]").unwrap())
        .filter_map(|node| node.value().attr("id"))
        .collect::<BTreeSet<_>>();
    for link in parsed.select(&scraper::Selector::parse("a[href^='#']").unwrap()) {
        let target = link.value().attr("href").unwrap().trim_start_matches('#');
        assert!(ids.contains(target), "Unresolved fragment: {target}");
    }
    for (path, contents) in [(output.with_extension("md"), markdown), (output, html)] {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }
}

#[test]
fn reader_brief_requires_own_evidence_and_never_truncates_authored_text() {
    let source = vec![SEED_EVIDENCE_ID.to_string()];
    let mut value = json!({
        "observed":"One measured capture contains two waiting sessions.",
        "explanation":"The sample identifies the next investigation target without proving the trigger.",
        "next_step":"Capture the holder and waiter timeline during the next occurrence.",
        "boundary":"One sample does not establish the complete wait duration.",
        "evidence_refs":[SEED_EVIDENCE_ID]
    });
    assert!(parse_reader_brief(Some(&value), &source).unwrap().is_some());
    value["evidence_refs"] = json!(["E-not-in-finding"]);
    assert_eq!(
        parse_reader_brief(Some(&value), &source).unwrap_err()["error_code"],
        "READER_BRIEF_EVIDENCE_MISMATCH"
    );
    value["evidence_refs"] = json!([SEED_EVIDENCE_ID]);
    value["observed"] = json!("x".repeat(4_001));
    assert_eq!(
        parse_reader_brief(Some(&value), &source).unwrap_err()["error_code"],
        "INVALID_READER_BRIEF"
    );
    assert_eq!(value["observed"].as_str().unwrap().len(), 4_001);
    assert!(parse_reader_brief(None, &source).unwrap().is_none());
}

#[test]
fn localized_compact_report_preserves_complete_original_analysis_and_contract() {
    let mut state = AnalysisSession::new(json!({}), ReportConfig::default(), vec![]);
    state.config.language = "pl-PL".into();
    state.config.detail_level = "compact".into();
    let mut record = finding("F-reader", "sql", "high");
    record.reader_brief = Some(ReaderBrief {
        source_audit_brief: None,
        observed: "Dwie sesje czekały w obserwowanym przedziale.".into(),
        explanation: "Ten pomiar wskazuje miejsce dalszego badania.".into(),
        next_step: "DBA powinien zebrać chronologię blokowania.".into(),
        boundary: "Nie znamy zdarzenia, które rozpoczęło oczekiwanie.".into(),
        evidence_refs: vec![SEED_EVIDENCE_ID.into()],
    });
    record.details = "The source uses the literal phrase Next action and SQL_column_1.".into();
    state
        .findings
        .insert(record.finding_id.clone(), record.clone());
    let markdown = render_markdown(&json!({}), &state, &ReportEntityLinks::default());
    validate_stable_markdown_report(&markdown).unwrap();
    assert!(markdown.contains("## 4. Analiza zapytań SQL"));
    assert!(markdown.contains("Następny krok"));
    let parsed = scraper::Html::parse_document(&render_markdown_html_document(
        &markdown,
        "",
        "",
        HashMap::new(),
    ));
    assert_eq!(parsed.root_element().value().attr("lang"), Some("pl"));
    assert!(parsed
        .select(&scraper::Selector::parse("h3.severity-high").unwrap())
        .next()
        .is_some());
    let evidence = parsed
        .select(&scraper::Selector::parse(".finding-evidence").unwrap())
        .next()
        .unwrap();
    let technical = evidence.text().collect::<Vec<_>>().join(" ");
    for expected in [
        &record.conclusion,
        &record.mechanism,
        &record.affected_workload,
        &record.temporal_pattern,
        &record.evidence_summary,
        &record.evidence_limitations,
        &record.details,
    ] {
        assert!(
            technical.contains(expected),
            "Missing original analysis: {expected}"
        );
    }
    let bad = markdown.replace("## 5. Segmenty i obiekty", "## 5. Missing required heading");
    assert_eq!(
        validate_stable_markdown_report(&bad).unwrap_err()["error_code"],
        "MISSING_REPORT_SECTIONS"
    );
}

#[test]
fn narrative_review_covers_issues_actions_and_plan_explanations_without_mutation() {
    let mut state = AnalysisSession::new(json!({}), ReportConfig::default(), vec![]);
    let mut record = finding("F-density", "sql", "high");
    let dense = "operation_1; operation_2; operation_3; operation_4; operation_5; verify later.";
    record.recommendations[0].action_summary = Some(dense.into());
    state.findings.insert(record.finding_id.clone(), record);
    state.issues.insert(
        "I-density".into(),
        ReportIssue {
            issue_id: "I-density".into(),
            title: "Investigate the sample".into(),
            decision_summary: dense.into(),
            decision_boundary: "The sample does not establish the initiating event.".into(),
            scope: "Instance 1, observed sample.".into(),
            grouping_rationale: "This finding describes one capture target.".into(),
            confidence: "medium".into(),
            canonical_finding_id: "F-density".into(),
            finding_ids: vec!["F-density".into()],
        },
    );
    state.report_tables.insert(
        "T-density".into(),
        ReportTable {
            table_id: "T-density".into(),
            kind: "execution_plans".into(),
            category: "sql".into(),
            title: "Plan sample".into(),
            rows: vec![ReportTableRow {
                cells: BTreeMap::from([("risk".into(), dense.into())]),
                evidence_refs: vec![SEED_EVIDENCE_ID.into()],
            }],
        },
    );
    let before =
        serde_json::to_value((&state.findings, &state.issues, &state.report_tables)).unwrap();
    let review = report_readability_review(&state);
    assert_eq!(review["blocking"], false);
    for kind in ["finding", "issue", "table"] {
        assert!(review["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["entity_kind"] == kind
                && warning["code"] == "DENSE_TECHNICAL_PROSE"));
    }
    assert_eq!(
        serde_json::to_value((&state.findings, &state.issues, &state.report_tables)).unwrap(),
        before
    );
}

#[test]
fn plan_review_keeps_all_textual_fields_when_no_change_is_recommended() {
    let state = AnalysisSession::new(json!({}), ReportConfig::default(), vec![]);
    let cells = EXECUTION_PLAN_TABLE_COLUMNS
        .iter()
        .map(|(key, _)| {
            (
                (*key).to_string(),
                match *key {
                    "recommendation_type" => "no_change".into(),
                    "plan_status" => "analyzed".into(),
                    _ => format!("original {key} value"),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let table = ReportTable {
        table_id: "T-plan".into(),
        kind: "execution_plans".into(),
        category: "sql".into(),
        title: "Plan analysis".into(),
        rows: vec![ReportTableRow {
            cells: cells.clone(),
            evidence_refs: vec![],
        }],
    };
    let html = render_execution_plan_reviews(
        &table,
        &state,
        &BTreeMap::new(),
        &ReportEntityLinks::default(),
    );
    for (field, value) in cells {
        assert!(html.contains(&value), "Plan field omitted: {field}");
    }
}

#[test]
fn sourced_reader_brief_projects_only_the_matching_retained_visual_revision() {
    let mut state =
        AnalysisSession::new(json!({}), ReportConfig::default(), vec!["project-1".into()]);
    let mut record = finding("F-visual", "sql", "high");
    record.reader_brief = Some(ReaderBrief {
        source_audit_brief: Some(AuditBriefReference {
            brief_id: "B-visual".into(),
            revision: 1,
        }),
        observed: "The synthetic timeline contains one known and one missing observation.".into(),
        explanation: "The diagram describes a hypothesis for the next capture.".into(),
        next_step: "DBA should capture the next occurrence.".into(),
        boundary: "The fixture does not establish a causal chain.".into(),
        evidence_refs: vec![SEED_EVIDENCE_ID.into()],
    });
    let visual = json!({"title":"Timeline and hypothesis","panels":[
        {"type":"line","title":"Fixture timeline","classification":"measured","source":SEED_EVIDENCE_ID,"unit":"seconds","x_label":"Fixture time","series":[{"name":"instance-1","points":[{"x":1,"label":"10:00","value":2},{"x":2,"label":"10:10","value":null}]}]},
        {"type":"flow","title":"Proposed mechanism","classification":"hypothesis","source":SEED_EVIDENCE_ID,"steps":[{"title":"Observe","detail":"Identify the next capture target."},{"title":"Test","detail":"Compare the retained measurements."}]}
    ]});
    state.audit = Some(audit_workflow::AuditState::default());
    state.audit.as_mut().unwrap().briefs.insert(
        "B-visual".into(),
        audit_workflow::AuditBrief {
            brief_id: "B-visual".into(),
            revision: 1,
            updated_at_revision: 1,
            project_id: "project-1".into(),
            chapter_id: "sql".into(),
            question: "What should the next capture inspect?".into(),
            explanation: "Fixture explanation.".into(),
            conclusion: "Capture the next occurrence.".into(),
            evidence_boundary: "Synthetic renderer fixture.".into(),
            next_step: "Collect the timeline.".into(),
            open_questions: vec![],
            evidence_refs: vec![SEED_EVIDENCE_ID.into()],
            visual: Some(visual),
        },
    );
    state.findings.insert(record.finding_id.clone(), record);
    let markdown = render_markdown(&json!({}), &state, &ReportEntityLinks::default());
    assert!(markdown.contains("class=\"audit-visual\""));
    assert!(markdown.contains("data-classification=\"hypothesis\""));
    assert!(markdown.contains("Missing observation (null)"));
    assert!(markdown.find("reader-brief").unwrap() < markdown.find("audit-visual").unwrap());
    assert!(markdown.find("audit-visual").unwrap() < markdown.find("finding-evidence").unwrap());
    state
        .audit
        .as_mut()
        .unwrap()
        .briefs
        .get_mut("B-visual")
        .unwrap()
        .revision = 2;
    let stale = render_markdown(&json!({}), &state, &ReportEntityLinks::default());
    assert!(!stale.contains("class=\"audit-visual\""));
    assert!(stale.contains("audit-visual-unavailable"));
}
