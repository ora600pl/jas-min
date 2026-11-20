import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from '../server.mjs';
import { panelTextChunks } from '../discussed-entities.mjs';

test('discussion and transient boards are checkpointed by project before the next operation', async t => {
  const app = await createServer();
  t.after(() => app.close());
  const projects = ['project-1', 'project-2'];
  const audit = { analysis_id: 'a', audit_id: 'audit', project_ids: projects, current_chapter_id: 'sql',
    chapters: [{ chapter_id: 'sql' }], briefs: [], snapshot: { persisted: true, revision: 1 } };
  const calls = [];
  let failNext = false;
  app.companion.client = {
    tools: async () => ['start_audit', 'get_audit_status', 'record_discussed_entities'].map(name => ({ name })),
    close: async () => {},
    call: async (name, args) => {
      if (name === 'get_audit_status') return structuredClone(audit);
      assert.equal(name, 'record_discussed_entities');
      if (failNext) { failNext = false; throw Error('temporary storage error'); }
      calls.push(args);
      return { revision: ++audit.snapshot.revision, added: 1 };
    },
  };
  await app.companion.tools();
  await app.companion.applyAudit(audit, projects[0]);
  const sql = '5jsn2k7v1q4cb';
  await app.call('companion_record_discussed_entities', { project_id: projects[1], sql_ids: [sql, sql.toUpperCase()], wait_events: ['log file sync','log file sync'] });
  assert.deepEqual(calls, [{ analysis_id: 'a', project_id: projects[1], sql_ids: [sql], wait_events: ['log file sync'] }]);
  await assert.rejects(app.call('companion_record_discussed_entities', { project_id: 'unknown', sql_ids: [sql] }));
  const board = { title: 'Compare intervals', takeaway: 'Inspect each interval.', panels: [
    { type: 'table', title: 'SQL', classification: 'measured', source: 'observed', columns: ['Instance','SQL ID','Wait event'], rows: [['A',sql,'—'],['B',null,'log file parallel write']], row_project_ids: projects },
    { type: 'flow', title: 'Mechanism', classification: 'hypothesis', source: 'discussion', project_id: projects[1], steps: [
      { title: 'Block', detail: 'gc buffer busy release' }, { title: 'Read', detail: 'read by other session' }] },
  ] };
  const shown = await app.call('companion_present', board);
  const byProject = Object.fromEntries(calls.slice(1).map(c => [c.project_id, c.panel_text]));
  assert.ok(byProject[projects[0]].includes(sql));
  assert.deepEqual(calls[1].sql_ids, [sql]);
  assert.deepEqual(calls[1].wait_events, []);
  assert.deepEqual(calls[2].wait_events, ['log file parallel write']);
  assert.ok(!byProject[projects[0]].includes('log file parallel write'));
  assert.ok(!byProject[projects[1]].includes(sql));
  assert.match(byProject[projects[1]], /log file parallel write/);
  assert.match(byProject[projects[1]], /gc buffer busy release/);
  const previousCount = calls.length;
  await app.call('companion_show', { view_id: shown.view_id });
  assert.equal(calls.length, previousCount);
  failNext = true;
  await assert.rejects(app.call('companion_note', { text: 'Also check log buffer space', project_id: projects[1] }), /temporary storage error/);
  await app.call('companion_context');
  assert.ok(calls.some(c => c.project_id === projects[1] && c.panel_text?.includes('log buffer space')));
  assert.ok(!calls.some(c => c.project_id === projects[0] && c.panel_text?.includes('log buffer space')));
  assert.equal(app.companion.state.audit.snapshot.revision, audit.snapshot.revision);
  assert.equal(app.companion.state.current, shown.view_id);
  assert.equal(app.companion.state.audit.current_chapter_id, 'sql');
  const view = { id: 'measurement', tool: 'get_sql_text', args: { sql_id: sql },
    data: { project_id: projects[0], evidence_id: 'E-7', result: { sql_id: sql, sql_text: 'select 1 from dual' } } };
  app.companion.record(view, false);
  const openRaw = (project_id = projects[0]) => fetch(app.panel_url + 'raw-evidence', {
    method: 'POST', headers: { 'x-companion-client': 'panel' },
    body: JSON.stringify({ analysis_id: 'a', project_id, view_id: view.id }),
  });
  assert.equal((await openRaw(projects[1])).status, 400);
  assert.equal((await openRaw()).status, 200);
  assert.equal((await openRaw()).status, 200);
  assert.deepEqual(calls.filter(c => c.evidence_ref), [{ analysis_id: 'a', project_id: projects[0], evidence_ref: 'E-7' }]);
});

test('bounded display requests preserve identifiers near Unicode chunk boundaries', () => {
  const name = 'log file parallel write';
  const long = 'ą'.repeat(8188) + name + '字'.repeat(17000);
  const chunks = [...panelTextChunks(['leading field', long, 'trailing field'])];
  assert.ok(chunks.every(s => Buffer.byteLength(s) <= 32768));
  assert.ok(chunks.some(s => s.includes(name)));
  assert.equal(chunks[0], 'leading field');
  assert.equal(chunks.at(-1), 'trailing field');
});

test('an instance label on an older comparison board scopes its row', async () => {
  const { panelEntitiesByProject } = await import('../discussed-entities.mjs');
  const projects = ['instance1_awr_day', 'instance2_awr_day'];
  const groups = panelEntitiesByProject({ panels: [{columns:['Instancja','Wait event'],rows:[['Instance2','observed wait']]}] },
    projects[0], projects);
  assert.ok(groups.get(projects[1]).wait_events.has('observed wait'));
  assert.ok(!groups.get(projects[0])?.wait_events.has('observed wait'));
});
