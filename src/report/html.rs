use crate::awr::GetStats;
use crate::common::tools::get_safe_filename;
use crate::debug_note;
use base64::{engine::general_purpose, Engine as _};
use chrono::Local;
use html_escape::{encode_double_quoted_attribute, encode_text};
use ndarray::{iter, Array1, Array2};
use ndarray_stats::histogram::Grid;
use ndarray_stats::interpolate::Linear;
use ndarray_stats::{CorrelationExt, QuantileExt};
use noisy_float::types::N64;
use prettytable::{Cell, Row, Table};
use pulldown_cmark::{html, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use regex::Regex;
use serde::Serialize;
use serde_json::{Number, Value};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs::File;
use std::io::{stdout, BufRead, BufReader, BufWriter, Write as Write2};
use std::{collections::HashMap, collections::HashSet, env, fs, path::Path};
use tokio::sync::oneshot;

const JASMIN_AUDIT_LOGO_SVG: &str = include_str!("../../img/jasmin_LOGO_ora600_white.svg");
const ORA600_LOGO_PNG: &[u8] = include_bytes!("../../img/ora-600.png");

pub(crate) fn jasmin_brand_banner_html() -> String {
    format!(
        r#"<div class="jasmin-brand-banner" style="margin:0 0 16px;padding:12px 18px;border-top:5px solid #c52228;border-radius:12px;background:#111111"><a href="https://github.com/ora600pl/jas-min" target="_blank" rel="noopener" aria-label="JAS-MIN project">{}</a></div>"#,
        JASMIN_AUDIT_LOGO_SVG
    )
}

pub fn table_to_html_string(table: &Table, title: &str, headers: &[&str]) -> String {
    let mut html = String::new();
    html.push_str(&format!(
        r#"<p><span style="color:blue;font-weight:bold;">{title}<br></span>"#,
        title = title
    ));

    html.push_str("<table border=\"1\" cellpadding=\"4\" cellspacing=\"0\" >\n");

    // Headers
    html.push_str("  <thead><tr>");
    for &h in headers {
        write!(html, "<th>{}</th>", h).unwrap();
    }
    html.push_str("</tr></thead>\n");

    html.push_str("  <tbody>\n");
    // The rest
    for (i, row) in table.row_iter().enumerate() {
        html.push_str("    <tr>");
        for cell in row.iter() {
            write!(html, "<td><code>{}</code></td>", cell.get_content()).unwrap();
        }
        html.push_str("</tr>\n");
    }

    html.push_str("  </tbody>\n</table>\n");
    html
}

/// Converts Markdown input into a full HTML document with:
/// - CSS styling
/// - Table of Contents (TOC)
/// - Anchored headings
fn classic_source_navigation(html_dir: &str, html_absolute_dir: &str) -> String {
    if html_dir.trim().is_empty() || html_absolute_dir.trim().is_empty() {
        return String::new();
    }

    let relative_root = Path::new(html_dir);
    let absolute_root = Path::new(html_absolute_dir);
    let mut links = Vec::new();
    for (label, suffix) in [
        ("Main JAS-MIN dashboard", "jasmin_main.html"),
        ("Load profile", "stats/jasmin_highlight.html"),
        ("Secondary load profile", "stats/jasmin_highlight2.html"),
    ] {
        if !absolute_root.join(suffix).is_file() {
            continue;
        }
        let target = relative_root.join(suffix);
        links.push(format!(
            r#"<li><a href="{}" target="_blank" rel="noopener">{}</a></li>"#,
            encode_double_quoted_attribute(&target.to_string_lossy()),
            encode_text(label)
        ));
    }

    if links.is_empty() {
        String::new()
    } else {
        format!(
            "<nav class=\"source-navigation\" aria-label=\"Interactive JAS-MIN source reports\"><strong>Interactive source reports:</strong><ul>{}</ul></nav>",
            links.join("")
        )
    }
}

fn markdown_to_html_with_toc(
    markdown_input: &str,
    html_dir: &str,
    html_absolute_dir: &str,
) -> String {
    // Upgrade the old MCP summary format without treating ordinary bold labels
    // (Mechanism, Evidence basis, etc.) as headings.
    let legacy_summary = Regex::new(r"(?m)^\*\*(\d+\. [^\n]+ \[(?:critical|high|medium|low|informational) / [^\]\n]+\])\*\* — ([^\n]+)$").unwrap();
    let markdown_input = legacy_summary.replace_all(markdown_input, "### $1\n\n$2");
    // Enable desired Markdown extensions
    let mut options = Options::empty();
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);

    // Parse the Markdown with extensions
    let parser = Parser::new_ext(&markdown_input, options);

    // Prepare variables
    let mut toc: Vec<(usize, String)> = Vec::new(); // (level, id, title)
    let mut html_output = String::new(); // Final HTML body
    let mut parser_with_ids = Vec::new(); // Modified event stream
    let mut used_heading_ids = HashSet::new();
    let mut heading_counter = 0; // For generating unique IDs
    let mut table_counter = 0; // For keyboard-focusable overflow regions
    let mut current_heading_level = 1; // For closing tags manually
    let mut headings_map: HashMap<String, String> = HashMap::new();

    // Clear TOC before parsing
    toc.clear();

    // Iterate over Markdown events and process headings, capturing heading text for TOC
    let mut in_heading = false;
    let mut heading_text = String::new();
    let mut current_heading_id = String::new();
    let mut current_heading_level_for_map = 1;
    let mut heading_events_buffer = Vec::new();
    let mut parser_iter = parser.into_iter().peekable();
    while let Some(event) = parser_iter.next() {
        match &event {
            Event::Start(Tag::Heading { level, id, .. }) => {
                heading_counter += 1;
                current_heading_level = heading_level_to_int(level);
                current_heading_level_for_map = current_heading_level;
                let requested_id = id
                    .as_ref()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| format!("section-{}", heading_counter));
                let mut id = requested_id.clone();
                let mut suffix = 2;
                while !used_heading_ids.insert(id.clone()) {
                    id = format!("{requested_id}-{suffix}");
                    suffix += 1;
                }
                current_heading_id = id.clone();
                heading_text.clear();
                in_heading = true;
                // Add heading to TOC
                toc.push((current_heading_level, current_heading_id.clone()));
                // Buffer the heading events, but also collect text
                heading_events_buffer.clear();
            }
            Event::End(TagEnd::Heading { .. }) => {
                in_heading = false;
                // Add the heading text to the map
                headings_map.insert(current_heading_id.clone(), heading_text.clone());
                let normalized = heading_text.to_ascii_lowercase();
                let mut heading_class = match current_heading_level_for_map {
                    1 => "report-title".to_string(),
                    2 => "section-title".to_string(),
                    3 => {
                        if normalized.contains("[critical /") {
                            "finding-title severity-critical".to_string()
                        } else if normalized.contains("[high /") {
                            "finding-title severity-high".to_string()
                        } else if normalized.contains("[medium /") {
                            "finding-title severity-medium".to_string()
                        } else if normalized.contains("[low /") {
                            "finding-title severity-low".to_string()
                        } else if normalized.contains("[informational /") {
                            "finding-title severity-informational".to_string()
                        } else {
                            "subsection-title".to_string()
                        }
                    }
                    4 => "group-title".to_string(),
                    _ => "metric-table-title".to_string(),
                };
                if normalized.starts_with("11. prioritized actions") {
                    heading_class.push_str(" actions-section-title");
                }
                if normalized == "mandatory assessments" {
                    heading_class.push_str(" assessment-list-title");
                }
                parser_with_ids.push(Event::Html(
                    format!(
                        r#"<h{} id="{}" class="{}">"#,
                        current_heading_level_for_map,
                        encode_double_quoted_attribute(&current_heading_id),
                        heading_class
                    )
                    .into(),
                ));
                // Push any buffered heading events (if any)
                for buffered_event in heading_events_buffer.drain(..) {
                    parser_with_ids.push(buffered_event);
                }
                // Close heading tag manually
                parser_with_ids.push(Event::Html(
                    format!("</h{}>", current_heading_level_for_map).into(),
                ));
            }
            Event::Start(Tag::Table(_)) if !in_heading => {
                table_counter += 1;
                parser_with_ids.push(Event::Html(
                    format!(
                        "<div class=\"table-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Scrollable report table {table_counter}\">"
                    )
                    .into(),
                ));
                parser_with_ids.push(event);
            }
            Event::End(TagEnd::Table) if !in_heading => {
                parser_with_ids.push(event);
                parser_with_ids.push(Event::Html("</div>".into()));
            }
            _ => {
                if in_heading {
                    // Collect text for heading label
                    match &event {
                        Event::Text(t) => {
                            heading_text.push_str(t);
                        }
                        Event::Code(t) => {
                            heading_text.push_str(t);
                        }
                        _ => {}
                    }
                    // Buffer heading content events to replay after heading open tag
                    heading_events_buffer.push(event);
                } else {
                    // Pass other events unchanged
                    parser_with_ids.push(event);
                }
            }
        }
    }

    // Generate HTML Table of Contents
    let mut toc_html = String::from(
        "<nav class=\"toc\" aria-label=\"Report contents\"><h2>Report contents</h2><ul>",
    );
    for (level, id) in &toc {
        let label = encode_text(&headings_map[id]);
        toc_html.push_str(&format!(
            r##"<li class="level-{}"><a href="#{}">{}</a></li>"##,
            level,
            encode_double_quoted_attribute(id),
            label
        ));
    }
    toc_html.push_str("</ul></nav>");

    // Render HTML from modified parser stream
    html::push_html(&mut html_output, parser_with_ids.into_iter());

    let classic_navigation = classic_source_navigation(html_dir, html_absolute_dir);
    let ora600_logo_data = general_purpose::STANDARD.encode(ORA600_LOGO_PNG);

    // Wrap the result in a complete HTML template
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>JAS-MIN Oracle Performance Analysis</title>
    <style>
        :root {{
            color-scheme: light;
            --navy: #111111;
            --navy-2: #2a2a2a;
            --blue: #b3131b;
            --cyan: #c52228;
            --ink: #1b1b1b;
            --muted: #666666;
            --line: #dddddd;
            --surface: #ffffff;
            --surface-soft: #f6f6f6;
            --critical: #a90f17;
            --high: #c52228;
            --medium: #686868;
            --low: #3f3f3f;
            --info: #171717;
        }}
        * {{ box-sizing: border-box; }}
        html {{ scroll-behavior: smooth; }}
        body {{
            margin: 0;
            padding: 2rem clamp(1rem, 3vw, 3rem) 4rem;
            display: grid;
            grid-template-columns: minmax(240px, 300px) minmax(0, 1120px);
            column-gap: clamp(1.25rem, 3vw, 2.5rem);
            align-items: start;
            justify-content: center;
            background:
                radial-gradient(circle at top right, rgba(197, 34, 40, 0.08), transparent 28rem),
                #f1f1f1;
            color: var(--ink);
            font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
            font-size: 16px;
            line-height: 1.65;
        }}
        body > *:not(.toc) {{ grid-column: 2; min-width: 0; }}
        .brand-banner {{
            margin: 0 0 1rem;
            padding: 1rem 1.5rem;
            border-top: 5px solid #c52228;
            border-radius: 14px;
            background: #111111;
            box-shadow: 0 12px 30px rgba(0, 0, 0, 0.16);
        }}
        .brand-banner a {{ display: block; }}
        .brand-banner svg {{ display: block; width: min(100%, 520px); height: auto; margin: auto; }}
        .brand-footer {{
            margin-top: 3rem;
            padding: 1.25rem;
            border: 1px solid var(--line);
            border-bottom: 5px solid #c52228;
            border-radius: 14px;
            background: white;
            text-align: center;
        }}
        .brand-footer img {{ display: block; width: min(100%, 220px); height: auto; margin: auto; }}
        p {{ margin: 0.72rem 0; max-width: 88ch; }}
        li {{ margin: 0.48rem 0; }}
        .toc {{
            grid-column: 1;
            grid-row: 1 / span 999;
            position: sticky;
            top: 1.5rem;
            max-height: calc(100vh - 3rem);
            overflow: auto;
            align-self: start;
            padding: 1.1rem;
            border: 1px solid #d5d5d5;
            border-radius: 16px;
            background: rgba(255, 255, 255, 0.94);
            box-shadow: 0 14px 35px rgba(0, 0, 0, 0.10);
        }}
        .toc h2 {{
            margin: 0 0 0.8rem;
            padding: 0 0 0.65rem;
            border-bottom: 2px solid #e4e4e4;
            background: none;
            color: var(--navy);
            font-size: 1rem;
            letter-spacing: 0.08em;
            text-transform: uppercase;
        }}
        .toc ul {{ list-style: none; margin: 0; padding: 0; }}
        .toc li {{ margin: 0; }}
        .toc a {{
            display: block;
            padding: 0.42rem 0.5rem;
            border-radius: 8px;
            color: #4b4b4b;
            font-size: 0.82rem;
            line-height: 1.35;
        }}
        .toc a:hover {{ background: #fae9ea; color: var(--blue); text-decoration: none; }}
        .toc li.level-1 {{ display: none; }}
        .toc li.level-2 a {{ margin-top: 0.25rem; color: var(--navy); font-weight: 750; }}
        .toc li.level-3 a {{ padding-left: 1rem; border-left: 2px solid #e1e1e1; }}
        .toc li.level-4, .toc li.level-5, .toc li.level-6 {{ display: none; }}
        .report-title {{
            margin: 0 0 1rem;
            padding: clamp(1.6rem, 4vw, 3rem);
            border-radius: 20px;
            border-top: 5px solid #c52228;
            background: linear-gradient(135deg, #111111 0%, #292929 100%);
            box-shadow: 0 18px 40px rgba(0, 0, 0, 0.20);
            color: white;
            font-size: clamp(2rem, 4vw, 3.35rem);
            line-height: 1.08;
            letter-spacing: -0.035em;
        }}
        #section-1 + p {{
            display: inline-block;
            margin: -0.35rem 0 1rem;
            padding: 0.35rem 0.7rem;
            border-radius: 999px;
            background: #f5e3e4;
            color: #8f151b;
            font-size: 0.84rem;
            font-weight: 700;
        }}
        .section-title {{
            margin: 3rem 0 1.2rem;
            padding: 0.85rem 1.1rem;
            border-radius: 12px;
            background: linear-gradient(90deg, var(--navy), var(--navy-2));
            color: white;
            font-size: clamp(1.25rem, 2vw, 1.65rem);
            line-height: 1.25;
            letter-spacing: -0.01em;
            box-shadow: 0 8px 20px rgba(0, 0, 0, 0.12);
        }}
        .finding-title, .subsection-title {{
            margin: 2rem 0 0;
            padding: 1rem 1.15rem;
            border: 1px solid var(--line);
            border-left: 7px solid var(--blue);
            border-radius: 12px 12px 0 0;
            background: var(--surface);
            color: var(--navy);
            font-size: 1.1rem;
            line-height: 1.38;
            box-shadow: 0 8px 22px rgba(0, 0, 0, 0.07);
        }}
        .group-title {{
            margin: 1.5rem 0 0.65rem;
            padding: 0.55rem 0.8rem;
            border-left: 5px solid var(--high);
            background: #f4f4f4;
            color: var(--navy);
            font-size: 1rem;
        }}
        .metric-table-title {{
            margin: 1.15rem 0 0.35rem;
            color: var(--navy);
            font-size: 0.93rem;
            letter-spacing: 0.025em;
        }}
        .finding-title::before {{
            display: inline-block;
            margin: 0 0.55rem 0.25rem 0;
            padding: 0.16rem 0.5rem;
            border-radius: 999px;
            color: white;
            font-size: 0.67rem;
            font-weight: 850;
            letter-spacing: 0.08em;
            vertical-align: 0.13em;
        }}
        .severity-critical {{ border-left-color: var(--critical); }}
        .severity-critical::before {{ content: "CRITICAL"; background: var(--critical); }}
        .severity-high {{ border-left-color: var(--high); }}
        .severity-high::before {{ content: "HIGH"; background: var(--high); }}
        .severity-medium {{ border-left-color: var(--medium); }}
        .severity-medium::before {{ content: "MEDIUM"; background: var(--medium); }}
        .severity-low {{ border-left-color: var(--low); }}
        .severity-low::before {{ content: "LOW"; background: var(--low); }}
        .severity-informational {{ border-left-color: var(--info); }}
        .severity-informational::before {{ content: "INFO"; background: var(--info); }}
        h3 + p {{
            margin-top: 0;
            max-width: none;
            padding: 1rem 1.2rem;
            border: 1px solid var(--line);
            border-top: 0;
            border-radius: 0 0 12px 12px;
            background: var(--surface);
            box-shadow: 0 8px 22px rgba(0, 0, 0, 0.06);
            font-size: 1.02rem;
        }}
        .actions-section-title + ul, .assessment-list-title + ul {{
            display: grid;
            grid-template-columns: repeat(2, minmax(0, 1fr));
            gap: 0.8rem;
            margin: 0;
            padding: 0;
            list-style: none;
            counter-reset: audit-item;
        }}
        .actions-section-title + ul > li, .assessment-list-title + ul > li {{
            position: relative;
            margin: 0;
            padding: 1rem 1rem 1rem 3.25rem;
            border: 1px solid var(--line);
            border-radius: 12px;
            background: var(--surface);
            box-shadow: 0 6px 16px rgba(0, 0, 0, 0.05);
            counter-increment: audit-item;
        }}
        .actions-section-title + ul > li::before, .assessment-list-title + ul > li::before {{
            content: counter(audit-item, decimal-leading-zero);
            position: absolute;
            top: 1rem;
            left: 0.9rem;
            width: 1.8rem;
            height: 1.8rem;
            border-radius: 50%;
            background: var(--navy);
            color: white;
            font-size: 0.68rem;
            font-weight: 800;
            line-height: 1.8rem;
            text-align: center;
        }}
        .table-scroll {{
            width: 100%;
            max-width: 100%;
            margin: 1rem 0 1.5rem;
            overflow-x: auto;
            overflow-y: visible;
            overscroll-behavior-inline: contain;
            scrollbar-gutter: stable both-edges;
            border: 1px solid #d5d5d5;
            border-radius: 12px;
            background: var(--surface);
            box-shadow: 0 8px 22px rgba(0, 0, 0, 0.07);
        }}
        .table-scroll:focus-visible {{
            outline: 3px solid rgba(197,34,40,0.35);
            outline-offset: 3px;
        }}
        .table-scroll::-webkit-scrollbar, .plan-canvas::-webkit-scrollbar {{ height: 13px; }}
        .table-scroll::-webkit-scrollbar-track, .plan-canvas::-webkit-scrollbar-track {{ background: #ececec; border-radius: 999px; }}
        .table-scroll::-webkit-scrollbar-thumb, .plan-canvas::-webkit-scrollbar-thumb {{ background: #777777; border: 3px solid #ececec; border-radius: 999px; }}
        .table-scroll::after {{
            content: "scroll horizontally when needed →";
            position: sticky;
            left: 0;
            display: block;
            width: max-content;
            padding: 0.32rem 0.7rem 0.45rem;
            color: var(--muted);
            font-size: 0.72rem;
            font-weight: 700;
            letter-spacing: 0.03em;
        }}
        table {{
            width: max-content;
            min-width: 100%;
            margin: 0;
            border: 0;
            border-collapse: separate;
            border-spacing: 0;
            background: var(--surface);
            font-size: 0.92rem;
        }}
        th {{
            position: sticky;
            top: 0;
            z-index: 2;
            padding: 0.72rem 0.8rem;
            background: var(--navy);
            color: white;
            font-size: 0.79rem;
            letter-spacing: 0.035em;
            text-align: left;
            text-transform: uppercase;
        }}
        .table-sort-button {{
            display: flex;
            width: 100%;
            min-width: 8rem;
            align-items: center;
            justify-content: space-between;
            gap: 0.65rem;
            margin: 0;
            padding: 0;
            border: 0;
            background: transparent;
            color: inherit;
            font: inherit;
            font-weight: 850;
            letter-spacing: inherit;
            text-align: left;
            text-transform: inherit;
            cursor: pointer;
        }}
        .table-sort-button:focus-visible {{
            outline: 3px solid rgba(255,255,255,0.7);
            outline-offset: 4px;
            border-radius: 2px;
        }}
        .table-sort-indicator {{
            flex: 0 0 auto;
            color: #d7d7d7;
            font-size: 0.9rem;
            line-height: 1;
        }}
        th[aria-sort="ascending"] .table-sort-indicator,
        th[aria-sort="descending"] .table-sort-indicator {{ color: #ffffff; }}
        td {{
            min-width: 10rem;
            max-width: 32rem;
            padding: 0.68rem 0.8rem;
            border-top: 1px solid #e4e4e4;
            vertical-align: top;
            overflow-wrap: anywhere;
        }}
        tbody tr:nth-child(even) {{ background: #f7f7f7; }}
        tbody tr:hover {{ background: #fae9ea; }}
        table a {{ font-weight: 720; }}
        .plan-review {{
            margin: 1rem 0 1.6rem;
            border: 1px solid #cfcfcf;
            border-radius: 14px;
            overflow: hidden;
            background: var(--surface);
            box-shadow: 0 10px 26px rgba(0,0,0,0.08);
        }}
        .plan-review-header {{
            display: flex;
            align-items: center;
            justify-content: space-between;
            gap: 1rem;
            padding: 1rem 1.15rem;
            background: linear-gradient(100deg, #171717, #303030);
            color: white;
        }}
        .plan-review-header h4 {{ margin: 0.2rem 0 0; color: white; font-size: 1.05rem; }}
        .plan-review-header code {{ border-color: #666; background: #2b2b2b; color: white; }}
        .plan-scope {{ color: #f0b9bc; font-size: 0.75rem; font-weight: 800; letter-spacing: 0.05em; text-transform: uppercase; }}
        .plan-recommendation-badge {{ padding: 0.3rem 0.65rem; border-radius: 999px; background: var(--high); font-size: 0.73rem; font-weight: 850; text-transform: uppercase; }}
        .sql-context {{ display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 0; margin: 0; background: #fbf1f2; border-bottom: 1px solid var(--line); }}
        .sql-context > div {{ padding: 0.9rem 1rem; border-right: 1px solid #ead5d6; border-top: 1px solid #ead5d6; }}
        .sql-context dt, .plan-conclusion dt {{ color: var(--muted); font-size: 0.7rem; font-weight: 850; letter-spacing: 0.05em; text-transform: uppercase; }}
        .sql-context dd, .plan-conclusion dd {{ margin: 0.25rem 0 0; }}
        .plan-conclusion {{ display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 0; margin: 0; border-bottom: 1px solid var(--line); }}
        .plan-conclusion > div {{ padding: 0.8rem 1rem; border-right: 1px solid var(--line); border-top: 1px solid var(--line); }}
        .entity-link {{ color: #ffccd0; text-decoration-thickness: 2px; text-underline-offset: 0.18em; }}
        .entity-link code {{ color: inherit; }}
        .plan-toolbar {{ display: flex; flex-wrap: wrap; align-items: center; gap: 0.8rem; padding: 0.7rem 1rem; background: #f4f4f4; border-bottom: 1px solid var(--line); font-size: 0.78rem; }}
        .plan-toolbar button {{ padding: 0.42rem 0.7rem; border: 1px solid #999; border-radius: 7px; background: white; color: var(--ink); font-weight: 750; cursor: pointer; }}
        .plan-toolbar button[aria-pressed="true"] {{ border-color: var(--high); background: #fae9ea; color: #8f151b; }}
        .plan-toolbar label {{ display: flex; align-items: center; gap: 0.4rem; font-weight: 750; }}
        .plan-toolbar span {{ margin-left: auto; color: var(--muted); }}
        .plan-canvas {{ max-width: 100%; overflow-x: auto; padding: 1rem; scrollbar-gutter: stable both-edges; background: #f8f8f8; }}
        .plan-canvas:focus-visible {{ outline: 3px solid rgba(197,34,40,0.35); outline-offset: -3px; }}
        .plan-tree {{ width: max-content; min-width: 100%; zoom: var(--plan-zoom, 1); }}
        .plan-node {{
            width: clamp(32rem, 72vw, 66rem);
            margin: 0.35rem 0 0.35rem calc(var(--plan-depth) * 1.55rem);
            padding: 0.65rem 0.75rem;
            border: 1px solid #d6d6d6;
            border-left: 5px solid #777;
            border-radius: 9px;
            background: white;
            box-shadow: 0 3px 9px rgba(0,0,0,0.04);
        }}
        .plan-node-high {{ border-left-color: var(--high); background: #fff7f7; }}
        .plan-node-medium {{ border-left-color: #9b6b00; background: #fffaf0; }}
        .plan-node-main {{ display: flex; align-items: center; gap: 0.5rem; }}
        .plan-node-main > strong {{ color: var(--navy); }}
        .plan-node-toggle {{ width: 1.6rem; height: 1.6rem; padding: 0; border: 1px solid #999; border-radius: 5px; background: #f5f5f5; cursor: pointer; font-weight: 900; line-height: 1; }}
        .plan-node-leaf {{ width: 1.6rem; text-align: center; color: #888; }}
        .plan-node-id {{ display: inline-grid; place-items: center; min-width: 1.7rem; height: 1.7rem; border-radius: 50%; background: #222; color: white; font-size: 0.7rem; font-weight: 850; }}
        .plan-node-main code {{ margin-left: auto; }}
        .plan-node-metrics {{ display: flex; flex-wrap: wrap; gap: 0.4rem 0.8rem; margin: 0.5rem 0 0 4.3rem; color: #555; font-size: 0.75rem; }}
        .plan-node-flags {{ display: flex; flex-wrap: wrap; gap: 0.35rem; margin: 0.5rem 0 0 4.3rem; }}
        .plan-node-flags span {{ padding: 0.18rem 0.45rem; border-radius: 999px; background: #f2d4d6; color: #8f151b; font-size: 0.67rem; font-weight: 800; }}
        .plan-node[data-filtered="true"], .plan-node[data-collapsed="true"] {{ display: none; }}
        .plan-graph-unavailable {{ margin: 0; padding: 1rem; background: #fff7df; color: #5f4a00; }}
        .plan-coverage {{ margin: 1rem 0 1.5rem; padding: 0.8rem 1rem; border: 1px solid var(--line); border-radius: 10px; background: #f8f8f8; }}
        .plan-coverage summary {{ cursor: pointer; color: var(--navy); font-weight: 800; }}
        .cell-details summary {{ min-width: 12rem; cursor: pointer; color: #8f151b; font-weight: 800; }}
        .cell-details[open] summary {{ margin-bottom: 0.45rem; }}
        .evidence-appendix {{ margin: 1rem 0 2rem; padding: 0.9rem 1rem; border: 1px solid var(--line); border-radius: 12px; background: white; }}
        .evidence-appendix summary {{ cursor: pointer; color: var(--navy); font-weight: 850; }}
        .evidence-appendix ul {{ columns: 2; column-gap: 2rem; padding-left: 1.2rem; }}
        .evidence-appendix li {{ break-inside: avoid; font-size: 0.82rem; }}
        .source-navigation {{
            margin: 0 0 1.25rem;
            padding: 1rem 1.1rem;
            border: 1px solid #d6d6d6;
            border-left: 5px solid var(--blue);
            border-radius: 12px;
            background: #fafafa;
        }}
        .source-navigation ul {{ margin: 0.65rem 0 0; }}
        pre {{
            margin: 1rem 0 1.5rem;
            padding: 1.1rem;
            border: 1px solid #3b3b3b;
            border-radius: 12px;
            overflow-x: auto;
            background: #111111;
            color: #f1f1f1;
            box-shadow: inset 0 0 0 1px rgba(255,255,255,0.025), 0 8px 22px rgba(0,0,0,0.12);
            font-size: 0.78rem;
            line-height: 1.5;
        }}
        code {{
            padding: 0.12em 0.34em;
            border: 1px solid #dddddd;
            border-radius: 5px;
            background: #f3f3f3;
            color: #8f151b;
            font-size: 0.9em;
        }}
        pre code {{
            background: transparent;
            color: inherit;
            border: 0;
            padding: 0;
        }}
        a {{
            color: var(--blue);
            text-decoration: none;
            text-decoration-thickness: 1px;
            text-underline-offset: 0.16em;
        }}
        a:hover {{ text-decoration: underline; }}
        a:focus-visible {{ outline: 3px solid rgba(197,34,40,0.35); outline-offset: 3px; border-radius: 4px; }}
        blockquote {{
            margin: 1rem 0;
            padding: 0.85rem 1rem;
            border-left: 5px solid var(--medium);
            border-radius: 0 10px 10px 0;
            background: #f7f7f7;
        }}
        .severity-legend {{
            display: flex;
            flex-wrap: wrap;
            gap: 0.55rem;
            margin: 1rem 0 1.5rem;
            padding: 0.85rem 1rem;
            border: 1px solid var(--line);
            border-radius: 12px;
            background: var(--surface);
            color: var(--muted);
            font-size: 0.82rem;
        }}
        .severity-legend strong {{ color: var(--navy); margin-right: 0.25rem; }}
        .legend-chip {{ padding: 0.18rem 0.5rem; border-radius: 999px; color: white; font-weight: 800; letter-spacing: 0.04em; }}
        .legend-critical {{ background: var(--critical); }}
        .legend-high {{ background: var(--high); }}
        .legend-medium {{ background: var(--medium); }}
        .legend-info {{ background: var(--info); }}
        @media (max-width: 980px) {{
            body {{ display: block; padding: 1rem; }}
            .toc {{ position: relative; top: 0; max-height: none; margin-bottom: 1rem; }}
            .actions-section-title + ul, .assessment-list-title + ul {{ grid-template-columns: 1fr; }}
            .plan-conclusion {{ grid-template-columns: 1fr; }}
            .plan-toolbar span {{ width: 100%; margin-left: 0; }}
            .sql-context, .plan-conclusion {{ grid-template-columns: 1fr; }}
            .evidence-appendix ul {{ columns: 1; }}
            td {{ min-width: 9rem; }}
        }}
        @media print {{
            body {{ display: block; padding: 0; background: white; font-size: 10pt; }}
            .toc {{ position: static; max-height: none; box-shadow: none; page-break-after: always; }}
            .section-title {{ break-before: page; box-shadow: none; }}
            .finding-title, .subsection-title, pre {{ break-inside: avoid; box-shadow: none; }}
            .table-scroll {{ overflow: visible; border: 0; box-shadow: none; }}
            .table-scroll::after, .plan-toolbar {{ display: none; }}
            .table-sort-indicator {{ display: none; }}
            table {{ width: 100%; min-width: 0; font-size: 7pt; box-shadow: none; }}
            th {{ position: static; }}
            th, td {{ min-width: 0; max-width: none; padding: 0.3rem; overflow-wrap: anywhere; }}
            tr {{ break-inside: avoid; }}
            .plan-review {{ break-inside: auto; box-shadow: none; }}
            .plan-canvas {{ overflow: visible; padding: 0.4rem; }}
            .plan-tree {{ width: 100%; zoom: 0.72; }}
            .plan-node {{ width: 100%; }}
            a {{ color: inherit; text-decoration: underline; }}
        }}
    </style>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <style>{reader_css}</style>
</head>
<body>
<header class="brand-banner"><a href="https://github.com/ora600pl/jas-min" target="_blank" rel="noopener" aria-label="JAS-MIN project">{brand_logo}</a></header>
{toc}
{navigation}
{content}
<footer class="brand-footer"><a href="https://www.ora-600.pl" target="_blank" rel="noopener"><img src="data:image/png;base64,{ora600_logo}" width="220" alt="ORA-600 Database Whisperers"/></a></footer>
<script>
function tableSortText(cell) {{
    return (cell.textContent || "").replace(/\s+/g, " ").trim();
}}
function tableSortNumber(value) {{
    const normalized = value
        .replace(/\u2212/g, "-")
        .replace(/,/g, "")
        .trim();
    const match = normalized.match(/^([+-]?(?:\d+(?:\.\d+)?|\.\d+)(?:e[+-]?\d+)?)(?:\s*(?:%|ms|s|sec|seconds?|bytes?|kb|mb|gb|tb|\/s|per second))?$/i);
    return match ? Number(match[1]) : null;
}}
function tableSortDate(value) {{
    const match = value.match(/^(\d{{1,2}})-([A-Za-z]{{3}})-(\d{{2,4}})\s+(\d{{2}}):(\d{{2}})(?::(\d{{2}}))?/);
    if (!match) return null;
    const months = {{jan:0,feb:1,mar:2,apr:3,may:4,jun:5,jul:6,aug:7,sep:8,oct:9,nov:10,dec:11}};
    const month = months[match[2].toLowerCase()];
    if (month === undefined) return null;
    let year = Number(match[3]);
    if (year < 100) year += 2000;
    return Date.UTC(year, month, Number(match[1]), Number(match[4]), Number(match[5]), Number(match[6] || 0));
}}
function tableSortValue(cell) {{
    const text = tableSortText(cell);
    const number = tableSortNumber(text);
    if (number !== null && Number.isFinite(number)) return {{kind: "number", value: number}};
    const date = tableSortDate(text);
    if (date !== null) return {{kind: "date", value: date}};
    return {{kind: "text", value: text.toLocaleLowerCase()}};
}}
document.querySelectorAll(".table-scroll table").forEach(function (table) {{
    const body = table.tBodies[0];
    const headerRow = table.tHead && table.tHead.rows[0];
    if (!body || !headerRow || body.rows.length < 2) return;
    Array.from(body.rows).forEach(function (row, index) {{ row.dataset.originalOrder = String(index); }});
    Array.from(headerRow.cells).forEach(function (header, columnIndex) {{
        const label = tableSortText(header);
        const button = document.createElement("button");
        const indicator = document.createElement("span");
        button.type = "button";
        button.className = "table-sort-button";
        button.setAttribute("aria-label", "Sort by " + label + " ascending");
        indicator.className = "table-sort-indicator";
        indicator.setAttribute("aria-hidden", "true");
        indicator.textContent = "↕";
        button.append(document.createTextNode(label), indicator);
        header.textContent = "";
        header.appendChild(button);
        header.setAttribute("aria-sort", "none");
        button.addEventListener("click", function () {{
            const ascending = header.getAttribute("aria-sort") !== "ascending";
            Array.from(headerRow.cells).forEach(function (other) {{
                other.setAttribute("aria-sort", "none");
                const otherIndicator = other.querySelector(".table-sort-indicator");
                if (otherIndicator) otherIndicator.textContent = "↕";
            }});
            header.setAttribute("aria-sort", ascending ? "ascending" : "descending");
            indicator.textContent = ascending ? "↑" : "↓";
            button.setAttribute("aria-label", "Sort by " + label + (ascending ? " descending" : " ascending"));
            const rows = Array.from(body.rows);
            rows.sort(function (left, right) {{
                const a = tableSortValue(left.cells[columnIndex]);
                const b = tableSortValue(right.cells[columnIndex]);
                let comparison;
                if (a.kind === b.kind && a.kind !== "text") comparison = a.value - b.value;
                else comparison = String(a.value).localeCompare(String(b.value), undefined, {{numeric: true, sensitivity: "base"}});
                if (comparison === 0) comparison = Number(left.dataset.originalOrder) - Number(right.dataset.originalOrder);
                return ascending ? comparison : -comparison;
            }});
            rows.forEach(function (row) {{ body.appendChild(row); }});
        }});
    }});
}});
document.addEventListener("click", function (event) {{
    const filter = event.target.closest("[data-plan-filter]");
    if (filter) {{
        const review = filter.closest("[data-plan-review]");
        const pressed = filter.getAttribute("aria-pressed") !== "true";
        filter.setAttribute("aria-pressed", String(pressed));
        filter.textContent = pressed ? "Show complete plan" : "Show flagged paths only";
        review.querySelectorAll("[data-plan-node]").forEach(function (node) {{
            node.dataset.filtered = String(pressed && node.dataset.onFlaggedPath !== "true");
        }});
        return;
    }}
    const toggle = event.target.closest("[data-plan-node-toggle]");
    if (!toggle) return;
    const node = toggle.closest("[data-plan-node]");
    const depth = Number(node.dataset.depth || 0);
    const expanded = toggle.getAttribute("aria-expanded") === "true";
    let sibling = node.nextElementSibling;
    while (sibling && sibling.matches("[data-plan-node]")) {{
        if (Number(sibling.dataset.depth || 0) <= depth) break;
        sibling.dataset.collapsed = String(expanded);
        sibling = sibling.nextElementSibling;
    }}
    toggle.setAttribute("aria-expanded", String(!expanded));
    toggle.textContent = expanded ? "+" : "−";
}});
document.addEventListener("input", function (event) {{
    if (!event.target.matches("[data-plan-zoom]")) return;
    const review = event.target.closest("[data-plan-review]");
    review.querySelector(".plan-tree").style.setProperty("--plan-zoom", String(Number(event.target.value) / 100));
}});
</script>
<script>{signals_js}</script>
<script>{reader_js}</script>
</body>
</html>"#,
        reader_css = concat!(
            include_str!("../report/assets/report_reader.css"),
            "\n",
            include_str!("../report/assets/report_signals.css")
        ),
        signals_js = include_str!("../report/assets/report_signals.js"),
        reader_js = include_str!("../report/assets/report_reader.js"),
        brand_logo = JASMIN_AUDIT_LOGO_SVG,
        toc = toc_html,
        navigation = classic_navigation,
        content = html_output,
        ora600_logo = ora600_logo_data
    )
}

/// Maps pulldown_cmark HeadingLevel to integer
fn heading_level_to_int(level: &HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

pub fn add_links_to_html(
    html: String,
    events_sqls: HashMap<&str, HashSet<String>>,
    html_dir: String,
    html_absolute_dir: String,
) -> String {
    let mut html_with_links: String = html;
    let bgevents = events_sqls.get("BG").cloned().unwrap_or_default();
    for (name_type, names) in events_sqls {
        //first deal with Forground events and SQLIDs
        for name in names {
            if name_type == "FG" {
                let file_name = get_safe_filename(name.clone(), "fg".to_string());
                let path = Path::new(&html_dir).join(&file_name);
                let absolute_path = Path::new(&html_absolute_dir).join(&file_name);
                if absolute_path.exists() {
                    let link_txt = format!(
                        r#"<a href={} target="_blank">{}</a>"#,
                        path.to_string_lossy(),
                        &name
                    );
                    let link_txt2 = format!(
                        r#"<strong><a href={} target="_blank">{}</a>"#,
                        path.to_string_lossy(),
                        &name
                    );
                    let from_name = format!("<code>{}</code>", &name);
                    let from_name2 = format!("<strong>{}", &name);
                    html_with_links = html_with_links.replace(&from_name, &link_txt);
                    html_with_links = html_with_links.replace(&from_name2, &link_txt2);
                    //println!("added link for: {}", &name);
                }
            } else if name_type == "SQL" {
                let file_name = format!("{}/sqlid/sqlid_{}.html", html_dir, &name);
                let absolute_file_name =
                    format!("{}/sqlid/sqlid_{}.html", html_absolute_dir, &name);
                let path = Path::new(&absolute_file_name);
                if path.exists() {
                    let link_txt =
                        format!(r#"<a href={} target="_blank">{}</a>"#, file_name, &name);
                    html_with_links = html_with_links.replace(&name, &link_txt);
                }
            }
        }
    }
    for name in bgevents {
        //then check what's left for Background Events
        let file_name = get_safe_filename(name.clone(), "bg".to_string());
        let path = Path::new(&html_dir).join(&file_name);
        let absolute_path = Path::new(&html_absolute_dir).join(&file_name);
        if absolute_path.exists() {
            let link_txt = format!(
                r#"<a href={} target="_blank">{}</a>"#,
                path.to_string_lossy(),
                &name
            );
            let link_txt2 = format!(
                r#"<strong><a href={} target="_blank">{}</a>"#,
                path.to_string_lossy(),
                &name
            );
            let from_name = format!("<code>{}</code>", &name);
            let from_name2 = format!("<strong>{}", &name);
            html_with_links = html_with_links.replace(&from_name, &link_txt);
            html_with_links = html_with_links.replace(&from_name2, &link_txt2);
        }
    }
    html_with_links
}

/// Renders Markdown with the same template and report links used by classic AI mode.
///
/// Keeping this function free of file-system and GUI side effects allows the MCP
/// adapter to enforce its own output-path policy while sharing the exact renderer.
pub(crate) fn render_markdown_html_document(
    markdown: &str,
    html_dir: &str,
    html_absolute_dir: &str,
    events_sqls: HashMap<&str, HashSet<String>>,
) -> String {
    debug_note!(
        "Rendering Markdown HTML document: markdown_bytes={}, html_dir='{}', absolute_dir='{}', link_groups={}",
        markdown.len(),
        html_dir,
        html_absolute_dir,
        events_sqls.len()
    );
    try_render_markdown_html_document(markdown, html_dir, html_absolute_dir, events_sqls)
        .expect("invalid report decision metadata")
}

pub(crate) fn try_render_markdown_html_document(
    markdown: &str,
    html_dir: &str,
    html_absolute_dir: &str,
    events_sqls: HashMap<&str, HashSet<String>>,
) -> Result<String, String> {
    let (prepared, _) = crate::report_issues::prepare_api_report(markdown)?;
    let html_plain = markdown_to_html_with_toc(&prepared, html_dir, html_absolute_dir);
    let html = add_links_to_html(
        html_plain,
        events_sqls,
        html_dir.to_string(),
        html_absolute_dir.to_string(),
    );
    debug_note!("Markdown HTML document rendered: html_bytes={}", html.len());
    Ok(html)
}

/// Reads a Markdown file, converts to HTML with TOC, writes to .html file
pub fn convert_md_to_html_file(
    input_path: &str,
    events_sqls: HashMap<&str, HashSet<String>>,
) -> Result<(), String> {
    debug_note!(
        "Starting Markdown file conversion: input='{}', link_groups={}",
        input_path,
        events_sqls.len()
    );
    let markdown = fs::read_to_string(input_path)
        .unwrap_or_else(|_| panic!("Could not read file '{}'", input_path));

    let mut html_dir = format!(
        "{}.html_reports",
        input_path.split('.').collect::<Vec<&str>>()[0]
    );
    let html_absolute_dir = html_dir.clone();
    if input_path.contains("_deep_") {
        html_dir = ".".to_string();
    }
    let html =
        try_render_markdown_html_document(&markdown, &html_dir, &html_absolute_dir, events_sqls)?;

    let output_path = Path::new(input_path).with_extension("html");

    fs::write(&output_path, html)
        .unwrap_or_else(|_| panic!("Could not write to file '{:?}'", output_path));

    debug_note!(
        "Markdown file conversion completed: output='{}'",
        output_path.display()
    );

    println!("✅ HTML file generated at: {:?}", output_path);
    open::that(output_path);
    Ok(())
}
