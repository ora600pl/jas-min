# JAS-MIN Companion

A local plugin for Codex and Claude Code that supports Oracle performance investigations with a persistent panel: evidence charts, baseline comparisons, SQL tables, and mechanism diagrams, accompanied by a hooded avatar.

For Claude Code Desktop (local Code sessions) and the terminal, see [Claude Code installation](README-claude.md). From the repository root, register this checkout and install with:

```sh
claude plugin marketplace add "$PWD"
claude plugin install jasmin-companion@jasmin-local --scope user
```

The Codex instructions follow below. Both hosts use the same MCP server, panel and investigation skill. Claude uses its own manifest and opens the returned local panel URL in an available browser.

Discuss the investigation in text or through the host's native voice interface. Panel labels and interpretations follow the conversation: Polish for Polish dialogue, English for English dialogue. Codex passes the detected language to the plugin; the panel does not read the main chat itself. SQL, source identifiers and raw evidence stay verbatim. The plugin does not capture audio, synthesize speech, or call the OpenAI API, and requires no OpenAI API key.

## Requirements

- Codex desktop with support for local plugins and the `codex plugin` CLI commands.
- Node.js **22 or newer**, available on the desktop application's `PATH`.
- Git to clone this repository.
- For live analysis: a running JAS-MIN MCP server with an AWR/STATSPACK collection loaded. Demo mode needs no database or JAS-MIN server.

The launcher uses a POSIX shell. Installation and desktop integration have been tested on macOS with Node.js 24.18.0. Windows installation has not been validated. The package has no npm dependencies; **do not run `npm install`**.

## Install the plugin

Clone the repository, or use your existing checkout:

```sh
git clone https://github.com/ora600pl/jas-min.git
cd jas-min
node --version
node plugins/jasmin-companion/install.mjs
```

The installer registers this checkout as the local marketplace `jasmin-local`, installs `jasmin-companion@jasmin-local`, and prints the installed package location. Keep the checkout if you want to reinstall or update from it.

On macOS, the installer prefers the CLI bundled with `/Applications/ChatGPT.app` when present. Otherwise, it uses `codex` from `PATH`. To choose a specific compatible CLI:

```sh
CODEX_CLI="/absolute/path/to/codex" node plugins/jasmin-companion/install.mjs
```

**Reload Codex after installation** so that it loads the skill and MCP tools. Installation is not tied to the JAS-MIN repository: you can use Companion from another project chat.

### Try the demo

In a Codex chat, select the `jasmin-companion` skill from the skill picker, or ask:

> Use JAS-MIN Companion in demo mode. Show database load, waits at the peak, and the most expensive SQL statements. Present the related evidence together in the side panel.

Demo data is synthetic and clearly labelled. Codex opens the panel in its browser and calls the plugin's tools to update it.

If your host offers native voice, start it in the same chat. The plugin works with text when voice is unavailable. The avatar's pulse indicates idle/tool activity, not microphone or speaking state.

## Connect to real data

Build or install JAS-MIN using the [main repository instructions](../../README.md#installation). Then start its MCP server with your collection:

```sh
./target/release/jas-min \
  -j "/absolute/path/to/collection.json" \
  --security-level 2 \
  --mcp 127.0.0.1:4242/mcp
```

Keep that process running. In your investigation chat, ask:

> Open JAS-MIN Companion in live mode using http://127.0.0.1:4242/mcp. List the available projects, select my database, and show the most important workload evidence.

For a focused investigation:

> Compare the incident with a representative baseline. Show SQL activity and wait duration on aligned timelines, separate RAC instances, and illustrate the proposed mechanism. Clearly distinguish measurements from hypotheses.

The plugin discovers project IDs and available tool schemas from JAS-MIN. Selecting a project creates an analysis and starts a guided audit when the server supports it. The evidence tool allowlist includes plans, child cursors, alert logs, segments, parameters and analytic signals, so each chapter can follow the actual evidence. Controlled audit tools save explanations and navigate chapters; arbitrary report-writing tools are not exposed through `companion_query`. Only local HTTP MCP endpoints are accepted. Use an SSH tunnel for a remote server. A failed live connection never silently switches to demo.

Metric names come from `list_available_metrics`. Use the exact returned name: some servers expose `DB Time(s)` and return no measurements for the alias `DBTime`.

## How the panel supports the investigation

- `companion_query` collects evidence and renders load summaries, metric/wait/SQL timelines, SQL text, comparison tables and `full_gradients` regression results.
- `companion_set_language` switches between `pl` and `en` while preserving evidence, selection and history. The interface and automatic chart captions switch immediately. Codex translates any pending authored descriptions through the same tool; translations are cached per view. No external translation service or API key is used. New authored boards and notes receive the dialogue language directly. For example: “Przejdźmy na polski” or “Let’s continue in English.”
- `companion_present` composes up to six related charts, tables, or mechanism cards. Each custom panel includes its source and an explicit `measured`, `hypothesis`, or `explanation` label. See the [presentation schema and example](skills/jasmin-companion/references/presentations.md).
- An active presentation stays visible while further queries collect evidence. `display:false` also prevents a query from changing an ordinary evidence view.
- Use the chart's time-window selector to inspect the peak. Select a point or SQL row, then ask Codex to investigate that selection. `companion_context` supplies the selected snapshot, SQL, and instance when available. Clicking does not send a chat message by itself.
- **Earlier views** restores previous charts and notes. Raw data is available in a collapsed technical section rather than replacing the main analytical view.

Missing observations remain unknown, not zero. When MCP returns duplicate RAC timestamps without instance identity, the panel shows separate points and a warning instead of inventing an instance mapping. Instance-specific presentations require identity verified from source evidence. A mechanism diagram is an explanation or hypothesis, not proof of a blocker chain.

## Guided audit

After scope, open the first analytical chapter to compare Ridge, Elastic Net, Huber and Quantile 95 for DB Time and DB CPU. Read across a predictor's row for its signed coefficients, then compare modeled impact at P90 and P99 and check the fit diagnostics. Saved workload boards for the chapter remain available. A missing coefficient on the returned ranking page appears as a dash. Inspect an individual predictor with a `full_gradients` query using its exact family and contributor. The ninth chapter revisits the opening hypotheses using the later checks.

Click a chapter at the top of the panel to reopen its explanations and the views displayed in this panel session. Select a view by title and ask about it in the same chat. The assistant can see your selection while the audit retains its current step. To continue the discussion at a different step, ask the assistant to move there. The chapter keeps its displayed views after the recent-view history rolls over. Boards attached to saved briefs can also be reopened in a new panel session connected to the same audit.

`companion_record_brief` saves the question, explanation, conclusion, evidence boundary, next step and exact evidence references in the upstream analysis. Reusing a brief ID preserves its corrections as revisions. An optional `view_id` attaches a copy of the evidence board. These records are independent of the panel's 20-view display cache. The core validates evidence references; a custom chart's free-text source label alone is not proof of provenance.

Only an explicit request to produce the final report permits `companion_authorize_report`. Finishing chapters, exporting the audit and saying “next” do not authorize a report. The request is recorded as provenance; the tool cannot independently authenticate what the user said. The report still requires the existing evidence coverage and finalization checks. A changed brief or chapter transition revokes the authorization. The report writer uses the same analysis identity and saved explanations, keeping the full technical evidence alongside the reader-facing narrative.

The audit tools are:

| Tool | Purpose |
|---|---|
| `companion_start_audit` | Start or continue guided discussion in the selected analysis |
| `companion_audit_status` | Read chapters, saved explanations and report authorization |
| `companion_record_brief` | Save an explanation or correction with evidence references |
| `companion_advance_audit` | Move to or revisit a chapter |
| `companion_record_discussed_entities` | Retain SQL IDs and exact wait names from the conversation for their source project |
| `companion_authorize_report` | Record the user's explicit final-report request |
| `companion_export_audit` | Return a checkpoint for saving to a local artifact |
| `companion_resume_audit` | Reconnect to an existing analysis on the running server |
| `companion_restore_audit` | Restore a saved checkpoint after server restart |

### Problem-oriented client reports

When you prepare a preview, Companion downloads every public page, chart and required asset for that edition and verifies its SHA-256 checksum. The panel serves these files locally. You can keep using the same URL, including links you have never opened, after the upstream MCP session expires or the core stops. Each edition stays available until the Companion panel process closes. Publishing a ZIP still requires the current reviewed preview and a live core connection.

On a compatible core, use `companion_get_client_report_plan` to read the saved draft and its source revisions. `companion_save_client_report_plan` checkpoints a complete ordered chapter list. Each chapter selects briefs and measurements. Tables selected from a board keep their units and column headers. SQL summaries use eligible common intervals and the per-execution ratios of totals. The `sql_family` and `sql_intervals` selections distinguish a population from an example. Use `sql_excerpt` for a logic finding. Select literal passages from the instance’s captured statement, explain what they do, describe when work or result multiplicity grows, and give a test of the change. The reader can open the cited positions in the selected source. Recorded corrections mark dependent drafts for review.

Companion records the SQL IDs and Oracle wait events displayed in a guided audit, including temporary boards and expanded raw measurements. For mentions in the conversation the agent calls `companion_record_discussed_entities` with the corresponding project. The client report links each SQL_ID to the instance chart and full captured SQL, and each wait to its available foreground or background charts. A register entry records a mention. Its measured impact belongs in the problem chapter.

Use the opening DB Time and DB CPU gradient evidence to explain where the audit's initial hypotheses came from. Ground SQL recommendations in a measured example tied to its statement text or execution plan and specify how to measure the effect. Omit boilerplate when evidence is unavailable.

The panel shows the saved chapters in reading order, and the arrows move them within the plan. **Prepare preview** or `companion_preview_client_report` opens access to the client view for that revision. Click a title to read its chapter or follow a source chart. The tool returns `panel_preview_url` and the entry file on the MCP host. `companion_publish_client_report` issues the reviewed edition upon an explicit request for the final report.

The default guided audit writes its first private checkpoint when a project is selected.
Recording or correcting a brief and moving the active discussion to another chapter each
write a revision. Opening a previous chapter for inspection preserves the existing revision.
The panel shows the confirmed revision and the exact file under **Checkpoint file on the MCP server**.
The MCP server writes these files under `.jasmin/audits/<audit_id>/` in its working directory.
Set `JASMIN_AUDIT_WORKSPACE` when starting the server to select a different directory.
Keep the audit folder together with its `objects/` directory and the original source captures when moving an audit.

To reconnect to a live analysis, use `companion_resume_audit` with its existing `analysis_id`.
To continue after a restart, reopen the same workspace with its source datasets, choose **Saved audits**,
and resume the saved `audit_id`. The new live analysis has its own `analysis_id`.
A portable `companion_export_audit` is also available. Save its returned `checkpoint` intact
and pass it to `companion_restore_audit` on the destination server. A current server creates
the first durable revision of the imported audit and returns its file path.

On an older core, an audit may be stored only in server memory. Companion identifies that
state in the panel. Save a portable checkpoint before stopping such a server.
The synthetic demo supports ordinary evidence discussion.

### TX lock pilot

Start the core in the intended audit workspace, or configure its directory with `JASMIN_AUDIT_WORKSPACE`. Select the project in Companion and ask the agent to start a saved TX audit. The agent calls `companion_enable_tx_pilot`. To compare instances, supply their `project_ids` and the active `project_id` to `companion_select_project`. Use `companion_resume_audit` on that analysis to view another included project.

Save a correction with `companion_record_brief` using its original `brief_id`. Attach the supporting board with `view_id`, preserve its evidence references and record the unresolved question and next test. The optional `private_note` can retain the auditor’s reason for the correction. Chapter buttons browse earlier work. **Discuss here** or `companion_advance_audit` moves the discussion and writes a snapshot. The panel displays the confirmed snapshot revision. To retry the same operation after a lost response, supply the same `operation_id`, `expected_revision` and content.

After a restart, open Companion against the same core workspace and choose a project from **Saved audits**. The corresponding tools are `companion_list_saved_audits` and `companion_resume_saved_audit`. The restored context includes the active chapter, last private note, focused brief, its questions, next test and attached board. The workspace belongs to the core host when connecting through a tunnel.

For a client delivery, call `companion_preview_tx_report` with the selected brief revision, public title, owner, acceptance criterion and bounded TX timeline references. For evidence from another project, also select the supporting brief and its revision for that reference. The brief’s conclusion, evidence boundary and next step are public copy. Choose **Open client preview** to review the prepared pages and follow their evidence links. Inspect the public text, measurement totals and interval coverage. After an explicit request, use **Publish client ZIP** or `companion_publish_tx_report` with the preview ID. Use **Download ZIP** to retrieve the verified archive through the panel. The receipt also gives its path in the core workspace. On a core without artifact transport, the panel displays the server paths.

### RAC CPU pilot

Select the participating instances with `companion_select_project` using the full `project_ids` list and the active `project_id`. Ask the agent to start a saved RAC CPU audit. The agent calls `companion_enable_cpu_pilot`. In later chats **Saved audits** identifies the CPU and TX investigations separately. A CPU audit uses the same automatic snapshots, private notes, saved board selection and revision checks as the TX pilot.

Use `companion_query` with `get_rac_cpu_scenario`. Supply the participating `project_ids`, the measured monitoring module prefix (`monitor_module_prefix`), the selected Oracle SQL identifiers (`selected_sql_ids`) and the assumed reduction in their CPU cost (`sql_reduction_pct`). The board shows the observed and modeled cluster peaks with their windows, the individual instance peaks and the modeled CPU-hours saved. The cluster peak uses simultaneous load across the instances. Retain the evidence ID and attach the board when saving or correcting the discussion brief.

Prepare the client pages with `companion_preview_cpu_report`, specifying the brief ID and revision, scenario evidence ID (`evidence_ref`), public title, owner, acceptance criterion and `instance_labels` mapping each project ID to its public name. Only request `include_sql_ids` when the auditor explicitly chooses to disclose the identifiers. Review the public conclusion, assumption, boundary, and action in **Open client preview**. On the auditor's explicit request choose **Publish client ZIP** or call `companion_publish_cpu_report`, then use **Download ZIP**. The observed peak is an interval average. The scenario estimates a change in work under its stated assumptions. Decisions about provisioned capacity also need throughput, response-time and failover checks.

### Combined TX and RAC CPU audit

Select both RAC instances in one analysis and ask the agent to start a combined TX and RAC CPU audit. The agent calls `companion_enable_combined_pilot`. Use the two buttons under **Discussion topics**, or call `companion_switch_audit_topic` with `topic` set to `tx` or `cpu`. Switching saves a snapshot and restores the selected topic’s project, chapter, focused explanation and board. Chapter browsing stays within the active topic. Save a correction under its original brief ID and include the supporting evidence. After a server restart, reopen the same workspace and select the combined entry in **Saved audits**.

Use `companion_preview_combined_report` with the edition title and separate `tx` and `cpu` selections. Each selection uses the same public titles, brief revisions, actions and evidence fields as its individual report. Inspect the overview and follow each topic to its source measurements. A request for the final client report authorizes the agent to prepare, review and publish the ZIP with `companion_publish_combined_report`. The same publication is available in the panel under **Publish client ZIP**. Extract the archive and open `index.html` for the entire report. The sibling `wykresy/` directory holds the chart pages and shared offline assets.

The combined client edition includes a chart directory. Follow the source-chart
links beside the measurements to inspect the available original figures in the
report preview. Companion retrieves the approved chart pages, local plotting
library and brand image from the exact preview via the connected core. The
same files ship in the ZIP and run offline after extraction.


## Update or uninstall

From a checkout on `main`:

```sh
git pull --ff-only
node plugins/jasmin-companion/install.mjs
```

Finish using the old panel before running the installer. A version update can replace the directory from which the old panel loads its files. Reload Codex, then use `companion_resume_audit` with the existing analysis ID while the same JAS-MIN server is running. After a core restart, reopen the checkpoint from **Saved audits**. If a previous core only held the audit in memory, import its portable checkpoint.

To uninstall, use a compatible Codex CLI (the same one used for installation):

```sh
codex plugin remove jasmin-companion@jasmin-local
```

Uninstalling the plugin leaves this repository intact.

## Troubleshooting

| Symptom | What to check |
|---|---|
| The skill appears, but no `companion_*` tools are available | Check desktop plugin-loader diagnostics and the installed manifest. Confirm Node.js is on the desktop application's `PATH`. After fixing configuration, reload Codex. Repeated restarts alone do not fix a loader error. |
| `Agent Plugins stdio command must be a bare executable name or a contained ./ path` | The portable `mcp.json` must use `./launch.sh`. Do not replace it with an absolute Node.js path. Reinstall from this checkout. |
| Installation works in a terminal, but the desktop cannot load the plugin | Ensure the installer used the desktop-compatible CLI. A successful test with an older standalone CLI does not establish desktop compatibility. Use `CODEX_CLI` if needed. |
| Live connection fails | Check that JAS-MIN is still running and that the loopback host, port, and `/mcp` path match. |
| A metric chart has no observations | Discover the exact metric name and inspect data availability. An empty result is not a measured zero. |
| The panel URL stops working | Reopen Companion, then resume the upstream analysis. If the server also restarted, open **Saved audits** or restore a manual checkpoint. The old URL cannot restore the display cache. |
| Guided audit is unavailable | Update both the JAS-MIN server and the installed plugin. Capability negotiation reports unsupported servers explicitly. |
| A new tool is missing | Reinstall, then reload Codex. Use MCP tool discovery to check the tools exposed by the running package. |

## Runtime and data handling

```text
Codex conversation + Companion skill
                  |
          Companion stdio MCP
            /             \
   JAS-MIN HTTP MCP    local browser panel
```

Each Companion process owns an isolated panel. A new project selection starts its own upstream analysis; explicitly resuming an analysis reconnects to that existing audit. The panel listens only on `127.0.0.1`, using a random port and a secret session URL. Its history of up to 20 views is held in memory. Audit explanations are stored separately in the upstream analysis and survive panel reconnection; current cores also persist checkpoint revisions of all guided audits in the configured workspace and list them under **Saved audits**. Treat the URL as access to that session's data; do not publish it.

Tool results enter the Codex conversation under its data settings. A local panel does not mean the AI conversation runs offline. Source SQL and identifiers are preserved verbatim. The plugin uses no separate model or paid Realtime integration.

The portable manifest launches `./launch.sh`, which changes to its package directory before running the server. The installer also generates an ignored `.mcp.json` with absolute paths for older CLI compatibility. Do not commit that machine-specific file.

## Development and validation

```sh
cd plugins/jasmin-companion
npm test
npm run preview
```

The preview prints a local URL, loads synthetic data, and runs until Ctrl+C. It is separate from the panel session opened by the installed plugin.

The automated suite covers stdio startup from another directory, HTTP/SSE lifecycle, session isolation, origin checks, selection, timeline response shapes, missing values, RAC ambiguity, presentation retention and language changes. Audit tests cover controlled routing, history eviction, revised hypotheses, project isolation, reconnect, checkpoint restoration, compatibility negotiation and the report authorization boundary. The in-process audit fixture tests the Companion contract; the core server tests validate checkpoint evidence and report completeness.
- A microphone conversation has not been validated end to end.

The avatar is an original generated asset; its [provenance and prompt](assets/avatar.md) are included. Reduced-motion preferences disable its animation.

A source-ID and revision match establishes where a measurement came from. During editorial review read the actual measurement for each card, check its instance, time window, SQL or event, and verify that it supports the stated interpretation. Review unselected or shared cards in the readability feedback. A common measurement can legitimately support different decisions.
