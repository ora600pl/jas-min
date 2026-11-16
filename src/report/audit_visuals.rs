//! Data-only projection of retained Companion boards. Source strings are escaped;
//! charts never infer missing observations, instance identities or causal proof.
use crate::report::i18n::ReportLanguage;
use html_escape::{encode_double_quoted_attribute, encode_text};
use serde_json::Value;

fn text<'a>(value: &'a Value, key: &str, maximum: usize) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty() && text.chars().count() <= maximum)
        .ok_or_else(|| format!("{key} must be a non-empty string of at most {maximum} characters"))
}
fn array<'a>(value: &'a Value, key: &str, maximum: usize) -> Result<&'a Vec<Value>, String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .filter(|items| items.len() <= maximum)
        .ok_or_else(|| format!("{key} must be an array of at most {maximum} items"))
}
fn finite(value: &Value) -> bool {
    value.as_f64().is_some_and(f64::is_finite)
}
fn observed(value: &Value) -> bool {
    value.is_null() || finite(value)
}
fn optional_text(value: &Value, key: &str, maximum: usize) -> Result<(), String> {
    if value.get(key).is_some() {
        text(value, key, maximum)?;
    }
    Ok(())
}

/// Validate both authored and automatic Companion board shapes before retention.
pub(crate) fn validate_visual(board: &Value) -> Result<(), String> {
    if !board.is_object()
        || serde_json::to_vec(board).map_err(|e| e.to_string())?.len() > 128 * 1024
    {
        return Err("The retained visual must be an object of at most 128 KiB".into());
    }
    text(board, "title", 180)?;
    optional_text(board, "takeaway", 1200)?;
    let panels = array(board, "panels", 6)?;
    if panels.is_empty() {
        return Err("A visual requires at least one supported panel".into());
    }
    for panel in panels {
        text(panel, "title", 180)?;
        text(panel, "source", 1000)?;
        optional_text(panel, "note", 1500)?;
        let classification = text(panel, "classification", 32)?;
        if !["measured", "hypothesis", "explanation"].contains(&classification) {
            return Err("Panel classification must be measured, hypothesis or explanation".into());
        }
        let kind = text(panel, "type", 16)?;
        if ["line", "bar"].contains(&kind) {
            text(panel, "unit", 100)?;
        }
        match kind {
            "flow" => {
                if classification == "measured" {
                    return Err(
                        "A mechanism flow must be labelled hypothesis or explanation".into(),
                    );
                }
                let steps = array(panel, "steps", 8)?;
                if steps.len() < 2 {
                    return Err("A flow requires at least two steps".into());
                }
                for step in steps {
                    text(step, "title", 100)?;
                    text(step, "detail", 500)?;
                }
            }
            "line" => {
                text(panel, "x_label", 100)?;
                if panel.get("scatter").is_some_and(|v| !v.is_boolean()) {
                    return Err("scatter must be boolean".into());
                }
                let scatter = panel["scatter"].as_bool().unwrap_or(false);
                let series = array(panel, "series", 6)?;
                if series.is_empty() {
                    return Err("A line panel requires at least one series".into());
                }
                for series in series {
                    text(series, "name", 180)?;
                    let points = array(series, "points", 3000)?;
                    let mut previous = None;
                    for point in points {
                        let x = point
                            .get("x")
                            .and_then(Value::as_f64)
                            .filter(|v| v.is_finite())
                            .ok_or("Every line point needs a finite numeric x")?;
                        if previous.is_some_and(
                            |previous| if scatter { x < previous } else { x <= previous },
                        ) {
                            return Err("Line x coordinates must increase; only explicit scatter panels may retain duplicate x values".into());
                        }
                        previous = Some(x);
                        text(point, "label", 180)?;
                        if !point.get("value").is_some_and(observed) {
                            return Err("Every line value must be finite or explicit null".into());
                        }
                        optional_text(point, "instance", 100)?;
                        if point.get("end").is_some_and(|v| !v.is_null()) {
                            optional_text(point, "end", 180)?;
                        }
                        if point.get("snap_id").is_some_and(|v| v.as_i64().is_none()) {
                            return Err("snap_id must be an integer".into());
                        }
                    }
                }
            }
            "bar" => {
                for item in array(panel, "items", 30)? {
                    text(item, "label", 180)?;
                    let value = item
                        .get("value")
                        .ok_or("Every bar needs a value or explicit null")?;
                    if !observed(value) || value.as_f64().is_some_and(|v| v < 0.0) {
                        return Err(
                            "Bar values must be nonnegative finite measurements or null".into()
                        );
                    }
                }
            }
            "table" => {
                let columns = array(panel, "columns", 8)?;
                if columns.is_empty()
                    || columns.iter().any(|c| {
                        c.as_str()
                            .is_none_or(|s| s.trim().is_empty() || s.chars().count() > 100)
                    })
                {
                    return Err("Tables require 1–8 non-empty column labels".into());
                }
                let rows = array(panel, "rows", 100)?;
                for row in rows {
                    if row.as_array().is_none_or(|cells| {
                        cells.len() != columns.len()
                            || cells.iter().any(|cell| {
                                !(observed(cell)
                                    || cell.as_str().is_some_and(|s| s.chars().count() <= 1000))
                            })
                    }) {
                        return Err("Table rows must match columns and contain text, finite numbers or null".into());
                    }
                }
                if let Some(selections) = panel.get("selections") {
                    if selections.as_array().is_none_or(|items| {
                        items.len() != rows.len() || items.iter().any(|item| !item.is_object())
                    }) {
                        return Err("Table selections must align with rows".into());
                    }
                }
            }
            "sql" => {
                text(panel, "text", 100_000)?;
            }
            _ => return Err(format!("Unsupported retained panel type: {kind}")),
        }
    }
    Ok(())
}

fn value_text(value: &Value, l: ReportLanguage) -> String {
    if value.is_null() {
        l.choose("Missing observation (null)", "Brak pomiaru (null)")
            .into()
    } else if let Some(value) = value.as_str() {
        value.into()
    } else {
        value.to_string()
    }
}
fn label<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
fn cell(value: &Value, l: ReportLanguage) -> String {
    format!("<td>{}</td>", encode_text(&value_text(value, l)))
}
fn table_start(columns: &[&str]) -> String {
    let mut output = String::from("<div class=\"table-scroll\" tabindex=\"0\"><table><thead><tr>");
    for column in columns {
        output.push_str(&format!("<th scope=\"col\">{}</th>", encode_text(column)));
    }
    output.push_str("</tr></thead><tbody>");
    output
}
fn end_table(output: &mut String) {
    output.push_str("</tbody></table></div>");
}
fn point_caption(series: &Value, point: &Value, unit: &str, l: ReportLanguage) -> String {
    let mut caption = format!(
        "{} · {} · {} {}",
        label(series, "name"),
        label(point, "label"),
        value_text(&point["value"], l),
        unit
    );
    for key in ["end", "instance", "snap_id"] {
        if let Some(value) = point.get(key).filter(|v| !v.is_null()) {
            caption.push_str(&format!(" · {key}: {}", value_text(value, l)));
        }
    }
    caption
}

fn line(panel: &Value, l: ReportLanguage) -> String {
    let series = panel["series"].as_array().unwrap();
    let all = series
        .iter()
        .flat_map(|s| s["points"].as_array().unwrap())
        .collect::<Vec<_>>();
    let measured = all
        .iter()
        .filter(|p| finite(&p["value"]))
        .collect::<Vec<_>>();
    let mut output = String::new();
    if measured.is_empty() {
        output.push_str(&format!(
            "<p>{}</p>",
            l.choose(
                "No retained measurements. Missing is not zero.",
                "Brak zachowanych pomiarów. Brak danych nie oznacza zera."
            )
        ));
    } else {
        // Normalize by the largest absolute coordinate before calculating ranges;
        // this avoids overflow for finite input values without altering source data.
        let xscale = all
            .iter()
            .map(|p| p["x"].as_f64().unwrap().abs())
            .fold(1.0, f64::max);
        let yscale = measured
            .iter()
            .map(|p| p["value"].as_f64().unwrap().abs())
            .fold(1.0, f64::max);
        let lo = all
            .iter()
            .map(|p| p["x"].as_f64().unwrap() / xscale)
            .fold(f64::INFINITY, f64::min);
        let hi = all
            .iter()
            .map(|p| p["x"].as_f64().unwrap() / xscale)
            .fold(f64::NEG_INFINITY, f64::max);
        let min = measured
            .iter()
            .map(|p| p["value"].as_f64().unwrap() / yscale)
            .fold(0.0, f64::min);
        let max = measured
            .iter()
            .map(|p| p["value"].as_f64().unwrap() / yscale)
            .fold(0.0, f64::max);
        let x = |v: f64| {
            80.0 + if hi == lo {
                320.0
            } else {
                640.0 * (v / xscale - lo) / (hi - lo)
            }
        };
        let y = |v: f64| {
            230.0
                - if max == min {
                    0.0
                } else {
                    200.0 * (v / yscale - min) / (max - min)
                }
        };
        output.push_str(&format!("<svg class=\"audit-line-chart\" viewBox=\"0 0 760 290\" role=\"img\" aria-label=\"{}\"><title>{}</title><line x1=\"80\" y1=\"30\" x2=\"80\" y2=\"230\" stroke=\"#777\"/><line x1=\"80\" y1=\"230\" x2=\"720\" y2=\"230\" stroke=\"#777\"/>", encode_double_quoted_attribute(label(panel,"title")), encode_text(label(panel,"title"))));
        for v in [min * yscale, max * yscale] {
            output.push_str(&format!(
                "<text x=\"74\" y=\"{:.2}\" text-anchor=\"end\">{}</text>",
                y(v) + 4.0,
                encode_text(&v.to_string())
            ));
        }
        for (index, s) in series.iter().enumerate() {
            let color = [
                "#b3131b", "#147d64", "#4158b5", "#8d5e00", "#77518d", "#116c93",
            ][index];
            let mut path = String::new();
            let mut previous: Option<&Value> = None;
            for p in s["points"].as_array().unwrap() {
                let Some(value) = p["value"].as_f64() else {
                    previous = None;
                    continue;
                };
                let connected = previous.is_some_and(|prior| {
                    prior.get("instance") == p.get("instance")
                        && match (prior["snap_id"].as_i64(), p["snap_id"].as_i64()) {
                            (Some(a), Some(b)) => a.checked_add(1) == Some(b),
                            _ => true,
                        }
                });
                path.push_str(&format!(
                    "{}{:.2},{:.2} ",
                    if connected { "L" } else { "M" },
                    x(p["x"].as_f64().unwrap()),
                    y(value)
                ));
                previous = Some(p);
            }
            if !panel["scatter"].as_bool().unwrap_or(false) {
                output.push_str(&format!("<path data-audit-series=\"{index}\" d=\"{path}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2\"/>"));
            }
            for p in s["points"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| finite(&p["value"]))
            {
                output.push_str(&format!("<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"3\" fill=\"{color}\"><title>{}</title></circle>", x(p["x"].as_f64().unwrap()), y(p["value"].as_f64().unwrap()), encode_text(&point_caption(s,p,label(panel,"unit"),l))));
            }
            output.push_str(&format!(
                "<text x=\"{}\" y=\"285\" fill=\"{color}\">{}</text>",
                80 + index * 110,
                encode_text(&label(s, "name").chars().take(18).collect::<String>())
            ));
        }
        if let (Some(first), Some(last)) = (
            all.iter()
                .min_by(|a, b| a["x"].as_f64().partial_cmp(&b["x"].as_f64()).unwrap()),
            all.iter()
                .max_by(|a, b| a["x"].as_f64().partial_cmp(&b["x"].as_f64()).unwrap()),
        ) {
            for (px, anchor, p) in [(80, "start", first), (720, "end", last)] {
                output.push_str(&format!(
                    "<text x=\"{px}\" y=\"250\" text-anchor=\"{anchor}\">{}</text>",
                    encode_text(&label(p, "label").chars().take(32).collect::<String>())
                ));
            }
        }
        output.push_str("</svg>");
    }
    output.push_str(&format!(
        "<p>{}: {} · {}</p><details class=\"audit-visual-values\"><summary>{}</summary>",
        encode_text(label(panel, "x_label")),
        encode_text(label(panel, "unit")),
        l.choose(
            "Gaps remain gaps; full labels and exact values are retained below.",
            "Luki pozostają lukami; pełne etykiety i dokładne wartości są poniżej."
        ),
        l.choose("Exact source values", "Dokładne wartości źródłowe")
    ));
    output.push_str(&table_start(&[
        l.choose("Series", "Seria"),
        "x",
        l.choose("Time / label", "Czas / etykieta"),
        l.choose("End", "Koniec"),
        l.choose("Instance", "Instancja"),
        "Snapshot",
        label(panel, "unit"),
    ]));
    for s in series {
        for p in s["points"].as_array().unwrap() {
            output.push_str("<tr>");
            for value in [
                &s["name"],
                &p["x"],
                &p["label"],
                &p["end"],
                &p["instance"],
                &p["snap_id"],
                &p["value"],
            ] {
                output.push_str(&cell(value, l));
            }
            output.push_str("</tr>");
        }
    }
    end_table(&mut output);
    output.push_str("</details>");
    output
}

/// Render only the supported, validated data fields. No input is interpreted as HTML.
pub(crate) fn render_visual(board: &Value, language: &str) -> String {
    let l = ReportLanguage::new(language);
    if let Err(error) = validate_visual(board) {
        return format!(
            "<p class=\"audit-visual-unavailable\">{}: {}</p>",
            l.choose(
                "Retained visual unavailable",
                "Zapisana wizualizacja jest niedostępna"
            ),
            encode_text(&error)
        );
    }
    let mut output = format!(
        "<section class=\"audit-visual\"><h4>{}</h4>",
        encode_text(label(board, "title"))
    );
    if let Some(takeaway) = board["takeaway"].as_str() {
        output.push_str(&format!("<p>{}</p>", encode_text(takeaway)));
    }
    for panel in board["panels"].as_array().unwrap() {
        let classification = label(panel, "classification");
        let visible_class = match classification {
            "measured" => l.choose("Measured", "Pomiar"),
            "hypothesis" => l.choose("Hypothesis", "Hipoteza"),
            _ => l.choose("Explanation", "Wyjaśnienie"),
        };
        output.push_str(&format!("<figure class=\"audit-panel\" data-classification=\"{classification}\"><figcaption><strong>{}</strong> · {visible_class}</figcaption>",encode_text(label(panel,"title"))));
        if let Some(unit) = panel["unit"].as_str() {
            output.push_str(&format!(
                "<p class=\"audit-unit\">{}</p>",
                encode_text(unit)
            ));
        }
        match label(panel, "type") {
            "line" => output.push_str(&line(panel, l)),
            "flow" => {
                output.push_str("<ol class=\"audit-flow\">");
                for step in panel["steps"].as_array().unwrap() {
                    output.push_str(&format!(
                        "<li><strong>{}</strong><p>{}</p></li>",
                        encode_text(label(step, "title")),
                        encode_text(label(step, "detail"))
                    ));
                }
                output.push_str("</ol>");
            }
            "bar" => {
                let items = panel["items"].as_array().unwrap();
                let max = items
                    .iter()
                    .filter_map(|i| i["value"].as_f64())
                    .fold(0.0, f64::max);
                for item in items {
                    output.push_str(&format!(
                        "<div class=\"audit-bar\"><span>{}</span><strong>{} {}</strong>",
                        encode_text(label(item, "label")),
                        encode_text(&value_text(&item["value"], l)),
                        encode_text(label(panel, "unit"))
                    ));
                    if let Some(value) = item["value"].as_f64() {
                        output.push_str(&format!("<div class=\"audit-bar-track\"><div class=\"audit-bar-fill\" style=\"width:{:.4}%\"></div></div>",if max>0.0 {value/max*100.0} else {0.0}));
                    }
                    output.push_str("</div>");
                }
            }
            "table" => {
                let mut columns = panel["columns"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect::<Vec<_>>();
                if panel.get("selections").is_some() {
                    columns.push(l.choose("Source selection metadata", "Metadane wyboru źródła"));
                }
                output.push_str(&table_start(&columns));
                for (index, row) in panel["rows"].as_array().unwrap().iter().enumerate() {
                    output.push_str("<tr>");
                    for value in row.as_array().unwrap() {
                        output.push_str(&cell(value, l));
                    }
                    if let Some(selections) = panel.get("selections") {
                        output.push_str(&cell(&Value::String(selections[index].to_string()), l));
                    }
                    output.push_str("</tr>");
                }
                end_table(&mut output);
            }
            "sql" => output.push_str(&format!(
                "<pre><code>{}</code></pre>",
                encode_text(label(panel, "text"))
            )),
            _ => unreachable!("validated panel"),
        }
        if let Some(note) = panel["note"].as_str() {
            output.push_str(&format!(
                "<p class=\"audit-visual-note\">{}</p>",
                encode_text(note)
            ));
        }
        output.push_str(&format!(
            "<p class=\"audit-visual-source\">{}: {}</p></figure>",
            l.choose("Source", "Źródło"),
            encode_text(label(panel, "source"))
        ));
    }
    output.push_str("</section>\n\n");
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn board() -> Value {
        json!({"title":"Compare instance-1", "takeaway":"Correlation does not establish the cause.","panels":[
            {"type":"flow","title":"Possible mechanism","classification":"hypothesis","source":"E-1; separate mechanism explanation","steps":[{"title":"<script>unsafe()</script>","detail":"Read supplied rows."},{"title":"Compare","detail":"Measure the actual work."}]},
            {"type":"line","title":"CPU","classification":"measured","source":"E-2; exact sampled values","unit":"seconds","x_label":"Database time","series":[{"name":"instance-1","points":[{"x":1,"label":"10:00","value":7,"instance":"instance-1","snap_id":10},{"x":2,"label":"10:10","value":null,"instance":"instance-1","snap_id":11},{"x":3,"label":"10:20","value":0,"instance":"instance-1","snap_id":12}]}]}
        ]})
    }
    #[test]
    fn flow_and_line_preserve_classification_gaps_identity_and_escape_text() {
        let mut value = board();
        value["panels"][1]["title"] = json!("CPU \" onload=\"unsafe()\"");
        let before = value.clone();
        let html = render_visual(&value, "pl");
        let doc = scraper::Html::parse_fragment(&html);
        let sel = |s| scraper::Selector::parse(s).unwrap();
        assert!(doc.select(&sel("script")).next().is_none());
        assert!(doc.select(&sel("[onload]")).next().is_none());
        assert!(html.contains("&lt;script&gt;unsafe()&lt;/script&gt;"));
        assert!(html.contains("data-classification=\"hypothesis\""));
        assert!(html.contains("Hipoteza"));
        assert_eq!(doc.select(&sel("circle")).count(), 2);
        let path = doc
            .select(&sel("path"))
            .next()
            .unwrap()
            .value()
            .attr("d")
            .unwrap();
        assert_eq!(path.matches('M').count(), 2);
        assert!(!path.contains('L'));
        assert!(html.contains("Brak pomiaru (null)"));
        assert!(html.contains("instance-1"));
        assert!(html.contains("10:10"));
        assert!(html.contains("E-2; exact sampled values"));
        assert_eq!(before, value);
    }
    #[test]
    fn invalid_panel_cannot_become_a_measured_flow_or_fabricated_chart() {
        let mut value = board();
        value["panels"][0]["classification"] = json!("measured");
        assert!(validate_visual(&value).is_err());
        assert!(!render_visual(&value, "en").contains("<svg"));
        value["panels"][0]["classification"] = json!("hypothesis");
        value["panels"][1]["series"][0]["points"][1]["value"] = json!("missing");
        assert!(validate_visual(&value).is_err());
    }
    #[test]
    fn null_bar_has_no_zero_length_measurement_and_scatter_never_joins_points() {
        let mut value = board();
        value["panels"][1]["scatter"] = json!(true);
        value["panels"].as_array_mut().unwrap().push(json!({"type":"bar","title":"Coverage","classification":"measured","source":"E-3","unit":"seconds","items":[{"label":"unknown","value":null}]}));
        let html = render_visual(&value, "en");
        assert!(!html.contains("<path"));
        assert!(!html.contains("audit-bar-track"));
        assert!(html.contains("Missing observation (null)"));
    }
}
