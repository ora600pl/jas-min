use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::collections::{BTreeMap, BTreeSet};

fn labeled_text<'a>(title: &'a str, id: &str) -> Result<&'a str, String> {
    let title = title.trim();
    let text = title
        .get(..id.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(id))
        .and_then(|_| {
            title[id.len()..]
                .trim_start()
                .strip_prefix(['—', '–', '-', ':', '.'])
        })
        .map(str::trim)
        .filter(|text| !text.is_empty());
    text.ok_or_else(|| format!("Use the matching ID and visible text in the {id} heading"))
}

fn same_statement(visible: &str, original: &str) -> bool {
    if visible.split_whitespace().eq(original.split_whitespace()) {
        return true;
    }
    let mut text = String::new();
    for event in Parser::new(original) {
        match event {
            Event::Text(part) | Event::Code(part) => text.push_str(&part),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            Event::Html(_) | Event::InlineHtml(_) => return false,
            _ => {}
        }
    }
    visible.split_whitespace().eq(text.split_whitespace())
}

/// Match each visible opening proposition to the same ID in the investigator's
/// original review. Markdown presentation and whitespace may differ.
pub(super) fn validate(markdown: &str, original: &[&str]) -> Result<(), String> {
    let mut openings = BTreeMap::new();
    let mut checked = BTreeSet::new();
    let mut reviewed = BTreeSet::new();
    let mut opening_gap = false;
    let mut review_gap = false;
    let mut section = 0;
    let mut heading = None;
    let mut hypothesis_label = None;
    let mut title = String::new();
    let mut needs_explanation = None;
    for event in Parser::new_ext(markdown, Options::ENABLE_HEADING_ATTRIBUTES) {
        match event {
            Event::Start(Tag::Heading { level, id, .. }) => {
                if let Some(previous) = needs_explanation.take() {
                    return Err(format!(
                        "Explain the evidence or coverage gap below {previous}"
                    ));
                }
                heading = Some(level);
                title.clear();
                let Some(id) = id else { continue };
                let anchor = id.as_ref();
                if let Some(key) = anchor.strip_prefix("hypothesis-") {
                    if section != 2 || level != HeadingLevel::H3 {
                        return Err(format!("Place {anchor} in a section 2 subheading"));
                    }
                    if key == "gap" && !opening_gap {
                        opening_gap = true;
                    } else {
                        if !["h1", "h2", "h3"].contains(&key) || openings.contains_key(key) {
                            return Err(format!("Invalid or repeated opening hypothesis {anchor}"));
                        }
                        hypothesis_label = Some((key.to_owned(), true));
                    }
                    needs_explanation = Some(id.to_string());
                }
                if let Some(key) = anchor.strip_prefix("review-") {
                    if section != 9 || level != HeadingLevel::H3 {
                        return Err(format!("Place {anchor} in a section 9 subheading"));
                    }
                    if key == "gap" && !review_gap {
                        review_gap = true;
                    } else {
                        let (key, outcome) = key
                            .split_once('-')
                            .ok_or_else(|| format!("Missing outcome in {anchor}"))?;
                        if !["confirmed", "rejected", "unresolved"].contains(&outcome)
                            || !reviewed.insert(key.to_string())
                        {
                            return Err(format!("Invalid or repeated hypothesis outcome {anchor}"));
                        }
                        hypothesis_label = Some((key.to_owned(), false));
                    }
                    needs_explanation = Some(id.to_string());
                }
            }
            Event::End(TagEnd::Heading(level)) => {
                if let Some((id, opening)) = hypothesis_label.take() {
                    let text = labeled_text(&title, &id)?;
                    if opening {
                        openings.insert(id, text.split_whitespace().collect::<Vec<_>>().join(" "));
                    }
                }
                if level == HeadingLevel::H2 {
                    section = title
                        .split_once(". ")
                        .and_then(|(n, _)| n.parse::<usize>().ok())
                        .unwrap_or(0);
                }
                heading = None;
            }
            Event::Text(text) | Event::Code(text) => {
                if heading.is_some() {
                    title.push_str(&text);
                } else if !text.trim().is_empty() {
                    needs_explanation = None;
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) if (3..=8).contains(&section) => {
                if let Some(id) = dest_url.strip_prefix("#hypothesis-") {
                    checked.insert(id.to_string());
                }
            }
            _ => {}
        }
    }
    if let Some(id) = needs_explanation {
        return Err(format!("Explain the evidence or coverage gap below {id}"));
    }
    let ordered_ids = ["h1", "h2", "h3"].into_iter().take(openings.len());
    if !openings.keys().map(String::as_str).eq(ordered_ids)
        || openings.len() != original.len()
        || (openings.len() < 2 && !opening_gap)
        || opening_gap != review_gap
    {
        return Err("Preserve the original two or three hypotheses in section 2, or explain insufficient coverage in sections 2 and 9".into());
    }
    let distinct: BTreeSet<_> = openings.values().map(|text| text.to_lowercase()).collect();
    if distinct.len() != openings.len() {
        return Err("Each opening hypothesis must state a distinct explanation".into());
    }
    for ((id, visible), expected) in openings.iter().zip(original) {
        if !same_statement(visible, expected) {
            return Err(format!(
                "Preserve the original statement for {id} in section 2"
            ));
        }
    }
    if !checked.iter().eq(openings.keys()) || !reviewed.iter().eq(openings.keys()) {
        return Err("Each opening hypothesis needs a linked check in sections 3–8 and a reasoned outcome in section 9".into());
    }
    Ok(())
}
