//! One client edition from current TX and CPU briefs in the same saved audit.
use super::*;
use std::fmt::Write as _;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    analysis_id: String,
    title: String,
    tx: super::Recipe,
    cpu: cpu::Recipe,
}

pub(super) fn tools() -> Vec<Value> {
    let text = json!({"type":"string","minLength":1});

    vec![
        function_definition("preview_combined_report", "Prepare the final client report for a combined TX and RAC CPU audit as a single index.html with a wykresy/ directory. Select the current brief revisions from both topics and use the same public instance labels. Review the summary, both chapters, the measurements and chart links in the preview.", json!({
            "type":"object","additionalProperties":false,
            "properties":{"title":text,"tx":tx_preview_params(),"cpu":cpu::preview_params()},
            "required":["title","tx","cpu"]
        })),
        function_definition("publish_combined_report", "When the user requests the final combined report, issue the reviewed preview as an offline ZIP and return its path. The original final-report request authorizes both preparation and publication. Both topics and their linked brief revisions are revalidated.", json!({
            "type":"object","additionalProperties":false,"properties":{"preview_id":text,"user_request":{"type":"string","minLength":1,"maxLength":4000}},
            "required":["preview_id","user_request"]
        })),
        function_definition("read_combined_report_asset", "Read a verified file from the preview or its issued ZIP by byte offset.", json!({
            "type":"object","additionalProperties":false,"properties":{"preview_id":text,"asset":{"type":"string","description":"Exact relative path from this preview's files list, or report.zip once issued.","pattern":"^(?:wykresy/)?[a-z0-9][a-z0-9.-]*$"},"offset":{"type":"integer","minimum":0}},
            "required":["preview_id","asset"]
        })),
    ]
}

#[path = "combined_report_layout.rs"]
pub(super) mod layout;

fn decision(
    body: &mut String,
    copy: &PublicReport,
    topic: &str,
    lang: ReportLanguage,
) {
    let t = |en, pl| lang.choose(en, pl);
    write!(body,r##"<p class="decision-conclusion">{}</p><p><strong>{}</strong> {}</p><nav aria-label="{}"><a href="#{topic}-action">{}</a><a href="#{topic}-evidence">{}</a></nav></section>"##,
        esc(&copy.conclusion),t("Action owner","Właściciel działania"),esc(&copy.owner),esc(&copy.title),
        t("Action and success criterion","Działanie i kryterium powodzenia"),t("Inspect measurements","Sprawdź pomiary")).unwrap();
}

fn landing(
    title: &str,
    tx: &PublicReport,
    waits: &[Measurement],
    cpu: &cpu::Projection,
    lang: ReportLanguage,
) -> String {
    let t = |en, pl| lang.choose(en, pl);
    let mut body = String::new();
    let m = &cpu.model;
    let date = |value: &str| {
        let iso = value.split(' ').next().unwrap_or(value);
        chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
            .map(|d| d.format(t("%Y-%m-%d", "%d.%m.%Y")).to_string())
            .unwrap_or_else(|_| esc(iso))
    };
    write!(
        body,
        r##"<section class="report-overview" id="overview" data-menu-section="overview"><div class="report-intro"><h1>{}</h1><p class="scope-line"><strong>{}</strong> <span>{} – {}</span></p></div>
<div class="topic-grid"><section class="topic-summary tx-summary"><p class="topic-kind">{}</p><h2><a href="#tx">{}</a></h2>
<ul class="metrics" aria-label="{}">"##,
        esc(title),
        t("AWR capture", "Okres AWR"),
        date(&m.begin), date(&m.end),
        t("TX row locks · measurement", "Blokady TX · pomiar"),
        esc(&tx.title),
        t("Foreground session wait time", "Czas oczekiwania sesji pierwszoplanowych")
    )
    .unwrap();
    for m in waits {
        write!(body,r#"<li><span>{}</span><p class="value">{} <span>s</span></p><p class="scope">{} – {} · {} {}</p></li>"#,
            esc(&m.instance_label),number(m.total_wait_s,lang),esc(&m.begin),esc(&m.end),m.observed_intervals,t("intervals","przedziałów")).unwrap();
    }
    body.push_str("</ul>");
    decision(&mut body, tx, "tx", lang);
    write!(body,r##"<section class="topic-summary cpu-summary"><p class="topic-kind">{}</p><h2><a href="#cpu">{}</a></h2>
<p class="scope">{} – {} · {} {}</p><ul class="metrics">"##,
        t("RAC CPU · conditional scenario","CPU RAC · scenariusz warunkowy"),esc(&cpu.report.title),esc(&m.begin),esc(&m.end),
        m.paired_intervals,t("paired intervals","wspólnych przedziałów")).unwrap();
    for (label, value, unit) in [
        (
            t("Measured cluster peak", "Zmierzony szczyt RAC"),
            m.observed_peak.active_cpu,
            t("active CPU", "aktywnych CPU"),
        ),
        (
            t("Scenario peak", "Szczyt w scenariuszu"),
            m.scenario_peak.active_cpu,
            t("active CPU", "aktywnych CPU"),
        ),
        (
            t(
                "Modeled reduction over the period",
                "Modelowana oszczędność w okresie",
            ),
            m.saved_cpu_hours,
            t("CPU-h", "godz. CPU"),
        ),
    ] {
        let formatted = format!("{value:.2}").replace('.', t(".", ","));
        write!(
            body,
            r#"<li><span>{label}</span><p class="value">{formatted} <span>{unit}</span></p></li>"#
        )
        .unwrap();
    }
    body.push_str("</ul>");
    decision(&mut body, &cpu.report, "cpu", lang);
    body.push_str("</div></section>");
    body
}

impl PublicationRecipe for Recipe {
    const ZIP_FILENAME: &'static str = "raport.zip";
    const FROZEN_ASSETS: bool = true;
    fn validate_sources(&self, session: &AnalysisSession) -> ToolResult<()> {
        brief(session, &self.cpu.brief_id, self.cpu.brief_revision)?;
        brief(session, &self.tx.brief_id, self.tx.brief_revision)?;
        for row in &self.tx.evidence {
            if let Some(source) = &row.source_brief_id {
                brief(
                    session,
                    source,
                    row.source_brief_revision.unwrap_or(self.tx.brief_revision),
                )?;
            }
        }
        Ok(())
    }
    fn analysis_id(&self) -> &str {
        &self.analysis_id
    }

    fn render(
        &self,
        session: &AnalysisSession,
        lang: ReportLanguage,
        runtime: Option<&AnalysisRuntime>,
    ) -> ToolResult<(Files, Value)> {
        let title = public_text(&self.title, 400)?;
        if !self.tx.analysis_id.is_empty() || !self.cpu.analysis_id.is_empty() {
            return Err(invalid("Only the outer recipe selects an analysis."));
        }
        let saved = session
            .tx_pilot
            .as_ref()
            .ok_or_else(|| invalid("Resume the combined audit first."))?;
        if saved.topic_for_brief(&self.tx.brief_id) != Some("tx")
            || saved.topic_for_brief(&self.cpu.brief_id) != Some("cpu")
        {
            return Err(invalid(
                "Select the current TX and CPU briefs from their corresponding topics.",
            ));
        }
        for selection in &self.tx.evidence {
            let id = selection
                .source_brief_id
                .as_ref()
                .unwrap_or(&self.tx.brief_id);
            if saved.topic_for_brief(id) != Some("tx") {
                return Err(invalid(
                    "Select TX measurements from briefs in the TX topic.",
                ));
            }
            let revision = selection
                .source_brief_revision
                .unwrap_or(self.tx.brief_revision);
            let source = brief(session, id, revision)?;
            if self
                .cpu
                .instance_labels
                .get(&source.project_id)
                .map(|s| s.trim())
                != Some(selection.instance_label.trim())
            {
                return Err(invalid(
                    "Use the same public label for a given instance in both topics.",
                ));
            }
        }
        let (tx, waits) = project(session, &self.tx)?;
        let cpu = self.cpu.project(session)?;
        let charts = native_charts::collect(
            runtime,
            &self.cpu.instance_labels,
            &self.tx.evidence,
            cpu.model.selected_sql_ids.as_deref().unwrap_or_default(),
        )?;
        let tx_pages = Pages::Tx(&charts);
        let mut body = landing(&title, &tx, &waits, &cpu, lang);
        let (cpu_chapter, cpu_evidence) = cpu.chapter_bodies(lang, Pages::Cpu(&charts));
        body.push_str(&view::overview(&tx, &waits, lang, tx_pages));
        body.push_str(&cpu_chapter);
        body.push_str(&view::evidence(&tx, &waits, lang, tx_pages));
        body.push_str(&cpu_evidence);
        body.push_str(&layout::chart_index(lang, &charts));

        let mut public = Files::from([
            ("index.html".into(), layout::document(lang, false, &title, &body).into_bytes()),
            ("wykresy/report.css".into(), concat!(
                include_str!("tx_pilot_report.css"), include_str!("cpu_pilot_report.css"),
                include_str!("combined_pilot_report.css")
            ).as_bytes().to_vec()),
            ("wykresy/report-nav.js".into(), include_bytes!("combined_report_nav.js").to_vec()),
            ("wykresy/ora-600.png".into(), include_bytes!("../../img/ora-600.png").to_vec()),
        ]);
        let mut source_projects = BTreeMap::new();
        if let Some(runtime) = runtime {
            for (id, label) in &self.cpu.instance_labels {
                source_projects.insert(label.trim(), runtime.project(id)?);
            }
        }
        for chart in &charts {
            let sql_text = chart.sql_id.as_ref().and_then(|id| {
                source_projects.get(chart.instance_label.as_str())?.collection.sql_text.get(id).map(String::as_str)
            });
            public.insert(
                format!("wykresy/{}", chart.filename),
                layout::chart_document(lang, chart, &waits, &cpu, sql_text).into_bytes(),
            );
        }
        if !charts.is_empty() {
            public.insert("wykresy/jasmin-plot.js".into(), native_charts::VIEWER_JS.to_vec());
            public.insert("wykresy/plotly.min.js".into(), native_charts::plotly_js().to_vec());
        }
        Ok((
            public,
            json!({"title":title,"tx":self.tx.details(&tx,&waits),"cpu":cpu.details()}),
        ))
    }
}
