//! Localized static client pages for a paired RAC CPU scenario.
use super::super::combined::layout::{instance_link, sql_links};
use super::{esc, timestamp, Interval, Model, Pages, Peak, PublicReport, ReportLanguage};
use std::fmt::Write as _;

fn num(value: f64, l: ReportLanguage) -> String {
    let raw = format!("{value:.4}");
    let short = raw.trim_end_matches('0').trim_end_matches('.');
    short.replace('.', l.choose(".", ","))
}

fn summary_number(value: f64, l: ReportLanguage) -> String {
    format!("{value:.2}").replace('.', l.choose(".", ","))
}

fn header(body: &mut String, l: ReportLanguage, pages: Pages, back: bool) {
    if !matches!(pages, Pages::Standalone) {
        return;
    }
    let (href, label) = if back {
        (
            pages.href(false, Some("decision")),
            l.choose("Return to conclusion", "Wróć do wniosku"),
        )
    } else {
        (
            pages.href(true, Some("timeline")),
            l.choose("Open measurement timeline", "Zobacz przebieg pomiarów"),
        )
    };
    write!(body, r#"<header><span class="wordmark">JAS-MIN</span><span>Oracle RAC · DB CPU</span><a href="{href}">{label}</a></header>"#).unwrap();
}

fn hours_table(body: &mut String, m: &Model, l: ReportLanguage) {
    let t = |en, pl| l.choose(en, pl);
    write!(body, r#"<div class="table-wrap" role="region" tabindex="0" aria-label="{}"><table><caption>{}</caption><thead><tr><th scope="col">{}</th><th scope="col">{}</th><th scope="col">{}</th><th scope="col">{}</th></tr></thead><tbody>"#,
        t("CPU hours","Godziny CPU"), t("CPU hours over the selected intervals","Suma godzin CPU w wybranych przedziałach"),
        t("Scope","Zakres"),t("Observed [CPU-h]","Pomiar [godz. CPU]"),t("Scenario [CPU-h]","Scenariusz [godz. CPU]"),
        t("Conditional reduction [CPU-h]","Warunkowa redukcja [godz. CPU]")).unwrap();
    let mut row = |label: &str, actual, scenario, saved| {
        write!(
            body,
            "<tr><th scope=\"row\">{}</th><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(label),
            summary_number(actual, l),
            summary_number(scenario, l),
            summary_number(saved, l)
        )
        .unwrap();
    };
    row(
        t("RAC cluster", "Cały klaster RAC"),
        m.observed_cpu_hours,
        m.scenario_cpu_hours,
        m.saved_cpu_hours,
    );
    for node in &m.instances {
        row(
            &node.instance_label,
            node.observed_cpu_hours,
            node.scenario_cpu_hours,
            node.saved_cpu_hours,
        );
    }
    body.push_str("</tbody></table></div>");
}

fn peak_cell(body: &mut String, peak: &Peak, l: ReportLanguage) {
    write!(body,"<td><strong>{}</strong><br><span class=\"scope\">{}</span><br><span class=\"scope\">{}</span></td>",summary_number(peak.active_cpu,l),esc(&peak.begin),esc(&peak.end)).unwrap();
}

fn peaks(body: &mut String, m: &Model, l: ReportLanguage) {
    let t = |en, pl| l.choose(en, pl);
    write!(body,r#"<div class="table-wrap" role="region" tabindex="0" aria-label="{}"><table><caption>{}</caption><thead><tr><th scope="col">{}</th><th scope="col">{}</th><th scope="col">{}</th></tr></thead><tbody>"#,
        t("Peak windows","Okna szczytu"), t("Highest average active CPU and its source interval","Najwyższa średnia liczba aktywnych CPU i przedział źródłowy"),
        t("Scope","Zakres"),t("Observed peak","Szczyt zmierzony"),t("Scenario peak","Szczyt warunkowy")).unwrap();
    let mut row = |label: &str, actual: &Peak, conditional: &Peak| {
        write!(body, "<tr><th scope=\"row\">{}</th>", esc(label)).unwrap();
        peak_cell(body, actual, l);
        peak_cell(body, conditional, l);
        body.push_str("</tr>");
    };
    row(
        t("RAC cluster", "Cały klaster RAC"),
        &m.observed_peak,
        &m.scenario_peak,
    );
    for node in &m.instances {
        row(
            &node.instance_label,
            &node.observed_peak,
            &node.scenario_peak,
        );
    }
    body.push_str("</tbody></table></div>");
}

fn assumptions(body: &mut String, m: &Model, l: ReportLanguage, pages: Pages) {
    let t = |en, pl| l.choose(en, pl);
    let prefix = pages.prefix();
    let (_, h2, _) = pages.headings();
    write!(body,r#"<section id="{prefix}assumptions"><{h2}>{}</{h2}><p>{}</p><dl><dt>{}</dt><dd>{} – {} · {} {}.</dd>
<dt>{}</dt><dd><code>{}</code> · {}% · {} {}.</dd><dt>{}</dt><dd>{} SQL_ID · {}% · {} {}.</dd></dl>"#,
        t("Scenario assumptions","Założenia scenariusza"),
        t("The scenario holds the other measured workload constant and applies both changes in each aligned interval.",
          "W każdym uzgodnionym przedziale scenariusz stosuje obie zmiany przy stałym pozostałym obciążeniu."),
        t("Scope and source time","Zakres i czas źródłowy"),esc(&m.begin),esc(&m.end),m.paired_intervals,t("aligned intervals","przedziałów wspólnych"),
        t("Monitoring module prefix","Prefiks modułu monitoringu"),esc(&m.monitor_module_prefix),summary_number(m.monitor_reduction_pct,l),summary_number(m.monitor_cpu_hours,l),
        t("CPU-h observed in matching TOP CPU rows","godz. CPU zmierzonych w pasujących wierszach TOP CPU"),
        t("Selected SQL","Wybrane zapytania"),m.selected_sql_count,summary_number(m.sql_reduction_pct,l),summary_number(m.selected_sql_cpu_hours,l),
        t("CPU-h observed for the selection before reduction","godz. CPU zmierzonych dla tej grupy przed zastosowaniem redukcji")).unwrap();
    if let Some(ids) = &m.selected_sql_ids {
        write!(
            body,
            "<details><summary>{}</summary><p class=\"sql-ids\">",
            t(
                "Selected SQL identifiers",
                "Identyfikatory wybranych zapytań"
            )
        )
        .unwrap();
        for id in ids {
            let links = sql_links(pages.charts(), id, l);
            if links.is_empty() {
                write!(body, "<code>{}</code> ", esc(id)).unwrap();
            } else {
                write!(body, r#"<span class="sql-reference">{links}</span>"#).unwrap();
            }
        }
        body.push_str("</p></details>");
    }
    write!(body,"<p>{}</p><p>{}</p></section>",
        t("Only captured TOP CPU rows with SELECT or WITH text contribute to the modeled reduction. Monitoring is assigned from the MODULE value on each AWR SQL ordered by CPU Time row, using a case-insensitive prefix. Each row combines the reported executions of the statement. SQL absent from a TOP list can still consume CPU. Monitoring and selected SQL are accounted for separately, without double counting.",
          "Do redukcji wliczono wiersze TOP CPU z tekstem SELECT lub WITH. Do monitoringu przypisano wiersze zestawienia AWR „SQL ordered by CPU Time” z pasującym prefiksem pola MODULE, bez rozróżniania wielkości liter. Czas CPU w wierszu obejmuje wykonania zapytania ujęte w tym zestawieniu. Zapytanie poza listą nadal mogło zużywać CPU. Model rozlicza osobno monitoring i wybrane SQL, bez podwójnego odliczania."),
        t("DB CPU measures processor time used by database sessions. CPU hours add across instances. Average active CPU is CPU seconds divided by elapsed seconds. These estimates describe database workload and do not measure host capacity, user response times or an achieved saving.",
          "DB CPU to czas pracy procesora wykorzystany przez sesje bazy. Godziny CPU sumują się między instancjami. Średnia liczba aktywnych CPU to sekundy CPU podzielone przez czas trwania przedziału. Osobnego pomiaru wymagają wykorzystanie całego hosta, czas odpowiedzi użytkowników i efekt wdrożenia.")).unwrap();
}

pub(super) fn overview(r: &PublicReport, m: &Model, l: ReportLanguage, pages: Pages) -> String {
    let timeline_href = pages.href(true, Some("timeline"));
    let prefix = pages.prefix();
    let (h1, h2, h3) = pages.headings();
    let t = |en, pl| l.choose(en, pl);
    let mut body = String::new();
    header(&mut body, l, pages, false);
    write!(body,"<{h1}>{}</{h1}><p class=\"scope\">{} – {} · {} · {} {}</p><section id=\"{prefix}decision\"><{h2}>{}</{h2}><p class=\"lead\">{}</p><ul class=\"metrics\">",
        esc(&r.title),esc(&m.begin),esc(&m.end),m.instances.iter().map(|n| esc(&n.instance_label)).collect::<Vec<_>>().join(" / "),
        m.paired_intervals,t("aligned intervals","przedziałów wspólnych"),t("Conclusion","Wniosek"),esc(&r.conclusion)).unwrap();
    for (label, value, unit) in [
        (
            t("Measured cluster peak", "Zmierzony szczyt klastra"),
            m.observed_peak.active_cpu,
            t("active CPU", "aktywnych CPU"),
        ),
        (
            t("Conditional cluster peak", "Warunkowy szczyt klastra"),
            m.scenario_peak.active_cpu,
            t("active CPU", "aktywnych CPU"),
        ),
        (
            t("Conditional CPU reduction", "Warunkowa redukcja CPU"),
            m.saved_cpu_hours,
            t("CPU-h", "godz. CPU"),
        ),
    ] {
        write!(
            body,
            "<li><{h3}>{label}</{h3}><p class=\"value\">{} <span>{unit}</span></p></li>",
            summary_number(value, l)
        )
        .unwrap();
    }
    write!(body,"</ul><p class=\"boundary\">{}</p></section><section id=\"{prefix}cluster\"><{h2}>{}</{h2}><p>{}</p>",
        esc(&r.boundary),t("The cluster and its instances","Klaster i jego instancje"),
        t("For every paired interval, the instance rates are summed before the cluster maximum is taken. Independent node maxima can occur in different intervals. The peak of the conditional series can also move.",
          "W każdym wspólnym przedziale sumujemy średnie aktywne CPU instancji i spośród tych sum wybieramy maksimum klastra. Szczyty poszczególnych instancji mogą wypaść w różnych godzinach. Również po zastosowaniu założeń szczyt może przypaść w innym przedziale.")).unwrap();
    peaks(&mut body, m, l);
    hours_table(&mut body, m, l);
    if !pages.charts().is_empty() {
        let links: String = m
            .instances
            .iter()
            .map(|node| instance_link(pages.charts(), "cpu", &node.instance_label, l))
            .collect();
        if !links.is_empty() {
            write!(
                body,
                r#"<nav class="chart-return" aria-label="{}">{links}</nav>"#,
                t("Original DB CPU charts", "Oryginalne wykresy DB CPU")
            )
            .unwrap();
        }
    }
    write!(
        body,
        "<p><a href=\"{timeline_href}\">{}</a></p></section>",
        t(
            "Compare every paired interval",
            "Porównaj wszystkie wspólne przedziały"
        )
    )
    .unwrap();
    assumptions(&mut body, m, l, pages);
    write!(body,"<section id=\"{prefix}action\"><{h2}>{}</{h2}><p class=\"lead\">{}</p><dl><dt>{}</dt><dd>{}</dd><dt>{}</dt><dd>{}</dd></dl></section>",
        t("Action and verification","Działanie i weryfikacja"),esc(&r.action),t("Owner","Właściciel"),esc(&r.owner),
        t("Success criterion","Kryterium powodzenia"),esc(&r.success_criterion)).unwrap();
    pages.page(l, &r.title, &body, false)
}

fn chart(m: &Model, rows: &[Interval], l: ReportLanguage) -> String {
    let t = |en, pl| l.choose(en, pl);
    let base = timestamp(&rows[0].begin).expect("validated timestamp");
    let end = timestamp(&rows.last().unwrap().end).expect("validated timestamp");
    let duration = (end - base).num_milliseconds().max(1) as f64;
    let ceiling = m
        .observed_peak
        .active_cpu
        .max(m.scenario_peak.active_cpu)
        .ceil()
        .max(1.);
    let x = |date: &str| {
        52. + 650.
            * (timestamp(date).expect("validated timestamp") - base).num_milliseconds() as f64
            / duration
    };
    let y = |rate: f64| 145. - rate * 118. / ceiling;
    let mut observed = String::new();
    let mut scenario = String::new();
    let mut previous: Option<&str> = None;
    for r in rows {
        let start = x(&r.begin);
        let end = x(&r.end);
        let command = if previous == Some(&r.begin) { "L" } else { "M" };
        write!(
            observed,
            "{command}{start:.2},{:.2}H{end:.2}",
            y(r.observed_active_cpu)
        )
        .unwrap();
        write!(
            scenario,
            "{command}{start:.2},{:.2}H{end:.2}",
            y(r.scenario_active_cpu)
        )
        .unwrap();
        previous = Some(&r.end);
    }
    format!(
        r#"<figure><figcaption><strong>{title}</strong> · {legend}</figcaption><svg viewBox="0 0 730 165" role="img" aria-label="{title}"><desc>{legend}</desc><path class="gridlines" d="M52,27H702 M52,86H702 M52,145H702"/><text x="46" y="31" text-anchor="end" class="tick">{top}</text><text x="46" y="90" text-anchor="end" class="tick">{middle}</text><text x="46" y="149" text-anchor="end" class="tick">0</text><path class="cpu-observed" d="{observed}"/><path class="cpu-scenario" d="{scenario}"/></svg><p class="scope">{first} – {last}</p></figure>"#,
        title = t(
            "Cluster average active CPU over time",
            "Średnia liczba aktywnych CPU klastra w czasie"
        ),
        legend = t(
            "solid line: observed · dashed line: conditional scenario",
            "linia ciągła – pomiar · linia przerywana – scenariusz warunkowy"
        ),
        top = num(ceiling, l),
        middle = num(ceiling / 2., l),
        first = esc(&m.begin),
        last = esc(&m.end)
    )
}

fn table_start(body: &mut String, caption: &str, columns: &[&str]) {
    write!(body,"<div class=\"table-wrap\" role=\"region\" tabindex=\"0\" aria-label=\"{}\"><table><caption>{}</caption><thead><tr>",esc(caption),esc(caption)).unwrap();
    for title in columns {
        write!(body, "<th scope=\"col\">{}</th>", esc(title)).unwrap();
    }
    body.push_str("</tr></thead><tbody>");
}
fn table_end(body: &mut String) {
    body.push_str("</tbody></table></div>");
}

pub(super) fn evidence(
    r: &PublicReport,
    m: &Model,
    rows: &[Interval],
    l: ReportLanguage,
    pages: Pages,
) -> String {
    let action_href = pages.href(false, Some("action"));
    let cluster_href = pages.href(false, Some("cluster"));
    let prefix = pages.prefix();
    let (h1, h2, _) = pages.headings();
    let t = |en, pl| l.choose(en, pl);
    let title = t(
        "RAC CPU measurements and scenario",
        "Pomiary CPU RAC i scenariusz",
    );
    let mut body = String::new();
    header(&mut body, l, pages, true);
    write!(body,"<{h1}>{title}</{h1}><p class=\"lead\">{}</p><p>{} {} {}.</p><nav aria-label=\"{}\"><a href=\"#{prefix}timeline\">{}</a>",
        esc(&r.title),t("The observed node rates use their own source durations. The maximum alignment tolerance for interval boundaries is",
            "Każdą średnią dla instancji obliczono z jej czasu pomiaru. Dopuszczalna różnica granic wspólnego przedziału wynosi"),num(m.alignment_tolerance_seconds,l),
        t("seconds","sekund"),t("Measurement scopes","Zakresy pomiarów"),t("Cluster","Klaster")).unwrap();
    write!(
        body,
        r#"<a href="{action_href}">{}</a>"#,
        t(
            "Go to action and verification",
            "Przejdź do działania i weryfikacji"
        )
    )
    .unwrap();
    for (i, node) in m.instances.iter().enumerate() {
        write!(
            body,
            "<a href=\"#{prefix}node-{}\">{}</a>",
            i + 1,
            esc(&node.instance_label)
        )
        .unwrap();
    }
    write!(
        body,
        "</nav><section id=\"{prefix}timeline\"><{h2}>{}</{h2}>{}",
        t("Cluster timeline", "Przebieg obciążenia klastra"),
        chart(m, rows, l)
    )
    .unwrap();
    let start = t("Interval start", "Początek przedziału");
    let end = t("Interval end", "Koniec przedziału");
    let observed = t("Observed active CPU", "Aktywne CPU z pomiaru");
    let conditional = t("Conditional active CPU", "Warunkowe aktywne CPU");
    table_start(
        &mut body,
        t(
            "Cluster measurements by paired interval",
            "Pomiary klastra w kolejnych wspólnych przedziałach",
        ),
        &[
            start,
            end,
            observed,
            conditional,
            t("Modeled reduction [CPU s]", "Modelowana redukcja [s CPU]"),
        ],
    );
    for row in rows {
        write!(
            body,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(&row.begin),
            esc(&row.end),
            num(row.observed_active_cpu, l),
            num(row.scenario_active_cpu, l),
            num(row.saved_cpu_seconds, l)
        )
        .unwrap();
    }
    table_end(&mut body);
    body.push_str("</section>");
    for (i, node) in m.instances.iter().enumerate() {
        write!(
            body,
            "<section id=\"{prefix}node-{}\"><{h2}>{}</{h2}>{}",
            i + 1,
            esc(&node.instance_label),
            instance_link(pages.charts(), "cpu", &node.instance_label, l)
        )
        .unwrap();
        table_start(
            &mut body,
            t(
                "Source measurements and conditional rates",
                "Pomiary źródłowe i warunkowe obciążenie",
            ),
            &[
                start,
                end,
                t("Elapsed [s]", "Czas [s]"),
                t("Observed DB CPU [s]", "Zmierzony DB CPU [s]"),
                t("Monitoring CPU [s]", "CPU monitoringu [s]"),
                t("Selected SQL CPU [s]", "CPU wybranych SQL [s]"),
                t("Monitoring TOP rows", "Wiersze TOP monitoringu"),
                t("Selected SQL TOP rows", "Wiersze TOP wybranych SQL"),
                observed,
                conditional,
            ],
        );
        for row in rows {
            let v = &row.instances[i];
            write!(body,"<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&v.begin),esc(&v.end),num(v.wall_seconds,l),num(v.db_cpu_seconds,l),num(v.monitor_cpu_seconds,l),num(v.selected_sql_cpu_seconds,l),
                v.monitor_top_rows,v.selected_top_rows,num(v.observed_active_cpu,l),num(v.scenario_active_cpu,l)).unwrap();
        }
        table_end(&mut body);
        write!(
            body,
            "<p><a href=\"{cluster_href}\">{}</a></p></section>",
            t("Return to cluster comparison", "Wróć do porównania klastra")
        )
        .unwrap();
    }
    pages.page(l, title, &body, true)
}
