import oracledb
import requests

audit_id = "prdedi"

conn = oracledb.connect("awranalyzer/awr@localhost:1521/audits")
cur = conn.cursor()
cur.execute("alter session set nls_date_format='DD-Mon-YY HH24:MI:SS'")
cur.execute("alter session set NLS_NUMERIC_CHARACTERS = '.,'")


r = requests.get("http://localhost:6751/parsedir/prdedi")

files_json = r.json()
for f in files_json:
    awr_data = f["awr_doc"]
    if awr_data["status"] == "OK":
        load_profile = awr_data["load_profile"]
        db_instance_information = awr_data["db_instance_information"]
        wait_classes = awr_data["wait_classes"]
        host_cpu = awr_data["host_cpu"]
        time_model_stats = awr_data["time_model_stats"]
        foreground_wait_events = awr_data["foreground_wait_events"]
        sql_elapsed_time = awr_data["sql_elapsed_time"]
        sql_cpu_time = awr_data["sql_cpu_time"]
        sql_io_time = awr_data["sql_io_time"]
        snap_info = awr_data["snap_info"]

        bsnap_id = snap_info["begin_snap_id"]
        esnap_id = snap_info["end_snap_id"]
        bsnap_time = snap_info["begin_snap_time"]
        esnap_time = snap_info["end_snap_time"]

        instance_stats = awr_data["key_instance_stats"]

        cur.execute("insert into perf_host_cpu values (:cpus, :cores, :sockets, :pct_user, :pct_system, :pct_wio, :pct_idle, :begin_snap, :end_snap, :audit_id)",
                    [host_cpu["cpus"], host_cpu["cores"], host_cpu["sockets"], host_cpu["pct_user"], host_cpu["pct_system"], host_cpu["pct_wio"], host_cpu["pct_idle"], bsnap_time, esnap_time, audit_id])

        for l in load_profile:
            cur.execute("insert into perf_load_profile values(:sn, :ps, :pt, :bs, :es, :ai)",
                        [ l["stat_name"],l["per_second"],l["per_transaction"], bsnap_time, esnap_time, audit_id  ])

        for w in wait_classes:
            cur.execute("insert into perf_wait_class values(:sn, :tw, :pt, :bs, :es, :ai)",
                        [w["wait_class"], w["total_wait_time_s"], w["db_time_pct"], bsnap_time, esnap_time, audit_id])

        for w in foreground_wait_events:
            cur.execute("insert into perf_forground_events values(:sn, :w, :wt, :bs, :es, :ai)",
                        [w["event"], w["waits"], w["total_wait_time_s"], bsnap_time, esnap_time, audit_id])

        for s in sql_elapsed_time:
            cur.execute("insert into sql_by_ela values(:sqid, :ela, :exe, :elaexe, :bs, :es, :ai)",
                        [s["sql_id"], s["elapsed_time_s"], s["executions"], s["elpased_time_exec_s"], bsnap_time, esnap_time, audit_id])

        for s in sql_cpu_time:
            cur.execute("insert into sql_by_cpu values(:sqid, :ela, :exe, :elaexe, :bs, :es, :ai)",
                        [s["sql_id"], s["cpu_time_s"], s["executions"], s["cpu_time_exec_s"], bsnap_time, esnap_time, audit_id])

        for s in sql_io_time:
            cur.execute("insert into sql_by_io_wait values(:sqid, :ela, :exe, :elaexe, :bs, :es, :ai)",
                        [s["sql_id"], s["io_time_s"], s["executions"], s["io_time_exec_s"], bsnap_time, esnap_time, audit_id])

        for s in instance_stats:
            cur.execute("insert into perf_instance_activity values(:s, :t, :bs, :es, :ai)",
                        [s["statname"], s["total"], bsnap_time, esnap_time, audit_id])
            
        for t in time_model_stats:
            cur.execute("insert into PERF_TIME_MODEL values(:s, :t, :bs, :es, :ai) ",
                        [t["stat_name"], t["time_s"], bsnap_time, esnap_time, audit_id])


    conn.commit()
