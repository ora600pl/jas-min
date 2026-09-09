//! Shared decision contract for MCP authoring and Markdown returned by API models.
//! Grouping is explicit; neither SQL IDs nor prose similarity establish identity.
use html_escape::encode_text;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActionKind {
    EvidenceCapture,
    Mitigation,
    DurableFix,
    #[default]
    Unclassified,
}

impl ActionKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::EvidenceCapture => "Evidence capture",
            Self::Mitigation => "Mitigation",
            Self::DurableFix => "Durable fix",
            Self::Unclassified => "Unclassified (legacy)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReportIssue {
    pub issue_id: String,
    pub title: String,
    pub decision_summary: String,
    pub decision_boundary: String,
    pub scope: String,
    pub grouping_rationale: String,
    pub confidence: String,
    pub canonical_finding_id: String,
    pub finding_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueAction {
    pub finding_id: String,
    pub kind: ActionKind,
    pub owner: String,
    pub priority: String,
    pub action: String,
    pub rationale: String,
    pub success_criterion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueManifest {
    pub version: u32,
    pub issues: Vec<ReportIssue>,
    pub actions: Vec<IssueAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_atlas: Option<crate::report_signals::SignalAtlas>,
}

pub(crate) fn finding_anchor(id: &str) -> String {
    format!("finding-{}", id.to_ascii_lowercase())
}
pub(crate) fn issue_anchor(id: &str) -> String {
    format!("issue-{}", id.to_ascii_lowercase())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn bounded(name: &str, text: &str, maximum: usize) -> Result<(), String> {
    if text.trim().is_empty() || text.chars().count() > maximum {
        return Err(format!(
            "{name} must contain 1..={maximum} characters; revise it without truncating facts"
        ));
    }
    Ok(())
}

/// Returns normalized finding anchor -> issue ID. Partial grouping is useful
/// during authoring; completeness is checked separately before finalization.
pub(crate) fn validate_issues(
    issues: &[ReportIssue],
    known_anchors: &BTreeSet<String>,
) -> Result<BTreeMap<String, String>, String> {
    if issues.len() > 128 {
        return Err("At most 128 issues are supported".into());
    }
    let mut ids = BTreeSet::new();
    let mut membership = BTreeMap::new();
    for issue in issues {
        if !valid_id(&issue.issue_id) || !ids.insert(issue_anchor(&issue.issue_id)) {
            return Err(format!("Invalid or duplicate issue_id: {}", issue.issue_id));
        }
        for (name, value, max) in [
            ("title", &issue.title, 160),
            ("decision_summary", &issue.decision_summary, 800),
            ("decision_boundary", &issue.decision_boundary, 800),
            ("scope", &issue.scope, 800),
            ("grouping_rationale", &issue.grouping_rationale, 1600),
        ] {
            bounded(name, value, max)?;
        }
        if !["high", "medium", "low", "unknown"].contains(&issue.confidence.as_str()) {
            return Err("Issue confidence must be high, medium, low or unknown".into());
        }
        if issue.finding_ids.is_empty() || issue.finding_ids.len() > 64 {
            return Err("An issue must reference 1..=64 existing findings".into());
        }
        if !issue
            .finding_ids
            .iter()
            .any(|id| finding_anchor(id) == finding_anchor(&issue.canonical_finding_id))
        {
            return Err("canonical_finding_id must belong to finding_ids".into());
        }
        for id in &issue.finding_ids {
            let anchor = finding_anchor(id);
            if !valid_id(id) || !known_anchors.contains(&anchor) {
                return Err(format!("Unknown finding reference: {id}"));
            }
            if membership.insert(anchor, issue.issue_id.clone()).is_some() {
                return Err(format!("Finding {id} is assigned more than once; replace or split the existing issue explicitly"));
            }
        }
    }
    Ok(membership)
}

pub(crate) fn priority_rank(priority: &str) -> usize {
    match priority {
        "immediate" => 0,
        "high" => 1,
        "medium" => 2,
        "low" => 3,
        _ => 4,
    }
}

fn issue_actions<'a>(issue: &ReportIssue, actions: &'a [IssueAction]) -> Vec<&'a IssueAction> {
    let mut selected = actions
        .iter()
        .filter(|action| {
            issue
                .finding_ids
                .iter()
                .any(|id| finding_anchor(id) == finding_anchor(&action.finding_id))
        })
        .collect::<Vec<_>>();
    selected.sort_by_key(|action| {
        (
            priority_rank(&action.priority),
            action.kind,
            action.finding_id.as_str(),
        )
    });
    selected
}

pub(crate) fn render_issue_queue(issues: &[ReportIssue], actions: &[IssueAction]) -> String {
    let mut ranked = issues.iter().collect::<Vec<_>>();
    ranked.sort_by_key(|issue| {
        (
            issue_actions(issue, actions)
                .first()
                .map(|a| priority_rank(&a.priority))
                .unwrap_or(4),
            issue.issue_id.as_str(),
        )
    });
    let mut output = format!("{} issues · [Complete action register](#action-register). Priorities describe actions; confidence describes the issue synthesis.\n\n", ranked.len());
    for (index, issue) in ranked.into_iter().take(5).enumerate() {
        output.push_str(&format!(
            "<section class=\"decision-card\" id=\"{}\">\n\n### {}. {}\n\n{}\n\n",
            issue_anchor(&issue.issue_id),
            index + 1,
            encode_text(&issue.title),
            encode_text(&issue.decision_summary)
        ));
        if let Some(action) = issue_actions(issue, actions).first() {
            output.push_str(&format!(
                "**Next action — {} / {} / {}:** {}\n\n",
                encode_text(&action.owner),
                action.priority.to_ascii_uppercase(),
                action.kind.label(),
                encode_text(&action.action)
            ));
        } else {
            output.push_str("**Next action:** No action recorded; review the decision boundary before intervening.\n\n");
        }
        output.push_str(&format!("<p class=\"decision-boundary\"><strong>Decision boundary:</strong> {}</p>\n\n<p class=\"decision-meta\">Scope: {} · Confidence: {} · <a href=\"#{}\">Canonical finding</a> · {} supporting finding(s)</p>\n\n</section>\n\n",
            encode_text(&issue.decision_boundary), encode_text(&issue.scope), encode_text(&issue.confidence), finding_anchor(&issue.canonical_finding_id), issue.finding_ids.len()));
    }
    output
}

/// The complete registry also supplies targets for issues outside the first five.
pub(crate) fn render_issue_register(issues: &[ReportIssue]) -> String {
    let mut output = String::from("<details class=\"issue-register\"><summary>Issue register — scope and supporting findings</summary>\n\n");
    for issue in issues {
        output.push_str(&format!("<a id=\"issue-detail-{}\"></a>\n\n#### {} — {}\n\n{}\n\n**Scope:** {}\n\n**Decision boundary:** {}\n\n**Why grouped:** {}\n\n**Supporting findings:** {}\n\n",
            issue.issue_id.to_ascii_lowercase(), encode_text(&issue.issue_id), encode_text(&issue.title), encode_text(&issue.decision_summary), encode_text(&issue.scope), encode_text(&issue.decision_boundary), encode_text(&issue.grouping_rationale),
            issue.finding_ids.iter().map(|id| format!("[{}](#{}){}", id, finding_anchor(id), if id.eq_ignore_ascii_case(&issue.canonical_finding_id) { " (canonical)" } else { "" })).collect::<Vec<_>>().join(", ")));
    }
    output.push_str("</details>\n\n");
    output
}

pub(crate) fn issue_schema() -> Value {
    json!({"type":"object", "additionalProperties":false, "properties": {
        "issue_id":{"type":"string", "pattern":"^[A-Za-z0-9_-]{1,64}$", "description":"Stable explicit identity, e.g. I-0001; reuse it to replace the grouping."},
        "title":{"type":"string", "minLength":1,"maxLength":160},
        "decision_summary":{"type":"string","minLength":1,"maxLength":800,"description":"Short decision brief with decisive impact; aim for 40-60 words. Do not copy the full finding."},
        "decision_boundary":{"type":"string","minLength":1,"maxLength":800,"description":"The limitation that could change the decision; keep historical, live and lab evidence distinct."},
        "scope":{"type":"string","minLength":1,"maxLength":800,"description":"Explicit workload, instances and time windows; explain separately collected periods."},
        "grouping_rationale":{"type":"string","minLength":1,"maxLength":1600,"description":"Why these perspectives belong to one investigation. A shared SQL_ID alone is insufficient."},
        "confidence":{"type":"string","enum":["high","medium","low","unknown"]},
        "canonical_finding_id":{"type":"string"},
        "finding_ids":{"type":"array","minItems":1,"maxItems":64,"uniqueItems":true,"items":{"type":"string"}}
    },"required":["issue_id","title","decision_summary","decision_boundary","scope","grouping_rationale","confidence","canonical_finding_id","finding_ids"]})
}

pub(crate) fn render_action_register(actions: &[IssueAction]) -> String {
    let mut output = String::new();
    let mut ranked = actions.iter().collect::<Vec<_>>();
    ranked.sort_by_key(|a| {
        (
            priority_rank(&a.priority),
            a.owner.as_str(),
            a.kind,
            a.action.as_str(),
        )
    });
    let mut seen = BTreeSet::new();
    let mut priority = "";
    for action in &ranked {
        let key = (
            &action.owner,
            &action.priority,
            action.kind,
            &action.action,
            &action.rationale,
            &action.success_criterion,
        );
        if !seen.insert(key) {
            continue;
        }
        if priority != action.priority {
            priority = &action.priority;
            output.push_str(&format!(
                "### {} priority\n\n",
                priority.to_ascii_uppercase()
            ));
        }
        let supports = ranked
            .iter()
            .filter(|other| {
                (
                    &other.owner,
                    &other.priority,
                    other.kind,
                    &other.action,
                    &other.rationale,
                    &other.success_criterion,
                ) == key
            })
            .map(|other| {
                format!(
                    "[{}](#{})",
                    other.finding_id,
                    finding_anchor(&other.finding_id)
                )
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!("- **{} / {} — {}**  \n  **Why:** {}  \n  **Success criterion:** {}  \n  *Supports: {}*\n\n", encode_text(&action.owner), action.kind.label(), encode_text(&action.action), encode_text(&action.rationale), encode_text(&action.success_criterion), supports));
    }
    output
}

/// Parse only a real fenced code block. A mention inside prose/code is not metadata.
pub(crate) fn prepare_api_report(
    markdown: &str,
) -> Result<(String, Option<IssueManifest>), String> {
    let options = Options::ENABLE_HEADING_ATTRIBUTES | Options::ENABLE_TABLES;
    let mut manifest_range = None;
    let mut in_manifest = false;
    let mut manifest_start = 0;
    let mut payload = String::new();
    let mut heading_start = 0;
    let mut heading_text = String::new();
    let mut in_h2 = false;
    let mut sections = Vec::new();
    let mut known = BTreeSet::new();
    let mut finding_positions = Vec::new();
    let mut duplicate_anchor = None;
    let start_marker = "<!-- jasmin-actions:start -->";
    let end_marker = "<!-- jasmin-actions:end -->";
    let mut action_starts = Vec::new();
    let mut action_ends = Vec::new();
    let mut signal_starts = Vec::new();
    let mut signal_ends = Vec::new();
    let mut source_ids = std::collections::BTreeMap::<String, Vec<usize>>::new();
    let signal_start_marker = "<!-- jasmin-signals:start -->";
    let signal_end_marker = "<!-- jasmin-signals:end -->";
    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                if info.trim() == "jasmin-issues" =>
            {
                if manifest_range.is_some() {
                    return Err("Only one jasmin-issues block is allowed".into());
                }
                in_manifest = true;
                manifest_start = range.start;
            }
            Event::Text(text) if in_manifest => payload.push_str(&text),
            Event::End(TagEnd::CodeBlock) if in_manifest => {
                in_manifest = false;
                manifest_range = Some(manifest_start..range.end);
            }
            Event::Start(Tag::Heading {
                level: HeadingLevel::H2,
                ..
            }) => {
                in_h2 = true;
                heading_start = range.start;
                heading_text.clear();
            }
            Event::Text(text) | Event::Code(text) if in_h2 => heading_text.push_str(&text),
            Event::End(TagEnd::Heading(HeadingLevel::H2)) => {
                in_h2 = false;
                sections.push((heading_text.clone(), heading_start, range.end));
            }
            Event::Start(Tag::Heading {
                level: HeadingLevel::H3,
                id: Some(id),
                ..
            }) if id.starts_with("finding-") => {
                if !known.insert(id.to_string()) {
                    duplicate_anchor = Some(id.to_string());
                }
                finding_positions.push((id.to_string(), range.start));
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                // Ignore literal marker examples in Markdown code blocks.
                action_starts.extend(
                    html.match_indices(start_marker)
                        .map(|(offset, _)| range.start + offset),
                );
                action_ends.extend(
                    html.match_indices(end_marker)
                        .map(|(offset, _)| range.start + offset),
                );
                signal_starts.extend(
                    html.match_indices(signal_start_marker)
                        .map(|(offset, _)| range.start + offset),
                );
                signal_ends.extend(
                    html.match_indices(signal_end_marker)
                        .map(|(offset, _)| range.start + offset),
                );
                for node in scraper::Html::parse_fragment(&html)
                    .select(&scraper::Selector::parse("[id]").unwrap())
                {
                    if let Some(id) = node.value().attr("id") {
                        source_ids
                            .entry(id.to_string())
                            .or_default()
                            .push(range.start);
                    }
                }
            }
            _ => {}
        }
    }
    let Some(manifest_range) = manifest_range else {
        return Ok((markdown.to_string(), None));
    };
    if let Some(id) = duplicate_anchor {
        return Err(format!("Duplicate finding anchor: {id}"));
    }
    if payload.len() > 256 * 1024 {
        return Err("Issue manifest exceeds 256 KiB".into());
    }
    let manifest: IssueManifest =
        serde_json::from_str(&payload).map_err(|e| format!("Invalid jasmin-issues JSON: {e}"))?;
    if manifest.version != 1 || manifest.issues.is_empty() {
        return Err("Expected issue manifest version 1 with at least one issue".into());
    }
    let membership = validate_issues(&manifest.issues, &known)?;
    for anchor in &known {
        if !membership.contains_key(anchor) {
            return Err(format!("Finding without an issue: {anchor}"));
        }
    }
    if manifest.actions.is_empty() || manifest.actions.len() > 1024 {
        return Err("Expected 1..=1024 issue actions".into());
    }
    for action in &manifest.actions {
        if !membership.contains_key(&finding_anchor(&action.finding_id)) {
            return Err(format!(
                "Action references an unassigned finding: {}",
                action.finding_id
            ));
        }
        if action.kind == ActionKind::Unclassified {
            return Err("New issue actions need an explicit kind".into());
        }
        if !["DBA", "Developer", "Management"].contains(&action.owner.as_str())
            || priority_rank(&action.priority) > 3
        {
            return Err("Invalid action owner or priority".into());
        }
        for (name, text) in [
            ("action", &action.action),
            ("rationale", &action.rationale),
            ("success_criterion", &action.success_criterion),
        ] {
            bounded(name, text, 2000)?;
        }
    }
    let section = |number: usize| -> Result<(usize, usize), String> {
        let found = sections
            .iter()
            .enumerate()
            .filter(|(_, (title, _, _))| title.starts_with(&format!("{number}. ")))
            .collect::<Vec<_>>();
        if found.len() != 1 {
            return Err(format!("Expected one numbered H2 section {number}"));
        }
        let (index, (_, _, end)) = found[0];
        Ok((
            *end,
            sections
                .get(index + 1)
                .map(|(_, start, _)| *start)
                .unwrap_or(markdown.len()),
        ))
    };
    let mut previous_end = 0;
    for number in 1..=11 {
        let bounds = section(number)?;
        if bounds.0 < previous_end {
            return Err("The eleven sections must retain their numbered order".into());
        }
        previous_end = bounds.1;
    }
    let summary = section(1)?;
    let actions_section = section(11)?;
    if summary.1 > actions_section.0 || manifest_range.start < actions_section.0 {
        return Err("Place issue metadata after the report's numbered sections".into());
    }
    if action_starts.len() != 1 || action_ends.len() != 1 {
        return Err(
            "Section 11 needs exactly one jasmin-actions:start/end marker pair outside code blocks"
                .into(),
        );
    }
    let action_start = action_starts[0];
    let action_end = action_ends[0];
    if action_start < actions_section.0
        || action_end < action_start
        || action_end >= actions_section.1
        || action_end >= manifest_range.start
    {
        return Err("Action markers must be ordered inside section 11 before metadata".into());
    }
    for (id, position) in &finding_positions {
        if (summary.0..summary.1).contains(position)
            || (action_start..action_end).contains(position)
        {
            return Err(format!("Finding anchor {id} would be removed by generated content; put findings outside section 1 and the action markers"));
        }
    }
    let mut replacements = vec![
        (
            summary.0..summary.1,
            format!(
                "\n\n{}{}",
                render_issue_queue(&manifest.issues, &manifest.actions),
                render_issue_register(&manifest.issues)
            ),
        ),
        (
            action_start + start_marker.len()..action_end,
            format!(
                "\n\n<a id=\"action-register\"></a>\n\n{}\n",
                render_action_register(&manifest.actions)
            ),
        ),
        (manifest_range, String::new()),
    ];
    if let Some(atlas) = &manifest.signal_atlas {
        crate::report_signals::validate(atlas)?;
        if signal_starts.len() != 1 || signal_ends.len() != 1 {
            return Err(
                "Signal atlas needs one jasmin-signals:start/end marker pair in section 9".into(),
            );
        }
        let signals_section = section(9)?;
        let start = signal_starts[0];
        let end = signal_ends[0];
        if start < signals_section.0 || end < start || end >= signals_section.1 {
            return Err("Signal atlas markers must be ordered inside section 9".into());
        }
        if finding_positions
            .iter()
            .any(|(_, position)| (start..end).contains(position))
        {
            return Err("Put technical findings outside the generated signal atlas markers".into());
        }
        let refs = atlas
            .panels
            .iter()
            .flat_map(|p| &p.points)
            .flat_map(|p| &p.evidence_refs)
            .chain(atlas.briefs.iter().flat_map(|b| &b.evidence_refs))
            .chain(atlas.moments.iter().flat_map(|m| &m.evidence_refs));
        for id in refs {
            if !source_ids
                .get(&format!("evidence-{}", id.to_ascii_lowercase()))
                .is_some_and(|positions| {
                    positions.iter().any(|p| {
                        !(start..end).contains(p)
                            && !replacements.iter().any(|(range, _)| range.contains(p))
                    })
                })
            {
                return Err(format!(
                    "Signal atlas requires a real provenance anchor for {id}"
                ));
            }
        }
        replacements.push((
            start + signal_start_marker.len()..end,
            format!("\n\n{}\n", crate::report_signals::render(atlas, true)),
        ));
    }
    replacements.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut rendered = markdown.to_string();
    for (range, replacement) in replacements {
        rendered.replace_range(range, &replacement);
    }
    Ok((rendered, Some(manifest)))
}

/// Keep validated metadata with the normalized Markdown for reproducible re-export.
pub(crate) fn finalize_api_markdown(markdown: &str) -> Result<String, String> {
    let (mut rendered, manifest) = prepare_api_report(markdown)?;
    let manifest = manifest.ok_or("New AI reports require one validated jasmin-issues block; raw response retained for correction")?;
    {
        rendered.push_str(&format!(
            "\n\n```jasmin-issues\n{}\n```\n",
            serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?
        ));
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> IssueManifest {
        IssueManifest {
            version: 1,
            signal_atlas: None,
            issues: vec![ReportIssue {
                issue_id: "I-1".into(),
                title: "Import cursor incident".into(),
                decision_summary: "Two waiters make holder capture the next step.".into(),
                decision_boundary: "The later sample cannot explain every historical wait.".into(),
                scope: "Instance 1, July history and separate September live sample.".into(),
                grouping_rationale:
                    "Wait and cursor views support one investigation without merging periods."
                        .into(),
                confidence: "medium".into(),
                canonical_finding_id: "F-1".into(),
                finding_ids: vec!["F-1".into(), "F-2".into()],
            }],
            actions: vec![IssueAction {
                finding_id: "F-1".into(),
                kind: ActionKind::EvidenceCapture,
                owner: "DBA".into(),
                priority: "immediate".into(),
                action: "Capture the complete holder timeline.".into(),
                rationale: "The initiating transition remains unknown.".into(),
                success_criterion: "Aligned onset and recovery counters identify the same cursor."
                    .into(),
            }],
        }
    }

    fn report(manifest: &IssueManifest) -> String {
        let mut text = String::from("# Report\n\n");
        for number in 1..=11 {
            text.push_str(&format!("## {number}. Section\n\n"));
            match number {
                1 => text.push_str("Generator inserts the queue.\n\n"),
                2 => text.push_str("### Wait evidence {#finding-f-1}\n\nInstance 1: 2 waiters.\n\n"),
                4 => text.push_str("### Cursor evidence {#finding-f-2}\n\nSeparate September sample; cause still unknown.\n\n"),
                11 => text.push_str("<!-- jasmin-actions:start -->\n<!-- jasmin-actions:end -->\n\n### Mandatory assessments\n\nCPU capacity remains unknown.\n\n"),
                _ => text.push_str("Complete category evidence.\n\n"),
            }
        }
        text.push_str(&format!(
            "```jasmin-issues\n{}\n```\n",
            serde_json::to_string(manifest).unwrap()
        ));
        text
    }

    #[test]
    fn api_and_mcp_share_queue_rendering_and_reexport_preserves_metadata() {
        let manifest = manifest();
        let raw = report(&manifest);
        let (prepared, parsed) = prepare_api_report(&raw).unwrap();
        assert!(prepared.contains(&render_issue_queue(&manifest.issues, &manifest.actions)));
        assert_eq!(prepared.matches("class=\"decision-card\"").count(), 1);
        assert!(prepared.contains("Instance 1: 2 waiters."));
        assert!(prepared.contains("CPU capacity remains unknown."));
        assert!(!prepared.contains("```jasmin-issues"));
        assert!(parsed.is_some());
        let finalized = finalize_api_markdown(&raw).unwrap();
        let (reexported, _) = prepare_api_report(&finalized).unwrap();
        assert_eq!(prepared.trim(), reexported.trim());
        let html = crate::tools::try_render_markdown_html_document(
            &finalized,
            "",
            "",
            std::collections::HashMap::new(),
        )
        .unwrap();
        assert!(html.contains("id=\"finding-f-1\""));
        assert!(!html.contains("language-jasmin-issues"));
    }

    #[test]
    fn invalid_membership_or_action_types_cannot_publish_an_api_report() {
        let original = manifest();
        let mut invalid = original.clone();
        invalid.issues[0].finding_ids.push("F-missing".into());
        assert!(prepare_api_report(&report(&invalid))
            .unwrap_err()
            .contains("Unknown finding"));
        let mut invalid = original.clone();
        invalid.issues.push(invalid.issues[0].clone());
        invalid.issues[1].issue_id = "I-2".into();
        assert!(prepare_api_report(&report(&invalid))
            .unwrap_err()
            .contains("assigned more than once"));
        let mut invalid = original.clone();
        invalid.issues[0].canonical_finding_id = "F-9".into();
        assert!(prepare_api_report(&report(&invalid))
            .unwrap_err()
            .contains("canonical_finding_id"));
        let mut invalid = original.clone();
        invalid.actions[0].kind = ActionKind::Unclassified;
        assert!(prepare_api_report(&report(&invalid))
            .unwrap_err()
            .contains("explicit kind"));
        let mut invalid = original.clone();
        invalid.actions[0].finding_id = "F-99".into();
        assert!(prepare_api_report(&report(&invalid))
            .unwrap_err()
            .contains("unassigned finding"));
        let mut invalid = original.clone();
        invalid.issues[0].finding_ids.pop();
        assert!(prepare_api_report(&report(&invalid))
            .unwrap_err()
            .contains("without an issue"));
        assert!(prepare_api_report(
            &report(&original).replace("## 5. Section", "## Missing section")
        )
        .is_err());
        assert!(
            prepare_api_report(&report(&original).replace("<!-- jasmin-actions:end -->", ""))
                .is_err()
        );
    }

    #[test]
    fn api_replacement_ignores_code_examples_and_never_removes_findings() {
        let raw = report(&manifest());
        let example =
            "```html\n<!-- jasmin-actions:start -->\n<!-- jasmin-actions:end -->\n```\n\n";
        let with_example = raw.replacen("## 2. Section", &format!("{example}## 2. Section"), 1);
        assert!(prepare_api_report(&with_example).is_ok());
        let duplicate_marker = raw.replacen(
            "<!-- jasmin-actions:end -->",
            "<!-- jasmin-actions:end -->\n<!-- jasmin-actions:end -->",
            1,
        );
        assert!(prepare_api_report(&duplicate_marker).is_err());
        let summary_finding = raw.replace("## 2. Section\n\n", "").replacen(
            "## 3. Section",
            "## 2. Section\n\n## 3. Section",
            1,
        );
        assert!(prepare_api_report(&summary_finding)
            .unwrap_err()
            .contains("would be removed"));
        let action_finding = raw.replace("### Wait evidence {#finding-f-1}\n\nInstance 1: 2 waiters.\n\n", "").replacen("<!-- jasmin-actions:end -->", "### Wait evidence {#finding-f-1}\n\nInstance 1: 2 waiters.\n\n<!-- jasmin-actions:end -->", 1);
        assert!(prepare_api_report(&action_finding)
            .unwrap_err()
            .contains("would be removed"));
        let mut no_actions = manifest();
        no_actions.actions.clear();
        assert!(prepare_api_report(&report(&no_actions))
            .unwrap_err()
            .contains("issue actions"));
    }

    #[test]
    fn api_atlas_uses_shared_renderer_requires_sources_and_preserves_technical_text() {
        let mut manifest = manifest();
        manifest.signal_atlas = Some(crate::report_signals::tests::fixture());
        let raw = report(&manifest).replace("## 9. Section\n\n", "## 9. Section\n\n<!-- jasmin-signals:start -->\n<!-- jasmin-signals:end -->\n\n<a id=\"evidence-e-1\"></a>\n\nExact source measurements remain here.\n\n");
        let (prepared, _) = prepare_api_report(&raw).unwrap();
        assert!(prepared.contains("class=\"signal-atlas\""));
        assert!(prepared.contains("Exact source measurements remain here."));
        assert!(prepared.contains("Instance 1: 2 waiters."));
        let normalized = finalize_api_markdown(&raw).unwrap();
        assert_eq!(
            prepared.trim(),
            prepare_api_report(&normalized).unwrap().0.trim()
        );
        assert!(
            prepare_api_report(&raw.replace("<a id=\"evidence-e-1\"></a>", ""))
                .unwrap_err()
                .contains("provenance anchor")
        );
        assert!(prepare_api_report(&raw.replace("<!-- jasmin-signals:end -->", "")).is_err());
        let removed_source = raw.replace("<a id=\"evidence-e-1\"></a>", "").replace(
            "<!-- jasmin-signals:end -->",
            "<a id=\"evidence-e-1\"></a>\n<!-- jasmin-signals:end -->",
        );
        assert!(prepare_api_report(&removed_source).is_err());
        let removed_finding = raw.replace("### Wait evidence {#finding-f-1}\n\nInstance 1: 2 waiters.\n\n", "").replace("<!-- jasmin-signals:end -->", "### Wait evidence {#finding-f-1}\n\nInstance 1: 2 waiters.\n\n<!-- jasmin-signals:end -->");
        assert!(prepare_api_report(&removed_finding)
            .unwrap_err()
            .contains("technical findings"));
    }

    #[test]
    fn legacy_markdown_is_convertible_but_new_generation_requires_metadata() {
        let legacy = "# Old report\n\n## 1. Executive Summary\n\nExisting evidence.";
        assert_eq!(prepare_api_report(legacy).unwrap().0, legacy);
        assert!(finalize_api_markdown(legacy).is_err());
        assert!(prepare_api_report("```jasmin-issues\n{broken json}\n```\n").is_err());
        let json = serde_json::to_value(manifest()).unwrap();
        let mut unknown = json.clone();
        unknown["unexpected"] = json!(true);
        assert!(serde_json::from_value::<IssueManifest>(unknown).is_err());
    }

    #[test]
    fn action_kind_is_part_of_identity_and_priority_precedes_kind() {
        let mut actions = manifest().actions;
        let mut fix = actions[0].clone();
        fix.kind = ActionKind::DurableFix;
        fix.priority = "high".into();
        actions.push(fix);
        let mut mitigation = actions[0].clone();
        mitigation.kind = ActionKind::Mitigation;
        actions.push(mitigation);
        actions.push(actions[0].clone());
        let rendered = render_action_register(&actions);
        assert_eq!(
            rendered
                .matches("Capture the complete holder timeline")
                .count(),
            3
        );
        assert!(rendered.find("IMMEDIATE").unwrap() < rendered.find("HIGH").unwrap());
        assert!(rendered.contains("Mitigation"));
        assert!(rendered.contains("Durable fix"));
    }
}
