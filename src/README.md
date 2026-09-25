# Source layout

The source tree is organized by responsibility while preserving the existing
JAS-MIN behavior and recognizable module names.

- `cli.rs` owns command-line arguments; `main.rs` remains the application entry point.
- `model/` contains serialized Oracle and analysis data structures.
- `parsing/` reads AWR, STATSPACK, and prepared JSON into the shared models.
- `analysis/` contains calculations and analysis orchestration.
- `report/` contains classic report generators, report contracts, interactive
  signal views, and their static assets.
- `ai/` contains the shared evidence-tool catalog plus cloud, local, and MCP
  execution modes.
- `nmon/` remains a self-contained optional NMON subsystem.
- `common/` contains statistics, formatting helpers, static metadata, and macros.

Crate-level re-exports in `main.rs` intentionally preserve legacy internal paths
while the modules are migrated. They prevent a directory-only refactor from
changing parser, analysis, or report behavior.
