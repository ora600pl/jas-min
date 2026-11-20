// Compact enough to attach to a brief. The evidence record retains all paired intervals.
export function cpuScenarioPresentation(model, source, language = 'en') {
  const t = (en, pl) => language === 'pl' ? pl : en;
  const format = value => new Intl.NumberFormat(language, { maximumFractionDigits: 2 }).format(value);
  const title = t('Cluster CPU scenario', 'Scenariusz CPU klastra');
  if (!Number.isFinite(model?.observed_peak?.active_cpu) || !Number.isFinite(model?.scenario_peak?.active_cpu) ||
      !Number.isFinite(model?.saved_cpu_hours) || !Number.isInteger(model?.paired_intervals)) return { title, panels: [] };
  const rows = [['RAC', model], ...(model.instances || []).map(instance => [
    instance.instance_label || `${instance.project_id} · #${instance.instance_number}`, instance,
  ])];
  const columns = [t('Scope', 'Zakres'), t('From', 'Od'), t('To', 'Do'), t('Active CPU', 'Aktywne CPU')];
  const peak = value => [value?.begin ?? null, value?.end ?? null, value?.active_cpu ?? null];
  const assumptions = model.assumptions;
  const change = typeof assumptions?.monitor_module_prefix === 'string' && Array.isArray(assumptions.selected_sql_ids) &&
      Number.isFinite(assumptions.sql_reduction_pct) && Number.isFinite(assumptions.monitor_reduction_pct)
    ? t(`The model removes ${format(assumptions.monitor_reduction_pct)}% of observed CPU for modules with prefix ${assumptions.monitor_module_prefix} and reduces CPU by ${format(assumptions.sql_reduction_pct)}% for ${assumptions.selected_sql_ids.length} selected SQL IDs. `,
      `Model usuwa ${format(assumptions.monitor_reduction_pct)}% zmierzonego CPU modułów z prefiksem ${assumptions.monitor_module_prefix} i zmniejsza koszt CPU ${assumptions.selected_sql_ids.length} wybranych SQL o ${format(assumptions.sql_reduction_pct)}%. `)
    : '';
  const takeaway = t(
    `Across ${model.paired_intervals} paired intervals, the observed cluster peak was ${format(model.observed_peak.active_cpu)} active CPU. The scenario peak is ${format(model.scenario_peak.active_cpu)} active CPU. Estimated work reduction over the period is ${format(model.saved_cpu_hours)} CPU-hours.`,
    `W ${model.paired_intervals} sparowanych przedziałach zmierzony szczyt klastra wyniósł ${format(model.observed_peak.active_cpu)} aktywnego CPU. Szczyt w scenariuszu wynosi ${format(model.scenario_peak.active_cpu)} aktywnego CPU. Modelowana redukcja pracy w całym okresie wynosi ${format(model.saved_cpu_hours)} CPU-godz.`
  );
  const panels = [
    { type: 'table', title: t('Observed peak CPU', 'Zmierzony szczyt CPU'), classification: 'measured', source, columns,
      rows: rows.map(([name, row]) => [name, ...peak(row.observed_peak)]),
      note: t('The cluster series sums the simultaneous instance averages. Each instance peak has its own time window.',
        'Wynik klastra to suma średnich aktywnych CPU poszczególnych instancji w tym samym przedziale. Szczyty poszczególnych instancji mają własne okna czasowe.') },
    { type: 'table', title: t('Modeled peak and work reduction', 'Modelowany szczyt i redukcja pracy'), classification: 'hypothesis', source,
      columns: [...columns, t('Modeled reduction [CPU-hours]', 'Modelowana redukcja [CPU-godz.]')],
      rows: rows.map(([name, row]) => [name, ...peak(row.scenario_peak), row.saved_cpu_hours ?? null]),
      note: change + t('The peak represents average active CPU in the busiest modeled interval. CPU-hours measure work across the period.',
        'Szczyt oznacza średnią liczbę aktywnych CPU w najbardziej obciążonym przedziale scenariusza. CPU-godziny opisują pracę w całym okresie.') },
  ];
  return { title, takeaway, panels };
}
