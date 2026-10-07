// Fictional data only. Deliberately mirrors JAS-MIN tool response shapes.
const values = [3.2, 2.8, 3.5, 4.1, 5.8, 9.2, 18.4, 24.6, 16.8, 8.4, 5.1, 3.7];
export const snapshots = values.map((value, i) => ({ snap_id: 100 + i,
  begin_snap_time: `2026-10-01 ${String(8 + Math.floor(i / 2)).padStart(2, '0')}:${i % 2 ? '30' : '00'}:00`,
  end_snap_time: `2026-10-01 ${String(8 + Math.floor((i + 1) / 2)).padStart(2, '0')}:${(i + 1) % 2 ? '30' : '00'}:00`, value }));
const schema = (name, properties = {}, required = []) => ({ name, description: `Demo: ${name}`, inputSchema: { type: 'object', properties, required } });
export class DemoClient {
  async initialize() {}
  async close() {}
  async tools() { return [
    schema('get_metric_time_series', { kind: { type: 'string' }, name: { type: 'string' } }, ['kind', 'name']),
    schema('list_snapshots', { limit: { type: 'integer' } }),
    schema('top_wait_events_in_snapshot', { snap_id: { type: 'integer' } }, ['snap_id']),
    schema('top_sqls_in_snapshot', { snap_id: { type: 'integer' } }, ['snap_id']),
    schema('get_database_load_summary'),
  ]; }
  async call(name, args = {}) {
    if (name === 'list_performance_projects') return { projects: [{ project_id: 'demo', db_name: 'DEMO · dane syntetyczne', date_from: snapshots[0].begin_snap_time, date_to: snapshots.at(-1).end_snap_time }] };
    if (name === 'start_performance_analysis') return { analysis_id: 'demo-analysis', dataset_manifest: { project_id: 'demo' } };
    let result;
    if (name === 'get_metric_time_series') {
      const cpu = /cpu/i.test(args.name);
      result = { kind: args.kind, name: args.name, field: 'value', points: snapshots.length,
        series: snapshots.map(p => ({ ...p, value: cpu ? Math.round(p.value * .36 * 100) / 100 : p.value })) };
    } else if (name === 'list_snapshots') result = { snapshots };
    else if (name === 'top_wait_events_in_snapshot') result = { snap_id: args.snap_id, rank_by: 'pct_dbtime', items: [
      { event: 'db file sequential read', pct_dbtime: 42.8, total_wait_time_s: 18952 },
      { event: 'log file sync', pct_dbtime: 12.4, total_wait_time_s: 5491 },
      { event: 'enq: TX - row lock contention', pct_dbtime: 6.8, total_wait_time_s: 3011 },
      { event: 'direct path read', pct_dbtime: 2.1, total_wait_time_s: 930 },
    ] };
    else if (name === 'top_sqls_in_snapshot') result = { snap_id: args.snap_id, metric: 'elapsed_time', items: [
      { sql_id: 'demo_sql_001', elapsed_time_s: 12480, executions: 480, module: 'DEMO · order-service' },
      { sql_id: 'demo_sql_002', elapsed_time_s: 6120, executions: 24, module: 'DEMO · reporting' },
      { sql_id: 'demo_sql_003', elapsed_time_s: 2840, executions: 8200, module: 'DEMO · inventory' },
    ] };
    else if (name === 'get_database_load_summary') result = { source: 'SYNTHETIC DEMO', snapshots: snapshots.length, note: 'To fikcyjny scenariusz, nie diagnoza bazy.' };
    else throw new Error('Narzędzie niedostępne w demo.');
    return { analysis_id: 'demo-analysis', project_id: 'demo', evidence_id: `DEMO-${name}`, tool_name: name, result };
  }
}
