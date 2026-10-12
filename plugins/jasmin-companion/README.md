# JAS-MIN Companion

A local Codex plugin that supports Oracle performance investigations with a persistent side panel: evidence charts, baseline comparisons, SQL tables, and mechanism diagrams, accompanied by a hooded avatar.

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

The plugin discovers project IDs and available tool schemas from JAS-MIN and creates its own analysis session. It allows 12 read-only upstream tools; it does not expose report-writing tools. Only local HTTP MCP endpoints are accepted. Use an SSH tunnel for a remote server. A failed live connection never silently switches to demo.

Metric names come from `list_available_metrics`. Use the exact returned name: some servers expose `DB Time(s)` and return no measurements for the alias `DBTime`.

## How the panel supports the investigation

- `companion_query` collects evidence and automatically renders supported load summaries, metric/wait/SQL timelines, SQL text, and comparison tables.
- `companion_set_language` switches between `pl` and `en` while preserving evidence, selection and history. The interface and automatic chart captions switch immediately. Codex translates any pending authored descriptions through the same tool; translations are cached per view. No external translation service or API key is used. New authored boards and notes receive the dialogue language directly. For example: “Przejdźmy na polski” or “Let’s continue in English.”
- `companion_present` composes up to six related charts, tables, or mechanism cards. Each custom panel includes its source and an explicit `measured`, `hypothesis`, or `explanation` label. See the [presentation schema and example](skills/jasmin-companion/references/presentations.md).
- An active presentation stays visible while further queries collect evidence. `display:false` also prevents a query from changing an ordinary evidence view.
- Use the chart's time-window selector to inspect the peak. Select a point or SQL row, then ask Codex to investigate that selection. `companion_context` supplies the selected snapshot, SQL, and instance when available. Clicking does not send a chat message by itself.
- **Earlier views** restores previous charts and notes. Raw data is available in a collapsed technical section rather than replacing the main analytical view.

Missing observations remain unknown, not zero. When MCP returns duplicate RAC timestamps without instance identity, the panel shows separate points and a warning instead of inventing an instance mapping. Instance-specific presentations require identity verified from source evidence. A mechanism diagram is an explanation or hypothesis, not proof of a blocker chain.

## Update or uninstall

From a checkout on `main`:

```sh
git pull --ff-only
node plugins/jasmin-companion/install.mjs
```

Reload Codex to load updated server code, tools, and skill instructions. Editing the repository alone does not update the installed copy. Refreshing the browser reloads panel assets, but does not restart the MCP server.

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
| The panel URL stops working | Its MCP process has ended. Ask Codex to reopen Companion; the old session URL cannot restore in-memory state. |
| The new presentation tool is missing | Reinstall, then reload Codex. The current package exposes ten Companion tools, including `companion_present` and `companion_set_language`. |

## Runtime and data handling

```text
Codex conversation + Companion skill
                  |
          Companion stdio MCP
            /             \
   JAS-MIN HTTP MCP    local browser panel
```

Each MCP process owns an isolated panel and analysis session. The panel listens only on `127.0.0.1`, using a random port and a secret session URL. Its history of up to 20 views is held in memory. Treat the URL as access to that session's data; do not publish it.

Tool results enter the Codex conversation under its data settings. A local panel does not mean the AI conversation runs offline. Source SQL and identifiers are preserved verbatim. The plugin uses no separate model or paid Realtime integration.

The portable manifest launches `./launch.sh`, which changes to its package directory before running the server. The installer also generates an ignored `.mcp.json` with absolute paths for older CLI compatibility. Do not commit that machine-specific file.

## Development and validation

```sh
cd plugins/jasmin-companion
npm test
npm run preview
```

The preview prints a local URL, loads synthetic data, and runs until Ctrl+C. It is separate from the panel session opened by the installed plugin.

Validation as of 12 October 2026:

- Eight automated tests cover stdio startup from another project directory, HTTP/SSE lifecycle, session isolation, origin checks, selection, real timeline response shapes, missing values, RAC ambiguity, and presentation validation/retention.
- The installed launcher was checked against a live local JAS-MIN collection: nine tools loaded, a wait timeline returned 1,224 records, and a four-card presentation survived another query.
- Browser checks covered SQL/wait charts, peak zoom, instance-aware selection, and restoring an analytical presentation. Client datasets and local preview artifacts are not included in the repository.
- A microphone conversation has not been validated end to end.

The avatar is an original generated asset; its [provenance and prompt](assets/avatar.md) are included. Reduced-motion preferences disable its animation.
