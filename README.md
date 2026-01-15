JAS-MIN

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="/img/jasmin_LOGO_black.png">
  <source media="(prefers-color-scheme: light)" srcset="/img/jasmin_LOGO_white.png">
  <img alt="JAS-MIN logo" src="/img/jasmin_LOGO_white.png">
</picture>


JSON AWR / STATSPACK Mining & Analysis Tool

JAS-MIN is an advanced Oracle performance analysis tool that parses AWR and STATSPACK reports into structured JSON and applies statistical, numerical and AI-assisted analysis to understand DB Time behavior.

Instead of treating DB Time as a flat metric or relying on static Top-N lists, JAS-MIN models DB Time as a multidimensional function of waits, statistics and SQL workload — and analyzes it using:
	•	numerical gradient estimation
	•	regularized regression (Ridge, Elastic Net)
	•	anomaly detection (MAD)
	•	correlation analysis
	•	optional LLM-based interpretation

The result is a data-driven ranking of what actually drives DB Time, not what merely looks busy.

Created for DBAs, SREs and performance engineers who are tired of superstition.

⸻

Key Capabilities

Parsing & Data Model
	•	Parse single AWR (.html) or STATSPACK (.txt) reports
	•	Parse entire directories of reports in parallel
	•	Normalize and aggregate data into structured JSON
	•	Re-analyze previously generated JSON files

DB Time Analysis
	•	Treat DB Time as a multidimensional function of:
	•	Foreground wait events
	•	System and instance statistics
	•	I/O volumes and timings
	•	SQL workload characteristics
	•	Numerically estimate DB Time gradients using:
	•	Ridge Regression (stable, dense signals)
	•	Elastic Net Regression (sparse, selective signals)

Statistical Techniques
	•	Pearson correlation analysis against DB Time
	•	Anomaly detection using Median Absolute Deviation (MAD)
	•	Sliding-window anomaly detection
	•	Snapshot range filtering

SQL & Workload Insight
	•	Identify TOP SQL by elapsed time
	•	Optionally force inclusion of specific SQL_IDs
	•	Correlate SQL activity with DB Time behavior

AI Assistance (Optional)
	•	Use OpenRouter, OpenAI or Google Gemini models to:
	•	Interpret performance data
	•	Generate human-readable explanations
	•	Perform deep analysis on selected snapshot ranges
	•	Modular token budgeting to control cost
	•	Optional URL-based context injection (Gemini)

Security & Privacy
	•	Configurable security levels:
	•	0 – no object names or SQL text stored
	•	1 – include segment names
	•	2 – include full SQL text

Automation Friendly
	•	CLI-first design
	•	Deterministic JSON output
	•	Suitable for CI, pipelines and offline analysis

⸻

Usage

jas-min [OPTIONS]

Input Selection

Option	Description
--file <FILE>	Parse a single AWR or STATSPACK file
-d, --directory <DIRECTORY>	Parse an entire directory of reports
-j, --json-file <JSON_FILE>	Analyze an existing JSON file

Output Control

Option	Description
-o, --outfile <OUTFILE>	Custom output file name
-q, --quiet	Suppress terminal output (logs still written)

Snapshot & Filtering

Option	Description
-s, --snap-range <BEGIN-END>	Filter snapshots by SNAP ID range
-f, --filter-db-time <VALUE>	Ignore snapshots below DB Time threshold

Performance Model Parameters

Option	Description	Default
-t, --time-cpu-ratio	DB CPU / DB Time threshold	0.666
-R, --ridge-lambda	Ridge regularization strength	50
-E, --en-lambda	Elastic Net regularization strength	30
-A, --en-alpha	Elastic Net L1/L2 mix	0.666
-I, --en-max-iter	Elastic Net max iterations	5000
-T, --en-tol	Elastic Net convergence tolerance	1e-6

Anomaly Detection

Option	Description	Default
-m, --mad-threshold	MAD anomaly threshold	7
-W, --mad-window-size	Sliding window size (% of probes)	100

Parallelism

Option	Description	Default
-P, --parallel	Parallelism level	4

AI Integration

Option	Description
-a, --ai <VENDOR:MODEL:LANG>	Enable AI interpretation
-b, --backend-assistant	Launch JAS-MIN backend AI agent
-C, --token-count-factor	Output token scaling factor
-B, --tokens-budget	Token budget for modular analysis
-D, --deep-check	Enable deep AI analysis mode
-u, --url-context-file	Gemini URL context support

Security

Option	Description	Default
-S, --security-level	Data sensitivity level (0–2)	0


⸻

AI Configuration

To enable AI features, set one of the following environment variables:
	•	OPENAI_API_KEY
	•	GEMINI_API_KEY
	•	OPENROUTER_API_KEY



Example:

-a openai:gpt-4-turbo:EN
-a google:gemini-2.5-flash:PL
-a openrouter:anthropic/claude-sonnet-4.5:en

Backend assistant configuration is loaded from .env.

⸻

Assumptions
	•	.html files are treated as AWR reports
	•	.txt files are treated as STATSPACK reports
	•	Snapshot ranges refer to SNAP IDs

⸻

Documentation & Articles
	•	https://blog.struktuur.pl/blog/jasmin_part1/
	•	https://blog.struktuur.pl/blog/jasmin_part2/
	•	https://blog.ora-600.pl/2025/07/28/jas-min-and-ai/

⸻

Project Status
	•	Actively developed
	•	Used on real-world Oracle systems
	•	APIs and output formats may evolve

Expect sharp edges. They are intentional.

⸻

Authors
	•	Kamil Stawiarski kamil@ora-600.pl￼
	•	Radosław Kut radek@ora-600.pl￼

