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
- Parses raw AWR (.html) and STATSPACK (.txt) reports
- Generates an interactive, standalone HTML dashboard (Plotly-based)
- Correlates wait events and instance statistics with DB time
- Identifies top SQLs and background/foreground wait events
- Includes AI Assistant — chat with your report!
- CLI-based, friendly to scripting or pipeline integration
- Clean JSON output for further automation or logging


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

    -o, --outfile <OUTFILE>
            Write output to nondefault file? Default is directory_name.json [default: NO]

    -p, --plot <PLOT>
            Draw a plot? [default: 1]

    -s, --snap-range <SNAP_RANGE>
            Filter snapshots, based on dates in format BEGIN-END [default: 0-666666666]

    -t, --time-cpu-ratio <TIME_CPU_RATIO>
            Ratio of DB CPU / DB TIME [default: 0.666]

    -q, --quiet
          Should I be quiet? This mode suppresses terminal output but still writes to log file

    -T, --token-count-factor <TOKEN_COUNT_FACTOR>
          Base output token count is 8192 - you can update maximum number of output tokens by this factor [default: 8]

    -b, --backend-assistant <BACKEND_ASSISTANT>
          Launches the backend agent used by the JASMIN Assistant. -b <openai>|<gemini:model> Configuration details such as API keys and the selected PORT number are loaded from the .env file [default: ]

    -m, --mad-threshold <MAD_THRESHOLD>
          Threshold for detecting anomalies using MAD [default: 7] 

    -W, --mad-window-size <MAD_WINDOW_SIZE>
          Window size for detecting anomalies using MAD for local sliding window specified as % of probes [default: 100]

    -S, --security-level <LEVEL>
          Controls JAS-MIN to include sensitive infromations when generating report:
          0 - default (no sensitive data)
          1 - include objects and segments names

    -P, --parallel <PARALLEL>
          Parallelism level [default: 4]

    -V, --version
            Print version information
    

If you choose to plot a chart, you will see a basic distribution of DB Time vs DB CPU, distribution TOP 5 wait events from the times, when DB CPU was less then 66.6% of DB Time plus some TOP SQLs by Elapsed Time, sorted by amount of occuriance.  

To read reasonnings.txt and .env from other directory than current working directory, use JASMIN_HOME