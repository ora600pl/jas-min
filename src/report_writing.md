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
