export function seriesPoints(data) {
  return (Array.isArray(data?.series) ? data.series : []).filter(p => Number.isFinite(p.value) && p.value !== null);
}
export function unitFor(data) {
  if (data.kind === 'load_profile' && /db.?time|db.?cpu/i.test(data.name)) return 's/s · average active sessions';
  if (data.field === 'pct_dbtime' || (data.kind?.startsWith('wait_event') && data.field === 'value')) return '% DB Time';
  if (data.field?.startsWith('pct_')) return '%';
  if (data.field === 'total_wait_time_s' || data.kind === 'time_model') return 'seconds';
  return data.field === 'per_second' ? 'per second' : (data.field || 'value');
}
const node = (tag, text, cls) => { const el = document.createElement(tag); if (text !== undefined) el.textContent = text; if (cls) el.className = cls; return el; };
const svgNode = (tag, attrs = {}, text) => { const el = document.createElementNS('http://www.w3.org/2000/svg', tag); for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v); if (text !== undefined) el.textContent = text; return el; };
const fmt = n => Number.isFinite(n) ? n.toLocaleString('en-US', { maximumFractionDigits: 2 }) : '—';
export function renderChart(container, view, onSelect) {
  container.replaceChildren();
  const data = view.data.result ?? view.data;
  if (Array.isArray(data.series) && data.series.some(p => 'value' in p)) {
    const points = seriesPoints(data);
    if (!points.length) { container.append(node('p', 'No measurements for this metric. This does not mean zero.', 'chart-note')); return; }
    const w = 760, h = 250, left = 49, right = 18, top = 25, bottom = 42;
    const low = Math.min(0, ...points.map(p => p.value)), high = Math.max(1, ...points.map(p => p.value)) * 1.12;
    const x = i => left + (w - left - right) * i / Math.max(1, points.length - 1);
    const y = v => top + (h - top - bottom) * (high - v) / (high - low);
    const svg = svgNode('svg', { viewBox: `0 0 ${w} ${h}`, role: 'group', 'aria-label': `${data.name}, ${unitFor(data)}. Points can be selected using the keyboard.` });
    for (let i = 0; i <= 4; i++) {
      const value = low + (high - low) * i / 4;
      svg.append(svgNode('line', { x1: left, y1: y(value), x2: w - right, y2: y(value), stroke: '#28363b', 'stroke-dasharray': '3 5' }));
      svg.append(svgNode('text', { x: left - 9, y: y(value) + 4, 'text-anchor': 'end' }, fmt(value)));
    }
    // Snapshot order, not a continuous time axis; do not silently interpolate missing rows.
    const line = points.map((p, i) => `${i === 0 || Number(p.snap_id) !== Number(points[i-1].snap_id) + 1 ? 'M' : 'L'}${x(i)},${y(p.value)}`).join(' ');
    svg.append(svgNode('path', { d: line, fill: 'none', stroke: '#70d8be', 'stroke-width': '2.5' }));
    const every = Math.max(1, Math.ceil(points.length / 6));
    points.forEach((p, i) => {
      if (i % every === 0 || i === points.length - 1) svg.append(svgNode('text', { x: x(i), y: h - 14, 'text-anchor': i === 0 ? 'start' : i === points.length - 1 ? 'end' : 'middle' }, `#${p.snap_id}`));
      const c = svgNode('circle', { cx: x(i), cy: y(p.value), r: points.length > 100 ? 2 : 4, class: 'point', 'data-snap-id': p.snap_id, tabindex: '0', role: 'button', 'aria-label': `Snapshot ${p.snap_id}, ${p.begin_snap_time}, ${fmt(p.value)}` });
      c.append(svgNode('title', {}, `${p.begin_snap_time} → ${p.end_snap_time}\nSnapshot ${p.snap_id}: ${fmt(p.value)}`));
      c.addEventListener('click', () => onSelect(p)); c.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onSelect(p); } }); svg.append(c);
    });
    container.append(node('p', unitFor(data), 'chart-note'), svg, node('p', `${points.length} measurements · X axis: available snapshots in order · select a point to set context`, 'chart-note'));
  } else if (view.tool === 'top_wait_events_in_snapshot' && Array.isArray(data.items)) {
    const field = data.rank_by || 'pct_dbtime';
    const unit = field === 'pct_dbtime' ? '% DB Time' : field === 'total_wait_time_s' ? 's' : field;
    const max = Math.max(1, ...data.items.map(x => Number(x[field]) || 0));
    for (const item of data.items) {
      const row = node('div', undefined, 'bar-row'), head = node('div', undefined, 'bar-top');
      head.append(node('span', item.event), node('span', `${fmt(item[field])} ${unit}`));
      const track = node('div', undefined, 'bar-track'), fill = node('div', undefined, 'bar-fill');
      fill.style.width = `${Math.max(0, (Number(item[field]) || 0) / max * 100)}%`; track.append(fill); row.append(head, track); container.append(row);
    }
    if (!data.items.length) container.append(node('p', 'No wait events in the response.', 'chart-note'));
  } else if (view.tool === 'top_sqls_in_snapshot' && Array.isArray(data.items)) {
    const metrics = { elapsed_time: ['elapsed_time_s', 'Elapsed [s]'], cpu_time: ['cpu_time_s', 'CPU [s]'], io_time: ['io_time_s', 'I/O [s]'], buffer_gets: ['buffer_gets', 'Buffer gets'], physical_reads: ['physical_reads', 'Physical reads'] };
    const [field, label] = metrics[data.metric] || metrics.elapsed_time;
    const table = node('table'), tr = node('tr');
    for (const title of ['SQL ID', label, 'Executions', 'Module']) tr.append(node('th', title));
    const thead = node('thead'); thead.append(tr); table.append(thead);
    const tbody = node('tbody');
    for (const item of data.items) {
      const row = node('tr'), cell = node('td'), button = node('button', item.sql_id);
      button.addEventListener('click', () => onSelect({ sql_id: item.sql_id, snap_id: data.snap_id })); cell.append(button);
      row.append(cell, node('td', fmt(item[field])), node('td', fmt(item.executions)), node('td', item.sql_module || item.module || '—')); tbody.append(row);
    }
    table.append(tbody); const wrap = node('div', undefined, 'table-wrap'); wrap.append(table); container.append(wrap);
    if (!data.items.length) container.append(node('p', 'No SQL statements in the response.', 'chart-note'));
  } else {
    container.append(node('p', 'Tool response — full evidence below.', 'chart-note'), node('pre', JSON.stringify(data, null, 2)));
  }
}
