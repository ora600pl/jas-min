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
| AI tools mode | Enables function/tool-call loops for OpenAI and OpenRouter with `--tools-mode`. |
| Security | Controls whether object names and SQL text are stored with `--security-level`. |

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

For `openai` and `openrouter`, `--tools-mode` enables an iterative tool-call loop. In this mode the model can request focused diagnostic data from the parsed collection instead of relying only on the initial summary.

```bash
jas-min -d ./awr_reports --ai openai:o3:EN --tools-mode --max-tool-iterations 12
jas-min -d ./awr_reports --ai openrouter:openai/gpt-4.1:EN --tools-mode
```

If a sibling `<stem>_attachments/` directory exists, tools mode can also expose execution-plan attachments to the model.

### URL Context and Custom Reasoning

`--url-context-file` loads a JSON file used to add URL instructions for matching events or SQL IDs. The file is mainly useful with Gemini URL context workflows.

```bash
jas-min -d ./awr_reports --ai google:gemini-2.5-flash:EN -u url_context.json
```

JAS-MIN also appends `reasonings.txt` to AI prompts when the file exists. If `JASMIN_HOME` is set, it reads `$JASMIN_HOME/reasonings.txt`; otherwise it tries `./reasonings.txt`.

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
      --tools-mode                           Enable AI tools mode for OpenAI/OpenRouter
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
