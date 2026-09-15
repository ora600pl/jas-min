import importlib.util
import contextlib
import io
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
COLLECTOR_PATH = ROOT / "jas-min-collector.py"

spec = importlib.util.spec_from_file_location("jas_min_collector", COLLECTOR_PATH)
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)


class CollectorIdentityTests(unittest.TestCase):
    def test_version_without_oracle_environment(self):
        # End users must be able to identify a copy before configuring Oracle.
        env = {key: value for key, value in os.environ.items()
               if key not in ("ORACLE_HOME", "ORACLE_SID")}
        result = subprocess.run(
            [sys.executable, str(COLLECTOR_PATH), "--version"],
            env=env, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "jas-min-collector 0.1.11")

    def test_json_provenance_preserves_legacy_payload(self):
        # Metadata is additive: removing it leaves the original collection shape.
        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir)
            report = output / "sp_8_9.txt"
            report.write_text("", encoding="utf-8")
            path = collector.parse_reports_to_json([report], output, "sample", 0)
            collection = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(next(iter(collection)), "collector_info")
            info = collection.pop("collector_info")
            self.assertEqual(info["version"], collector.COLLECTOR_VERSION)
            self.assertEqual(info["parser"], "python-collector")
            self.assertEqual(info["script_sha256"], hashlib.sha256(COLLECTOR_PATH.read_bytes()).hexdigest())
            self.assertRegex(info["parsed_at_utc"], r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
            self.assertEqual(set(collection), {"db_instance_information", "initialization_parameters", "awrs", "sql_text"})
            self.assertEqual(collection["awrs"][0]["snap_info"]["begin_snap_id"], 8)
            self.assertEqual(collection["awrs"][0]["snap_info"]["end_snap_id"], 9)

    def test_manifest_identity_for_reports_only_and_json(self):
        # Both package modes carry the same script identity as generated JSON.
        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir)
            identity = collector.collector_identity()
            for mode in (collector.PACKAGE_REPORTS, collector.PACKAGE_JSON):
                path = collector.write_manifest(
                    {"oracle_sid": "TEST", "oracle_home": "/example/oracle"},
                    output, "STATSPACK", collector.datetime(2026, 9, 10),
                    collector.datetime(2026, 9, 11), [], None, mode, None, 0, None,
                )
                text = path.read_text(encoding="utf-8")
                self.assertIn("collector_version={}\n".format(identity["version"]), text)
                self.assertIn("collector_script_sha256={}\n".format(identity["script_sha256"]), text)

    def test_local_edits_change_script_identity(self):
        # A modified copy remains distinguishable even without a version bump.
        with tempfile.TemporaryDirectory() as tmpdir:
            script = Path(tmpdir) / "collector.py"
            script.write_bytes(b"original\n")
            with mock.patch.object(collector, "__file__", str(script)):
                before = collector.collector_identity()
                script.write_bytes(b"modified\n")
                after = collector.collector_identity()
            self.assertEqual(before["version"], after["version"])
            self.assertNotEqual(before["script_sha256"], after["script_sha256"])


class CollectorZipPackageTests(unittest.TestCase):
    def test_parse_int_uses_default_for_negative_unsigned_values(self):
        self.assertEqual(collector.parse_int("-4,254,126,895"), 0)
        self.assertEqual(collector.parse_int("1,234"), 1234)

    def test_zip_package_preserves_report_and_attachment_directories(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            output_dir = Path(tmpdir)
            stem = "szpital_awr_20260614_0000_20260615_1400"

            reports_dir = output_dir / stem
            reports_dir.mkdir()
            report = reports_dir / "awrrpt_1_252_253.html"
            report.write_text("<html>AWR</html>\n", encoding="utf-8")

            json_path = output_dir / "{}.json".format(stem)
            json_path.write_text("{}\n", encoding="utf-8")

            attachments_dir = output_dir / "{}_attachments".format(stem)
            attachments_dir.mkdir()
            alert_log = attachments_dir / "alert_szpital.log"
            alert_log.write_text("alert\n", encoding="utf-8")
            xplan = attachments_dir / "abc123.xplan"
            xplan.write_text("plan\n", encoding="utf-8")
            child_reasons = attachments_dir / "abc123.shared_cursor_reasons"
            child_reasons.write_text("decoded reasons\n", encoding="utf-8")

            os_stats_dir = output_dir / "prepared_os_stats"
            os_stats_dir.mkdir()
            vmstat = os_stats_dir / "vmstat.out"
            vmstat.write_text("vmstat\n", encoding="utf-8")
            nested_os_stats_dir = os_stats_dir / "nested"
            nested_os_stats_dir.mkdir()
            iostat = nested_os_stats_dir / "iostat.out"
            iostat.write_text("iostat\n", encoding="utf-8")
            os_stats_info = collector.copy_os_stats(
                os_stats_dir,
                collector.list_os_stats_files(os_stats_dir),
                output_dir,
                stem,
                platform_dir_name="linux",
            )

            manifest = output_dir / "manifest.txt"
            manifest.write_text("manifest\n", encoding="utf-8")

            zip_path = collector.create_zip_package(
                output_dir,
                stem,
                [report],
                json_path,
                (alert_log, "filtered alert log"),
                {
                    "files": [xplan],
                    "child_cursor_reason_files": [child_reasons],
                },
                manifest,
                collector.PACKAGE_BOTH,
                os_stats_info,
            )

            with zipfile.ZipFile(str(zip_path), "r") as archive:
                names = sorted(archive.namelist())

        self.assertEqual(
            names,
            sorted(
                [
                    "manifest.txt",
                    "{}.json".format(stem),
                    "{}/awrrpt_1_252_253.html".format(stem),
                    "{}_attachments/abc123.xplan".format(stem),
                    "{}_attachments/abc123.shared_cursor_reasons".format(stem),
                    "{}_attachments/alert_szpital.log".format(stem),
                    "{}_attachments/linux/nested/iostat.out".format(stem),
                    "{}_attachments/linux/vmstat.out".format(stem),
                ]
            ),
        )


class CollectorCliTests(unittest.TestCase):
    def test_help_lists_non_interactive_collector_options(self):
        help_text = collector.build_arg_parser().format_help()

        self.assertIn("--report-type", help_text)
        self.assertIn("--start", help_text)
        self.assertIn("--end", help_text)
        self.assertIn("--include-alert-log", help_text)
        self.assertIn("--execution-plans", help_text)
        self.assertIn("--package-content", help_text)
        self.assertIn("--security-level", help_text)
        self.assertIn("--include-os-stats", help_text)
        self.assertIn("--os-stats-dir", help_text)

    def test_parse_collector_args_normalizes_cli_values(self):
        args = collector.parse_collector_args(
            [
                "--report-type",
                "statspack",
                "--start",
                "2026-06-14 00:00",
                "--end",
                "2026-06-15 14:00",
                "--no-alert-log",
                "--execution-plans",
                "--sql-id",
                "ABC123,def456",
                "--sql-id",
                "abc123",
                "--package-content",
                "json",
                "--security-level",
                "2",
            ]
        )

        self.assertEqual(args.report_type, "STATSPACK")
        self.assertEqual(collector.datetime_sql(args.start_dt), "2026-06-14 00:00")
        self.assertEqual(collector.datetime_sql(args.end_dt), "2026-06-15 14:00")
        self.assertFalse(args.include_alert)
        self.assertTrue(args.include_sql_plans)
        self.assertEqual(args.manual_sql_ids, ["abc123", "def456"])
        self.assertEqual(args.package_mode, collector.PACKAGE_JSON)
        self.assertEqual(args.security_level, 2)

    def test_os_stats_dir_argument_implies_os_stats_collection(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            args = collector.parse_collector_args(["--os-stats-dir", tmpdir])

        self.assertTrue(args.include_os_stats)
        self.assertEqual(args.os_stats_dir, Path(tmpdir).resolve())

    def test_os_stats_dir_cannot_be_used_when_disabled(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                with self.assertRaises(SystemExit) as exc:
                    collector.parse_collector_args(["--no-os-stats", "--os-stats-dir", tmpdir])

        self.assertNotEqual(exc.exception.code, 0)

    def test_os_stats_platform_directory_names_match_supported_systems(self):
        self.assertEqual(collector.os_stats_platform_dir_name("AIX"), "AIX")
        self.assertEqual(collector.os_stats_platform_dir_name("Linux"), "linux")

    def test_sql_id_argument_implies_execution_plan_collection(self):
        args = collector.parse_collector_args(["--sql-id", "ABC123"])

        self.assertTrue(args.include_sql_plans)
        self.assertEqual(args.manual_sql_ids, ["abc123"])

    def test_awr_pair_discovery_uses_current_instance_only(self):
        sql = collector.awr_pairs_sql(
            collector.parse_datetime("2026-06-14 00:00"),
            collector.parse_datetime("2026-06-15 14:00"),
        )

        self.assertIn(
            "s.instance_number = (select instance_number from v$instance)",
            sql,
        )

    def test_multi_child_cursor_discovery_is_limited_to_selected_top_sql_ids(self):
        sql = collector.multi_child_cursor_sql(["ABC123", "def456"])

        self.assertIn("where sql_id in ('ABC123', 'def456')", sql)
        self.assertIn("count(distinct child_number) > 1", sql)

        with mock.patch.object(
            collector,
            "run_sqlplus",
            return_value="abc123|3\nnoise\ndef456|2\n",
        ):
            rows = collector.discover_multi_child_cursor_sqls(
                {"sqlplus": "unused"}, ["abc123", "def456"]
            )

        self.assertEqual(
            rows,
            [
                {"sql_id": "abc123", "child_count": 3},
                {"sql_id": "def456", "child_count": 2},
            ],
        )

    def test_shared_cursor_reason_sql_decodes_all_reason_nodes_and_payload_fields(self):
        sql = collector.shared_cursor_reasons_sql("AbC123")

        self.assertIn("where s.sql_id = 'AbC123'", sql)
        self.assertIn("'/ReasonRoot/ChildNode'", sql)
        self.assertIn("not(self::ChildNumber or self::ID or self::reason or self::size)", sql)
        self.assertIn("A/B denote comparison-vector sides", sql)
        self.assertIn("when p.reason_id = 44", sql)
        self.assertIn("when p.reason_id = 3", sql)
        self.assertIn("SUMMARY:", sql)

    def test_child_cursor_reason_collection_writes_one_attachment_per_sql_id(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            target_dir = Path(tmpdir)
            with mock.patch.object(
                collector,
                "run_sqlplus",
                return_value="CHILD CURSOR 1\n+-- [01] Optimizer mismatch\n",
            ):
                files, failures = collector.collect_shared_cursor_reasons(
                    {"sqlplus": "unused"},
                    target_dir,
                    [{"sql_id": "abc123", "child_count": 2}],
                )

            self.assertEqual(failures, [])
            self.assertEqual(
                [path.name for path in files],
                ["abc123.shared_cursor_reasons"],
            )
            self.assertIn("Optimizer mismatch", files[0].read_text(encoding="utf-8"))

    def test_end_must_be_later_than_start(self):
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            with self.assertRaises(SystemExit) as exc:
                collector.parse_collector_args(
                    [
                        "--start",
                        "2026-06-15 14:00",
                        "--end",
                        "2026-06-14 00:00",
                    ]
                )

        self.assertNotEqual(exc.exception.code, 0)
