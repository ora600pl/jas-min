import { McpClient } from './mcp-client.mjs';
import { DemoClient } from './demo.mjs';

export const ALLOWED = new Set(['get_database_load_summary', 'list_snapshots', 'list_available_metrics',
  'get_metric_time_series', 'get_wait_event_timeline', 'get_sql_timeline', 'top_wait_events_in_snapshot',
  'top_sqls_in_snapshot', 'get_sql_text', 'compare_snapshots', 'get_snapshot_details', 'get_diagnostic_guidance']);
export function localEndpoint(value) {
  const u = new URL(value);
  if (u.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(u.hostname) || u.username || u.password || u.hash)
    throw new Error('Use a local MCP endpoint: http://127.0.0.1:PORT/mcp (or an SSH tunnel).');
  return u.href;
}
export class Companion {
  constructor() {
    this.state = { revision: 0, mode: null, status: 'idle', project: null, selection: {}, views: [], current: null, note: '' };
    this.client = null; this.analysis = null; this.schemas = [];
  }
  update(patch) { Object.assign(this.state, patch); this.state.revision++; }
  async open({ mode = 'live', endpoint = 'http://127.0.0.1:4242/mcp' } = {}) {
    if (!['demo', 'live'].includes(mode)) throw new Error('mode must be demo or live');
    if (mode === 'live') endpoint = localEndpoint(endpoint);
    if (this.client && mode === this.state.mode && (mode === 'demo' || endpoint === this.endpoint)) return;
    const client = mode === 'demo' ? new DemoClient() : new McpClient(endpoint);
    try { await client.initialize(); } catch (e) { await client.close().catch(() => {}); throw e; }
    await this.client?.close().catch(() => {});
    this.client = client; this.endpoint = endpoint; this.analysis = null; this.schemas = [];
    this.update({ mode, project: null, selection: {}, views: [], current: null, note: '' });
  }
  requireClient() { if (!this.client) throw new Error('Call companion_open first.'); }
  async projects() { this.requireClient(); return this.client.call('list_performance_projects'); }
  async select(project_id) {
    this.requireClient();
    const projects = await this.projects();
    if (!projects.projects?.some(p => p.project_id === project_id)) throw new Error('Unknown project. Use companion_projects.');
    const result = await this.client.call('start_performance_analysis', { project_id });
    if (!result.analysis_id) throw new Error('MCP did not return an analysis_id.');
    this.analysis = result.analysis_id;
    this.update({ project: project_id, selection: {}, views: [], current: null, note: '' });
    return { project_id, analysis_id: this.analysis };
  }
  async tools() {
    this.requireClient();
    this.schemas = (await this.client.tools()).filter(t => ALLOWED.has(t.name)).map(t => {
      const schema = structuredClone(t.inputSchema);
      for (const k of ['analysis_id', 'project_id']) delete schema.properties?.[k];
      if (schema.required) schema.required = schema.required.filter(k => !['analysis_id', 'project_id'].includes(k));
      return { name: t.name, description: t.description, inputSchema: schema };
    });
    return { tools: this.schemas };
  }
  async query(tool, args = {}) {
    this.requireClient();
    if (!this.analysis) throw new Error('Call companion_select_project first.');
    if (!ALLOWED.has(tool)) throw new Error('Tool not allowed in the read-only panel.');
    if (!args || typeof args !== 'object' || Array.isArray(args)) throw new Error('arguments must be an object.');
    if ('project_id' in args || 'analysis_id' in args) throw new Error('Select the project with companion_select_project.');
    const bounded = { ...args };
    for (const key of ['limit', 'top_n']) if (key in bounded && (!Number.isInteger(bounded[key]) || bounded[key] < 1 || bounded[key] > 100)) throw new Error(`${key}: must be between 1 and 100.`);
    if (!this.schemas.length) await this.tools();
    const schema = this.schemas.find(t => t.name === tool)?.inputSchema;
    if (!schema) throw new Error('Tool unavailable on this server.');
    for (const key of schema.required || []) if (!(key in bounded)) throw new Error(`Missing argument: ${key}`);
    for (const key of ['limit', 'top_n']) if (schema.properties?.[key] && !(key in bounded)) bounded[key] = 20;
    const data = await this.client.call(tool, { ...bounded, analysis_id: this.analysis, project_id: this.state.project });
    const view = { id: crypto.randomUUID(), tool, args: bounded, data, time: new Date().toISOString() };
    this.update({ views: [...this.state.views.slice(-19), view], current: view.id, note: '' });
    return { view_id: view.id, ...data };
  }
  show(view_id) {
    if (!this.state.views.some(v => v.id === view_id)) throw new Error('Unknown view in this session.');
    this.update({ current: view_id, note: '' }); return { view_id };
  }
  selectPoint(selection) {
    const view = this.state.views.find(v => v.id === this.state.current);
    const data = view?.data.result ?? view?.data;
    const snap = selection.snap_id;
    const sql = selection.sql_id;
    if (sql) {
      if (view?.tool !== 'top_sqls_in_snapshot' || !data?.items?.some(x => x.sql_id === sql) || snap !== data.snap_id) throw new Error('SQL statement is outside the current view.');
    } else if (!data?.series?.some(x => x.snap_id === snap)) throw new Error('Snapshot is outside the current view.');
    this.update({ selection: { snap_id: snap, ...(sql ? { sql_id: sql } : {}) } });
  }
  async close() { await this.client?.close(); this.client = null; }
}
