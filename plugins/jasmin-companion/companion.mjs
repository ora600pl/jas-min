import { McpClient } from './mcp-client.mjs';
import { DemoClient } from './demo.mjs';
import { automaticPresentation } from './public/charts.js';
import { validatePresentation } from './presentation.mjs';

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
  async query(tool, args = {}, display = true) {
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
    const current = this.state.views.find(v => v.id === this.state.current);
    const visual = automaticPresentation(view).panels.length > 0;
    const activate = display && visual && !current?.presentation;
    this.record(view, activate);
    return { view_id: view.id, displayed: activate, presentation_active: !!current?.presentation, ...data };
  }
  record(view, activate = true) {
    // Keep the displayed board intact while collecting supporting evidence.
    const current = this.state.views.find(v => v.id === this.state.current);
    const keep = this.state.views.filter(v => v.id !== current?.id).slice(-18);
    this.update({ views: [...keep, ...(current ? [current] : []), view],
      ...(activate ? { current: view.id, note: view.note || '' } : {}) });
  }
  present({ title, takeaway, panels = [], view_ids = [] }) {
    if (!this.state.project) throw new Error('Call companion_select_project first.');
    if (!Array.isArray(view_ids) || view_ids.length > 6 || !Array.isArray(panels)) throw new Error('Invalid presentation panels/view_ids.');
    const custom = panels.length ? validatePresentation({ title, takeaway, panels }).panels : [];
    const referenced = view_ids.flatMap(id => {
      const view = this.state.views.find(v => v.id === id);
      if (!view || view.presentation) throw new Error('Use existing evidence view IDs from this session.');
      const result = automaticPresentation(view).panels;
      if (!result.length) throw new Error('This evidence needs a custom visual panel.');
      return result;
    });
    const combined = [...referenced, ...custom];
    if (typeof title !== 'string' || !title.trim() || title.length > 180 || typeof takeaway !== 'string' || !takeaway.trim() || takeaway.length > 1200 || !combined.length || combined.length > 6) throw new Error('Presentation needs title, takeaway and 1–6 panels.');
    const view = { id: crypto.randomUUID(), tool: 'companion_present', args: {}, data: { source_view_ids: view_ids },
      presentation: { title, takeaway, panels: combined }, time: new Date().toISOString() };
    this.record(view);
    return { view_id: view.id, displayed: true, panel_count: combined.length };
  }
  note(text) {
    const view = this.state.views.find(v => v.id === this.state.current);
    if (view) view.note = text;
    this.update({ note: text });
  }
  show(view_id) {
    const view = this.state.views.find(v => v.id === view_id);
    if (!view) throw new Error('Unknown view in this session.');
    this.update({ current: view_id, note: view.note || '' }); return { view_id };
  }
  selectPoint(selection) {
    const view = this.state.views.find(v => v.id === this.state.current);
    if (!view) throw new Error('No current view.');
    const board = view.presentation || automaticPresentation(view);
    const candidates = board.panels.flatMap(p => p.series?.flatMap(s => s.points) || p.selections || []);
    const matches = candidates.filter(p => p.snap_id === selection.snap_id &&
      (p.sql_id || null) === (selection.sql_id || null) && (p.instance || null) === (selection.instance || null));
    if (!matches.length) throw new Error('Selection is outside the current view.');
    if (!selection.instance && board.panels.some(p => p.scatter && p.series.some(s => s.points.filter(v => v.snap_id === selection.snap_id).length > 1)))
      throw new Error('Instance identity is missing for this snapshot. Verify the source AWR before selecting a RAC record.');
    const p = matches[0];
    this.update({ selection: { snap_id: p.snap_id, ...(p.sql_id ? { sql_id: p.sql_id } : {}), ...(p.instance ? { instance: p.instance } : {}) } });
  }
  async close() { await this.client?.close(); this.client = null; }
}
