use super::{evidence_anchor, ReportLanguage};
use html_escape::encode_text;
use serde_json::Value;
use std::collections::BTreeSet;

fn plain(value: &Value) -> String {
    encode_text(value.as_str().unwrap_or_default()).replace('\n', "<br>")
}

fn sources(references: &Value, cited: &mut BTreeSet<String>, language: ReportLanguage) -> String {
    let links = references
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|reference| {
            cited.insert(reference.to_owned());
            format!(
                "<a href=\"#evidence-{}\">{}</a>",
                evidence_anchor(reference),
                encode_text(reference)
            )
        })
        .collect::<Vec<_>>();
    if links.is_empty() {
        String::new()
    } else {
        format!(
            "<p><strong>{}</strong> {}</p>\n",
            language.text("Technical provenance"),
            links.join(", ")
        )
    }
}

pub(super) fn render_chapter(
    chapter: &str,
    projects: &[(&str, Value)],
    language: ReportLanguage,
    show_sources: bool,
    cited: &mut BTreeSet<String>,
) -> String {
    let mut content = String::new();
    for (project_index, (label, project)) in projects.iter().enumerate() {
        let mut block = String::new();
        let openings = project["opening_hypotheses"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let Some(gap) = project["hypothesis_gap"].as_str() {
            if matches!(chapter, "performance_profile" | "gradients_anomalies") {
                block.push_str(&format!(
                    "<p><strong>{}</strong> {}</p>\n",
                    language.choose("Coverage limitation", "Ograniczenie danych"),
                    encode_text(gap).replace('\n', "<br>")
                ));
            }
        }
        for (index, opening) in openings.iter().enumerate() {
            let key = format!("{}-{}", project_index + 1, index + 1);
            let anchor = format!("audit-hypothesis-{key}");
            let id = &opening["id"];
            let title = format!("{} — {}", plain(id), plain(&opening["statement"]));
            if chapter == "performance_profile" {
                block.push_str(&format!(
                    "<p id=\"{anchor}\"><strong>{title}</strong></p>\n"
                ));
                for (label, field) in [
                    (
                        language.choose("Gradient signal", "Sygnał gradientowy"),
                        "gradient_signal",
                    ),
                    (
                        language.choose("Workload profile", "Profil obciążenia"),
                        "workload_profile",
                    ),
                    (
                        language.choose("Comparison window", "Okno porównania"),
                        "comparison_window",
                    ),
                    (
                        language.choose("How to test it", "Jak sprawdzić hipotezę"),
                        "test",
                    ),
                ] {
                    block.push_str(&format!(
                        "<p><strong>{label}</strong> {}</p>\n",
                        plain(&opening[field])
                    ));
                }
                if project["hypothesis_outcomes"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|row| &row["id"] == id))
                {
                    block.push_str(&format!(
                        "<p><a href=\"#audit-outcome-{key}\">{}</a></p>\n",
                        language.choose(
                            "See the outcome in section 9",
                            "Zobacz rozstrzygnięcie w rozdziale 9"
                        )
                    ));
                }
                continue;
            }
            let synthesis = chapter == "gradients_anomalies";
            let rows = if synthesis {
                &project["hypothesis_outcomes"]
            } else {
                &project["hypothesis_checks"]
            };
            for row in rows
                .as_array()
                .into_iter()
                .flatten()
                .filter(|row| &row["id"] == id)
            {
                if !synthesis && row["chapter_id"].as_str() != Some(chapter) {
                    continue;
                }
                let link = format!("<a href=\"#{anchor}\">{title}</a>");
                if synthesis {
                    let status = match row["status"].as_str() {
                        Some("confirmed") => language.choose("Confirmed", "Potwierdzona"),
                        Some("rejected") => language.choose("Rejected", "Odrzucona"),
                        _ => language.choose("Unresolved", "Nierozstrzygnięta"),
                    };
                    block.push_str(&format!(
                        "<p id=\"audit-outcome-{key}\">{link} · <strong>{status}</strong></p>\n"
                    ));
                } else {
                    block.push_str(&format!("<p>{link}</p>\n"));
                }
                block.push_str(&format!("<p>{}</p>\n", plain(&row["reason"])));
                if show_sources {
                    block.push_str(&sources(&row["evidence_refs"], cited, language));
                }
            }
        }
        if block.is_empty() {
            continue;
        }
        if chapter == "performance_profile" && show_sources {
            block.push_str(&sources(&project["opening_evidence_refs"], cited, language));
        }
        if projects.len() > 1 {
            let mut heading = encode_text(label).into_owned();
            if projects.iter().filter(|(name, _)| name == label).count() > 1 {
                heading.push_str(&format!(" ({})", plain(&project["project_id"])));
            }
            content.push_str(&format!("<h4>{heading}</h4>\n"));
        }
        content.push_str(&format!("<div class=\"reader-brief\">\n{block}</div>\n\n"));
    }
    if content.is_empty() {
        return content;
    }
    let title = match chapter {
        "performance_profile" => language.choose("Opening hypotheses", "Hipotezy robocze"),
        "gradients_anomalies" => language.choose("Hypothesis outcomes", "Weryfikacja hipotez"),
        _ => language.choose("Hypothesis checks", "Sprawdzenie hipotez"),
    };
    format!("### {title}\n\n{content}")
}
