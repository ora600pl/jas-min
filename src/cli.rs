use crate::mcp_server::McpEndpoint;
use clap::Parser;
use std::path::PathBuf;

///This tool will parse STATSPACK or AWR report into JSON format which can be used by visualization tool of your choice.
///The assumption is that text file is a STATSPACK report and HTML is AWR, but it tries to parse AWR report also.
/// It was tested only against 19c reports
/// The tool is under development and it has a lot of bugs, so please test it and don't hasitate to suggest some code changes :)
#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None, verbatim_doc_comment)]
pub(crate) struct Args {
    ///Parse a single text or html file
    #[clap(long, default_value = "")]
    pub(crate) file: String,

    ///Parse a directory of report files. Repeat with --mcp to load multiple projects.
    #[clap(short, long, value_name = "DIRECTORY")]
    pub(crate) directory: Vec<String>,

    ///Write output to nondefault file? Default is directory_name.json
    #[clap(short, long, default_value = "")]
    pub(crate) outfile: String,

    ///Ratio of DB CPU / DB TIME
    #[clap(short, long, default_value_t = 0.666)]
    pub(crate) time_cpu_ratio: f64,

    ///Filter only for DBTIME greater than (if zero the filter is not effective)
    #[clap(short, long, default_value_t = 0.0)]
    pub(crate) filter_db_time: f64,

    ///Include indicated SQL_IDs as TOP SQL in fomrat SQL_ID1, SQL_ID2,...
    ///This is experimental function
    #[clap(short, long, default_value = "", verbatim_doc_comment)]
    pub(crate) id_sqls: String,

    ///Analyze a JSON file. Repeat with --mcp to load multiple projects.
    #[clap(short, long, value_name = "JSON_FILE")]
    pub(crate) json_file: Vec<String>,

    ///Filter snapshots, based on SNAP IDs in format BEGIN_ID- END_ID
    #[clap(short, long, default_value = "0-666666666")]
    pub(crate) snap_range: String,

    ///Optional JSON policy for deterministic HINTS (thresholds and explicit baseline).
    #[clap(long, default_value = "")]
    pub(crate) hints_policy: String,

    ///Should I be quiet? This mode suppresses terminal output but still writes to log file
    #[clap(short, long)]
    pub(crate) quiet: bool,

    ///Use AI model to interpret collected statistics and describe them.
    ///Environment variable [OPENAI_API_KEY | GEMINI_API_KEY | OPENROUTER_API_KEY | LOCAL_API_KEY] should be set to your personal API key
    ///The parameter should be set to the value in format: VENDOR:MODEL_NAME:LANGUAGE_CODE (for example openai:gpt-4-turbo:PL or google:gemini-2.0-flash:PL)
    /// Currently supported vendors are:
    ///		- openai
    ///		- google
    ///		- openrouter
    ///		- local - for local models served by LM Studio; always uses the two-session tools workflow
    #[clap(short, long, default_value = "", verbatim_doc_comment)]
    pub(crate) ai: String,

    ///TOPn for retaining anomalies detected using MAD
    #[clap(short, long, default_value_t = 10)]
    pub(crate) mad_top: usize,

    ///Window size for detecting anomalies using MAD for local sliding window specified as % of probes
    #[clap(short = 'W', long, default_value_t = 100)]
    pub(crate) mad_window_size: usize,

    /// Keep only top N largest anomaly clusters in the summary.
    /// A cluster is one snapshot date grouped across anomaly categories.
    /// 0 means no cluster trimming.
    #[arg(short = 'T', long, default_value_t = 0)]
    pub top_cluster_anomalies: usize,

    ///Parallelism level
    #[clap(short = 'P', long, default_value_t = 4)]
    pub(crate) parallel: usize,

    ///Security level:
    ///		0 - JAS-MIN will not store any object names, database names or any other sensitive data
    ///		1 - JAS-MIN will store segment_names from Segment Statistics section
    ///		2 - JAS-MIN will store Full SQL Text from AWR reports
    #[clap(short = 'S', long, default_value_t = 0, verbatim_doc_comment)]
    pub(crate) security_level: usize,

    ///This can be used with Gemini models - Using the URL context tool, you can provide Gemini with URLs as additional context for your prompt. The model can then retrieve content from the URLs and use that content to inform and shape its response.
    ///Check Google Documentation for more info: https://ai.google.dev/gemini-api/docs/url-context
    #[clap(short, long, default_value = "", verbatim_doc_comment)]
    pub(crate) url_context_file: String,

    ///Token budget for AI analysis; for local models this is the configured context ceiling
    #[clap(short = 'B', long, default_value_t = 256000)]
    pub(crate) tokens_budget: usize,

    ///For calculating gradient - ridge_lambda: L2 regularization strength (>= 0)
    #[clap(short = 'R', long, default_value_t = 0.05)]
    pub(crate) ridge_lambda: f64,

    ///For calculating gradient - fixed Elastic Net regularization strength (>= 0).
    ///When omitted, lambda is selected automatically using forward-chaining validation.
    #[clap(short = 'E', long, verbatim_doc_comment)]
    pub(crate) en_lambda: Option<f64>,

    ///For calculating gradient - mixing between L1 and L2 in Elastic Net:
    ///     alpha = 1.0 -> Lasso (pure L1)
    ///     alpha = 0.0 -> Ridge-like (pure L2)
    #[clap(short = 'A', long, default_value_t = 0.2, verbatim_doc_comment)]
    pub(crate) en_alpha: f64,

    ///Max iterations for coordinate descent in Elastic Net
    #[clap(short = 'I', long, default_value_t = 5000)]
    pub(crate) en_max_iter: usize,

    ///Convergence tolerance for coefficient change in Elastic Net
    #[clap(long, default_value_t = 1e-6)]
    pub(crate) en_tol: f64,

    /// Select top N active, peak and extreme candidates per model; retain full rankings.
    #[arg(long, default_value_t = 10)]
    pub top_gradient: usize,

    ///Convert existing markdown file to HTML without calling AI model
    #[clap(short, long, default_value = "", verbatim_doc_comment)]
    pub(crate) convert_md2html: String,

    ///Build customer gradient analyze for given SQL_ID or wait event
    /// Usage: SQL=0zv508wsas63c
    ///        EVENT='log file sync'
    #[clap(short = 'G', long, default_value = "", verbatim_doc_comment)]
    pub(crate) gradient_custom: String,

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

    ///Optional directory containing one or more IBM AIX/Linux *.nmon captures.
    ///NMON data is parsed once and embedded in the generated JAS-MIN dataset.
    #[arg(long, value_name = "DIRECTORY", verbatim_doc_comment)]
    pub nmon: Option<PathBuf>,
}

impl Args {
    /// Returns the active directory for code paths that process one project.
    pub(crate) fn directory(&self) -> &str {
        self.directory.first().map(String::as_str).unwrap_or("")
    }

    /// Returns the active JSON file for code paths that process one project.
    pub(crate) fn json_file(&self) -> &str {
        self.json_file.first().map(String::as_str).unwrap_or("")
    }

    pub(crate) fn for_directory(&self, directory: String) -> Self {
        let mut project_args = self.clone();
        project_args.directory = vec![directory];
        project_args.json_file.clear();
        project_args
    }

    pub(crate) fn for_json_file(&self, json_file: String) -> Self {
        let mut project_args = self.clone();
        project_args.directory.clear();
        project_args.json_file = vec![json_file];
        project_args
    }
}
