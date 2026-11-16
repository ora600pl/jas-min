//! Neutral acceptance cases for the production report renderer.
//! These cases intentionally do not satisfy the full audit coverage contract.
use super::*;
use std::fs;
use std::io::Write;

fn examples() -> Value {
    serde_json::from_str(include_str!(
        "../../test_support/fixtures/report_readability/cases.json"
    ))
    .unwrap()
}

fn example_state() -> (Value, AnalysisSession) {
    let fixture = examples();
    let config = ReportConfig {
        language: "PL".into(),
        audience: "technical".into(),
        detail_level: "deep".into(),
        include_evidence_appendix: true,
        ..ReportConfig::default()
    };
    let mut state = AnalysisSession::new(json!({}), config, vec!["project-1".into()]);
    for value in fixture["evidence"].as_array().unwrap() {
        let record = EvidenceRecord {
            evidence_id: value["evidence_id"].as_str().unwrap().into(),
            tool_name: value["tool_name"].as_str().unwrap().into(),
            project_id: value["project_id"].as_str().map(str::to_string),
            arguments: value["arguments"].clone(),
            result: value["result"].clone(),
        };
        state.evidence.insert(record.evidence_id.clone(), record);
    }
    for value in fixture["findings"].as_array().unwrap() {
        let finding: ReportFinding = serde_json::from_value(value.clone()).unwrap();
        let brief = finding.reader_brief.as_ref().unwrap();
        let issue = ReportIssue {
            issue_id: finding.finding_id.replacen("F-", "I-", 1),
            title: finding.title.clone(),
            decision_summary: brief.observed.clone(),
            decision_boundary: brief.boundary.clone(),
            scope: finding.temporal_pattern.clone(),
            grouping_rationale: "Jedno ustalenie opisuje jeden odrębny cel pomiaru w project-1."
                .into(),
            confidence: finding.confidence.clone(),
            canonical_finding_id: finding.finding_id.clone(),
            finding_ids: vec![finding.finding_id.clone()],
        };
        state.findings.insert(finding.finding_id.clone(), finding);
        state.issues.insert(issue.issue_id.clone(), issue);
    }
    let document = json!({
        "analysis_id": "A-readability-preview",
        "preview_notice": "Podgląd dwóch anonimowych przykładów redakcji. To materiał testowy, a nie kompletny raport z audytu.",
        "revision": 1,
        "datasets": [{"project_id":"project-1", "label":"instance-1"}]
    });
    (document, state)
}

fn selector(value: &str) -> scraper::Selector {
    scraper::Selector::parse(value).unwrap()
}

fn text_of(node: scraper::ElementRef<'_>) -> String {
    node.text().collect::<Vec<_>>().join(" ")
}

fn article<'a>(document: &'a scraper::Html, finding_id: &str) -> scraper::ElementRef<'a> {
    let anchor = selector(&format!("#finding-{}", finding_id.to_ascii_lowercase()));
    document
        .select(&selector("article.report-finding"))
        .find(|node| node.select(&anchor).next().is_some())
        .expect("finding is present in the production HTML")
}

fn render_examples() -> (String, String) {
    let (document, state) = example_state();
    let markdown = render_markdown(&document, &state, &ReportEntityLinks::default());
    let html = render_markdown_html_document(&markdown, "", "", HashMap::new());
    (markdown, html)
}

#[test]
fn report_readability_examples_keep_explanations_before_complete_technical_details() {
    let (document, state) = example_state();
    let evidence_before = serde_json::to_value(&state.evidence).unwrap();
    let findings_before = serde_json::to_value(&state.findings).unwrap();
    let markdown = render_markdown(&document, &state, &ReportEntityLinks::default());
    let html = render_markdown_html_document(&markdown, "", "", HashMap::new());
    let parsed = scraper::Html::parse_document(&html);

    for id in ["F-remote-1", "F-cpu-1"] {
        let card = article(&parsed, id);
        let brief = card.select(&selector(".reader-brief")).next().unwrap();
        let evidence = card.select(&selector(".finding-evidence")).next().unwrap();
        assert!(evidence.value().attr("open").is_none());
        assert!(brief.select(&selector("p")).count() >= 4);
        let authored = state.findings[id].reader_brief.as_ref().unwrap();
        let visible = text_of(brief);
        for expected in [
            &authored.observed,
            &authored.explanation,
            &authored.next_step,
            &authored.boundary,
        ] {
            assert!(
                visible.contains(expected),
                "Missing reader explanation: {id}"
            );
        }
        let serialized = card.html();
        assert!(
            serialized.find("reader-brief").unwrap() < serialized.find("finding-evidence").unwrap()
        );
        let technical = text_of(evidence);
        assert!(technical.contains(&state.findings[id].mechanism));
        assert!(technical.contains(&state.findings[id].evidence_summary));
    }

    let remote = article(&parsed, "F-remote-1");
    let visible = text_of(remote.select(&selector(".reader-brief")).next().unwrap());
    assert!(!visible.contains("FAST DUAL"));
    assert!(!visible.contains("136"));
    assert!(!visible.contains("operacja 70"));
    assert!(visible.contains("Jeśli pomiar to potwierdzi"));
    assert!(visible.contains("Nie wiemy jeszcze"));
    let technical = text_of(
        remote
            .select(&selector(".finding-evidence"))
            .next()
            .unwrap(),
    );
    for preserved in [
        "136 operacji",
        "102 FAST DUAL",
        "8 REMOTE",
        "Operacja 70",
        "Operacja 69",
        "w 61",
        "Operacja 67",
        "nested loops 63",
        "49/55",
        "rezerwacje 52",
        "Ranking <=3",
        "57/58",
        "Fragment 71",
        "ROW_NUMBER",
        "LISTAGG DISTINCT",
        "LEFT JOIN",
        "duplikatów",
        "ucięty",
        "display_refresh_counter",
    ] {
        assert!(
            technical.contains(preserved),
            "Lost technical fact: {preserved}"
        );
    }

    assert_eq!(
        serde_json::to_value(&state.evidence).unwrap(),
        evidence_before
    );
    assert_eq!(
        serde_json::to_value(&state.findings).unwrap(),
        findings_before
    );
}

#[test]
fn report_readability_examples_split_cpu_next_step_from_conditional_followups() {
    let (_, state) = example_state();
    let finding = &state.findings["F-cpu-1"];
    let next_step = &finding.reader_brief.as_ref().unwrap().next_step;
    assert!(next_step.contains("wolne i referencyjne wykonanie"));
    assert!(next_step.contains("podobnej liczbie obsłużonych zadań"));
    assert!(!next_step.contains("HWM"));
    assert!(!next_step.contains("continued row"));
    assert!(!next_step.contains("chained"));
    assert_eq!(finding.recommendations.len(), 3);
    assert!(finding
        .recommendations
        .iter()
        .all(|action| action.kind == ActionKind::EvidenceCapture));
    assert!(finding.recommendations[1]
        .action
        .starts_with("Dopiero po potwierdzeniu"));
    assert!(finding.recommendations[2]
        .action
        .starts_with("Oddzielnie przypisać"));
    assert!(finding.recommendations[0]
        .decision_rule
        .as_ref()
        .unwrap()
        .contains("Wzrost samej liczby zadań"));

    let (_, html) = render_examples();
    let parsed = scraper::Html::parse_document(&html);
    let card = article(&parsed, "F-cpu-1");
    let visible = text_of(card.select(&selector(".reader-brief")).next().unwrap());
    assert!(visible.contains("73–78%"));
    assert!(visible.contains("0,3%"));
    assert!(visible.contains("Bez niego nie potrafimy wskazać zmiany"));
    assert!(
        text_of(card.select(&selector(".finding-evidence")).next().unwrap())
            .contains("Nie dowiedziono fragmentacji")
    );
    let all_text = parsed.root_element().text().collect::<Vec<_>>().join(" ");
    for action in &finding.recommendations {
        assert!(all_text.contains(&action.action));
        assert!(all_text.contains(&action.success_criterion));
    }
}

#[test]
fn report_readability_examples_preserve_exact_measurements_and_unknowns() {
    let (_, state) = example_state();
    assert_eq!(
        state.evidence["E-plan-1"].result,
        json!({
            "plan_operations":136, "fast_dual_operations":102, "remote_operations":8,
            "local_join_operation":61, "filtered_orders_operation":69,
            "unfiltered_materials_operation":70, "nested_loop_operation":63,
            "nested_remote_operation":67, "related_remote_operations":[49,55],
            "reservations_operation":52, "ranking_operations":[57,58], "ranking_limit":3,
            "truncated_remote_operation":71, "remote_text_complete":false,
            "starts":null, "actual_rows":null, "actual_bytes":null
        })
    );
    assert_eq!(
        state.evidence["E-cpu-1"].result,
        json!({
            "db_cpu_pct_of_db_time_min":73, "db_cpu_pct_of_db_time_max":78,
            "host_idle_pct":0.3, "comparable_business_work":null,
            "sql_to_segment_mapping":null, "fragmentation_confirmed":false,
            "continued_row_sql_attribution":null
        })
    );
    for (id, evidence) in [("F-remote-1", "E-plan-1"), ("F-cpu-1", "E-cpu-1")] {
        let finding = &state.findings[id];
        assert_eq!(finding.evidence_refs, vec![evidence]);
        assert_eq!(
            finding.reader_brief.as_ref().unwrap().evidence_refs,
            vec![evidence]
        );
    }
}

#[test]
#[ignore = "requires JASMIN_READABILITY_PREVIEW pointing to a new absolute HTML path"]
fn preview_report_readability_examples() {
    let output =
        PathBuf::from(std::env::var("JASMIN_READABILITY_PREVIEW").expect("new output HTML path"));
    assert!(output.is_absolute(), "preview path must be absolute");
    assert_eq!(
        output.extension().and_then(|value| value.to_str()),
        Some("html")
    );
    assert!(!output.exists(), "HTML preview already exists");
    assert!(
        !output.with_extension("md").exists(),
        "Markdown preview already exists"
    );
    let (markdown, html) = render_examples();
    for (path, contents) in [(output.with_extension("md"), markdown), (output, html)] {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }
}
