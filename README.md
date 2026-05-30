<p align="center">
  <img src="https://raw.githubusercontent.com/rakustow/jas-min/main/img/jasmin_LOGO_white.png" width="300" alt="JAS-MIN Logo"/>
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
| Cached analysis | Re-analyzes an existing JAS-MIN JSON file with `--json-file`. |
| HTML dashboard | Generates `<input>.html_reports/jasmin_main.html` and detail pages for waits, SQL IDs, statistics, I/O, latches, segments, anomalies, and gradients. |
| Peak detection | Marks snapshots where `DB CPU / DB Time` is below `--time-cpu-ratio`, optionally requiring DB Time above `--filter-db-time`. |
| Snap filtering | Limits analysis to a snapshot range with `--snap-range BEGIN-END`. |
| Anomalies | Uses MAD-based anomaly detection with configurable threshold, sliding-window percentage, and optional per-cluster trimming. |
| Correlation | Computes Pearson correlations between DB Time and wait events, SQL elapsed time, and instance statistics. |
| Gradient analysis | Runs Ridge, Elastic Net, Huber, and Quantile-95 regression models over DB Time and DB CPU drivers. |
| Custom gradient | Builds extra gradient pages for a selected SQL ID or wait event with `--gradient-custom`. |
| AI reports | Supports OpenAI, Google Gemini, OpenRouter, OpenRouter small-context modular mode, and local OpenAI-compatible models. |
| AI tools mode | Enables function/tool-call loops for OpenAI, Google Gemini, and OpenRouter with `--tools-mode`. |
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

- Rust toolchain, recommended 1.75 or newer.

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
jas-min -d ./awr_reports -m 10 -W 25 --top-cluster-anomalies 5
```

`-m` is a numeric TOPn/MAD cutoff used by the current anomaly logic. `-W` is the local sliding-window size as a percentage of probes. `100` means global behavior.

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
| `openroutersmall` | Modular pipeline through OpenRouter | `OPENROUTER_API_KEY` |
| `local` | Modular pipeline through an OpenAI-compatible local endpoint | `LOCAL_BASE_URL`, optional `LOCAL_API_KEY` |

Examples:

```bash
export GEMINI_API_KEY="your-key"
jas-min -d ./awr_reports --ai google:gemini-2.5-flash:EN

export OPENAI_API_KEY="your-key"
jas-min -d ./awr_reports --ai openai:o3:EN

export OPENROUTER_API_KEY="your-key"
jas-min -d ./awr_reports --ai openrouter:anthropic/claude-sonnet-4:EN

export LOCAL_BASE_URL="http://localhost:1234/v1/chat/completions"
export LOCAL_API_KEY="lm-studio"
jas-min -d ./awr_reports --ai local:qwen3-32b:EN -B 60000
```

The language code, for example `EN` or `PL`, controls the requested report language.

### Tools Mode

For `openai`, `google`, and `openrouter`, `--tools-mode` enables an iterative tool-call loop. In this mode the model can request focused diagnostic data from the parsed collection instead of relying only on the initial summary.

```bash
jas-min -d ./awr_reports --ai openai:o3:EN --tools-mode --max-tool-iterations 12
jas-min -d ./awr_reports --ai google:gemini-2.5-flash:EN --tools-mode
jas-min -d ./awr_reports --ai openrouter:openai/gpt-4.1:EN --tools-mode
```

If a sibling `<stem>_attachments/` directory exists, tools mode can also expose execution-plan attachments to the model.

### One-Shot Batch Analysis

For `google`, `openai`, and `openrouter`, JAS-MIN can send the compact `ReportForAI` structure plus Load Profile statistics to the selected model in one call. The report payload is serialized as TOON/JSON-like text to keep token usage lower than pretty JSON while preserving structure.

Typical output files are named after the text log and model, for example:

```text
awr_reports.txt_gemini.md
awr_reports.txt_o3.md
awr_reports.txt_anthropic_claude-sonnet-4.md
```

The generated Markdown is converted to HTML with links back to JAS-MIN detail pages where possible.

### Modular LLM Pipeline

For `openroutersmall` and `local`, JAS-MIN uses a modular pipeline designed for smaller context windows:

1. Split `ReportForAI` into focused sections such as baseline, foreground waits, background waits, SQLs, I/O, latches, segment statistics, correlations, anomaly clusters, and gradient sections.
2. Attach a compact context capsule with general data and top spikes to each section.
3. Trim section payloads to fit `--tokens-budget`.
4. Ask the model for per-section notes.
5. Compose the section notes into a final Markdown report.

This is slower than one-shot mode, but it can produce useful reports with local or smaller-context models.

### URL Context and Custom Reasoning

`--url-context-file` loads a JSON file used to add URL instructions for matching events or SQL IDs. The file is mainly useful with Gemini URL context workflows.

```bash
jas-min -d ./awr_reports --ai google:gemini-2.5-flash:EN -u url_context.json
```

JAS-MIN also appends `reasonings.txt` to AI prompts when the file exists. If `JASMIN_HOME` is set, it reads `$JASMIN_HOME/reasonings.txt`; otherwise it tries `./reasonings.txt`.

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

An observation is treated as anomalous when its MAD score is above the configured cutoff. In the current CLI this is controlled by `-m, --mad-threshold`, whose default is `10`.

`-W, --mad-window-size` controls whether MAD is global or local:

- `-W 100` uses the whole time series as the reference population.
- Values below `100` use a sliding local window expressed as a percentage of probes, which helps detect anomalies relative to nearby behavior instead of the whole observation period.

MAD analysis is applied to areas such as foreground and background wait events, SQL elapsed time, Load Profile metrics, instance activity statistics, dictionary cache, library cache, latch activity, and time model statistics. `--top-cluster-anomalies` can additionally trim anomaly clusters to the top N anomalies per category per snapshot.

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
LOCAL_BASE_URL=http://localhost:1234/v1/chat/completions

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
  -d, --directory <DIRECTORY>                Parse a directory of report files
  -o, --outfile <OUTFILE>                    Write parsed JSON to a non-default file
  -t, --time-cpu-ratio <TIME_CPU_RATIO>      DB CPU / DB Time threshold [default: 0.666]
  -f, --filter-db-time <FILTER_DB_TIME>      Ignore peaks below this DB Time [default: 0]
  -i, --id-sqls <ID_SQLS>                    Include comma-separated SQL_IDs in TOP SQL
  -j, --json-file <JSON_FILE>                Analyze an existing JAS-MIN JSON file
  -s, --snap-range <SNAP_RANGE>              Snapshot filter BEGIN-END [default: 0-666666666]
  -q, --quiet                                Suppress terminal output, still write log
  -a, --ai <AI>                              AI mode: VENDOR:MODEL:LANG
  -m, --mad-threshold <MAD_THRESHOLD>        TOPn/MAD anomaly parameter [default: 10]
  -W, --mad-window-size <MAD_WINDOW_SIZE>    MAD window size as percent of probes [default: 100]
      --top-cluster-anomalies <N>            Keep top N anomalies per category per snapshot [default: 0]
  -P, --parallel <PARALLEL>                  Rayon parallelism level [default: 4]
  -S, --security-level <SECURITY_LEVEL>      Security level: 0, 1, or 2 [default: 0]
  -u, --url-context-file <URL_CONTEXT_FILE>  URL context JSON file
  -B, --tokens-budget <TOKENS_BUDGET>        Token budget for modular LLM analysis [default: 80000]
  -R, --ridge-lambda <RIDGE_LAMBDA>          Ridge L2 regularization [default: 50]
  -E, --en-lambda <EN_LAMBDA>                Elastic Net regularization [default: 30]
  -A, --en-alpha <EN_ALPHA>                  Elastic Net L1/L2 mix [default: 0.333]
  -I, --en-max-iter <EN_MAX_ITER>            Elastic Net max iterations [default: 5000]
  -T, --en-tol <EN_TOL>                      Elastic Net tolerance [default: 0.000001]
      --top-gradient <TOP_GRADIENT>          Top N rows per regression model [default: 10]
  -c, --convert-md2html <CONVERT_MD2HTML>    Convert Markdown to HTML without AI call
  -G, --gradient-custom <GRADIENT_CUSTOM>    Custom gradient: SQL=<sql_id> or WAIT=<event>
      --tools-mode                           Enable AI tools mode for OpenAI/Google/OpenRouter
      --max-tool-iterations <N>              Max tool-call iterations [default: 10]
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

For useful statistics, collect a meaningful run of consecutive reports. A week or more is usually better than a few isolated snapshots.

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
