# JAS-MIN MCP Server

JAS-MIN can expose one or more parsed Oracle AWR or STATSPACK collections as a stateful
Model Context Protocol (MCP) server. The server is intended for interactive,
evidence-backed performance investigations: a model receives a compact map of
the available statistics, requests focused evidence, records conclusions with
explicit references, and asks JAS-MIN to render a report with a stable
structure.

The MCP mode reuses the same analytical data structures and diagnostic tools as
the existing AI integrations. It does not implement a second parser or a
parallel analysis engine.

## Design goals

The server is built around the following constraints:

- parsing and statistical analysis finish before the endpoint starts listening;
- multiple large AWR collections can remain in memory and are queried through
  stable project handles and bounded tools;
- the first analysis call teaches the model what JAS-MIN has already computed;
- observed data, diagnostic methodology, and model-authored conclusions remain
  separate;
- every report conclusion can refer to evidence collected in the same analysis;
- the final report has a deterministic core structure, while its language,
  audience, detail, and appendices remain configurable;
- missing data remains `unknown` instead of being silently interpreted as an
  absent problem;
- the built-in listener is local-only and does not pretend to provide
  authentication or transport security.

## Starting the server

Parse a directory and start MCP:

```bash
jas-min --directory ./awr_reports --security-level 2 \
  --mcp 127.0.0.1:4242/mcp
```

Reuse an existing serialized collection:

```bash
jas-min --json-file ./awr_reports.json --security-level 2 \
  --mcp 127.0.0.1:4242/mcp
```

Load several projects into one comparative server by repeating either input
option. Directory and JSON inputs may be mixed:

```bash
jas-min \
  -j ./before_upgrade.json \
  -j ./after_upgrade.json \
  -d ./month_end_awr \
  --security-level 2 \
  --mcp 127.0.0.1:4242/mcp
```

One server accepts at most 32 projects.

Repeated `--directory` or `--json-file` inputs require `--mcp`. JAS-MIN derives
a stable, lowercase `project_id` from every basename and adds a numeric suffix
when basenames collide. The IDs are published by
`list_performance_projects`; clients must not infer them from paths.

With several inputs, `--outfile` is rejected because one global destination is
ambiguous. JAS-MIN also resolves the classic chart directory generated for
each input before parsing begins and rejects collisions. This prevents two
projects with similar filenames from silently overwriting one another's HTML
assets. Rename one input or start JAS-MIN in a different working directory when
the validation reports a collision.

The argument has the form `IP:PORT/PATH`. If the path is omitted, `/mcp` is
used. An `http://` prefix is accepted, but `https://` is rejected because the
embedded server does not terminate TLS.

Only loopback addresses are accepted. For example,
`127.0.0.1:4242/mcp` is valid, while `0.0.0.0:4242/mcp` is rejected. Remote
access must be provided by a trusted proxy that adds TLS, authentication, and an
appropriate authorization policy.

The `--file` input mode cannot be combined with `--mcp`. A single parsed report
does not provide the complete time-series collection required by the tool
layer.

The minimum supported Rust version for this feature is 1.88.

## Startup architecture

JAS-MIN performs the following work synchronously before binding the TCP
listener:

1. Validate every input and reject duplicate canonical paths.
2. Parse every input directory or deserialize every requested JSON collection.
3. Build one full `AWRSCollection` time series per project.
4. Calculate a corresponding compact `ReportForAI`, including degradation, correlations,
   gradients, anomalies, waits, SQL, I/O, latch, segment, and parameter
   summaries.
5. Resolve each dataset stem used to discover sibling attachments.
6. Load and index diagnostic guidance from `reasonings.txt`.
7. Construct `AnalysisRuntime` with an immutable project map and an empty analysis
   session map.
8. Bind the loopback listener and mount the Streamable HTTP service at the
   configured path.

The runtime retains the following objects until the process exits:

| Object | Storage | Purpose |
|---|---|---|
| Project map | shared ordered map | Stable `project_id` to immutable project data. |
| `AWRSCollection` | one shared `Arc` per project | Complete parsed Oracle time series and SQL/parameter data. |
| `ReportForAI` | one shared `Arc` per project | Precomputed and bounded statistical summaries. |
| Dataset stem | one shared `Arc` per project | Locates `<stem>_attachments` and its AIX subdirectory. |
| Guidance library | shared `Arc` | Indexed sections loaded from `reasonings.txt`. |
| Analysis sessions | concurrent `DashMap` | Evidence, guidance references, findings, assessments, and report configuration keyed by `analysis_id`. |
| Analysis sequence | atomic counter | Generates unique process-local analysis identifiers. |

Parsed collections and precomputed reports are immutable after startup.
Conversational state is isolated in per-analysis records protected by a mutex.

## Request flow

The following graph shows the normal request path and the boundary between the
MCP transport session and the JAS-MIN analysis session.

```mermaid
flowchart TD
    Client["MCP client or model host"]
    Endpoint["Streamable HTTP endpoint<br/>127.0.0.1:PORT/PATH"]
    Transport["LocalSessionManager<br/>Mcp-Session-Id"]
    Handler["JasminMcpServer<br/>ServerHandler"]
    Runtime["AnalysisRuntime<br/>shared project registry"]
    Projects["ProjectData map<br/>project_id to collection/report/attachments"]
    Session["AnalysisSession<br/>analysis_id plus selected project_ids"]
    Existing["Existing JAS-MIN tool dispatcher"]
    Compare["Cross-project comparison<br/>metric and SQL distributions"]
    Guidance["GuidanceLibrary<br/>reasonings.txt"]
    Report["Report state and renderer"]
    HtmlRenderer["Classic AI Markdown renderer<br/>TOC, CSS, report links"]
    WorkingDir["JAS-MIN working directory<br/>new .html file"]

    Client -->|"POST initialize"| Endpoint
    Endpoint --> Transport
    Transport --> Handler
    Handler -->|"ServerInfo, capabilities, instructions,<br/>Mcp-Session-Id"| Client

    Client -->|"POST notifications/initialized"| Endpoint
    Client -->|"POST tools/list or prompts/list"| Endpoint
    Handler -->|"Dynamic evidence tools plus<br/>MCP workflow tools"| Client

    Client -->|"tools/call: list_performance_projects"| Handler
    Handler --> Runtime
    Runtime -->|"Project IDs, ranges, database identity,<br/>sample and attachment counts"| Client

    Client -->|"tools/call: start_performance_analysis<br/>with selected project_ids"| Handler
    Handler --> Runtime
    Runtime -->|"Create analysis_id and SEED-E0001"| Session
    Runtime --> Projects
    Session -->|"Per-project manifests and seeds,<br/>calculation catalog, quality gates, report contract"| Client

    Client -->|"Project evidence call<br/>with analysis_id and project_id"| Handler
    Handler --> Projects
    Handler -->|"Measurement request"| Existing
    Existing -->|"Structured result"| Runtime
    Runtime -->|"Cache result and assign E-nnnn"| Session
    Session -->|"Evidence result"| Client

    Client -->|"compare_project_metric or compare_project_sql<br/>with baseline and candidate project IDs"| Handler
    Handler --> Compare
    Projects --> Compare
    Compare -->|"Normalized distributions, coverage,<br/>deltas, effect size, guarded classification"| Session
    Session -->|"Comparative E-nnnn evidence"| Client

    Handler -->|"Guidance request"| Guidance
    Guidance -->|"GUIDE-section references;<br/>methodology only"| Session
    Session -->|"Guidance result"| Client

    Client -->|"configure_report, record_finding,<br/>record_report_table, set_report_assessment"| Handler
    Handler --> Report
    Report --> Session
    Client -->|"get_report_status"| Handler
    Handler -->|"Coverage and missing requirements"| Client
    Client -->|"finalize_report"| Handler
    Report -->|"Stable Markdown and/or JSON"| Client
    Client -->|"If HTML requested:<br/>convert_markdown_to_html"| Handler
    Handler --> HtmlRenderer
    HtmlRenderer -->|"Validated 11-section report"| WorkingDir
    WorkingDir -->|"Output path and byte counts"| Client

    Client -->|"HTTP DELETE with Mcp-Session-Id"| Endpoint
    Transport -->|"Close transport session"| Client
```

An evidence tool request is processed in this order:

1. The Streamable HTTP layer validates and routes the JSON-RPC request.
2. `JasminMcpServer` resolves the tool and passes its arguments to
   `AnalysisRuntime`.
3. The runtime requires a non-empty `analysis_id`, locates the corresponding
   `AnalysisSession`, and verifies that the requested `project_id` belongs to
   that analysis. `project_id` may be omitted only when the analysis contains
   one project.
4. The runtime removes `analysis_id` and `project_id` before calling the existing JAS-MIN
   dispatcher, so MCP-specific state never changes the legacy tool contract.
5. A tool-native error is returned immediately and is not registered as
   evidence.
6. Successful arguments are canonicalized. Object key order therefore does not
   affect cache identity.
7. An identical successful call in the same analysis reuses its existing
   evidence record; otherwise the result receives the next `E-nnnn` identifier.
8. The result is returned as MCP structured content together with its
   `analysis_id`, `project_id`, `evidence_id`, tool name, and cache status.

Comparison tools resolve both project IDs inside the same analysis, calculate
bounded distribution summaries, and register the complete comparison as one
evidence record. Missing samples remain missing rather than becoming zeros.

## Transport lifecycle

The endpoint uses MCP over Streamable HTTP. Clients negotiating a legacy
version such as `2025-06-18` use the stateful lifecycle below; modern
`2026-07-28` clients use self-contained request metadata and standard MCP HTTP
headers. A client library normally handles either lifecycle automatically.

Legacy stateful lifecycle:

1. Send `initialize` with the client name, version, capabilities, and supported
   protocol version.
2. Read the negotiated server information and `Mcp-Session-Id` response header.
3. Send `notifications/initialized` with that session header.
4. Discover `tools/list` and, if useful, `prompts/list` or `prompts/get`.
5. Call `list_performance_projects` when the server may contain several inputs.
6. Call `start_performance_analysis` with one `project_id`, several
   `project_ids`, or no selection to include every loaded project.
7. Use the returned `analysis_id` in every later JAS-MIN tool call and include
   `project_id` in project-specific calls when the analysis is comparative.
8. End the transport session with HTTP `DELETE` and the
   `Mcp-Session-Id` header.

### MCP 2026-07-28 cache hints

When a client negotiates protocol version `2026-07-28` or newer,
`tools/list` includes the cache fields required by that protocol revision:

```json
{
  "resultType": "complete",
  "ttlMs": 300000,
  "cacheScope": "private",
  "tools": []
}
```

The five-minute TTL is a freshness hint, not server-side expiration. The tool
catalog is immutable for the lifetime of a JAS-MIN process, but `private`
prevents a client from sharing a cached catalog across users or authorization
contexts. Sessions negotiated with an older MCP version omit both cache fields
to retain the legacy response shape.

A manual `2026-07-28` `tools/list` request must carry the negotiated version in
both `_meta` and `MCP-Protocol-Version`, as well as the routing header
`Mcp-Method: tools/list`. These requirements come from the modern transport
contract and are separate from the cache hints returned by JAS-MIN.

Minimal initialization request:

```bash
curl -i -X POST http://127.0.0.1:4242/mcp \
  -H 'Accept: application/json, text/event-stream' \
  -H 'Content-Type: application/json' \
  --data '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2025-06-18",
      "capabilities": {},
      "clientInfo": {"name": "manual-test", "version": "1.0"}
    }
  }'
```

Use the version negotiated in the response rather than assuming that the
example version will remain current.

After initialization, include the returned session header in every transport
request:

```text
Mcp-Session-Id: <transport-session-id>
```

Close the transport session when it is no longer needed:

```bash
curl -i -X DELETE http://127.0.0.1:4242/mcp \
  -H 'Mcp-Session-Id: <transport-session-id>'
```

### Transport session versus analysis session

`Mcp-Session-Id`, `project_id`, and `analysis_id` are deliberately different identifiers.

| Identifier | Created by | Scope | Lifetime |
|---|---|---|---|
| `Mcp-Session-Id` | MCP Streamable HTTP transport | Protocol negotiation and HTTP transport state. | Until HTTP `DELETE`, transport expiry, or process exit. |
| `project_id` | JAS-MIN startup registry | Routes evidence to one immutable parsed collection. | Until JAS-MIN exits. |
| `analysis_id` | `start_performance_analysis` | Evidence, guidance references, report configuration, findings, assessments, and revisions. | Until JAS-MIN exits. |

Deleting an MCP transport session does not currently delete its JAS-MIN
analysis. Analysis sessions are process-local, have no persistence, and are not
restored after restart. Because the runtime is shared, an `analysis_id` can be
used from another valid local MCP transport session while the process is still
running. Treat both identifiers as sensitive local handles, not as
authentication credentials.

## Server capabilities and onboarding

The server advertises MCP `tools` and `prompts` capabilities. Model onboarding
is distributed across four mechanisms:

1. **Server instructions** define mandatory evidence discipline and report
   completion rules.
2. **`oracle_performance_analysis` prompt** provides a reusable tool-first user
   message with optional `language` and `focus` arguments.
3. **`list_performance_projects` tool** exposes stable project handles and
   compact manifests before an analysis is created.
4. **`start_performance_analysis` tool** creates the analysis session and
   returns project-specific maps of available calculations and evidence.

This avoids placing the entire `ReportForAI`, raw snapshots, attachments, and
`reasonings.txt` into an initial prompt. The model learns what can be calculated
immediately, but retrieves the expensive detail only when a hypothesis requires
it.

### Optional MCP prompt

The prompt can be requested by name:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "prompts/get",
  "params": {
    "name": "oracle_performance_analysis",
    "arguments": {
      "language": "EN",
      "focus": "DB Time degradation and cursor contention"
    }
  }
}
```

The prompt is guidance for the model host. It does not create an analysis
session and does not replace `start_performance_analysis`.

## Analysis bootstrap

`list_performance_projects` and `start_performance_analysis` do not require an
existing `analysis_id`.

Example call:

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "start_performance_analysis",
    "arguments": {
      "project_ids": ["before-upgrade", "after-upgrade"],
      "focus": "Investigate DB Time degradation and cursor contention",
      "language": "EN",
      "audience": "mixed"
    }
  }
}
```

The returned structured payload contains:

| Field | Meaning |
|---|---|
| `schema_version` | Version of the JAS-MIN MCP analysis contract. |
| `analysis_id` | Required handle for every later tool call. |
| `project_ids` | Ordered set of projects selected for the analysis. |
| `comparison_mode` | `true` when the analysis contains more than one project. |
| `seed_evidence_id` | `SEED-E0001`, representing the bounded initial case seed. |
| `projects` | Per-project manifest, bounded seed, triage preview, quality gates, and opening calls. |
| `dataset_manifest` | Backward-compatible single-project manifest; present only for one-project analyses. |
| `available_calculations` | Statistical methods, access tools, outputs, and interpretation caveats. |
| `case_seed` | Bounded high-signal summary derived from `ReportForAI`. |
| `triage_preview` | Small wait, SQL, I/O, latch, anomaly, and parameter indexes. |
| `diagnostic_guidance` | Catalog of indexed `reasonings.txt` sections, not their full contents. |
| `quality_gates` | Dataset- and platform-aware proof requirements. |
| `report_contract` | Stable sections, required finding categories, structured-table schemas, artifact coverage, parameter checklist, assessments, and current output configuration. |
| `recommended_next_calls` | Dataset-aware opening calls, including attachment tools when relevant non-empty files exist. |
| `recommended_comparison_calls` | Opening cross-project calls when comparison mode is active. |

The bootstrap is intentionally bounded. It is a routing map, not a complete
performance report.

## Available statistical calculations

The bootstrap catalog describes both the calculation and the tool needed to
retrieve it:

- descriptive mean, median, standard deviation, occurrence percentage, and
  percentiles;
- DB CPU / DB Time workload composition;
- global and sliding-window median absolute deviation anomalies;
- temporal anomaly clusters;
- Pearson correlations;
- Ridge, Elastic Net, Huber, and Quantile-95 gradients;
- VIF and collinear-group diagnostics;
- robust baseline-versus-recent DB Time degradation;
- metric, wait-event, and SQL timelines;
- peak-versus-baseline snapshot comparison;
- wait-event latency histograms.

These values rank and shape hypotheses. They do not prove causation. In
particular:

- correlation requires temporal alignment and independent verification;
- gradient importance can be unstable for near-zero baselines or collinear
  predictors;
- an anomaly is a deviation from a baseline, not automatically a bottleneck;
- DB CPU / DB Time describes workload composition but cannot independently
  prove or dismiss AIX LPAR CPU pressure.

## Tool catalog construction

At startup, JAS-MIN takes the union of the existing OpenAI/OpenRouter function
schemas available across all loaded projects and converts it to MCP tools. Each
converted evidence schema receives a required `analysis_id` and an optional
`project_id`. The runtime requires `project_id` when the analysis contains more
than one project. The adapter also supplies a permissive object output schema and MCP
annotations:

- evidence and catalog reads are marked read-only and idempotent where
  appropriate;
- report mutations and analysis creation are not marked read-only;
- no tool is marked destructive;
- tools are marked closed-world because they operate only on the parsed
  collection, its local attachments, in-memory report state, and explicitly
  named new HTML files in the JAS-MIN working directory.

The core evidence catalog is always available. An attachment tool is registered
when matching files are discovered under any project's `<stem>_attachments`.
Calling it for another project returns that tool's ordinary unavailable result.

With no attachments the server exposes 21 core evidence tools and fifteen MCP
workflow tools. Every supported attachment class adds its own discovery or
inspection tools; a dataset containing plans, child-cursor reasons, an alert
log, and AIX telemetry exposes eight additional evidence tools.

### Core evidence tools

| Area | Tools |
|---|---|
| Global and point lookups | `get_database_load_summary`, `get_snapshot_details`, `get_sql_text`, `get_init_parameter`, `get_db_instance_info` |
| Snapshot aggregations | `list_snapshots`, `top_sqls_in_snapshot`, `top_wait_events_in_snapshot`, `top_segments_in_snapshot`, `top_latches_in_snapshot` |
| Search | `search_sql_text`, `find_sqls_touching_object`, `find_snapshots_with_event`, `find_snapshots_with_sql`, `find_sqls_by_module` |
| Timelines and comparison | `get_metric_time_series`, `get_wait_event_timeline`, `get_sql_timeline`, `compare_snapshots`, `get_wait_event_histogram` |
| Discovery | `list_available_metrics` |

### Conditional attachment tools

| Attachment | Discovery condition | Tools |
|---|---|---|
| SQL execution plans | At least one `*.xplan` file | `list_available_sql_plans`, `get_sql_execution_plan` |
| Child cursor reasons | At least one `.shared_cursor_reasons` file | `list_available_child_cursor_reasons`, `get_child_cursor_reasons` |
| Oracle alert log | At least one alert-log candidate | `get_alertlog_errors` |
| AIX telemetry | At least one supported file under `AIX/` | `list_aix_os_attachments`, `get_aix_os_attachment`, `get_aix_cpu_entitlement_summary` |

`list_available_sql_plans` reads the complete attachment and returns plan
instance counts, unique plan hashes, and per-hash counts. Supplying one of those
hashes to `get_sql_execution_plan` returns a representative complete block for
that variant, avoiding the whole-file byte truncation that can hide later child
plans in large attachments.

### MCP workflow tools

| Tool | State effect |
|---|---|
| `list_performance_projects` | Lists immutable project manifests without creating an analysis. |
| `start_performance_analysis` | Creates an `AnalysisSession` and its seed evidence. |
| `get_analysis_catalog` | Repeats selected project manifests, calculation catalog, guidance catalog, and report contract. |
| `get_precomputed_analysis` | Registers a requested `ReportForAI` section as evidence. |
| `get_wait_event_sql_contributors` | Registers correlation and direct-ASH SQL relationships for one material foreground wait, including plan applicability and attachment coverage. |
| `get_diagnostic_guidance` | Registers methodology references, never evidence IDs. |
| `compare_project_metric` | Registers a normalized metric-distribution comparison as evidence. |
| `compare_project_sql` | Registers a same-SQL cross-project comparison as evidence. |
| `configure_report` | Updates report presentation settings. |
| `record_finding` | Creates or replaces an evidence-backed finding. |
| `record_report_table` | Creates or replaces a provenance-validated structured analysis table. |
| `set_report_assessment` | Stores one mandatory final assessment. |
| `get_report_status` | Validates report coverage without rendering it. |
| `finalize_report` | Validates and renders Markdown, JSON, or both. |
| `convert_markdown_to_html` | Validates finalized Markdown and creates a new HTML file in the working directory using the classic AI renderer. |

## Evidence registry

Every successful measurement tool call is wrapped in an evidence envelope:

```json
{
  "schema_version": "2026-08-23.4",
  "analysis_id": "A-20260804T100000Z-0001",
  "project_id": "before-upgrade",
  "evidence_id": "E-0002",
  "tool_name": "get_database_load_summary",
  "cached": false,
  "result": {
    "...": "tool-specific structured data"
  }
}
```

`SEED-E0001` is created with the analysis bootstrap. Later successful evidence
calls use `E-0002`, `E-0003`, and so on.

Evidence identity is scoped to one `analysis_id`. A finding cannot cite an
evidence identifier from another analysis. Repeating the same tool with
canonically identical arguments reuses the existing identifier and returns
`"cached": true`.

The cache is an evidence registry, not a general response cache. The underlying
tool may still execute before a concurrent duplicate call observes the stored
record, but only one canonical evidence reference is retained for the session.

### Cross-project comparisons

`compare_project_metric` accepts a baseline project, a candidate project, the
same `kind`/`name`/`field` selectors as `get_metric_time_series`, a direction,
and a materiality threshold. It returns observed sample counts, mean, median,
p95, minimum, maximum, standard deviation, mean/median/p95 deltas, relative
mean change, and standardized mean difference.

Direction defaults to `neutral`. JAS-MIN assigns `improved`, `degraded`, or
`no_material_change` only when the caller explicitly selects
`lower_is_better` or `higher_is_better`. This prevents workload counters from
being mislabeled as regressions merely because business volume increased.

`compare_project_sql` compares the same SQL ID across two projects. It treats
elapsed, CPU, I/O, buffer gets, and physical reads per execution as efficiency
metrics, while totals and execution counts remain neutral workload-volume
metrics. The response also reports snapshot coverage, modules, SQL-text
availability, and plan hashes observed in top-event rows.

Neither tool turns missing observations into zero. An absent metric or SQL ID
therefore produces an explicit coverage error or unavailable submetric. The
model must still verify database identity, workload mix, snapshot duration,
seasonality, plans, and relevant host conditions before attributing a change to
an application, parameter, or infrastructure intervention.

## Diagnostic guidance

JAS-MIN resolves `reasonings.txt` in this order:

1. `$JASMIN_HOME/reasonings.txt`;
2. `./reasonings.txt`.

The file is parsed into independently addressable sections. The bootstrap
returns only a catalog. `get_diagnostic_guidance` accepts either an exact
section such as `§1.1` or a concrete symptom and returns at most the requested
bounded number of matching sections.

Returned references use the form `GUIDE-§1.1`. They are recorded separately
from evidence and the response explicitly declares `methodology_only: true`.
This distinction is enforced when a finding or assessment is stored:

- `evidence_refs` must identify measurement results from the same analysis;
- `guidance_refs` must identify sections actually retrieved in that analysis;
- guidance can explain a diagnostic rule but cannot prove that its trigger is
  present in the dataset.

Symptom matching is token-based. For example, a query about `log file sync`
must not be routed to unrelated `LOGON STORMS` guidance merely because both
contain a similar character sequence.

## Recommended investigation workflow

A robust investigation normally follows this order:

1. Call `list_performance_projects` and verify project identity, time ranges,
   sample counts, and attachment coverage. For alert logs, compare the observed
   first/last timestamp and `coverage_status` with the dataset dates rather than
   inferring coverage from the filename or enclosing AWR period.
2. Call `start_performance_analysis` with the intended `project_ids`; preserve
   which project is the baseline and which is the candidate in every finding.
3. Establish each whole-window workload envelope with
   `get_database_load_summary(project_id=...)`.
4. In comparative work, use `compare_project_metric` for normalized load,
   latency, wait, CPU, and I/O distributions. Use a neutral direction until the
   semantic meaning of higher or lower values is justified.
5. Retrieve every section listed in `required_precomputed_sections` for each
   project through `get_precomputed_analysis`.
6. Use `list_snapshots` to select representative peak, neighboring, and quiet
   baseline snapshots.
7. For SQL IDs material to either period, call `compare_project_sql`, then
   inspect each project's SQL timeline and text. Inventory and inspect every
   supplied execution plan and child-cursor attachment.
8. Form competing hypotheses instead of committing to the first correlated
   metric.
9. Verify or falsify them with narrow wait, SQL, timeline, histogram, snapshot,
   plan, child-cursor, alert-log, parameter, segment, latch, and OS calls.
10. Retrieve only the `reasonings.txt` sections relevant to symptoms already
   observed.
11. Store findings with their evidence and optional guidance references, then
    record all required structured tables and rows.
12. Complete every mandatory assessment, using `unknown` when required data is
   unavailable.
13. Call `get_report_status`, resolve missing coverage, and then call
    `finalize_report`.

### Platform-aware quality gates

The bootstrap returns rules the model must satisfy before making high-impact
claims:

- **AIX CPU pressure:** inspect entitlement, physical CPU consumption,
  capped/shared mode, and aligned timestamps. AWR host CPU alone is
  insufficient.
- **Disk quality:** separate storage service latency from the volume of I/O
  requests. Inspect LGWR, DBWR, buffer-cache, and direct-I/O evidence.
- **Application and commit policy:** do not infer bad design from executions or
  waits alone. Verify transaction, redo, latency, and direct anti-pattern
  evidence.
- **SQL tuning:** inspect SQL text, its timeline, and available execution plans
  before recommending access-path or join changes.
- **Cursor contention:** inspect decoded child-cursor reasons together with
  parse, reload, invalidation, library-cache, or mutex evidence.
- **Parameter changes:** record the observed current value and a causal
  rationale. Missing parameters remain unknown.

### AIX date alignment

When the platform is AIX, the bootstrap derives `date_from` and `date_to` from
the first and last Oracle snapshots and recommends those arguments for
`get_aix_cpu_entitlement_summary`.

If either date filter is active, observations without a parseable date are
excluded. This prevents a valid CPU statistic from an unrelated OS collection
period from contaminating the Oracle performance interval. The returned result
reports parsed, excluded, and filtered observation counts so the model can
evaluate coverage before drawing a CPU conclusion.

## Report state machine

The report workflow is stateful but intentionally simple:

```mermaid
stateDiagram-v2
    [*] --> Started: start_performance_analysis
    Started --> Investigating: evidence and guidance calls
    Investigating --> Configured: configure_report
    Configured --> Investigating: further evidence
    Investigating --> Findings: record_finding
    Findings --> Findings: create or replace findings
    Findings --> Tables: record_report_table
    Tables --> Tables: create or replace structured tables
    Tables --> Assessed: set_report_assessment
    Assessed --> Assessed: complete or revise assessments
    Assessed --> Checked: get_report_status
    Checked --> Investigating: missing coverage
    Checked --> Finalized: ready_to_finalize = true
    Checked --> Draft: allow_incomplete = true
    Finalized --> HTML: convert_markdown_to_html
    Draft --> HTML: convert_markdown_to_html
    Finalized --> Finalized: finalize again creates next revision
    Draft --> Investigating: complete missing work
```

The states are conceptual; the implementation stores independent collections
of evidence, findings, structured tables, and assessments rather than a single
enum. Evidence can
be gathered and report configuration can be changed at any point after the
analysis starts.

## Configurable report contract

`configure_report` accepts:

- `output_format`: `markdown`, `json`, or `both`;
- `language`: a short report-language identifier;
- `audience`: `technical`, `management`, or `mixed`;
- `detail_level`: `compact`, `standard`, or `deep`;
- `detail_overrides`: per-category detail settings;
- `include_evidence_appendix`;
- `include_guidance_appendix`.

The default configuration is:

```json
{
  "output_format": "both",
  "language": "EN",
  "audience": "mixed",
  "detail_level": "standard",
  "detail_overrides": {},
  "include_evidence_appendix": false,
  "include_guidance_appendix": false
}
```

The server owns the stable section order:

1. Executive Summary
2. Overall Performance Profile and DB Time Degradation
3. Wait Events
4. SQL-Level Analysis
5. Segments and Objects
6. Latches and Internal Contention
7. I/O and Disk Assessment
8. UNDO, Redo and Load Profile
9. Gradient and Anomaly Synthesis
10. Relevant Initialization Parameters
11. Prioritized Actions and Mandatory Assessments

Core sections cannot be removed. A non-draft report requires an evidence-backed
finding in every analytical section, so `finalize_report` can no longer publish
an empty gradient, segment, latch, UNDO/redo, or parameter section. The explicit
no-finding text is retained only for a deliberately incomplete draft created
with `allow_incomplete=true`.

### Deterministic completeness gate

`get_report_status` is the authoritative completion checklist. In addition to
all nine analytical finding categories and the five mandatory assessments, it
requires, for every selected project:

- the database load summary and the mandatory precomputed sections for DB Time
  degradation, foreground/background waits, top SQL, segments, latches, I/O,
  gradients, load-profile anomalies, and anomaly clusters;
- inventory plus classification of every supplied `*.xplan` artifact. SQL
  statements require inspection of every unique plan hash through its
  representative complete plan block; PL/SQL `BEGIN`/`DECLARE`/`CALL` entry
  points are recorded as `not_applicable_plsql` and require profiling plus
  inner-SQL analysis instead of top-level plan recapture;
- inventory plus inspection of every supplied
  `*.shared_cursor_reasons` file;
- one structured row for every project/SQL ID/plan-hash variant (or explicit
  unusable plan attachment). A plan-change recommendation also cites observed
  SQL timeline, top-SQL, or project-comparison evidence and states why the SQL
  was selected, measured impact, time coverage, comparison context, evidence
  limitations, concrete action, and a measurable success criterion;
- one `get_wait_event_sql_contributors` call and structured wait-to-SQL table
  for every foreground wait reaching 10% DB Time. The five strongest returned
  associations preserve aligned Pearson correlation and direct ASH attribution
  separately. The strongest material contributor requires SQL text, timeline,
  and explicit plan coverage; missing plan evidence is reported as
  `missing_attachment`, not silently omitted;
- a full `get_alertlog_errors` call with `include_parse_error_details=true`, one
  row for every deterministic `error_summary` code, and an SQL-level finding
  citing the alert evidence whenever parse errors exist;
- one structured row for every object in every non-empty precomputed segment
  category;
- separate gradient, anomaly, and anomaly-cluster rows whenever those families
  contain data. The gate requires the five highest-impact cross-model rows in
  each gradient family and every foreground wait reaching 10% DB Time; a
  material wait omitted by the models must be recorded explicitly as
  `material_not_selected`. Cross-signal synthesis is required when at least two
  analytic families exist;
- an exact review row for every collected parameter value. Uncollected values
  do not become report rows.

The response exposes `missing_required_evidence`,
`missing_structured_table_kinds`, and `missing_structured_table_rows`.
`ready_to_finalize` remains false until all three collections are empty.

### Structured analysis tables

`record_report_table` creates or replaces a block of kind `gradients`,
`anomalies`, `anomaly_clusters`, `analytic_signal_synthesis`,
`execution_plans`, `child_cursors`, `alert_log_errors`, `segments`,
`segment_synthesis`, or `parameters`. `get_analysis_catalog` returns the exact
columns and enumerated values. Every row has `evidence_refs`; the server
verifies project/entity identity against the cited result. Analytic synthesis
must cite every available analytic family and provide target metrics, at least
three exact top-five gradient contributors from two target families, concrete
model agreement and classification, exact anomaly/cluster localization,
counterevidence, a mechanism-level conclusion, and a runtime validation. The
gate rejects generic prose that merely says independent detectors converge on
CPU, DB-time, I/O, RAC, or other activity. Plan rows must select one of the
contract recommendation types; generic “validate actual rows” text and an
actionable plan recommendation supported only by xplan evidence are rejected.
Alert rows reproduce exact aggregate counts and time bounds. Parameter rows
reproduce collected values.

The renderer uses deterministic, DBA-oriented projections rather than exposing
the storage schema verbatim. It removes the repeated `Project` column and groups
rows under readable instance labels, merges tables of the same kind so the TOC
contains one semantic entry rather than repeated generic headings, splits
segment details into one table per statistic, renders gradient families under
named subheadings, and shows only `concern`/`critical` parameter rows. Plan rows
begin with workload relevance and limitations before the interactive row-source
graph; plans longer than 30 operations open on flagged paths. `no_change`
variants remain in a compact coverage disclosure. SQL IDs and wait events link
directly to each project-specific JAS-MIN detail page, and HTML export rejects
missing local link targets.

### Findings

`record_finding` requires:

- a report category;
- title, severity, confidence, and conclusion;
- a diagnostic `mechanism` connecting the measured symptom to the working
  explanation without presenting correlation as causality;
- a `temporal_pattern` with peak windows, recurrence, baseline direction, and
  episodic-versus-sustained behavior;
- an `affected_workload` naming the supported SQL IDs, PL/SQL units, modules,
  services, objects, or business paths, or stating explicitly that attribution
  is unavailable;
- `evidence_limitations` containing counterevidence, the precise causal
  boundary, and the runtime proof still required;
- a human-readable `evidence_summary` containing the exact supporting values,
  time scope, and project or instance context;
- an `evidence_refs` array;
- optional detail, guidance references, verified guidance quotations, and
  prioritized recommendations.

Evidence identifiers are machine provenance, not reader-facing prose. The
Markdown renderer presents `evidence_summary` in the finding and keeps raw IDs
out of the human report unless the optional technical evidence appendix is
explicitly enabled. That appendix lists only cited records, gives each tool a
human-readable label and scope, and links provenance references to their
entries.

For comparative reports, every value in reader-facing prose must name its
project or instance; an unlabeled `X/Y` pair is not accepted as human-readable
evidence. Attachment inventory exposes alert-log byte counts, empty/non-empty
totals, observed first/last ISO timestamps, timestamp-line counts, and coverage
status relative to the dataset dates. Zero-byte attachments are missing
collection coverage; partial attachments must be scoped to their observed
interval and must not be presented as full-period totals. A zero-match literal
proves only the exact submitted search/filter, so inspect raw context and known
punctuation/message variants before declaring an event absent.

When `guidance_refs` is non-empty, `guidance_quotes` must contain one contiguous
verbatim excerpt for every referenced section. JAS-MIN validates each excerpt
against the guidance text retrieved in the same analysis. A paraphrase,
invented rule, missing quote, or quote for an unreferenced section is rejected.

The server validates every supplied reference. Analytical findings are rejected
when `evidence_refs` is empty; only an explicit `limitations` finding may use an
empty array. Non-unknown mandatory assessments are also rejected when their
evidence array is empty.

Severity is one of `critical`, `high`, `medium`, `low`, or `informational`.
Confidence is one of `high`, `medium`, `low`, or `unknown`.

A returned `finding_id` can be passed back to replace that finding after deeper
analysis. If the supplied identifier does not refer to an existing finding, the
server creates a new identifier instead of overwriting arbitrary state.

Recommendations are structured as an owner (`DBA`, `Developer`, or
`Management`), a priority (`immediate`, `high`, `medium`, or `low`), an action,
the evidence-backed `rationale` for that action and priority, and a measurable
`success_criterion` including a regression guard. The renderer groups actions
by accountable owner rather than emitting one flat list.

Accepted finding categories are `performance_profile`, `wait_events`, `sql`,
`segments`, `latches`, `io`, `undo_redo`, `gradients_anomalies`, `parameters`,
and `limitations`. The structured JSON retains all categories. The numbered
Markdown body maps the first nine analytical categories to sections 2 through
10; a high-severity `limitations` finding can appear in the executive summary
but currently has no dedicated numbered section.

The deterministic Markdown renderer leads every section with diagnostic
findings and follows them with the exhaustive structured evidence tables. The
executive summary repeats the mechanism, affected workload, temporal pattern,
and evidence boundary for the five leading findings; a one-sentence register
alone is not considered a sufficient summary. `compact` omits a finding's
`details` field, `standard` keeps it in a disclosure below the diagnostic
synthesis, and `deep` renders it inline.

### Mandatory assessments

The final report must explicitly cover:

- `disk_quality`;
- `application_design`;
- `commit_policy`;
- `cpu_pressure`;
- `parameter_hygiene`.

Assessment status is `proven`, `not_proven`, or `unknown`. A `proven` or
`not_proven` assessment must cite at least one measurement evidence reference.
Every assessment also supplies a human-readable `evidence_summary`. An
`unknown` assessment records an honest limitation and identifies the precise
missing source data.

### Completion rules

`get_report_status` declares the report ready only when all of the following are
true:

- at least one finding exists;
- findings cover all nine analytical categories: `performance_profile`,
  `wait_events`, `sql`, `segments`, `latches`, `io`, `undo_redo`,
  `gradients_anomalies`, and `parameters`;
- all five mandatory assessments exist;
- every required evidence call, structured table kind, and artifact/entity row
  reported by the deterministic completeness gate is present;
- at least one structured recommendation action exists.

The status response lists present and missing categories, completed and missing
assessments, missing evidence, missing table kinds and rows, evidence and
guidance counts, action count, and the recommended next step.

`finalize_report` rejects an incomplete report with `REPORT_INCOMPLETE` unless
`allow_incomplete: true` is explicitly supplied. An incomplete rendering is
marked as a draft. Each successful finalization increments the report revision.
Any later evidence, guidance, configuration, finding, table, or assessment
change invalidates the stored Markdown until the model finalizes a new revision.

## HTML export

HTML is deliberately a second step rather than another `output_format` value.
This preserves the auditable report contract: the model first produces and
validates the canonical Markdown report, then passes that exact document to the
classic JAS-MIN HTML renderer.

When the user requests HTML, the model should:

1. call `configure_report` with `output_format` set to `markdown` or `both`;
2. complete the evidence, findings, mandatory assessments, and actions;
3. call `get_report_status` and resolve missing requirements;
4. call `finalize_report`;
5. copy the returned `markdown` value unchanged into
   `convert_markdown_to_html`;
6. report the returned `output_path` to the user.

Example tool arguments:

```json
{
  "analysis_id": "A-20260805T100000Z-0001",
  "markdown": "# Oracle Performance Analysis\n\n## 1. Executive Summary\n...",
  "output_filename": "oracle-performance-report.html"
}
```

The conversion tool enforces the following policy:

- `markdown` is required and limited to 4 MiB;
- `markdown` must exactly equal the latest `finalize_report` output for the same
  analysis session; edited or never-finalized Markdown is rejected;
- the exact `# Oracle Performance Analysis` title must be present;
- all 11 stable `##` headings must be present in server-defined order;
- unresolved navigation placeholders such as `{load_profile}` or
  `{jasmin_main}` are rejected;
- `output_filename` is an optional basename, never a path;
- `/`, `\\`, hidden names, control characters, and non-HTML extensions are
  rejected;
- `.html` is appended if no extension is supplied;
- the destination is always the current JAS-MIN working directory;
- the file is created with create-new semantics and an existing file is never
  overwritten;
- no browser or desktop application is opened by the server.

When no filename is supplied, the server derives one from the dataset name and
`analysis_id`, making collisions between independent analyses unlikely. A
multi-project analysis uses `jas-min-comparison` as the dataset portion of this
default name.

The renderer is shared with classic AI mode. It generates a complete HTML5
document with a mandatory responsive ORA-600-aligned audit layout: a
self-contained vector JAS-MIN wordmark, white/black/red palette, sticky report
navigation, severity-marked findings, action cards, and a print stylesheet.
Every table is wrapped in an always-active horizontal overflow region with a
visible scrollbar, sticky headers, and keyboard focus. Column headers are
keyboard-accessible sort buttons with stable numeric, percentage, unit, Oracle
timestamp, and text ordering. Gradient families initially sort by descending
peak impact, while the reader may re-sort any rendered table. Actionable execution
plans use an interactive dependency view with flagged-path filtering,
expand/collapse controls, zoom, and horizontal panning. It also provides
anchored headings, an embedded ORA-600 logo, links to the main HTML
report, and links to SQL/event pages when the relevant
classic report assets and link index are available. Material wait-event names
and SQL IDs in reader-facing findings must be direct links to every existing
project-specific detail report; in comparative output, each target must name
its instance or project rather than using a generic link label. The response reports every
linked report directory and whether its main report exists. A custom output
filename does not change those link targets. Single-project exports retain the
classic navigation as verified active links to existing dashboard/load-profile
files. Comparative exports publish explicit active links to each selected
project's main dashboard and load-profile reports. Neither path emits unresolved
iframe placeholders.

The HTML file is an output artifact, not an evidence record and not part of the
in-memory report revision. Repeating the call with the same filename returns
`OUTPUT_EXISTS`; choose another basename rather than silently replacing an
auditable result.

## Error model

MCP protocol errors and JAS-MIN tool errors have different roles:

- malformed protocol requests or an unknown MCP prompt use JSON-RPC/MCP error
  handling;
- tool validation and analysis errors are returned as an MCP tool result marked
  as an error, with structured diagnostic content;
- failed evidence tools do not receive an evidence identifier and cannot be
  cited by findings.

Common structured error codes include:

| Code | Cause | Recovery |
|---|---|---|
| `MISSING_ANALYSIS_ID` | A non-bootstrap tool was called without `analysis_id`. | Call `start_performance_analysis` and pass its handle. |
| `UNKNOWN_ANALYSIS` | The handle does not exist in this process. | Start a new analysis or verify the handle. |
| `UNKNOWN_PROJECT` | A requested project is not loaded. | Call `list_performance_projects` and use a returned ID. |
| `MISSING_PROJECT_ID` | A project-specific tool was called in a multi-project analysis without routing information. | Pass one selected `project_id`. |
| `PROJECT_OUTSIDE_ANALYSIS` | The project is loaded but was not selected for this analysis. | Start an analysis containing it or use one of the analysis project IDs. |
| `MISSING_COMPARISON_DATA` | One side has no observed samples for the requested metric. | Verify the exact metric name/field and do not substitute missing values with zero. |
| `INVALID_COMPARISON_FIELD` | The field is not defined for the selected metric kind. | Use a field published for that kind; comparison calls do not silently fall back to another field. |
| `MISSING_SQL_COMPARISON_DATA` | The SQL ID is not observed in both projects. | Compare workload catalogs first or state that the SQL appeared/disappeared without fabricating zero cost. |
| `SESSION_LOCK` | The per-analysis state lock was poisoned. | Restart the server; in-memory state cannot be trusted. |
| `UNKNOWN_EVIDENCE_REF` | A finding cited evidence not registered in this analysis. | Use an `evidence_id` returned by a successful call in the same analysis. |
| `UNKNOWN_GUIDANCE_REF` | A finding cited guidance not retrieved in this analysis. | Call `get_diagnostic_guidance` first. |
| `ASSESSMENT_WITHOUT_EVIDENCE` | A non-unknown assessment cited no measurements. | Add evidence or change the status to `unknown`. |
| `FINDING_WITHOUT_EVIDENCE` | An analytical finding cited no measurements. | Add session evidence; only a `limitations` finding may omit it. |
| `REPORT_TABLE_EVIDENCE_MISMATCH` | A table row does not cite evidence for its exact project/entity. | Use the matching project, SQL ID, plan hash, segment, or parameter evidence. |
| `REPORT_INCOMPLETE` | Finalization was requested before the contract was complete. | Follow the embedded status object or request an explicit draft. |
| `REPORT_NOT_FINALIZED` | HTML conversion was requested before finalization. | Call `finalize_report` first. |
| `MARKDOWN_NOT_FINALIZED` | HTML conversion received edited or stale Markdown. | Pass the exact latest `finalize_report` Markdown. |
| `INVALID_REPORT_TITLE` | HTML conversion received Markdown without the canonical report title. | Pass the exact finalized Markdown. |
| `MISSING_REPORT_SECTIONS` | One or more stable Markdown headings are missing. | Finalize the report before conversion or restore the missing headings. |
| `INVALID_REPORT_SECTION_ORDER` | Stable sections are not in server-defined order. | Pass finalized Markdown unchanged. |
| `MARKDOWN_TOO_LARGE` | HTML conversion input exceeds 4 MiB. | Reduce optional detail or appendices before finalization. |
| `INVALID_OUTPUT_FILENAME` | The requested HTML name is unsafe or has another extension. | Supply a simple basename ending in `.html`. |
| `OUTPUT_EXISTS` | Create-new output protection found an existing file. | Select a new filename. |
| `WORKING_DIRECTORY_UNAVAILABLE` | The server cannot resolve its current directory. | Restore access to the process working directory or restart JAS-MIN there. |
| `HTML_WRITE_FAILED` | The working directory cannot create or complete the file. | Check permissions, free space, and filename. |

Individual evidence tools can also return tool-specific validation errors such
as an unknown snapshot, invalid date, unsupported metric, unavailable SQL text,
missing attachment, or unsafe attachment path.

## Concurrency and memory behavior

All MCP server instances created by the transport share one `AnalysisRuntime`.
Every parsed project's AWR data and statistical report are immutable and can be read by
multiple requests concurrently. The analysis registry is a concurrent map, and
mutations within one analysis are serialized by its mutex.

This design provides session isolation without cloning large collections for
every conversation. It also means:

- the memory cost of every loaded project is paid once at server startup;
- evidence results and model-authored report state increase memory usage for the
  lifetime of the process;
- there is currently no per-analysis expiry or deletion tool;
- there is no durable persistence or crash recovery for analysis sessions;
- two analyses over the same project set have independent evidence identifiers,
  findings, assessments, configuration, and revisions.

## Context and payload controls

The MCP interface is designed to avoid unnecessary context growth:

- the bootstrap projects only selected high-signal fields;
- verbose analytical sections are fetched separately through
  `get_precomputed_analysis`;
- list and top-N tools enforce upper limits;
- snapshot details can be restricted to named sections;
- execution plans, child-cursor data, alert logs, and raw AIX files have byte or
  record limits;
- AIX attachment scanning limits recursion depth, files, bytes per file, and
  returned sample records;
- repeated evidence calls reuse a short reference instead of creating duplicate
  report evidence;
- Markdown-to-HTML input is capped at 4 MiB and creates only one output file.

Clients should prefer narrow calls and store conclusions through
`record_finding` rather than copying every raw tool result into the conversation
history.

## Operational logging

The server writes MCP lifecycle information directly to the terminal. The
`READY` line confirms that parsing and precomputation completed and that the
HTTP listener is accepting connections. Every `tools/call` request then emits
two UTC-timestamped lines: `START` followed by `OK` or `ERROR`.

```text
2026-08-05T12:00:00.123Z [MCP] status=START call_id=7 rpc_id="42" tool="set_report_assessment" analysis_id="A-20260805T085728Z-0001" request_bytes=512 response_bytes=null duration_ms=null error_code=null
2026-08-05T12:00:00.124Z [MCP] status=OK call_id=7 rpc_id="42" tool="set_report_assessment" analysis_id="A-20260805T085728Z-0001" request_bytes=512 response_bytes=241 duration_ms=1 error_code=null
```

The fields have the following meanings:

| Field | Meaning |
|---|---|
| `call_id` | Process-local, monotonically increasing tool-call identifier. |
| `rpc_id` | JSON-RPC request identifier supplied by the client. |
| `tool` | Requested MCP tool name. |
| `analysis_id` | Analysis handle when the argument is present; otherwise `null`. |
| `request_bytes` | Serialized size of the tool argument object, excluding the JSON-RPC envelope. |
| `response_bytes` | Serialized size of the structured tool result or error. |
| `duration_ms` | Time spent in JAS-MIN tool dispatch, measured with a monotonic clock. |
| `error_code` | Stable JAS-MIN tool error code for `ERROR`; otherwise `null`. |

If execution unwinds or the request future is dropped before normal completion,
the guard emits `ABORTED` with `CALL_DID_NOT_COMPLETE`. A forced process kill
cannot emit that final line.

Logs deliberately exclude argument and response bodies. They therefore do not
copy SQL text, object names, AIX samples, findings, or complete Markdown reports
to the terminal. Client-controlled identifiers are length-bounded and JSON
escaped so that one call always occupies one physical log line.

Terminal logging is operational diagnostics, not durable audit storage. To
retain it, redirect both stdout and stderr through an operator-controlled log
collector or file with suitable access controls and rotation.

## Security model

The embedded server is designed for a trusted local workstation:

- endpoint parsing enforces a loopback IP address;
- allowed HTTP Host values are restricted to the selected loopback address and
  `localhost`, with and without the selected port;
- allowed origins are restricted to the corresponding local HTTP origins;
- absolute AIX attachment paths and path traversal are rejected;
- the service exposes no TLS, user authentication, authorization, or durable
  audit log;
- MCP and analysis session identifiers are routing handles, not security
  boundaries;
- HTML conversion cannot select a directory or overwrite an existing file, and
  it does not automatically open model-generated HTML.

The classic renderer preserves raw HTML embedded in Markdown and is not an HTML
sanitizer. Treat model-generated reports as untrusted local content: review
unexpected raw markup and open the result only in an appropriate local browser
context. Disabling automatic browser launch prevents conversion itself from
executing embedded content.

Security level 1 or 2 may expose object names, SQL text, execution plans, alert
log excerpts, and operating-system diagnostics. Select the lowest
`--security-level` that still supplies the evidence required by the
investigation.

Do not bind this implementation directly to an untrusted interface. A remote
deployment requires a separate authenticated proxy and an explicit policy for
who may inspect database and host evidence.

## Shutdown and cleanup

Pressing Ctrl-C triggers graceful Axum shutdown and cancels the Streamable HTTP
service. The listener, transport sessions, analysis sessions, evidence records,
and report state then disappear with the process.

HTML files successfully written by `convert_markdown_to_html` remain on disk in
the working directory after shutdown. They are ordinary output artifacts and
must be retained or removed by the operator.

A client should still send HTTP `DELETE` before disconnecting. This releases
transport state promptly, although it does not currently remove the associated
`analysis_id` from the runtime.

The endpoint is created only after parsing and precomputation succeed. A parser
failure therefore cannot leave a partially initialized MCP listener running.

## Troubleshooting

### The endpoint does not start

- Confirm that the input directory or JSON file exists.
- When several inputs are supplied, confirm that none resolve to the same
  canonical path and omit the ambiguous global `--outfile` option.
- Do not combine `--file` with `--mcp`.
- Confirm that the requested address is loopback and that the port is free.
- Check whether parsing or precomputed analysis failed before the bind step.

### Attachment tools are missing

- Verify that the attachment directory is named `<stem>_attachments`.
- Place AIX files under `<stem>_attachments/AIX`.
- Confirm that execution plans use the `.xplan` suffix and child-cursor files
  use `.shared_cursor_reasons`.
- Start a new MCP transport session after adding attachments. Tool registration
  occurs when its server handler is created, so an already initialized
  transport keeps its original catalog. Restarting JAS-MIN also guarantees a
  fresh catalog and attachment inventory.

### A tool reports `UNKNOWN_ANALYSIS`

The process may have restarted, the handle may belong to another JAS-MIN
instance, or the client may be using the MCP transport identifier in place of
`analysis_id`. Call `start_performance_analysis` again.

### A project-specific tool reports `MISSING_PROJECT_ID`

The analysis contains several projects. Call `get_analysis_catalog` to inspect
its selected IDs, then repeat the evidence call with exactly one `project_id`.
Do not use the baseline or candidate project ID as the `analysis_id`.

### A comparison appears to prove improvement or degradation

Treat the label as a direction-aware statistical description, not a causal
claim. Check sample counts, periods, database identity, workload mix, snapshot
duration, seasonality, SQL plan evidence, parameter changes, and aligned OS
telemetry. Use `neutral` direction for business-volume metrics where a higher
value is neither intrinsically better nor worse.

### The report cannot be finalized

Call `get_report_status` and inspect `missing_required_categories`,
`missing_assessments`, `missing_required_evidence`,
`missing_structured_table_kinds`, `missing_structured_table_rows`, and
`recommendation_actions`. Use `allow_incomplete: true` only when the requested
deliverable is explicitly a draft.

### A tool call times out or the client reports `Tool execution failed`

Match the client's JSON-RPC identifier with `rpc_id` in the terminal:

- no `START` line means the request did not reach JAS-MIN tool dispatch;
- `START` without a terminal status identifies the in-flight tool and its
  argument size;
- `ERROR` identifies a completed JAS-MIN rejection through `error_code`;
- `OK` followed by a client-side failure points to response delivery or MCP
  transport handling rather than the tool implementation;
- `ABORTED` means execution left the normal call path before a result was
  produced.

Keep the server process running while investigating an active `analysis_id`.
Analysis state is in memory and is lost on restart.

### AIX CPU statistics disagree with an earlier report

Compare `date_from`, `date_to`, parsed observation count, filtered observation
count, and excluded undated count. A summary spanning multiple OS collection
periods can be statistically correct but temporally invalid for the AWR window.

## Verification

Run the local validation suite after changing the MCP adapter, the shared tool
schemas, or report rules:

```bash
cargo fmt --check
cargo test
cargo check
cargo build
git diff --check
```

A protocol-level integration test should exercise the complete lifecycle, not
only the listening port:

1. `initialize`;
2. `notifications/initialized`;
3. `tools/list` and `prompts/list`;
4. `list_performance_projects`;
5. `start_performance_analysis` with at least two project IDs;
6. a project-specific call with and without the required `project_id`;
7. `compare_project_metric` and `compare_project_sql` with coverage checks;
8. at least one precomputed and one narrow evidence call;
9. guidance lookup when guidance is available;
10. report configuration, finding, assessment, status, and finalization calls;
11. `convert_markdown_to_html`, verification of the returned path, and a check
   that an existing output is not overwritten;
12. HTTP `DELETE` for the transport session;
13. Ctrl-C and confirmation that the listening socket has closed.

## Extending the server

To expose a new measurement tool:

1. Add its OpenAI-compatible schema to `tools_schema` in `src/ai_tools.rs`.
2. Add its structured implementation to `dispatch_tool_call_value`.
3. Keep output bounded and return structured errors instead of panicking.
4. Add unit tests for schema validation, dispatch, path handling, and output
   limits.
5. The MCP adapter will convert the schema, add `analysis_id` and optional
   `project_id`, and register
   successful results as evidence automatically.

To add an MCP-only workflow tool:

1. Add its schema to `mcp_control_definitions` in `src/mcp_server.rs`.
2. Route it in `AnalysisRuntime::call_tool`.
3. Decide whether it reads or mutates analysis state and set annotations
   consistently in `build_mcp_tools`.
4. Validate every evidence or guidance reference before storing state.
5. Update the MCP analysis schema version when the external contract changes.
6. Add report-contract and protocol tests before documenting the tool as
   available.

Keep measurement, methodology, and conclusions as separate data types. That
separation is the central invariant that makes an interactive JAS-MIN report
auditable.
