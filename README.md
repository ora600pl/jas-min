<p align="center">
  <img src="https://raw.githubusercontent.com/rakustow/jas-min/main/img/jasmin_LOGO_white.png" width="300" alt="JAS-MIN Logo"/>
</p>

<h1 align="center">JAS-MIN — JSON AWR & Statspack Miner</h1>

<p align="center">
  <strong>Oracle Database Performance Analysis Tool</strong><br/>
  Parse · Analyze · Visualize · Consult AI
</p>


---

## Table of Contents

- [What is JAS-MIN?](#what-is-jas-min)
- [Key Features](#key-features)
- [Architecture Overview](#architecture-overview)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Usage Reference](#usage-reference)
  - [Parsing & Analysis](#parsing--analysis)
  - [Snap Range Filtering](#snap-range-filtering)
  - [Security Levels](#security-levels)
  - [Anomaly Detection (MAD)](#anomaly-detection-mad)
  - [Gradient Analysis](#gradient-analysis)
  - [AI Integration](#ai-integration)
  - [JAS-MIN Assistant (Backend Chat)](#jas-min-assistant-backend-chat)
- [Statistical Algorithms](#statistical-algorithms)
  - [DB CPU / DB Time Ratio Analysis](#db-cpu--db-time-ratio-analysis)
  - [Median Absolute Deviation (MAD)](#median-absolute-deviation-mad)
  - [Pearson Correlation Coefficient](#pearson-correlation-coefficient)
  - [Bonferroni-Corrected Significance Threshold](#bonferroni-corrected-significance-threshold)
  - [Multi-Model Gradient Regression](#multi-model-gradient-regression)
  - [Cross-Model Triangulation](#cross-model-triangulation)
  - [Descriptive Statistics](#descriptive-statistics)
- [AI Model Integration](#ai-model-integration)
  - [Supported Vendors](#supported-vendors)
  - [One-Shot Batch Analysis](#one-shot-batch-analysis)
  - [Modular LLM Pipeline](#modular-llm-pipeline)
  - [Deep-Check Mode (Gemini)](#deep-check-mode-gemini)
  - [Backend Assistant](#backend-assistant)
  - [ReportForAI Data Structure](#reportforai-data-structure)
  - [Custom Reasonings & URL Context](#custom-reasonings--url-context)
- [Output Structure](#output-structure)
- [Environment Variables](#environment-variables)
- [CLI Reference](#cli-reference)
- [Further Reading](#further-reading)
- [Authors](#authors)
- [License](#license)

---

## What is JAS-MIN?

**JAS-MIN** (JSON AWR & Statspack Miner) is a high-performance Oracle Database performance analysis tool written in **Rust**. It parses hundreds of AWR (`.html`) and STATSPACK (`.txt`) reports, converts them into structured JSON, and runs a comprehensive suite of statistical and numerical analyses focused on **DB Time decomposition**.

Instead of manually combing through verbose report files, JAS-MIN produces a single interactive HTML dashboard with Plotly-based visualizations, statistical summaries, anomaly detection, correlation analysis, multi-model gradient regression, and optional AI-generated interpretations.

> Named after **Jasmin Fluri** — one of the SOUC founders — when the tool was first introduced at SOUC Database Circle 2024.

---

## Key Features

| Category | Capabilities |
|---|---|
| **Parsing** | Parallel parsing of AWR (`.html`) and STATSPACK (`.txt`) report directories into a unified JSON format. Supports Oracle 11g through 23ai report formats. |
| **Visualization** | Interactive Plotly HTML dashboards: time-series, heatmaps, histograms, box plots for wait events, SQL statistics, Load Profile, I/O stats, Instance Efficiency, Latch Activity, Segment Statistics. |
| **Anomaly Detection** | Median Absolute Deviation (MAD) with configurable thresholds and sliding window across wait events, SQL elapsed times, Load Profile, Instance Statistics, Dictionary Cache, Library Cache, Latch Activity, and Time Model. |
| **Correlation** | Pearson correlation between DB Time and every instance statistic, wait event, and SQL, with Bonferroni-corrected significance thresholds. |
| **Gradient Analysis** | Four-model regression suite (Ridge, Elastic Net, Huber, Quantile-95) to determine which wait events, statistics, and SQL statements most influence DB Time and DB CPU changes. |
| **Cross-Model Triangulation** | Automated classification of bottlenecks by cross-referencing all four regression models (CONFIRMED_BOTTLENECK, TAIL_RISK, OUTLIER_DRIVEN, etc.). |
| **AI Integration** | One-shot analysis via OpenAI, Google Gemini, or OpenRouter; modular multi-step pipeline for smaller-context models; local model support (LM Studio, Ollama); interactive backend assistant chat. |
| **Security** | Three-tier security model controlling exposure of object names, SQL text, and other sensitive data in the JSON output. |
| **Parallelism** | Rayon-based parallel file parsing and anomaly detection with configurable thread count. |

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     AWR / STATSPACK Reports                     │
│                   (.html files / .txt files)                    │
└─────────────────┬───────────────────────────────────────────────┘
                  │ parallel parsing (rayon)
                  ▼
┌─────────────────────────────────────────────────────────────────┐
│                   AWRSCollection (JSON)                         │
│  ┌──────────┐ ┌───────────┐ ┌──────────┐ ┌──────────────────┐   │
│  │SnapInfo  │ │LoadProfile│ │WaitEvents│ │ SQL Elapsed/CPU  │   │
│  │HostCPU   │ │TimeModel  │ │ FG / BG  │ │ IO/Gets/Reads    │   │
│  │InstStats │ │Efficiency │ │Histograms│ │ ASH Top Events   │   │
│  │IOStats   │ │DictCache  │ │LibCache  │ │ Segment Stats    │   │
│  │LatchAct  │ │RedoLog    │ │WaitClass │ │ Init Parameters  │   │
│  └──────────┘ └───────────┘ └──────────┘ └──────────────────┘   │
└─────────────────┬───────────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Analysis Engine                              │
│                                                                 │
│  ┌────────────────┐  ┌───────────────┐  ┌───────────────────┐   │
│  │ Peak Detection │  │ MAD Anomaly   │  │ Pearson           │   │
│  │ (CPU/Time      │  │ Detection     │  │ Correlation       │   │
│  │  Ratio)        │  │ (sliding      │  │ (Bonferroni       │   │
│  │                │  │  window)      │  │  corrected)       │   │
│  └────────────────┘  └───────────────┘  └───────────────────┘   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │ Multi-Model Gradient Regression                            │ │
│  │  Ridge · Elastic Net · Huber (IRLS) · Quantile-95 (IRLS)   │ │
│  │  → Cross-Model Triangulation                               │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────┬───────────────────────────────────────────────┘
                  │
          ┌───────┴────────┐
          ▼                ▼
┌──────────────┐   ┌──────────────────┐
│  HTML Report │   │  ReportForAI     │
│  (Plotly     │   │  (TOON/JSON)     │
│  dashboard)  │   │       │          │
│              │   │       ▼          │
│  + TXT log   │   │  AI Integration  │
│  + CSV       │   │  (OpenAI/Gemini/ │
│    anomalies │   │   OpenRouter/    │
│              │   │   Local LLM)     │
└──────────────┘   └──────────────────┘
```

---

## Installation

### Prerequisites

- **Rust** toolchain (1.75+ recommended): [rustup.rs](https://rustup.rs/)

### Build from Source

```bash
git clone https://github.com/ora600pl/jas-min.git
cd jas-min
cargo build --release
```

The binary will be at `./target/release/jas-min`.

### Generating Test Reports

- **STATSPACK**: Use the included `gen_statspack_reps.sh` script.
- **AWR**: Use [awr-generator.sql](https://github.com/flashdba) by @flashdba.

> You need at least a week of reports (ideally more) to make meaningful statistical analysis.

---

## Quick Start

### 1. Parse a directory of AWR/STATSPACK reports

```bash
jas-min -d ./awr_reports
```

This will:
- Parse all `.html` (AWR) and `.txt` (STATSPACK) files in `./awr_reports/`
- Produce `awr_reports.json` (structured data)
- Produce `awr_reports.txt` (text log)
- Generate `awr_reports.html_reports/` directory with the interactive HTML dashboard
- Open the main report in your default browser

### 2. Re-analyze from cached JSON (skip re-parsing)

```bash
jas-min -j awr_reports.json
```

### 3. Analyze with AI interpretation

```bash
# Using Google Gemini
export GEMINI_API_KEY="your-key"
jas-min -d ./awr_reports --ai google:gemini-2.5-flash:EN

# Using OpenAI
export OPENAI_API_KEY="your-key"
jas-min -d ./awr_reports --ai openai:o3:EN

# Using OpenRouter
export OPENROUTER_API_KEY="your-key"
jas-min -d ./awr_reports --ai openrouter:anthropic/claude-sonnet-4:EN

# Using a local model (LM Studio / Ollama)
export LOCAL_API_KEY="..."
export LOCAL_BASE_URL="http://localhost:1234/v1/chat/completions"
jas-min -d ./awr_reports --ai local:my-model:EN
```

### 4. Launch interactive AI assistant

```bash
# Create .env file with PORT and API keys first
jas-min -d ./awr_reports -b google:gemini-2.5-flash
```

---

## Usage Reference

### Parsing & Analysis

| Flag | Description | Default |
|---|---|---|
| `-d, --directory <DIR>` | Parse all reports in the given directory | — |
| `-j, --json-file <FILE>` | Analyze a previously generated JSON file | — |
| `--file <FILE>` | Parse a single report file and print JSON to stdout | — |
| `-o, --outfile <FILE>` | Write JSON output to a non-default file | `<dirname>.json` |
| `-P, --parallel <N>` | Parallelism level for file parsing | `4` |
| `-q, --quiet` | Suppress terminal output (still writes to log file) | `false` |

### Snap Range Filtering

```bash
jas-min -d ./reports -s 1000-2000
```

| Flag | Description | Default |
|---|---|---|
| `-s, --snap-range <BEGIN-END>` | Filter analysis to a specific snap ID range | `0-666666666` |

### Security Levels

| Level | Flag | Description |
|---|---|---|
| **0** | `-S 0` | Maximum security: no object names, database names, or sensitive data stored |
| **1** | `-S 1` | Stores segment names from Segment Statistics sections |
| **2** | `-S 2` | Stores full SQL text from AWR reports |

### Anomaly Detection (MAD)

| Flag | Description | Default |
|---|---|---|
| `-m, --mad-threshold <FLOAT>` | MAD score threshold for flagging anomalies | `7.0` |
| `-W, --mad-window-size <PCT>` | Sliding window size as percentage of total probes (100 = global) | `100` |

```bash
# Use a 10% sliding window with threshold 5
jas-min -d ./reports -W 10 -m 5
```

### Gradient Analysis

| Flag | Description | Default |
|---|---|---|
| `-R, --ridge-lambda <FLOAT>` | L2 regularization strength for Ridge regression | `50.0` |
| `-E, --en-lambda <FLOAT>` | Overall regularization strength for Elastic Net | `30.0` |
| `-A, --en-alpha <FLOAT>` | L1/L2 mixing: 1.0 = Lasso (pure L1), 0.0 = Ridge-like (pure L2) | `0.666` |
| `-I, --en-max-iter <N>` | Max iterations for Elastic Net coordinate descent | `5000` |
| `-T, --en-tol <FLOAT>` | Convergence tolerance for Elastic Net | `1e-6` |

### AI Integration

| Flag | Description | Default |
|---|---|---|
| `-a, --ai <VENDOR:MODEL:LANG>` | Run AI-powered interpretation after analysis | — |
| `-C, --token-count-factor <N>` | Multiply base output token count (8192) by this factor | `8` |
| `-B, --tokens-budget <N>` | Token budget for modular LLM analysis | `80000` |
| `-D, --deep-check <N>` | Ask AI to deep-analyze top-N snapshots (Gemini only) | `0` |
| `-u, --url-context-file <FILE>` | Provide URL context file for Gemini URL context tool | — |

### JAS-MIN Assistant (Backend Chat)

```bash
# Google Gemini backend
jas-min -d ./reports -b google:gemini-2.5-flash

# OpenAI backend (requires OPENAI_ASST_ID in .env)
jas-min -d ./reports -b openai
```

| Flag | Description |
|---|---|
| `-b, --backend-assistant <TYPE:MODEL>` | Launch the interactive assistant backend (`openai` or `google:model`) |

---

## Statistical Algorithms

### DB CPU / DB Time Ratio Analysis

JAS-MIN identifies performance peaks by computing the ratio of **DB CPU** to **DB Time** from the Load Profile section of each snapshot.

$$R = \frac{\text{DB CPU (s/s)}}{\text{DB Time (s/s)}}$$

- $R \approx 1.0$: CPU-bound workload — sessions spend most time on CPU.
- $R < 0.666$ (default threshold): Wait-bound workload — significant time is spent on wait events rather than CPU.
- The threshold is configurable via `-t, --time-cpu-ratio`.

When $R$ falls below the threshold **and** DB Time exceeds the optional filter (`-f, --filter-db-time`), that snapshot is marked as a **peak period**. The top wait events, background events, and SQL statements from each peak snapshot are selected for deeper analysis.

### Median Absolute Deviation (MAD)

MAD is used as a robust anomaly detection method across multiple data domains. Unlike standard deviation, MAD is resistant to outliers, making it ideal for performance data with bursty patterns.

**Computation:**

Given a time series $X = \{x_1, x_2, \ldots, x_n\}$:

1. Compute the median: $\tilde{x} = \text{median}(X)$
2. Compute absolute deviations: $d_i = |x_i - \tilde{x}|$
3. Compute MAD: $\text{MAD} = \text{median}(\{d_1, d_2, \ldots, d_n\})$
4. Compute the MAD score for each observation: $z_i = \frac{|x_i - \tilde{x}|}{\text{MAD}}$
5. Flag as anomaly if $z_i > \text{threshold}$ (default: 7.0)

**Sliding Window Mode** (`-W <PCT>`):

When the window size is less than 100%, JAS-MIN uses a sliding window approach: for each observation $x_i$, the MAD is computed only from the local neighborhood of size $w$ centered on $i$. This detects anomalies relative to local behavior rather than global behavior, and runs in parallel across statistics using Rayon.

**Applied to:**

- Foreground & Background Wait Events (total wait time)
- SQL Elapsed Times
- Load Profile metrics (per-second rates)
- Instance Activity Statistics
- Dictionary Cache (get requests)
- Library Cache (pin requests)
- Latch Activity (get requests)
- Time Model Statistics (time in seconds)

### Pearson Correlation Coefficient

The Pearson correlation coefficient $r$ measures the linear relationship between two time series. JAS-MIN computes $r$ between DB Time and:

- Every foreground/background wait event's total wait time
- Every SQL statement's elapsed time
- Every instance activity statistic

$$r = \frac{\sum_{i=1}^{n}(x_i - \bar{x})(y_i - \bar{y})}{\sqrt{\sum_{i=1}^{n}(x_i - \bar{x})^2} \cdot \sqrt{\sum_{i=1}^{n}(y_i - \bar{y})^2}}$$

Additionally, JAS-MIN computes pairwise correlations between each SQL's elapsed time and every foreground wait event to identify which wait events most strongly co-occur with specific SQL statements.

### Bonferroni-Corrected Significance Threshold

When correlating a large number of instance statistics with DB Time, JAS-MIN applies the Bonferroni correction to control the family-wise error rate:

$$r_{\text{threshold}} = \max\left(0.5,\; r_{\text{bonferroni}}(k, \alpha, n)\right)$$

Where $k$ is the number of statistics tested and $\alpha = 0.05$. Only statistics exceeding $|r| \geq r_{\text{threshold}}$ are reported.

### Multi-Model Gradient Regression

To determine **what drives DB Time changes**, JAS-MIN computes first-order differences (deltas) of both DB Time and each feature (wait event, statistic, SQL elapsed time), standardizes them, and fits four regression models:

**Pre-processing:**

1. Compute deltas: $\Delta y_t = y_{t+1} - y_t$ (DB Time), $\Delta x_{j,t} = x_{j,t+1} - x_{j,t}$ (features)
2. Standardize each feature's deltas: $\hat{x}_{j,t} = \frac{\Delta x_{j,t} - \mu_j}{\sigma_j}$
3. Compute MAD of raw deltas for impact scaling

**Four Regression Models:**

| Model | Method | Purpose |
|---|---|---|
| **Ridge** | Closed-form via normal equations + L2 penalty $(A^TA + \lambda I)\beta = A^Ty$ | Dense, stabilized ranking of all contributing factors |
| **Elastic Net** | Coordinate descent with L1+L2 penalty: $\min \frac{1}{2n}\|y - X\beta\|^2 + \lambda\alpha\|\beta\|_1 + \frac{\lambda(1-\alpha)}{2}\|\beta\|^2$ | Sparse ranking highlighting dominant factors; handles collinearity by zeroing out redundant features |
| **Huber** | Iteratively Reweighted Least Squares (IRLS) with Huber loss ($\delta = 1.345 \cdot \text{MAD}(\text{residuals})$) | Outlier-resistant ranking; downweights extreme snapshots |
| **Quantile 95** | IRLS with asymmetric check-loss function ($\tau = 0.95$) | Models the worst 5% of snapshots (tail risk) |

**Impact Score:**

$$\text{impact}_j = |\beta_j| \cdot \text{MAD}(\Delta x_j)$$

This quantifies the expected shift in DB Time for a typical perturbation in feature $j$.

**Applied to six gradient sections:**

1. **DB Time** vs. Foreground Wait Events (wait seconds)
2. **DB Time** vs. Instance Statistics — Counters
3. **DB Time** vs. Instance Statistics — Volumes (bytes)
4. **DB Time** vs. Instance Statistics — Time metrics
5. **DB Time** vs. SQL Elapsed Time
6. **DB CPU** vs. Instance Statistics — CPU-related
7. **DB CPU** vs. SQL CPU Time

### Cross-Model Triangulation

After fitting all four models, JAS-MIN automatically classifies each feature by checking its presence in the Top-N of each model (positive gradient coefficient and non-zero impact):

| Classification | Models Present | Interpretation | Priority |
|---|---|---|---|
| `CONFIRMED_BOTTLENECK` | All 4 | Systematic, robust bottleneck | **CRITICAL** |
| `CONFIRMED_BOTTLENECK_EN_COLLINEAR` | Ridge + Huber + Q95 | Bottleneck masked by L1 collinearity | **CRITICAL** |
| `STRONG_CONTRIBUTOR` | Ridge + EN + Huber | Reliable systematic contributor | MEDIUM |
| `STABLE_CONTRIBUTOR` | Ridge + Huber | Steady background contributor | LOW-MEDIUM |
| `TAIL_RISK` | Q95 only (not Ridge) | Rare catastrophic spikes | **HIGH** |
| `TAIL_OUTLIER` | Ridge + Q95 (not Huber) | Extreme snapshots that ARE the worst periods | HIGH |
| `OUTLIER_DRIVEN` | Ridge only (not Huber) | Impact from a few extreme snapshots | MEDIUM |
| `SPARSE_DOMINANT` | EN only (not Ridge) | Dominant among correlated group | MEDIUM |
| `ROBUST_ONLY` | Huber only | Background factor visible only without outliers | LOW |

### Descriptive Statistics

For wait events, SQL statements, and Load Profile metrics, JAS-MIN computes:

- **Mean** ($\bar{x}$), **Standard Deviation** ($\sigma$)
- **Median**, **Q1**, **Q3**, **IQR**, **Lower/Upper Fences**
- **Min**, **Max**, **Variance**
- **Weighted averages** for latch contention metrics (weighted by get requests)

---

## AI Model Integration

### Supported Vendors

| Vendor | Env Variable | Flag Format |
|---|---|---|
| **OpenAI** | `OPENAI_API_KEY` | `openai:gpt-4-turbo:EN` |
| **Google Gemini** | `GEMINI_API_KEY` | `google:gemini-2.5-flash:EN` |
| **OpenRouter** | `OPENROUTER_API_KEY` | `openrouter:anthropic/claude-sonnet-4:EN` |
| **OpenRouter (modular)** | `OPENROUTER_API_KEY` | `openroutersmall:model-name:EN` |
| **Local (LM Studio, Ollama)** | `LOCAL_API_KEY`, `LOCAL_BASE_URL` | `local:model-name:EN` |

The language code (`EN`, `PL`, etc.) controls the output language of the AI-generated report.

### One-Shot Batch Analysis

For models with large context windows (Gemini, OpenAI, OpenRouter), JAS-MIN sends the entire **ReportForAI** structure (serialized as [TOON](https://github.com/) — a compact JSON-like format) along with a comprehensive system prompt to a single API call.

```bash
jas-min -d ./reports --ai google:gemini-2.5-flash:EN
```

The system prompt includes:
- Complete role description and analytical methodology (6-step reasoning)
- Gradient analysis interpretation rules with cross-model classification table
- Output structure specification (11-section Markdown report)
- Initialization parameter analysis instructions with source requirements

Output: `<logfile>_gemini.md` → auto-converted to `.html` with interlinks.

### Modular LLM Pipeline

For models with smaller context windows (`openroutersmall:` or `local:`), JAS-MIN uses a **modular multi-step pipeline**:

1. **Section Extraction**: The ReportForAI is split into ~20 independent sections (Baseline, FG Waits, BG Waits, SQLs, I/O, Latches, 8× Segments, Stats Correlation, Load Profile Anomalies, Anomaly Clusters, 5× Gradients).
2. **Budget-Aware Trimming**: Each section is trimmed to fit within the configured `--tokens-budget` using binary search over the number of items.
3. **Context Capsule**: A compact summary (general_data + top spikes) is attached to every section call as a temporal anchor.
4. **Per-Section Analysis**: Each section is sent to the LLM as an independent call with the system prompt + context capsule + section data.
5. **Composition**: All section notes are bundled and sent in a final compose step to produce the unified Markdown report.

```bash
jas-min -d ./reports --ai local:qwen3-32b:EN -B 60000
```

### Deep-Check Mode (Gemini)

With `-D <N>`, JAS-MIN asks Gemini to select the top-N most critical snapshots, then sends full AWR JSON data for each snapshot for deep-dive analysis:

```bash
jas-min -d ./reports --ai google:gemini-2.5-flash:EN -D 5
```

### Backend Assistant

The assistant mode launches a local HTTP server (Axum) that proxies chat messages between the browser-based JAS-MIN dashboard and the AI backend:

```
Browser (JAS-MIN HTML) ←→ localhost:<PORT>/api/chat ←→ AI Backend
```

- **OpenAI Backend**: Creates a thread with file search (vector store) using the Assistants API v2.
- **Gemini Backend**: Maintains conversation history with the full report in context.

The assistant is embedded in the main HTML report as a collapsible chat widget.

### ReportForAI Data Structure

The `ReportForAI` JSON/TOON document sent to AI models contains:

| Section | Content |
|---|---|
| `general_data` | MAD/ratio analysis description |
| `top_spikes_marked` | Peak periods with DB Time, DB CPU, ratio |
| `top_foreground_wait_events` | Wait stats, correlations, MAD anomalies |
| `top_background_wait_events` | Background wait stats and anomalies |
| `top_sqls_by_elapsed_time` | SQL metrics, ASH events, correlations, MAD |
| `io_stats_by_function_summary` | Per-function I/O (LGWR, DBWR, etc.) |
| `latch_activity_summary` | Latch contention metrics |
| `top_10_segments_by_*` | 8 segment ranking sections |
| `instance_stats_pearson_correlation` | Statistics correlated with DB Time |
| `load_profile_anomalies` | Load Profile MAD anomalies |
| `anomaly_clusters` | Temporally grouped cross-domain anomalies |
| `db_time_gradient_*` | 5 gradient sections (DB Time) |
| `db_cpu_gradient_*` | 2 gradient sections (DB CPU) |
| `initialization_parameters` | Oracle init.ora parameters |

### Custom Reasonings & URL Context

- **`reasonings.txt`**: Place in `$JASMIN_HOME/` or the current directory. Appended to the system prompt as advanced rules.
- **URL Context** (`-u <file>`): JSON file mapping event/SQL names to URLs. Used with Gemini's URL context tool for grounding responses.

---

## Output Structure

After running JAS-MIN, the following outputs are generated:

```
<directory>.json                     # Structured AWR/STATSPACK data
<directory>.txt                      # Text analysis log
<directory>.html_reports/
├── jasmin_main.html                 # Main interactive dashboard
├── fg/                              # Foreground wait event detail pages
│   └── fg_<event_name>.html
├── bg/                              # Background wait event detail pages
│   └── bg_<event_name>.html
├── sqlid/                           # SQL statement detail pages
│   └── sqlid_<sql_id>.html
├── stats/
│   ├── statistics_corr.html         # Instance statistics correlation table
│   ├── gradient.html                # DB Time gradient analysis
│   ├── gradient_cpu.html            # DB CPU gradient analysis
│   ├── global_statistics.json       # Load Profile summary statistics
│   ├── jasmin_highlight.html        # Load Profile box plots
│   └── inst_stat_<name>.html        # Individual statistic detail pages
├── iostats/                         # I/O statistics by function
│   ├── iostats_zMAIN.html
│   └── iostats_<function>.html
├── latches/
│   └── latchstats_activity.html     # Latch activity summary table
├── segstats/                        # Segment statistics tables
│   └── segstats_<stat_name>.html
└── jasmin/anomalies/                # Anomaly CSV exports
    ├── anomalies_reference.csv
    └── <snap_id>.csv

report_for_ai.toon                   # TOON-encoded data for AI consumption
```

---

## Environment Variables

Create a `.env` file in `$JASMIN_HOME` or the current directory:

```env
# AI API Keys (set the ones you use)
OPENAI_API_KEY=sk-...
GEMINI_API_KEY=AI...
OPENROUTER_API_KEY=sk-or-...

# Local model configuration
LOCAL_API_KEY=lm-studio
LOCAL_BASE_URL=http://localhost:1234/v1/chat/completions
LOCAL_MODEL=my-local-model

# OpenAI custom endpoint (optional)
OPENAI_URL=https://api.openai.com/

# Backend assistant
PORT=3000
OPENAI_ASST_ID=asst_...   # Required for OpenAI assistant backend
```

Set `JASMIN_HOME` to load `.env` and `reasonings.txt` from a centralized location:

```bash
export JASMIN_HOME=/path/to/jasmin_home
```

---

## CLI Reference

```
jas-min [OPTIONS]

Options:
      --file <FILE>              Parse a single text or HTML file
  -d, --directory <DIR>          Parse whole directory of files
  -o, --outfile <FILE>           Write output to non-default file
  -t, --time-cpu-ratio <FLOAT>   DB CPU / DB Time ratio threshold [default: 0.666]
  -f, --filter-db-time <FLOAT>   Filter only DB Time > this value [default: 0.0]
  -i, --id-sqls <SQL_IDS>       Include specific SQL_IDs (comma-separated)
  -j, --json-file <FILE>         Analyze a previously generated JSON file
  -s, --snap-range <BEGIN-END>   Filter snap ID range [default: 0-666666666]
  -q, --quiet                    Suppress terminal output
  -a, --ai <VENDOR:MODEL:LANG>  AI model for interpretation
  -C, --token-count-factor <N>   Output token multiplier [default: 8]
  -b, --backend-assistant <TYPE> Launch backend assistant (openai | google:model)
  -m, --mad-threshold <FLOAT>   MAD anomaly threshold [default: 7.0]
  -W, --mad-window-size <PCT>    MAD sliding window size (% of probes) [default: 100]
  -P, --parallel <N>             Parallelism level [default: 4]
  -S, --security-level <N>       Security level: 0, 1, or 2 [default: 0]
  -u, --url-context-file <FILE>  URL context file for Gemini
  -D, --deep-check <N>           Deep-analyze top-N snapshots [default: 0]
  -B, --tokens-budget <N>        Token budget for modular LLM [default: 80000]
  -R, --ridge-lambda <FLOAT>     Ridge L2 regularization [default: 50.0]
  -E, --en-lambda <FLOAT>        Elastic Net regularization [default: 30.0]
  -A, --en-alpha <FLOAT>         Elastic Net L1/L2 mix [default: 0.666]
  -I, --en-max-iter <N>          Elastic Net max iterations [default: 5000]
  -T, --en-tol <FLOAT>           Elastic Net convergence tolerance [default: 1e-6]
  -h, --help                     Print help
  -V, --version                  Print version
```

---

## Further Reading

- [JAS-MIN Introduction (blog.ora-600.pl)](https://blog.ora-600.pl/2024/12/13/jas-min/)
- [JAS-MIN and AI (blog.ora-600.pl)](https://blog.ora-600.pl/2025/07/28/jas-min-and-ai/)
- [JAS-MIN Part 1 — Digging Deep into AWR & STATSPACK (struktuur.pl)](https://blog.struktuur.pl/blog/jasmin_part1/)

---

## Authors

- **Kamil Stawiarski** — [kamil@ora-600.pl](mailto:kamil@ora-600.pl) · [blog.ora-600.pl](https://blog.ora-600.pl)
- **Radosław Kut** — [radek@ora-600.pl](mailto:radek@ora-600.pl) · [blog.struktuur.pl](https://blog.struktuur.pl)

Built by [ORA-600 | Database Whisperers](https://www.ora-600.pl/en/) 🍺

---

## License

See repository for license details.

---

<p align="center">
  <em>If you need expert Oracle performance tuning, reach out to <a href="https://www.ora-600.pl/en/">ora-600.pl</a></em>
</p>