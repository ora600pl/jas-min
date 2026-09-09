//! A source-backed analytical reading layer. Rendering never changes model math
//! or infers causal agreement from similar words in a narrative.
use html_escape::{encode_double_quoted_attribute as attr, encode_text as esc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignalAtlas {
    pub version: u32,
    pub panels: Vec<SignalPanel>,
    pub briefs: Vec<SignalBrief>,
    pub moments: Vec<SignalMoment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignalPanel {
    pub id: String,
    pub project_id: String,
    pub project_label: String,
    pub window: String,
    pub family: String,
    pub target: String,
    pub coverage: String,
    pub points: Vec<SignalPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignalPoint {
    pub name: String,
    pub active: Option<f64>,
    pub peak: Option<f64>,
    pub classification: String,
    /// Ridge, Elastic Net, Huber, Quantile 95. None means unavailable evidence,
    /// false means not selected in the returned classification, never zero cost.
    pub selected: [Option<bool>; 4],
    pub interpretation: String,
    pub action: String,
    pub href: Option<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignalBrief {
    pub project_id: String,
    pub project_label: String,
    pub title: String,
    pub conclusion: String,
    pub boundary: String,
    pub validation: String,
    pub confidence: String,
    /// Explicit contributor names from this project's panels.
    pub signals: Vec<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignalMoment {
    pub project_id: String,
    pub project_label: String,
    pub kind: String,
    pub window: String,
    pub title: String,
    pub measure: String,
    pub context: String,
    pub evidence_refs: Vec<String>,
}

fn bounded(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= limit
}

pub(crate) fn safe_href(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value.chars().any(|c| c.is_control())
        && !value.starts_with("//")
        && (!value
            .split(['/', '#', '?'])
            .next()
            .unwrap_or("")
            .contains(':')
            || value.starts_with("https://")
            || value.starts_with("http://"))
}

pub(crate) fn validate(atlas: &SignalAtlas) -> Result<(), String> {
    if atlas.version != 1 || atlas.panels.is_empty() || atlas.panels.len() > 64 {
        return Err("Signal atlas needs version 1 and 1..=64 scoped panels".into());
    }
    let mut ids = BTreeSet::new();
    let mut fits = BTreeSet::new();
    let mut projects = BTreeSet::new();
    let check_refs = |refs: &[String]| -> Result<(), String> {
        if refs.is_empty()
            || refs.len() > 32
            || refs.iter().any(|id| {
                id.len() > 64
                    || id.is_empty()
                    || !id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            })
        {
            return Err("Signal items require bounded evidence IDs".into());
        }
        Ok(())
    };
    for panel in &atlas.panels {
        if panel.id.is_empty()
            || panel.id.len() > 64
            || !panel
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            || !ids.insert(&panel.id)
        {
            return Err("Signal panel IDs must be unique ASCII identifiers".into());
        }
        for text in [
            &panel.project_id,
            &panel.project_label,
            &panel.window,
            &panel.family,
            &panel.target,
            &panel.coverage,
        ] {
            if !bounded(text, 1600) {
                return Err("Missing or oversized signal panel scope".into());
            }
        }
        projects.insert(&panel.project_id);
        if !fits.insert((&panel.project_id, &panel.family, &panel.target)) {
            return Err("Duplicate project/family/target fit; use distinct project IDs for separate capture windows".into());
        }
        if panel.points.is_empty() || panel.points.len() > 256 {
            return Err("Signal panel needs 1..=256 contributors".into());
        }
        let mut names = BTreeSet::new();
        for point in &panel.points {
            if !bounded(&point.name, 320) || !names.insert(&point.name) {
                return Err("Duplicate or missing contributor in one fit".into());
            }
            for number in [point.active, point.peak].into_iter().flatten() {
                if !number.is_finite() || number < 0.0 {
                    return Err(
                        "Impact magnitudes must be finite and non-negative; unknown is null".into(),
                    );
                }
            }
            for text in [&point.classification, &point.interpretation, &point.action] {
                if !bounded(text, 4000) {
                    return Err("Missing signal interpretation, classification or next test".into());
                }
            }
            if point.href.as_deref().is_some_and(|href| !safe_href(href)) {
                return Err("Unsafe signal source URL".into());
            }
            check_refs(&point.evidence_refs)?;
        }
    }
    if atlas.briefs.len() > 64 || atlas.moments.len() > 128 {
        return Err("Signal atlas exceeds the brief/window limit".into());
    }
    for brief in &atlas.briefs {
        if !projects.contains(&brief.project_id) {
            return Err("Signal brief references an unknown project".into());
        }
        for text in [
            &brief.project_label,
            &brief.title,
            &brief.conclusion,
            &brief.boundary,
            &brief.validation,
            &brief.confidence,
        ] {
            if !bounded(text, 4000) {
                return Err("Incomplete signal decision brief".into());
            }
        }
        if brief.signals.is_empty()
            || brief.signals.len() > 12
            || brief.signals.iter().any(|name| {
                !atlas
                    .panels
                    .iter()
                    .filter(|p| p.project_id == brief.project_id)
                    .any(|p| p.points.iter().any(|s| &s.name == name))
            })
        {
            return Err(
                "Every brief must reference 1..=12 exact contributors in its own project".into(),
            );
        }
        check_refs(&brief.evidence_refs)?;
    }
    for moment in &atlas.moments {
        if !projects.contains(&moment.project_id) {
            return Err("Signal window references an unknown project".into());
        }
        for text in [
            &moment.project_label,
            &moment.kind,
            &moment.window,
            &moment.title,
            &moment.measure,
            &moment.context,
        ] {
            if !bounded(text, 4000) {
                return Err("Incomplete signal window".into());
            }
        }
        check_refs(&moment.evidence_refs)?;
    }
    Ok(())
}

pub(crate) fn number(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else if value.abs() < 0.01 || value.abs() >= 1_000_000.0 {
        format!("{value:.2e}")
    } else {
        format!("{value:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn measure(value: Option<f64>) -> String {
    value
        .map(|v| {
            format!(
                "<span class=\"signal-number\" title=\"{}\">{}</span>",
                v,
                number(v)
            )
        })
        .unwrap_or_else(|| "<span class=\"signal-unknown\">Not supplied</span>".into())
}

fn classification(class: &str) -> (&str, &str) {
    match class {
        "CONFIRMED_BOTTLENECK" => ("agreement", "Four-model selection"),
        "CONFIRMED_BOTTLENECK_EN_COLLINEAR" => ("agreement", "Ridge + Huber + Q95"),
        "STRONG_CONTRIBUTOR" | "STABLE_CONTRIBUTOR" => ("steady", "Recurring contributor"),
        "TAIL_RISK" | "TAIL_OUTLIER" => ("tail", "Tail-sensitive selection"),
        "OUTLIER_DRIVEN" => ("tail", "Outlier-sensitive selection"),
        "SPARSE_DOMINANT" => ("sparse", "Sparse-model selection"),
        "ROBUST_ONLY" => ("steady", "Robust-model selection"),
        _ => ("unknown", "Partial / unavailable selection"),
    }
}

fn point_id(panel: &SignalPanel, index: usize) -> String {
    format!("signal-{}-{}", panel.id, index + 1)
}

fn references(refs: &[String], linked: bool) -> String {
    if !linked {
        return refs
            .iter()
            .map(|id| format!("<code>{}</code>", esc(id)))
            .collect::<Vec<_>>()
            .join(" · ");
    }
    refs.iter()
        .map(|id| {
            format!(
                "<a href=\"#evidence-{}\">{}</a>",
                attr(&id.to_ascii_lowercase()),
                esc(id)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn featured(panel: &SignalPanel) -> BTreeSet<usize> {
    let mut result = BTreeSet::new();
    for peak in [false, true] {
        let mut ranked = panel
            .points
            .iter()
            .enumerate()
            .filter_map(|(i, p)| (if peak { p.peak } else { p.active }).map(|v| (i, v)))
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        result.extend(ranked.into_iter().take(3).map(|(i, _)| i));
    }
    result
}

/// Log(1+x) preserves zeros and ordering. Axes explicitly state the transform;
/// no jitter, inferred percentages, cross-fit sums or causal labels are added.
fn chart(panel: &SignalPanel, featured: &BTreeSet<usize>) -> String {
    let points = panel
        .points
        .iter()
        .enumerate()
        .filter_map(|(i, p)| Some((i, p, p.active?, p.peak?)))
        .collect::<Vec<_>>();
    if points.is_empty() {
        return "<p class=\"signal-empty\">No numeric active/peak pair was supplied for this fit. Coverage and actions remain below.</p>".into();
    }
    let max_x = points.iter().map(|p| p.2).fold(0.0, f64::max).max(0.01);
    let max_y = points.iter().map(|p| p.3).fold(0.0, f64::max).max(0.01);
    let tx = max_x.ln_1p();
    let ty = max_y.ln_1p();
    let mut out = format!("<svg class=\"signal-chart\" viewBox=\"0 0 720 370\" role=\"img\" aria-labelledby=\"chart-title-{} chart-desc-{}\"><title id=\"chart-title-{}\">{} — {}: active and peak model impact</title><desc id=\"chart-desc-{}\">Each bubble is a contributor in one fit. Both axes use log(1+x); labels show original values. Bubble area represents the count of selected models, not saved CPU. Numbered points link to the table and exact source data.</desc>", panel.id, panel.id, panel.id, esc(&panel.project_label), esc(&panel.family), panel.id);
    out.insert_str(0, "<div class=\"signal-plot\"><p class=\"signal-mobile-axis\" aria-hidden=\"true\">↑ Peak impact · P99 · log(1+x)</p>");
    out.push_str(
        "<rect x=\"65\" y=\"20\" width=\"625\" height=\"285\" rx=\"12\" class=\"signal-plot-bg\"/>",
    );
    for tick in 0..=4 {
        let f = tick as f64 / 4.0;
        let x = 65.0 + 625.0 * f;
        let y = 305.0 - 285.0 * f;
        out.push_str(&format!("<path d=\"M{x:.2} 20V305 M65 {y:.2}H690\" class=\"signal-gridline\"/><text x=\"{x:.2}\" y=\"327\" text-anchor=\"middle\" class=\"signal-tick\">{}</text><text x=\"55\" y=\"{:.2}\" text-anchor=\"end\" class=\"signal-tick\">{}</text>", number((tx*f).exp_m1()), y+4.0, number((ty*f).exp_m1())));
    }
    out.push_str("<text x=\"380\" y=\"358\" text-anchor=\"middle\" class=\"signal-axis\">Active impact · P90 · log(1+x)</text><text transform=\"translate(15,165) rotate(-90)\" text-anchor=\"middle\" class=\"signal-axis\">Peak impact · P99 · log(1+x)</text>");
    // Small bubbles first, important selections last. Positions retain exact values.
    for (index, point, active, peak) in points
        .iter()
        .filter(|p| !featured.contains(&p.0))
        .chain(points.iter().filter(|p| featured.contains(&p.0)))
    {
        let x = 65.0 + 625.0 * active.ln_1p() / tx;
        let y = 305.0 - 285.0 * peak.ln_1p() / ty;
        let votes = point.selected.iter().filter(|s| **s == Some(true)).count();
        let known = point.selected.iter().filter(|s| s.is_some()).count();
        let incomplete = known < 4 || votes == 0;
        let radius = if incomplete {
            8.0
        } else {
            6.5 * (votes as f64).sqrt()
        };
        let (tone, _) = classification(&point.classification);
        out.push_str(&format!("<a href=\"#{}\" class=\"signal-bubble tone-{} {}\" aria-label=\"{}; active {}; peak {}; {} known selections; {known} of 4 flags supplied. Open evidence.\"><title>{}\nActive: {}\nPeak: {}\n{} known selections; {known} of 4 flags supplied\n{}</title><circle cx=\"{x:.2}\" cy=\"{y:.2}\" r=\"{radius:.2}\"/>", point_id(panel,*index),tone,if incomplete{"selection-incomplete"}else{""},attr(&point.name),active,peak,votes,esc(&point.name),active,peak,votes,esc(&point.classification)));
        if featured.contains(index) {
            out.push_str(&format!("<text x=\"{x:.2}\" y=\"{:.2}\" text-anchor=\"middle\" class=\"signal-bubble-label\">{}</text>",y+4.0,index+1));
        }
        out.push_str("</a>");
    }
    out.push_str("</svg></div>");
    out
}

#[cfg(test)]
#[path = "report_signals_tests.rs"]
pub(crate) mod tests;

fn matrix(panel: &SignalPanel, indices: &[usize], linked: bool) -> String {
    let mut out = "<div class=\"signal-matrix-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Model selection and impact comparison\"><table class=\"signal-matrix\"><thead><tr><th scope=\"col\">Signal / source</th><th scope=\"col\">Active</th><th scope=\"col\">Peak</th><th scope=\"col\">Peak ÷ active</th><th scope=\"col\" title=\"Ridge\">R</th><th scope=\"col\" title=\"Elastic Net\">EN</th><th scope=\"col\" title=\"Huber\">H</th><th scope=\"col\" title=\"Quantile 95\">Q95</th></tr></thead><tbody>".to_string();
    for index in indices {
        let p = &panel.points[*index];
        let (tone, label) = classification(&p.classification);
        out.push_str(&format!("<tr><td><details id=\"{}\" class=\"signal-inspect\"><summary><span class=\"signal-index tone-{}\">{}</span><span>{}</span></summary><div class=\"signal-inspect-body\"><span class=\"signal-badge tone-{}\">{}</span><p>{}</p><p><strong>Next test</strong> {}</p>",point_id(panel,*index),tone,index+1,esc(&p.name),tone,label,esc(&p.interpretation),esc(&p.action)));
        if let Some(href) = &p.href {
            out.push_str(&format!(
                "<a class=\"signal-source\" href=\"{}\">Open source report ↗</a>",
                attr(href)
            ));
        }
        out.push_str(&format!("<p class=\"signal-provenance\">Exact model label: <code>{}</code><br>Source: {}</p></div></details></td><td>{}</td><td>{}</td><td>{}</td>",esc(&p.classification),references(&p.evidence_refs, linked),measure(p.active),measure(p.peak),match(p.active,p.peak){(Some(a),Some(b)) if a>0.0 && (b/a).is_finite()=>format!("{}×",number(b/a)),_=>"—".into()}));
        for (i, vote) in p.selected.iter().enumerate() {
            let model = ["Ridge", "Elastic Net", "Huber", "Quantile 95"][i];
            let (symbol, status, class) = match vote {
                Some(true) => ("●", "selected in source classification", "selected"),
                Some(false) => (
                    "–",
                    "not selected in source classification; not zero cost",
                    "absent",
                ),
                None => ("?", "selection not supplied", "unknown"),
            };
            out.push_str(&format!("<td class=\"signal-vote {class}\" title=\"{model}: {status}\" aria-label=\"{model}: {status}\">{symbol}</td>"));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table></div>");
    out
}

fn reading(panel: &SignalPanel, peak: bool) -> String {
    let strongest = panel
        .points
        .iter()
        .enumerate()
        .filter_map(|(i, p)| (if peak { p.peak } else { p.active }).map(|v| (i, p, v)))
        .max_by(|a, b| a.2.total_cmp(&b.2));
    let Some((index, point, value)) = strongest else {
        return String::new();
    };
    format!("<a class=\"signal-reading {}\" href=\"#{}\"><span>{}</span><strong>{}</strong><b>{}</b><small>Largest supplied {} impact in this fit</small></a>",if peak{"peak"}else{"active"},point_id(panel,index),if peak{"PEAK SIGNAL"}else{"ACTIVE LOAD"},esc(&point.name),number(value),if peak{"peak"}else{"active"})
}

pub(crate) fn render(atlas: &SignalAtlas, linked: bool) -> String {
    if atlas.panels.is_empty() {
        return String::new();
    }
    let projects = atlas
        .panels
        .iter()
        .map(|p| &p.project_id)
        .collect::<BTreeSet<_>>();
    let total: usize = atlas.panels.iter().map(|p| p.points.len()).sum();
    let mut out=format!("<section class=\"signal-atlas\" id=\"signal-atlas\" aria-label=\"Gradient and cross-signal analysis\"><header class=\"signal-hero\"><div><span class=\"signal-eyebrow\">GRADIENTS × ANOMALIES × RUNTIME EVIDENCE</span><h3>Find the work. Isolate the peaks.</h3><p>Read the model pattern, compare the evidence, then choose the test.</p></div><div class=\"signal-counts\"><span><b>{}</b> scoped instances</span><span><b>{}</b> separate fits</span><span><b>{}</b> supplied signals</span></div></header>\n",projects.len(),atlas.panels.len(),total);
    if !atlas.briefs.is_empty() {
        let shared = atlas.briefs.len() > 1
            && atlas.briefs.iter().all(|b| {
                b.conclusion == atlas.briefs[0].conclusion
                    && b.validation == atlas.briefs[0].validation
            });
        if shared {
            let brief = &atlas.briefs[0];
            out.push_str(&format!("<article class=\"signal-shared\"><div><span class=\"signal-eyebrow\">SHARED WORKING HYPOTHESIS</span><p>{}</p></div><div class=\"signal-test\"><b>TEST THAT CHANGES THE DECISION</b><p>{}</p></div></article>",esc(&brief.conclusion),esc(&brief.validation)));
        }
        out.push_str("<div class=\"signal-brief-grid\">");
        for brief in &atlas.briefs {
            out.push_str(&format!("<article class=\"signal-brief\"><span class=\"signal-eyebrow\">{}</span><h4>{}</h4>",esc(&brief.project_label),esc(&brief.title)));
            if !shared {
                out.push_str(&format!(
                    "<p class=\"signal-thesis\">{}</p>",
                    esc(&brief.conclusion)
                ));
            }
            out.push_str("<div class=\"signal-chips\">");
            for name in &brief.signals {
                out.push_str(&format!("<div class=\"signal-chip\"><b>{}</b>", esc(name)));
                for panel in atlas
                    .panels
                    .iter()
                    .filter(|p| p.project_id == brief.project_id)
                {
                    if let Some(index) = panel.points.iter().position(|p| &p.name == name) {
                        let point = &panel.points[index];
                        out.push_str(&format!(
                            "<a href=\"#{}\" title=\"{} → {}\"><small>{} → {}</small><span>Active {} <i>→</i> Peak {}</span></a>",
                            point_id(panel, index),
                            attr(&panel.family),
                            attr(&panel.target),
                            esc(&panel.family),esc(&panel.target),measure(point.active),measure(point.peak)
                        ));
                    }
                }
                out.push_str("</div>");
            }
            out.push_str("</div>");
            if !shared {
                out.push_str(&format!("<div class=\"signal-test\"><b>TEST THAT CHANGES THE DECISION</b><p>{}</p></div>",esc(&brief.validation)));
            }
            out.push_str(&format!("<details class=\"signal-caveat\"><summary>Confidence: {} · limits and counterevidence</summary><p>{}</p><p>Supporting evidence: {}</p></details></article>",esc(&brief.confidence),esc(&brief.boundary),references(&brief.evidence_refs, linked)));
        }
        out.push_str("</div>");
    }
    out.push_str("<div class=\"signal-explorer\"><div class=\"signal-explorer-heading\"><div><span class=\"signal-eyebrow\">EXPLORE ONE FIT AT A TIME</span><h4>Active load versus peak impact</h4></div><div class=\"signal-controls\" hidden><label>Instance<select data-signal-project>");
    for id in &projects {
        let panel = atlas.panels.iter().find(|p| &p.project_id == *id).unwrap();
        out.push_str(&format!(
            "<option value=\"{}\">{}</option>",
            attr(id),
            esc(&panel.project_label)
        ));
    }
    out.push_str("</select></label><label>Signal family → target<select data-signal-fit></select></label></div></div><p class=\"signal-chart-note\">Combined model-impact indices; compare contributors within the selected fit. These are neither DB Time percentages nor recoverable CPU. Filled bubble area shows selected-model count; hollow rings indicate incomplete or no selection. Colour shows the selection pattern.</p>");
    for (n, panel) in atlas.panels.iter().enumerate() {
        let featured = featured(panel);
        out.push_str(&format!("<details class=\"signal-panel\" data-signal-panel=\"{}\" data-project=\"{}\" data-label=\"{} → {}\" {}><summary>{} · {} → {}</summary><p class=\"signal-scope\">{} · {}</p><div class=\"signal-readings\">{}{}</div>",attr(&panel.id),attr(&panel.project_id),attr(&panel.family),attr(&panel.target),if n==0{"open"}else{""},esc(&panel.project_label),esc(&panel.family),esc(&panel.target),esc(&panel.project_label),esc(&panel.window),reading(panel,false),reading(panel,true)));
        out.push_str(&chart(panel, &featured));
        out.push_str("<div class=\"signal-legend\"><span class=\"tone-agreement\">● Model agreement</span><span class=\"tone-steady\">● Recurring</span><span class=\"tone-tail\">● Tail / outlier</span><span class=\"tone-sparse\">● Sparse</span><span>R · Ridge &nbsp; EN · Elastic Net &nbsp; H · Huber &nbsp; Q95 · Quantile 95</span></div>");
        let front = featured.iter().copied().collect::<Vec<_>>();
        out.push_str(&matrix(panel, &front, linked));
        let rest = (0..panel.points.len())
            .filter(|i| !featured.contains(i))
            .collect::<Vec<_>>();
        if !rest.is_empty() {
            out.push_str(&format!("<details class=\"signal-roster\"><summary>Explore all {} supplied contributors · {} more</summary>{}</details>",panel.points.len(),rest.len(),matrix(panel,&rest, linked)));
        }
        out.push_str(&format!("<p class=\"signal-coverage\">{} Numbered shortlist: union of the three largest active and three largest peak impacts. All supplied numeric pairs remain on the plot. Missing values are never plotted as zero.</p></details>",esc(&panel.coverage)));
    }
    out.push_str("</div>");
    if !atlas.moments.is_empty() {
        out.push_str("<div class=\"signal-moments-heading\"><span class=\"signal-eyebrow\">WHEN TO LOOK</span><h4>Observed windows to investigate</h4><p>Selected from the supplied evidence subset. Co-occurrence is a capture target, not proof of one common cause.</p></div><div class=\"signal-moments\">");
        for moment in &atlas.moments {
            out.push_str(&format!("<article class=\"signal-moment\"><span class=\"signal-eyebrow\">{} · {}</span><time>{}</time><h5>{}</h5><strong class=\"signal-moment-value\">{}</strong><p>{}</p><div class=\"signal-provenance\">{}</div></article>",esc(&moment.project_label),esc(&moment.kind),esc(&moment.window),esc(&moment.title),esc(&moment.measure),esc(&moment.context),references(&moment.evidence_refs, linked)));
        }
        out.push_str("</div>");
    }
    out.push_str("<details class=\"signal-method\"><summary>How to read this analysis</summary><p><b>Active / peak:</b> source combined P90 / P99 impact across model outputs. Different target and predictor families are separate fits. Do not add their scores or treat a high score as saved seconds.</p><p><b>Model selection:</b> a filled marker reproduces source top-selection membership. A dash is not a zero coefficient or proof of collinearity. A question mark means no selection evidence was supplied. Models share the same data; agreement is not independent causal proof.</p><p><b>Precision and scope:</b> displayed values are rounded; hover a number for the original value. Peak ÷ active is undefined at zero active impact and can be unstable near zero. Raw tables, exact values, method settings and provenance remain in the complete technical evidence below.</p></details></section>\n\n");
    out
}
