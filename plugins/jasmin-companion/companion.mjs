import { McpClient } from './mcp-client.mjs';
import { DemoClient } from './demo.mjs';
import { automaticPresentation, localizedView, viewTexts, translationStatus } from './public/charts.js';
import { validatePresentation } from './presentation.mjs';

export const ALLOWED = new Set(['get_database_load_summary', 'list_snapshots', 'list_available_metrics',
  'get_metric_time_series', 'get_wait_event_timeline', 'get_sql_timeline', 'top_wait_events_in_snapshot',
  'top_sqls_in_snapshot', 'get_sql_text', 'compare_snapshots', 'get_snapshot_details', 'get_diagnostic_guidance',
  'get_analysis_catalog', 'get_precomputed_analysis', 'get_db_instance_info', 'get_init_parameter',
  'get_wait_event_histogram', 'get_wait_event_sql_contributors', 'find_snapshots_with_event',
  'find_snapshots_with_sql', 'find_sqls_by_module', 'find_sqls_touching_object', 'search_sql_text',
  'list_available_sql_plans', 'get_sql_execution_plan', 'list_available_child_cursor_reasons',
  'get_child_cursor_reasons', 'get_alertlog_errors', 'top_segments_in_snapshot', 'top_latches_in_snapshot',
  'get_access_path_diagnostics', 'compare_project_metric', 'compare_project_sql']);
export function localEndpoint(value) {
  const u = new URL(value);
  if (u.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(u.hostname) || u.username || u.password || u.hash)
    throw new Error('Use a local MCP endpoint: http://127.0.0.1:PORT/mcp (or an SSH tunnel).');
  return u.href;
}
export class Companion {
  constructor() {
    this.state = { revision: 0, language: 'en', mode: null, status: 'idle', project: null, analysis_id: null,
      audit: null, audit_support: 'unknown', viewed_chapter_id: null, selection: {}, views: [], current: null, note: '' };
    this.client = null; this.analysis = null; this.schemas = []; this.upstreamSchemas = [];
    this.lastViewByChapter = new Map();
    this.archivedViews = new Map();
  }
  resetChapterViews() { this.lastViewByChapter.clear(); this.archivedViews.clear(); }
  update(patch) { Object.assign(this.state, patch); this.state.revision++; }
  async open({ mode = 'live', endpoint = 'http://127.0.0.1:4242/mcp', language = this.state.language } = {}) {
    this.validateLanguage(language);
    if (!['demo', 'live'].includes(mode)) throw new Error('mode must be demo or live');
    if (mode === 'live') endpoint = localEndpoint(endpoint);
    if (this.client && mode === this.state.mode && (mode === 'demo' || endpoint === this.endpoint)) { this.setLanguage({ language }); return; }
    const client = mode === 'demo' ? new DemoClient() : new McpClient(endpoint);
    try { await client.initialize(); } catch (e) { await client.close().catch(() => {}); throw e; }
    await this.client?.close().catch(() => {});
    this.client = client; this.endpoint = endpoint; this.analysis = null; this.schemas = []; this.upstreamSchemas = [];
    this.resetChapterViews();
    this.update({ mode, language, project: null, analysis_id: null, audit: null, audit_support: 'unknown', viewed_chapter_id: null, selection: {}, views: [], current: null, note: '' });
  }
  validateLanguage(language) {
    if (!['en', 'pl'].includes(language)) throw new Error('language must be en or pl');
  }
  languageContext() {
    return translationStatus(this.state.views.find(v => v.id === this.state.current), this.state.language);
  }
  setLanguage({ language, view_id, translations = [] }) {
    this.validateLanguage(language);
    if (!Array.isArray(translations) || translations.length > 300) throw new Error('Invalid translations.');
    const view = this.state.views.find(v => v.id === this.state.current);
    if (view_id !== undefined && view_id !== view?.id) throw new Error('View changed. Read companion_context before translating.');
    if (translations.length && (!view || view_id !== view.id)) throw new Error('Translations require the current view_id.');
    const allowed = new Map(viewTexts(view || {}).map(f => [f.path, f]));
    const patch = {};
    for (const entry of translations) {
      const field = entry && allowed.get(entry.path);
      if (!field || typeof entry.text !== 'string' || !entry.text.trim() || entry.text.length > field.maxLength || Object.hasOwn(patch,entry.path)) throw new Error('Invalid translation path, text or duplicate. Use pending fields from companion_set_language.');
      patch[entry.path] = entry.text;
    }
    // Validate the complete patch before changing language or any visible copy.
    if (translations.length) {
      view.translations ||= {};
      view.translations[language] = { ...view.translations[language], ...patch };
      view.copyRevision = (view.copyRevision || 0) + 1;
      this.rememberView(view);
    }
    this.update({ language });
    return this.languageContext();
  }
  requireClient() { if (!this.client) throw new Error('Call companion_open first.'); }
  async projects() { this.requireClient(); return this.client.call('list_performance_projects'); }
  async select(project_id, { guided = true } = {}) {
    this.requireClient();
    const projects = await this.projects();
    if (!projects.projects?.some(p => p.project_id === project_id)) throw new Error('Unknown project. Use companion_projects.');
    const result = await this.client.call('start_performance_analysis', { project_id, language: this.state.language });
    if (!result.analysis_id) throw new Error('MCP did not return an analysis_id.');
    this.analysis = result.analysis_id;
    this.resetChapterViews();
    this.update({ project: project_id, analysis_id: this.analysis, audit: null, viewed_chapter_id: null, selection: {}, views: [], current: null, note: '' });
    await this.tools();
    if (guided && this.state.audit_support === 'available') await this.startAudit();
    return { project_id, analysis_id: this.analysis, audit_support: this.state.audit_support, audit: this.state.audit };
  }
  async tools() {
    this.requireClient();
    this.upstreamSchemas = await this.client.tools();
    this.update({ audit_support: ['start_audit', 'get_audit_status'].every(name => this.upstreamSchemas.some(t => t.name === name)) ? 'available' : 'unsupported' });
    this.schemas = this.upstreamSchemas.filter(t => ALLOWED.has(t.name)).map(t => {
      const schema = structuredClone(t.inputSchema);
      for (const k of ['analysis_id', 'project_id']) delete schema.properties?.[k];
      if (schema.required) schema.required = schema.required.filter(k => !['analysis_id', 'project_id'].includes(k));
      return { name: t.name, description: t.description, inputSchema: schema };
    });
    return { tools: this.schemas, audit_support: this.state.audit_support };
  }
  async requireAuditTool(name) {
    this.requireClient();
    if (!this.upstreamSchemas.length) await this.tools();
    if (!this.upstreamSchemas.some(t => t.name === name))
      throw new Error(`Guided audit tool ${name} is unavailable on this server. Update JAS-MIN or continue an ordinary evidence discussion; audit briefs are not being saved.`);
  }
  applyAudit(status, project_id = this.state.project) {
    if (status?.audit_started === false) throw new Error('This analysis has no guided audit yet. Start its audit in JAS-MIN before resuming it in Companion.');
    if (!status || typeof status.analysis_id !== 'string' || !Array.isArray(status.project_ids) || !Array.isArray(status.chapters) || !Array.isArray(status.briefs))
      throw new Error('The server returned an incompatible audit status. Update the server and Companion together.');
    const project = project_id || (status.project_ids.length === 1 ? status.project_ids[0] : null);
    if (!project || !status.project_ids.includes(project)) throw new Error('Select a project_id belonging to this audit. No project identity is inferred from row order.');
    const changed = this.analysis !== status.analysis_id || this.state.project !== project;
    const previousChapter = this.viewedChapter();
    const previousView = this.state.views.find(v => v.id === this.state.current);
    const previouslyCurrent = previousView?.brief_id && this.isCurrentBriefView(previousView);
    if (changed) this.resetChapterViews();
    this.analysis = status.analysis_id;
    this.update({ analysis_id: this.analysis, project, audit: structuredClone(status), audit_support: 'available',
      ...(changed ? { viewed_chapter_id: null, selection: {}, views: [], current: null, note: '' } : {}) });
    const chapter = this.viewedChapter();
    if (changed || chapter !== previousChapter || (previouslyCurrent && !this.isCurrentBriefView(previousView))) {
      this.browseChapter(chapter);
    }
    return status;
  }
  viewedChapter() {
    return this.state.viewed_chapter_id ?? this.state.audit?.current_chapter_id ?? null;
  }
  isCurrentBriefView(view) {
    return !view.brief_id || this.state.audit?.briefs.some(b => b.project_id === this.state.project &&
      b.brief_id === view.brief_id && b.visual && (b.revision ?? b.updated_at_revision) === view.brief_revision);
  }
  lookupView(id) {
    return this.state.views.find(v => v.id === id) ?? this.archivedViews.get(id);
  }
  displayedBoard() {
    const view = this.lookupView(this.state.current);
    if (!view) return null;
    const localized = localizedView(view, this.state.language);
    const board = localized.presentation || automaticPresentation(view, this.state.language);
    return { view_id: view.id, chapter_id: view.chapter_id, brief_id: view.brief_id, brief_revision: view.brief_revision,
      superseded: !this.isCurrentBriefView(view), note: localized.note || '', evidence_id: view.data.evidence_id,
      evidence_refs: view.data.evidence_refs, source_view_ids: view.data.source_view_ids, source_view_id: board.source_view_id,
      title: board.title, takeaway: board.takeaway, panels: board.panels };
  }
  rememberView(view) {
    if (!view.chapter_id) return;
    let archived = this.archivedViews.get(view.id);
    if (!archived) {
      const presentation = view.presentation || automaticPresentation(view);
      if (!presentation.panels.length) return;
      // Store the rendered board and its provenance. Full measurement envelopes stay in the recent cache.
      archived = { id: view.id, tool: view.tool, args: {}, time: view.time, language: view.language || 'en',
        presentation, data: { evidence_id: view.data.evidence_id, evidence_refs: view.data.evidence_refs,
          source_view_ids: view.data.source_view_ids } };
      if (!view.presentation) {
        const fields = viewTexts({ presentation: automaticPresentation(view, 'pl') });
        archived.translations = { pl: Object.fromEntries(fields.map(f => [f.path, f.text])) };
        archived.automaticTextPaths = fields.map(f => f.path);
      }
      this.archivedViews.set(view.id, archived);
    }
    if (archived.note !== view.note) for (const entries of Object.values(archived.translations || {})) delete entries.note;
    Object.assign(archived, { chapter_id: view.chapter_id, brief_id: view.brief_id, brief_revision: view.brief_revision,
      note: view.note, noteLanguage: view.noteLanguage, copyRevision: view.copyRevision });
    if (view.presentation) {
      archived.translations = view.translations;
      archived.automaticTextPaths = view.automaticTextPaths;
    } else if (view.translations) {
      for (const [language, entries] of Object.entries(view.translations)) {
        archived.translations[language] = { ...archived.translations[language], ...entries };
      }
    }
    this.lastViewByChapter.set(view.chapter_id, view.id);
  }
  chapterViews() {
    const chapter = this.viewedChapter();
    if (!chapter) return [];
    const rows = [];
    for (const v of this.archivedViews.values()) {
      if (v.chapter_id !== chapter) continue;
      const title = v.translations?.[this.state.language]?.['presentation.title'] ?? v.presentation.title;
      rows.push({ view_id: v.id, brief_id: v.brief_id, brief_revision: v.brief_revision, superseded: !this.isCurrentBriefView(v), title, evidence_id: v.data.evidence_id });
    }
    for (const b of this.state.audit.briefs) {
      if (b.project_id === this.state.project && b.chapter_id === chapter && b.visual && !rows.some(v => v.brief_id === b.brief_id && !v.superseded)) {
        rows.push({ brief_id: b.brief_id, title: b.visual.title });
      }
    }
    return rows;
  }
  browseChapter(chapter_id, brief_id) {
    const audit = this.state.audit;
    if (!audit?.chapters.some(c => c.chapter_id === chapter_id)) throw new Error('Unknown chapter in this audit.');
    const briefs = audit.briefs.filter(b => b.chapter_id === chapter_id && b.project_id === this.state.project && b.visual);
    let view = this.lookupView(this.lastViewByChapter.get(chapter_id));
    if (view && !this.isCurrentBriefView(view)) view = null;
    if (brief_id !== undefined || !view) {
      let brief;
      if (brief_id !== undefined) {
        brief = briefs.find(b => b.brief_id === brief_id);
        if (!brief) throw new Error('No saved visual for this brief in the selected chapter.');
      } else {
        const newest = Math.max(0, ...briefs.map(b => b.updated_at_revision || 0));
        const candidates = briefs.filter(b => (b.updated_at_revision || 0) === newest);
        if (candidates.length === 1) brief = candidates[0];
      }
      if (brief) {
        const revision = brief.revision ?? brief.updated_at_revision;
        const previous = Array.from(this.archivedViews.values()).find(v => v.brief_id === brief.brief_id && v.brief_revision === revision);
        view = (previous && this.lookupView(previous.id)) || {
          id: crypto.randomUUID(), tool: 'record_audit_brief', args: {}, chapter_id, brief_id: brief.brief_id,
          brief_revision: revision, data: { evidence_refs: brief.evidence_refs }, language: brief.visual.language || this.state.language,
          presentation: structuredClone(brief.visual),
        };
      }
    }
    this.update({ viewed_chapter_id: chapter_id === audit.current_chapter_id ? null : chapter_id,
      current: null, note: '', selection: {} });
    if (view) this.record(view);
  }
  async auditStatus() {
    await this.requireAuditTool('get_audit_status');
    if (!this.analysis) throw new Error('Select a project or resume an audit first.');
    return this.applyAudit(await this.client.call('get_audit_status', { analysis_id: this.analysis }));
  }
  async auditMutation(name, args = {}) {
    await this.requireAuditTool(name);
    if (!this.analysis) throw new Error('Select a project or resume an audit first.');
    await this.client.call(name, { ...args, analysis_id: this.analysis });
    return this.auditStatus();
  }
  async startAudit() { return this.auditMutation('start_audit'); }
  async recordBrief({ view_id, ...brief }) {
    if (!this.state.project) throw new Error('Select a project first.');
    if (view_id !== undefined) {
      const view = this.lookupView(view_id);
      if (!view || !this.isCurrentBriefView(view)) throw new Error('Unknown view in this audit.');
      if (translationStatus(view, this.state.language).pending.some(field => field.path.startsWith('presentation.')))
        throw new Error('This board still needs translation. Show it and complete the pending translations before attaching it to the audit brief.');
      brief.visual = structuredClone(localizedView(view, this.state.language).presentation || automaticPresentation(view, this.state.language));
      brief.visual.source_view_id = view.id;
      brief.visual.language = this.state.language;
      if (Buffer.byteLength(JSON.stringify(brief.visual), 'utf8') > 128 * 1024)
        throw new Error('The audit visual exceeds 128 KiB. Select a narrower chart or save the explanation without view_id; evidence references are retained independently.');
    }
    const status = await this.auditMutation('record_audit_brief', { ...brief, project_id: this.state.project });
    if (view_id !== undefined) {
      const view = this.lookupView(view_id);
      if (view) {
        // A discussion may have revisited another topic before its board was attached.
        if (view.chapter_id !== brief.chapter_id && this.lastViewByChapter.get(view.chapter_id) === view.id) this.lastViewByChapter.delete(view.chapter_id);
        view.chapter_id = brief.chapter_id;
        const saved = status.briefs.find(b => b.project_id === this.state.project && b.chapter_id === brief.chapter_id &&
          b.visual?.source_view_id === view.id && b.updated_at_revision === status.revision);
        if (saved) {
          view.brief_id = saved.brief_id;
          view.brief_revision = saved.revision ?? saved.updated_at_revision;
        }
        this.rememberView(view);
        if (this.state.current === view_id) {
          this.update({ viewed_chapter_id: brief.chapter_id === status.current_chapter_id ? null : brief.chapter_id });
        }
      }
    }
    return status;
  }
  async advanceAudit(args) {
    const status = await this.auditMutation('advance_audit', args);
    if (this.state.viewed_chapter_id) this.browseChapter(status.current_chapter_id);
    return status;
  }
  async authorizeReport(args) { return this.auditMutation('authorize_report', args); }
  async exportAudit() {
    await this.requireAuditTool('export_audit');
    if (!this.analysis) throw new Error('Select a project or resume an audit first.');
    return this.client.call('export_audit', { analysis_id: this.analysis });
  }
  async resumeAudit({ analysis_id, project_id }) {
    await this.requireAuditTool('get_audit_status');
    const status = await this.client.call('get_audit_status', { analysis_id });
    return this.applyAudit(status, project_id || (status.project_ids?.length === 1 ? status.project_ids[0] : null));
  }
  async restoreAudit({ checkpoint, project_id }) {
    await this.requireAuditTool('restore_audit');
    if (Buffer.byteLength(JSON.stringify(checkpoint), 'utf8') > 64 * 1024 * 1024) throw new Error('The audit checkpoint exceeds the core 64 MiB restore limit.');
    const status = await this.client.call('restore_audit', { checkpoint });
    return this.applyAudit(status, project_id || (status.project_ids?.length === 1 ? status.project_ids[0] : null));
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
    view.chapter_id ??= this.viewedChapter();
    const current = this.state.views.find(v => v.id === this.state.current);
    const keep = this.state.views.filter(v => v.id !== view.id && v.id !== current?.id).slice(-18);
    if (activate) this.rememberView(view);
    this.update({ views: [...keep, ...(current && current.id !== view.id ? [current] : []), view],
      ...(activate ? { current: view.id, note: view.note || '', selection: {} } : {}) });
  }
  present({ title, takeaway, panels = [], view_ids = [], language = this.state.language }) {
    this.validateLanguage(language);
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
    view.language = language;
    view.translations = { en: {}, pl: {} };
    view.automaticTextPaths = [];
    const automaticPL = view_ids.flatMap(id => automaticPresentation(this.state.views.find(v => v.id === id), 'pl').panels);
    for (const [locale, auto] of [['en', referenced], ['pl', automaticPL]]) {
      for (const field of viewTexts({ presentation: { panels: auto } })) {
        view.translations[locale][field.path] = field.text;
        if (locale === 'en') view.automaticTextPaths.push(field.path);
      }
    }
    this.update({ language });
    this.record(view);
    return { view_id: view.id, displayed: true, panel_count: combined.length };
  }
  note(text, language = this.state.language) {
    this.validateLanguage(language);
    const view = this.state.views.find(v => v.id === this.state.current);
    if (view) {
      view.note = text; view.noteLanguage = language;
      for (const entries of Object.values(view.translations || {})) delete entries.note;
      this.rememberView(view);
    }
    this.update({ note: text, language });
  }
  show(view_id) {
    const view = this.lookupView(view_id);
    if (!view) throw new Error('Unknown view in this session.');
    if (view.chapter_id) {
      this.update({ viewed_chapter_id: view.chapter_id === this.state.audit.current_chapter_id ? null : view.chapter_id });
    }
    this.record(view);
    return { view_id, ...this.languageContext() };
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
