//! Project-scoped opening hypotheses, chapter checks and their reviewed outcomes.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct OpeningHypothesis {
    id: String,
    statement: String,
    gradient_signal: String,
    workload_profile: String,
    comparison_window: String,
    test: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct HypothesisCheck {
    id: String,
    reason: String,
    evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct HypothesisOutcome {
    id: String,
    status: String,
    reason: String,
    evidence_refs: Vec<String>,
}

fn invalid(message: &str) -> Value {
    tool_error("INVALID_AUDIT_HYPOTHESES", message)
}

fn valid_text(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= 4000
}

fn has_opening(brief: &AuditBrief) -> bool {
    !brief.opening_hypotheses.is_empty() || brief.hypothesis_gap.is_some()
}

fn opening<'a>(audit: &'a AuditState, project: &str) -> Option<&'a AuditBrief> {
    audit
        .briefs
        .values()
        .find(|b| b.project_id == project && has_opening(b))
}

fn project_has_comparison(project: &ProjectData) -> bool {
    use crate::measurements::{db_load_rate, db_load_seconds, DbLoadMetric};
    gradient_families(&project.report)
        .into_iter()
        .any(|(_, target, s)| {
            target != "Custom target"
                && (!s.cross_model_classifications.is_empty()
                    || !s.ridge_top.is_empty()
                    || !s.huber_top.is_empty()
                    || !s.elastic_net_top.is_empty()
                    || !s.quantile95_top.is_empty()
                    || s.model_rankings.values().any(|r| !r.is_empty()))
        })
        && project.collection.awrs.iter().any(|a| {
            [DbLoadMetric::DbTime, DbLoadMetric::DbCpu]
                .into_iter()
                .any(|metric| {
                    db_load_rate(a, metric).is_some() || db_load_seconds(a, metric).is_some()
                })
        })
}

fn contains_gradient_rows(record: &EvidenceRecord) -> bool {
    record.tool_name == "get_precomputed_analysis"
        && record.arguments["section"] == "full_gradients"
        && record.result["data"].as_object().is_some_and(|families| {
            families.iter().any(|(name, section)| {
                let nonempty =
                    |value: &Value| value.as_array().is_some_and(|rows| !rows.is_empty());
                (name.starts_with("db_time_") || name.starts_with("db_cpu_"))
                    && ([
                        "cross_model_classifications",
                        "ridge_top",
                        "elastic_net_top",
                        "huber_top",
                        "quantile95_top",
                    ]
                    .iter()
                    .any(|key| nonempty(&section[key]))
                        || section["model_rankings"]
                            .as_object()
                            .is_some_and(|models| models.values().any(nonempty)))
            })
        })
}

pub(super) fn validate_content(
    brief: &AuditBrief,
    state: &AnalysisSession,
    project: &ProjectData,
) -> Result<(), Value> {
    let rows = &brief.opening_hypotheses;
    if has_opening(brief) {
        if brief.chapter_id != "performance_profile" {
            return Err(invalid("Open hypotheses in the performance_profile brief."));
        }
        let comparison_available = project_has_comparison(project);
        if let Some(gap) = &brief.hypothesis_gap {
            if !valid_text(gap) || !rows.is_empty() || comparison_available {
                return Err(invalid("Use hypothesis_gap for a missing gradient or workload profile. When both inputs exist, record two or three hypotheses and state any limits of the scoped comparison."));
            }
        } else {
            if !(2..=3).contains(&rows.len()) {
                return Err(invalid("Record two or three distinct opening hypotheses."));
            }
            let mut ids = BTreeSet::new();
            let mut statements = BTreeSet::new();
            for row in rows {
                if !matches!(row.id.as_str(), "H1" | "H2" | "H3")
                    || !ids.insert(&row.id)
                    || !statements.insert(row.statement.trim().to_lowercase())
                    || [
                        &row.statement,
                        &row.gradient_signal,
                        &row.workload_profile,
                        &row.comparison_window,
                        &row.test,
                    ]
                    .iter()
                    .any(|s| !valid_text(s))
                {
                    return Err(invalid("Use distinct H1–H3 IDs and statements. Provide a gradient signal, measured absolute workload profile, comparison window and distinguishing test for each."));
                }
            }
            let cited = || {
                brief
                    .evidence_refs
                    .iter()
                    .filter_map(|r| state.evidence.get(r))
                    .filter(|r| r.project_id.as_deref() == Some(brief.project_id.as_str()))
            };
            if !comparison_available
                || !cited().any(contains_gradient_rows)
                || !cited().any(|r| r.tool_name == "get_database_load_summary")
            {
                return Err(invalid("Cite this project's measured get_database_load_summary and get_precomputed_analysis(full_gradients) with DB Time or DB CPU rows."));
            }
        }
    }
    let check_chapter = !matches!(
        brief.chapter_id.as_str(),
        "scope" | "performance_profile" | "gradients_anomalies" | "synthesis"
    );
    if !brief.hypothesis_checks.is_empty() && !check_chapter {
        return Err(invalid(
            "Record hypothesis_checks in relevant diagnostic chapters.",
        ));
    }
    if !brief.hypothesis_outcomes.is_empty() && brief.chapter_id != "gradients_anomalies" {
        return Err(invalid(
            "Record hypothesis_outcomes in chapter 9 (gradients_anomalies).",
        ));
    }
    let mut used = BTreeSet::new();
    for (id, reason, refs) in brief
        .hypothesis_checks
        .iter()
        .map(|r| (&r.id, &r.reason, &r.evidence_refs))
        .chain(
            brief
                .hypothesis_outcomes
                .iter()
                .map(|r| (&r.id, &r.reason, &r.evidence_refs)),
        )
    {
        if !matches!(id.as_str(), "H1" | "H2" | "H3")
            || !used.insert(id)
            || !valid_text(reason)
            || refs.len() > 128
            || refs.iter().any(|r| !brief.evidence_refs.contains(r))
        {
            return Err(invalid("Each ID needs one reason and its exact evidence_refs retained in this brief. Document unavailable checks explicitly."));
        }
    }
    for row in &brief.hypothesis_outcomes {
        if !matches!(row.status.as_str(), "confirmed" | "rejected" | "unresolved") {
            return Err(invalid(
                "Use confirmed, rejected or unresolved for each outcome.",
            ));
        }
    }
    Ok(())
}

type Basis = BTreeMap<String, u64>;

fn scope_basis(audit: &AuditState, project: &str) -> Basis {
    audit
        .briefs
        .values()
        .filter(|b| b.project_id == project && b.chapter_id == "scope")
        .map(|b| (b.brief_id.clone(), b.revision))
        .collect()
}

fn current_opening<'a>(audit: &'a AuditState, project: &str) -> Option<&'a AuditBrief> {
    opening(audit, project).filter(|b| {
        let scope = scope_basis(audit, project);
        !scope.is_empty() && b.hypothesis_basis == scope
    })
}

fn opening_basis(open: &AuditBrief) -> Basis {
    BTreeMap::from([(open.brief_id.clone(), open.revision)])
}

fn current_checks<'a>(audit: &'a AuditState, open: &AuditBrief) -> Vec<&'a AuditBrief> {
    let source = opening_basis(open);
    audit
        .briefs
        .values()
        .filter(|b| {
            b.project_id == open.project_id
                && !b.hypothesis_checks.is_empty()
                && b.hypothesis_basis == source
        })
        .collect()
}

fn outcome_basis(open: &AuditBrief, checks: &[&AuditBrief]) -> Basis {
    let mut basis = opening_basis(open);
    basis.extend(checks.iter().map(|b| (b.brief_id.clone(), b.revision)));
    basis
}

// Return the source revisions to stamp into a live brief or compare during restore.
pub(super) fn validate_dependencies(
    brief: &AuditBrief,
    audit: &AuditState,
    evidence: &BTreeMap<String, EvidenceRecord>,
) -> Result<Basis, Value> {
    let project = &brief.project_id;
    let open = opening(audit, project);
    if has_opening(brief) {
        let scope = scope_basis(audit, project);
        if scope.is_empty() {
            return Err(invalid(
                "Save the scope discussion for this project before opening hypotheses.",
            ));
        }
        if open.is_some_and(|b| b.brief_id != brief.brief_id) {
            return Err(invalid(
                "Revise the existing opening brief_id for this project.",
            ));
        }
        return Ok(scope);
    } else if open.is_some_and(|b| b.brief_id == brief.brief_id) {
        return Err(invalid(
            "Retain the opening hypotheses or the documented gap when revising their brief.",
        ));
    }
    if brief.hypothesis_checks.is_empty() && brief.hypothesis_outcomes.is_empty() {
        return Ok(Basis::new());
    }
    let open = current_opening(audit, project)
        .filter(|b| !b.opening_hypotheses.is_empty())
        .ok_or_else(|| {
            invalid("Save opening_hypotheses for the current project scope before recording a check or outcome.")
        })?;
    let ids: BTreeSet<_> = open.opening_hypotheses.iter().map(|h| &h.id).collect();
    if !brief.hypothesis_checks.is_empty() {
        if brief.hypothesis_checks.iter().any(|c| !ids.contains(&c.id)) {
            return Err(invalid(
                "A check must refer to an opening ID for the same project.",
            ));
        }
        return Ok(opening_basis(open));
    }
    if audit.briefs.values().any(|b| {
        b.project_id == *project
            && !b.hypothesis_outcomes.is_empty()
            && b.brief_id != brief.brief_id
    }) {
        return Err(invalid(
            "Revise the existing chapter 9 outcome brief_id for this project.",
        ));
    }
    if brief
        .hypothesis_outcomes
        .iter()
        .map(|o| &o.id)
        .collect::<BTreeSet<_>>()
        != ids
    {
        return Err(invalid(
            "Chapter 9 must resolve every opening ID for this project exactly once.",
        ));
    }
    let checks = current_checks(audit, open);
    for outcome in &brief.hypothesis_outcomes {
        let mut tested = false;
        let mut cited = false;
        for check in checks
            .iter()
            .flat_map(|b| &b.hypothesis_checks)
            .filter(|c| c.id == outcome.id)
        {
            tested = true;
            cited |= check.evidence_refs.iter().any(|r| {
                let record = &evidence[r];
                outcome.evidence_refs.contains(r)
                    && !open.evidence_refs.contains(r)
                    && record.project_id.as_deref() == Some(project.as_str())
                    && !(record.tool_name == "get_precomputed_analysis"
                        && record.arguments["section"] == "full_gradients")
            });
        }
        if !tested || (outcome.status != "unresolved" && !cited) {
            return Err(invalid("Revisit each opening ID in a diagnostic chapter. Cite a project-specific follow-up measurement beyond the opening evidence and gradient ranking for each confirmed/rejected outcome. Record an unavailable test and its reason when unresolved."));
        }
    }
    Ok(outcome_basis(open, &checks))
}

fn current_outcomes<'a>(
    audit: &'a AuditState,
    open: &AuditBrief,
    checks: &[&AuditBrief],
) -> Option<&'a AuditBrief> {
    let basis = outcome_basis(open, checks);
    audit.briefs.values().find(|b| {
        b.project_id == open.project_id
            && !b.hypothesis_outcomes.is_empty()
            && b.hypothesis_basis == basis
    })
}

pub(super) fn chapter_complete(audit: &AuditState, chapter: &str, project: &str) -> bool {
    match chapter {
        "performance_profile" => current_opening(audit, project).is_some(),
        "gradients_anomalies" => current_opening(audit, project).is_some_and(|open| {
            open.hypothesis_gap.is_some()
                || current_outcomes(audit, open, &current_checks(audit, open)).is_some()
        }),
        _ => true,
    }
}

pub(in super::super) fn hypothesis_status(audit: &AuditState, project: &str) -> Value {
    let open = current_opening(audit, project);
    let checks = open.map(|b| current_checks(audit, b)).unwrap_or_default();
    let outcomes = open.and_then(|b| current_outcomes(audit, b, &checks));
    let check_rows = checks.iter().flat_map(|brief| brief.hypothesis_checks.iter().map(move |row|
        json!({"id":row.id,"chapter_id":brief.chapter_id,"reason":row.reason,"evidence_refs":row.evidence_refs}))).collect::<Vec<_>>();
    json!({
        "project_id":project,
        "opening_brief_id":opening(audit,project).map(|b| &b.brief_id),
        "opening_needs_scope_review":open.is_none() && opening(audit,project).is_some(),
        "opening_hypotheses":open.map_or(&[][..],|b| b.opening_hypotheses.as_slice()),
        "opening_evidence_refs":open.map_or(&[][..],|b| b.evidence_refs.as_slice()),
        "hypothesis_gap":open.and_then(|b| b.hypothesis_gap.as_deref()),
        "hypothesis_checks":check_rows,
        "hypothesis_outcomes":outcomes.map_or(&[][..],|b| b.hypothesis_outcomes.as_slice())
    })
}

pub(super) fn add_tool_fields(properties: &mut Value) {
    let id = json!({"type":"string","enum":["H1","H2","H3"]});
    let text = json!({"type":"string","minLength":1,"maxLength":4000});
    let refs = json!({"type":"array","items":{"type":"string"},"description":"Exact references also retained in the evidence_refs of this brief. Empty only for a documented gap."});
    let rows = |fields: Value, required: &[&str], min| json!({"type":"array","minItems":min,"maxItems":3,"items":{"type":"object","additionalProperties":false,"properties":fields,"required":required}});
    properties["opening_hypotheses"] = rows(
        json!({"id":id,"statement":text,"gradient_signal":text,"workload_profile":text,"comparison_window":text,"test":text}),
        &[
            "id",
            "statement",
            "gradient_signal",
            "workload_profile",
            "comparison_window",
            "test",
        ],
        2,
    );
    properties["opening_hypotheses"]["description"] = json!("After scope, compare project-specific full_gradients with get_database_load_summary. Include measured values/units, incident and baseline, and a distinguishing test for each competing explanation.");
    properties["hypothesis_gap"] = text.clone();
    properties["hypothesis_gap"]["description"] = json!("In performance_profile explain a missing gradient or workload profile when the data cannot support opening hypotheses.");
    properties["hypothesis_checks"] = rows(
        json!({"id":id,"reason":text,"evidence_refs":refs}),
        &["id", "reason", "evidence_refs"],
        1,
    );
    properties["hypothesis_outcomes"] = rows(
        json!({"id":id,"status":{"type":"string","enum":["confirmed","rejected","unresolved"]},"reason":text,"evidence_refs":refs}),
        &["id", "status", "reason", "evidence_refs"],
        2,
    );
}
