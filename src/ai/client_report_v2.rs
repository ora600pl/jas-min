//! Problem-oriented public editions of a durable guided audit.
use super::super::{ProjectData, audit_workflow::AuditState, tx_pilot::PilotKind};
use super::*;
use std::collections::BTreeSet;

#[path = "client_report_v2_html.rs"]
mod html;
#[path = "client_report_v2_sql.rs"]
mod sql;
#[path = "client_report_v2_sql_advice.rs"]
mod sql_advice;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Plan {
    title: String,
    introduction: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    window_label: String,
    chapters: Vec<Chapter>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Chapter {
    id: String,
    title: String,
    question: String,
    conclusion: String,
    interpretation: String,
    limitation: String,
    owner: String,
    next_step: String,
    success_metric: String,
    briefs: Vec<BriefRef>,
    measurements: Vec<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BriefRef {
    brief_id: String,
    revision: u64,
}

fn kind(s: &Value) -> &str {
    s["kind"].as_str().unwrap_or("")
}

impl Plan {
    pub(crate) fn source_issues(&self, audit: &AuditState) -> Vec<String> {
        self.chapters
            .iter()
            .flat_map(|c| {
                c.briefs.iter().filter_map(|r| {
                    let current = audit.briefs.get(&r.brief_id).map(|b| b.revision);
                    (current != Some(r.revision)).then(|| {
                        format!(
                            "{}: {} @{} → {}",
                            c.id,
                            r.brief_id,
                            r.revision,
                            current.map_or_else(|| "—".into(), |n| n.to_string())
                        )
                    })
                })
            })
            .collect()
    }

    fn source_changes(&self, audit: &AuditState) -> Vec<Value> {
        self.chapters.iter().flat_map(|chapter|chapter.briefs.iter().filter_map(|source|{
            let current=audit.briefs.get(&source.brief_id);
            if current.map(|brief|brief.revision)==Some(source.revision){return None;}
            let previous=audit.history.iter().find(|b|b.brief_id==source.brief_id && b.revision==source.revision);
            let copy=|b:&AuditBrief|json!({"revision":b.revision,"conclusion":b.conclusion,"explanation":b.explanation,
                "next_step":b.next_step,"evidence_boundary":b.evidence_boundary});
            Some(json!({"chapter_id":chapter.id,"brief_id":source.brief_id,"pinned_revision":source.revision,
                "current_revision":current.map(|b|b.revision),"previous":previous.map(copy),"current":current.map(copy)}))
        })).collect()
    }

    fn fingerprint(&self) -> ToolResult<String> {
        Ok(digest(&serde_json::to_vec(self).map_err(io_error)?))
    }

    fn validate(&self, runtime: &AnalysisRuntime, session: &AnalysisSession) -> ToolResult<()> {
        public_text(&self.title, 320)?;
        public_text(&self.introduction, 4000)?;
        if !self.window_label.is_empty() {
            public_text(&self.window_label, 240)?;
        }
        if self.chapters.is_empty() || self.chapters.len() > 40 {
            return Err(invalid("Choose between one and forty problem chapters."));
        }
        let mut ids = HashSet::new();
        let mut labels = BTreeMap::new();
        for c in &self.chapters {
            if c.id.len() > 64
                || !c.id.starts_with(|b: char| b.is_ascii_lowercase())
                || !c
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                || !ids.insert(&c.id)
            {
                return Err(invalid("Choose distinct lowercase public chapter anchors."));
            }
            public_text(&c.id, 64)?;
            for t in [
                &c.title,
                &c.question,
                &c.conclusion,
                &c.interpretation,
                &c.limitation,
                &c.owner,
                &c.next_step,
                &c.success_metric,
            ] {
                public_text(t, 12000)?;
            }
            if c.briefs.is_empty()
                || c.briefs.len() > 128
                || c.measurements.is_empty()
                || c.measurements.len() > 64
            {
                return Err(invalid(
                    "Each problem needs scoped source briefs and measurements.",
                ));
            }
            let mut seen = HashSet::new();
            for r in &c.briefs {
                if !seen.insert(&r.brief_id) {
                    return Err(invalid("Repeated source brief in a chapter."));
                }
                brief(session, &r.brief_id, r.revision)?;
            }
            for s in &c.measurements {
                if kind(s) == "board_panel" {
                    selected_panel(session, c, s)?;
                } else {
                    let project = checked_project(runtime, session, c, s)?;
                    let label = s["instance_label"].as_str().unwrap_or("");
                    if labels
                        .insert(project.project_id.as_str(), label)
                        .is_some_and(|old| old != label)
                    {
                        return Err(invalid("Use a consistent public label for each instance."));
                    }
                    // Validate against the actual retained measurements before committing the plan.
                    if kind(s) == "sql_excerpt" {
                        sql_excerpt(
                            project,
                            s,
                            "",
                            "",
                            0,
                            ReportLanguage::new(&session.config.language),
                        )?;
                    } else {
                        sql::tables(project, s, ReportLanguage::new(&session.config.language))
                            .map_err(invalid)?;
                    }
                }
            }
        }
        let unique: HashSet<_> = labels.values().collect();
        if unique.len() != labels.len() {
            return Err(invalid(
                "Different instances must have different public labels.",
            ));
        }
        Ok(())
    }
}

fn checked_project<'a>(
    runtime: &'a AnalysisRuntime,
    session: &AnalysisSession,
    chapter: &Chapter,
    spec: &Value,
) -> ToolResult<&'a ProjectData> {
    let common = ["kind", "project_id", "instance_label", "label"];
    let extra: &[&str] = match kind(spec) {
        "sql_summary" => &["sql_id", "no_per_execution", "include_gets"],
        "sql_family" => &["module_prefix", "text_contains"],
        "sql_intervals" => &["sql_id", "begin_snap_ids"],
        "sql_excerpt" => &[
            "sql_id",
            "fragments",
            "meaning",
            "scaling",
            "verification",
            "recommendation",
        ],
        _ => return Err(invalid("Unknown client measurement kind.")),
    };
    let obj = spec
        .as_object()
        .ok_or_else(|| invalid("Expected a measurement selection."))?;
    if obj
        .keys()
        .any(|k| !common.contains(&k.as_str()) && !extra.contains(&k.as_str()))
    {
        return Err(invalid("Unknown SQL measurement field."));
    }
    for key in ["label", "instance_label"] {
        public_text(spec[key].as_str().unwrap_or(""), 320)?;
    }
    if kind(spec) == "sql_family" {
        for key in ["module_prefix", "text_contains"] {
            public_text(spec[key].as_str().unwrap_or(""), 400)?;
        }
    }
    for key in ["no_per_execution", "include_gets"] {
        if spec.get(key).is_some_and(|v| !v.is_boolean()) {
            return Err(invalid(format!("{key} must be true or false.")));
        }
    }
    if spec["include_gets"] == true && spec["no_per_execution"] == true {
        return Err(invalid(
            "Select separate TOP coverage or matched cost with logical reads.",
        ));
    }
    let project_id = spec["project_id"].as_str().unwrap_or("");
    if !session.project_ids.iter().any(|id| id == project_id)
        || !chapter.briefs.iter().any(|r| {
            session
                .audit
                .as_ref()
                .and_then(|a| a.briefs.get(&r.brief_id))
                .is_some_and(|b| b.project_id == project_id)
        })
    {
        return Err(invalid(
            "Link a current brief from each selected measurement project.",
        ));
    }
    if matches!(kind(spec), "sql_summary" | "sql_intervals" | "sql_excerpt") {
        let id = spec["sql_id"].as_str().unwrap_or("");
        if id.len() != 13
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err(invalid("Use a 13-character lowercase SQL_ID."));
        }
    }
    runtime
        .projects
        .get(project_id)
        .map(AsRef::as_ref)
        .ok_or_else(|| invalid("Selected project is unavailable."))
}

/// Resolve each selected passage against the captured statement, in source order.
/// Positions are one-based Unicode character offsets in the original capture.
fn matched_fragments(captured: &str, selection: &Value) -> ToolResult<Vec<html::SqlFragment>> {
    let excerpts = selection
        .as_array()
        .filter(|a| (1..=6).contains(&a.len()))
        .ok_or_else(|| invalid("Select one to six literal passages from this statement."))?;
    let mut end = 0;
    let mut character_offset = 0;
    let mut total = 0;
    let mut fragments = Vec::new();
    for fragment in excerpts {
        let passage = fragment
            .as_str()
            .ok_or_else(|| invalid("Each selected SQL passage must be text."))?;
        public_text(passage, 1800)?;
        total += passage.len();
        if total > 5400 {
            return Err(invalid("Select shorter passages covering the SQL finding."));
        }
        let offset = captured[end..].find(passage).ok_or_else(|| {
            invalid("A selected passage does not exactly match the captured SQL after the preceding passage.")
        })?;
        let start = end + offset;
        character_offset += captured[end..start].chars().count();
        end = start + passage.len();
        let text = &captured[start..end];
        let first = character_offset + 1;
        character_offset += text.chars().count();
        fragments.push(html::SqlFragment {
            text: text.to_owned(),
            first,
            last: character_offset,
        });
    }
    Ok(fragments)
}

fn sql_excerpt(
    project: &ProjectData,
    spec: &Value,
    id: &str,
    source_id: &str,
    after_tables: usize,
    lang: ReportLanguage,
) -> ToolResult<html::SqlExample> {
    let sql_id = spec["sql_id"].as_str().unwrap_or("");
    let captured = project.collection.sql_text.get(sql_id).ok_or_else(|| {
        invalid("This project's AWR capture has no SQL text for the selected statement.")
    })?;
    let copy = |key: &str| public_text(spec[key].as_str().unwrap_or(""), 2400);
    // Legacy plans keep verification in the checkpoint. The public rendering
    // uses the concrete recommendation, when one is supported by the SQL text.
    if let Some(verification) = spec.get("verification") {
        let text = verification
            .as_str()
            .ok_or_else(|| invalid("Verification must be text."))?;
        if !text.trim().is_empty() {
            public_text(text, 2400)?;
        }
    }
    let recommendation = spec.get("recommendation").map(|value| -> ToolResult<html::SqlRecommendation> {
        let candidate: html::SqlRecommendation = serde_json::from_value(value.clone()).map_err(|e| invalid(format!("Invalid SQL recommendation: {e}")))?;
        for value in [&candidate.rationale, &candidate.assumption, &candidate.before, &candidate.after] {
            public_text(value, 4000)?;
        }
        if !captured.contains(&candidate.before) || candidate.before.trim() == candidate.after.trim() {
            return Err(invalid("The before fragment must match this captured statement exactly and the proposed SQL must differ."));
        }
        Ok(candidate)
    }).transpose()?;
    let fragments = matched_fragments(captured, &spec["fragments"])?;
    let recommendation =
        recommendation.or_else(|| sql_advice::year_example(captured, &fragments, lang));
    Ok(html::SqlExample {
        id: id.to_owned(),
        source_id: source_id.to_owned(),
        after_tables,
        title: copy("label")?,
        instance: copy("instance_label")?,
        sql_id: sql_id.to_owned(),
        meaning: copy("meaning")?,
        scaling: copy("scaling")?,
        recommendation,
        fragments,
    })
}

fn selected_panel<'a>(
    session: &'a AnalysisSession,
    chapter: &Chapter,
    spec: &Value,
) -> ToolResult<(Value, Vec<String>, &'a Value)> {
    let fields = [
        "kind",
        "brief_id",
        "revision",
        "panel_index",
        "title",
        "public_note",
    ];
    if spec
        .as_object()
        .is_none_or(|o| o.keys().any(|k| !fields.contains(&k.as_str())))
    {
        return Err(invalid("Unknown board panel selection field."));
    }
    let id = spec["brief_id"].as_str().unwrap_or("");
    let rev = spec["revision"].as_u64().unwrap_or(0);
    if !chapter
        .briefs
        .iter()
        .any(|r| r.brief_id == id && r.revision == rev)
    {
        return Err(invalid("Select a panel from a cited brief revision."));
    }
    let b = brief(session, id, rev)?;
    let panel = spec["panel_index"]
        .as_u64()
        .and_then(|i| usize::try_from(i).ok())
        .and_then(|i| b.visual.as_ref()?.get("panels")?.get(i))
        .ok_or_else(|| invalid("The selected board panel is absent."))?;
    if panel["type"] != "table" || panel["classification"] != "measured" {
        return Err(invalid("Select a measured table from the saved board."));
    }
    let refs: BTreeSet<&str> = if let Some(list) = panel["evidence_refs"].as_array() {
        list.iter()
            .map(|v| v.as_str().ok_or_else(|| invalid("Invalid panel source.")))
            .collect::<ToolResult<_>>()?
    } else {
        panel["source"]
            .as_str()
            .unwrap_or("")
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .filter(|id| {
                id.strip_prefix("E-")
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
            })
            .collect()
    };
    if refs.is_empty()
        || refs.iter().any(|r| {
            !session.evidence.contains_key(*r)
                || !chapter.briefs.iter().any(|br| {
                    session
                        .audit
                        .as_ref()
                        .and_then(|a| a.briefs.get(&br.brief_id))
                        .is_some_and(|source| source.evidence_refs.iter().any(|id| id == r))
                })
        })
    {
        return Err(invalid(
            "Each selected panel source must be retained and cited by a linked brief.",
        ));
    }
    let title = public_text(
        spec["title"]
            .as_str()
            .or_else(|| panel["title"].as_str())
            .unwrap_or(""),
        400,
    )?;
    let note = spec["public_note"]
        .as_str()
        .or_else(|| panel["note"].as_str())
        .unwrap_or("");
    if !note.trim().is_empty() {
        public_text(note, 4000)?;
    }
    let columns = panel["columns"]
        .as_array()
        .ok_or_else(|| invalid("Source table has no column headers."))?;
    let rows = panel["rows"]
        .as_array()
        .ok_or_else(|| invalid("Source table has no rows."))?;
    if columns.is_empty() || rows.is_empty() {
        return Err(invalid("Source table must have rows and named columns."));
    }
    for col in columns {
        public_text(col.as_str().unwrap_or(""), 400)?;
    }
    for row in rows {
        let cells = row
            .as_array()
            .filter(|r| r.len() == columns.len())
            .ok_or_else(|| invalid("Source table row and header widths differ."))?;
        for cell in cells {
            match cell {
                Value::String(t) if !t.is_empty() => {
                    public_text(t, 4000)?;
                }
                Value::String(_) | Value::Null | Value::Number(_) => {}
                _ => return Err(invalid("Use scalar measured table cells.")),
            }
        }
    }
    Ok((
        json!({"title":title,"columns":columns,"rows":rows,"note":note}),
        refs.into_iter().map(str::to_owned).collect(),
        panel,
    ))
}

/// Map retained project provenance to public labels. Older boards expressed the
/// same scope in their title, in instance columns, or in column headings.
fn scope_panel(
    table: &mut Value,
    panel: &Value,
    labels: &BTreeMap<String, String>,
    evidence_instance: Option<&str>,
) -> Option<String> {
    let matches = |text: &str| -> BTreeSet<&str> {
        let text = text.to_ascii_lowercase();
        let word = |c: char| c.is_alphanumeric() || c == '_';
        labels
            .values()
            .filter(|label| {
                let name = label.to_ascii_lowercase();
                !name.is_empty()
                    && text.match_indices(&name).any(|(i, _)| {
                        !text[..i].chars().next_back().is_some_and(word)
                            && !text[i + name.len()..].chars().next().is_some_and(word)
                    })
            })
            .map(String::as_str)
            .collect()
    };
    fn unique<'a>(names: &BTreeSet<&'a str>) -> Option<&'a str> {
        names.first().copied().filter(|_| names.len() == 1)
    }
    let project_label = |value: &Value| labels.get(value.as_str()?).map(String::as_str);
    let columns = table["columns"].as_array().expect("validated columns");
    let column_scopes: Vec<_> = columns
        .iter()
        .map(|col| matches(col.as_str().unwrap()))
        .collect();
    let instance_column = columns
        .iter()
        .position(|col| html::is_instance_column(col.as_str().unwrap()));
    let mut mentioned = matches(table["title"].as_str().unwrap());
    mentioned.extend(matches(panel["title"].as_str().unwrap_or("")));
    for names in &column_scopes {
        mentioned.extend(names);
    }
    let rows: Vec<_> = table["rows"]
        .as_array()
        .expect("validated rows")
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut names = instance_column
                .and_then(|i| row[i].as_str())
                .map(&matches)
                .unwrap_or_default();
            let explicit = panel["row_project_ids"].get(index).and_then(&project_label);
            names.extend(explicit);
            mentioned.extend(&names);
            explicit.or_else(|| unique(&names))
        })
        .collect();
    if rows.iter().any(Option::is_some) {
        table["row_instances"] = json!(rows);
    }
    let columns: Vec<_> = column_scopes.iter().map(unique).collect();
    if columns.iter().any(Option::is_some) {
        table["column_instances"] = json!(columns);
    }
    if let Some(instance) = project_label(&panel["project_id"]) {
        mentioned.insert(instance);
    }
    match mentioned.len() {
        0 => evidence_instance.map(str::to_owned),
        1 => mentioned.first().map(|name| (*name).to_string()),
        _ => None,
    }
}

impl AnalysisRuntime {
    pub(crate) fn save_client_report_plan(&self, args: &Map<String, Value>) -> ToolResult<Value> {
        let raw = args
            .get("plan")
            .ok_or_else(|| invalid("Supply the full ordered report plan."))?;
        if serde_json::to_vec(raw).map_err(io_error)?.len() > 1024 * 1024 {
            return Err(invalid("Client plan exceeds 1 MiB."));
        }
        let plan: Plan = serde_json::from_value(raw.clone())
            .map_err(|e| invalid(format!("Invalid report plan: {e}")))?;
        self.edit_pilot_discussion("save_client_report_plan", args, |s, saved| {
            if saved.is_none_or(|s| s.pilot_kind != PilotKind::General) {
                return Err(invalid("Start or restore a durable general audit first."));
            }
            plan.validate(self, s)?;
            s.audit
                .as_mut()
                .ok_or_else(|| invalid("Guided audit required."))?
                .client_report_plan = Some(plan);
            Ok(None)
        })
    }
    pub(crate) fn client_report_plan(&self, args: &Map<String, Value>) -> ToolResult<Value> {
        let handle = self.session(Self::analysis_id(args)?)?;
        let s = handle
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
        let audit = s
            .audit
            .as_ref()
            .ok_or_else(|| invalid("Guided audit required."))?;
        let issues = audit
            .client_report_plan
            .as_ref()
            .map_or_else(Vec::new, |p| p.source_issues(audit));
        Ok(
            json!({"contract_version":2,"plan":audit.client_report_plan,"stale":!issues.is_empty(),"source_issues":issues,"source_changes":audit.client_report_plan.as_ref().map_or_else(Vec::new,|p|p.source_changes(audit))}),
        )
    }
}

pub(crate) type Discussion = BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)>;

/// Resolve every SQL page before rendering the narrative. Family membership comes
/// from the same measured rows published in the family detail, for this project.
fn source_inventory(
    runtime: &AnalysisRuntime,
    session: &AnalysisSession,
    plan: &Plan,
    labels: &BTreeMap<String, String>,
    mut discussion: Discussion,
    lang: ReportLanguage,
) -> ToolResult<BTreeMap<String, html::InstanceInventory>> {
    for chapter in &plan.chapters {
        for spec in &chapter.measurements {
            if kind(spec) == "board_panel" {
                continue;
            }
            let project = checked_project(runtime, session, chapter, spec)?;
            let id = project.project_id.as_str();
            let (ids, _) = discussion.entry(id.into()).or_default();
            if let Some(sql) = spec["sql_id"].as_str() {
                ids.insert(sql.into());
            } else if kind(spec) == "sql_family" {
                for table in sql::family_members(project, spec, lang).map_err(invalid)? {
                    for row in table["rows"].as_array().into_iter().flatten() {
                        if let Some(member) = row[0].as_str() {
                            ids.insert(member.into());
                        }
                    }
                }
            }
        }
    }
    Ok(public_inventory(session, discussion, labels))
}

fn sql_filename(ordinal: usize, sql_id: &str) -> String {
    format!("sql-p{ordinal}-{sql_id}.html")
}

fn wait_filename(ordinal: usize, name: &str) -> String {
    format!(
        "wait-p{ordinal}-{}.html",
        digest(name.to_ascii_lowercase().as_bytes())
    )
}

pub(crate) fn discussed_wait_path(
    session: &AnalysisSession,
    project_id: &str,
    name: &str,
) -> Option<String> {
    if name.trim().is_empty() {
        return None;
    }
    let ordinal = ordered_projects(session).position(|id| id == project_id)? + 1;
    Some(format!("wykresy/{}", wait_filename(ordinal, name)))
}

/// Stable public paths use the ordinal of the project in the complete analysis,
/// including projects with no discussed entities or measurements in this edition.
pub(crate) fn discussed_sql_path(
    session: &AnalysisSession,
    project_id: &str,
    sql_id: &str,
) -> Option<String> {
    if sql_id.len() != 13
        || !sql_id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return None;
    }
    let ordinal = ordered_projects(session).position(|id| id == project_id)? + 1;
    Some(format!("wykresy/{}", sql_filename(ordinal, sql_id)))
}

fn ordered_projects(session: &AnalysisSession) -> impl Iterator<Item = &String> {
    session
        .project_ids
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
}

fn public_inventory(
    session: &AnalysisSession,
    mut discussion: Discussion,
    labels: &BTreeMap<String, String>,
) -> BTreeMap<String, html::InstanceInventory> {
    let mut scopes = BTreeMap::new();
    for (index, project) in ordered_projects(session).enumerate() {
        let Some((sql_ids, waits)) = discussion.remove(project) else {
            continue;
        };
        if sql_ids.is_empty() && waits.is_empty() {
            continue;
        }
        scopes.insert(
            project.clone(),
            html::InstanceInventory {
                ordinal: index + 1,
                instance: labels[project].clone(),
                sql_ids,
                waits,
            },
        );
    }
    scopes
}

fn project_labels(
    session: &AnalysisSession,
    labels: &BTreeMap<String, String>,
    lang: ReportLanguage,
) -> BTreeMap<String, String> {
    let mut reserved: BTreeSet<_> = labels.values().cloned().collect();
    let mut next_label = 1;
    ordered_projects(session)
        .map(|project| {
            let label = labels.get(project).cloned().unwrap_or_else(|| {
                loop {
                    let candidate =
                        format!("{} {next_label}", lang.choose("Instance", "Instancja"));
                    next_label += 1;
                    if reserved.insert(candidate.clone()) {
                        break candidate;
                    }
                }
            });
            (project.clone(), label)
        })
        .collect()
}

fn source_intervals(
    project: &ProjectData,
    spec: &Value,
    summary: &[Value],
    lang: ReportLanguage,
) -> ToolResult<Vec<Value>> {
    match kind(spec) {
        "sql_family" => return sql::family_members(project, spec, lang).map_err(invalid),
        "sql_intervals" => return Ok(summary.to_vec()),
        _ => (),
    }
    let id = spec["sql_id"].as_str().expect("validated SQL selection");
    let snaps = project
        .collection
        .awrs
        .iter()
        .filter(|a| {
            a.sql_cpu_time.contains_key(id) || a.sql_elapsed_time.iter().any(|row| row.sql_id == id)
        })
        .map(|a| a.snap_info.begin_snap_id)
        .collect::<Vec<_>>();
    if snaps.is_empty() {
        return Ok(summary.to_vec());
    }
    let mut table = None;
    for ids in snaps.chunks(64) {
        let mut part = spec.clone();
        part["kind"] = json!("sql_intervals");
        part["begin_snap_ids"] = json!(ids);
        let mut values = sql::tables(project, &part, lang).map_err(invalid)?;
        let mut incoming = values
            .pop()
            .ok_or_else(|| invalid("Missing SQL interval source."))?;
        if let Some(o) = incoming.as_object_mut() {
            o.remove("plot");
        }
        match table {
            Some(ref mut target) => {
                let target: &mut Value = target;
                target["rows"]
                    .as_array_mut()
                    .expect("normalized interval table")
                    .extend(
                        incoming["rows"]
                            .as_array()
                            .expect("normalized interval table")
                            .iter()
                            .cloned(),
                    );
            }
            None => table = Some(incoming),
        }
    }
    Ok(table.into_iter().collect())
}

fn add_tables(
    tables: &mut Vec<Value>,
    incoming: Vec<Value>,
    source: &str,
    anchor: &str,
    allow_merge: bool,
) -> String {
    let mut first = None;
    for (index, mut table) in incoming.into_iter().enumerate() {
        let id = format!("{anchor}-{}", index + 1);
        let group = table.get("group").and_then(Value::as_str);
        // Only explicitly compatible measurements can share a table. A rank in
        // one TOP family and an intersection of two TOP families stay distinct.
        let merge = tables.last_mut().filter(|prior| {
            allow_merge
                && group.is_some()
                && prior["group"] == table["group"]
                && (group != Some("sql_family") || prior["note"] == table["note"])
                && prior["columns"] == table["columns"]
                && prior["title"] == table["title"]
                && prior["description"] == table["description"]
        });
        let rows = table["rows"].as_array().map_or(0, Vec::len);
        let row_sources = vec![json!([source]); rows];
        if let Some(prior) = merge {
            first.get_or_insert_with(|| prior["id"].as_str().unwrap().to_string());
            if prior.get("row_source_ids").is_none() {
                prior["row_source_ids"] = json!(vec![
                    prior["source_ids"].clone();
                    prior["rows"].as_array().unwrap().len()
                ]);
            }
            prior["rows"]
                .as_array_mut()
                .expect("normalized table")
                .extend(
                    table["rows"]
                        .as_array()
                        .expect("normalized table")
                        .iter()
                        .cloned(),
                );
            prior["row_source_ids"]
                .as_array_mut()
                .unwrap()
                .extend(row_sources);
            prior["source_ids"]
                .as_array_mut()
                .unwrap()
                .push(json!(source));
            if let Some(note) = table["note"].as_str().filter(|s| !s.is_empty()) {
                let old = prior["note"].as_str().unwrap_or("");
                if !old.lines().any(|l| l == note) {
                    prior["note"] = json!(if old.is_empty() {
                        note.to_owned()
                    } else {
                        format!("{old}\n{note}")
                    });
                }
            }
        } else {
            first.get_or_insert_with(|| id.clone());
            table["id"] = json!(id);
            table["source_ids"] = json!([source]);

            tables.push(table);
        }
    }
    first.unwrap_or_else(|| anchor.to_string())
}

type Charts<'a> = BTreeMap<&'a str, BTreeMap<String, native_charts::NativeChart>>;

fn entity_assets<'a>(
    runtime: &AnalysisRuntime,
    inventory: &'a BTreeMap<String, html::InstanceInventory>,
    lang: ReportLanguage,
    entry_file: &str,
) -> ToolResult<(Files, Charts<'a>)> {
    let mut charts = BTreeMap::new();
    for (project, scope) in inventory {
        if !scope.sql_ids.is_empty() {
            charts.insert(
                project.as_str(),
                native_charts::client_sql_charts(
                    runtime,
                    project,
                    &scope.instance,
                    &scope.sql_ids,
                )?,
            );
        }
    }
    let mut files = Files::new();
    let mut used_plotly = false;
    let lang_code = lang.choose("en", "pl");
    let inventory_title = lang.choose(
        "SQL statements and wait events discussed",
        "Omówione zapytania SQL i zdarzenia oczekiwania",
    );
    for (project_id, scope) in inventory {
        let project = runtime
            .projects
            .get(project_id)
            .ok_or_else(|| invalid("Discussion project is unavailable."))?;
        if !scope.waits.is_empty() {
            let wait_charts = native_charts::client_wait_charts(
                runtime,
                project_id,
                &scope.instance,
                &scope.waits,
            )?;
            for event in &scope.waits {
                let originals = wait_charts
                    .get(event)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                used_plotly |= !originals.is_empty();
                let period = originals
                    .first()
                    .map(|c| format!("{} · {} – {}", scope.instance, c.begin, c.end))
                    .unwrap_or_else(|| scope.instance.clone());
                let page = html::source_wait_page(
                    &format!("{} · {}", scope.instance, event),
                    &period,
                    originals,
                    inventory_title,
                    lang_code,
                    entry_file,
                )
                .map_err(invalid)?;
                files.insert(
                    format!("wykresy/{}", wait_filename(scope.ordinal, event)),
                    page.into_bytes(),
                );
            }
        }
        for id in &scope.sql_ids {
            let chart = charts
                .get(project_id.as_str())
                .and_then(|items| items.get(id));
            let plot = chart.map(native_charts::plot_markup);
            used_plotly |= plot.is_some();
            let detail = html::SqlDetail {
                plot: plot.as_deref(),
                reconstructed: chart.is_some_and(|c| c.kind == "sql_awr"),
                text: project.collection.sql_text.get(id).map(String::as_str),
            };
            let period = chart
                .map(|c| format!("{} · {} – {}", scope.instance, c.begin, c.end))
                .unwrap_or_else(|| scope.instance.clone());
            let page = html::source_chart_page(
                &format!("{} · SQL {id}", scope.instance),
                &period,
                &detail,
                inventory_title,
                html::INVENTORY_ANCHOR,
                lang_code,
                entry_file,
            )
            .map_err(invalid)?;
            files.insert(format!("wykresy/{}", scope.sql_file(id)), page.into_bytes());
        }
    }
    if used_plotly {
        files.insert(
            "wykresy/jasmin-plot.js".into(),
            native_charts::VIEWER_JS.to_vec(),
        );
        files.insert(
            "wykresy/plotly.min.js".into(),
            native_charts::plotly_js().to_vec(),
        );
    }
    Ok((files, charts))
}

/// Build the same offline SQL cards for report formats with a separate index.
/// Explicit instance labels constrain the projection to their projects.
pub(crate) fn discussed_assets(
    runtime: &AnalysisRuntime,
    session: &AnalysisSession,
    labels: &BTreeMap<String, String>,
    lang: ReportLanguage,
) -> ToolResult<Files> {
    let mut discussion = runtime.discussed_entities(session);
    discussion.retain(|project, _| labels.is_empty() || labels.contains_key(project));
    let inventory = public_inventory(session, discussion, &project_labels(session, labels, lang));
    if inventory.is_empty() {
        return Ok(Files::new());
    }
    let (mut files, _) = entity_assets(runtime, &inventory, lang, "discussion.html")?;
    let index = html::render(&json!({
        "language": lang.choose("en", "pl"), "title": lang.choose("Audit index", "Indeks audytu"),
        "chapters": [], "inventory": inventory.values().collect::<Vec<_>>()
    }))
    .map_err(invalid)?;
    files.insert("discussion.html".into(), index.into_bytes());
    Ok(files)
}

impl Plan {
    fn render(
        &self,
        runtime: &AnalysisRuntime,
        session: &AnalysisSession,
        lang: ReportLanguage,
        discussion: Discussion,
    ) -> ToolResult<(Files, Value)> {
        self.validate(runtime, session)?;
        let explicit_labels: BTreeMap<_, _> = self
            .chapters
            .iter()
            .flat_map(|c| &c.measurements)
            .filter_map(|s| {
                Some((
                    s["project_id"].as_str()?.to_owned(),
                    s["instance_label"].as_str()?.to_owned(),
                ))
            })
            .collect();
        let labels = project_labels(session, &explicit_labels, lang);
        let inventory = source_inventory(runtime, session, self, &labels, discussion, lang)?;
        let (mut files, charts) = entity_assets(runtime, &inventory, lang, "raport.html")?;
        let mut chapters = Vec::new();
        let mut evidence = Vec::new();
        for (cindex, c) in self.chapters.iter().enumerate() {
            let mut tables = Vec::new();
            let mut sources = Vec::new();
            let mut examples: Vec<html::SqlExample> = Vec::new();
            for (mi, spec) in c.measurements.iter().enumerate() {
                let slot = format!("{}-{}", cindex + 1, mi + 1);
                let anchor = format!("measurement-{slot}");
                let source_id = format!("source-{slot}");
                let src_project = spec["project_id"].as_str().unwrap_or("");
                let mut instance = labels.get(src_project).cloned();
                let id = spec["sql_id"].as_str();
                let chart = id.and_then(|id| charts.get(src_project)?.get(id));
                let plot = chart.map(native_charts::plot_markup);
                let detail = html::SqlDetail {
                    plot: plot.as_deref(),
                    reconstructed: chart.is_some_and(|c| c.kind == "sql_awr"),
                    text: id
                        .and_then(|id| {
                            runtime
                                .projects
                                .get(src_project)?
                                .collection
                                .sql_text
                                .get(id)
                        })
                        .map(String::as_str),
                };
                if kind(spec) == "sql_excerpt" {
                    let project = checked_project(runtime, session, c, spec)?;
                    let example =
                        sql_excerpt(project, spec, &anchor, &source_id, tables.len(), lang)?;
                    let path = format!("wykresy/sql-{slot}.html");
                    let page = html::source_excerpt_page(
                        &example,
                        &detail,
                        &c.title,
                        lang.choose("en", "pl"),
                    )
                    .map_err(invalid)?;
                    files.insert(path.clone(), page.into_bytes());
                    sources.push(json!({"id":source_id,"title":format!("{} · SQL {}",example.instance,example.sql_id),
                        "href":path,"instance":example.instance,"description":lang.choose("Selected passages in the captured SQL text.","Wybrane fragmenty i ich pozycje w źródłowym tekście zapytania.")}));
                    examples.push(example);
                    continue;
                }
                let mut href = None;
                let native = chart;
                let mut fallback = None;
                let description;
                let title;
                let input_tables;
                if kind(spec) == "board_panel" {
                    let (mut t, refs, panel) = selected_panel(session, c, spec)?;
                    let evidence_scopes: BTreeSet<_> = refs
                        .iter()
                        .map(|id| {
                            session.evidence[id]
                                .project_id
                                .as_ref()
                                .and_then(|id| labels.get(id))
                                .map(String::as_str)
                        })
                        .collect();
                    let evidence_instance = evidence_scopes
                        .first()
                        .copied()
                        .flatten()
                        .filter(|_| evidence_scopes.len() == 1);
                    instance = scope_panel(&mut t, panel, &labels, evidence_instance);
                    title = t["title"].as_str().unwrap().to_owned();
                    description = lang.choose(
                        "Selected measured table with original columns and units.",
                        "Wybrana tabela pomiarowa z oryginalnymi kolumnami i jednostkami.",
                    );
                    evidence.push(json!({"chapter":c.id,"measurement":mi,"evidence_refs":refs}));
                    input_tables = vec![t];
                } else {
                    let project = checked_project(runtime, session, c, spec)?;
                    input_tables = sql::tables(project, spec, lang).map_err(invalid)?;
                    if input_tables.is_empty() {
                        return Err(invalid("SQL selection did not produce a source table."));
                    }
                    let label = spec["instance_label"].as_str().unwrap();
                    title = if let Some(id) = spec["sql_id"].as_str() {
                        format!("{label} · SQL {id}")
                    } else {
                        format!("{label} · {}", spec["label"].as_str().unwrap())
                    };
                    description = if detail.reconstructed {
                        lang.choose(
                            "JAS-MIN chart from the retained AWR intervals for this instance.",
                            "Wykres JAS-MIN z zachowanych interwałów AWR tej instancji.",
                        )
                    } else if native.is_some() {
                        lang.choose(
                            "Original JAS-MIN SQL figure for this instance, full captured period.",
                            "Oryginalny wykres SQL JAS-MIN tej instancji z całego badanego okresu.",
                        )
                    } else if spec["sql_id"].is_string() {
                        lang.choose("The original figure is unavailable for this instance. The AWR table gives the retained measurement.","Oryginalny wykres dla tej instancji jest niedostępny. Tabela AWR zawiera pomiar z wybranej próby.")
                    } else {
                        lang.choose(
                            "Source AWR TOP lists for the selected instance and SQL population.",
                            "Źródłowe listy TOP AWR dla wskazanej instancji i rodziny SQL.",
                        )
                    };
                }
                if kind(spec) != "board_panel" && native.is_none() {
                    let project = checked_project(runtime, session, c, spec)?;
                    let details = source_intervals(project, spec, &input_tables, lang)?;
                    let captures = &project.collection.awrs;
                    let range = captures
                        .first()
                        .zip(captures.last())
                        .map(|(a, b)| {
                            format!(
                                "{} – {}",
                                a.snap_info.begin_snap_time, b.snap_info.end_snap_time
                            )
                        })
                        .unwrap_or_default();
                    fallback = Some((
                        details,
                        format!(
                            "{} · {} · {range}",
                            spec["instance_label"].as_str().unwrap(),
                            lang.choose("AWR capture", "Okres AWR")
                        ),
                    ));
                }
                let allow_merge = examples
                    .last()
                    .is_none_or(|example| example.after_tables != tables.len());
                let target =
                    add_tables(&mut tables, input_tables, &source_id, &anchor, allow_merge);
                let lang_code = lang.choose("en", "pl");
                let page = if let Some(chart) = native {
                    let scope = format!("{} – {}", chart.begin, chart.end);
                    Some(
                        html::source_chart_page(
                            &title,
                            &scope,
                            &detail,
                            &c.title,
                            &target,
                            lang_code,
                            "raport.html",
                        )
                        .map_err(invalid)?,
                    )
                } else if let Some((details, scope)) = fallback {
                    Some(
                        html::source_table_page(
                            &title,
                            &scope,
                            &details,
                            inventory.get(src_project),
                            id.map(|_| &detail),
                            &c.title,
                            &target,
                            lang_code,
                        )
                        .map_err(invalid)?,
                    )
                } else {
                    None
                };
                if let Some(bytes) = page {
                    let path = format!("wykresy/sql-{slot}.html");
                    files.insert(path.clone(), bytes.into_bytes());
                    href = Some(path);
                }
                sources.push(
                    json!({"id":source_id,"title":title,"href":href,"description":description,"instance":instance}),
                );
            }
            let chapter_projects: BTreeSet<_> = c
                .briefs
                .iter()
                .map(|r| brief(session, &r.brief_id, r.revision).map(|b| &b.project_id))
                .collect::<ToolResult<_>>()?;
            let chapter_instance = if chapter_projects.len() == 1 {
                chapter_projects.first().and_then(|p| labels.get(*p))
            } else {
                None
            };
            chapters.push(json!({"instance":chapter_instance,"id":c.id,"title":c.title,"question":c.question,"conclusion":c.conclusion,"paragraphs":[c.interpretation],"tables":tables,"sql_examples":examples,"sources":sources,"limitation":c.limitation,"owner":c.owner,"action":c.next_step,"success":c.success_metric}));
        }
        let openings = runtime.gradient_openings(session, &labels, lang);
        let document = json!({"language":lang.choose("en","pl"),"gradient_intro":openings,"title":self.title,"summary":self.introduction,"period":self.window_label,"chapters":chapters,"inventory":inventory.values().collect::<Vec<_>>()});
        let output = html::render(&document).map_err(invalid)?;
        files.insert("raport.html".into(), output.into_bytes());
        let count = self.chapters.len();
        Ok((
            files,
            json!({"contract_version":2,"entry_file":"raport.html","chapter_count":count,"public_report":document,"panel_sources":evidence}),
        ))
    }
}

fn discussion_fingerprint(
    session: &AnalysisSession,
    discussion: &Discussion,
) -> ToolResult<String> {
    let opening: BTreeMap<_, _> = ordered_projects(session)
        .map(|id| {
            (
                id,
                super::super::gradient_intro::basis(session.audit.as_ref(), id),
            )
        })
        .collect();
    Ok(digest(
        &serde_json::to_vec(&(discussion, opening)).map_err(io_error)?,
    ))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Edition {
    analysis_id: String,
    plan_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    discussion_sha256: Option<String>,
}

impl PublicationRecipe for Edition {
    const ZIP_FILENAME: &'static str = "raport.zip";
    const ENTRY_FILE: &'static str = "raport.html";
    const FROZEN_ASSETS: bool = true;
    fn analysis_id(&self) -> &str {
        &self.analysis_id
    }
    fn validate_sources(&self, session: &AnalysisSession) -> ToolResult<()> {
        let a = session
            .audit
            .as_ref()
            .ok_or_else(|| invalid("Guided audit required."))?;
        let plan = a
            .client_report_plan
            .as_ref()
            .ok_or_else(|| invalid("Save a client report plan first."))?;
        if self.plan_sha256 != plan.fingerprint()? || !plan.source_issues(a).is_empty() {
            return Err(tool_error(
                "CLIENT_PREVIEW_STALE",
                "The plan or its source briefs have changed. Review the current revisions and prepare a new preview.",
            ));
        }
        Ok(())
    }
    fn render(
        &self,
        session: &AnalysisSession,
        lang: ReportLanguage,
        runtime: Option<&AnalysisRuntime>,
    ) -> ToolResult<(Files, Value)> {
        self.validate_sources(session)?;
        let runtime =
            runtime.ok_or_else(|| invalid("The original project captures are required."))?;
        let discussion = runtime.discussed_entities(session);
        let current = discussion_fingerprint(session, &discussion)?;
        if self.discussion_sha256.as_ref() != Some(&current) {
            return Err(tool_error(
                "CLIENT_PREVIEW_STALE",
                "Prepare a new preview to include all discussed SQL statements and wait events.",
            ));
        }
        session
            .audit
            .as_ref()
            .unwrap()
            .client_report_plan
            .as_ref()
            .unwrap()
            .render(runtime, session, lang, discussion)
    }
}

pub(super) fn edition(
    name: &str,
    args: &Map<String, Value>,
    session: &AnalysisSession,
    workspace: &Path,
    audit_id: &str,
    runtime: Option<&AnalysisRuntime>,
) -> ToolResult<Value> {
    let mut selected = args.clone();
    // The outer MCP router authenticated the active analysis and verified the
    // latest on-disk revision. Bind the private edition to the durable audit so
    // a restart can reopen unchanged evidence and the identical preview.
    selected.insert("analysis_id".into(), json!(audit_id));
    if name == "preview_client_report" {
        let plan = session
            .audit
            .as_ref()
            .and_then(|a| a.client_report_plan.as_ref())
            .ok_or_else(|| invalid("Save a client report plan first."))?;
        selected.insert("plan_sha256".into(), json!(plan.fingerprint()?));
        let runtime =
            runtime.ok_or_else(|| invalid("The original project captures are required."))?;
        selected.insert(
            "discussion_sha256".into(),
            json!(discussion_fingerprint(
                session,
                &runtime.discussed_entities(session)
            )?),
        );
    }
    run_edition::<Edition>(name, &selected, session, workspace, audit_id, runtime)
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

pub(super) fn tools() -> Vec<Value> {
    let text = json!({"type":"string","minLength":1});
    let rev = json!({"type":"integer","minimum":1});
    let number = json!({"type":"integer","minimum":0});
    let reference = object(
        json!({"brief_id":text,"revision":rev}),
        &["brief_id", "revision"],
    );
    let measurement = |name: &str, fields: Value, required: &[&str]| {
        let mut fields = fields.as_object().unwrap().clone();
        fields.insert("kind".into(), json!({"const":name,"type":"string"}));
        let mut req = vec!["kind"];
        req.extend_from_slice(required);
        object(Value::Object(fields), &req)
    };
    let mut common = json!({"project_id":text,"label":text,"instance_label":text});
    let mut sum = common.clone();
    sum["sql_id"] = text.clone();
    sum["no_per_execution"] = json!({"type":"boolean"});
    sum["include_gets"] = json!({"type":"boolean","description":"Include logical reads on the common elapsed, CPU and buffer gets population."});
    let mut intervals = common.clone();
    intervals["sql_id"] = text.clone();
    intervals["begin_snap_ids"] = json!({"type":"array","items":number,"minItems":1});
    let mut excerpt = common.clone();
    for field in ["sql_id", "meaning", "scaling"] {
        excerpt[field] = text.clone();
    }
    excerpt["verification"] = json!({"type":"string","description":"Legacy plan input."});
    excerpt["recommendation"] = object(
        json!({"rationale":{"type":"string","description":"SQL construct to change and the specific work it may avoid."},
        "assumption":{"type":"string","description":"Conditions required to preserve results and make the change applicable."},
        "before":{"type":"string","description":"Exact contiguous fragment of this captured SQL."},
        "after":{"type":"string","description":"Illustrative replacement for that fragment; not an observed statement."}}),
        &["rationale", "assumption", "before", "after"],
    );
    excerpt["fragments"] = json!({"type":"array","items":{"type":"string","minLength":1,"maxLength":1800},"minItems":1,"maxItems":6,"description":"Exact contiguous substrings of this captured SQL, in source order."});
    common["module_prefix"] = text.clone();
    common["text_contains"] = text.clone();
    let specs = json!({"oneOf":[
        measurement("sql_summary",sum,&["project_id","instance_label","sql_id","label"]),
        measurement("sql_excerpt",excerpt,&["project_id","instance_label","sql_id","label","fragments","meaning","scaling"]),
        measurement("sql_family",common,&["project_id","instance_label","module_prefix","text_contains","label"]),
        measurement("sql_intervals",intervals,&["project_id","instance_label","sql_id","begin_snap_ids","label"]),
        measurement("board_panel",json!({"brief_id":text,"revision":rev,"panel_index":number,"title":text,"public_note":{"type":"string"}}),&["brief_id","revision","panel_index"])
    ]});
    let chap = object(
        json!({"id":text,"title":text,"question":text,"conclusion":text,"interpretation":text,"limitation":text,"owner":text,"next_step":text,"success_metric":text,"briefs":{"type":"array","minItems":1,"items":reference},"measurements":{"type":"array","minItems":1,"items":specs}}),
        &[
            "id",
            "title",
            "question",
            "conclusion",
            "interpretation",
            "limitation",
            "owner",
            "next_step",
            "success_metric",
            "briefs",
            "measurements",
        ],
    );
    let plan = object(
        json!({"title":text,"introduction":text,"window_label":text,"chapters":{"type":"array","items":chap,"minItems":1}}),
        &["title", "introduction", "chapters"],
    );
    vec![
        function_definition(
            "save_client_report_plan",
            "Save an ordered set of client problems to the guided audit checkpoint. Pin the current source brief revisions and select each measured table or AWR measurement. Existing approved editions remain recoverable.",
            object(
                json!({"plan":plan,"expected_revision":rev,"operation_id":text}),
                &["plan"],
            ),
        ),
        function_definition(
            "get_client_report_plan",
            "Get the private saved plan and any stale brief references.",
            object(json!({}), &[]),
        ),
        function_definition(
            "preview_client_report",
            "Build a local, offline ORA-600 client report from the saved plan and original data. A preview does not publish a ZIP.",
            object(json!({}), &[]),
        ),
        function_definition(
            "read_client_report_asset",
            "Read a verified relative file from the reviewed preview. Its ZIP becomes available only after explicit publication.",
            object(
                json!({"preview_id":text,"asset":text,"offset":number}),
                &["preview_id", "asset"],
            ),
        ),
        function_definition(
            "publish_client_report",
            "Issue the exact reviewed preview as an offline ZIP only when the user explicitly requests the final client edition.",
            object(
                json!({"preview_id":text,"user_request":text}),
                &["preview_id", "user_request"],
            ),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn open(root: &Path) -> AnalysisRuntime {
        let mut runtime = super::super::super::tests::runtime();
        runtime.audit_workspace = root.into();
        runtime
    }
    fn call(r: &AnalysisRuntime, t: &str, p: Value) -> ToolResult<Value> {
        r.call_tool(t, p.as_object().unwrap().clone())
    }
    #[test]
    fn board_scope_uses_retained_labels_and_keeps_comparisons_scoped_per_axis() {
        let labels = BTreeMap::from([
            ("project-a".into(), "Node 1".into()),
            ("project-b".into(), "Node 10".into()),
        ]);
        let table = json!({"title":"Peak","columns":["Metric","Value"],"rows":[["wait",3]]});
        let single =
            |panel: Value, evidence| scope_panel(&mut table.clone(), &panel, &labels, evidence);
        assert_eq!(
            single(json!({"project_id":"project-a"}), None).as_deref(),
            Some("Node 1")
        );
        assert_eq!(
            single(json!({"title":"node 10 · measurements"}), None).as_deref(),
            Some("Node 10")
        );
        assert_eq!(single(json!({}), Some("Node 1")).as_deref(), Some("Node 1"));
        assert_eq!(
            single(
                json!({"title":"Changes between Node 1 and Node 10"}),
                Some("Node 1")
            ),
            None
        );

        let mut by_column =
            json!({"title":"Peak","columns":["Event","Node 1","Node 10"],"rows":[["wait",2,3]]});
        assert_eq!(scope_panel(&mut by_column, &json!({}), &labels, None), None);
        assert_eq!(
            by_column["column_instances"],
            json!([null, "Node 1", "Node 10"])
        );

        let mut by_row = json!({"title":"Peak","columns":["Instance","SQL_ID"],"rows":[["node 10","x"],["node 1","x"]]});
        assert_eq!(scope_panel(&mut by_row, &json!({}), &labels, None), None);
        assert_eq!(by_row["row_instances"], json!(["Node 10", "Node 1"]));
        by_row["columns"][0] = json!("Local alias");
        assert_eq!(
            scope_panel(
                &mut by_row,
                &json!({"project_id":"project-b","row_project_ids":["project-a","project-b"]}),
                &labels,
                None
            ),
            None
        );
        assert_eq!(by_row["row_instances"], json!(["Node 1", "Node 10"]));
    }

    #[test]
    fn verified_excerpts_link_to_full_escaped_sql_in_both_source_views() {
        use std::sync::Arc;
        let root = tempfile::tempdir().unwrap();
        let mut r = open(root.path());
        let project = Arc::make_mut(&mut r.projects).values_mut().next().unwrap();
        let project = Arc::make_mut(project);
        let sql_id = "00000000abcde";
        let original = "select 'ł' from order_lines where price < :limit and trunc(placed_at) = :day and unselected_column = 9";
        Arc::make_mut(&mut project.collection)
            .sql_text
            .insert(sql_id.into(), original.into());
        let project = project.project_id.to_string();
        let id = call(&r, "start_performance_analysis", json!({})).unwrap()["analysis_id"].clone();
        call(&r, "start_audit", json!({"analysis_id":id})).unwrap();
        let evidence = call(
            &r,
            "get_database_load_summary",
            json!({"analysis_id":id,"project_id":project}),
        )
        .unwrap()["evidence_id"]
            .clone();
        let brief = json!({"analysis_id":id,"project_id":project,"chapter_id":"sql","brief_id":"sample-predicate","question":"What does the filter select?",
            "explanation":"It applies an upper bound.","conclusion":"Check the rows in the range.","evidence_boundary":"Plan rows remain unknown.",
            "next_step":"Measure comparable binds.","evidence_refs":[evidence]});
        call(&r, "record_audit_brief", brief).unwrap();
        let spec = json!({"kind":"sql_excerpt","instance_label":"Instance 1","project_id":project,"sql_id":sql_id,"label":"Range predicate",
            "fragments":["from order_lines","price < :limit"],"meaning":"The condition keeps lower prices.",
            "scaling":"The candidate set grows with the matching population.","verification":"Compare result counts for equal binds."});
        let mut plan = json!({"title":"Range analysis","introduction":"Inspect the range used by the batch.","chapters":[{"id":"range","title":"Which rows qualify?",
            "question":"Which rows qualify?","conclusion":"Check the retained range.","interpretation":"The filter affects the eligible population.",
            "limitation":"Row counts require an execution.","owner":"Application owner","next_step":"Capture rows.","success_metric":"Rows inspected for the same result.",
            "briefs":[{"brief_id":"sample-predicate","revision":1}],"measurements":[spec]}]});
        for invalid_fragments in [
            json!(["PRICE < :limit"]),
            json!(["price < :limit", "from order_lines"]),
            json!([""]),
        ] {
            let mut bad = plan.clone();
            bad["chapters"][0]["measurements"][0]["fragments"] = invalid_fragments;
            assert!(
                call(
                    &r,
                    "save_client_report_plan",
                    json!({"analysis_id":id,"plan":bad})
                )
                .is_err()
            );
        }
        let recommendation = json!({"rationale":"A half-open range exposes the date column directly to an index range scan.", "assumption":"The column is a DATE and the day bind is midnight in the same calendar.",
            "before":"trunc(placed_at) = :day","after":"placed_at >= :day and placed_at < :day + 1"});
        let mut invalid_example = plan.clone();
        invalid_example["chapters"][0]["measurements"][0]["recommendation"] =
            recommendation.clone();
        invalid_example["chapters"][0]["measurements"][0]["recommendation"]["before"] =
            json!("invented source fragment");
        assert!(
            call(
                &r,
                "save_client_report_plan",
                json!({"analysis_id":id,"plan":invalid_example})
            )
            .is_err()
        );
        plan["chapters"][0]["measurements"][0]["recommendation"] = recommendation;
        call(
            &r,
            "save_client_report_plan",
            json!({"analysis_id":id,"plan":plan}),
        )
        .unwrap();
        let preview = call(&r, "preview_client_report", json!({"analysis_id":id})).unwrap();
        let main = PathBuf::from(preview["index_path"].as_str().unwrap());
        let doc = fs::read_to_string(&main).unwrap();
        let cited =
            fs::read_to_string(main.parent().unwrap().join("wykresy/sql-1-1.html")).unwrap();
        let char_pos = original[..original.find("price").unwrap()].chars().count() + 1;
        let example = &preview["public_report"]["chapters"][0]["sql_examples"][0];
        assert_eq!(example["fragments"][1]["first"], char_pos);
        assert!(doc.contains("price &lt; :limit") && !doc.contains("unselected_column"));
        assert!(
            doc.contains("placed_at &lt; :day + 1")
                && !doc.contains("Compare result counts for equal binds.")
        );
        let canonical = format!("wykresy/sql-p1-{sql_id}.html");
        assert!(doc.contains(&canonical));
        for page in [
            cited,
            fs::read_to_string(main.parent().unwrap().join(canonical)).unwrap(),
        ] {
            assert!(page.contains(&*html_escape::encode_text(original)));
        }
        plan["chapters"][0]["measurements"][0]["sql_id"] = json!("00000000abcdf");
        assert!(
            call(
                &r,
                "save_client_report_plan",
                json!({"analysis_id":id,"plan":plan})
            )
            .is_err()
        );
    }

    #[test]
    fn checkpoint_preserves_selected_panel_units_and_reviewed_public_bytes() {
        let root = tempfile::tempdir().unwrap();
        let r = open(root.path());
        let id = call(&r, "start_performance_analysis", json!({})).unwrap()["analysis_id"].clone();
        let start = call(&r, "start_audit", json!({"analysis_id":id})).unwrap();
        let project = start["project_ids"][0].clone();
        let evidence = call(
            &r,
            "get_database_load_summary",
            json!({"analysis_id":id,"project_id":project}),
        )
        .unwrap()["evidence_id"]
            .clone();
        assert!(evidence.is_string());
        let fields = json!({"analysis_id":id,"project_id":project,"chapter_id":"sql","brief_id":"sample-sql","question":"How many calls were recorded?",
            "explanation":"The session recorded the sampled interval.","conclusion":"Use a comparable interval.","evidence_boundary":"The sample is limited to one interval.","next_step":"Compare the next interval.","evidence_refs":[evidence],
            "visual":{"title":"Observed sample","panels":[{"title":"Calls per interval","type":"table","classification":"measured","source":evidence,"columns":["Calls [count]","Sample coverage [h]"],"rows":[[10,1]]}]}});
        let saved = call(&r, "record_audit_brief", fields.clone()).unwrap();
        let plan = json!({"title":"Workload review","introduction":"Inspect the sampled request.","chapters":[{"id":"requests","title":"Frequent requests","question":"How often do requests run?","conclusion":"Ten calls occurred in the captured interval.","interpretation":"Measure work for a comparable batch.","limitation":"One interval was sampled.","owner":"Application owner","next_step":"Measure the next batch.","success_metric":"Calls per completed batch.","briefs":[{"brief_id":"sample-sql","revision":1}],"measurements":[{"kind":"board_panel","brief_id":"sample-sql","revision":1,"panel_index":0}]}]});
        let save_args = json!({"analysis_id":id,"plan":plan,"expected_revision":saved["snapshot"]["revision"],"operation_id":"draft-v2"});
        let planned = call(&r, "save_client_report_plan", save_args.clone()).unwrap();
        assert_eq!(
            planned["snapshot"]["revision"].as_u64(),
            saved["snapshot"]["revision"].as_u64().map(|i| i + 1)
        );
        assert_eq!(
            call(&r, "save_client_report_plan", save_args).unwrap()["snapshot"],
            planned["snapshot"]
        );
        let preview = call(&r, "preview_client_report", json!({"analysis_id":id})).unwrap();
        assert_eq!(preview["files"], json!(["raport.html"]));
        let content = fs::read_to_string(preview["index_path"].as_str().unwrap()).unwrap();
        assert!(content.contains("Sample coverage [h]"));
        assert!(!content.contains(evidence.as_str().unwrap()));
        assert!(!root.path().join("wydania").exists());
        let restart = open(root.path());
        let resumed = call(
            &restart,
            "resume_saved_audit",
            json!({"audit_id":start["audit_id"]}),
        )
        .unwrap();
        let new_id = &resumed["analysis_id"];
        assert_ne!(new_id, &id);
        assert_eq!(resumed["client_report_plan"], plan);
        let fresh = call(
            &restart,
            "preview_client_report",
            json!({"analysis_id":new_id}),
        )
        .unwrap();
        assert_eq!(fresh["preview_id"], preview["preview_id"]);
        let asset =
            json!({"analysis_id":new_id,"preview_id":fresh["preview_id"],"asset":"raport.html"});
        assert_eq!(
            call(&restart, "read_client_report_asset", asset.clone()).unwrap()["bytes"].as_u64(),
            Some(content.len() as u64)
        );
        let publish_args = json!({"analysis_id":new_id,"preview_id":fresh["preview_id"],"user_request":"Issue the reviewed synthetic client example."});
        let receipt = call(&restart, "publish_client_report", publish_args.clone()).unwrap();
        assert!(Path::new(receipt["zip_path"].as_str().unwrap()).is_file());
        // An item mentioned after the reviewed preview belongs in the final report,
        // even if every pinned chapter and approved table stayed the same.
        let mentioned = "00000000abcdf";
        call(
            &restart,
            "record_discussed_entities",
            json!({"analysis_id":new_id,"project_id":project,
            "sql_ids":[mentioned],"wait_events":["log file sync"]}),
        )
        .unwrap();
        assert_eq!(
            call(&restart, "publish_client_report", publish_args).unwrap_err()["error_code"],
            "CLIENT_PREVIEW_STALE"
        );
        let expanded = call(
            &restart,
            "preview_client_report",
            json!({"analysis_id":new_id}),
        )
        .unwrap();
        let public = Path::new(expanded["index_path"].as_str().unwrap());
        let index = fs::read_to_string(public).unwrap();
        let link = format!("wykresy/sql-p1-{mentioned}.html");
        assert!(index.contains("log file sync") && index.contains(&link));
        assert!(public.parent().unwrap().join(link).is_file());
        let mut correction = fields;
        correction["analysis_id"] = new_id.clone();
        correction["conclusion"] = json!("Use a larger comparable sample.");
        let revised = call(&restart, "record_audit_brief", correction).unwrap();
        assert_eq!(revised["client_report_plan_stale"], true);
        let stale = call(
            &restart,
            "get_client_report_plan",
            json!({"analysis_id":new_id}),
        )
        .unwrap();
        assert_eq!(stale["source_changes"][0]["current_revision"], 2);
        assert_eq!(
            stale["source_changes"][0]["current"]["conclusion"],
            "Use a larger comparable sample."
        );
        assert_eq!(
            call(&restart, "read_client_report_asset", asset).unwrap_err()["error_code"],
            "CLIENT_PREVIEW_STALE"
        );
        assert!(
            call(
                &restart,
                "preview_client_report",
                json!({"analysis_id":new_id})
            )
            .is_err()
        );
    }
}
