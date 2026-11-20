use super::*;
use crate::awr::{SQLElapsedTime, WaitEvents};

const A: &str = "instance-a";
const B: &str = "instance-b";
const Q1: &str = "000000000000a";
const Q2: &str = "000000000000b";
const CHAT: &str = "000000000000c";
const V1: &str = "000000000000d";
const V2: &str = "000000000000f";
const WA: &str = "sample wait alpha";
const WB: &str = "sample wait beta";

fn runtime(dir: &Path) -> AnalysisRuntime {
    let projects = [(A, Q1, WA), (B, Q2, WB)]
        .into_iter()
        .map(|(id, sql, event)| {
            let mut project = super::super::tests::comparison_project(id, &[1.0], &[]);
            let snap = &mut project.collection.awrs[0];
            snap.sql_elapsed_time.push(SQLElapsedTime {
                sql_id: sql.into(),
                ..Default::default()
            });
            snap.foreground_wait_events.push(WaitEvents {
                event: event.into(),
                ..Default::default()
            });
            project
        })
        .collect();
    let mut runtime = AnalysisRuntime::from_projects(projects).unwrap();
    runtime.audit_workspace = dir.to_owned();
    runtime
}

fn call(r: &AnalysisRuntime, tool: &str, args: Value) -> Value {
    r.call_tool(tool, args.as_object().unwrap().clone())
        .unwrap()
}

fn start(r: &AnalysisRuntime) -> String {
    let analysis = call(
        r,
        "start_performance_analysis",
        json!({"project_ids":[A,B]}),
    );
    analysis["analysis_id"].as_str().unwrap().into()
}

fn inventory(r: &AnalysisRuntime, id: &str) -> Inventory {
    let session = r.session(id).unwrap();
    let state = session.lock().unwrap();
    r.discussed_entities(&state)
}

#[test]
fn displayed_panel_and_expanded_full_snapshot_remain_scoped_to_their_instance() {
    let work = tempfile::tempdir().unwrap();
    let r = runtime(work.path());
    let id = start(&r);
    let snapshot = call(
        &r,
        "get_snapshot_details",
        json!({"analysis_id":id,"project_id":B,"snap_id":1}),
    );
    let reference = snapshot["evidence_id"].as_str().unwrap();
    assert!(
        inventory(&r, &id).is_empty(),
        "caching an entire snapshot must not register every row"
    );
    call(
        &r,
        "record_discussed_entities",
        json!({"analysis_id":id,"project_id":A,"panel_text":format!("Shown {Q1} and {WA}.")}),
    );
    let before = inventory(&r, &id);
    assert_eq!(
        before[&A.to_string()],
        (BTreeSet::from([Q1.into()]), BTreeSet::from([WA.into()]))
    );
    assert!(!before.contains_key(B));
    let error = r
        .call_tool(
            "record_discussed_entities",
            json!({"analysis_id":id,"project_id":A,"evidence_ref":reference})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap_err();
    assert_eq!(error["error_code"], "INVALID_DISCUSSION_SOURCE");
    call(
        &r,
        "record_discussed_entities",
        json!({"analysis_id":id,"project_id":B,"evidence_ref":reference}),
    );
    let after = inventory(&r, &id);
    assert_eq!(
        after[B],
        (BTreeSet::from([Q2.into()]), BTreeSet::from([WB.into()]))
    );
    assert_eq!(after[A], before[A]);
}

#[test]
fn comparative_evidence_registers_only_the_expanded_project_side() {
    let work = tempfile::tempdir().unwrap();
    let r = runtime(work.path());
    let id = start(&r);
    let key = "E-comparison";
    let session = r.session(&id).unwrap();
    session.lock().unwrap().evidence.insert(key.into(), EvidenceRecord {
        evidence_id:key.into(), tool_name:"compare_project_metric".into(),project_id:None,
        arguments:json!({"baseline_project_id":A,"candidate_project_id":B}),
        result:json!({"baseline":{"sql_id":Q1,"event_name":WA},"candidate":{"sql_id":Q2,"event_name":WB}}),
    });
    for (project, sql, event) in [(B, Q2, WB), (A, Q1, WA)] {
        call(
            &r,
            "record_discussed_entities",
            json!({"analysis_id":id,"project_id":project,"evidence_ref":key}),
        );
        assert_eq!(
            inventory(&r, &id)[project],
            (BTreeSet::from([sql.into()]), BTreeSet::from([event.into()]))
        );
    }
}

#[test]
fn chat_and_superseded_briefs_survive_resume_and_legacy_checkpoint_import() {
    let work = tempfile::tempdir().unwrap();
    let r = runtime(work.path());
    let id = start(&r);
    let started = call(
        &r,
        "start_audit",
        json!({"analysis_id":id,"focus_project_id":A}),
    );
    let mention = json!({"analysis_id":id,"project_id":B,"sql_ids":[CHAT.to_ascii_uppercase()],"wait_events":[WB.to_ascii_uppercase()]});
    let first = call(&r, "record_discussed_entities", mention.clone());
    let duplicate = call(&r, "record_discussed_entities", mention);
    assert_eq!(first["added"], 2);
    assert_eq!(duplicate["added"], 0);
    assert_eq!(duplicate["revision"], first["revision"]);
    let mut brief = json!({"analysis_id":id,"project_id":A,"brief_id":"session-sql","chapter_id":"sql",
        "question":"Which statement was captured?", "explanation":format!("Prior mention {V1}."), "conclusion":"Compare elapsed time.",
        "evidence_boundary":"One interval was sampled.","next_step":"Review executions.","evidence_refs":[SEED_EVIDENCE_ID]});
    call(&r, "record_audit_brief", brief.clone());
    brief["explanation"] = json!(format!("Revised mention {V2}."));
    call(&r, "record_audit_brief", brief);
    let before = inventory(&r, &id);
    assert_eq!(before[A].0, BTreeSet::from([V1.into(), V2.into()]));
    assert_eq!(
        before[B],
        (BTreeSet::from([CHAT.into()]), BTreeSet::from([WB.into()]))
    );
    let mut old = call(&r, "export_audit", json!({"analysis_id":id}))["checkpoint"].clone();
    old.as_object_mut().unwrap().remove("discussed_entities");
    drop(r);
    let fresh = runtime(work.path());
    let resumed = call(
        &fresh,
        "resume_saved_audit",
        json!({"audit_id":started["audit_id"]}),
    );
    assert_eq!(
        inventory(&fresh, resumed["analysis_id"].as_str().unwrap()),
        before
    );
    let restored = call(
        &fresh,
        "restore_audit",
        json!({"checkpoint":old,"focus_project_id":A}),
    );
    let derived = inventory(&fresh, restored["analysis_id"].as_str().unwrap());
    assert_eq!(
        derived[A], before[A],
        "old checkpoints recover identifiers from both versions of their briefs"
    );
    assert!(
        !derived.contains_key(B),
        "a legacy checkpoint does not contain the explicit chat register"
    );
}

#[test]
fn archived_visual_uses_explicit_wait_and_sql_columns_and_per_row_projects() {
    let work = tempfile::tempdir().unwrap();
    let r = runtime(work.path());
    let id = start(&r);
    call(
        &r,
        "start_audit",
        json!({"analysis_id":id,"focus_project_id":A}),
    );
    let mut brief = json!({"analysis_id":id,"project_id":A,"brief_id":"retained-board","chapter_id":"sql",
        "question":"Which sessions were sampled?", "explanation":"The board records both instances.",
        "conclusion":"Compare the waits.","evidence_boundary":"One interval was sampled.","next_step":"Inspect the source intervals.",
        "evidence_refs":[SEED_EVIDENCE_ID],
        "visual":{"title":"Two instances","panels":[{"type":"table","classification":"measured","source":SEED_EVIDENCE_ID,
        "title":"Retained rows","columns":["SQL_ID","Zdarzenie oczekiwania"],"row_project_ids":[B,A],
        "rows":[["1111111111111","archived event B"],["2222222222222","archived event A"]]}]}
    });
    call(&r, "record_audit_brief", brief.clone());
    brief.as_object_mut().unwrap().remove("visual");
    brief["explanation"] = json!("The board was removed in this revision.");
    call(&r, "record_audit_brief", brief);
    let entities = inventory(&r, &id);
    assert_eq!(
        entities[A],
        (
            BTreeSet::from(["2222222222222".into()]),
            BTreeSet::from(["archived event A".into()])
        )
    );
    assert_eq!(
        entities[B],
        (
            BTreeSet::from(["1111111111111".into()]),
            BTreeSet::from(["archived event B".into()])
        )
    );
}

#[test]
fn existing_comparison_board_without_row_metadata_obeys_its_instance_column() {
    let work = tempfile::tempdir().unwrap();
    let runtime = runtime(work.path());
    let projects = vec![A.to_owned(), B.to_owned()];
    let mut c = Collector {
        result: BTreeMap::new(),
        projects: &runtime.projects,
        selected: &projects,
    };
    c.read(
        A,
        &json!({"columns":["Instance","Wait event"],"rows":[[B,"archive-only wait"]]}),
        true,
    );
    assert!(c.result[B].1.contains("archive-only wait"));
    assert!(!c
        .result
        .get(A)
        .is_some_and(|e| e.1.contains("archive-only wait")));
}

#[test]
fn comparative_brief_preserves_explicit_node_in_each_sentence() {
    let work = tempfile::tempdir().unwrap();
    let runtime = runtime(work.path());
    let projects = vec![A.to_owned(), B.to_owned()];
    let mut c = Collector {
        result: BTreeMap::new(),
        projects: &runtime.projects,
        selected: &projects,
    };
    c.read(
        A,
        &json!(format!(
            "On {B}, {Q2} contributed to {WB}. On {A}, {Q1} contributed to {WA}."
        )),
        true,
    );
    assert_eq!(c.result[A].0, BTreeSet::from([Q1.to_owned()]));
    assert_eq!(c.result[B].1, BTreeSet::from([WB.to_owned()]));
}

#[test]
fn class_annotations_share_one_card_across_current_and_archived_sources() {
    let work = tempfile::tempdir().unwrap();
    let runtime = runtime(work.path());
    let id = start(&runtime);
    call(&runtime, "start_audit", json!({"analysis_id":id}));
    let mut old = call(&runtime, "export_audit", json!({"analysis_id":id}))["checkpoint"].clone();
    old["discussed_entities"] =
        json!({A:[[],[format!("{WA} · tło"),format!("{WA} · foreground")]]});
    let restored = call(
        &runtime,
        "restore_audit",
        json!({"checkpoint":old, "focus_project_id":A}),
    );
    let id = restored["analysis_id"].as_str().unwrap();
    let first = inventory(&runtime, id);
    assert_eq!(first[A].1, BTreeSet::from([WA.to_owned()]));
    let duplicate = call(
        &runtime,
        "record_discussed_entities",
        json!({"analysis_id":id,"project_id":A,"wait_events":[WA,format!("{WA} · BACKGROUND")],
               "panel_text":format!("{WA} · foreground")}),
    );
    assert_eq!(duplicate["added"], 0);
    assert_eq!(duplicate["revision"], restored["snapshot"]["revision"]);

    let added = call(
        &runtime,
        "record_discussed_entities",
        json!({"analysis_id":id,"project_id":A,"wait_events":["archive only · TŁO","archive only","archive only · foreground"]}),
    );
    assert_eq!(added["added"], 1);
    let checkpoint = call(&runtime, "export_audit", json!({"analysis_id":id}));
    assert_eq!(
        checkpoint["checkpoint"]["discussed_entities"][A][1],
        json!(["archive only", WA])
    );

    let mut c = Collector {
        result: BTreeMap::new(),
        projects: &runtime.projects,
        selected: &[A.into(), B.into()],
    };
    c.read(
        A,
        &json!({"columns":["Instance","Wait event"],"rows":[[B,format!("{WB} · foreground")],
        [B,format!("{WB} · tło")],[B,"unknown wait · foreground"],[B,"ON CPU · tło"]]}),
        true,
    );
    assert_eq!(
        c.result[B].1,
        BTreeSet::from([WB.to_owned(), "unknown wait".into()])
    );
}

#[test]
fn genuine_catalog_names_and_unrelated_suffixes_survive_class_annotation_removal() {
    let catalog = Catalog {
        sql: HashSet::new(),
        waits: BTreeMap::from([
            (
                "transfer · foreground".into(),
                "Transfer · foreground".into(),
            ),
            ("transfer".into(), "Transfer".into()),
        ]),
        wait_pattern: None,
    };
    for (input, expected) in [
        ("transfer · foreground", "Transfer · foreground"),
        ("Transfer · foreground · tło", "Transfer · foreground"),
        ("transfer · background", "Transfer"),
        ("transfer (foreground)", "transfer (foreground)"),
        ("foreground checkpoint", "foreground checkpoint"),
        ("transfer · custom tag", "transfer · custom tag"),
    ] {
        assert_eq!(catalog.wait_name(input), expected);
    }
}
