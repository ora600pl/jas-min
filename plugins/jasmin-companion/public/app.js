import { renderChart } from './charts.js';
const $ = id => document.getElementById(id);
const names = { get_metric_time_series: 'Obciążenie bazy', top_wait_events_in_snapshot: 'Na co czeka baza?', top_sqls_in_snapshot: 'SQL pod lupą', get_database_load_summary: 'Podsumowanie obciążenia', list_snapshots: 'Snapshoty' };
let revision = -1;
async function action(route, args) {
  try {
    const r = await fetch(route, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Companion-Client': 'panel' }, body: JSON.stringify(args) });
    const data = await r.json(); if (!r.ok) throw new Error(data.error); revision = -1;
  } catch (e) { $('error').textContent = e.message; }
}
function render(state) {
  document.body.classList.toggle('working', state.status === 'working');
  $('activity').textContent = state.status === 'working' ? 'Sprawdzam dane…' : 'Gotowy do pracy';
  $('mode').textContent = state.mode === 'demo' ? 'DEMO · DANE SYNTETYCZNE' : state.mode === 'live' ? 'JAS-MIN · MCP' : 'OCZEKIWANIE NA CODEX';
  $('project').textContent = state.project || '';
  const view = state.views.find(v => v.id === state.current);
  const selection = state.selection;
  $('selection').textContent = selection.sql_id ? `SQL ${selection.sql_id} · #${selection.snap_id}` : selection.snap_id != null ? `Snapshot #${selection.snap_id}` : 'Wybierz punkt na wykresie';
  $('history').replaceChildren();
  for (const item of [...state.views].reverse()) {
    const b = document.createElement('button');
    b.textContent = item.args.name || names[item.tool] || item.tool;
    if (item.args.snap_id != null) b.textContent += ` · #${item.args.snap_id}`;
    b.classList.toggle('active', item.id === state.current); b.setAttribute('aria-pressed', String(item.id === state.current));
    b.onclick = () => action('show', { view_id: item.id }); $('history').append(b);
  }
  if (view) {
    $('title').textContent = view.args.name || names[view.tool] || view.tool;
    $('evidence').textContent = view.data.evidence_id || 'Odpowiedź MCP';
    renderChart($('chart'), view, point => action('selection', { snap_id: point.snap_id, ...(point.sql_id ? { sql_id: point.sql_id } : {}) }));
    for (const p of document.querySelectorAll('.point')) p.classList.toggle('selected', p.dataset.snapId === String(selection.snap_id));
    $('raw').textContent = JSON.stringify(view.data, null, 2);
  } else {
    $('title').textContent = 'Czekam na pierwsze pytanie'; $('chart').textContent = 'Powiedz w Codex: „Pokaż obciążenie bazy”.'; $('evidence').textContent = ''; $('raw').textContent = '{}';
  }
  $('note').textContent = state.note;
}
async function poll() {
  try {
    const response = await fetch('state'); if (!response.ok) throw new Error('Panel niedostępny. Otwórz ponownie przez companion_open.');
    const state = await response.json();
    if (state.revision !== revision) { render(state); revision = state.revision; }
    $('error').textContent = '';
  } catch (e) { $('error').textContent = e.message; $('activity').textContent = 'Utracono połączenie z panelem'; }
  setTimeout(poll, 1000);
}
poll();
