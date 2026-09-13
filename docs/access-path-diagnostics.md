# Scan and row-continuation hypotheses in the existing analysis

Implemented from `oracle-EmptyCalories/JASMIN_GAPS.md`, AI/tool contract `2026-09-13.2`.
The normal gradient and degradation reports supply the signals; AI connects them with
SQL cost and workload evidence to suggest a mechanism, alternatives and a targeted check.
There is no separate empty-blocks HTML report or mandatory structural-evidence gate
before a hypothesis. HTML TOP selection and model rankings remain unchanged.

The same built-in [reasoning policy](../src/access_path_reasoning.md) is used by the
classic API, local investigator/reviewer and MCP instructions/report contract. It asks AI
for a short synthesis in the existing gradient/degradation or SQL finding:

- More scan work and logical reads with higher cost per equivalent SQL work: consider
  inefficient scanning, including empty/sparse blocks below HWM, alongside data growth,
  workload and plan changes.
- Material continuation activity with higher logical reads and SQL/CPU cost: consider
  chaining/migration, even if the extra visits are cached and physical I/O stays low.
- Tiny systemic continuation counts remain context. Model agreement is association;
  instance counters do not identify an affected object, and missing data is unknown.

These are conditional hypotheses, not a diagnosis automatically assigned by a threshold.
SQL/segment measurements are confirmation steps; their absence must not suppress a
useful lead. No automatic MOVE, SHRINK or REBUILD recommendation follows from a counter.

## What changed

| Gap | Behavior |
| --- | --- |
| Missing scan predictors | `table scan blocks gotten` and `table scan rows gotten` participate in DB Time counter and DB CPU regressions. Scan row visits are not live rows. |
| Mixed-unit DB Time shares | Removed `estimated_db_time_delta_share` and summed `total_positive_delta`. Findings expose `unit`, dimensionless `change_score` and `domain_rank`; domain summaries retain a maximum score, not a sum. Statistical severity is not mechanism materiality. |
| Unequal exposure | Regression interval totals for statistics, SQL and waits are divided by actual snapshot wall seconds. Gauges ending in ` current` retain their levels. Missing statistic observations or invalid exposure exclude the statistic predictor; original raw counter charts remain raw. |
| Cost of useful work | SQL costs use sum(buffer gets)/sum(executions) and sum(elapsed seconds)/sum(executions). Zero executions and missing TOP rows are excluded. An optional application-defined work counter provides both costs per work unit. If supplied, it must corroborate the execution-cost increase. |
| Small systemic signals | AI checks absolute volume, rates and workload cost before prioritizing a mechanism; regression labels alone are insufficient. The optional evidence tool applies additional attribution gates to its own assessments. |
| Attribution and availability | Exact DBID/INST_ID/CON_ID/SQL_ID/child/plan/OBJ#/DATAOBJ# groups; explicit window references and missing evidence; nullable CPU tool values and chart gaps; availability masks for uncollected domains. |

The regression coefficient basis changed. Recompute old projects before comparing coefficient
values; historical interval-total coefficients cannot be compared directly with rate coefficients.
The general SQL/wait regressions retain their existing AWR TOP-list proxies; absence from a TOP
list is still censored data, not measured zero. The new SQL-cost diagnostic uses observed costs only.
`predictor_coverage` documents existing SQL proxy coverage. A missing duration excludes rate
features rather than silently assuming a one-second or one-hour window.

The generic degradation report detects a recent DB Time increase and statistically changing
metrics. Its score is `min(max(z,0),99) + ln(1 + max(delta_pct,0)/100) * max(correlation,0)`.
Degradation tools paginate independently within each domain, with an optional exact
`domain` filter; a long wait-event list cannot hide every instance-statistic finding.
There is no cross-unit time accounting. AWR SQL elapsed can include parallel worker time and
must not be treated as an additive share of foreground DB Time.

## Optional confirmation evidence

1. `degradation_detected`: the DB Time detector finds a recent increase.
2. `access_path_suspected`: enough SQL-attributed scan or continuation events and a measured
   increase in both gets/execution and elapsed/execution (also per useful work when supplied).
3. `segment_candidate`: that evidence has child, plan, container, object and data-object identity.
4. `structure_confirmed`: a referenced, same-window structural observation records verified
   empty blocks below HWM or chained rows. `fs4_blocks` alone cannot confirm empty blocks.
5. `intervention_verified`: an explicit before/after link selects recorded SQL observations,
   preserves the SQL/plan/container identity and supplies references for equivalent work,
   logical data, cache and concurrency controls. Both normalized costs must decrease by the
   configured threshold. The post-intervention DATAOBJ# may change but must be specified.

These on-demand tool levels do not gate AI hypothesis formation in the main analysis.
They are assessments of **supplied evidence**, not statements that JAS-MIN independently
re-executed a structural inspection or intervention. Structural confirmation is not causal
confirmation. Row continuation includes both chaining and migration; distinguishing them
requires structural inspection. No universal CHAIN_CNT or free-space-percentage threshold is used.

The optional tool uses default gates of 100 recent attributed events, 1 event/s,
25% growth in both costs, 3 baseline and 2 recent observed samples. They are tool
assessment gates, not universal Oracle health thresholds or AI reporting requirements.
The supplemental evidence format remains `2026-09-13.1`.

The split remains the last `ceil(0.25*N)` windows (bounded to 2–48 and leaving at least
3 baseline samples) versus earlier windows. It does not interpret lab phase names. For
15 windows the comparison is 4 versus 11. Structure and intervention evidence use exact
window identities; they are not inferred from these phase boundaries.

## Use with existing data

```sh
cargo run --release -- --json-file /path/to/project.json --quiet
```

Inspect the existing DB Time counter and DB CPU instance-statistic gradients, then
DB Time Degradation. The full fits remain available in `report_for_ai.full.json`.
AI receives the same standard analytical sections, without `access_path_diagnostics`.

MCP/local tools can inspect individual fitted statistics without extending TOP tables:

```json
{"analysis_id":"YOUR_ANALYSIS_ID","project_id":"YOUR_PROJECT_ID","section":"full_gradients","family":"db_time_instance_stats_counters","contributor":"table scan blocks gotten","limit":1}
```

Use `get_precomputed_analysis` for that query. The corresponding CPU family is
`db_cpu_instance_stats`; `section="db_time_degradation"` supports independent per-domain
pagination and an exact `domain` filter. `get_metric_time_series(kind="instance_stat",
name=..., field="per_second")` gives rates for aligned comparisons; the default field
returns raw totals. `get_sql_timeline` supplies observed SQL executions, gets and elapsed
costs. Missing TOP SQL observations must not be manufactured as zeros.

`get_access_path_diagnostics` remains an optional follow-up for supplied SQL/structure
measurements, with exact `sql_id`, `offset`/`limit` and `evidence_offset`/`evidence_limit`
filters. It is neither a precomputed report section nor part of the initial seed.

## Supply SQL-attributed and structural measurements

The AWR report's existing SQL-by-gets and SQL-by-elapsed sections provide SQL costs,
but do not provide a reliable child/segment identity. Do not fill missing child or
container identity with zero. A richer capture may populate the optional
`AWR.access_path_observations` list directly, or join a supplemental JSON file:

```sh
python3 scripts/attach_access_path_evidence.py input.json evidence.json enriched.json
```

The script creates a new file and rejects an existing destination. For a new collector
package, use `jas-min-collector.py --access-path-evidence evidence.json --security-level 2`
alongside the normal collection arguments. The evidence becomes part of the packaged
JSON, even when report files are also included. Level 2 is required because supplied
source references can contain object names; the importer does not silently redact them.

Supplemental file shape (illustrative numbers, not a measured database):

```json
{
  "schema_version": "2026-09-13.1",
  "dbid": 123,
  "inst_id": 1,
  "windows": [{
    "snap_info": {
      "begin_snap_id": 100,
      "end_snap_id": 101,
      "begin_snap_time": "13-Sep-26 10:00:00",
      "end_snap_time": "13-Sep-26 10:30:00"
    },
    "data_availability": {"host_cpu": false},
    "observations": [{
      "scope": {
        "dbid": 123, "inst_id": 1, "con_id": 3, "sql_id": "abc123",
        "child_number": 0, "plan_hash_value": 321,
        "object_id": 456, "data_object_id": 789
      },
      "evidence_ref": "capture/window-100-session-trace.json",
      "executions": 100, "buffer_gets": 60000, "elapsed_s": 12.5,
      "useful_work": {"name": "orders completed", "unit": "orders", "completed": 500},
      "scan_blocks": 50000, "continued_rows": 0,
      "structure": {
        "evidence_ref": "capture/window-100-block-inspection.txt",
        "observed_at": "2026-09-13T10:15:00",
        "method": "block_inspection",
        "blocks_below_hwm": 500,
        "verified_empty_blocks_below_hwm": 200,
        "fs4_blocks": null,
        "chained_rows": null
      }
    }]
  }]
}
```

The DBID/instance and **all four snapshot identity fields must match exactly**. Duplicate
windows and duplicate scope rows are rejected, as are negative/reset deltas and zero
executions. A missing metric uses `null`, never a manufactured zero. An observation
belongs to its enclosing interval. Structure timestamps must fall within that interval;
use consistent time zones (RFC3339 offsets are normalized; naive AWR timestamps remain
naive local time). A SQL touching multiple segments needs independently attributed
measurements; never copy the entire SQL cost or a session/instance counter onto each object.

Optional `intervention` fields on the pre-intervention observation are:
`evidence_ref`, `controls_evidence_ref`, `before_begin_snap_id`, `after_begin_snap_id`,
`after_scope` (complete scope object), `same_plan`, `same_logical_data`,
`comparable_cache_and_concurrency`, and `equivalent_work`. All four controls must be
explicitly true and backed by the referenced artifact. The before window must be in
the recent comparison period. Both windows must contain usable observations. One
paired test verifies that pair only; repeated/interleaved controls are still needed
before generalizing to production throughput or SLOs.

`AWR.data_availability` is optional for legacy JSON. A false entry overrides supplied
numeric placeholders. Empty collections are reported as unavailable/partial, not clean.
Host CPU with all-zero percentages is unknown, even when CPU count was populated by
an old lab adapter. CPU percentages can be valid without captured CPU topology.

## Capture boundaries and sources

- Use aligned SQL deltas and cursor lifecycle checks; `V$SQL` has one row per child.
  Capture executions, buffer gets and elapsed microseconds twice, reject resets/reloads,
  and convert elapsed deltas to seconds. [Oracle 19c V$SQL](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/V-SQL.html).
- Existing AWR captures provide interval deltas and plan hashes, but not a trustworthy
  `CHILD_NUMBER` in `DBA_HIST_SQLSTAT`. Preserve that absence. `ROWS_PROCESSED_DELTA`
  describes returned rows and is not an application-work denominator for an aggregate.
  [Oracle 19c DBA_HIST_SQLSTAT](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/DBA_HIST_SQLSTAT.html).
- Obtain attributed scan/continuation deltas from controlled session measurements or
  trace with a verified SQL/window scope; never copy instance totals into these fields.
  [Oracle 19c statistic descriptions](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/statistics-descriptions-2.html).
- Record object/data-object/partition identity with space probes. `DBMS_SPACE.SPACE_USAGE`
  describes ASSM blocks below HWM; FS4 spans 75–100% free space. It does not establish
  an exact empty-block count. Preserve probe time and source output; LOB semantics differ.
  [Oracle 19c DBMS_SPACE](https://docs.oracle.com/en/database/oracle/oracle-database/19/arpls/DBMS_SPACE.html).
- Chained-row lists and controlled interventions are prepared evidence. The collector
  does not run `ANALYZE ... LIST CHAINED ROWS`, `MOVE`, `SHRINK`, rebuilds or workload DML.

## Validation

`cargo test --offline` covers exposure, legacy missing CPU, SQL work normalization,
materiality, distinct scopes, duplicate observations, stale structure, FS4 ambiguity,
intervention controls, classic/local/MCP parity, unit-invariant degradation rankings,
and both recorded EmptyCalories degradation series. Python unittest discovery covers
collector packaging, exact evidence joins, rejection without overwriting input, and
Statspack timestamp preservation. Recorded fixtures include source SHA-256 hashes.
No fresh Oracle collection or intervention is implied by offline tests.
