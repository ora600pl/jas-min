//! Remember reader-visible discussion independently of the evidence cache and chapter selection.
use super::*;
use std::sync::LazyLock;

type Entities = (BTreeSet<String>, BTreeSet<String>);
pub(super) type Inventory = BTreeMap<String, Entities>;

const MAX_BATCH: usize = 2048;
const MAX_PANEL_BYTES: usize = 131072;
static SQL_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[0-9abcdfghjkmnpqrstuvwxyz]{13}\b").unwrap());

fn valid_sql(id: &str) -> bool {
    id.len() == 13 && SQL_TOKEN.is_match(id)
}
fn inferred_wait(name: &str) -> bool {
    valid_wait(name)
        && !matches!(
            name.trim().to_ascii_lowercase().as_str(),
            "on cpu" | "cpu" | "cpu + wait for cpu" | "n/a" | "unknown" | "brak" | "—" | "–" | "-"
        )
}
fn valid_wait(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
}

pub(super) fn validate_inventory(inventory: &Inventory, projects: &[String]) -> Result<(), Value> {
    if inventory.iter().any(|(project, (sql, waits))| {
        !projects.contains(project)
            || sql
                .iter()
                .any(|id| !valid_sql(id) || id != &id.to_ascii_lowercase())
            || waits.iter().any(|event| !valid_wait(event))
    }) {
        return Err(tool_error(
            "INVALID_DISCUSSION_ENTITY",
            "Invalid project, SQL_ID or wait event in the discussion register",
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub(super) struct Catalog {
    sql: HashSet<String>,
    waits: BTreeMap<String, String>,
    wait_pattern: Option<Regex>,
}

impl ProjectData {
    fn entity_catalog(&self) -> &Catalog {
        self.discussion_catalog.get_or_init(|| {
            let mut sql: HashSet<_> = self
                .collection
                .sql_text
                .keys()
                .map(|s| s.to_ascii_lowercase())
                .collect();
            let mut waits = BTreeMap::new();
            let mut wait = |name: &str| {
                if valid_wait(name)
                    && !["on cpu", "cpu", "cpu + wait for cpu"]
                        .contains(&name.to_ascii_lowercase().as_str())
                {
                    waits
                        .entry(name.to_ascii_lowercase())
                        .or_insert_with(|| name.to_owned());
                }
            };
            for snap in &self.collection.awrs {
                for row in snap
                    .foreground_wait_events
                    .iter()
                    .chain(&snap.background_wait_events)
                {
                    wait(&row.event);
                }
                sql.extend(
                    snap.access_path_observations
                        .iter()
                        .map(|r| r.scope.sql_id.to_ascii_lowercase()),
                );
                for row in snap.top_sql_with_top_events.values() {
                    wait(&row.event_name);
                    sql.insert(row.sql_id.to_ascii_lowercase());
                }
                sql.extend(
                    snap.sql_elapsed_time
                        .iter()
                        .map(|r| r.sql_id.to_ascii_lowercase()),
                );
                sql.extend(
                    snap.sql_cpu_time
                        .keys()
                        .chain(snap.sql_io_time.keys())
                        .chain(snap.sql_gets.keys())
                        .chain(snap.sql_reads.keys())
                        .map(|id| id.to_ascii_lowercase()),
                );
            }
            for row in &self.report.top_foreground_wait_events {
                wait(&row.event_name);
            }
            for row in &self.report.top_background_wait_events {
                wait(&row.event_name);
            }
            for row in &self.report.top_sqls_by_elapsed_time {
                sql.insert(row.sql_id.to_ascii_lowercase());
                for event in &row.wait_events_found_in_ash_sections_for_this_sql {
                    wait(&event.event_name);
                }
                for event in &row.wait_events_with_strong_pearson_correlation {
                    wait(&event.event_name);
                }
            }
            for key in ["FG", "BG"] {
                for name in self.report_links.get(key).into_iter().flatten() {
                    wait(name);
                }
            }
            if let Some(ids) = self.report_links.get("SQL") {
                sql.extend(ids.iter().map(|s| s.to_ascii_lowercase()));
            }
            // Prefer a complete name when two Oracle events share a prefix.
            let mut names: Vec<_> = waits.keys().collect();
            names.sort_by_key(|s| std::cmp::Reverse(s.len()));
            let wait_pattern = if names.is_empty() {
                None
            } else {
                let pattern = names
                    .into_iter()
                    .map(|s| regex::escape(s))
                    .collect::<Vec<_>>()
                    .join("|");
                // Fall back to individual complete names for unusually large catalogs.
                Regex::new(&format!("(?i:{pattern})")).ok()
            };
            Catalog {
                sql,
                waits,
                wait_pattern,
            }
        })
    }
}
fn word_part(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn bounded(text: &str, start: usize, end: usize) -> bool {
    !text[..start].chars().next_back().is_some_and(word_part)
        && !text[end..].chars().next().is_some_and(word_part)
}
impl Catalog {
    // A board may append a foreground/background marker to a wait label. Look up the
    // complete name first so that a genuine collected event remains distinct.
    fn wait_name<'a>(&'a self, label: &'a str) -> &'a str {
        let mut name = label.trim();
        loop {
            if let Some(original) = self.waits.get(&name.to_ascii_lowercase()) {
                return original;
            }
            let Some((base, suffix)) = name.rsplit_once('·') else {
                return name;
            };
            if !matches!(
                suffix.trim().to_lowercase().as_str(),
                "foreground" | "background" | "tło"
            ) {
                return name;
            }
            name = base.trim_end();
        }
    }

    fn canonical_waits(&self, waits: &BTreeSet<String>) -> BTreeSet<String> {
        let mut seen = HashSet::new();
        waits
            .iter()
            .map(|name| self.wait_name(name))
            .filter(|name| valid_wait(name) && seen.insert(name.to_ascii_lowercase()))
            .map(str::to_owned)
            .collect()
    }

    fn scan(&self, text: &str, found: &mut Entities, scoped: bool) {
        for m in SQL_TOKEN.find_iter(text) {
            let id = m.as_str().to_ascii_lowercase();
            // Free prose can contain 13-letter words from the Oracle alphabet.
            // Missing-capture IDs can still appear as an explicit, scoped mention.
            if self.sql.contains(&id)
                || (scoped
                    && id.bytes().any(|c| c.is_ascii_digit())
                    && id.bytes().any(|c| c.is_ascii_alphabetic()))
            {
                found.0.insert(id);
            }
        }
        if let Some(pattern) = &self.wait_pattern {
            for m in pattern.find_iter(text) {
                if bounded(text, m.start(), m.end()) {
                    if let Some(name) = self.waits.get(&m.as_str().to_ascii_lowercase()) {
                        found.1.insert(name.clone());
                    }
                }
            }
        } else if !self.waits.is_empty() {
            let lower = text.to_ascii_lowercase();
            for (key, name) in &self.waits {
                if lower
                    .match_indices(key)
                    .any(|(i, _)| bounded(&lower, i, i + key.len()))
                {
                    found.1.insert(name.clone());
                }
            }
        }
    }
}

struct Collector<'a> {
    result: Inventory,
    projects: &'a BTreeMap<String, Arc<ProjectData>>,
    selected: &'a [String],
}
impl Collector<'_> {
    fn row_scope(&self, obj: &Map<String, Value>, row: &Value, i: usize, default: &str) -> String {
        if let Some(id) = obj
            .get("row_project_ids")
            .and_then(|ids| ids.get(i))
            .and_then(Value::as_str)
        {
            return id.to_owned();
        }
        let label = obj
            .get("columns")
            .and_then(Value::as_array)
            .and_then(|cols| {
                cols.iter().position(|c| {
                    let s = c
                        .as_str()
                        .or_else(|| c["label"].as_str())
                        .unwrap_or_default()
                        .trim()
                        .to_ascii_lowercase();
                    matches!(
                        s.as_str(),
                        "project"
                            | "project id"
                            | "project_id"
                            | "projekt"
                            | "instance"
                            | "instancja"
                            | "scope"
                            | "zakres"
                    )
                })
            })
            .and_then(|index| row.get(index))
            .and_then(Value::as_str)
            .map(str::trim);
        if let Some(label) = label {
            let mut candidates = self.selected.iter().filter(|id| {
                let p = &self.projects[*id];
                let stem = Path::new(p.stem.as_str())
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default();
                let short = stem.split_once("_awr_").map_or(stem, |(prefix, _)| prefix);
                [id.as_str(), stem, short]
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(label))
            });
            if let Some(first) = candidates.next() {
                if candidates.next().is_none() {
                    return first.clone();
                }
            }
        }
        default.to_owned()
    }
    // Comparative briefs are stored under one project, even when a sentence
    // explicitly describes another instance. Keep such observations with the named instance.
    fn read_prose(&mut self, default: &str, text: &str, scoped: bool) {
        if self.selected.len() == 1 {
            self.projects[default].entity_catalog().scan(
                text,
                self.result.entry(default.into()).or_default(),
                scoped,
            );
            return;
        }
        static SENTENCE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?:[.!?]\s+|\n+)").unwrap());
        for sentence in SENTENCE.split(text) {
            let lower = sentence.to_ascii_lowercase();
            let mut named = Vec::new();
            for id in self.selected {
                let p = &self.projects[id];
                let stem = Path::new(p.stem.as_str())
                    .file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or_default();
                let short = stem.split_once("_awr_").map_or(stem, |(label, _)| label);
                if [id.as_str(), short].iter().any(|label| {
                    !label.is_empty()
                        && lower
                            .match_indices(&label.to_ascii_lowercase())
                            .any(|(at, _)| bounded(&lower, at, at + label.len()))
                }) {
                    named.push(id.as_str());
                }
            }
            if named.is_empty() {
                self.projects[default].entity_catalog().scan(
                    sentence,
                    self.result.entry(default.into()).or_default(),
                    scoped,
                );
            } else {
                let narrow = named.len() == 1;
                for id in named {
                    self.projects[id].entity_catalog().scan(
                        sentence,
                        self.result.entry(id.into()).or_default(),
                        narrow,
                    );
                }
            }
        }
    }
    fn read(&mut self, id: &str, value: &Value, scoped: bool) {
        if !self.selected.iter().any(|s| s == id) {
            return;
        }
        match value {
            Value::String(text) => {
                self.read_prose(id, text, scoped);
            }
            Value::Array(values) => {
                for v in values {
                    self.read(id, v, scoped);
                }
            }
            Value::Object(obj) => {
                let id = obj.get("project_id").and_then(Value::as_str).unwrap_or(id);
                if !self.selected.iter().any(|s| s == id) {
                    return;
                }
                for (key, v) in obj {
                    match key.as_str() {
                        "rows" => {
                            for (i, row) in v.as_array().into_iter().flatten().enumerate() {
                                let row_project = self.row_scope(obj, row, i, id);
                                let row_id = row_project.as_str();
                                let columns = obj.get("columns").and_then(Value::as_array);
                                if let (Some(columns), Some(cells)) = (columns, row.as_array()) {
                                    for (col, cell) in columns.iter().zip(cells) {
                                        let name = col
                                            .as_str()
                                            .or_else(|| col["label"].as_str())
                                            .unwrap_or_default()
                                            .to_ascii_lowercase();
                                        let field = match name.trim() {
                                            "sql_id" | "sql id" => "sql_id",
                                            "event"
                                            | "event name"
                                            | "wait event"
                                            | "wait_event"
                                            | "zdarzenie oczekiwania"
                                            | "zdarzenie" => "event_name",
                                            _ => continue,
                                        };
                                        if let Some(text) =
                                            cell.as_str().filter(|s| !matches!(*s, "—" | "–" | "-"))
                                        {
                                            self.read(row_id, &json!({(field):text}), scoped);
                                        }
                                    }
                                }
                                self.read(row_id, row, scoped);
                            }
                        }
                        "sql_id" => {
                            if let Some(sql) = v.as_str().filter(|s| valid_sql(s)) {
                                self.result
                                    .entry(id.into())
                                    .or_default()
                                    .0
                                    .insert(sql.to_ascii_lowercase());
                            }
                        }
                        "wait_event" | "event_name" => {
                            if let Some(name) = v
                                .as_str()
                                .map(|label| self.projects[id].entity_catalog().wait_name(label))
                                .filter(|name| inferred_wait(name))
                            {
                                self.result
                                    .entry(id.into())
                                    .or_default()
                                    .1
                                    .insert(name.to_owned());
                            }
                        }
                        "id" | "project_id" | "row_project_ids" | "brief_id" | "chapter_id"
                        | "source" | "evidence_refs" | "guidance_refs" | "guidance_quotes"
                        | "evidence_ref" | "sql_text" | "fragments" | "hypothesis_basis" => {}
                        _ => {
                            // A raw series can use SQL IDs or event names as object keys.
                            let catalog = self.projects[id].entity_catalog();
                            if catalog.sql.contains(&key.to_ascii_lowercase())
                                || catalog.waits.contains_key(&key.to_ascii_lowercase())
                            {
                                catalog.scan(
                                    key,
                                    self.result.entry(id.to_owned()).or_default(),
                                    true,
                                );
                            }
                            self.read(id, v, scoped);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

pub(super) fn collect(
    session: &AnalysisSession,
    projects: &BTreeMap<String, Arc<ProjectData>>,
) -> Inventory {
    let mut c = Collector {
        result: session.discussed_entities.clone(),
        projects,
        selected: &session.project_ids,
    };
    if let Some(audit) = &session.audit {
        for brief in audit.history.iter().chain(audit.briefs.values()) {
            c.read(
                &brief.project_id,
                &serde_json::to_value(brief).unwrap(),
                true,
            );
            // A cited direct lookup may carry the entity only in its arguments.
            for ev in brief
                .evidence_refs
                .iter()
                .filter_map(|id| session.evidence.get(id))
            {
                if ev
                    .project_id
                    .as_ref()
                    .is_some_and(|p| p != &brief.project_id)
                {
                    continue;
                }
                for key in ["sql_id", "event_name", "wait_event"] {
                    if let Some(value) = ev.arguments.get(key) {
                        c.read(&brief.project_id, &json!({(key):value}), true);
                    }
                }
            }
        }
        if let Some(plan) = &audit.client_report_plan {
            let value = serde_json::to_value(plan).unwrap();
            for id in &session.project_ids {
                for k in ["title", "introduction"] {
                    c.read(id, &value[k], session.project_ids.len() == 1);
                }
            }
            for chapter in value["chapters"].as_array().into_iter().flatten() {
                let ids: BTreeSet<_> = chapter["briefs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| audit.briefs.get(r["brief_id"].as_str()?))
                    .map(|b| &b.project_id)
                    .collect();
                for id in &ids {
                    for k in [
                        "title",
                        "question",
                        "conclusion",
                        "interpretation",
                        "limitation",
                        "next_step",
                        "success_metric",
                    ] {
                        c.read(id, &chapter[k], ids.len() == 1);
                    }
                }
                for m in chapter["measurements"].as_array().into_iter().flatten() {
                    if let Some(id) = m["project_id"].as_str() {
                        c.read(id, m, true);
                    }
                }
            }
        }
    }
    for table in session.report_tables.values() {
        for row in &table.rows {
            if let Some(id) = row.cells.get("project_id") {
                c.read(id, &json!(row.cells), true);
            } else if session.project_ids.len() == 1 {
                c.read(&session.project_ids[0], &json!(row.cells), true);
            }
        }
    }
    for finding in session.findings.values() {
        let value = serde_json::to_value(finding).unwrap();
        let ids: BTreeSet<_> = finding
            .evidence_refs
            .iter()
            .filter_map(|r| session.evidence.get(r)?.project_id.as_ref())
            .collect();
        if ids.len() == 1 {
            c.read(ids.first().unwrap(), &value, true);
        } else {
            for id in &session.project_ids {
                c.read(id, &value, session.project_ids.len() == 1);
            }
        }
    }
    for id in &session.project_ids {
        for (kind, name) in gradient_intro::entities(&projects[id]) {
            let entry = c.result.entry(id.clone()).or_default();
            match kind {
                "sql" if valid_sql(&name) => {
                    entry.0.insert(name.to_ascii_lowercase());
                }
                "wait" if valid_wait(&name) => {
                    entry.1.insert(name);
                }
                _ => {}
            }
        }
    }
    // The stored register predates display-label normalization on older checkpoints.
    // Normalize in the projection even when the caller is only previewing that checkpoint.
    for (id, (_, waits)) in &mut c.result {
        *waits = projects[id].entity_catalog().canonical_waits(waits);
    }
    c.result.retain(|_, (s, w)| !s.is_empty() || !w.is_empty());
    c.result
}

fn batch(args: &Map<String, Value>, key: &str) -> Result<Vec<String>, Value> {
    let Some(value) = args.get(key) else {
        return Ok(vec![]);
    };
    let error = || {
        tool_error(
            "INVALID_DISCUSSION_ENTITY",
            "Supply a bounded array of individual SQL_IDs or wait names.",
        )
    };
    let array = value
        .as_array()
        .filter(|a| a.len() <= MAX_BATCH)
        .ok_or_else(error)?;
    array
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::trim)
                .map(str::to_owned)
                .ok_or_else(error)
        })
        .collect()
}

impl AnalysisRuntime {
    pub(super) fn discussed_entities(&self, session: &AnalysisSession) -> Inventory {
        collect(session, &self.projects)
    }
    pub(super) fn record_discussed_entities(
        &self,
        args: &Map<String, Value>,
    ) -> Result<Value, Value> {
        let id = Self::analysis_id(args)?;
        let project_id = required_string(args, "project_id", 256)?;
        let (session, project) = self.selected_project(id, &project_id)?;
        let catalog = project.entity_catalog();
        let mut incoming = Entities::default();
        for sql in batch(args, "sql_ids")? {
            if !valid_sql(&sql) {
                return Err(tool_error(
                    "INVALID_DISCUSSION_ENTITY",
                    "Expected a 13-character Oracle SQL_ID",
                ));
            }
            incoming.0.insert(sql.to_ascii_lowercase());
        }
        for event in batch(args, "wait_events")? {
            let name = catalog.wait_name(&event);
            if !valid_wait(&event) || !valid_wait(name) {
                return Err(tool_error(
                    "INVALID_DISCUSSION_ENTITY",
                    "Expected an individual wait event name",
                ));
            }
            incoming.1.insert(name.to_owned());
        }
        if let Some(v) = args.get("panel_text") {
            let text = v
                .as_str()
                .filter(|s| s.len() <= MAX_PANEL_BYTES)
                .ok_or_else(|| {
                    tool_error(
                        "INVALID_DISCUSSION_ENTITY",
                        "Supply a bounded excerpt of displayed panel text",
                    )
                })?;
            catalog.scan(text, &mut incoming, true);
        }
        let mut s = session
            .lock()
            .map_err(|_| tool_error("SESSION_LOCK", "Session unavailable"))?;
        if let Some(reference) = args.get("evidence_ref") {
            let ev = reference
                .as_str()
                .and_then(|r| s.evidence.get(r))
                .ok_or_else(|| {
                    tool_error(
                        "INVALID_DISCUSSION_SOURCE",
                        "Unknown displayed evidence in this session",
                    )
                })?;
            let mut c = Collector {
                result: BTreeMap::new(),
                projects: &self.projects,
                selected: std::slice::from_ref(&project_id),
            };
            if ev.project_id.as_ref() == Some(&project_id)
                || (ev.project_id.is_none() && s.project_ids.len() == 1)
            {
                c.read(&project_id, &ev.result, true);
            } else if ev.project_id.is_none()
                && ["compare_project_metric", "compare_project_sql"]
                    .contains(&ev.tool_name.as_str())
            {
                let side = ["baseline", "candidate"]
                    .into_iter()
                    .find(|side| {
                        ev.arguments[format!("{side}_project_id")].as_str() == Some(&project_id)
                    })
                    .ok_or_else(|| {
                        tool_error(
                            "INVALID_DISCUSSION_SOURCE",
                            "The compared projects do not include this instance",
                        )
                    })?;
                for key in ["metric", "sql_id", side] {
                    c.read(&project_id, &ev.result[key], true);
                }
            } else if ev.project_id.is_none()
                && ev.tool_name == "get_rac_cpu_scenario"
                && ev.arguments["project_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some(&project_id)))
            {
                c.read(
                    &project_id,
                    &ev.result["assumptions"]["selected_sql_ids"],
                    true,
                );
                for row in ev.result["instances"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|r| r["project_id"].as_str() == Some(&project_id))
                {
                    c.read(&project_id, row, true);
                }
            } else {
                return Err(tool_error(
                    "INVALID_DISCUSSION_SOURCE",
                    "Choose displayed evidence belonging to this project and session",
                ));
            }
            if let Some((sql, waits)) = c.result.remove(&project_id) {
                incoming.0.extend(sql);
                incoming.1.extend(waits);
            }
        }
        incoming.1 = catalog.canonical_waits(&incoming.1);
        let existing = s.discussed_entities.get(&project_id);
        let recorded_waits = existing
            .map(|(_, waits)| catalog.canonical_waits(waits))
            .unwrap_or_default();
        if let Some((ids, _)) = existing {
            incoming.0.retain(|id| !ids.contains(id));
            incoming.1.retain(|name| {
                !recorded_waits
                    .iter()
                    .any(|old| old.eq_ignore_ascii_case(name))
            });
        }
        let added = incoming.0.len() + incoming.1.len();
        if added > 0 {
            let previous = s.discussed_entities.get(&project_id).cloned();
            let audit = s.audit.clone();
            let markdown = s.finalized_markdown.take();
            let entry = s.discussed_entities.entry(project_id.clone()).or_default();
            entry.0.extend(incoming.0);
            entry.1 = recorded_waits;
            entry.1.extend(incoming.1);
            if let Some(a) = s.audit.as_mut() {
                a.changed();
            }
            if let Some(mut saved) = s.tx_pilot.clone() {
                if let Err(error) = self
                    .private_workspace()
                    .and_then(|root| self.write_snapshot(&root, &s, &mut saved))
                {
                    if let Some(entry) = previous {
                        s.discussed_entities.insert(project_id.clone(), entry);
                    } else {
                        s.discussed_entities.remove(&project_id);
                    }
                    s.audit = audit;
                    s.finalized_markdown = markdown;
                    return Err(error);
                }
                s.tx_pilot = Some(saved);
            }
        }
        Ok(
            json!({"project_id":project_id,"added":added,"revision":s.tx_pilot.as_ref().map(|p|p.revision),"persisted":s.tx_pilot.is_some()}),
        )
    }
}

pub(super) fn tool_definition() -> Value {
    function_definition("record_discussed_entities","Retain SQL_IDs and wait events mentioned in this conversation or visible on a Companion board for the final report. Pass the project to which each mention applies. Optional panel_text is matched against its AWR catalog. Repeated entries are idempotent; a mention is not measured impact.",json!({"type":"object","additionalProperties":false,"properties":{"project_id":{"type":"string"},"sql_ids":{"type":"array","maxItems":MAX_BATCH,"items":{"type":"string"}},"wait_events":{"type":"array","maxItems":MAX_BATCH,"items":{"type":"string"}},"panel_text":{"type":"string","maxLength":MAX_PANEL_BYTES},"evidence_ref":{"type":"string","description":"An evidence payload actually expanded by the reader, scoped to this project"}},"required":["project_id"]}))
}

impl ReportEntityLinks {
    pub(super) fn discussion_prose(&self, project: &str, text: &str) -> String {
        let lower = text.to_ascii_lowercase();
        let mut spans = Vec::new();
        for (id, kind, name) in self.targets.keys() {
            if id != project || name.is_empty() {
                continue;
            }
            for (start, _) in lower.match_indices(name) {
                let end = start + name.len();
                if bounded(&lower, start, end) {
                    spans.push((start, end, kind));
                }
            }
        }
        spans.sort_by_key(|(start, end, kind)| {
            (*start, std::cmp::Reverse(*end), *kind == "background_wait")
        });
        let mut html = String::new();
        let mut cursor = 0;
        for (start, end, kind) in spans {
            if start < cursor {
                continue;
            }
            html.push_str(&encode_text(&text[cursor..start]));
            html.push_str(&self.html_code_link(project, kind, &text[start..end]));
            cursor = end;
        }
        html.push_str(&encode_text(&text[cursor..]));
        html
    }
}
/// Classic Markdown uses absolute links, as do its existing AWR detail pages.
/// Publish the whole offline card directory atomically so concurrent previews share complete assets.
impl AnalysisRuntime {
    pub(super) fn classic_discussion_links(
        &self,
        state: &AnalysisSession,
        doc: &Value,
    ) -> Result<ReportEntityLinks, Value> {
        use super::tx_pilot_publication::client_report_v2::{
            discussed_assets, discussed_sql_path, discussed_wait_path,
        };
        use sha2::{Digest, Sha256};
        let mut links = ReportEntityLinks::build(&state.project_ids, &self.projects);
        let Some(items) = doc["discussed_entities"]
            .as_object()
            .filter(|m| !m.is_empty())
        else {
            return Ok(links);
        };
        let assets = discussed_assets(
            self,
            state,
            &report_project_labels(doc),
            ReportLanguage::new(&state.config.language),
        )?;
        if assets.is_empty() {
            return Ok(links);
        }
        let mut digest = Sha256::new();
        for (name, data) in &assets {
            digest.update((name.len() as u64).to_be_bytes());
            digest.update(name.as_bytes());
            digest.update((data.len() as u64).to_be_bytes());
            digest.update(data);
        }
        let stash = self.private_workspace()?.join("discussion-assets");
        tx_pilot::private_directory(&stash)?;
        let destination = stash.join(format!("{:x}", digest.finalize()));
        let error = |e: std::io::Error| {
            tool_error(
                "AUDIT_STORAGE_ERROR",
                format!("Cannot store discussion cards: {e}"),
            )
        };
        if !destination.is_dir() {
            let tmp = tempfile::Builder::new()
                .prefix(".cards-")
                .tempdir_in(&stash)
                .map_err(error)?;
            for (name, data) in &assets {
                let path = tmp.path().join(name);
                std::fs::create_dir_all(path.parent().unwrap()).map_err(error)?;
                std::fs::write(path, data).map_err(error)?;
            }
            if let Err(e) = std::fs::rename(tmp.path(), &destination) {
                if !destination.is_dir() {
                    return Err(error(e));
                }
            }
        }
        for (project, pair) in items {
            for id in pair[0]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if let Some(path) =
                    discussed_sql_path(state, project, id).filter(|name| assets.contains_key(name))
                {
                    links.targets.insert(
                        (project.clone(), "sql".into(), id.into()),
                        destination.join(path).to_string_lossy().into_owned(),
                    );
                }
            }
            for event in pair[1]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if let Some(path) = discussed_wait_path(state, project, event)
                    .filter(|name| assets.contains_key(name))
                {
                    links.targets.insert(
                        (project.clone(), "wait".into(), event.to_ascii_lowercase()),
                        destination.join(path).to_string_lossy().into_owned(),
                    );
                }
            }
        }
        Ok(links)
    }
}

pub(super) fn append_markdown(
    out: &mut String,
    doc: &Value,
    links: &ReportEntityLinks,
    l: ReportLanguage,
) {
    let Some(projects) = doc["discussed_entities"]
        .as_object()
        .filter(|x| !x.is_empty())
    else {
        return;
    };
    out.push_str(&format!(
        "## {}\n\n",
        l.choose(
            "Discussed SQL and wait events",
            "Omawiane zapytania SQL i zdarzenia oczekiwania"
        )
    ));
    let labels = report_project_labels(doc);
    for (id, pair) in projects {
        let label = labels.get(id).unwrap_or(id);
        out.push_str(&format!("### {}\n\n", encode_text(label)));
        for (index, title, kind) in [
            (0, "SQL_ID", "sql"),
            (1, l.choose("Wait events", "Zdarzenia oczekiwania"), "wait"),
        ] {
            let items: Vec<_> = pair[index]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|name| {
                    let kind = if kind == "wait" && links.target(id, kind, name).is_none() {
                        "background_wait"
                    } else {
                        kind
                    };
                    links.html_code_link(id, kind, name)
                })
                .collect();
            if !items.is_empty() {
                out.push_str(&format!("**{title}**\n\n{}\n\n", items.join(", ")));
            }
        }
    }
}

#[cfg(test)]
#[path = "discussed_entities_tests.rs"]
mod tests;
