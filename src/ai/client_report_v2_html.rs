//! Offline client edition. Only the public projection is rendered.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use html_escape::encode_text;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

#[derive(Deserialize)]
struct Document {
    language: String,
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    period: String,
    chapters: Vec<Chapter>,
    #[serde(default)]
    inventory: Vec<InstanceInventory>,
    #[serde(default)]
    gradient_intro: Vec<super::super::super::gradient_intro::Opening>,
}

/// Public scope only. The project key never becomes an asset name.
#[derive(Serialize, Deserialize)]
pub(super) struct InstanceInventory {
    pub ordinal: usize,
    pub instance: String,
    pub sql_ids: BTreeSet<String>,
    pub waits: BTreeSet<String>,
}

impl InstanceInventory {
    pub fn sql_file(&self, id: &str) -> String {
        super::sql_filename(self.ordinal, id)
    }
}

pub(super) const INVENTORY_ANCHOR: &str = "discussion_inventory";

/// Only recognized IDs are linked. A shared ID in unscoped prose gets one
/// visibly labeled link per instance, so readers never land on another node.
struct Destination<'a> {
    scope: &'a str,
    filename: String,
}

#[derive(Default)]
pub(super) struct SqlLinks<'a> {
    sql: HashMap<String, Vec<Destination<'a>>>,
    waits: HashMap<String, Vec<Destination<'a>>>,
    waits_by_first: HashMap<u8, Vec<String>>,
}

impl<'a> SqlLinks<'a> {
    fn new(inventory: impl IntoIterator<Item = &'a InstanceInventory>) -> Self {
        let mut links = Self::default();
        for scope in inventory {
            for id in &scope.sql_ids {
                links.sql.entry(id.clone()).or_default().push(Destination {
                    scope: &scope.instance,
                    filename: scope.sql_file(id),
                });
            }
            for event in &scope.waits {
                links
                    .waits
                    .entry(event.to_ascii_lowercase())
                    .or_default()
                    .push(Destination {
                        scope: &scope.instance,
                        filename: super::wait_filename(scope.ordinal, event),
                    });
            }
        }
        for event in links.waits.keys() {
            if let Some(&first) = event.as_bytes().first() {
                links
                    .waits_by_first
                    .entry(first)
                    .or_default()
                    .push(event.clone());
            }
        }
        // Longest complete event wins, e.g. "gc current block busy" over a shorter prefix.
        for group in links.waits_by_first.values_mut() {
            group.sort_by_key(|name| std::cmp::Reverse(name.len()));
        }
        links
    }

    pub(super) fn text(&self, value: &str, instance: Option<&str>, local: bool) -> String {
        if self.sql.is_empty() && self.waits.is_empty() {
            return encode_text(value).into_owned();
        }
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let mut result = String::new();
        let mut start = 0;
        let base = if local { "" } else { "wykresy/" };
        for (at, _) in value.char_indices() {
            if at < start || value[..at].chars().next_back().is_some_and(word) {
                continue;
            }
            let tail = &value[at..];
            let bounded = |n| {
                tail.get(n..)
                    .is_some_and(|s| !s.chars().next().is_some_and(word))
            };
            let event = self
                .waits_by_first
                .get(&tail.as_bytes()[0].to_ascii_lowercase())
                .and_then(|names| {
                    names.iter().find(|name| {
                        tail.get(..name.len())
                            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(name))
                            && bounded(name.len())
                    })
                });
            let found = if let Some(event) = event {
                self.waits.get(event).map(|targets| (event.len(), targets))
            } else {
                tail.get(..13)
                    .filter(|id| id.bytes().all(|b| b.is_ascii_alphanumeric()) && bounded(13))
                    .and_then(|id| self.sql.get(&id.to_ascii_lowercase()))
                    .map(|targets| (13, targets))
            };
            let Some((len, targets)) = found else {
                continue;
            };
            result.push_str(&encode_text(&value[start..at]));
            let label = encode_text(&tail[..len]);
            let scoped = instance.and_then(|name| targets.iter().find(|t| name == t.scope));
            if let Some(target) = scoped {
                write!(result, r#"<a href="{base}{}">{label}</a>"#, target.filename).unwrap();
            } else if instance.is_some() {
                result.push_str(&label);
            } else {
                for (i, target) in targets.iter().enumerate() {
                    if i > 0 {
                        result.push_str(" / ");
                    }
                    write!(result, r#"<a href="{base}{}">{label}"#, target.filename).unwrap();
                    if targets.len() > 1 {
                        write!(result, " ({})", encode_text(target.scope)).unwrap();
                    }
                    result.push_str("</a>");
                }
            }
            start = at + len;
        }
        result.push_str(&encode_text(&value[start..]));
        result
    }
}

#[derive(Deserialize)]
struct Chapter {
    #[serde(default)]
    instance: Option<String>,
    id: String,
    title: String,
    #[serde(default)]
    question: String,
    #[serde(default)]
    conclusion: String,
    #[serde(default)]
    paragraphs: Vec<String>,
    #[serde(default)]
    tables: Vec<Table>,
    #[serde(default)]
    sql_examples: Vec<SqlExample>,
    #[serde(default)]
    sources: Vec<Source>,
    #[serde(default)]
    limitation: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    owner: String,
    #[serde(default)]
    success: String,
}

#[derive(Deserialize, Serialize)]
pub(super) struct SqlFragment {
    pub text: String,
    pub first: usize,
    pub last: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SqlRecommendation {
    pub rationale: String,
    pub assumption: String,
    pub before: String,
    pub after: String,
}

#[derive(Deserialize, Serialize)]
pub(super) struct SqlExample {
    pub id: String,
    pub source_id: String,
    pub after_tables: usize,
    pub title: String,
    pub instance: String,
    pub sql_id: String,
    pub fragments: Vec<SqlFragment>,
    pub meaning: String,
    pub scaling: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation: Option<SqlRecommendation>,
}

#[derive(Deserialize)]
struct Table {
    #[serde(default)]
    id: Option<String>,
    title: String,
    #[serde(default)]
    description: String,
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    #[serde(default)]
    note: String,
    #[serde(default)]
    source_ids: Vec<String>,
    #[serde(default)]
    row_source_ids: Vec<Vec<String>>,
    #[serde(default)]
    row_instances: Vec<Option<String>>,
    #[serde(default)]
    column_instances: Vec<Option<String>>,
    #[serde(default)]
    plot: Option<TablePlot>,
}

#[derive(Deserialize)]
struct TablePlot {
    kind: String,
    title: String,
    unit: String,
    label_column: usize,
    executions_column: usize,
    series_columns: Vec<usize>,
    #[serde(default)]
    series_labels: Vec<String>,
}

impl Table {
    fn anchor(&self, chapter: &Chapter, index: usize) -> String {
        self.id
            .clone()
            .unwrap_or_else(|| format!("{}-table-{}", chapter.id, index + 1))
    }
}

#[derive(Deserialize)]
struct Source {
    #[serde(default)]
    instance: Option<String>,
    id: String,
    title: String,
    #[serde(default)]
    href: Option<String>,
    #[serde(default)]
    description: String,
}

fn valid_anchor(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 180
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn valid_link(href: &str, anchors: &HashSet<String>) -> bool {
    if let Some(anchor) = href.strip_prefix('#') {
        return anchors.contains(anchor);
    }
    let (path, fragment) = href.split_once('#').unwrap_or((href, ""));
    let Some(relative) = path.strip_prefix("wykresy/") else {
        return false;
    };
    !relative.is_empty()
        && relative.split('/').all(|part| {
            part.as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
        && (fragment.is_empty() || valid_anchor(fragment))
}

fn validate(doc: &Document) -> Result<(), String> {
    if doc.title.trim().is_empty() || (doc.chapters.is_empty() && doc.inventory.is_empty()) {
        return Err("A client edition needs a title and chapter or discussion inventory".into());
    }
    if !matches!(doc.language.as_str(), "en" | "pl") {
        return Err("Unsupported report language".into());
    }
    let mut anchors: HashSet<String> = ["top", "report-menu", "content"]
        .map(String::from)
        .into_iter()
        .collect();
    let mut ordinals = HashSet::new();
    let mut names = HashSet::new();
    for scope in &doc.inventory {
        if scope.ordinal == 0
            || !ordinals.insert(scope.ordinal)
            || scope.instance.trim().is_empty()
            || !names.insert(&scope.instance)
            || scope.sql_ids.iter().any(|id| {
                id.len() != 13
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
        {
            return Err("Invalid public SQL source scope".into());
        }
    }
    if !doc.inventory.is_empty() {
        anchors.insert(INVENTORY_ANCHOR.into());
    }
    for chapter in &doc.chapters {
        if !valid_anchor(&chapter.id)
            || !anchors.insert(chapter.id.clone())
            || chapter.title.trim().is_empty()
        {
            return Err("Each chapter needs a distinct HTML anchor and a title".into());
        }
        for example in &chapter.sql_examples {
            if !valid_anchor(&example.id)
                || !anchors.insert(example.id.clone())
                || example.after_tables > chapter.tables.len()
                || example.fragments.is_empty()
            {
                return Err("A SQL example needs an ordered location, selected source text and distinct anchor".into());
            }
        }
        for (i, table) in chapter.tables.iter().enumerate() {
            let anchor = table.anchor(chapter, i);
            if !valid_anchor(&anchor) || !anchors.insert(anchor) {
                return Err("Each measurement table needs a distinct HTML anchor".into());
            }
        }
    }
    for chapter in &doc.chapters {
        let mut sources = HashSet::new();
        for source in &chapter.sources {
            if source.id.is_empty()
                || !sources.insert(source.id.as_str())
                || source.title.trim().is_empty()
            {
                return Err("Chapter source references must be distinct and named".into());
            }
            if let Some(href) = &source.href {
                if !valid_link(href, &anchors) {
                    return Err(
                        "A source must reference a local chart or an existing report anchor".into(),
                    );
                }
            } else if source.description.trim().is_empty() {
                return Err("An unavailable source needs an explanation".into());
            }
        }
        if chapter
            .sql_examples
            .iter()
            .any(|e| !sources.contains(e.source_id.as_str()))
        {
            return Err("A SQL example refers to a source outside its chapter".into());
        }
        for table in &chapter.tables {
            if table.title.trim().is_empty()
                || table.columns.is_empty()
                || table.columns.iter().any(|c| c.trim().is_empty())
            {
                return Err("A measurement table needs a title and named columns".into());
            }
            if table.rows.iter().any(|row| {
                row.len() != table.columns.len()
                    || row.iter().any(|cell| {
                        !matches!(cell, Value::String(_) | Value::Number(_) | Value::Null)
                    })
            }) {
                return Err(
                    "Every table row must match its named columns and contain scalar measurements"
                        .into(),
                );
            }
            if !table.row_source_ids.is_empty() && table.row_source_ids.len() != table.rows.len() {
                return Err("Per-row source references must align with the measurements".into());
            }
            if table
                .source_ids
                .iter()
                .chain(table.row_source_ids.iter().flatten())
                .any(|s| !sources.contains(s.as_str()))
            {
                return Err("A table refers to a source outside its chapter".into());
            }
        }
    }
    Ok(())
}

struct Copy(bool);

impl Copy {
    fn word<'a>(&self, english: &'a str, polish: &'a str) -> &'a str {
        if self.0 { polish } else { english }
    }

    fn number(&self, n: &serde_json::Number) -> String {
        let raw = if n.is_f64() {
            let f = n.as_f64().unwrap();
            if f != 0.0 && f.abs() < 0.005 {
                return format!(
                    "{}{}",
                    if f > 0.0 { "< " } else { "> −" },
                    self.word("0.01", "0,01")
                );
            }
            let decimal = format!("{f:.2}");
            decimal
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_owned()
        } else {
            n.to_string()
        };
        let (integer, fraction) = raw.split_once('.').unwrap_or((&raw, ""));
        let digits = integer.trim_start_matches('-');
        let mut result = String::new();
        if integer.starts_with('-') && raw != "-0" {
            result.push('−');
        }
        for (i, digit) in digits.chars().enumerate() {
            if i != 0 && (digits.len() - i) % 3 == 0 {
                result.push_str(self.word(",", " "));
            }
            result.push(digit);
        }
        if !fraction.is_empty() {
            result.push_str(self.word(".", ","));
            result.push_str(fraction);
        }
        result
    }
}

fn prose(out: &mut String, class: &str, value: &str) {
    for line in value.split("\n\n").map(str::trim).filter(|s| !s.is_empty()) {
        write!(out, "<p class=\"{class}\">{}</p>", encode_text(line)).unwrap();
    }
}

fn linked_prose(
    out: &mut String,
    class: &str,
    value: &str,
    links: &SqlLinks,
    instance: Option<&str>,
    local: bool,
) {
    for line in value.split("\n\n").map(str::trim).filter(|s| !s.is_empty()) {
        write!(
            out,
            r#"<p class="{class}">{}</p>"#,
            links.text(line, instance, local)
        )
        .unwrap();
    }
}

fn source_link(out: &mut String, s: &Source, links: &SqlLinks, local: bool) {
    let title = encode_text(&s.title);
    if let Some(href) = &s.href {
        write!(out, "<a class=\"source-link\" href=\"{href}\">{title} <span aria-hidden=\"true\">↗</span></a>").unwrap();
    } else {
        write!(
            out,
            "<strong>{}</strong>",
            links.text(&s.title, s.instance.as_deref(), local)
        )
        .unwrap();
    }
    if !s.description.is_empty() {
        write!(
            out,
            "<span class=\"source-description\">{}</span>",
            links.text(&s.description, s.instance.as_deref(), local)
        )
        .unwrap();
    }
}

fn sources_list(out: &mut String, sources: &[&Source], copy: &Copy, links: &SqlLinks, local: bool) {
    if sources.is_empty() {
        return;
    }
    write!(
        out,
        "<div class=\"sources\"><p>{}</p><ul>",
        copy.word("Measurement sources", "Źródła pomiaru")
    )
    .unwrap();
    for source in sources {
        out.push_str("<li>");
        source_link(out, source, links, local);
        out.push_str("</li>");
    }
    out.push_str("</ul></div>");
}

/// Draw directly from the published table, including its execution-count boundary.
fn interval_plot(out: &mut String, table: &Table, plot: &TablePlot, copy: &Copy) {
    let columns = table.columns.len();
    if plot.kind != "interval_cpu"
        || table.rows.len() < 2
        || plot.title.trim().is_empty()
        || plot.unit.trim().is_empty()
        || plot.executions_column >= columns
        || plot.label_column >= columns
        || plot.series_columns.len() != 2
        || plot.series_columns[0] == plot.series_columns[1]
        || plot.series_columns.iter().any(|&idx| idx >= columns)
        || table.rows.iter().any(|row| {
            row[plot.executions_column].as_f64() != Some(1.0)
                || row[plot.label_column]
                    .as_str()
                    .is_none_or(|label| label.trim().is_empty())
                || plot
                    .series_columns
                    .iter()
                    .any(|&idx| row[idx].as_f64().is_none_or(|n| !n.is_finite() || n < 0.0))
        })
    {
        return;
    }
    let peak = table
        .rows
        .iter()
        .flat_map(|row| plot.series_columns.iter().filter_map(|&i| row[i].as_f64()))
        .fold(0.0, f64::max);
    if peak == 0.0 {
        return;
    }
    let unit = encode_text(&plot.unit);
    let label = |index: usize| {
        encode_text(
            plot.series_labels
                .get(index)
                .unwrap_or(&table.columns[plot.series_columns[index]]),
        )
    };
    write!(
        out,
        "<figure class=\"interval-plot\"><figcaption>{}</figcaption><ul class=\"plot-legend\">",
        encode_text(&plot.title)
    )
    .unwrap();
    for index in 0..2 {
        write!(
            out,
            "<li><span class=\"swatch series-{index}\"></span>{} [{unit}]</li>",
            label(index)
        )
        .unwrap();
    }
    out.push_str("</ul><ol class=\"plot-rows\">");
    for row in &table.rows {
        write!(
            out,
            "<li><strong>{}</strong><div class=\"plot-pair\">",
            encode_text(row[plot.label_column].as_str().unwrap())
        )
        .unwrap();
        for (index, &column) in plot.series_columns.iter().enumerate() {
            let number = row[column].as_number().unwrap();
            let percent = 100.0 * number.as_f64().unwrap() / peak;
            write!(out, "<div class=\"plot-series\"><span class=\"plot-track\" aria-hidden=\"true\"><i class=\"series-{index}\" style=\"width:{percent:.3}%\"></i></span><span class=\"plot-value\"><span class=\"sr-only\">{} </span>{} {unit}</span></div>", label(index), encode_text(&copy.number(number))).unwrap();
        }
        out.push_str("</div></li>");
    }
    out.push_str("</ol></figure>");
}

pub(super) fn is_instance_column(name: &str) -> bool {
    matches!(
        name.trim().to_lowercase().as_str(),
        "instance"
            | "instancja"
            | "node"
            | "węzeł"
            | "project"
            | "projekt"
            | "project_id"
            | "project id"
            | "scope"
            | "zakres"
    )
}

fn measurement(
    out: &mut String,
    chapter: &Chapter,
    index: usize,
    table: &Table,
    sources: &HashMap<&str, &Source>,
    copy: &Copy,
    links: &SqlLinks,
    local: bool,
) {
    let id = table.anchor(chapter, index);
    // A chapter may discuss several instances, and a comparison can be attached
    // to just one brief. The measurement's own sources take precedence.
    let source_scope = |ids: &[String]| {
        let mut scopes = ids
            .iter()
            .map(|id| sources.get(id.as_str()).and_then(|s| s.instance.as_deref()));
        let first = scopes.next()?;
        first.filter(|_| scopes.all(|other| other == first))
    };
    let instance = if table.source_ids.is_empty() {
        chapter.instance.as_deref()
    } else {
        source_scope(&table.source_ids)
    };
    let column_instance = |i: usize| table.column_instances.get(i).and_then(Option::as_deref);
    let title = links.text(&table.title, instance, local);
    let width = table.columns.len();
    write!(
        out,
        "<figure class=\"measurement\" id=\"{id}\"><figcaption><h3>{title}</h3></figcaption>"
    )
    .unwrap();
    linked_prose(
        out,
        "measurement-description",
        &table.description,
        links,
        instance,
        local,
    );
    if let Some(plot) = &table.plot {
        interval_plot(out, table, plot, copy);
    }
    let label = html_escape::encode_double_quoted_attribute(&table.title);
    let caption = encode_text(&table.title);
    write!(out, "<div class=\"scroll-table\" role=\"region\" aria-label=\"{label}\" tabindex=\"0\"><table><caption class=\"sr-only\">{caption}</caption><thead><tr>").unwrap();
    let numeric: Vec<_> = (0..width)
        .map(|i| {
            table.rows.iter().any(|r| r[i].is_number())
                && table
                    .rows
                    .iter()
                    .all(|r| r[i].is_number() || r[i].is_null())
        })
        .collect();
    for (i, name) in table.columns.iter().enumerate() {
        let class = if numeric[i] { " class=\"num\"" } else { "" };
        write!(
            out,
            "<th scope=\"col\"{class}>{}</th>",
            links.text(name, column_instance(i).or(instance), local)
        )
        .unwrap();
    }
    let common_row_sources = table
        .row_source_ids
        .first()
        .filter(|first| table.row_source_ids.iter().all(|ids| ids == *first));
    let has_row_links =
        common_row_sources.is_none() && table.row_source_ids.iter().any(|ids| !ids.is_empty());
    if has_row_links {
        write!(
            out,
            "<th scope=\"col\">{}</th>",
            copy.word("Source", "Źródło")
        )
        .unwrap();
    }
    out.push_str("</tr></thead><tbody>");
    let instance_column = table
        .columns
        .iter()
        .position(|name| is_instance_column(name));
    for (row_no, cells) in table.rows.iter().enumerate() {
        out.push_str("<tr>");
        let row_instance = table
            .row_instances
            .get(row_no)
            .and_then(Option::as_deref)
            .or_else(|| {
                table
                    .row_source_ids
                    .get(row_no)
                    .and_then(|ids| source_scope(ids))
            })
            .or_else(|| {
                instance_column
                    .and_then(|i| cells[i].as_str())
                    .map(str::trim)
            });
        for (column, cell) in cells.iter().enumerate() {
            let (tag, attr) = if column == 0 && !numeric[column] {
                ("th", " scope=\"row\"")
            } else {
                ("td", "")
            };
            let class = if numeric[column] {
                " class=\"num\""
            } else {
                ""
            };
            let content = match cell {
                Value::Null => format!(
                    "<span class=\"missing\" aria-label=\"{}\">—</span>",
                    copy.word("Unavailable", "Brak pomiaru")
                ),
                Value::Number(n) => encode_text(&copy.number(n)).into_owned(),
                Value::String(s) => links.text(
                    s,
                    row_instance.or(column_instance(column)).or(instance),
                    local,
                ),
                _ => unreachable!(),
            };
            write!(out, "<{tag}{attr}{class}>{content}</{tag}>").unwrap();
        }
        if has_row_links {
            out.push_str("<td class=\"row-sources\">");
            for id in &table.row_source_ids[row_no] {
                let source = sources[id.as_str()];
                let description = format!("{} · {}", source.title, source.description);
                let label = html_escape::encode_double_quoted_attribute(&description);
                if let Some(href) = &source.href {
                    let short = copy.word("Measurement", "Pomiar");
                    write!(out, "<a class=\"source-link\" href=\"{href}\" title=\"{label}\" aria-label=\"{label}\">{short} <span aria-hidden=\"true\">↗</span></a>").unwrap();
                } else {
                    write!(
                        out,
                        "<span title=\"{label}\">{}</span>",
                        copy.word("Unavailable", "Brak")
                    )
                    .unwrap();
                }
            }
            out.push_str("</td>");
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table></div>");
    if table.rows.is_empty() {
        prose(
            out,
            "table-note",
            copy.word(
                "No matching measurements in this sample.",
                "W tej próbie brak pasujących pomiarów.",
            ),
        );
    } else if table.rows.iter().flatten().any(Value::is_null) {
        prose(
            out,
            "table-note",
            copy.word(
                "A dash indicates an unavailable measurement.",
                "Kreska oznacza brak dostępnego pomiaru.",
            ),
        );
    }
    linked_prose(out, "table-note", &table.note, links, instance, local);
    let row_ids: HashSet<_> = table.row_source_ids.iter().flatten().collect();
    let mut ids: Vec<_> = table
        .source_ids
        .iter()
        .filter(|id| !has_row_links || !row_ids.contains(*id))
        .collect();
    if let Some(shared) = common_row_sources {
        for id in shared {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    let selected: Vec<_> = ids.into_iter().map(|id| sources[id.as_str()]).collect();
    sources_list(out, &selected, copy, links, local);
    if has_row_links {
        let mut shown = HashSet::new();
        for id in table.row_source_ids.iter().flatten() {
            let source = sources[id.as_str()];
            if shown.insert(&source.description) && !source.description.is_empty() {
                linked_prose(
                    out,
                    "table-note",
                    &source.description,
                    links,
                    source.instance.as_deref(),
                    local,
                );
            }
        }
    }
    out.push_str("</figure>");
}

fn fragment_code(out: &mut String, fragments: &[SqlFragment], positions: bool, copy: &Copy) {
    out.push_str("<div class=\"sql-passages\">");
    for fragment in fragments {
        if positions {
            write!(
                out,
                "<p class=\"sql-location\">{} {}–{}</p>",
                copy.word("Characters", "Znaki"),
                fragment.first,
                fragment.last
            )
            .unwrap();
        }
        write!(
            out,
            "<pre><code>{}</code></pre>",
            encode_text(&fragment.text)
        )
        .unwrap();
    }
    out.push_str("</div>");
}

fn sql_example(
    out: &mut String,
    example: &SqlExample,
    source: &Source,
    copy: &Copy,
    links: &SqlLinks,
) {
    write!(
        out,
        "<section class=\"sql-example\" id=\"{}\"><h3>{}</h3><p class=\"measurement-description\">{} · SQL <code>{}</code></p>",
        example.id, links.text(&example.title, Some(&example.instance), false), encode_text(&example.instance), links.text(&example.sql_id, Some(&example.instance), false)
    ).unwrap();
    fragment_code(out, &example.fragments, false, copy);
    linked_prose(
        out,
        "narrative",
        &example.meaning,
        links,
        Some(&example.instance),
        false,
    );
    linked_prose(
        out,
        "narrative",
        &example.scaling,
        links,
        Some(&example.instance),
        false,
    );
    if let Some(r) = &example.recommendation {
        write!(
            out,
            r#"<section class="sql-recommendation"><h4>{}</h4>"#,
            copy.word("Proposed SQL change", "Proponowana zmiana SQL")
        )
        .unwrap();
        linked_prose(
            out,
            "narrative",
            &r.rationale,
            links,
            Some(&example.instance),
            false,
        );
        write!(
            out,
            "<p><strong>{}</strong> {}</p>",
            copy.word("Applicable when", "Warunek zastosowania"),
            links.text(&r.assumption, Some(&example.instance), false)
        )
        .unwrap();
        out.push_str(r#"<div class="sql-passages">"#);
        for (label, sql) in [
            (
                copy.word("Captured SQL fragment", "Fragment zachowanego SQL"),
                &r.before,
            ),
            (
                copy.word("Example replacement", "Przykładowy zapis po zmianie"),
                &r.after,
            ),
        ] {
            write!(
                out,
                "<h5>{label}</h5><pre><code>{}</code></pre>",
                encode_text(sql)
            )
            .unwrap();
        }
        out.push_str("</div></section>");
    }
    sources_list(out, &[source], copy, links, false);
    out.push_str("</section>");
}

fn chapter_section(out: &mut String, chapter: &Chapter, copy: &Copy, links: &SqlLinks) {
    let scope = chapter.instance.as_deref();
    write!(
        out,
        "<section class=\"chapter\" id=\"{}\"><h2>{}</h2>",
        chapter.id,
        links.text(&chapter.title, scope, false)
    )
    .unwrap();
    if chapter.question != chapter.title {
        linked_prose(out, "question", &chapter.question, links, scope, false);
    }
    linked_prose(out, "conclusion", &chapter.conclusion, links, scope, false);
    for paragraph in &chapter.paragraphs {
        linked_prose(out, "narrative", paragraph, links, scope, false);
    }
    let sources: HashMap<_, _> = chapter.sources.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut selected = HashSet::new();
    for index in 0..=chapter.tables.len() {
        for example in chapter
            .sql_examples
            .iter()
            .filter(|e| e.after_tables == index)
        {
            sql_example(
                out,
                example,
                sources[example.source_id.as_str()],
                copy,
                links,
            );
            selected.insert(&example.source_id);
        }
        if let Some(table) = chapter.tables.get(index) {
            measurement(out, chapter, index, table, &sources, copy, links, false);
            selected.extend(
                table
                    .source_ids
                    .iter()
                    .chain(table.row_source_ids.iter().flatten()),
            );
        }
    }
    let remaining: Vec<_> = chapter
        .sources
        .iter()
        .filter(|s| !selected.contains(&s.id))
        .collect();
    sources_list(out, &remaining, copy, links, false);
    if !chapter.limitation.is_empty() {
        write!(
            out,
            "<aside class=\"boundary\"><h3>{}</h3>",
            copy.word("What remains open", "Co pozostaje otwarte")
        )
        .unwrap();
        linked_prose(out, "narrative", &chapter.limitation, links, scope, false);
        out.push_str("</aside>");
    }
    if !chapter.action.is_empty() || !chapter.owner.is_empty() || !chapter.success.is_empty() {
        write!(
            out,
            "<section class=\"decision\"><h3>{}</h3>",
            copy.word("Next step", "Następny krok")
        )
        .unwrap();
        linked_prose(out, "action", &chapter.action, links, scope, false);
        for (label, value) in [
            (copy.word("Owner", "Odpowiada"), &chapter.owner),
            (
                copy.word("What to check", "Jak sprawdzić wynik"),
                &chapter.success,
            ),
        ] {
            if !value.is_empty() {
                write!(
                    out,
                    "<p class=\"assignment\"><strong>{label}</strong> {}</p>",
                    links.text(value, scope, false)
                )
                .unwrap();
            }
        }
        out.push_str("</section>");
    }
    out.push_str("</section>");
}

fn inventory_section(out: &mut String, scopes: &[InstanceInventory], copy: &Copy) {
    write!(
        out,
        r#"<section class="chapter" id="{INVENTORY_ANCHOR}"><h2>{}</h2>"#,
        copy.word(
            "SQL statements and wait events discussed",
            "Omówione zapytania SQL i zdarzenia oczekiwania"
        )
    )
    .unwrap();
    for scope in scopes {
        write!(out, "<section><h3>{}</h3>", encode_text(&scope.instance)).unwrap();
        if !scope.sql_ids.is_empty() {
            out.push_str("<h4>SQL_ID</h4><ul class=\"entity-index\">");
            for id in &scope.sql_ids {
                write!(
                    out,
                    r#"<li><a href="wykresy/{}"><code>{id}</code></a></li>"#,
                    scope.sql_file(id)
                )
                .unwrap();
            }
            out.push_str("</ul>");
        }
        if !scope.waits.is_empty() {
            write!(
                out,
                r#"<h4>{}</h4><ul class="entity-index">"#,
                copy.word("Wait events", "Zdarzenia oczekiwania")
            )
            .unwrap();
            for name in &scope.waits {
                write!(
                    out,
                    r#"<li><a href="wykresy/{}">{}</a></li>"#,
                    super::wait_filename(scope.ordinal, name),
                    encode_text(name)
                )
                .unwrap();
            }
            out.push_str("</ul>");
        }
        out.push_str("</section>");
    }
    out.push_str("</section>");
}

pub(super) fn render(public: &Value) -> Result<String, String> {
    let doc: Document = serde_json::from_value(public.clone())
        .map_err(|e| format!("Invalid public report: {e}"))?;
    validate(&doc)?;
    let copy = Copy(doc.language == "pl");
    let title = encode_text(&doc.title);
    let sections = copy.word("Contents", "Spis treści");
    let overview = copy.word("Overview", "Wprowadzenie");
    let top = copy.word("Back to top", "Na początek");
    let logo = STANDARD.encode(include_bytes!("../../img/ora-600.png"));
    let mut menu = format!("<ul><li><a href=\"#content\">{overview}</a></li>");
    for chapter in &doc.chapters {
        write!(
            menu,
            "<li><a href=\"#{}\">{}</a></li>",
            chapter.id,
            encode_text(&chapter.title)
        )
        .unwrap();
    }
    if !doc.inventory.is_empty() {
        write!(
            menu,
            r##"<li><a href="#{INVENTORY_ANCHOR}">{}</a></li>"##,
            copy.word(
                "SQL statements and wait events discussed",
                "Omówione zapytania SQL i zdarzenia oczekiwania"
            )
        )
        .unwrap();
    }
    menu.push_str("</ul>");
    let mut out = format!(
        r##"<!doctype html><html lang="{}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>{title} · ORA-600</title><style>{}"##,
        doc.language,
        include_str!("client_report_v2.css")
    );
    if !doc.gradient_intro.is_empty() {
        out.push_str(include_str!("../report/assets/report_signal_plot.css"));
        out.push_str(include_str!("../report/assets/report_gradient_intro.css"));
    }
    if doc
        .chapters
        .iter()
        .any(|chapter| !chapter.sql_examples.is_empty())
        || !doc.inventory.is_empty()
    {
        out.push_str(include_str!("client_report_v2_sql.css"));
    }
    // CSS highlighting follows deep links offline, without executing author-supplied scripts.
    for chapter in &doc.chapters {
        let id = &chapter.id;
        write!(out, "body:has(.chapter[id=\"{id}\"]:target, .chapter[id=\"{id}\"] :target) .toc a[href=\"#{id}\"]{{color:var(--accent);border-inline-start-color:var(--accent);background:var(--tint)}}").unwrap();
    }
    write!(out, r##"</style></head><body id="top"><a class="skip" href="#content">{}</a><header class="masthead"><div class="masthead-inner"><a href="#top" class="brand"><img src="data:image/png;base64,{logo}" width="478" height="230" alt="ORA-600 Database Whisperers"></a><span class="product">JAS-MIN</span></div></header><div class="layout" id="report-menu"><nav class="toc desk" aria-label="{sections}"><p>{sections}</p>{menu}</nav><div class="reading"><nav class="toc mobile" aria-label="{sections}"><details><summary>{sections}</summary>{menu}</details></nav><main id="content" tabindex="-1"><header class="intro">"##, copy.word("Skip to report", "Przejdź do raportu")).unwrap();
    if !doc.period.trim().is_empty() {
        write!(
            out,
            "<p class=\"period\"><span>{}</span> {}</p>",
            copy.word("Scope", "Zakres"),
            encode_text(&doc.period)
        )
        .unwrap();
    }
    write!(out, "<h1>{title}</h1>").unwrap();
    let links = SqlLinks::new(&doc.inventory);
    linked_prose(&mut out, "summary", &doc.summary, &links, None, false);
    out.push_str(&super::super::super::gradient_intro::render(
        &doc.gradient_intro,
        copy.0,
        |text, instance| links.text(text, Some(instance), false),
    ));
    out.push_str("</header>");
    for chapter in &doc.chapters {
        chapter_section(&mut out, chapter, &copy, &links);
    }
    if !doc.inventory.is_empty() {
        inventory_section(&mut out, &doc.inventory, &copy);
    }
    write!(out, r##"</main></div></div><footer><div class="footer-inner"><strong>ORA-600 <span>Database Whisperers</span></strong><a href="#top">{top} ↑</a></div></footer></body></html>"##).unwrap();
    Ok(out)
}

fn source_document(
    title: &str,
    description: &str,
    source_label: &str,
    body: &str,
    chapter: &str,
    anchor: &str,
    lang: &str,
    extra_css: &str,
    entry_file: &str,
) -> Result<String, String> {
    if !matches!(entry_file, "raport.html" | "discussion.html")
        || !matches!(lang, "pl" | "en")
        || !valid_anchor(anchor)
        || [title, description, chapter]
            .iter()
            .any(|s| s.trim().is_empty())
    {
        return Err("A source page needs a title, a scope and a valid return destination".into());
    }
    let copy = Copy(lang == "pl");
    let title = encode_text(title);
    let chapter_title = encode_text(chapter);
    let back_label = copy.word("Return to the measurement", "Wróć do pomiaru");
    let logo = STANDARD.encode(include_bytes!("../../img/ora-600.png"));
    let style = include_str!("client_report_v2.css");
    let mut page = format!(
        r##"<!doctype html><html lang="{lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>{title} · ORA-600</title><style>{style}{extra_css}</style></head><body><header class="masthead"><div class="masthead-inner"><a class="brand" href="../{entry_file}#{anchor}"><img src="data:image/png;base64,{logo}" width="478" height="230" alt="ORA-600 Database Whisperers"></a><span class="product">JAS-MIN</span></div></header><main class="source-page"><a class="source-return" href="../{entry_file}#{anchor}">← {back_label} · {chapter_title}</a><p class="period">{source_label}</p><h1>{title}</h1>"##
    );
    prose(&mut page, "summary", description);
    page.push_str(body);
    write!(page, r##"</main><footer><div class="footer-inner"><strong>ORA-600 <span>Database Whisperers</span></strong><a href="../{entry_file}#{anchor}">← {back_label}</a></div></footer></body></html>"##).unwrap();
    Ok(page)
}

/// Text comes exclusively from the captured SQL dictionary of this instance.
fn full_sql(out: &mut String, captured: Option<&str>, copy: &Copy) {
    write!(
        out,
        r#"<section class="sql-passages full-sql"><h2>{}</h2>"#,
        copy.word(
            "Full SQL text retained in AWR",
            "Pełna treść SQL zachowana w AWR"
        )
    )
    .unwrap();
    if let Some(text) = captured.filter(|s| !s.trim().is_empty()) {
        write!(out, "<pre><code>{}</code></pre>", encode_text(text)).unwrap();
    } else {
        prose(
            out,
            "table-note",
            copy.word(
                "The AWR capture for this instance has no SQL text for this statement.",
                "W danych AWR tej instancji brakuje tekstu tego zapytania.",
            ),
        );
    }
    out.push_str("</section>");
}

pub(super) struct SqlDetail<'a> {
    pub plot: Option<&'a str>,
    pub reconstructed: bool,
    pub text: Option<&'a str>,
}

impl SqlDetail<'_> {
    fn chart_title(&self, copy: &Copy) -> &'static str {
        if self.reconstructed {
            copy.word(
                "JAS-MIN chart from captured AWR intervals",
                "Wykres JAS-MIN z zachowanych interwałów AWR",
            )
        } else {
            copy.word("Original JAS-MIN chart", "Oryginalny wykres JAS-MIN")
        }
    }

    fn render(&self, body: &mut String, copy: &Copy) {
        if let Some(plot) = self.plot {
            write!(body, r#"<section class="native-figure"><h2>{}</h2>{plot}<noscript><p>{}</p></noscript></section>"#,
                self.chart_title(copy), copy.word("Enable JavaScript to explore the chart.", "Włącz JavaScript, aby obejrzeć wykres.")).unwrap();
        } else {
            prose(body, "table-note", copy.word("No chart or interval series is available for this statement on this instance.",
                "W danych tej instancji brakuje wykresu i serii interwałowej dla tego zapytania."));
        }
        full_sql(body, self.text, copy);
    }
}

pub(super) fn source_excerpt_page(
    example: &SqlExample,
    detail: &SqlDetail,
    chapter: &str,
    lang: &str,
) -> Result<String, String> {
    let copy = Copy(lang == "pl");
    let mut body = String::new();
    fragment_code(&mut body, &example.fragments, true, &copy);
    detail.render(&mut body, &copy);
    source_document(
        &format!("{} · SQL {}", example.instance, example.sql_id),
        copy.word(
            "Character positions refer to the SQL text retained for this instance in the AWR capture. Separate passages may omit intervening text.",
            "Pozycje znaków odnoszą się do tekstu SQL zachowanego w AWR dla tej instancji. Pomiędzy fragmentami mogą występować pominięte instrukcje lub klauzule.",
        ),
        copy.word("SQL text in AWR", "Tekst SQL w AWR"),
        &body,
        chapter,
        &example.id,
        lang,
        include_str!("client_report_v2_sql.css"),
        "raport.html",
    )
}

/// Table-only source view for a selected capture whose native chart is unavailable.
pub(super) fn source_table_page(
    title: &str,
    description: &str,
    tables: &[Value],
    instance: Option<&InstanceInventory>,
    statement: Option<&SqlDetail>,
    chapter: &str,
    anchor: &str,
    lang: &str,
) -> Result<String, String> {
    if tables.is_empty() {
        return Err("A local AWR source needs measurements".into());
    }
    let doc: Document = serde_json::from_value(serde_json::json!({
        "title": title, "language":lang, "chapters":[{"id":"measurements","title":title,"instance":instance.map(|scope| &scope.instance), "tables":tables}]
    })).map_err(|e| format!("Invalid source table: {e}"))?;
    validate(&doc)?;
    let copy = Copy(lang == "pl");
    let section = &doc.chapters[0];
    let mut body = String::new();
    let links = SqlLinks::new(instance);
    for (index, table) in section.tables.iter().enumerate() {
        measurement(
            &mut body,
            section,
            index,
            table,
            &HashMap::new(),
            &copy,
            &links,
            true,
        );
    }
    if let Some(detail) = statement {
        full_sql(&mut body, detail.text, &copy);
    }
    source_document(
        title,
        description,
        copy.word("Source AWR measurements", "Źródłowe pomiary AWR"),
        &body,
        chapter,
        anchor,
        lang,
        include_str!("client_report_v2_sql.css"),
        "raport.html",
    )
}

/// Each native figure belongs to this event and instance. Foreground and
/// background AWR sections can both contain the same event name.
pub(super) fn source_wait_page(
    title: &str,
    scope: &str,
    charts: &[super::native_charts::NativeChart],
    chapter: &str,
    lang: &str,
    entry_file: &str,
) -> Result<String, String> {
    let copy = Copy(lang == "pl");
    let mut body = String::new();
    for (slot, chart) in charts.iter().enumerate() {
        let (id, heading) = match chart.kind {
            "foreground" => (
                "fg",
                copy.word("Foreground waits", "Oczekiwania sesji pierwszoplanowych"),
            ),
            "background" => (
                "bg",
                copy.word("Background waits", "Oczekiwania procesów tła"),
            ),
            _ => return Err("Unsupported wait chart class".into()),
        };
        write!(
            body,
            r#"<section class="native-figure" id="{id}"><h2>{heading}</h2>{}</section>"#,
            super::native_charts::numbered_plot_markup(chart, slot)
        )
        .unwrap();
    }
    if charts.is_empty() {
        prose(&mut body, "table-note", copy.word(
            "An original JAS-MIN chart for this event could not be verified in this instance's capture.",
            "W danych tej instancji nie udało się potwierdzić oryginalnego wykresu JAS-MIN dla tego zdarzenia."));
    }
    source_document(
        title,
        scope,
        if charts.is_empty() {
            copy.word("Wait event", "Zdarzenie oczekiwania")
        } else {
            copy.word(
                "Original JAS-MIN wait charts",
                "Oryginalne wykresy oczekiwań JAS-MIN",
            )
        },
        &body,
        chapter,
        INVENTORY_ANCHOR,
        lang,
        "",
        entry_file,
    )
}

/// The chart markup must come from the native figure renderer, never authored report text.
pub(super) fn source_chart_page(
    title: &str,
    scope: &str,
    detail: &SqlDetail,
    chapter_title: &str,
    anchor: &str,
    lang: &str,
    entry_file: &str,
) -> Result<String, String> {
    let copy = Copy(lang == "pl");
    let mut body = String::new();
    detail.render(&mut body, &copy);
    source_document(
        title,
        scope,
        copy.word("SQL measurements", "Pomiary SQL"),
        &body,
        chapter_title,
        anchor,
        lang,
        include_str!("client_report_v2_sql.css"),
        entry_file,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::{Html, Selector};
    use serde_json::json;

    fn doc() -> Value {
        json!({
            "language":"pl", "title":"Workload <sample>", "period":"2026-01-01", "private_note":"private-only",
            "chapters":[{
                "id":"workload", "title":"Daily batch", "conclusion":"Measure comparable executions.",
                "tables":[{"id":"measured", "title":"Sample", "columns":["SQL", "Elapsed [s]","CPU [s]"],
                    "rows":[["statement-1", 1234.567, null],["statement-2", 32, 12.4]], "row_source_ids":[["original"],[]]}],
                "sources":[{"id":"original","title":"Original chart","href":"wykresy/statement-1.html"},
                    {"id":"local","title":"Timing series","href":"#measured"},
                    {"id":"unavailable","title":"Execution plan","description":"Source capture unavailable."}],
                "owner":"Application team", "action":"Compare completed batches.","success":"Track elapsed per completed batch."
            }]
        })
    }

    #[test]
    fn renders_typed_evidence_with_source_at_the_contributing_row() {
        let page = render(&doc()).unwrap();
        let tree = Html::parse_document(&page);
        let find = |s: &str| tree.select(&Selector::parse(s).unwrap()).count();
        assert_eq!(find("main h1"), 1);
        assert_eq!(find("section#workload > h2"), 1);
        assert_eq!(find("figure#measured th[scope='col']"), 4);
        assert_eq!(
            find("#measured tbody tr:first-child a[href='wykresy/statement-1.html']"),
            1
        );
        assert_eq!(find("#measured tbody tr:nth-child(2) a"), 0);
        assert_eq!(find("a[href='#measured']"), 1);
        assert_eq!(find("script, iframe, a[href^='http']"), 0);
        assert!(page.contains("1 234,57") && page.contains("12,4"));
        assert!(page.contains("Brak pomiaru") && page.contains("Source capture unavailable."));
        assert!(!page.contains("private-only") && page.contains("Workload &lt;sample&gt;"));
    }

    #[test]
    fn shared_row_sources_render_as_one_table_link_on_small_screens() {
        let mut v = doc();
        let t = &mut v["chapters"][0]["tables"][0];
        t["row_source_ids"] = json!([["original"], ["original"]]);
        t["source_ids"] = json!(["original"]);
        let html = render(&v).unwrap();
        let page = Html::parse_document(&html);
        assert_eq!(
            page.select(&Selector::parse("#measured thead th").unwrap())
                .count(),
            3
        );
        assert_eq!(
            page.select(&Selector::parse("#measured a[href='wykresy/statement-1.html']").unwrap())
                .count(),
            1
        );
    }

    #[test]
    fn errors_on_dangling_links_unsafe_paths_and_unlabeled_measurements() {
        for link in [
            "#missing",
            "https://example.invalid",
            "wykresy/../no.html",
            "wykresy/a%2f..",
            "wykresy/x.html#\u{22}",
        ] {
            let mut v = doc();
            v["chapters"][0]["sources"][0]["href"] = json!(link);
            assert!(render(&v).is_err(), "{link}");
        }
        let mut v = doc();
        v["chapters"][0]["tables"][0]["columns"][1] = json!("");
        assert!(render(&v).is_err());
        let mut v = doc();
        v["chapters"][0]["tables"][0]["row_source_ids"][0][0] = json!("missing-source");
        assert!(render(&v).is_err());
    }

    #[test]
    fn single_measurement_scopes_the_entire_figure_and_never_borrows_a_card() {
        let id = "00000000abcde";
        let event = "log file sync";
        let foreign = "log file parallel write";
        let text = format!("{id}, {event}, {foreign}");
        let mut v = doc();
        v["inventory"] = json!([
            {"ordinal":1,"instance":"Node A","sql_ids":[id],"waits":[event]},
            {"ordinal":2,"instance":"Node B","sql_ids":[id],"waits":[event,foreign]}
        ]);
        v["chapters"][0]["instance"] = json!("Node B");
        v["chapters"][0]["tables"] = json!([{"id":"measured","title":text,"description":text,
            "columns":[text],"rows":[[text]],"note":text,"source_ids":["original"]}]);
        v["chapters"][0]["sources"] =
            json!([{"id":"original","instance":"Node A","title":text,"description":text}]);
        let report = Html::parse_document(&render(&v).unwrap());
        let figure = report
            .select(&Selector::parse("#measured").unwrap())
            .next()
            .unwrap();
        let internal: Vec<_> = figure
            .select(&Selector::parse("a").unwrap())
            .map(|a| a.value().attr("href").unwrap())
            .collect();
        assert_eq!(internal.len(), 14);
        assert!(internal.iter().all(|href| href.contains("-p1-")));
        assert!(figure.text().any(|value| value.contains(foreign)));
    }

    #[test]
    fn comparative_headers_cells_and_merged_source_rows_retain_their_own_scope() {
        let event = "log file sync";
        let mut v = doc();
        v["inventory"] = json!([
            {"ordinal":1,"instance":"Node A","sql_ids":[],"waits":[event]},
            {"ordinal":2,"instance":"Node B","sql_ids":[],"waits":[event]}
        ]);
        v["chapters"][0]["instance"] = json!("Node B");
        v["chapters"][0]["sources"] = json!([
          {"id":"a","instance":"Node A","title":"Source A","description":"Wait time."},
          {"id":"b","instance":"Node B","title":"Source B","description":"Wait time."}
        ]);
        v["chapters"][0]["tables"] = json!([
            {"id":"across","title":event,"columns":[event,event],"column_instances":["Node A","Node B"],"rows":[[event,event]],"source_ids":["a","b"]},
            {"id":"merged","title":event,"columns":["Event"],"rows":[[event],[event]],"row_source_ids":[["b"],["a"]],"source_ids":["a","b"]}
        ]);
        let report = Html::parse_document(&render(&v).unwrap());
        let targets = |selector: &str| -> Vec<&str> {
            report
                .select(&Selector::parse(selector).unwrap())
                .map(|a| a.value().attr("href").unwrap())
                .collect()
        };
        let a = format!("wykresy/{}", super::super::wait_filename(1, event));
        let b = format!("wykresy/{}", super::super::wait_filename(2, event));
        for selector in [
            "#across h3 a",
            "#across th[scope=col] a",
            "#across tbody a",
            "#merged h3 a",
        ] {
            assert_eq!(targets(selector), [a.as_str(), b.as_str()]);
        }
        assert_eq!(targets("#merged th[scope=row] a"), [b.as_str(), a.as_str()]);
    }

    #[test]
    fn links_remain_instance_scoped_in_shared_rows_and_offline_family_pages() {
        let id = "00000000abcde";
        let mut v = doc();
        v["inventory"] = json!([
            {"ordinal":1,"instance":"Instance A","sql_ids":[id],"waits":["sample wait <A>"]},
            {"ordinal":2,"instance":"Instance B","sql_ids":[id],"waits":[]}
        ]);
        v["chapters"][0]["conclusion"] = json!(format!("Review {id}."));
        v["chapters"][0]["tables"][0]["columns"] = json!(["Instance", "SQL_ID", "Calls"]);
        v["chapters"][0]["tables"][0]["rows"] =
            json!([["Instance A", id, 4], ["Instance B", id, 2]]);
        let page = render(&v).unwrap();
        let doc = Html::parse_document(&page);
        let matches = |selector: &str| doc.select(&Selector::parse(selector).unwrap()).count();
        assert_eq!(matches(".conclusion a"), 2);
        for node in 1..=2 {
            let file = format!("sql-p{node}-{id}.html");
            assert_eq!(
                matches(&format!(
                    "#measured tbody tr:nth-child({node}) a[href='wykresy/{file}']"
                )),
                1
            );
            assert_eq!(
                matches(&format!("#{INVENTORY_ANCHOR} a[href='wykresy/{file}']")),
                1
            );
            let scope: InstanceInventory =
                serde_json::from_value(v["inventory"][node - 1].clone()).unwrap();
            let family = json!({"title":"Members", "columns":["SQL_ID"], "rows":[[id]]});
            let details = source_table_page(
                "Members",
                &scope.instance,
                &[family],
                Some(&scope),
                None,
                "Workload",
                "measured",
                "en",
            )
            .unwrap();
            let local = Html::parse_document(&details);
            assert_eq!(
                local
                    .select(&Selector::parse(&format!("tbody a[href='{file}']")).unwrap())
                    .count(),
                1
            );
        }
        assert!(page.contains("sample wait &lt;A&gt;"));
    }

    #[test]
    fn full_event_names_link_by_instance_in_narrative_tables_and_index() {
        let mut v = doc();
        let event = "SQL*Net message from dblink";
        let prefix = "SQL*Net message";
        v["inventory"] = json!([
            {"ordinal":1,"instance":"Instance A","sql_ids":[],"waits":[event, prefix]},
            {"ordinal":2,"instance":"Instance B","sql_ids":[],"waits":[event]}
        ]);
        v["chapters"][0]["conclusion"] = json!(format!("The wait was {event}."));
        v["chapters"][0]["tables"][0]["columns"] = json!(["Instance", "Event", "Seconds"]);
        v["chapters"][0]["tables"][0]["rows"] = json!([
            ["Instance A", event.to_uppercase(), 4],
            ["Instance B", event, 2]
        ]);
        let page = Html::parse_document(&render(&v).unwrap());
        let matches = |query: &str| page.select(&Selector::parse(query).unwrap()).count();
        assert_eq!(matches(".conclusion a"), 2);
        for node in 1..=2 {
            let file = super::super::wait_filename(node, event);
            assert_eq!(
                matches(&format!(
                    "#measured tbody tr:nth-child({node}) a[href='wykresy/{file}']"
                )),
                1
            );
            assert_eq!(
                matches(&format!("#{INVENTORY_ANCHOR} a[href='wykresy/{file}']")),
                1
            );
            assert_eq!(matches(&format!(".conclusion a[href='wykresy/{file}']")), 1);
        }
    }

    #[test]
    fn explicit_instance_never_links_to_a_different_instances_sql_or_wait() {
        let id = "00000000abcdf";
        let event = "log file sync";
        let scope = InstanceInventory {
            ordinal: 2,
            instance: "Instance B".into(),
            sql_ids: BTreeSet::from([id.into()]),
            waits: BTreeSet::from([event.into()]),
        };
        let links = SqlLinks::new([&scope]);
        let value = format!("{event} < {id}");
        assert_eq!(
            links.text(&value, Some("Instance A"), false),
            format!("{event} &lt; {id}")
        );
        for instance in [None, Some("Instance B")] {
            let text = links.text(&value, instance, false);
            assert_eq!(text.matches("<a href=").count(), 2);
            assert!(text.contains("wykresy/sql-p2-") && text.contains("wykresy/wait-p2-"));
        }
    }

    #[test]
    fn event_card_shows_both_original_classes_and_reports_a_missing_original() {
        let chart = |kind| super::super::native_charts::NativeChart {
            filename: String::new(),
            kind,
            instance_label: "Instance A".into(),
            sql_id: None,
            begin: "1".into(),
            end: "2".into(),
            points: 1,
            plot: json!({"data":[],"layout":{}}),
        };
        let page = source_wait_page(
            "Wait",
            "Instance A",
            &[chart("foreground"), chart("background")],
            "Index",
            "en",
            "discussion.html",
        )
        .unwrap();
        let doc = Html::parse_document(&page);
        let matches = |query: &str| doc.select(&Selector::parse(query).unwrap()).count();
        assert_eq!(
            matches("#fg script[data-jasmin-target], #bg script[data-jasmin-target]"),
            2
        );
        assert_eq!(matches("script[src='plotly.min.js']"), 1);
        let missing =
            source_wait_page("Wait", "Instance A", &[], "Index", "en", "discussion.html").unwrap();
        assert!(
            missing.contains("could not be verified") && !missing.contains("src=\"plotly.min.js\"")
        );
    }

    #[test]
    fn sql_details_preserve_the_entire_capture_as_text_and_label_unavailable_data() {
        let sql = "select '<script>x()</script>'\nfrom sample where value < 4";
        for (reconstructed, label) in [
            (true, "JAS-MIN chart from captured AWR intervals"),
            (false, "Original JAS-MIN chart"),
        ] {
            let detail = SqlDetail {
                plot: Some("<figure></figure>"),
                text: Some(sql),
                reconstructed,
            };
            let page = source_chart_page(
                "Instance A · SQL",
                "Sample period",
                &detail,
                "SQL",
                "measured",
                "en",
                "raport.html",
            )
            .unwrap();
            let doc = Html::parse_document(&page);
            assert!(page.contains(label));
            assert_eq!(
                doc.select(&Selector::parse(".full-sql pre code").unwrap())
                    .next()
                    .unwrap()
                    .text()
                    .collect::<String>(),
                sql
            );
            assert_eq!(doc.select(&Selector::parse("script").unwrap()).count(), 0);
        }
        let unavailable = SqlDetail {
            plot: None,
            text: None,
            reconstructed: false,
        };
        let page = source_chart_page(
            "Instance A · SQL",
            "Sample period",
            &unavailable,
            "SQL",
            "measured",
            "en",
            "raport.html",
        )
        .unwrap();
        assert!(page.contains("No chart or interval series") && page.contains("has no SQL text"));
    }

    #[test]
    fn interval_figure_draws_measured_totals_only_for_aligned_single_executions() {
        let mut v = doc();
        let mut table = json!({"title":"Individual intervals","columns":["Interval","Exec","Elapsed [s]","CPU [s]"],
            "rows":[["01–02",1,4.5,4.2],["02–03",1,7.1,6.8]],
            "plot":{"kind":"interval_cpu","unit":"s","title":"Recorded time",
                    "label_column":0,"executions_column":1,"series_columns":[2,3],"series_labels":["Elapsed","CPU"]}});
        v["chapters"][0]["sources"] = json!([]);
        v["chapters"][0]["tables"] = json!([table.clone()]);
        let html = render(&v).unwrap();
        assert!(html.contains("class=\"interval-plot\"") && html.contains("4,5 s"));
        table["rows"][0][1] = json!(0);
        v["chapters"][0]["tables"] = json!([table]);
        assert!(!render(&v).unwrap().contains("class=\"interval-plot\""));
    }

    #[test]
    fn local_awr_table_distinguishes_source_and_returns_to_the_same_measurement() {
        let table =
            json!({"title":"Elapsed","columns":["Period","Elapsed [s]"],"rows":[["01–02",125.71]]});
        let html = source_table_page(
            "Batch",
            "Instance 1, 01–02",
            &[table],
            None,
            None,
            "Daily job",
            "daily-t1",
            "en",
        )
        .unwrap();
        assert!(html.contains("Source AWR measurements") && html.contains("125.71"));
        assert!(
            html.contains("href=\"../raport.html#daily-t1\"")
                && !html.contains("Original JAS-MIN chart")
        );
    }

    #[test]
    fn language_and_zero_do_not_turn_missing_or_small_measurements_into_zero() {
        let mut v = doc();
        v["language"] = json!("en");
        v["chapters"][0]["tables"][0]["rows"][1] = json!(["statement-2", 0, 0.0001]);
        let page = render(&v).unwrap();
        assert!(
            page.contains("1,234.57") && page.contains("&lt; 0.01") && page.contains(">0</td>")
        );
        assert!(page.contains("A dash indicates an unavailable measurement."));
    }
}
