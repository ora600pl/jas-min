import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "jas_min_collector_parser", ROOT / "jas-min-collector.py"
)
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)


class CollectorStatspackParserTests(unittest.TestCase):
    def test_all_statspack_samples_fill_reference_sections(self):
        # Exercise 11g and two 19c layouts, including Linux and AIX reports.
        report_dirs = ("test_snaps_11", "test_snaps_19.10", "test_snaps_abakus")
        reports = [
            report
            for directory in report_dirs
            for report in sorted((ROOT / "tests" / directory).glob("*.txt"))
        ]
        self.assertEqual(len(reports), 22)

        for report in reports:
            with self.subTest(report=report.name):
                awr, sql_text, parameters, db_instance = collector.parse_text_report(report, 2)
                self.assertEqual(len(awr["load_profile"]), 15)
                self.assertEqual(len(awr["instance_efficiency"]), 10)
                self.assertTrue(awr["time_model_stats"])
                self.assertTrue(awr["foreground_wait_events"])
                self.assertTrue(awr["sql_cpu_time"])
                self.assertTrue(awr["sql_gets"])
                self.assertGreater(len(awr["instance_stats"]), 200)
                self.assertTrue(awr["dictionary_cache"])
                self.assertTrue(awr["library_cache"])
                self.assertGreater(len(awr["latch_activity"]), 200)
                self.assertTrue(sql_text)
                self.assertTrue(parameters)
                self.assertGreater(db_instance["db_id"], 0)
                self.assertGreater(db_instance["db_block_size"], 0)

    def test_aix_statspack_values_match_awr_rs_reference(self):
        # Keep representative values from every newly supported section stable.
        report = ROOT / "tests/test_snaps_19.10/sp_13_14.txt"
        awr, sql_text, parameters, db_instance = collector.parse_text_report(report, 2)

        efficiency = {item["eff_stat"]: item["eff_pct"] for item in awr["instance_efficiency"]}
        time_model = {item["stat_name"]: item for item in awr["time_model_stats"]}
        foreground = {item["event"]: item for item in awr["foreground_wait_events"]}
        dictionary = {item["statname"]: item for item in awr["dictionary_cache"]}
        library = {item["statname"]: item for item in awr["library_cache"]}

        self.assertEqual(efficiency["Execute to Parse %"], 77.39)
        self.assertEqual(awr["host_cpu"]["cpus"], 64)
        self.assertEqual(awr["host_cpu"]["pct_idle"], 92.87)
        self.assertEqual(time_model["DB time"]["time_s"], 13205.1)
        self.assertEqual(foreground["resmgr:cpu quantum"]["pct_dbtime"], 47.5)
        self.assertEqual(
            foreground["resmgr:cpu quantum"]["waitevent_histogram_ms"]["7: <=1s"],
            94.5,
        )
        self.assertEqual(awr["redo_log"]["stat_name"], "log switches (derived)")
        self.assertEqual(dictionary["dc_objects"]["final_usage"], 25439)
        self.assertEqual(library["SQL AREA"]["pin_requests"], 7376261)
        self.assertEqual(awr["io_stats_byfunc"]["LGWR"]["writes_data"], 635.0)
        self.assertEqual(len(awr["instance_stats"]), 368)
        self.assertEqual(len(awr["latch_activity"]), 427)
        self.assertEqual(len(sql_text), 21)
        self.assertEqual(parameters["control_files"], "/u02/oradata/bpsc/control01.ctl,/u02/oradata/bpsc/control02.ctl")
        self.assertEqual(db_instance["platform"], "AIX-Based Systems (64-")
        self.assertEqual(db_instance["db_block_size"], 8192)

    def test_legacy_hash_values_are_not_confused_with_wrapped_sql(self):
        # Oracle 11g STATSPACK uses numeric OLD_HASH_VALUE identifiers.
        report = ROOT / "tests/test_snaps_11/sp_90_91.txt"
        awr, sql_text, _, _ = collector.parse_text_report(report, 2)

        self.assertIn("3914144968", awr["sql_cpu_time"])
        self.assertIn("3914144968", awr["sql_gets"])
        self.assertIn("3914144968", sql_text)
        self.assertNotIn("wit", sql_text)


class CollectorAwrParserTests(unittest.TestCase):
    def test_all_awr_samples_keep_core_sections(self):
        # Parse every supplied 19c AWR report to guard the shared output schema.
        reports = sorted((ROOT / "tests/test_awrs_19").glob("*.html"))
        self.assertEqual(len(reports), 14)

        for report in reports:
            with self.subTest(report=report.name):
                awr, sql_text, parameters, db_instance = collector.parse_html_report(report, 2)
                self.assertTrue(awr["load_profile"])
                self.assertEqual(len(awr["instance_efficiency"]), 11)
                self.assertTrue(awr["time_model_stats"])
                self.assertTrue(awr["foreground_wait_events"])
                self.assertTrue(awr["background_wait_events"])
                self.assertTrue(awr["sql_elapsed_time"])
                self.assertTrue(awr["sql_gets"])
                self.assertGreater(len(awr["instance_stats"]), 400)
                self.assertTrue(sql_text)
                self.assertTrue(parameters)
                self.assertGreater(db_instance["db_id"], 0)

    def test_awr_decimal_commas_and_idle_events_match_reference(self):
        # This report mixes decimal dots and decimal commas in one SQL table.
        report = ROOT / "tests/test_awrs_19/awrrpt_1_42071_42072.html"
        awr, _, _, _ = collector.parse_html_report(report, 2)
        sql_gets = awr["sql_gets"]["g1gp1a3z0ns6s"]
        foreground_names = {item["event"] for item in awr["foreground_wait_events"]}
        background_names = {item["event"] for item in awr["background_wait_events"]}

        self.assertEqual(sql_gets["pct_cpu"], 91.9)
        self.assertEqual(sql_gets["pct_io"], 6.1)
        self.assertNotIn("jobq slave wait", foreground_names)
        self.assertNotIn("Data Guard: Timer", background_names)
        self.assertIn("SQL*Net message to client", foreground_names)


if __name__ == "__main__":
    unittest.main()
