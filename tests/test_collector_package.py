import importlib.util
import contextlib
import io
import hashlib
import json
import os
import re
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


def shared_cursor_transport_record(child_number, reason, chunk_chars=40):
    """Encode a REASON CLOB exactly like the SQL*Plus transport block."""
    chunks = [reason[index:index + chunk_chars]
              for index in range(0, len(reason), chunk_chars)]
    lines = ["JASMIN_REASON_BEGIN|{}|{}".format(child_number, len(reason))]
    for sequence, chunk in enumerate(chunks, start=1):
        lines.append(
            "JASMIN_REASON_DATA|{}|{}|{}".format(
                child_number, sequence, chunk.encode("utf-8").hex().upper()
            )
        )
    lines.append("JASMIN_REASON_END|{}|{}".format(child_number, len(chunks)))
    return "\n".join(lines)


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
        self.assertEqual(result.stdout.strip(), "jas-min-collector 0.1.15")

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

    def test_statspack_sql_parser_ignores_wrapped_scheduler_source(self):
        # Wrapped PL/SQL can also have seven fields, but WIT is not an Oracle SQL_ID.
        rows = collector.parse_text_sql_section(
            [
                "61,110,353 2 30,555,176.5 2.4 39.51 147.48 a5j2xsnpgpxjx",
                "Module: DBMS_SCHEDULER",
                "job_owner VARCHAR2(128) := :job_owner; job_start TIMESTAMP WIT",
            ],
            "elapsed",
        )

        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["sql_id"], "a5j2xsnpgpxjx")
        self.assertEqual(rows[0]["elapsed_time_s"], 61110353.0)
        self.assertEqual(rows[0]["sql_module"], "DBMS_SCHEDULER")

    def test_top_sql_selection_skips_malformed_ids_from_legacy_json(self):
        # Previously generated JSON stays loadable without wasting a plan slot on WIT.
        with tempfile.TemporaryDirectory() as tmpdir:
            json_path = Path(tmpdir) / "legacy.json"
            json_path.write_text(
                json.dumps(
                    {
                        "awrs": [
                            {
                                "sql_elapsed_time": [
                                    {"sql_id": "wit", "elapsed_time_s": 0.0},
                                    {"sql_id": "a5j2xsnpgpxjx", "elapsed_time_s": 12.5},
                                ]
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )

            rows = collector.top_elapsed_sql_id_counts_from_json(json_path)

        self.assertEqual(
            rows,
            [
                {
                    "sql_id": "a5j2xsnpgpxjx",
                    "count": 1,
                    "elapsed_time_s": 12.5,
                    "first_seen": 0,
                }
            ],
        )

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
        self.assertIn("--execution-plan-timeout", help_text)
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
        self.assertEqual(
            args.execution_plan_timeout,
            collector.DEFAULT_XPLAN_TIMEOUT_SECONDS,
        )

    def test_execution_plan_timeout_must_be_positive(self):
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            with self.assertRaises(SystemExit) as exc:
                collector.parse_collector_args(["--execution-plan-timeout", "0"])

        self.assertNotEqual(exc.exception.code, 0)

    def test_execution_plan_timeout_can_be_overridden(self):
        args = collector.parse_collector_args(
            ["--execution-plan-timeout", "45"]
        )

        self.assertEqual(args.execution_plan_timeout, 45)

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

    def test_execution_plan_cursor_discovery_selects_one_concrete_child(self):
        sql = collector.execution_plan_cursors_sql(["ABC123", "def456"])

        self.assertIn("from v$sql", sql)
        self.assertIn("is_shareable = 'Y'", sql)
        self.assertIn("last_active_time desc nulls last", sql)
        with mock.patch.object(
            collector,
            "run_sqlplus",
            return_value="abc123|7|3\ndef456|2|1\n",
        ) as run:
            rows = collector.discover_execution_plan_cursors(
                {"sqlplus": "unused"}, ["abc123", "def456"], 45
            )

        self.assertEqual(
            rows,
            [
                {"sql_id": "abc123", "child_number": 7, "child_count": 3},
                {"sql_id": "def456", "child_number": 2, "child_count": 1},
            ],
        )
        self.assertEqual(run.call_args.kwargs["timeout"], 45)

    def test_xplan_uses_concrete_child_cursor(self):
        sql = collector.xplan_sql("abc123", "abc123.xplan", 7)

        self.assertIn("display_cursor('abc123',7,'TYPICAL')", sql)
        self.assertNotIn("display_cursor('abc123',null)", sql.lower())

    def test_run_sqlplus_converts_timeout_to_collector_error(self):
        timeout = subprocess.TimeoutExpired(
            cmd=["sqlplus"], timeout=12, output=b"partial output\n"
        )
        with mock.patch.object(collector.subprocess, "run", side_effect=timeout):
            with self.assertRaisesRegex(
                collector.CollectorError,
                "(?s)timed out after 12 second.*partial output",
            ):
                collector.run_sqlplus(
                    {"sqlplus": "/oracle/bin/sqlplus", "env": {}},
                    "select 1 from dual;",
                    timeout=12,
                )

    def test_plan_timeout_removes_partial_file_and_continues(self):
        def fake_run(_ctx, script, cwd=None, timeout=None, **_kwargs):
            filename = re.search(r"^spool ([^\n]+)", script, re.MULTILINE).group(1)
            target = Path(cwd) / filename
            if filename == "abc123.xplan":
                target.write_text("partial\n", encoding="utf-8")
                raise collector.CollectorError(
                    "sqlplus timed out after {} second(s)".format(timeout)
                )
            target.write_text("complete plan\n", encoding="utf-8")
            return ""

        with tempfile.TemporaryDirectory() as tmpdir:
            target_dir = Path(tmpdir)
            with mock.patch.object(collector, "run_sqlplus", side_effect=fake_run):
                files, failures = collector.collect_sql_execution_plans(
                    {"sqlplus": "unused"},
                    target_dir,
                    ["abc123", "def456"],
                    {"abc123": 7, "def456": 2},
                    timeout=30,
                )

            self.assertFalse((target_dir / "abc123.xplan").exists())
            self.assertTrue((target_dir / "def456.xplan").is_file())

        self.assertEqual([path.name for path in files], ["def456.xplan"])
        self.assertEqual(failures[0][0], "abc123")
        self.assertIn("timed out after 30", failures[0][1])

    def test_shared_cursor_reason_sql_transports_clob_without_with_clause(self):
        sql = collector.shared_cursor_reasons_sql("AbC123")

        self.assertIn("where sql_id = 'AbC123'", sql)
        self.assertIn("dbms_lob.substr(cursor_row.reason, 500, l_position)", sql)
        self.assertIn("utl_i18n.string_to_raw(l_chunk, 'AL32UTF8')", sql)
        self.assertIn("JASMIN_REASON_BEGIN|", sql)
        self.assertIn("JASMIN_REASON_DATA|", sql)
        self.assertNotIn("\nwith\n", sql.lower())

    def test_shared_cursor_reason_transport_preserves_unicode_and_chunk_order(self):
        # Hex framing prevents SQL*Plus wrapping from changing XML or national text.
        reason = (
            "<ChildNode><ChildNumber>2</ChildNumber><ID>44</ID>"
            "<reason>NLS Settings(0)</reason><size>1x1</size>"
            "<nls_language>[POLSKI]->[AMERICAN]</nls_language></ChildNode>"
        )
        output = shared_cursor_transport_record(2, reason, chunk_chars=13)

        records = collector.parse_shared_cursor_reason_transport(output)

        self.assertEqual(records, [{"view_child": 2, "reason": reason}])

    def test_shared_cursor_reason_formatter_preserves_nodes_fields_and_order(self):
        # This is a compact form of the Oracle 19.10 payload observed on AIX.
        reason = (
            "<ChildNode><ChildNumber>2</ChildNumber><ID>5</ID>"
            "<reason>Max Long Length Different(0)</reason><size>2x4</size>"
            "<max_long_length_kkschlngv_cursor>4000</max_long_length_kkschlngv_cursor>"
            "<max_long_length_HSTMXLNG_current>32767</max_long_length_HSTMXLNG_current>"
            "</ChildNode>"
            "<ChildNode><ChildNumber>2</ChildNumber><ID>33</ID>"
            "<reason>Rolling Invalidate Window Exceeded(2)</reason><size>0x0</size>"
            "<details>already_processed</details></ChildNode>"
        )
        records = collector.parse_shared_cursor_reason_transport(
            shared_cursor_transport_record(2, reason)
        )

        rendered = collector.format_shared_cursor_reasons("1k5d6mkhqtnbp", records)

        self.assertIn("CHILD CURSOR 2", rendered)
        self.assertIn(
            "+-- [01] Max Long Length Different  {ID=5, subcode=0, payload=2x4}",
            rendered,
        )
        self.assertIn("|   01. max_long_length_kkschlngv_cursor  = 4000", rendered)
        self.assertIn(
            "+-- [02] Rolling Invalidate Window Exceeded  {ID=33, subcode=2, payload=0x0}",
            rendered,
        )
        self.assertLess(rendered.index("+-- [01]"), rendered.index("+-- [02]"))
        self.assertTrue(
            rendered.endswith(
                "SUMMARY: 1 child cursor(s), 2 reason node(s), 3 diagnostic field(s)."
            )
        )

    def test_shared_cursor_reason_formatter_keeps_pair_and_datatype_decoding(self):
        # Python retains the former SQL formatter's A/B and bind-type explanations.
        reason = (
            "<ChildNode><ChildNumber>4</ChildNumber><ID>44</ID>"
            "<reason>NLS Settings(0)</reason><size>1x1</size>"
            "<nls_language>[POLISH]->[AMERICAN]</nls_language></ChildNode>"
            "<ChildNode><ChildNumber>4</ChildNumber><ID>39</ID>"
            "<reason>Bind mismatch(7)</reason><size>2x4</size>"
            "<original_oacdty>1</original_oacdty><new_oacdty>2</new_oacdty>"
            "</ChildNode>"
        )

        rendered = collector.format_shared_cursor_reasons(
            "1k5d6mkhqtnbp", [{"view_child": 4, "reason": reason}]
        )

        self.assertIn("A=[POLISH] | B=[AMERICAN]", rendered)
        self.assertIn("[original] = 1 (VARCHAR2)", rendered)
        self.assertIn("[new] = 2 (NUMBER)", rendered)

    def test_shared_cursor_reason_transport_rejects_missing_chunk(self):
        # Missing sequence numbers must fail instead of producing partial evidence.
        output = "\n".join([
            "JASMIN_REASON_BEGIN|2|3",
            "JASMIN_REASON_DATA|2|2|414243",
            "JASMIN_REASON_END|2|1",
        ])

        with self.assertRaisesRegex(collector.CollectorError, "out of sequence"):
            collector.parse_shared_cursor_reason_transport(output)

    def test_shared_cursor_reason_formatter_rejects_malformed_xml(self):
        # Corrupt evidence must be reported instead of creating a partial attachment.
        with self.assertRaisesRegex(collector.CollectorError, "Malformed.*XML"):
            collector.format_shared_cursor_reasons(
                "1k5d6mkhqtnbp",
                [{"view_child": 2, "reason": "<ChildNode>"}],
            )

    def test_shared_cursor_reason_formatter_preserves_repeated_nodes(self):
        # Oracle can repeat one comparison reason and each occurrence remains evidence.
        node = (
            "<ChildNode><ChildNumber>2</ChildNumber><ID>41</ID>"
            "<reason>Marked for Purge(5)</reason><size>1x1</size>"
            "<unsafe_ddl_code>0</unsafe_ddl_code></ChildNode>"
        )

        rendered = collector.format_shared_cursor_reasons(
            "1k5d6mkhqtnbp", [{"view_child": 2, "reason": node + node}]
        )

        self.assertIn("+-- [01] Marked for Purge", rendered)
        self.assertIn("+-- [02] Marked for Purge", rendered)
        self.assertTrue(
            rendered.endswith(
                "SUMMARY: 1 child cursor(s), 2 reason node(s), 2 diagnostic field(s)."
            )
        )

    def test_child_cursor_reason_collection_writes_one_attachment_per_sql_id(self):
        reason = (
            "<ChildNode><ChildNumber>1</ChildNumber><ID>3</ID>"
            "<reason>Optimizer mismatch(0)</reason><size>0x0</size>"
            "</ChildNode>"
        )
        with tempfile.TemporaryDirectory() as tmpdir:
            target_dir = Path(tmpdir)
            with mock.patch.object(
                collector,
                "run_sqlplus",
                return_value=shared_cursor_transport_record(1, reason),
            ):
                files, failures = collector.collect_shared_cursor_reasons(
                    {"sqlplus": "unused"},
                    target_dir,
                    [{"sql_id": "1k5d6mkhqtnbp", "child_count": 2}],
                )

            self.assertEqual(failures, [])
            self.assertEqual(
                [path.name for path in files],
                ["1k5d6mkhqtnbp.shared_cursor_reasons"],
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
