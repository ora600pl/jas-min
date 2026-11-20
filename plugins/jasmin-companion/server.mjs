import http from 'node:http';
import metadata from './package.json' with { type: 'json' };
import { readFile, realpath } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { Companion, isReportChart, isClientReportChart } from './companion.mjs';
import { presentationProperties } from './presentation.mjs';

const definition = (name, description, properties = {}, required = []) => ({ name, description,
  inputSchema: { type: 'object', properties, required, additionalProperties: false },
  annotations: { readOnlyHint: !['companion_set_language', 'companion_present', 'companion_note', 'companion_show', 'companion_select_project', 'companion_open', 'companion_start_audit', 'companion_record_discussed_entities', 'companion_record_brief', 'companion_advance_audit', 'companion_authorize_report', 'companion_resume_audit', 'companion_restore_audit', 'companion_enable_tx_pilot', 'companion_resume_saved_audit', 'companion_preview_tx_report', 'companion_publish_tx_report', 'companion_enable_cpu_pilot', 'companion_preview_cpu_report', 'companion_publish_cpu_report', 'companion_enable_combined_pilot', 'companion_switch_audit_topic', 'companion_preview_combined_report', 'companion_publish_combined_report', 'companion_save_client_report_plan', 'companion_preview_client_report', 'companion_publish_client_report'].includes(name), destructiveHint: false, openWorldHint: false } });
const str = { type: 'string' };
const evidenceRefs = { type: 'array', items: str };
const integer = { type: 'integer', minimum: 0 };
const snapshotFields = {
  private_note: { type: 'string', description: 'Private correction, rationale or handoff question saved with this operation.' },
  expected_revision: { ...integer, description: 'Expected snapshot.revision. Defaults to the last revision seen by this panel.' },
  operation_id: { type: 'string', description: 'Reuse the same ID and arguments to retry an interrupted save.' },
};
const hypothesisId = { type: 'string', enum: ['H1', 'H2', 'H3'] };
const hypothesisText = { type: 'string', minLength: 1, maxLength: 4000 };
const rows = (properties, minItems) => ({
  type: 'array', minItems, maxItems: 3,
  items: { type: 'object', properties, required: Object.keys(properties), additionalProperties: false },
});
const hypothesisFields = {
  opening_hypotheses: rows({
    id: hypothesisId, statement: hypothesisText, gradient_signal: hypothesisText, workload_profile: hypothesisText,
    comparison_window: hypothesisText, test: hypothesisText,
  }, 2),
  hypothesis_gap: { ...hypothesisText, description: 'Explain the missing gradient or measured load evidence in the performance profile when it prevents opening hypotheses.' },
  hypothesis_checks: rows({ id: hypothesisId, reason: hypothesisText, evidence_refs: evidenceRefs }, 1),
  hypothesis_outcomes: rows({
    id: hypothesisId, status: { type: 'string', enum: ['confirmed', 'rejected', 'unresolved'] }, reason: hypothesisText, evidence_refs: evidenceRefs,
  }, 2),
};
const language = { type: 'string', enum: ['en', 'pl'], description: 'Language of the current user dialogue: pl for Polish, en for English. Follow prose, not SQL or quoted source text.' };
const pubFields = { brief_id: str, brief_revision: integer, title: str, owner: str, success_criterion: str };
const txSelection = {
  type: 'object', additionalProperties: false,
  properties: {
    ...pubFields,
    evidence: { type: 'array', minItems: 1, items: { type: 'object', additionalProperties: false, properties: {
      evidence_ref: str, instance_label: str, first_snap_id: integer, last_snap_id: integer, source_brief_id: str, source_brief_revision: integer,
    }, required: ['evidence_ref', 'instance_label', 'first_snap_id', 'last_snap_id'] } },
  },
  required: [...Object.keys(pubFields), 'evidence'],
};
const cpuSelection = {
  type: 'object', additionalProperties: false,
  properties: {
    ...pubFields, evidence_ref: str,
    instance_labels: { type: 'object', additionalProperties: str, description: 'Map each selected project ID to its distinct public instance label.' },
    include_sql_ids: { type: 'boolean', description: 'Include the SQL IDs in the client edition when selected for publication.' },
  },
  required: [...Object.keys(pubFields), 'evidence_ref', 'instance_labels'],
};
const object = (properties, required = Object.keys(properties)) => ({ type: 'object', properties, required, additionalProperties: false });
const selection = (kind, properties, optional = {}) =>
  object({ kind: { type: 'string', enum: [kind] }, ...properties, ...optional }, ['kind', ...Object.keys(properties)]);
const briefSelection = { brief_id: str, revision: { type: 'integer', minimum: 1 } };
const instanceMeasurement = { project_id: str, instance_label: str, label: str };
const chapter = object({
  id: str, title: str, question: str, conclusion: str, interpretation: str, limitation: str,
  owner: str, next_step: str, success_metric: str,
  briefs: { type: 'array', minItems: 1, items: object(briefSelection) },
  measurements: { type: 'array', items: { oneOf: [
    selection('sql_summary', { ...instanceMeasurement, sql_id: str }, { no_per_execution: { type: 'boolean' }, include_gets: { type: 'boolean', description: 'Show logical reads on intervals with matching elapsed, CPU and gets execution counts.' } }),
    selection('sql_family', { ...instanceMeasurement, module_prefix: str, text_contains: str }),
    selection('sql_excerpt', { ...instanceMeasurement, sql_id: str,
      fragments: { type: 'array', minItems: 1, maxItems: 6, items: { type: 'string', minLength: 1, maxLength: 1800 }, description: 'Literal passages from this captured SQL in source order.' },
      meaning: str, scaling: str }, {
      recommendation: object({ rationale: str, assumption: str, before: str, after: str }),
      verification: { type: 'string', description: 'Legacy field for an existing plan.' },
    }),
    selection('sql_intervals', { ...instanceMeasurement, sql_id: str, begin_snap_ids: { type: 'array', items: integer } }),
    selection('board_panel', { ...briefSelection, panel_index: { ...integer, description: 'Zero-based table panel index in the selected brief revision.' } }, { title: str, public_note: str }),
  ] } },
});
const reportPlan = object({ title: str, introduction: str, window_label: str, chapters: { type: 'array', minItems: 1, items: chapter } }, ['title', 'introduction', 'chapters']);
export const TOOLS = [
  definition('companion_open', 'Start the local avatar panel. Open the returned panel_url in the host browser or a local web browser. Live is default; demo is synthetic.', { mode: { enum: ['live', 'demo'], type: 'string' }, endpoint: str, language }),
  definition('companion_set_language', 'Follow the main conversation language without clearing evidence or selection. Returns pending authored text fields to translate. If pending is nonempty, translate each text (preserve SQL, identifiers and Oracle metric names), then call again with the returned view_id and translations. Originals and measurements stay unchanged; switching back reuses translations.', { language, view_id: str, translations: { type: 'array', items: { type: 'object', properties: { path: str, text: str }, required: ['path','text'], additionalProperties: false } } }, ['language']),
  definition('companion_projects', 'List actual JAS-MIN project IDs.'),
  definition('companion_select_project', 'Start an analysis and, by default, a guided audit with automatic checkpoints on current servers. For a comparative analysis supply project_ids and select one of them as the active project_id.', { project_id: str,
    project_ids: { type: 'array', minItems: 1, maxItems: 32, uniqueItems: true, items: str }, guided: { type: 'boolean', description: 'Default true; false starts an ordinary evidence discussion.' } }, ['project_id']),
  definition('companion_tools', 'Read schemas of available read-only JAS-MIN evidence tools.'),
  definition('companion_query', 'Collect evidence from JAS-MIN. Supported results become charts; an active presentation stays visible. Use companion_present to compose or replace the analytical board. Read companion_tools first.', { tool: str, arguments: { type: 'object' }, display: { type: 'boolean', description: 'False collects evidence without changing the visible view.' } }, ['tool']),
  definition('companion_present', 'Compose the side panel to support your explanation: related charts, baseline comparisons, tables and labelled mechanism flows. Match all authored copy to the current conversation language; pass language. Preserve source identifiers. Use source-verified numbers, units and instance identity; missing values are null. The board stays visible during subsequent queries. For line panels x is a numeric timestamp or ordered index, points strictly increasing per series. Separate different units into separate panels.', {...presentationProperties, language}, ['title', 'takeaway']),
  definition('companion_context', 'Read the project, viewed_chapter_id, viewed_brief_id, displayed view and snapshot/SQL selection. Use the viewed chapter for follow-up questions. audit.current_chapter_id tracks the saved audit progress.', {
    include_displayed_board: { type: 'boolean', description: 'Include the localized displayed board, measured points or rows, provenance and note for a follow-up about the selected view. Default false.' },
  }),
  definition('companion_show', 'Display a previously returned view_id from this chat.', { view_id: str }, ['view_id']),
  definition('companion_note', 'Display your short interpretation next to the evidence. Write the note in the current conversation language and pass language. Preserve source identifiers. Does not synthesize audio.', { text: { type: 'string', description: 'Concise panel interpretation in the dialogue language.', maxLength: 2000 }, language, project_id: str }, ['text']),
  definition('companion_start_audit', 'Start or continue the selected guided audit. On a current server this binds the existing analysis to its durable workspace and returns the snapshot receipt.'),
  definition('companion_audit_status', 'Read saved audit chapters, explanations, open questions and report authorization from the upstream analysis. Briefs survive display-history eviction.'),
  definition('companion_record_discussed_entities', 'Checkpoint all exact SQL IDs and Oracle wait names mentioned by either speaker for the final report, including passing mentions. Call once for each relevant project, including when no panel or brief is created.', {
    project_id: { type: 'string', description: 'Source project in the current audit. Defaults to the selected project.' },
    sql_ids: { type: 'array', items: { type: 'string', pattern: '^[0-9A-DF-HJKMNP-Za-df-hjkmnp-z]{13}$' } },
    wait_events: { type: 'array', items: str },
  }),
  definition('companion_record_brief', 'Save a human-readable explanation after a discussion or correction. Reuse brief_id to retain a revision history. Preserve uncertainty and exact evidence IDs. A saved chapter is discussed, not user-approved; changes revoke previous report authorization.', {
    brief_id: str, chapter_id: str, question: str, explanation: str, conclusion: str, evidence_boundary: str, next_step: str,
    open_questions: { type: 'array', items: str }, evidence_refs: evidenceRefs, ...hypothesisFields, ...snapshotFields,
    evidence_cards: { type: 'array', maxItems: 32, items: { type: 'object', additionalProperties: false,
      properties: { id: { type: 'string', pattern: '^[A-Za-z0-9_-]{1,64}$' }, kind: { type: 'string', enum: ['measurement', 'comparison', 'hypothesis', 'verification'] },
        title: { type: 'string', minLength: 1, maxLength: 180 }, body: { type: 'string', minLength: 1, maxLength: 8000 },
        evidence_refs: { type: 'array', minItems: 1, maxItems: 32, uniqueItems: true, items: str } },
      required: ['id', 'kind', 'title', 'body', 'evidence_refs'] } },
    view_id: { type: 'string', description: 'Optional current display-cache view to snapshot with the brief. The text and evidence references remain independently required.' },
  }, ['chapter_id', 'question', 'explanation', 'conclusion', 'evidence_boundary', 'next_step', 'open_questions', 'evidence_refs']),
  definition('companion_advance_audit', 'Move the active discussion. A current server writes this chapter transition to its private audit workspace and returns snapshot.path and the revision. To inspect a chapter without changing the active discussion use the chapter buttons in the panel.', { chapter_id: str, complete_current: { type: 'boolean' }, focus_brief_id: str, ...snapshotFields }, ['chapter_id']),
  definition('companion_authorize_report', 'Only after the user explicitly requests the final report: record their actual request. Never infer authorization from acknowledgements, finished chapters or an export request. Does not finalize or bypass report completeness checks.', { user_request: { type: 'string', description: 'Verbatim explicit request from the user to produce the report.' } }, ['user_request']),
  definition('companion_export_audit', 'Export a checkpoint with the saved audit and its evidence for later continuation. Save the returned checkpoint intact to a local artifact; an in-memory response alone is not durable. Does not authorize a report.'),
  definition('companion_resume_audit', 'Reconnect this panel to an existing analysis on the same running server. Restores the canonical audit state without creating another analysis. Pass project_id when the audit contains several projects.', { analysis_id: str, project_id: str }, ['analysis_id']),
  definition('companion_restore_audit', 'Restore a saved checkpoint after server restart. The core validates source projects and evidence and returns a new analysis identity. Preserve the returned identity for later work.', { checkpoint: { type: 'object' }, project_id: str }, ['checkpoint']),
  definition('companion_enable_tx_pilot', 'Select the TX lock workflow for this analysis and checkpoint it in the configured audit workspace.'),
  definition('companion_enable_cpu_pilot', 'Select the cluster CPU scenario workflow for the selected RAC instances and checkpoint it in the configured audit workspace.'),
  definition('companion_enable_combined_pilot', 'Start one durable audit for transaction waits and the cluster CPU scenario in a selected multi-instance RAC analysis.'),
  definition('companion_switch_audit_topic', 'Move the combined discussion to a topic, save a snapshot and restore its project, chapter, focused brief and board.', {
    topic: { type: 'string', enum: ['tx', 'cpu'] }, expected_revision: snapshotFields.expected_revision, operation_id: snapshotFields.operation_id,
  }, ['topic']),
  definition('companion_list_saved_audits', 'List durable audits available in the connected server workspace, including general, TX, CPU and combined audits after a restart.'),
  definition('companion_get_client_report_plan', 'Read the ordered client chapters, pinned source brief revisions and any stale selections on the current audit.'),
  definition('companion_save_client_report_plan', 'Save the complete ordered client report plan in the audit checkpoint. Edit the plan returned by companion_get_client_report_plan to change, add or reorder chapters. Select stored brief revisions and source panels or measured SQL populations. This saves the draft for preview.', {
    plan: reportPlan, expected_revision: snapshotFields.expected_revision, operation_id: snapshotFields.operation_id,
  }, ['plan']),
  definition('companion_preview_client_report', 'Generate a read-only client preview from the saved report plan and measured data. Returns the local panel_preview_url and the file on the MCP host. A request for this preview does not issue a ZIP.'),
  definition('companion_publish_client_report', 'Issue the reviewed offline report ZIP only on an explicit final publication request. Use the actual request and the current preview ID.', { preview_id: str, user_request: str }, ['preview_id', 'user_request']),
  definition('companion_resume_saved_audit', 'Continue a saved audit from disk with its correction, open questions and saved board. Select an earlier snapshot revision to start a separate branch.', { audit_id: str, project_id: str, revision: { type: 'integer', minimum: 1 }, as_general: { type: 'boolean', description: 'Explicitly migrate the latest saved single-topic audit to the general guided workflow.' } }, ['audit_id']),
  definition('companion_preview_tx_report', 'Prepare the exact offline client pages for a TX report from the selected corrected brief and verified wait evidence. Inspect the public conclusion, boundary, action, numbers and file list before publishing.', txSelection.properties, txSelection.required),
  definition('companion_preview_cpu_report', 'Prepare offline client pages for the paired RAC CPU scenario from its verified evidence and the current brief revision. Assign a public label to each instance.', cpuSelection.properties, cpuSelection.required),

  definition('companion_preview_combined_report', 'Prepare the single HTML client report with its chart directory from the current combined audit. On a request for the final report, review this preview and publish it with companion_publish_combined_report.', {
    title: str, tx: txSelection, cpu: cpuSelection,
  }, ['title', 'tx', 'cpu']),
  definition('companion_publish_tx_report', 'After the user requests the client ZIP, issue the reviewed TX pilot preview as an offline ZIP and return its path.', { preview_id: str, user_request: str }, ['preview_id', 'user_request']),
  definition('companion_publish_cpu_report', 'Issue the reviewed RAC CPU client ZIP at the user’s explicit request.', { preview_id: str, user_request: str }, ['preview_id', 'user_request']),
  definition('companion_publish_combined_report', 'Issue the reviewed combined report as an offline ZIP containing index.html and wykresy/. Use the request for the final client report as user_request; the same request authorizes this publication.', { preview_id: str, user_request: str }, ['preview_id', 'user_request']),
];

export async function createServer({ port = 0 } = {}) {
  const companion = new Companion();
  const token = randomBytes(24).toString('hex');
  const files = new Map([
    ['', ['public/index.html', 'text/html']], ['app.js', ['public/app.js', 'text/javascript']],
    ['style.css', ['public/style.css', 'text/css']], ['charts.js', ['public/charts.js', 'text/javascript']],
    ['audit.js', ['public/audit.js', 'text/javascript']], ['gradients.js', ['public/gradients.js', 'text/javascript']],
    ['cpu-scenario.js', ['public/cpu-scenario.js', 'text/javascript']],
    ['audit.css', ['public/audit.css', 'text/css']],
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
      if (req.method === 'GET' && route === 'state') return reply(200, { ...companion.state, chapter_views: companion.chapterViews() });
      if (req.method === 'GET' && route.startsWith('client/')) {
        const match = /^client\/(?:(cpu|combined|report)\/)?([a-f0-9]{64})\/((?:wykresy\/)?[a-z0-9][a-z0-9.-]*\.(?:html|css|png|js|zip))$/.exec(route);
        if (!match) return reply(404, { error: 'Unknown client artifact' });
        const [, routeKind = 'tx', previewId, name] = match;
        const kind = routeKind === 'report' ? 'client' : routeKind;
        // Reading a prepared page must not wait on an audit mutation or touch its MCP session.
        const read = () => companion.readPilotAsset(kind, previewId, name);
        const file = name === 'report.zip' ? await enqueue(read) : await read();
        if (!file) return reply(404, { error: 'Client artifact is unavailable in this panel' });
        const ext = name.slice(name.lastIndexOf('.') + 1);
        const mime = {html:'text/html; charset=utf-8', css:'text/css; charset=utf-8', js:'text/javascript; charset=utf-8', png:'image/png', zip:'application/zip'}[ext];
        let policy = "style-src 'self'; img-src 'self'";
        if (isReportChart(name) || (kind === 'client' && isClientReportChart(name))) policy = "script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:";
        if (kind === 'client' && name === 'raport.html') policy = "style-src 'self' 'unsafe-inline'; img-src 'self' data:";
        if (kind === 'combined' && name === 'index.html' && companion.previews.has(kind, previewId, 'wykresy/report-nav.js'))
          policy += "; script-src 'self'";
        res.setHeader('Content-Security-Policy', `default-src 'none'; ${policy}; form-action 'none'; frame-ancestors 'none'; base-uri 'none'`);
        res.setHeader('Content-Disposition', name === 'report.zip' ? `attachment; filename="jas-min-${kind}.zip"` : 'inline');
        res.writeHead(200, { 'Content-Type': mime, 'Content-Length': file.buffer.length, ETag: '"' + file.sha256 + '"' });
        return res.end(file.buffer);
      }
      if (req.method === 'POST' && ['selection', 'raw-evidence', 'show', 'chapter', 'advance', 'saved-audits', 'resume-saved', 'publish-tx', 'publish-cpu', 'publish-combined', 'audit-topic', 'move-report-chapter', 'preview-client-report'].includes(route)) {
        if (req.headers['x-companion-client'] !== 'panel') return reply(403, { error: 'Missing client header' });
        let body = '';
        for await (const part of req) { body += part; if (body.length > 4096) { reply(413, { error: 'Request too large' }); return; } }
        const args = JSON.parse(body);
        await enqueue(() => {
          if (route === 'raw-evidence') return companion.captureRawEvidence(args);
          if (route === 'selection') companion.selectPoint(args);
          else if (route === 'show') companion.show(args.view_id);
          else {
            if (args.analysis_id !== companion.analysis || args.project_id !== companion.state.project) throw new Error('The panel switched to another audit or project.');
            if (route === 'saved-audits') return companion.listSavedAudits();
            if (route === 'move-report-chapter') return companion.moveClientChapter(args);
            if (route === 'preview-client-report') return companion.previewClientReport();
            if (route === 'resume-saved') return companion.resumeSavedAudit({ audit_id: args.audit_id, project_id: args.resume_project_id, revision: args.revision });
            if (route.startsWith('publish-')) return companion.publishPilotReport(route.slice(8), args);
            if (route === 'audit-topic') {
              if (!['tx', 'cpu'].includes(args.topic) || !Number.isSafeInteger(args.expected_revision)) throw new Error('Refresh the saved audit before changing topics.');
              return companion.switchAuditTopic({ topic: args.topic, expected_revision: args.expected_revision });
            }
            if (route === 'advance') {
              if (!companion.state.audit?.audit_id || !Number.isSafeInteger(args.expected_revision)) throw new Error('Refresh the saved audit before changing chapters.');
              return companion.advanceAudit({ chapter_id: args.chapter_id, expected_revision: args.expected_revision,
                ...(args.focus_brief_id ? { focus_brief_id: args.focus_brief_id } : {}) });
            }
            return companion.browseChapter(args.chapter_id, args.brief_id);
          }
        });
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
      if (!rule || (rule.type && (rule.type === 'array' ? !Array.isArray(value) : rule.type === 'integer' ? !Number.isSafeInteger(value) || value < 0 : typeof value !== rule.type)) || (rule.enum && !rule.enum.includes(value))) throw new Error(`Invalid ${key}`);
    }
    companion.update({ status: 'working' });
    try {
      switch (name) {
        case 'companion_open': await companion.open(args); return { panel_url, mode: companion.state.mode, saved_audits_available: companion.state.saved_audits_available, tx_pilot_available: companion.state.tx_pilot_available, cpu_pilot_available: companion.state.cpu_pilot_available, combined_pilot_available: companion.state.combined_pilot_available, saved_audit_count: companion.state.saved_audits.length, ...companion.languageContext(), voice: 'Use text or the host native voice interface when available; panel has no microphone.' };
        case 'companion_set_language': return companion.setLanguage(args);
        case 'companion_projects': return await companion.projects();
        case 'companion_select_project': return await companion.select(args.project_id, args);
        case 'companion_tools': return await companion.tools();
        case 'companion_query': return await companion.query(args.tool, args.arguments, args.display);
        case 'companion_present': return companion.present(args);
        case 'companion_context': {
          if (companion.state.audit) await companion.auditStatus();
          const view = companion.state.views.find(v => v.id === companion.state.current);
          return { project_id: companion.state.project, analysis_id: companion.analysis, discussed_entities_available: companion.state.discussed_entities_available, mode: companion.state.mode, audit_support: companion.state.audit_support, audit: companion.state.audit,
            viewed_chapter_id: companion.viewedChapter(), viewed_brief_id: view?.brief_id ?? null, chapter_views: companion.chapterViews(),
            ...(args.include_displayed_board ? { displayed_board: companion.displayedBoard() } : {}),
            selection: companion.state.selection, current_view_id: companion.state.current, ...companion.languageContext(),
            views: companion.state.views.map(v => ({ view_id: v.id, chapter_id: v.chapter_id, tool: v.tool, args: v.args, evidence_id: v.data.evidence_id, title: v.presentation?.title, presentation: !!v.presentation })) };
        }
        case 'companion_start_audit': return await companion.startAudit();
        case 'companion_audit_status': return { ...await companion.auditStatus(), viewed_chapter_id: companion.viewedChapter() };
        case 'companion_record_discussed_entities': return await companion.recordDiscussedEntities(args);
        case 'companion_record_brief': return await companion.recordBrief(args);
        case 'companion_advance_audit': return await companion.advanceAudit(args);
        case 'companion_authorize_report': return await companion.authorizeReport(args);
        case 'companion_export_audit': return await companion.exportAudit();
        case 'companion_get_client_report_plan': return await companion.getClientReportPlan();
        case 'companion_save_client_report_plan': return await companion.saveClientReportPlan(args);
        case 'companion_preview_client_report': {
          const preview = await companion.previewClientReport();
          return { ...preview, ...(companion.state.client_assets_available ? { panel_preview_url: panel_url + 'client/report/' + preview.preview_id + '/raport.html' } : {}) };
        }
        case 'companion_publish_client_report': return await companion.publishPilotReport('client', args);
        case 'companion_resume_audit': return await companion.resumeAudit(args);
        case 'companion_restore_audit': return await companion.restoreAudit(args);
        case 'companion_enable_tx_pilot': return await companion.enablePilot('tx');
        case 'companion_enable_cpu_pilot': return await companion.enablePilot('cpu');
        case 'companion_enable_combined_pilot': return await companion.enablePilot('combined');
        case 'companion_switch_audit_topic': return await companion.switchAuditTopic(args);
        case 'companion_list_saved_audits': return await companion.listSavedAudits();
        case 'companion_resume_saved_audit': return await companion.resumeSavedAudit(args);
        case 'companion_preview_tx_report': return await companion.previewPilotReport('tx', args);
        case 'companion_preview_cpu_report': return await companion.previewPilotReport('cpu', args);
        case 'companion_publish_tx_report': return await companion.publishPilotReport('tx', args);
        case 'companion_preview_combined_report': return await companion.previewPilotReport('combined', args);
        case 'companion_publish_combined_report': return await companion.publishPilotReport('combined', args);
        case 'companion_publish_cpu_report': return await companion.publishPilotReport('cpu', args);
        case 'companion_show': return companion.show(args.view_id);
        case 'companion_note':
          if (args.text.length > 2000) throw new Error('Maximum note length is 2000.');
          companion.note(args.text, args.language, args.project_id); return { displayed: true };
      }
    } finally { companion.update({ status: 'idle' }); }
  }
  function enqueue(operation) {
    const result = queue.then(async () => {
      // Retry an uncheckpointed display before replacing it or producing a report.
      await companion.captureDisplayedEntities();
      const value = await operation();
      await companion.captureDisplayedEntities();
      const audit = companion.state.audit;
      if (audit && value?.analysis_id === companion.analysis && value.audit) return { ...value, audit };
      if (audit && value?.analysis_id === companion.analysis && value.snapshot && Array.isArray(value.briefs))
        return { ...value, ...audit };
      return value;
    });
    queue = result.catch(() => {}); return result;
  }
  function call(name, args = {}) { return enqueue(() => execute(name, args)); }
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
    const maxRequestBytes = (request.method === 'tools/call' && request.params?.name === 'companion_restore_audit' ? 65 : 1) * 1024 * 1024;
    if (Buffer.byteLength(line, 'utf8') > maxRequestBytes)
      return send({ jsonrpc: '2.0', id: request.id, error: { code: -32600, message: 'MCP request exceeds its size limit' } });
    const response = { jsonrpc: '2.0', id: request.id };
    try {
      switch (request.method) {
        case 'initialize': response.result = { protocolVersion: ['2024-11-05', '2025-03-26', '2025-06-18', '2025-11-25'].includes(request.params?.protocolVersion) ? request.params.protocolVersion : '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'jasmin-companion', version: metadata.version } }; break;
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
    if (buffer.length > 65 * 1024 * 1024) throw new Error('MCP input too large');
    let i;
    while ((i = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, i); buffer = buffer.slice(i + 1);
      if (line.trim()) pending = pending.then(() => handle(line));
    }
  }
  await pending;
}

// Plugin caches can be reached through symlinks; compare resolved entry paths.
const isMain = process.argv[1] && await realpath(process.argv[1]).catch(() => null) === await realpath(fileURLToPath(import.meta.url));
if (isMain) {
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
