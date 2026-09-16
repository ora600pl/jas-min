"""Exercise the generated pairing query without connecting to a production DB.

SQLite executes the window/filter logic; its TO_DATE shim uses sortable ISO text.
These tests do not replace an Oracle integration check.
"""

import importlib.util
import contextlib
import io
import sqlite3
import tempfile
import unittest
from datetime import datetime
from pathlib import Path
from unittest import mock


# Load the standalone script without running its interactive entry point.
spec = importlib.util.spec_from_file_location(
    "collector_pairs", Path(__file__).resolve().parents[1] / "jas-min-collector.py"
)
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)


class StatspackPairTests(unittest.TestCase):
    def setUp(self):
        # Reproduce the source tables and local database/instance identity.
        self.db = sqlite3.connect(":memory:")
        self.addCleanup(self.db.close)
        self.db.executescript("""
            ATTACH DATABASE ':memory:' AS perfstat;
            CREATE TABLE perfstat.STATS$SNAPSHOT (
                dbid INTEGER, instance_number INTEGER, startup_time TEXT,
                snap_id INTEGER, snap_time TEXT
            );
            CREATE TABLE v$database (dbid INTEGER);
            INSERT INTO v$database VALUES (100);
            CREATE TABLE v$instance (instance_number INTEGER);
            INSERT INTO v$instance VALUES (1);
        """)
        self.db.create_function("to_date", 2, lambda value, fmt: value)
        self.db.create_function("to_char", 2, lambda value, fmt: value)
        self.db.create_function("nvl", 2, lambda value, fallback: value if value is not None else fallback)

    def add_snap(self, snap_id, time, startup="2026-09-09 00:00:00", dbid=100, instance=1):
        # Keep test fixtures explicit, including foreign instances and restarts.
        self.db.execute(
            "INSERT INTO perfstat.STATS$SNAPSHOT VALUES (?, ?, ?, ?, ?)",
            (dbid, instance, startup, snap_id, "2026-09-10 " + time),
        )

    def query_sqlplus(self, ctx, script):
        # Oracle formatting functions are shimmed above; pairing SQL is unchanged.
        query = script[script.index("with "):].split(";", 1)[0]
        return "\n".join(row[0] for row in self.db.execute(query))

    def periods(self):
        # Exercise grouping and result decoding together using realistic timestamps.
        with mock.patch.object(collector, "run_sqlplus", side_effect=self.query_sqlplus):
            return collector.discover_statspack_startup_periods(
                {}, datetime(2026, 9, 10, 13), datetime(2026, 9, 10, 18, 30)
            )

    def pairs(self, start="13:00", end="18:30", startup=None):
        # Execute the actual generated SQL after removing SQL*Plus commands.
        with mock.patch.object(collector, "run_sqlplus", side_effect=self.query_sqlplus):
            pairs = collector.discover_statspack_pairs(
                {}, datetime.fromisoformat("2026-09-10 " + start),
                datetime.fromisoformat("2026-09-10 " + end),
                startup,
            )
        return [(int(pair["begin_snap"]), int(pair["end_snap"])) for pair in pairs]

    def test_manual_snapshots_gaps_and_scheduler_seconds(self):
        # Reproduce the reported sequence: short manual captures, then 30-minute jobs.
        for snap_id, time in [
            (8, "13:10:15"), (9, "13:19:37"), (13, "14:00:00"),
            (14, "14:30:00"), (15, "15:00:00"), (16, "15:30:00"),
            (17, "16:00:01"), (18, "16:30:00"),
        ]:
            self.add_snap(snap_id, time)
        self.assertEqual(self.pairs(), [(8, 9), (9, 13), (13, 14), (14, 15),
                                        (15, 16), (16, 17), (17, 18)])

    def test_restarts_and_foreign_instances_stay_separate(self):
        # Include older startup data without pairing across a restart or another DB.
        self.add_snap(8, "13:10:00")
        self.add_snap(9, "13:20:00")
        self.add_snap(10, "13:30:00", dbid=200)
        self.add_snap(11, "13:40:00", instance=2)
        self.add_snap(13, "14:00:00", startup="2026-09-10 13:50:00")
        self.add_snap(14, "14:30:00", startup="2026-09-10 13:50:00")
        self.assertEqual(self.pairs(), [(8, 9), (13, 14)])

    def test_both_endpoints_must_be_inside_inclusive_range(self):
        # Exact bounds are included; snapshots even one second outside are excluded.
        for snap_id, time in [(7, "12:59:59"), (8, "13:00:00"),
                              (9, "13:30:00"), (13, "14:00:00"), (14, "14:00:01")]:
            self.add_snap(snap_id, time)
        self.assertEqual(self.pairs(end="14:00"), [(8, 9), (9, 13)])

    def test_equal_and_decreasing_times_are_not_bridged(self):
        # Reject invalid adjacent intervals without creating replacement long pairs.
        for snap_id, time in [(8, "13:10:00"), (9, "13:10:00"),
                              (13, "13:09:00"), (14, "13:30:00")]:
            self.add_snap(snap_id, time)
        self.assertEqual(self.pairs(), [(13, 14)])

    def test_empty_or_single_snapshot_has_no_pair(self):
        # A report always needs two usable endpoints.
        self.assertEqual(self.pairs(), [])
        self.add_snap(8, "13:10:00")
        self.assertEqual(self.pairs(), [])

    def add_two_periods(self):
        # The first period deliberately contains non-zero seconds and a missing ID.
        for snap_id, time in [(8, "13:10:15"), (9, "13:19:37"), (13, "14:00:01")]:
            self.add_snap(snap_id, time)
        for snap_id, time in [(14, "15:00:02"), (15, "15:30:03")]:
            self.add_snap(snap_id, time, startup="2026-09-10 14:45:12")

    def test_period_discovery_includes_singletons_and_excludes_other_instances(self):
        # A one-snapshot startup is still important context, even without reports.
        self.add_two_periods()
        self.add_snap(16, "16:30:00", startup="2026-09-10 16:00:00")
        self.add_snap(17, "17:30:00", startup="2026-09-10 17:00:00", instance=2)
        periods = self.periods()
        self.assertEqual([p["snapshot_count"] for p in periods], [3, 2, 1])
        self.assertEqual([p["pair_count"] for p in periods], [2, 1, 0])
        self.assertEqual(periods[0]["start"], datetime(2026, 9, 10, 13, 10, 15))

    def test_numbered_selection_reprompts_without_default(self):
        # Blank, non-numeric, out-of-range and unavailable choices must not collect data.
        self.add_two_periods()
        self.add_snap(16, "16:30:00", startup="2026-09-10 16:00:00")
        periods = self.periods()
        output = io.StringIO()
        with contextlib.redirect_stdout(output), mock.patch(
            "builtins.input", side_effect=["", "abc", "0", "4", "3", "2"]
        ) as prompt:
            selected = collector.select_statspack_startup_period(periods, True)
        self.assertEqual(selected, periods[1])
        self.assertEqual(prompt.call_count, 6)
        self.assertIn("Choose startup period [1-3]", prompt.call_args.args[0])
        self.assertIn("Range 1: --start", output.getvalue())
        self.assertIn("Range 2: --start", output.getvalue())
        self.assertIn("unavailable: no valid pairs", output.getvalue())
        self.assertIn("statistics, anomalies and findings", output.getvalue())

    def test_single_historical_period_continues_without_prompt(self):
        # Historical data is allowed when all chosen snapshots share one startup.
        self.add_snap(8, "13:10:15")
        self.add_snap(9, "13:19:37")
        periods = self.periods()
        with mock.patch("builtins.input", side_effect=AssertionError("unexpected prompt")):
            self.assertEqual(collector.select_statspack_startup_period(periods, False), periods[0])

    def test_empty_singleton_and_all_unavailable_periods_fail(self):
        # No report collection should start for a range with no usable pair.
        with self.assertRaises(collector.CollectorError):
            collector.select_statspack_startup_period([], False)
        self.add_snap(8, "13:10:15")
        with self.assertRaises(collector.CollectorError):
            collector.select_statspack_startup_period(self.periods(), False)
        self.add_snap(9, "14:10:15", startup="2026-09-10 14:00:00")
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(collector.CollectorError):
            collector.select_statspack_startup_period(self.periods(), True)

    def test_eof_during_selection_is_a_friendly_error(self):
        # Closing input must cancel rather than select a default or expose a traceback.
        self.add_two_periods()
        with contextlib.redirect_stdout(io.StringIO()), mock.patch("builtins.input", side_effect=EOFError):
            with self.assertRaisesRegex(collector.CollectorError, "No startup period selected"):
                collector.select_statspack_startup_period(self.periods(), True)

    def test_selected_startup_is_pinned_even_with_a_broad_range(self):
        # Generation filters identity as well as dates, so groups cannot be mixed.
        self.add_two_periods()
        self.assertEqual(self.pairs(startup=datetime(2026, 9, 10, 14, 45, 12)), [(14, 15)])
        self.assertEqual(self.pairs(start="13:10:15", end="14:00:01",
                                    startup=datetime(2026, 9, 9)), [(8, 9), (9, 13)])

    def full_cli(self):
        # Supplying every collection choice represents an unattended invocation.
        return ["--report-type", "statspack", "--start", "2026-09-10 13:00",
                "--end", "2026-09-10 18:30", "--no-alert-log", "--no-execution-plans",
                "--no-os-stats", "--package-content", "reports"]

    def test_unattended_main_exits_before_any_collection_side_effects(self):
        # Even with a terminal, fully specified CLI arguments must never prompt.
        self.add_two_periods()
        output, errors = io.StringIO(), io.StringIO()
        with mock.patch.object(collector, "run_sqlplus", side_effect=self.query_sqlplus), \
             mock.patch.object(collector, "require_oracle_context", return_value={"oracle_sid": "TEST", "oracle_home": "/oracle"}), \
             mock.patch.object(collector.sys.stdin, "isatty", return_value=True), \
             mock.patch.object(collector, "unique_output_dir") as mkdir, \
             mock.patch.object(collector, "generate_statspack_reports") as generate, \
             mock.patch("builtins.input", side_effect=AssertionError("unexpected prompt")), \
             contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
            status = collector.main(self.full_cli())
        self.assertEqual(status, 1)
        mkdir.assert_not_called()
        generate.assert_not_called()
        self.assertIn("Range 2:", output.getvalue())
        self.assertIn("Rerun with START and END", errors.getvalue())

    def test_interactive_main_collects_only_selected_period_and_records_it(self):
        # Run the orchestration through manifest/ZIP creation, replacing only DB/report I/O.
        self.add_two_periods()
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            def generate_reports(ctx, pairs, output):
                self.assertEqual(pairs, [{"begin_snap": "14", "end_snap": "15"}])
                report = output / "sp_14_15.txt"
                report.write_text("STATSPACK fixture\n", encoding="utf-8")
                return [report]

            with mock.patch.object(collector, "run_sqlplus", side_effect=self.query_sqlplus), \
                 mock.patch.object(collector, "require_oracle_context", return_value={"oracle_sid": "TEST", "oracle_home": "/oracle"}), \
                 mock.patch.object(collector.sys.stdin, "isatty", return_value=True), \
                 mock.patch.object(collector, "unique_output_dir", return_value=root), \
                 mock.patch.object(collector, "generate_statspack_reports", side_effect=generate_reports), \
                 mock.patch("builtins.input", side_effect=["statspack", "2026-09-10 13:00", "2026-09-10 18:30", "2", "n", "n", "n", "reports"]), \
                 contextlib.redirect_stdout(io.StringIO()):
                status = collector.main([])
            self.assertEqual(status, 0)
            manifest = (root / "manifest.txt").read_text(encoding="utf-8")
            self.assertIn("selected_startup=2026-09-10 14:45:12", manifest)
            self.assertIn("requested_start=2026-09-10 13:00:00", manifest)
            self.assertIn("start=2026-09-10 15:00:02", manifest)
            self.assertIn("end=2026-09-10 15:30:03", manifest)

    def test_redirected_input_never_requests_startup_selection(self):
        # Redirected runs fail with suggested ranges even if other CLI choices are absent.
        with mock.patch.object(collector.sys.stdin, "isatty", return_value=False):
            self.assertFalse(collector.startup_selection_can_prompt(collector.parse_collector_args([])))

    def test_date_arguments_accept_exact_seconds(self):
        # Users can paste the proposed START and END without rounding away snapshots.
        args = collector.parse_collector_args(["--start", "2026-09-10 13:10:15", "--end", "2026-09-10 14:00:01"])
        self.assertEqual(args.start_dt.second, 15)
        self.assertEqual(args.end_dt.second, 1)


class AwrPairTests(unittest.TestCase):
    def setUp(self):
        # Mirror the AWR catalog columns used by discovery and pair generation.
        self.db = sqlite3.connect(":memory:")
        self.addCleanup(self.db.close)
        self.db.executescript("""
            CREATE TABLE dba_hist_snapshot (
                dbid INTEGER, instance_number INTEGER, startup_time TEXT,
                snap_id INTEGER, end_interval_time TEXT
            );
            CREATE TABLE v$database (dbid INTEGER);
            INSERT INTO v$database VALUES (100);
            CREATE TABLE v$instance (instance_number INTEGER);
            INSERT INTO v$instance VALUES (1);
        """)
        self.db.create_function("to_timestamp", 2, lambda value, fmt: value)
        self.db.create_function("to_char", 2, lambda value, fmt: value)
        self.db.create_function("nvl", 2, lambda value, fallback: value if value is not None else fallback)

    def add_snap(self, snap_id, time, startup="2026-09-09 08:15:00", dbid=100, instance=1):
        # Store ISO text so SQLite ordering matches Oracle timestamp ordering.
        self.db.execute(
            "INSERT INTO dba_hist_snapshot VALUES (?, ?, ?, ?, ?)",
            (dbid, instance, startup, snap_id, "2026-09-10 " + time),
        )

    def add_two_periods(self):
        for snap_id, time in [(101, "13:10:15"), (102, "13:19:37"), (106, "14:00:01")]:
            self.add_snap(snap_id, time)
        for snap_id, time in [(107, "15:00:02"), (108, "15:30:03")]:
            self.add_snap(snap_id, time, startup="2026-09-10 14:45:12")

    def query_sqlplus(self, ctx, script):
        # Execute the exact CTE/select body while omitting SQL*Plus directives.
        query = script[script.index("with "):].split(";", 1)[0]
        return "\n".join(str(row[0]) for row in self.db.execute(query))

    def periods(self):
        with mock.patch.object(collector, "run_sqlplus", side_effect=self.query_sqlplus):
            return collector.discover_awr_startup_periods(
                {}, datetime(2026, 9, 10, 13), datetime(2026, 9, 10, 18, 30)
            )

    def pairs(self, startup=None):
        with mock.patch.object(collector, "run_sqlplus", side_effect=self.query_sqlplus):
            return collector.discover_awr_pairs(
                {}, datetime(2026, 9, 10, 13), datetime(2026, 9, 10, 18, 30), startup
            )

    def test_awr_periods_are_grouped_by_startup_and_current_instance(self):
        # Foreign DB/instance rows cannot become selectable AWR periods.
        self.add_two_periods()
        self.add_snap(109, "16:00:00", startup="2026-09-10 15:45:00", dbid=200)
        self.add_snap(110, "16:30:00", startup="2026-09-10 16:15:00", instance=2)
        periods = self.periods()
        self.assertEqual([p["snapshot_count"] for p in periods], [3, 2])
        self.assertEqual([p["pair_count"] for p in periods], [2, 1])
        self.assertEqual(periods[1]["startup_time"], datetime(2026, 9, 10, 14, 45, 12))

    def test_awr_pairs_are_pinned_to_selected_startup(self):
        # A broad requested range produces only pairs from the reviewed startup.
        self.add_two_periods()
        pairs = self.pairs(datetime(2026, 9, 10, 14, 45, 12))
        self.assertEqual(pairs, [{"dbid": "100", "inst_num": "1", "begin_snap": "107", "end_snap": "108"}])

    def test_awr_pairs_never_cross_restart(self):
        # Partitioning protects direct helper callers even without a startup filter.
        self.add_two_periods()
        self.assertEqual(
            [(p["begin_snap"], p["end_snap"]) for p in self.pairs()],
            [("101", "102"), ("102", "106"), ("107", "108")],
        )

    def test_interactive_awr_main_collects_numbered_period(self):
        # Exercise AWR orchestration through manifest creation with database I/O mocked.
        self.add_two_periods()
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)

            def generate_reports(ctx, pairs, output):
                self.assertEqual([(p["begin_snap"], p["end_snap"]) for p in pairs], [("107", "108")])
                report = output / "awrrpt_1_107_108.html"
                report.write_text("<html>AWR fixture</html>\n", encoding="utf-8")
                return [report]

            with mock.patch.object(collector, "run_sqlplus", side_effect=self.query_sqlplus), \
                 mock.patch.object(collector, "require_oracle_context", return_value={"oracle_sid": "TEST", "oracle_home": "/oracle"}), \
                 mock.patch.object(collector.sys.stdin, "isatty", return_value=True), \
                 mock.patch.object(collector, "unique_output_dir", return_value=root), \
                 mock.patch.object(collector, "generate_awr_reports", side_effect=generate_reports), \
                 mock.patch("builtins.input", side_effect=["awr", "2026-09-10 13:00", "2026-09-10 18:30", "2", "n", "n", "n", "reports"]), \
                 contextlib.redirect_stdout(io.StringIO()):
                status = collector.main([])

            self.assertEqual(status, 0)
            manifest = (root / "manifest.txt").read_text(encoding="utf-8")
            self.assertIn("report_type=AWR", manifest)
            self.assertIn("selected_startup=2026-09-10 14:45:12", manifest)
            self.assertIn("requested_start=2026-09-10 13:00:00", manifest)
            self.assertIn("start=2026-09-10 15:00:02", manifest)
            self.assertIn("end=2026-09-10 15:30:03", manifest)


if __name__ == "__main__":
    unittest.main()
