"""Import/coverage contracts; no Oracle connection or new third-party packages."""
import copy
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("collector", ROOT / "jas-min-collector.py")
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)


class AccessPathEvidenceTests(unittest.TestCase):
    def test_segment_scope_legacy_values_and_security_mask(self):
        parser = collector.AWRHTMLTableParser()
        parser.feed((ROOT / "tests/fixtures/empty_calories/segment_scope.html").read_text())
        scoped = collector.parse_segment_stats(parser.tables[0], "Logical Reads", 1)
        self.assertEqual(len(scoped), 1)
        self.assertEqual((scoped[0]["owner"], scoped[0]["pdb_name"], scoped[0]["con_id"], scoped[0]["subobject_name"]), ("LAB", "PDB_A", 3, "P_01"))
        self.assertEqual(scoped[0]["stat_vlalue"], 12345)
        legacy = collector.parse_segment_stats(parser.tables[1], "Logical Reads", 1)
        self.assertEqual(legacy[0]["stat_vlalue"], 9876)
        self.assertEqual(legacy[0]["obj"], 0)
        hidden = collector.parse_segment_stats(parser.tables[0], "Logical Reads", 0)[0]
        self.assertEqual(hidden["object_name"], "#")
        self.assertIsNone(hidden["owner"])
        self.assertIsNone(hidden["pdb_name"])
        self.assertIsNone(hidden["subobject_name"])

    def fixtures(self):
        collection = json.loads((ROOT / "tests/fixtures/empty_calories/scan_degradation.json").read_text())
        info = collection["db_instance_information"]
        scope = dict(dbid=info["db_id"], inst_id=info["instance_num"], con_id=3, sql_id="testsql", child_number=0, plan_hash_value=10, object_id=20, data_object_id=30)
        evidence = dict(schema_version="2026-09-13.1", dbid=info["db_id"], inst_id=info["instance_num"], windows=[dict(snap_info=copy.deepcopy(collection["awrs"][0]["snap_info"]), observations=[dict(scope=scope, evidence_ref="fixture-only", executions=200, elapsed_s=0.34, buffer_gets=38000, continued_rows=0)], data_availability={"host_cpu": False})])
        return collection, evidence

    def merge(self, collection, evidence):
        with tempfile.TemporaryDirectory() as directory:
            dest = Path(directory) / "input.json"
            source = Path(directory) / "evidence.json"
            dest.write_text(json.dumps(collection))
            source.write_text(json.dumps(evidence))
            collector.merge_access_path_evidence(dest, source)
            return json.loads(dest.read_text())

    def test_exact_scoped_window_is_preserved_in_packaged_json(self):
        c, e = self.fixtures()
        merged = self.merge(c, e)
        self.assertEqual(merged["awrs"][0]["access_path_observations"], e["windows"][0]["observations"])
        self.assertFalse(merged["awrs"][0]["data_availability"]["host_cpu"])
        self.assertNotIn("access_path_observations", merged["awrs"][1])

    def test_wrong_db_instance_time_and_duplicate_window_are_rejected_atomically(self):
        c, e = self.fixtures()
        for field in ("dbid", "inst_id", "timestamp", "duplicate", "reset", "duplicate_scope"):
            bad = copy.deepcopy(e)
            if field in ("dbid", "inst_id"):
                bad[field] += 1
            elif field == "timestamp":
                bad["windows"][0]["snap_info"]["begin_snap_time"] = "wrong"
            elif field == "duplicate":
                bad["windows"].append(copy.deepcopy(bad["windows"][0]))
            elif field == "reset":
                bad["windows"][0]["observations"][0]["buffer_gets"] = -1
            else:
                bad["windows"][0]["observations"].append(copy.deepcopy(bad["windows"][0]["observations"][0]))
            with tempfile.TemporaryDirectory() as directory:
                dest, source = Path(directory) / "input.json", Path(directory) / "evidence.json"
                dest.write_text(json.dumps(c)); original = dest.read_bytes()
                source.write_text(json.dumps(bad))
                with self.assertRaises(collector.CollectorError, msg=field):
                    collector.merge_access_path_evidence(dest, source)
                self.assertEqual(dest.read_bytes(), original)

    def test_placeholder_cpu_unknown_and_real_idle_valid_without_topology(self):
        a = collector.default_awr("test")
        a["host_cpu"]["cpus"] = 10
        collector.mark_data_availability(a)
        self.assertFalse(a["data_availability"]["host_cpu"])
        a["host_cpu"]["cpus"] = 0
        a["host_cpu"]["pct_idle"] = 100
        collector.mark_data_availability(a)
        self.assertTrue(a["data_availability"]["host_cpu"])
        self.assertFalse(a["data_availability"]["segment_stats"])

    def test_statspack_adapter_keeps_timestamps_for_exposure(self):
        result = collector.parse_text_snap_info(Path("test_10_11.txt"), ["Begin Snap: 10 13-Sep-26 10:00:00", "End Snap: 11 13-Sep-26 10:30:00"])
        self.assertEqual(result["begin_snap_time"], "13-Sep-26 10:00:00")
        self.assertEqual(result["end_snap_time"], "13-Sep-26 10:30:00")


if __name__ == "__main__":
    unittest.main()
