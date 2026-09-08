# Explicit issues and decision summaries

Schema `2026-09-08.2` separates an **issue** (one decision or investigation) from
its **findings** (the evidence perspectives in SQL, waits, latches, models or
other sections). An issue need not be a proven cause or a currently active
incident. Grouping never combines measurements, changes instance/time labels,
or weakens the eleven-section evidence contract.

## MCP authoring

1. Collect and record evidence-backed findings as before. Give every
   recommendation a `kind`: `evidence_capture`, `mitigation` or `durable_fix`.
2. Call `record_issue` after its member findings exist. Supply all fields:

   ```json
   {
     "analysis_id": "A-returned-by-server",
     "issue_id": "I-0001",
     "title": "Investigate import cursor contention",
     "decision_summary": "Two sampled waiters identify the next holder-capture target.",
     "decision_boundary": "A later live sample cannot establish every historical wait's cause.",
     "scope": "Import service; July AWR on two instances and a separate September capture.",
     "grouping_rationale": "Wait attribution and cursor diagnostics support one investigation while retaining distinct periods and proof limits.",
     "confidence": "medium",
     "canonical_finding_id": "F-0002",
     "finding_ids": ["F-0002", "F-0003"]
   }
   ```

   These are example values, not evidence for a real database. Use IDs returned
   in the current session. Each issue has one canonical finding and each finding
   belongs to exactly one issue. A shared SQL_ID or similar wording alone is
   insufficient justification for grouping.
3. Reuse `issue_id` to replace the entire grouping and decision brief atomically.
   `delete_issue` removes only the grouping, preserving findings and evidence.
   When splitting an issue, update its members and then create the new issue.
4. Check `get_report_status`. In addition to all previous evidence checks,
   explicit mode requires empty `missing_issue_assignments`,
   `unclassified_actions` and `stale_issues` lists. Updating any member finding
   invalidates its issue's reviewed snapshot. Review the changed evidence and
   call `record_issue` again before finalization.

Unknown references, duplicate membership (including duplicates within one
issue), ambiguous IDs, an external canonical finding and invalid brief fields
are rejected without replacing the previously valid issue. References resolve
only in the current analysis session. A syntactically valid grouping still
requires expert judgment; validation does not prove that its causal argument
is correct.

`finalize_report` exports an `issues` array, a derived `issue_id` on every
finding, issue IDs in `section_index`, and recommendation kinds. The issue
registry is the authoritative relationship; a derived field is not a second
independent mapping.

## Presentation

The summary contains at most five issues, ordered by their earliest action
priority, then issue ID for deterministic ties. The next action is selected
from every supporting finding. Within the same priority, evidence capture
precedes mitigation and a durable fix. The brief, scope, decision boundary,
confidence and canonical link are visible. A complete expandable issue register
retains every issue, its grouping rationale and all supporting links.

Every analytical finding and technical table remains in its original section.
Findings link to their issue and canonical description. Actions are ordered by
priority, identify their kind and owner, and retain rationale, acceptance
criteria and supporting finding links. Only exactly identical actions of the
same kind and priority are consolidated.

Action kind describes intent, not approval or proven efficacy:

| Kind | Meaning | Required context |
|---|---|---|
| `evidence_capture` | Capture, reproduce or validate measurements | The observation that discriminates remaining hypotheses |
| `mitigation` | Temporary containment | Preconditions, success metric and rollback/guard conditions |
| `durable_fix` | A lasting change or an explicitly proposed prototype | Correctness and performance acceptance criteria; actual test status |

Do not label an untested proposal as a verified remedy. Split new actions that
mix collecting evidence and changing production. Historical recommendations
are not proof that the problem is occurring now.

## Classic APIs and local reviewer

All API providers and the local reviewer's final report use the same
[writing contract](../src/report_writing.md). They produce the eleven numbered
Markdown sections plus one fenced `jasmin-issues` JSON block. The block contains
`version: 1`, the same issue objects, and typed actions with `finding_id`,
`kind`, `owner`, `priority`, `action`, `rationale` and `success_criterion`.

Declare each finding with a unique lowercase H3 anchor, for example:

```markdown
### Import wait attribution {#finding-f-0002}
```

Its metadata ID is `F-0002`. Place these empty markers in section 11 before the
mandatory assessments:

```html
<!-- jasmin-actions:start -->
<!-- jasmin-actions:end -->
```

The shared validator checks all eleven section numbers/order, metadata types
and version, IDs, memberships, anchors, action kinds and register placement.
At least one action is required. Markers must occur exactly once outside code
examples; findings cannot occupy the generated summary or action-marker body.
The generator replaces section 1 and the action markers with the same queue
and typed register used by MCP. Technical findings and mandatory assessments
are preserved. Validated metadata remains in normalized Markdown for repeatable
re-export and is omitted from HTML. Metadata is not a substitute for factual
evidence references in the findings.

A newly generated API response without valid metadata fails finalization. Its
raw Markdown response remains available for diagnosis; no new HTML is written.
The program returns a nonzero status. There is no fallback that silently
ignores a malformed manifest. This structural validation is deterministic;
quality of synthesis and semantic grouping still depends on the model and
reviewer. Provider calls and database interventions are not required by the
unit/replay tests.

## Compatibility and migration

- New MCP sessions default to `issue_grouping: "explicit"`.
- An older client can explicitly select `issue_grouping: "legacy"` through
  `configure_report`. It retains the prior finding-based queue and evidence
  completeness gates. Existing explicit groupings must be removed before
  choosing legacy mode; this prevents hiding a partial registry.
- Old serialized configs lacking `issue_grouping` deserialize as legacy in
  archival replay. Missing recommendation kinds become `unclassified`, never
  an inferred intervention type. Explicit mode cannot finalize such actions.
- Existing Markdown without metadata remains convertible through
  `--convert-md2html`. New AI generation requires metadata; malformed metadata
  is rejected even during standalone conversion.
- The optional replay test accepts `JASMIN_REPORT_ISSUES=/path/to/overlay.json`.
  The reviewed overlay contains `issues` and `action_kinds`, a map from finding
  ID to the ordered list of recommendation kinds. It must cover every finding
  and action. The source archive is unchanged and outputs use new filenames.

Rebuild and restart the MCP process for the new schema and tools. Reconnect
clients that cached the tool catalog. No persisted analysis sessions or running
database settings are migrated automatically.
