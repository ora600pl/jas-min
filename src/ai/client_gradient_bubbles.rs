//! The opening is a whole-capture view. Each chart keeps one fit's native coordinates.
use super::{Opening, Signal};
use crate::report::i18n::ReportLanguage;
use crate::report_signals::{self as atlas, SignalPanel};
use html_escape::{encode_double_quoted_attribute as attr, encode_text as esc};
use std::fmt::Write as _;

fn picker(out: &mut String, title: &str) {
    write!(out, r#"<div class="gradient-picker" role="group" aria-label="{}"><span class="gradient-prompt" aria-hidden="true">{}</span>"#, attr(title), esc(title)).unwrap();
}

// A native radio owns the adjacent panel. CSS only changes visibility, so this
// also works as a standalone document without JavaScript or external resources.
fn choice(out: &mut String, group: &str, index: usize, title: &str, checked: bool) {
    write!(out, r#"<input class="gradient-switch" type="radio" name="{group}" id="{group}-{index}" aria-controls="{group}-{index}-view"{}><label for="{group}-{index}">{}</label><div class="gradient-choice" id="{group}-{index}-view">"#, if checked { " checked" } else { "" }, esc(title)).unwrap();
}

fn number(value: Option<f64>, pl: bool) -> String {
    value
        .map(|value| {
            let s = atlas::number(value);
            if pl { s.replace('.', ",") } else { s }
        })
        .unwrap_or_else(|| "—".into())
}

fn source_table(
    panel: &SignalPanel,
    language: ReportLanguage,
    pl: bool,
    link: &impl Fn(&str, &str) -> String,
) -> String {
    let mut out = format!(
        r#"<details class="gradient-data"><summary>{} ({})</summary><div class="table-scroll"><table><caption>{} · {} · {}</caption><thead><tr><th scope="col">{}</th><th scope="col">P90</th><th scope="col">P99</th><th scope="col">{}</th></tr></thead><tbody>"#,
        language.choose(
            "Source values and model selection",
            "Wartości źródłowe i wskazania modeli"
        ),
        panel.points.len(),
        esc(&panel.project_label),
        esc(&panel.target),
        esc(&panel.family),
        language.choose("Contributor", "Predyktor"),
        language.choose("Selected by", "Wskazujące modele")
    );
    for (i, point) in panel.points.iter().enumerate() {
        let flags: String = point
            .selected
            .iter()
            .map(|flag| match flag {
                Some(true) => '1',
                Some(false) => '0',
                None => '?',
            })
            .collect();
        let mut models = Vec::new();
        for (name, flag) in ["Ridge", "Elastic Net", "Huber", "Quantile 95"]
            .into_iter()
            .zip(point.selected)
        {
            match flag {
                Some(true) => models.push(name.to_owned()),
                None => models.push(format!(
                    "{name} ({})",
                    language.choose("unavailable", "brak danych")
                )),
                Some(false) => {}
            }
        }
        let selection = if models.is_empty() {
            language.choose("No selections", "Brak wskazań").into()
        } else {
            models.join(", ")
        };
        let raw = |v: Option<f64>| v.map(|n| n.to_string()).unwrap_or_default();
        write!(out, r#"<tr id="{}" data-name="{}" data-active="{}" data-peak="{}" data-selected="{flags}"><th scope="row"><span class="gradient-index">{}</span> {}</th><td>{}</td><td>{}</td><td>{selection}</td></tr>"#,
            attr(&atlas::point_id(panel, i)), attr(&point.name), raw(point.active), raw(point.peak), i+1,
            link(&point.name, &panel.project_label), number(point.active, pl), number(point.peak, pl)).unwrap();
    }
    out.push_str("</tbody></table></div></details>");
    out
}

fn has_coordinates(panel: &SignalPanel) -> bool {
    panel
        .points
        .iter()
        .any(|point| point.active.is_some() && point.peak.is_some())
}

fn figure(
    panel: &SignalPanel,
    language: ReportLanguage,
    pl: bool,
    link: &impl Fn(&str, &str) -> String,
) -> String {
    let featured = atlas::featured(panel);
    let mut out = format!(
        r#"<figure class="gradient-fit" data-fit="{}" data-instance="{}" data-family="{}" data-target="{}"><figcaption><strong>{} · {}</strong><span>{}</span></figcaption>"#,
        attr(&panel.id),
        attr(&panel.project_label),
        attr(&panel.family),
        attr(&panel.target),
        esc(&panel.target),
        esc(&panel.family),
        esc(&panel.project_label)
    );
    out.push_str(&atlas::chart(panel, &featured, language));
    out.push_str(r#"<ul class="gradient-key">"#);
    for (i, point) in panel
        .points
        .iter()
        .enumerate()
        .filter(|(i, _)| featured.contains(i))
    {
        let (tone, _) = atlas::classification(&point.classification);
        write!(
            out,
            r#"<li><span aria-hidden="true" class="gradient-chip tone-{tone}">{}</span>{}</li>"#,
            i + 1,
            link(&point.name, &panel.project_label)
        )
        .unwrap();
    }
    out.push_str("</ul>");
    out.push_str(&source_table(panel, language, pl, link));
    out.push_str("</figure>");
    out
}

fn fallback_table<'a>(
    rows: impl Iterator<Item = &'a Signal>,
    scope: &str,
    l: ReportLanguage,
    pl: bool,
    link: &impl Fn(&str, &str) -> String,
) -> String {
    let mut rows = rows.peekable();
    if rows.peek().is_none() {
        return String::new();
    }
    let mut out = format!(
        r#"<details class="gradient-data"><summary>{}</summary><div class="table-scroll"><table><thead><tr><th scope="col">{}</th><th scope="col">{}</th><th scope="col">Model</th><th scope="col">P90</th><th scope="col">P99</th><th scope="col">{}</th></tr></thead><tbody>"#,
        l.choose("Available model rankings", "Dostępne rankingi modeli"),
        l.choose("Predictor family", "Rodzina predyktorów"),
        l.choose("Contributor", "Predyktor"),
        l.choose("Unit", "Jednostka")
    );
    for row in rows {
        let family = super::super::report_signal_adapter::family_label(&row.family, l);

        write!(out, "<tr><td>{}</td><th scope=\"row\">{}</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", esc(family), link(&row.name,scope), esc(&row.model), number(row.active,pl), number(row.peak,pl), esc(&row.unit)).unwrap();
    }
    out.push_str("</tbody></table></div></details>");
    out
}

pub(in super::super) fn render(
    openings: &[Opening],
    pl: bool,
    link: impl Fn(&str, &str) -> String,
) -> String {
    if openings.is_empty() {
        return String::new();
    }
    let l = ReportLanguage::new(if pl { "PL" } else { "EN" });
    let mut out = format!(r#"<section class="gradient-intro" id="gradient-overview"><h2>{}</h2><p class="gradient-guide">{}</p>"#,
        l.choose("Gradient analysis across the full period", "Analiza gradientowa całego okresu"),
        l.choose("Each bubble shows how a predictor tracked changes in DB Time or DB CPU across AWR intervals. Its coordinates are model impact indices at the 90th and 99th percentiles of absolute changes in that predictor over the full period. Compare points within the same chart.",
        "Każdy bąbelek pokazuje związek predyktora ze zmianami DB Time lub DB CPU między interwałami AWR. Współrzędne to wskaźniki wpływu modeli przy 90. i 99. percentylu bezwzględnych zmian tego predyktora w całym okresie. Porównuj punkty w obrębie jednego wykresu."));
    picker(&mut out, l.choose("Instance", "Instancja"));
    let first_instance = openings
        .iter()
        .position(|scope| scope.panels.iter().any(has_coordinates))
        .unwrap_or(0);
    for (index, scope) in openings.iter().enumerate() {
        choice(
            &mut out,
            "gradient-instance",
            index,
            &scope.instance,
            index == first_instance,
        );
        write!(
            out,
            r#"<p class="gradient-window"><strong>{}</strong> · {}"#,
            esc(&scope.instance),
            esc(&scope.window)
        )
        .unwrap();
        if scope.intervals > 0 {
            write!(
                out,
                " · {} {}",
                l.choose("AWR intervals", "interwały AWR"),
                scope.intervals
            )
            .unwrap();
        }
        out.push_str("</p>");
        picker(&mut out, l.choose("Target", "Miara"));
        let targets = ["DB Time", "DB CPU"];
        let first_target = targets
            .iter()
            .position(|target| {
                scope
                    .panels
                    .iter()
                    .any(|p| p.target == *target && has_coordinates(p))
            })
            .or_else(|| {
                targets
                    .iter()
                    .position(|target| scope.fallback.iter().any(|s| s.target == *target))
            })
            .unwrap_or(0);
        for (k, target) in targets.into_iter().enumerate() {
            choice(
                &mut out,
                &format!("gradient-target-{index}"),
                k,
                target,
                k == first_target,
            );
            let fits: Vec<_> = scope
                .panels
                .iter()
                .filter(|panel| panel.target == target)
                .collect();
            if fits.is_empty() {
                write!(
                    out,
                    "<p class=\"gradient-empty\">{target} · {}</p>",
                    l.choose(
                        "No combined model-impact coordinates are available for this period.",
                        "Brak współrzędnych łącznego wpływu modeli dla tego okresu."
                    )
                )
                .unwrap();
            } else {
                picker(&mut out, l.choose("Predictors", "Predyktory"));
                let first_fit = fits.iter().position(|p| has_coordinates(p)).unwrap_or(0);
                for (j, panel) in fits.into_iter().enumerate() {
                    choice(
                        &mut out,
                        &format!("gradient-family-{index}-{k}"),
                        j,
                        &panel.family,
                        j == first_fit,
                    );
                    out.push_str(&figure(panel, l, pl, &link));
                    out.push_str("</div>");
                }
                out.push_str("</div>");
            }
            out.push_str(&fallback_table(
                scope.fallback.iter().filter(|s| s.target == target),
                &scope.instance,
                l,
                pl,
                &link,
            ));
            out.push_str("</div>");
        }
        out.push_str("</div></div>");
    }
    out.push_str("</div>");
    if openings.iter().any(|o| !o.panels.is_empty()) {
        write!(out,r#"<aside class="gradient-legend"><p>{}</p><ul>"#,
            l.choose("Bubble area follows the number of models that selected the predictor. An open circle marks missing selection data or no selections. Both axes use log(1+x) and show the original values.",
            "Pole bąbelka odpowiada liczbie modeli wskazujących predyktor. Pusty okrąg oznacza brak wskazań lub niepełne dane. Obie osie mają skalę log(1+x), a podziałka podaje wartości źródłowe.")).unwrap();
        for (tone, name) in [
            ("agreement", l.choose("Model agreement", "Zgodność modeli")),
            (
                "steady",
                l.choose("Recurring selection", "Powtarzalne wskazanie"),
            ),
            (
                "tail",
                l.choose("Tail or outliers", "Ogon rozkładu lub obserwacje odstające"),
            ),
            ("sparse", l.choose("Sparse model", "Model rzadki")),
            (
                "unknown",
                l.choose("Other selection", "Pozostałe wskazania"),
            ),
        ] {
            write!(out, r#"<li class="tone-{tone}">{name}</li>"#).unwrap();
        }
        out.push_str("</ul></aside>");
    }
    out.push_str("</section>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report_signals::SignalPoint;
    use scraper::{Html, Selector};

    #[test]
    fn whole_period_chart_links_every_bubble_to_its_scoped_original_values() {
        let panel = |id: &str, instance: &str, target: &str| SignalPanel {
            id: id.into(),
            project_id: "scope".into(),
            project_label: instance.into(),
            window: "01–15".into(),
            family: "SQL".into(),
            target: target.into(),
            coverage: String::new(),
            points: vec![SignalPoint {
                name: "a < b".into(),
                active: Some(0.0),
                peak: Some(9.0),
                selected: [Some(true), Some(false), Some(true), None],
                classification: "TAIL_RISK".into(),
                interpretation: String::new(),
                action: String::new(),
                href: None,
                evidence_refs: vec![],
            }],
        };
        let mut unavailable = panel("intro-fit-1-sql", "Instance A", "DB Time");
        unavailable.points[0].peak = None;
        let a = Opening {
            instance: "Instance A".into(),
            window: "01–15".into(),
            intervals: 30,
            panels: vec![unavailable],
            fallback: vec![],
        };
        let b = Opening {
            instance: "Instance B".into(),
            window: "01–15".into(),
            intervals: 30,
            panels: vec![panel("intro-fit-2-sql", "Instance B", "DB CPU")],
            fallback: vec![],
        };
        let html = render(&[a, b], true, |s, i| {
            format!(r#"<a href="{}">{}</a>"#, attr(i), esc(s))
        });
        let doc = Html::parse_fragment(&html);
        let find = |q: &str| Selector::parse(q).unwrap();
        assert_eq!(
            doc.select(&find(".signal-chart a.signal-bubble")).count(),
            1
        );
        let bubble = doc
            .select(&find(".signal-chart a.signal-bubble"))
            .next()
            .unwrap();
        assert!(
            bubble
                .value()
                .classes()
                .any(|c| c == "selection-incomplete")
        );
        let row = doc
            .select(&find(bubble.attr("href").unwrap()))
            .next()
            .unwrap();
        for (name, value) in [
            ("data-active", "0"),
            ("data-peak", "9"),
            ("data-selected", "101?"),
        ] {
            assert_eq!(row.attr(name), Some(value));
        }
        assert_eq!(
            row.select(&find("a")).next().unwrap().attr("href"),
            Some("Instance B")
        );
        let checked: Vec<_> = doc
            .select(&find("input[checked]"))
            .filter_map(|input| input.attr("id"))
            .collect();
        assert!(
            checked.contains(&"gradient-instance-1") && checked.contains(&"gradient-target-1-1")
        );
        assert_eq!(doc.select(&find("#gradient-overview")).count(), 1);
        assert_eq!(doc.select(&find("script, iframe")).count(), 0);
        for control in doc.select(&find("input[aria-controls]")) {
            assert!(
                doc.select(&find(&format!(
                    "#{}",
                    control.attr("aria-controls").unwrap()
                )))
                .next()
                .is_some()
            );
        }
        assert!(html.contains("P99") && html.contains("całym okresie"));
        assert!(!html.contains("hipotez") && !html.contains("w szczycie"));
    }
}
