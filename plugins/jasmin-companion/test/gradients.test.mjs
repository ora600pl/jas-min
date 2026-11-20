import test from 'node:test';
import assert from 'node:assert/strict';
import { automaticPresentation, localizedView } from '../public/charts.js';
import { validatePresentation } from '../presentation.mjs';

function board(data, language = 'en', source = 'E-0007') {
  return automaticPresentation({ tool: 'get_precomputed_analysis',
    data: { evidence_id: source, result: { section: 'full_gradients', data } } }, language);
}

test('gradient board compares signed coefficients for the same contributor in all returned model fits', () => {
  const io = { event_name: 'read wait', gradient_coef: 0.000034, impact_active: 0, impact_peak: 2.5, impact_extreme: 8.7 };
  const lock = { event_name: 'lock wait', gradient_coef: 0.33, impact_active: 0.7, impact_peak: 3.1 };
  const fit = { ranking: 'extreme', model_rankings: {
    ridge: [io, lock], elastic_net: [{ ...io, gradient_coef: 0 }], huber: [{ ...io, gradient_coef: -0.002 }],
    quantile95: [lock],
  }, settings: { input_db_time_unit: 'db_time_per_second', ridge_lambda: 0.1,
    elastic_net_lambda: 0.025, elastic_net_alpha: 0.7, quantile95: { lambda: 0.0005, converged: false, status: 'iteration_limit', iterations: 20000 } } };
  const result = board({ db_cpu_sql_cpu_time: null, db_time_foreground_wait_events: fit }, 'pl');
  validatePresentation(result);
  const [matrix, impacts, settings] = result.panels;
  assert.match(result.takeaway, /Ridge, Elastic Net, Huber → read wait.*Quantile 95 \(brak zbieżności\) → lock wait/);
  assert.match(result.takeaway, /max \|Δx\|/);
  assert.deepEqual(matrix.rows, [
    ['DB Time · oczekiwania pierwszoplanowe', 'read wait', 0.000034, 0, -0.002, null, 'DB Time [s/s]'],
    ['DB Time · oczekiwania pierwszoplanowe', 'lock wait', 0.33, null, null, 0.33, 'DB Time [s/s]'],
  ]);
  assert.deepEqual(matrix.columns.slice(2, 6), ['Ridge', 'Elastic Net', 'Huber', 'Quantile 95']);
  assert.match(matrix.note, /nie osiągnął zbieżności/);
  assert.ok(impacts.rows.some(row => row[1] === 'Ridge' && row[2] === 'read wait' && row[4] === 0 && row[5] === 2.5 && row[6] === 8.7));
  assert.deepEqual(settings.rows[0].slice(2), [0.1, 0.025, 0.7, 0.0005, 'iteration_limit', 20000]);
  assert.ok(result.panels.every(panel => panel.source === 'E-0007'));
  assert.equal(matrix.number_format, 'significant');
  const retained = localizedView({ presentation: result }, 'en').presentation;
  assert.equal(retained.panels[0].rows[0][0], 'DB Time · foreground waits');
  assert.deepEqual(retained.panels[0].rows[0].slice(1), matrix.rows[0].slice(1));
  assert.equal(retained.panels[0].unconverged_q95_families[0], 'db_time_foreground_wait_events');
});

test('compact older sources use the actual TOP lists, while an explicitly empty full ranking page remains empty', () => {
  const row = { event_name: 'sql-1', gradient_coef: -0.005, impact_peak: 0 };
  const legacy = board({ db_cpu_sql_cpu_time: { ridge_top: [row], quantile95_top: [row] } });
  assert.equal(legacy.panels[0].rows[0][2], -0.005);
  assert.equal(legacy.panels[0].rows[0][3], null);
  assert.equal(legacy.panels[0].rows[0][6], null);
  assert.equal(legacy.panels[1].rows[0][4], null);
  assert.match(legacy.panels[0].note, /convergence was not reported/);
  for (const data of [
    { db_time_foreground_wait_events: null },
    { db_cpu_sql_cpu_time: { model_rankings: { ridge: [] }, ridge_top: [row] } },
  ]) {
    const empty = board(data, 'en', null);
    assert.match(empty.takeaway, /returned no regression coefficients/);
    assert.deepEqual(empty.panels[0].rows, []);
    assert.equal(empty.panels[0].source, 'get_precomputed_analysis(full_gradients)');
  }
});
