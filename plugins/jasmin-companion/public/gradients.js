const models = [
  ['ridge', 'Ridge'], ['elastic_net', 'Elastic Net'], ['huber', 'Huber'], ['quantile95', 'Quantile 95'],
];

const families = {
  db_time_foreground_wait_events: ['DB Time · foreground waits', 'DB Time · oczekiwania pierwszoplanowe'],
  db_time_sql_elapsed_time: ['DB Time · SQL elapsed', 'DB Time · czas wykonania SQL'],
  db_time_instance_stats_counters: ['DB Time · instance counters', 'DB Time · liczniki instancji'],
  db_time_instance_stats_volumes: ['DB Time · instance volumes', 'DB Time · wolumeny instancji'],
  db_time_instance_stats_time: ['DB Time · instance times', 'DB Time · czasy instancji'],
  db_cpu_sql_cpu_time: ['DB CPU · SQL CPU', 'DB CPU · czas CPU SQL'],
  db_cpu_instance_stats: ['DB CPU · instance statistics', 'DB CPU · statystyki instancji'],
  custom_wait_events: ['Custom target · waits', 'Własny cel · oczekiwania'],
  custom_instance_stats: ['Custom target · instance statistics', 'Własny cel · statystyki instancji'],
};

const units = { db_time_per_second: 'DB Time [s/s]', db_cpu_per_second: 'DB CPU [s/s]' };
const number = v => Number.isFinite(v) ? v : null;

// Only generated family labels can change on a retained board. Predictor names and
// every numerical column remain source data, even when they resemble UI copy.
export function localizeGradientFamilies(panels, language) {
  for (const panel of panels) {
    if (panel.type !== 'table' || !Array.isArray(panel.rows) || !Array.isArray(panel.gradient_families)) continue;
    panel.rows.forEach((row, i) => {
      const labels = families[panel.gradient_families[i]];
      if (Array.isArray(labels)) row[0] = labels[language === 'pl' ? 1 : 0];
    });
  }
}

function modelRows(fit, key) {
  // An empty page is meaningful, including when the legacy TOP lists contain other rows.
  const page = fit.model_rankings?.[key];
  const rows = Array.isArray(page) ? page : fit[`${key}_top`];
  return Array.isArray(rows) ? rows.filter(row => typeof row?.event_name === 'string' && row.event_name) : [];
}

export function gradientPresentation(result, source, language = 'en') {
  const t = (en, pl) => language === 'pl' ? pl : en;
  const data = result.data || {};
  const matrix = [], impacts = [], diagnostics = [], unconverged = [], unverified = [];
  let firstContributors = '';
  const orderedFamilies = [...new Set([...Object.keys(families), ...Object.keys(data)])];
  const available = orderedFamilies.filter(key => data[key] && typeof data[key] === 'object').map(key => [key, data[key]]);
  const familyLimit = available.length === 1 ? 20 : models.length;
  for (const [key, fit] of available) {
    const family = families[key]?.[language === 'pl' ? 1 : 0] || key;
    const settings = fit.settings || {};
    const targetUnit = units[settings.input_db_time_unit] || settings.input_db_time_unit || null;
    const modelPages = models.map(([name]) => modelRows(fit, name));
    const byName = modelPages.map(rows => new Map(rows.map(row => [row.event_name, row])));
    if (!firstContributors) {
      const leaders = new Map();
      modelPages.forEach(([first], i) => {
        if (!Number.isFinite(first?.gradient_coef)) return;
        const labels = leaders.get(first.event_name) || [];
        let model = models[i][1];
        if (models[i][0] === 'quantile95' && settings.quantile95?.converged === false) {
          model += t(' (diagnostic fit)', ' (brak zbieżności)');
        }
        labels.push(model);
        leaders.set(first.event_name, labels);
      });
      if (leaders.size) {
        const text = [...leaders].map(([name, labels]) => `${labels.join(', ')} → ${name}`).join(' · ');
        if (text.length < 500) firstContributors = `${family} · ${text}.`;
      }
    }
    // Interleave the returned ranks so every model contributes to the overview.
    const names = new Set();
    const depth = Math.max(...modelPages.map(rows => rows.length));
    for (let i = 0; i < depth && names.size < familyLimit; i++) {
      for (const rows of modelPages) {
        if (rows[i] && names.size < familyLimit) names.add(rows[i].event_name);
      }
    }
    for (const name of names) {
      const coefficients = byName.map(rows => number(rows.get(name)?.gradient_coef));
      if (coefficients.some(v => v !== null)) matrix.push([key, name, ...coefficients, targetUnit]);
    }
    // Across families show each model's first returned contributor. A family lookup
    // keeps the displayed contributors available for a closer P90/P99 comparison.
    for (const [index, [modelKey, model]] of models.entries()) {
      const shown = available.length === 1 ? modelPages[index].filter(row => names.has(row.event_name)) : modelPages[index].slice(0, 1);
      for (const row of shown) {
        const hasExtreme = Array.isArray(fit.model_rankings?.[modelKey]) || settings.methodology_version === 'gradient-v2' || row.impact_extreme > 0;
        impacts.push([key, model, row.event_name, targetUnit, number(row.impact_active), number(row.impact_peak),
          hasExtreme ? number(row.impact_extreme) : null]);
      }
    }
    const q = settings.quantile95;
    if (q?.converged === false) unconverged.push(key);
    if (q?.converged == null && modelPages[3].length) unverified.push(family);
    if (Object.keys(settings).length) diagnostics.push([key, targetUnit, number(settings.ridge_lambda),
      number(settings.elastic_net_lambda), number(settings.elastic_net_alpha), number(q?.lambda),
      q?.status || (q?.converged === true ? 'converged' : q?.converged === false ? 'unconverged' : null), number(q?.iterations)]);
  }

  const table = (title, columns, rows, note) => ({ type: 'table', classification: 'measured', source,
    title, columns, rows, note, number_format: 'significant', gradient_families: rows.map(row => row[0]) });
  const title = t('Gradient analysis · regression models', 'Analiza gradientowa · modele regresji');
  const familyColumn = t('Model family', 'Rodzina modeli');
  const predictor = t('Predictor', 'Predyktor');
  const panel = table(t('Signed regression coefficients (β)', 'Współczynniki regresji ze znakiem (β)'),
    [familyColumn, predictor, ...models.map(([, label]) => label), t('Target unit', 'Jednostka celu')], matrix,
    t('Each cell shows the estimated change in the target for a one-standard-deviation increase in the predictor delta. Bars share a scale within each row. A dash means the queried rows did not provide a coefficient.',
      'Komórka pokazuje szacowaną zmianę celu przy wzroście delty predyktora o jedno odchylenie standardowe. Słupki mają wspólną skalę w obrębie wiersza. Kreska oznacza brak współczynnika w pobranych wierszach.'));
  panel.coefficient_columns = [2, 3, 4, 5];
  if (unconverged.length) {
    panel.unconverged_q95_families = unconverged;
    const labels = unconverged.map(key => families[key]?.[language === 'pl' ? 1 : 0] || key);
    panel.note += t(' † Quantile 95 did not converge for ', ' † Quantile 95 nie osiągnął zbieżności dla ') +
      labels.join(', ') + t('. Treat its coefficients as diagnostic only.', '. Te współczynniki mają wyłącznie znaczenie diagnostyczne.');
  }
  if (unverified.length) panel.note += t(' Quantile 95 convergence was not reported for ', ' Brak informacji o zbieżności Quantile 95 dla ') + unverified.join(', ') + '.';
  const panels = [panel];
  if (impacts.length) panels.push(table(t('Modeled change at P90, P99 and the maximum', 'Modelowana zmiana przy P90, P99 i maksimum'),
    [familyColumn, 'Model', predictor, t('Target unit', 'Jednostka celu'), 'P90 (impact_active)', 'P99 (impact_peak)', 'Max (impact_extreme)'], impacts,
    t('P90, P99 and the maximum refer to absolute predictor changes across intervals, including zeros. The displayed values are modeled magnitudes in target units. Model agreement alone does not establish a cause.',
      'P90, P99 i maksimum dotyczą bezwzględnych zmian predyktora między przedziałami, z uwzględnieniem zer. Wartości to oszacowane wielkości zmiany w jednostkach celu. Sama zgodność modeli nie rozstrzyga o przyczynie.')));
  if (diagnostics.length) panels.push(table(t('Fit parameters and convergence', 'Parametry dopasowania i zbieżność'),
    [familyColumn, t('Target unit', 'Jednostka celu'), 'Ridge λ', 'Elastic Net λ', 'Elastic Net α', 'Quantile 95 λ', t('Quantile 95 status', 'Status Quantile 95'), t('Q95 iterations', 'Iteracje Q95')], diagnostics,
    t('Quantile 95 fits the conditional 95th percentile of the target delta across all observations. The status and iteration count come from the solver.',
      'Quantile 95 dopasowuje warunkowy 95. percentyl delty celu na wszystkich obserwacjach. Status i liczba iteracji pochodzą z procedury dopasowania.')));
  let takeaway = matrix.length
    ? t('Compare the direction and size of β for the same predictor across four models. The table shows a selection from the returned ranking page.',
      'Porównaj kierunek i wielkość β dla tego samego predyktora w czterech modelach. Tabela pokazuje wybrane pozycje z pobranej strony rankingu.')
    : t('The selected scope and ranking page returned no regression coefficients.', 'Dla wybranego zakresu i strony rankingu serwer nie zwrócił współczynników regresji.');
  const ranked = available.filter(([, fit]) => Object.keys(fit.model_rankings || {}).length > 0);
  const returnedRankings = new Set(ranked.map(([, fit]) => fit.ranking).filter(Boolean));
  const order = {
    selection: t('Selection across P90, P99 and the maximum', 'Wybór według P90, P99 i maksimum'),
    active: 'impact_active (P90 |Δx|)', peak: 'impact_peak (P99 |Δx|)', extreme: 'impact_extreme (max |Δx|)',
  };
  if (matrix.length && returnedRankings.size === 1) {
    const label = order[[...returnedRankings][0]];
    if (label) takeaway += ' Ranking · ' + label + '.';
  }
  if (firstContributors) takeaway = t('First on this page for ', 'Pierwsze pozycje pobranej strony dla ') + firstContributors + ' ' + takeaway;
  if (available.some(([, fit]) => models.some(([m]) => !Array.isArray(fit.model_rankings?.[m]) && fit[m + '_top']?.length))) {
    takeaway += t(' For fits without a full ranking, this view uses the saved TOP lists.', ' Dla modeli bez pełnego rankingu panel wykorzystuje zapisane listy TOP.');
  }
  localizeGradientFamilies(panels, language);
  return { title, takeaway, panels };
}
