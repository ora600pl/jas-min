# Guided audit and readable reports

A guided audit discusses one chapter at a time before the auditor requests a
report. Discussion progress and evidence completeness are separate: a chapter
can have an explained coverage gap, while the original report gates still
require every applicable measurement, table, assessment and finding.

## From discussion to the client report

Companion records the SQL IDs and wait events shown on boards, including table
rows, annotations and previously saved chapter revisions. The conversation host
calls `companion_record_discussed_entities` for names mentioned in chat. A
direct MCP client can call `record_discussed_entities` with the analysis,
project, `sql_ids` and `wait_events`. Reopening the audit restores the register.
The problem-oriented report includes these entries even when their source brief
was outside the selected chapters. A mention alone carries no measured cost.

In a single-instance table, SQL IDs and wait names link to charts for that
instance. A SQL chart includes the full captured statement. Each instance
has one card per wait event, with the available foreground and background
charts on the same card. The client report opens with bubble plots of the DB
Time and DB CPU gradients across the full observation period of each instance.
Each plot compares contributors within one predictor family and target. Its
axes show model impact at P90 and P99 of changes across that period. Discuss
dated peaks, their comparison windows and the checks of individual hypotheses
in the chapters about those problems. An optional SQL recommendation quotes the applicable source
expression, shows a proposed replacement and states the conditions under which
it preserves the result.

## Conversation workflow

1. Select a project in Companion. Compatible servers start a guided audit with
   `start_audit`; older servers explicitly report that guided auditing is not
   available. The ordinary evidence panel remains usable.
2. Read `get_audit_status` to obtain the current question, chapter progress,
   saved explanations and unresolved questions. Discuss the current chapter at
   the auditor's pace. Acknowledgements and chapter transitions do not request
   report generation.
3. Use `record_audit_brief` after a meaningful conclusion or correction. Store
   the question, explanation, conclusion, evidence boundary and next step, with
   exact evidence references. Reuse `brief_id` for corrections. Previous
   revisions remain in the audit history. A brief without evidence references
   records a discussion or coverage gap; it is not measurement evidence.
4. Use `advance_audit` to move to or revisit a chapter. The sections cover scope,
   gradients and anomalies alongside the workload profile, waits, SQL, segments,
   internal contention, I/O, UNDO/redo, hypothesis review, configuration and
   final decisions.
   Comparative audits need a discussion or explicit gap for each selected
   project in every chapter. One project's brief cannot close another's scope.
5. When the auditor explicitly asks for the report, call `authorize_report`
   with that actual request. The server records the request against the current
   discussion revision. It cannot authenticate chat intent independently; the
   Companion skill requires the assistant to use the user's actual instruction.
   Saving briefs, exporting a checkpoint and completing chapters never grant
   permission to generate a report.

Click a chapter in Companion to see its question, explanations and the views
displayed during this panel session. Select a view by title and ask about it
in the chat. `companion_context` identifies the selected chapter, view and
brief alongside the active discussion step. With `include_displayed_board`
it also returns the values and sources visible on the selected board.
Browsing preserves the discussion
step and report authorization. Chapter views remain available after the
twenty-view display cache rolls over. To retain a board when reconnecting to
the same analysis in a new panel session, attach it to a brief with `view_id`.
A chapter without a board still shows its question and any saved briefs.

### Following a hypothesis through the audit

After recording the scope, compare the DB Time and DB CPU gradients with the
absolute workload for each project. Inspect the incident and a comparable
baseline. Read `load_profile_anomalies` and `anomaly_clusters` at this stage
to identify the measured windows and metrics that warrant a closer look.
Keep the observation and its evidence reference in the opening brief.
Use `top_sqls_in_snapshot` in both windows to find recurring costs.
For example, a SQL statement can use substantial CPU in both periods
and have a low gradient. A second statement with a large peak gradient needs a
timeline check to establish whether its changes coincide with the incident.

Save two or three `opening_hypotheses` in the performance profile. Give each
one an ID, a statement, its gradient signal, its measured workload, the
comparison window and a distinguishing test. Cite the gradient and load
measurements in the brief. Use `hypothesis_gap` to explain an unavailable
opening comparison. Retrieve the current hypotheses from `get_audit_status`
when resuming a chapter.

In the relevant intervening chapters, record `hypothesis_checks` against
the original IDs. For example, compare the number of SQL executions and the
cost per execution in both windows to distinguish increased demand from more
expensive executions. In chapter 9, give every opening ID a
`hypothesis_outcomes` entry with a verdict, a reason and its evidence.
Use `confirmed` or `rejected` for a measured result and `unresolved` when
the observations cannot distinguish the explanations. Name the missing
measurement. Keep each check's and outcome's evidence references in its brief.

## Report authoring and publication

Existing `record_finding`, `record_issue`, `record_report_table` and assessment
tools remain authoritative for the complete analysis. A guided analytical
finding also supplies a `reader_brief` with connected paragraphs:

- `observed`: what was observed and why the reader should care;
- `explanation`: the reasoning, with hypotheses labelled accurately;
- `next_step`: the next useful action and its purpose;
- `boundary`: the uncertainty that can change the decision;
- `evidence_refs`: up to 32 decisive references;
- `source_audit_brief`: the saved `brief_id` and current `revision`.

Keep all source-brief evidence in the finding's complete `evidence_refs` (up to
128); additional evidence is allowed. Link to the correct chapter and project.
Changing a discussion brief invalidates the old source revision and revokes
its recorded authorization. During preparation of an already requested report,
review the correction and refresh the authorization using the same still-valid
user request; this does not require another permission question. Existing unguided reports retain their compatible path.

Recommendations can use `action_summary`, ordered `steps` and `decision_rule`
in addition to the original full action, rationale and success criterion.
Separate independent investigations and state when a conditional follow-up is
justified. The readable layer does not replace the original evidence fields.
Original fields and technical procedures remain available in the output,
including the free-form details of a finding with a reader brief.

After authoring, call `preview_report` for Markdown, HTML or both. This does not
publish, increment the report revision or enable HTML export. In guided audits
it is available only after the final report request. Read the actual preview,
revise unclear prose, then call `record_report_review` with its `preview_id`,
specific notes and all five checks:

- `explanation_clear`
- `actions_executable`
- `uncertainty_preserved`
- `evidence_preserved`
- `language_consistent`

These recorded checks attest to an editorial review; they are not an automatic
proof that prose is good. Soft warnings identify dense identifiers, long fields
and repeated or overloaded actions without deleting facts or imposing a hard
word limit. A later content or configuration change makes the review stale.

`finalize_report` retains the original evidence completeness contract and adds
the discussion authorization, current source linkage and editorial review
checks for guided audits. An explicitly requested incomplete draft still needs
the guided publication checks. The structured output includes complete evidence
payloads, guidance and audit revision history. `convert_markdown_to_html` still
accepts only the exact latest finalized Markdown. Polish generated headings and
labels are localized; source SQL, identifiers and measurements are preserved.

## Continuation and checkpoints

`export_audit` returns a checkpoint containing every evidence payload, guidance
excerpt, discussion revision and report object. The assistant must save the
returned `checkpoint` object in a private JSON file in the configured audit
workspace before claiming that it is durable on disk. Paths and project
identities are supplied at runtime. An in-memory audit survives Companion view
eviction and reconnection to the same server, but not a server restart.

`companion_resume_audit` reconnects to an existing analysis. After a server
restart, `restore_audit` accepts a saved checkpoint, creates a fresh analysis,
verifies the selected datasets and replays measurement calls. Changed source
results reject the restore without leaving a partial analysis. Saved guidance
excerpts must still match the corresponding current text; the stored excerpt
boundary is retained. Findings and tables pass through their existing validators.
Restoring always clears publication authorization and editorial review.

Export and restore currently have a 64 MiB complete-checkpoint limit. Oversized
exports fail explicitly and leave the session intact; they never drop evidence
or truncate narrative to fit. Checkpoints contain sensitive audit data and
should follow the same storage policy as the original capture.

## Verification and rollout

The two neutral acceptance examples cover a distributed SQL plan and CPU cost
per unit of work. Their tests verify narrative order, separate conditional
actions, exact retained technical details, unchanged source evidence and the
production Markdown/HTML renderer. The examples deliberately do not represent
a complete audit or pass its evidence gate.

```sh
cargo test --bin jas-min audit_workflow
cargo test --bin jas-min report_
cargo test --bin jas-min companion_core_transport_integration -- --ignored
cd plugins/jasmin-companion
npm test
```

For the keyboard and click-through checks in Chromium, make Playwright
available to Node and run the transport test from the repository root.
Set `BROWSER_PATH` if using a separately installed Chromium executable.

```sh
JASMIN_AUDIT_BROWSER_QA=1 cargo test --bin jas-min companion_core_transport_integration -- --ignored
```

An opt-in preview writes new artifacts only; choose a new output path:

```sh
JASMIN_READABILITY_PREVIEW=/tmp/new-reader-preview.html \
  cargo test --bin jas-min preview_report_readability_examples -- --ignored
```

Build the updated binary and restart the MCP server after saving active audit
work. Update/reload the installed Companion plugin so its skill and tool catalog
match the new server. Existing HTML reports require regeneration. The new code
does not migrate or silently discard a running server's in-memory sessions.
