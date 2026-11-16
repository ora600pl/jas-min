# JAS-MIN Companion for Claude Code

Use Companion in a **local Code session in Claude Desktop**, or in the Claude Code terminal. The plugin supplies the same local MCP server, avatar, charts, SQL views, conversation-language switching and investigation workflow as the Codex version.

## Install from this checkout

Install Node.js 22 or newer and make `node` available on the desktop application's `PATH`. Use a Claude Code release with plugin support. The package has no npm dependencies.

Run these commands from the repository root:

```sh
claude plugin validate plugins/jasmin-companion --strict
claude plugin validate . --strict
claude plugin marketplace add "$PWD"
claude plugin install jasmin-companion@jasmin-local --scope user
```

Use the quoted absolute path `"$PWD"` for marketplace registration. A bare `.` can be rejected as `Invalid marketplace source format` even when `claude plugin validate . --strict` succeeds. The quotes preserve paths containing spaces.

Keep the checkout: it is the source of this local marketplace. Start a new local Code session in Claude Desktop after installation. In the plugin browser, confirm that `jasmin-companion@jasmin-local` is enabled. The skill can be invoked as `/jasmin-companion:jasmin-companion`.

For a temporary terminal session without installing:

```sh
claude --plugin-dir ./plugins/jasmin-companion
```

## Try it

Ask in the new Code session:

> Use JAS-MIN Companion in demo mode. Open its panel, show database load and the busiest SQL statements, and explain the peak using the charts.

The demo uses synthetic evidence. The assistant calls `companion_open` and opens its returned `panel_url`. A browser pane may be available in a local Desktop session; the system browser is the fallback. The panel can stay beside the conversation and continues to receive the same MCP session's updates. This plugin does not register a custom native sidebar or provide its own voice interface.

For real evidence, start JAS-MIN with a collection as described in the [shared README](README.md#connect-to-real-data), then ask:

> Open JAS-MIN Companion in live mode at http://127.0.0.1:4242/mcp. List the projects and help me investigate my database.

The core server must be running on the same machine, or reachable through a local SSH tunnel. A failed live connection remains an error; the plugin does not silently use demo evidence.

The guided audit requires a core JAS-MIN server exposing the audit tools. If it is unavailable, Companion continues an ordinary evidence discussion and reports that saved audit briefs are unsupported. Final report generation additionally needs the core JAS-MIN report MCP tools in the same Claude session; this plugin does not register a second core connection automatically.

## Update or remove

After updating this checkout, refresh and update the installed copy:

```sh
claude plugin marketplace update jasmin-local
claude plugin update jasmin-companion@jasmin-local --scope user
```

Start a new local Code session to load the updated server and skill. Current Claude versions may read a local marketplace plugin directly from the checkout (`readFromFolder` in `claude plugin list --json`); keep that directory available. Refreshing the browser alone only reloads panel assets.

```sh
claude plugin uninstall jasmin-companion@jasmin-local --scope user
```

## Runtime and troubleshooting

Claude reads `.claude-plugin/plugin.json` and starts `node` with `${CLAUDE_PLUGIN_ROOT}/server.mjs` as a single argument. This works independently of the conversation's project directory and handles installation paths containing spaces. The inline `companion` entry overrides that name in the ignored `.mcp.json` generated for Codex; installing either host does not require rewriting the other's manifest.

If tools are absent, check the plugin browser's errors, `claude plugin list`, and whether the desktop can resolve `node`. If the panel stops responding, its MCP process may have ended: reopen it through `companion_open` in a current session. Do not run `npm run preview` to restore a conversation panel; preview is a separate synthetic session.

This integration targets local Code sessions. Claude's ordinary Chat tab, Cowork, remote and cloud sessions have different runtime and networking constraints and are not validated by this integration. Browser-pane placement and native voice availability depend on the host. Keep the capability URL private. Tool results enter the Claude conversation under its data settings; the local panel does not make the model conversation offline. No separate model API key is required by Companion.

## Validation

Run `npm test` in this directory. The Claude transport test starts the manifest's MCP command from an unrelated working directory and a copied installation path containing spaces and a symlink, then exercises demo discovery, charts, presentation, selection and shutdown. Use `claude plugin validate` for both the plugin and marketplace.

On 11 November 2026, all 19 Companion tests passed on macOS with Node.js 24.18.0. Claude Code 2.1.308 validated both manifests and the skill, installed the plugin in an isolated configuration, discovered one skill and one MCP server, and reported the server connected. Its loaded MCP definition used the Claude manifest even with the Codex compatibility file present. These checks did not run a model conversation, a live database investigation in Claude, or verify Desktop browser-pane embedding.

The integration follows the official [Desktop plugin documentation](https://code.claude.com/docs/en/desktop#install-plugins), [plugin manifest reference](https://code.claude.com/docs/en/plugins-reference#mcpservers), and [local marketplace workflow](https://code.claude.com/docs/en/plugin-marketplaces).
