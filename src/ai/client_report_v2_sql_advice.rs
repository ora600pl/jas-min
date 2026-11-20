//! Source-grounded examples for selected excerpts. A candidate must occur at a
//! SQL expression boundary in the captured statement on the selected instance.
use super::{
    ReportLanguage,
    html::{SqlFragment, SqlRecommendation},
};
use regex::Regex;
use std::sync::LazyLock;

fn quoted_len(text: &str) -> Option<usize> {
    if text.starts_with("--") {
        return Some(text.find(['\n', '\r']).unwrap_or(text.len()));
    }
    if let Some(body) = text.strip_prefix("/*") {
        return Some(body.find("*/").map_or(text.len(), |end| end + 4));
    }
    // Oracle's q'[...]' and nq'!...!' allow ordinary quotes in the body.
    for prefix in ["q'", "nq'"] {
        if text
            .get(..prefix.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(prefix))
        {
            let rest = &text[prefix.len()..];
            let opener = rest.chars().next()?;
            let close = match opener {
                '[' => ']',
                '{' => '}',
                '(' => ')',
                '<' => '>',
                c => c,
            };
            let start = prefix.len() + opener.len_utf8();
            let delimiter = format!("{close}'");
            return Some(
                text[start..]
                    .find(&delimiter)
                    .map_or(text.len(), |end| start + end + delimiter.len()),
            );
        }
    }
    let quote = *text.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let mut i = 1;
    while i < text.len() {
        if text.as_bytes()[i] == quote {
            if text.as_bytes().get(i + 1) != Some(&quote) {
                return Some(i + 1);
            }
            i += 1;
        }
        i += 1;
    }
    Some(text.len())
}

fn is_expression_start(sql: &str, start: usize) -> bool {
    let mut i = 0;
    let mut previous = None;
    while i < start {
        let tail = &sql[i..];
        if let Some(len) = quoted_len(tail) {
            if !tail.starts_with("/*") && !tail.starts_with("--") {
                previous = Some('\'');
            }
            i += len;
        } else {
            let ch = tail.chars().next().unwrap();
            if !ch.is_whitespace() {
                previous = Some(ch);
            }
            i += ch.len_utf8();
        }
    }
    i == start
        && previous != Some('.')
        && !sql[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '#'))
}

pub(super) fn year_example(
    sql: &str,
    fragments: &[SqlFragment],
    lang: ReportLanguage,
) -> Option<SqlRecommendation> {
    static YEAR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?ix-u)\bTO_NUMBER\s*\(\s*TO_CHAR\s*\(\s*TRUNC\s*\(\s*(?<col>[a-z][a-z0-9_$\#]*(?:\s*\.\s*[a-z][a-z0-9_$\#]*){0,2})\s*\)\s*,\s*'yyyy'\s*\)\s*,\s*'9999'\s*\)"
    ).unwrap()
    });
    for fragment in fragments {
        let origin = sql.char_indices().nth(fragment.first.checked_sub(1)?)?.0;
        for found in YEAR.captures_iter(&fragment.text) {
            let before = found.get(0)?;
            if !is_expression_start(sql, origin + before.start()) {
                continue;
            }
            return Some(SqlRecommendation {
                before: before.as_str().into(),
                after: format!("EXTRACT(YEAR FROM {})", &found["col"]),
                rationale: lang.choose(
                    "EXTRACT returns the numeric year directly. This removes day truncation, text formatting and conversion back to a number from this expression.",
                    "EXTRACT zwraca rok jako liczbę. W tym wyrażeniu pozwala pominąć obcięcie czasu, formatowanie daty jako tekstu i ponowną konwersję na liczbę."
                ).into(),
                assumption: lang.choose(
                    "The column has type DATE, its non-null values are in the common era and the report uses Gregorian calendar years.",
                    "Kolumna ma typ DATE, jej niepuste wartości to daty naszej ery, a raport posługuje się rokiem kalendarza gregoriańskiego."
                ).into(),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    const YEAR: &str = "TO_NUMBER(TO_CHAR( TRUNC(t.booked_at), 'yyyy'), '9999')";
    fn example(sql: &str) -> Option<SqlRecommendation> {
        let at = sql.find(YEAR)?;
        let f = SqlFragment {
            text: YEAR.into(),
            first: sql[..at].chars().count() + 1,
            last: sql[..at].chars().count() + YEAR.chars().count(),
        };
        year_example(sql, &[f], ReportLanguage::new("EN"))
    }
    #[test]
    fn rewrites_only_a_cited_year_expression_in_executable_sql() {
        let r = example(&format!("select /* 'ł' */ {YEAR} from t")).unwrap();
        assert_eq!(r.before, YEAR);
        assert_eq!(r.after, "EXTRACT(YEAR FROM t.booked_at)");
        for sql in [
            format!("-- {YEAR}\nselect 1 from t"),
            format!("/*{YEAR}*/"),
            format!("select nq'«{YEAR}«' from t"),
            format!("select q'[{YEAR}]' from t"),
            format!("select '{YEAR}' from t"),
            format!("select \"{YEAR}\" from t"),
            format!("select extension. /* custom */ {YEAR} from t"),
        ] {
            assert!(example(&sql).is_none(), "{sql}");
        }
    }
}
