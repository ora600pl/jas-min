//! Shared offline reader layout for the combined client edition.
use super::{esc, ReportLanguage};
use std::fmt::Write as _;

fn menu(lang: ReportLanguage, from_chart: bool) -> String {
    let t = |en, pl| lang.choose(en, pl);
    let entries = [
        ("overview", t("Overview", "Podsumowanie")),
        ("tx", t("TX locks", "Blokady TX")),
        ("cpu", t("RAC CPU scenario", "Scenariusz CPU RAC")),
        ("charts", t("JAS-MIN charts", "Wykresy JAS-MIN")),
    ];
    let report = if from_chart { "../index.html" } else { "" };
    let mut html = format!(
        r#"<nav class="edition-nav" aria-label="{}"><ul>"#,
        t("Report navigation", "Menu raportu")
    );
    for (section, label) in entries {
        let current = if from_chart && section == "charts" {
            r#" aria-current="location""#
        } else {
            ""
        };
        write!(html, r#"<li><a href="{report}#{section}"{current}>{label}</a></li>"#).unwrap();
    }
    html.push_str("</ul></nav>");
    html
}

pub(in super::super) fn document(
    lang: ReportLanguage,
    from_chart: bool,
    title: &str,
    body: &str,
) -> String {
    let t = |en, pl| lang.choose(en, pl);
    let navigation = menu(lang, from_chart);
    let (assets, overview, script) = if from_chart {
        ("", "../index.html#overview", "")
    } else {
        ("wykresy/", "#overview", r#"<script defer src="wykresy/report-nav.js"></script>"#)
    };
    let subject = t(
        "Oracle RAC performance audit",
        "Audyt wydajności Oracle RAC",
    );
    let home = t("Report overview", "Podsumowanie raportu");
    let skip = t("Skip to content", "Przejdź do treści");
    let top = t("Back to top", "Początek strony");
    let title = esc(title);
    let language = t("en", "pl");
    format!(
        r##"<!doctype html><html lang="{language}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>{title} · ORA-600</title><link rel="stylesheet" href="{assets}report.css">{script}</head>
<body class="report-edition" id="top"><a class="skip-link" href="#content">{skip}</a>
<header class="report-masthead"><div class="masthead-inner"><a class="brand-home" href="{overview}" aria-label="ORA-600 · {home}"><img src="{assets}ora-600.png" width="478" height="230" alt="ORA-600 Database Whisperers"></a><div class="masthead-title"><strong>JAS-MIN</strong><span>{subject}</span></div></div></header>{navigation}
<main id="content" tabindex="-1">{body}</main>
<footer class="report-footer"><div class="footer-inner"><div><strong>ORA-600</strong><span>Database Whisperers</span></div><p>JAS-MIN <span aria-hidden="true">/</span> {subject}</p><a href="#top">{top} <span aria-hidden="true">↑</span></a></div></footer></body></html>"##
    )
}

use super::super::{
    cpu,
    native_charts::{self, NativeChart},
    Measurement,
};

fn chart_name(chart: &NativeChart, lang: ReportLanguage) -> String {
    let t = |en, pl| lang.choose(en, pl);
    match chart.kind {
        "tx" => t("TX row lock waits", "Oczekiwania na blokady TX").to_owned(),
        "cpu" => "DB CPU / DB Time".to_owned(),
        _ => format!("SQL_ID {}", chart.sql_id.as_deref().unwrap_or_default()),
    }
}

pub(in super::super) fn instance_link(
    charts: &[NativeChart],
    kind: &str,
    label: &str,
    lang: ReportLanguage,
) -> String {
    let Some(chart) = charts
        .iter()
        .find(|c| c.kind == kind && c.instance_label == label)
    else {
        return String::new();
    };
    let text = lang.choose("JAS-MIN chart", "Wykres JAS-MIN");
    format!(
        r#"<a class="source-chart-link" href="wykresy/{}">{text} · {}</a>"#,
        esc(&chart.filename),
        esc(label)
    )
}

pub(in super::super) fn sql_links(
    charts: &[NativeChart],
    id: &str,
    lang: ReportLanguage,
) -> String {
    let mut links = String::new();
    for chart in charts
        .iter()
        .filter(|c| c.kind == "sql" && c.sql_id.as_deref() == Some(id))
    {
        write!(
            links,
            r#" <a class="source-chart-link" href="wykresy/{}" aria-label="{} · {} · {}"><code>{}</code> · {}</a>"#,
            esc(&chart.filename), lang.choose("JAS-MIN chart", "Wykres JAS-MIN"),
            esc(id), esc(&chart.instance_label), esc(id), esc(&chart.instance_label)
        )
        .unwrap();
    }
    links
}

pub(in super::super) fn chart_index(lang: ReportLanguage, charts: &[NativeChart]) -> String {
    let t = |en, pl| lang.choose(en, pl);
    let title = t("Original JAS-MIN charts", "Oryginalne wykresy JAS-MIN");
    let mut body = format!(
        r#"<section id="charts" class="report-chapter" data-menu-section="charts" aria-label="{title}"><div class="report-intro"><h2>{title}</h2><p class="lead">{}</p></div>"#,
        t(
            "Choose an instance to open the source chart and its AWR time range.",
            "Wybierz instancję, aby otworzyć wykres źródłowy i sprawdzić jego zakres AWR."
        )
    );
    for (kind, heading, description) in [
        ("tx", t("TX lock waits", "Blokady transakcyjne TX"),
         t("The original event chart covers its full source window. The decision chapter reports the selected incident window.",
           "Oryginalny wykres zdarzenia obejmuje pełny zakres pomiaru. W rozdziale o blokadach wskazano przedziały wybrane do analizy incydentu.")),
        ("cpu",t("Database CPU and workload", "Czas CPU i obciążenie bazy"),
         t("DB CPU and DB Time are source rates for each instance, in seconds per second. The CPU chapter calculates the conditional cluster scenario.",
           "DB CPU i DB Time to źródłowe wartości dla poszczególnych instancji, wyrażone w sekundach na sekundę. Rozdział CPU zawiera wyliczenie scenariusza dla klastra.")),
        ("sql",t("Statements in the scenario", "Zapytania ujęte w scenariuszu"),
         t("Each link opens the source card created for that instance. Choose the instance to inspect its available execution history.", "Każdy odsyłacz prowadzi do karty źródłowej utworzonej dla danej instancji. Wybierz instancję, aby obejrzeć historię zapytania dostępną dla danego źródła.")),
    ] {
        let mut selected: Vec<_> = charts.iter().filter(|c| c.kind == kind).collect();
        if selected.is_empty() {
            if kind == "sql" { continue; }
            write!(body, r#"<section id="charts-{kind}"><h3>{heading}</h3><p>{}</p></section>"#,t("No original chart is available for this selection.", "Dla tego zakresu brak oryginalnego wykresu.")).unwrap();
            continue;
        }
        write!(body,r#"<section id="charts-{kind}" class="chart-catalog-group"><h3>{heading}</h3><p>{description}</p>"#).unwrap();
        selected.sort_by(|a,b| (&a.sql_id,&a.instance_label).cmp(&(&b.sql_id,&b.instance_label)));
        if kind == "sql" {
            let mut counts = std::collections::BTreeMap::new();
            for chart in &selected {
                *counts.entry(&chart.instance_label).or_insert(0usize) += 1;
            }
            write!(body,r#"<p class="scope-line"><strong>{}</strong>"#,
                t("Available source cards","Dostępne karty źródłowe")).unwrap();
            for (label,n) in counts {
                write!(body,"<span>{} · {n}</span>",esc(label)).unwrap();
            }
            body.push_str("</p>");
            body.push_str(r#"<ul class="sql-catalog">"#);
            for group in selected.chunk_by(|a,b| a.sql_id == b.sql_id) {
                let id = esc(group[0].sql_id.as_deref().unwrap_or_default());
                body.push_str("<li><div>");
                for chart in group {
                    write!(body,r#"<a class="source-chart-link" href="wykresy/{}" aria-label="SQL_ID {id} · {}"><code>{id}</code> · {}</a>"#,
                        esc(&chart.filename),esc(&chart.instance_label),esc(&chart.instance_label)).unwrap();
                }
                body.push_str("</div></li>");
            }
        } else {
            body.push_str(r#"<ul class="chart-catalog">"#);
            for chart in selected {
                write!(body,r#"<li><a class="catalog-link" href="wykresy/{}"><strong>{}</strong><span>{}</span><small>{} – {}</small></a></li>"#,
                    esc(&chart.filename),esc(&chart.instance_label),esc(&chart_name(chart,lang)),esc(&chart.begin),esc(&chart.end)).unwrap();
            }
        }
        body.push_str("</ul></section>");
    }
    body.push_str("</section>");
    body
}

fn sql_text_section(text: Option<&str>, lang: ReportLanguage) -> String {
    let heading = lang.choose("Full SQL text retained in AWR", "Pełna treść SQL zachowana w AWR");
    let content = match text.filter(|s| !s.trim().is_empty()) {
        Some(sql) => format!("<pre><code>{}</code></pre>", esc(sql)),
        None => format!("<p>{}</p>", lang.choose("The AWR capture for this instance has no SQL text for this statement.", "W danych AWR tej instancji brakuje tekstu tego zapytania.")),
    };
    format!(r#"<section class="sql-source-text"><h2>{heading}</h2>{content}</section>"#)
}

pub(in super::super) fn chart_document(
    lang: ReportLanguage,
    chart: &NativeChart,
    waits: &[Measurement],
    cpu: &cpu::Projection,
    sql_text: Option<&str>,
) -> String {
    let t = |en, pl| lang.choose(en, pl);
    let title = format!("{} · {}", chart_name(chart, lang), chart.instance_label);
    let mut body = format!(
        r#"<div class="report-intro chart-intro"><p class="topic-kind">{}</p><h1>{}</h1><p class="scope-line"><strong>{}</strong> <span>{} – {}</span></p></div>"#,
        t("Original JAS-MIN chart", "Oryginalny wykres JAS-MIN"),
        esc(&title),
        t("Source AWR window", "Zakres źródłowy AWR"),
        esc(&chart.begin),
        esc(&chart.end)
    );
    body.push_str(r#"<nav class="chart-return">"#);
    if chart.kind == "tx" {
        for (i, m) in waits
            .iter()
            .enumerate()
            .filter(|(_, m)| m.instance_label == chart.instance_label)
        {
            write!(body,r#"<a class="chart-back-to-evidence" href="../index.html#tx-window-{}">{} · {} – {}</a>"#, i+1,
                t("Incident measurements", "Pomiary incydentu"),esc(&m.begin),esc(&m.end)).unwrap();
        }
    } else if let Some(i) = cpu.node_number(&chart.instance_label) {
        write!(
            body,
            r#"<a class="chart-back-to-evidence" href="../index.html#cpu-node-{i}">{} · {}</a>"#,
            t("Instance measurements", "Pomiary instancji"),
            esc(&chart.instance_label)
        )
        .unwrap();
    }
    if chart.kind == "sql" {
        write!(
            body,
            r#"<a href="../index.html#cpu-assumptions">{}</a>"#,
            t("Scenario assumptions", "Założenia scenariusza")
        )
        .unwrap();
    }
    write!(
        body,
        r#"<a href="../index.html#charts-{}">{}</a></nav><p class="chart-scroll-tip">{}</p><div class="native-chart" role="region" aria-label="{}" tabindex="0">"#,
        esc(chart.kind),
        t("Chart directory", "Spis wykresów"),
        t("Scroll horizontally to see the full chart and its legend.", "Przewiń wykres w poziomie, aby zobaczyć całą oś i legendę."),
        esc(&title)
    )
    .unwrap();
    write!(body,"<noscript><p>{}</p></noscript>", t(
        "To explore the interactive chart, enable JavaScript. The measurement tables are available through the links above.",
        "Włącz JavaScript, aby korzystać z interaktywnego wykresu. Odsyłacze powyżej prowadzą do tabel pomiarowych.",
    )).unwrap();
    body.push_str(&native_charts::plot_markup(chart));
    body.push_str("</div>");
    if chart.sql_id.is_some() {
        body.push_str(&sql_text_section(sql_text, lang));
    }
    document(lang, true, &title, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chart(file: &str, label: &str) -> NativeChart {
        NativeChart {
            filename: file.into(),
            kind: "cpu",
            instance_label: label.into(),
            sql_id: None,
            begin: "2026-01-01 01:00:00".into(),
            end: "2026-01-01 02:00:00".into(),
            points: 1,
            plot: json!({}),
        }
    }

    #[test]
    fn opted_in_sql_cards_link_the_identifier_and_show_complete_escaped_text() {
        let lang = ReportLanguage::new("en");
        let mut sql = chart("sql-chart-1-1.html", "instance-1");
        let id = "000000000000a";
        sql.sql_id = Some(id.into());
        sql.kind = "sql";
        assert!(sql_links(&[sql], id, lang).contains(&format!("><code>{id}</code> · instance-1</a>")));
        let captured = "select '<tag>'\nfrom example_table\nwhere id = :b1";
        let output = sql_text_section(Some(captured),lang);
        assert!(!output.contains("<tag>"));
        assert!(output.contains("&lt;tag&gt;"));
        assert!(output.contains("where id = :b1</code></pre>"));
        assert!(sql_text_section(None,lang).contains("no SQL text"));
    }

    #[test]
    fn report_and_child_charts_link_to_sections_and_assets_at_their_respective_depth() {
        let lang = ReportLanguage::new("pl");
        let index = document(lang, false, "Raport", "<h1>Raport</h1>");
        let chart = document(lang, true, "Wykres", "<h1>Wykres</h1>");
        for anchor in ["overview", "tx", "cpu", "charts"] {
            assert!(index.contains(&format!("href=\"#{anchor}\"")));
            assert!(chart.contains(&format!("href=\"../index.html#{anchor}\"")));
        }
        assert!(index.contains("wykresy/report.css") && index.contains("wykresy/ora-600.png"));
        assert!(chart.contains("href=\"report.css\"") && chart.contains("src=\"ora-600.png\""));
        assert!(chart.contains("href=\"../index.html#charts\" aria-current=\"location\""));
    }

    #[test]
    fn links_follow_public_labels_independently_of_chart_order_and_omit_missing_sources() {
        let lang = ReportLanguage::new("pl");
        let charts = [
            chart("cpu-chart-2.html", "instance-2"),
            chart("cpu-chart-1.html", "instance-1"),
        ];
        assert!(instance_link(&charts, "cpu", "instance-1", lang).contains("cpu-chart-1.html"));
        assert!(instance_link(&charts, "tx", "instance-1", lang).is_empty());
        let hub = chart_index(lang, &charts);
        assert!(hub.find("cpu-chart-1.html").unwrap() < hub.find("cpu-chart-2.html").unwrap());
        assert!(!hub.contains("tx-chart-") && !hub.contains("sql-chart-"));
    }
}
