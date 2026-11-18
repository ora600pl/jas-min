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

- `companion_query` collects evidence and automatically renders supported load summaries, metric/wait/SQL timelines, SQL text, and comparison tables.
- `companion_set_language` switches between `pl` and `en` while preserving evidence, selection and history. The interface and automatic chart captions switch immediately. Codex translates any pending authored descriptions through the same tool; translations are cached per view. No external translation service or API key is used. New authored boards and notes receive the dialogue language directly. For example: “Przejdźmy na polski” or “Let’s continue in English.”
- `companion_present` composes up to six related charts, tables, or mechanism cards. Each custom panel includes its source and an explicit `measured`, `hypothesis`, or `explanation` label. See the [presentation schema and example](skills/jasmin-companion/references/presentations.md).
- An active presentation stays visible while further queries collect evidence. `display:false` also prevents a query from changing an ordinary evidence view.
- Use the chart's time-window selector to inspect the peak. Select a point or SQL row, then ask Codex to investigate that selection. `companion_context` supplies the selected snapshot, SQL, and instance when available. Clicking does not send a chat message by itself.
- **Earlier views** restores previous charts and notes. Raw data is available in a collapsed technical section rather than replacing the main analytical view.

Missing observations remain unknown, not zero. When MCP returns duplicate RAC timestamps without instance identity, the panel shows separate points and a warning instead of inventing an instance mapping. Instance-specific presentations require identity verified from source evidence. A mechanism diagram is an explanation or hypothesis, not proof of a blocker chain.

## Guided audit

After scope, the first analytical chapter compares gradient signals, anomaly windows and measured workload. The ninth chapter revisits the opening hypotheses using the later checks.

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
| `companion_authorize_report` | Record the user's explicit final-report request |
| `companion_export_audit` | Return a checkpoint for saving to a local artifact |
| `companion_resume_audit` | Reconnect to an existing analysis on the running server |
| `companion_restore_audit` | Restore a saved checkpoint after server restart |

To continue in a new panel while the server is still running, use `companion_resume_audit` with the existing `analysis_id`. To survive a server restart, export the checkpoint and save the returned package intact to a local artifact. Import it using `companion_restore_audit`; the core validates the source projects and evidence and returns a new analysis identity. An export response that has not been saved is still only in memory. Checkpoints contain investigation data and should be kept with the source collection, outside the repository.

An older server, including the lightweight synthetic demo, explicitly reports that guided audit storage is unavailable. Ordinary charts and discussion remain usable; the panel does not pretend to save an audit. Update the server and installed plugin together to enable the guided workflow.

## Update or uninstall

From a checkout on `main`:

```sh
git pull --ff-only
node plugins/jasmin-companion/install.mjs
```

Finish using the old panel before running the installer. A version update can replace the directory from which the old panel loads its files. Reload Codex, then use `companion_resume_audit` with the existing analysis ID while the same JAS-MIN server is running. If you also plan to restart that server, save an audit checkpoint first and use `companion_restore_audit` afterward.

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
| The panel URL stops working | Reopen Companion, then resume the upstream analysis. If the server also restarted, restore a saved audit checkpoint. The old URL cannot restore the display cache. |
| Guided audit is unavailable | Update both the JAS-MIN server and the installed plugin. Capability negotiation reports unsupported servers explicitly. |
| A new tool is missing | Reinstall, then reload Codex. The current package exposes eighteen Companion tools, including eight controlled audit tools. |

## Runtime and data handling

```text
Codex conversation + Companion skill
                  |
          Companion stdio MCP
            /             \
   JAS-MIN HTTP MCP    local browser panel
```

Each Companion process owns an isolated panel. A new project selection starts its own upstream analysis; explicitly resuming an analysis reconnects to that existing audit. The panel listens only on `127.0.0.1`, using a random port and a secret session URL. Its history of up to 20 views is held in memory. Audit explanations are stored separately in the upstream analysis and survive panel reconnection; saving an export is necessary to survive a server restart. Treat the URL as access to that session's data; do not publish it.

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
