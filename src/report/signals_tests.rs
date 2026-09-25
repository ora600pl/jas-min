use super::*;

pub(crate) fn fixture() -> SignalAtlas {
    SignalAtlas {
        version: 1,
        panels: vec![SignalPanel {
            id: "p1-waits".into(),
            project_id: "p1".into(),
            project_label: "Instance 1".into(),
            window: "01-Sep-2026 05:00–06:00".into(),
            family: "Foreground waits".into(),
            target: "DB Time".into(),
            coverage: "Two selected signals and one unscored material wait.".into(),
            points: vec![
                SignalPoint {
                    name: "cursor: pin S wait on X".into(),
                    active: Some(4.533861353837059),
                    peak: Some(71.41438138101672),
                    classification: "CONFIRMED_BOTTLENECK".into(),
                    selected: [Some(true); 4],
                    interpretation: "A peak-sensitive association; the holder is unknown.".into(),
                    action: "Capture the holder during the same incident.".into(),
                    href: Some("reports/fg_cursor.html".into()),
                    evidence_refs: vec!["E-1".into()],
                },
                SignalPoint {
                    name: "cell disk open".into(),
                    active: Some(8.979495103330466),
                    peak: Some(23.391584744175685),
                    classification: "TAIL_RISK".into(),
                    selected: [Some(false), None, Some(false), Some(true)],
                    interpretation: "Check request volume and latency separately.".into(),
                    action: "Capture the aligned request volume.".into(),
                    href: None,
                    evidence_refs: vec!["E-1".into()],
                },
                SignalPoint {
                    name: "unselected material wait".into(),
                    active: None,
                    peak: None,
                    classification: "MATERIAL_NOT_SELECTED".into(),
                    selected: [None; 4],
                    interpretation: "Material wait outside the selection.".into(),
                    action: "Retain the incident capture.".into(),
                    href: None,
                    evidence_refs: vec!["E-1".into()],
                },
            ],
        }],
        briefs: vec![SignalBrief {
            project_id: "p1".into(),
            project_label: "Instance 1".into(),
            title: "Separate recurring work from pin peaks".into(),
            conclusion:
                "The models select both signals; their timing needs independent verification."
                    .into(),
            boundary: "Selection is not holder proof.".into(),
            validation: "Capture a complete holder chain.".into(),
            confidence: "medium".into(),
            signals: vec!["cursor: pin S wait on X".into()],
            evidence_refs: vec!["E-1".into()],
        }],
        moments: vec![],
    }
}

#[test]
fn atlas_keeps_unknowns_exact_numbers_and_model_selection_distinct() {
    let atlas = fixture();
    validate(&atlas).unwrap();
    let html = render(&atlas, true);
    let parsed = scraper::Html::parse_fragment(&html);
    let sel = |s| scraper::Selector::parse(s).unwrap();
    assert_eq!(parsed.select(&sel(".signal-bubble")).count(), 2);
    assert_eq!(parsed.select(&sel(".signal-inspect")).count(), 3);
    assert!(html.contains("4.533861353837059"));
    assert!(html.contains("71.41"));
    assert!(html.contains("Not supplied"));
    assert!(html.contains("selection not supplied"));
    assert!(html.contains("not selected in source classification; not zero cost"));
    assert!(html.contains("log(1+x)"));
    assert!(parsed
        .select(&sel(".signal-test p"))
        .next()
        .unwrap()
        .text()
        .collect::<String>()
        .contains("Capture a complete holder chain"));
    assert!(parsed
        .select(&sel(".signal-caveat summary"))
        .next()
        .unwrap()
        .text()
        .collect::<String>()
        .starts_with("Confidence: medium"));
    assert!(!render(&atlas, false).contains("href=\"#evidence-"));
}

#[test]
fn atlas_rejects_invented_scopes_nonfinite_magnitudes_and_active_content() {
    let original = fixture();
    let mut bad = original.clone();
    bad.panels[0].points[0].active = Some(f64::NAN);
    assert!(validate(&bad).is_err());
    let mut bad = original.clone();
    bad.panels[0].points[0].peak = Some(-1.0);
    assert!(validate(&bad).is_err());
    let mut bad = original.clone();
    bad.panels.push(bad.panels[0].clone());
    assert!(validate(&bad).is_err());
    let mut bad = original.clone();
    bad.briefs[0].project_id = "p2".into();
    assert!(validate(&bad).is_err());
    let mut bad = original.clone();
    bad.briefs[0].signals = vec!["invented contributor".into()];
    assert!(validate(&bad).is_err());
    for href in [
        "javascript:alert(1)",
        "data:text/html,test",
        "//evil.example",
        "java\nscript:alert(1)",
    ] {
        let mut bad = original.clone();
        bad.panels[0].points[0].href = Some(href.into());
        assert!(validate(&bad).is_err());
    }
    let mut escaped = original;
    escaped.panels[0].points[0].interpretation = "<script>alert(1)</script>".into();
    assert!(!render(&escaped, true).contains("<script>"));
    assert!(render(&escaped, true).contains("&lt;script&gt;"));
}

#[test]
fn named_signal_keeps_distinct_cpu_and_time_fits_in_the_brief() {
    let mut atlas = fixture();
    let mut other = atlas.panels[0].clone();
    other.id = "p1-cpu".into();
    other.family = "SQL CPU".into();
    other.target = "DB CPU".into();
    other.points[0].active = Some(2.0);
    other.points[0].peak = Some(3.0);
    atlas.panels.push(other);
    validate(&atlas).unwrap();
    let html = render(&atlas, true);
    let parsed = scraper::Html::parse_fragment(&html);
    let chips = parsed
        .select(&scraper::Selector::parse(".signal-chip a").unwrap())
        .collect::<Vec<_>>();
    assert_eq!(chips.len(), 2);
    assert_eq!(chips[0].value().attr("href"), Some("#signal-p1-waits-1"));
    assert_eq!(chips[1].value().attr("href"), Some("#signal-p1-cpu-1"));
    assert!(chips[1]
        .text()
        .collect::<String>()
        .contains("SQL CPU → DB CPU"));
    assert!(chips[1].text().collect::<String>().contains("Active 2"));
}

#[test]
fn zero_active_has_no_infinite_ratio_or_fake_bubble_coordinates() {
    let mut atlas = fixture();
    atlas.panels[0].points[0].active = Some(0.0);
    let html = render(&atlas, true);
    assert!(!html.contains("NaN"));
    assert!(!html.contains("inf×"));
    assert!(html.contains("cx=\"65.00\""));
    assert_eq!(number(0.000000012345), "1.23e-8");
    assert_eq!(number(71.41438138101672), "71.41");
}
