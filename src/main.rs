#![allow(dead_code, unused)]
use clap::Parser;
use colored::*;
use dotenvy::from_path;
use rayon::ThreadPoolBuilder;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str;

mod ai;
mod analysis;
mod cli;
mod common;
mod model;
mod nmon;
mod parsing;
mod report;

// Transitional crate-level aliases keep the existing implementation paths
// stable while source files move into responsibility-based directories.
pub(crate) use ai::{ai_tools, local_agent, mcp_server, reasonings};
pub(crate) use analysis::{
    access_path, analyze, anomalies, degradation, gradient, measurements, performance_hints,
    quantile,
};
pub(crate) use cli::Args;
pub(crate) use common::{staticdata, tools};
pub(crate) use parsing::awr;
pub(crate) use report::{issues as report_issues, signals as report_signals};

use crate::local_agent::{analyze_report_local_agent, write_local_agent_outputs};
use crate::mcp_server::{
    run_mcp_server, AnalysisProject, AnalysisRuntime, McpEndpoint, MAX_MCP_PROJECTS,
};
use crate::reasonings::*;
use crate::reasonings::{
    AnomalyDescription, AnomlyCluster, IOStatsByFunctionSummary, InstanceStatisticCorrelation,
    LatchActivitySummary, LoadProfileAnomalies, MadAnomaliesEvents, MadAnomaliesSQL,
    PctOfTimesThisSQLFoundInOtherTopSections, ReportForAI, StatisticsDescription, StatsSummary,
    Top10SegmentStats, TopBackgroundWaitEvents, TopForegroundWaitEvents, TopPeaksSelected,
    TopSQLsByElapsedTime, WaitEventsFromASH, WaitEventsWithStrongCorrelation,
};
use crate::tools::*;

use toon::encode;

fn load_env() -> Result<(), String> {
    // 1.Check existense of $JASMIN_HOME
    let env_loaded = if let Ok(jasmin_home) = env::var("JASMIN_HOME") {
        let mut path = PathBuf::from(jasmin_home);
        path.push(".env");

        // 2. Check if .env exists in the directory
        if path.exists() {
            from_path(&path).map_err(|error| {
                format!("Cannot load the environment file configured by JASMIN_HOME: {error}")
            })?;
            debug_note!("Environment file loaded from JASMIN_HOME");
            true
        } else {
            false
        }
    } else {
        false
    };

    // 3. If there is no .env in JASMIN_HOME, check local dir
    if !env_loaded {
        let local_path = PathBuf::from(".env");
        if local_path.exists() {
            from_path(&local_path)
                .map_err(|error| format!("Cannot load the local environment file: {error}"))?;
            debug_note!("Environment file loaded from current directory");
        } else {
            debug_note!("No environment file found; using process environment only");
        }
    }

    Ok(())
}

fn validate_cli_inputs(args: &Args) -> Result<(), String> {
    performance_hints::Policy::load(&args.hints_policy)?;
    let project_source_count = args.directory.len() + args.json_file.len();

    if let Some(nmon) = args.nmon.as_ref() {
        if !nmon.is_dir() {
            return Err(format!(
                "NMON source '{}' is not a directory",
                nmon.display()
            ));
        }
        if project_source_count != 1 || !args.file.is_empty() {
            return Err(
                "--nmon requires exactly one --directory or --json-file project source".to_string(),
            );
        }
    }

    if args.mcp.is_some() {
        if project_source_count == 0 {
            return Err("--mcp requires at least one --directory or --json-file".to_string());
        }
        if !args.file.is_empty() {
            return Err(
                "--mcp cannot be combined with --file; use --directory or --json-file".to_string(),
            );
        }
        if !args.convert_md2html.is_empty() {
            return Err("--mcp cannot be combined with --convert-md2html".to_string());
        }
    } else {
        if project_source_count > 1 {
            return Err("repeated --directory/--json-file inputs require --mcp".to_string());
        }
        if !args.file.is_empty() && project_source_count > 0 {
            return Err("--file cannot be combined with --directory or --json-file".to_string());
        }
        if args.file.is_empty() && project_source_count == 0 && args.convert_md2html.is_empty() {
            return Err(
                "no input supplied; use --file, --directory, --json-file, or --convert-md2html"
                    .to_string(),
            );
        }
    }

    if !args.file.is_empty() && !Path::new(&args.file).is_file() {
        return Err(format!("input report '{}' is not a file", args.file));
    }
    for directory in &args.directory {
        if !Path::new(directory).is_dir() {
            return Err(format!(
                "project directory '{directory}' is not a directory"
            ));
        }
    }
    for json_file in &args.json_file {
        if !Path::new(json_file).is_file() {
            return Err(format!("project JSON source '{json_file}' is not a file"));
        }
    }
    if !args.convert_md2html.is_empty() && !Path::new(&args.convert_md2html).is_file() {
        return Err(format!(
            "Markdown source '{}' is not a file",
            args.convert_md2html
        ));
    }

    if !args.ridge_lambda.is_finite() || args.ridge_lambda < 0.0 {
        return Err("--ridge-lambda must be a finite value >= 0".to_string());
    }
    if let Some(lambda) = args.en_lambda {
        if !lambda.is_finite() || lambda < 0.0 {
            return Err("--en-lambda must be a finite value >= 0".to_string());
        }
    } else if args.en_alpha <= 0.0 {
        return Err("automatic Elastic Net lambda selection requires --en-alpha > 0; provide --en-lambda for alpha=0".to_string());
    }
    if !args.en_alpha.is_finite() || !(0.0..=1.0).contains(&args.en_alpha) {
        return Err("--en-alpha must be a finite value in [0, 1]".to_string());
    }
    if args.en_max_iter == 0 {
        return Err("--en-max-iter must be greater than 0".to_string());
    }
    if !args.en_tol.is_finite() || args.en_tol <= 0.0 {
        return Err("--en-tol must be a finite value > 0".to_string());
    }

    Ok(())
}

fn project_id_base(path: &str) -> String {
    let path = Path::new(path);
    let raw = if path.is_dir() {
        path.file_name()
    } else {
        path.file_stem().or_else(|| path.file_name())
    }
    .and_then(|value| value.to_str())
    .unwrap_or("project");
    let mut id = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    let id = id.trim_matches('-');
    if id.is_empty() {
        "project".to_string()
    } else {
        id.chars().take(80).collect()
    }
}

fn unique_project_id(path: &str, used: &mut HashSet<String>) -> String {
    let base = project_id_base(path);
    let mut candidate = base.clone();
    let mut suffix = 2;
    while used.contains(&candidate) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    used.insert(candidate.clone());
    candidate
}

fn owned_report_links(
    report_links: &HashMap<&str, HashSet<String>>,
) -> HashMap<String, HashSet<String>> {
    report_links
        .iter()
        .map(|(kind, names)| ((*kind).to_string(), names.clone()))
        .collect()
}

fn mcp_html_output_dir(source: &str, is_directory: bool) -> PathBuf {
    if is_directory {
        PathBuf::from(source).with_extension("html_reports")
    } else {
        PathBuf::from(source)
            .file_stem()
            .map(|value| PathBuf::from(value).with_extension("html_reports"))
            .unwrap_or_else(|| PathBuf::from("jas-min.html_reports"))
    }
}

fn absolute_output_key(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| format!("Cannot resolve the working directory: {error}"))?
            .join(path)
    };
    let file_name = absolute
        .file_name()
        .ok_or_else(|| format!("Invalid MCP output path '{}'", path.display()))?;
    let parent = absolute.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    Ok(canonical_parent.join(file_name))
}

fn load_mcp_projects(args: &Args) -> Result<Vec<AnalysisProject>, String> {
    let source_count = args.directory.len() + args.json_file.len();
    debug_note!(
        "Validating MCP project sources: directories={}, json_files={}, total={}",
        args.directory.len(),
        args.json_file.len(),
        source_count
    );
    if source_count == 0 {
        return Err("--mcp requires at least one --directory or --json-file".to_string());
    }
    if source_count > MAX_MCP_PROJECTS {
        return Err(format!(
            "{source_count} MCP projects were supplied; the maximum is {MAX_MCP_PROJECTS}"
        ));
    }
    if !args.file.is_empty() {
        return Err(
            "--mcp cannot be combined with --file; use --directory or --json-file".to_string(),
        );
    }
    if source_count > 1 && !args.outfile.is_empty() {
        return Err(
            "--outfile is ambiguous with multiple MCP projects; omit it or load one project"
                .to_string(),
        );
    }

    let mut canonical_sources = HashSet::new();
    for source in args.directory.iter().chain(args.json_file.iter()) {
        let canonical = fs::canonicalize(source)
            .map_err(|error| format!("Cannot access MCP project source '{source}': {error}"))?;
        if !canonical_sources.insert(canonical) {
            return Err(format!(
                "MCP project source '{source}' was provided more than once"
            ));
        }
    }

    // Classic report generation creates charts while each project is loaded. Reject
    // colliding targets before parsing so one project cannot overwrite another.
    let mut output_owners = HashMap::<PathBuf, String>::new();
    for (source, is_directory) in args
        .directory
        .iter()
        .map(|source| (source, true))
        .chain(args.json_file.iter().map(|source| (source, false)))
    {
        let output_dir = mcp_html_output_dir(source, is_directory);
        let output_key = absolute_output_key(&output_dir)?;
        if let Some(previous_source) = output_owners.insert(output_key, source.clone()) {
            return Err(format!(
                "MCP project sources '{previous_source}' and '{source}' use the same generated HTML directory '{}'; rename one input or start JAS-MIN from another working directory",
                output_dir.display()
            ));
        }
    }

    let mut used_project_ids = HashSet::new();
    let mut projects = Vec::with_capacity(source_count);

    for directory in &args.directory {
        if !Path::new(directory).is_dir() {
            return Err(format!(
                "MCP project directory '{directory}' is not a directory"
            ));
        }
        let project_args = args.for_directory(directory.clone());
        let mut report_links = HashMap::new();
        let json_output = if args.outfile.is_empty() {
            PathBuf::from(directory)
                .with_extension("json")
                .to_string_lossy()
                .into_owned()
        } else {
            args.outfile.clone()
        };
        debug_note!("Starting to parse MCP project directory: {}", directory);
        let parsed = awr::parse_awr_dir(project_args, &mut report_links, &json_output)?;
        debug_note!(
            "MCP directory project parsed: source='{}', snapshots={}, report_link_groups={}",
            directory,
            parsed.collection.awrs.len(),
            report_links.len()
        );
        projects.push(AnalysisProject::new(
            unique_project_id(directory, &mut used_project_ids),
            parsed.collection,
            parsed.report_for_ai,
            directory.clone(),
            args.security_level,
            owned_report_links(&report_links),
            PathBuf::from(directory)
                .with_extension("html_reports")
                .to_string_lossy()
                .into_owned(),
        ));
    }

    for json_file in &args.json_file {
        if !Path::new(json_file).is_file() {
            return Err(format!(
                "MCP project JSON source '{json_file}' is not a file"
            ));
        }
        let project_args = args.for_json_file(json_file.clone());
        let mut report_links = HashMap::new();
        debug_note!("Starting to load MCP project JSON: {}", json_file);
        let parsed = awr::prarse_json_file(project_args, &mut report_links)?;
        debug_note!(
            "MCP JSON project parsed: source='{}', snapshots={}, report_link_groups={}",
            json_file,
            parsed.collection.awrs.len(),
            report_links.len()
        );
        let stem = PathBuf::from(json_file)
            .with_extension("")
            .to_string_lossy()
            .into_owned();
        let html_reports_dir = mcp_html_output_dir(json_file, false)
            .to_string_lossy()
            .into_owned();
        projects.push(AnalysisProject::new(
            unique_project_id(json_file, &mut used_project_ids),
            parsed.collection,
            parsed.report_for_ai,
            stem,
            args.security_level,
            owned_report_links(&report_links),
            html_reports_dir,
        ));
    }

    debug_note!("MCP project loading completed: projects={}", projects.len());
    Ok(projects)
}

fn main() {
    let args = Args::parse();
    validate_cli_inputs(&args).unwrap_or_else(|error| {
        eprintln!("ERROR: {error}");
        std::process::exit(2);
    });
    load_env().unwrap_or_else(|error| {
        eprintln!("ERROR: {error}");
        std::process::exit(2);
    });
    let mut reportfile: String = "".to_string();
    debug_note!(
        "JAS-MIN invocation parsed: mcp={}, directories={}, json_files={}, single_file={}, ai={}, markdown_conversion={}, parallel={}",
        args.mcp.is_some(),
        args.directory.len(),
        args.json_file.len(),
        !args.file.is_empty(),
        !args.ai.is_empty(),
        !args.convert_md2html.is_empty(),
        args.parallel
    );
    if !args.quiet {
        println!(
            "{}{} (Running with parallel degree: {})",
            "JAS-MIN v".bright_yellow(),
            env!("CARGO_PKG_VERSION").bright_yellow(),
            args.parallel
        );
    }

    let mut report_for_ai = ReportForAI::default();

    //This creates a global pool configuration for rayon to limit threads for par_iter
    ThreadPoolBuilder::new()
        .num_threads(args.parallel)
        .build_global()
        .expect("Can't create rayon pool");
    debug_note!(
        "Rayon global thread pool initialized: threads={}",
        args.parallel
    );

    if let Some(endpoint) = args.mcp.clone() {
        debug_note!("Entering MCP server mode: endpoint={}", endpoint.url());
        let projects = load_mcp_projects(&args).unwrap_or_else(|error| {
            eprintln!("ERROR: {error}");
            std::process::exit(2);
        });
        let runtime = AnalysisRuntime::from_projects(projects).unwrap_or_else(|error| {
            eprintln!("ERROR: Cannot initialize MCP projects: {error:#}");
            std::process::exit(2);
        });
        if let Err(error) = run_mcp_server(runtime, endpoint) {
            debug_note!("MCP server terminated with error: {:#}", error);
            eprintln!("ERROR: MCP server failed: {error:#}");
            std::process::exit(1);
        }
        return;
    }

    //This is map that will be used to generate and insert appropriate links to html AI output
    let mut events_sqls: &mut HashMap<&str, HashSet<String>> = &mut HashMap::new();

    if !args.file.is_empty() {
        debug_note!("Entering single-report parse mode: file='{}'", args.file);
        let awr_doc = awr::parse_awr_report(&args.file, false, &args).unwrap_or_else(|error| {
            eprintln!("ERROR: Cannot parse report '{}': {error}", args.file);
            std::process::exit(1);
        });
        if !args.quiet {
            println!("{}", awr_doc);
        }
    } else if !args.directory().is_empty() {
        debug_note!(
            "Entering directory analysis mode: directory='{}'",
            args.directory()
        );
        if PathBuf::from(args.directory()).exists() {
            let mut fname = PathBuf::from(args.directory())
                .with_extension("json")
                .to_string_lossy()
                .into_owned();
            reportfile = PathBuf::from(args.directory())
                .with_extension("txt")
                .to_string_lossy()
                .into_owned();
            if !args.outfile.is_empty() {
                fname = args.outfile.clone();
            }
            debug_note!("Starting to parse directory: {}", args.directory());
            let parsed =
                awr::parse_awr_dir(args.clone(), events_sqls, &fname).unwrap_or_else(|error| {
                    eprintln!("ERROR: {error}");
                    std::process::exit(2);
                });
            report_for_ai = parsed.report_for_ai;
        } else {
            eprintln!("ERROR: Directory: '{}' does not exists!", args.directory());
            std::process::exit(2);
        }
    } else if !args.json_file().is_empty() {
        debug_note!("Entering JSON analysis mode: file='{}'", args.json_file());
        if PathBuf::from(args.json_file()).exists() {
            let parsed = awr::prarse_json_file(args.clone(), events_sqls).unwrap_or_else(|error| {
                eprintln!("ERROR: {error}");
                std::process::exit(2);
            });
            report_for_ai = parsed.report_for_ai;
            //let file_and_ext: Vec<&str> = args.json_file.split('.').collect();
            reportfile = match PathBuf::from(args.json_file()).file_stem() {
                Some(stem) => PathBuf::from(stem)
                    .with_extension("txt")
                    .to_string_lossy()
                    .into_owned(),
                None => {
                    eprintln!("Invalid filename: {}", args.json_file());
                    std::process::exit(10);
                }
            };
        } else {
            eprintln!("ERROR: JSON file: '{}' does not exists!", args.json_file());
            std::process::exit(2);
        }
    }

    fs::write(
        "report_for_ai.full.json",
        serde_json::to_vec(&report_for_ai).unwrap(),
    )
    .unwrap();
    let j = rounded_json_for_toon(gradient_prompt_value(&report_for_ai));
    let toon_str = encode(&j, None);
    if toon_str.len() > 128 {
        let mut f = fs::File::create("report_for_ai.toon").unwrap();
        f.write_all(toon_str.as_bytes()).unwrap();
        if !args.quiet {
            println!("\n🎲 The TOON file alone will consume around {} tokens. Take it under consideration if you want to use AI processing.", estimate_tokens_from_str(&toon_str));
        }
    }

    if !args.ai.is_empty() {
        let vendor_model_lang_parts = args.ai.split(":").collect::<Vec<&str>>();
        let vendor_model_lang = if vendor_model_lang_parts.len() > 3 {
            let vendor = vendor_model_lang_parts[0];
            let lang = vendor_model_lang_parts[vendor_model_lang_parts.len() - 1];
            let model = &args.ai[vendor.len() + 1..args.ai.len() - lang.len() - 1];
            vec![vendor, model, lang]
        } else {
            vendor_model_lang_parts
        };
        debug_note!(
            "Starting AI analysis: vendor='{}', model='{}', language='{}', tools_mode={}",
            vendor_model_lang.first().copied().unwrap_or(""),
            vendor_model_lang.get(1).copied().unwrap_or(""),
            vendor_model_lang.get(2).copied().unwrap_or(""),
            args.tools_mode
        );

        if vendor_model_lang[0] == "openai" {
            if let Err(error) = openai_gpt(
                &reportfile,
                vendor_model_lang,
                events_sqls.clone(),
                &args,
                &toon_str,
            ) {
                eprintln!("❌ OpenAI analysis failed: {error}");
                std::process::exit(1);
            }
        } else if vendor_model_lang[0] == "google" {
            if let Err(error) = gemini(
                &reportfile,
                vendor_model_lang,
                events_sqls.clone(),
                &args,
                &toon_str,
            ) {
                eprintln!("❌ Gemini analysis failed: {error}");
                std::process::exit(1);
            }
        } else if vendor_model_lang[0] == "openrouter" {
            if let Err(error) = openrouter(
                &reportfile,
                vendor_model_lang,
                events_sqls.clone(),
                &args,
                &toon_str,
            ) {
                eprintln!("❌ OpenRouter analysis failed: {error}");
                std::process::exit(1);
            }
        } else if vendor_model_lang[0] == "local" {
            match analyze_report_local_agent(
                &report_for_ai,
                &args,
                &reportfile,
                vendor_model_lang[1],
                vendor_model_lang[2],
            ) {
                Ok(outcome) => {
                    if let Err(e) = write_local_agent_outputs(&reportfile, &outcome) {
                        eprintln!("❌ writing local agent outputs failed: {e}");
                        std::process::exit(1);
                    } else {
                        convert_md_to_html_file(
                            &format!("{reportfile}.final.md"),
                            events_sqls.clone(),
                        )
                        .unwrap_or_else(|error| {
                            eprintln!("Report export failed: {error}");
                            std::process::exit(1);
                        });
                    }
                }
                Err(e) => {
                    eprintln!("❌ local agent analysis failed: {e}");
                    std::process::exit(1);
                }
            }
        } else {
            eprintln!("Unrecognized vendor. Supported vendors: openai, google, openrouter, local");
            std::process::exit(2);
        }
    }

    if !args.convert_md2html.is_empty() {
        convert_md_to_html_file(&args.convert_md2html, events_sqls.clone()).unwrap_or_else(
            |error| {
                eprintln!("Report export failed: {error}");
                std::process::exit(1);
            },
        );
    }
    debug_note!("JAS-MIN invocation completed");
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn clap_accepts_repeated_mcp_project_sources() {
        let args = Args::try_parse_from([
            "jas-min",
            "--mcp",
            "127.0.0.1:4242/mcp",
            "-d",
            "before",
            "--directory",
            "after",
            "-j",
            "reference.json",
        ])
        .unwrap();
        assert_eq!(args.directory, vec!["before", "after"]);
        assert_eq!(args.json_file, vec!["reference.json"]);
    }

    #[test]
    fn quiet_remains_an_opt_in_flag() {
        let default_args = Args::try_parse_from(["jas-min", "--file", "report.html"]).unwrap();
        assert!(!default_args.quiet);
        assert_eq!(default_args.ridge_lambda, 0.05);
        assert_eq!(default_args.en_lambda, None);
        assert_eq!(default_args.en_alpha, 0.2);

        let quiet_args =
            Args::try_parse_from(["jas-min", "--file", "report.html", "--quiet"]).unwrap();
        assert!(quiet_args.quiet);
    }

    #[test]
    fn elastic_net_lambda_is_an_optional_fixed_override() {
        let args =
            Args::try_parse_from(["jas-min", "--file", "report.html", "--en-lambda", "0.125"])
                .unwrap();
        assert_eq!(args.en_lambda, Some(0.125));
    }

    #[test]
    fn nmon_is_an_optional_directory_argument() {
        let without_nmon = Args::try_parse_from(["jas-min", "--file", "report.html"]).unwrap();
        assert!(without_nmon.nmon.is_none());

        let with_nmon = Args::try_parse_from([
            "jas-min",
            "--directory",
            "reports",
            "--nmon",
            "host-captures",
        ])
        .unwrap();
        assert_eq!(with_nmon.nmon, Some(PathBuf::from("host-captures")));
    }

    #[test]
    fn classic_mode_requires_an_input() {
        let args = Args::try_parse_from(["jas-min"]).unwrap();
        assert_eq!(
            validate_cli_inputs(&args).unwrap_err(),
            "no input supplied; use --file, --directory, --json-file, or --convert-md2html"
        );
    }

    #[test]
    fn project_ids_are_stable_and_collision_safe() {
        let mut used = HashSet::new();
        assert_eq!(
            unique_project_id("/tmp/Before AWR.json", &mut used),
            "before-awr"
        );
        assert_eq!(
            unique_project_id("/other/Before AWR.json", &mut used),
            "before-awr-2"
        );
    }
}

#[cfg(test)]
mod toon_regression_tests {
    use serde_json::json;

    #[test]
    fn optimized_toon_is_byte_compatible_with_upstream() {
        let strings = [
            "",
            " ",
            "true",
            "false",
            "null",
            "0",
            "05",
            "-3.14",
            "1e-6",
            "1E6",
            "a,b",
            "a:b",
            "a.b",
            "[x]",
            "{x}",
            "- item",
            "a\"b",
            "a\\b",
            "a\nb\rc\td",
            "Żółć",
            "abc",
            "a東京",
            "１２",
            "١٢",
        ];
        let mut objects = Vec::new();
        for (i, text) in strings.iter().enumerate() {
            let value = json!({*text: text, "number": i as f64 / 1000.0, "flag": i % 2 == 0, "missing": null, "nested": {"key": text}});
            assert_eq!(
                toon::encode(&value, None),
                toon_reference::encode(&value, None)
            );
            objects.push(value);
        }
        let report = json!({"hints": objects, "rows": [{"x":0,"y":"hello"},{"x":1,"y":"05"}], "strings":strings,"mixed":[null,{},[],1,true,"abc"],"empty":[],"large":1e20});
        assert_eq!(
            toon::encode(&report, None),
            toon_reference::encode(&report, None)
        );
    }
}
