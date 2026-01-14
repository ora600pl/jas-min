use std::collections::BTreeMap;
use std::{env, fs, collections::HashMap, path::Path, collections::HashSet};
use std::fs::File;
use std::fmt::Write;
use std::io::{BufRead, BufReader, BufWriter, stdout, Write as Write2};
use ndarray::{iter, Array1, Array2};
use ndarray_stats::{CorrelationExt, QuantileExt};
use ndarray_stats::histogram::Grid;
use ndarray_stats::interpolate::Linear;
use noisy_float::types::N64;
use prettytable::{Table, Row, Cell};
use html_escape::encode_text;
use pulldown_cmark::{html, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use crate::awr::GetStats;
use tokio::sync::oneshot;
use serde::Serialize;



pub fn table_to_html_string(table: &Table, title: &str, headers: &[&str]) -> String {
    let mut html = String::new();
    html.push_str(&format!(r#"<p><span style="color:blue;font-weight:bold;">{title}<br></span>"#, title=title));

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
            write!(html, "<td>{}</td>", cell.get_content()).unwrap();
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
fn markdown_to_html_with_toc(markdown_input: &str, html_dir: &str) -> String {
    // Enable desired Markdown extensions
    let mut options = Options::empty();
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);

    // Parse the Markdown with extensions
    let parser = Parser::new_ext(markdown_input, options);

    // Prepare variables
    let mut toc: Vec<(usize, String)> = Vec::new(); // (level, id, title)
    let mut html_output = String::new();    // Final HTML body
    let mut parser_with_ids = Vec::new();   // Modified event stream
    let mut heading_counter = 0;            // For generating unique IDs
    let mut current_heading_level = 1;      // For closing tags manually
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
            Event::Start(Tag::Heading { level, .. }) => {
                heading_counter += 1;
                current_heading_level = heading_level_to_int(level);
                current_heading_level_for_map = current_heading_level;
                let id = format!("section-{}", heading_counter);
                current_heading_id = id.clone();
                heading_text.clear();
                in_heading = true;
                // Add heading to TOC
                toc.push((current_heading_level, current_heading_id.clone()));
                // Inject heading with id
                parser_with_ids.push(Event::Html(
                    format!(r#"<h{} id="{}">"#, current_heading_level, id).into(),
                ));
                // Buffer the heading events, but also collect text
                heading_events_buffer.clear();
            }
            Event::End(TagEnd::Heading { .. }) => {
                in_heading = false;
                // Add the heading text to the map
                headings_map.insert(current_heading_id.clone(), heading_text.clone());
                // Push any buffered heading events (if any)
                for buffered_event in heading_events_buffer.drain(..) {
                    parser_with_ids.push(buffered_event);
                }
                // Close heading tag manually
                parser_with_ids.push(Event::Html(
                    format!("</h{}>", current_heading_level_for_map).into(),
                ));
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
    let mut toc_html = String::from("<div class=\"toc\"><h2>Table of Contents</h2><ul>");
    for (level, id) in &toc {
        let label = encode_text(&headings_map[id]);
        toc_html.push_str(&format!(
            r##"<li class="level-{}"><a href="#{}">{}</a></li>"##,
            level,
            id,
            label
        ));
    }
    toc_html.push_str("</ul></div>");

    // Render HTML from modified parser stream
    html::push_html(&mut html_output, parser_with_ids.into_iter());

    let jasmin_main = format!("{}/jasmin_main.html", &html_dir);
    let load_profile = format!("{}/stats/jasmin_highlight.html", &html_dir);
    let load_profile2 = format!("{}/stats/jasmin_highlight2.html", &html_dir);

    // Wrap the result in a complete HTML template
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>JAS-MIN thoughts</title>
    <style>
        body {{
            font-family: Arial, sans-serif;
            padding: 2em;
            background: #fdfdfd;
            color: #333;
        }}
        .toc {{
            background: #f0f0f0;
            padding: 1em;
            margin-bottom: 2em;
            border-left: 4px solid #444;
        }}
        .toc ul {{
            list-style: none;
            padding-left: 0;
        }}
        .toc li {{
            margin: 0.5em 0;
        }}
        .toc li.level-1 {{ margin-left: 0em; }}
        .toc li.level-2 {{ margin-left: 1em; }}
        .toc li.level-3 {{ margin-left: 2em; }}
        .toc li.level-4 {{ margin-left: 3em; }}
        pre {{
            background: #c2cc91ff;
            color: #f8f8f2;
            padding: 1em;
            overflow-x: auto;
        }}
        code {{
            background: #c2cc91ff;
            padding: 0.2em 0.4em;
            border-radius: 4px;
        }}
        a {{
            color: #0077cc;
            text-decoration: none;
        }}
        a:hover {{
            text-decoration: underline;
        }}
    </style>
</head>
<body>
<p align="center"><a href="https://github.com/ora600pl/jas-min" target="_blank">
        <img src="https://raw.githubusercontent.com/ora600pl/jas-min/main/img/jasmin_LOGO_white.png" width="150" alt="JAS-MIN" onerror="this.style.display='none';"/>
    </a></p>
{toc}
<iframe src="{lp}" width="100%" height="400px" style="border: none;"></iframe>
<iframe src="{lp2}" width="100%" height="400px" style="border: none;"></iframe>
<a href="{jm}" target="_blank">JASMIN MAIN</a>
{content}
<p align="center"><a href="https://www.ora-600.pl" target="_blank">
        <img src="https://raw.githubusercontent.com/ora600pl/jas-min/main/img/ora-600.png" width="150" alt="ORA-600" onerror="this.style.display='none';"/>
    </a></p>
</body>
</html>"#,
        toc = toc_html,
        lp = load_profile,
        lp2 = load_profile2,
        jm = jasmin_main,
        content = html_output
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

fn add_links_to_html(html: String, events_sqls: HashMap<&str, HashSet<String>>, html_dir: String, html_absolute_dir: String) -> String {
    let mut html_with_links: String = html;
    let bgevents = events_sqls.get("BG").unwrap().clone();
    for (name_type, names) in events_sqls { //first deal with Forground events and SQLIDs
        for name in names {
            if name_type == "FG" {
                let file_name = get_safe_filename(name.clone(), "fg".to_string());
                let path = Path::new(&html_dir).join(&file_name);
                let absolute_path = Path::new(&html_absolute_dir).join(&file_name);
                if absolute_path.exists() {
                    let link_txt = format!(r#"<a href={} target="_blank">{}</a>"#, path.to_string_lossy(), &name);
                    let link_txt2 = format!(r#"<strong><a href={} target="_blank">{}</a>"#, path.to_string_lossy(), &name);
                    let from_name = format!("<code>{}</code>", &name);
                    let from_name2 = format!("<strong>{}", &name);
                    html_with_links = html_with_links.replace(&from_name, &link_txt);
                    html_with_links = html_with_links.replace(&from_name2, &link_txt2);
                    //println!("added link for: {}", &name);
                }
            } else if name_type == "SQL" {
                let file_name = format!("{}/sqlid/sqlid_{}.html", html_dir, &name);
                let absolute_file_name= format!("{}/sqlid/sqlid_{}.html", html_absolute_dir, &name);
                let path = Path::new(&absolute_file_name);
                if path.exists() {
                    let link_txt = format!(r#"<a href={} target="_blank">{}</a>"#, file_name, &name);
                    html_with_links = html_with_links.replace(&name, &link_txt);
                }
            }
        }
    }
    for name in bgevents { //then check what's left for Background Events
        let file_name = get_safe_filename(name.clone(), "bg".to_string());
        let path = Path::new(&html_dir).join(&file_name);
        let absolute_path = Path::new(&html_absolute_dir).join(&file_name);
        if absolute_path.exists() {
            let link_txt = format!(r#"<a href={} target="_blank">{}</a>"#, path.to_string_lossy(), &name);
            let link_txt2 = format!(r#"<strong><a href={} target="_blank">{}</a>"#, path.to_string_lossy(), &name);
            let from_name = format!("<code>{}</code>", &name);
            let from_name2 = format!("<strong>{}", &name);
            html_with_links = html_with_links.replace(&from_name, &link_txt);
            html_with_links = html_with_links.replace(&from_name2, &link_txt2);
        }
    }
    html_with_links
}

/// Reads a Markdown file, converts to HTML with TOC, writes to .html file
pub fn convert_md_to_html_file(input_path: &str, events_sqls: HashMap<&str, HashSet<String>>) {
    let markdown = fs::read_to_string(input_path)
        .unwrap_or_else(|_| panic!("Could not read file '{}'", input_path));

    let mut html_dir = format!("{}.html_reports", input_path.split('.').collect::<Vec<&str>>()[0]);
    let html_absolute_dir = html_dir.clone();
    if input_path.contains("_deep_") {
        html_dir = ".".to_string();
    }
    let html_plain = markdown_to_html_with_toc(&markdown, &html_dir);
    let html = add_links_to_html(html_plain, events_sqls, html_dir, html_absolute_dir);

    let output_path = Path::new(input_path).with_extension("html");

    fs::write(&output_path, html)
        .unwrap_or_else(|_| panic!("Could not write to file '{:?}'", output_path));

    println!("✅ HTML file generated at: {:?}", output_path);
    open::that(output_path);
}

//Calculate pearson correlation of 2 vectors and return simple result
pub fn pearson_correlation_2v(vec1: &Vec<f64>, vec2: &Vec<f64>) -> f64 {
    let rows: usize = 2;
    let cols: usize = vec1.len();

    let mut data: Vec<f64> = Vec::new();
    data.extend(vec1);
    data.extend(vec2);
    
    let a: ndarray::ArrayBase<ndarray::OwnedRepr<f64>, ndarray::Dim<[usize; 2]>> = Array2::from_shape_vec((rows, cols), data).unwrap();
    let crr = a.pearson_correlation().unwrap();

    crr.row(0)[1]
}

pub fn mean(data: Vec<f64>) -> Option<f64> {
    let sum: f64 = data.iter().sum::<f64>() as f64;
    let count: usize = data.len();

    match count {
        positive if positive > 0 => Some(sum / count as f64),
        _ => None,
    }
}

pub fn std_deviation(data: Vec<f64>) -> Option<f64> {
    match (mean(data.clone()), data.len()) {
        (Some(data_mean), count) if count > 0 => {
            let variance: f64 = data.iter().map(|value| {
                let diff: f64 = data_mean - (*value as f64);

                diff * diff
            }).sum::<f64>() / count as f64;

            Some(variance.sqrt())
        },
        _ => None
    }
}

pub fn median(data: &[f64]) -> f64 {
    let mut sorted = data.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let len = sorted.len();
    if len == 0 {
        return 0.0;
    }
    if len % 2 == 0 {
        (sorted[len / 2 - 1] + sorted[len / 2]) / 2.0
    } else {
        sorted[len / 2]
    }
}

pub fn mad(data: &[f64], med: f64) -> f64 {
    let deviations: Vec<f64> = data.iter().map(|x| (x - med).abs()).collect();
    median(&deviations)
}

pub fn get_safe_filename(name: String, category: String ) -> String {
    // Replace invalid characters for filenames (e.g., slashes or spaces)
    let safe_event_name: String = name.replace("/", "_").replace(" ", "_").replace(":","").replace("*","_");
    let mut file_name: String = String::new();
    if category == "fg".to_string(){
        file_name = format!("fg/fg_{}.html", safe_event_name);
    } else if category == "bg".to_string(){
        file_name = format!("bg/bg_{}.html", safe_event_name);
    } else if category == "inst_stat".to_string(){
        file_name = format!("stats/stat_{}.html", safe_event_name);
    }
    file_name
}

pub fn get_statistics(data: Vec<f64>) -> Option<GetStats> {
    if data.is_empty() {
        return None;
    }

    let samples = data.len() as u64;
    let arr = Array1::from(data);
    
    // Calculate basic stats using ndarray methods
    let min = *arr.min().unwrap();
    let max = *arr.max().unwrap();
    let mean = arr.mean().unwrap();
    let variance = arr.var(0.0);
    let std_dev = arr.std(0.0);
    
    // Convert to noisy_float for quantile calculations
    let mut arr_n64: Array1<N64> = arr.mapv(N64::new);
    
    // Calculate quartiles
    let q1 = arr_n64.quantile_axis_mut(
        ndarray::Axis(0), 
        N64::new(0.25), 
        &Linear
    ).unwrap().into_scalar().raw();
    
    let median = arr_n64.quantile_axis_mut(
        ndarray::Axis(0), 
        N64::new(0.5), 
        &Linear
    ).unwrap().into_scalar().raw();
    
    let q3 = arr_n64.quantile_axis_mut(
        ndarray::Axis(0), 
        N64::new(0.75), 
        &Linear
    ).unwrap().into_scalar().raw();
    
    // Calculate theoretical fence boundaries
    let iqr = q3 - q1;
    let lower_fence_boundary = q1 - 1.5 * iqr;
    let upper_fence_boundary = q3 + 1.5 * iqr;
    
    // Match Plotly's behavior: fences are the min/max data points within boundaries
    let lower_fence = arr.iter()
        .filter(|&&x| x >= lower_fence_boundary)
        .min_by(|a, b| a.partial_cmp(b).unwrap())
        .copied()
        .unwrap_or(min);
    
    let upper_fence = arr.iter()
        .filter(|&&x| x <= upper_fence_boundary)
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .copied()
        .unwrap_or(max);
    
    let round2 = |x: f64| (x * 100.0).round() / 100.0;
    
    Some(GetStats {
        samples,
        min: round2(min),
        lower_fence: round2(lower_fence),
        q1: round2(q1),
        mean: round2(mean),
        median: round2(median),
        q3: round2(q3),
        upper_fence: round2(upper_fence),
        max: round2(max),
        variance: round2(variance),
        std_dev: round2(std_dev)
    })
}

pub async fn spinning_beer(mut done: oneshot::Receiver<()>) {
    let frames = ["🍺", "🍻", "🍺", "🍻"];
    let mut i = 0;
    while done.try_recv().is_err() {
        print!("\r{}", frames[i % frames.len()]);
        stdout().flush().unwrap();
        i += 1;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    println!("\r✅ Got response!");
}

/// Rough token estimate: ~4 chars per token.
/// Works fine as a budget guardrail for JSON-heavy prompts.
pub fn estimate_tokens_from_str(s: &str) -> usize {
    (s.chars().count() + 3) / 4
}

/// Builds the "combined string" that we actually send (base prompt + capsule + input JSON).
pub fn estimate_request_tokens(base_user_prompt_str: &str, capsule_json_str: &str, input_json: &serde_json::Value) -> usize {
    let input_str = serde_json::to_string(input_json).unwrap_or_default();
    let combined = format!(
        "{}\n{}\nINPUT:\n{}",
        base_user_prompt_str,
        capsule_json_str,
        input_str
    );
    estimate_tokens_from_str(&combined)
}

/// Generic helper: given a sorted Vec<T>, find the maximum prefix length that fits into budget.
/// `wrap` is responsible for putting the slice into the JSON shape used by the section.
pub fn max_prefix_that_fits<T: Serialize>(
    items: &[T],
    base_user_prompt_str: &str,
    capsule_json_str: &str,
    budget_tokens: usize,
    wrap: impl Fn(&[T]) -> serde_json::Value,
) -> usize {
    if items.is_empty() || budget_tokens < 256 {
        return 0;
    }

    let fits = |k: usize| -> bool {
        if k == 0 { return true; }
        let v = wrap(&items[..k]);
        estimate_request_tokens(base_user_prompt_str, capsule_json_str, &v) <= budget_tokens
    };

    if !fits(1) {
        return 0;
    }

    let mut lo = 1usize;
    let mut hi = items.len();

    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if fits(mid) { lo = mid; } else { hi = mid - 1; }
    }

    lo
}
