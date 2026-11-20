//! Resolve discussion evidence at finding scope. Full source records live in the checkpoint.
use super::*;
use pulldown_cmark::{Event, Parser, Tag};
use serde::Deserialize;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceCard {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub evidence_refs: Vec<String>,
}

pub(super) fn card_schema() -> Value {
    json!({"type":"array","maxItems":32,"description":"One entry per distinct measured comparison, supported hypothesis or verification. Keep its references specific to this claim.","items":{
        "type":"object","additionalProperties":false,
        "properties":{"id":{"type":"string","pattern":"^[A-Za-z0-9_-]{1,64}$"},
        "kind":{"type":"string","enum":["measurement","comparison","hypothesis","verification"]},
        "title":{"type":"string","minLength":1,"maxLength":180},"body":{"type":"string","minLength":1,"maxLength":8000},
        "evidence_refs":{"type":"array","minItems":1,"maxItems":32,"uniqueItems":true,"items":{"type":"string"}}},
        "required":["id","kind","title","body","evidence_refs"]}})
}

pub(super) fn validate_cards(brief: &audit_workflow::AuditBrief) -> std::result::Result<(), Value> {
    let mut ids = HashSet::new();
    let mut bodies = HashSet::new();
    if brief.evidence_cards.len() > 32 {
        return Err(tool_error(
            "INVALID_EVIDENCE_CARD",
            "At most 32 distinct cards per brief",
        ));
    }
    for card in &brief.evidence_cards {
        let refs: HashSet<_> = card.evidence_refs.iter().collect();
        if card.id.is_empty()
            || card.id.len() > 64
            || !card
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || !ids.insert(&card.id)
            || !matches!(
                card.kind.as_str(),
                "measurement" | "comparison" | "hypothesis" | "verification"
            )
            || card.title.trim().is_empty()
            || card.title.chars().count() > 180
            || card.body.trim().is_empty()
            || card.body.chars().count() > 8000
            || !bodies.insert(card.body.trim())
        {
            return Err(tool_error(
                "INVALID_EVIDENCE_CARD",
                format!(
                    "Check id, kind, title and unique body on card '{}'",
                    card.id
                ),
            ));
        }
        if refs.is_empty()
            || refs.len() > 32
            || refs.len() != card.evidence_refs.len()
            || refs.iter().any(|id| !brief.evidence_refs.contains(id))
        {
            return Err(tool_error(
                "EVIDENCE_CARD_SCOPE_MISMATCH",
                format!(
                    "Card '{}' must cite distinct sources retained in this discussion brief",
                    card.id
                ),
            ));
        }
        // Cards use Markdown for authoring. Machine-local absolute URLs cannot travel with the report.
        for event in Parser::new(&card.body) {
            match event {
                Event::InlineHtml(_) | Event::Html(_) => {
                    return Err(tool_error(
                        "INVALID_EVIDENCE_CARD",
                        format!("Use Markdown instead of HTML in card '{}'", card.id),
                    ))
                }
                Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. })
                    if dest_url.starts_with('/')
                        || !crate::report_signals::safe_href(&dest_url) =>
                {
                    return Err(tool_error(
                        "NONPORTABLE_REPORT_LINK",
                        format!("Use a relative report-package link in card '{}'", card.id),
                    ))
                }
                _ => (),
            }
        }
    }
    Ok(())
}

/// Strict at review/publication time. An older brief has no card IDs and still renders.
pub(super) fn selected_cards<'a>(
    reader: &ReaderBrief,
    source: &'a audit_workflow::AuditBrief,
) -> std::result::Result<Vec<&'a EvidenceCard>, Value> {
    if reader.supporting_card_ids.is_empty() && !source.evidence_cards.is_empty() {
        return Err(tool_error(
            "FINDING_CARDS_REQUIRED",
            "Select the source cards for this finding by ID",
        ));
    }
    let mut selected = Vec::new();
    let mut paragraphs = HashSet::new();
    for text in [
        &reader.observed,
        &reader.explanation,
        &reader.next_step,
        &reader.boundary,
    ] {
        if distinct_prose(text, &mut paragraphs) != text.trim_matches('\n')
            && !source.evidence_cards.is_empty()
        {
            return Err(tool_error(
                "DUPLICATED_FINDING_PARAGRAPH",
                "Each part of the reader summary must add a distinct observation or decision",
            ));
        }
    }
    let mut ids = HashSet::new();
    for id in &reader.supporting_card_ids {
        let card = source
            .evidence_cards
            .iter()
            .find(|c| &c.id == id)
            .filter(|c| {
                c.evidence_refs
                    .iter()
                    .all(|r| reader.evidence_refs.contains(r))
            });
        match card {
            Some(card) if ids.insert(id) => {
                if distinct_prose(&card.body,&mut paragraphs) != card.body.trim_matches('\n') {
                    return Err(tool_error("DUPLICATED_FINDING_PARAGRAPH",format!("Card '{id}' repeats a prose paragraph from the reader summary or another selected card")));
                }
                selected.push(card);
            },
            _ => return Err(tool_error("FINDING_CARD_SCOPE_MISMATCH",format!("Card '{id}' must belong to the cited discussion and use only the finding's decisive evidence"))),
        }
    }
    Ok(selected)
}

pub(super) fn render_cards(cards: &[&EvidenceCard], l: ReportLanguage, provenance: bool) -> String {
    let mut output = String::new();
    for card in cards {
        let kind = match card.kind.as_str() {
            "comparison" => l.choose("Comparison", "Porównanie"),
            "hypothesis" => l.choose("Hypothesis", "Hipoteza"),
            "verification" => l.choose("Verification", "Weryfikacja"),
            _ => l.choose("Measurement", "Pomiar"),
        };
        output.push_str(&format!("<section class=\"finding-proof\">\n<header><span>{kind}</span><h4>{}</h4></header>\n\n{}\n\n",encode_text(&card.title),card.body));
        if provenance {
            output.push_str(&format!(
                "**{}:** {}\n\n",
                l.text("Technical provenance"),
                evidence_links(&card.evidence_refs)
            ));
        }
        output.push_str("</section>\n\n");
    }
    output
}

// Only compare complete prose paragraphs. Code blocks, tables, anchors, links and
// list items retain their original spelling, indentation, order and multiplicity.
pub(super) fn distinct_prose(markdown: &str, seen: &mut HashSet<String>) -> String {
    use pulldown_cmark::{Options, TagEnd};
    let mut simple = None;
    let mut skip = Vec::new();
    let mut nested = 0usize;
    for (event, range) in Parser::new_ext(markdown, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::Paragraph) if nested == 0 => {
                simple = Some(range.clone());
                nested += 1;
            }
            Event::Text(_) => (),
            Event::End(TagEnd::Paragraph) if nested == 1 => {
                if let Some(r) = simple.take() {
                    let raw = &markdown[r.clone()];
                    // Keep intentional line breaks, e.g. delimited SQL or a source recipe.
                    let key = raw.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !raw.trim().contains('\n') && !seen.insert(key) {
                        skip.push(r);
                    }
                }
                nested -= 1;
            }
            Event::Start(_) => {
                nested += 1;
                simple = None;
            }
            Event::End(_) => {
                nested = nested.saturating_sub(1);
                simple = None;
            }
            _ => {
                simple = None;
            }
        }
    }
    let mut result = markdown.to_owned();
    for r in skip.into_iter().rev() {
        result.replace_range(r, "");
    }
    result.trim_matches('\n').to_string()
}

pub(super) fn technical_markdown(
    finding: &ReportFinding,
    include_context: bool,
    l: ReportLanguage,
    deep: bool,
) -> String {
    let mut seen = HashSet::new();
    if let Some(reader) = &finding.reader_brief {
        for text in [
            &reader.observed,
            &reader.explanation,
            &reader.next_step,
            &reader.boundary,
        ] {
            distinct_prose(text, &mut seen);
        }
    } else {
        for text in [&finding.conclusion, &finding.evidence_limitations] {
            distinct_prose(text, &mut seen);
        }
        if !include_context {
            for text in [
                &finding.evidence_summary,
                &finding.affected_workload,
                &finding.temporal_pattern,
            ] {
                distinct_prose(text, &mut seen);
            }
        }
    }
    let mut output = String::new();
    for (include, label, body) in [
        (
            include_context,
            "Temporal pattern",
            &finding.temporal_pattern,
        ),
        (
            include_context,
            "Affected workload",
            &finding.affected_workload,
        ),
        (include_context, "Evidence basis", &finding.evidence_summary),
        (true, "Diagnostic mechanism", &finding.mechanism),
        (
            finding.reader_brief.is_some(),
            "Original diagnostic conclusion",
            &finding.conclusion,
        ),
        (
            finding.reader_brief.is_some(),
            "Decision boundary",
            &finding.evidence_limitations,
        ),
    ] {
        if include {
            let value = distinct_prose(body, &mut seen);
            if !value.is_empty() {
                output.push_str(&format!(
                    "<div class=\"finding-proof\"><h4>{}</h4>\n\n{value}\n\n</div>\n\n",
                    l.text(label)
                ));
            }
        }
    }
    let details = distinct_prose(&finding.details, &mut seen);
    if !details.is_empty() {
        if deep {
            output.push_str(&details);
            output.push_str("\n\n");
        } else {
            output.push_str(&format!("<details class=\"finding-detail\"><summary>{}</summary>\n\n{details}\n\n</details>\n\n",l.text("Additional diagnostic context")));
        }
    }
    output
}

pub(super) fn card_coverage_warnings(state: &AnalysisSession) -> Vec<Value> {
    let Some(audit) = &state.audit else {
        return Vec::new();
    };
    let mut owners = BTreeMap::<(&str, &str), Vec<&str>>::new();
    for finding in state.findings.values() {
        if let Some(reader) = &finding.reader_brief {
            if let Some(source) = &reader.source_audit_brief {
                if audit
                    .briefs
                    .get(&source.brief_id)
                    .is_some_and(|b| b.revision == source.revision)
                {
                    for card in &reader.supporting_card_ids {
                        owners
                            .entry((&source.brief_id, card))
                            .or_default()
                            .push(&finding.finding_id);
                    }
                }
            }
        }
    }
    let mut warnings = Vec::new();
    for brief in audit.briefs.values() {
        for card in &brief.evidence_cards {
            let findings = owners
                .get(&(brief.brief_id.as_str(), card.id.as_str()))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let (code,suggestion)=match findings.len() {
                0=>("UNSELECTED_DISCUSSION_CARD","Check that this card belongs only to the audit history, or select it for a finding."),
                1=>continue,
                _=>("SHARED_DISCUSSION_CARD","Verify that each finding uses this evidence for a distinct decision, or link to its main discussion."),
            };
            warnings.push(json!({"code":code,"suggestion":suggestion,"brief_id":brief.brief_id,"revision":brief.revision,
               "card_id":card.id,"title":card.title,"finding_ids":findings}));
        }
    }
    warnings
}
