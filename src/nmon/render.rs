use super::model::NmonDataset;
use html_escape::encode_text;
use plotly::common::Mode;
use plotly::{Plot, Scatter};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

pub fn write_overview(dataset: &NmonDataset, html_root: &Path) -> Result<(), String> {
    let output_dir = html_root.join("nmon");
    fs::create_dir_all(&output_dir).map_err(|error| {
        format!(
            "cannot create NMON HTML directory '{}': {error}",
            output_dir.display()
        )
    })?;

    let cpu_plot = plot_for_prefixes(
        dataset,
        "nmon-cpu-plot",
        &[
            "cpu_all.user_pct",
            "cpu_all.sys_pct",
            "cpu_all.wait_pct",
            "cpu_all.busy",
        ],
    );
    let lpar_plot = plot_for_prefixes(
        dataset,
        "nmon-lpar-plot",
        &[
            "lpar.physicalcpu",
            "lpar.entitlement_utilization_pct",
            "process.runnable",
        ],
    );
    let disk_keys = dataset
        .metrics
        .iter()
        .filter(|(_, metric)| {
            metric.domain == "disk"
                && matches!(
                    metric.section.as_str(),
                    "DISKREAD" | "DISKWRITE" | "DISKRIO" | "DISKWIO" | "DISKBUSY" | "DISKWAIT"
                )
        })
        .map(|(key, _)| key.as_str())
        .collect::<Vec<_>>();
    let disk_plot = plot_for_prefixes(dataset, "nmon-disk-plot", &disk_keys);

    let metadata = &dataset.metadata;
    let capture = &dataset.capture;
    let plotly_scripts = Plot::offline_js_sources();
    let brand = crate::tools::jasmin_brand_banner_html();
    let mut summary_rows = String::new();
    for (key, summary) in &dataset.summaries {
        let metric = &dataset.metrics[key];
        if metric.domain == "cpu" {
            continue;
        }
        summary_rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            encode_text(key),
            encode_text(metric.unit.as_deref().unwrap_or("")),
            summary.count,
            format_value(summary.average),
            format_value(summary.p95),
            format_value(summary.p99),
            format_value(summary.max),
            encode_text(&format!("{:?}", metric.source).to_ascii_lowercase()),
        ));
    }
    let html = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>NMON dataset</title>
<style>body{{font-family:Arial,sans-serif;margin:24px;color:#24292f}}.jasmin-brand-banner svg{{display:block;width:min(100%,520px);height:auto;margin:0}}table{{border-collapse:collapse;width:100%;font-size:13px}}th,td{{border:1px solid #d0d7de;padding:8px;text-align:right}}th:first-child,td:first-child{{text-align:left}}thead th{{background:#111;color:#fff;cursor:pointer;user-select:none;white-space:nowrap}}thead th::after{{content:" ↕";color:#b9b9b9;font-size:11px}}thead th[aria-sort="ascending"]::after{{content:" ▲";color:#fff}}thead th[aria-sort="descending"]::after{{content:" ▼";color:#fff}}tbody tr:nth-child(even){{background:#f2f2f2}}tbody tr:hover{{background:#ece5f7}}.meta{{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:10px}}.box{{border:1px solid #d0d7de;border-radius:8px;padding:12px}}.plot-wrap{{width:100%;height:520px;min-height:520px}}.table-tools{{display:flex;align-items:center;gap:10px;margin:0 0 12px;flex-wrap:wrap}}.table-tools label{{font-weight:bold}}.table-filter{{box-sizing:border-box;width:min(100%,420px);padding:9px 11px;border:1px solid #8c959f;border-radius:4px;font:inherit}}.table-scroll{{overflow-x:auto;border:1px solid #d0d7de}}.table-scroll table{{border:0}}.table-scroll th:first-child,.table-scroll td:first-child{{border-left:0}}.table-scroll th:last-child,.table-scroll td:last-child{{border-right:0}}.filter-status{{color:#57606a;font-size:13px}}h1,h2{{color:#4b2e83}}code{{font-size:12px}}</style>{plotly_scripts}</head><body>
{brand}<h1>NMON dataset</h1>
<div class="meta"><div class="box"><b>Host</b><br>{host}<br>{os}</div><div class="box"><b>LPAR</b><br>{lpar}<br>mode={mode}, capped={capped}, entitlement={entitlement}</div><div class="box"><b>Capture</b><br>{start} — {end}<br>{samples} samples, {interval}s interval</div></div>
<p>Timezone: {timezone}. Missing values remain missing; they are not converted to zero. Derived metrics are labelled separately.</p>
<h2>CPU</h2><div class="plot-wrap">{cpu_plot}</div><h2>LPAR and run queue</h2><div class="plot-wrap">{lpar_plot}</div><h2>Disk activity (original NMON metric identities)</h2><div class="plot-wrap">{disk_plot}</div>
<h2>Metric summaries</h2>
<div class="table-tools"><label for="metric-filter">Filter metrics</label><input id="metric-filter" class="table-filter" type="search" placeholder="Type part of a metric name…" autocomplete="off"><span id="metric-filter-status" class="filter-status" aria-live="polite"></span></div>
<div class="table-scroll" tabindex="0"><table id="metric-summaries"><thead><tr><th scope="col" tabindex="0">Metric key</th><th scope="col" tabindex="0">Unit</th><th scope="col" tabindex="0">Count</th><th scope="col" tabindex="0">Average</th><th scope="col" tabindex="0">P95</th><th scope="col" tabindex="0">P99</th><th scope="col" tabindex="0">Max</th><th scope="col" tabindex="0">Source</th></tr></thead><tbody>{summary_rows}</tbody></table></div>
<script>
(() => {{
  const table = document.getElementById("metric-summaries");
  const body = table.tBodies[0];
  const headers = Array.from(table.tHead.rows[0].cells);
  const filter = document.getElementById("metric-filter");
  const status = document.getElementById("metric-filter-status");
  const total = body.rows.length;
  Array.from(body.rows).forEach((row, index) => row.dataset.originalOrder = index);

  function updateFilter() {{
    const pattern = filter.value.trim().toLocaleLowerCase();
    let visible = 0;
    Array.from(body.rows).forEach(row => {{
      const matches = row.cells[0].textContent.toLocaleLowerCase().includes(pattern);
      row.hidden = !matches;
      if (matches) visible += 1;
    }});
    status.textContent = `${{visible}} of ${{total}} metrics`;
  }}

  function sortValue(cell) {{
    const text = cell.textContent.trim();
    const number = Number(text);
    return text !== "" && Number.isFinite(number)
      ? {{ kind: "number", value: number }}
      : {{ kind: "text", value: text }};
  }}

  function sortBy(column) {{
    const header = headers[column];
    const ascending = header.getAttribute("aria-sort") !== "ascending";
    headers.forEach(cell => cell.removeAttribute("aria-sort"));
    header.setAttribute("aria-sort", ascending ? "ascending" : "descending");
    const rows = Array.from(body.rows);
    rows.sort((left, right) => {{
      const a = sortValue(left.cells[column]);
      const b = sortValue(right.cells[column]);
      let comparison;
      if (a.kind === "number" && b.kind === "number") comparison = a.value - b.value;
      else comparison = String(a.value).localeCompare(String(b.value), undefined, {{ numeric: true, sensitivity: "base" }});
      if (comparison === 0) comparison = Number(left.dataset.originalOrder) - Number(right.dataset.originalOrder);
      return ascending ? comparison : -comparison;
    }});
    rows.forEach(row => body.appendChild(row));
  }}

  headers.forEach((header, column) => {{
    header.addEventListener("click", () => sortBy(column));
    header.addEventListener("keydown", event => {{
      if (event.key === "Enter" || event.key === " ") {{
        event.preventDefault();
        sortBy(column);
      }}
    }});
  }});
  filter.addEventListener("input", updateFilter);
  updateFilter();
}})();
</script>
</body></html>"#,
        host = encode_text(metadata.host.as_deref().unwrap_or("unknown")),
        os = encode_text(
            metadata
                .os_version
                .as_deref()
                .unwrap_or("unknown OS version")
        ),
        lpar = encode_text(
            metadata
                .lpar
                .partition_name
                .as_deref()
                .unwrap_or("unknown partition")
        ),
        mode = encode_text(metadata.lpar.processor_mode.as_deref().unwrap_or("unknown")),
        capped = metadata
            .lpar
            .capped
            .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
        entitlement = metadata
            .lpar
            .entitled_capacity
            .map_or_else(|| "unknown".to_string(), |value| format!("{value:.3}")),
        start = encode_text(capture.start.as_deref().unwrap_or("unknown")),
        end = encode_text(capture.end.as_deref().unwrap_or("unknown")),
        samples = capture.sample_count,
        interval = capture
            .sampling_interval_seconds
            .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
        timezone = encode_text(
            capture
                .timezone
                .as_deref()
                .unwrap_or("not supplied by NMON")
        ),
    );
    let output = output_dir.join("nmon_overview.html");
    fs::write(&output, html)
        .map_err(|error| format!("cannot write NMON HTML '{}': {error}", output.display()))
}

fn plot_for_prefixes(dataset: &NmonDataset, id: &str, keys: &[&str]) -> String {
    let unique = keys.iter().copied().collect::<BTreeSet<_>>();
    let mut plot = Plot::new();
    for key in unique {
        let Some(metric) = dataset.metrics.get(key) else {
            continue;
        };
        let trace = Scatter::new(dataset.timestamps.clone(), metric.values.clone())
            .mode(Mode::Lines)
            .name(key);
        plot.add_trace(trace);
    }
    plot.to_inline_html(Some(id))
}

fn format_value(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_string(), |value| format!("{value:.4}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nmon::model::{NmonMetricSeries, NmonMetricSource};

    #[test]
    fn generated_page_embeds_plotly_and_sizes_plot_containers() {
        let mut dataset = NmonDataset {
            timestamps: vec!["2026-09-10T10:00:00".to_string()],
            ..Default::default()
        };
        dataset.metrics.insert(
            "cpu_all.user_pct".to_string(),
            NmonMetricSeries {
                section: "CPU_ALL".to_string(),
                source_name: "User%".to_string(),
                source_label: "User%".to_string(),
                domain: "cpu_all".to_string(),
                unit: Some("%".to_string()),
                source: NmonMetricSource::Observed,
                values: vec![Some(10.0)],
                ..Default::default()
            },
        );
        let root = std::env::temp_dir().join(format!(
            "jasmin-nmon-render-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&root);
        write_overview(&dataset, &root).unwrap();
        let html = fs::read_to_string(root.join("nmon/nmon_overview.html")).unwrap();
        assert!(html.contains("Plotly.newPlot"));
        assert!(html.contains("class=\"plot-wrap\""));
        assert!(html.contains("height:520px"));
        assert!(html.contains("class=\"jasmin-brand-banner\""));
        assert!(html.contains("<title>NMON dataset</title>"));
        assert!(html.contains("<h1>NMON dataset</h1>"));
        assert!(!html.contains("prepared NMON dataset"));
        assert!(html.contains("id=\"metric-filter\""));
        assert!(html.contains("id=\"metric-summaries\""));
        assert!(html.contains("function updateFilter()"));
        assert!(html.contains("function sortBy(column)"));
        fs::remove_dir_all(root).unwrap();
    }
}
