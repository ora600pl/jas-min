# Human-readable performance report contract

Write for an expert DBA making decisions during an incident. Keep the full investigation and deterministic evidence coverage; control repetition and reading order instead of omitting evidence.

- Executive Summary is a decision queue, not a second copy of the report. Aim for 250 words and at most five issues. Start with immediate actions, then high-priority work. For each issue state the affected workload, measured impact, next action and owner, and a link to its canonical finding. If there is no proven mitigation, say that the next action is evidence capture. Do not invent a fix or declare a historical incident currently active.
- Use real Markdown headings: `##` for the eleven report sections and `###` for findings. A numbered bold paragraph is not a heading. Keep titles short (aim for 12 words); move qualifications, chronology and citations into the body. Severity, action priority and confidence are distinct: high confidence does not mean urgent.
- Record one canonical finding per mechanism and affected workload. Connect wait, SQL, latch, segment and model views with links instead of repeating the diagnosis and action in each category. A section may explain its distinct contribution or evidence boundary without claiming another independent problem. Do not merge distinct instances, periods or mechanisms just because they share a SQL_ID.
- Each finding starts with a concise conclusion, one or two decisive measurements (including unit, instance and time window), a specific next action and owner, and the limitation that could change that decision. Keep this visible decision layer around 120 words. Use `mechanism`, `evidence_summary` and `details` for the complete supporting analysis, rather than copying the same paragraph into every field. These are editorial targets, never grounds for truncating facts or skipping completeness checks.
- Separate observed facts, supported hypotheses and unknowns. A later live sample does not prove the cause of an earlier AWR interval; a lab reproduction does not prove the production mechanism. A model classification is a model signal, not causal proof. State the important boundary next to the recommendation even when other technical detail is collapsed.
- Preserve exact evidence_refs in structured state. In prose, show decisive values once, with one contextual link to the supporting detail. Avoid long chains of raw evidence IDs and repeated links to the same artifact. Put the full provenance in expandable evidence details or an appendix; never remove provenance from the stored evidence.
- Store verified guidance quotations separately from measurement evidence. Print each distinct quotation once in a methodology appendix and link to it where applied. Do not repeatedly quote methodology in findings, tables and recommendations. Never invent source references or MOS notes.
- Keep all required tables, model families, plan variants, child cursors, segments, parameter reviews and mandatory assessments. Put exhaustive tables and extended narrative inside `<details class="technical-evidence"><summary>Technical evidence</summary>` blocks with blank lines around the Markdown body and closing tag. Keep findings, actions, decisive measurements and decision-changing limitations outside those blocks. Use a deep report to retain additional content, not to force every appendix open.
- Give each action one accountable owner, a priority, the reason, a measurable success criterion and any necessary test/rollback condition. Distinguish evidence capture, temporary mitigation and a tested durable fix in the action wording. Do not promise a latency reduction without a measured test. Consolidate identical actions and link every supporting finding. Rank by priority before grouping by owner.
- In section 11 provide the complete action register and explicit mandatory assessments. Keep negative results concise (for example, no generalized slow-storage evidence); expand only measurements or uncertainty relevant to a decision. Technical depth belongs one click away, not on every line of the decision queue.

Use the requested report language. Keep the eleven existing sections and all validation requirements. Do not shorten an investigation by skipping available evidence or making stronger claims.

## Working hypotheses after scope

Once the instance, workload and period are established, compare DB Time and DB CPU gradients with measured workload levels in the same scope. Include a costly, steady contributor if one is observed. Distinguish typical activity and peak signals, and locate each candidate on the actual timeline. Choose two or three distinct, testable explanations. Give each a stable identity (H1, H2 and optionally H3), the gradient observation, the absolute measurement with its unit, the comparison window and a check that could disprove it. If coverage prevents forming two, explain the missing comparison and retain any viable hypothesis.

Open section 2 with the measured workload, named gradient contributors and the observed anomaly and cluster windows for each instance. Give the timestamp or snapshot, anomalous metric, value and comparison when available. Explain any missing family. Then present the opening hypotheses as they stood before the checks. In the relevant sections 3 through 8 refer back to each hypothesis and record what the chapter's measurements added, including contrary evidence or a specific coverage gap. Continue investigating material workload outside the opening hypotheses. Open section 9 with the result for every original identity. State confirmed, rejected or unresolved in the report language, the decisive observation and why it changes the interpretation. For an unresolved result specify the missing comparison. Support a confirmed or rejected outcome with measurements beyond model agreement.

For classic API and local reports, give each opening hypothesis a Markdown heading such as `### H1 — <statement> {#hypothesis-h1}`. From the relevant later chapter link back with `[H1](#hypothesis-h1)` next to its evidence. Use a corresponding heading in section 9, for example `### H1 — <localized outcome> {#review-h1-confirmed}`. The anchor's last word must be `confirmed`, `rejected` or `unresolved`. Place the reasoning in a paragraph under each heading, outside the generated atlas markers. When fewer than two hypotheses are supportable, explain the coverage gap in section 2 under `{#hypothesis-gap}` and its consequence in section 9 under `{#review-gap}`.

## Reader explanations and the guided audit handoff

The complete diagnostic record and the reader explanation are different layers. In MCP `record_finding`, keep `conclusion`, `mechanism`, `temporal_pattern`, `affected_workload`, `evidence_summary`, `evidence_limitations` and `details` complete. Add a `reader_brief` with four connected paragraphs:

- `observed`: what happened, including the decisive measurement and its scope.
- `explanation`: why it matters and how the possible mechanism works. Explain technical terms at first use when they are needed for the decision.
- `next_step`: one concrete next investigation or decision, with an accountable owner. Put the full measurement recipe in the action record.
- `boundary`: what remains unknown and how it could change the decision.

Cite a non-empty subset of the finding's evidence in `reader_brief.evidence_refs`. The structured record keeps every original diagnostic field. Earlier briefs render distinct diagnostic paragraphs in supporting evidence. Do not compress several investigations into a paragraph to satisfy a word target; split the decisions and preserve the complete technical explanation. Operation numbers, exhaustive object lists and measurement recipes belong in supporting analysis unless they are necessary to understand the decision.

For a guided audit, create the report only after the user's explicit final report request. Link each analytical finding's reader explanation to the current discussion brief with `source_audit_brief: {brief_id, revision}`. Match its category and project scope, preserve every evidence reference from that discussion brief in the finding, and add new supporting references when necessary. Discussion briefs remain editable audit records; a changed source revision requires reviewing and updating the report explanation. Never copy an obsolete interpretation merely because its wording reads well.

For a guided report, save observations as evidence cards while discussing each question. Give every card an ID local to the brief, a kind (measurement, comparison, hypothesis or verification), a short title, a Markdown body and the exact source IDs supporting that card. Keep measurements and deductions distinguishable. Use small tables for like-for-like comparisons, fenced SQL for examples, and a short numbered list for a verification procedure. Keep each card focused on one claim.

Select the cards for each finding in `reader_brief.supporting_card_ids`, in reading order. Their evidence IDs must be included in the decisive `reader_brief.evidence_refs`. The source revision, selection and source membership are validated at review and publication. With cards, only the selected supporting analysis is projected into the finding. Keep full metrics and code needed for the decision in those cards. The full diagnostic fields, other cards and all provenance remain in structured output. Retained visuals with broader evidence scope stay with the discussion. For delivery, validate the HTML and Markdown at the final package location with `scripts/validate-report-package.py`.

Each recommendation may add `action_summary`, `steps` and `decision_rule`. Use `action_summary` for one readable next action, preserve the complete procedure in `action` and ordered `steps`, and use `decision_rule` to explain how different measured outcomes change the next decision. Keep the existing owner, priority, kind, rationale and measurable success criterion. Separate an initial capture from conditional follow-up investigations or a production change.

Before final publication, use the report preview and review the actual rendered reader layer. Check that the explanation states the problem, significance, next action and uncertainty without requiring the reader to reconstruct them from counters. Review issues, action summaries and plan explanations as well as findings. Editorial density warnings are hints, not proof of bad prose; word counts cannot certify clarity. The semantic review must also verify that no measurement, time window, instance, evidence reference, unknown value or condition was lost and that hypotheses did not become facts. Preserve every deterministic coverage gate. A preview or editorial review does not itself authorize final report generation.

## Explicit issue identity and typed actions

An issue is one decision or investigation, possibly supported by findings in several sections. It is not automatically a proven cause or a currently active incident. Give it an explicit `issue_id`, a short `title`, `decision_summary`, `decision_boundary`, `scope`, `grouping_rationale`, `confidence`, one `canonical_finding_id` and all supporting `finding_ids`. Group only when the investigation identity is established. State differences between instances and periods in scope; do not silently combine historical AWR, a later live capture and a lab experiment. A finding belongs to exactly one issue; cross-cutting background coverage can have its own issue.

Classify every action as `evidence_capture` (obtain or validate measurements), `mitigation` (temporary containment with guard/rollback conditions), or `durable_fix` (a tested or explicitly proposed lasting change, with correctness and performance acceptance criteria). A `durable_fix` label does not prove the fix was tested. Keep priority independent of kind. Do not classify a speculative intervention as an established remedy. Split actions that combine evidence collection and a production change. Preserve owner, priority, rationale and success_criterion.

**MCP authoring:** record findings first, including recommendation `kind`; call `record_issue` to link existing finding IDs. Reuse issue_id to replace a grouping; use `delete_issue` to remove only the grouping when restructuring. The default `explicit` mode requires every finding to be assigned and every recommendation to have a kind before finalization. Follow missing_issue_assignments, unclassified_actions and stale_issues from get_report_status. After editing a member finding, review its issue and call record_issue again to refresh the decision brief. Never select legacy mode merely to evade the new contract. MCP renders the issue queue from structured state; do not add a metadata block to its finalized Markdown.

**Classic API / local reviewer authoring:** keep the eleven numbered `##` sections. Assign a unique lowercase explicit anchor to each finding: `### Finding title {#finding-f-0001}`. Use its matching ID (`F-0001`) in metadata. Section 1 can say that the generator will insert the issue queue. In section 11 put these two empty markers, followed by the full mandatory assessments:

<!-- jasmin-actions:start -->
<!-- jasmin-actions:end -->

After the report, append exactly one fenced block with language `jasmin-issues` and valid JSON. Use this schema (example data below illustrates syntax only, not case evidence):

```jasmin-issues
{
  "version": 1,
  "issues": [{
    "issue_id": "I-0001",
    "title": "Investigate import cursor contention",
    "decision_summary": "State the observed impact and decision in about 40-60 words, using actual case measurements.",
    "decision_boundary": "State the missing proof that could change the decision.",
    "scope": "Name the actual workload, instances and time windows.",
    "grouping_rationale": "Explain why the linked findings are perspectives of this one investigation.",
    "confidence": "medium",
    "canonical_finding_id": "F-0001",
    "finding_ids": ["F-0001", "F-0002"]
  }],
  "actions": [{
    "finding_id": "F-0001",
    "kind": "evidence_capture",
    "owner": "DBA",
    "priority": "immediate",
    "action": "Specify the concrete next capture for this case.",
    "rationale": "Explain why this capture discriminates between the remaining hypotheses.",
    "success_criterion": "Specify the required timestamped measurements and identities."
  }]
}
```

Replace every example value with case-specific content. Include every finding in exactly one issue and all recommendations in `actions`. Write the complete technical findings and mandatory assessments in Markdown; never put that detail only in metadata. Do not duplicate the action register manually. The generator validates references and renders the summary/register; metadata is retained in saved Markdown and hidden in HTML. Missing or malformed metadata in a newly generated AI report blocks HTML publication. Existing Markdown without metadata remains supported by standalone conversion.

## Analytical atlas: conclusions before numeric transcripts

Use the signal atlas at the start of section 2 to answer: which named workload contributes during active periods; which signal becomes disproportionately large in the tail; where models disagree; what the anomaly/cluster actually localizes; and which runtime test can distinguish the remaining explanations. Explain the connection, not merely that several signals exist. Use source measurements to make these answers specific to each instance. Do not copy the same generic hypothesis across projects when their named signals differ. If a hypothesis is genuinely shared, explicitly retain each instance's different evidence.

Keep the visible finding conclusion around 60 words. Put exact model coefficients, full-precision values, source timestamps and selection labels in structured rows and expandable evidence. Use short linked names and rounded display values; retain the exact source values in data. Never manufacture a current incident, missing model vote, zero value, common cause or savings estimate for the visual.

**MCP:** record `analytic_signal_synthesis` before the `gradients_anomalies` finding. Its validated rows satisfy the requirement for exact named contributors, model agreement and localized windows. The finding then states the decision and boundary without repeating the numbers. The generator creates the atlas from the recorded gradient rows and their cited `full_gradients` envelopes. Supply specific `hypothesis`, `counterevidence` and `recommended_validation` in the synthesis rows. A cross-model class is a model selection pattern, not causal proof. Elastic Net omission is not, by itself, proof of a zero coefficient or collinearity. Models share data; do not describe them as independent causal witnesses.

**Classic API / local reviewer:** when gradient data is available, add `signal_atlas` to the existing `jasmin-issues` JSON object. Put empty `<!-- jasmin-signals:start -->` and `<!-- jasmin-signals:end -->` markers in section 2, after the opening signal context and before the hypotheses, outside technical findings. The generator fills this region with the shared visual atlas. Use the exact source combined active/P90 and peak/P99 impacts, not MAD-based legacy impact, percentages, beta coefficients or estimated saved CPU. Use null for an unavailable magnitude or model selection. Do not sum across projects, target metrics or predictor families.

The atlas has `version: 1`, `panels`, `briefs` and `moments` arrays with these exact object fields:

- Panel: `id` (unique ASCII letters/digits/hyphens), `project_id`, `project_label`, `window`, `family`, `target`, `coverage`, `points`. One panel is one project and one target/predictor fit. State returned limits and partial coverage.
- Point: `name`, `active` (number/null), `peak` (number/null), `classification` (exact source class or explicit unavailable label), `selected` (four booleans/nulls in Ridge, Elastic Net, Huber, Quantile 95 order), `interpretation`, `action`, `href` (existing source-report URL or null), `evidence_refs` (source IDs). Model selection must reproduce the source classification flags, not whether a model name appears in prose. False means outside top selection, not zero cost.
- Brief: `project_id`, `project_label`, `title`, `conclusion`, `boundary`, `validation`, `confidence`, `signals` (1–12 exact contributor names from that project's panels), `evidence_refs`. Write a discriminating test and explain how its outcomes change the decision.
- Moment: `project_id`, `project_label`, `kind`, `window`, `title`, `measure`, `context`, `evidence_refs`. Include exact observation windows and explain the supplied subset; do not imply exhaustive incident coverage or rank incident severity by anomaly count.

Every referenced source ID requires a real provenance anchor outside generated regions, for example `<a id="evidence-e-0001"></a>` for `E-0001`, with its actual source and measurement context. Keep full source tables below the atlas. The source data belongs in technical evidence even if only a subset is visualized. When no valid gradient data is supplied, leave `signal_atlas` null and state the missing coverage; never invent chart coordinates. Reports without an atlas remain supported for compatibility.


## Gradient v2 evidence contract

Read `settings.methodology_version` and `settings.quantile95` before treating Q95 as usable.
Q95 models the conditional upper quantile of target changes, with a free intercept and independent,
unit-invariant regularization. An iteration-limited fit is diagnostic, excluded from agreement.

Compare independent `active_p90`, `peak_p99` and `extreme_max` selections. P90 and P99 include zeros;
zero active impact does not mean no incident impact. Use `model_rankings`, `selection_reasons`, ranks
and `predictor_coverage` to explain rare peaks. In MCP/local API tools, query full_gradients with
`family`, `contributor`, `ranking` and `offset` for full signed coefficients and paging metadata.
Source absence, outside TOP, fitted zero and negative coefficient are different states.

SQL fits use retained top-list work with a zero-filled proxy; missing rows are not measured zeros.
Report observed/missing counts and observed delta pairs when coverage affects the decision. Align
actual SQL peaks with DB Time/waits before calling them incidents. Max and P99 are distinct metrics.
Cross-model selection labels never establish causality, collinearity, explained variance or savings.
