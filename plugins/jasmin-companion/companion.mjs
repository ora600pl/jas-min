import { McpClient } from './mcp-client.mjs';
import { DemoClient } from './demo.mjs';
import { automaticPresentation, localizedView, viewTexts, translationStatus } from './public/charts.js';
import { topicBriefs } from './public/audit.js';
import { validatePresentation } from './presentation.mjs';
import { panelEntitiesByProject, panelTextChunks } from './discussed-entities.mjs';
import { createHash } from 'node:crypto';
import { PreviewStore } from './preview-store.mjs';

const legacyCombinedFiles = ['index.html', 'tx.html', 'tx-evidence.html', 'cpu.html', 'cpu-evidence.html', 'report.css'];
const chartPage = /^(?:wykresy\/)?(?:(?:tx|cpu)-chart-[1-9][0-9]*|sql-chart-[1-9][0-9]*-[1-9][0-9]*)\.html$/;
const chartAssets = ['ora-600.png', 'report.css', 'plotly.min.js', 'jasmin-plot.js'];
export function isReportChart(name) { return chartPage.test(name); }
export function isClientReportChart(name) {
  return /^wykresy\/sql-(?:(?:chart-)?[1-9][0-9]*-[1-9][0-9]*|p[1-9][0-9]*-[a-z0-9]{13})\.html$/.test(name) ||
    /^wykresy\/wait-p[1-9][0-9]*-[a-f0-9]{64}\.html$/.test(name);
}
function clientReportAsset(name) {
  return name === 'raport.html' || isClientReportChart(name) || ['wykresy/plotly.min.js', 'wykresy/jasmin-plot.js'].includes(name);
}
function combinedAsset(name) {
  return legacyCombinedFiles.includes(name) || isReportChart(name) ||
     name === 'charts.html' || chartAssets.includes(name) || name === 'wykresy/report-nav.js' ||
     (name.startsWith('wykresy/') && chartAssets.includes(name.slice('wykresy/'.length)));
}
const pilotFiles = ['index.html', 'evidence.html', 'report.css'];
const emptyPublications = { tx_preview: null, tx_release: null, cpu_preview: null, cpu_release: null, combined_preview: null, combined_release: null, client_preview: null, client_release: null };
const gradientOverview = { section: 'full_gradients', ranking: 'extreme', limit: 4 };
function isGradientOverview(args) {
  return args?.section === 'full_gradients' && !args.family && !args.contributor && !args.offset;
}
function retainedGradient(checkpoint, project) {
  return Object.values(checkpoint?.evidence || {}).find(record => record.project_id === project &&
    record.tool_name === 'get_precomputed_analysis' && isGradientOverview(record.arguments));
}
function requireSavedCheckpoint(status) {
  if (!status?.audit_id || status.snapshot?.persisted !== true || !Number.isSafeInteger(status.snapshot.revision) || status.snapshot.revision < 1)
    throw new Error('JAS-MIN did not confirm a checkpoint. Check the server workspace before restarting it.');
  return status;
}

export const ALLOWED = new Set(['get_database_load_summary', 'list_snapshots', 'list_available_metrics',
  'get_metric_time_series', 'get_wait_event_timeline', 'get_sql_timeline', 'top_wait_events_in_snapshot',
  'top_sqls_in_snapshot', 'get_sql_text', 'compare_snapshots', 'get_snapshot_details', 'get_diagnostic_guidance',
  'get_analysis_catalog', 'get_precomputed_analysis', 'get_db_instance_info', 'get_init_parameter',
  'get_wait_event_histogram', 'get_wait_event_sql_contributors', 'find_snapshots_with_event',
  'find_snapshots_with_sql', 'find_sqls_by_module', 'find_sqls_touching_object', 'search_sql_text',
  'list_available_sql_plans', 'get_sql_execution_plan', 'list_available_child_cursor_reasons',
  'get_child_cursor_reasons', 'get_alertlog_errors', 'top_segments_in_snapshot', 'top_latches_in_snapshot',
  'get_access_path_diagnostics', 'compare_project_metric', 'compare_project_sql', 'get_rac_cpu_scenario']);
export function localEndpoint(value) {
  const u = new URL(value);
  if (u.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(u.hostname) || u.username || u.password || u.hash)
    throw new Error('Use a local MCP endpoint: http://127.0.0.1:PORT/mcp (or an SSH tunnel).');
  return u.href;
}
export class Companion {
  constructor() {
    this.previews = new PreviewStore();
    this.state = { revision: 0, language: 'en', mode: null, status: 'idle', project: null, project_ids: [], analysis_id: null,
      audit: null, audit_support: 'unknown', saved_audits_available: false, tx_pilot_available: false, tx_assets_available: false, cpu_pilot_available: false, cpu_assets_available: false, combined_pilot_available: false, combined_assets_available: false, saved_audits: [], saved_audits_error: null,
      client_report_available: false, client_assets_available: false, ...emptyPublications, viewed_chapter_id: null, selection: {}, views: [], current: null, note: '' };
    this.lastDiscussedBoard = null;
    this.client = null; this.analysis = null; this.schemas = []; this.upstreamSchemas = [];
    this.lastViewByChapter = new Map();
    this.archivedViews = new Map();
    this.resetChapterViews();
  }
  resetChapterViews() {
    this.lastViewByChapter.clear(); this.archivedViews.clear();
    this.gradientLoaded = false; this.restoredGradient = null; this.state.gradient_error = null;
  }
  update(patch) { Object.assign(this.state, patch); this.state.revision++; }
  async open({ mode = 'live', endpoint = 'http://127.0.0.1:4242/mcp', language = this.state.language } = {}) {
    this.validateLanguage(language);
    if (!['demo', 'live'].includes(mode)) throw new Error('mode must be demo or live');
    if (mode === 'live') endpoint = localEndpoint(endpoint);
    if (this.client && mode === this.state.mode && (mode === 'demo' || endpoint === this.endpoint)) { this.setLanguage({ language }); return; }
    const client = mode === 'demo' ? new DemoClient() : new McpClient(endpoint);
    try { await client.initialize(); } catch (e) { await client.close().catch(() => {}); throw e; }
    await this.client?.close().catch(() => {});
    this.lastDiscussedBoard = null;
    this.client = client; this.endpoint = endpoint; this.analysis = null; this.schemas = []; this.upstreamSchemas = [];
    this.resetChapterViews();
    this.update({ mode, language, project: null, project_ids: [], analysis_id: null, audit: null, audit_support: 'unknown',
      saved_audits_available: false, tx_pilot_available: false, tx_assets_available: false, cpu_pilot_available: false, cpu_assets_available: false, combined_pilot_available: false, combined_assets_available: false, saved_audits: [], saved_audits_error: null, ...emptyPublications,
      client_report_available: false, client_assets_available: false, viewed_chapter_id: null, selection: {}, views: [], current: null, note: '' });
    await this.tools();
    if (this.state.saved_audits_available) {
      try { await this.listSavedAudits(); } catch (error) { this.update({ saved_audits_error: error.message }); }
    }
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
  async select(project_id, { guided = true, project_ids } = {}) {
    this.requireClient();
    const projects = await this.projects();
    if (!projects.projects?.some(p => p.project_id === project_id)) throw new Error('Unknown project. Use companion_projects.');
    if (project_ids && (!Array.isArray(project_ids) || !project_ids.includes(project_id) ||
        new Set(project_ids).size !== project_ids.length || project_ids.some(id => !projects.projects.some(p => p.project_id === id))))
      throw new Error('Select distinct known project_ids including the active project_id.');
    const result = await this.client.call('start_performance_analysis', {
      ...(project_ids ? { project_ids } : { project_id }), language: this.state.language,
    });
    if (!result.analysis_id) throw new Error('MCP did not return an analysis_id.');
    this.analysis = result.analysis_id;
    this.resetChapterViews();
    this.update({ project: project_id, project_ids: project_ids || [project_id], analysis_id: this.analysis, audit: null, ...emptyPublications, viewed_chapter_id: null, selection: {}, views: [], current: null, note: '' });
    await this.tools();
    if (guided && this.state.audit_support === 'available') await this.startAudit();
    return { project_id, analysis_id: this.analysis, audit_support: this.state.audit_support, audit: this.state.audit };
  }
  async tools() {
    this.requireClient();
    this.upstreamSchemas = await this.client.tools();
    const available = name => this.upstreamSchemas.some(t => t.name === name);
    this.update({ discussed_entities_available: available('record_discussed_entities'), audit_support: ['start_audit', 'get_audit_status'].every(available) ? 'available' : 'unsupported',
      saved_audits_available: ['list_saved_audits', 'resume_saved_audit'].every(available),
      tx_pilot_available: ['enable_tx_pilot', 'list_saved_audits', 'resume_saved_audit'].every(available),
      tx_assets_available: available('read_tx_report_asset'),
      cpu_pilot_available: ['enable_cpu_pilot', 'list_saved_audits', 'resume_saved_audit'].every(available),
      cpu_assets_available: available('read_cpu_report_asset'),
      combined_pilot_available: ['enable_combined_pilot', 'switch_audit_topic', 'resume_saved_audit', 'list_saved_audits'].every(available),
      client_report_available: ['save_client_report_plan', 'get_client_report_plan', 'preview_client_report'].every(available),
      client_assets_available: available('read_client_report_asset'),
      combined_assets_available: available('read_combined_report_asset') });
    this.schemas = this.upstreamSchemas.filter(t => ALLOWED.has(t.name)).map(t => {
      const schema = structuredClone(t.inputSchema);
      for (const k of ['analysis_id', 'project_id']) delete schema.properties?.[k];
      if (schema.required) schema.required = schema.required.filter(k => !['analysis_id', 'project_id'].includes(k));
      return { name: t.name, description: t.description, inputSchema: schema };
    });
    return { tools: this.schemas, discussed_entities_available: this.state.discussed_entities_available, audit_support: this.state.audit_support, saved_audits_available: this.state.saved_audits_available, tx_pilot_available: this.state.tx_pilot_available, cpu_pilot_available: this.state.cpu_pilot_available, combined_pilot_available: this.state.combined_pilot_available, client_report_available: this.state.client_report_available };
  }
  async requireAuditTool(name) {
    this.requireClient();
    if (!this.upstreamSchemas.length) await this.tools();
    if (!this.upstreamSchemas.some(t => t.name === name))
      throw new Error(`Guided audit tool ${name} is unavailable on this server. Update JAS-MIN or continue an ordinary evidence discussion; audit briefs are not being saved.`);
  }
  async recordDiscussedEntities({ project_id = this.state.project, sql_ids = [], wait_events = [] }) {
    this.requireClient();
    if (!this.upstreamSchemas.length) await this.tools();
    if (!this.state.discussed_entities_available)
      throw new Error('This JAS-MIN core cannot register discussion mentions. Keep their project and exact names in an audit brief.');
    if (!this.state.audit || !this.state.project_ids.includes(project_id)) throw new Error('Choose a project in the current guided audit.');
    if (!Array.isArray(sql_ids) || sql_ids.some(id => typeof id !== 'string' || !/^[0-9abcdfghjkmnpqrstuvwxyz]{13}$/i.test(id)) ||
        !Array.isArray(wait_events) || wait_events.some(name => typeof name !== 'string' || !name.trim()) ||
        (!sql_ids.length && !wait_events.length)) throw new Error('Supply Oracle SQL IDs or exact wait event names.');
    const args = { sql_ids: [...new Set(sql_ids.map(id => id.toLowerCase()))], wait_events: [...new Set(wait_events.map(name => name.trim()))] };
    const result = await this.client.call('record_discussed_entities', { analysis_id: this.analysis, project_id, ...args });
    if (result.revision !== this.state.audit.snapshot?.revision) await this.auditStatus();
    return result;
  }
  async captureRawEvidence({ analysis_id, project_id, view_id }) {
    if (!this.state.discussed_entities_available || !this.state.audit) return;
    if (analysis_id !== this.analysis || project_id !== this.state.project)
      throw new Error('The raw view belongs to another audit or project.');
    const view = this.lookupView(view_id);
    if (!view) throw new Error('The evidence view is no longer available.');
    if (view.presentation || view.rawEntitiesRecorded) return;
    const comparative = view.args?.project_ids || [view.args?.baseline_project_id, view.args?.candidate_project_id].filter(Boolean);
    const projects = view.data.project_id ? [view.data.project_id] : comparative.length ? comparative : [project_id];
    let revision;
    for (const id of new Set(projects)) {
      if (!this.state.project_ids.includes(id)) throw new Error('The raw evidence contains a project outside this audit.');
      // Preview-only views have no retained evidence ID. Their displayed JSON
      // still belongs to the project that answered the original request.
      const parts = view.data.evidence_id ? [{ evidence_ref: view.data.evidence_id }] :
        [...panelTextChunks([JSON.stringify(view.data.result ?? view.data)])].map(panel_text => ({ panel_text }));
      for (const part of parts) {
        const result = await this.client.call('record_discussed_entities', { analysis_id, project_id: id, ...part });
        revision = result.revision;
      }
    }
    if (revision !== undefined && revision !== this.state.audit.snapshot?.revision) await this.auditStatus();
    view.rawEntitiesRecorded = true;
  }
  async captureDisplayedEntities() {
    if (!this.state.audit || !this.state.discussed_entities_available) return;
    const { project, current, note, language } = this.state;
    const view = this.lookupView(current);
    const key = JSON.stringify([this.analysis, project, current, language, view?.copyRevision, note, view?.note_project_id ?? this.state.note_project_id]);
    if (key === this.lastDiscussedBoard) return;
    const board = this.displayedBoard() || { note };
    // The core already indexes saved briefs and their superseded versions.
    // Reopening one is read-only. A new note or authored translation may add names.
    const content = view?.brief_id && !view.copyRevision ? { note: board.note } : board;
    await this.captureBoardContent(content, view ? view.note_project_id : this.state.note_project_id);
    this.lastDiscussedBoard = key;
  }
  async captureBoardContent(board, noteProject) {
    const { project, project_ids } = this.state;
    const groups = panelEntitiesByProject(board, project, project_ids, noteProject);
    let revision;
    for (const [project_id, group] of groups) {
      if (!project_ids.includes(project_id)) throw new Error('Displayed panel references a project outside this audit.');
      let typed = { sql_ids: [...group.sql_ids], wait_events: [...group.wait_events] };
      for (const panel_text of panelTextChunks(group.fields)) {
        const result = await this.client.call('record_discussed_entities', { analysis_id: this.analysis, project_id, panel_text, ...typed });
        typed = {};
        revision = result.revision;
      }
    }
    if (revision !== undefined && revision !== this.state.audit.snapshot?.revision) await this.auditStatus();
  }
  async applyAudit(status, project_id = this.state.project, checkpoint) {
    if (status?.audit_started === false) throw new Error('This analysis has no guided audit yet. Start its audit in JAS-MIN before resuming it in Companion.');
    if (!status || typeof status.analysis_id !== 'string' || !Array.isArray(status.project_ids) || !Array.isArray(status.chapters) || !Array.isArray(status.briefs))
      throw new Error('The server returned an incompatible audit status. Update the server and Companion together.');
    const project = project_id || (status.project_ids.length === 1 ? status.project_ids[0] : null);
    if (!project || !status.project_ids.includes(project)) throw new Error('Select a project_id belonging to this audit. No project identity is inferred from row order.');
    const changed = this.analysis !== status.analysis_id || this.state.project !== project || this.state.audit?.active_topic !== status.active_topic;
    const previousChapter = this.viewedChapter();
    const previousView = this.state.views.find(v => v.id === this.state.current);
    const previouslyCurrent = previousView?.brief_id && this.isCurrentBriefView(previousView);
    if (changed) this.resetChapterViews();
    if (checkpoint) this.restoredGradient = retainedGradient(checkpoint, project);
    const snapshotChanged = this.state.audit?.snapshot?.revision !== status.snapshot?.revision;
    this.analysis = status.analysis_id;
    if (status.audit_id && status.snapshot?.persisted) {
      const row = { audit_id: status.audit_id, active_topic: status.active_topic, pilot_kind: status.pilot_kind || status.snapshot.pilot_kind || 'tx', revision: status.snapshot.revision, project_ids: status.project_ids,
        current_chapter_id: status.current_chapter_id, focus_project_id: status.resume_context?.focus_project_id, updated_at: status.snapshot.updated_at };
      const { saved_audits } = this.state;
      this.state.saved_audits = [row, ...saved_audits.filter(entry => entry.audit_id !== row.audit_id)];
    }
    this.update({ analysis_id: this.analysis, project, project_ids: status.project_ids, audit: structuredClone(status), audit_support: 'available',
      ...(changed || snapshotChanged ? emptyPublications : {}),
      ...(changed ? { viewed_chapter_id: null, selection: {}, views: [], current: null, note: '' } : {}) });
    const chapter = this.viewedChapter();
    if (changed || chapter !== previousChapter || (previouslyCurrent && !this.isCurrentBriefView(previousView))) {
      const focus = changed && status.resume_context?.focus_brief_id;
      const saved = focus && topicBriefs(status).find(b => b.brief_id === focus && b.chapter_id === chapter && b.project_id === project && b.visual);
      await this.browseChapter(chapter, saved ? saved.brief_id : undefined);
    }
    return status;
  }
  viewedChapter() {
    return this.state.viewed_chapter_id ?? this.state.audit?.current_chapter_id ?? null;
  }
  isCurrentBriefView(view) {
    return !view.brief_id || topicBriefs(this.state.audit).some(b => b.project_id === this.state.project &&
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
      opening_gradient: view.opening_gradient,
      note: view.note, note_project_id: view.note_project_id, noteLanguage: view.noteLanguage, copyRevision: view.copyRevision });
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
    for (const b of topicBriefs(this.state.audit)) {
      if (b.project_id === this.state.project && b.chapter_id === chapter && b.visual && !rows.some(v => v.brief_id === b.brief_id && !v.superseded)) {
        rows.push({ brief_id: b.brief_id, title: b.visual.title });
      }
    }
    return rows;
  }
  async openingGradient() {
    const tool = 'get_precomputed_analysis';
    const schema = this.schemas.find(entry => entry.name === tool)?.inputSchema;
    if (!schema) throw new Error('Gradient evidence is unavailable on this server.');
    const collected = this.state.views.find(view => view.tool === tool && view.data.result && isGradientOverview(view.args));
    let retained = collected && { ...collected.data, arguments: collected.args };
    if (!retained && !schema.properties?.preview_only) retained = this.restoredGradient;
    if (!retained && !schema.properties?.preview_only && this.state.audit.report_authorized) {
      // A legacy core can invalidate a reviewed report when collecting new evidence.
      retained = retainedGradient((await this.exportAudit()).checkpoint, this.state.project);
      if (!retained) throw new Error('The audit has no saved regression overview. Update JAS-MIN to preview models.');
    }
    const args = retained?.arguments || gradientOverview;
    const data = retained || await this.client.call(tool, {
      ...args, ...(schema.properties?.preview_only ? { preview_only: true } : {}),
      analysis_id: this.analysis, project_id: this.state.project,
    });
    return { id: crypto.randomUUID(), tool, args, data: { result: data.result, evidence_id: data.evidence_id },
      time: new Date().toISOString(), chapter_id: 'performance_profile', opening_gradient: true };
  }
  async browseChapter(chapter_id, brief_id) {
    const audit = this.state.audit;
    if (!audit?.chapters.some(c => c.chapter_id === chapter_id)) throw new Error('Unknown chapter in this audit.');
    const briefs = topicBriefs(audit).filter(b => b.chapter_id === chapter_id && b.project_id === this.state.project && b.visual);
    let view = this.lookupView(this.lastViewByChapter.get(chapter_id));
    if (view && !this.isCurrentBriefView(view)) view = null;
    if (chapter_id === 'performance_profile' && brief_id === undefined && !this.gradientLoaded) {
      try {
        view = await this.openingGradient();
        this.gradientLoaded = true;
        this.state.gradient_error = null;
      } catch (e) {
        this.state.gradient_error = e.message;
      }
    }
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
    const status = await this.client.call('get_audit_status', { analysis_id: this.analysis });
    if (status.active_topic && status.active_topic !== this.state.audit?.active_topic)
      return this.restoreFocus(status, status.resume_context?.focus_project_id);
    return this.applyAudit(status);
  }
  async auditMutation(name, args = {}) {
    await this.requireAuditTool(name);
    if (!this.analysis) throw new Error('Select a project or resume an audit first.');
    const result = await this.client.call(name, { ...args, analysis_id: this.analysis });
    if (result?.audit_id && result.snapshot?.persisted && Array.isArray(result.briefs)) return this.applyAudit(result);
    return this.auditStatus();
  }
  acceptsAuditFocus(tool) {
    return this.upstreamSchemas.some(t => t.name === tool && t.inputSchema?.properties?.focus_project_id);
  }
  async startAudit() {
    await this.requireAuditTool('start_audit');
    if (!this.acceptsAuditFocus('start_audit')) return this.auditMutation('start_audit');
    return requireSavedCheckpoint(await this.auditMutation('start_audit', { focus_project_id: this.state.project }));
  }
  async enablePilot(kind) {
    const tool = `enable_${kind}_pilot`;
    await this.requireAuditTool(tool);
    if (!this.analysis) throw new Error('Select a project first.');
    if (kind !== 'tx' && this.state.project_ids.length < 2) throw new Error('Select at least two RAC instances for the cluster CPU scenario.');
    const status = await this.client.call(tool, { analysis_id: this.analysis, focus_project_id: this.state.project });
    return this.applyAudit(status);
  }
  async switchAuditTopic(args) {
    await this.requireAuditTool('switch_audit_topic');
    if (this.state.audit?.pilot_kind !== 'combined') throw new Error('Open a combined audit before switching topics.');
    const status = await this.client.call('switch_audit_topic', { ...this.durableArgs(args), analysis_id: this.analysis });
    return this.restoreFocus(status, status.resume_context?.focus_project_id);
  }
  async listSavedAudits() {
    await this.requireAuditTool('list_saved_audits');
    const result = await this.client.call('list_saved_audits', {});
    if (!Array.isArray(result.audits)) throw new Error('Incompatible saved audit list.');
    this.update({ saved_audits: result.audits, saved_audits_error: null });
    return result;
  }
  async resumeSavedAudit({ audit_id, project_id, revision, as_general }) {
    await this.requireAuditTool('resume_saved_audit');
    const entry = (await this.listSavedAudits()).audits.find(a => a.audit_id === audit_id);
    if (!entry) throw new Error('Unknown saved audit. Refresh the saved audit list.');
    if (entry.unavailable && (!revision || revision >= entry.revision)) throw new Error('The latest snapshot is unavailable. Select an earlier snapshot revision.');
    if (project_id && entry.project_ids && !entry.project_ids.includes(project_id)) throw new Error('Choose a project_id belonging to this saved audit.');
    const status = await this.client.call('resume_saved_audit', { audit_id, ...(revision != null ? { revision } : {}), ...(as_general === true ? { as_general: true } : {}) });
    const project = project_id || status.resume_context?.focus_project_id || (status.project_ids.length === 1 ? status.project_ids[0] : null);
    return this.restoreFocus(status, project);
  }
  async restoreFocus(status, project, checkpoint) {
    await this.applyAudit(status, project, checkpoint);
    const saved = topicBriefs(status).find(b => b.brief_id === status.resume_context?.focus_brief_id && b.project_id === project);
    if (saved?.visual && this.displayedBoard()?.brief_id !== saved.brief_id) await this.browseChapter(saved.chapter_id, saved.brief_id);
    if (saved?.visual?.source_selection && this.displayedBoard()?.brief_id === saved.brief_id) this.selectPoint(saved.visual.source_selection, false);
    return status;
  }
  durableArgs(args) {
    if (!this.state.audit?.audit_id) return args;
    return { expected_revision: this.state.audit.snapshot.revision, operation_id: crypto.randomUUID(), ...args };
  }
  async recordBrief({ view_id, ...brief }) {
    if (!this.state.project) throw new Error('Select a project first.');
    if (view_id !== undefined) {
      const view = this.lookupView(view_id);
      if (!view || !this.isCurrentBriefView(view)) throw new Error('Unknown view in this audit.');
      if (translationStatus(view, this.state.language).pending.some(field => field.path.startsWith('presentation.')))
        throw new Error('This board still needs translation. Show it and complete the pending translations before attaching it to the audit brief.');
      brief.visual = structuredClone(localizedView(view, this.state.language).presentation || automaticPresentation(view, this.state.language));
      const retained = new Set(brief.evidence_refs);
      if (brief.visual.panels.some(panel => panel.evidence_refs?.some(ref => !retained.has(ref))))
        throw new Error('Include every selected panel evidence reference in the saved brief.');
      brief.visual.source_view_id = view.id;
      brief.visual.language = this.state.language;
      delete brief.visual.source_selection;
      if (this.state.audit?.audit_id && view_id === this.state.current && this.state.selection.snap_id != null)
        brief.visual.source_selection = structuredClone(this.state.selection);
      if (Buffer.byteLength(JSON.stringify(brief.visual), 'utf8') > 128 * 1024)
        throw new Error('The audit visual exceeds 128 KiB. Select a narrower chart or save the explanation without view_id; evidence references are retained independently.');
    }
    const status = await this.auditMutation('record_audit_brief', this.durableArgs({ ...brief, project_id: this.state.project }));
    if (view_id !== undefined) {
      const view = this.lookupView(view_id);
      if (view) {
        // A discussion may have revisited another topic before its board was attached.
        if (view.chapter_id !== brief.chapter_id && this.lastViewByChapter.get(view.chapter_id) === view.id) this.lastViewByChapter.delete(view.chapter_id);
        view.chapter_id = brief.chapter_id;
        const saved = topicBriefs(status).find(b => b.project_id === this.state.project && b.chapter_id === brief.chapter_id &&
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
    const status = await this.auditMutation('advance_audit', this.durableArgs(this.state.audit?.audit_id ?
      { focus_project_id: this.state.project, ...args } : args));
    if (this.state.viewed_chapter_id) await this.browseChapter(status.current_chapter_id);
    return status;
  }
  async authorizeReport(args) { return this.auditMutation('authorize_report', args); }
  async saveClientReportPlan(args) {
    if (!this.state.audit?.snapshot?.persisted) throw new Error('Open a saved guided audit before editing its report.');
    const status = await this.auditMutation('save_client_report_plan', this.durableArgs(args));
    return requireSavedCheckpoint(status);
  }
  async getClientReportPlan() {
    await this.requireAuditTool('get_client_report_plan');
    if (!this.analysis) throw new Error('Select a project or resume an audit first.');
    return this.client.call('get_client_report_plan', { analysis_id: this.analysis });
  }
  async moveClientChapter({ chapter_id, direction, expected_revision }) {
    const audit = this.state.audit;
    if (!Number.isSafeInteger(expected_revision) || expected_revision !== audit?.snapshot?.revision)
      throw new Error('Refresh the report outline before changing its order.');
    const plan = structuredClone(audit.client_report_plan);
    const chapters = plan?.chapters;
    const index = chapters?.findIndex(chapter => chapter.id === chapter_id) ?? -1;
    if (![-1, 1].includes(direction) || index < 0 || index + direction < 0 || index + direction >= chapters.length)
      throw new Error('Choose a report chapter and an available position.');
    [chapters[index], chapters[index + direction]] = [chapters[index + direction], chapters[index]];
    return this.saveClientReportPlan({ plan, expected_revision });
  }
  async previewClientReport() {
    await this.requireAuditTool('preview_client_report');
    if (!this.state.audit?.audit_id) throw new Error('Open a saved guided audit before previewing a report.');
    const preview = await this.client.call('preview_client_report', { analysis_id: this.analysis });
    if (!/^[a-f0-9]{64}$/.test(preview.preview_id) || preview.entry_file !== 'raport.html' || !Array.isArray(preview.files) || !preview.files.includes(preview.entry_file))
      throw new Error('The server returned an incompatible report preview.');
    return this.retainPreview('client', preview);
  }
  async previewPilotReport(kind, args) {
    const tool = `preview_${kind}_report`;
    await this.requireAuditTool(tool);
    if (!this.state.audit?.audit_id || (this.state.audit.pilot_kind || this.state.audit.snapshot?.pilot_kind || 'tx') !== kind)
      throw new Error(`Enable or resume a saved ${kind.toUpperCase()} audit first.`);
    const preview = await this.client.call(tool, { ...args, analysis_id: this.analysis });
    return this.retainPreview(kind, preview);
  }
  async retainPreview(kind, preview) {
    if (this.state[`${kind}_assets_available`]) {
      const legacyFiles = kind === 'combined' ? legacyCombinedFiles : pilotFiles;
      const files = preview.files ?? legacyFiles;
      const entry = kind === 'client' ? 'raport.html' : 'index.html';
      if (!Array.isArray(files) || !files.includes(entry)) throw new Error('Missing public report entry.');
      const allowed = kind === 'client' ? clientReportAsset : kind === 'combined' ? combinedAsset : name => pilotFiles.includes(name);
      const names = files.filter(name => typeof name === 'string' && allowed(name));
      const client = this.client, analysis = this.analysis;
      await this.previews.capture(kind, preview.preview_id, names,
        name => this.fetchReportAsset(client, analysis, kind, preview.preview_id, name));
    }
    this.update({ [`${kind}_preview`]: preview, [`${kind}_release`]: null });
    return preview;
  }
  async publishPilotReport(kind, { preview_id, user_request }) {
    const tool = `publish_${kind}_report`;
    await this.requireAuditTool(tool);
    if (!this.state.audit?.audit_id || !preview_id || preview_id !== this.state[`${kind}_preview`]?.preview_id)
      throw new Error(`Review the current ${kind.toUpperCase()} report preview in this panel before publishing.`);
    const release = await this.client.call(tool, { analysis_id: this.analysis, preview_id, user_request });
    this.update({ [`${kind}_release`]: release });
    return release;
  }
  async readPilotAsset(kind, preview_id, asset) {
    if (asset !== 'report.zip') return this.previews.read(kind, preview_id, asset);
    const release = this.state[`${kind}_release`];
    const preview = this.state[`${kind}_preview`];
    if (!this.state[`${kind}_assets_available`] || !this.analysis || preview?.preview_id !== preview_id || release?.preview_id !== preview_id) return null;
    return this.fetchReportAsset(this.client, this.analysis, kind, preview_id, asset, release.zip_sha256);
  }
  async fetchReportAsset(client, analysisId, kind, preview_id, asset, releaseHash) {
    const parts = [], hash = createHash('sha256');
    let offset = 0, size, sha256;
    do {
      const chunk = await client.call(`read_${kind}_report_asset`, { analysis_id: analysisId, preview_id, asset, offset });
      if (offset === 0) { size = chunk.bytes; sha256 = chunk.sha256; }
      if (!Number.isSafeInteger(size) || size < 0 || !/^[a-f0-9]{64}$/.test(sha256) ||
          chunk.bytes !== size || chunk.sha256 !== sha256 || chunk.offset !== offset || typeof chunk.content_base64 !== 'string')
        throw new Error('Inconsistent client artifact metadata.');
      const part = Buffer.from(chunk.content_base64, 'base64');
      offset += part.length;
      if (offset > size || (offset === size ? chunk.next_offset !== null : !part.length || chunk.next_offset !== offset))
        throw new Error('Incomplete client artifact.');
      hash.update(part); parts.push(part);
    } while (offset < size);
    if (hash.digest('hex') !== sha256 || (asset === 'report.zip' && releaseHash !== sha256))
      throw new Error('Client artifact checksum does not match the reviewed release.');
    return { buffer: Buffer.concat(parts, size), sha256 };
  }
  async exportAudit() {
    await this.requireAuditTool('export_audit');
    if (!this.analysis) throw new Error('Select a project or resume an audit first.');
    return this.client.call('export_audit', { analysis_id: this.analysis });
  }
  async resumeAudit({ analysis_id, project_id }) {
    await this.requireAuditTool('get_audit_status');
    let status = await this.client.call('get_audit_status', { analysis_id });
    const project = project_id || status.resume_context?.focus_project_id || (status.project_ids?.length === 1 ? status.project_ids[0] : null);
    if (status.audit_started && !status.audit_id && this.acceptsAuditFocus('start_audit')) {
      if (!project || !status.project_ids.includes(project)) throw new Error('Select a project_id belonging to this audit.');
      status = requireSavedCheckpoint(await this.client.call('start_audit', { analysis_id, focus_project_id: project }));
    }
    if (status.pilot_kind === 'combined' || status.pilot_kind === 'general') return this.restoreFocus(status, project);
    return this.applyAudit(status, project);
  }
  async restoreAudit({ checkpoint, project_id }) {
    await this.requireAuditTool('restore_audit');
    if (Buffer.byteLength(JSON.stringify(checkpoint), 'utf8') > 64 * 1024 * 1024) throw new Error('The audit checkpoint exceeds the core 64 MiB restore limit.');
    const focus = this.acceptsAuditFocus('restore_audit') && project_id;
    const status = await this.client.call('restore_audit', { checkpoint, ...(focus ? { focus_project_id: focus } : {}) });
    if (this.acceptsAuditFocus('restore_audit')) requireSavedCheckpoint(status);
    const project = project_id || status.resume_context?.focus_project_id || (status.project_ids?.length === 1 ? status.project_ids[0] : null);
    if (status.audit_id) return this.restoreFocus(status, project, checkpoint);
    return this.applyAudit(status, project, checkpoint);
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
    const comparative = ['compare_project_metric', 'compare_project_sql', 'get_rac_cpu_scenario'].includes(tool);
    const data = await this.client.call(tool, { ...bounded, analysis_id: this.analysis, ...(comparative ? {} : { project_id: this.state.project }) });
    const view = { id: crypto.randomUUID(), tool, args: bounded, data, time: new Date().toISOString() };
    const current = this.state.views.find(v => v.id === this.state.current);
    const board = automaticPresentation(view, this.state.language);
    const visual = board.panels.length > 0;
    const presentationActive = !!(current?.presentation || current?.opening_gradient);
    const activate = display && visual && !presentationActive;
    // Supporting charts also appear in the panel's history while a custom board stays in focus.
    if (visual && !activate && this.state.audit && this.state.discussed_entities_available) await this.captureBoardContent(board);
    this.record(view, activate);
    return { view_id: view.id, displayed: activate, presentation_active: presentationActive, ...data };
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
    const custom = panels.length ? validatePresentation({ title, takeaway, panels }, this.state.project_ids).panels : [];
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
  note(text, language = this.state.language, project_id = this.state.project) {
    if (project_id && !this.state.project_ids.includes(project_id)) throw new Error('Choose a project in the current analysis.');
    this.validateLanguage(language);
    const view = this.state.views.find(v => v.id === this.state.current);
    if (view) {
      view.note = text; view.note_project_id = project_id; view.noteLanguage = language;
      for (const entries of Object.values(view.translations || {})) delete entries.note;
      this.rememberView(view);
    }
    this.update({ note: text, note_project_id: project_id, language });
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
  selectPoint(selection, required = true) {
    const view = this.state.views.find(v => v.id === this.state.current);
    if (!view) throw new Error('No current view.');
    const board = view.presentation || automaticPresentation(view);
    const candidates = board.panels.flatMap(p => p.series?.flatMap(s => s.points) || p.selections || []);
    const matches = candidates.filter(p => p.snap_id === selection.snap_id &&
      (p.sql_id || null) === (selection.sql_id || null) && (p.instance || null) === (selection.instance || null));
    if (!matches.length) {
      if (!required) return;
      throw new Error('Selection is outside the current view.');
    }
    if (!selection.instance && board.panels.some(p => p.scatter && p.series.some(s => s.points.filter(v => v.snap_id === selection.snap_id).length > 1))) {
      if (!required) return;
      throw new Error('Instance identity is missing for this snapshot. Verify the source AWR before selecting a RAC record.');
    }
    const p = matches[0];
    this.update({ selection: { snap_id: p.snap_id, ...(p.sql_id ? { sql_id: p.sql_id } : {}), ...(p.instance ? { instance: p.instance } : {}) } });
  }
  async close() {
    try { await this.client?.close(); }
    finally { this.client = null; await this.previews.close(); }
  }
}
