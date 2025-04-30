#[macro_export]
macro_rules! make_notes {
    ($file:expr, $quiet:expr, $($arg:tt)*) => {{
        use std::io::Write;
        // if $quiet is false we will write output to screen
        if !$quiet {
            print!($($arg)*);
        }

        let formatted = format!($($arg)*);

        //Create plain text from colored one
        let plain = {
            // remove everything starting with ESC ( \x1B ), than [, digits and ;,
            // and ending with 'm' or 'K'.
            let re = regex::Regex::new(r"\x1B\[[0-9;]*[mK]").unwrap();
            re.replace_all(&formatted, "").to_string()
        };

        // Let's append to a logfile
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open($file)
            .expect("Can't open file");
        write!(file, "{}", plain).expect("Unable to write to log file");
    }};
}