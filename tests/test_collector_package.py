import importlib.util
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
