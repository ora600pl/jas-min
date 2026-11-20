import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { createHash } from 'node:crypto';
import { Companion, ALLOWED } from '../companion.mjs';
import { McpClient } from '../mcp-client.mjs';
import { createServer, TOOLS } from '../server.mjs';
import { auditModel } from '../public/audit.js';

// A retained upstream fixture models transport boundaries, not evidence validation.
class AuditFixture {
  constructor() { this.sessions = new Map(); this.calls = []; this.sequence = 0; }
  async close() {}
  async tools() { return ['start_audit', 'get_audit_status', 'record_audit_brief', 'advance_audit', 'authorize_report', 'export_audit', 'restore_audit', 'get_metric_time_series'].map(name => ({ name, inputSchema: { type: 'object', properties: {}, required: [] } })); }
  async call(name, args = {}) {
    this.calls.push({ name, args: structuredClone(args) });
    if (name === 'list_performance_projects') return { projects: [{ project_id: 'project-1' }, { project_id: 'project-2' }] };
    if (name === 'start_performance_analysis') {
      const analysis_id = `analysis-${++this.sequence}`;
      this.sessions.set(analysis_id, { analysis_id, project_ids: [args.project_id], revision: 1, current_chapter_id: 'scope', report_authorized: false, phase: 'discussion', briefs: [], chapters: [
        { chapter_id: 'scope', title: 'Scope', title_pl: 'Zakres', question: 'Which interval matters?', question_pl: 'Który okres badamy?', status: 'pending' },
        { chapter_id: 'synthesis', title: 'Synthesis', title_pl: 'Wnioski', question: 'What do we know?', question_pl: 'Co już wiemy?', status: 'pending' },
      ] }); return { analysis_id };
    }
    if (name === 'restore_audit') {
      if (args.checkpoint?.version !== 'fixture-1') throw new Error('Invalid checkpoint');
      const status = structuredClone(args.checkpoint.audit); status.analysis_id = `analysis-${++this.sequence}`;
      this.sessions.set(status.analysis_id, status); return structuredClone(status);
    }
    const status = this.sessions.get(args.analysis_id);
    if (!status) throw new Error('Unknown analysis');
    if (name === 'get_metric_time_series') return { analysis_id: args.analysis_id, project_id: args.project_id, evidence_id: `E-${++this.sequence}`, result: { name: 'DB Time(s)', series: [{ snap_id: 1, value: 2 }] } };
    if (name === 'export_audit') return { analysis_id: args.analysis_id, persisted: false, checkpoint: { version: 'fixture-1', audit: structuredClone(status) } };
    if (name === 'record_audit_brief') {
      const old = status.briefs.find(b => b.brief_id === args.brief_id);
      const brief = { ...structuredClone(args), brief_id: args.brief_id || `brief-${status.briefs.length + 1}`, updated_at_revision: status.revision + 1, revisions: old ? [...old.revisions, { explanation: old.explanation, conclusion: old.conclusion }] : [] };
      status.briefs = status.briefs.filter(b => b.brief_id !== brief.brief_id); status.briefs.push(brief);
      status.chapters.find(c => c.chapter_id === args.chapter_id).status = 'discussed';
      status.report_authorized = false; status.phase = 'discussion'; status.revision++;
    } else if (name === 'advance_audit') {
      if (!status.chapters.some(c => c.chapter_id === args.chapter_id)) throw new Error('Unknown chapter');
      status.current_chapter_id = args.chapter_id; status.report_authorized = false; status.phase = 'discussion'; status.revision++;
    } else if (name === 'authorize_report') {
      if (!args.user_request?.trim() || status.chapters.some(c => c.status !== 'discussed')) throw new Error('Report gate: request and discussed chapters required');
      status.report_authorized = true; status.phase = 'report_requested'; status.authorized_revision = status.revision;
    }
    return structuredClone(status);
  }
}
const brief = (chapter_id = 'scope') => ({ chapter_id, question: 'Why did time rise?', explanation: 'The measured work increased. We still need comparable executions.', conclusion: 'Compare the same work before changing SQL.', evidence_boundary: 'Elapsed time alone does not prove a worse access path.', next_step: 'Compare executions and rows processed.', open_questions: ['Did the workload grow?'], evidence_refs: ['E-1'] });
const attach = client => { const c = new Companion(); c.client = client; c.update({ mode: 'live' }); return c; };

test('reconnecting an in-memory audit adopts the selected project and preserves its current briefs', async () => {
  const upstream = new AuditFixture(), writer = attach(upstream);
  await writer.select('project-1');
  const original = await writer.recordBrief(brief());
  const status = upstream.sessions.get(original.analysis_id);
  status.audit_started = true;
  status.project_ids = ['project-1', 'project-2'];

  const schemas = await upstream.tools();
  schemas.find(t => t.name === 'start_audit').inputSchema.properties.focus_project_id = { type: 'string' };
  upstream.tools = async () => [...schemas, ...['list_saved_audits','resume_saved_audit'].map(name => ({ name }))];
  const call = upstream.call.bind(upstream);
  let saves = 0;
  upstream.call = async (name, args) => {
    if (name === 'start_audit') {
      assert.equal(args.analysis_id, original.analysis_id);
      assert.equal(args.focus_project_id, 'project-2');
      saves++;
      Object.assign(status, { audit_id: 'audit-1', pilot_kind: 'general', snapshot: { persisted: true, revision: 1 },
        resume_context: { focus_project_id: 'project-2' } });
      return structuredClone(status);
    }
    return call(name, args);
  };
  const resumed = attach(upstream);
  await assert.rejects(resumed.resumeAudit({ analysis_id: original.analysis_id, project_id: 'invalid' }), /Select a project/);
  const adopted = await resumed.resumeAudit({ analysis_id: original.analysis_id, project_id: 'project-2' });
  assert.deepEqual(adopted.briefs, original.briefs);
  assert.equal(adopted.revision, original.revision);
  assert.equal(resumed.state.saved_audits_available, true);
  assert.equal(resumed.state.saved_audits[0].pilot_kind, 'general');
  assert.equal(resumed.state.tx_pilot_available, false);
  await resumed.resumeAudit({ analysis_id: original.analysis_id });
  assert.equal(saves, 1);
  assert.equal(resumed.state.project, 'project-2');
});

test('saved brief and visual outlive 20 display views; corrections preserve history and revoke report authorization', async () => {
  const upstream = new AuditFixture(), c = attach(upstream); await c.select('project-1');
  const visual = c.present({ title: 'Why did time rise?', takeaway: 'More work is one hypothesis.', panels: [{ type: 'flow', title: 'Possible mechanism', classification: 'hypothesis', source: 'E-1', steps: [{ title: 'Input', detail: 'More rows.' }, { title: 'Work', detail: 'More processing.' }] }] });
  let status = await c.recordBrief({ ...brief(), view_id: visual.view_id });
  const id = status.briefs[0].brief_id;
  const evidence = await c.query('get_metric_time_series'); c.show(evidence.view_id);
  for (let i = 0; i < 24; i++) await c.query('get_metric_time_series');
  assert.ok(c.state.views.length <= 20);
  assert.equal(c.state.views.some(v => v.id === visual.view_id), false);
  assert.equal((await c.auditStatus()).briefs[0].visual.panels[0].source, 'E-1');
  await c.recordBrief(brief('synthesis')); await c.authorizeReport({ user_request: 'Please produce the final report.' });
  status = await c.recordBrief({ ...brief(), brief_id: id, explanation: 'Correction: the comparison does not establish workload growth.', conclusion: 'The cause remains unknown.' });
  assert.equal(status.report_authorized, false);
  assert.equal(status.briefs.find(b => b.brief_id === id).revisions.length, 1);
  assert.deepEqual(status.briefs.find(b => b.brief_id === id).evidence_refs, ['E-1']);
  assert.equal(upstream.calls.filter(c => c.name === 'start_performance_analysis').length, 1);
});

test('report gate is upstream; export and chapter completion never authorize a report', async () => {
  const upstream = new AuditFixture(), c = attach(upstream); await c.select('project-1');
  await assert.rejects(c.authorizeReport({ user_request: 'Please produce the final report.' }), /Report gate/);
  await c.recordBrief(brief()); await c.advanceAudit({ chapter_id: 'synthesis' }); await c.recordBrief(brief('synthesis'));
  const exported = await c.exportAudit(); assert.equal(exported.checkpoint.audit.report_authorized, false); assert.equal(exported.persisted, false);
  assert.equal(upstream.calls.some(c => c.name === 'authorize_report' && !c.args.user_request), false);
  await c.authorizeReport({ user_request: 'Please produce the final report.' });
  assert.equal(c.state.audit.report_authorized, true);
  await c.advanceAudit({ chapter_id: 'scope' }); assert.equal(c.state.audit.report_authorized, false);
});

test('saved visual uses completed dialogue translations while preserving evidence identifiers', async () => {
  const c = attach(new AuditFixture()); await c.select('project-1');
  const shown = c.present({ title: 'Observed work', takeaway: 'More work is a hypothesis.', panels: [{ type: 'bar', title: 'Measured work', classification: 'measured', source: 'E-1', unit: 'rows', items: [{ label: 'Interval 1', value: 42 }] }] });
  const pending = c.setLanguage({ language: 'pl' }).pending;
  await assert.rejects(c.recordBrief({ ...brief(), view_id: shown.view_id }), /translation/);
  c.setLanguage({ language: 'pl', view_id: shown.view_id, translations: pending.map(({ path, text }) => ({ path, text: `PL ${text}` })) });
  const status = await c.recordBrief({ ...brief(), view_id: shown.view_id });
  assert.equal(status.briefs[0].visual.title, 'PL Observed work');
  assert.equal(status.briefs[0].visual.language, 'pl');
  assert.equal(status.briefs[0].visual.panels[0].source, 'E-1');
  assert.equal(status.briefs[0].visual.panels[0].items[0].value, 42);
});

test('resume restores the same audit; saved checkpoint restores to a new identity after server restart', async () => {
  const upstream = new AuditFixture(), a = attach(upstream); await a.select('project-1'); await a.recordBrief(brief());
  const checkpoint = JSON.parse(JSON.stringify((await a.exportAudit()).checkpoint));
  const b = attach(upstream); await b.resumeAudit({ analysis_id: a.analysis });
  assert.equal(b.analysis, a.analysis); assert.equal(b.state.audit.briefs[0].explanation, brief().explanation);
  assert.equal(upstream.calls.filter(c => c.name === 'start_performance_analysis').length, 1);
  const restarted = new AuditFixture(); restarted.sequence = 100;
  const restored = attach(restarted); await restored.restoreAudit({ checkpoint });
  assert.notEqual(restored.analysis, a.analysis); assert.deepEqual(restored.state.audit.briefs, a.state.audit.briefs);
  assert.equal(restored.state.project, 'project-1');
});

test('project selection and resume reject mismatched scope without mixing brief state', async () => {
  const upstream = new AuditFixture(), a = attach(upstream), b = attach(upstream);
  await a.select('project-1'); await a.recordBrief(brief()); await b.select('project-2');
  await assert.rejects(b.resumeAudit({ analysis_id: a.analysis, project_id: 'project-2' }), /belonging/);
  assert.equal(b.state.project, 'project-2'); assert.deepEqual(b.state.audit.briefs, []);
  await b.resumeAudit({ analysis_id: a.analysis, project_id: 'project-1' });
  assert.equal(b.state.audit.briefs[0].project_id, 'project-1');
  await a.select('project-2'); assert.deepEqual(a.state.audit.briefs, []); assert.deepEqual(a.state.views, []);
});

test('older server explicitly reports unsupported audit; no arbitrary mutation enters read-only query', async () => {
  const c = new Companion(); await c.open({ mode: 'demo' }); const selected = await c.select('demo');
  assert.equal(selected.audit_support, 'unsupported'); assert.equal(selected.audit, null);
  await assert.rejects(c.startAudit(), /unavailable/);
  for (const name of ['record_finding', 'finalize_report', 'authorize_report', 'restore_audit']) {
    assert.equal(ALLOWED.has(name), false); await assert.rejects(c.query(name), /not allowed/);
  }
  assert.ok(ALLOWED.has('get_sql_execution_plan')); assert.ok(ALLOWED.has('get_alertlog_errors'));
});

test('controlled MCP tools retain canonical analysis identity and reject caller routing overrides', async t => {
  const app = await createServer(); t.after(() => app.close()); app.companion.client = new AuditFixture(); app.companion.update({ mode: 'live' });
  app.companion.update({ language: 'pl' });
  const selected = await app.call('companion_select_project', { project_id: 'project-1' });
  assert.equal(app.companion.client.calls.find(c => c.name === 'start_performance_analysis').args.language, 'pl');
  await app.call('companion_record_brief', brief());
  const context = await app.call('companion_context');
  assert.equal(context.analysis_id, selected.analysis_id); assert.equal(context.audit.briefs.length, 1);
  await assert.rejects(app.call('companion_record_brief', { ...brief(), project_id: 'project-2' }), /Invalid project_id/);
  await assert.rejects(app.call('companion_authorize_report', {}), /Missing user_request/);
  assert.equal(TOOLS.find(t => t.name === 'companion_authorize_report').annotations.readOnlyHint, false);
  assert.equal(TOOLS.find(t => t.name === 'companion_export_audit').annotations.readOnlyHint, true);
});

test('audit display model localizes chapter questions and filters briefs by project', () => {
  const state = { project: 'project-1', language: 'pl', audit: { current_chapter_id: 'scope', chapters: [{ chapter_id: 'scope', title: 'Scope', title_pl: 'Zakres', question: 'Which interval?', question_pl: 'Który okres?', status: 'pending' }], briefs: [{ ...brief(), project_id: 'project-2' }] } };
  const model = auditModel(state); assert.equal(model.question, 'Który okres?'); assert.deepEqual(model.briefs, []);
  state.audit.briefs.push({ ...brief(), project_id: 'project-1' });
  assert.equal(auditModel(state).question, brief().question); assert.deepEqual(auditModel(state).openQuestions, ['Did the workload grow?']);
});

test('audit display follows the last corrected brief without inferring chronology from IDs', () => {
  const state = { project: 'project-1', language: 'en', audit: { current_chapter_id: 'scope', chapters: [{ chapter_id: 'scope', question: 'Chapter question' }], briefs: [
    { ...brief(), project_id: 'project-1', brief_id: 'B-100', question: 'Earlier question', updated_at_revision: 4 },
    { ...brief(), project_id: 'project-1', brief_id: 'B-001', question: 'Corrected question', updated_at_revision: 9 },
  ] } };
  assert.equal(auditModel(state).question, 'Corrected question');
  for (const b of state.audit.briefs) delete b.updated_at_revision;
  assert.equal(auditModel(state).question, 'Chapter question');
});

test('a resumed gradient chapter opens its model evidence, retains a saved workload board and only reads the source once', async () => {
  const upstream = new AuditFixture(), create = attach(upstream);
  const selected = await create.select('project-1');
  const audit = upstream.sessions.get(selected.analysis_id);
  audit.chapters.push({ chapter_id: 'performance_profile', title: 'Performance profile' });
  const board = await create.query('get_metric_time_series');
  const status = await create.recordBrief({ ...brief('performance_profile'), view_id: board.view_id });
  audit.current_chapter_id = 'performance_profile';
  const c = attach(upstream);
  const originalTools = upstream.tools.bind(upstream);
  upstream.tools = async () => [...await originalTools(), { name: 'get_precomputed_analysis',
    inputSchema: { properties: { preview_only: { type: 'boolean' } } } }];
  const originalCall = upstream.call.bind(upstream), reads = [];
  upstream.call = async (name, args) => {
    if (name !== 'get_precomputed_analysis') return originalCall(name, args);
    reads.push(args);
    return { result: { section: 'full_gradients', data: { db_time_sql_elapsed_time: {
      model_rankings: { ridge: [{ event_name: 'sql-1', gradient_coef: -1.5 }] } } } } };
  };
  await c.resumeAudit({ analysis_id: selected.analysis_id });
  assert.equal(c.state.audit.revision, status.revision);
  assert.deepEqual(reads, [{ analysis_id: selected.analysis_id, project_id: 'project-1',
    section: 'full_gradients', ranking: 'extreme', limit: 4, preview_only: true }]);
  const overview = c.state.current;
  assert.ok(c.lookupView(overview).opening_gradient);
  assert.equal((await c.query('get_metric_time_series')).displayed, false);
  assert.equal(c.state.current, overview);
  await c.browseChapter('scope'); await c.browseChapter('performance_profile');
  assert.equal(c.state.current, overview); assert.equal(reads.length, 1);
  await c.browseChapter('performance_profile', status.briefs[0].brief_id);
  assert.equal(c.displayedBoard().panels[0].series[0].points[0].value, 2);
  await c.auditStatus(); assert.equal(c.displayedBoard().brief_id, status.briefs[0].brief_id);
});

test('legacy reviewed audit reuses retained regression evidence without collecting new measurements', async () => {
  const upstream = new AuditFixture(), c = attach(upstream);
  const selected = await c.select('project-1');
  const audit = upstream.sessions.get(selected.analysis_id);
  audit.chapters.push({ chapter_id: 'performance_profile', title: 'Performance profile' });
  audit.report_authorized = true;
  const saved = { evidence_id: 'E-fits', project_id: 'project-1', tool_name: 'get_precomputed_analysis',
    arguments: { section: 'full_gradients', limit: 2 }, result: { section: 'full_gradients', data: {} } };
  const exports = [];
  const originalCall = upstream.call.bind(upstream);
  upstream.call = async (name, args) => {
    assert.notEqual(name, 'get_precomputed_analysis');
    if (name !== 'export_audit') return originalCall(name, args);
    exports.push(args); return { checkpoint: { evidence: { 'E-fits': saved } } };
  };
  c.schemas.push({ name: 'get_precomputed_analysis', inputSchema: { properties: {} } });
  await c.auditStatus();
  await c.browseChapter('performance_profile');
  assert.equal(c.displayedBoard().evidence_id, 'E-fits');
  assert.equal(c.state.audit.report_authorized, true);
  assert.equal(c.state.audit.current_chapter_id, 'scope');
  await c.browseChapter('scope'); await c.browseChapter('performance_profile');
  assert.deepEqual(exports, [{ analysis_id: selected.analysis_id }]);
});

test('chapter navigation recalls multiple evicted views with measurements and notes but no raw envelopes', async () => {
  const upstream = new AuditFixture(), c = attach(upstream); await c.select('project-1');
  const raw = await c.query('get_metric_time_series');
  c.state.views.find(v => v.id === raw.view_id).data.result.extra_raw = 'Only in the raw response';
  c.note('Inspect this sample.', 'en'); c.selectPoint({ snap_id: 1 });
  const second = c.present({ title: 'Second board', takeaway: 'Review the comparison.', panels: [
    { type: 'flow', title: 'Possible mechanism', classification: 'hypothesis', source: 'Recorded sample',
      steps: [{ title: 'Input', detail: 'More rows.' }, { title: 'Cost', detail: 'More reads.' }] }] });
  await c.advanceAudit({ chapter_id: 'synthesis' });
  for (let i = 0; i < 25; i++) await c.query('get_metric_time_series', {}, false);
  assert.equal(c.state.views.some(v => [raw.view_id, second.view_id].includes(v.id)), false);
  const calls = upstream.calls.length, audit = structuredClone(c.state.audit);
  await c.browseChapter('scope'); assert.equal(c.state.current, second.view_id);
  assert.deepEqual(c.chapterViews().map(row => row.view_id), [raw.view_id, second.view_id]);
  c.show(raw.view_id);
  const restored = c.state.views.find(v => v.id === raw.view_id);
  assert.equal(restored.presentation.panels[0].series[0].points[0].value, 2);
  assert.equal(restored.data.evidence_id, raw.evidence_id);
  assert.equal(restored.data.result, undefined); assert.equal(c.state.note, 'Inspect this sample.');
  assert.deepEqual(c.state.selection, {}); c.selectPoint({ snap_id: 1 });
  assert.deepEqual(c.setLanguage({ language: 'pl' }).pending.map(f => f.path), ['note']);
  assert.equal(c.state.views.length <= 20, true); assert.deepEqual(c.state.audit, audit);
  assert.equal(upstream.calls.length, calls);
  await c.select('project-2'); assert.deepEqual(c.chapterViews(), []);
  assert.throws(() => c.show(raw.view_id), /Unknown view/);
});

test('attaching a revisited topic corrects the board chapter independently of active audit progress', async () => {
  const c = attach(new AuditFixture()); await c.select('project-1');
  await c.advanceAudit({ chapter_id: 'synthesis' });
  const v = await c.query('get_metric_time_series');
  const saved = await c.recordBrief({ ...brief('scope'), view_id: v.view_id });
  assert.equal(c.viewedChapter(), 'scope'); assert.equal(saved.current_chapter_id, 'synthesis');
  assert.deepEqual(c.chapterViews().map(row => row.view_id), [v.view_id]);
  await c.browseChapter('synthesis'); assert.deepEqual(c.chapterViews(), []);
  await c.browseChapter('scope'); assert.equal(c.state.current, v.view_id);
});

test('context supplies exact displayed measurements on request and an earlier saved brief provides its own question', async t => {
  const app = await createServer(); t.after(() => app.close());
  const c = app.companion; c.client = new AuditFixture(); await c.select('project-1');
  const first = await c.query('get_metric_time_series');
  const saved = await c.recordBrief({ ...brief(), question: 'First question', next_step: 'First check', view_id: first.view_id });
  await c.recordBrief({ ...brief(), question: 'Later question', next_step: 'Later check' });
  await c.advanceAudit({ chapter_id: 'synthesis' });
  for (let i = 0; i < 25; i++) await c.query('get_metric_time_series', {}, false);
  await c.browseChapter('scope', saved.briefs[0].brief_id);
  const model = auditModel(c.state); assert.equal(model.question, 'First question'); assert.equal(model.nextStep, 'First check');
  const compact = await app.call('companion_context');
  assert.ok(!('displayed_board' in compact)); assert.equal(compact.viewed_brief_id, saved.briefs[0].brief_id);
  const { displayed_board: board } = await app.call('companion_context', { include_displayed_board: true });
  assert.equal(board.panels[0].source, first.evidence_id);
  assert.equal(board.panels[0].series[0].points[0].value, 2);
  assert.equal(board.view_id, first.view_id); assert.equal(board.title, 'DB Time(s)');
});

test('Companion exposes the hypothesis fields and shows only the selected project after resume', async t => {
  const app = await createServer(); t.after(() => app.close());
  const fixture = new AuditFixture(); app.companion.client = fixture; app.companion.update({ mode: 'live' });
  const selected = await app.call('companion_select_project', { project_id: 'project-1' });
  const audit = fixture.sessions.get(selected.analysis_id);
  audit.chapters.push({ chapter_id: 'performance_profile', title: 'Performance profile' });
  const hypotheses = ['H1', 'H2'].map(id => ({ id, statement: `Explanation ${id}`, gradient_signal: 'More reads',
    workload_profile: 'Read time per second', comparison_window: 'Incident and baseline', test: 'Compare reads per execution' }));
  const cards = [{ id: 'reads', title: 'More reads', kind: 'comparison', body: 'The period used more reads per call.', evidence_refs: ['E-example'] }];
  await app.call('companion_record_brief', { ...brief('performance_profile'), opening_hypotheses: hypotheses, evidence_cards: cards });
  assert.deepEqual(fixture.calls.find(c => c.name === 'record_audit_brief').args.evidence_cards, cards);
  assert.deepEqual(fixture.calls.find(c => c.name === 'record_audit_brief').args.opening_hypotheses, hypotheses);

  audit.project_ids.push('project-2');
  audit.hypotheses = [{ project_id: 'project-1', opening_hypotheses: hypotheses },
    { project_id: 'project-2', hypothesis_gap: 'Missing load profile.' }];
  await app.call('companion_resume_audit', { analysis_id: selected.analysis_id, project_id: 'project-2' });
  const model = auditModel(app.companion.state);
  assert.equal(model.hypotheses.hypothesis_gap, 'Missing load profile.');
  assert.deepEqual(model.briefs, []);
  const { properties } = TOOLS.find(t => t.name === 'companion_record_brief').inputSchema;
  assert.deepEqual(properties.hypothesis_outcomes.items.properties.status.enum, ['confirmed', 'rejected', 'unresolved']);
  assert.ok(properties.hypothesis_checks.items.required.includes('evidence_refs'));
  assert.ok(properties.evidence_cards.items.required.includes('evidence_refs'));
});

test('checkpoint transport accepts retained evidence beyond ordinary response limits', async t => {
  const payload = 'x'.repeat(9 * 1024 * 1024);
  const server = http.createServer(async (req, res) => {
    let body = ''; for await (const part of req) body += part;
    const msg = JSON.parse(body);
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result: { structuredContent: { checkpoint: { payload } } } }));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  const client = new McpClient(`http://127.0.0.1:${server.address().port}/mcp`);
  assert.equal((await client.call('export_audit', { analysis_id: 'analysis-1' })).checkpoint.payload.length, payload.length);
  await assert.rejects(client.call('get_sql_text', { analysis_id: 'analysis-1' }), /too large/);
});

test('saved comparative audit restores the focused project and TX board in a fresh panel and protects stale actions', async t => {
  const a = await createServer(), b = await createServer();
  t.after(() => Promise.all([a.close(), b.close()]));
  const core = new AuditFixture();
  const baseTools = await core.tools();
  core.tools = async () => [...baseTools, ...['enable_tx_pilot', 'list_saved_audits', 'resume_saved_audit', 'preview_tx_report', 'publish_tx_report']
    .map(name => ({ name, inputSchema: { type: 'object', properties: {} } }))];
  const call = core.call.bind(core);
  const state = { audit_id: 'saved-1', project_ids: ['project-1', 'project-2'], focus_project_id: 'project-2' };
  let last;
  core.call = async (name, args = {}) => {
    if (name === 'list_saved_audits') return { audits: last ? [state] : [] };
    if (name === 'resume_saved_audit') {
      assert.equal(args.audit_id, state.audit_id);
      const resumed = { ...structuredClone(last), analysis_id: 'new-session' };
      core.sessions.set(resumed.analysis_id, resumed);
      return resumed;
    }
    if (name === 'start_performance_analysis') {
      assert.deepEqual(args.project_ids, state.project_ids); assert.equal(args.project_id, undefined);
      const started = await call(name, { project_id: state.focus_project_id });
      core.sessions.get(started.analysis_id).project_ids = state.project_ids;
      return started;
    }
    const current = core.sessions.get(args.analysis_id);
    if (name === 'enable_tx_pilot') {
      assert.equal(args.focus_project_id, state.focus_project_id);
      Object.assign(current, { audit_id: state.audit_id, snapshot: { revision: 8, persisted: true }, resume_context: {} });
      last = structuredClone(current); return last;
    }
    if (name === 'record_audit_brief' || name === 'advance_audit') {
      assert.equal(args.expected_revision, current.snapshot.revision);
      assert.ok(args.operation_id);
      await call(name, args);
      current.snapshot.revision++;
      Object.assign(current.resume_context, { focus_project_id: state.focus_project_id });
      if (name === 'record_audit_brief') Object.assign(current.resume_context, {
        last_private_note: args.private_note, focus_brief_id: args.brief_id,
      });
      last = structuredClone(current); return last;
    }
    if (name === 'preview_tx_report') {
      assert.equal(args.analysis_id, 'new-session');
      assert.equal(args.evidence[0].evidence_ref, 'E-1');
      assert.equal(args.private_note, undefined);
      return { preview_id: 'preview-1', index_path: 'preview/index.html', files: ['index.html'] };
    }
    if (name === 'publish_tx_report') {
      assert.equal(args.preview_id, 'preview-1');
      assert.equal(args.user_request, 'Wydaj ZIP dla klienta');
      return { preview_id: args.preview_id, zip_path: 'releases/client.zip' };
    }
    return call(name, args);
  };
  for (const app of [a, b]) { app.companion.client = core; app.companion.update({ mode: 'live' }); }
  await assert.rejects(a.call('companion_select_project', { project_id: 'project-2', project_ids: ['project-1'] }), /including/);
  await a.call('companion_select_project', { project_id: 'project-2', project_ids: state.project_ids });
  const enabled = await a.call('companion_enable_tx_pilot'); assert.equal(enabled.snapshot.revision, 8);
  const board = await a.call('companion_present', { title: 'TX interval', takeaway: 'Waiting sessions', panels: [
    { type: 'line', classification: 'measured', title: 'Wait time', unit: 's', x_label: 'Interval', source: 'E-1',
      series: [{ name: 'TX', points: [{ label: 'Interval', x: 1, value: 42, snap_id: 100, instance: 'instance-2' }] }] }] });
  a.companion.selectPoint({ snap_id: 100, instance: 'instance-2' });
  await a.call('companion_record_brief', { ...brief(), brief_id: 'tx', private_note: 'Private correction', view_id: board.view_id });
  const panelPost = (app, route, values) => fetch(app.panel_url + route, { method: 'POST',
    headers: { 'X-Companion-Client': 'panel', 'Content-Type': 'application/json' }, body: JSON.stringify(values) });
  const scope = { analysis_id: a.companion.analysis, project_id: 'project-2' };
  assert.equal((await panelPost(a, 'chapter', { ...scope, chapter_id: 'synthesis' })).status, 200);
  assert.equal(a.companion.state.audit.snapshot.revision, 9);
  assert.equal((await panelPost(a, 'advance', { ...scope, chapter_id: 'synthesis', expected_revision: 9 })).status, 200);
  assert.equal(a.companion.state.audit.snapshot.revision, 10);
  assert.equal((await panelPost(a, 'advance', { ...scope, chapter_id: 'scope', expected_revision: 9 })).status, 400);
  const list = await b.call('companion_list_saved_audits'); assert.equal(list.audits[0].audit_id, state.audit_id);
  const restored = await b.call('companion_resume_saved_audit', { audit_id: state.audit_id });
  assert.equal(restored.current_chapter_id, 'synthesis');
  assert.equal(b.companion.state.project, 'project-2');
  assert.equal(b.companion.viewedChapter(), 'scope');
  assert.equal(restored.resume_context.last_private_note, 'Private correction');
  assert.deepEqual(auditModel(b.companion.state).openQuestions, brief().open_questions);
  assert.equal(b.companion.displayedBoard().panels[0].series[0].points[0].value, 42);
  assert.deepEqual(b.companion.state.selection, { snap_id: 100, instance: 'instance-2' });
  const selection = { brief_id: 'tx', brief_revision: 1, title: 'Lock waits', owner: 'DBA', success_criterion: 'Identify the blocking session',
    evidence: [{ evidence_ref: 'E-1', instance_label: 'instance-2', first_snap_id: 1, last_snap_id: 2 }] };
  const preview = await b.call('companion_preview_tx_report', selection);
  const nextScope = { analysis_id: restored.analysis_id, project_id: 'project-2' };
  assert.equal((await panelPost(b, 'publish-tx', { ...nextScope, preview_id: preview.preview_id, user_request: 'Wydaj ZIP dla klienta' })).status, 200);
  assert.equal(b.companion.state.tx_release.zip_path, 'releases/client.zip');
  await b.call('companion_advance_audit', { chapter_id: 'scope' });
  assert.equal(b.companion.state.tx_preview, null);
  await assert.rejects(b.call('companion_publish_tx_report', { preview_id: preview.preview_id, user_request: 'Wydaj ZIP dla klienta' }), /preview/);
});

test('TX preview verifies complete assets on capture and only serves the ZIP issued for the active preview', async t => {
  const app = await createServer(); t.after(() => app.close());
  const id = 'a'.repeat(64);
  const content = {
    'index.html': Buffer.from('<link rel="stylesheet" href="report.css"><a href="evidence.html">Evidence</a>'),
    'evidence.html': Buffer.from('<a href="index.html#action">Action</a>'),
    'report.css': Buffer.from('body{color:#223}'),
    'report.zip': Buffer.from([80, 75, 3, 4, 0, 254, 255, 1, 5]),
  };
  const digest = b => createHash('sha256').update(b).digest('hex');
  let defect, captureId = id;
  const requests = [];
  app.companion.analysis = 'analysis-1';
  app.companion.client = { close: async () => {}, tools: async () => ['preview_tx_report','read_tx_report_asset'].map(name => ({name})), call: async (name, args) => {
    if (name === 'preview_tx_report') return { preview_id: captureId };
    assert.equal(name, 'read_tx_report_asset');
    assert.equal(args.preview_id, captureId); assert.equal(args.analysis_id, 'analysis-1');
    requests.push(args);
    const b = content[args.asset], end = Math.min(b.length, args.offset + 5);
    return { sha256: defect === 'checksum' ? 'f'.repeat(64) : digest(b), bytes: b.length, offset: args.offset,
      next_offset: end === b.length ? null : defect === 'offset' ? 0 : end, content_base64: b.subarray(args.offset, end).toString('base64') };
  } };
  app.companion.update({ audit: {audit_id:'audit-1', pilot_kind:'tx'} });
  const selection = { brief_id:'wait', brief_revision:1, title:'Waits', owner:'DBA',
    success_criterion:'Measure wait time', evidence:[{evidence_ref:'E-1',instance_label:'Node 1',first_snap_id:1,last_snap_id:2}] };
  await app.call('companion_preview_tx_report', selection);
  const captured = requests.length;
  assert.ok(captured > 3);
  const base = app.panel_url + 'client/' + id + '/';
  for (const path of ['report.zip', 'recipe.json', '%2Fprivate%2Fnote', '%2e%2e%2fprivate.json']) assert.equal((await fetch(base + path)).status, 404);
  assert.equal((await fetch(app.panel_url + 'client/' + 'b'.repeat(64) + '/index.html')).status, 404);
  assert.equal(requests.length, captured);
  for (const file of ['index.html', 'evidence.html', 'report.css']) {
    const response = await fetch(base + file); assert.equal(response.status, 200);
    assert.equal(response.headers.get('etag'), '"' + digest(content[file]) + '"');
    assert.match(response.headers.get('content-security-policy'), /default-src 'none'/);
    assert.deepEqual(Buffer.from(await response.arrayBuffer()), content[file]);
  }
  app.companion.update({ tx_release: { preview_id: id, zip_sha256: digest(content['report.zip']) } });
  const download = await fetch(base + 'report.zip');
  assert.equal(download.headers.get('content-type'), 'application/zip');
  assert.equal(download.headers.get('content-disposition'), 'attachment; filename="jas-min-tx.zip"');
  assert.deepEqual(Buffer.from(await download.arrayBuffer()), content['report.zip']);
  captureId = 'b'.repeat(64);
  for (defect of ['checksum', 'offset']) {
    await assert.rejects(app.call('companion_preview_tx_report', selection));
    assert.equal((await fetch(base.replace(id, captureId) + 'index.html')).status, 404);
  }
  captureId = id;
  defect = null;
  app.companion.update({ tx_release: { preview_id: id, zip_sha256: '0'.repeat(64) } });
  assert.equal((await fetch(base + 'report.zip')).status, 400);
  app.companion.update({ tx_assets_available: false });
  assert.equal((await fetch(base + 'index.html')).status, 200);
});

test('problem-oriented chapters remain ordered and pinned and preview the exact public assets from the core', async t => {
  const app = await createServer(); t.after(() => app.close());
  const c = app.companion, previewId = 'c'.repeat(64);
  const rootPage = '<h2 id="elapsed">Elapsed per run</h2><a href="wykresy/sql-1-1.html">Source</a>';
  const status = { analysis_id:'analysis-1', audit_id:'audit-1', project_ids:['project-1'], pilot_kind:'general',
    current_chapter_id:'sql', chapters:[{chapter_id:'sql'}], briefs:[], snapshot:{persisted:true,revision:7}, revision:12 };
  const names = ['save_client_report_plan','get_client_report_plan','preview_client_report','publish_client_report','read_client_report_asset','get_audit_status','start_audit'];
  const assets = {'raport.html':rootPage, 'wykresy/sql-1-1.html':'A measured chart', 'wykresy/sql-p2-5jsn2k7v1q4cb.html':'Native chart and full SQL', 'wykresy/plotly.min.js':'/* chart library */'};
  const waitPath = 'wykresy/wait-p2-' + 'b'.repeat(64);
  assets[waitPath + '.html'] = 'Native wait chart';
  let published = false;
  c.client = { close:async () => {}, tools:async () => names.map(name => ({name})), call:async (name,args) => {
    assert.equal(args.analysis_id, status.analysis_id);
    switch (name) {
      case 'save_client_report_plan':
        assert.equal(args.expected_revision, status.snapshot.revision); assert.ok(args.operation_id);
        status.client_report_plan = structuredClone(args.plan); status.snapshot.revision++; break;
      case 'get_client_report_plan': return { plan:structuredClone(status.client_report_plan), stale:false, source_issues:[] };
      case 'get_audit_status': break;
      case 'preview_client_report': return { preview_id:previewId, entry_file:'raport.html', files:[...Object.keys(assets),'wykresy/private.html'] };
      case 'read_client_report_asset': {
        const body = Buffer.from(assets[args.asset]); assert.equal(args.offset,0);
        return { bytes: body.length, sha256:createHash('sha256').update(body).digest('hex'), offset:0,
          next_offset:null, content_base64:body.toString('base64') };
      }
      case 'publish_client_report': published = true; throw Error('unexpected publication');
      default: throw Error(name);
    }
    return structuredClone(status);
  } };
  await c.tools(); await c.applyAudit(status,'project-1');
  assert.equal(c.state.client_report_available,true);
  const chapters = ['elapsed','volume','job'].map(id => ({id, title:id, briefs:[{brief_id:'B-1',revision:2}],
    measurements:[{kind:'board_panel',brief_id:'B-1',revision:2,panel_index:0}]}));
  const plan = {title:'Load review',introduction:'Compare the same workload.',chapters};
  const saved = await app.call('companion_save_client_report_plan', {plan});
  assert.equal(saved.snapshot.revision,8);
  assert.deepEqual((await app.call('companion_get_client_report_plan',{})).plan,plan);
  const draft = await app.call('companion_preview_client_report',{});
  assert.equal((await fetch(draft.panel_preview_url)).status,200);
  assert.equal(await (await fetch(draft.panel_preview_url)).text(),rootPage);
  const chart = await fetch(new URL('wykresy/sql-1-1.html',draft.panel_preview_url)); assert.equal(chart.status,200);
  assert.match(chart.headers.get('content-security-policy'),/script-src 'self'/);
  const wait = await fetch(new URL(waitPath + '.html#bg', draft.panel_preview_url));
  assert.equal(await wait.text(), 'Native wait chart');
  assert.match(wait.headers.get('content-security-policy'), /script-src 'self'/);
  const sql = await fetch(new URL('wykresy/sql-p2-5jsn2k7v1q4cb.html',draft.panel_preview_url));
  assert.equal(await sql.text(), 'Native chart and full SQL');
  assert.match(sql.headers.get('content-security-policy'),/script-src 'self'/);
  for (const name of ['report.zip','wykresy/private.html','wykresy/sql-99-1.html','wykresy/sql-p3-5jsn2k7v1q4cb.html']) {
    assert.equal((await fetch(new URL(name,draft.panel_preview_url))).status,404);
  }
  const move = revision => fetch(app.panel_url+'move-report-chapter',{method:'POST',headers:{'x-companion-client':'panel'},
    body:JSON.stringify({chapter_id:'job',direction:-1,expected_revision:revision,analysis_id:c.analysis,project_id:c.state.project})});
  assert.equal((await move(7)).status,400);
  assert.equal((await move(8)).status,200); assert.deepEqual(c.state.audit.client_report_plan.chapters.map(x=>x.id),['elapsed','job','volume']);
  assert.equal(await (await fetch(draft.panel_preview_url)).text(),rootPage);
  await assert.rejects(app.call('companion_publish_client_report', { preview_id: previewId, user_request: 'Publish the previous edition.' }), /current/i);
  assert.equal(published,false); assert.equal(c.state.audit.revision,12);
});

test('RAC CPU discussion uses all selected instances and its own durable preview and ZIP routes', async t => {
  const app = await createServer(); t.after(() => app.close());
  const project_ids = ['project-1', 'project-2'];
  const id = 'c'.repeat(64), bytes = Buffer.from('CPU report');
  const hash = createHash('sha256').update(bytes).digest('hex');
  const first = { begin: '2025-01-01 10:00:00', end: '2025-01-01 11:00:00' };
  const later = { begin: '2025-01-01 12:00:00', end: '2025-01-01 13:00:00' };
  const model = { paired_intervals: 3, observed_peak: { ...first, active_cpu: 9 }, scenario_peak: { ...later, active_cpu: 7.5 }, saved_cpu_hours: 2,
    assumptions: { monitor_module_prefix: 'observer', selected_sql_ids: ['sql-1'], monitor_reduction_pct: 100, sql_reduction_pct: 50 },
    instances: project_ids.map((project_id, i) => ({ project_id, instance_number: i + 1, saved_cpu_hours: 1,
      observed_peak: { ...(i ? later : first), active_cpu: i ? 6 : 5 }, scenario_peak: { ...later, active_cpu: 4 } })) };
  const input = { project_ids, selected_sql_ids: ['sql-1'], sql_reduction_pct: 50, monitor_module_prefix: 'observer' };
  const saved = { analysis_id: 'analysis-1', audit_id: 'audit-cpu', pilot_kind: 'cpu', current_chapter_id: 'scope',
    project_ids, chapters: [{ chapter_id: 'scope', title: 'Scope' }], briefs: [], snapshot: { revision: 1, persisted: true, pilot_kind: 'cpu' } };
  const names = ['enable_cpu_pilot', 'preview_cpu_report', 'publish_cpu_report', 'read_cpu_report_asset', 'list_saved_audits', 'resume_saved_audit',
    'start_audit', 'get_audit_status', 'get_rac_cpu_scenario', 'preview_tx_report'];
  const client = { close: async () => {}, tools: async () => names.map(name => ({ name, inputSchema: {
    type: 'object', properties: {}, required: name === 'get_rac_cpu_scenario' ? Object.keys(input) : [],
  } })), call: async (name, args) => {
    if (name === 'enable_cpu_pilot') { assert.equal(args.focus_project_id, 'project-2'); return saved; }
    if (name === 'get_rac_cpu_scenario') {
      assert.deepEqual(args, { ...input, analysis_id: saved.analysis_id });
      return { evidence_id: 'E-CPU', result: model };
    }
    if (name === 'preview_cpu_report') {
      assert.deepEqual(args.instance_labels, { 'project-1': 'instance-1', 'project-2': 'instance-2' });
      assert.equal(args.include_sql_ids, undefined); assert.equal(args.evidence_ref, 'E-CPU');
      return { preview_id: id, cpu_model: model, public_report: { title: 'Cluster CPU' } };
    }
    if (name === 'publish_cpu_report') {
      assert.equal(args.preview_id, id); return { preview_id: id, zip_sha256: hash, zip_path: 'release/cpu.zip' };
    }
    if (name === 'read_cpu_report_asset') {
      assert.equal(args.preview_id, id); assert.equal(args.offset, 0);
      return { bytes: bytes.length, sha256: hash, offset: 0, next_offset: null, content_base64: bytes.toString('base64') };
    }
    throw new Error('Unexpected tool ' + name);
  } };
  const c = app.companion; c.client = client; c.analysis = saved.analysis_id;
  c.update({ analysis_id: c.analysis, project: 'project-2', project_ids, language: 'pl' });
  assert.equal((await app.call('companion_tools')).cpu_pilot_available, true);
  assert.equal((await app.call('companion_enable_cpu_pilot')).pilot_kind, 'cpu');
  assert.equal(c.state.saved_audits[0].pilot_kind, 'cpu');
  const result = await app.call('companion_query', { tool: 'get_rac_cpu_scenario', arguments: input });
  assert.equal(result.evidence_id, 'E-CPU');
  const board = c.displayedBoard(); assert.equal(board.title, 'Scenariusz CPU klastra');
  assert.deepEqual(board.panels.map(p => p.classification), ['measured', 'hypothesis']);
  assert.deepEqual(board.panels[0].rows[0], ['RAC', first.begin, first.end, 9]);
  assert.deepEqual(board.panels[1].rows[0], ['RAC', later.begin, later.end, 7.5, 2]);
  assert.match(board.panels[1].note, /observer.*50%/);
  assert.match(board.takeaway, /3 sparowanych/); assert.match(board.takeaway, /CPU-godz/);
  const recipe = { title: 'Cluster CPU', brief_id: 'B-1', brief_revision: 2, owner: 'DBA', success_criterion: 'Verify hourly CPU' };
  await assert.rejects(app.call('companion_preview_tx_report', { ...recipe, evidence: [] }), /TX audit/);
  await app.call('companion_preview_cpu_report', { ...recipe, evidence_ref: 'E-CPU', instance_labels: { 'project-1': 'instance-1', 'project-2': 'instance-2' } });
  const cpu = app.panel_url + 'client/cpu/' + id + '/';
  assert.equal((await fetch(cpu + 'index.html')).status, 200);
  assert.equal((await fetch(cpu + 'evidence.html')).status, 200);
  assert.equal((await fetch(cpu + 'report.css')).status, 200);
  assert.equal((await fetch(cpu + 'report.zip')).status, 404);
  assert.equal((await fetch(app.panel_url + 'client/' + id + '/index.html')).status, 404);
  await app.call('companion_publish_cpu_report', { preview_id: id, user_request: 'Publish the CPU report.' });
  const download = await fetch(cpu + 'report.zip');
  assert.equal(download.headers.get('content-disposition'), 'attachment; filename="jas-min-cpu.zip"');
  assert.deepEqual(Buffer.from(await download.arrayBuffer()), bytes);
  await c.applyAudit({ ...saved, snapshot: { ...saved.snapshot, revision: 2 } });
  assert.equal((await fetch(cpu + 'report.zip')).status, 404);
});

test('combined topics restore their own project, brief and board and issue one HTML report with a chart directory', async t => {
  const app = await createServer(); t.after(() => app.close());
  const projects = ['project-1', 'project-2'];
  const topics = {
    tx: { current_chapter_id: 'synthesis', focus_project_id: projects[1], focus_brief_id: 'wait' },
    cpu: { current_chapter_id: 'synthesis', focus_project_id: projects[0], focus_brief_id: 'load' },
  };
  const briefs = [['tx', 'wait', projects[1]], ['tx', 'wait-reference', projects[0]], ['cpu', 'load', projects[0]]]
    .map(([topic, id, project_id]) => ({ ...brief('synthesis'), topic, brief_id: id, project_id,
      revision: 1, updated_at_revision: 1, visual: { title: id, language: 'en', takeaway: 'Next check', panels: [] } }));
  const status = { analysis_id: 'analysis-1', audit_id: 'audit-paired', pilot_kind: 'combined', active_topic: 'tx',
    topics, project_ids: projects, resume_context: topics.tx, current_chapter_id: 'synthesis',
    snapshot: { revision: 1, persisted: true, pilot_kind: 'combined' }, briefs, chapters: [{ chapter_id: 'synthesis' }, { chapter_id: 'sql' }] };
  const names = ['enable_combined_pilot', 'switch_audit_topic', 'list_saved_audits', 'resume_saved_audit',
    'get_audit_status', 'preview_combined_report', 'publish_combined_report', 'read_combined_report_asset'];
  const preview = 'd'.repeat(64), older = 'a'.repeat(64), blob = Buffer.from('paired edition');
  const legacyFiles = ['index.html', 'charts.html', 'tx-chart-1.html', 'plotly.min.js'];
  const assets = ['index.html', ...['report.css', 'ora-600.png', 'report-nav.js', 'tx-chart-1.html', 'cpu-chart-1.html',
    'sql-chart-1-1.html', 'plotly.min.js', 'jasmin-plot.js'].map(file => 'wykresy/' + file)];
  const sha256 = createHash('sha256').update(blob).digest('hex');
  const c = app.companion; c.analysis = status.analysis_id;
  c.update({ analysis_id: c.analysis, project_ids: projects, project: projects[1] });
  c.client = { close: async () => {}, tools: async () => names.map(name => ({ name, inputSchema: { properties: {} } })),
    call: async (name, args) => {
      if (name === 'enable_combined_pilot') assert.equal(args.focus_project_id, projects[1]);
      else if (name === 'switch_audit_topic') {
        assert.equal(args.analysis_id, status.analysis_id); assert.ok(args.operation_id);
        if (args.expected_revision !== status.snapshot.revision) throw Error('conflict');
        status.active_topic = args.topic; status.resume_context = topics[args.topic]; status.snapshot.revision++;
      } else if (name === 'list_saved_audits') return { audits: [{ audit_id: status.audit_id, revision: status.snapshot.revision, project_ids: projects }] };
      else if (name === 'resume_saved_audit') status.analysis_id = 'analysis-2';
      else if (name === 'preview_combined_report') {
        assert.equal(args.tx.brief_id, 'wait'); assert.equal(args.cpu.brief_id, 'load');
        if (args.title === 'Legacy layout') return { preview_id: older, files: legacyFiles };
        return { title: args.title, tx: { public_report: { title: 'Waits' } }, cpu: { public_report: { title: 'CPU' } }, preview_id: preview, files: [...assets, 'private.html', 'recipe.json', 'wykresy/private.html', 'wykresy/other.js'] };
      } else if (name === 'publish_combined_report') {
        assert.equal(args.preview_id, preview); return { preview_id: preview, zip_sha256: sha256, zip_path: 'release/report.zip' };
      } else if (name === 'read_combined_report_asset') {
        assert.ok([preview, older].includes(args.preview_id));
        return { bytes: blob.length, sha256, content_base64: blob.toString('base64'), offset: 0, next_offset: null };
      } else assert.equal(name, 'get_audit_status');
      return structuredClone(status);
    },
  };
  assert.equal((await app.call('companion_tools')).combined_pilot_available, true);
  await app.call('companion_enable_combined_pilot');
  assert.equal(c.displayedBoard().brief_id, 'wait');
  await c.browseChapter('sql'); assert.equal(c.viewedChapter(), 'sql');
  const post = topic => fetch(app.panel_url + 'audit-topic', { method: 'POST', headers: { 'x-companion-client': 'panel' },
    body: JSON.stringify({ analysis_id: c.analysis, project_id: c.state.project, expected_revision: status.snapshot.revision, topic }) });
  assert.equal((await post('cpu')).status, 200);
  assert.equal(c.state.project, projects[0]); assert.equal(c.viewedChapter(), 'synthesis');
  assert.equal(c.displayedBoard().brief_id, 'load');
  assert.deepEqual(auditModel(c.state).briefs.map(b => b.brief_id), ['load']);
  assert.deepEqual(c.chapterViews().map(v => v.brief_id), ['load']);
  await assert.rejects(c.browseChapter('synthesis', 'wait-reference'), /No saved visual/);
  const restored = await app.call('companion_resume_saved_audit', { audit_id: status.audit_id });
  assert.equal(restored.analysis_id, 'analysis-2'); assert.equal(c.displayedBoard().title, 'load');
  await app.call('companion_switch_audit_topic', { topic: 'tx' }); assert.equal(c.displayedBoard().title, 'wait');
  await app.call('companion_preview_combined_report', { title: 'Shared decisions', tx: { brief_id: 'wait' }, cpu: { brief_id: 'load' } });
  const url = app.panel_url + 'client/combined/' + preview + '/';
  const mime = {html:/text\/html/, css:/text\/css/, png:/image\/png/, js:/text\/javascript/};
  for (const asset of assets) {
    const result = await fetch(url + asset);
    assert.equal(result.status, 200);
    assert.match(result.headers.get('content-type'), mime[asset.split('.').at(-1)]);
    if (asset === 'wykresy/tx-chart-1.html' || asset === 'index.html') assert.match(result.headers.get('content-security-policy'), /script-src 'self'/);
  }
  for (const asset of ['evidence.html', 'private.html', 'recipe.json', 'wykresy/private.html', 'wykresy/other.js', 'wykresy/cpu-chart-2.html', 'wykresy/next/report.css', 'tx-chart-1.html', 'report.zip']) {
    assert.equal((await fetch(url + asset)).status, 404);
  }
  await app.call('companion_publish_combined_report', { preview_id: preview, user_request: 'Generate the final client report.' });
  const issued = await fetch(url + 'report.zip'); assert.equal(issued.headers.get('content-disposition'), 'attachment; filename="jas-min-combined.zip"');
  assert.deepEqual(Buffer.from(await issued.arrayBuffer()), blob);
  await app.call('companion_switch_audit_topic', { topic: 'cpu' });
  assert.equal((await fetch(url + 'report.zip')).status, 404);
  await app.call('companion_preview_combined_report', { title: 'Legacy layout', tx: { brief_id: 'wait' }, cpu: { brief_id: 'load' } });
  const legacyUrl = app.panel_url + 'client/combined/' + older + '/';
  for (const file of legacyFiles) assert.equal((await fetch(legacyUrl + file)).status, 200);
  assert.doesNotMatch((await fetch(legacyUrl + 'index.html')).headers.get('content-security-policy'), /script-src 'self'/);
  assert.equal((await fetch(legacyUrl + 'wykresy/plotly.min.js')).status, 404);
  assert.match((await fetch(url + 'index.html')).headers.get('content-security-policy'), /script-src 'self'/);
  assert.equal((await fetch(url + 'wykresy/plotly.min.js')).status, 200);
});
