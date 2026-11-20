//! Project one registered cluster calculation into the client edition.
use super::*;
use std::collections::BTreeSet;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) analysis_id: String,
    pub(super) brief_id: String,
    pub(super) brief_revision: u64,
    title: String,
    owner: String,
    success_criterion: String,
    evidence_ref: String,
    pub(super) instance_labels: BTreeMap<String, String>,
    #[serde(default)]
    include_sql_ids: bool,
}

pub(super) fn preview_params() -> Value {
    let text = json!({"type":"string","minLength":1});
    json!({
        "type":"object","additionalProperties":false,"properties":{
            "brief_id":text,"brief_revision":{"type":"integer","minimum":1},"title":text,
            "owner":text,"success_criterion":text,"evidence_ref":text,
            "instance_labels":{"type":"object","minProperties":2,"additionalProperties":text},
            "include_sql_ids":{"type":"boolean","default":false}
        },"required":["brief_id","brief_revision","title","owner","success_criterion","evidence_ref","instance_labels"]
    })
}

pub(super) fn tools() -> Vec<Value> {
    let text = json!({"type":"string","minLength":1});
    vec![
        function_definition("preview_cpu_report", "Prepare the offline RAC CPU client report from a current brief and its registered whole-cluster get_rac_cpu_scenario evidence. Supply a public instance label for every selected project. The report presents the monitoring module prefix and SQL reduction percentage. Opt in to include the exact selected SQL_IDs.", preview_params()),
        function_definition("read_cpu_report_asset", "Read one verified public CPU report file or the published ZIP, in byte-offset chunks.", json!({
            "type":"object","additionalProperties":false,"properties":{"preview_id":text,"asset":{"type":"string","enum":["index.html","evidence.html","report.css","report.zip"]},"offset":{"type":"integer","minimum":0}},"required":["preview_id","asset"]
        })),
        function_definition("publish_cpu_report", "Issue the exact checked CPU preview as a client ZIP after the user's explicit request. The current analysis and source brief revision must still match.", json!({
            "type":"object","additionalProperties":false,"properties":{"preview_id":text,"user_request":{"type":"string","minLength":1,"maxLength":4000}},"required":["preview_id","user_request"]
        })),
    ]
}

#[derive(Deserialize)]
struct Source {
    project_ids: Vec<String>,
    paired_intervals: usize,
    alignment_tolerance_seconds: f64,
    assumptions: Assumptions,
    observed_peak: Peak,
    scenario_peak: Peak,
    saved_cpu_hours: f64,
    monitor_cpu_hours: f64,
    selected_sql_cpu_hours: f64,
    instances: Vec<SourceNode>,
    intervals: Vec<SourceInterval>,
}

#[derive(Deserialize)]
struct Assumptions {
    monitor_module_prefix: String,
    selected_sql_ids: Vec<String>,
    sql_reduction_pct: f64,
    monitor_reduction_pct: f64,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Peak {
    begin: String,
    end: String,
    pub(super) active_cpu: f64,
}

#[derive(Deserialize)]
struct SourceNode {
    project_id: String,
    observed_peak: Peak,
    scenario_peak: Peak,
    saved_cpu_hours: f64,
}

#[derive(Deserialize)]
struct SourceInterval {
    begin: String,
    end: String,
    observed_active_cpu: f64,
    scenario_active_cpu: f64,
    saved_cpu_seconds: f64,
    instances: Vec<SourceNodeInterval>,
}

#[derive(Deserialize)]
struct SourceNodeInterval {
    project_id: String,
    #[serde(flatten)]
    measured: NodeInterval,
}

#[derive(Deserialize, Serialize)]
struct NodeInterval {
    begin: String,
    end: String,
    wall_seconds: f64,
    db_cpu_seconds: f64,
    monitor_cpu_seconds: f64,
    selected_sql_cpu_seconds: f64,
    monitor_top_rows: usize,
    selected_top_rows: usize,
    observed_active_cpu: f64,
    scenario_active_cpu: f64,
}

// These types are the public allowlist. Private project IDs and raw evidence are never serialized.
#[derive(Serialize)]
pub(super) struct Model {
    pub(super) begin: String,
    pub(super) end: String,
    pub(super) paired_intervals: usize,
    alignment_tolerance_seconds: f64,
    monitor_module_prefix: String,
    monitor_reduction_pct: f64,
    selected_sql_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) selected_sql_ids: Option<Vec<String>>,
    sql_reduction_pct: f64,
    pub(super) observed_peak: Peak,
    pub(super) scenario_peak: Peak,
    pub(super) saved_cpu_hours: f64,
    observed_cpu_hours: f64,
    scenario_cpu_hours: f64,
    monitor_cpu_hours: f64,
    selected_sql_cpu_hours: f64,
    instances: Vec<Node>,
}

#[derive(Serialize)]
struct Node {
    instance_label: String,
    observed_peak: Peak,
    scenario_peak: Peak,
    saved_cpu_hours: f64,
    observed_cpu_hours: f64,
    scenario_cpu_hours: f64,
}

struct Interval {
    begin: String,
    end: String,
    observed_active_cpu: f64,
    scenario_active_cpu: f64,
    saved_cpu_seconds: f64,
    // Same explicit project order as the public instance summaries.
    instances: Vec<NodeInterval>,
}

fn timestamp(value: &str) -> ToolResult<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .map_err(|_| invalid("The scenario contains an invalid source timestamp."))
}

fn projection(
    recipe: &Recipe,
    session: &AnalysisSession,
    source: &AuditBrief,
) -> ToolResult<(Model, Vec<Interval>)> {
    let record = session
        .evidence
        .get(&recipe.evidence_ref)
        .filter(|e| {
            e.tool_name == "get_rac_cpu_scenario"
                && e.project_id.is_none()
                && source.evidence_refs.contains(&e.evidence_id)
        })
        .ok_or_else(|| {
            invalid("Select registered cluster CPU scenario evidence cited by the current brief.")
        })?;
    let s: Source = serde_json::from_value(record.result.clone())
        .map_err(|e| invalid(format!("The stored CPU calculation is incomplete: {e}")))?;
    let projects = s.project_ids.iter().collect::<BTreeSet<_>>();
    if projects.len() < 2
        || projects.len() != s.project_ids.len()
        || !projects.contains(&source.project_id)
        || projects != session.project_ids.iter().collect()
        || projects != recipe.instance_labels.keys().collect()
        || projects != s.instances.iter().map(|n| &n.project_id).collect()
        || s.instances.len() != projects.len()
    {
        return Err(invalid("The calculation and its public labels must cover every instance in this audit exactly once."));
    }
    if s.intervals.is_empty() || s.intervals.len() != s.paired_intervals {
        return Err(invalid(
            "The calculation lacks its complete aligned timeline.",
        ));
    }
    let mut labels = HashSet::new();
    let mut nodes = Vec::new();
    let mut hours = vec![0.; s.instances.len()];
    let mut timeline = Vec::with_capacity(s.paired_intervals);
    for interval in s.intervals {
        timestamp(&interval.begin)?;
        timestamp(&interval.end)?;
        let mut by_project = interval
            .instances
            .into_iter()
            .map(|i| (i.project_id, i.measured))
            .collect::<BTreeMap<_, _>>();
        if by_project.keys().collect::<BTreeSet<_>>() != projects {
            return Err(invalid(
                "A timeline interval does not cover the selected instances.",
            ));
        }
        let mut rows = Vec::with_capacity(s.instances.len());
        for (i, node) in s.instances.iter().enumerate() {
            let row = by_project
                .remove(&node.project_id)
                .expect("checked each project");
            hours[i] += row.db_cpu_seconds / 3600.;
            rows.push(row);
        }
        timeline.push(Interval {
            begin: interval.begin,
            end: interval.end,
            observed_active_cpu: interval.observed_active_cpu,
            scenario_active_cpu: interval.scenario_active_cpu,
            saved_cpu_seconds: interval.saved_cpu_seconds,
            instances: rows,
        });
    }
    for (node, observed) in s.instances.into_iter().zip(hours) {
        let label = public_text(&recipe.instance_labels[&node.project_id], 120)?;
        if !labels.insert(label.to_lowercase()) {
            return Err(invalid("Each instance needs a distinct public label."));
        }
        nodes.push(Node {
            instance_label: label,
            observed_peak: node.observed_peak,
            scenario_peak: node.scenario_peak,
            saved_cpu_hours: node.saved_cpu_hours,
            observed_cpu_hours: observed,
            scenario_cpu_hours: observed - node.saved_cpu_hours,
        });
    }
    let scope = s.assumptions;
    let prefix = public_text(&scope.monitor_module_prefix, 128)?;
    let count = scope.selected_sql_ids.len();
    let ids = if recipe.include_sql_ids {
        Some(
            scope
                .selected_sql_ids
                .iter()
                .map(|id| public_text(id, 32))
                .collect::<ToolResult<Vec<_>>>()?,
        )
    } else {
        None
    };
    let observed = nodes.iter().map(|n| n.observed_cpu_hours).sum::<f64>();
    let model = Model {
        begin: timeline[0].begin.clone(),
        end: timeline.last().unwrap().end.clone(),
        paired_intervals: s.paired_intervals,
        alignment_tolerance_seconds: s.alignment_tolerance_seconds,
        monitor_module_prefix: prefix,
        monitor_reduction_pct: scope.monitor_reduction_pct,
        selected_sql_count: count,
        selected_sql_ids: ids,
        sql_reduction_pct: scope.sql_reduction_pct,
        observed_peak: s.observed_peak,
        scenario_peak: s.scenario_peak,
        saved_cpu_hours: s.saved_cpu_hours,
        observed_cpu_hours: observed,
        scenario_cpu_hours: observed - s.saved_cpu_hours,
        monitor_cpu_hours: s.monitor_cpu_hours,
        selected_sql_cpu_hours: s.selected_sql_cpu_hours,
        instances: nodes,
    };
    Ok((model, timeline))
}

impl PublicationRecipe for Recipe {
    const ZIP_FILENAME: &'static str = "raport-cpu.zip";
    fn analysis_id(&self) -> &str {
        &self.analysis_id
    }
    fn render(
        &self,
        session: &AnalysisSession,
        lang: ReportLanguage,
        _runtime: Option<&AnalysisRuntime>,
    ) -> ToolResult<(Files, Value)> {
        let projected = self.project(session)?;
        Ok((
            projected.files(lang, Pages::Standalone),
            projected.details(),
        ))
    }
}

pub(super) struct Projection {
    pub(super) report: PublicReport,
    pub(super) model: Model,
    timeline: Vec<Interval>,
    brief_revision: u64,
}

impl Recipe {
    pub(super) fn project(&self, session: &AnalysisSession) -> ToolResult<Projection> {
        let main = brief(session, &self.brief_id, self.brief_revision)?;
        let report = public_copy(main, &self.title, &self.owner, &self.success_criterion)?;
        let (model, timeline) = projection(self, session, main)?;
        Ok(Projection {
            report,
            model,
            timeline,
            brief_revision: self.brief_revision,
        })
    }
}

impl Projection {
    pub(super) fn node_number(&self, label: &str) -> Option<usize> {
        self.model
            .instances
            .iter()
            .position(|n| n.instance_label == label)
            .map(|i| i + 1)
    }

    pub(super) fn details(&self) -> Value {
        json!({"brief_revision":self.brief_revision,"public_report":self.report,"cpu_model":self.model})
    }

    pub(super) fn chapter_bodies(&self, lang: ReportLanguage, pages: Pages) -> (String, String) {
        (
            view::overview(&self.report, &self.model, lang, pages),
            view::evidence(&self.report, &self.model, &self.timeline, lang, pages),
        )
    }

    pub(super) fn files(&self, lang: ReportLanguage, pages: Pages) -> Files {
        let (decision, evidence) = (pages.section(false), pages.section(true));
        let (chapter, appendix) = self.chapter_bodies(lang, pages);
        BTreeMap::from([
            (decision.into(), chapter.into_bytes()),
            (evidence.into(), appendix.into_bytes()),
            (
                ASSETS[2].into(),
                concat!(
                    include_str!("tx_pilot_report.css"),
                    include_str!("cpu_pilot_report.css")
                )
                .as_bytes()
                .to_vec(),
            ),
        ])
    }
}

#[path = "cpu_pilot_report_view.rs"]
mod view;

#[cfg(test)]
#[path = "cpu_pilot_publication_tests.rs"]
pub(super) mod tests;
