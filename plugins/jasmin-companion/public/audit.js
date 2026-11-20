// The upstream audit is canonical; display history is only a visual cache.
const labels = {
  'Guided audit': 'Audyt krok po kroku', 'chapters discussed': 'omówionych rozdziałów',
  'This project': 'Ten projekt',
  'Current question': 'Teraz sprawdzamy', 'Next step': 'Następny krok',
  'Open questions': 'Pytania bez odpowiedzi', 'Saved explanations': 'Zapisane wyjaśnienia',
  'Working hypotheses': 'Hipotezy robocze', 'Gradient signal': 'Sygnał gradientowy',
  'Selected chapter': 'Wybrany etap', 'Chapter views': 'Widoki etapu',
  'Browse chapter': 'Obejrzyj etap', 'Discuss here': 'Rozmawiaj tutaj',
  'Saved audits': 'Zapisane audyty', 'Refresh list': 'Odśwież listę', 'Resume': 'Wznów',
  'No saved audits in this workspace.': 'Brak zapisanych audytów w tym katalogu roboczym.',
  'Combined RAC audit': 'Wspólny audyt RAC', 'Discussion topics': 'Tematy audytu',
  'Transaction locks (TX)': 'Blokady transakcyjne (TX)', 'RAC CPU scenario': 'Scenariusz CPU RAC',
  'TX lock audit': 'Audyt blokad TX', 'RAC CPU audit': 'Audyt CPU klastra RAC',
  'Observed cluster peak': 'Zmierzony szczyt klastra', 'Modeled cluster peak': 'Modelowany szczyt klastra',
  'Modeled work reduction': 'Modelowana redukcja pracy', 'CPU-hours': 'CPU-godz.', 'active CPU': 'aktywnego CPU',
  'Paired intervals': 'Sparowane przedziały',
  'Snapshot saved': 'Zapisano snapshot', 'Last private note': 'Ostatnia prywatna notatka',
  'Checkpoint file on the MCP server': 'Plik checkpointu na serwerze MCP',
  'This audit is in server memory. Save a portable checkpoint before restarting the server.': 'Ten audyt jest w pamięci serwera. Przed restartem zapisz przenośny checkpoint.',
  'Last saved topic': 'Ostatnio zapisany temat', 'View saved topic': 'Obejrzyj zapisany temat',
  'Snapshot revision': 'Wersja snapshotu', 'An earlier snapshot opens a separate branch.': 'Starsza wersja otwiera osobną gałąź audytu.',
  'The latest snapshot is unavailable. Select an earlier revision to recover.': 'Najnowszy snapshot jest niedostępny. Aby wznowić pracę, wybierz starszą wersję.',
  'Last correction': 'Ostatnia korekta', 'Intervals': 'Przedziały',
  'Client preview': 'Podgląd raportu klienta', 'Publish client ZIP': 'Wydaj ZIP dla klienta',
  'Client ZIP': 'Paczka ZIP dla klienta', 'Preview file': 'Plik podglądu',
  'Client report chapters': 'Rozdziały raportu klienta', 'Prepare preview': 'Przygotuj podgląd',
  'Move up': 'Przenieś wyżej', 'Move down': 'Przenieś niżej',
  'A source brief changed. Review the affected chapters in the conversation.': 'Zmieniono omówienie źródłowe. Sprawdź powiązane rozdziały w rozmowie.',
  'Open client preview': 'Otwórz podgląd klienta', 'Download ZIP': 'Pobierz ZIP',
  'This core provides file paths on the server. Update JAS-MIN to open the files in this panel.': 'Ten serwer podaje ścieżki do plików. Zaktualizuj JAS-MIN, aby otwierać je bezpośrednio z panelu.',
  'Owner': 'Odpowiedzialny', 'Acceptance criterion': 'Kryterium odbioru',
  'Action': 'Działanie', 'Measured TX wait': 'Zmierzony czas oczekiwania TX',
  'Could not load regression models.': 'Nie udało się wczytać modeli regresji.',
  'Gradient evidence is unavailable on this server.': 'Ten serwer nie udostępnia danych gradientowych.',
  'The audit has no saved regression overview. Update JAS-MIN to preview models.': 'Audyt nie ma zapisanego zestawienia regresji. Zaktualizuj JAS-MIN, aby wyświetlić modele.',
  'Retry': 'Ponów',
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
export function topicBriefs(audit) {
  if (audit?.pilot_kind !== 'combined') return audit?.briefs || [];
  return audit.briefs.filter(brief => (brief.topic || audit.brief_topics?.[brief.brief_id]) === audit.active_topic);
}
export function auditModel(state) {
  const audit = state.audit;
  if (!audit) return { unsupported: state.audit_support === 'unsupported', chapters: [], briefs: [] };
  const chapters = (audit.chapters || []).map(chapterLabels);
  const chapterId = state.viewed_chapter_id ?? audit.current_chapter_id;
  const current = chapters.find(c => c.chapter_id === chapterId);
  const briefs = topicBriefs(audit).filter(b => b.project_id === state.project && b.chapter_id === chapterId)
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
  return { audit, chapters, briefs, current, brief, hypotheses, language, gradientOverview: displayed?.opening_gradient,
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
function commandButton(label, action) {
  const button = node('button', label);
  button.type = 'button';
  button.onclick = action;
  return button;
}
function renderClientReport(container, state, t, action) {
  const audit = state.audit;
  const plan = audit?.client_report_plan;
  if (!state.client_report_available || !plan?.chapters?.length) return;
  const section = node('section', undefined, 'audit-report');
  section.append(node('h3', t('Client report chapters')));
  const preview = state.client_preview;
  const url = preview && state.client_assets_available ? 'client/report/' + encodeURIComponent(preview.preview_id) + '/' : null;
  const list = node('ol');
  for (const [index, chapter] of plan.chapters.entries()) {
    const row = node('li');
    if (url) {
      const link = node('a', chapter.title);
      link.href = url + 'raport.html#' + encodeURIComponent(chapter.id);
      link.target = '_blank'; link.rel = 'noopener noreferrer';
      row.append(link);
    } else row.append(node('span', chapter.title));
    const moves = node('span', undefined, 'audit-report-order');
    for (const [direction, label] of [[-1, 'Move up'], [1, 'Move down']]) {
      const button = commandButton(direction < 0 ? '↑' : '↓', () => action('move-report-chapter', {
        chapter_id: chapter.id, direction, expected_revision: audit.snapshot.revision,
      }));
      button.setAttribute('aria-label', t(label) + ' · ' + chapter.title);
      button.title = t(label);
      button.disabled = index + direction < 0 || index + direction >= plan.chapters.length || !!audit.client_report_plan_stale;
      moves.append(button);
    }
    row.append(moves); list.append(row);
  }
  section.append(list);
  if (audit.client_report_plan_stale) section.append(node('p', t('A source brief changed. Review the affected chapters in the conversation.'), 'audit-boundary'));
  else if (url) {
    const link = node('a', t('Open client preview'));
    link.href = url + 'raport.html'; link.target = '_blank'; link.rel = 'noopener noreferrer'; section.append(link);
    if (state.client_release?.preview_id === preview.preview_id) {
      const zip = node('a', t('Download ZIP')); zip.href = url + 'report.zip'; zip.download = 'jas-min-report.zip';
      section.append(document.createTextNode(' · '), zip);
    }
  } else if (preview) section.append(node('p', t('Preview file') + ' · ' + preview.index_path, 'source'));
  else section.append(commandButton(t('Prepare preview'), () => action('preview-client-report', {})));
  container.append(section);
}
const auditLabels = { general: 'Guided audit', tx: 'TX lock audit', cpu: 'RAC CPU audit', combined: 'Combined RAC audit' };
const topicLabels = { tx: 'Transaction locks (TX)', cpu: 'RAC CPU scenario' };
function renderSavedAudits(container, state, t, action) {
  const group = node('details', undefined, 'audit-saved');
  group.open = !state.audit;
  group.append(node('summary', t('Saved audits')));
  group.append(commandButton(t('Refresh list'), () => action('saved-audits', {})));
  if (state.saved_audits_error) group.append(node('p', state.saved_audits_error));
  else if (!state.saved_audits?.length) group.append(node('p', t('No saved audits in this workspace.')));
  for (const entry of state.saved_audits || []) {
    const row = node('div', undefined, 'audit-saved-row');
    const label = entry.unavailable ? null : t(auditLabels[entry.pilot_kind || 'tx'] || auditLabels.general);
    row.append(node('span', [label, entry.audit_id, entry.active_topic && t(topicLabels[entry.active_topic]), entry.current_chapter_id, entry.updated_at].filter(Boolean).join(' · ')));
    const select = node('select');
    select.setAttribute('aria-label', t('This project'));
    if (entry.unavailable) row.append(node('span', t('The latest snapshot is unavailable. Select an earlier revision to recover.')));
    const projects = entry.project_ids || [];
    for (const project of projects) {
      const option = node('option', project); option.value = project; select.append(option);
    }
    if (entry.focus_project_id) select.value = entry.focus_project_id;
    if (projects.length > 1 && entry.pilot_kind !== 'combined') row.append(select);
    const revision = node('input');
    revision.type = 'number'; revision.min = '1'; revision.max = String(entry.revision); revision.value = revision.max;
    revision.setAttribute('aria-label', t('Snapshot revision'));
    if (entry.revision > 1) {
      const label = node('label', t('Snapshot revision') + ' ');
      label.title = t('An earlier snapshot opens a separate branch.'); label.append(revision); row.append(label);
    }
    row.append(commandButton(t('Resume'), () => {
      if (!revision.reportValidity()) return;
      const requested = revision.valueAsNumber;
      return action('resume-saved', { audit_id: entry.audit_id, ...(select.value && entry.pilot_kind !== 'combined' ? { resume_project_id: select.value } : {}),
        ...(requested > 0 && requested < entry.revision ? { revision: requested } : {}) });
    }));
    group.append(row);
  }
  container.append(group);
}
function renderSavedAudit(container, state, t, browse, action) {
  const audit = state.audit;
  if (!audit?.audit_id) return;
  const kind = audit.pilot_kind || audit.snapshot?.pilot_kind || 'tx';
  if (kind === 'combined') {
    const heading = t('Discussion topics');
    container.append(node('h2', heading));
    const topics = node('div', undefined, 'audit-topics');
    topics.setAttribute('role', 'group');
    topics.setAttribute('aria-label', heading);
    for (const [topic, label] of Object.entries(topicLabels)) {
      const selected = topic === audit.active_topic;
      const button = commandButton(t(label), () => {
        if (!selected) return action('audit-topic', { topic, expected_revision: audit.snapshot.revision });
      });
      button.dataset.auditTopic = topic;
      button.setAttribute('aria-pressed', String(selected));
      topics.append(button);
    }
    container.append(topics);
  }
  if (audit.snapshot?.persisted) {
    const receipt = node('p', [kind !== 'general' && t(auditLabels[kind]), t('Snapshot saved'), audit.snapshot.revision, audit.snapshot.updated_at].filter(Boolean).join(' · '), 'audit-progress');
    receipt.setAttribute('role', 'status'); container.append(receipt);
    if (audit.snapshot.path) {
      const path = node('details', undefined, 'audit-checkpoint-path');
      path.append(node('summary', t('Checkpoint file on the MCP server')), node('code', audit.snapshot.path));
      container.append(path);
    }
  }
  const context = audit.resume_context || {};
  const focus = audit.briefs.find(b => b.brief_id === context.focus_brief_id && b.project_id === state.project);
  const correction = context.last_correction;
  if (correction?.conclusion && audit.briefs.some(b => b.brief_id === correction.brief_id && b.project_id === state.project)) {
    const p = node('p', undefined, 'audit-next');
    p.append(node('strong', t('Last correction') + ' · '), document.createTextNode(correction.conclusion)); container.append(p);
  }
  if (context.last_private_note) {
    const note = node('p', undefined, 'audit-next');
    note.append(node('strong', t('Last private note') + ' · '), document.createTextNode(context.last_private_note));
    container.append(note);
  }
  if (focus) {
    const group = node('details', undefined, 'audit-briefs');
    group.append(node('summary', t('Last saved topic') + ' · ' + focus.question));
    for (const text of [focus.conclusion, focus.evidence_boundary, ...(focus.open_questions || []), focus.next_step]) {
      if (text) group.append(node('p', text));
    }
    group.append(commandButton(t('View saved topic'), () => browse(focus.chapter_id, focus.visual ? focus.brief_id : undefined)));
    container.append(group);
  }
  const preview = state[`${kind}_preview`], release = state[`${kind}_release`];
  if (!preview) return;
  const group = node('section', undefined, 'audit-brief');
  group.append(node('h3', t('Client preview')));
  if (kind === 'combined') {
    group.append(node('h4', preview.title));
    for (const topic of ['tx', 'cpu']) {
      const details = node('details');
      details.append(node('summary', t(topicLabels[topic])));
      appendPreviewTopic(details, preview[topic], topic, t, state.language);
      group.append(details);
    }
  } else {
    appendPreviewTopic(group, preview, kind, t, state.language);
  }
  if (state[`${kind}_assets_available`]) {
    const url = (kind === 'tx' ? 'client/' : 'client/' + kind + '/') + encodeURIComponent(preview.preview_id) + '/';
    const link = node('a', t('Open client preview'));
    link.id = kind + '-open-preview'; link.href = url + 'index.html'; link.target = '_blank'; link.rel = 'noopener noreferrer';
    group.append(link);
    if (release) {
      const download = node('a', t('Download ZIP'));
      download.id = kind + '-download-zip'; download.href = url + 'report.zip'; download.download = 'jas-min-' + kind + '.zip';
      group.append(document.createTextNode(' · '), download);
    }
  } else {
    group.append(node('p', t('Preview file') + ' · ' + preview.index_path, 'source'),
      node('p', t('This core provides file paths on the server. Update JAS-MIN to open the files in this panel.'), 'audit-hint'));
  }
  if (release) group.append(node('p', t('Client ZIP') + ' · ' + release.zip_path, 'audit-next'));
  else {
    const label = t('Publish client ZIP');
    group.append(commandButton(label, () => action('publish-' + kind, { preview_id: preview.preview_id, user_request: label })));
  }
  container.append(group);
}
function appendPreviewTopic(group, preview, kind, t, language) {
  const report = preview.public_report;
  if (report) {
    group.append(node('h4', report.title), node('p', report.conclusion));
    for (const [label, value] of [['Evidence boundary', report.boundary], ['Action', report.action], ['Owner', report.owner], ['Acceptance criterion', report.success_criterion]]) {
      if (value) group.append(node('p', t(label) + ' · ' + value));
    }
  }
  const format = new Intl.NumberFormat(language, { maximumFractionDigits: 2 });
  if (kind === 'cpu' && preview.cpu_model) {
    const model = preview.cpu_model;
    for (const [key, label] of [['observed_peak', 'Observed cluster peak'], ['scenario_peak', 'Modeled cluster peak']]) {
      const peak = model[key];
      if (Number.isFinite(peak?.active_cpu))
        group.append(node('p', [t(label), format.format(peak.active_cpu) + ' ' + t('active CPU'), peak.begin, peak.end].join(' · ')));
    }
    if (Number.isFinite(model.saved_cpu_hours))
      group.append(node('p', [t('Modeled work reduction'), format.format(model.saved_cpu_hours) + ' ' + t('CPU-hours'),
        t('Paired intervals') + ' ' + model.paired_intervals].join(' · ')));
  } else if (kind === 'tx') {
    for (const row of preview.measurements || []) {
      group.append(node('p', [t('Measured TX wait'), row.instance_label, format.format(row.total_wait_s) + ' s',
        row.begin, row.end, t('Intervals') + ' ' + row.observed_intervals].join(' · ')));
    }
  }
}
export function renderAudit(container, gradientDiscussion, state, browse, showView, action) {
  const m = auditModel(state), language = state.language || 'en', t = text => tr(language, text);
  const focused = container.contains(document.activeElement) ? document.activeElement.dataset : {};
  container.replaceChildren();
  gradientDiscussion.replaceChildren();
  gradientDiscussion.hidden = m.current?.chapter_id !== 'performance_profile';
  container.hidden = !m.audit && !m.unsupported && !state.saved_audits_available;
  if (state.saved_audits_available) renderSavedAudits(container, state, t, action);
  renderSavedAudit(container, state, t, browse, action);
  renderClientReport(container, state, t, action);
  if (m.unsupported) { container.append(node('p', t('The server does not support guided audit checkpoints. This panel is an ordinary evidence discussion.'), 'audit-boundary')); return; }
  if (!m.audit) return;
  if (!m.audit.snapshot?.persisted) container.append(node('p', t('This audit is in server memory. Save a portable checkpoint before restarting the server.'), 'audit-boundary'));
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
    if (m.audit.audit_id) button.title = t('Browse chapter') + ' · ' + title;
    button.setAttribute('aria-pressed', String(c.chapter_id === m.current?.chapter_id));
    if (active) button.setAttribute('aria-current', 'step');
    button.onclick = () => browse(c.chapter_id);
    item.append(button); chapters.append(item);
  }
  container.append(chapters);
  if (m.audit.audit_id && state.viewed_chapter_id) {
    const shown = state.views?.find(view => view.id === state.current);
    const currentBrief = m.brief?.brief_id;
    const focus = shown?.brief_id && !state.chapter_views?.find(v => v.view_id === shown.id)?.superseded ? shown.brief_id : currentBrief;
    const discuss = commandButton(t('Discuss here'), () => action('advance', {
      chapter_id: m.current.chapter_id, expected_revision: m.audit.snapshot.revision,
      ...(focus ? { focus_brief_id: focus } : {}),
    }));
    discuss.className = 'audit-discuss'; container.append(discuss);
  }
  if (m.current?.chapter_id === 'performance_profile' && state.gradient_error) {
    const error = node('p', undefined, 'audit-next');
    error.setAttribute('role', 'status');
    error.append(document.createTextNode(t('Could not load regression models.') + ' ' + t(state.gradient_error) + ' '));
    const retry = node('button', t('Retry')); retry.type = 'button'; retry.onclick = () => browse('performance_profile');
    error.append(retry); container.append(error);
  }
  const question = node('div', undefined, 'audit-question');
  const questionContainer = m.gradientOverview && !gradientDiscussion.hidden ? gradientDiscussion : container;
  question.append(node('span', m.current?.chapter_id === m.audit.current_chapter_id ? t('Current question') : t('Selected chapter'), 'eyebrow'), node('h3', m.question || m.title)); questionContainer.append(question);
  if (m.nextStep) { const next = node('p', undefined, 'audit-next'); next.append(node('strong', `${t('Next step')}: `), document.createTextNode(m.nextStep)); questionContainer.append(next); }
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
  const discussion = gradientDiscussion.hidden ? container : gradientDiscussion;
  renderHypotheses(discussion, m, t);
  if (m.openQuestions.length) {
    discussion.append(node('h3', t('Open questions'))); const list = node('ul'); for (const text of m.openQuestions) list.append(node('li', text)); discussion.append(list);
  }
  if (m.briefs.length) {
    const details = node('details', undefined, 'audit-briefs');
    details.open = !!state.viewed_chapter_id && m.current?.chapter_id !== 'performance_profile';
    details.append(node('summary', `${t('Saved explanations')} (${m.briefs.length})`));
    for (const brief of m.briefs) {
      const article = node('section', undefined, 'audit-brief'); article.append(node('h3', brief.question), node('p', brief.explanation));
      for (const [label, value] of [['What this means', brief.conclusion], ['Evidence boundary', brief.evidence_boundary]]) {
        const paragraph = node('p'); paragraph.append(node('strong', `${t(label)}: `), document.createTextNode(value || '')); article.append(paragraph);
      }
      if (brief.evidence_refs?.length) article.append(node('p', `${t('Evidence')}: ${brief.evidence_refs.join(', ')}`, 'source'));
      details.append(article);
    }
    discussion.append(details);
  } else if (state.viewed_chapter_id) discussion.append(node('p', t('No saved explanation for this chapter yet.'), 'audit-hint'));
  discussion.append(node('p', t(m.audit.report_authorized ? 'Report requested' : 'We are discussing the evidence. The report starts only when you explicitly request it.'), 'audit-boundary'));
  if (focused.auditTopic || focused.chapterId || focused.briefId || focused.viewId) {
    const key = focused.auditTopic ? 'auditTopic' : focused.chapterId ? 'chapterId' : focused.viewId ? 'viewId' : 'briefId';
    Array.from(container.querySelectorAll('button')).find(b => b.dataset[key] === focused[key])?.focus({ preventScroll: true });
  }
}
