//! Public SQL tables computed on the captured project, with an explicit denominator.
use crate::awr::{AWR, AWRSCollection};
use crate::mcp_server::ProjectData;
use crate::measurements::{available, timestamp};
use crate::report::i18n::ReportLanguage;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn text<'a>(spec: &'a Value, key: &str) -> Result<&'a str, String> {
    spec[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("A nonempty {key} is required"))
}

fn bounded(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

#[derive(Clone, Copy)]
struct Counter {
    cost: f64,
    executions: u64,
}

impl Counter {
    fn read(cost: f64, executions: u64) -> Option<Self> {
        bounded(cost).then_some(Self { cost, executions })
    }
}

struct Sample {
    elapsed: Option<Counter>,
    cpu: Option<Counter>,
    gets: Option<Counter>,
}

impl Sample {
    fn from(awr: &AWR, id: &str) -> Result<Self, String> {
        let mut series = awr.sql_elapsed_time.iter().filter(|s| s.sql_id == id);
        let elapsed = if available(awr, "sql_elapsed_time", true) {
            let row = series.next();
            if series.next().is_some() {
                return Err(format!(
                    "Ambiguous elapsed rows for SQL {id} in snapshot {}",
                    awr.snap_info.begin_snap_id
                ));
            }
            row.and_then(|s| Counter::read(s.elapsed_time_s, s.executions))
        } else {
            None
        };
        let cpu = awr
            .sql_cpu_time
            .get(id)
            .filter(|_| available(awr, "sql_cpu_time", true))
            .and_then(|s| Counter::read(s.cpu_time_s, s.executions));
        let gets = awr
            .sql_gets
            .get(id)
            .filter(|_| available(awr, "sql_gets", true))
            .and_then(|s| Counter::read(s.buffer_gets, s.executions));
        Ok(Self { elapsed, cpu, gets })
    }
}

#[derive(Default)]
struct Totals {
    seen: usize,
    cost: f64,
    executions: u64,
    zero: usize,
}

impl Totals {
    fn add(&mut self, row: Counter) -> Result<(), String> {
        self.cost += row.cost;
        self.executions = self
            .executions
            .checked_add(row.executions)
            .ok_or("Execution total overflow")?;
        if !self.cost.is_finite() {
            return Err("Measurement total overflow".into());
        }
        self.seen += 1;
        self.zero += usize::from(row.executions == 0);
        Ok(())
    }
    fn average(&self) -> Option<f64> {
        (self.executions > 0).then(|| self.cost / self.executions as f64)
    }
    fn hours(&self) -> Option<f64> {
        (self.seen > 0).then(|| self.cost / 3600.0)
    }
    fn count(&self) -> Option<u64> {
        (self.seen > 0).then_some(self.executions)
    }
}

fn coverage(seen: usize, total: usize) -> String {
    format!("{seen} / {total}")
}

fn sql_summary(
    data: &AWRSCollection,
    spec: &Value,
    lang: ReportLanguage,
) -> Result<Vec<Value>, String> {
    let id = text(spec, "sql_id")?;
    let label = text(spec, "label")?;
    let instance = text(spec, "instance_label")?;
    let t = |en, pl| lang.choose(en, pl);
    let mut cpu = Totals::default();
    let mut elapsed = Totals::default();
    let mut common_cpu = Totals::default();
    let mut common_elapsed = Totals::default();
    let include_gets = spec["include_gets"].as_bool() == Some(true);
    let mut matched_cpu = Totals::default();
    let mut matched_elapsed = Totals::default();
    let mut matched_gets = Totals::default();
    for awr in &data.awrs {
        let s = Sample::from(awr, id)?;
        if let Some(row) = s.cpu {
            cpu.add(row)?;
        }
        if let Some(row) = s.elapsed {
            elapsed.add(row)?;
        }
        if let (Some(c), Some(e)) = (s.cpu, s.elapsed) {
            if c.executions == e.executions {
                common_cpu.add(c)?;
                common_elapsed.add(e)?;
                if let Some(g) = s
                    .gets
                    .filter(|g| include_gets && g.executions == c.executions && c.executions > 0)
                {
                    matched_cpu.add(c)?;
                    matched_elapsed.add(e)?;
                    matched_gets.add(g)?;
                }
            }
        }
    }
    let total = data.awrs.len();
    if include_gets {
        return Ok(vec![json!({
            "group":"sql_cpu_elapsed_gets",
            "title":t("Work per execution", "Ile pracy przypada na jedno wykonanie?"),
            "description":t("Only intervals present in TOP elapsed, TOP CPU and TOP buffer gets with the same positive execution count. Averages are weighted by executions.",
                "Tylko interwały z jednakową, dodatnią liczbą wykonań na trzech listach TOP (elapsed, CPU i odczyty logiczne). Średnie uwzględniają liczbę wykonań."),
            "columns":[t("Operation / instance","Operacja / instancja"),"SQL_ID",t("Matched / studied intervals","Wspólne / badane interwały"),t("Executions","Wykonania"),"Elapsed [h]","CPU [h]",t("Elapsed / exec. [s]","Elapsed / wyk. [s]"),t("CPU / exec. [s]","CPU / wyk. [s]"),t("Logical reads / exec.","Odczyty logiczne / wyk."),t("Total logical reads [billion]","Odczyty logiczne razem [mld]")],
            "rows":[[format!("{label} · {instance}"),id,coverage(matched_cpu.seen,total),matched_cpu.count(),matched_elapsed.hours(),matched_cpu.hours(),matched_elapsed.average(),matched_cpu.average(),matched_gets.average(),(matched_gets.seen > 0).then_some(matched_gets.cost / 1e9)]],
            "note":""
        })]);
    }
    if spec["no_per_execution"].as_bool() == Some(true) {
        let columns = [
            t("AWR list", "Lista AWR"),
            t("Intervals in TOP / studied", "Interwały w TOP / badane"),
            t(
                "Executions in these intervals",
                "Wykonania w tych interwałach",
            ),
            t("Accumulated time [h]", "Czas skumulowany [h]"),
        ];
        let note = if common_cpu.seen > 0 {
            t(
                "The common sample has {n} intervals, including {z} with zero change in the execution counter.",
                "Wspólna próba obejmuje {n} interwałów, w tym {z} z zerowym przyrostem licznika wykonań.",
            )
            .replace("{n}", &common_cpu.seen.to_string())
            .replace("{z}", &common_cpu.zero.to_string())
        } else {
            String::new()
        };
        return Ok(vec![
            json!({"title":label,"description":format!("{instance} · SQL {id}"),"columns":columns,
            "rows":[["TOP elapsed",coverage(elapsed.seen,total),elapsed.count(),elapsed.hours()],["TOP CPU",coverage(cpu.seen,total),cpu.count(),cpu.hours()]],"note":note}),
        ]);
    }
    let note = if common_cpu.zero > 0 {
        let boundary=t("{n} matched intervals recorded no change in the execution counter. Per-execution values are ratios of sums across the whole matched sample.",
          "W {n} wspólnych interwałach licznik wykonań pozostał bez zmiany. Koszt jednostkowy jest ilorazem sum dla całej wspólnej próby.").replace("{n}",&common_cpu.zero.to_string());
        format!("{label} ({instance}, {id}). {boundary}")
    } else {
        String::new()
    };
    Ok(vec![
        json!({"group":"sql_pair","title":t("Execution cost on a matched sample","Koszt wykonania we wspólnej próbie"),
        "description":t("Intervals present in both TOP CPU and TOP elapsed with equal execution counters.","Interwały ze zgodną liczbą wykonań w TOP CPU i TOP elapsed."),
        "columns":[t("Operation / instance","Operacja / instancja"),"SQL_ID",t("Common / studied","Wspólne / badane interwały"),t("Executions","Wykonania"),"Elapsed [h]","CPU [h]",t("Elapsed / exec. [s]","Elapsed / wyk. [s]"),t("CPU / exec. [s]","CPU / wyk. [s]")],
        "rows":[[format!("{label} · {instance}"),id,coverage(common_cpu.seen,total),common_cpu.count(),common_elapsed.hours(),common_cpu.hours(),common_elapsed.average(),common_cpu.average()]],"note":note}),
    ])
}

#[derive(Default)]
struct FamilyCosts {
    total: Totals,
    cpu: Totals,
    gets: Totals,
}

impl FamilyCosts {
    fn record(&mut self, cpu: Counter, gets: Option<Counter>) -> Result<(), String> {
        self.total.add(cpu)?;
        if let Some(gets) = gets {
            self.cpu.add(cpu)?;
            self.gets.add(gets)?;
        }
        Ok(())
    }
}

struct Family<'a> {
    costs: FamilyCosts,
    members: BTreeMap<&'a str, FamilyCosts>,
}

fn read_family<'a>(data: &'a AWRSCollection, spec: &Value) -> Result<Family<'a>, String> {
    let prefix = text(spec, "module_prefix")?.to_ascii_uppercase();
    let needle = text(spec, "text_contains")?.to_ascii_uppercase();
    let matching: BTreeSet<_> = data
        .sql_text
        .iter()
        .filter(|(_, s)| s.to_ascii_uppercase().contains(&needle))
        .map(|(id, _)| id.as_str())
        .collect();
    let mut family = Family {
        costs: FamilyCosts::default(),
        members: BTreeMap::new(),
    };
    for a in &data.awrs {
        if !available(a, "sql_cpu_time", true) {
            continue;
        }
        // Sort before accumulation so frozen editions can be reproduced on another process.
        for &id in &matching {
            let Some(c) = a.sql_cpu_time.get(id) else {
                continue;
            };
            if !c.sql_module.to_ascii_uppercase().starts_with(&prefix) {
                continue;
            }
            let Some(cpu) = Counter::read(c.cpu_time_s, c.executions) else {
                continue;
            };
            let gets = a
                .sql_gets
                .get(id)
                .filter(|g| {
                    available(a, "sql_gets", true)
                        && g.executions == c.executions
                        && c.executions > 0
                })
                .and_then(|g| Counter::read(g.buffer_gets, g.executions));
            family.costs.record(cpu, gets)?;
            family.members.entry(id).or_default().record(cpu, gets)?;
        }
    }
    Ok(family)
}

pub(super) fn family_members(
    project: &ProjectData,
    spec: &Value,
    lang: ReportLanguage,
) -> Result<Vec<Value>, String> {
    let family = read_family(&project.collection, spec)?;
    let mut members: Vec<_> = family.members.into_iter().collect();
    members.sort_by(|(ka, a), (kb, b)| {
        b.total
            .cost
            .total_cmp(&a.total.cost)
            .then_with(|| ka.cmp(kb))
    });
    let rows: Vec<_> = members
        .into_iter()
        .map(|(id, s)| {
            json!([
                id,
                s.total.seen,
                s.total.hours(),
                s.cpu.seen,
                s.cpu.count(),
                s.cpu.average(),
                s.gets.average().map(|n| n / 1e6)
            ])
        })
        .collect();
    let t = |en, pl| lang.choose(en, pl);
    Ok(vec![
        json!({"title":t("SQL statements in the measured family","Zapytania należące do badanej rodziny"),"description":format!("{} · {}",text(spec,"instance_label")?,text(spec,"text_contains")?),
    "columns":["SQL_ID",t("Intervals in TOP CPU","Interwały w TOP CPU"),"TOP CPU [h]",t("Matched intervals","Zgodne interwały"),t("Executions in matched intervals","Wykonania w zgodnych interwałach"),t("CPU / exec. [s]","CPU / wyk. [s]"),t("Logical reads / exec. [M]","Odczyty logiczne / wyk. [mln]")],"rows":rows}),
    ])
}

fn sql_family(
    data: &AWRSCollection,
    spec: &Value,
    lang: ReportLanguage,
) -> Result<Vec<Value>, String> {
    let family = read_family(data, spec)?;
    let statements = family.members;
    let FamilyCosts {
        total: all,
        cpu: paired_cpu,
        gets: paired_gets,
    } = family.costs;
    let t = |en, pl| lang.choose(en, pl);
    Ok(vec![
        json!({"group":"sql_family","title":text(spec,"label")?,
        "description":t("Membership uses the named module prefix and the object string in captured SQL text. Unit costs use SQL/interval pairs with matching positive executions in TOP CPU and TOP buffer gets.",
        "Do rodziny należą zapytania spełniające warunek modułu i tekstu SQL. Koszt jednostkowy liczymy na parach SQL–interwał ze zgodną, dodatnią liczbą wykonań w TOP CPU i TOP buffer gets."),
        "columns":[t("Instance","Instancja"),t("SQL IDs in TOP CPU","SQL_ID w TOP CPU"),t("Total TOP CPU [h]","Łączny TOP CPU [h]"),t("Matched SQL/interval pairs","Zgodne pary SQL–interwał"),t("Executions in pairs","Wykonania w parach"),t("CPU / exec. [s]","CPU / wyk. [s]"),t("Logical reads / exec. [M]","Odczyty logiczne / wyk. [mln]")],
        "rows":[[text(spec,"instance_label")?,statements.len(),all.hours(),paired_cpu.seen,paired_cpu.count(),paired_cpu.average(),paired_gets.average().map(|x|x/1_000_000.0)]],
        "note":format!("{} {} · {} {}",t("Module starts with","Początek nazwy modułu:"),text(spec,"module_prefix")?,t("SQL text contains","Fragment tekstu SQL:"),text(spec,"text_contains")?)}),
    ])
}

fn sql_intervals(
    data: &AWRSCollection,
    spec: &Value,
    lang: ReportLanguage,
) -> Result<Vec<Value>, String> {
    let picks = spec["begin_snap_ids"]
        .as_array()
        .filter(|r| !r.is_empty() && r.len() <= 64)
        .ok_or("Specify 1 to 64 beginning snapshot IDs")?;
    let mut ids = BTreeSet::new();
    for snap in picks {
        if !ids.insert(snap.as_u64().ok_or("Invalid beginning snapshot ID")?) {
            return Err("Repeated beginning snapshot ID".into());
        }
    }
    let id = text(spec, "sql_id")?;
    let t = |en, pl| lang.choose(en, pl);
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for a in &data.awrs {
        let begin_id = a.snap_info.begin_snap_id;
        if !ids.contains(&begin_id) {
            continue;
        }
        if !seen.insert(begin_id) {
            return Err("Beginning snapshot ID is ambiguous within this capture".into());
        }
        let s = Sample::from(a, id)?;
        let fmt = |v: &str| {
            timestamp(v)
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .ok_or_else(|| "Unrecognized snapshot timestamp".to_owned())
        };
        // Keep the denominators of each source separate even when they happen to agree.
        let time = format!(
            "{} – {}",
            fmt(&a.snap_info.begin_snap_time)?,
            fmt(&a.snap_info.end_snap_time)?
        );
        let gets = s
            .gets
            .filter(|g| s.cpu.is_some_and(|c| c.executions == g.executions));
        rows.push(json!([
            time,
            s.elapsed.map(|r| r.executions),
            s.elapsed.map(|r| r.cost),
            s.cpu.map(|r| r.executions),
            s.cpu.map(|r| r.cost),
            gets.map(|r| r.cost / 1_000_000.0)
        ]));
    }
    if rows.len() != ids.len() {
        return Err("Selected snapshot IDs must each identify exactly one scoped interval".into());
    }
    let comparable = rows.len() >= 2
        && rows
            .iter()
            .all(|row| row[1].as_u64() == Some(1) && row[3].as_u64() == Some(1));
    let mut table = json!({"title":text(spec,"label")?,"description":format!("{} · SQL {id}",text(spec,"instance_label")?),
        "columns":[t("AWR interval","Interwał AWR"),t("Exec. TOP elapsed","Wyk. TOP elapsed"),"Elapsed [s]",t("Exec. TOP CPU","Wyk. TOP CPU"),"CPU [s]",t("Logical reads [M]","Odczyty logiczne [mln]")],"rows":rows});
    if comparable {
        table["plot"] = json!({"kind":"interval_cpu","title":t("Elapsed and CPU in intervals with one execution","Czas wykonania i CPU w interwałach z jednym wykonaniem"),"unit":"s","label_column":0,"executions_column":1,"series_columns":[2,4],"series_labels":["Elapsed","CPU"]});
    }
    Ok(vec![table])
}

pub(super) fn tables(
    project: &ProjectData,
    spec: &Value,
    lang: ReportLanguage,
) -> Result<Vec<Value>, String> {
    let data = &project.collection;
    match text(spec, "kind")? {
        "sql_summary" => sql_summary(data, spec, lang),
        "sql_family" => sql_family(data, spec, lang),
        "sql_intervals" => sql_intervals(data, spec, lang),
        _ => Err("Unknown SQL measurement kind".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awr::{SQLCPUTime, SQLElapsedTime, SQLGets};

    fn capture() -> AWRSCollection {
        let mut data = AWRSCollection {
            db_instance_information: Default::default(),
            initialization_parameters: Default::default(),
            awrs: Vec::new(),
            sql_text: Default::default(),
            nmon: None,
        };
        let id = "0abc000000001";
        data.sql_text
            .insert(id.to_owned(), "select value from sample_orders".into());
        for (i, (cpu, e, executions, elapsed_ex, gets_ex)) in [
            (20., 30., 2, 2, 2),
            (10., 25., 0, 0, 0),
            (75., 80., 3, 4, 4),
        ]
        .into_iter()
        .enumerate()
        {
            let mut a = AWR::default();
            a.snap_info.begin_snap_id = i as u64 + 10;
            a.snap_info.begin_snap_time = format!("2026-01-01 0{i}:00:00");
            a.snap_info.end_snap_time = format!("2026-01-01 0{}:00:00", i + 1);
            a.sql_cpu_time.insert(
                id.to_owned(),
                SQLCPUTime {
                    sql_id: id.into(),
                    cpu_time_s: cpu,
                    executions,
                    sql_module: "job_abc".into(),
                    ..Default::default()
                },
            );
            a.sql_elapsed_time.push(SQLElapsedTime {
                sql_id: id.into(),
                elapsed_time_s: e,
                executions: elapsed_ex,
                ..Default::default()
            });
            a.sql_gets.insert(
                id.to_owned(),
                SQLGets {
                    buffer_gets: 8e6,
                    executions: gets_ex,
                    ..Default::default()
                },
            );
            data.awrs.push(a);
        }
        data
    }

    #[test]
    fn matched_sql_counts_include_cost_in_zero_delta_intervals() {
        let sample = capture();
        let spec =
            json!({"sql_id":"0abc000000001","label":"Operation","instance_label":"Instance 1"});
        let lang = ReportLanguage::new("en");
        let t = sql_summary(&sample, &spec, lang).unwrap();
        let row = &t[0]["rows"][0];
        assert_eq!(row[2], "2 / 3");
        assert_eq!(row[3], 2);
        assert_eq!(row[6], 27.5);
        assert_eq!(row[7], 15.0);
        let mut with_gets = spec.clone();
        with_gets["include_gets"] = json!(true);
        let t = sql_summary(&sample, &with_gets, lang).unwrap();
        let row = &t[0]["rows"][0];
        assert_eq!(row[2], "1 / 3");
        assert_eq!(row[3], 2);
        assert_eq!(row[7], 10.0);
        assert_eq!(row[8], 4_000_000.0);
        assert_eq!(row[9], 0.008);
        let t=sql_family(&sample,&json!({"label":"Family","module_prefix":"JOB_","text_contains":"SAMPLE_ORDERS","instance_label":"Instance 1"}),lang).unwrap();
        let row = &t[0]["rows"][0];
        assert_eq!(row[1], 1);
        assert_eq!(row[3], 1);
        assert_eq!(row[4], 2);
        assert_eq!(row[5], 10.0);
        assert_eq!(row[6], 4.0);
    }
    #[test]
    fn accumulated_times_only_describe_a_nonempty_matched_sample() {
        let mut sample = capture();
        let spec = json!({"sql_id":"0abc000000001","label":"Operation","instance_label":"Instance 1","no_per_execution":true});
        for (language, zero_delta) in [("en", "including 1"), ("pl", "w tym 1")] {
            let tables = sql_summary(&sample, &spec, ReportLanguage::new(language)).unwrap();
            let note = tables[0]["note"].as_str().unwrap();
            assert!(note.contains('2') && note.contains(zero_delta));
        }

        // The last interval still has both TOP rows, but their execution counts differ.
        for awr in &mut sample.awrs[..2] {
            awr.sql_cpu_time.clear();
        }
        for language in ["en", "pl"] {
            let tables = sql_summary(&sample, &spec, ReportLanguage::new(language)).unwrap();
            assert_eq!(tables[0]["rows"][0][1], "3 / 3");
            assert_eq!(tables[0]["rows"][1][1], "1 / 3");
            assert_eq!(tables[0]["note"], "");
        }
    }

    #[test]
    fn missing_or_ambiguous_sources_do_not_give_false_measurements() {
        let mut sample = capture();
        let spec = json!({"sql_id":"0abc000000001","label":"Operation","instance_label":"Instance 1","no_per_execution":true});
        let lang = ReportLanguage::new("en");
        for a in &mut sample.awrs {
            a.data_availability.insert("sql_cpu_time".into(), false);
        }
        let t = sql_summary(&sample, &spec, lang).unwrap();
        let cpu = &t[0]["rows"][1];
        assert_eq!(cpu[1], "0 / 3");
        assert!(cpu[2].is_null() && cpu[3].is_null());
        assert_eq!(t[0]["note"], "");
        sample.awrs[1].snap_info.begin_snap_id = 10;
        let mut selected = spec.clone();
        selected["begin_snap_ids"] = json!([10, 11]);
        assert!(sql_intervals(&sample, &selected, lang).is_err());
        let duplicate = sample.awrs[0].sql_elapsed_time[0].clone();
        sample.awrs[0].sql_elapsed_time.push(duplicate);
        assert!(sql_summary(&sample, &spec, lang).is_err());
    }
}
