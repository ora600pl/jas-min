use super::*;
use std::fs;

fn collect(value: &Value, analysis_id: &str, state: &mut AnalysisSession) {
    match value {
        Value::Object(object) => {
            if value["analysis_id"].as_str() == Some(analysis_id) {
                if let (Some(id), Some(tool), Some(result)) = (
                    value["evidence_id"].as_str(),
                    value["tool_name"].as_str(),
                    object.get("result"),
                ) {
                    state.evidence.insert(
                        id.into(),
                        EvidenceRecord {
                            evidence_id: id.into(),
                            tool_name: tool.into(),
                            project_id: value["project_id"].as_str().map(str::to_string),
                            arguments: object.get("arguments").cloned().unwrap_or(json!({})),
                            result: result.clone(),
                        },
                    );
                }
            }
            if let (Some(reference), Some(text)) =
                (value["guidance_ref"].as_str(), value["text"].as_str())
            {
                state.guidance.insert(
                    reference.into(),
                    GuidanceRecord {
                        title: value["title"]
                            .as_str()
                            .unwrap_or("Diagnostic guidance")
                            .into(),
                        text: text.into(),
                    },
                );
            }
            for value in object.values() {
                collect(value, analysis_id, state);
            }
        }
        Value::Array(array) => {
            for value in array {
                collect(value, analysis_id, state);
            }
        }
        _ => {}
    }
}

#[test]
#[ignore = "requires JASMIN_SIGNAL_FIXTURE, JASMIN_SIGNAL_ORIGINAL and a new JASMIN_SIGNAL_PREVIEW path"]
fn replay_signal_atlas_from_archived_audit() {
    let fixture = PathBuf::from(std::env::var("JASMIN_SIGNAL_FIXTURE").unwrap());
    let original = PathBuf::from(std::env::var("JASMIN_SIGNAL_ORIGINAL").unwrap());
    let output = PathBuf::from(std::env::var("JASMIN_SIGNAL_PREVIEW").unwrap());
    let base = output.parent().unwrap();
    let document: Value =
        serde_json::from_str(&fs::read_to_string(fixture.join("report.json")).unwrap()).unwrap();
    let mut state = AnalysisSession::new(
        json!({}),
        serde_json::from_value(document["config"].clone()).unwrap(),
        serde_json::from_value(document["project_ids"].clone()).unwrap(),
    );
    for finding in document["findings"].as_array().unwrap() {
        let f: ReportFinding = serde_json::from_value(finding.clone()).unwrap();
        state.findings.insert(f.finding_id.clone(), f);
    }
    for issue in document["issues"].as_array().unwrap() {
        let i: ReportIssue = serde_json::from_value(issue.clone()).unwrap();
        state.issues.insert(i.issue_id.clone(), i);
    }
    for table in document["structured_tables"].as_array().unwrap() {
        let t: ReportTable = serde_json::from_value(table.clone()).unwrap();
        state.report_tables.insert(t.table_id.clone(), t);
    }
    for (name, assessment) in document["mandatory_assessments"].as_object().unwrap() {
        state.assessments.insert(
            name.clone(),
            serde_json::from_value(assessment.clone()).unwrap(),
        );
    }
    for file in [
        "evidence_core.json",
        "evidence_artifacts.json",
        "evidence_sql.json",
        "evidence_metrics.json",
        "evidence_extra.json",
        "evidence_final.json",
    ] {
        let value: Value =
            serde_json::from_str(&fs::read_to_string(fixture.join(file)).unwrap()).unwrap();
        collect(
            &value,
            document["analysis_id"].as_str().unwrap(),
            &mut state,
        );
    }
    let mut links = ReportEntityLinks::default();
    for dataset in document["datasets"].as_array().unwrap() {
        let project = dataset["project_id"].as_str().unwrap();
        let root = base.join(dataset["source_reports"]["directory"].as_str().unwrap());
        for row in state.report_tables.values().flat_map(|t| &t.rows) {
            for (key, kind, prefix) in [
                ("sql_id", "sql", "sqlid"),
                ("wait_event", "wait", "fg"),
                ("contributor", "sql", "sqlid"),
                ("contributor", "wait", "fg"),
            ] {
                if let Some(name) = row.cells.get(key) {
                    let path = if kind == "sql" {
                        root.join("sqlid").join(format!("sqlid_{name}.html"))
                    } else {
                        root.join(get_safe_filename(name.clone(), prefix.into()))
                    };
                    if path.is_file() {
                        links.targets.insert(
                            (project.into(), kind.into(), name.to_ascii_lowercase()),
                            path.to_string_lossy().into(),
                        );
                    }
                }
            }
        }
    }
    let atlas =
        build_signal_atlas(&document, &state, &links).expect("The source-backed atlas must render");
    crate::report_signals::validate(&atlas).unwrap();
    assert_eq!(atlas.panels.len(), 14);
    assert_eq!(
        atlas.panels.iter().map(|p| p.points.len()).sum::<usize>(),
        228
    );
    assert_eq!(atlas.briefs.len(), 2);
    assert_eq!(atlas.moments.len(), 4);
    let cursor = atlas
        .panels
        .iter()
        .find(|p| p.project_label.contains("CEBSOFPR1") && p.family == "Foreground waits")
        .unwrap()
        .points
        .iter()
        .find(|p| p.name == "cursor: pin S wait on X")
        .unwrap();
    assert_eq!(cursor.peak, Some(71.41438138101672));
    assert_eq!(cursor.selected, [Some(true); 4]);
    let before = serde_json::to_value(&state.report_tables).unwrap();
    let mut markdown = render_markdown(&document, &state, &links);
    assert_eq!(before, serde_json::to_value(&state.report_tables).unwrap());
    // Replay can lack original tool-call arguments. Keep the delivered provenance
    // and methodology verbatim instead of reconstructing them from partial logs.
    let old = fs::read_to_string(original).unwrap();
    markdown.replace_range(
        markdown.find("## Appendix A.").unwrap()..,
        &old[old.find("## Appendix A.").unwrap()..],
    );
    let html =
        crate::tools::try_render_markdown_html_document(&markdown, "", "", HashMap::new()).unwrap();
    validate_local_html_targets(&html, base).unwrap();
    let parsed = scraper::Html::parse_document(&html);
    let selector = |s| scraper::Selector::parse(s).unwrap();
    let ids = parsed
        .select(&selector("[id]"))
        .filter_map(|node| node.value().attr("id"))
        .collect::<Vec<_>>();
    assert_eq!(
        ids.len(),
        ids.iter().collect::<BTreeSet<_>>().len(),
        "Duplicate ID"
    );
    for node in parsed.select(&selector("a[href^='#']")) {
        let id = &node.value().attr("href").unwrap()[1..];
        assert!(ids.contains(&id), "Broken fragment: {id}");
    }
    for (path, contents) in [
        (output.with_extension("md"), markdown),
        (
            output.with_extension("signals.json"),
            serde_json::to_string_pretty(&atlas).unwrap(),
        ),
        (output, html),
    ] {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }
}
