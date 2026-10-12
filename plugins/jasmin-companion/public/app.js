import { renderChart, automaticPresentation, localizedView, translate } from './charts.js';
const $ = id => document.getElementById(id);
let revision = -1, renderedView = null, actionError = '', language = 'en';
const t = text => translate(language, text);
async function action(route, args) {
  try {
    const r = await fetch(route, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Companion-Client': 'panel' }, body: JSON.stringify(args) });
    const data = await r.json(); if (!r.ok) throw new Error(data.error); actionError = ''; revision = -1;
  } catch (e) { actionError = e.message; $('error').textContent = actionError ? `${t('The action could not be completed.')} ${actionError}` : ''; }
}
function render(state) {
  language = state.language || 'en';
  document.documentElement.lang = language;
  for (const element of document.querySelectorAll('[data-i18n]')) element.textContent = t(element.dataset.i18n);
  document.querySelector('.portrait img').alt = t('Hooded JAS-MIN guide');
  $('history').setAttribute('aria-label', t('Evidence history'));
  document.body.classList.toggle('working', state.status === 'working');
  $('activity').textContent = state.status === 'working' ? t('Checking the evidence…') : t('Ready when you are');
  $('mode').textContent = state.mode === 'demo' ? t('DEMO · SYNTHETIC DATA') : state.mode === 'live' ? 'JAS-MIN · MCP' : t('WAITING FOR CODEX');
  $('project').textContent = state.project || '';
  const sourceView = state.views.find(v => v.id === state.current), view = sourceView && localizedView(sourceView, language), selection = state.selection;
  $('selection').textContent = selection.snap_id != null ? `${selection.instance ? selection.instance+' · ' : ''}${selection.sql_id ? 'SQL '+selection.sql_id+' · ' : ''}Snapshot #${selection.snap_id}` : t('Select a point to focus the conversation');
  $('history').replaceChildren();
  for (const item of [...state.views].reverse()) {
    const board = localizedView(item, language).presentation || automaticPresentation(item, language);
    if (!board.panels.length) continue;
    const b = document.createElement('button'); b.textContent = board.title;
    b.classList.toggle('active', item.id === state.current); b.setAttribute('aria-pressed', String(item.id === state.current));
    b.onclick = () => action('show', { view_id: item.id }); $('history').append(b);
  }
  if (view) {
    $('title').textContent = (view.presentation || automaticPresentation(view, language)).title;
    $('evidence').textContent = view.presentation ? t('ANALYSIS BOARD') : view.data.evidence_id || t('MEASURED EVIDENCE');
    // Polling/tool activity must not reset chart zoom or keyboard focus.
    const renderKey = `${view.id}:${language}:${view.copyRevision || 0}`;
    if (renderedView !== renderKey) {
      renderChart($('chart'), view, point => action('selection', { snap_id: point.snap_id, ...(point.instance ? { instance: point.instance } : {}), ...(point.sql_id ? { sql_id: point.sql_id } : {}) }), language);
      $('raw').textContent = JSON.stringify(view.presentation || view.data, null, 2); renderedView = renderKey;
    }
    for (const p of document.querySelectorAll('.point')) p.classList.toggle('selected', p.dataset.snapId === String(selection.snap_id) && p.dataset.instance === (selection.instance || ''));
  } else {
    $('title').textContent = t('What should we investigate?'); $('chart').textContent = t('Useful comparisons and explanations will appear here as we investigate.'); $('evidence').textContent = ''; $('raw').textContent = '{}'; renderedView = null;
  }
  $('note').textContent = view?.note ?? state.note;
}
async function poll() {
  try {
    const response = await fetch('state'); if (!response.ok) throw new Error(t('Panel unavailable. Reopen it with companion_open.'));
    const state = await response.json();
    if (state.revision !== revision) { render(state); revision = state.revision; }
    $('error').textContent = actionError ? `${t('The action could not be completed.')} ${actionError}` : '';
  } catch (e) { $('error').textContent = e.message; $('activity').textContent = t('Panel connection lost'); }
  setTimeout(poll, 1000);
}
poll();
