#!/usr/bin/env python3
"""
JAS-MIN report collector.

Interactive and command-line helper for collecting Oracle AWR or Statspack
reports for a date range detected from ORACLE_HOME and ORACLE_SID. The script
intentionally uses only Python standard library modules.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import zipfile
from collections import Counter
from datetime import datetime, timedelta
from html import unescape
from html.parser import HTMLParser
import json
from pathlib import Path


DATE_FORMAT = "%Y-%m-%d %H:%M"
DATE_FORMAT_LABEL = "YYYY-MM-DD HH24:MI"
NLS_LANG = "AMERICAN_AMERICA.AL32UTF8"
STATSPACK_MIN_INTERVAL_MINUTES = 30
PACKAGE_REPORTS = "reports"
PACKAGE_JSON = "json"
PACKAGE_BOTH = "both"
SQL_ID_RE = re.compile(r"^[A-Za-z0-9]{1,30}$")


class CollectorError(Exception):
    """Expected runtime error shown without a traceback."""


def tail(text, limit=4000):
    if len(text) <= limit:
        return text
    return text[-limit:]


def parse_datetime(value):
    try:
        return datetime.strptime(value, DATE_FORMAT)
    except ValueError:
        raise CollectorError("Invalid date format. Expected: {}".format(DATE_FORMAT_LABEL))


def parse_datetime_arg(value):
    try:
        return parse_datetime(value)
    except CollectorError as exc:
        raise argparse.ArgumentTypeError(str(exc))


def datetime_sql(value):
    return value.strftime(DATE_FORMAT)


def ask_report_type():
    while True:
        value = input("Statspack or AWR? [AWR/statspack]: ").strip().lower()
        if value in ("", "awr", "a"):
            return "AWR"
        if value in ("statspack", "stat", "sp", "s"):
            return "STATSPACK"
        print("Please choose AWR or Statspack.")


def parse_report_type_arg(value):
    normalized = (value or "").strip().lower()
    if normalized in ("awr", "a"):
        return "AWR"
    if normalized in ("statspack", "stat", "sp", "s"):
        return "STATSPACK"
    raise argparse.ArgumentTypeError("Expected AWR or STATSPACK.")


def ask_datetime(prompt):
    while True:
        value = input(prompt).strip()
        try:
            return parse_datetime(value)
        except CollectorError as exc:
            print(exc)


def ask_date_range():
    print("Let's choose date range")
    while True:
        start_dt = ask_datetime("  Start ({}): ".format(DATE_FORMAT_LABEL))
        end_dt = ask_datetime("  End   ({}): ".format(DATE_FORMAT_LABEL))
        if end_dt <= start_dt:
            print("End must be later than start.")
            continue
        return start_dt, end_dt


def resolve_date_range(start_dt, end_dt):
    if start_dt is None and end_dt is None:
        return ask_date_range()

    if start_dt is not None and end_dt is not None:
        if end_dt <= start_dt:
            raise CollectorError("End must be later than start.")
        return start_dt, end_dt

    print("Let's choose date range")
    if start_dt is None:
        while True:
            start_dt = ask_datetime("  Start ({}): ".format(DATE_FORMAT_LABEL))
            if end_dt <= start_dt:
                print("End must be later than start.")
                continue
            return start_dt, end_dt

    while True:
        end_dt = ask_datetime("  End   ({}): ".format(DATE_FORMAT_LABEL))
        if end_dt <= start_dt:
            print("End must be later than start.")
            continue
        return start_dt, end_dt


def ask_yes_no(prompt):
    while True:
        value = input(prompt).strip().lower()
        if value in ("y", "yes"):
            return True
        if value in ("n", "no"):
            return False
        print("Please answer Y or N.")


def ask_package_mode():
    print("Choose ZIP package content")
    print("  reports - full AWR/Statspack reports")
    print("  json    - parsed JAS-MIN JSON only")
    print("  both    - full reports and parsed JSON")
    while True:
        value = input("Package content? [both/json/reports]: ").strip().lower()
        if value in ("", "both", "b"):
            return PACKAGE_BOTH
        if value in ("json", "j"):
            return PACKAGE_JSON
        if value in ("reports", "report", "awr", "full", "r"):
            return PACKAGE_REPORTS
        print("Please choose reports, json, or both.")


def parse_package_mode_arg(value):
    normalized = (value or "").strip().lower()
    if normalized in ("both", "b"):
        return PACKAGE_BOTH
    if normalized in ("json", "j"):
        return PACKAGE_JSON
    if normalized in ("reports", "report", "awr", "full", "r"):
        return PACKAGE_REPORTS
    raise argparse.ArgumentTypeError("Expected reports, json, or both.")


def normalize_sql_id(value):
    return normalize_cell(value).lower()


def is_valid_sql_id(value):
    sql_id = normalize_sql_id(value)
    return bool(SQL_ID_RE.match(sql_id))


def parse_sql_id_list(value):
    sql_ids = []
    rejected = []
    seen = set()
    for raw_sql_id in re.split(r"[,;\s]+", value or ""):
        sql_id = normalize_sql_id(raw_sql_id)
        if not sql_id:
            continue
        if not is_valid_sql_id(sql_id):
            rejected.append(raw_sql_id)
            continue
        if sql_id not in seen:
            seen.add(sql_id)
            sql_ids.append(sql_id)
    return sql_ids, rejected


def parse_sql_ids_arg(value):
    sql_ids, rejected = parse_sql_id_list(value)
    if rejected:
        raise argparse.ArgumentTypeError(
            "Invalid SQL_ID value(s): {}".format(", ".join(rejected))
        )
    return sql_ids


def merge_cli_sql_ids(sql_id_groups):
    if sql_id_groups is None:
        return None

    sql_ids = []
    seen = set()
    for group in sql_id_groups:
        for sql_id in group:
            if sql_id not in seen:
                seen.add(sql_id)
                sql_ids.append(sql_id)
    return sql_ids


def ask_sql_execution_plans():
    include_plans = ask_yes_no("Attach execution plans for top elapsed SQL_IDs? (Y/N): ")
    if not include_plans:
        return False, []

    value = input("Additional SQL_IDs to include (comma-separated, empty for none): ").strip()
    sql_ids, rejected = parse_sql_id_list(value)
    for rejected_sql_id in rejected:
        print("WARNING: Ignoring invalid SQL_ID: {}".format(rejected_sql_id))
    return True, sql_ids


def package_includes_reports(package_mode):
    return package_mode in (PACKAGE_REPORTS, PACKAGE_BOTH)


def package_includes_json(package_mode):
    return package_mode in (PACKAGE_JSON, PACKAGE_BOTH)


def ask_security_level():
    print("Choose JAS-MIN security level for JSON")
    print("  0 - do not store object names, database names, or other sensitive names where masked")
    print("  1 - include segment names from Segment Statistics")
    print("  2 - include full SQL text when parsed")
    while True:
        value = input("Security Level? [0/1/2]: ").strip()
        if value in ("", "0"):
            return 0
        if value in ("1", "2"):
            return int(value)
        print("Please choose 0, 1, or 2.")


def parse_security_level_arg(value):
    try:
        level = int(value)
    except (TypeError, ValueError):
        raise argparse.ArgumentTypeError("Expected 0, 1, or 2.")
    if level not in (0, 1, 2):
        raise argparse.ArgumentTypeError("Expected 0, 1, or 2.")
    return level


def build_arg_parser():
    examples = """examples:
  python3 jas-min-collector.py --report-type awr --start "2026-06-14 00:00" --end "2026-06-15 14:00"
  python3 jas-min-collector.py --report-type statspack --start "2026-06-14 00:00" --end "2026-06-15 14:00" --no-alert-log --no-execution-plans --package-content reports
  python3 jas-min-collector.py --report-type awr --start "2026-06-14 00:00" --end "2026-06-15 14:00" --include-alert-log --execution-plans --sql-id abc123,def456 --package-content both --security-level 1
"""
    parser = argparse.ArgumentParser(
        description="Collect Oracle AWR or Statspack reports and package them for JAS-MIN.",
        epilog=examples,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "-t",
        "--report-type",
        metavar="{awr,statspack}",
        type=parse_report_type_arg,
        help="report source type; asks interactively when omitted",
    )
    parser.add_argument(
        "--start",
        metavar=DATE_FORMAT_LABEL,
        dest="start_dt",
        type=parse_datetime_arg,
        help="collection start timestamp; asks interactively when omitted",
    )
    parser.add_argument(
        "--end",
        metavar=DATE_FORMAT_LABEL,
        dest="end_dt",
        type=parse_datetime_arg,
        help="collection end timestamp; asks interactively when omitted",
    )

    alert_group = parser.add_mutually_exclusive_group()
    alert_group.add_argument(
        "--include-alert-log",
        "--alert-log",
        dest="include_alert",
        action="store_true",
        default=None,
        help="include an alert log excerpt",
    )
    alert_group.add_argument(
        "--no-alert-log",
        dest="include_alert",
        action="store_false",
        help="do not include an alert log excerpt",
    )

    xplan_group = parser.add_mutually_exclusive_group()
    xplan_group.add_argument(
        "--include-execution-plans",
        "--execution-plans",
        dest="include_sql_plans",
        action="store_true",
        default=None,
        help="attach execution plans for top elapsed SQL_IDs",
    )
    xplan_group.add_argument(
        "--no-execution-plans",
        dest="include_sql_plans",
        action="store_false",
        help="do not attach SQL execution plans",
    )
    parser.add_argument(
        "--sql-id",
        "--sql-ids",
        dest="sql_id_groups",
        metavar="SQL_ID[,SQL_ID...]",
        action="append",
        type=parse_sql_ids_arg,
        help="additional SQL_IDs for execution-plan collection; may be repeated",
    )
    parser.add_argument(
        "-p",
        "--package-content",
        "--package-mode",
        dest="package_mode",
        metavar="{both,json,reports}",
        type=parse_package_mode_arg,
        help="ZIP package content; asks interactively when omitted",
    )
    parser.add_argument(
        "-S",
        "--security-level",
        metavar="{0,1,2}",
        type=parse_security_level_arg,
        help="JSON security level; asks when JSON is required and omitted",
    )
    return parser


def parse_collector_args(argv=None):
    parser = build_arg_parser()
    args = parser.parse_args(argv)
    args.manual_sql_ids = merge_cli_sql_ids(args.sql_id_groups)

    if args.start_dt is not None and args.end_dt is not None and args.end_dt <= args.start_dt:
        parser.error("--end must be later than --start")
    if args.manual_sql_ids is not None:
        if args.include_sql_plans is False:
            parser.error("--sql-id can only be used when execution plans are enabled")
        if args.include_sql_plans is None:
            args.include_sql_plans = True

    return args


def require_oracle_context():
    oracle_home = os.environ.get("ORACLE_HOME", "").strip()
    oracle_sid = os.environ.get("ORACLE_SID", "").strip()

    missing = []
    if not oracle_home:
        missing.append("ORACLE_HOME")
    if not oracle_sid:
        missing.append("ORACLE_SID")
    if missing:
        raise CollectorError("Missing environment variable(s): {}".format(", ".join(missing)))

    sqlplus = Path(oracle_home) / "bin" / "sqlplus"
    if not sqlplus.is_file():
        raise CollectorError("sqlplus not found at {}".format(sqlplus))
    if not os.access(str(sqlplus), os.X_OK):
        raise CollectorError("sqlplus is not executable: {}".format(sqlplus))

    env = os.environ.copy()
    env.setdefault("NLS_LANG", NLS_LANG)

    return {
        "oracle_home": Path(oracle_home),
        "oracle_sid": oracle_sid,
        "sqlplus": sqlplus,
        "env": env,
    }


def run_sqlplus(ctx, script, cwd=None, check_output_errors=True):
    command = [str(ctx["sqlplus"]), "-S", "/ as sysdba"]
    proc = subprocess.run(
        command,
        input=script,
        universal_newlines=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=str(cwd) if cwd else None,
        env=ctx["env"],
    )
    output = (proc.stdout or "") + (proc.stderr or "")
    if proc.returncode != 0:
        raise CollectorError("sqlplus failed:\n{}".format(tail(output).strip()))
    if check_output_errors and re.search(r"\b(?:ORA|SP2)-\d+\b", output):
        raise CollectorError("sqlplus reported an Oracle error:\n{}".format(tail(output).strip()))
    return proc.stdout or ""


def parse_delimited_rows(output, expected_fields):
    rows = []
    for raw_line in output.splitlines():
        line = raw_line.strip()
        if not line or "|" not in line:
            continue
        parts = [part.strip() for part in line.split("|")]
        if len(parts) == expected_fields and all(parts):
            rows.append(parts)
    return rows


def normalize_cell(value):
    value = unescape(value or "")
    value = value.replace("\xa0", " ")
    return re.sub(r"\s+", " ", value).strip()


def parse_float(value, default=0.0):
    value = normalize_cell(value)
    value = value.replace(",", "")
    value = value.replace("%", "")
    if not value or value in ("-", "."):
        return default
    try:
        return float(value)
    except ValueError:
        return default


def parse_int(value, default=0):
    value = normalize_cell(value)
    value = value.replace(",", "")
    if not value or value in ("-", "."):
        return default
    try:
        parsed = int(float(value))
    except ValueError:
        return default
    # The JSON schema stores these AWR counters in unsigned Rust fields.
    return parsed if parsed >= 0 else default


def parse_count(value):
    value = normalize_cell(value).replace(",", ".").lower()
    if not value or value in ("-", "."):
        return 0
    multipliers = {
        "k": 1000.0,
        "m": 1000000.0,
        "g": 1000000000.0,
        "t": 1000000000000.0,
        "p": 1000000000000000.0,
    }
    suffix = value[-1]
    multiplier = multipliers.get(suffix, 1.0)
    number = value[:-1] if suffix in multipliers else value
    try:
        return int(float(number) * multiplier)
    except ValueError:
        return 0


def parse_wait_ms(value):
    value = normalize_cell(value).replace(",", "")
    lower = value.lower()
    try:
        if lower.endswith("us"):
            return float(lower[:-2].strip()) / 1000.0
        if lower.endswith("ms"):
            return float(lower[:-2].strip())
        if lower.endswith("ns"):
            return float(lower[:-2].strip()) / 1000000.0
        if lower.endswith("s"):
            return float(lower[:-1].strip()) * 1000.0
        return float(lower)
    except ValueError:
        return 0.0


def parse_size_mb(value):
    value = normalize_cell(value).replace(",", ".")
    if not value or value in ("-", "."):
        return 0.0
    unit = value[-1].upper()
    number = value[:-1] if unit.isalpha() else value
    try:
        parsed = float(number)
    except ValueError:
        return 0.0
    if unit == "K":
        return parsed / 1024.0
    if unit == "M":
        return parsed
    if unit == "G":
        return parsed * 1024.0
    if unit == "T":
        return parsed * 1024.0 * 1024.0
    return parsed


def infer_sql_type(sql_text):
    upper = normalize_cell(sql_text).upper()
    if upper.startswith("UPDATE"):
        return "UPDATE"
    if upper.startswith("DELETE"):
        return "DELETE"
    if upper.startswith("INSERT"):
        return "INSERT"
    if upper.startswith("MERGE"):
        return "MERGE"
    if upper.startswith("BEGIN") or upper.startswith("DECLARE") or upper.startswith("CALL"):
        return "PL/SQL"
    return "SELECT"


IDLE_EVENTS = set([
    "SQL*Net message from client",
    "SQL*Net message to client",
    "rdbms ipc message",
    "pmon timer",
    "smon timer",
    "VKTM Logical Idle Wait",
    "class slave wait",
    "Space Manager: slave idle wait",
    "Streams AQ: waiting for messages in the queue",
    "Streams AQ: qmn coordinator idle wait",
])


def is_idle_event(name):
    return normalize_cell(name) in IDLE_EVENTS


class AWRHTMLTableParser(HTMLParser):
    def __init__(self):
        HTMLParser.__init__(self)
        self.tables = []
        self.current_table = None
        self.table_depth = 0
        self.current_row = None
        self.current_tags = None
        self.current_cell = None
        self.current_cell_tag = None

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "table":
            summary = attrs.get("summary")
            if self.current_table is None and summary:
                self.current_table = {"summary": summary, "rows": [], "tags": []}
                self.table_depth = 1
            elif self.current_table is not None:
                self.table_depth += 1
        elif self.current_table is not None and tag == "tr":
            self.current_row = []
            self.current_tags = []
        elif self.current_table is not None and tag in ("td", "th") and self.current_row is not None:
            self.current_cell = []
            self.current_cell_tag = tag

    def handle_data(self, data):
        if self.current_cell is not None:
            self.current_cell.append(data)

    def handle_endtag(self, tag):
        if self.current_table is not None and tag in ("td", "th") and self.current_cell is not None:
            self.current_row.append(normalize_cell("".join(self.current_cell)))
            self.current_tags.append(self.current_cell_tag)
            self.current_cell = None
            self.current_cell_tag = None
        elif self.current_table is not None and tag == "tr":
            if self.current_row:
                self.current_table["rows"].append(self.current_row)
                self.current_table["tags"].append(self.current_tags)
            self.current_row = None
            self.current_tags = None
        elif tag == "table" and self.current_table is not None:
            self.table_depth -= 1
            if self.table_depth <= 0:
                self.tables.append(self.current_table)
                self.current_table = None
                self.table_depth = 0


def data_rows(table):
    rows = []
    for row, tags in zip(table.get("rows", []), table.get("tags", [])):
        if "td" in tags:
            rows.append(row)
    return rows


def header_rows(table):
    rows = []
    for row, tags in zip(table.get("rows", []), table.get("tags", [])):
        if "th" in tags:
            rows.append(row)
    return rows


def default_db_instance():
    return {
        "db_id": 0,
        "instance_num": 0,
        "startup_time": "",
        "release": "",
        "rac": "",
        "platform": "",
        "cpus": 0,
        "cores": 0,
        "sockets": 0,
        "memory": 0,
        "db_block_size": 0,
    }


def default_awr(path):
    return {
        "file_name": str(path),
        "snap_info": {
            "begin_snap_id": 0,
            "end_snap_id": 0,
            "begin_snap_time": "",
            "end_snap_time": "",
        },
        "status": "OK",
        "load_profile": [],
        "instance_efficiency": [],
        "redo_log": {"stat_name": "", "per_hour": 0.0},
        "wait_classes": [],
        "host_cpu": {
            "cpus": 0,
            "cores": 0,
            "sockets": 0,
            "load_avg_begin": 0.0,
            "load_avg_end": 0.0,
            "pct_user": 0.0,
            "pct_system": 0.0,
            "pct_wio": 0.0,
            "pct_idle": 0.0,
        },
        "time_model_stats": [],
        "foreground_wait_events": [],
        "background_wait_events": [],
        "sql_elapsed_time": [],
        "sql_cpu_time": {},
        "sql_io_time": {},
        "sql_gets": {},
        "sql_reads": {},
        "top_sql_with_top_events": {},
        "instance_stats": [],
        "dictionary_cache": [],
        "io_stats_byfunc": {},
        "library_cache": [],
        "latch_activity": [],
        "segment_stats": {},
    }


def merge_db_instance(target, source):
    for key, value in source.items():
        if value not in ("", 0, 0.0, None):
            target[key] = value


def parse_db_instance_from_tables(tables):
    dbi = default_db_instance()
    for table in tables:
        summary = table.get("summary", "")
        rows = data_rows(table)
        heads = header_rows(table)
        if summary == "This table displays database instance information":
            if rows:
                row = rows[0]
                if len(heads) > 0 and len(heads[0]) == 8 and len(row) >= 7:
                    dbi["db_id"] = parse_int(row[1])
                    dbi["release"] = row[5]
                    dbi["rac"] = row[6]
                elif len(row) >= 7:
                    dbi["db_id"] = parse_int(row[1])
                    dbi["instance_num"] = parse_int(row[3])
                    dbi["startup_time"] = row[4]
                    dbi["release"] = row[5]
                    dbi["rac"] = row[6]
                elif len(row) >= 3:
                    dbi["instance_num"] = parse_int(row[1])
                    dbi["startup_time"] = row[2]
        elif summary == "This table displays host information" and rows:
            row = rows[0]
            if len(row) >= 6:
                dbi["platform"] = row[1]
                dbi["cpus"] = parse_int(row[2])
                dbi["cores"] = parse_int(row[3])
                dbi["sockets"] = parse_int(row[4])
                dbi["memory"] = int(round(parse_float(row[5])))
        elif summary.startswith("This table displays name and value of"):
            for row in rows:
                if len(row) >= 2 and row[0] == "db_block_size":
                    dbi["db_block_size"] = parse_int(row[1])
    return dbi


def parse_load_profile(table):
    result = []
    for row in data_rows(table):
        if len(row) == 5:
            result.append({
                "stat_name": row[0].rstrip(":"),
                "per_second": parse_float(row[1]),
                "per_transaction": parse_float(row[2]),
            })
    return result


def parse_instance_efficiency(table):
    result = []
    for row in data_rows(table):
        if len(row) >= 2:
            value = parse_float(row[1], None)
            result.append({"eff_stat": row[0].rstrip(":"), "eff_pct": value if value is None or value >= 0 else None})
        if len(row) >= 4:
            value = parse_float(row[3], None)
            result.append({"eff_stat": row[2].rstrip(":"), "eff_pct": value if value is None or value >= 0 else None})
    return result


def parse_wait_classes(table):
    result = []
    for row in data_rows(table):
        if len(row) == 6:
            result.append({
                "wait_class": row[0],
                "waits": parse_int(row[1]),
                "total_wait_time_s": parse_float(row[3]),
                "avg_wait_ms": parse_float(row[4]),
                "db_time_pct": parse_float(row[5]),
            })
    return result


def parse_host_cpu(table):
    result = default_awr("")["host_cpu"]
    for row in data_rows(table):
        if len(row) == 9:
            result.update({
                "cpus": parse_int(row[0]),
                "cores": parse_int(row[1]),
                "sockets": parse_int(row[2]),
                "load_avg_begin": parse_float(row[3]),
                "load_avg_end": parse_float(row[4]),
                "pct_user": parse_float(row[5]),
                "pct_system": parse_float(row[6]),
                "pct_wio": parse_float(row[7]),
                "pct_idle": parse_float(row[8]),
            })
        elif len(row) == 6:
            result.update({
                "load_avg_begin": parse_float(row[0]),
                "load_avg_end": parse_float(row[1]),
                "pct_user": parse_float(row[2]),
                "pct_system": parse_float(row[3]),
                "pct_wio": parse_float(row[4]),
                "pct_idle": parse_float(row[5]),
            })
    return result


def parse_time_model_stats(table):
    result = []
    for row in data_rows(table):
        if len(row) >= 3:
            result.append({
                "stat_name": row[0],
                "time_s": parse_float(row[1]),
                "pct_dbtime": parse_float(row[2]),
            })
    return result


def parse_wait_events(table):
    result = []
    for row in data_rows(table):
        if len(row) == 7 and not is_idle_event(row[0]):
            result.append({
                "event": row[0],
                "waits": parse_int(row[1]),
                "total_wait_time_s": parse_float(row[3]),
                "avg_wait": parse_wait_ms(row[4]),
                "pct_dbtime": parse_float(row[6]),
                "waitevent_histogram_ms": {},
            })
    return result


def parse_sql_elapsed_time(table):
    result = []
    for row in data_rows(table):
        if len(row) >= 9:
            sql_text = row[-1]
            result.append({
                "sql_id": row[6],
                "elapsed_time_s": parse_float(row[0]),
                "executions": parse_int(row[1]),
                "elpased_time_exec_s": parse_float(row[2]),
                "pct_total": parse_float(row[3]),
                "pct_cpu": parse_float(row[4]),
                "pct_io": parse_float(row[5]),
                "sql_module": row[7],
                "sql_type": infer_sql_type(sql_text),
            })
    return result


def parse_sql_cpu_time(table):
    result = {}
    for row in data_rows(table):
        if len(row) >= 10:
            sql_id = row[7]
            result[sql_id] = {
                "sql_id": sql_id,
                "cpu_time_s": parse_float(row[0]),
                "executions": parse_int(row[1]),
                "cpu_time_exec_s": parse_float(row[2]),
                "pct_total": parse_float(row[3]),
                "pct_cpu": parse_float(row[5]),
                "pct_io": parse_float(row[6]),
                "sql_module": row[8],
            }
    return result


def parse_sql_io_time(table):
    result = {}
    for row in data_rows(table):
        if len(row) >= 10:
            sql_id = row[7]
            result[sql_id] = {
                "sql_id": sql_id,
                "io_time_s": parse_float(row[0]),
                "executions": parse_int(row[1]),
                "io_time_exec_s": parse_float(row[2]),
                "pct_total": parse_float(row[3]),
                "pct_cpu": parse_float(row[5]),
                "pct_io": parse_float(row[6]),
                "sql_module": row[8],
            }
    return result


def parse_sql_gets(table):
    result = {}
    for row in data_rows(table):
        if len(row) >= 10:
            sql_id = row[7]
            result[sql_id] = {
                "sql_id": sql_id,
                "buffer_gets": parse_float(row[0]),
                "executions": parse_int(row[1]),
                "gets_per_exec": parse_float(row[2]),
                "pct_total": parse_float(row[3]),
                "pct_cpu": parse_float(row[5]),
                "pct_io": parse_float(row[6]),
                "sql_module": row[8],
            }
    return result


def parse_sql_reads(table):
    result = {}
    for row in data_rows(table):
        if len(row) >= 10:
            sql_id = row[7]
            result[sql_id] = {
                "sql_id": sql_id,
                "physical_reads": parse_float(row[0]),
                "executions": parse_int(row[1]),
                "reads_per_exec": parse_float(row[2]),
                "pct_total": parse_float(row[3]),
                "cpu_time_pct": parse_float(row[5]),
                "pct_io": parse_float(row[6]),
                "sql_module": row[8],
            }
    return result


def parse_snap_info(table):
    result = default_awr("")["snap_info"]
    for row in data_rows(table):
        if len(row) >= 5:
            if row[0] == "Begin Snap:":
                result["begin_snap_id"] = parse_int(row[1])
                result["begin_snap_time"] = row[2]
            elif row[0] == "End Snap:":
                result["end_snap_id"] = parse_int(row[1])
                result["end_snap_time"] = row[2]
    return result


def parse_instance_stats(table):
    result = []
    for row in data_rows(table):
        if len(row) == 4:
            result.append({"statname": row[0], "total": parse_int(row[1])})
    return result


def parse_io_stats(table):
    result = {}
    for row in data_rows(table):
        if len(row) == 9 and row[0] != "TOTAL:":
            result[row[0]] = {
                "reads_data": parse_size_mb(row[1]),
                "reads_req_s": parse_float(row[2]),
                "reads_data_s": parse_size_mb(row[3]),
                "writes_data": parse_size_mb(row[4]),
                "writes_req_s": parse_float(row[5]),
                "writes_data_s": parse_size_mb(row[6]),
                "waits_count": parse_count(row[7]),
                "avg_time": parse_wait_ms(row[8]) if normalize_cell(row[8]) else None,
            }
    return result


def parse_redo_log(table):
    result = {"stat_name": "", "per_hour": 0.0}
    for row in data_rows(table):
        if len(row) == 3 and row[0].startswith("log switches (derived)"):
            result = {"stat_name": row[0], "per_hour": parse_float(row[2])}
    return result


def parse_dictionary_cache(table):
    result = []
    for row in data_rows(table):
        if len(row) >= 7:
            result.append({
                "statname": row[0],
                "get_requests": parse_int(row[1]),
                "final_usage": parse_int(row[6]),
            })
    return result


def parse_library_cache(table):
    result = []
    for row in data_rows(table):
        if len(row) >= 7:
            result.append({
                "statname": row[0],
                "get_requests": parse_int(row[1]),
                "get_pct_miss": parse_float(row[2]),
                "pin_requests": parse_int(row[3]),
            })
    return result


def parse_latch_activity(table):
    result = []
    for row in data_rows(table):
        if len(row) >= 7:
            result.append({
                "statname": row[0],
                "get_requests": parse_int(row[1]),
                "get_pct_miss": parse_float(row[2]),
                "wait_time": parse_float(row[4]),
            })
    return result


SEGMENT_SUMMARIES = {
    "This table displays top segments by row lock waits. Owner, tablespace name, object type, row lock waits, etc. are displayed for each segment": ("Row Lock Waits", "Row Lock Waits"),
    "This table displays top segments by logical reads. Owner, tablespace name, object type, logical read, etc. are displayed for each segment": ("Logical Reads", "Logical Reads"),
    "This table displays top segments by physical reads. Owner, tablespace name, object type, physical reads, etc. are displayed for each segment": ("Physical Reads", "Reads"),
    "This table displays top segments by physical read requests. Owner, tablespace name, object type, physical read requests, etc. are displayed for each segment": ("Physical Read Requests", "Read Requests"),
    "This table displays top segments by direct physical reads. Owner, tablespace name, object type, direct reads, etc. are displayed for each segment": ("Direct Physical Reads", "Direct Reads"),
    "This table displays top segments by physical writes. Owner, tablespace name, object type, physical writes, etc. are displayed for each segment": ("Physical Writes", "Writes"),
    "This table displays top segments by physical write requests. Owner, tablespace name, object type, physical write requests, etc. are displayed for each segment": ("Physical Write Requests", "Write Requests"),
    "This table displays top segments by direct physical writes. Owner, tablespace name, object type, direct writes, etc. are displayed for each segment": ("Direct Physical Writes", "Direct Writes"),
    "This table displays top segments by buffer busy waits. Owner, tablespace name, object type, buffer busy waits, etc. are displayed for each segment": ("Buffer Busy Waits", "Busy Waits"),
    "This table displays top segments by global cache buffer busy waits. Owner, tablespace name, object type, GC buffer busy waits, etc. are displayed for each segment": ("Global Cache Buffer Busy", "GCBusy Waits"),
}


def parse_segment_stats(table, stat_name, security_level):
    result = []
    for row in data_rows(table):
        if len(row) >= 7:
            version_modifier = 1 if len(row) == 7 else 0
            result.append({
                "obj": parse_int(row[5]) if version_modifier == 0 and len(row) > 5 else 0,
                "objd": parse_int(row[6]) if version_modifier == 0 and len(row) > 6 else 0,
                "object_name": row[2] if security_level > 0 and len(row) > 2 else "#",
                "object_type": row[4] if len(row) > 4 else "",
                "stat_name": stat_name,
                "stat_vlalue": parse_float(row[7 - version_modifier]) if len(row) > 7 - version_modifier else 0.0,
            })
    return result


def parse_top_sql_with_top_events(table):
    result = {}
    for row in data_rows(table):
        if len(row) > 8:
            sql_id = row[0]
            result[sql_id] = {
                "sql_id": sql_id,
                "plan_hash_value": parse_int(row[1]),
                "executions": parse_int(row[2]),
                "pct_activity": parse_float(row[3]),
                "event_name": row[4],
                "pct_event": parse_float(row[5]),
                "top_row_source": row[6],
                "pct_row_source": parse_float(row[7]),
            }
    return result


def parse_sql_text(table):
    result = {}
    for row in data_rows(table):
        if len(row) >= 2:
            sql_id = row[0]
            sql_text = row[-1]
            if sql_id and sql_text:
                result[sql_id] = sql_text
    return result


def parse_initialization_parameters(table):
    result = {}
    for row in data_rows(table):
        if len(row) >= 2:
            result[row[0]] = row[1]
    return result


def parse_wait_histogram(table):
    result = {}
    buckets = []
    for row in header_rows(table):
        if len(row) == 10 and (row[3] == "<1ms" or row[3] == "<2ms"):
            buckets = row[2:10]
    if not buckets:
        return result
    for row in data_rows(table):
        if len(row) == 10:
            event = row[0]
            result[event] = {}
            for idx, bucket in enumerate(buckets):
                result[event]["{}: {}".format(idx, bucket)] = parse_float(row[idx + 2])
    return result


def apply_wait_histogram(awr, histogram):
    for key in ("foreground_wait_events", "background_wait_events"):
        for event in awr[key]:
            if event["event"] in histogram:
                event["waitevent_histogram_ms"] = histogram[event["event"]]


def parse_html_report(path, security_level):
    text = path.read_text(encoding="utf-8", errors="replace")
    parser = AWRHTMLTableParser()
    parser.feed(text)
    tables = parser.tables
    awr = default_awr(path.name)
    sql_text = {}
    parameters = {}
    db_instance = parse_db_instance_from_tables(tables)

    for table in tables:
        summary = table.get("summary", "")
        if summary == "This table displays load profile":
            awr["load_profile"] = parse_load_profile(table)
        elif summary == "This table displays instance efficiency percentages":
            awr["instance_efficiency"] = parse_instance_efficiency(table)
        elif summary == "This table displays foreground wait class statistics":
            awr["wait_classes"] = parse_wait_classes(table)
        elif summary == "This table displays system load statistics":
            awr["host_cpu"] = parse_host_cpu(table)
        elif summary == "This table displays different time model statistics. For each statistic, time and % of DB time are displayed":
            awr["time_model_stats"] = parse_time_model_stats(table)
        elif summary == "This table displays Foreground Wait Events and their wait statistics":
            awr["foreground_wait_events"] = parse_wait_events(table)
        elif summary == "This table displays background wait events statistics":
            awr["background_wait_events"] = parse_wait_events(table)
        elif summary == "This table displays top SQL by elapsed time":
            awr["sql_elapsed_time"] = parse_sql_elapsed_time(table)
        elif summary == "This table displays top SQL by CPU time":
            awr["sql_cpu_time"] = parse_sql_cpu_time(table)
        elif summary == "This table displays top SQL by user I/O time":
            awr["sql_io_time"] = parse_sql_io_time(table)
        elif summary == "This table displays top SQL by buffer gets":
            awr["sql_gets"] = parse_sql_gets(table)
        elif summary == "This table displays top SQL by physical reads":
            awr["sql_reads"] = parse_sql_reads(table)
        elif summary == "This table displays snapshot information":
            awr["snap_info"] = parse_snap_info(table)
        elif summary == "This table displays Instance activity statistics. For each instance, activity total, activity per second, and activity per transaction are displayed":
            awr["instance_stats"] = parse_instance_stats(table)
        elif summary == "This table displays the IO Statistics for different functions. IO stats includes amount of reads and writes, requests per second, data per second, wait count and average wait time":
            awr["io_stats_byfunc"] = parse_io_stats(table)
        elif summary == "This table displays thread activity stats in the instance. For each activity , total number of activity and activity per hour are displayed":
            awr["redo_log"] = parse_redo_log(table)
        elif summary == "This table displays dictionary cache statistics. Get requests, % misses, scan requests, final usage, etc. are displayed for each cache":
            awr["dictionary_cache"] = parse_dictionary_cache(table)
        elif summary == "This table displays library cache statistics. Get requests, % misses, pin request, % miss, reloads, etc. are displayed for each library cache namespace":
            awr["library_cache"] = parse_library_cache(table)
        elif summary == "This table displays latch statistics. Get requests, % get miss, wait time, noWait requests are displayed for each latch":
            awr["latch_activity"] = parse_latch_activity(table)
        elif summary in SEGMENT_SUMMARIES:
            segment_key, stat_name = SEGMENT_SUMMARIES[summary]
            awr["segment_stats"][segment_key] = parse_segment_stats(table, stat_name, security_level)
        elif security_level >= 2 and summary.startswith("This table displays the text of the SQL"):
            sql_text.update(parse_sql_text(table))
        elif summary.startswith("This table displays name and value of the modified initialization parameters") or summary.startswith("This table displays name and value of init.ora parameters") or summary.startswith("This table displays name and value of the initialization parametersmodified by the current container"):
            parameters.update(parse_initialization_parameters(table))
        elif summary == "This table displays the Top SQL by Top Wait Events":
            awr["top_sql_with_top_events"] = parse_top_sql_with_top_events(table)
        elif summary == "This table displays total number of waits, and information about total wait time, for each wait event":
            apply_wait_histogram(awr, parse_wait_histogram(table))

    return awr, sql_text, parameters, db_instance


def find_text_section(lines, start_marker, end_markers):
    start = None
    for idx, line in enumerate(lines):
        if start_marker in line:
            start = idx + 1
            break
    if start is None:
        return []
    end = len(lines)
    for idx in range(start, len(lines)):
        if any(marker in lines[idx] for marker in end_markers):
            end = idx
            break
    return lines[start:end]


def parse_text_snap_info(path, lines):
    result = default_awr("")["snap_info"]
    match = re.search(r"(\d+)_(\d+)", path.stem)
    if match:
        result["begin_snap_id"] = int(match.group(1))
        result["end_snap_id"] = int(match.group(2))
    for idx, line in enumerate(lines):
        if "Begin Snap:" in line or re.search(r"\bBegin\s+Snap", line):
            nums = re.findall(r"\d+", line)
            if nums:
                result["begin_snap_id"] = int(nums[0])
        elif "End Snap:" in line or re.search(r"\bEnd\s+Snap", line):
            nums = re.findall(r"\d+", line)
            if nums:
                result["end_snap_id"] = int(nums[0])
    return result


def parse_text_load_profile(lines):
    result = []
    for line in lines:
        if ":" not in line:
            continue
        name, rest = line.split(":", 1)
        values = rest.split()
        if values:
            result.append({
                "stat_name": name.strip(),
                "per_second": parse_float(values[0]),
                "per_transaction": parse_float(values[1]) if len(values) > 1 else 0.0,
            })
    return result


def parse_text_wait_events(lines):
    result = []
    for line in lines:
        if len(line) < 45 or line.strip().startswith("-"):
            continue
        event = line[:28].strip()
        waits = parse_int(line[29:41] if len(line) > 41 else "")
        if not event or not waits or is_idle_event(event):
            continue
        result.append({
            "event": event,
            "waits": waits,
            "total_wait_time_s": parse_float(line[46:57] if len(line) > 57 else ""),
            "avg_wait": parse_wait_ms(line[57:64] if len(line) > 64 else ""),
            "pct_dbtime": parse_float(line[73:80] if len(line) > 80 else ""),
            "waitevent_histogram_ms": {},
        })
    return result


def parse_text_sql_section(lines, kind):
    items = {} if kind != "elapsed" else []
    last_sql_id = ""
    for line in lines:
        fields = line.split()
        if len(fields) == 7:
            sql_id = fields[6]
            if kind == "elapsed":
                item = {
                    "sql_id": sql_id,
                    "elapsed_time_s": parse_float(fields[0]),
                    "executions": parse_int(fields[1]),
                    "elpased_time_exec_s": parse_float(fields[2]),
                    "pct_total": parse_float(fields[3]),
                    "pct_cpu": -1.0,
                    "pct_io": -1.0,
                    "sql_module": "?",
                    "sql_type": "",
                }
                items.append(item)
            elif kind == "cpu":
                items[sql_id] = {
                    "sql_id": sql_id,
                    "cpu_time_s": parse_float(fields[0]),
                    "executions": parse_int(fields[1]),
                    "cpu_time_exec_s": parse_float(fields[2]),
                    "pct_total": parse_float(fields[3]),
                    "pct_cpu": -1.0,
                    "pct_io": -1.0,
                    "sql_module": "?",
                }
            elif kind == "gets":
                items[sql_id] = {
                    "sql_id": sql_id,
                    "buffer_gets": parse_float(fields[0]),
                    "executions": parse_int(fields[1]),
                    "gets_per_exec": parse_float(fields[2]),
                    "pct_total": parse_float(fields[3]),
                    "pct_cpu": -1.0,
                    "pct_io": -1.0,
                    "sql_module": "?",
                }
            elif kind == "reads":
                items[sql_id] = {
                    "sql_id": sql_id,
                    "physical_reads": parse_float(fields[0]),
                    "executions": parse_int(fields[1]),
                    "reads_per_exec": parse_float(fields[2]),
                    "pct_total": parse_float(fields[3]),
                    "cpu_time_pct": parse_float(fields[4]),
                    "pct_io": -1.0,
                    "sql_module": "?",
                }
            last_sql_id = sql_id
        elif line.startswith("Module:") and last_sql_id:
            module = line.split(":", 1)[1].strip()
            if kind == "elapsed" and items:
                items[-1]["sql_module"] = module
            elif kind != "elapsed" and last_sql_id in items:
                items[last_sql_id]["sql_module"] = module
    return items


def parse_text_report(path, security_level):
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    awr = default_awr(path.name)
    awr["snap_info"] = parse_text_snap_info(path, lines)
    awr["load_profile"] = parse_text_load_profile(find_text_section(lines, "Load Profile", ["Instance Efficiency", "Instance Efficiency Percentages"]))
    awr["foreground_wait_events"] = parse_text_wait_events(find_text_section(lines, "Foreground Wait Events", ["Background Wait Events"]))
    awr["background_wait_events"] = parse_text_wait_events(find_text_section(lines, "Background Wait Events", ["Wait Events", "SQL ordered by"]))
    awr["sql_elapsed_time"] = parse_text_sql_section(find_text_section(lines, "SQL ordered by Elapsed", ["SQL ordered by Gets", "SQL ordered by CPU", "SQL ordered by Reads"]), "elapsed")
    awr["sql_cpu_time"] = parse_text_sql_section(find_text_section(lines, "SQL ordered by CPU", ["SQL ordered by Elapsed", "SQL ordered by Gets"]), "cpu")
    awr["sql_gets"] = parse_text_sql_section(find_text_section(lines, "SQL ordered by Gets", ["SQL ordered by Reads", "SQL ordered by Executions"]), "gets")
    awr["sql_reads"] = parse_text_sql_section(find_text_section(lines, "SQL ordered by Reads", ["SQL ordered by Executions", "SQL ordered by Parse"]), "reads")
    db_instance = default_db_instance()
    return awr, {}, {}, db_instance


def parse_reports_to_json(reports, output_dir, stem, security_level):
    awrs = []
    sql_text = {}
    parameters = {}
    db_instance = default_db_instance()

    for report in reports:
        print("Parsing report to JSON: {}".format(report.name))
        if report.suffix.lower() == ".html":
            awr, sqls, params, dbi = parse_html_report(report, security_level)
        else:
            awr, sqls, params, dbi = parse_text_report(report, security_level)
        awrs.append(awr)
        sql_text.update(sqls)
        parameters.update(params)
        merge_db_instance(db_instance, dbi)

    awrs.sort(key=lambda item: item.get("snap_info", {}).get("begin_snap_id", 0))
    collection = {
        "db_instance_information": db_instance,
        "initialization_parameters": parameters,
        "awrs": awrs,
        "sql_text": sql_text if security_level >= 2 else {},
    }
    json_path = output_dir / "{}.json".format(stem)
    with json_path.open("w", encoding="utf-8") as fh:
        json.dump(collection, fh, indent=2, sort_keys=False)
        fh.write("\n")
    return json_path


def top_elapsed_sql_id_counts_from_json(json_path, limit=10):
    with json_path.open("r", encoding="utf-8") as fh:
        collection = json.load(fh)

    counts = Counter()
    elapsed_totals = Counter()
    first_seen = {}
    ordinal = 0

    for awr in collection.get("awrs", []):
        for sql in awr.get("sql_elapsed_time", []) or []:
            sql_id = normalize_sql_id(sql.get("sql_id", ""))
            if not is_valid_sql_id(sql_id):
                continue
            if sql_id not in first_seen:
                first_seen[sql_id] = ordinal
                ordinal += 1
            counts[sql_id] += 1
            try:
                elapsed_totals[sql_id] += float(sql.get("elapsed_time_s") or 0.0)
            except (TypeError, ValueError):
                pass

    ranked = [
        {
            "sql_id": sql_id,
            "count": count,
            "elapsed_time_s": elapsed_totals[sql_id],
            "first_seen": first_seen.get(sql_id, 0),
        }
        for sql_id, count in counts.items()
    ]
    ranked.sort(key=lambda item: (-item["count"], -item["elapsed_time_s"], item["first_seen"], item["sql_id"]))
    return ranked[:limit]


def merge_plan_sql_ids(top_sqls, manual_sql_ids):
    result = []
    seen = set()
    for item in top_sqls:
        sql_id = item["sql_id"] if isinstance(item, dict) else item
        if sql_id not in seen:
            seen.add(sql_id)
            result.append(sql_id)
    for sql_id in manual_sql_ids:
        if sql_id not in seen:
            seen.add(sql_id)
            result.append(sql_id)
    return result


def sql_literal(value):
    return value.replace("'", "''")


def xplan_sql(sql_id, filename):
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 50000 linesize 32767 trimspool on trimout on tab off
set long 100000000 longchunksize 10000000
set termout off
spool {filename}
select * from table(dbms_xplan.display_cursor('{sql_id}',null));
spool off
set termout on
exit
""".format(
        filename=filename,
        sql_id=sql_literal(sql_id),
    )


def collect_sql_execution_plans(ctx, target_dir, sql_ids):
    target_dir.mkdir(parents=True, exist_ok=True)
    generated = []
    failures = []

    for idx, sql_id in enumerate(sql_ids, start=1):
        filename = "{}.xplan".format(sql_id)
        target = target_dir / filename
        print("Collecting execution plan {}/{}: {}".format(idx, len(sql_ids), filename))
        try:
            run_sqlplus(ctx, xplan_sql(sql_id, filename), cwd=target_dir)
            ensure_generated(target)
            generated.append(target)
        except CollectorError as exc:
            failures.append((sql_id, str(exc)))
            print("WARNING: Could not collect execution plan for {}: {}".format(sql_id, exc))

    return generated, failures


def awr_pairs_sql(start_dt, end_dt):
    start_value = datetime_sql(start_dt)
    end_value = datetime_sql(end_dt)
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 0 trimspool on linesize 32767 tab off
with snapshots as (
    select d.dbid,
           s.instance_number,
           s.snap_id as begin_snap,
           lead(s.snap_id, 1, null) over (
               partition by s.instance_number
               order by s.snap_id
           ) as end_snap
      from dba_hist_snapshot s
      join v$database d on d.dbid = s.dbid
     where s.end_interval_time >= to_date('{start_value}', 'YYYY-MM-DD HH24:MI')
       and s.end_interval_time <= to_date('{end_value}', 'YYYY-MM-DD HH24:MI')
       and s.instance_number = (select instance_number from v$instance)
)
select dbid || '|' || instance_number || '|' || begin_snap || '|' || end_snap
  from snapshots
 where end_snap is not null
 order by instance_number, begin_snap;
exit
""".format(start_value=start_value, end_value=end_value)


def statspack_pairs_sql(start_dt, end_dt):
    start_value = datetime_sql(start_dt)
    end_value = datetime_sql(end_dt)
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 0 trimspool on linesize 32767 tab off
with v_snaps as (
    select snap_id as begin_snap,
           lead(snap_id, 1, null) over (order by snap_id) as end_snap,
           (lead(snap_time, 1, null) over (order by snap_id) - snap_time) * 24 * 60
               as snap_interval_minutes
      from perfstat.STATS$SNAPSHOT
     where startup_time = (
               select max(startup_time)
                 from perfstat.STATS$SNAPSHOT
                where dbid = (select dbid from v$database)
                  and instance_number = (select instance_number from v$instance)
           )
       and dbid = (select dbid from v$database)
       and instance_number = (select instance_number from v$instance)
       and snap_time >= to_date('{start_value}', 'YYYY-MM-DD HH24:MI')
       and snap_time <= to_date('{end_value}', 'YYYY-MM-DD HH24:MI')
)
select begin_snap || '|' || end_snap
  from v_snaps
 where end_snap is not null
   and snap_interval_minutes >= {min_interval}
 order by begin_snap;
exit
""".format(
        start_value=start_value,
        end_value=end_value,
        min_interval=STATSPACK_MIN_INTERVAL_MINUTES,
    )


def discover_awr_pairs(ctx, start_dt, end_dt):
    output = run_sqlplus(ctx, awr_pairs_sql(start_dt, end_dt))
    rows = parse_delimited_rows(output, 4)
    pairs = []
    for dbid, inst_num, begin_snap, end_snap in rows:
        pairs.append(
            {
                "dbid": dbid,
                "inst_num": inst_num,
                "begin_snap": begin_snap,
                "end_snap": end_snap,
            }
        )
    return pairs


def discover_statspack_pairs(ctx, start_dt, end_dt):
    output = run_sqlplus(ctx, statspack_pairs_sql(start_dt, end_dt))
    rows = parse_delimited_rows(output, 2)
    return [{"begin_snap": begin_snap, "end_snap": end_snap} for begin_snap, end_snap in rows]


def awr_report_sql(pair, filename):
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 0 linesize 32767 trimspool on trimout on
set long 100000000 longchunksize 10000000
set termout off
spool {filename}
select output
  from table(dbms_workload_repository.awr_report_html(
       {dbid},
       {inst_num},
       {begin_snap},
       {end_snap},
       0
  ));
spool off
set termout on
exit
""".format(
        filename=filename,
        dbid=pair["dbid"],
        inst_num=pair["inst_num"],
        begin_snap=pair["begin_snap"],
        end_snap=pair["end_snap"],
    )


def statspack_report_sql(pair, filename):
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set termout off
define report_name={filename}
define begin_snap={begin_snap}
define end_snap={end_snap}
@?/rdbms/admin/spreport
set termout on
exit
""".format(
        filename=filename,
        begin_snap=pair["begin_snap"],
        end_snap=pair["end_snap"],
    )


def ensure_generated(path):
    if not path.is_file():
        raise CollectorError("Expected report was not created: {}".format(path))
    if path.stat().st_size == 0:
        raise CollectorError("Generated report is empty: {}".format(path))


def generate_awr_reports(ctx, pairs, output_dir):
    generated = []
    for idx, pair in enumerate(pairs, start=1):
        filename = "awrrpt_{inst}_{begin}_{end}.html".format(
            inst=pair["inst_num"],
            begin=pair["begin_snap"],
            end=pair["end_snap"],
        )
        print("Generating AWR report {}/{}: {}".format(idx, len(pairs), filename))
        run_sqlplus(ctx, awr_report_sql(pair, filename), cwd=output_dir, check_output_errors=False)
        report_path = output_dir / filename
        ensure_generated(report_path)
        generated.append(report_path)
    return generated


def patch_file_once(path, replacements):
    backup = Path(str(path) + ".bak.ora600pl")
    if backup.exists():
        return False

    shutil.copy2(str(path), str(backup))
    data = path.read_text(encoding="latin-1")
    for old, new in replacements:
        data = data.replace(old, new)

    tmp = Path(str(path) + ".tmp.ora600pl")
    tmp.write_text(data, encoding="latin-1")
    tmp.replace(path)
    return True


def prepare_statspack_templates(ctx):
    admin_dir = ctx["oracle_home"] / "rdbms" / "admin"
    sprepcon = admin_dir / "sprepcon.sql"
    sprepins = admin_dir / "sprepins.sql"

    if not sprepcon.is_file() or not sprepins.is_file():
        print("WARNING: Statspack templates were not found under {}".format(admin_dir))
        return

    try:
        changed_con = patch_file_once(
            sprepcon,
            [
                ("linesize_fmt = 80", "linesize_fmt = 83"),
            ],
        )
        changed_ins = patch_file_once(
            sprepins,
            [
                ("col aa format a80", "col aa format a83"),
                ("topn.old_hash_value,10", "st.sql_id,13"),
                ("topn.old_hash_value, 10", "st.sql_id,13"),
                ("topn.old_hash_value,11", "st.sql_id,14"),
                ("topn.module,80", "topn.module,83"),
            ],
        )
    except OSError as exc:
        print("WARNING: Could not patch Statspack templates: {}".format(exc))
        return

    if changed_con or changed_ins:
        print("Statspack templates patched; backups use suffix .bak.ora600pl")


def generate_statspack_reports(ctx, pairs, output_dir):
    generated = []
    prepare_statspack_templates(ctx)
    for idx, pair in enumerate(pairs, start=1):
        filename = "sp_{begin}_{end}.txt".format(
            begin=pair["begin_snap"],
            end=pair["end_snap"],
        )
        print("Generating Statspack report {}/{}: {}".format(idx, len(pairs), filename))
        run_sqlplus(ctx, statspack_report_sql(pair, filename), cwd=output_dir, check_output_errors=False)
        report_path = output_dir / filename
        ensure_generated(report_path)
        generated.append(report_path)
    return generated


def diag_trace_sql():
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 0 trimspool on linesize 32767 tab off
select value from v$diag_info where name = 'Diag Trace';
exit
"""


def get_diag_trace(ctx):
    output = run_sqlplus(ctx, diag_trace_sql())
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    if not lines:
        raise CollectorError("Could not find Diag Trace path in v$diag_info")
    return Path(lines[-1])


ALERT_TS_RE = re.compile(r"^(\d{4}-\d{2}-\d{2})[T ](\d{2}:\d{2}:\d{2})")
ALERT_LEGACY_RE = re.compile(
    r"^(Mon|Tue|Wed|Thu|Fri|Sat|Sun)\s+"
    r"(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+"
    r"(\d{1,2})\s+(\d{2}:\d{2}:\d{2})\s+(\d{4})"
)


def parse_alert_timestamp(line):
    match = ALERT_TS_RE.match(line)
    if match:
        value = "{} {}".format(match.group(1), match.group(2))
        try:
            return datetime.strptime(value, "%Y-%m-%d %H:%M:%S")
        except ValueError:
            return None

    match = ALERT_LEGACY_RE.match(line)
    if match:
        value = "{} {} {} {} {}".format(
            match.group(1),
            match.group(2),
            match.group(3),
            match.group(4),
            match.group(5),
        )
        try:
            return datetime.strptime(value, "%a %b %d %H:%M:%S %Y")
        except ValueError:
            return None

    return None


def find_alert_log(ctx, diag_trace):
    expected = diag_trace / "alert_{}.log".format(ctx["oracle_sid"])
    if expected.is_file():
        return expected

    candidates = []
    try:
        for candidate in diag_trace.glob("alert*.log"):
            if candidate.is_file():
                candidates.append(candidate)
    except OSError:
        candidates = []

    if not candidates:
        raise CollectorError("Alert log not found under {}".format(diag_trace))

    candidates.sort(key=lambda item: item.stat().st_mtime, reverse=True)
    return candidates[0]


def collect_alert_log(ctx, target_dir, start_dt, end_dt):
    diag_trace = get_diag_trace(ctx)
    source = find_alert_log(ctx, diag_trace)
    target_dir.mkdir(parents=True, exist_ok=True)
    target = target_dir / "alert_{}.log".format(ctx["oracle_sid"])

    end_exclusive = end_dt + timedelta(minutes=1)
    seen_timestamps = 0
    written_lines = 0
    current_in_range = False

    with source.open("r", encoding="utf-8", errors="replace") as src:
        with target.open("w", encoding="utf-8") as dst:
            for line in src:
                timestamp = parse_alert_timestamp(line)
                if timestamp is not None:
                    seen_timestamps += 1
                    current_in_range = start_dt <= timestamp < end_exclusive
                if current_in_range:
                    dst.write(line)
                    written_lines += 1

    if seen_timestamps == 0:
        shutil.copy2(str(source), str(target))
        return target, "copied full alert log; timestamp format was not recognized"

    return target, "filtered alert log to requested date range ({} lines)".format(written_lines)


def safe_name(value):
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", value).strip("_")


def collection_stem(report_type, sid, start_dt, end_dt):
    return "{sid}_{kind}_{start}_{end}".format(
        sid=safe_name(sid),
        kind=report_type.lower(),
        start=start_dt.strftime("%Y%m%d_%H%M"),
        end=end_dt.strftime("%Y%m%d_%H%M"),
    )


def unique_output_dir(report_type, sid, start_dt, end_dt):
    base_name = "jasmin_collect_{}".format(collection_stem(report_type, sid, start_dt, end_dt))
    base = Path.cwd() / base_name
    if not base.exists():
        base.mkdir(parents=True)
        return base

    for index in range(2, 1000):
        candidate = Path.cwd() / "{}_{}".format(base_name, index)
        if not candidate.exists():
            candidate.mkdir(parents=True)
            return candidate

    raise CollectorError("Could not create a unique output directory for {}".format(base_name))


def package_mode_label(package_mode):
    if package_mode == PACKAGE_JSON:
        return "JSON only"
    if package_mode == PACKAGE_REPORTS:
        return "full reports only"
    return "full reports and JSON"


def json_attachments_dir(output_dir, json_path):
    return output_dir / "{}_attachments".format(json_path.stem)


def relative_package_path(output_dir, path):
    output_dir = Path(output_dir).resolve()
    path = Path(path).resolve()
    try:
        return path.relative_to(output_dir).as_posix()
    except ValueError:
        return path.name


def should_mask_manifest(package_mode, security_level):
    return package_mode == PACKAGE_JSON and security_level == 0


def write_manifest(
    ctx,
    output_dir,
    report_type,
    start_dt,
    end_dt,
    reports,
    alert_info,
    package_mode,
    json_path,
    security_level,
    xplan_info,
):
    manifest = output_dir / "manifest.txt"
    mask_manifest = should_mask_manifest(package_mode, security_level)
    packaged_files = []
    if package_includes_reports(package_mode):
        packaged_files.extend([relative_package_path(output_dir, report) for report in reports])
    if json_path:
        packaged_files.append(relative_package_path(output_dir, json_path))
    if alert_info:
        packaged_files.append(relative_package_path(output_dir, alert_info[0]))
    if xplan_info:
        packaged_files.extend(
            [relative_package_path(output_dir, xplan_file) for xplan_file in xplan_info.get("files", [])]
        )
    packaged_files.append(manifest.name)

    with manifest.open("w", encoding="utf-8") as fh:
        fh.write("JAS-MIN collector manifest\n")
        fh.write("==========================\n")
        fh.write("ORACLE_SID={}\n".format("masked by security level 0" if mask_manifest else ctx["oracle_sid"]))
        fh.write("ORACLE_HOME={}\n".format("masked by security level 0" if mask_manifest else ctx["oracle_home"]))
        fh.write("report_type={}\n".format(report_type))
        fh.write("start={}\n".format(datetime_sql(start_dt)))
        fh.write("end={}\n".format(datetime_sql(end_dt)))
        fh.write("generated_at={}\n".format(datetime.now().strftime("%Y-%m-%d %H:%M:%S")))
        fh.write("report_count={}\n".format(len(reports)))
        fh.write("package_content={}\n".format(package_mode_label(package_mode)))
        if json_path:
            fh.write("json_file={}\n".format(relative_package_path(output_dir, json_path)))
            fh.write("json_security_level={}\n".format(security_level))
        else:
            fh.write("json_file=not requested\n")
        fh.write("\nGenerated reports:\n")
        for report in reports:
            fh.write("  {}\n".format(relative_package_path(output_dir, report)))
        fh.write("\nAlert log:\n")
        if alert_info:
            alert_path, alert_note = alert_info
            fh.write("  {}\n".format(relative_package_path(output_dir, alert_path)))
            fh.write("  {}\n".format(alert_note))
        else:
            fh.write("  not requested\n")
        fh.write("\nExecution plans:\n")
        if xplan_info and xplan_info.get("requested"):
            top_sqls = xplan_info.get("top_sqls", [])
            if top_sqls:
                fh.write("  top SQL_IDs from SQLs Ordered by Elapsed time:\n")
                for item in top_sqls:
                    fh.write(
                        "    {sql_id} - appearances={count}, elapsed_time_s={elapsed:.3f}\n".format(
                            sql_id=item["sql_id"],
                            count=item["count"],
                            elapsed=item["elapsed_time_s"],
                        )
                    )
            else:
                fh.write("  top SQL_IDs from SQLs Ordered by Elapsed time: none found\n")

            manual_sql_ids = xplan_info.get("manual_sql_ids", [])
            fh.write(
                "  manual SQL_IDs: {}\n".format(
                    ", ".join(manual_sql_ids) if manual_sql_ids else "none"
                )
            )

            files = xplan_info.get("files", [])
            if files:
                fh.write("  generated files:\n")
                for xplan_file in files:
                    fh.write("    {}\n".format(relative_package_path(output_dir, xplan_file)))
            else:
                fh.write("  generated files: none\n")

            failures = xplan_info.get("failures", [])
            if failures:
                fh.write("  failures:\n")
                for sql_id, message in failures:
                    fh.write("    {} - {}\n".format(sql_id, message.replace("\n", " ")))
        else:
            fh.write("  not requested\n")
        fh.write("\nPackaged files:\n")
        for packaged_file in packaged_files:
            fh.write("  {}\n".format(packaged_file))
    return manifest


def create_zip_package(output_dir, stem, reports, json_path, alert_info, xplan_info, manifest, package_mode):
    zip_name = "jasmin_package_{}.zip".format(stem)
    zip_path = output_dir / zip_name
    files = []
    if package_includes_reports(package_mode):
        files.extend(reports)
    if json_path:
        files.append(json_path)
    if alert_info:
        files.append(alert_info[0])
    if xplan_info:
        files.extend(xplan_info.get("files", []))
    files.append(manifest)

    with zipfile.ZipFile(str(zip_path), "w", zipfile.ZIP_DEFLATED) as archive:
        for file_path in files:
            archive.write(str(file_path), arcname=relative_package_path(output_dir, file_path))
    return zip_path


def main(argv=None):
    try:
        args = parse_collector_args(argv)
        ctx = require_oracle_context()
        print("Detected database from environment:")
        print("  ORACLE_SID={}".format(ctx["oracle_sid"]))
        print("  ORACLE_HOME={}".format(ctx["oracle_home"]))

        report_type = args.report_type if args.report_type is not None else ask_report_type()
        start_dt, end_dt = resolve_date_range(args.start_dt, args.end_dt)
        include_alert = (
            args.include_alert
            if args.include_alert is not None
            else ask_yes_no("Include alertlog? (Y/N): ")
        )
        if args.include_sql_plans is None:
            include_sql_plans, manual_sql_ids = ask_sql_execution_plans()
        else:
            include_sql_plans = args.include_sql_plans
            manual_sql_ids = args.manual_sql_ids or []
        package_mode = args.package_mode if args.package_mode is not None else ask_package_mode()
        json_required = package_includes_json(package_mode) or include_sql_plans
        security_level = args.security_level
        if json_required:
            security_level = security_level if security_level is not None else ask_security_level()

        stem = collection_stem(report_type, ctx["oracle_sid"], start_dt, end_dt)
        output_dir = unique_output_dir(report_type, ctx["oracle_sid"], start_dt, end_dt)
        reports_dir = output_dir / stem
        reports_dir.mkdir(parents=True, exist_ok=True)
        print("Output directory: {}".format(output_dir))
        print("Reports directory: {}".format(reports_dir))

        if report_type == "AWR":
            pairs = discover_awr_pairs(ctx, start_dt, end_dt)
            if not pairs:
                raise CollectorError("No AWR snapshot pairs found for the selected date range.")
            reports = generate_awr_reports(ctx, pairs, reports_dir)
        else:
            pairs = discover_statspack_pairs(ctx, start_dt, end_dt)
            if not pairs:
                raise CollectorError(
                    "No Statspack snapshot pairs found for the selected date range "
                    "(minimum interval: {} minutes).".format(STATSPACK_MIN_INTERVAL_MINUTES)
                )
            reports = generate_statspack_reports(ctx, pairs, reports_dir)

        json_path = None
        if json_required:
            print("Parsing generated reports to JAS-MIN JSON...")
            json_path = parse_reports_to_json(reports, output_dir, stem, security_level)
            print("JSON file: {}".format(json_path))

        xplan_info = {
            "requested": include_sql_plans,
            "top_sqls": [],
            "manual_sql_ids": manual_sql_ids,
            "sql_ids": [],
            "files": [],
            "failures": [],
        }
        if include_sql_plans:
            top_sqls = top_elapsed_sql_id_counts_from_json(json_path, 10)
            plan_sql_ids = merge_plan_sql_ids(top_sqls, manual_sql_ids)
            xplan_info["top_sqls"] = top_sqls
            xplan_info["sql_ids"] = plan_sql_ids

            if top_sqls:
                print("Top SQL_IDs from SQLs Ordered by Elapsed time:")
                for item in top_sqls:
                    print(
                        "  {sql_id} - appearances={count}, elapsed_time_s={elapsed:.3f}".format(
                            sql_id=item["sql_id"],
                            count=item["count"],
                            elapsed=item["elapsed_time_s"],
                        )
                    )
            else:
                print("No SQL_IDs found in SQLs Ordered by Elapsed time sections.")

            if plan_sql_ids:
                xplan_target_dir = json_attachments_dir(output_dir, json_path)
                xplan_files, xplan_failures = collect_sql_execution_plans(ctx, xplan_target_dir, plan_sql_ids)
                xplan_info["files"] = xplan_files
                xplan_info["failures"] = xplan_failures
                print("Execution plan attachment(s): {}".format(len(xplan_files)))
            else:
                print("No SQL_IDs selected for execution plan collection.")

        alert_info = None
        if include_alert:
            print("Collecting alert log...")
            alert_target_dir = output_dir
            if json_path:
                alert_target_dir = json_attachments_dir(output_dir, json_path)
            alert_info = collect_alert_log(ctx, alert_target_dir, start_dt, end_dt)
            print("Alert log: {}".format(alert_info[1]))

        manifest = write_manifest(
            ctx,
            output_dir,
            report_type,
            start_dt,
            end_dt,
            reports,
            alert_info,
            package_mode,
            json_path,
            security_level,
            xplan_info,
        )
        zip_path = create_zip_package(
            output_dir,
            stem,
            reports,
            json_path,
            alert_info,
            xplan_info,
            manifest,
            package_mode,
        )
        print("Manifest: {}".format(manifest))
        print("ZIP package: {}".format(zip_path))
        print("Done. Collected {} report(s) in {}".format(len(reports), output_dir))
        return 0
    except KeyboardInterrupt:
        print("\nCancelled.")
        return 130
    except CollectorError as exc:
        print("ERROR: {}".format(exc), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
