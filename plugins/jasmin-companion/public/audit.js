// The upstream audit is canonical; display history is only a visual cache.
const labels = {
  'Guided audit': 'Audyt krok po kroku', 'chapters discussed': 'omówionych rozdziałów',
  'This project': 'Ten projekt',
  'Current question': 'Teraz sprawdzamy', 'Next step': 'Następny krok',
  'Open questions': 'Pytania bez odpowiedzi', 'Saved explanations': 'Zapisane wyjaśnienia',
  'What this means': 'Co z tego wynika', 'Evidence boundary': 'Czego jeszcze nie wiemy',
  'Evidence': 'Dowody', 'discussed': 'omówiony', 'current': 'teraz', 'pending': 'przed nami',
  'Report requested': 'Raport został zamówiony',
  'We are discussing the evidence. The report starts only when you explicitly request it.': 'Omawiamy wyniki. Raport powstanie dopiero na Twoją wyraźną prośbę.',
  'The server does not support guided audit checkpoints. This panel is an ordinary evidence discussion.': 'Ten serwer nie obsługuje zapisu audytu krok po kroku. Panel działa jako zwykła rozmowa o pomiarach.',
  'Revisit any chapter by asking in the conversation.': 'Do każdego rozdziału możesz wrócić w rozmowie.',
  'Discuss the current question and identify what the evidence can establish.': 'Omówmy bieżące pytanie i ustalmy, co wynika z pomiarów.',
  'Prepare the report from the saved explanations and check it against the evidence.': 'Przygotujmy raport z zapisanych wyjaśnień i sprawdźmy go z pomiarami.',
};
const tr = (language, text) => language === 'pl' ? labels[text] || text : text;
export function auditModel(state) {
  const audit = state.audit;
  if (!audit) return { unsupported: state.audit_support === 'unsupported', chapters: [], briefs: [] };
  const chapters = audit.chapters || [];
  const briefs = (audit.briefs || []).filter(b => b.project_id === state.project)
    .sort((a, b) => (a.updated_at_revision || 0) - (b.updated_at_revision || 0));
  const current = chapters.find(c => c.chapter_id === audit.current_chapter_id);
  const candidates = briefs.filter(b => b.chapter_id === audit.current_chapter_id);
  const latestRevision = Math.max(0, ...candidates.map(b => b.updated_at_revision || 0));
  const latest = candidates.filter(b => (b.updated_at_revision || 0) === latestRevision);
  // Older checkpoints may lack chronology; do not infer it from IDs or array order.
  const brief = latest.length === 1 ? latest[0] : undefined;
  const language = state.language || 'en';
  return { audit, chapters, briefs, current, brief,
    discussed: chapters.filter(c => c.status === 'discussed').length,
    projectDiscussed: chapters.filter(c => c.project_coverage?.find(p => p.project_id === state.project)?.discussed).length,
    title: language === 'pl' ? current?.title_pl || current?.title : current?.title,
    question: brief?.question || (language === 'pl' ? current?.question_pl || current?.question : current?.question),
    nextStep: audit.report_authorized ? tr(language, 'Prepare the report from the saved explanations and check it against the evidence.') : brief?.next_step || tr(language, 'Discuss the current question and identify what the evidence can establish.'),
    openQuestions: candidates.flatMap(b => b.open_questions || []),
  };
}
const node = (tag, text, cls) => {
  const el = document.createElement(tag); if (text !== undefined) el.textContent = text; if (cls) el.className = cls; return el;
};
export function renderAudit(container, state) {
  const m = auditModel(state), language = state.language || 'en', t = text => tr(language, text);
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
    const item = node('li', `${title} · ${t(active ? 'current' : c.status === 'discussed' ? 'discussed' : 'pending')}`, active ? 'active' : c.status);
    if (active) item.setAttribute('aria-current', 'step'); chapters.append(item);
  }
  container.append(chapters, node('p', t('Revisit any chapter by asking in the conversation.'), 'audit-hint'));
  const question = node('div', undefined, 'audit-question');
  question.append(node('span', t('Current question'), 'eyebrow'), node('h3', m.question || m.title)); container.append(question);
  if (m.nextStep) { const next = node('p', undefined, 'audit-next'); next.append(node('strong', `${t('Next step')}: `), document.createTextNode(m.nextStep)); container.append(next); }
  if (m.openQuestions.length) {
    container.append(node('h3', t('Open questions'))); const list = node('ul'); for (const text of m.openQuestions) list.append(node('li', text)); container.append(list);
  }
  if (m.briefs.length) {
    const details = node('details', undefined, 'audit-briefs'); details.append(node('summary', `${t('Saved explanations')} (${m.briefs.length})`));
    for (const brief of m.briefs) {
      const article = node('section', undefined, 'audit-brief'); article.append(node('h3', brief.question), node('p', brief.explanation));
      for (const [label, value] of [['What this means', brief.conclusion], ['Evidence boundary', brief.evidence_boundary]]) {
        const paragraph = node('p'); paragraph.append(node('strong', `${t(label)}: `), document.createTextNode(value || '')); article.append(paragraph);
      }
      if (brief.evidence_refs?.length) article.append(node('p', `${t('Evidence')}: ${brief.evidence_refs.join(', ')}`, 'source'));
      details.append(article);
    }
    container.append(details);
  }
  container.append(node('p', t(m.audit.report_authorized ? 'Report requested' : 'We are discussing the evidence. The report starts only when you explicitly request it.'), 'audit-boundary'));
}
