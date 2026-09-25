//! Read only recorded rows and their cited source envelopes. No prose-to-number
//! inference and no join between different instances or gradient fits.
use super::*;
use crate::report_signals::{SignalAtlas, SignalBrief, SignalMoment, SignalPanel, SignalPoint};

#[cfg(test)]
#[path = "signal_replay_tests.rs"]
mod replay_tests;

fn cell<'a>(row: &'a ReportTableRow, key: &str) -> &'a str {
    row.cells.get(key).map(String::as_str).unwrap_or("")
}

pub(super) fn build_signal_atlas(
    document: &Value,
    state: &AnalysisSession,
    links: &ReportEntityLinks,
) -> Option<SignalAtlas> {
    let labels = report_project_labels(document);
    let label = |project: &str| {
        labels
            .get(project)
            .cloned()
            .unwrap_or_else(|| project.into())
    };
    let rows = |kind: &str| {
        state
            .report_tables
            .values()
            .filter(move |t| t.kind == kind)
            .flat_map(|t| t.rows.iter())
            .collect::<Vec<_>>()
    };
    let mut groups = BTreeMap::<(String, String), Vec<&ReportTableRow>>::new();
    for row in rows("gradients") {
        groups
            .entry((
                cell(row, "project_id").into(),
                cell(row, "analysis_family").into(),
            ))
            .or_default()
            .push(row);
    }
    let mut atlas = SignalAtlas {
        version: 1,
        ..Default::default()
    };
    // Put waits and SQL first; accounting identities remain available in their own fits.
    let family_order = |family: &str| match family {
        "db_time_foreground_wait_events" => 0,
        "db_cpu_sql_cpu_time" => 1,
        "db_time_sql_elapsed_time" => 2,
        _ => 3,
    };
    let mut groups = groups.into_iter().collect::<Vec<_>>();
    groups.sort_by(|a, b| {
        a.0 .0
            .cmp(&b.0 .0)
            .then(family_order(&a.0 .1).cmp(&family_order(&b.0 .1)))
            .then(a.0 .1.cmp(&b.0 .1))
    });
    for ((project, family), mut group) in groups {
        sort_gradient_rows_by_peak_impact(&mut group);
        let mut names = BTreeSet::new();
        group.retain(|r| names.insert(cell(r, "contributor")));
        let dataset = document["datasets"].as_array().and_then(|datasets| {
            datasets
                .iter()
                .find(|d| d["project_id"].as_str() == Some(&project))
        });
        let window = dataset
            .map(|d| {
                format!(
                    "{} – {} · {} snapshots",
                    d["begin_time"].as_str().unwrap_or("start not supplied"),
                    d["end_time"].as_str().unwrap_or("end not supplied"),
                    d["snapshots"].as_u64().unwrap_or(0)
                )
            })
            .unwrap_or_else(|| "Window retained in the cited source evidence".into());
        let points = group
            .iter()
            .map(|row| {
                let raw = row
                    .evidence_refs
                    .iter()
                    .filter_map(|id| state.evidence.get(id))
                    .filter(|r| {
                        r.project_id.as_deref() == Some(&project)
                            && r.result["section"] == "full_gradients"
                    })
                    .filter_map(|r| r.result.get("data")?.get(&family))
                    .filter_map(|s| s["cross_model_classifications"].as_array())
                    .find_map(|list| {
                        list.iter().find(|v| {
                            v["event_name"].as_str().is_some_and(|name| {
                                name.eq_ignore_ascii_case(cell(row, "contributor"))
                            })
                        })
                    });
                let selected = ["in_ridge", "in_elastic_net", "in_huber", "in_quantile95"]
                    .map(|key| raw.and_then(|r| r[key].as_bool()));
                let magnitude = |key: &str, column: &str| {
                    raw.and_then(|r| r[key].as_f64())
                        .filter(|n| n.is_finite() && *n >= 0.0)
                        .or_else(|| report_table_numeric_value(row, column).filter(|n| *n >= 0.0))
                };
                let kind = if family.contains("sql_") {
                    Some("sql")
                } else if family.contains("wait_events") {
                    Some("wait")
                } else {
                    None
                };
                let href = kind.and_then(|kind| {
                    links
                        .targets
                        .get(&(
                            project.clone(),
                            kind.into(),
                            cell(row, "contributor").to_ascii_lowercase(),
                        ))
                        .cloned()
                });
                SignalPoint {
                    name: cell(row, "contributor").into(),
                    active: magnitude("combined_impact", "typical_impact"),
                    peak: magnitude("combined_peak_impact", "peak_impact"),
                    classification: raw
                        .and_then(|r| r["classification"].as_str())
                        .unwrap_or(cell(row, "classification"))
                        .into(),
                    selected,
                    interpretation: cell(row, "interpretation").into(),
                    action: cell(row, "action").into(),
                    href,
                    evidence_refs: row.evidence_refs.clone(),
                }
            })
            .collect::<Vec<_>>();
        let family_label = match family.as_str() {
            "db_time_foreground_wait_events" => "Foreground waits",
            "db_time_sql_elapsed_time" => "SQL elapsed time",
            "db_cpu_sql_cpu_time" => "SQL CPU",
            "db_cpu_instance_stats" => "CPU counters",
            "db_time_instance_stats_counters" => "Activity counters",
            "db_time_instance_stats_volumes" => "Work volumes",
            "db_time_instance_stats_time" => "Time-model signals",
            _ => family.as_str(),
        };
        atlas.panels.push(SignalPanel{id:format!("fit-{}",atlas.panels.len()+1),project_id:project.clone(),project_label:label(&project),window,family:family_label.into(),target:cell(group[0],"target_metric").into(),coverage:format!("{} recorded contributors in this project/family. Scope is the supplied top-selection and material-wait coverage, not every predictor in the database.",points.len()),points});
    }
    if atlas.panels.is_empty() {
        return None;
    }
    for row in rows("analytic_signal_synthesis") {
        let project = cell(row, "project_id");
        let text = cell(row, "dominant_gradient_signals").to_ascii_lowercase();
        let mut signals = atlas
            .panels
            .iter()
            .filter(|p| p.project_id == project)
            .flat_map(|p| &p.points)
            .filter(|p| text.contains(&p.name.to_ascii_lowercase()))
            .map(|p| p.name.clone())
            .collect::<Vec<_>>();
        signals.sort_by_key(|n| std::cmp::Reverse(n.len()));
        let all = signals.clone();
        signals.retain(|name| {
            !all.iter()
                .any(|other| other.len() > name.len() && other.contains(name))
        });
        signals.sort();
        signals.dedup();
        signals.truncate(6);
        if signals.is_empty() {
            continue;
        }
        atlas.briefs.push(SignalBrief {
            project_id: project.into(),
            project_label: label(project),
            title: cell(row, "entity").into(),
            conclusion: cell(row, "hypothesis").into(),
            boundary: cell(row, "counterevidence").into(),
            validation: cell(row, "recommended_validation").into(),
            confidence: cell(row, "confidence").into(),
            signals,
            evidence_refs: row.evidence_refs.clone(),
        });
    }
    // Show one strongest returned MAD anomaly and one most populated recorded
    // cluster per project. These selectors say nothing about unreturned windows.
    for project in &state.project_ids {
        let source = state
            .evidence
            .values()
            .filter(|r| {
                r.project_id.as_ref() == Some(project)
                    && r.result["section"] == "load_profile_anomalies"
            })
            .filter(|r| {
                state
                    .report_tables
                    .values()
                    .filter(|t| t.kind == "anomalies")
                    .flat_map(|t| &t.rows)
                    .any(|row| row.evidence_refs.contains(&r.evidence_id))
            })
            .max_by_key(|r| &r.evidence_id);
        if let Some(source) = source {
            if let Some(data) = source.result["data"].as_array() {
                if let Some(top) = data
                    .iter()
                    .filter(|v| v["mad_score"].as_f64().is_some_and(f64::is_finite))
                    .max_by(|a, b| {
                        a["mad_score"]
                            .as_f64()
                            .unwrap()
                            .total_cmp(&b["mad_score"].as_f64().unwrap())
                    })
                {
                    if let (Some(value), Some(baseline), Some(score)) = (
                        top["per_second"].as_f64(),
                        top["avg_value_per_second"].as_f64(),
                        top["mad_score"].as_f64(),
                    ) {
                        atlas.moments.push(SignalMoment{project_id:project.clone(),project_label:label(project),kind:"Returned anomaly".into(),window:top["anomaly_date"].as_str().unwrap_or("Unknown timestamp").into(),title:top["load_profile_stat_name"].as_str().unwrap_or("Unnamed metric").into(),measure:format!("{} /s · MAD {}",crate::report_signals::number(value),crate::report_signals::number(score)),context:format!("Largest MAD among {} returned anomalies. Source mean {} /s; not a matched-workload baseline. Check volume and latency in the same window.",data.len(),crate::report_signals::number(baseline)),evidence_refs:vec![source.evidence_id.clone()]});
                    }
                }
            }
        }
        if let Some(row) = rows("anomaly_clusters")
            .into_iter()
            .filter(|row| cell(row, "project_id") == project)
            .max_by_key(|row| {
                cell(row, "members")
                    .split(';')
                    .filter(|s| !s.trim().is_empty())
                    .count()
            })
        {
            let count = cell(row, "members")
                .split(';')
                .filter(|s| !s.trim().is_empty())
                .count();
            let selected = rows("anomaly_clusters")
                .into_iter()
                .filter(|r| cell(r, "project_id") == project)
                .count();
            atlas.moments.push(SignalMoment {
                project_id: project.clone(),
                project_label: label(project),
                kind: "Recorded cluster".into(),
                window: cell(row, "time_scope").into(),
                title: format!("{} co-occurring signals", count),
                measure: format!("Snapshot {}", cell(row, "cluster_id")),
                context: format!(
                    "Most populated of {selected} recorded clusters. {}",
                    cell(row, "common_context")
                ),
                evidence_refs: row.evidence_refs.clone(),
            });
        }
    }
    // Invalid legacy rows must not suppress the full report or manufacture data.
    // The original structured tables are always rendered independently.
    match crate::report_signals::validate(&atlas) {
        Ok(()) => Some(atlas),
        Err(error) => {
            debug_note!("Signal atlas unavailable: {}", error);
            None
        }
    }
}
