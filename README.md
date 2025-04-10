jas-min 0.4.2

Kamil Stawiarski <kamil@ora-600.pl>

Rdosław Kut <radek@ora-600.pl>

This tool will parse STATSPACK or AWR report into JSON format which can be used by visualization

tool of your choice. The assumption is that text file is a STATSPACK report and HTML is AWR. The

tool is under development and it has a lot of bugs, so please test it and don't hasitate to suggest

some code changes :)

JAS-MIN has also possibility of drawing basic plots using plotly library. 


USAGE:

    jas-min [OPTIONS]


OPTIONS:

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

    -a, --ai <AI>
          Use AI model to interpret collected statistics and describe them. 
          Environment variable [OPENAI_API_KEY | GEMINI_API_KEY] should be set to your personal API key.
          The parameter should be set to the value in format VENDOR:MODEL_NAME:LANGUAGE_CODE 
          (for example openai:gpt-4-turbo:PL or google:gemini-2.0-flash:PL) [default: NO]
    
    -b --backend-assistant
          Launches the backend agent used by the JASMIN Assistant. 
          Configuration details such as API keys and the selected PORT number are loaded from the .env file

    -V, --version
            Print version information
    

If you choose to plot a chart, you will see a basic distribution of DB Time vs DB CPU, distribution TOP 5 wait events from the times, when DB CPU was less then 66.6% of DB Time plus some TOP SQLs by Elapsed Time, sorted by amount of occuriance.  
