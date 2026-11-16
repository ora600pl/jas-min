import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import { cp, mkdir, mkdtemp, readFile, rm, symlink } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

test('Claude manifest launches a relocated plugin and keeps panel evidence in its MCP session', async t => {
  const scratch = await mkdtemp(join(tmpdir(), 'companion-claude-'));
  t.after(() => rm(scratch, { recursive: true, force: true }));
  const installed = join(scratch, 'plugin cache with spaces');
  const project = join(scratch, 'unrelated project');
  await mkdir(project);
  for (const file of ['.claude-plugin', 'server.mjs', 'companion.mjs', 'demo.mjs', 'mcp-client.mjs', 'presentation.mjs', 'public', 'assets', 'skills']) {
    await cp(new URL('../' + file, import.meta.url), join(installed, file), { recursive: true });
  }
  const linked = join(scratch, 'linked plugin');
  await symlink(installed, linked, 'junction');
  const manifest = JSON.parse(await readFile(join(installed, '.claude-plugin/plugin.json'), 'utf8'));
  const config = manifest.mcpServers.companion;
  const args = config.args.map(arg => arg.replaceAll('${CLAUDE_PLUGIN_ROOT}', linked));
  const child = spawn(config.command, args, { cwd: project, stdio: ['pipe', 'pipe', 'pipe'] });
  t.after(() => child.kill());
  const exit = once(child, 'exit');
  let stderr = '', sequence = 0;
  const pending = new Map();
  child.stderr.setEncoding('utf8').on('data', chunk => { stderr += chunk; });
  const rejectPending = error => { for (const request of pending.values()) request.reject(error); pending.clear(); };
  child.on('error', rejectPending);
  child.on('exit', code => rejectPending(new Error(`MCP exited ${code}: ${stderr}`)));
  createInterface({ input: child.stdout }).on('line', line => {
    const response = JSON.parse(line), request = pending.get(response.id);
    pending.delete(response.id);
    if (response.error) request?.reject(new Error(JSON.stringify(response.error)));
    else request?.resolve(response.result);
  });
  const rpc = (method, params) => new Promise((resolve, reject) => {
    const id = ++sequence;
    pending.set(id, { resolve, reject });
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  });
  const call = async (name, arguments_ = {}) => {
    const result = await rpc('tools/call', { name, arguments: arguments_ });
    assert.ok(!result.isError, JSON.stringify(result.content));
    return result.structuredContent;
  };
  assert.equal((await rpc('initialize', { protocolVersion: '2025-06-18' })).serverInfo.name, 'jasmin-companion');
  child.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n');
  const names = new Set((await rpc('tools/list')).tools.map(tool => tool.name));
  for (const name of ['companion_open', 'companion_query', 'companion_present', 'companion_set_language', 'companion_context']) assert.ok(names.has(name));
  const { panel_url: url } = await call('companion_open', { mode: 'demo', language: 'pl' });
  for (const file of ['', 'app.js', 'charts.js', 'style.css', 'avatar.png']) assert.equal((await fetch(url + file)).status, 200);
  const projectId = (await call('companion_projects')).projects[0].project_id;
  await call('companion_select_project', { project_id: projectId });
  const evidence = await call('companion_query', { tool: 'get_metric_time_series', arguments: { kind: 'load_profile', name: 'DBTime' } });
  assert.equal(evidence.result.series[7].value, 24.6);
  const board = await call('companion_present', { language: 'pl', title: 'Obciążenie bazy', takeaway: 'Dane syntetyczne.', view_ids: [evidence.view_id] });
  const selection = await fetch(url + 'selection', { method: 'POST', headers: { 'X-Companion-Client': 'panel' }, body: JSON.stringify({ snap_id: 107 }) });
  assert.equal(selection.status, 200);
  const context = await call('companion_context');
  assert.equal(context.current_view_id, board.view_id);
  assert.equal(context.selection.snap_id, 107);
  const state = await (await fetch(url + 'state')).json();
  assert.equal(state.language, 'pl');
  assert.equal(state.current, board.view_id);
  child.stdin.end();
  assert.equal((await exit)[0], 0, stderr);
});
