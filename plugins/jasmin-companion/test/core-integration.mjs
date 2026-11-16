// Explicit integration harness for the core server's isolated synthetic fixture.
// Invoked by the Rust ignored integration test, never by the ordinary Node suite.
import assert from 'node:assert/strict';
import { Companion } from '../companion.mjs';

const endpoint = process.env.JASMIN_AUDIT_TEST_ENDPOINT;
assert.ok(endpoint, 'JASMIN_AUDIT_TEST_ENDPOINT must identify the dedicated synthetic test server.');
const companion = new Companion();
const restored = new Companion();
const checks = {};

try {
  await companion.open({ mode: 'live', endpoint, language: 'pl' });
  const projects = await companion.projects();
  assert.equal(projects.projects.length, 1, 'Use an isolated one-project fixture.');
  const project_id = projects.projects[0].project_id;
  const selected = await companion.select(project_id);
  const analysis_id = selected.analysis_id;
  assert.equal(selected.audit_support, 'available');
  assert.equal(selected.audit.audit_started, true);
  assert.equal(selected.audit.report_authorized, false);
  assert.equal(selected.audit.current_chapter_id, 'scope');
  assert.ok(selected.audit.chapters.some(c => c.chapter_id === 'performance_profile'));
  const core = (name, args = {}) => companion.client.call(name, { analysis_id, ...args });

  const evidence = await companion.query('get_database_load_summary', { top_n: 3 });
  assert.equal(evidence.project_id, project_id);
  assert.equal(evidence.analysis_id, analysis_id);
  assert.match(evidence.evidence_id, /^E-/);
  assert.equal(typeof evidence.result.snapshot_count, 'number');
  assert.ok(evidence.result.total_db_time_s === null || Number.isFinite(evidence.result.total_db_time_s));
  const refs = [evidence.evidence_id];
  const dbTimeObservation = evidence.result.total_db_time_s === null ? 'łączny DB Time jest niedostępny' : `łączny DB Time wynosi ${evidence.result.total_db_time_s} s`;
  const observed = `Próbka testowa obejmuje ${evidence.result.snapshot_count} snapshotów; ${dbTimeObservation}.`;
  const originalExplanation = 'Podsumowanie opisuje zakres próbki. Nie rozstrzyga, czy zmiana czasu wynika z większej ilości pracy, czy droższego pojedynczego wykonania.';
  const correctedExplanation = 'Korekta: łączny czas opisuje jedynie badaną próbkę. Bez porównywalnych wykonań nie przypisujemy go ani większej ilości pracy, ani pogorszeniu planu.';
  const boundary = 'To syntetyczny test integracji. Zebrano wyłącznie podsumowanie, bez pełnej analizy przyczyn i bez dowodu skuteczności jakiejkolwiek zmiany.';
  const nextStep = 'Przed rekomendacją zmiany porównaj tę samą operację w spokojnym okresie i w szczycie, zachowując liczbę wykonań oraz przetworzonych wierszy.';
  const board = companion.present({ language: 'pl', title: 'Co mówi podsumowanie próbki?', takeaway: observed,
    panels: [{ type: 'table', title: 'Zakres pomiaru', classification: 'measured', source: evidence.evidence_id,
      columns: ['Miara', 'Wartość'], rows: [['Liczba snapshotów', evidence.result.snapshot_count], ['DB Time [s]', evidence.result.total_db_time_s]] }] });
  await companion.advanceAudit({ chapter_id: 'performance_profile' });
  const briefInput = { chapter_id: 'performance_profile', question: 'Czy podsumowanie ustala przyczynę kosztu?',
    explanation: originalExplanation, conclusion: 'Podsumowanie wyznacza następny pomiar; przyczyna pozostaje nieustalona.',
    evidence_boundary: boundary, next_step: nextStep, open_questions: ['Czy pojedyncze wykonanie stało się droższe?'], evidence_refs: refs };
  let status = await companion.recordBrief({ ...briefInput, view_id: board.view_id });
  let brief = status.briefs.find(b => b.chapter_id === 'performance_profile' && b.project_id === project_id);
  assert.ok(brief?.brief_id); assert.equal(brief.revision, 1);
  companion.show(evidence.view_id);
  for (let i = 0; i < 24; i++) await companion.query('get_database_load_summary', { top_n: 3 });
  assert.ok(companion.state.views.length <= 20);
  assert.equal(companion.state.views.some(v => v.id === board.view_id), false);
  status = await companion.auditStatus();
  assert.equal(status.briefs.find(b => b.brief_id === brief.brief_id).visual.panels[0].source, evidence.evidence_id);
  status = await companion.recordBrief({ ...briefInput, brief_id: brief.brief_id, explanation: correctedExplanation });
  brief = status.briefs.find(b => b.brief_id === brief.brief_id);
  assert.equal(brief.revision, 2); assert.equal(status.history_count, 1);
  assert.ok(brief.updated_at_revision > 0);
  assert.deepEqual(brief.evidence_refs, refs);
  checks.evicted_visual_retained = true;
  checks.corrected_brief_revision = brief.revision;

  await assert.rejects(core('preview_report'), /AUDIT_REPORT_NOT_REQUESTED/);
  await assert.rejects(core('finalize_report', { allow_incomplete: true }), /AUDIT_REPORT_NOT_REQUESTED/);
  await assert.rejects(companion.authorizeReport({ user_request: 'W teście proszę o raport roboczy.' }), /AUDIT_DISCUSSION_INCOMPLETE/);
  checks.report_request_gate = true;

  // Test-only coverage gaps exercise chapter state without pretending an audit was done.
  for (const chapter of status.chapters) {
    await companion.advanceAudit({ chapter_id: chapter.chapter_id });
    if (chapter.chapter_id === 'performance_profile') continue;
    await companion.recordBrief({ chapter_id: chapter.chapter_id, question: chapter.question_pl,
      explanation: 'W tym teście integracji nie badamy tej kategorii. Zapis służy wyłącznie sprawdzeniu zachowania jawnej luki w rozmowie i późniejszym raporcie.',
      conclusion: 'Brak pomiarów pozwalających wydać ocenę w tej kategorii.', evidence_boundary: boundary,
      next_step: 'W rzeczywistym audycie zbierz pomiary odpowiednie dla tej kategorii przed formułowaniem oceny.',
      open_questions: ['Jakie pomiary będą potrzebne do zamknięcia tej luki?'], evidence_refs: [] });
  }
  status = await companion.auditStatus();
  assert.ok(status.chapters.every(c => c.status === 'discussed'));
  assert.equal(status.report_authorized, false);
  checks.chapters_discussed = status.chapters.length;
  await assert.rejects(core('preview_report'), /AUDIT_REPORT_NOT_REQUESTED/);
  await companion.authorizeReport({ user_request: 'W tym teście przygotuj wyłącznie roboczy raport z jawnie niepełnymi pomiarami.' });

  const finding = await core('record_finding', { category: 'performance_profile', title: 'Granice podsumowania testowej próbki',
    severity: 'informational', confidence: 'low', conclusion: brief.conclusion,
    reader_brief: { source_audit_brief: { brief_id: brief.brief_id, revision: brief.revision }, observed,
      explanation: correctedExplanation, next_step: nextStep, boundary, evidence_refs: refs },
    mechanism: 'Agregacja sumuje obserwowany czas bazy. Ten rachunek nie rozdziela wolumenu pracy od kosztu pojedynczego wykonania.',
    temporal_pattern: `Zakres próbki zaczyna się od ${evidence.result.first_snapshot?.begin_snap_time || 'braku oznaczonego początku'} i kończy na ${evidence.result.last_snapshot?.end_snap_time || 'braku oznaczonego końca'}; nie wyznaczono okresu porównawczego.`,
    affected_workload: 'Nie przypisano kosztu do konkretnej operacji biznesowej ani do SQL; ten kontrolowany test sprawdza jedynie przepływ ustaleń.',
    evidence_limitations: boundary, evidence_summary: `Projekt ${project_id}: ${observed}`,
    details: 'Dodatkowe testowe wyjaśnienie pozostaje w danych technicznych. Nie stanowi pomiaru ani zalecenia zmiany produkcyjnej.',
    evidence_refs: refs, guidance_refs: [], recommendations: [] });
  await core('record_issue', { issue_id: 'I-0001', title: 'Uzupełnienie porównania', decision_summary: brief.conclusion,
    decision_boundary: boundary, scope: 'Syntetyczna próbka jednego projektu w teście integracji.',
    grouping_rationale: 'Jedno ustalenie opisuje granice tego samego podsumowania, bez łączenia odrębnych mechanizmów.',
    confidence: 'low', canonical_finding_id: finding.finding_id, finding_ids: [finding.finding_id] });
  await core('configure_report', { output_format: 'both' });
  await assert.rejects(core('finalize_report', { allow_incomplete: true }), /REPORT_EDITORIAL_REVIEW_REQUIRED/);
  const preview = await core('preview_report', { format: 'both' });
  assert.equal(preview.preview, true); assert.equal(preview.published, false);
  assert.equal(preview.status.ready_to_finalize, false);
  for (const text of [observed, correctedExplanation, boundary, nextStep]) assert.ok(preview.markdown.includes(text), `Preview lost reader text: ${text}`);
  assert.ok(preview.html.includes(correctedExplanation));
  const expectedChecks = ['explanation_clear', 'actions_executable', 'uncertainty_preserved', 'evidence_preserved', 'language_consistent'];
  assert.deepEqual(preview.editorial_checklist, expectedChecks);
  await core('record_report_review', { preview_id: preview.preview_id,
    checks: Object.fromEntries(expectedChecks.map(key => [key, true])),
    notes: 'Test sprawdził w rzeczywistym podglądzie obecność poprawionego polskiego wyjaśnienia, następnego pomiaru, ograniczenia oraz jawnie niepełnego pokrycia. Nie poświadcza kompletnej diagnozy.' });
  await assert.rejects(core('finalize_report'), /REPORT_INCOMPLETE/);
  // Deliberate fixture draft: production finalization remains blocked by missing evidence.
  const draft = await core('finalize_report', { allow_incomplete: true });
  assert.equal(draft.draft, true);
  assert.equal(draft.report.config.language.toLowerCase(), 'pl');
  assert.equal(draft.report.coverage.ready_to_finalize, false);
  assert.deepEqual(draft.report.evidence[evidence.evidence_id].result, evidence.result);
  assert.equal(draft.report.findings[0].reader_brief.source_audit_brief.revision, 2);
  assert.ok(draft.markdown.includes(correctedExplanation));
  checks.editorial_gate = true; checks.explicit_incomplete_draft = true; checks.raw_evidence_preserved = true;

  const exported = await companion.exportAudit();
  assert.equal(exported.persisted, false); assert.equal(exported.checkpoint.config.language.toLowerCase(), 'pl');
  assert.equal(exported.checkpoint.audit.history[0].visual.panels[0].source, evidence.evidence_id);
  assert.deepEqual(exported.checkpoint.evidence[evidence.evidence_id].result, evidence.result);
  // A fresh transport and analysis exercise the same import path used after restart.
  await restored.open({ mode: 'live', endpoint, language: 'pl' });
  const resumed = await restored.restoreAudit({ checkpoint: JSON.parse(JSON.stringify(exported.checkpoint)) });
  assert.notEqual(resumed.analysis_id, analysis_id);
  assert.equal(resumed.report_authorized, false); assert.equal(resumed.editorial_review_current, false);
  assert.deepEqual(resumed.briefs, (await companion.auditStatus()).briefs);
  assert.equal(resumed.history_count, 1);
  await assert.rejects(restored.client.call('finalize_report', { analysis_id: resumed.analysis_id, allow_incomplete: true }), /AUDIT_REPORT_NOT_REQUESTED/);
  const reexported = await restored.exportAudit();
  assert.deepEqual(reexported.checkpoint.evidence, exported.checkpoint.evidence);
  assert.deepEqual(reexported.checkpoint.audit.history, exported.checkpoint.audit.history);
  checks.restore_new_analysis = true; checks.restore_clears_authorization = true;
  console.log(JSON.stringify({ ok: true, ...checks }));
} finally {
  await Promise.allSettled([companion.close(), restored.close()]);
}
