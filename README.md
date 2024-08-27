jas-min 0.1.1

ChangeLog:
- Improved 'Host CPU Load' parsing, added to plots (@rakustow)

Kamil Stawiarski <kamil@ora-600.pl>

This tool will parse STATSPACK or AWR report into JSON format which can be used by visualization

tool of your choice. The assumption is that text file is a STATSPACK report and HTML is AWR. The

tool is under development and it has a lot of bugs, so please test it and don't hasitate to suggest

some code changes :)

JAS-MIN has also possibility of drawing basic plots using plotly library. 


USAGE:

    jas-min [OPTIONS]


OPTIONS:

    -d, --directory <DIRECTORY>    Parse whole directory of files [default: NO]

    -f, --file <FILE>              Parse a single text or html file [default: NO]

    -h, --help                     Print help information

    -o, --outfile <OUTFILE>        Write output to nondefault file? Default is directory_name.json

    -p, --plot <plot>              Draw a plot? [default: false]

    -s, --server <SERVER>          Run in server mode - you can parse files via GET/POST methods.

                                   HTTP will listen on 6751 port by default [default: 0.0.0.0:6751]

    -V, --version                  Print version information
    

If you choose to plot a chart, you will see a basic distribution of DB Time vs DB CPU, distribution TOP 5 wait events from the times, when DB CPU was less then 66.6% of DB Time plus some TOP SQLs by Elapsed Time, sorted by amount of occuriance.  
