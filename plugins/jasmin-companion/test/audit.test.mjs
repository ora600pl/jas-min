import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
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
