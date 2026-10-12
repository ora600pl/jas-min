import { readFile, writeFile, access } from 'node:fs/promises';
import { dirname, resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { homedir } from 'node:os';
import { execFileSync } from 'node:child_process';

const root = dirname(fileURLToPath(import.meta.url));
const repo = resolve(root, '../..');
const manifest = JSON.parse(await readFile(join(root, 'plugin.json'), 'utf8'));
const codexHome = process.env.CODEX_HOME || join(homedir(), '.codex');
const installed = join(codexHome, 'plugins/cache/jasmin-local', manifest.name, manifest.version);
// Codex 0.144.5 does not expand PLUGIN_ROOT in MCP args. Generate exact local paths
// for the installed copy, without changing global MCP config or shell variables.
const config = { command: process.execPath, args: [join(installed, 'server.mjs')] };
await writeFile(join(root, '.mcp.json'), JSON.stringify({ mcpServers: { companion: config } }, null, 2) + '\n');
// The portable mcp.json uses a contained launcher, never host-specific paths.
// Prefer the desktop's engine: PATH may point at an older standalone CLI.
const desktopCli = '/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex';
const hasDesktopCli = await access(desktopCli).then(() => true, () => false);
const cli = process.env.CODEX_CLI || (hasDesktopCli ? desktopCli : 'codex');
const run = args => JSON.parse(execFileSync(cli, ['plugin', ...args, '--json'], { cwd: repo, encoding: 'utf8' }));
run(['marketplace', 'add', repo]);
const result = run(['add', `${manifest.name}@jasmin-local`]);
if (result.installedPath !== installed) throw new Error(`Unexpected install path: ${result.installedPath}. Expected ${installed}.`);
console.log(`Installed ${result.pluginId} ${manifest.version}\n${installed}\nReload Codex to load the skill and MCP tools.`);
