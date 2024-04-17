jas-min 0.1.0

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

    -p, --plot <plot>              Draw a plot? [default: false]

    -s, --server <SERVER>          Run in server mode - you can parse files via GET/POST methods.

                                   HTTP will listen on 6751 port by default [default: 0.0.0.0:6751]

    -V, --version                  Print version information
    
