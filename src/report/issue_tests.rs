use super::*;

fn issue(id: &str, members: &[&str]) -> ReportIssue {
    ReportIssue {
        issue_id: id.into(), title: "Investigate cursor contention".into(),
        decision_summary: "Two sampled waiters identify the next capture target.".into(),
        decision_boundary: "A later sample does not establish the historical cause.".into(),
        scope: "Instance 1 in July, plus a separate September sample.".into(),
        grouping_rationale: "Wait attribution and cursor diagnostics support the same import investigation, with distinct temporal limits.".into(),
        confidence: "medium".into(), canonical_finding_id: members[0].into(),
        finding_ids: members.iter().map(|id| (*id).into()).collect(),
    }
}

fn finding_arguments(analysis_id: &str, kind: Option<&str>) -> Map<String, Value> {
    let mut value = json!({"analysis_id":analysis_id, "category":"sql", "title":"Cursor diagnostics",
        "severity":"high", "confidence":"medium", "conclusion":"A sampled parsing holder merits targeted capture.",
        "mechanism":"An exclusive cursor pin can serialize another session's cursor access.",
        "temporal_pattern":"The fixture separates a July window from a September sample.",
        "affected_workload":"The import request and its measured SQL workload on instance 1.",
        "evidence_limitations":"A single sample cannot establish the complete wait duration or initiating event.",
        "evidence_summary":"The initial seed is the only factual fixture evidence available.",
        "evidence_refs":[SEED_EVIDENCE_ID],
        "recommendations":[{"owner":"DBA","priority":"immediate","action":"Capture the next holder and waiter chain.",
            "rationale":"The chronology is required to distinguish invalidation from expensive parsing.",
            "success_criterion":"Timestamped onset and recovery share the same cursor and session identities."}]});
    if let Some(kind) = kind {
        value["recommendations"][0]["kind"] = json!(kind);
    }
    value.as_object().unwrap().clone()
}

#[test]
fn grouping_is_atomic_scoped_and_becomes_stale_after_member_update() {
    let runtime = super::tests::runtime();
    let start = runtime
        .call_tool("start_performance_analysis", Map::new())
        .unwrap();
    let analysis_id = start["analysis_id"].as_str().unwrap();
    let args = finding_arguments(analysis_id, Some("evidence_capture"));
    let id = runtime.call_tool("record_finding", args.clone()).unwrap()["finding_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut issue_args = serde_json::to_value(issue("I-1", &[&id]))
        .unwrap()
        .as_object()
        .unwrap()
        .clone();
    issue_args.insert("analysis_id".into(), json!(analysis_id));
    runtime
        .call_tool("record_issue", issue_args.clone())
        .unwrap();
    let get_status = || {
        runtime
            .call_tool(
                "get_report_status",
                Map::from_iter([("analysis_id".into(), json!(analysis_id))]),
            )
            .unwrap()
    };
    assert_eq!(get_status()["missing_issue_assignments"], json!([]));
    assert_eq!(get_status()["stale_issues"], json!([]));

    let mut overlap = issue_args.clone();
    overlap.insert("issue_id".into(), json!("I-2"));
    assert_eq!(
        runtime.call_tool("record_issue", overlap).unwrap_err()["error_code"],
        "INVALID_ISSUE_REFERENCES"
    );
    let mut unknown = issue_args.clone();
    unknown.insert("finding_ids".into(), json!(["F-missing"]));
    assert!(runtime.call_tool("record_issue", unknown).is_err());
    assert_eq!(get_status()["issues"], 1);

    let mut update = args;
    update.insert("finding_id".into(), json!(id));
    update.insert("title".into(), json!("Updated scope after another capture"));
    runtime.call_tool("record_finding", update).unwrap();
    assert_eq!(get_status()["stale_issues"], json!(["I-1"]));
    assert_eq!(get_status()["ready_to_finalize"], false);
    runtime.call_tool("record_issue", issue_args).unwrap();
    assert_eq!(get_status()["stale_issues"], json!([]));
    runtime
        .call_tool(
            "delete_issue",
            Map::from_iter([
                ("analysis_id".into(), json!(analysis_id)),
                ("issue_id".into(), json!("I-1")),
            ]),
        )
        .unwrap();
    assert_eq!(get_status()["findings"], 1);
    assert_eq!(get_status()["missing_issue_assignments"], json!([id]));
}

#[test]
fn explicit_mode_requires_action_classification_and_links_are_session_local() {
    let runtime = super::tests::runtime();
    let start = runtime
        .call_tool("start_performance_analysis", Map::new())
        .unwrap();
    let analysis_id = start["analysis_id"].as_str().unwrap();
    runtime
        .call_tool("record_finding", finding_arguments(analysis_id, None))
        .unwrap();
    let args = Map::from_iter([("analysis_id".into(), json!(analysis_id))]);
    let status = runtime
        .call_tool("get_report_status", args.clone())
        .unwrap();
    assert_eq!(
        status["unclassified_actions"],
        json!(["F-0001:recommendation:1"])
    );
    assert_eq!(
        runtime.call_tool("finalize_report", args).unwrap_err()["error_code"],
        "REPORT_INCOMPLETE"
    );
    let other = runtime
        .call_tool("start_performance_analysis", Map::new())
        .unwrap();
    let mut fields = serde_json::to_value(issue("I-1", &["F-0001"]))
        .unwrap()
        .as_object()
        .unwrap()
        .clone();
    fields.insert("analysis_id".into(), other["analysis_id"].clone());
    assert!(runtime.call_tool("record_issue", fields).is_err());
}

#[test]
fn one_issue_combines_perspectives_without_losing_findings_or_action_types() {
    let runtime = super::tests::runtime();
    let start = runtime
        .call_tool("start_performance_analysis", Map::new())
        .unwrap();
    let analysis_id = start["analysis_id"].as_str().unwrap();
    let mut ids = Vec::new();
    for kind in ["evidence_capture", "mitigation", "durable_fix"] {
        let result = runtime
            .call_tool("record_finding", finding_arguments(analysis_id, Some(kind)))
            .unwrap();
        ids.push(result["finding_id"].as_str().unwrap().to_string());
    }
    let mut fields = serde_json::to_value(issue(
        "I-1",
        &ids.iter().map(String::as_str).collect::<Vec<_>>(),
    ))
    .unwrap()
    .as_object()
    .unwrap()
    .clone();
    fields.insert("analysis_id".into(), json!(analysis_id));
    runtime.call_tool("record_issue", fields).unwrap();
    let session = runtime.session(analysis_id).unwrap();
    let state = session.lock().unwrap();
    let markdown = render_markdown(&json!({}), &state, &ReportEntityLinks::default());
    assert_eq!(markdown.matches("class=\"decision-card\"").count(), 1);
    assert_eq!(
        markdown
            .matches("### Cursor diagnostics [high / medium]")
            .count(),
        3
    );
    for kind in ["Evidence capture", "Mitigation", "Durable fix"] {
        assert!(markdown.contains(kind));
    }
    for id in ids {
        assert!(markdown.contains(&format!("id=\"finding-{}\"", id.to_ascii_lowercase())));
    }
    assert!(markdown.contains("issue-detail-i-1"));
}
