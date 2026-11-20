//! All output values come from the explicitly selected client projection.
use super::{
    combined::layout::instance_link, esc, number, Measurement, Pages, PublicReport, ReportLanguage,
    EVENT,
};
use chrono::NaiveDateTime;
use std::fmt::Write as _;

fn timestamp(text: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
        .expect("timestamps were normalized during evidence projection")
}

fn seconds_between(start: NaiveDateTime, end: NaiveDateTime) -> f64 {
    let difference = end - start;
    difference.num_seconds() as f64 + f64::from(difference.subsec_nanos()) / 1e9
}

fn aligned(measures: &[Measurement]) -> bool {
    let first = &measures[0];
    measures.iter().skip(1).all(|m| {
        m.intervals.len() == first.intervals.len()
            && m.intervals.iter().zip(&first.intervals).all(|(a, b)| {
                let (a0, a1) = (timestamp(&a.begin), timestamp(&a.end));
                let (b0, b1) = (timestamp(&b.begin), timestamp(&b.end));
                let shorter = seconds_between(a0, a1).min(seconds_between(b0, b1));
                let tolerance = 60_f64.min(shorter / 10.);
                seconds_between(a0, b0).abs() <= tolerance
                    && seconds_between(a1, b1).abs() <= tolerance
            })
    })
}

fn peak(m: &Measurement) -> f64 {
    m.intervals.iter().map(|r| r.wait_s).fold(0., f64::max)
}

fn chart(m: &Measurement, ceiling: f64, shared: bool, lang: ReportLanguage) -> String {
    let label = lang.choose("Accumulated TX wait [s]", "Suma oczekiwań TX [s]");
    let scope = lang.choose(
        "Interval start to end in source time",
        "Od początku do końca przedziału w czasie źródłowym",
    );
    let scale = if shared {
        lang.choose(
            "All compared windows use the same vertical scale.",
            "Wszystkie porównywane okna mają wspólną skalę pionową.",
        )
    } else {
        lang.choose(
            "The vertical scale applies to this window.",
            "Skala pionowa dotyczy tego okna.",
        )
    };
    let (left, right, top, bottom) = (82.0, 720.0, 25.0, 145.0);
    let mut bars = String::new();
    let base = timestamp(&m.begin);
    let duration = seconds_between(base, timestamp(&m.end));
    for r in &m.intervals {
        let x = left + seconds_between(base, timestamp(&r.begin)) / duration * (right - left);
        let end = left + seconds_between(base, timestamp(&r.end)) / duration * (right - left);
        let h = r.wait_s / ceiling * (bottom - top);
        write!(
            bars,
            r#"<rect x="{x:.2}" y="{:.2}" width="{:.2}" height="{h:.2}"/>"#,
            bottom - h,
            (end - x) * 0.92
        )
        .unwrap();
    }
    format!(
        r#"<figure><figcaption><strong>{label}</strong> · {scale}</figcaption>
<svg role="img" aria-label="{}" viewBox="0 0 740 180">
<title>{label} · {}</title><desc>{scope}. {scale}</desc>
<g class="gridlines"><path d="M82 25H720 M82 85H720 M82 145H720"/></g>
<g class="tick" text-anchor="end"><text x="75" y="31">{}</text><text x="75" y="91">{}</text><text x="75" y="151">0</text></g>
<g class="bars">{bars}</g><g class="tick"><text x="82" y="172">{}</text><text x="720" y="172" text-anchor="end">{}</text></g>
</svg><p class="scope">{scope}.</p></figure>"#,
        esc(&format!("{label} · {}", m.instance_label)),
        esc(&m.instance_label),
        number(ceiling, lang),
        number(ceiling / 2., lang),
        esc(&m.begin),
        esc(&m.end)
    )
}

pub(super) fn overview(
    r: &PublicReport,
    measures: &[Measurement],
    lang: ReportLanguage,
    pages: Pages,
) -> String {
    let evidence = pages.href(true, None);
    let window_href = pages.href(true, Some("window-"));
    let prefix = pages.prefix();
    let (h1, h2, h3) = pages.headings();
    let choose = |en, pl| lang.choose(en, pl);
    let mut out = String::new();
    if matches!(pages, Pages::Standalone) {
        write!(
            out,
            r#"<header><span class="wordmark">JAS-MIN</span><span>{}</span></header>"#,
            choose("Oracle performance report", "Raport wydajności Oracle")
        )
        .unwrap();
    }
    write!(out,r##"<article id="{prefix}decision"><{h1}>{}</{h1}><p class="lead">{}</p><nav aria-label="{}"><a href="#{prefix}action">{}</a><a href="{evidence}">{}</a></nav></article>"##,
        esc(&r.title),esc(&r.conclusion),
        choose("Report","Raport"),choose("Action and success criterion","Działanie i kryterium decyzji"),choose("Open the measurements","Otwórz pomiary")).unwrap();
    write!(out,r##"<section aria-labelledby="{prefix}observations"><{h2} id="{prefix}observations">{}</{h2}><p>{}</p><ul class="metrics">"##,
        choose("Measured wait time","Zmierzony czas oczekiwania"),
        choose("The totals are the summed wait seconds of foreground sessions in each selected window.","Wartości sumują sekundy oczekiwania sesji pierwszoplanowych w każdym z wybranych okien.")).unwrap();
    for (i, m) in measures.iter().enumerate() {
        write!(out,r##"<li><{h3}>{}</{h3}><p class="value">{} <span>s</span></p><p class="scope"><time>{}</time> – <time>{}</time></p><a href="{window_href}{}">{}</a>{}</li>"##,
            esc(&m.instance_label),number(m.total_wait_s,lang),esc(&m.begin),esc(&m.end),i+1,choose("Inspect intervals","Sprawdź przedziały"),instance_link(pages.charts(),"tx",&m.instance_label,lang)).unwrap();
    }
    write!(out,r##"</ul></section><section class="boundary"><{h2}>{}</{h2}><p>{}</p></section><section id="{prefix}action"><{h2}>{}</{h2}><dl><dt>{}</dt><dd>{}</dd><dt>{}</dt><dd>{}</dd><dt>{}</dt><dd>{}</dd></dl><a href="{evidence}">{}</a></section>"##,
        choose("Limits of this conclusion","Granice wniosku"),esc(&r.boundary),choose("Action plan","Plan działania"),
        choose("Owner","Odpowiedzialność"),esc(&r.owner),choose("Next step","Następny krok"),esc(&r.action),choose("Decision criterion","Kryterium decyzji"),esc(&r.success_criterion),choose("Check the source measurements","Sprawdź pomiary źródłowe")).unwrap();
    pages.page(lang, &r.title, &out, false)
}

pub(super) fn evidence(
    r: &PublicReport,
    measures: &[Measurement],
    lang: ReportLanguage,
    pages: Pages,
) -> String {
    let decision_href = pages.href(false, Some("decision"));
    let action_href = pages.href(false, Some("action"));
    let prefix = pages.prefix();
    let (h1, h2, _) = pages.headings();
    let choose = |en, pl| lang.choose(en, pl);
    let title = choose("TX wait measurements", "Pomiary oczekiwań TX");
    let mut out = String::new();
    if matches!(pages, Pages::Standalone) {
        write!(out,r##"<header><span class="wordmark">JAS-MIN</span><a href="{decision_href}">{}</a></header>"##,choose("Return to conclusion","Wróć do wniosku")).unwrap();
    }
    write!(out,r##"<{h1}>{title}</{h1}><p class="lead">{}</p><p>{} <code>{EVENT}</code>.</p><p>{}</p><nav aria-label="{}"><a href="{action_href}">{}</a></nav>"##,
        esc(&r.title),
        choose("Source metric in the selected snapshot intervals:","Metryka źródłowa w wybranych przedziałach migawek:"),
        choose("Each row shows accumulated foreground session wait time in one source interval. The sum can exceed elapsed wall-clock time. A wait alone does not identify the blocking session.",
               "Każdy wiersz przedstawia sumę czasu oczekiwania sesji pierwszoplanowych w jednym przedziale źródłowym. Suma może przekroczyć czas zegarowy. Do wskazania sesji blokującej potrzebny jest dodatkowy pomiar."),
        choose("Report","Raport"),choose("Open the linked action","Przejdź do powiązanego działania")).unwrap();
    let shared = measures.len() > 1 && aligned(measures);
    let ceiling = measures.iter().map(peak).fold(0., f64::max).max(1.);
    if measures.len() > 1 {
        let explanation = if shared {
            choose("The paired source intervals align within one minute. The charts use the same scale.",
                "Granice odpowiadających sobie przedziałów źródłowych różnią się najwyżej o minutę. Wykresy mają wspólną skalę.")
        } else {
            choose("These observations cover different windows. The charts display each window with its own scale and timestamps.",
                "Obserwacje dotyczą różnych okien. Każdy wykres ma własną skalę i daty pomiarów.")
        };
        write!(out, "<p>{explanation}</p>").unwrap();
    }
    for (i, m) in measures.iter().enumerate() {
        let plot = chart(
            m,
            if shared { ceiling } else { peak(m).max(1.) },
            shared,
            lang,
        );
        write!(out,r##"<section id="{prefix}window-{}"><{h2}>{}</{h2}><p><strong>{} s</strong> · {} {}.</p>{}{plot}<div class="table-wrap" role="region" tabindex="0" aria-label="{}"><table><caption>{}</caption><thead><tr><th scope="col">{}</th><th scope="col">{}</th><th scope="col">{}</th></tr></thead><tbody>"##,
            i+1,esc(&m.instance_label),number(m.total_wait_s,lang),choose("Observed intervals:","Liczba obserwowanych przedziałów:"),m.observed_intervals,instance_link(pages.charts(),"tx",&m.instance_label,lang),
            esc(&m.instance_label),choose("Accumulated wait in each interval","Suma oczekiwań w kolejnych przedziałach"),
            choose("Interval start","Początek przedziału"),choose("Interval end","Koniec przedziału"),choose("Session wait [s]","Oczekiwanie sesji [s]")).unwrap();
        for row in &m.intervals {
            write!(
                out,
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&row.begin),
                esc(&row.end),
                number(row.wait_s, lang)
            )
            .unwrap();
        }
        write!(
            out,
            r##"</tbody></table></div><p><a href="{decision_href}">{}</a></p></section>"##,
            choose("Return to conclusion", "Wróć do wniosku")
        )
        .unwrap();
    }
    pages.page(lang, title, &out, true)
}
