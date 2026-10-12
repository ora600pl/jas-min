# Composing an analytical board

`companion_present` accepts `title`, `takeaway`, optional `view_ids` and optional `panels`. Combined output is limited to six panels. Boards remain visible while more evidence is collected. Read the returned validation error and correct the payload rather than falling back to raw JSON.

Every custom panel needs `type`, `title`, `classification` (`measured`, `hypothesis`, `explanation`) and `source` (exact evidence ID or artifact and extraction description). Add `note` for limitations. All copy is English.

- `line`: `unit`, `x_label`, `series: [{name, points: [{x, label, value, snap_id?, instance?}]}]`. `x` is an increasing numeric timestamp (milliseconds, with labels preserving database wall-clock time) or ordered index. One series per verified instance/metric, one unit per panel. `value:null` preserves missing observations. Include missing intervals explicitly; do not interpolate gaps. A point's optional `instance` and `snap_id` support selection. Maximum six series, 3000 points each; narrow the time window when needed.
- `bar`: `unit`, `items:[{label,value}]`. Up to 30 nonnegative measurements; unknown is null.
- `table`: `columns:[...]`, `rows:[[...],...]`. Up to eight columns and 100 rows; cells are text, finite numbers or null.
- `flow`: `steps:[{title,detail},...]`. Two to eight ordered steps. Must be `explanation` or `hypothesis`, never `measured`. Describe arrows as a proposed mechanism, not an observed blocker chain unless separately verified.

For existing evidence, the simplest call is:

```json
{
  "title": "Does the SQL activity coincide with the wait spike?",
  "takeaway": "Compare timing first. Coincidence alone does not identify the blocker.",
  "view_ids": ["<SQL timeline view_id>", "<wait timeline view_id>"]
}
```

Use custom panels when source records need verified instance grouping or a focused baseline. For example, after extracting measurements from actual AWR reports, create a bar panel of job duration across days, a separate bar panel of cursor-wait duration across the same days, and a flow labelled `hypothesis`. Cite reports and the extraction rule. Never copy illustrative numbers into a live presentation.

Before ending the answer, confirm that `companion_present` returned `displayed:true`, and that panel titles, units, source references and the takeaway match the answer. If a presentation cannot be built from the available evidence, state the specific missing evidence in the panel and conversation.
