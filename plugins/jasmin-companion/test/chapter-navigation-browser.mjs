// Opt in through the isolated Rust companion_core_transport_integration test with
// JASMIN_AUDIT_BROWSER_QA=1. Playwright must be available to Node. BROWSER_PATH
// can point to an installed Chromium executable.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { createServer } from '../server.mjs';

export async function checkChapterNavigation(endpoint, analysis_id, project_id) {
  const { chromium } = createRequire(import.meta.url)('playwright');
  const browser = await chromium.launch({ executablePath: process.env.BROWSER_PATH });
  let app;
  try {
    app = await createServer();
    const { panel_url } = await app.call('companion_open', { endpoint, mode: 'live', language: 'pl' });
    const audit = await app.call('companion_resume_audit', { analysis_id, project_id });
    assert.equal(audit.report_authorized, true);
    assert.equal(app.companion.state.views.length, 0);
    const active = audit.current_chapter_id;
    assert.notEqual(active, 'scope');
    const scope = audit.briefs.find(b => b.chapter_id === 'scope' && b.project_id === project_id);
    assert.ok(scope.visual?.panels?.length);
    const page = await browser.newPage();
    page.setDefaultTimeout(8000);
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.goto(panel_url);
    const stages = page.locator('.audit-chapters button[data-chapter-id]');
    const stage = id => page.locator(`.audit-chapters button[data-chapter-id="${id}"]`);
    await stages.first().waitFor();
    assert.equal(await stages.count(), audit.chapters.length);
    assert.deepEqual((await stages.evaluateAll(items => items.map(item => item.dataset.chapterId))).slice(0, 2),
      ['scope', 'performance_profile']);
    assert.match(await stage('performance_profile').innerText(), /gradient.*anomali/i);

    // The panel starts with an empty local history. The scope visual is available
    // only from the previously saved brief.
    await stage('scope').focus();
    await page.keyboard.press('Enter');
    await page.waitForFunction(title => document.querySelector('#title')?.textContent === title, scope.visual.title);
    assert.equal(await page.evaluate(() => document.activeElement?.dataset.chapterId), 'scope');
    assert.ok(await page.getByText(scope.question, { exact: true }).first().isVisible());
    assert.ok(await page.locator('#chart').getByText(scope.visual.panels[0].title, { exact: true }).isVisible());
    assert.equal(await stage('scope').getAttribute('aria-pressed'), 'true');
    assert.equal(await stage(active).getAttribute('aria-current'), 'step');
    const selected = await app.call('companion_context');
    assert.equal(selected.viewed_chapter_id, 'scope');
    assert.equal(selected.audit.current_chapter_id, active);
    assert.equal(selected.audit.revision, audit.revision);
    assert.equal(selected.audit.report_authorized, true);

    await stage('performance_profile').click();
    await page.waitForFunction(() =>
      document.querySelector('.audit-chapters button[data-chapter-id="performance_profile"]')?.getAttribute('aria-pressed') === 'true');
    const profile = audit.briefs.find(b => b.chapter_id === 'performance_profile' && b.project_id === project_id);
    assert.ok(await page.getByText(profile.question, { exact: true }).first().isVisible());
    if (!profile.visual) assert.notEqual(await page.locator('#title').innerText(), scope.visual.title);

    // Neither of these chapter views is attached to a brief. The source query
    // is reused so browsing can be checked against the existing audit revision.
    const evidenceArgs = { tool: 'get_database_load_summary', arguments: { top_n: 3 }, display: false };
    const sample = await app.call('companion_query', evidenceArgs);
    async function displayProfileBoard(title, label, value) {
      const board = await app.call('companion_present', { language: 'pl', title, takeaway: label,
        panels: [{ type: 'table', title: label, classification: 'measured', source: sample.evidence_id,
          columns: ['Miara', 'Wartość'], rows: [[label, value]] }] });
      await page.waitForFunction(text => document.querySelector('#title')?.textContent === text, title);
      return board.view_id;
    }
    const firstTitle = 'Liczba snapshotów w próbce';
    const secondTitle = 'DB Time w zebranej próbce';
    const firstView = await displayProfileBoard(firstTitle, 'Liczba snapshotów', sample.result.snapshot_count);
    const secondView = await displayProfileBoard(secondTitle, 'Łączny DB Time [s]', sample.result.total_db_time_s);
    await stage(active).click();
    await page.waitForFunction(id =>
      document.querySelector('.audit-chapters button[aria-pressed="true"]')?.dataset.chapterId === id, active);
    for (let i = 0; i < 24; i++) await app.call('companion_query', evidenceArgs);
    for (const id of [firstView, secondView]) assert.ok(!app.companion.state.views.some(v => v.id === id));
    await stage('performance_profile').click();
    await page.waitForFunction(title => document.querySelector('#title')?.textContent === title, secondTitle);
    await page.locator('#audit').getByRole('button', { name: firstTitle, exact: true }).click();
    await page.waitForFunction(title => document.querySelector('#title')?.textContent === title, firstTitle);
    const recalled = await app.call('companion_context');
    assert.equal(recalled.viewed_chapter_id, 'performance_profile');
    assert.equal(recalled.current_view_id, firstView);
    assert.equal(recalled.views.find(v => v.view_id === firstView)?.title, firstTitle);
    for (const id of [firstView, secondView]) assert.ok(recalled.chapter_views.some(v => v.view_id === id));
    assert.equal(await page.locator('#chart tbody tr').first().locator('td').last().innerText(), String(sample.result.snapshot_count));
    assert.equal(recalled.audit.current_chapter_id, active);
    assert.equal(recalled.audit.revision, audit.revision);
    assert.equal(recalled.audit.report_authorized, true);

    await stage(active).click();
    await page.waitForFunction(id =>
      document.querySelector('.audit-chapters button[aria-pressed="true"]')?.dataset.chapterId === id, active);
    const returned = await app.call('companion_context');
    assert.equal(returned.audit.current_chapter_id, active);
    assert.equal(returned.audit.revision, audit.revision);
    assert.equal(returned.audit.report_authorized, true);
    assert.deepEqual(errors, []);
  } finally {
    try { await app?.close(); } finally { await browser.close(); }
  }
}
