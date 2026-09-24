#!/usr/bin/env python3
"""
JAS-MIN report collector.

Interactive and command-line helper for collecting Oracle AWR or Statspack
reports for a date range detected from ORACLE_HOME and ORACLE_SID. The script
intentionally uses only Python standard library modules.
"""

import argparse
import hashlib
import os
import platform
import re
import shutil
import subprocess
import sys
import zipfile
import xml.etree.ElementTree as ET
from collections import Counter
from datetime import datetime, timedelta, timezone
from html import unescape
from html.parser import HTMLParser
import json
import math
from pathlib import Path


# Version the standalone collector independently from the Rust application.
COLLECTOR_VERSION = "0.1.15"
COLLECTOR_NAME = "jas-min-collector"

DATE_FORMAT = "%Y-%m-%d %H:%M"
SNAPSHOT_DATE_FORMAT = "%Y-%m-%d %H:%M:%S"
DATE_FORMAT_LABEL = "YYYY-MM-DD HH24:MI[:SS]"
NLS_LANG = "AMERICAN_AMERICA.AL32UTF8"
PACKAGE_REPORTS = "reports"
PACKAGE_JSON = "json"
PACKAGE_BOTH = "both"
SQL_ID_RE = re.compile(r"^[A-Za-z0-9]{1,30}$")
ORACLE_SQL_ID_RE = re.compile(r"^[0-9a-z]{13}$", re.IGNORECASE)
SHARED_CURSOR_REASON_SUFFIX = ".shared_cursor_reasons"
DEFAULT_XPLAN_TIMEOUT_SECONDS = 120


class CollectorError(Exception):
    """Expected runtime error shown without a traceback."""


def collector_identity():
    """Identify the release and the exact script copied to the Oracle host."""
    # Hash the script itself; this also identifies locally modified copies.
    try:
        script_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    except OSError as exc:
        raise CollectorError("Could not identify collector script: {}".format(exc))
    return {
        "name": COLLECTOR_NAME,
        "version": COLLECTOR_VERSION,
        "script_sha256": script_hash,
    }


def tail(text, limit=4000):
    if len(text) <= limit:
        return text
    return text[-limit:]


def parse_datetime(value):
    # Accept existing minute inputs and exact snapshot boundaries copied from the menu.
    for date_format in (DATE_FORMAT, SNAPSHOT_DATE_FORMAT):
        try:
            return datetime.strptime(value, date_format)
        except ValueError:
            continue
    raise CollectorError("Invalid date format. Expected: {}".format(DATE_FORMAT_LABEL))


def parse_datetime_arg(value):
    try:
        return parse_datetime(value)
    except CollectorError as exc:
        raise argparse.ArgumentTypeError(str(exc))


def datetime_sql(value):
    # Preserve seconds when present, while keeping existing minute labels familiar.
    return value.strftime(SNAPSHOT_DATE_FORMAT if value.second else DATE_FORMAT)


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


def ask_os_stats_dir():
    while True:
        value = input("Enter OS statistics directory: ").strip()
        try:
            return validate_existing_dir(value)
        except CollectorError as exc:
            print(exc)


def validate_existing_dir(value):
    if not value:
        raise CollectorError("Directory path cannot be empty.")
    path = Path(value).expanduser()
    if not path.is_dir():
        raise CollectorError("Directory does not exist or is not a directory: {}".format(value))
    return path.resolve()


def parse_existing_dir_arg(value):
    try:
        return validate_existing_dir(value)
    except CollectorError as exc:
        raise argparse.ArgumentTypeError(str(exc))


def os_stats_platform_dir_name(system_name=None):
    system_name = system_name if system_name is not None else platform.system()
    normalized = (system_name or "").strip().lower()
    if normalized == "aix":
        return "AIX"
    if normalized == "linux":
        return "linux"
    raise CollectorError(
        "OS statistics attachments are supported only on AIX and Linux (detected: {}).".format(
            system_name or "unknown"
        )
    )


def list_os_stats_files(source_dir):
    source_dir = Path(source_dir)
    files = sorted([path for path in source_dir.rglob("*") if path.is_file()])
    if not files:
        raise CollectorError("No OS statistics files found under {}".format(source_dir))
    return files


def ask_os_stats_options(include_os_stats, os_stats_dir):
    if include_os_stats is None:
        include_os_stats = ask_yes_no("Include OS statistics? (Y/N): ")
    if not include_os_stats:
        return None, []

    if os_stats_dir is not None:
        try:
            return os_stats_dir, list_os_stats_files(os_stats_dir)
        except OSError as exc:
            raise CollectorError("Could not read OS statistics directory {}: {}".format(os_stats_dir, exc))

    while True:
        source_dir = ask_os_stats_dir()
        try:
            return source_dir, list_os_stats_files(source_dir)
        except CollectorError as exc:
            print(exc)
        except OSError as exc:
            print("Could not read OS statistics directory {}: {}".format(source_dir, exc))


def attachments_dir(output_dir, stem):
    return output_dir / "{}_attachments".format(stem)


def copy_os_stats(source_dir, source_files, output_dir, stem, platform_dir_name=None):
    platform_dir_name = platform_dir_name or os_stats_platform_dir_name()
    target_dir = attachments_dir(output_dir, stem) / platform_dir_name
    copied_files = []

    try:
        target_dir.mkdir(parents=True, exist_ok=True)
        for source_file in source_files:
            relative_path = source_file.relative_to(source_dir)
            target_file = target_dir / relative_path
            if source_file.resolve() == target_file.resolve():
                raise CollectorError("OS statistics source and target file are the same: {}".format(source_file))
            target_file.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(str(source_file), str(target_file))
            copied_files.append(target_file)
    except OSError as exc:
        raise CollectorError("Could not copy OS statistics from {}: {}".format(source_dir, exc))

    return {
        "requested": True,
        "source_dir": source_dir,
        "platform": platform_dir_name,
        "target_dir": target_dir,
        "files": copied_files,
    }


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


def is_oracle_sql_id(value):
    """Recognize the fixed-width SQL identifier emitted by Oracle reports."""
    return bool(ORACLE_SQL_ID_RE.fullmatch(normalize_sql_id(value)))


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


def parse_positive_int_arg(value):
    try:
        number = int(value)
    except (TypeError, ValueError):
        raise argparse.ArgumentTypeError("Expected a positive integer.")
    if number <= 0:
        raise argparse.ArgumentTypeError("Expected a positive integer.")
    return number


def build_arg_parser():
    examples = """examples:
  python3 jas-min-collector.py --report-type awr --start "2026-06-14 00:00" --end "2026-06-15 14:00"
  python3 jas-min-collector.py --report-type statspack --start "2026-06-14 00:00" --end "2026-06-15 14:00" --no-alert-log --no-execution-plans --package-content reports
  python3 jas-min-collector.py --report-type awr --start "2026-06-14 00:00" --end "2026-06-15 14:00" --include-alert-log --execution-plans --sql-id abc123,def456 --package-content both --security-level 1
  python3 jas-min-collector.py --report-type awr --start "2026-06-14 00:00" --end "2026-06-15 14:00" --os-stats-dir /path/to/os-stats --package-content reports
"""
    parser = argparse.ArgumentParser(
        description="Collect Oracle AWR or Statspack reports and package them for JAS-MIN.",
        epilog=examples,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    # argparse exits here, so identifying the version needs no Oracle connection.
    parser.add_argument(
        "--version", action="version",
        version="{} {}".format(COLLECTOR_NAME, COLLECTOR_VERSION),
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
        "--execution-plan-timeout",
        dest="execution_plan_timeout",
        metavar="SECONDS",
        type=parse_positive_int_arg,
        default=DEFAULT_XPLAN_TIMEOUT_SECONDS,
        help="maximum time for cursor discovery or one execution plan (default: %(default)s seconds)",
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
    os_stats_group = parser.add_mutually_exclusive_group()
    os_stats_group.add_argument(
        "--include-os-stats",
        "--os-stats",
        dest="include_os_stats",
        action="store_true",
        default=None,
        help="include operating system statistics",
    )
    os_stats_group.add_argument(
        "--no-os-stats",
        dest="include_os_stats",
        action="store_false",
        help="do not include operating system statistics",
    )
    parser.add_argument(
        "--os-stats-dir",
        dest="os_stats_dir",
        metavar="DIR",
        type=parse_existing_dir_arg,
        help="directory containing prepared operating system statistics files",
    )
    parser.add_argument(
        "--access-path-evidence", type=Path, metavar="JSON",
        help="merge scoped SQL/segment evidence into collected JSON; exact DB/instance/window match required; requires security level 2",
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

    if args.os_stats_dir is not None:
        if args.include_os_stats is False:
            parser.error("--os-stats-dir can only be used when OS stats are enabled")
        if args.include_os_stats is None:
            args.include_os_stats = True

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


def run_sqlplus(ctx, script, cwd=None, check_output_errors=True, timeout=None):
    command = [str(ctx["sqlplus"]), "-S", "/ as sysdba"]
    try:
        proc = subprocess.run(
            command,
            input=script,
            universal_newlines=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            cwd=str(cwd) if cwd else None,
            env=ctx["env"],
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        output_parts = []
        for value in (exc.stdout, exc.stderr):
            if isinstance(value, bytes):
                value = value.decode("utf-8", errors="replace")
            if value:
                output_parts.append(value)
        output = "".join(output_parts)
        detail = "\n{}".format(tail(output).strip()) if output.strip() else ""
        raise CollectorError(
            "sqlplus timed out after {} second(s){}".format(timeout, detail)
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


# Keep this catalog aligned with src/staticdata.rs so collector JSON and
# jas-min -d classify foreground/background waits the same way.
IDLE_EVENT_PREFIXES = (
    "cached session",
    "VKTM Logical Idle Wait",
    "VKTM Init Wait for GSGA",
    "IORM Scheduler Slave Idle Wait",
    "rdbms ipc message",
    "i/o slave wait",
    "OFS Receive Queue",
    "OFS idle",
    "Generic Process Pool Dispatcher: idle",
    "Generic Process Pool Worker: sleep",
    "VKRM Idle",
    "wait for unread message on broadcast channel",
    "wait for unread message on multiple broadcast channels",
    "class slave wait",
    "idle class spare wait event 1",
    "idle class spare wait event 2",
    "idle class spare wait event 3",
    "idle class spare wait event 4",
    "idle class spare wait event 5",
    "idle class spare wait event 6",
    "idle class spare wait event 7",
    "idle class spare wait event 8",
    "idle class spare wait event 9",
    "idle class spare wait event 10",
    "RMA: IPC0 completion sync",
    "PING",
    "spawn request deferred",
    "watchdog main loop",
    "process in prespawned state",
    "pmon timer",
    "pman timer",
    "DNFS disp IO slave idle",
    "NVM disp IO slave idle",
    "BRDG: bridge controller idle",
    "Network Retrans by Server",
    "Network Retrans by Client",
    "Distributed Trace: Archival Worker Idle",
    "DIAG idle wait",
    "ges remote message",
    "SCM slave idle",
    "LMS CR slave timer",
    "gcs remote message",
    "gcs yield cpu",
    "heartbeat monitor sleep",
    "GCR sleep",
    "Shutdown completion due to error",
    "SGA: MMAN sleep for component shrink",
    "DBWR timer",
    "Data Guard: Gap Manager",
    "Data Guard: controlfile update",
    "MRP redo arrival",
    "Data Guard: Timer",
    "LNS ASYNC archive log",
    "LNS ASYNC dest activation",
    "LNS ASYNC end of log",
    "Archiver: redo logs",
    "simulated log write delay",
    "heartbeat redo informer",
    "LGWR real time apply sync",
    "LGWR worker group idle",
    "parallel recovery slave idle wait",
    "Backup Appliance waiting for work",
    "Backup Appliance waiting restore start",
    "Backup Appliance Surrogate wait",
    "Backup Appliance Servlet wait",
    "Backup Appliance Comm SGA setup wait",
    "LogMiner builder: idle",
    "LogMiner builder: branch",
    "LogMiner preparer: idle",
    "LogMiner reader: log (idle)",
    "LogMiner reader: redo (idle)",
    "LogMiner merger: idle",
    "LogMiner client: transaction",
    "LogMiner: other",
    "LogMiner: activate",
    "LogMiner: reset",
    "LogMiner: find session",
    "LogMiner: internal",
    "Logical Standby Apply Delay",
    "parallel recovery coordinator waits for slave cleanup",
    "parallel recovery coordinator idle wait",
    "parallel recovery control message reply",
    "parallel recovery slave next change",
    "nologging fetch slave idle",
    "recovery sender idle",
    "recovery receiver idle",
    "recovery coordinator idle",
    "recovery logmerger idle",
    "block compare coord process idle",
    "Data Guard PDB query SCN service idle",
    "True Cache: background process idle",
    "PX Deq: Txn Recovery Start",
    "PX Deq: Txn Recovery Reply",
    "fbar timer",
    "smon timer",
    "PX Deq: Metadata Update",
    "Space Manager: slave idle wait",
    "PX Deq: Index Merge Reply",
    "PX Deq: Index Merge Execute",
    "PX Deq: Index Merge Close",
    "PX Deq: kdcph_mai",
    "PX Deq: kdcphc_ack",
    "imco timer",
    "IMFS defer writes scheduler",
    "memoptimize write drain idle",
    "MLE sleep",
    "virtual circuit next request",
    "shared server idle wait",
    "dispatcher timer",
    "cmon timer",
    "pool server timer",
    "lreg timer",
    "JOX Jit Process Sleep",
    "jobq slave wait",
    "pipe get",
    "PX Deque wait",
    "PX Idle Wait",
    "PX Deq Credit: need buffer",
    "PX Deq Credit: send blkd",
    "PX Deq: Msg Fragment",
    "PX Deq: Parse Reply",
    "PX Deq: Execute Reply",
    "PX Deq: Execution Msg",
    "PX Deq: Table Q Normal",
    "PX Deq: Table Q Sample",
    "REPL Apply: txns",
    "REPL Capture/Apply: messages",
    "REPL Capture: archive log",
    "single-task message",
    "SQL*Net message from client",
    "SQL*Net vector message from client",
    "SQL*Net vector message from dblink",
    "PL/SQL lock timer",
    "Streams AQ: emn coordinator idle wait",
    "EMON slave idle wait",
    "Emon coordinator main loop",
    "Emon slave main loop",
    "Streams AQ: waiting for messages in the queue",
    "Streams AQ: waiting for time management or cleanup tasks",
    "Streams AQ: delete acknowledged messages",
    "Streams AQ: deallocate messages from Streams Pool",
    "Streams AQ: qmn coordinator idle wait",
    "Streams AQ: qmn slave idle wait",
    "AQ: 12c message cache init wait",
    "AQ Cross Master idle",
    "AQPC idle",
    "Streams AQ: load balancer idle",
    "Sharded  Queues : Part Maintenance idle",
    "Sharded  Queues : Part Truncate idle",
    "REPL Capture/Apply: RAC AQ qmn coordinator",
    "Streams AQ: opt idle",
    "HS message to agent",
    "ASM background timer",
    "ASM cluster membership changes",
    "AUTO access ASM_CLIENT registration",
    "iowp msg",
    "iowp file id",
    "netp network",
    "gopp msg",
    "auto-sqltune: wait graph update",
    "WCR: replay client notify",
    "WCR: replay clock",
    "WCR: replay paused",
    "JS external job",
    "cell worker idle",
    "Multi-Tenant Redo File Server - Flush Header Interval",
    "Sharding replication",
    "Consensus service idle",
    "Blockchain apply clean",
    "blockchain apply short",
    "blockchain apply long",
    "Blockchain reader process idle",
)


def is_idle_event(name):
    # STATSPACK truncates names to 28 characters, so match the Rust parser's
    # full-name prefix rule instead of requiring an exact string.
    event = normalize_cell(name)
    return any(idle.startswith(event) for idle in IDLE_EVENT_PREFIXES)


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
        "data_availability": {},
        "access_path_observations": [],
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


def mark_data_availability(awr):
    """Empty sections remain unknown; legacy numeric placeholders are not measurements."""
    domains = ("load_profile", "instance_stats", "sql_elapsed_time", "sql_gets",
               "time_model_stats", "foreground_wait_events", "segment_stats")
    mask = {key: bool(awr.get(key)) for key in domains}
    host = awr["host_cpu"]
    percentages = [host[key] for key in ("pct_user", "pct_system", "pct_wio", "pct_idle")]
    mask["host_cpu"] = all(0 <= x <= 100 for x in percentages) and 95 <= sum(percentages) <= 105
    mask["ash"] = bool(awr["top_sql_with_top_events"])
    mask["access_path_observations"] = bool(awr["access_path_observations"])
    awr["data_availability"] = mask


def evidence_integer(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def validate_evidence_scope(scope):
    for name in ("dbid", "inst_id", "con_id"):
        if not evidence_integer(scope.get(name)):
            raise CollectorError("Invalid scope identity: " + name)
    for name in ("child_number", "plan_hash_value", "object_id", "data_object_id"):
        if scope.get(name) is not None and not evidence_integer(scope[name]):
            raise CollectorError("Invalid optional scope identity: " + name)
    if not isinstance(scope.get("sql_id"), str) or not SQL_ID_RE.fullmatch(scope["sql_id"]):
        raise CollectorError("Invalid SQL_ID")


def merge_access_path_evidence(json_path, evidence_path):
    """Validate every window before writing; no ordinal, nearest-time or cross-RAC joins."""
    try:
        collection = json.loads(Path(json_path).read_text(encoding="utf-8"))
        evidence = json.loads(Path(evidence_path).read_text(encoding="utf-8"))
        dbi = collection["db_instance_information"]
        if evidence["schema_version"] != "2026-09-13.1":
            raise CollectorError("Unsupported access-path evidence schema_version")
        if evidence["dbid"] != dbi["db_id"] or evidence["inst_id"] != dbi["instance_num"]:
            raise CollectorError("Access-path evidence DBID/INST_ID does not match collection")
        index = {}
        keys = ("begin_snap_id", "end_snap_id", "begin_snap_time", "end_snap_time")
        for awr in collection["awrs"]:
            key = tuple(awr["snap_info"][k] for k in keys)
            if key in index:
                raise CollectorError("Ambiguous duplicate collection window")
            index[key] = awr
        seen = set()
        for window in evidence["windows"]:
            key = tuple(window["snap_info"][k] for k in keys)
            if key in seen or key not in index:
                raise CollectorError("Evidence window must match exactly one DB/instance/snapshot/time interval")
            seen.add(key)
            awr = index[key]
            if awr.get("access_path_observations"):
                raise CollectorError("Refusing to overwrite existing access-path observations")
            identities = set()
            for observation in window["observations"]:
                scope = observation["scope"]
                validate_evidence_scope(scope)
                if scope["dbid"] != evidence["dbid"] or scope["inst_id"] != evidence["inst_id"]:
                    raise CollectorError("Observation identity does not match evidence DBID/INST_ID")
                identity = tuple(scope.get(k) for k in ("dbid", "inst_id", "con_id", "sql_id", "child_number", "plan_hash_value", "object_id", "data_object_id"))
                if identity in identities:
                    raise CollectorError("Duplicate SQL/child/plan/segment observation in one window")
                identities.add(identity)
                if not isinstance(scope.get("con_id"), int) or scope["con_id"] < 0 or not SQL_ID_RE.fullmatch(scope["sql_id"]):
                    raise CollectorError("Invalid SQL/container identity")
                if not observation["evidence_ref"].strip() or not evidence_integer(observation["executions"]) or observation["executions"] <= 0:
                    raise CollectorError("Observation requires an evidence reference and positive execution delta")
                for metric in ("buffer_gets", "elapsed_s", "scan_blocks", "continued_rows"):
                    value = observation.get(metric)
                    if value is not None and (isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0):
                        raise CollectorError("Invalid or reset delta: " + metric)
                    if metric in ("scan_blocks", "continued_rows") and value is not None and not evidence_integer(value):
                        raise CollectorError("Expected integer delta: " + metric)
                work = observation.get("useful_work")
                if work is not None:
                    completed = work.get("completed")
                    if not work["name"].strip() or not work["unit"].strip() or isinstance(completed, bool) or not isinstance(completed, (int, float)) or not math.isfinite(completed) or completed <= 0:
                        raise CollectorError("Useful work requires a name, unit and positive measured count")
                structure = observation.get("structure")
                if structure is not None:
                    for field in ("evidence_ref", "observed_at", "method"):
                        if not isinstance(structure.get(field), str) or not structure[field].strip():
                            raise CollectorError("Structural evidence requires " + field)
                    for field in ("blocks_below_hwm", "verified_empty_blocks_below_hwm", "fs4_blocks", "chained_rows"):
                        if structure.get(field) is not None and not evidence_integer(structure[field]):
                            raise CollectorError("Structural counts must be nonnegative integers")
                intervention = observation.get("intervention")
                if intervention is not None:
                    validate_evidence_scope(intervention["after_scope"])
                    for field in ("before_begin_snap_id", "after_begin_snap_id"):
                        if not evidence_integer(intervention.get(field)):
                            raise CollectorError("Intervention requires snapshot identity: " + field)
                    for field in ("same_plan", "same_logical_data", "comparable_cache_and_concurrency", "equivalent_work"):
                        if not isinstance(intervention.get(field), bool):
                            raise CollectorError("Intervention controls must be explicit booleans")
                    for field in ("evidence_ref", "controls_evidence_ref"):
                        if not isinstance(intervention.get(field), str) or not intervention[field].strip():
                            raise CollectorError("Intervention requires " + field)
            awr["access_path_observations"] = window["observations"]
            mask = window.get("data_availability", {})
            if any(not isinstance(v, bool) for v in mask.values()):
                raise CollectorError("Availability masks must contain booleans")
            awr.setdefault("data_availability", {}).update(mask)
            awr["data_availability"]["access_path_observations"] = bool(window["observations"])
        Path(json_path).write_text(json.dumps(collection, indent=2, allow_nan=False) + "\n", encoding="utf-8")
    except CollectorError:
        raise
    except (OSError, KeyError, TypeError, ValueError, AttributeError) as exc:
        raise CollectorError("Invalid access-path evidence: {}".format(exc))


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
                # Some AWR releases emit these two percentages with a decimal comma.
                "pct_cpu": parse_float(row[5].replace(",", ".")),
                "pct_io": parse_float(row[6].replace(",", ".")),
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
                "avg_time": round(parse_wait_ms(row[8]), 6) if normalize_cell(row[8]) else None,
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
    headers = [normalize_cell(cell).lower() for row in header_rows(table) for cell in row]

    def field(row, names, fallback=None):
        index = next((headers.index(name) for name in names if name in headers), fallback)
        return (row[index] or None) if index is not None and index < len(row) else None

    def identifier(value):
        value = (value or "").replace(",", "")
        return int(value) if value.isdigit() else None

    for row in data_rows(table):
        if len(row) < 7:
            continue
        legacy = len(row) == 7
        raw_value = field(row, [stat_name.lower()], 5 if legacy else 7)
        try:
            value = float((raw_value or "").replace(",", ""))
        except ValueError:
            continue
        if not math.isfinite(value) or value < 0:
            continue
        result.append({
            "owner": field(row, ["owner"]) if security_level > 0 else None,
            "pdb_name": field(row, ["pdb name", "container name"]) if security_level > 0 else None,
            "con_id": identifier(field(row, ["con_id", "con id", "container id"])),
            "subobject_name": field(row, ["subobject name", "subobject"]) if security_level > 0 else None,
            "obj": identifier(field(row, ["obj#", "object id"], None if legacy else 5)) or 0,
            "objd": identifier(field(row, ["dataobj#", "data object id"], None if legacy else 6)) or 0,
            "object_name": (field(row, ["object name"], 2) or "") if security_level > 0 else "#",
            "object_type": field(row, ["obj. type", "object type"], 4) or "",
            "stat_name": stat_name,
            "stat_vlalue": value,
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
        if len(row) < 2 or not row[0].strip():
            continue
        name, value = row[0].strip(), row[1].strip()
        # Preserve ordered multi-valued rows in the existing string schema.
        previous = result.setdefault(name, "")
        if value:
            result[name] = previous + ", " + value if previous else value
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

    mark_data_availability(awr)
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
            date = re.search(r"\d{1,2}-[A-Za-z]{3}-\d{2,4}\s+\d{2}:\d{2}:\d{2}", line)
            if date:
                result["begin_snap_time"] = date.group(0)
        elif "End Snap:" in line or re.search(r"\bEnd\s+Snap", line):
            nums = re.findall(r"\d+", line)
            if nums:
                result["end_snap_id"] = int(nums[0])
            date = re.search(r"\d{1,2}-[A-Za-z]{3}-\d{2,4}\s+\d{2}:\d{2}:\d{2}", line)
            if date:
                result["end_snap_time"] = date.group(0)
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
        raw_waits = line[29:41].strip() if len(line) > 41 else ""
        if not event or not re.fullmatch(r"[\d,]+", raw_waits) or is_idle_event(event):
            continue
        waits = parse_int(raw_waits)
        result.append({
            "event": event,
            "waits": waits,
            "total_wait_time_s": parse_float(line[46:57] if len(line) > 57 else ""),
            "avg_wait": parse_wait_ms(line[57:64] if len(line) > 64 else ""),
            "pct_dbtime": parse_float(line[73:80] if len(line) >= 80 else ""),
            "waitevent_histogram_ms": {},
        })
    return result


def is_statspack_sql_row(fields):
    """Reject wrapped SQL text that happens to contain seven whitespace fields."""
    sql_key = normalize_sql_id(fields[6]) if len(fields) == 7 else ""
    if len(fields) != 7 or not (5 <= len(sql_key) <= 20 and sql_key.isalnum()):
        return False

    # A real TOP SQL row has six numeric metrics before its Oracle SQL_ID.
    for value in fields[:6]:
        normalized = normalize_cell(value).replace(",", "").replace("%", "")
        try:
            if not math.isfinite(float(normalized)):
                return False
        except ValueError:
            return False
    return True


def parse_text_sql_section(lines, kind):
    items = {} if kind != "elapsed" else []
    last_sql_id = ""
    for line in lines:
        fields = line.split()
        if is_statspack_sql_row(fields):
            sql_id = normalize_sql_id(fields[6])
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


def parse_text_instance_efficiency(lines):
    """Read both percentage pairs and stop before Shared Pool statistics."""
    start = next((idx for idx, line in enumerate(lines) if "Instance Efficiency" in line), None)
    if start is None:
        return []

    result = []
    pair = re.compile(r"([^:]+):\s*(\S+)")
    for raw_line in lines[start + 1:]:
        line = raw_line.strip()
        if not line:
            if result:
                break
            continue
        if line.startswith("Shared Pool") or line.startswith("Top "):
            break
        for match in pair.finditer(line):
            value = parse_float(match.group(2), None)
            result.append({
                "eff_stat": normalize_cell(match.group(1)),
                "eff_pct": value if value is None or value >= 0 else None,
            })
    return result


def parse_text_host_cpu(lines):
    """Parse the Host CPU header and the first numeric data row below it."""
    result = default_awr("")["host_cpu"]
    section = find_text_section(lines, "Host CPU", ["Instance CPU"])
    header = next((line for line in lines if "Host CPU" in line), "")
    match = re.search(r"CPUs:\s*(\d+)\s*Cores:\s*(\d+)\s*Sockets:\s*(\d+)", header)
    if match:
        result["cpus"], result["cores"], result["sockets"] = map(int, match.groups())

    for line in section:
        columns = line.split()
        if len(columns) >= 6 and all(re.fullmatch(r"[\d.,]+", value) for value in columns[:6]):
            result.update({
                "load_avg_begin": parse_float(columns[0]),
                "load_avg_end": parse_float(columns[1]),
                "pct_user": parse_float(columns[2]),
                "pct_system": parse_float(columns[3]),
                "pct_idle": parse_float(columns[4]),
                "pct_wio": parse_float(columns[5]),
            })
            break
    return result


def parse_text_redo_log(lines):
    """Extract the derived log-switch rate from the load-profile area."""
    for line in lines:
        if "log switches (derived)" in line:
            values = line.split()
            return {
                "stat_name": "log switches (derived)",
                "per_hour": parse_float(values[-1]) if values else 0.0,
            }
    return default_awr("")["redo_log"]


def parse_text_time_model(lines):
    """Read the fixed-width Time Model rows used by STATSPACK."""
    result = []
    for line in lines:
        if len(line) < 56:
            continue
        stat_name = line[:35].strip()
        raw_time = line[35:56].strip()
        if not stat_name or stat_name.startswith("-") or not re.fullmatch(r"[\d,.]+", raw_time):
            continue
        result.append({
            "stat_name": stat_name,
            "time_s": parse_float(raw_time),
            "pct_dbtime": parse_float(line[56:66]) if len(line) >= 66 else 0.0,
        })
    return result


def parse_text_instance_stats(lines):
    """Read fixed-width instance counters while discarding page headers."""
    result = []
    for line in lines:
        if len(line) < 52:
            continue
        stat_name = line[:35].strip()
        raw_total = line[35:52].strip()
        if stat_name and re.fullmatch(r"[\d,]+", raw_total):
            result.append({"statname": stat_name, "total": parse_int(raw_total)})
    return result


def parse_text_dictionary_cache(lines):
    """Read dictionary-cache request and final-usage counters by column."""
    result = []
    for line in lines:
        if len(line) < 77:
            continue
        raw_gets = line[26:38].strip()
        raw_usage = line[69:79].strip()
        if re.fullmatch(r"[\d,]+", raw_gets) and re.fullmatch(r"[\d,]+", raw_usage):
            result.append({
                "statname": line[:25].strip(),
                "get_requests": parse_int(raw_gets),
                "final_usage": parse_int(raw_usage),
            })
    return result


def parse_text_library_cache(lines):
    """Join each namespace row with its wrapped continuation row."""
    data_lines = []
    header_words = {"Get", "Pct", "Pin", "Requests", "Miss", "Reloads", "DB/Inst:"}
    for line in lines:
        trimmed = line.strip()
        tokens = trimmed.split()
        if (not trimmed or trimmed.startswith("---") or trimmed.startswith("Library Cache")
                or trimmed.startswith("->") or "Namespace" in trimmed
                or "Invali-" in trimmed or "dations" in trimmed
                or (tokens and all(token in header_words for token in tokens))):
            continue
        data_lines.append(line)

    result = []
    idx = 0
    while idx < len(data_lines):
        first = data_lines[idx]
        if first and not first[0].isspace():
            boundary = re.search(r"\s{2,}(?=[\d-])", first)
            if boundary:
                values = first[boundary.end():].split()
                if idx + 1 < len(data_lines) and data_lines[idx + 1][:1].isspace():
                    idx += 1
                    values.extend(data_lines[idx].split())
                if len(values) >= 2:
                    result.append({
                        "statname": first[:boundary.start()].strip(),
                        "get_requests": parse_int(values[0]),
                        "get_pct_miss": parse_float(values[1]),
                        "pin_requests": parse_int(values[2]) if len(values) > 2 else 0,
                    })
        idx += 1
    return result


def parse_text_latch_activity(lines):
    """Read latch activity using the same stable columns as awr.rs."""
    result = []
    for line in lines:
        if len(line) < 72 or line.startswith(" "):
            continue
        raw_gets = line[25:39].strip()
        raw_miss = line[40:46].strip()
        raw_wait = line[54:60].strip()
        if (re.fullmatch(r"[\d,]+", raw_gets)
                and re.fullmatch(r"[\d,.]+", raw_miss)
                and re.fullmatch(r"[\d,.]+", raw_wait)):
            result.append({
                "statname": line[:24].strip(),
                "get_requests": parse_int(raw_gets),
                "get_pct_miss": parse_float(raw_miss),
                "wait_time": parse_float(raw_wait),
            })
    return result


def parse_text_io_stats(lines):
    """Parse STATSPACK I/O summary tokens, including K/M/G/T/P suffixes."""
    result = {}
    in_data = False

    def is_numeric_or_volume(value):
        if value in ("", "."):
            return True
        number = value[:-1] if value[-1:].isalpha() else value
        try:
            float(number)
            return True
        except ValueError:
            return False

    def text_data_size(value):
        value = value.strip().replace(",", ".")
        if not value or value == ".":
            return 0.0
        unit = value[-1]
        try:
            number = float(value[:-1])
        except ValueError:
            return 0.0
        return number * {"K": 1 / 1024.0, "M": 1.0, "G": 1024.0,
                         "T": 1024.0 * 1024.0}.get(unit, 1.0)

    for raw_line in lines:
        line = raw_line.strip()
        if not line or line.startswith("->") or line.startswith("IO Stat"):
            continue
        if "Function" in line and "Volume" in line:
            in_data = True
            continue
        if line.startswith(("---", "===")) or "-----" in line or not in_data:
            continue

        parts = line.split()
        data_start = next((idx for idx, value in enumerate(parts) if is_numeric_or_volume(value)), len(parts))
        name = " ".join(parts[:data_start])
        if name == "Buffer Cache Re":
            name = "Buffer Cache Reads"
        values = parts[data_start:]
        values.extend([""] * (8 - len(values)))
        if not name or name == "TOTAL:":
            continue
        avg_time = None if values[7] in ("", ".") else round(parse_wait_ms(values[7]), 6)
        result[name] = {
            "reads_data": text_data_size(values[0]),
            "reads_req_s": parse_float(values[1].replace(",", ".")),
            "reads_data_s": text_data_size(values[2]),
            "writes_data": text_data_size(values[3]),
            "writes_req_s": parse_float(values[4].replace(",", ".")),
            "writes_data_s": text_data_size(values[5]),
            "waits_count": parse_count(values[6]),
            "avg_time": avg_time,
        }
    return result


def parse_text_wait_histogram(lines, event_names):
    """Attach fixed-width histogram percentages to full wait-event names."""
    result = {}
    name_map = {(name[:26] if len(name) >= 26 else name): name for name in event_names}
    bucket_names = ("1: <1ms", "2: <2ms", "3: <4ms", "4: <8ms",
                    "5: <16ms", "6: <32ms", "7: <=1s", "8: >1s")
    for line in lines:
        if len(line) <= 26:
            continue
        short_name = line[:26].strip()
        if short_name not in name_map:
            continue
        values = {}
        for idx, start in enumerate(range(33, 81, 6)):
            values[bucket_names[idx]] = parse_float(line[start:start + 5])
        result[name_map[short_name]] = values
    return result


def parse_text_sql_text(lines):
    """Collect wrapped SQL text and ignore repeated STATSPACK page headers."""
    result = {}
    current_key = ""
    current_sql = []
    collecting = False
    sql_start = re.compile(r"^(SELECT|INSERT|UPDATE|DELETE|MERGE|DECLARE|BEGIN)\b", re.I)

    def flush():
        if current_key and current_sql:
            value = "\n".join(current_sql)
            if len(value) > len(result.get(current_key, "")):
                result[current_key] = value

    def page_header(line):
        stripped = line.strip()
        return (not stripped or stripped.startswith((
            "SQL ordered by ", "-> ", "------", "CPU ", "Time (s)",
            "Elapsed", "Elap per", "Buffer Gets", "Physical Rds",
            "Executions", "Parse Calls", "Max", "Cluster", "Memory (KB)",
            "Version", "%Total", "% Total", "Sharable", "CPU per", "Old",
        )) or "Hash Value" in stripped or "DB/Inst:" in stripped or "Snaps:" in stripped)

    for line in lines:
        stripped = line.strip()
        if page_header(line):
            continue
        fields = stripped.split()
        if len(fields) >= 7:
            candidate = fields[-1]
            first_is_number = bool(re.fullmatch(r"[\d,.]+", fields[0]))
            if 5 <= len(candidate) <= 20 and candidate.isalnum() and first_is_number:
                flush()
                current_key = normalize_sql_id(candidate)
                current_sql = []
                collecting = False
                continue
        if stripped.startswith("Module:"):
            continue
        if current_key and not collecting and sql_start.match(stripped):
            collecting = True
            current_sql = []
        if collecting and stripped:
            current_sql.append(stripped)
    flush()
    return result


def parse_text_initialization_parameters(lines):
    """Join wrapped begin/end values from the fixed-width parameter table."""
    result = {}
    current_name = None
    begin_value = ""
    end_value = ""
    has_end_value = False

    def append_wrapped(value, continuation):
        continuation = continuation.strip()
        if not continuation:
            return value
        no_space = (value and value[-1] in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,./:+-_()"
                    and continuation[0] in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,./:+-_()")
        return value + ("" if no_space else " ") + continuation

    def finish():
        if current_name is not None:
            result.setdefault(current_name, end_value.strip() if has_end_value and end_value.strip()
                              else begin_value.strip())

    for raw_line in lines:
        line = raw_line.rstrip()
        if (not line or line.startswith("Parameter Name") or line.strip().startswith("----")
                or "init.ora Parameters" in line or "End value" in line):
            continue
        continuation = not raw_line or raw_line[0].isspace()
        if not continuation:
            finish()
            current_name = line[:29].strip()
            begin_value = line[30:63].strip()
            raw_end = line[64:].strip() if len(line) > 64 else ""
            end_value = ""
            has_end_value = False
            if raw_end:
                if len(line) > 63:
                    begin_value = append_wrapped(begin_value, raw_end)
                else:
                    end_value = raw_end
                    has_end_value = True
        elif current_name is not None:
            begin_part = raw_line[30:63].strip() if len(raw_line) > 30 else ""
            end_part = raw_line[64:].strip() if len(raw_line) > 64 else ""
            if has_end_value:
                end_value = append_wrapped(end_value, end_part or begin_part)
            else:
                begin_value = append_wrapped(begin_value, begin_part)
                begin_value = append_wrapped(begin_value, end_part)
    finish()
    return result


def parse_text_db_instance(lines):
    """Read database, host and block-size metadata from a STATSPACK header."""
    result = default_db_instance()
    database_line = ""
    host_line = ""
    for idx, line in enumerate(lines):
        if "Database" in line and "DB Id" in line:
            database_line = next((candidate for candidate in lines[idx + 1:]
                                  if re.match(r"\s*\d+\s+\S+\s+\d+\s+", candidate)), "")
        if (line.lstrip().startswith("Host") and "Platform" in line) or line.strip() == "Host":
            host_line = next((candidate for candidate in lines[idx + 1:]
                              if re.search(r"\s\d+\s+\d+\s+\d+\s+[\d.]+\s*$", candidate)), "")
        if line.startswith("db_block_size"):
            result["db_block_size"] = parse_int(line.split()[-1], 8192)

    db_values = database_line.split()
    if len(db_values) >= 7:
        result.update({
            "db_id": parse_int(db_values[0]),
            "instance_num": parse_int(db_values[2]),
            "startup_time": "{} {}".format(db_values[3], db_values[4]),
            "release": db_values[5],
            "rac": db_values[6],
        })
    host_values = host_line.split()
    if len(host_values) >= 8:
        result.update({
            "platform": " ".join(host_values[1:4]),
            "cpus": parse_int(host_values[4]),
            "cores": parse_int(host_values[5]),
            "sockets": parse_int(host_values[6]),
            "memory": int(round(parse_float(host_values[7]))),
        })
    if not result["db_block_size"]:
        result["db_block_size"] = 8192
    return result


def parse_text_report(path, security_level):
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    awr = default_awr(path.name)
    awr["snap_info"] = parse_text_snap_info(path, lines)
    awr["load_profile"] = parse_text_load_profile(find_text_section(lines, "Load Profile", ["Instance Efficiency", "Instance Efficiency Percentages"]))
    awr["instance_efficiency"] = parse_text_instance_efficiency(lines)
    awr["redo_log"] = parse_text_redo_log(lines)
    awr["host_cpu"] = parse_text_host_cpu(lines)
    awr["time_model_stats"] = parse_text_time_model(
        find_text_section(lines, "Time Model", ["Foreground Wait Events"]))

    foreground = find_text_section(lines, "Foreground Wait Events", ["Background Wait Events"])
    background = find_text_section(lines, "Background Wait Events", ["Wait Events (fg and bg)", "SQL ordered by"])
    awr["foreground_wait_events"] = parse_text_wait_events(foreground)
    awr["background_wait_events"] = parse_text_wait_events(background)

    elapsed_sql = find_text_section(lines, "SQL ordered by Elapsed", ["SQL ordered by Gets", "SQL ordered by CPU", "SQL ordered by Reads"])
    cpu_sql = find_text_section(lines, "SQL ordered by CPU", ["SQL ordered by Elapsed", "SQL ordered by Gets"])
    gets_sql = find_text_section(lines, "SQL ordered by Gets", ["SQL ordered by Reads", "SQL ordered by Executions"])
    reads_sql = find_text_section(lines, "SQL ordered by Reads", ["SQL ordered by Executions", "SQL ordered by Parse"])
    awr["sql_elapsed_time"] = parse_text_sql_section(elapsed_sql, "elapsed")
    awr["sql_cpu_time"] = parse_text_sql_section(cpu_sql, "cpu")
    awr["sql_gets"] = parse_text_sql_section(gets_sql, "gets")
    awr["sql_reads"] = parse_text_sql_section(reads_sql, "reads")

    instance_lines = find_text_section(
        lines, "Instance Activity Stats", ["workarea executions - optimal"])
    final_instance_line = next(
        (line for line in lines if line[:35].strip() == "workarea executions - optimal"), None)
    if final_instance_line:
        # awr.rs includes this boundary row because it is also a real counter.
        instance_lines.append(final_instance_line)
    awr["instance_stats"] = parse_text_instance_stats(instance_lines)
    awr["io_stats_byfunc"] = parse_text_io_stats(
        find_text_section(lines, "IO Stat by Function - summary", ["IO Stat by Function - detail"]))
    awr["dictionary_cache"] = parse_text_dictionary_cache(
        find_text_section(lines, "Dictionary Cache Stats", ["Library Cache Activity"]))
    awr["library_cache"] = parse_text_library_cache(
        find_text_section(lines, "Library Cache Activity", ["Rule Sets", "Rule Set", "Shared Pool Advisory", "Latch Activity"]))
    awr["latch_activity"] = parse_text_latch_activity(
        find_text_section(lines, "Latch Activity", ["Latch Sleep breakdown"]))

    histogram_lines = find_text_section(lines, "Wait Event Histogram", ["SQL ordered by"])
    histogram = parse_text_wait_histogram(
        histogram_lines,
        [event["event"] for event in awr["foreground_wait_events"] + awr["background_wait_events"]],
    )
    apply_wait_histogram(awr, histogram)

    parameter_lines = find_text_section(lines, "init.ora Parameters", ["End of Report"])
    parameters = parse_text_initialization_parameters(parameter_lines)
    sql_text = parse_text_sql_text(cpu_sql + elapsed_sql + gets_sql + reads_sql) if security_level >= 2 else {}
    db_instance = parse_text_db_instance(lines)
    mark_data_availability(awr)
    return awr, sql_text, parameters, db_instance


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
    # Keep provenance first for readers; existing JSON consumers ignore this field.
    collector_info = collector_identity()
    collector_info.update({
        "parser": "python-collector",
        "parsed_at_utc": datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z"),
    })
    collection = {
        "collector_info": collector_info,
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
            # Old JSON remains readable, but malformed parser artifacts are never selected.
            if not is_oracle_sql_id(sql_id):
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


def execution_plan_cursors_sql(sql_ids):
    literals = ", ".join("'{}'".format(sql_literal(sql_id)) for sql_id in sql_ids)
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 0 linesize 32767 trimspool on trimout on tab off
with ranked_cursors as (
    select lower(sql_id) as sql_id,
           child_number,
           count(*) over (partition by sql_id) as child_count,
           row_number() over (
               partition by sql_id
               order by case when is_shareable = 'Y' then 0 else 1 end,
                        last_active_time desc nulls last,
                        executions desc nulls last,
                        child_number desc
           ) as cursor_rank
      from v$sql
     where sql_id in ({sql_ids})
)
select sql_id || '|' || child_number || '|' || child_count
  from ranked_cursors
 where cursor_rank = 1
 order by sql_id;
exit
""".format(sql_ids=literals)


def discover_execution_plan_cursors(ctx, sql_ids, timeout):
    if not sql_ids:
        return []
    output = run_sqlplus(
        ctx,
        execution_plan_cursors_sql(sql_ids),
        timeout=timeout,
    )
    rows = []
    for sql_id, child_number, child_count in parse_delimited_rows(output, 3):
        try:
            rows.append(
                {
                    "sql_id": sql_id.lower(),
                    "child_number": int(child_number),
                    "child_count": int(child_count),
                }
            )
        except ValueError:
            continue
    return rows


def xplan_sql(sql_id, filename, child_number=0):
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 50000 linesize 32767 trimspool on trimout on tab off
set long 100000000 longchunksize 10000000
set termout off
spool {filename}
select * from table(dbms_xplan.display_cursor('{sql_id}',{child_number},'TYPICAL'));
spool off
set termout on
exit
""".format(
        filename=filename,
        sql_id=sql_literal(sql_id),
        child_number=int(child_number),
    )


def multi_child_cursor_sql(sql_ids):
    literals = ", ".join("'{}'".format(sql_literal(sql_id)) for sql_id in sql_ids)
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 0 linesize 32767 trimspool on trimout on tab off
select lower(sql_id) || '|' || count(distinct child_number)
  from v$sql
 where sql_id in ({sql_ids})
 group by sql_id
having count(distinct child_number) > 1
 order by sql_id;
exit
""".format(sql_ids=literals)


def discover_multi_child_cursor_sqls(ctx, sql_ids):
    if not sql_ids:
        return []

    output = run_sqlplus(ctx, multi_child_cursor_sql(sql_ids))
    rows = []
    for sql_id, child_count in parse_delimited_rows(output, 2):
        sql_id = normalize_sql_id(sql_id)
        if not is_valid_sql_id(sql_id):
            continue
        try:
            count = int(child_count)
        except ValueError:
            continue
        if count > 1:
            rows.append({"sql_id": sql_id, "child_count": count})
    rows.sort(key=lambda item: item["sql_id"])
    return rows


SHARED_CURSOR_REASON_CATALOG = {
    1: ("Unbound cursor (not fully parsed)", "The candidate cursor was not fully parsed and is not shareable."),
    2: ("SQL type mismatch", "The statement or cursor SQL type differs."),
    3: ("Optimizer mismatch", "The optimizer environment snapshots differ; fields identify the differing attributes."),
    4: ("SQL Tune Base Object Different", "The SQL Tuning Base object context differs."),
    5: ("Max Long Length Different", "The maximum LONG value length used by the cursor differs."),
    6: ("error_on_overlap_time parameter mismatch", "The ERROR_ON_OVERLAP_TIME session setting differs."),
    7: ("Top Level RPI Cursor", "The top-level recursive program interface cursor context differs."),
    8: ("Flashback Archive mismatch", "The Flashback Data Archive context differs."),
    9: ("PQ Slave mismatch", "The parallel-query slave compilation or execution context differs."),
    10: ("Top-level DDL", "The top-level DDL cursor context differs."),
    11: ("Multi-PX and slave-compiled cursor", "The multi-PX or slave-compiled cursor context differs."),
    12: ("Bind-peeked PQ cursor", "The bind-peeking context used for parallel query differs."),
    13: ("ANYDATA transformation", "The ANYDATA transformation context differs."),
    14: ("Stored outline mismatch", "The stored-outline context differs."),
    15: ("LogMiner attributes mismatch", "The LogMiner session or statement attributes differ."),
    16: ("Statistics row-source mismatch", "The statistics row-source context differs."),
    17: ("Literal replacement settings mismatch", "Cursor-sharing literal-replacement settings differ."),
    18: ("Literal replacement compilation", "The literal-replacement compilation context differs."),
    19: ("SQL Analyze", "The SQL Analyze cursor context differs."),
    20: ("Explain Plan cursor", "One cursor was compiled for EXPLAIN PLAN or its explain context differs."),
    21: ("Flashback cursor", "The flashback-query cursor context differs."),
    22: ("Buffered DML mismatch", "The buffered-DML context differs."),
    23: ("No Trigger Indicated mismatch", "The no-trigger indicator differs."),
    24: ("Parallel DML environment mismatch", "The parallel-DML environment differs."),
    25: ("Insert Direct Load mismatch", "The insert direct-load context differs."),
    26: ("Logical Standby Apply", "The logical-standby apply context differs."),
    27: ("Not Typechecked", "The candidate cursor has not completed compatible type checking."),
    28: ("Different Call Duration", "The call-duration cursor attribute differs."),
    29: ("Bind UACs mismatch", "Internal bind user-argument descriptors differ."),
    30: ("User Bind Peek settings mismatch", "User bind-peeking settings differ."),
    31: ("PL/SQL Compiler Switches", "PL/SQL compiler settings differ."),
    32: ("Materialized View Rewrite cursor", "The materialized-view rewrite context differs."),
    33: ("Rolling Invalidate Window Exceeded", "The rolling invalidation window was exceeded."),
    34: ("Editions mismatch", "Edition-based redefinition context differs."),
    35: ("Incarnation number mismatch", "An object or cursor incarnation number differs."),
    36: ("Authorization Check failed", "Authorization objects, schemas, synonyms, or translation entries differ."),
    37: ("Describe Cursor mismatch", "The describe-cursor context differs."),
    38: ("ACL Check mismatch", "The access-control-list check context differs."),
    39: ("Bind mismatch", "Bind metadata differs; fields identify position, datatype, length, or descriptor flags."),
    40: ("Session Cached Cursor", "The session-cached-cursor context differs."),
    41: ("Marked for Purge", "The cursor was marked unsafe or selected for purge."),
    42: ("Code Address Relocation", "A code-address relocation attribute differs."),
    43: ("Parallel DDL environment mismatch", "The parallel-DDL environment differs."),
    44: ("NLS Settings", "NLS environment snapshots differ; decoded text can be equal when raw handle bytes differ."),
    45: ("XDS Privilege Check mismatch", "The XDS privilege-check context differs."),
    46: ("Session Specific Cursor Session Mismatch", "A cursor restricted to a session was compared from a different session context."),
    47: ("Remote PDB ID Mismatch", "The remote pluggable-database identifier differs."),
    48: ("Auto Reoptimization Mismatch", "Automatic reoptimization or feedback state differs."),
    49: ("Show Invisible Columns Session Mismatch", "The session setting controlling invisible-column visibility differs."),
    50: ("Target CON_ID mismatch for CONTAINERS()", "The target container identifier for a CONTAINERS() cursor differs."),
    51: ("Permanent X$ attributes mismatch", "Attributes of an internal permanent X$ object differ."),
    52: ("Preplugin backup X$ attributes mismatch", "Attributes of an internal preplugin-backup X$ object differ."),
    53: ("COMMON_SCHEMA_ACCESS lockdown mismatch", "The COMMON_SCHEMA_ACCESS lockdown context differs."),
    54: ("EXEMPT REDACTION POLICY mismatch", "The EXEMPT REDACTION POLICY privilege context differs."),
    55: ("ADG redirected-statement sharing check", "The sharing context for a statement redirected from Active Data Guard differs."),
    56: ("Statistics Query Transformation sharing check", "The sharing context for Statistics Query Transformation differs."),
    57: ("Cross-container object DOP mismatch", "The degree of parallelism for a cross-container object differs."),
}

SHARED_CURSOR_DATATYPES = {
    1: "VARCHAR2",
    2: "NUMBER",
    8: "LONG",
    9: "VARCHAR",
    12: "DATE",
    23: "RAW",
    24: "LONG RAW",
    69: "ROWID",
    96: "CHAR",
    100: "BINARY_FLOAT",
    101: "BINARY_DOUBLE",
    102: "CURSOR / REF CURSOR",
    104: "UROWID",
    112: "CLOB",
    113: "BLOB",
    114: "BFILE",
    180: "TIMESTAMP",
    181: "TIMESTAMP WITH TIME ZONE",
    182: "INTERVAL YEAR TO MONTH",
    183: "INTERVAL DAY TO SECOND",
    231: "TIMESTAMP WITH LOCAL TIME ZONE",
}

SHARED_CURSOR_TRANSPORT_BEGIN = "JASMIN_REASON_BEGIN|"
SHARED_CURSOR_TRANSPORT_DATA = "JASMIN_REASON_DATA|"
SHARED_CURSOR_TRANSPORT_END = "JASMIN_REASON_END|"


def shared_cursor_reasons_sql(sql_id):
    """Fetch REASON CLOBs as ordered UTF-8 hex chunks without SQL*Plus wrapping."""
    return r"""
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 0 linesize 32767
set trimspool on trimout on tab off recsep off
set serveroutput on size unlimited format wrapped

declare
  l_position  pls_integer;
  l_sequence  pls_integer;
  l_length    pls_integer;
  l_chunk     varchar2(2000);
begin
  for cursor_row in (
    select child_number, reason
      from v$sql_shared_cursor
     where sql_id = '__SQL_ID__'
       and reason is not null
       and dbms_lob.getlength(reason) > 0
     order by child_number
  ) loop
    l_position := 1;
    l_sequence := 0;
    l_length := dbms_lob.getlength(cursor_row.reason);
    dbms_output.put_line(
      'JASMIN_REASON_BEGIN|' || cursor_row.child_number || '|' || l_length
    );

    while l_position <= l_length loop
      l_chunk := dbms_lob.substr(cursor_row.reason, 500, l_position);
      if l_chunk is null then
        raise_application_error(-20001, 'Unexpected empty REASON CLOB chunk');
      end if;
      l_sequence := l_sequence + 1;
      dbms_output.put_line(
        'JASMIN_REASON_DATA|' || cursor_row.child_number || '|' ||
        l_sequence || '|' || rawtohex(
          utl_i18n.string_to_raw(l_chunk, 'AL32UTF8')
        )
      );
      l_position := l_position + length(l_chunk);
    end loop;

    dbms_output.put_line(
      'JASMIN_REASON_END|' || cursor_row.child_number || '|' || l_sequence
    );
  end loop;
end;
/
exit
""".replace("__SQL_ID__", sql_literal(sql_id))


def parse_shared_cursor_reason_transport(output):
    """Reassemble and validate the hex-framed CLOB records emitted by SQL*Plus."""
    records = []
    active = None

    for raw_line in output.splitlines():
        line = raw_line.strip()
        if line.startswith(SHARED_CURSOR_TRANSPORT_BEGIN):
            parts = line.split("|")
            if len(parts) != 3 or active is not None:
                raise CollectorError("Malformed child cursor reason BEGIN record")
            try:
                active = {
                    "view_child": int(parts[1]),
                    "declared_length": int(parts[2]),
                    "chunks": [],
                }
            except ValueError as exc:
                raise CollectorError("Invalid child cursor reason BEGIN record") from exc
            continue

        if line.startswith(SHARED_CURSOR_TRANSPORT_DATA):
            parts = line.split("|", 3)
            if len(parts) != 4 or active is None:
                raise CollectorError("Malformed child cursor reason DATA record")
            try:
                child_number = int(parts[1])
                sequence = int(parts[2])
                chunk = bytes.fromhex(parts[3])
            except ValueError as exc:
                raise CollectorError("Invalid child cursor reason DATA record") from exc
            if child_number != active["view_child"]:
                raise CollectorError("Child cursor number changed inside REASON transport")
            if sequence != len(active["chunks"]) + 1:
                raise CollectorError("Child cursor REASON chunks are out of sequence")
            active["chunks"].append(chunk)
            continue

        if line.startswith(SHARED_CURSOR_TRANSPORT_END):
            parts = line.split("|")
            if len(parts) != 3 or active is None:
                raise CollectorError("Malformed child cursor reason END record")
            try:
                child_number = int(parts[1])
                chunk_count = int(parts[2])
            except ValueError as exc:
                raise CollectorError("Invalid child cursor reason END record") from exc
            if child_number != active["view_child"]:
                raise CollectorError("Child cursor number changed at REASON transport end")
            if chunk_count != len(active["chunks"]):
                raise CollectorError("Child cursor REASON transport is incomplete")
            try:
                reason = b"".join(active["chunks"]).decode("utf-8")
            except UnicodeDecodeError as exc:
                raise CollectorError("Child cursor REASON is not valid UTF-8") from exc
            if len(reason) != active["declared_length"]:
                raise CollectorError("Child cursor REASON length does not match its CLOB length")
            records.append({"view_child": active["view_child"], "reason": reason})
            active = None

    if active is not None:
        raise CollectorError("Child cursor REASON transport ended before its END record")
    return records


def xml_element_text(element):
    """Match XMLTABLE string(.) while treating an empty Oracle string as null."""
    if element is None:
        return None
    value = "".join(element.itertext())
    return value if value else None


def parse_shared_cursor_reason_integer(value, label):
    """Keep malformed internal identifiers visible as a collection failure."""
    if value is None:
        return None
    try:
        return int(value)
    except ValueError as exc:
        raise CollectorError("Invalid {} in V$SQL_SHARED_CURSOR.REASON".format(label)) from exc


def shared_cursor_field_details(reason_id, field_name, raw_value):
    """Decode comparison pairs and explain Oracle's diagnostic payload fields."""
    if field_name is None:
        return {
            "value_format": "NO_FIELD_PAYLOAD",
            "value_a": None,
            "value_b": None,
            "meaning": "This reason has no field-level payload.",
        }

    value_format = "SCALAR"
    value_a = raw_value
    value_b = None
    arrow = raw_value.find("->") if raw_value is not None else -1

    if reason_id == 44 and arrow >= 0:
        value_format = "NLS_PAIR"
        value_a = raw_value[1:max(1, arrow - 1)]
        value_b = raw_value[arrow + 3:-1]
    elif reason_id == 3 and raw_value is not None and len(raw_value) == 42:
        value_format = "OPTIMIZER_PAIR"
        value_a = raw_value[1:21].rstrip()
        value_b = raw_value[22:42].rstrip()
    elif reason_id == 3:
        value_format = "OPTIMIZER_RAW"
    elif field_name.startswith("original_"):
        value_format = "ORIGINAL_SCALAR"
    elif field_name.startswith("new_") or field_name.startswith("upgradeable_new_"):
        value_format = "NEW_SCALAR"

    lower_name = field_name.lower()
    if reason_id == 44:
        meaning = "NLS environment setting; A/B are comparison-vector sides."
    elif reason_id == 3 or field_name.startswith("_"):
        meaning = "Optimizer environment attribute or hidden parameter; A/B are comparison-vector sides."
    elif lower_name in ("bind_position", "pos"):
        meaning = "Internal zero-based bind position."
    elif lower_name in ("dty", "oacdty", "original_oacdty", "new_oacdty"):
        meaning = "Oracle internal bind datatype code."
    elif "oacmxl" in lower_name:
        meaning = "Maximum bind value or buffer length."
    elif re.search(r"(flg|flags)[0-9_]*$", lower_name):
        meaning = "Oracle internal flag bit mask; preserve the raw value unless the target-build enum is known."
    elif lower_name.endswith("sig"):
        meaning = "Oracle internal signature or hash used by the sharing comparison."
    else:
        meaning = "Oracle criterion-specific diagnostic field; the raw value is authoritative."

    return {
        "value_format": value_format,
        "value_a": value_a,
        "value_b": value_b,
        "meaning": meaning,
    }


def shared_cursor_datatype(value):
    """Translate the datatype codes previously decoded by the Oracle CASE expression."""
    if value is None or not re.fullmatch(r"[0-9]+", value.strip()):
        return None
    code = int(value.strip())
    return SHARED_CURSOR_DATATYPES.get(code, "datatype code {}".format(code))


def parse_shared_cursor_reason_nodes(records):
    """Parse every ChildNode while preserving source order and repeated reasons."""
    nodes = []
    structural_names = {"ChildNumber", "ID", "reason", "size"}

    for record in records:
        try:
            root = ET.fromstring("<ReasonRoot>{}</ReasonRoot>".format(record["reason"]))
        except ET.ParseError as exc:
            raise CollectorError(
                "Malformed V$SQL_SHARED_CURSOR.REASON XML for child {}: {}".format(
                    record["view_child"], exc
                )
            ) from exc

        reason_elements = [element for element in root if element.tag == "ChildNode"]
        for node_no, element in enumerate(reason_elements, start=1):
            values = {child.tag: xml_element_text(child) for child in element}
            reason_id = parse_shared_cursor_reason_integer(values.get("ID"), "reason ID")
            xml_child = parse_shared_cursor_reason_integer(
                values.get("ChildNumber"), "XML child number"
            )
            reason_text = values.get("reason")
            detail_match = re.search(r"\(([0-9]+)\)$", reason_text or "")
            reason_detail_code = int(detail_match.group(1)) if detail_match else None
            reason_name = re.sub(r"\([0-9]+\)$", "", reason_text or "") or None
            catalog = SHARED_CURSOR_REASON_CATALOG.get(reason_id)

            fields = []
            for child in element:
                if child.tag in structural_names:
                    continue
                raw_value = xml_element_text(child)
                details = shared_cursor_field_details(reason_id, child.tag, raw_value)
                details.update({
                    "field_no": len(fields) + 1,
                    "field_name": child.tag,
                    "raw_value": raw_value,
                })
                fields.append(details)

            nodes.append({
                "view_child": record["view_child"],
                "node_no": node_no,
                "xml_child": xml_child,
                "reason_id": reason_id,
                "reason_name": reason_name or (catalog[0] if catalog else None),
                "reason_detail_code": reason_detail_code,
                "payload_shape": values.get("size"),
                "reason_meaning": catalog[1] if catalog else (
                    "Unknown or release-specific sharing criterion; inspect the raw field values."
                ),
                "fields": fields,
            })
    return nodes


def format_shared_cursor_field(field):
    """Render one payload field using the collector's established labels."""
    value_a = field["value_a"]
    value_b = field["value_b"]
    value_format = field["value_format"]
    value_a_decoded = shared_cursor_datatype(value_a) if field["field_name"] and field["field_name"].lower() in (
        "dty", "oacdty", "original_oacdty", "new_oacdty"
    ) else None

    if value_format in ("NLS_PAIR", "OPTIMIZER_PAIR"):
        rendered = "A=[{}] | B=[{}]".format(
            value_a if value_a is not None else "<null>",
            value_b if value_b is not None else "<null>",
        )
    elif value_format == "OPTIMIZER_RAW":
        rendered = "[raw; pair not safely separable] = {}".format(field["raw_value"])
    elif value_format == "ORIGINAL_SCALAR":
        rendered = "[original] = {}".format(value_a if value_a is not None else "<null>")
        if value_a_decoded is not None:
            rendered += " ({})".format(value_a_decoded)
    elif value_format == "NEW_SCALAR":
        rendered = "[new] = {}".format(value_a if value_a is not None else "<null>")
        if value_a_decoded is not None:
            rendered += " ({})".format(value_a_decoded)
    elif value_format == "NO_FIELD_PAYLOAD":
        rendered = "<no field-level payload>"
    else:
        rendered = "= {}".format(value_a if value_a is not None else "<null>")
        if value_a_decoded is not None:
            rendered += " ({})".format(value_a_decoded)
    return rendered


def format_shared_cursor_reasons(sql_id, records):
    """Produce the same human-readable attachment previously formatted in SQL."""
    nodes = parse_shared_cursor_reason_nodes(records)
    children = sorted({node["view_child"] for node in nodes})
    lines = [
        "V$SQL_SHARED_CURSOR.REASON",
        "SQL_ID: {}".format(sql_id),
        "A/B denote comparison-vector sides, never chronological old/new values.",
    ]

    for view_child in children:
        lines.extend(["", "=" * 100, "CHILD CURSOR {}".format(view_child), "=" * 100])
        child_nodes = [node for node in nodes if node["view_child"] == view_child]
        for node in child_nodes:
            reason_name = node["reason_name"] or ""
            subcode = node["reason_detail_code"]
            payload_shape = node["payload_shape"] or "?"
            header = "+-- [{:02d}] {}  {{ID={}, subcode={}, payload={}}}".format(
                node["node_no"],
                reason_name,
                node["reason_id"] if node["reason_id"] is not None else "",
                subcode if subcode is not None else "?",
                payload_shape,
            )
            if node["xml_child"] is not None and node["xml_child"] != view_child:
                header += "  [XML child={}]".format(node["xml_child"])
            lines.extend(["", header, "|   Why: {}".format(node["reason_meaning"])])

            fields = node["fields"] or [{
                "field_no": 0,
                "field_name": None,
                "raw_value": None,
                **shared_cursor_field_details(node["reason_id"], None, None),
            }]
            for field in fields:
                lines.append(
                    "|   {:02d}. {}  {}".format(
                        field["field_no"],
                        field["field_name"] or "<no field payload>",
                        format_shared_cursor_field(field),
                    )
                )
                lines.append("|       Meaning: {}".format(field["meaning"]))

    field_count = sum(len(node["fields"]) for node in nodes)
    lines.extend([
        "",
        "-" * 100,
        "SUMMARY: {} child cursor(s), {} reason node(s), {} diagnostic field(s).".format(
            len(children), len(nodes), field_count
        ),
    ])
    return "\n".join(lines)


def collect_shared_cursor_reasons(ctx, target_dir, multi_child_sqls):
    target_dir.mkdir(parents=True, exist_ok=True)
    generated = []
    failures = []

    for idx, item in enumerate(multi_child_sqls, start=1):
        sql_id = item["sql_id"]
        filename = "{}{}".format(sql_id, SHARED_CURSOR_REASON_SUFFIX)
        target = target_dir / filename
        print(
            "Collecting child cursor reasons {}/{}: {}".format(
                idx, len(multi_child_sqls), filename
            )
        )
        try:
            output = run_sqlplus(
                ctx,
                shared_cursor_reasons_sql(sql_id),
            )
            records = parse_shared_cursor_reason_transport(output)
            if not records:
                raise CollectorError(
                    "V$SQL_SHARED_CURSOR returned no decoded reasons for {}".format(sql_id)
                )
            rendered = format_shared_cursor_reasons(sql_id, records)
            target.write_text(rendered.rstrip() + "\n", encoding="utf-8")
            ensure_generated(target)
            generated.append(target)
        except (CollectorError, OSError) as exc:
            failures.append((sql_id, str(exc)))
            print(
                "WARNING: Could not collect child cursor reasons for {}: {}".format(
                    sql_id, exc
                )
            )

    return generated, failures


def collect_sql_execution_plans(
    ctx, target_dir, sql_ids, selected_cursors=None,
    timeout=DEFAULT_XPLAN_TIMEOUT_SECONDS,
):
    target_dir.mkdir(parents=True, exist_ok=True)
    generated = []
    failures = []
    selected_cursors = selected_cursors or {}

    for idx, sql_id in enumerate(sql_ids, start=1):
        filename = "{}.xplan".format(sql_id)
        target = target_dir / filename
        child_number = selected_cursors.get(sql_id)
        if child_number is None:
            message = "No current child cursor found in V$SQL"
            failures.append((sql_id, message))
            print(
                "WARNING: Could not collect execution plan for {}: {}".format(
                    sql_id, message
                )
            )
            continue
        print(
            "Collecting execution plan {}/{}: {} (child {})".format(
                idx, len(sql_ids), filename, child_number
            )
        )
        try:
            run_sqlplus(
                ctx,
                xplan_sql(sql_id, filename, child_number),
                cwd=target_dir,
                timeout=timeout,
            )
            ensure_generated(target)
            generated.append(target)
        except (CollectorError, OSError) as exc:
            try:
                if target.exists():
                    target.unlink()
            except OSError as cleanup_exc:
                exc = CollectorError(
                    "{}; could not remove partial file: {}".format(exc, cleanup_exc)
                )
            failures.append((sql_id, str(exc)))
            print("WARNING: Could not collect execution plan for {}: {}".format(sql_id, exc))

    return generated, failures


def awr_pairs_sql(start_dt, end_dt, startup_time=None):
    """Pair consecutive AWR snapshots inside the selected instance startup."""
    start_value = start_dt.strftime(SNAPSHOT_DATE_FORMAT)
    end_value = end_dt.strftime(SNAPSHOT_DATE_FORMAT)
    # Pin generation to the reviewed startup so report pairs cannot cross a restart.
    startup_filter = ""
    if startup_time is not None:
        startup_filter = "and s.startup_time = to_timestamp('{}', 'YYYY-MM-DD HH24:MI:SS')".format(
            startup_time.strftime(SNAPSHOT_DATE_FORMAT)
        )
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 0 trimspool on linesize 32767 tab off
with snapshots as (
    select s.dbid,
           s.instance_number,
           s.snap_id as begin_snap,
           s.end_interval_time as begin_time,
           lead(s.snap_id, 1, null) over (
               partition by s.dbid, s.instance_number, s.startup_time
               order by s.snap_id
           ) as end_snap,
           lead(s.end_interval_time, 1, null) over (
               partition by s.dbid, s.instance_number, s.startup_time
               order by s.snap_id
           ) as end_time
      from dba_hist_snapshot s
     where s.dbid = (select dbid from v$database)
       and s.instance_number = (select instance_number from v$instance)
       and s.end_interval_time >= to_timestamp('{start_value}', 'YYYY-MM-DD HH24:MI:SS')
       and s.end_interval_time <= to_timestamp('{end_value}', 'YYYY-MM-DD HH24:MI:SS')
       {startup_filter}
)
select dbid || '|' || instance_number || '|' || begin_snap || '|' || end_snap
  from snapshots
 where end_snap is not null
   and end_time > begin_time
 order by instance_number, begin_snap;
exit
""".format(start_value=start_value, end_value=end_value, startup_filter=startup_filter)


def awr_startup_periods_sql(start_dt, end_dt):
    """List AWR startup periods represented by snapshots in the requested range."""
    # Use snapshot end times because those are the boundaries accepted by AWR pairing.
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 0 trimspool on linesize 32767 tab off
with period_snaps as (
    select startup_time, end_interval_time as snap_time,
           lead(end_interval_time) over (
               partition by dbid, instance_number, startup_time
               order by snap_id
           ) as next_time
      from dba_hist_snapshot
     where dbid = (select dbid from v$database)
       and instance_number = (select instance_number from v$instance)
       and end_interval_time >= to_timestamp('{start}', 'YYYY-MM-DD HH24:MI:SS')
       and end_interval_time <= to_timestamp('{end}', 'YYYY-MM-DD HH24:MI:SS')
)
select nvl(to_char(startup_time, 'YYYY-MM-DD HH24:MI:SS'), 'UNKNOWN') || '|' ||
       to_char(min(snap_time), 'YYYY-MM-DD HH24:MI:SS') || '|' ||
       to_char(max(snap_time), 'YYYY-MM-DD HH24:MI:SS') || '|' ||
       count(*) || '|' ||
       sum(case when next_time > snap_time then 1 else 0 end)
  from period_snaps
 group by startup_time
 order by startup_time;
exit
""".format(start=start_dt.strftime(SNAPSHOT_DATE_FORMAT), end=end_dt.strftime(SNAPSHOT_DATE_FORMAT))


def statspack_pairs_sql(start_dt, end_dt, startup_time=None):
    """Pair consecutive snapshots inside each startup, including manual captures."""
    start_value = start_dt.strftime(SNAPSHOT_DATE_FORMAT)
    end_value = end_dt.strftime(SNAPSHOT_DATE_FORMAT)
    # Pin generation to the selected startup even if new snapshots arrive after discovery.
    startup_filter = ""
    if startup_time is not None:
        startup_filter = "and startup_time = to_date('{}', 'YYYY-MM-DD HH24:MI:SS')".format(
            startup_time.strftime(SNAPSHOT_DATE_FORMAT)
        )
    # Partition before pairing so a requested period can safely include restarts.
    # Compare timestamps directly: scheduled intervals may differ by a second.
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 0 trimspool on linesize 32767 tab off
with v_snaps as (
    select snap_id as begin_snap,
           snap_time as begin_time,
           lead(snap_id, 1, null) over (
               partition by dbid, instance_number, startup_time
               order by snap_id
           ) as end_snap,
           lead(snap_time, 1, null) over (
               partition by dbid, instance_number, startup_time
               order by snap_id
           ) as end_time
      from perfstat.STATS$SNAPSHOT
     where dbid = (select dbid from v$database)
       and instance_number = (select instance_number from v$instance)
       and snap_time >= to_date('{start_value}', 'YYYY-MM-DD HH24:MI:SS')
       and snap_time <= to_date('{end_value}', 'YYYY-MM-DD HH24:MI:SS')
       {startup_filter}
)
select begin_snap || '|' || end_snap
  from v_snaps
 where end_snap is not null
   and end_time > begin_time
 order by begin_snap;
exit
""".format(
        start_value=start_value,
        end_value=end_value,
        startup_filter=startup_filter,
    )


def statspack_startup_periods_sql(start_dt, end_dt):
    """List observed startups, including groups too small to produce a report."""
    # Count valid adjacent pairs so the menu never offers an unusable period.
    return """
whenever oserror exit failure
whenever sqlerror exit failure
set heading off feedback off verify off echo off pagesize 0 trimspool on linesize 32767 tab off
with period_snaps as (
    select startup_time, snap_time,
           lead(snap_time) over (
               partition by dbid, instance_number, startup_time
               order by snap_id
           ) as next_time
      from perfstat.STATS$SNAPSHOT
     where dbid = (select dbid from v$database)
       and instance_number = (select instance_number from v$instance)
       and snap_time >= to_date('{start}', 'YYYY-MM-DD HH24:MI:SS')
       and snap_time <= to_date('{end}', 'YYYY-MM-DD HH24:MI:SS')
)
select nvl(to_char(startup_time, 'YYYY-MM-DD HH24:MI:SS'), 'UNKNOWN') || '|' ||
       to_char(min(snap_time), 'YYYY-MM-DD HH24:MI:SS') || '|' ||
       to_char(max(snap_time), 'YYYY-MM-DD HH24:MI:SS') || '|' ||
       count(*) || '|' ||
       sum(case when next_time > snap_time then 1 else 0 end)
  from period_snaps
 group by startup_time
 order by startup_time;
exit
""".format(start=start_dt.strftime(SNAPSHOT_DATE_FORMAT), end=end_dt.strftime(SNAPSHOT_DATE_FORMAT))


def parse_startup_periods(output, report_type):
    """Decode the common period inventory returned by AWR and STATSPACK queries."""
    periods = []
    # Reject malformed identity data instead of silently collecting an ambiguous group.
    for startup, first, last, count, pairs in parse_delimited_rows(output, 5):
        try:
            period = {
                "startup_time": datetime.strptime(startup, SNAPSHOT_DATE_FORMAT),
                "start": datetime.strptime(first, SNAPSHOT_DATE_FORMAT),
                "end": datetime.strptime(last, SNAPSHOT_DATE_FORMAT),
                "snapshot_count": int(count),
                "pair_count": int(pairs),
            }
        except ValueError as exc:
            raise CollectorError("Could not read {} startup period: {}".format(report_type, exc))
        periods.append(period)
    return periods


def discover_awr_startup_periods(ctx, start_dt, end_dt):
    """Read AWR periods before creating output files or generating reports."""
    return parse_startup_periods(run_sqlplus(ctx, awr_startup_periods_sql(start_dt, end_dt)), "AWR")


def discover_statspack_startup_periods(ctx, start_dt, end_dt):
    """Read STATSPACK periods before creating output files or patching templates."""
    return parse_startup_periods(run_sqlplus(ctx, statspack_startup_periods_sql(start_dt, end_dt)), "STATSPACK")


def startup_selection_can_prompt(args):
    """Respect unattended runs, including a fully specified command in a terminal."""
    # Missing collection choices indicate interactive or mixed use; redirected input never prompts.
    json_needed = (args.package_mode != PACKAGE_REPORTS or args.include_sql_plans
                   or args.access_path_evidence is not None)
    choices = (args.report_type, args.start_dt, args.end_dt, args.include_alert,
               args.include_sql_plans, args.include_os_stats, args.package_mode)
    return sys.stdin.isatty() and (
        any(value is None for value in choices)
        or (json_needed and args.security_level is None)
    )


def select_startup_period(periods, allow_prompt, report_type):
    """Require one observed startup per package, with stable numbered choices."""
    if not periods:
        raise CollectorError("No {} snapshots found for the requested date range.".format(report_type))

    # Single-startup ranges continue automatically, including historical startups.
    if len(periods) == 1:
        if periods[0]["pair_count"] == 0:
            raise CollectorError("No valid {} pairs in this startup; two snapshots with increasing timestamps are required.".format(report_type))
        return periods[0]

    print("INFO: The requested range contains snapshots from {} instance startups.".format(len(periods)))
    print("INFO: Mixing startup periods in one analysis can affect statistics, anomalies and findings.")
    print("INFO: We recommend a separate analysis for each startup. Select one period for this package.")
    print("INFO: Only startups recorded in snapshots are listed; restarts without snapshots cannot be detected.")
    print(" No.  Instance startup      First snapshot        Last snapshot         Snapshots  Reports")
    # Keep unavailable groups numbered too, so all observed restarts remain visible.
    for number, period in enumerate(periods, 1):
        print(" {:>3}  {}   {}   {}   {:>9}  {:>7}{}".format(
            number, period["startup_time"].strftime(SNAPSHOT_DATE_FORMAT),
            period["start"].strftime(SNAPSHOT_DATE_FORMAT), period["end"].strftime(SNAPSHOT_DATE_FORMAT),
            period["snapshot_count"], period["pair_count"],
            " (unavailable: no valid pairs)" if not period["pair_count"] else "",
        ))
        if period["pair_count"]:
            print('      Range {}: --start "{}" --end "{}"'.format(
                number, period["start"].strftime(SNAPSHOT_DATE_FORMAT),
                period["end"].strftime(SNAPSHOT_DATE_FORMAT),
            ))

    # No default is chosen: a batch run must be retried with one of the printed ranges.
    if not any(period["pair_count"] for period in periods):
        raise CollectorError("None of the startup periods contains a valid {} pair.".format(report_type))
    if not allow_prompt:
        raise CollectorError("Multiple {} startups require a selection. Rerun with START and END from one available numbered range above.".format(report_type))
    while True:
        try:
            choice = input("Choose startup period [1-{}]: ".format(len(periods))).strip()
        except EOFError:
            raise CollectorError("No startup period selected. Rerun with one of the available ranges above.")
        if not choice.isascii() or not choice.isdigit() or not 1 <= int(choice) <= len(periods):
            print("Please enter an available period number from 1 to {}.".format(len(periods)))
            continue
        selected = periods[int(choice) - 1]
        if not selected["pair_count"]:
            print("This period has no valid report pairs. Please choose another number.")
            continue
        return selected


def select_statspack_startup_period(periods, allow_prompt):
    """Keep the public helper explicit for existing callers and tests."""
    return select_startup_period(periods, allow_prompt, "STATSPACK")


def discover_awr_pairs(ctx, start_dt, end_dt, startup_time=None):
    output = run_sqlplus(ctx, awr_pairs_sql(start_dt, end_dt, startup_time))
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


def discover_statspack_pairs(ctx, start_dt, end_dt, startup_time=None):
    output = run_sqlplus(ctx, statspack_pairs_sql(start_dt, end_dt, startup_time))
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
    return attachments_dir(output_dir, json_path.stem)


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
    os_stats_info=None,
    startup_selection=None,
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
        packaged_files.extend(
            [
                relative_package_path(output_dir, reason_file)
                for reason_file in xplan_info.get("child_cursor_reason_files", [])
            ]
        )
    if os_stats_info:
        packaged_files.extend(
            [relative_package_path(output_dir, stats_file) for stats_file in os_stats_info.get("files", [])]
        )
    packaged_files.append(manifest.name)

    # Include identity even in reports-only packages, without exposing host paths.
    identity = collector_identity()
    with manifest.open("w", encoding="utf-8") as fh:
        fh.write("JAS-MIN collector manifest\n")
        fh.write("==========================\n")
        fh.write("collector_name={}\n".format(identity["name"]))
        fh.write("collector_version={}\n".format(identity["version"]))
        fh.write("collector_script_sha256={}\n".format(identity["script_sha256"]))
        fh.write("ORACLE_SID={}\n".format("masked by security level 0" if mask_manifest else ctx["oracle_sid"]))
        fh.write("ORACLE_HOME={}\n".format("masked by security level 0" if mask_manifest else ctx["oracle_home"]))
        fh.write("report_type={}\n".format(report_type))
        fh.write("start={}\n".format(datetime_sql(start_dt)))
        fh.write("end={}\n".format(datetime_sql(end_dt)))
        # Preserve both the original request and the exact chosen snapshot period.
        if startup_selection:
            for key, value in startup_selection.items():
                fh.write("{}={}\n".format(key, value.strftime(SNAPSHOT_DATE_FORMAT)))
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
        fh.write("\nOS statistics:\n")
        if os_stats_info and os_stats_info.get("requested"):
            source_dir = os_stats_info.get("source_dir")
            files = os_stats_info.get("files", [])
            fh.write(
                "  source_dir={}\n".format(
                    "masked by security level 0" if mask_manifest else source_dir
                )
            )
            fh.write("  platform={}\n".format(os_stats_info.get("platform", "")))
            if files:
                fh.write("  copied files:\n")
                for stats_file in files:
                    fh.write("    {}\n".format(relative_package_path(output_dir, stats_file)))
            else:
                fh.write("  copied files: none\n")
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
            fh.write(
                "  per-plan timeout: {} second(s)\n".format(
                    xplan_info.get(
                        "execution_plan_timeout", DEFAULT_XPLAN_TIMEOUT_SECONDS
                    )
                )
            )

            selected_child_cursors = xplan_info.get("selected_child_cursors", [])
            if selected_child_cursors:
                fh.write("  selected current child cursors:\n")
                for item in selected_child_cursors:
                    fh.write(
                        "    {sql_id} - child_number={child_number}, "
                        "child_count={child_count}\n".format(**item)
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

            fh.write("\nChild cursor sharing reasons for TOP SQL_IDs:\n")
            multi_child_sqls = xplan_info.get("multi_child_sqls", [])
            if multi_child_sqls:
                fh.write("  SQL_IDs with multiple current child cursors:\n")
                for item in multi_child_sqls:
                    fh.write(
                        "    {sql_id} - child_count={child_count}\n".format(**item)
                    )
            else:
                fh.write("  SQL_IDs with multiple current child cursors: none found\n")

            reason_files = xplan_info.get("child_cursor_reason_files", [])
            if reason_files:
                fh.write("  generated files:\n")
                for reason_file in reason_files:
                    fh.write(
                        "    {}\n".format(
                            relative_package_path(output_dir, reason_file)
                        )
                    )
            else:
                fh.write("  generated files: none\n")

            discovery_failure = xplan_info.get("child_cursor_discovery_failure")
            if discovery_failure:
                fh.write(
                    "  discovery failure: {}\n".format(
                        discovery_failure.replace("\n", " ")
                    )
                )
            reason_failures = xplan_info.get("child_cursor_reason_failures", [])
            if reason_failures:
                fh.write("  collection failures:\n")
                for sql_id, message in reason_failures:
                    fh.write("    {} - {}\n".format(sql_id, message.replace("\n", " ")))
        else:
            fh.write("  not requested\n")
        fh.write("\nPackaged files:\n")
        for packaged_file in packaged_files:
            fh.write("  {}\n".format(packaged_file))
    return manifest


def create_zip_package(
    output_dir,
    stem,
    reports,
    json_path,
    alert_info,
    xplan_info,
    manifest,
    package_mode,
    os_stats_info=None,
):
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
        files.extend(xplan_info.get("child_cursor_reason_files", []))
    if os_stats_info:
        files.extend(os_stats_info.get("files", []))
    files.append(manifest)

    with zipfile.ZipFile(str(zip_path), "w", zipfile.ZIP_DEFLATED) as archive:
        for file_path in files:
            archive.write(str(file_path), arcname=relative_package_path(output_dir, file_path))
    return zip_path


def main(argv=None):
    try:
        args = parse_collector_args(argv)
        # Make copied terminal logs identify the collector release too.
        print("{} {}".format(COLLECTOR_NAME, COLLECTOR_VERSION))
        ctx = require_oracle_context()
        print("Detected database from environment:")
        print("  ORACLE_SID={}".format(ctx["oracle_sid"]))
        print("  ORACLE_HOME={}".format(ctx["oracle_home"]))

        report_type = args.report_type if args.report_type is not None else ask_report_type()
        start_dt, end_dt = resolve_date_range(args.start_dt, args.end_dt)
        # Resolve startup ambiguity before any collection side effects or attachment prompts.
        startup_selection = None
        pairs = None
        requested_start, requested_end = start_dt, end_dt
        periods = (
            discover_awr_startup_periods(ctx, start_dt, end_dt)
            if report_type == "AWR"
            else discover_statspack_startup_periods(ctx, start_dt, end_dt)
        )
        selected = select_startup_period(
            periods, startup_selection_can_prompt(args), report_type
        )
        startup_selection = {
            "requested_start": requested_start,
            "requested_end": requested_end,
            "selected_startup": selected["startup_time"],
        }
        start_dt, end_dt = selected["start"], selected["end"]
        print("INFO: Selected startup {}. Report range: {} to {}.".format(
            selected["startup_time"].strftime(SNAPSHOT_DATE_FORMAT),
            start_dt.strftime(SNAPSHOT_DATE_FORMAT), end_dt.strftime(SNAPSHOT_DATE_FORMAT),
        ))
        if report_type == "AWR":
            pairs = discover_awr_pairs(ctx, start_dt, end_dt, selected["startup_time"])
        else:
            pairs = discover_statspack_pairs(ctx, start_dt, end_dt, selected["startup_time"])
        if not pairs:
            raise CollectorError(
                "No {} pairs remain in the selected startup. Snapshots may have been purged; rerun collection.".format(report_type)
            )
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
        os_stats_dir, os_stats_source_files = ask_os_stats_options(
            args.include_os_stats,
            args.os_stats_dir,
        )
        package_mode = args.package_mode if args.package_mode is not None else ask_package_mode()
        json_required = package_includes_json(package_mode) or include_sql_plans or args.access_path_evidence is not None
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
            reports = generate_awr_reports(ctx, pairs, reports_dir)
        else:
            reports = generate_statspack_reports(ctx, pairs, reports_dir)

        json_path = None
        if json_required:
            print("Parsing generated reports to JAS-MIN JSON...")
            json_path = parse_reports_to_json(reports, output_dir, stem, security_level)
            if args.access_path_evidence is not None:
                if security_level != 2:
                    raise CollectorError("--access-path-evidence requires --security-level 2 to preserve explicit evidence references")
                merge_access_path_evidence(json_path, args.access_path_evidence)
            print("JSON file: {}".format(json_path))

        xplan_info = {
            "requested": include_sql_plans,
            "top_sqls": [],
            "manual_sql_ids": manual_sql_ids,
            "sql_ids": [],
            "files": [],
            "failures": [],
            "selected_child_cursors": [],
            "execution_plan_timeout": args.execution_plan_timeout,
            "multi_child_sqls": [],
            "child_cursor_reason_files": [],
            "child_cursor_reason_failures": [],
            "child_cursor_discovery_failure": None,
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
                try:
                    cursor_rows = discover_execution_plan_cursors(
                        ctx, plan_sql_ids, args.execution_plan_timeout
                    )
                    xplan_info["selected_child_cursors"] = cursor_rows
                    selected_cursors = {
                        item["sql_id"]: item["child_number"]
                        for item in cursor_rows
                    }
                    top_sql_id_set = {item["sql_id"] for item in top_sqls}
                    multi_child_sqls = [
                        {
                            "sql_id": item["sql_id"],
                            "child_count": item["child_count"],
                        }
                        for item in cursor_rows
                        if item["sql_id"] in top_sql_id_set
                        and item["child_count"] > 1
                    ]
                    xplan_info["multi_child_sqls"] = multi_child_sqls
                except CollectorError as exc:
                    xplan_info["child_cursor_discovery_failure"] = str(exc)
                    selected_cursors = {sql_id: 0 for sql_id in plan_sql_ids}
                    multi_child_sqls = []
                    print(
                        "WARNING: Could not select current child cursors; "
                        "falling back to child 0: {}".format(exc)
                    )

                xplan_files, xplan_failures = collect_sql_execution_plans(
                    ctx,
                    xplan_target_dir,
                    plan_sql_ids,
                    selected_cursors,
                    args.execution_plan_timeout,
                )
                xplan_info["files"] = xplan_files
                xplan_info["failures"] = xplan_failures
                print("Execution plan attachment(s): {}".format(len(xplan_files)))

                if multi_child_sqls:
                    print("TOP SQL_IDs with multiple current child cursors:")
                    for item in multi_child_sqls:
                        print(
                            "  {sql_id} - child_count={child_count}".format(**item)
                        )
                    reason_files, reason_failures = collect_shared_cursor_reasons(
                        ctx, xplan_target_dir, multi_child_sqls
                    )
                    xplan_info["child_cursor_reason_files"] = reason_files
                    xplan_info["child_cursor_reason_failures"] = reason_failures
                    print(
                        "Child cursor reason attachment(s): {}".format(
                            len(reason_files)
                        )
                    )
                elif not xplan_info["child_cursor_discovery_failure"]:
                    print(
                        "No TOP SQL_IDs with multiple current child cursors "
                        "were found."
                    )
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

        os_stats_info = None
        if os_stats_dir is not None:
            print("Copying OS statistics...")
            os_stats_info = copy_os_stats(os_stats_dir, os_stats_source_files, output_dir, stem)
            print(
                "OS statistics attachment(s): {} in {}".format(
                    len(os_stats_info.get("files", [])),
                    os_stats_info.get("target_dir"),
                )
            )

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
            os_stats_info,
            startup_selection=startup_selection,
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
            os_stats_info,
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
