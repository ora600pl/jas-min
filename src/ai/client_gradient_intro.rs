//! Full-period model signals for the report introduction. Incident hypotheses remain in the audit.
use super::*;
use crate::report_signals::{SignalPanel, SignalPoint};
use serde::Deserialize;

#[path = "client_gradient_bubbles.rs"]
mod bubbles;
pub(super) use bubbles::render;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Signal {
    family: String,
    target: String,
    model: String,
    pub(super) name: String,
    active: Option<f64>,
    peak: Option<f64>,
    unit: String,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Opening {
    pub(super) instance: String,
    window: String,
    #[serde(default)]
    intervals: usize,
    #[serde(default)]
    panels: Vec<SignalPanel>,
    #[serde(default, alias = "signals", skip_serializing_if = "Vec::is_empty")]
    fallback: Vec<Signal>,
}

pub(super) fn basis(audit: Option<&audit_workflow::AuditState>, project: &str) -> Value {
    let Some(audit) = audit else {
        return Value::Null;
    };
    let status = audit_workflow::hypothesis_status(audit, project);
    // Preserve the opening rationale in the fingerprint of existing audit editions.
    let public = |v: &Value| {
        v.as_str()
            .and_then(|s| tx_pilot_publication::public_text(s, 4000).ok())
    };
    let rows=status["opening_hypotheses"].as_array().into_iter().flatten().filter_map(|h| {
        Some(json!({"statement":public(&h["statement"])?,"gradient_signal":public(&h["gradient_signal"]),
            "workload_profile":public(&h["workload_profile"]),"comparison_window":public(&h["comparison_window"])}))
    }).collect::<Vec<_>>();
    json!({"rows":rows,"gap":public(&status["hypothesis_gap"])})
}

fn magnitude(n: f64) -> Option<f64> {
    (n.is_finite() && n >= 0.0).then_some(n)
}

fn model_leaders(family: &str, target: &str, section: &DbTimeGradientSection) -> Vec<Signal> {
    let mut result = vec![];
    let unit = match section.settings.input_db_time_unit.as_str() {
        "db_time_per_second" => "AAS",
        "db_cpu_per_second" => "s/s",
        other => other,
    };
    for (key, model, rows) in [
        ("ridge", "Ridge", &section.ridge_top),
        ("huber", "Huber", &section.huber_top),
        ("elastic_net", "Elastic Net", &section.elastic_net_top),
        ("quantile95", "Quantile 95", &section.quantile95_top),
    ] {
        if key == "quantile95"
            && section
                .settings
                .quantile95
                .as_ref()
                .is_some_and(|q| !q.converged)
        {
            continue;
        }
        let candidates = if rows.is_empty() {
            section.model_rankings.get(key).unwrap_or(rows)
        } else {
            rows
        };
        let mut chosen = BTreeSet::new();
        for peak in [false, true] {
            let value = |r: &crate::reasonings::GradientTopItem| {
                if peak {
                    r.impact_peak
                } else {
                    r.impact_active
                }
            };
            let row = candidates
                .iter()
                .filter(|r| {
                    (!rows.is_empty() || r.gradient_coef > 0.0)
                        && value(r).is_finite()
                        && value(r) > 0.0
                        && !r.event_name.is_empty()
                })
                .max_by(|a, b| {
                    value(a)
                        .total_cmp(&value(b))
                        .then_with(|| b.event_name.cmp(&a.event_name))
                });
            if let Some(row) = row.filter(|r| chosen.insert(&r.event_name)) {
                result.push(Signal {
                    family: family.into(),
                    target: target.into(),
                    model: model.into(),
                    name: row.event_name.clone(),
                    active: magnitude(row.impact_active),
                    peak: magnitude(row.impact_peak),
                    unit: unit.into(),
                });
            }
        }
    }

    result
}

fn entity_kind(family: &str) -> Option<&'static str> {
    if family.contains("_sql_") {
        Some("sql")
    } else if family.ends_with("_wait_events") {
        Some("wait")
    } else {
        None
    }
}

pub(super) fn entities(project: &ProjectData) -> BTreeSet<(&'static str, String)> {
    let mut result = BTreeSet::new();
    for (family, target, section) in gradient_families(&project.report) {
        let Some(kind) = entity_kind(family).filter(|_| target != "Custom target") else {
            continue;
        };
        // Keep previously presented per-model leaders when reading an older audit.
        for row in model_leaders(family, target, section) {
            result.insert((kind, row.name));
        }
        for row in &section.cross_model_classifications {
            if !row.event_name.trim().is_empty() {
                result.insert((kind, row.event_name.clone()));
            }
        }
    }
    result
}

fn overview(
    project: &ProjectData,
    ordinal: usize,
    instance: String,
    lang: ReportLanguage,
) -> Opening {
    // The MCP capture is restricted to the same observation range that produced these fits.
    let snapshots = &project.collection.awrs;
    let window = snapshots
        .first()
        .zip(snapshots.last())
        .map(|(a, b)| {
            format!(
                "{} – {}",
                a.snap_info.begin_snap_time, b.snap_info.end_snap_time
            )
        })
        .unwrap_or_default();
    let mut panels = Vec::new();
    let mut fallback = Vec::new();
    for (family, target, section) in gradient_families(&project.report)
        .into_iter()
        .filter(|(_, target, _)| *target != "Custom target")
    {
        // A single-model impact is not a combined score. Older captures can
        // retain the former without the joint classification.
        if section.cross_model_classifications.is_empty() {
            fallback.extend(model_leaders(family, target, section));
            continue;
        }
        let q95_available = section
            .settings
            .quantile95
            .as_ref()
            .is_none_or(|d| d.converged);
        let points: Vec<_> = section
            .cross_model_classifications
            .iter()
            .filter(|row| !row.event_name.trim().is_empty())
            .map(|row| SignalPoint {
                name: row.event_name.clone(),
                active: magnitude(row.combined_impact),
                peak: magnitude(row.combined_peak_impact),
                classification: row.classification.clone(),
                selected: [
                    Some(row.in_ridge),
                    Some(row.in_elastic_net),
                    Some(row.in_huber),
                    q95_available.then_some(row.in_quantile95),
                ],
                // An introductory plot describes measurements; no incident-specific action or evidence is attached.
                interpretation: String::new(),
                action: String::new(),
                evidence_refs: vec![],
                href: None,
            })
            .collect();
        if !points.is_empty() {
            panels.push(SignalPanel {
                id: format!("intro-fit-{ordinal}-{}", family.replace('_', "-")),
                project_id: format!("instance-{ordinal}"),
                project_label: instance.clone(),
                window: window.clone(),
                family: super::report_signal_adapter::family_label(family, lang).into(),
                target: target.into(),
                coverage: format!(
                    "{} {}",
                    points.len(),
                    lang.choose(
                        "contributors in the recorded model selection",
                        "pozycji we wskazaniach modeli"
                    )
                ),
                points,
            });
        }
    }
    Opening {
        instance,
        window,
        intervals: snapshots.len(),
        panels,
        fallback,
    }
}

impl AnalysisRuntime {
    pub(super) fn gradient_openings(
        &self,
        state: &AnalysisSession,
        labels: &BTreeMap<String, String>,
        lang: ReportLanguage,
    ) -> Vec<Opening> {
        let mut reserved: BTreeSet<_> = labels.values().cloned().collect();
        let mut next = 1;
        state
            .project_ids
            .iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .enumerate()
            .map(|(index, id)| {
                let p = &self.projects[id];
                let instance = labels.get(id).cloned().unwrap_or_else(|| loop {
                    let name = format!("{} {next}", lang.choose("Instance", "Instancja"));
                    next += 1;
                    if reserved.insert(name.clone()) {
                        break name;
                    }
                });
                overview(p, index + 1, instance, lang)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reasonings::{GradientTopItem, TopPeaksSelected};

    fn row(name: &str, active: f64, peak: f64) -> CrossModelClassification {
        CrossModelClassification {
            event_name: name.into(),
            combined_impact: active,
            combined_peak_impact: peak,
            in_ridge: true,
            in_huber: true,
            ..Default::default()
        }
    }

    #[test]
    fn whole_capture_keeps_every_joint_point_and_source_selection_per_fit() {
        let mut p = super::super::tests::comparison_project(
            "project-1",
            &[1.0, 3.0, 0.5],
            &[2.0, 17.0, 4.0],
        );
        let begin = p.collection.awrs[0].snap_info.begin_snap_time.clone();
        let end = p.collection.awrs[2].snap_info.end_snap_time.clone();
        let cpu_name = "0123456789abc";
        let earlier_name = "123456789abcd";
        let cpu = DbTimeGradientSection {
            ridge_top: vec![GradientTopItem {
                event_name: earlier_name.into(),
                impact_active: 8.0,
                impact_peak: 9.0,
                ..Default::default()
            }],
            cross_model_classifications: vec![row(cpu_name, 0.0, 1.25), row("zero-pair", 0.0, 0.0)],
            ..Default::default()
        };
        p.report.top_spikes_marked.push(TopPeaksSelected {
            report_name: "incident-only".into(),
            report_date: "incident-hour".into(),
            ..Default::default()
        });
        p.report.db_cpu_gradient_sql_cpu_time = Some(cpu.clone());
        p.report.custom_gradient_instance_stats = Some(cpu);
        p.report.db_time_gradient_fg_wait_events = Some(DbTimeGradientSection {
            cross_model_classifications: vec![row("example wait", 4.5, 8.0)],
            ..Default::default()
        });
        let p = ProjectData::from(p);
        let scope = overview(&p, 2, "Public node".into(), ReportLanguage::new("EN"));
        assert_eq!(scope.window, format!("{begin} – {end}"));
        assert_eq!(scope.intervals, 3);
        assert_eq!(scope.panels.len(), 2);
        assert!(scope.fallback.is_empty());
        assert_eq!(scope.panels[0].target, "DB Time");
        let cpu = &scope.panels[1];
        assert_eq!(cpu.target, "DB CPU");
        assert_eq!(cpu.points.len(), 2);
        assert_eq!(cpu.window, scope.window);
        assert_eq!(cpu.id, "intro-fit-2-db-cpu-sql-cpu-time");
        assert_eq!(
            cpu.points[0].selected,
            [Some(true), Some(false), Some(true), Some(false)]
        );
        assert_eq!(
            (cpu.points[0].active, cpu.points[0].peak),
            (Some(0.0), Some(1.25))
        );
        assert_eq!(
            (cpu.points[1].active, cpu.points[1].peak),
            (Some(0.0), Some(0.0))
        );
        let visible = json!(scope).to_string();
        assert!(!visible.contains("incident-hour") && !visible.contains("project-1"));
        let entities = entities(&p);
        assert!(entities.contains(&("sql", cpu_name.to_string())));
        assert!(entities.contains(&("sql", earlier_name.to_string())));
        assert!(entities.contains(&("wait", "example wait".into())));
    }

    #[test]
    fn older_capture_preserves_per_model_observations_without_constructing_joint_scores() {
        let mut p = super::super::tests::comparison_project("project-1", &[2.0], &[0.2]);
        let mut section = DbTimeGradientSection::default();
        section.settings.input_db_time_unit = "db_cpu_per_second".into();
        section.ridge_top.push(GradientTopItem {
            event_name: "low".into(),
            impact_active: 4.0,
            impact_peak: 5.0,
            ..Default::default()
        });
        section.ridge_top.push(GradientTopItem {
            event_name: "upper".into(),
            impact_active: 0.1,
            impact_peak: 9.0,
            ..Default::default()
        });
        section.model_rankings.insert(
            "huber".into(),
            vec![GradientTopItem {
                event_name: "inverse".into(),
                gradient_coef: -2.0,
                impact_peak: 30.0,
                ..Default::default()
            }],
        );
        p.report.db_cpu_gradient_sql_cpu_time = Some(section);
        let scope = overview(
            &ProjectData::from(p),
            1,
            "Instance 1".into(),
            ReportLanguage::new("EN"),
        );
        assert!(scope.panels.is_empty());
        assert_eq!(scope.fallback.len(), 2);
        assert_eq!(scope.fallback[0].active, Some(4.0));
        assert_eq!(
            (scope.fallback[1].peak, scope.fallback[1].unit.as_str()),
            (Some(9.0), "s/s")
        );
        let obsolete = json!({"instance":"Instance 1","window":"full","signals":scope.fallback,"basis":{"rows":[{"statement":"stored incident"}]}});
        let decoded: Opening = serde_json::from_value(obsolete).unwrap();
        assert_eq!(decoded.fallback.len(), 2);
        assert!(!json!(decoded).to_string().contains("stored incident"));
        assert_eq!(magnitude(-1.0), None);
        assert_eq!(magnitude(f64::NAN), None);
    }
}
