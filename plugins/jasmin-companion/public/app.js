import { renderChart, automaticPresentation } from './charts.js';
const $ = id => document.getElementById(id);
let revision = -1, renderedView = null, actionError = '';
async function action(route, args) {
  try {
    const r = await fetch(route, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Companion-Client': 'panel' }, body: JSON.stringify(args) });
    const data = await r.json(); if (!r.ok) throw new Error(data.error); actionError = ''; revision = -1;
  } catch (e) { actionError = e.message; $('error').textContent = actionError; }
}
function render(state) {
  document.body.classList.toggle('working', state.status === 'working');
  $('activity').textContent = state.status === 'working' ? 'Checking the evidence…' : 'Ready when you are';
  $('mode').textContent = state.mode === 'demo' ? 'DEMO · SYNTHETIC DATA' : state.mode === 'live' ? 'JAS-MIN · MCP' : 'WAITING FOR CODEX';
  $('project').textContent = state.project || '';
  const view = state.views.find(v => v.id === state.current), selection = state.selection;
  $('selection').textContent = selection.snap_id != null ? `${selection.instance ? selection.instance+' · ' : ''}${selection.sql_id ? 'SQL '+selection.sql_id+' · ' : ''}Snapshot #${selection.snap_id}` : 'Select a point to focus the conversation';
  $('history').replaceChildren();
  for (const item of [...state.views].reverse()) {
    const board = item.presentation || automaticPresentation(item);
    if (!board.panels.length) continue;
    const b = document.createElement('button'); b.textContent = board.title;
    b.classList.toggle('active', item.id === state.current); b.setAttribute('aria-pressed', String(item.id === state.current));
    b.onclick = () => action('show', { view_id: item.id }); $('history').append(b);
  }
  if (view) {
    $('title').textContent = (view.presentation || automaticPresentation(view)).title;
    $('evidence').textContent = view.presentation ? 'ANALYSIS BOARD' : view.data.evidence_id || 'MEASURED EVIDENCE';
    // Polling/tool activity must not reset chart zoom or keyboard focus.
    if (renderedView !== view.id) {
      renderChart($('chart'), view, point => action('selection', { snap_id: point.snap_id, ...(point.instance ? { instance: point.instance } : {}), ...(point.sql_id ? { sql_id: point.sql_id } : {}) }));
      $('raw').textContent = JSON.stringify(view.presentation || view.data, null, 2); renderedView = view.id;
    }
    for (const p of document.querySelectorAll('.point')) p.classList.toggle('selected', p.dataset.snapId === String(selection.snap_id) && p.dataset.instance === (selection.instance || ''));
  } else {
    $('title').textContent = 'What should we investigate?'; $('chart').textContent = 'Useful comparisons and explanations will appear here as we investigate.'; $('evidence').textContent = ''; $('raw').textContent = '{}'; renderedView = null;
  }
  $('note').textContent = state.note;
}
async function poll() {
  try {
    const response = await fetch('state'); if (!response.ok) throw new Error('Panel unavailable. Reopen it with companion_open.');
    const state = await response.json();
    if (state.revision !== revision) { render(state); revision = state.revision; }
    $('error').textContent = actionError;
  } catch (e) { $('error').textContent = e.message; $('activity').textContent = 'Panel connection lost'; }
  setTimeout(poll, 1000);
}
poll();
