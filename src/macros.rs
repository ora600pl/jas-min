#[macro_export]
macro_rules! make_notes {
    ($file:expr, $quiet:expr, $heading:literal, $($arg:tt)*) => {{
        use std::io::Write;
        // if $quiet is false we will write output to screen
        if !$quiet {
            print!($($arg)*);
        }

        let formatted = format!($($arg)*);

        //Create plain text from colored one
        let mut plain = {
            // remove everything starting with ESC ( \x1B ), than [, digits and ;,
            // and ending with 'm' or 'K'.
            let re = regex::Regex::new(r"\x1B\[[0-9;]*[mK]").unwrap();
            re.replace_all(&formatted, "").to_string()
        };;

        // Add Markdown heading if requested
        if $heading > 0 {
            let prefix = "#".repeat($heading as usize);
            plain = format!("{} {}\n", prefix, plain.trim_start().trim_end());
        }

        // Append to file
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open($file)
            .expect("Can't open file");
        write!(file, "{}", plain).expect("Unable to write to log file");
    }};
}

#[macro_export]
macro_rules! debug_trace {
    ($($arg:tt)*) => {{
        use std::io::Write;
        use std::env;
        use std::thread;

        if let Ok(base) = env::var("JASMIN_TRACE") {
            let tid = format!("{:?}", thread::current().id());
            let tid = tid
                .strip_prefix("ThreadId(")
                .and_then(|s| s.strip_suffix(")"))
                .unwrap_or("x");

            let trace_file = format!("{}.t{}", base, tid);

            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(trace_file)
                .expect("Can't create trace file");

            writeln!(file, $($arg)*)
                .expect("Unable to write to trace file");
        }
    }};
}

#[macro_export]
macro_rules! debug_note {
    ($($arg:tt)*) => {{
        use crate::tools::get_timestamp;
        let time = get_timestamp();
        let file = file!();
        let line = line!();
        $crate::debug_trace!(
            "[{}] [{}:{}] {}",
            time,
            file,
            line,
            format_args!($($arg)*)
        );
    }};
}