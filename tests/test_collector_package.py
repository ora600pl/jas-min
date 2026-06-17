import importlib.util
import contextlib
import io
import tempfile
import unittest
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
COLLECTOR_PATH = ROOT / "jas-min-collector.py"

spec = importlib.util.spec_from_file_location("jas_min_collector", COLLECTOR_PATH)
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)


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

            manifest = output_dir / "manifest.txt"
            manifest.write_text("manifest\n", encoding="utf-8")

            zip_path = collector.create_zip_package(
                output_dir,
                stem,
                [report],
                json_path,
                (alert_log, "filtered alert log"),
                {"files": [xplan]},
                manifest,
                collector.PACKAGE_BOTH,
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
                    "{}_attachments/alert_szpital.log".format(stem),
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
