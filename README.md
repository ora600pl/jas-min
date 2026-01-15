# JAS-MIN

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="/img/jasmin_LOGO_black.png">
  <source media="(prefers-color-scheme: light)" srcset="/img/jasmin_LOGO_white.png">
  <img alt="JAS-MIN logo" src="/img/jasmin_LOGO_white.png">
</picture>

## JSON AWR / STATSPACK Mining & Analysis Tool

**JAS-MIN** is an Oracle performance analysis tool that parses **AWR** (`.html`) and **STATSPACK** (`.txt`) reports into structured JSON and runs statistical + numerical analysis focused on **DB Time**.

Instead of treating DB Time as a flat metric or trusting static *Top-N* lists, JAS-MIN models DB Time as a **multidimensional function** of many variables (waits, stats, SQL workload). To estimate “what moves DB Time the most”, it numerically estimates a **gradient-like sensitivity** using regularized regression:

- **Ridge Regression** (stable, dense signal)
- **Elastic Net Regression** (sparse / selective signal)

On top of that you get correlation, anomaly detection (MAD), snapshot filtering, and optional AI interpretation.

---

## Features

- Parse multiple AWR/STATSPACK reports **in parallel**
- Generate clean **JSON output** for automation / archiving / downstream analysis
- Identify TOP: SQLs, Foreground/Background wait events, IO stats, Segment stats, and more
- Correlate wait events & instance statistics with DB Time (Pearson correlation)
- Detect anomalies using **MAD** (Median Absolute Deviation), optionally in a sliding window
- Optional LLM-based assistant to interpret results (OpenAI / Gemini)
- Configurable security levels for sensitive data handling
- CLI-first, scripting/pipeline friendly

---

## Assumptions

- `.txt` files are treated as STATSPACK reports
- `.html` files are treated as AWR reports
- Best results come from analyzing **a sequence** of reports (directory), not a single snapshot

---

## Quick start guides (articles)

- https://blog.struktuur.pl/blog/jasmin_part1/
- https://blog.struktuur.pl/blog/jasmin_part2/
- https://blog.ora-600.pl/2025/07/28/jas-min-and-ai/

---

## Installation

Build from source (example):

```bash
cargo build --release
```

Binary will be in:

```bash
./target/release/jas-min
```

---

## Usage

```bash
jas-min [OPTIONS]
```

### Options (full list)

> Below is the current CLI help output formatted for README, including descriptions and defaults.

#### Input

- `--file <FILE>`  
  Parse a single text or html file  
  **Default:** empty

- `-d, --directory <DIRECTORY>`  
  Parse whole directory of files  
  **Default:** empty

- `-j, --json-file <JSON_FILE>`  
  Analyze provided JSON file  
  **Default:** empty

#### Output

- `-o, --outfile <OUTFILE>`  
  Write output to nondefault file. Default is `directory_name.json`  
  **Default:** empty

- `-q, --quiet`  
  Suppress terminal output (still writes to log file)

#### Filters / ranges

- `-f, --filter-db-time <FILTER_DB_TIME>`  
  Filter only for DBTIME greater than (0 disables the filter)  
  **Default:** `0`

- `-s, --snap-range <SNAP_RANGE>`  
  Filter snapshots by SNAP IDs in format `BEGIN_ID-END_ID`  
  **Default:** `0-666666666`

- `-t, --time-cpu-ratio <TIME_CPU_RATIO>`  
  Ratio of `DB CPU / DB TIME` used for certain heuristic selections  
  **Default:** `0.666`

#### SQL selection

- `-i, --id-sqls <ID_SQLS>`  
  Include indicated SQL_IDs as TOP SQL in format `SQL_ID1,SQL_ID2,...` (experimental)  
  **Default:** empty

- `SQLsI (by elapsed time)`  
  JAS-MIN ranks SQLs primarily by **Elapsed Time** unless your workflow overrides it.

#### AI / Assistant

- `-a, --ai <AI>`  
  Use an AI model to interpret collected statistics and describe them.  
  Requires env var: `OPENAI_API_KEY` or `GEMINI_API_KEY`.  
  Format: `VENDOR:MODEL_NAME:LANGUAGE_CODE`  
  Example: `openai:gpt-4-turbo:PL` or `google:gemini-2.0-flash:PL`  
  **Default:** empty

- `-b, --backend-assistant <BACKEND_ASSISTANT>`  
  Launch the backend agent used by the JASMIN Assistant.  
  Value: `<openai>` or `<google:model>`  
  Config (API keys, PORT, etc.) loaded from `.env`  
  **Default:** empty

- `-u, --url-context-file <URL_CONTEXT_FILE>`  
  (Gemini) Provide a file with URLs for Gemini URL context tool  
  **Default:** empty

- `-D, --deep-check <DEEP_CHECK>`  
  Ask the AI to perform a deep analysis of detailed JSON statistics by proposing top-N SNAPs and analyzing all statistics from that period  
  **Default:** `0`

- `-B, --tokens-budget <TOKENS_BUDGET>`  
  Token budget for modular LLM analysis (minimize token usage)  
  **Default:** `80000`

- `-C, --token-count-factor <TOKEN_COUNT_FACTOR>`  
  Base output token count is `8192`; multiply by this factor to set maximum output tokens  
  **Default:** `8`

#### Anomaly detection (MAD)

- `-m, --mad-threshold <MAD_THRESHOLD>`  
  Threshold for detecting anomalies using MAD  
  **Default:** `7`

- `-W, --mad-window-size <MAD_WINDOW_SIZE>`  
  Window size for MAD local sliding window, specified as `% of probes`  
  **Default:** `100`

#### Parallelism

- `-P, --parallel <PARALLEL>`  
  Parallelism level  
  **Default:** `4`

#### Security / privacy

- `-S, --security-level <SECURITY_LEVEL>`  
  Security level for sensitive data storage:  
  - `0` – highest security: store no object names, database names, or other sensitive data  
  - `1` – store `segment_names` from Segment Statistics  
  - `2` – store full SQL Text from AWR reports  
  **Default:** `0`

#### Gradient model parameters

- `-R, --ridge-lambda <RIDGE_LAMBDA>`  
  L2 regularization strength for Ridge (>= 0)  
  **Default:** `50`

- `-E, --en-lambda <EN_LAMBDA>`  
  Overall regularization strength for Elastic Net (>= 0)  
  **Default:** `30`

- `-A, --en-alpha <EN_ALPHA>`  
  Elastic Net L1/L2 mixing:  
  - `alpha = 1.0` → Lasso (pure L1)  
  - `alpha = 0.0` → Ridge-like (pure L2)  
  **Default:** `0.666`

- `-I, --en-max-iter <EN_MAX_ITER>`  
  Max iterations for coordinate descent in Elastic Net  
  **Default:** `5000`

- `-T, --en-tol <EN_TOL>`  
  Convergence tolerance for coefficient change in Elastic Net  
  **Default:** `0.000001`

#### Misc

- `-h, --help`  
  Print help

- `-V, --version`  
  Print version

---

## Examples

### Parse a directory of reports and write JSON

```bash
jas-min -d ./awr_dir -o output.json
```

### Filter by DB Time threshold

```bash
jas-min -d ./awr_dir -f 1000
```

### Restrict to a SNAP range

```bash
jas-min -d ./awr_dir -s 1200-1250
```

### Run with AI interpretation (OpenAI)

```bash
export OPENAI_API_KEY="..."
jas-min -d ./awr_dir -a openai:gpt-4-turbo:PL
```

### Run with AI interpretation (Gemini + URL context)

```bash
export GEMINI_API_KEY="..."
jas-min -d ./awr_dir -u urls.txt -a google:gemini-2.0-flash:PL
```

### Increase AI output budget (carefully)

```bash
jas-min -d ./awr_dir -a openai:gpt-4-turbo:EN -C 12 -B 120000
```

---

## Notes

- If you run analysis with plotting/HTML generation in your workflow, the output is typically an interactive dashboard based on Plotly (depending on build/features).
- To read `reasonnings.txt` and `.env` from a directory other than the current working directory, set `JASMIN_HOME`.

---

## Project status

Actively developed. Used on real-world systems. Expect sharp edges.

---

## Authors

- **Kamil Stawiarski** <kamil@ora-600.pl>  
- **Radosław Kut** <radek@ora-600.pl>

