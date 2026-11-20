import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { createServer } from '../server.mjs';

test('the report URL serves its whole reviewed edition after upstream expiry and a later preview', async t => {
  const app = await createServer();
  t.after(() => app.close());
  const sql = 'wykresy/sql-p1-5jsn2k7v1q4cb.html';
  const wait = 'wykresy/wait-p1-' + '9'.repeat(64) + '.html';
  const js = 'wykresy/jasmin-plot.js';
  const assets = {
    'raport.html': '<a href="' + sql + '">SQL</a><a href="' + wait + '">Wait</a>',
    [sql]: '<pre>select count(*) from sample_orders</pre>',
    [wait]: '<section id="fg">Foreground chart</section>',
    [js]: '/* offline chart code */',
  };
  const one = '1'.repeat(64), two = '2'.repeat(64);
  const c = app.companion;
  const status = { analysis_id: 'analysis-1', audit_id: 'audit-1', project_ids: ['project-1'],
    pilot_kind: 'general', current_chapter_id: 'sql', chapters: [{chapter_id: 'sql'}],
    briefs: [], snapshot: {persisted: true, revision: 7} };
  let id = one, expired = false, badAsset;
  const calls = [];
  const tools = ['get_audit_status', 'get_client_report_plan', 'save_client_report_plan',
    'preview_client_report', 'read_client_report_asset', 'publish_client_report'];
  c.client = {
    close: async () => {},
    tools: async () => tools.map(name => ({name})),
    call: async (name, args) => {
      calls.push([name, args.asset]);
      if (expired) throw Error('MCP HTTP 404. Check the server and session.');
      if (name === 'preview_client_report') return {preview_id:id, entry_file:'raport.html', files:Object.keys(assets)};
      assert.equal(name, 'read_client_report_asset');
      assert.equal(args.analysis_id, c.analysis);
      assert.equal(args.preview_id, id);
      if (args.asset === badAsset) throw Error('Interrupted asset read');
      const bytes = Buffer.from(assets[args.asset] + id);
      return {bytes:bytes.length, sha256:createHash('sha256').update(bytes).digest('hex'),
        content_base64:bytes.toString('base64'), offset:0, next_offset:null};
    },
  };
  await c.tools();
  await c.applyAudit(status);
  const first = await app.call('companion_preview_client_report');
  assert.equal(await (await fetch(first.panel_preview_url)).text(), assets['raport.html'] + one);
  expired = true;
  const before = calls.length;
  // Each URL below is being requested for the first time by the browser.
  for (const name of [sql, wait, js]) {
    const source = await fetch(new URL(name, first.panel_preview_url));
    assert.equal(source.status, 200);
    assert.equal(await source.text(), assets[name] + one);
  }
  assert.equal((await fetch(new URL('report.zip', first.panel_preview_url))).status, 404);
  assert.equal(calls.length, before);

  expired = false;
  await c.applyAudit({...status, audit_id:'audit-2', analysis_id:'analysis-2'});
  id = two;
  badAsset = js;
  await assert.rejects(app.call('companion_preview_client_report'));
  const secondUrl = first.panel_preview_url.replace(one, two);
  assert.equal((await fetch(secondUrl)).status, 404);
  assert.equal(await (await fetch(first.panel_preview_url)).text(), assets['raport.html'] + one);
  badAsset = undefined;
  const second = await app.call('companion_preview_client_report');
  assert.equal(second.panel_preview_url, secondUrl);
  expired = true;
  const after = calls.length;
  for (const [url, edition] of [[first.panel_preview_url, one], [second.panel_preview_url, two]]) {
    assert.equal(await (await fetch(url)).text(), assets['raport.html'] + edition);
    assert.equal(await (await fetch(new URL(sql, url))).text(), assets[sql] + edition);
  }
  assert.equal(calls.length, after);
});
