//! Reuse the figures emitted by the classic reporter, within each selected project.
//! Public pages receive the plot specifications alone, never the surrounding source HTML.
use super::super::AnalysisRuntime;
use super::{invalid, Selection, ToolResult};
use crate::awr::{AWRSCollection, WaitEvents};
use crate::measurements::{db_load_rate, timestamp, DbLoadMetric};
use crate::report::classic::SqlChartWindow;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const EVENT: &str = "enq: TX - row lock contention";
const TX_PATH: &str = "fg/fg_enq_TX_-_row_lock_contention.html";
const DB_TIME: &str = "DB Time (s/s)";
const DB_CPU: &str = "DB CPU (s/s)";
const SQL_SERIES: &[&str] = &[
    "# Buffer Gets",
    "# Physical Reads",
    "% Ela Time as DB Time",
    "% CPU of Ela",
    "% IO of Ela",
    "% CPU Time as DB CPU",
    "% IO Time as DB IO Wait",
    "% Gets as Total Gets",
    "% Phys Reads as Total Disk Reads",
    "(s) Elapsed Time per Exec",
    "(s) CPU Time",
    "(s) User I/O Wait Time",
    "# Executions",
];

pub(super) struct NativeChart {
    pub(super) filename: String,
    pub(super) kind: &'static str,
    pub(super) instance_label: String,
    pub(super) sql_id: Option<String>,
    pub(super) begin: String,
    pub(super) end: String,
    pub(super) points: usize,
    pub(super) plot: Value,
}

pub(super) const VIEWER_JS: &[u8] = br#"(() => {
  for (const source of document.querySelectorAll('script[data-jasmin-target]')) {
    const figure = JSON.parse(source.textContent);
    Plotly.newPlot(source.dataset.jasminTarget, figure.data, figure.layout, {
      responsive: true, displaylogo: false, showLink: false,
      modeBarButtonsToRemove: ['sendDataToCloud']
    });
  }
})();"#;

pub(super) fn plot_markup(chart: &NativeChart) -> String {
    numbered_plot_markup(chart, 0)
}

pub(super) fn numbered_plot_markup(chart: &NativeChart, slot: usize) -> String {
    let figure = chart
        .plot
        .to_string()
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    let suffix = if slot == 0 {
        String::new()
    } else {
        format!("-{slot}")
    };
    let scripts = if slot == 0 {
        r#"<script src="plotly.min.js" defer></script><script src="jasmin-plot.js" defer></script>"#
    } else {
        ""
    };
    format!(
        r#"<div class="jasmin-native-plot" id="jasmin-native-plot{suffix}"></div><script type="application/json" id="jasmin-figure{suffix}" data-jasmin-target="jasmin-native-plot{suffix}">{figure}</script>{scripts}"#
    )
}

pub(super) fn plotly_js() -> &'static [u8] {
    static JS: OnceLock<String> = OnceLock::new();
    JS.get_or_init(|| {
        let sources = plotly::Plot::offline_js_sources();
        let (library, _) = sources
            .strip_prefix("<script type=\"text/javascript\">")
            .and_then(|html| html.split_once("</script>"))
            .expect("plotly's bundled offline scripts");
        library.to_owned()
    })
    .as_bytes()
}

// Only allow measurements, the classic reporter's axis captions and its presentation settings.
// Input files can also contain SQL text, filesystem paths, cross-links and executable script.
fn style_value(v: &Value) -> bool {
    match v {
        Value::Bool(_) | Value::Number(_) | Value::Null => true,
        Value::Array(a) => a.iter().all(style_value),
        Value::Object(o) => o.iter().all(|(k, v)| style_key(k) && style_value(v)),
        Value::String(s) => {
            matches!(
                s.as_str(),
                "#" | "%"
                    | "(s) per Exec"
                    | "(s/s)"
                    | "Total Wait (s)"
                    | "Wait count #"
                    | "% DBTime"
                    | "% Wait Event ms"
                    | "Electric"
                    | "legendonly"
                    | "h"
                    | "x"
                    | "lines+text"
                    | "lines+markers"
                    | "markers"
                    | "overlay"
                    | "tozero"
                    | "diamond"
                    | "diamond-tall"
                    | "star-diamond"
                    | "square"
                    | "scatter"
                    | "histogram"
                    | "heatmap"
                    | "box"
            ) || (s.len() == 7
                && s.starts_with('#')
                && s[1..].bytes().all(|c| c.is_ascii_hexdigit()))
                || (s.len() == 2
                    && matches!(s.as_bytes()[0], b'x' | b'y')
                    && s.as_bytes()[1].is_ascii_digit())
        }
    }
}

fn style_key(key: &str) -> bool {
    matches!(
        key,
        "type"
            | "hovermode"
            | "height"
            | "grid"
            | "rows"
            | "columns"
            | "range"
            | "domain"
            | "title"
            | "text"
            | "showgrid"
            | "showticklabels"
            | "anchor"
            | "zeroline"
            | "rangemode"
            | "bargap"
            | "barmode"
            | "mode"
            | "marker"
            | "opacity"
            | "color"
            | "symbol"
            | "visible"
            | "showlegend"
            | "showscale"
            | "hoverongaps"
            | "colorscale"
            | "reversescale"
            | "orientation"
            | "autobinx"
            | "boxmean"
    ) || ["xaxis", "yaxis"].iter().any(|p| {
        key.strip_prefix(p)
            .is_some_and(|n| n.is_empty() || (n.len() == 1 && n.as_bytes()[0].is_ascii_digit()))
    })
}

fn bucket(s: &str) -> bool {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        regex::Regex::new(concat!(
            r"^(?:[0-9]+:\s*)?(?:[<>=]{0,2}\s*)?",
            r"[0-9]+(?:\.[0-9]+)?\s*(?:us|ms|s)",
            r"(?:\s*-\s*[0-9]+\s*(?:us|ms|s))?$"
        ))
        .expect("valid wait-time bucket pattern")
    });
    pattern.is_match(s)
}

fn numeric(v: &Value) -> bool {
    v.is_null() || v.is_number()
}

fn numbers(v: &Value) -> bool {
    v.as_array().is_some_and(|a| a.iter().all(numeric))
}

fn safe_trace(trace: &Value, kind: &str, axis: &[Value], event: Option<&str>) -> bool {
    let Some(obj) = trace.as_object() else {
        return false;
    };
    let typ = trace["type"].as_str().unwrap_or("");
    let name = trace["name"].as_str().unwrap_or("");
    let named = match kind {
        "wait" => {
            matches!(name, "" | "%" | "Total Wait Time (s)" | "Wait Count")
                || Some(name) == event
                || bucket(name)
        }
        "cpu" => matches!(name, DB_TIME | DB_CPU),
        "sql" => SQL_SERIES.contains(&name),
        _ => false,
    };
    named
        && obj.iter().all(|(k, v)| match k.as_str() {
            "name" => true,
            "legendgroup" => v.as_str().is_some_and(|s| Some(s) == event || bucket(s)),
            "x" if matches!(typ, "scatter" | "heatmap") => v.as_array().is_some_and(|a| a == axis),
            "x" => numbers(v),
            "y" if typ == "heatmap" => v
                .as_array()
                .is_some_and(|a| a.iter().all(|b| b.as_str().is_some_and(bucket))),
            "y" if typ == "box" => v.as_array().is_some_and(|a| {
                a.iter()
                    .all(|b| b.as_str().is_some_and(|s| Some(s) == event || bucket(s)))
            }),
            "y" => numbers(v),
            "z" => typ == "heatmap" && v.as_array().is_some_and(|a| a.iter().all(numbers)),
            _ => style_key(k) && style_value(v),
        })
}

fn matches_series(trace: &Value, expected: impl Iterator<Item = Option<f64>>) -> bool {
    matches_numbers(&trace["y"], expected)
}

fn matches_numbers(values: &Value, expected: impl Iterator<Item = Option<f64>>) -> bool {
    let Some(ys) = values.as_array() else {
        return false;
    };
    let mut iter = expected;
    ys.iter().all(|observed| match (observed, iter.next()) {
        (Value::Null, Some(None)) => true,
        (v, Some(Some(want))) => v
            .as_f64()
            .is_some_and(|v| (want - v).abs() <= 1e-8_f64.max(want.abs() * 1e-7)),
        _ => false,
    }) && iter.next().is_none()
}

fn payloads(html: &str) -> impl Iterator<Item = &str> {
    html.rmatch_indices("Plotly.newPlot(")
        .filter_map(|(pos, marker)| {
            let start = html[pos + marker.len()..]
                .strip_prefix("graph_div,")?
                .trim_start();
            Some(start)
        })
}

fn decode(input: &str) -> Option<(Value, &str)> {
    let mut reader = serde_json::Deserializer::from_str(input).into_iter::<Value>();
    let value = reader.next()?.ok()?;
    Some((value, &input[reader.byte_offset()..]))
}

fn cpu_figure(html: &str, collection: &AWRSCollection, axis: &[Value]) -> Option<Value> {
    // The classic dashboard opens with DB Time and DB CPU. Read just these two traces.
    // Other panels can contain SQL, file names, and hundreds of unrelated series.
    for input in payloads(html) {
        let Some(data) = input.strip_prefix("{\"data\":[") else {
            continue;
        };
        let Some((time, tail)) = decode(data) else {
            continue;
        };
        if time["name"] != DB_TIME {
            continue;
        }
        let (cpu, _) = decode(tail.trim_start().strip_prefix(',')?.trim_start())?;
        if cpu["name"] != DB_CPU {
            continue;
        }
        for (trace, metric) in [(&time, DbLoadMetric::DbTime), (&cpu, DbLoadMetric::DbCpu)] {
            if !safe_trace(trace, "cpu", axis, None)
                || !matches_series(
                    trace,
                    collection
                        .awrs
                        .iter()
                        .map(|a| db_load_rate(a, metric).map(|v| v.per_second)),
                )
            {
                return None;
            }
        }
        return Some(
            json!({"data":[time,cpu],"layout":{"height":520,"hovermode":"x",
            "xaxis":{"type":"category","nticks":8},"yaxis":{"title":{"text":"s/s"},"rangemode":"tozero"}}}),
        );
    }
    None
}

fn captured_figure(html: &str, axis: &[Value], sql: Option<&str>) -> Option<(Value, bool)> {
    let id = sql.unwrap_or(EVENT);
    // Classic SQL cards without attached SQL text are plain Plotly documents.
    let bare_sql = sql.is_some() && !html.contains("<h2");
    if !bare_sql && !html.contains(&format!(">{id}</h2>")) {
        return None;
    }
    let (figure, _) = decode(payloads(html).next()?)?;
    let kind = if sql.is_some() { "sql" } else { "wait" };
    let figure = public_figure(figure, kind, axis, sql.is_none().then_some(EVENT))?;
    Some((figure, bare_sql))
}

fn public_figure(
    mut figure: Value,
    kind: &str,
    axis: &[Value],
    event: Option<&str>,
) -> Option<Value> {
    let traces = figure["data"].as_array()?;
    let layout = &figure["layout"];
    if !layout.is_object()
        || !style_value(layout)
        || !traces.iter().all(|t| safe_trace(t, kind, axis, event))
    {
        return None;
    }
    let data = figure["data"].take();
    let layout = figure["layout"].take();
    Some(json!({"data": data, "layout": layout}))
}

fn native_figure(
    html: &str,
    collection: &AWRSCollection,
    axis: &[Value],
    sql: Option<&str>,
) -> Option<Value> {
    let (figure, bare_sql) = captured_figure(html, axis, sql)?;
    let traces = figure["data"].as_array()?;
    let agrees = if let Some(id) = sql {
        let measured = traces.iter().find(|t| t["name"] == "(s) CPU Time")?;
        if bare_sql && !measured["y"].as_array()?.iter().any(Value::is_number) {
            return None;
        }
        matches_series(
            measured,
            collection
                .awrs
                .iter()
                .map(|a| a.sql_cpu_time.get(id).map(|r| r.cpu_time_exec_s)),
        )
    } else {
        let measured = traces.iter().find(|t| t["name"] == "Total Wait Time (s)")?;
        matches_series(
            measured,
            collection.awrs.iter().map(|a| {
                a.foreground_wait_events
                    .iter()
                    .find(|e| e.event == EVENT)
                    .map(|e| e.total_wait_time_s)
            }),
        )
    };
    agrees.then_some(figure)
}

fn read_source(directory: &Path, name: &str) -> Option<String> {
    let root = directory.canonicalize().ok()?;
    let source = root.join(name).canonicalize().ok()?;
    if !source.starts_with(&root) || !source.is_file() {
        return None;
    }
    fs::read_to_string(source).ok()
}

fn archive_parent() -> ToolResult<Option<PathBuf>> {
    std::env::var_os("JASMIN_ORIGINAL_REPORTS_DIR")
        .map(|path| {
            PathBuf::from(path)
                .canonicalize()
                .ok()
                .filter(|p| p.is_dir())
                .ok_or_else(|| {
                    invalid("The configured original JAS-MIN reports directory is unavailable.")
                })
        })
        .transpose()
}

fn original_directory(
    generated: &str,
    archive_parent: Option<&Path>,
) -> ToolResult<Option<PathBuf>> {
    let generated = Path::new(generated);
    let Some(parent) = archive_parent else {
        return Ok(Some(generated.to_owned()));
    };
    let basename = generated
        .file_name()
        .ok_or_else(|| invalid("Invalid original report directory name."))?;
    Ok(parent
        .join(basename)
        .canonicalize()
        .ok()
        .filter(|path| path.starts_with(parent)))
}

// An archived SQL figure must have the complete set of measured series for this instance.
// Preserving nulls in the comparison also rejects mismatched TOP section coverage.
fn client_sql_figure(
    window: &SqlChartWindow<'_>,
    sql_id: &str,
    directory: Option<&Path>,
) -> Option<(&'static str, Value)> {
    let plot = window.measured_plot(sql_id)?;
    let expected = json!({"data": plot.data(), "layout": plot.layout()});
    let original = directory.and_then(|directory| {
        let html = read_source(directory, &format!("sqlid/sqlid_{sql_id}.html"))?;
        let traces = expected["data"].as_array()?;
        let axis = traces.first()?["x"].as_array()?;
        let (figure, _) = captured_figure(&html, axis, Some(sql_id))?;
        let archived = figure["data"].as_array()?;
        if archived.len() != traces.len()
            || !archived.iter().zip(traces).all(|(source, reference)| {
                source["name"] == reference["name"]
                    && source["type"] == reference["type"]
                    && source["x"] == reference["x"]
                    && source["y"] == reference["y"]
            })
        {
            return None;
        }
        Some(figure)
    });
    Some(match original {
        Some(figure) => ("sql", figure),
        None => ("sql_awr", expected),
    })
}

pub(super) fn client_sql_charts(
    runtime: &AnalysisRuntime,
    project_id: &str,
    label: &str,
    sql_ids: &BTreeSet<String>,
) -> ToolResult<BTreeMap<String, NativeChart>> {
    let project = runtime.project(project_id)?;
    let snapshots = &project.collection.awrs;
    let mut charts = BTreeMap::new();
    let (Some(first), Some(last)) = (snapshots.first(), snapshots.last()) else {
        return Ok(charts);
    };
    // A missing optional archive still permits rebuilding from the selected project's AWR.
    let directory = match archive_parent() {
        Ok(parent) => original_directory(project.html_reports_dir.as_str(), parent.as_deref())?,
        Err(_) => None,
    };
    let window = SqlChartWindow::new(snapshots.iter());
    let date = |s: &str| {
        timestamp(s)
            .map(|t| t.format("%Y-%m-%d %H:%M:%S%.f").to_string())
            .unwrap_or_else(|| s.to_owned())
    };
    let begin = date(&first.snap_info.begin_snap_time);
    let end = date(&last.snap_info.end_snap_time);
    for id in sql_ids {
        if id.len() != 13 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(invalid("Invalid client SQL chart selection."));
        }
        if let Some((kind, plot)) = client_sql_figure(&window, id, directory.as_deref()) {
            charts.insert(
                id.clone(),
                NativeChart {
                    filename: format!("sqlid_{id}.html"),
                    kind,
                    instance_label: label.trim().to_owned(),
                    sql_id: Some(id.clone()),
                    begin: begin.clone(),
                    end: end.clone(),
                    points: snapshots.len(),
                    plot,
                },
            );
        }
    }
    Ok(charts)
}

fn wait_distribution_matches(trace: &Value, event: &str, rows: &[Option<&WaitEvents>]) -> bool {
    match trace["type"].as_str() {
        Some("scatter") => matches!(
            trace["name"].as_str(),
            Some("Total Wait Time (s)" | "Wait Count")
        ),
        Some("heatmap") => {
            let (Some(buckets), Some(matrix)) = (trace["y"].as_array(), trace["z"].as_array())
            else {
                return false;
            };
            buckets.len() == matrix.len()
                && buckets.iter().zip(matrix).all(|(bucket, series)| {
                    bucket.as_str().is_some_and(|bucket| {
                        matches_numbers(
                            series,
                            rows.iter().map(|r| {
                                r.and_then(|r| r.waitevent_histogram_ms.get(bucket))
                                    .map(|v| f64::from(*v))
                            }),
                        )
                    })
                })
        }
        Some("histogram" | "box") => {
            let Some(group) = trace["legendgroup"].as_str() else {
                return false;
            };
            if group == event {
                return matches_numbers(&trace["x"], rows.iter().map(|r| r.map(|r| r.pct_dbtime)));
            }
            // The classic duration distribution starts when the bucket first appears.
            // Missing event rows add a gap after that point; a present event without
            // the bucket contributes no entry to this particular distribution.
            let mut seen = false;
            let expected = rows.iter().filter_map(|row| match row {
                Some(r) => r.waitevent_histogram_ms.get(group).map(|v| {
                    seen = true;
                    Some(f64::from(*v))
                }),
                None => seen.then_some(None),
            });
            matches_numbers(&trace["x"], expected)
        }
        _ => false,
    }
}

/// Read a classic event plot only when the heading, time axis and event measurements agree
/// with this instance and the selected foreground/background AWR section.
fn wait_figure(
    html: &str,
    event: &str,
    foreground: bool,
    collection: &AWRSCollection,
    axis: &[Value],
) -> Option<(Value, std::ops::Range<usize>)> {
    if !html.contains(&format!(">{event}</h2>")) {
        return None;
    }
    let (figure, _) = decode(payloads(html).next()?)?;
    let traces = figure["data"].as_array()?;
    let series = |name: &str| {
        traces
            .iter()
            .find(|t| t["type"] == "scatter" && t["name"] == name)
    };
    let total = series("Total Wait Time (s)")?;
    let counts = series("Wait Count")?;
    let wanted_axis = total["x"].as_array()?;
    let first = wanted_axis.first()?;
    let start = axis.iter().position(|label| label == first)?;
    let range = start..start.checked_add(wanted_axis.len())?;
    let scope = axis.get(range.clone())?;
    if wanted_axis != scope || counts["x"] != total["x"] {
        return None;
    }
    let rows: Vec<_> = collection.awrs[range.clone()]
        .iter()
        .map(|snap| {
            let events = if foreground {
                &snap.foreground_wait_events
            } else {
                &snap.background_wait_events
            };
            events.iter().find(|r| r.event == event)
        })
        .collect();
    if rows.iter().all(Option::is_none)
        || !matches_series(total, rows.iter().map(|r| r.map(|r| r.total_wait_time_s)))
        || counts["y"] != json!(rows.iter().map(|r| r.map(|r| r.waits)).collect::<Vec<_>>())
        || !traces
            .iter()
            .all(|trace| wait_distribution_matches(trace, event, &rows))
    {
        return None;
    }
    Some((public_figure(figure, "wait", scope, Some(event))?, range))
}

pub(super) fn client_wait_charts(
    runtime: &AnalysisRuntime,
    project_id: &str,
    label: &str,
    events: &BTreeSet<String>,
) -> ToolResult<BTreeMap<String, Vec<NativeChart>>> {
    let project = runtime.project(project_id)?;
    let mut charts = BTreeMap::new();
    let Ok(parent) = archive_parent() else {
        return Ok(charts);
    };
    let Some(directory) = original_directory(project.html_reports_dir.as_str(), parent.as_deref())?
    else {
        return Ok(charts);
    };
    let snapshots = &project.collection.awrs;
    let axis: Vec<_> = snapshots
        .iter()
        .map(|s| {
            json!(format!(
                "{} ({})",
                s.snap_info.begin_snap_time, s.snap_info.begin_snap_id
            ))
        })
        .collect();
    for (slot, event) in events.iter().enumerate() {
        // Plotly permits HTML in trace names. Only plain event labels can be copied from an archive.
        if event.is_empty()
            || event.len() > 256
            || event.contains(['<', '>'])
            || event.chars().any(char::is_control)
        {
            continue;
        }
        for (category, kind) in [("fg", "foreground"), ("bg", "background")] {
            let source = crate::tools::get_safe_filename(event.clone(), category.to_owned());
            let Some(html) = read_source(&directory, &source) else {
                continue;
            };
            let Some((plot, range)) =
                wait_figure(&html, event, category == "fg", &project.collection, &axis)
            else {
                continue;
            };
            let date = |s: &str| {
                timestamp(s)
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S%.f").to_string())
                    .unwrap_or_else(|| s.to_owned())
            };
            charts
                .entry(event.clone())
                .or_insert_with(Vec::new)
                .push(NativeChart {
                    filename: format!("wait-{}-{category}.html", slot + 1),
                    kind,
                    instance_label: label.trim().to_owned(),
                    sql_id: None,
                    begin: date(&snapshots[range.start].snap_info.begin_snap_time),
                    end: date(&snapshots[range.end - 1].snap_info.end_snap_time),
                    points: range.len(),
                    plot,
                });
        }
    }
    Ok(charts)
}

pub(super) fn collect(
    runtime: Option<&AnalysisRuntime>,
    labels: &BTreeMap<String, String>,
    tx: &[Selection],
    selected_sql_ids: &[String],
) -> ToolResult<Vec<NativeChart>> {
    let mut charts = Vec::new();
    let Some(runtime) = runtime else {
        return Ok(charts);
    };
    let archives = archive_parent()?;
    for (index, (project_id, label)) in labels.iter().enumerate() {
        let project = runtime.project(project_id)?;
        let Some(directory) =
            original_directory(project.html_reports_dir.as_str(), archives.as_deref())?
        else {
            continue;
        };
        let root = directory.as_path();
        let collection = &*project.collection;
        let snapshots = &collection.awrs;
        let (Some(first), Some(last)) = (snapshots.first(), snapshots.last()) else {
            continue;
        };
        let date = |s: &str| timestamp(s).map(|t| t.format("%Y-%m-%d %H:%M:%S%.f").to_string());
        let (Some(begin), Some(end)) = (
            date(&first.snap_info.begin_snap_time),
            date(&last.snap_info.end_snap_time),
        ) else {
            continue;
        };
        let axis: Vec<_> = snapshots
            .iter()
            .map(|a| {
                json!(format!(
                    "{} ({})",
                    a.snap_info.begin_snap_time, a.snap_info.begin_snap_id
                ))
            })
            .collect();
        let mut add = |kind, filename, sql_id, plot| {
            if let Some(plot) = plot {
                charts.push(NativeChart {
                    filename,
                    kind,
                    instance_label: label.trim().to_owned(),
                    sql_id,
                    begin: begin.clone(),
                    end: end.clone(),
                    points: axis.len(),
                    plot,
                });
            }
        };
        let node = index + 1;
        if tx.iter().any(|s| s.instance_label.trim() == label.trim()) {
            let figure = read_source(root, TX_PATH).and_then(|html| {
                let (figure, window) = wait_figure(&html, EVENT, true, collection, &axis)?;
                (window == (0..axis.len())).then_some(figure)
            });
            add("tx", format!("tx-chart-{node}.html"), None, figure);
        }
        let cpu =
            read_source(root, "jasmin_main.html").and_then(|s| cpu_figure(&s, collection, &axis));
        add("cpu", format!("cpu-chart-{node}.html"), None, cpu);
        for (slot, id) in selected_sql_ids.iter().enumerate() {
            if id.len() != 13 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return Err(invalid("Invalid client SQL chart selection."));
            }
            let figure = read_source(root, &format!("sqlid/sqlid_{id}.html"))
                .and_then(|s| native_figure(&s, collection, &axis, Some(id)));
            add(
                "sql",
                format!("sql-chart-{node}-{}.html", slot + 1),
                Some(id.clone()),
                figure,
            );
        }
    }
    Ok(charts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awr::{WaitEvents, AWR};

    fn snapshots() -> AWRSCollection {
        let mut row = AWR::default();
        row.snap_info.begin_snap_time = "01-Jan-26 09:00:00".into();
        row.snap_info.end_snap_time = "01-Jan-26 10:00:00".into();
        row.snap_info.begin_snap_id = 42;
        row.foreground_wait_events.push(WaitEvents {
            event: EVENT.into(),
            total_wait_time_s: 72.5,
            ..Default::default()
        });
        AWRSCollection {
            awrs: vec![row],
            db_instance_information: Default::default(),
            initialization_parameters: Default::default(),
            sql_text: Default::default(),
            nmon: None,
        }
    }
    fn document(id: &str, figure: &Value) -> String {
        format!("<h2>{id}</h2><div>Private working note</div><script>Plotly.newPlot(graph_div, {figure});</script>")
    }

    #[test]
    fn copies_real_figure_only_after_scope_and_values_match() {
        let data = snapshots();
        let axis = vec![json!("01-Jan-26 09:00:00 (42)")];
        let plot = json!({"data":[{"type":"scatter","name":"Total Wait Time (s)","x":axis,"y":[72.5],"xaxis":"x1","yaxis":"y2","marker":{"opacity":0.5}}],"layout":{"height":700,"yaxis2":{"title":{"text":"Total Wait (s)"}}},"config":{},"frames":null});
        let from = |v: &Value| native_figure(&document(EVENT, v), &data, &axis, None);
        let result = from(&plot).unwrap();
        assert_eq!(result["data"], plot["data"]);
        assert_eq!(result["layout"], plot["layout"]);
        assert!(!result.to_string().contains("Private working note"));
        let mut wrong = plot.clone();
        wrong["data"][0]["y"] = json!([7.25]);
        assert!(from(&wrong).is_none());
        wrong = plot.clone();
        wrong["data"][0]["x"] = json!(["01-Jan-26 10:00:00 (43)"]);
        assert!(from(&wrong).is_none());
        wrong = plot.clone();
        wrong["layout"]["images"] = json!([{"source":"about:blank"}]);
        assert!(from(&wrong).is_none());
        wrong = plot;
        wrong["data"][0]["customdata"] = json!(["Private working note"]);
        assert!(from(&wrong).is_none());
    }

    #[test]
    fn classic_sql_card_without_text_heading_is_bound_to_the_matching_cpu_series() {
        let mut collection = snapshots();
        let id = "000000000000a";
        collection.awrs[0].sql_cpu_time.insert(
            id.into(),
            crate::awr::SQLCPUTime {
                sql_id: id.into(),
                cpu_time_exec_s: 4.0,
                ..Default::default()
            },
        );
        let axis = vec![json!("01-Jan-26 09:00:00 (42)")];
        let plot = json!({"data":[{"type":"scatter","name":"(s) CPU Time","x":axis,"y":[4.0]}],"layout":{"height":800}});
        let bare = format!("<script>Plotly.newPlot(graph_div, {plot});</script>");
        assert!(native_figure(&bare, &collection, &axis, Some(id)).is_some());
        assert!(native_figure(&bare, &collection, &axis, Some("000000000000b")).is_none());
        let mut no_samples = plot;
        no_samples["data"][0]["y"] = json!([null]);
        let empty = format!("<script>Plotly.newPlot(graph_div, {no_samples});</script>");
        assert!(native_figure(&empty, &collection, &axis, Some("000000000000b")).is_none());
        assert!(native_figure(
            &format!("<h2>another statement</h2>{bare}"),
            &collection,
            &axis,
            Some(id)
        )
        .is_none());
    }

    #[test]
    fn client_sql_card_preserves_classic_series_and_rejects_another_instances_values() {
        use crate::awr::{SQLCPUTime, SQLElapsedTime, SQLGets, SQLIOTime, SQLReads};

        let id = "000000000000a";
        let mut data = snapshots();
        let mut gap = AWR::default();
        gap.snap_info.begin_snap_id = 43;
        gap.snap_info.begin_snap_time = "01-Jan-26 10:00:00".into();
        let mut last = AWR::default();
        last.snap_info.begin_snap_id = 44;
        last.snap_info.begin_snap_time = "01-Jan-26 11:00:00".into();
        last.sql_reads.insert(
            id.into(),
            SQLReads {
                sql_id: id.into(),
                reads_per_exec: 10.0,
                executions: 0,
                cpu_time_pct: 19.0,
                ..Default::default()
            },
        );
        let first = &mut data.awrs[0];
        first.sql_elapsed_time.push(SQLElapsedTime {
            sql_id: id.into(),
            executions: 7,
            elpased_time_exec_s: 8.0,
            pct_cpu: 11.0,
            pct_io: 12.0,
            pct_total: 13.0,
            ..Default::default()
        });
        first.sql_cpu_time.insert(
            id.into(),
            SQLCPUTime {
                sql_id: id.into(),
                executions: 9,
                cpu_time_exec_s: 4.0,
                pct_total: 14.0,
                ..Default::default()
            },
        );
        first.sql_io_time.insert(
            id.into(),
            SQLIOTime {
                sql_id: id.into(),
                io_time_exec_s: 2.0,
                pct_total: 15.0,
                ..Default::default()
            },
        );
        first.sql_gets.insert(
            id.into(),
            SQLGets {
                sql_id: id.into(),
                gets_per_exec: 16.0,
                pct_total: 17.0,
                ..Default::default()
            },
        );
        data.awrs.extend([gap, last]);
        let window = SqlChartWindow::new(data.awrs.iter());
        assert!(client_sql_figure(&window, "000000000000b", None).is_none());
        let (kind, rebuilt) = client_sql_figure(&window, id, None).unwrap();
        assert_eq!(kind, "sql_awr");
        let traces = rebuilt["data"].as_array().unwrap();
        let expected = [
            json!([16.0, null, null]),
            json!([null, null, 10.0]),
            json!([13.0, null, null]),
            json!([11.0, null, 19.0]),
            json!([12.0, null, 0.0]),
            json!([14.0, null, null]),
            json!([15.0, null, null]),
            json!([17.0, null, null]),
            json!([null, null, 0.0]),
            json!([8.0, null, null]),
            json!([4.0, null, null]),
            json!([2.0, null, null]),
            json!([7, null, 0]),
        ];
        assert_eq!(traces.len(), expected.len());
        for ((trace, name), values) in traces.iter().zip(SQL_SERIES).zip(expected) {
            assert_eq!(trace["name"], *name);
            assert_eq!(trace["y"], values);
            assert_eq!(
                trace["x"],
                json!([
                    "01-Jan-26 09:00:00 (42)",
                    "01-Jan-26 10:00:00 (43)",
                    "01-Jan-26 11:00:00 (44)"
                ])
            );
        }
        let folder = tempfile::tempdir().unwrap();
        fs::create_dir(folder.path().join("sqlid")).unwrap();
        let file = folder.path().join(format!("sqlid/sqlid_{id}.html"));
        window.measured_plot(id).unwrap().write_html(&file);
        let (kind, original) = client_sql_figure(&window, id, Some(folder.path())).unwrap();
        assert_eq!(kind, "sql");
        assert_eq!(original["data"], rebuilt["data"]);
        let mut different = original;
        different["data"][0]["y"][0] = json!(99.0);
        fs::write(&file, document(id, &different)).unwrap();
        assert_eq!(
            client_sql_figure(&window, id, Some(folder.path())),
            Some(("sql_awr", rebuilt))
        );
    }

    #[test]
    fn original_wait_card_verifies_event_instance_class_and_source_window() {
        let name = "sample wait: foreground/background";
        let mut data = snapshots();
        let mut first = AWR::default();
        first.snap_info.begin_snap_id = 41;
        first.snap_info.end_snap_id = 42;
        first.snap_info.begin_snap_time = "01-Jan-26 08:00:00".into();
        let measured = &mut data.awrs[0];
        measured.snap_info.end_snap_id = 43;
        let wait = |seconds, waits| WaitEvents {
            event: name.into(),
            total_wait_time_s: seconds,
            waits,
            pct_dbtime: 12.5,
            waitevent_histogram_ms: BTreeMap::from([
                ("1: <1ms".into(), 10.0),
                ("2: >=64ms".into(), 90.0),
            ]),
            ..Default::default()
        };
        measured.foreground_wait_events = vec![wait(22.5, 3)];
        measured.background_wait_events = vec![wait(22.5, 33)];
        data.awrs.insert(0, first);
        let original = tempfile::tempdir().unwrap();
        let path = original.path();
        let axis = [
            json!("01-Jan-26 08:00:00 (41)"),
            json!("01-Jan-26 09:00:00 (42)"),
        ];
        for (folder, is_fg) in [("fg", true), ("bg", false)] {
            fs::create_dir(path.join(folder)).unwrap();
            crate::report::classic::generate_events_plotfiles(
                &data.awrs,
                &BTreeMap::from([(name.to_owned(), 1)]),
                is_fg,
                &(42, 43),
                path.to_str().unwrap(),
            );
            let file = crate::tools::get_safe_filename(name.into(), folder.into());
            let html = read_source(path, &file).unwrap();
            let (figure, scope) = wait_figure(&html, name, is_fg, &data, &axis).unwrap();
            assert_eq!(scope, 1..2);
            assert_eq!(
                figure["data"][2]["y"],
                if is_fg { json!([3]) } else { json!([33]) }
            );
            assert!(wait_figure(&html, name, !is_fg, &data, &axis).is_none());
            assert!(wait_figure(&html, "another event", is_fg, &data, &axis).is_none());
            assert!(wait_figure(&html, name, is_fg, &data, &axis[..1]).is_none());
            let mut wrong = data.clone();
            wrong.awrs[1].foreground_wait_events[0].pct_dbtime = 0.0;
            wrong.awrs[1].background_wait_events[0].pct_dbtime = 0.0;
            assert!(wait_figure(&html, name, is_fg, &wrong, &axis).is_none());
            let mut other_buckets = data.clone();
            for row in &mut other_buckets.awrs {
                for sample in row
                    .foreground_wait_events
                    .iter_mut()
                    .chain(&mut row.background_wait_events)
                {
                    sample.waitevent_histogram_ms.insert("1: <1ms".into(), 25.0);
                }
            }
            assert!(wait_figure(&html, name, is_fg, &other_buckets, &axis).is_none());
        }
    }

    #[test]
    fn isolates_dashboard_series_and_preserves_missing_rates() {
        let a = snapshots();
        let axis = vec![json!("01-Jan-26 09:00:00 (42)")];
        let trace = |name: &str| json!({"type":"scatter","mode":"lines+text","name":name,"x":axis,"y":[null],"xaxis":"x1","yaxis":"y1"});
        let plot = json!({"data":[trace(DB_TIME),trace(DB_CPU),trace("Private working note")],"layout":{"title":"Private working note"}});
        let published = cpu_figure(&document("dashboard", &plot), &a, &axis).unwrap();
        assert_eq!(published["data"], json!([trace(DB_TIME), trace(DB_CPU)]));
        assert!(!published.to_string().contains("Private working note"));
    }

    #[test]
    fn viewer_embeds_data_without_executable_source_markup() {
        let card = NativeChart {
            filename: "tx-chart-1.html".into(),
            kind: "tx",
            instance_label: "instance-1".into(),
            sql_id: None,
            begin: String::new(),
            end: String::new(),
            points: 1,
            plot: json!({"x":"</script> < > &"}),
        };
        let html = plot_markup(&card);
        assert!(html.contains("\\u003c/script\\u003e"));
        assert!(html.contains("src=\"plotly.min.js\""));
        assert!(!html.contains("http:") && !html.contains("https:"));
        let second = numbered_plot_markup(&card, 1);
        assert!(second.contains(r#"data-jasmin-target="jasmin-native-plot-1""#));
        assert!(!second.contains("src="));
        assert!(plotly_js().starts_with(b"/**\n* plotly.js v"));
        assert!(!plotly_js().windows(9).any(|w| w == b"</script>"));
    }
}
