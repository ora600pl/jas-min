use super::*;
use std::fs;

fn finding(id: &str, category: &str, priority: &str) -> ReportFinding {
    ReportFinding {
        finding_id: id.into(),
        category: category.into(),
        title: format!("Issue {id}"),
        severity: "high".into(),
        confidence: "high".into(),
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
            owner: "DBA".into(),
            priority: priority.into(),
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
