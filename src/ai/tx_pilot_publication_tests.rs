use super::super::{EvidenceRecord, ReportConfig, audit_workflow::AuditState};
use super::*;
use std::io::Read;

fn session() -> AnalysisSession {
    let mut state = AnalysisSession::new(
        json!({"internal":"private-canary"}),
        ReportConfig::default(),
        vec!["project-1".into(), "project-2".into()],
    );
    state.audit = Some(AuditState::default());
    for index in 1..=2 {
        let id = format!("E-{index:04}");
        let project = format!("project-{index}");
        state.evidence.insert(id.clone(),EvidenceRecord{
            evidence_id:id.clone(),tool_name:"get_wait_event_timeline".into(),project_id:Some(project.clone()),
            arguments:json!({"private":"private-canary"}),result:json!({"event_name":EVENT,"kind":"foreground","timeline":[
                {"event":EVENT,"begin_snap_time":"2020-01-01 11:00","end_snap_time":"2020-01-01 12:00","snap_id":11,"total_wait_time_s":3*index},
                {"event":EVENT,"begin_snap_time":"2020-01-01 12:00","end_snap_time":"2020-01-01 13:00","snap_id":12,"total_wait_time_s":4*index}
            ]})});
        let b = AuditBrief {
            brief_id: format!("source-{index}"),
            project_id: project,
            revision: 2,
            chapter_id: "wait_events".into(),
            question: "private-canary".into(),
            explanation: "private-canary".into(),
            visual: Some(json!({"note":"private-canary"})),
            open_questions: vec!["private-canary".into()],
            evidence_refs: vec![id],
            conclusion: "Workers waited for a transaction.".into(),
            evidence_boundary: "A sample identifies the waiter.".into(),
            next_step: "Compare two concurrent requests.".into(),
            ..Default::default()
        };
        state
            .audit
            .as_mut()
            .unwrap()
            .briefs
            .insert(b.brief_id.clone(), b);
    }
    state
}

fn selection() -> Map<String, Value> {
    json!({"analysis_id":"active-analysis","brief_id":"source-1","brief_revision":2,"title":"Transaction wait <test>",
        "owner":"DBA","success_criterion":"Identify the lock holder for both cases.",
        "evidence":[{"evidence_ref":"E-0001","instance_label":"instance-1","first_snap_id":11,"last_snap_id":12},
        {"evidence_ref":"E-0002","instance_label":"instance-2","source_brief_id":"source-2","source_brief_revision":2,"first_snap_id":11,"last_snap_id":12}]
    }).as_object().unwrap().clone()
}

fn publish_args(preview: &Value) -> Map<String, Value> {
    json!({"analysis_id":"active-analysis","preview_id":preview["preview_id"],"user_request":"private-canary"}).as_object().unwrap().clone()
}

#[test]
fn tx_client_zip_is_the_reviewed_public_projection() {
    let s = session();
    let dir = tempfile::tempdir().unwrap();
    let preview = run("preview_tx_report", &selection(), &s, dir.path(), "audit-1").unwrap();
    assert_eq!(preview["measurements"][0]["total_wait_s"], 7.);
    assert_eq!(preview["measurements"][1]["total_wait_s"], 14.);
    let issued = run(
        "publish_tx_report",
        &publish_args(&preview),
        &s,
        dir.path(),
        "audit-1",
    )
    .unwrap();
    let zip_path = issued["zip_path"].as_str().unwrap();
    let raw = fs::read(zip_path).unwrap();
    assert_eq!(json!(digest(&raw)), issued["zip_sha256"]);
    let mut archive = zip::ZipArchive::new(Cursor::new(raw)).unwrap();
    assert_eq!(archive.len(), ASSETS.len());
    let preview_dir = Path::new(preview["index_path"].as_str().unwrap())
        .parent()
        .unwrap();
    for name in ASSETS {
        let mut body = String::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert_eq!(body, fs::read_to_string(preview_dir.join(name)).unwrap());
        for private in [
            "private-canary",
            "active-analysis",
            "project-1",
            "E-0001",
            "source-2",
        ] {
            assert!(!body.contains(private), "{name}: {private}");
        }
        if name == "index.html" {
            assert!(body.contains("&lt;test&gt;"));
            assert!(body.contains("evidence.html#window-2"));
        }
        if name == "evidence.html" {
            assert_eq!(body.matches("<svg ").count(), 2);
            assert!(body.contains("same vertical scale"));
            assert!(body.contains("index.html#action"));
        }
    }
    let repeated = run(
        "publish_tx_report",
        &publish_args(&preview),
        &s,
        dir.path(),
        "audit-1",
    )
    .unwrap();
    assert_eq!(issued["zip_sha256"], repeated["zip_sha256"]);
}

#[test]
fn tx_client_zip_detects_stale_companion_brief_and_changed_preview() {
    let mut s = session();
    let dir = tempfile::tempdir().unwrap();
    let preview = run("preview_tx_report", &selection(), &s, dir.path(), "audit-1").unwrap();
    let mut args = publish_args(&preview);
    s.audit
        .as_mut()
        .unwrap()
        .briefs
        .get_mut("source-2")
        .unwrap()
        .revision += 1;
    assert_eq!(
        run("publish_tx_report", &args, &s, dir.path(), "audit-1").unwrap_err()["error_code"],
        "TX_PREVIEW_STALE"
    );
    s.audit
        .as_mut()
        .unwrap()
        .briefs
        .get_mut("source-2")
        .unwrap()
        .revision -= 1;
    args.insert("analysis_id".into(), json!("new-analysis"));
    assert_eq!(
        run("publish_tx_report", &args, &s, dir.path(), "audit-1").unwrap_err()["error_code"],
        "TX_PREVIEW_STALE"
    );
    fs::write(preview["index_path"].as_str().unwrap(), "unexpected change").unwrap();
    assert_eq!(
        run(
            "publish_tx_report",
            &publish_args(&preview),
            &s,
            dir.path(),
            "audit-1"
        )
        .unwrap_err()["error_code"],
        "TX_PREVIEW_CHANGED"
    );
    assert!(!dir.path().join("wydania").exists());
}

#[test]
fn tx_client_projection_verifies_project_event_and_window() {
    let mut s = session();
    let dir = tempfile::tempdir().unwrap();
    let mut args = selection();
    args["evidence"][1]["source_brief_id"] = json!("source-1");
    assert!(run("preview_tx_report", &args, &s, dir.path(), "audit-1").is_err());
    args = selection();
    args["evidence"][0]["first_snap_id"] = json!(10);
    assert!(run("preview_tx_report", &args, &s, dir.path(), "audit-1").is_err());
    let record = s.evidence.get_mut("E-0001").unwrap();
    record.result["kind"] = json!("background");
    assert!(run("preview_tx_report", &selection(), &s, dir.path(), "audit-1").is_err());
    assert!(public_text("MCP review and /tmp/example", 200).is_err());
}

#[test]
fn tx_client_displays_distinct_windows_and_preserves_private_receipt() {
    let mut state = session();
    let rows = state.evidence.get_mut("E-0002").unwrap().result["timeline"]
        .as_array_mut()
        .unwrap();
    for row in rows {
        for field in ["begin_snap_time", "end_snap_time"] {
            row[field] = json!(
                row[field]
                    .as_str()
                    .unwrap()
                    .replace("2020-01-01", "2020-01-02")
            );
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let preview = run(
        "preview_tx_report",
        &selection(),
        &state,
        dir.path(),
        "audit-1",
    )
    .unwrap();
    let folder = Path::new(preview["index_path"].as_str().unwrap())
        .parent()
        .unwrap();
    let html = fs::read_to_string(folder.join("evidence.html")).unwrap();
    assert!(html.contains("different windows"));
    assert!(html.contains("own scale"));
    assert!(!html.contains("same vertical scale"));
    let issued = run(
        "publish_tx_report",
        &publish_args(&preview),
        &state,
        dir.path(),
        "audit-1",
    )
    .unwrap();
    let receipt: Value =
        serde_json::from_slice(&fs::read(issued["receipt_path"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(receipt["preview_id"], issued["preview_id"]);
    assert_eq!(receipt["zip_sha256"], issued["zip_sha256"]);
    assert_eq!(receipt["user_request"], "private-canary");
}

#[cfg(unix)]
#[test]
fn tx_client_keeps_editions_inside_the_workspace() {
    let state = session();
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let preview = run(
        "preview_tx_report",
        &selection(),
        &state,
        dir.path(),
        "audit-1",
    )
    .unwrap();
    std::os::unix::fs::symlink(other.path(), dir.path().join("wydania")).unwrap();
    assert!(
        run(
            "publish_tx_report",
            &publish_args(&preview),
            &state,
            dir.path(),
            "audit-1"
        )
        .is_err()
    );
    assert_eq!(other.path().read_dir().unwrap().count(), 0);
}

#[test]
fn tx_asset_reader_is_scoped_to_preview_and_the_published_edition() {
    let mut state = session();
    let dir = tempfile::tempdir().unwrap();
    let preview = run(
        "preview_tx_report",
        &selection(),
        &state,
        dir.path(),
        "audit-1",
    )
    .unwrap();
    let mut read=json!({"analysis_id":"active-analysis","preview_id":preview["preview_id"],"asset":"index.html"})
        .as_object().unwrap().clone();
    let read_asset = |state: &AnalysisSession, params: &Map<String, Value>| {
        run("read_tx_report_asset", params, state, dir.path(), "audit-1")
    };
    let data = read_asset(&state, &read).unwrap();
    assert_eq!(data["filename"], "index.html");
    assert_eq!(data["next_offset"], Value::Null);
    let decoded = STANDARD
        .decode(data["content_base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        decoded,
        fs::read(preview["index_path"].as_str().unwrap()).unwrap()
    );
    assert_eq!(data["sha256"], digest(&decoded));
    for asset in [
        "recipe.json",
        "../recipe.json",
        "receipts",
        "..%2frecipe.json",
        "/tmp",
        "report.css?x",
    ] {
        read["asset"] = json!(asset);
        assert!(read_asset(&state, &read).is_err());
    }
    read["asset"] = json!("report.zip");
    assert_eq!(
        read_asset(&state, &read).unwrap_err()["error_code"],
        "TX_ZIP_NOT_PUBLISHED"
    );
    let issued = run(
        "publish_tx_report",
        &publish_args(&preview),
        &state,
        dir.path(),
        "audit-1",
    )
    .unwrap();
    assert_eq!(
        read_asset(&state, &read).unwrap()["sha256"],
        issued["zip_sha256"]
    );
    read["analysis_id"] = json!("unrelated");
    assert_eq!(
        read_asset(&state, &read).unwrap_err()["error_code"],
        "TX_PREVIEW_STALE"
    );
    read["analysis_id"] = json!("active-analysis");
    let zip_path = Path::new(issued["zip_path"].as_str().unwrap());
    let good = fs::read(zip_path).unwrap();
    fs::write(zip_path, b"changed").unwrap();
    assert_eq!(
        read_asset(&state, &read).unwrap_err()["error_code"],
        "TX_EDITION_CHANGED"
    );
    fs::write(zip_path, good).unwrap();
    state
        .audit
        .as_mut()
        .unwrap()
        .briefs
        .get_mut("source-1")
        .unwrap()
        .revision += 1;
    let mut new_selection = selection();
    new_selection["brief_revision"] = json!(3);
    let next = run(
        "preview_tx_report",
        &new_selection,
        &state,
        dir.path(),
        "audit-1",
    )
    .unwrap();
    assert_eq!(preview["content_sha256"], next["content_sha256"]);
    read["preview_id"] = next["preview_id"].clone();
    assert_eq!(
        read_asset(&state, &read).unwrap_err()["error_code"],
        "TX_ZIP_NOT_PUBLISHED"
    );
}

#[test]
fn tx_asset_chunks_report_full_file_identity_and_exact_byte_offsets() {
    let bytes = vec![37_u8; ASSET_CHUNK_BYTES + 3];
    let first = asset_chunk("report.zip", "application/zip", &bytes, None).unwrap();
    let tail = asset_chunk(
        "report.zip",
        "application/zip",
        &bytes,
        Some(&first["next_offset"]),
    )
    .unwrap();
    assert_eq!(first["bytes"], bytes.len());
    assert_eq!(first["next_offset"], ASSET_CHUNK_BYTES);
    assert_eq!(tail["sha256"], first["sha256"]);
    assert_eq!(tail["next_offset"], Value::Null);
    let mut recovered = STANDARD
        .decode(first["content_base64"].as_str().unwrap())
        .unwrap();
    recovered.extend(
        STANDARD
            .decode(tail["content_base64"].as_str().unwrap())
            .unwrap(),
    );
    assert_eq!(recovered, bytes);
    for offset in [json!(-1), json!("12"), json!(bytes.len() + 1)] {
        assert!(asset_chunk("report.zip", "application/zip", &bytes, Some(&offset)).is_err());
    }
}

fn combined_selection() -> (AnalysisSession, Map<String, Value>) {
    let mut tx = session();
    let mut tx_input = selection();
    let (mut cpu, mut cpu_input) = super::cpu::tests::seed();
    let mut record = cpu.evidence.remove("E-0001").unwrap();
    record.evidence_id = "E-0003".into();
    tx.evidence.insert(record.evidence_id.clone(), record);
    let mut cpu_brief = cpu.audit.as_mut().unwrap().briefs.remove("B-0001").unwrap();
    cpu_brief.evidence_refs = vec!["E-0003".into()];
    tx.audit
        .as_mut()
        .unwrap()
        .briefs
        .insert(cpu_brief.brief_id.clone(), cpu_brief);
    tx_input.remove("analysis_id");
    cpu_input.remove("analysis_id");
    cpu_input.insert("evidence_ref".into(), json!("E-0003"));
    cpu_input.insert(
        "instance_labels".into(),
        json!({"project-1":"instance-1","project-2":"instance-2"}),
    );
    let mut saved = json!(super::super::tx_pilot::SavedAudit::default());
    saved["pilot_kind"] = json!("combined");
    saved["combined"] = json!({"active_topic":"tx","brief_topics":{"source-1":"tx","source-2":"tx","B-0001":"cpu"},
        "other_focus":{"current_chapter_id":"sql"}});
    tx.tx_pilot = Some(serde_json::from_value(saved).unwrap());
    let args = json!({"analysis_id":"active-analysis","title":"Database performance","tx":tx_input,"cpu":cpu_input});
    (tx, args.as_object().unwrap().clone())
}

#[test]
fn combined_publication_pins_both_topics_and_synthesizes_only_client_measurements() {
    let (mut state, args) = combined_selection();
    let dir = tempfile::tempdir().unwrap();
    let call = |tool: &str, args: &Map<String, Value>, s: &AnalysisSession| {
        run(tool, args, s, dir.path(), "audit-combined")
    };
    let preview = call("preview_combined_report", &args, &state).unwrap();
    // This fixture has measurements but no archived native charts.
    let expected = [
        "index.html",
        "wykresy/ora-600.png",
        "wykresy/report-nav.js",
        "wykresy/report.css",
    ];
    assert_eq!(preview["files"], json!(expected));
    let mut asset = json!({"analysis_id":args["analysis_id"],"preview_id":preview["preview_id"],"asset":"report.zip"}).as_object().unwrap().clone();
    assert_eq!(
        call("read_combined_report_asset", &asset, &state).unwrap_err()["error_code"],
        "COMBINED_ZIP_NOT_PUBLISHED"
    );
    let publish = json!({"analysis_id":args["analysis_id"],"preview_id":preview["preview_id"],"user_request":"private-canary"}).as_object().unwrap().clone();
    let edition = call("publish_combined_report", &publish, &state).unwrap();
    let bytes = fs::read(edition["zip_path"].as_str().unwrap()).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert_eq!(archive.len(), expected.len());
    let public = Path::new(edition["index_path"].as_str().unwrap())
        .parent()
        .unwrap();
    for name in expected {
        let mut bytes = Vec::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, fs::read(public.join(name)).unwrap());
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("private-canary")
                && !text.contains("E-000")
                && !text.contains("active-analysis")
        );
        asset.insert("asset".into(), json!(name));
        let chunk = call("read_combined_report_asset", &asset, &state).unwrap();
        assert_eq!(chunk["sha256"], digest(&bytes));
        if name == "index.html" {
            assert!(text.contains("Workers waited") && text.contains("CPU varies"));
            assert!(text.contains("instance-2") && text.contains("conditional scenario"));
            for anchor in ["tx", "cpu", "charts", "tx-evidence", "cpu-evidence"] {
                assert!(
                    text.contains(&format!("id=\"{anchor}\"")),
                    "missing: {anchor}"
                );
            }
        }
    }
    for name in [
        "../recipe.json",
        "assets.json",
        "cpu-chart-99.html",
        "wykresy/private.html",
        "wykresy/../recipe.json",
    ] {
        asset.insert("asset".into(), json!(name));
        assert!(call("read_combined_report_asset", &asset, &state).is_err());
    }
    let mut mixed = args.clone();
    mixed.get_mut("tx").unwrap()["evidence"][0]["instance_label"] = json!("Different");
    assert!(call("preview_combined_report", &mixed, &state).is_err());
    for id in ["source-1", "source-2", "B-0001"] {
        state
            .audit
            .as_mut()
            .unwrap()
            .briefs
            .get_mut(id)
            .unwrap()
            .revision += 1;
        assert_eq!(
            call("publish_combined_report", &publish, &state).unwrap_err()["error_code"],
            "COMBINED_PREVIEW_STALE"
        );
        state
            .audit
            .as_mut()
            .unwrap()
            .briefs
            .get_mut(id)
            .unwrap()
            .revision -= 1;
    }
    let mut crossed = args;
    crossed.get_mut("tx").unwrap()["brief_id"] = json!("B-0001");
    assert_eq!(
        call("preview_combined_report", &crossed, &state).unwrap_err()["error_code"],
        "COMBINED_PUBLICATION_INVALID"
    );
}
