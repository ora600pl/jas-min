// Opt in through the isolated Rust companion_core_transport_integration test with
// JASMIN_AUDIT_BROWSER_QA=1. Playwright must be available to Node. BROWSER_PATH
// can point to an installed Chromium executable.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { createServer } from '../server.mjs';
import { Companion } from '../companion.mjs';

async function checkResumedGradientModels(browser, endpoint) {
  const project_id = 'gradient-fixture';
  const writer = new Companion();
  const resumed = await createServer();
  try {
    await writer.open({ endpoint, language: 'pl' });
    const { analysis_id } = await writer.select(project_id);
    const load = await writer.query('get_database_load_summary', { top_n: 3 }, false);
    const oldTitle = 'Liczba snapshotów w profilu obciążenia';
    const saved = writer.present({ title: oldTitle, takeaway: 'Liczba snapshotów w badanym okresie.', language: 'pl',
      panels: [{ type: 'table', title: 'Zakres próbki', source: load.evidence_id, classification: 'measured',
        columns: ['Miara', 'Wartość'], rows: [['Snapshoty', load.result.snapshot_count]] }] });
    await writer.recordBrief({ chapter_id: 'performance_profile', view_id: saved.view_id,
      question: 'Ile snapshotów obejmuje analiza?', explanation: 'Zliczono dostępne snapshoty.',
      conclusion: 'Zapisano zakres próbki.', evidence_boundary: 'Podsumowanie obejmuje całą próbkę.',
      next_step: 'Porównaj wyniki modeli.', evidence_refs: [load.evidence_id] });
    const prior = await writer.advanceAudit({ chapter_id: 'sql' });
    const before = (await writer.exportAudit()).checkpoint;
    const { panel_url } = await resumed.call('companion_open', { endpoint, language: 'pl' });
    await resumed.call('companion_resume_audit', { analysis_id, project_id });
    const page = await browser.newPage();
    page.setDefaultTimeout(8000);
    await page.goto(panel_url);
    await page.locator('.audit-chapters button[data-chapter-id="performance_profile"]').click();
    const chart = page.locator('#chart');
    const matrix = chart.locator('table').filter({ has: page.getByRole('columnheader', { name: /ridge/i }) }).first();
    await matrix.getByRole('columnheader', { name: /elastic net/i }).waitFor();
    const headers = await matrix.locator('th').allTextContents();
    const modelNames = ['ridge', 'elastic_net', 'huber', 'quantile95'];
    const indices = [/ridge/i, /elastic net/i, /huber/i, /(?:quantile|kwantyl|Q)\s*95/i].map(pattern => {
      const index = headers.findIndex(h => pattern.test(h));
      assert.ok(index >= 0, `Missing regression model in the rendered matrix: ${pattern}`);
      return index;
    });
    assert.ok(await chart.getByRole('columnheader', { name: 'P90 (impact_active)' }).isVisible());
    assert.ok(await chart.getByRole('columnheader', { name: 'P99 (impact_peak)' }).isVisible());
    assert.ok(await chart.getByRole('columnheader', { name: 'Status Quantile 95' }).isVisible());
    assert.ok(await matrix.locator('.coefficient-track > span').evaluateAll(bars =>
      bars.some(b => b.getBoundingClientRect().width > 1)));
    assert.ok(await page.evaluate(() => document.querySelector('#chart').compareDocumentPosition(
      document.querySelector('#gradient-discussion .audit-briefs')) & Node.DOCUMENT_POSITION_FOLLOWING));
    const dashboard = await resumed.call('companion_context', { include_displayed_board: true });
    assert.equal(dashboard.viewed_chapter_id, 'performance_profile');
    assert.equal(dashboard.audit.current_chapter_id, 'sql');
    assert.equal(dashboard.audit.revision, prior.revision);
    assert.equal(dashboard.audit.report_authorized, prior.report_authorized);
    const panel = dashboard.displayed_board.panels.find(p => p.columns?.some(c => /ridge/i.test(c)));
    assert.match(panel.source, /get_precomputed_analysis.*full_gradients/);
    const gradient = await writer.client.call('get_precomputed_analysis', { project_id, analysis_id,
      section: 'full_gradients', ranking: 'extreme', limit: 4, preview_only: true });
    const rendered = await matrix.locator('tbody tr').evaluateAll(rows => rows.map(r => Array.from(r.cells, c => c.textContent)));
    assert.ok(rendered.some(([family]) => family.includes('DB Time')));
    assert.ok(rendered.some(([family]) => family.includes('DB CPU')));
    const measuredModels = new Set();
    for (const [rowIndex, cells] of rendered.entries()) {
      const [family, predictor] = cells;
      const key = family.startsWith('DB Time') ? 'db_time_instance_stats_counters' : 'db_cpu_instance_stats';
      for (const [i, model] of modelNames.entries()) {
        const fitted = gradient.result.data[key].model_rankings[model].find(r => r.event_name === predictor);
        assert.equal(panel.rows[rowIndex][indices[i]], fitted?.gradient_coef ?? null);
        const value = cells[indices[i]];
        if (fitted) {
          measuredModels.add(model);
          const visible = Number(value.replaceAll(/\s/g, '').replace(',', '.').replace('−', '-'));
          const tolerance = Math.abs(fitted.gradient_coef) * 0.001;
          assert.ok(Number.isFinite(visible) && Math.abs(visible - fitted.gradient_coef) <= tolerance,
            `${family} / ${predictor} / ${model}: displayed ${value}, fitted ${fitted.gradient_coef}`);
        } else assert.equal(value, '—', 'A coefficient absent from the returned page stays unknown.');
      }
    }
    assert.deepEqual([...measuredModels].sort(), [...modelNames].sort());
    assert.deepEqual((await writer.exportAudit()).checkpoint, before);
    await page.setViewportSize({ width: 420, height: 800 });
    assert.ok(await matrix.evaluate(table => {
      const wrap = table.parentElement;
      return wrap.scrollWidth > wrap.clientWidth && wrap.getBoundingClientRect().right <= window.innerWidth;
    }), 'The narrow panel scrolls the model matrix within its own container.');
    await page.locator('#audit').getByRole('button', { name: oldTitle, exact: true }).click();
    await page.waitForFunction(title => document.querySelector('#title')?.textContent === title, oldTitle);
    assert.ok(await chart.getByText(String(load.result.snapshot_count), { exact: true }).isVisible());
    await page.close();
  } finally {
    await Promise.allSettled([writer.close(), resumed.close()]);
  }
}

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
    await checkResumedGradientModels(browser, endpoint);
  } finally {
    try { await app?.close(); } finally { await browser.close(); }
  }
}
