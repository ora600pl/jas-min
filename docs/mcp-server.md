# JAS-MIN MCP Server

JAS-MIN can retain a parsed Oracle performance collection and its statistical analysis in memory, then expose both through a local Streamable HTTP MCP endpoint. This mode is intended for an interactive investigation in which a model starts from a compact statistical map, drills into selected evidence, and produces a report with a stable structure.

## Start the server

Parse a directory and start MCP:

```bash
jas-min --directory ./awr_reports --security-level 2 \
  --mcp 127.0.0.1:4242/mcp
```

Reuse an existing parsed collection:

```bash
jas-min --json-file ./awr_reports.json --security-level 2 \
  --mcp 127.0.0.1:4242/mcp
```

JAS-MIN completes parsing and all precomputed analysis before it starts listening. The process then retains the full `AWRSCollection`, the compact `ReportForAI`, attachment locations, diagnostic guidance, evidence stores, and report sessions in memory until Ctrl-C.

`--mcp` currently accepts loopback addresses only. This is intentional: security level 1 or 2 can expose object names, SQL text, execution plans, alert log excerpts, and operating-system diagnostics. Use a trusted local proxy if remote access and authentication are required.

The minimum supported Rust version for this feature is 1.88.

## Model onboarding

The server uses three complementary MCP mechanisms instead of placing the complete dataset and the complete `reasonings.txt` in one large prompt:

1. Server instructions define non-negotiable analysis rules, evidence discipline, and the report completion workflow.
2. The optional `oracle_performance_analysis` MCP prompt gives a client a reusable user-message template with `language` and `focus` arguments.
3. `start_performance_analysis` is the mandatory first tool call. It creates an explicit `analysis_id` and returns the statistical catalog, dataset manifest, high-signal case seed, attachment inventory, platform-specific quality gates, report contract, and recommended next calls.

This bootstrap tells the model what JAS-MIN has already calculated before the model spends calls rediscovering capabilities. It deliberately includes only a bounded triage preview. Large timelines, plans, SQL text, raw snapshots, and guidance are retrieved on demand.

Every later tool call requires the returned `analysis_id`. This prevents findings or evidence references from leaking between concurrent conversations.

Example first call:

```json
{
  "name": "start_performance_analysis",
  "arguments": {
    "focus": "Investigate DB Time degradation and cursor contention",
    "language": "EN",
    "audience": "mixed"
  }
}
```

## Statistical capability catalog

The bootstrap advertises the existing calculations together with their access paths and interpretation caveats:

- descriptive statistics: mean, median, standard deviation, occurrence percentage, and percentiles;
- DB CPU / DB Time workload composition;
- global and sliding-window MAD anomalies plus temporal anomaly clusters;
- Pearson correlations;
- Ridge, Elastic Net, Huber, and Quantile-95 gradients, including VIF and collinear groups;
- robust baseline-versus-recent DB Time degradation;
- metric, wait-event, and SQL timelines, peak/baseline snapshot comparison, and wait histograms.

The recommended opening sequence is:

1. inspect the bootstrap seed and dataset availability;
2. obtain the whole-window load summary and recent DB Time degradation;
3. inspect multi-model gradients as hypothesis ranking, not causality proof;
4. select representative peak and quiet snapshots;
5. call narrow timeline, SQL, wait, plan, cursor, alert log, parameter, and OS tools to verify or falsify the hypotheses;
6. record evidence-backed findings and mandatory assessments;
7. validate and finalize the report.

On AIX, the bootstrap explicitly requires LPAR entitlement evidence before a CPU-pressure conclusion and recommends `date_from`/`date_to` values aligned with the parsed AWR interval. The AIX summary excludes undated observations when either date filter is active, preventing unrelated OS collection periods from contaminating entitlement statistics. For storage, the model must separate service latency from request volume. For application design, commit policy, SQL tuning, cursor contention, and parameter changes, the quality gates require direct supporting evidence and allow an explicit `unknown` result.

## Tool groups

The MCP server publishes the complete tool catalog already used by JAS-MIN AI modes. Availability still depends on parsed content and sibling attachments.

| Group | Examples | Purpose |
|---|---|---|
| Bootstrap and catalog | `start_performance_analysis`, `get_analysis_catalog` | Learn available calculations, data, quality gates, and report requirements. |
| Precomputed statistics | `get_precomputed_analysis` | Retrieve bounded `ReportForAI` sections such as gradients, degradation, waits, I/O, latches, anomalies, or parameters. |
| Time-series evidence | `list_snapshots`, `get_metric_time_series`, `get_wait_event_timeline`, `get_sql_timeline`, `compare_snapshots` | Establish chronology and compare peaks with baselines. |
| Database detail | SQL text, plans, snapshot sections, histograms, segment and latch tools | Verify a specific database hypothesis. |
| Attachments | execution-plan, child-cursor, alert log, and AIX tools | Use collector evidence that is not present in the core AWR series. |
| Diagnostic guidance | `get_diagnostic_guidance` | Retrieve only relevant `reasonings.txt` sections for a concrete observed symptom. |
| Report state | `configure_report`, `record_finding`, `set_report_assessment`, `get_report_status`, `finalize_report` | Build and render a validated report without relying on prompt-only formatting. |

Tool results that represent observed data receive an `evidence_id`. Repeated identical calls in one analysis reuse the same evidence record. Guidance calls return `guidance_ref` identifiers and are intentionally stored separately: diagnostic rules are methodology, not proof that a condition exists in this database.

## `reasonings.txt`

JAS-MIN resolves diagnostic guidance in this order:

1. `$JASMIN_HOME/reasonings.txt`;
2. `./reasonings.txt`.

The file is parsed into individual sections. `get_diagnostic_guidance` supports an exact section identifier such as `§1.1` or a symptom-oriented query and returns a bounded number of relevant sections. The model must cite measurement `evidence_refs` for factual findings; a guidance reference can explain the diagnostic method but cannot replace database or OS evidence.

## Stable, configurable report contract

`configure_report` sets the session-scoped presentation:

- `output_format`: `markdown`, `json`, or `both`;
- `language`: the requested report language;
- `audience`: `technical`, `management`, or `mixed`;
- `detail_level`: `compact`, `standard`, or `deep`;
- `detail_overrides`: per-category detail levels;
- optional evidence and guidance appendices.

The server, rather than the model prompt, owns the core report order:

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

Core sections cannot be removed, but their detail can vary and extra evidence can be included. A finding contains severity, confidence, conclusion, optional detail, verified evidence references, optional guidance references, and structured actions owned by DBA, Developer, or Management.

Before finalization, the report must cover the performance profile, wait events, SQL, I/O, and parameters and must explicitly assess:

- disk quality;
- application design;
- commit policy;
- CPU pressure;
- parameter hygiene.

`get_report_status` reports missing coverage. `finalize_report` refuses an incomplete final report unless the caller explicitly requests an incomplete draft with `allow_incomplete=true`. It returns deterministic Markdown and/or a structured JSON document, so the report shape does not depend on a model following formatting prose perfectly.

## MCP lifecycle example

The endpoint implements stateful Streamable HTTP. A client performs the standard lifecycle:

1. `initialize`;
2. `notifications/initialized` using the returned `Mcp-Session-Id`;
3. `tools/list` and optionally `prompts/list` or `prompts/get`;
4. `tools/call` for `start_performance_analysis` and subsequent calls;
5. HTTP `DELETE` with the session ID when the transport session is no longer needed.

The JAS-MIN `analysis_id` is separate from `Mcp-Session-Id`: the former scopes investigation evidence and report state; the latter belongs to the MCP transport.

Minimal initialize request:

```bash
curl -i -X POST http://127.0.0.1:4242/mcp \
  -H 'Accept: application/json, text/event-stream' \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"manual-test","version":"1.0"}}}'
```

Use the exact protocol version negotiated in the initialize response for client behavior. MCP clients normally perform these transport calls automatically.

## Operational notes

- The endpoint is created only after analysis succeeds; a parser error never leaves a partially initialized server.
- `--file` is rejected with `--mcp` because a single parsed document does not provide the complete time-series collection.
- MCP sessions are held in memory and disappear when the JAS-MIN process stops.
- Keep `--security-level` at the lowest level that still provides the SQL and object evidence required by the investigation.
- A model should preserve `unknown` when required OS, plan, cursor, parameter, or application evidence is unavailable.
