<picture>
  <source media="(prefers-color-scheme: dark)" srcset="/img/jasmin_LOGO_black.png">
  <source media="(prefers-color-scheme: light)" srcset="/img/jasmin_LOGO_white.png">
  <img alt="JAS-MIN logo" src="/img/jasmin_LOGO_white.png">
</picture>

### JSON AWR/STATSPACK Mining Tool

**JAS-MIN** is a performance analysis tool that parses Oracle AWR or STATSPACK reports into structured JSON, and generates an interactive HTML report using Plotly.  
It aims to help DBAs, SREs, and performance engineers understand sampled DB time, wait events, SQL activity, and system statistics in a deeply visual and explorable way.  
JAS-MIN loves geeks who love digging into performance numbers.

**Created and maintained by**:  
Kamil Stawiarski <kamil@ora-600.pl>  
Rdosław Kut <radek@ora-600.pl>


## Features:
- Parse multiple raw AWR (.html) and STATSPACK (.txt) reports in parallel
- Generate clean JSON output for further automation or logging
- Identify TOP: SQLs, Background/Foreground wait events, IO Stats, Segments Stats, ...
- Correlate Wait Events and Instance Statistics with DB Time using PCC (Pearson correlation coefficient)
- Hunt for statistical Anomalies using MAD (Median Absolute Deviation)
- Generate an interactive, standalone HTML dashboard that visualizes data
- Run AI Assistant — chat with your report!
- Generate database performance reports powered by AI
- Choose the level of trust in terms of which sensitive data can be inlcuded
- CLI-based, friendly to scripting or pipeline integration

## State of Development:
- **This tool is in active development and evolving fast.**
- It’s already usable for real-world workloads — but expect bugs, edge cases, and weirdness.
- Feedback, PRs, and testing are very welcome.

## Assumptions:
- `.txt` file is assumed to be a STATSPACK report
- `.html` file is assumed to be an AWR report
- You should provide a directory path with one or more such reports

## Quick start guides:
- https://blog.struktuur.pl/blog/jasmin_part1/
- https://blog.struktuur.pl/blog/jasmin_part2/
- https://blog.ora-600.pl/2025/07/28/jas-min-and-ai/

## HOW TO:
USAGE:

    jas-min [OPTIONS]

OPTIONS:

    -a, --ai <AI>
          Use AI model to interpret collected statistics and describe them. Environment variable [OPENAI_API_KEY | GEMINI_API_KEY] should be set to your personal API key The parameter should be set to the value in format: VENDOR:MODEL_NAME:LANGUAGE_CODE (for example openai:gpt-4-turbo:PL or google:gemini-2.0-flash:PL) [default: ]
   
    -b, --backend-assistant <BACKEND_ASSISTANT>
          Launches the backend agent used by the JASMIN Assistant. -b <openai>|<google:model> Configuration details such as API keys and the selected PORT number are loaded from the .env file [default: ]
          
    -d, --directory <DIRECTORY>
            Parse whole directory of files [default: NO]

    -f, --filter-db-time <FILTER_DB_TIME>
            Filter only for DBTIME greater than (if zero the filter is not effective) [default: 0]

        --file <FILE>
            Parse a single text or html file [default: NO]

    -h, --help
            Print help information

    -j, --json-file <JSON_FILE>
            Analyze provided JSON file [default: NO]

    -m, --mad-threshold <MAD_THRESHOLD>
          Threshold for detecting anomalies using MAD [default: 7] 

    -o, --outfile <OUTFILE>
            Write output to nondefault file? Default is directory_name.json [default: NO]

    -p, --plot <PLOT>
            Draw a plot? [default: 1]

    -P, --parallel <PARALLEL>
          Parallelism level [default: 4]

    -q, --quiet
          Should I be quiet? This mode suppresses terminal output but still writes to log file

    -s, --snap-range <SNAP_RANGE>
            Filter snapshots, based on dates in format BEGIN-END [default: 0-666666666]

    -S, --security-level <LEVEL>
          Controls JAS-MIN to include sensitive infromations when generating report:
          0 - default (no sensitive data)
          1 - include objects and segments names

    -t, --time-cpu-ratio <TIME_CPU_RATIO>
            Ratio of DB CPU / DB TIME [default: 0.666]

    -T, --token-count-factor <TOKEN_COUNT_FACTOR>
          Base output token count is 8192 - you can update maximum number of output tokens by this factor [default: 8]

    -V, --version
            Print version information

    -W, --mad-window-size <MAD_WINDOW_SIZE>
          Window size for detecting anomalies using MAD for local sliding window specified as % of probes [default: 100]
    

If you choose to plot a chart, you will see a basic distribution of DB Time vs DB CPU, distribution TOP 5 wait events from the times, when DB CPU was less then 66.6% of DB Time plus some TOP SQLs by Elapsed Time, sorted by amount of occuriance.  

To read reasonnings.txt and .env from other directory than current working directory, use JASMIN_HOME