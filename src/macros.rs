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