import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { createServer } from '../server.mjs';
import { localEndpoint } from '../companion.mjs';

test('Portable launcher from a different project: stdio lifecycle, evidence, selection and shutdown', async t => {
  const manifest = JSON.parse(await readFile(new URL('../mcp.json', import.meta.url), 'utf8'));
  const { command, args } = manifest.mcpServers.companion;
  assert.ok(command.startsWith('./'), 'Portable command must be a contained package launcher');
  const child = spawn(fileURLToPath(new URL('../' + command, import.meta.url)), args, { cwd: tmpdir(), stdio: ['pipe', 'pipe', 'pipe'] });
  t.after(() => child.kill());
  let seq = 0; const pending = new Map();
  createInterface({ input: child.stdout }).on('line', line => { const r = JSON.parse(line); pending.get(r.id)?.(r); pending.delete(r.id); });
  const rpc = (method, params) => new Promise(resolve => { const id = ++seq; pending.set(id, resolve); child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n'); });
  const call = async (name, args = {}) => (await rpc('tools/call', { name, arguments: args })).result;
  assert.equal((await rpc('initialize', { protocolVersion: '2025-06-18' })).result.serverInfo.name, 'jasmin-companion');
  child.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n');
  assert.equal((await rpc('tools/list')).result.tools.length, 10);
  const open = (await call('companion_open', { mode: 'demo' })).structuredContent;
  const url = open.panel_url;
  assert.equal((await fetch(url)).status, 200);
  assert.equal((await call('companion_projects')).structuredContent.projects[0].project_id, 'demo');
  await call('companion_select_project', { project_id: 'demo' });
  const query = (await call('companion_query', { tool: 'get_metric_time_series', arguments: { kind: 'load_profile', name: 'DBTime' } })).structuredContent;
  assert.equal(query.result.series[7].value, 24.6);
  const select = await fetch(url + 'selection', { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Companion-Client': 'panel' }, body: JSON.stringify({ snap_id: 107 }) });
  assert.equal(select.status, 200);
  assert.equal((await call('companion_context')).structuredContent.selection.snap_id, 107);
  await call('companion_note', { text: 'Hypothesis based on DEMO.' });
  assert.equal((await (await fetch(url + 'state')).json()).note, 'Hypothesis based on DEMO.');
  const board = (await call('companion_present', { title: 'Measured load', takeaway: 'Synthetic demonstration.', view_ids: [query.view_id] })).structuredContent;
  assert.equal(board.panel_count, 1);
  assert.equal((await call('companion_context')).structuredContent.current_view_id, board.view_id);
  assert.equal((await call('companion_query', { tool: 'get_sql_text', arguments: { project_id: 'other' } })).isError, true);
  assert.equal((await call('companion_query', { tool: 'write_report' })).isError, true);
  assert.equal((await call('companion_query', { tool: 'list_snapshots', arguments: { limit: 101 } })).isError, true);
  const exited = new Promise(resolve => child.once('exit', resolve)); child.stdin.end();
  assert.equal(await exited, 0);
});

test('panel is isolated, origin guarded, rejects unknown selection; project switch clears evidence', async t => {
  const a = await createServer(), b = await createServer(); t.after(async () => { await a.close(); await b.close(); });
  for (const app of [a,b]) { await app.call('companion_open', { mode: 'demo' }); await app.call('companion_select_project', { project_id: 'demo' }); }
  const view = await a.call('companion_query', { tool: 'top_sqls_in_snapshot', arguments: { snap_id: 107 } });
  await assert.rejects(b.call('companion_show', { view_id: view.view_id }), /Unknown view/);
  assert.equal((await fetch(new URL('/', a.panel_url))).status, 404);
  assert.equal((await fetch(a.panel_url + 'state', { headers: { Origin: 'https://example.org' } })).status, 403);
  assert.equal((await fetch(a.panel_url + 'selection', { method: 'POST', body: '{}' })).status, 403);
  assert.throws(() => a.companion.selectPoint({ snap_id: 107, sql_id: 'invented' }));
  a.companion.selectPoint({ snap_id: 107, sql_id: 'demo_sql_001' });
  assert.equal(a.companion.state.selection.sql_id, 'demo_sql_001');
  await a.call('companion_select_project', { project_id: 'demo' });
  assert.deepEqual(a.companion.state.views, []); assert.deepEqual(a.companion.state.selection, {});
});

test('HTTP upstream lifecycle with chunked SSE, injected routing and session DELETE', async t => {
  const seen = [];
  const upstream = http.createServer(async (req, res) => {
    if (req.method === 'DELETE') { seen.push('DELETE'); res.writeHead(200); res.end(); return; }
    let body = ''; for await (const chunk of req) body += chunk;
    const msg = JSON.parse(body); seen.push(msg);
    res.setHeader('Mcp-Session-Id', 'test-session');
    if (!('id' in msg)) { res.writeHead(202); res.end(); return; }
    let result;
    if (msg.method === 'initialize') result = { protocolVersion: '2025-06-18', capabilities: {}, serverInfo: { name: 'fixture', version: '1' } };
    else if (msg.method === 'tools/list') result = { tools: [{ name: 'get_metric_time_series', inputSchema: { type: 'object', properties: { name: { type: 'string' }, analysis_id: { type: 'string' }, project_id: { type: 'string' } }, required: ['name', 'analysis_id', 'project_id'] } }] };
    else {
      const name = msg.params.name;
      const value = name === 'list_performance_projects' ? { projects: [{ project_id: 'fixture-project' }] } : name === 'start_performance_analysis' ? { analysis_id: 'fixture-analysis' } : { evidence_id: 'E-1', result: { series: [{ snap_id: 1, value: 0 }, { snap_id: 2, value: 5 }] } };
      result = { content: [{ type: 'text', text: JSON.stringify(value) }] };
    }
    res.writeHead(200, { 'Content-Type': 'text/event-stream' });
    const frame = `data: ${JSON.stringify({ jsonrpc: '2.0', id: msg.id, result })}\n\n`;
    res.write(frame.slice(0, 14)); setTimeout(() => res.end(frame.slice(14)), 3);
  });
  await new Promise(r => upstream.listen(0, '127.0.0.1', r));
  t.after(() => { upstream.closeAllConnections(); upstream.close(); });
  const app = await createServer(); t.after(() => app.close());
  await app.call('companion_open', { mode: 'live', endpoint: `http://127.0.0.1:${upstream.address().port}/mcp` });
  await app.call('companion_select_project', { project_id: 'fixture-project' });
  const schemas = await app.call('companion_tools');
  assert.deepEqual(schemas.tools[0].inputSchema.required, ['name']);
  const value = await app.call('companion_query', { tool: 'get_metric_time_series', arguments: { name: 'DBTime' } });
  assert.equal(value.result.series[0].value, 0);
  const args = seen.find(m => m.params?.name === 'get_metric_time_series').params.arguments;
  assert.equal(args.analysis_id, 'fixture-analysis'); assert.equal(args.project_id, 'fixture-project');
  await app.companion.close(); assert.ok(seen.includes('DELETE'));
});

test('endpoint boundaries and missing observations', async () => {
  for (const url of ['https://example.com/mcp', 'http://127.0.0.1.evil/mcp', 'http://user:pass@localhost/mcp', 'file:///tmp/a']) assert.throws(() => localEndpoint(url));
  assert.equal(localEndpoint('http://127.0.0.1:4242/mcp'), 'http://127.0.0.1:4242/mcp');
  const { seriesPoints, unitFor } = await import('../public/charts.js');
  assert.deepEqual(seriesPoints({ series: [{ value: null }, { value: 0 }, { value: 4 }] }), [{ value: 0 }, { value: 4 }]);
  assert.match(unitFor({ kind: 'load_profile', name: 'DBTime' }), /s\/s/);
});
