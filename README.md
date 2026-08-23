<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="img/jasmin_LOGO_ora600_white.svg"/>
    <img src="img/jasmin_LOGO_ora600_black.svg" width="480" alt="JAS-MIN — Oracle Performance Evidence"/>
  </picture>
</p>

<h1 align="center">JAS-MIN - JSON AWR & Statspack Miner</h1>

<p align="center">
  <strong>Oracle Database Performance Analysis Tool</strong><br/>
  Parse, analyze, visualize, and optionally consult AI on AWR and STATSPACK reports.
</p>

---

## What is JAS-MIN?

JAS-MIN is a Rust command-line tool for mining Oracle AWR and STATSPACK performance reports. It parses directories of AWR HTML files and STATSPACK text files into structured JSON, then builds an interactive HTML report with Plotly charts, statistical summaries, anomaly detection, correlation analysis, and gradient/regression analysis for DB Time and DB CPU.

The tool can also send a compact `ReportForAI` representation to supported AI providers and convert the resulting Markdown analysis to linked HTML.

## Current Capabilities

| Area | What JAS-MIN does |
|---|---|
| Parsing | Parses a single report with `--file`, or a directory of `.html` and `.txt` reports with `--directory`. |
| Collection helper | Uses `jas-min-collector.py` to generate AWR/STATSPACK reports from a local Oracle environment and package reports, JSON, alert logs, optional SQL execution plans, child-cursor diagnostics, and prepared AIX/Linux statistics. |
| Cached analysis | Re-analyzes an existing JAS-MIN JSON file with `--json-file`. |
| HTML dashboard | Generates `<input>.html_reports/jasmin_main.html` and detail pages for waits, SQL IDs, statistics, I/O, latches, segments, anomalies, and gradients. |
| Peak detection | Marks snapshots where `DB CPU / DB Time` is below `--time-cpu-ratio`, optionally requiring DB Time above `--filter-db-time`. |
| Snap filtering | Limits analysis to a snapshot range with `--snap-range BEGIN-END`. |
| Anomalies | Uses MAD-based anomaly detection with configurable threshold, sliding-window percentage, and optional trimming to the largest anomaly clusters. |
| Correlation | Computes Pearson correlations between DB Time and wait events, SQL elapsed time, and instance statistics. |
| Gradient analysis | Runs Ridge, Elastic Net, Huber, and Quantile-95 regression models over DB Time and DB CPU drivers. |
| Custom gradient | Builds extra gradient pages for a selected SQL ID or wait event with `--gradient-custom`. |
| AI reports | Supports OpenAI, Google Gemini, OpenRouter, and a two-session local agent served by LM Studio. |
| AI tools mode | Enables function/tool-call loops for cloud providers with `--tools-mode`; local analysis always uses tools. |
| MCP server | Retains a parsed collection in memory and exposes interactive evidence, diagnostic guidance, and stable report-building tools through local Streamable HTTP. |
| Security | Controls whether object names and SQL text are stored with `--security-level`. |

## Architecture Overview

```text
+-----------------------------+
| AWR / STATSPACK reports     |
| .html files / .txt files    |
+--------------+--------------+
               |
               | parallel parsing with Rayon
               v
+-----------------------------+
| AWRSCollection JSON         |
| SnapInfo, LoadProfile,      |
| WaitEvents, SQL, Host CPU,  |
| Instance Stats, I/O,        |
| Latches, Segments, Params   |
+--------------+--------------+
               |
               v
+-----------------------------+
| Analysis engine             |
| Peak detection              |
| MAD anomalies               |
| Pearson correlations        |
| Multi-model gradients       |
| VIF and collinear groups    |
+------+----------------------+
       |
       +--> HTML dashboard and detail pages
       +--> TXT analysis log
       +--> CSV anomaly exports
       +--> ReportForAI TOON/JSON for AI reports
```

## Installation

### Requirements

- Rust toolchain 1.88 or newer.

### Build

```bash
git clone https://github.com/ora600pl/jas-min.git
cd jas-min
cargo build --release
```

The binary is created at:

```bash
./target/release/jas-min
```

## Quick Start

### Analyze a directory of reports

```bash
jas-min -d ./awr_reports
```

This parses all non-hidden `.html` and `.txt` files in `./awr_reports`, writes `./awr_reports.json`, writes `./awr_reports.txt`, creates `./awr_reports.html_reports/`, and attempts to open `./awr_reports.html_reports/jasmin_main.html` in the default browser.

### Re-analyze an existing JAS-MIN JSON file

```bash
jas-min -j awr_reports.json
```

### Parse one report to JSON on stdout

```bash
jas-min --file ./AWR_Report_100_101.html
```

### Restrict the snapshot range

```bash
jas-min -d ./awr_reports -s 1000-2000
```

### Tune peak detection

```bash
jas-min -d ./awr_reports -t 0.75 -f 5
```

This marks snapshots where `DB CPU / DB Time < 0.75` and DB Time is above `5`.

### Tune MAD anomaly detection

```bash
jas-min -d ./awr_reports --mad-top 10 -W 25 --top-cluster-anomalies 5
```

`-m, --mad-top` controls how many highest-scoring MAD anomalies are retained by the current anomaly logic. `-W` is the local sliding-window size as a percentage of probes. `100` means global behavior.

### Include specific SQL IDs in TOP SQL analysis

```bash
jas-min -d ./awr_reports -i 0zv508wsas63c,9abc123xyz
```

### Build a custom gradient page

```bash
jas-min -d ./awr_reports -G SQL=0zv508wsas63c
jas-min -d ./awr_reports -G "WAIT=log file sync"
```

When the target is found, JAS-MIN adds `stats/gradient_sqlid.html` with gradients of instance statistics and wait events against that SQL or wait-event time series.

## AI Analysis

AI mode is enabled with:

```bash
jas-min -d ./awr_reports --ai VENDOR:MODEL:LANG
```

Supported vendor prefixes are:

| Prefix | Backend | Required environment |
|---|---|---|
| `openai` | OpenAI Responses API | `OPENAI_API_KEY` |
| `google` | Google Gemini API | `GEMINI_API_KEY` |
| `openrouter` | OpenRouter chat completions | `OPENROUTER_API_KEY` |
| `local` | Two-session evidence-driven agent through LM Studio | `LOCAL_BASE_URL`, optional `LOCAL_API_KEY` |

Examples:

```bash
export GEMINI_API_KEY="your-key"
jas-min -d ./awr_reports --ai google:gemini-2.5-flash:EN

export OPENAI_API_KEY="your-key"
jas-min -d ./awr_reports --ai openai:o3:EN

export OPENROUTER_API_KEY="your-key"
jas-min -d ./awr_reports --ai openrouter:anthropic/claude-sonnet-4:EN

export LOCAL_BASE_URL="http://localhost:1234/v1"
export LOCAL_API_KEY="lm-studio"
export LOCAL_CONTEXT_TOKENS="128000" # must not exceed the context used when loading the model
jas-min -d ./awr_reports --ai local:qwen/qwen3.6-35b-a3b:EN -B 128000 --max-tool-iterations 8
```

The language code, for example `EN` or `PL`, controls the requested report language.

### Tools Mode

For `openai`, `google`, and `openrouter`, `--tools-mode` enables an iterative tool-call loop. In this mode the model can request focused diagnostic data from the parsed collection instead of relying only on the initial summary. The `local` backend always uses tools and ignores `--tools-mode`.

```bash
jas-min -d ./awr_reports --ai openai:o3:EN --tools-mode --max-tool-iterations 12
jas-min -d ./awr_reports --ai google:gemini-2.5-flash:EN --tools-mode
jas-min -d ./awr_reports --ai openrouter:openai/gpt-4.1:EN --tools-mode
```

If a sibling `<stem>_attachments/` directory exists, tools mode can also expose execution-plan, decoded child-cursor sharing reasons, alert log, and AIX OS attachments to the model.

OpenRouter requests are retried up to three times when the transport fails, the service returns a transient HTTP status, or a successful HTTP response contains empty or malformed JSON. Invalid response bodies are preserved next to the report as `*.bad_response.json` diagnostics. If all attempts fail, JAS-MIN exits with an error instead of panicking or publishing an empty report.

Execution plans are expected as `<stem>_attachments/<SQL_ID>.xplan`. The collector can create these files automatically for the SQL IDs that appear most often in `SQLs Ordered by Elapsed time` sections, plus any SQL IDs entered manually.

Decoded child-cursor sharing reasons are stored as `<stem>_attachments/<SQL_ID>.shared_cursor_reasons`. For TOP SQL_IDs selected from `SQLs Ordered by Elapsed time`, the collector checks the current local `V$SQL`; when more than one distinct child number exists, it parses every `ChildNode` and diagnostic payload field from `V$SQL_SHARED_CURSOR.REASON`. Tools mode then exposes `list_available_child_cursor_reasons` and `get_child_cursor_reasons` so the model can inspect the exact criterion IDs, subcodes, fields, and comparison-vector values. A/B values are comparison-vector sides, not chronological old/new values.

AIX OS data collected by the `oraix` project can be placed under `<stem>_attachments/AIX/`. When files are present there, tools mode exposes AIX-specific tools such as `list_aix_os_attachments`, `get_aix_os_attachment`, and `get_aix_cpu_entitlement_summary`. These tools scan for LPAR CPU entitlement evidence including `Entc%`, `%entc`, `physc`/`pc`, entitled capacity/`ec`, busy, idle, user, system, and wait CPU metrics.

For AIX systems, JAS-MIN instructs the AI model not to classify a database as CPU-bound from DB CPU, DB CPU / DB Time, or AWR Host CPU `%CPU` alone. The model must check AIX entitlement data first; if `Entc%` and related LPAR details are missing, it should ask for those operating-system details before making a final CPU-bound decision.

### Interactive MCP Server

JAS-MIN can expose one or more parsed collections and the same evidence tools as a local Streamable HTTP MCP server:

```bash
jas-min --json-file ./awr_reports.json --security-level 2 \
  --mcp 127.0.0.1:4242/mcp
```

Repeat `-d` or `-j` to retain several projects in one process:

```bash
jas-min -j ./before_upgrade.json -j ./after_upgrade.json \
  --security-level 2 --mcp 127.0.0.1:4242/mcp
```

`list_performance_projects` publishes stable project IDs, periods, database identity, sample counts, and attachment coverage. Alert-log inventory includes each file's byte length, empty/non-empty classification, first and last recognized ISO timestamp, timestamp-line count, and coverage status relative to the dataset dates. A zero-byte attachment is missing coverage, and a non-empty partial attachment is scoped to its observed timestamps rather than the enclosing AWR period. `start_performance_analysis` can select one project, several projects, or every loaded project. In comparative sessions, project-specific evidence tools are routed with `project_id`; `compare_project_metric` compares observed distributions and `compare_project_sql` separates per-execution efficiency from workload volume for the same SQL ID. Missing samples are never converted to zero, and improvement/degradation labels require an explicit metric direction.

Multi-project startup rejects `--outfile`, duplicate sources, and generated chart-directory collisions before analysis begins. This avoids ambiguous output ownership and silent overwrites.

The mandatory analysis bootstrap teaches the model which statistical calculations and attachments are available, returns compact high-signal seeds, and creates an explicit analysis session. Subsequent calls retrieve focused evidence or relevant sections from `reasonings.txt`. Report tools retain the stable eleven-section core while enforcing evidence coverage independently from reader-facing presentation. Non-draft finalization requires separate gradient, anomaly, and anomaly-cluster blocks plus cross-signal synthesis when multiple families exist. Gradient coverage includes the five highest-impact triangulated rows per family and every foreground wait reaching 10% DB Time; a material wait omitted by the models is disclosed explicitly. Every such wait also requires an explicit wait-to-SQL contributor table based on aligned correlation and/or direct ASH attribution. The strongest material contributor must be followed through SQL text, timeline, and plan applicability. Every unique SQL execution-plan hash is reviewed, while `BEGIN`/`DECLARE`/`CALL` entry points are classified as PL/SQL with `not_applicable_plsql`; their cards require PL/SQL instrumentation and inner-SQL analysis rather than a meaningless top-level plan recapture. A plan-change recommendation is accepted only with observed SQL timeline, top-SQL, wait-contributor, or project-comparison evidence proving why that SQL matters. The SQL card then states measured impact, coverage, comparison context, limitations, action, and success criterion. The gate also covers every child-cursor diagnostic, deterministic alert-log error summary and parse-error SQL finding, segment hotspot plus cross-statistic synthesis, and collected checklist parameter. Missing parameter values require no row, and only `concern`/`critical` ratings are rendered. `get_report_status` returns exact missing evidence and row keys, so completeness is a server gate rather than a prompt convention. Human-facing findings must connect the measured symptom to a mechanism, temporal pattern, named affected workload, and explicit evidence boundary; a conclusion plus a table dump is rejected. Recommendations require an owner, priority, evidence-backed rationale, and measurable success criterion. The renderer leads each section with that diagnostic synthesis, groups actions by owner, and only then presents deterministic technical tables. Raw evidence IDs remain structured provenance. Comparative prose labels every instance. Tables of the same kind are merged into one semantic TOC entry, long plans open on flagged paths, and SQL IDs or wait events link directly to the existing project-specific detail report. HTML export accepts only exact finalized Markdown and rejects broken local targets.

While MCP mode is running, JAS-MIN prints a UTC-timestamped `START` and `OK`, `ERROR`, or `ABORTED` status for every tool call. The lines include the JSON-RPC ID, tool name, analysis ID, payload sizes, duration, and stable error code, but never include SQL, report Markdown, or other argument and response bodies. See the technical documentation for the complete log format and timeout diagnosis workflow.

Clients negotiating MCP `2026-07-28` or newer receive the required `ttlMs` and `cacheScope` fields in `tools/list`; older sessions retain the legacy wire shape.

See [JAS-MIN MCP Server](docs/mcp-server.md) for client lifecycle, tool groups, evidence rules, quality gates, and the report contract.

### One-Shot Batch Analysis

For `google`, `openai`, and `openrouter`, JAS-MIN can send the compact `ReportForAI` structure plus Load Profile statistics to the selected model in one call. The report payload is serialized as TOON/JSON-like text to keep token usage lower than pretty JSON while preserving structure.

Typical output files are named after the text log and model, for example:

```text
awr_reports.txt_gemini.md
awr_reports.txt_o3.md
awr_reports.txt_anthropic_claude-sonnet-4.md
```

The generated Markdown is converted to HTML with links back to JAS-MIN detail pages where possible.

### Local Two-Session Investigation

The `local` backend uses a tool-driven workflow designed for LM Studio:

1. Session 1 receives only a compact seed containing gradient analyses, DB Time degradation, and DB CPU / DB Time ratios for detected peaks.
2. The investigator receives a small triage tool catalog first. Later rounds expose deeper timeline, plan, parameter, segment, latch, and attachment tools only when there is room to use them. Duplicate calls in one session return a short reference instead of repeating a large cached result.
3. When `reasonings.txt` is available, relevant §x.y sections are retrieved for detected symptoms. They remain methodology rather than database evidence.
4. The investigator submits a compact checkpoint. Qwen uses JSON Schema plus LM Studio's `reasoning_content` channel because live tests showed its forced checkpoint tool call repeatedly exhausted 4096 tokens; other models can use the dedicated checkpoint tool with the structured fallback.
5. Session 2 starts with a fresh context containing the original seed and checkpoint. A skeptical reviewer re-queries evidence and attempts to falsify the first session's findings.
6. Before synthesis, deterministic coverage states distinguish inspected, available-but-not-inspected, unavailable, and unknown data.
7. Checkpoint and report synthesis use a fresh compact case file assembled from the evidence store rather than the verbose tool transcript. Full tool results remain in the audit JSON while each model-visible evidence item receives a bounded share of the closing context.
8. The reviewer emits Markdown directly. If LM Studio returns `finish_reason=length`, JAS-MIN continues the report; it refuses to publish after exhausted continuation attempts rather than writing a truncated file. JAS-MIN also writes checkpoint, evidence, diagnostic-guidance, and token-usage audit files.

The local runner reserves context for thinking and final synthesis instead of filling the whole context window. `--tokens-budget` is treated as the local context ceiling and can be overridden with `LOCAL_CONTEXT_TOKENS`. Tool result size, sampling, and model settings can be controlled with `LOCAL_MAX_TOOL_RESULT_CHARS`, `LOCAL_MAX_GUIDANCE_CHARS`, `LOCAL_TEMPERATURE`, `LOCAL_TOP_P`, and `LOCAL_TOP_K`. Defaults are tuned for Qwen 3.6: tool results 16384 characters, guidance 8192 characters, `top_k=20`, `LOCAL_TOOL_OUTPUT_TOKENS=3072`, `LOCAL_CHECKPOINT_OUTPUT_TOKENS=4096`, and `LOCAL_FINAL_OUTPUT_TOKENS=12288`. Non-Qwen models retain `top_k=64`. Context guards begin with `LOCAL_TOKEN_ESTIMATE_SAFETY_FACTOR` (default 2.0) and calibrate upward from LM Studio's actual prompt-token usage. Thinking is enabled during evidence gathering but disabled during checkpoint serialization and final formatting. Session 1 artifacts are persisted before Session 2 starts, so a later model or transport failure does not discard the completed investigation. For MLX on a single workstation, LM Studio parallelism `1` avoids dividing the KV cache between concurrent slots.

For local analysis, `reasonings.txt` is parsed into individual §x.y sections and exposed through `get_diagnostic_guidance`; the full file is never appended to every model request. Guidance calls receive `S1-G...` or `S2-G...` references and are written to `*.local_agent.guidance.json`. They are methodology, not measurements: factual claims must still cite `SEED-E0001` or an `S*-E...` database evidence record. If the file is absent, the tool is not advertised and analysis continues without it.

### URL Context and Custom Reasoning

`--url-context-file` loads a JSON file used to add URL instructions for matching events or SQL IDs. The file is mainly useful with Gemini URL context workflows.

```bash
jas-min -d ./awr_reports --ai google:gemini-2.5-flash:EN -u url_context.json
```

For cloud-provider analysis, JAS-MIN appends `reasonings.txt` to the model instructions when the file exists. Local two-session analysis instead exposes individual sections through `get_diagnostic_guidance` to preserve context. If `JASMIN_HOME` is set, both modes read `$JASMIN_HOME/reasonings.txt`; otherwise they try `./reasonings.txt`.

### ReportForAI Data Structure

The `ReportForAI` document sent to AI providers is intentionally smaller than the full parsed JSON collection. It contains selected, analysis-oriented summaries:

| Section | Content |
|---|---|
| `general_data` | Descriptions of the ratio and MAD analysis context. |
| `top_spikes_marked` | Peak snapshots with DB Time, DB CPU, and DB CPU / DB Time ratio. |
| `top_foreground_wait_events` | Foreground waits, descriptive statistics, correlations, and anomalies. |
| `top_background_wait_events` | Background waits, descriptive statistics, and anomalies. |
| `top_sqls_by_elapsed_time` | SQL elapsed-time metrics, CPU time, ASH events, correlations, and MAD summaries. |
| `io_stats_by_function_summary` | Per-function I/O behavior such as DBWR, LGWR, and other Oracle components. |
| `latch_activity_summary` | Latch activity and contention summaries. |
| `top_10_segments_by_*` | Segment ranking sections when segment data is available and security level permits storing names. |
| `instance_stats_pearson_correlation` | Instance statistics correlated with DB Time. |
| `load_profile_anomalies` | MAD anomalies in Load Profile metrics. |
| `anomaly_clusters` | Cross-domain anomaly groups around the same snapshot period. |
| `db_time_gradient_*` | DB Time gradient sections with model results, VIF diagnostics, and group impact. |
| `db_cpu_gradient_*` | DB CPU gradient sections with model results, VIF diagnostics, and group impact. |
| `custom_gradient_*` | Custom SQL or wait-event gradient sections when `--gradient-custom` is used. |
| `initialization_parameters` | Initialization parameters parsed from reports. |

Each gradient section contains:

| Field | Content |
|---|---|
| `settings` | Model hyperparameters and unit descriptions. |
| `ridge_top` | Top Ridge regression rows. |
| `elastic_net_top` | Top non-zero Elastic Net rows. |
| `huber_top` | Top Huber robust regression rows. |
| `quantile95_top` | Top Quantile-95 tail-risk rows. |
| `cross_model_classifications` | Cross-model labels such as `CONFIRMED_BOTTLENECK` and `TAIL_RISK`. |
| `vif_diagnostics` | Predictors with elevated VIF and interpretation labels. |
| `collinear_group_impacts` | Combined impact for groups of strongly correlated predictors. |

## Markdown Conversion

Convert an existing Markdown AI report to linked HTML without calling an AI model:

```bash
jas-min -c awr_reports.txt_gemini.md
```

The output is written next to the Markdown file with an `.html` extension.

## Security Levels

| Level | Flag | Stored sensitive details |
|---|---|---|
| 0 | `-S 0` | Does not store object names, database names, or other sensitive names where the parser supports masking. |
| 1 | `-S 1` | Also stores segment names from Segment Statistics. |
| 2 | `-S 2` | Also stores full SQL text from AWR/STATSPACK sections when parsed. |

Default security level is `0`.

## Statistical Algorithms

### DB CPU / DB Time Ratio Analysis

JAS-MIN identifies performance peaks by comparing **DB CPU** with **DB Time** from the Load Profile section of each snapshot:

```text
R = DB CPU (s/s) / DB Time (s/s)
```

- `R` close to `1.0` usually means the workload is CPU-bound.
- `R` below the configured `--time-cpu-ratio` threshold means sessions spend a larger share of DB Time outside CPU, so wait events become more interesting.
- `--filter-db-time` can be used to ignore low-volume periods where the ratio looks bad but the absolute DB Time is not operationally important.

When `DB CPU / DB Time < --time-cpu-ratio` and the optional DB Time filter passes, the snapshot is marked as a peak period. JAS-MIN then selects the most relevant foreground waits, background waits, and SQL statements from those periods for deeper analysis and visualization.

### Median Absolute Deviation (MAD)

MAD is used as a robust anomaly detection method across several performance domains. It is less sensitive to extreme outliers than standard deviation, which makes it useful for bursty database workloads.

For a time series:

```text
X = {x1, x2, ..., xn}
```

JAS-MIN computes:

```text
median = median(X)
di     = |xi - median|
MAD    = median({d1, d2, ..., dn})
score  = |xi - median| / MAD
```

An observation is treated as anomalous when its MAD score is above the current fixed score cutoff of `7.0`. `-m, --mad-top` does not change that cutoff; it controls how many highest-scoring MAD anomalies are retained, with a default of `10`.

`-W, --mad-window-size` controls whether MAD is global or local:

- `-W 100` uses the whole time series as the reference population.
- Values below `100` use a sliding local window expressed as a percentage of probes, which helps detect anomalies relative to nearby behavior instead of the whole observation period.

MAD analysis is applied to areas such as foreground and background wait events, SQL elapsed time, Load Profile metrics, instance activity statistics, dictionary cache, library cache, latch activity, and time model statistics. `--top-cluster-anomalies` can additionally keep only the top N largest anomaly clusters in the summary. A cluster is one snapshot date grouped across anomaly categories, ranked by the total number of retained anomalies in that snapshot.

### Pearson Correlation Coefficient

JAS-MIN computes Pearson correlation between DB Time and many candidate drivers:

- foreground and background wait-event total wait time,
- SQL elapsed time,
- instance activity statistics.

The coefficient is:

```text
r = sum((xi - mean(x)) * (yi - mean(y)))
    / sqrt(sum((xi - mean(x))^2) * sum((yi - mean(y))^2))
```

The same idea is also used to correlate SQL elapsed time with foreground wait events, which helps identify which waits co-occur with specific SQL statements.

If a metric has zero variance, the raw Pearson formula can produce a non-finite value. JAS-MIN guards against that case and treats non-finite correlations as not useful instead of letting them pollute the report.

### Bonferroni-Corrected Significance Threshold

When many instance statistics are checked against DB Time, JAS-MIN uses a Bonferroni-style correction to reduce false positives from multiple comparisons:

```text
r_threshold = max(0.5, r_bonferroni(k, alpha, n))
```

where:

- `k` is the number of tested statistics,
- `alpha` is the family-wise significance level,
- `n` is the number of observations.

Only statistics whose absolute correlation exceeds the threshold are promoted into the correlation report.

### Multi-Model Gradient Regression

The gradient analysis answers a slightly different question than correlation: **when DB Time changes, which metrics move in a way that best explains that change?**

JAS-MIN first computes first-order differences:

```text
delta_y_t  = y_(t+1) - y_t
delta_x_jt = x_j,(t+1) - x_j,t
```

The target delta is centered, and predictors are standardized using sample standard deviation with Bessel's correction:

```text
x_hat_jt = (delta_x_jt - mean(delta_x_j)) / s_j

s_j = sqrt( sum((delta_x_jt - mean(delta_x_j))^2) / (N - 1) )
```

JAS-MIN then fits four complementary regression models:

| Model | Method | What it is good for |
|---|---|---|
| Ridge | Dense linear solve with L2 regularization: `(X'X + lambda I) beta = X'y` | Stable ranking when predictors are numerous or correlated. |
| Elastic Net | Coordinate descent with L1 and L2 penalties | Sparse ranking that highlights dominant drivers and suppresses redundant correlated predictors. |
| Huber | Iteratively Reweighted Least Squares with Huber loss | Robust ranking that downweights extreme outlier snapshots. |
| Quantile 95 | Quantile regression focused on the 95th percentile | Tail-risk analysis for the worst periods rather than average behavior. |

The configurable parameters are:

| Flag | Meaning | Default |
|---|---|---|
| `-R, --ridge-lambda` | Ridge L2 regularization strength | `50` |
| `-E, --en-lambda` | Elastic Net regularization strength | `30` |
| `-A, --en-alpha` | Elastic Net L1/L2 mix; `1.0` is Lasso, `0.0` is Ridge-like | `0.333` |
| `-I, --en-max-iter` | Coordinate descent iteration limit | `5000` |
| `-T, --en-tol` | Elastic Net convergence tolerance | `0.000001` |
| `--top-gradient` | Number of top rows kept per regression model | `10` |

JAS-MIN calculates an impact score using the fitted coefficient and the MAD of the raw predictor deltas:

```text
impact_j = beta_j * MAD(delta_x_j)
```

The sign is preserved. Positive values indicate metrics associated with DB Time increases; negative values indicate metrics associated with DB Time decreases. This prevents idle or anti-correlated metrics from being reported as bottlenecks simply because their absolute coefficient is large.

The standard gradient pages cover:

1. DB Time vs foreground wait events.
2. DB Time vs SQL elapsed time.
3. DB Time vs instance statistic counters.
4. DB Time vs instance statistic volumes.
5. DB Time vs instance statistic time metrics.
6. DB CPU vs CPU-related instance statistics.
7. DB CPU vs SQL CPU time.

`--gradient-custom` adds a targeted gradient for a selected SQL ID or wait event:

```bash
jas-min -d ./awr_reports -G SQL=0zv508wsas63c
jas-min -d ./awr_reports -G "WAIT=log file sync"
```

### Multicollinearity Diagnostics (VIF)

Oracle performance metrics are often highly collinear. For example, several enqueue waits, logical I/O metrics, or SQL elapsed-time series may rise and fall together. In that case, a multivariate model may know that the group matters but still struggle to assign impact cleanly to one member.

JAS-MIN computes the **Variance Inflation Factor** for predictors:

```text
VIF_j = 1 / (1 - R_j^2)
```

`R_j^2` is computed by regressing predictor `j` against the other predictors. High VIF means the predictor can be explained by the other predictors and its individual coefficient is less reliable.

| VIF range | Interpretation | How to read it |
|---|---|---|
| `1 - 5` | Acceptable | Individual coefficients are usually usable. |
| `5 - 10` | Moderate collinearity | Interpret together with related metrics. |
| `10 - 100` | High collinearity | Individual impact may be unstable; check group impact. |
| `> 100` | Severe collinearity | Prefer collinear group impact over individual coefficient. |

### Collinear Group Impact

When predictors are strongly collinear, JAS-MIN groups them using pairwise correlation clustering and estimates a combined signal:

```text
delta_x_group,t = sum(delta_x_j,t for j in group)
```

Then it fits a univariate relationship between the combined group signal and target DB Time deltas:

```text
beta_group = Cov(delta_x_group, delta_y) / Var(delta_x_group)
group_impact = |beta_group| * MAD(delta_x_group)
```

This helps with cases where each individual event looks weak because the model cannot separate it from its siblings, while the combined wait family is clearly important.

### Cross-Model Triangulation

After fitting Ridge, Elastic Net, Huber, and Quantile-95, JAS-MIN compares which predictors appear in each model's top results. The cross-model classification is intended to make the gradient output easier to read operationally:

| Classification | Typical evidence | Interpretation |
|---|---|---|
| `CONFIRMED_BOTTLENECK` | Present across all four models | Robust systematic driver. |
| `CONFIRMED_BOTTLENECK_EN_COLLINEAR` | Ridge, Huber, and Q95, but not Elastic Net | Likely real driver masked by sparse collinearity behavior. |
| `STRONG_CONTRIBUTOR` | Ridge, Elastic Net, and Huber | Stable average contributor. |
| `STABLE_CONTRIBUTOR` | Ridge and Huber | Persistent background contributor. |
| `TAIL_RISK` | Quantile-95 only | Driver of worst-case periods rather than normal periods. |
| `TAIL_OUTLIER` | Ridge and Quantile-95, but not Huber | Extreme snapshots influence the result. |
| `OUTLIER_DRIVEN` | Ridge only | Possible impact from a few unusually large observations. |
| `SPARSE_DOMINANT` | Elastic Net only | A sparse representative from a correlated group. |
| `ROBUST_ONLY` | Huber only | Visible after downweighting outliers. |

The VIF diagnostics and collinear group impact should be read together with these labels: classification says *what looks important*, while VIF and group impact help explain whether the importance is individually attributable or group-level.

### Descriptive Statistics

For wait events, SQL statements, Load Profile metrics, I/O, and latch activity, JAS-MIN computes descriptive statistics such as mean, standard deviation, median, quartiles, interquartile range, fences, minimum, maximum, variance, and weighted averages where appropriate.

These statistics feed both the HTML dashboard and the compact `ReportForAI` data sent to AI models.

## Output Structure

A directory run with `jas-min -d ./awr_reports` produces:

```text
awr_reports.json
awr_reports.txt
report_for_ai.toon
awr_reports.html_reports/
|-- jasmin_main.html
|-- fg/
|   `-- fg_<event_name>.html
|-- bg/
|   `-- bg_<event_name>.html
|-- sqlid/
|   `-- sqlid_<sql_id>.html
|-- stats/
|   |-- statistics_corr.html
|   |-- gradient.html
|   |-- gradient_cpu.html
|   |-- gradient_sqlid.html          # only when --gradient-custom produces data
|   |-- global_statistics.json
|   |-- jasmin_highlight.html
|   |-- jasmin_highlight2.html
|   `-- inst_stat_<name>.html
|-- iostats/
|   |-- iostats_zMAIN.html
|   `-- iostats_<function>.html
|-- latches/
|   `-- latchstats_activity.html
|-- segstats/
|   `-- segstats_<stat_name>.html
`-- jasmin/
    `-- anomalies/
        |-- anomalies_reference.csv
        `-- <snap_id>.csv
```

AI runs additionally write Markdown and HTML files named from the text log and model, for example `awr_reports.txt_gemini.md`, `awr_reports.txt_gemini.html`, or `awr_reports.txt_o3_tools.md`.

## Environment Variables

JAS-MIN loads `.env` from `$JASMIN_HOME/.env` first. If `JASMIN_HOME` is not set or the file is missing, it tries `./.env`.

```env
# AI API keys
OPENAI_API_KEY=sk-...
GEMINI_API_KEY=AI...
OPENROUTER_API_KEY=sk-or-...

# Optional custom OpenAI-compatible base for OpenAI Responses API
OPENAI_URL=https://api.openai.com/

# Local OpenAI-compatible chat endpoint used by --ai local:...
LOCAL_API_KEY=lm-studio
LOCAL_BASE_URL=http://localhost:1234/v1
LOCAL_CONTEXT_TOKENS=128000
# Optional Qwen/local-agent tuning:
# LOCAL_MAX_TOOL_RESULT_CHARS=16384
# LOCAL_MAX_GUIDANCE_CHARS=8192
# LOCAL_TEMPERATURE=1.0
# LOCAL_TOP_P=0.95
# LOCAL_TOP_K=20
# LOCAL_TOOL_OUTPUT_TOKENS=3072
# LOCAL_CHECKPOINT_OUTPUT_TOKENS=4096
# LOCAL_FINAL_OUTPUT_TOKENS=12288
# LOCAL_TOKEN_ESTIMATE_SAFETY_FACTOR=2.0

# Optional centralized home for .env and reasonings.txt
JASMIN_HOME=/path/to/jasmin_home

# Optional debug trace destination base path
JASMIN_TRACE=/tmp/jasmin_trace
```

## CLI Reference

Current options from the Rust CLI:

```text
Usage: jas-min [OPTIONS]

Options:
      --file <FILE>                          Parse a single text or HTML file
  -d, --directory <DIRECTORY>                Parse a directory of report files. Repeat with --mcp to load multiple projects
  -o, --outfile <OUTFILE>                    Write parsed JSON to a non-default file
  -t, --time-cpu-ratio <TIME_CPU_RATIO>      DB CPU / DB Time threshold [default: 0.666]
  -f, --filter-db-time <FILTER_DB_TIME>      Ignore peaks below this DB Time [default: 0]
  -i, --id-sqls <ID_SQLS>                    Include comma-separated SQL_IDs in TOP SQL
  -j, --json-file <JSON_FILE>                Analyze a JSON file. Repeat with --mcp to load multiple projects
  -s, --snap-range <SNAP_RANGE>              Snapshot filter BEGIN-END [default: 0-666666666]
  -q, --quiet                                Suppress terminal output, still write log
  -a, --ai <AI>                              AI mode: VENDOR:MODEL:LANG
  -m, --mad-top <MAD_TOP>                   TOPn for retaining anomalies detected using MAD [default: 10]
  -W, --mad-window-size <MAD_WINDOW_SIZE>    MAD window size as percent of probes [default: 100]
  -T, --top-cluster-anomalies <TOP_CLUSTER_ANOMALIES>
                                               Keep top N largest anomaly clusters in the summary [default: 0]
  -P, --parallel <PARALLEL>                  Rayon parallelism level [default: 4]
  -S, --security-level <SECURITY_LEVEL>      Security level: 0, 1, or 2 [default: 0]
  -u, --url-context-file <URL_CONTEXT_FILE>  URL context JSON file
  -B, --tokens-budget <TOKENS_BUDGET>        Token budget for AI analysis; local mode treats it as the context ceiling [default: 256000]
  -R, --ridge-lambda <RIDGE_LAMBDA>          Ridge L2 regularization [default: 50]
  -E, --en-lambda <EN_LAMBDA>                Elastic Net regularization [default: 30]
  -A, --en-alpha <EN_ALPHA>                  Elastic Net L1/L2 mix [default: 0.333]
  -I, --en-max-iter <EN_MAX_ITER>            Elastic Net max iterations [default: 5000]
  -T, --en-tol <EN_TOL>                      Elastic Net tolerance [default: 0.000001]
      --top-gradient <TOP_GRADIENT>          Top N rows per regression model [default: 10]
  -c, --convert-md2html <CONVERT_MD2HTML>    Convert Markdown to HTML without AI call
  -G, --gradient-custom <GRADIENT_CUSTOM>    Custom gradient: SQL=<sql_id> or WAIT=<event>
      --tools-mode                           Enable AI tools mode for cloud providers; local mode always uses tools
      --max-tool-iterations <N>              Max tool-call iterations [default: 10]
      --mcp <ADDRESS/PATH>                   Start a loopback Streamable HTTP MCP server after parsing
  -h, --help                                 Print help
  -V, --version                              Print version
```

## Docker

```bash
docker build -t ora600pl/jas-min:latest .

export AWRDIR=/path/to/reports
export JASMIN_HOME=/path/to/jasmin_home

docker run --rm \
  -v "$AWRDIR:/work" \
  -v "$JASMIN_HOME:/jasmin/home" \
  ora600pl/jas-min:latest \
  -d /work -q -m 10
```

## Generating Reports

- STATSPACK: use the included `gen_statspack_reps.sh`.
- AWR through SQL*Plus: use the included `awr-generator.sql`.
- AWR through ORDS: use the included `awr-ords-generator.sh`.
- Interactive local collection: use `jas-min-collector.py`.

For useful statistics, collect a meaningful run of consecutive reports. A week or more is usually better than a few isolated snapshots.

### Interactive Collector

`jas-min-collector.py` is a Python standard-library helper for environments where the reports should be generated directly from the target Oracle host. It expects `ORACLE_HOME`, `ORACLE_SID`, and a working `$ORACLE_HOME/bin/sqlplus` connection as `/ as sysdba`.

```bash
export ORACLE_HOME=/path/to/oracle/home
export ORACLE_SID=ORCL
python3 jas-min-collector.py
```

The collector supports interactive, mixed, and fully non-interactive use. Questions are shown only for choices not covered by command-line options. For example, this command supplies every choice and does not prompt:

```bash
python3 jas-min-collector.py \
  --report-type awr \
  --start "2026-06-14 00:00" \
  --end "2026-06-15 14:00" \
  --include-alert-log \
  --execution-plans \
  --sql-id abc123,def456 \
  --no-os-stats \
  --package-content both \
  --security-level 1
```

Run `python3 jas-min-collector.py --help` for the generated CLI help. The complete option set is:

| Option | Meaning and interaction behavior |
|---|---|
| `-h`, `--help` | Show the generated help and exit. |
| `-t`, `--report-type {awr,statspack}` | Select AWR or STATSPACK. The short input forms `a`, `stat`, `sp`, and `s` are also accepted. Prompts when omitted. |
| `--start "YYYY-MM-DD HH24:MI"` | Set the beginning of the collection range. Prompts only for the start when omitted. |
| `--end "YYYY-MM-DD HH24:MI"` | Set the end of the collection range; it must be later than `--start`. Prompts only for the end when omitted. |
| `--include-alert-log`, `--alert-log` | Include an alert-log excerpt for the requested range. Mutually exclusive with `--no-alert-log`. |
| `--no-alert-log` | Do not include an alert-log excerpt. |
| `--include-execution-plans`, `--execution-plans` | Attach current cursor plans for the automatically selected top elapsed SQL IDs and any IDs supplied with `--sql-id`. Mutually exclusive with `--no-execution-plans`. |
| `--no-execution-plans` | Do not collect SQL execution plans. |
| `--sql-id SQL_ID[,SQL_ID...]`, `--sql-ids SQL_ID[,SQL_ID...]` | Add one or more SQL IDs; the option may be repeated. It implies `--execution-plans` and cannot be combined with `--no-execution-plans`. Values are normalized to lowercase and duplicates are removed. |
| `-p`, `--package-content {both,json,reports}`, `--package-mode {both,json,reports}` | Select ZIP content. `both` is the interactive default; `b`, `j`, `r`, `report`, `awr`, and `full` are accepted input aliases. Prompts when omitted. |
| `-S`, `--security-level {0,1,2}` | Set the [JSON security level](#security-levels). Prompts when JSON is requested or must be generated for execution-plan selection. |
| `--include-os-stats`, `--os-stats` | Include prepared operating-system statistics. Prompts for their source directory unless `--os-stats-dir` is also supplied. Mutually exclusive with `--no-os-stats`. |
| `--no-os-stats` | Do not include operating-system statistics. |
| `--os-stats-dir DIR` | Recursively copy prepared OS-statistics files from `DIR`. It implies `--include-os-stats`, requires a non-empty existing directory, and cannot be combined with `--no-os-stats`. |

When `--execution-plans` is used without `--sql-id`, the collector attaches plans for the top elapsed SQL IDs found in the generated reports and does not ask for manual additions.

Without options, or for required options not provided in a mixed run, the collector asks for:

- report type: AWR or STATSPACK
- date range
- whether to include alert log excerpts
- whether to attach SQL execution plans
- optional additional SQL IDs when execution plans are enabled interactively
- whether to include prepared OS statistics and, if enabled, their source directory
- ZIP package content: reports, JSON, or both
- JSON security level when JSON is requested or needed for execution-plan selection

Common non-interactive examples:

```bash
# Reports-only STATSPACK package, with every optional attachment disabled.
python3 jas-min-collector.py \
  --report-type statspack \
  --start "2026-06-14 00:00" \
  --end "2026-06-15 14:00" \
  --no-alert-log \
  --no-execution-plans \
  --no-os-stats \
  --package-content reports

# AWR reports plus a recursively copied directory of prepared OS statistics.
python3 jas-min-collector.py \
  --report-type awr \
  --start "2026-06-14 00:00" \
  --end "2026-06-15 14:00" \
  --no-alert-log \
  --no-execution-plans \
  --os-stats-dir /path/to/os-stats \
  --package-content reports

# JSON-only AWR package at security level 2, including explicit SQL IDs.
python3 jas-min-collector.py \
  -t awr \
  --start "2026-06-14 00:00" \
  --end "2026-06-15 14:00" \
  --no-alert-log \
  --sql-id abc123,def456 \
  --no-os-stats \
  -p json \
  -S 2
```

OS statistics are attachments, not telemetry collected by this script. On AIX they are copied under `<collection_stem>_attachments/AIX/`; on Linux they are copied under `<collection_stem>_attachments/linux/`. The source directory hierarchy is preserved. Other collector host operating systems are rejected when OS attachments are requested.

When execution plans are requested, the collector parses the generated reports to JAS-MIN JSON even if the ZIP package was set to reports-only. It counts SQL IDs found in `SQLs Ordered by Elapsed time`, selects the top 10 by appearance count, allows extra comma-separated SQL IDs, and writes plans to `<collection_stem>_attachments/<sql_id>.xplan`.

For the automatically selected TOP SQL_IDs (not the manually added IDs), the collector also checks:

```sql
select sql_id, count(distinct child_number)
from v$sql
where sql_id in (...)
group by sql_id
having count(distinct child_number) > 1;
```

Each match is decoded from `V$SQL_SHARED_CURSOR.REASON` into `<collection_stem>_attachments/<sql_id>.shared_cursor_reasons`. Collection is best-effort: a missing/evicted cursor or an unavailable view does not prevent execution plans and the remaining package from being created; the manifest records discovery or per-SQL failures.

Execution plans are fetched with:

```sql
select * from table(dbms_xplan.display_cursor('sqlid',null));
```

The collector creates `jasmin_collect_<collection_stem>/` in the current directory, adding `_2`, `_3`, and so on instead of overwriting an existing collection. The directory contains the generated reports, optional JSON and attachment directory, `manifest.txt`, and `jasmin_package_<collection_stem>.zip`.

The package mode controls the primary report payload:

| Mode | ZIP contents |
|---|---|
| `reports` | Generated AWR/STATSPACK reports. If execution plans are requested, the JSON needed to select top SQL IDs is also produced and included. |
| `json` | Parsed JAS-MIN JSON; generated source reports remain in the collection directory but are not included in the ZIP. |
| `both` | Generated reports and parsed JAS-MIN JSON. |

In every mode, requested alert-log and OS-statistics attachments, available execution plans and decoded child-cursor reasons, and `manifest.txt` are included. The manifest records the selected options, packaged paths, selected SQL IDs, and best-effort collection failures.

## Further Reading

- [JAS-MIN Introduction](https://blog.ora-600.pl/2024/12/13/jas-min/)
- [JAS-MIN and AI](https://blog.ora-600.pl/2025/07/28/jas-min-and-ai/)
- [JAS-MIN Part 1 - Digging Deep into AWR & STATSPACK](https://blog.struktuur.pl/blog/jasmin_part1/)

## Authors

- Kamil Stawiarski - [kamil@ora-600.pl](mailto:kamil@ora-600.pl) - [blog.ora-600.pl](https://blog.ora-600.pl)
- Radoslaw Kut - [radek@ora-600.pl](mailto:radek@ora-600.pl) - [blog.struktuur.pl](https://blog.struktuur.pl)

Built by [ORA-600 | Database Whisperers](https://www.ora-600.pl/en/).

## License

See [LICENSE](LICENSE).

<p align="center">
  <em>If you need expert Oracle performance tuning, reach out to <a href="https://www.ora-600.pl/en/">ora-600.pl</a></em>
</p>
