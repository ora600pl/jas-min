# Analytical atlas for performance reports

Schema `2026-09-09.1` adds a shared analytical presentation to MCP and API reports.
The previous renderer put the strongest numbers into long `evidence_summary`
paragraphs and hid all structured analysis. The exact-contributor validation
also encouraged duplicating those numbers in a finding after recording them
in a synthesis table. The result was complete evidence with a poor reading order.

The atlas uses the same stored evidence, with no new model fit and no changed
gradient coefficients. It presents:

- Per-instance decision briefs with named, linked signals, a discriminating
  test and explicit limits. Exactly identical hypothesis/test text is shown
  once, with separate instance evidence below it.
- A family/target selector and an SVG bubble plot of combined active (P90)
  versus peak (P99) model impact. Axes use **log(1+x)**, with original-value tick
  labels. Zero is preserved. There is no jitter or invented coordinate.
- Bubble area based on selected-model count; colour based on the source
  selection pattern. Model agreement, recurring, tail/outlier and sparse
  selection have distinct colours and text labels.
- A readable matrix of active/peak impact, their ratio and the four source
  selection flags. The shortlist is the union of the three largest active and
  three largest peak values. All supplied numeric pairs remain on the plot;
  all other contributors, including unscored material waits, remain accessible.
- One largest returned MAD anomaly and one most populated recorded cluster
  per instance, labelled with their actual selection scope. These are bounded
  investigation targets, not a complete incident timeline or severity ranking.

The renderer never combines fits or instances, converts model indices into
recoverable CPU, treats missing values as zero, or infers model agreement from
model names in prose. A missing selection flag is `?`; a false flag means
outside the source top selection. In particular, an absent Elastic Net flag
does not establish a zero coefficient or prove collinearity. The existing
classification identifiers and classification math are unchanged; explanatory
descriptions now state those evidential limits instead of declaring causes.

Display values are rounded; full parsed impact precision remains in HTML number
titles and JSON, while original technical table cells remain unchanged. Ratios
at zero are undefined. Near-zero ratios
can be unstable. Combined impacts are sums of model outputs, not DB Time shares
or additive physical costs. Models share the same observations.

## MCP

Record source-backed `gradients` and `analytic_signal_synthesis` rows as before.
Record the synthesis before the `gradients_anomalies` finding: the validator
revalidates the structured synthesis and no longer requires its exact numbers
to be repeated in the prose. Without that table the previous exact-prose
validation still applies. All existing readiness gates, required families,
material waits, artifacts, tables and assessments remain mandatory.

The adapter matches a gradient row to its own cited source envelope, project,
family and contributor. It uses numeric source impacts and source selection
flags, with recorded numeric cells as a magnitude fallback for old archives.
Unavailable flags remain unknown. It does not infer a vote from a model name
in a free-text `method` field. JSON export includes a derived `signal_atlas`.
If a named contributor appears in both CPU and elapsed-time fits, its brief
shows both links with explicit family/target labels and separate magnitudes.

The source registry and tables remain authoritative. If legacy rows cannot
form a valid atlas, the original narrative and complete tables remain visible;
no partial chart is silently substituted for them. Evidence IDs are plain text
when the report's evidence appendix is disabled.

Findings also have clearer card boundaries, separate workload/time context,
and visible grouped source shortcuts. The long gradient evidence summary moves
inside the finding's supporting detail when the atlas is available.

## APIs and local reviewer

The shared [writing contract](../src/report_writing.md) defines `signal_atlas`
inside the existing `jasmin-issues` manifest. Models provide scoped numeric data,
source selection flags, conclusions and provenance anchors. Place the empty
`jasmin-signals:start/end` HTML comment pair in section 9. The shared renderer
fills it; normalized Markdown retains the JSON for reproducible conversion.

Validation rejects duplicate fits/contributors, negative or nonfinite magnitude
values, unknown project/contributor references, unsafe source URLs, missing
provenance anchors and misplaced generated regions. It prevents replacing a
technical finding or the cited provenance with generated content. This validates
structure; it does not independently verify a model-authored number against an
external database. API source fidelity still requires review.

Old Markdown and issue manifests without an atlas remain convertible. Models
must explain unavailable gradient coverage instead of inventing a visual.

## Interaction and regression checks

The report is a local, self-contained HTML file: native SVG, embedded CSS/JS,
no CDN, external font, chart package or telemetry. Keyboard-accessible selects,
links and disclosure controls accompany the chart. A bubble or signal chip
reveals the correct fit and supporting row, including after reload or browser
history navigation. Full tables remain available without JavaScript. Printing
opens evidence and includes hidden fit panels, then restores the reading state.

The optional `replay_signal_atlas_from_archived_audit` test takes:

```sh
JASMIN_SIGNAL_FIXTURE=/path/to/audit_archive \
JASMIN_SIGNAL_ORIGINAL=/path/to/original.md \
JASMIN_SIGNAL_PREVIEW=/path/to/new_visual.html \
cargo test --offline replay_signal_atlas_from_archived_audit -- --ignored --nocapture
```

The external customer fixture is not committed. Replay reads `report.json` and
the archived `evidence_*.json` envelopes, preserves delivered provenance and
methodology verbatim, checks all fragments/local links, and refuses to overwrite
any prior export. The real-data regression covers 14 separate fits and 228 recorded
contributors. Unit tests cover unknowns, exact values, flags, zero ratios,
scope/URL validation and API normalization.

Rebuild and restart MCP to load the schema and new renderer. Existing exported
HTML does not change automatically; produce a new export to keep earlier audits
reproducible.
