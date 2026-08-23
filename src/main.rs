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

mod ai_tools;
mod analyze;
mod anomalies;
mod awr;
mod degradation;
mod gradient;
mod local_agent;
mod macros;
mod mcp_server;
mod reasonings;
mod staticdata;
mod tools;

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

///This tool will parse STATSPACK or AWR report into JSON format which can be used by visualization tool of your choice.
///The assumption is that text file is a STATSPACK report and HTML is AWR, but it tries to parse AWR report also.
/// It was tested only against 19c reports
/// The tool is under development and it has a lot of bugs, so please test it and don't hasitate to suggest some code changes :)
#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None, verbatim_doc_comment)]
struct Args {
    ///Parse a single text or html file
    #[clap(long, default_value = "")]
    file: String,

    ///Parse a directory of report files. Repeat with --mcp to load multiple projects.
    #[clap(short, long, value_name = "DIRECTORY")]
    directory: Vec<String>,

    ///Write output to nondefault file? Default is directory_name.json
    #[clap(short, long, default_value = "")]
    outfile: String,

    ///Ratio of DB CPU / DB TIME
    #[clap(short, long, default_value_t = 0.666)]
    time_cpu_ratio: f64,

    ///Filter only for DBTIME greater than (if zero the filter is not effective)
    #[clap(short, long, default_value_t = 0.0)]
    filter_db_time: f64,

    ///Include indicated SQL_IDs as TOP SQL in fomrat SQL_ID1, SQL_ID2,...
    ///This is experimental function
    #[clap(short, long, default_value = "", verbatim_doc_comment)]
    id_sqls: String,

    ///Analyze a JSON file. Repeat with --mcp to load multiple projects.
    #[clap(short, long, value_name = "JSON_FILE")]
    json_file: Vec<String>,

    ///Filter snapshots, based on SNAP IDs in format BEGIN_ID- END_ID
    #[clap(short, long, default_value = "0-666666666")]
    snap_range: String,

    ///Should I be quiet? This mode suppresses terminal output but still writes to log file
    #[clap(short, long)]
    quiet: bool,

    ///Use AI model to interpret collected statistics and describe them.
    ///Environment variable [OPENAI_API_KEY | GEMINI_API_KEY | OPENROUTER_API_KEY | LOCAL_API_KEY] should be set to your personal API key
    ///The parameter should be set to the value in format: VENDOR:MODEL_NAME:LANGUAGE_CODE (for example openai:gpt-4-turbo:PL or google:gemini-2.0-flash:PL)
    /// Currently supported vendors are:
    ///		- openai
    ///		- google
    ///		- openrouter
    ///		- local - for local models served by LM Studio; always uses the two-session tools workflow
    #[clap(short, long, default_value = "", verbatim_doc_comment)]
    ai: String,

    ///TOPn for retaining anomalies detected using MAD
    #[clap(short, long, default_value_t = 10)]
    mad_top: usize,

    ///Window size for detecting anomalies using MAD for local sliding window specified as % of probes
    #[clap(short = 'W', long, default_value_t = 100)]
    mad_window_size: usize,

    /// Keep only top N largest anomaly clusters in the summary.
    /// A cluster is one snapshot date grouped across anomaly categories.
    /// 0 means no cluster trimming.
    #[arg(short = 'T', long, default_value_t = 0)]
    pub top_cluster_anomalies: usize,

    ///Parallelism level
    #[clap(short = 'P', long, default_value_t = 4)]
    parallel: usize,

    ///Security level:
    ///		0 - JAS-MIN will not store any object names, database names or any other sensitive data
    ///		1 - JAS-MIN will store segment_names from Segment Statistics section
    ///		2 - JAS-MIN will store Full SQL Text from AWR reports
    #[clap(short = 'S', long, default_value_t = 0, verbatim_doc_comment)]
    security_level: usize,

    ///This can be used with Gemini models - Using the URL context tool, you can provide Gemini with URLs as additional context for your prompt. The model can then retrieve content from the URLs and use that content to inform and shape its response.
    ///Check Google Documentation for more info: https://ai.google.dev/gemini-api/docs/url-context
    #[clap(short, long, default_value = "", verbatim_doc_comment)]
    url_context_file: String,

    ///Token budget for AI analysis; for local models this is the configured context ceiling
    #[clap(short = 'B', long, default_value_t = 256000)]
    tokens_budget: usize,

    ///For calculating gradient - ridge_lambda: L2 regularization strength (>= 0)
    #[clap(short = 'R', long, default_value_t = 0.05)]
    ridge_lambda: f64,

    ///For calculating gradient - fixed Elastic Net regularization strength (>= 0).
    ///When omitted, lambda is selected automatically using forward-chaining validation.
    #[clap(short = 'E', long, verbatim_doc_comment)]
    en_lambda: Option<f64>,

    ///For calculating gradient - mixing between L1 and L2 in Elastic Net:
    ///     alpha = 1.0 -> Lasso (pure L1)
    ///     alpha = 0.0 -> Ridge-like (pure L2)
    #[clap(short = 'A', long, default_value_t = 0.2, verbatim_doc_comment)]
    en_alpha: f64,

    ///Max iterations for coordinate descent in Elastic Net
    #[clap(short = 'I', long, default_value_t = 5000)]
    en_max_iter: usize,

    ///Convergence tolerance for coefficient change in Elastic Net
    #[clap(long, default_value_t = 1e-6)]
    en_tol: f64,

    /// Keep only top N results per regression model.
    #[arg(long, default_value_t = 10)]
    pub top_gradient: usize,

    ///Convert existing markdown file to HTML without calling AI model
    #[clap(short, long, default_value = "", verbatim_doc_comment)]
    convert_md2html: String,

    ///Build customer gradient analyze for given SQL_ID or wait event
    /// Usage: SQL=0zv508wsas63c
    ///        EVENT='log file sync'
    #[clap(short = 'G', long, default_value = "", verbatim_doc_comment)]
    gradient_custom: String,

    /// Enable TOOLS mode for cloud AI providers; local mode always uses tools
    #[arg(long, default_value_t = false)]
    pub tools_mode: bool,

    /// Maximum number of tool-call iterations
    #[arg(long, default_value_t = 10)]
    pub max_tool_iterations: usize,

    /// Start a loopback Streamable HTTP MCP server after parsing.
    /// Example: --mcp 127.0.0.1:4242/mcp
    #[arg(long, value_name = "ADDRESS/PATH")]
    pub mcp: Option<McpEndpoint>,
}

impl Args {
    /// Returns the active directory for code paths that process one project.
    fn directory(&self) -> &str {
        self.directory.first().map(String::as_str).unwrap_or("")
    }

    /// Returns the active JSON file for code paths that process one project.
    fn json_file(&self) -> &str {
        self.json_file.first().map(String::as_str).unwrap_or("")
    }

    fn for_directory(&self, directory: String) -> Self {
        let mut project_args = self.clone();
        project_args.directory = vec![directory];
        project_args.json_file.clear();
        project_args
    }

    fn for_json_file(&self, json_file: String) -> Self {
        let mut project_args = self.clone();
        project_args.directory.clear();
        project_args.json_file = vec![json_file];
        project_args
    }
}

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
    let project_source_count = args.directory.len() + args.json_file.len();

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
        let parsed = awr::parse_awr_dir(project_args, &mut report_links, &json_output);
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
        let parsed = awr::prarse_json_file(project_args, &mut report_links);
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
            let parsed = awr::parse_awr_dir(args.clone(), events_sqls, &fname);
            report_for_ai = parsed.report_for_ai;
        } else {
            eprintln!("ERROR: Directory: '{}' does not exists!", args.directory());
            std::process::exit(2);
        }
    } else if !args.json_file().is_empty() {
        debug_note!("Entering JSON analysis mode: file='{}'", args.json_file());
        if PathBuf::from(args.json_file()).exists() {
            let parsed = awr::prarse_json_file(args.clone(), events_sqls);
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

    let j = rounded_json_for_toon(serde_json::to_value(&report_for_ai).unwrap());
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
            openai_gpt(
                &reportfile,
                vendor_model_lang,
                events_sqls.clone(),
                &args,
                &toon_str,
            )
            .unwrap();
        } else if vendor_model_lang[0] == "google" {
            gemini(
                &reportfile,
                vendor_model_lang,
                events_sqls.clone(),
                &args,
                &toon_str,
            )
            .unwrap();
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
                    } else {
                        convert_md_to_html_file(
                            &format!("{reportfile}.final.md"),
                            events_sqls.clone(),
                        );
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
        convert_md_to_html_file(&args.convert_md2html, events_sqls.clone());
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
