use super::super::super::{audit_workflow::AuditState, EvidenceRecord, ReportConfig};
use super::*;
use std::io::Read;

pub(in super::super) fn seed() -> (AnalysisSession, Map<String, Value>) {
    let projects = vec!["project-1".into(), "project-2".into()];
    let mut state = AnalysisSession::new(json!({}), ReportConfig::default(), projects);
    let times = [
        "2020-01-15 01:00:00",
        "2020-01-15 02:00:00",
        "2020-01-15 03:00:00",
    ];
    let peak =
        |index: usize, value| json!({"begin":times[index],"end":times[index+1],"active_cpu":value});
    let mut intervals = vec![];
    for i in 0..2 {
        let mut instances = vec![];
        for j in 0..2 {
            let observed = [[6, 1], [3, 5]][i][j];
            let monitor = [[0, 0], [0, 1]][i][j];
            let sql = [[0, 0], [2, 2]][i][j];
            instances.push(json!({"project_id":format!("project-{}",j+1),
                "begin":times[i],"end":times[i+1],"wall_seconds":3600,
                "db_cpu_seconds":observed*3600,"monitor_cpu_seconds":monitor*3600,"selected_sql_cpu_seconds":sql*3600,
                "monitor_top_rows":if monitor>0{1}else{0},"selected_top_rows":if sql>0{1}else{0},
                "observed_active_cpu":observed,"scenario_active_cpu":observed-monitor-sql/2
            }));
        }
        intervals.push(json!({"begin":times[i],"end":times[i+1],"observed_active_cpu":([7,8][i]),"scenario_active_cpu":([7,5][i]),
            "saved_cpu_seconds":([0,10800][i]),"instances":instances}));
    }
    let result = json!({"project_ids":["project-1","project-2"],"paired_intervals":2,"alignment_tolerance_seconds":60,
        "assumptions":{"monitor_module_prefix":"MonitorAgent","selected_sql_ids":["abc123"],"sql_reduction_pct":50,"monitor_reduction_pct":100},
        "observed_peak":peak(1,8),"scenario_peak":peak(0,7),"saved_cpu_hours":3,"monitor_cpu_hours":1,"selected_sql_cpu_hours":4,
        "instances":[{"project_id":"project-1","observed_peak":peak(0,6),"scenario_peak":peak(0,6),"saved_cpu_hours":1},
            {"project_id":"project-2","observed_peak":peak(1,5),"scenario_peak":peak(1,3),"saved_cpu_hours":2}],
        "intervals":intervals,"private_note":"private-canary"});
    state.evidence.insert(
        "E-0001".into(),
        EvidenceRecord {
            evidence_id: "E-0001".into(),
            tool_name: "get_rac_cpu_scenario".into(),
            project_id: None,
            arguments: json!({"private_key":"private-canary"}),
            result,
        },
    );
    let mut audit = AuditState::default();
    audit.briefs.insert(
        "B-0001".into(),
        AuditBrief {
            brief_id: "B-0001".into(),
            project_id: "project-1".into(),
            revision: 1,
            evidence_refs: vec!["E-0001".into()],
            question: "private-canary".into(),
            conclusion: "CPU varies across intervals.".into(),
            evidence_boundary: "A fresh measurement is required.".into(),
            next_step: "Compare matched intervals.".into(),
            ..Default::default()
        },
    );
    state.audit = Some(audit);
    let args=json!({"analysis_id":"active-analysis","brief_id":"B-0001","brief_revision":1,"title":"Cluster CPU",
        "owner":"DBA","success_criterion":"Confirm the measured reduction.","evidence_ref":"E-0001",
        "instance_labels":{"project-1":"North","project-2":"South"}}).as_object().unwrap().clone();
    (state, args)
}

#[test]
fn cpu_publication_preserves_paired_peaks_exact_hours_and_private_boundary() {
    let (mut state, mut args) = seed();
    let dir = tempfile::tempdir().unwrap();
    let call = |name: &str, args: &Map<String, Value>, state: &AnalysisSession| {
        super::super::run(name, args, state, dir.path(), "audit-cpu")
    };
    for (language, conditional, timeline) in [
        ("en", "Conditional cluster peak", "Cluster timeline"),
        (
            "pl",
            "Warunkowy szczyt klastra",
            "Przebieg obciążenia klastra",
        ),
    ] {
        state.config.language = language.into();
        let p = call("preview_cpu_report", &args, &state).unwrap();
        let m = &p["cpu_model"];
        assert_eq!(m["observed_peak"]["active_cpu"], 8.);
        assert_eq!(m["scenario_peak"]["active_cpu"], 7.);
        assert_ne!(m["observed_peak"]["begin"], m["scenario_peak"]["begin"]);
        assert_eq!(m["observed_cpu_hours"], 15.);
        assert_eq!(m["scenario_cpu_hours"], 12.);
        assert_eq!(m["instances"][0]["saved_cpu_hours"], 1.);
        assert_eq!(m["instances"][1]["saved_cpu_hours"], 2.);
        let mut asset=json!({"analysis_id":"active-analysis","preview_id":p["preview_id"],"asset":"report.zip"})
            .as_object().unwrap().clone();
        assert_eq!(
            call("read_cpu_report_asset", &asset, &state).unwrap_err()["error_code"],
            "CPU_ZIP_NOT_PUBLISHED"
        );
        let publish=json!({"analysis_id":"active-analysis","preview_id":p["preview_id"],"user_request":"private-canary"}).as_object().unwrap().clone();
        let issued = call("publish_cpu_report", &publish, &state).unwrap();
        assert!(issued["zip_path"]
            .as_str()
            .unwrap()
            .ends_with("raport-cpu.zip"));
        let bytes = fs::read(issued["zip_path"].as_str().unwrap()).unwrap();
        assert_eq!(
            call("read_cpu_report_asset", &asset, &state).unwrap()["sha256"],
            digest(&bytes)
        );
        let mut zip = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        assert_eq!(zip.len(), ASSETS.len());
        for name in ASSETS {
            let mut content = String::new();
            zip.by_name(name)
                .unwrap()
                .read_to_string(&mut content)
                .unwrap();
            for private in [
                "private-canary",
                "project-1",
                "B-0001",
                "abc123",
                "active-analysis",
                "E-0001",
            ] {
                assert!(!content.contains(private));
            }
            if name == "index.html" {
                assert!(content.contains(conditional));
                assert!(content.contains("MonitorAgent"));
            }
            if name == "evidence.html" {
                assert!(content.contains(timeline));
                assert!(content.contains(r#"<section id="node-2">"#));
                assert!(content.contains("10800"));
            }
            asset["asset"] = json!(name);
            let chunk = call("read_cpu_report_asset", &asset, &state).unwrap();
            assert_eq!(
                STANDARD
                    .decode(chunk["content_base64"].as_str().unwrap())
                    .unwrap(),
                content.as_bytes()
            );
        }
    }
    args.insert("include_sql_ids".into(), json!(true));
    let p = call("preview_cpu_report", &args, &state).unwrap();
    assert!(fs::read_to_string(p["index_path"].as_str().unwrap())
        .unwrap()
        .contains("abc123"));
    let publish=json!({"analysis_id":"active-analysis","preview_id":p["preview_id"],"user_request":"approve"}).as_object().unwrap().clone();
    state
        .audit
        .as_mut()
        .unwrap()
        .briefs
        .get_mut("B-0001")
        .unwrap()
        .revision = 2;
    assert_eq!(
        call("publish_cpu_report", &publish, &state).unwrap_err()["error_code"],
        "CPU_PREVIEW_STALE"
    );
}

#[test]
fn cpu_publication_rejects_unbound_evidence_and_missing_instance_labels() {
    let (mut state, mut args) = seed();
    let dir = tempfile::tempdir().unwrap();
    let publish = |args: &Map<String, Value>, state: &AnalysisSession| {
        super::super::run("preview_cpu_report", args, state, dir.path(), "audit-cpu")
    };
    args["instance_labels"]["project-2"] = json!("North");
    assert!(publish(&args, &state).is_err());
    args["instance_labels"] = json!({"project-1":"North"});
    assert!(publish(&args, &state).is_err());
    args["instance_labels"]["project-2"] = json!("South");
    state
        .audit
        .as_mut()
        .unwrap()
        .briefs
        .get_mut("B-0001")
        .unwrap()
        .evidence_refs
        .clear();
    assert!(publish(&args, &state).is_err());
}
