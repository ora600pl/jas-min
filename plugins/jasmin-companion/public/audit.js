// The upstream audit is canonical; display history is only a visual cache.
const labels = {
  'Guided audit': 'Audyt krok po kroku', 'chapters discussed': 'omówionych rozdziałów',
  'This project': 'Ten projekt',
  'Current question': 'Teraz sprawdzamy', 'Next step': 'Następny krok',
  'Open questions': 'Pytania bez odpowiedzi', 'Saved explanations': 'Zapisane wyjaśnienia',
  'Working hypotheses': 'Hipotezy robocze', 'Gradient signal': 'Sygnał gradientowy',
  'Selected chapter': 'Wybrany etap', 'Chapter views': 'Widoki etapu',
  'Earlier revision': 'Poprzednia wersja',
  'No saved explanation for this chapter yet.': 'Ten etap nie ma jeszcze zapisanego wyjaśnienia.',
  'Measured workload': 'Zmierzony profil obciążenia', 'Comparison window': 'Okno porównania',
  'Distinguishing measurement': 'Pomiar rozstrzygający', 'Chapter checks': 'Pomiary w rozdziałach',
  'Revisit the performance profile after the scope change.': 'Po zmianie zakresu wróć do profilu wydajności i sprawdź hipotezy.',
  'confirmed': 'potwierdzona', 'rejected': 'odrzucona', 'unresolved': 'nierozstrzygnięta',
  'What this means': 'Co z tego wynika', 'Evidence boundary': 'Czego jeszcze nie wiemy',
  'Evidence': 'Dowody', 'discussed': 'omówiony', 'current': 'teraz', 'pending': 'przed nami',
  'Report requested': 'Raport został zamówiony',
  'We are discussing the evidence. The report starts only when you explicitly request it.': 'Omawiamy wyniki. Raport powstanie dopiero na Twoją wyraźną prośbę.',
  'The server does not support guided audit checkpoints. This panel is an ordinary evidence discussion.': 'Ten serwer nie obsługuje zapisu audytu krok po kroku. Panel działa jako zwykła rozmowa o pomiarach.',
  'Discuss the current question and identify what the evidence can establish.': 'Omówmy bieżące pytanie i ustalmy, co wynika z pomiarów.',
  'Prepare the report from the saved explanations and check it against the evidence.': 'Przygotujmy raport z zapisanych wyjaśnień i sprawdźmy go z pomiarami.',
};
const tr = (language, text) => language === 'pl' ? labels[text] || text : text;
// Older running cores keep their chapter IDs and their original discussion records.
function chapterLabels(chapter) {
  if (chapter.chapter_id === 'performance_profile' && chapter.title === 'Performance profile') {
    return { ...chapter, title: 'Gradients, anomalies and workload', title_pl: 'Gradienty, anomalie i obciążenie',
      question: 'Which DB Time and DB CPU gradient signals and dated anomalies or clusters match the measured workload? Which two or three hypotheses should we test?',
      question_pl: 'Które sygnały gradientowe DB Time i DB CPU oraz okna anomalii pokrywają się ze zmierzonym obciążeniem? Jakie dwie lub trzy hipotezy sprawdzimy?' };
  }
  if (chapter.chapter_id === 'gradients_anomalies' && chapter.title === 'Models and unusual periods') {
    return { ...chapter, title: 'Hypothesis review', title_pl: 'Weryfikacja hipotez' };
  }
  return chapter;
}
export function auditModel(state) {
  const audit = state.audit;
  if (!audit) return { unsupported: state.audit_support === 'unsupported', chapters: [], briefs: [] };
  const chapters = (audit.chapters || []).map(chapterLabels);
  const chapterId = state.viewed_chapter_id ?? audit.current_chapter_id;
  const current = chapters.find(c => c.chapter_id === chapterId);
  const briefs = (audit.briefs || []).filter(b => b.project_id === state.project && b.chapter_id === chapterId)
    .sort((a, b) => (a.updated_at_revision || 0) - (b.updated_at_revision || 0));
  const latestRevision = Math.max(0, ...briefs.map(b => b.updated_at_revision || 0));
  const latest = briefs.filter(b => (b.updated_at_revision || 0) === latestRevision);
  // Older checkpoints may lack chronology; do not infer it from IDs or array order.
  const displayed = state.views?.find(view => view.id === state.current);
  const selectedBrief = displayed?.brief_id && briefs.find(b => b.brief_id === displayed.brief_id &&
    (b.revision ?? b.updated_at_revision) === displayed.brief_revision);
  const brief = selectedBrief || (latest.length === 1 ? latest[0] : undefined);
  const language = state.language || 'en';
  const hypotheses = audit.hypotheses?.find(h => h.project_id === state.project);
  return { audit, chapters, briefs, current, brief, hypotheses, language,
    discussed: chapters.filter(c => c.status === 'discussed').length,
    projectDiscussed: chapters.filter(c => c.project_coverage?.find(p => p.project_id === state.project)?.discussed).length,
    title: language === 'pl' ? current?.title_pl || current?.title : current?.title,
    question: brief?.question || (language === 'pl' ? current?.question_pl || current?.question : current?.question),
    nextStep: brief?.next_step || tr(language, audit.report_authorized ? 'Prepare the report from the saved explanations and check it against the evidence.' : 'Discuss the current question and identify what the evidence can establish.'),
    openQuestions: selectedBrief ? selectedBrief.open_questions || [] : briefs.flatMap(b => b.open_questions || []),
  };
}
const node = (tag, text, cls) => {
  const el = document.createElement(tag); if (text !== undefined) el.textContent = text; if (cls) el.className = cls; return el;
};
function renderHypotheses(container, model, t) {
  const group = model.hypotheses;
  if (group?.opening_needs_scope_review) {
    container.append(node('p', t('Revisit the performance profile after the scope change.'), 'audit-next'));
    return;
  }
  if (!group?.opening_hypotheses?.length && !group?.hypothesis_gap) return;
  container.append(node('h3', t('Working hypotheses')));
  if (group.hypothesis_gap) container.append(node('p', group.hypothesis_gap));
  for (const h of group.opening_hypotheses || []) {
    const details = node('details', undefined, 'audit-briefs');
    const outcome = group.hypothesis_outcomes?.find(row => row.id === h.id);
    details.append(node('summary', [h.id, h.statement, outcome && t(outcome.status)].filter(Boolean).join(' · ')));
    const body = node('div', undefined, 'audit-brief');
    for (const [label, value] of [['Gradient signal', h.gradient_signal], ['Measured workload', h.workload_profile],
      ['Comparison window', h.comparison_window], ['Distinguishing measurement', h.test]]) {
      const p = node('p'); p.append(node('strong', t(label) + ' '), document.createTextNode(value)); body.append(p);
    }
    const checks = group.hypothesis_checks?.filter(row => row.id === h.id) || [];
    if (checks.length) {
      body.append(node('strong', t('Chapter checks')));
      const list = node('ul');
      for (const check of checks) {
        const chapter = model.chapters.find(c => c.chapter_id === check.chapter_id);
        const title = chapter && (model.language === 'pl' ? chapter.title_pl || chapter.title : chapter.title);
        list.append(node('li', [title, check.reason].filter(Boolean).join(' · ')));
      }
      body.append(list);
    }
    if (outcome) body.append(node('p', outcome.reason));
    details.append(body); container.append(details);
  }
}
export function renderAudit(container, state, browse, showView) {
  const m = auditModel(state), language = state.language || 'en', t = text => tr(language, text);
  const focused = container.contains(document.activeElement) ? document.activeElement.dataset : {};
  container.replaceChildren();
  container.hidden = !m.audit && !m.unsupported;
  if (m.unsupported) { container.append(node('p', t('The server does not support guided audit checkpoints. This panel is an ordinary evidence discussion.'), 'audit-boundary')); return; }
  if (!m.audit) return;
  const heading = node('div', undefined, 'audit-heading');
  heading.append(node('h2', t('Guided audit')), node('span', `${m.discussed}/${m.chapters.length} ${t('chapters discussed')}`, 'audit-progress'));
  container.append(heading);
  if (m.audit.project_ids?.length > 1) container.append(node('p', `${t('This project')}: ${m.projectDiscussed}/${m.chapters.length} ${t('chapters discussed')}`, 'audit-progress'));
  const progress = document.createElement('progress'); progress.max = Math.max(1, m.chapters.length); progress.value = m.discussed; progress.setAttribute('aria-label', t('chapters discussed')); container.append(progress);
  const chapters = node('ol', undefined, 'audit-chapters');
  for (const c of m.chapters) {
    const active = c.chapter_id === m.audit.current_chapter_id;
    const title = language === 'pl' ? c.title_pl || c.title : c.title;
    const discussed = c.project_coverage?.find(row => row.project_id === state.project)?.discussed ?? (c.status === 'discussed');
    const item = node('li');
    const button = node('button', `${title} · ${t(active ? 'current' : discussed ? 'discussed' : 'pending')}`);
    button.type = 'button'; button.dataset.chapterId = c.chapter_id;
    button.setAttribute('aria-pressed', String(c.chapter_id === m.current?.chapter_id));
    if (active) button.setAttribute('aria-current', 'step');
    button.onclick = () => browse(c.chapter_id);
    item.append(button); chapters.append(item);
  }
  container.append(chapters);
  const question = node('div', undefined, 'audit-question');
  question.append(node('span', m.current?.chapter_id === m.audit.current_chapter_id ? t('Current question') : t('Selected chapter'), 'eyebrow'), node('h3', m.question || m.title)); container.append(question);
  if (m.nextStep) { const next = node('p', undefined, 'audit-next'); next.append(node('strong', `${t('Next step')}: `), document.createTextNode(m.nextStep)); container.append(next); }
  if (state.chapter_views?.length) {
    const history = node('details', undefined, 'audit-chapter-views'); history.open = true;
    history.append(node('summary', `${t('Chapter views')} (${state.chapter_views.length})`));
    const list = node('ul');
    for (const view of state.chapter_views) {
      const item = node('li');
      const b = node('button', view.superseded ? `${view.title} · ${t('Earlier revision')}` : view.title, 'audit-show-view');
      b.type = 'button'; b.setAttribute('aria-pressed', String(view.view_id != null && view.view_id === state.current));
      if (view.brief_id) b.dataset.briefId = view.brief_id;
      if (view.view_id) { b.dataset.viewId = view.view_id; b.onclick = () => showView(view.view_id); }
      else b.onclick = () => browse(m.current.chapter_id, view.brief_id);
      item.append(b); list.append(item);
    }
    history.append(list); container.append(history);
  }
  renderHypotheses(container, m, t);
  if (m.openQuestions.length) {
    container.append(node('h3', t('Open questions'))); const list = node('ul'); for (const text of m.openQuestions) list.append(node('li', text)); container.append(list);
  }
  if (m.briefs.length) {
    const details = node('details', undefined, 'audit-briefs'); details.open = !!state.viewed_chapter_id;
    details.append(node('summary', `${t('Saved explanations')} (${m.briefs.length})`));
    for (const brief of m.briefs) {
      const article = node('section', undefined, 'audit-brief'); article.append(node('h3', brief.question), node('p', brief.explanation));
      for (const [label, value] of [['What this means', brief.conclusion], ['Evidence boundary', brief.evidence_boundary]]) {
        const paragraph = node('p'); paragraph.append(node('strong', `${t(label)}: `), document.createTextNode(value || '')); article.append(paragraph);
      }
      if (brief.evidence_refs?.length) article.append(node('p', `${t('Evidence')}: ${brief.evidence_refs.join(', ')}`, 'source'));
      details.append(article);
    }
    container.append(details);
  } else if (state.viewed_chapter_id) container.append(node('p', t('No saved explanation for this chapter yet.'), 'audit-hint'));
  container.append(node('p', t(m.audit.report_authorized ? 'Report requested' : 'We are discussing the evidence. The report starts only when you explicitly request it.'), 'audit-boundary'));
  if (focused.chapterId || focused.briefId || focused.viewId) {
    const key = focused.chapterId ? 'chapterId' : focused.viewId ? 'viewId' : 'briefId';
    Array.from(container.querySelectorAll('button')).find(b => b.dataset[key] === focused[key])?.focus({ preventScroll: true });
  }
}
