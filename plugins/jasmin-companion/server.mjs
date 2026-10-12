import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { Companion } from './companion.mjs';
import { presentationProperties } from './presentation.mjs';

const definition = (name, description, properties = {}, required = []) => ({ name, description,
  inputSchema: { type: 'object', properties, required, additionalProperties: false },
  annotations: { readOnlyHint: !['companion_present', 'companion_note', 'companion_show', 'companion_select_project', 'companion_open'].includes(name), destructiveHint: false, openWorldHint: false } });
const str = { type: 'string' };
export const TOOLS = [
  definition('companion_open', 'Start the local avatar panel. Returns panel_url to open in Codex browser. Live is default; demo is synthetic.', { mode: { enum: ['live', 'demo'], type: 'string' }, endpoint: str }),
  definition('companion_projects', 'List actual JAS-MIN project IDs.'),
  definition('companion_select_project', 'Start an analysis for a project. Clears previous panel evidence.', { project_id: str }, ['project_id']),
  definition('companion_tools', 'Read schemas of available read-only JAS-MIN evidence tools.'),
  definition('companion_query', 'Collect evidence from JAS-MIN. Supported results become charts; an active presentation stays visible. Use companion_present to compose or replace the analytical board. Read companion_tools first.', { tool: str, arguments: { type: 'object' }, display: { type: 'boolean', description: 'False collects evidence without changing the visible view.' } }, ['tool']),
  definition('companion_present', 'Compose the side panel to support your explanation: related charts, baseline comparisons, tables and labelled mechanism flows. Keep all copy in English. Use source-verified numbers, units and instance identity; missing values are null. The board stays visible during subsequent queries. For line panels x is a numeric timestamp or ordered index, points strictly increasing per series. Separate different units into separate panels.', presentationProperties, ['title', 'takeaway']),
  definition('companion_context', 'Read current project, view and snapshot/SQL selection made by the user.'),
  definition('companion_show', 'Display a previously returned view_id from this chat.', { view_id: str }, ['view_id']),
  definition('companion_note', 'Display your short interpretation next to the evidence. Always write the note in English, even when the conversation is in another language. Preserve source identifiers. Does not synthesize audio.', { text: { type: 'string', description: 'Concise panel interpretation in English.', maxLength: 2000 } }, ['text']),
];

export async function createServer({ port = 0 } = {}) {
  const companion = new Companion();
  const token = randomBytes(24).toString('hex');
  const files = new Map([
    ['', ['public/index.html', 'text/html']], ['app.js', ['public/app.js', 'text/javascript']],
    ['style.css', ['public/style.css', 'text/css']], ['charts.js', ['public/charts.js', 'text/javascript']],
    ['avatar.png', ['assets/avatar.png', 'image/png']],
  ]);
  let origin;
  const server = http.createServer(async (req, res) => {
    res.setHeader('Cache-Control', 'no-store');
    res.setHeader('Referrer-Policy', 'no-referrer');
    res.setHeader('X-Content-Type-Options', 'nosniff');
    res.setHeader('Content-Security-Policy', "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'");
    const reply = (code, value) => { res.writeHead(code, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(value)); };
    try {
      if (req.headers.host !== new URL(origin).host || (req.headers.origin && req.headers.origin !== origin)) return reply(403, { error: 'Origin/Host denied' });
      const path = new URL(req.url, origin).pathname;
      if (!path.startsWith(`/${token}/`)) return reply(404, { error: 'Unknown panel' });
      const route = path.slice(token.length + 2);
      if (req.method === 'GET' && route === 'state') return reply(200, companion.state);
      if (req.method === 'POST' && ['selection', 'show'].includes(route)) {
        if (req.headers['x-companion-client'] !== 'panel') return reply(403, { error: 'Missing client header' });
        let body = '';
        for await (const part of req) { body += part; if (body.length > 4096) { reply(413, { error: 'Request too large' }); return; } }
        const args = JSON.parse(body);
        if (route === 'selection') companion.selectPoint(args);
        else companion.show(args.view_id);
        return reply(200, { ok: true });
      }
      if (req.method !== 'GET' || !files.has(route)) return reply(404, { error: 'Not found' });
      const [file, mime] = files.get(route);
      const bytes = await readFile(new URL(file, import.meta.url));
      res.writeHead(200, { 'Content-Type': `${mime}${mime.startsWith('text/') ? '; charset=utf-8' : ''}` }); res.end(bytes);
    } catch (e) { if (!res.headersSent) reply(400, { error: e.message }); else res.end(); }
  });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(port, '127.0.0.1', resolve); });
  origin = `http://127.0.0.1:${server.address().port}`;
  const panel_url = `${origin}/${token}/`;
  let queue = Promise.resolve();
  async function execute(name, args) {
    const def = TOOLS.find(t => t.name === name);
    if (!def) throw new Error('Unknown tool');
    if (!args || typeof args !== 'object' || Array.isArray(args)) throw new Error('Invalid arguments');
    for (const key of def.inputSchema.required) if (!(key in args)) throw new Error(`Missing ${key}`);
    for (const [key, value] of Object.entries(args)) {
      const rule = def.inputSchema.properties[key];
      if (!rule || (rule.type && (rule.type === 'array' ? !Array.isArray(value) : typeof value !== rule.type)) || (rule.enum && !rule.enum.includes(value))) throw new Error(`Invalid ${key}`);
    }
    companion.update({ status: 'working' });
    try {
      switch (name) {
        case 'companion_open': await companion.open(args); return { panel_url, mode: companion.state.mode, voice: 'Use native Codex voice; panel has no microphone.' };
        case 'companion_projects': return await companion.projects();
        case 'companion_select_project': return await companion.select(args.project_id);
        case 'companion_tools': return await companion.tools();
        case 'companion_query': return await companion.query(args.tool, args.arguments, args.display);
        case 'companion_present': return companion.present(args);
        case 'companion_context': return { project_id: companion.state.project, mode: companion.state.mode, selection: companion.state.selection, current_view_id: companion.state.current,
          views: companion.state.views.map(v => ({ view_id: v.id, tool: v.tool, args: v.args, evidence_id: v.data.evidence_id, title: v.presentation?.title, presentation: !!v.presentation })) };
        case 'companion_show': return companion.show(args.view_id);
        case 'companion_note':
          if (args.text.length > 2000) throw new Error('Maximum note length is 2000.');
          companion.note(args.text); return { displayed: true };
      }
    } finally { companion.update({ status: 'idle' }); }
  }
  function call(name, args = {}) {
    const result = queue.then(() => execute(name, args));
    queue = result.catch(() => {}); return result;
  }
  return { panel_url, companion, call, async close() {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
    await queue; await companion.close();
  } };
}

export async function serveStdio(app) {
  const send = value => process.stdout.write(JSON.stringify(value) + '\n');
  let pending = Promise.resolve();
  const handle = async line => {
    let request;
    try { request = JSON.parse(line); } catch { return send({ jsonrpc: '2.0', id: null, error: { code: -32700, message: 'Parse error' } }); }
    if (!request || Array.isArray(request) || request.jsonrpc !== '2.0' || typeof request.method !== 'string')
      return send({ jsonrpc: '2.0', id: request?.id ?? null, error: { code: -32600, message: 'Invalid Request' } });
    if (!('id' in request)) return;
    const response = { jsonrpc: '2.0', id: request.id };
    try {
      switch (request.method) {
        case 'initialize': response.result = { protocolVersion: ['2024-11-05', '2025-03-26', '2025-06-18', '2025-11-25'].includes(request.params?.protocolVersion) ? request.params.protocolVersion : '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'jasmin-companion', version: '0.1.0' } }; break;
        case 'ping': response.result = {}; break;
        case 'tools/list': response.result = { tools: TOOLS }; break;
        case 'tools/call': {
          try {
            const result = await app.call(request.params?.name, request.params?.arguments ?? {});
            response.result = { content: [{ type: 'text', text: JSON.stringify(result) }], structuredContent: result };
          } catch (e) { response.result = { isError: true, content: [{ type: 'text', text: e.message }] }; }
          break;
        }
        default: response.error = { code: -32601, message: 'Method not found' };
      }
    } catch (e) { response.error = { code: -32603, message: e.message }; }
    send(response);
  };
  // Bounded newline-delimited JSON-RPC framing; stdout is reserved for MCP.
  let buffer = '';
  process.stdin.setEncoding('utf8');
  for await (const chunk of process.stdin) {
    buffer += chunk;
    if (buffer.length > 1024 * 1024) throw new Error('MCP input too large');
    let i;
    while ((i = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, i); buffer = buffer.slice(i + 1);
      if (line.trim()) pending = pending.then(() => handle(line));
    }
  }
  await pending;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const app = await createServer();
  let stopping = false;
  const stop = async () => { if (stopping) return; stopping = true; await app.close(); process.exit(0); };
  process.once('SIGTERM', stop); process.once('SIGINT', stop);
  if (process.argv.includes('--preview')) {
    await app.call('companion_open', { mode: 'demo' });
    await app.call('companion_select_project', { project_id: 'demo' });
    await app.call('companion_query', { tool: 'top_wait_events_in_snapshot', arguments: { snap_id: 107 } });
    await app.call('companion_query', { tool: 'top_sqls_in_snapshot', arguments: { snap_id: 107 } });
    await app.call('companion_query', { tool: 'get_metric_time_series', arguments: { kind: 'load_profile', name: 'DBTime' } });
    await app.call('companion_note', { text: 'In this synthetic demo, load peaks at 24.6 average active sessions at snapshot 107. Select a point to focus the conversation.' });
    console.error(app.panel_url);
  } else {
    try { await serveStdio(app); } finally { await app.close(); }
  }
}
