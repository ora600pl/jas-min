#!/usr/bin/env python3
"""Verify a two-instance RAC CPU scenario, audit recovery and offline release against external reference rows."""
import argparse
import asyncio
from hashlib import sha256
from html.parser import HTMLParser
import json
import math
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from uuid import uuid4
from zipfile import ZipFile

from pilot_replay import REPO, processes

TITLE = 'Zapotrzebowanie RAC na CPU po zmianach w monitoringu i SQL'
QUESTION = 'Ile czasu CPU odzyska klaster przy takim samym wolumenie pracy?'
OLD = 'Wstępne oszacowanie korzysta z osobnego maksimum każdej instancji.'
BOUNDARY = ('DB CPU mierzy czas procesora zużyty przez sesje bazy. Średnia za przedział AWR może ukrywać '
            'krótsze spiętrzenia. Zestawienia TOP SQL obejmują tylko widoczne wpisy. Koszt zastępczego '
            'monitoringu, czas odpowiedzi aplikacji i zachowanie po utracie węzła wymagają osobnego pomiaru.')
NEXT = ('Zespół DBA i właściciele aplikacji mierzą koszt zastępczego monitoringu i testują optymalizacje SQL '
        'przy porównywalnej liczbie transakcji. Zbierają DB CPU obu instancji w tych samych krótszych oknach '
        'oraz czasy odpowiedzi krytycznych operacji. Przed decyzją o konfiguracji vCPU przeprowadzają '
        'próbę przejęcia usług przez jeden węzeł.')
SUCCESS = ('Przy porównywalnym wolumenie pracy zespół potwierdza oszczędność CPU, utrzymanie uzgodnionych '
           'czasów odpowiedzi i ciągłość monitoringu, także podczas przejęcia usług.')


def close(actual, expected):
    assert math.isclose(actual, expected, rel_tol=1e-9, abs_tol=1e-8), (actual, expected)


def number(value):
    return f'{value:.2f}'.replace('.', ',')


def verify_series(result, reference, labels, fraction):
    """Reference rows are ordered by paired snapshot and contain independent per-instance seconds."""
    clusters = []
    saved_seconds = 0
    for interval, *nodes in zip(result['intervals'], *(reference[n] for n in labels.values()), strict=True):
        obs = modeled = removed = 0
        assert {r['snap'] for r in nodes} == {interval['begin_snap_id']}
        reported = {row['project_id']: row for row in interval['instances']}
        assert set(reported) == set(labels)
        for (project, _), row in zip(labels.items(), nodes, strict=True):
            actual = reported[project]
            for field, key in [('db_cpu_seconds','db_cpu'), ('wall_seconds','seconds'),
                               ('monitor_cpu_seconds','monitor'), ('selected_sql_cpu_seconds','selected')]:
                close(actual[field], row[key])
            removal = row['monitor'] + fraction * row['selected']
            rate = row['db_cpu'] / row['seconds']
            remaining = (row['db_cpu'] - removal) / row['seconds']
            close(actual['observed_active_cpu'], rate)
            close(actual['scenario_active_cpu'], remaining)
            obs += rate
            modeled += remaining
            removed += removal
        close(interval['observed_active_cpu'], obs)
        close(interval['scenario_active_cpu'], modeled)
        close(interval['saved_cpu_seconds'], removed)
        clusters.append((interval['begin'], interval['end'], obs, modeled))
        saved_seconds += removed
    assert len(clusters) == result['paired_intervals'] and clusters
    for index, key in [(2,'observed_peak'), (3,'scenario_peak')]:
        peak = max(clusters, key=lambda p: p[index])
        close(result[key]['active_cpu'], peak[index])
        assert (result[key]['begin'],result[key]['end']) == peak[:2]
    close(result['saved_cpu_hours'], saved_seconds / 3600)


class EvidenceTables(HTMLParser):
    """Read the static evidence cells as a client opening the ZIP would see them."""
    def __init__(self):
        super().__init__()
        self.tables = []
        self.in_cell = False
        self.row = []

    def handle_starttag(self, tag, attrs):
        if tag == 'tbody':
            self.tables.append([])
        if tag == 'td':
            self.in_cell = True
            self.value = ''

    def handle_endtag(self, tag):
        if tag == 'td':
            self.in_cell = False
            self.row.append(self.value)
        if tag == 'tr' and self.row:
            self.tables[-1].append(self.row)
        if tag in ('thead', 'tr', 'tbody'):
            self.row = []

    def handle_data(self, data):
        if self.in_cell:
            self.value += data


def verify_public_evidence(html, result):
    page = EvidenceTables()
    page.feed(html)
    clustered, *nodes = page.tables
    assert len(nodes) == len(result['instances'])
    def cells(row, record, fields):
        assert row[:2] == [record['begin'], record['end']]
        for raw, key in zip(row[2:], fields, strict=True):
            assert abs(float(raw.replace(',', '.')) - record[key]) <= 0.00005001, (key, raw, record[key])
    node_fields = ['wall_seconds', 'db_cpu_seconds', 'monitor_cpu_seconds', 'selected_sql_cpu_seconds',
                   'monitor_top_rows', 'selected_top_rows', 'observed_active_cpu', 'scenario_active_cpu']
    for record, cluster_row, *node_rows in zip(result['intervals'], clustered, *nodes, strict=True):
        cells(cluster_row, record, ['observed_active_cpu', 'scenario_active_cpu', 'saved_cpu_seconds'])
        for row, measurement in zip(node_rows, record['instances'], strict=True):
            cells(row, measurement, node_fields)


async def replay(args, scratch):
    recipe = json.loads(args.recipe.read_bytes())
    reference = json.loads(args.reference.read_bytes())
    marker = 'PRIVATE-' + uuid4().hex
    brief_id = 'B-RAC-CPU'
    note = 'Po wznowieniu sprawdzić warunki pomiaru po zmianie monitoringu. ' + marker
    async with processes(args, scratch, 1) as (c, before):
        projects = (await c.call('projects'))['projects']
        ids = {int(p['database']['instance_num']):p['project_id'] for p in projects}
        labels = {ids[num]:label for num, label in zip(args.instance, args.label, strict=True)}
        focus = next(iter(labels))
        selected = await c.call('select_project', project_id=focus, project_ids=list(labels))
        analysis = selected['analysis_id']
        initial = await c.call('enable_cpu_pilot')
        assert initial['pilot_kind'] == 'cpu'
        await c.call('advance_audit', chapter_id='sql')
        query = dict(project_ids=list(labels), **recipe)
        evidence = await c.call('query', tool='get_rac_cpu_scenario', arguments=query, display=True)
        result = evidence['result']
        verify_series(result, reference, labels, recipe['sql_reduction_pct'] / 100)
        obs, model = (result[k]['active_cpu'] for k in ('observed_peak','scenario_peak'))
        conclusion = (f'Szczyt całego RAC wyniósł {number(obs)} aktywnego CPU. Przy podanych założeniach '
                      f'maksimum spada do {number(model)} ({number((1-model/obs)*100)}% mniej). '
                      f'Łączna modelowana oszczędność w analizowanym okresie to {number(result["saved_cpu_hours"])} CPU·h.')
        if args.basket_description:
            conclusion += ' ' + args.basket_description
        main = dict(chapter_id='sql', brief_id=brief_id, question=QUESTION, evidence_refs=[evidence['evidence_id']],
                    view_id=evidence['view_id'], explanation='Dla każdego wspólnego przedziału zsumowano DB CPU obu '
                    'instancji po przeliczeniu przez czas trwania ich własnych pomiarów. Maksimum wybrano z tej serii.',
                    evidence_boundary=BOUNDARY, next_step=NEXT, open_questions=[QUESTION])
        status = await c.call('record_brief', **main, conclusion=OLD)
        status = await c.call('record_brief', **main, conclusion=conclusion, private_note=note,
                              expected_revision=status['snapshot']['revision'], operation_id=str(uuid4()))
        brief = next(b for b in status['briefs'] if b['brief_id'] == brief_id)
        assert brief['revision'] == 2 and brief['conclusion'] == conclusion and brief['visual']
        away = await c.call('advance_audit', chapter_id='wait_events', focus_brief_id=brief_id)
        saved = await c.call('advance_audit', chapter_id='sql', focus_brief_id=brief_id)
        assert status['snapshot']['revision'] + 1 == away['snapshot']['revision'] == saved['snapshot']['revision'] - 1
        audit_id = saved['audit_id']
        print(f'Checked {result["paired_intervals"]} pairs. Restarting MCP and Companion after snapshot {saved["snapshot"]["revision"]}.', flush=True)
    async with processes(args, scratch, 2) as (c, after):
        assert set(before).isdisjoint(after)
        catalog = (await c.call('list_saved_audits'))['audits']
        assert any(a['audit_id'] == audit_id and a['pilot_kind'] == 'cpu' for a in catalog)
        restored = await c.call('resume_saved_audit', audit_id=audit_id, project_id=focus)
        assert restored['analysis_id'] != analysis and restored['snapshot'] == saved['snapshot']
        assert restored['pilot_kind'] == 'cpu' and restored['current_chapter_id'] == 'sql'
        assert not restored['report_authorized'] and restored['resume_context']['last_private_note'] == note
        assert restored['resume_context']['last_correction']['brief_revision'] == 2
        assert brief == next(b for b in restored['briefs'] if b['brief_id'] == brief_id)
        context = await c.call('context', include_displayed_board=True)
        assert context['viewed_brief_id'] == brief_id and context['displayed_board']
        replayed = await c.call('query', tool='get_rac_cpu_scenario', arguments=query, display=False)
        assert replayed['result'] == result
        preview = await c.call('preview_cpu_report', **dict(brief_id=brief_id, brief_revision=2, title=TITLE, owner='Lider zespołu DBA',
                               success_criterion=SUCCESS, evidence_ref=evidence['evidence_id'], instance_labels=labels, include_sql_ids=True))
        index = Path(preview['index_path'])
        html = index.read_text() + (index.parent / 'evidence.html').read_text()
        assert all(s in html for s in [conclusion, BOUNDARY, NEXT, SUCCESS, recipe['monitor_module_prefix'], *recipe['selected_sql_ids'], *labels.values()])
        assert all(s not in html for s in [OLD, marker, brief_id, audit_id, analysis, restored['analysis_id']])
        release = await c.call('publish_cpu_report', preview_id=preview['preview_id'],
                               user_request='Przygotuj gotowy ZIP ze scenariuszem CPU całego RAC dla klienta.')
        zip_path = Path(release['zip_path'])
        with ZipFile(zip_path) as bundle, TemporaryDirectory(dir=scratch) as tmp:
            files = sorted(bundle.namelist())
            assert files == ['evidence.html','index.html','report.css'] and bundle.testzip() is None
            for name in files:
                raw = bundle.read(name)
                assert raw == (index.parent / name).read_bytes()
                for private in [marker, OLD, brief_id, analysis, audit_id, restored['analysis_id'], evidence['evidence_id']]:
                    assert private.encode() not in raw
            verify_public_evidence(bundle.read('evidence.html').decode(), result)
            bundle.extractall(tmp)
            test = await asyncio.create_subprocess_exec(sys.executable, REPO / 'scripts/validate-report-package.py',
                                                       '--root', tmp, 'index.html', 'evidence.html', stdout=asyncio.subprocess.PIPE)
            message, _ = await test.communicate()
            assert test.returncode == 0, message.decode()
        checksum = sha256(zip_path.read_bytes()).hexdigest()
        assert release['zip_sha256'] == checksum and Path(release['receipt_path']).is_file()
        return dict(audit_id=audit_id, snapshot=saved['snapshot'], analysis_before=analysis, analysis_after=restored['analysis_id'],
                    pids_before=before, pids_after=after, evidence_ref=evidence['evidence_id'], chapter='sql', brief_revision=2,
                    board_recovered=True, private_note_recovered=True, checked_against_reference=len(result['intervals'])*len(labels),
                    model={k:result[k] for k in ['paired_intervals','observed_peak','scenario_peak','instances','saved_cpu_hours','assumptions']},
                    public_files=files, zip_path=str(zip_path), index_path=str(index), zip_sha256=checksum,
                    release_receipt=release['receipt_path'], matches_preview=True, private_markers_absent=True, offline_links_valid=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, default=REPO / 'target/release/jas-min')
    p.add_argument('--companion', type=Path, default=REPO / 'plugins/jasmin-companion/server.mjs')
    p.add_argument('--basket-description', help='Optional reader-facing description of the selected SQL and unchanged workload')
    p.add_argument('--workspace', type=Path, required=True)
    p.add_argument('--recipe', type=Path, required=True, help='JSON scenario parameters accepted by get_rac_cpu_scenario')
    p.add_argument('--reference', type=Path, required=True, help='JSON rows by public label. Each row has snap, seconds, db_cpu, monitor and selected (CPU seconds, disjoint buckets).')
    p.add_argument('--captures', nargs=2, required=True, type=Path)
    p.add_argument('--instance', nargs=2, required=True, type=int)
    p.add_argument('--label', nargs=2, required=True)
    p.add_argument('--first', required=True, type=int)
    p.add_argument('--last', required=True, type=int)
    args = p.parse_args()
    os.umask(0o077)
    args.binary = args.binary.resolve(strict=True)
    args.companion = args.companion.resolve(strict=True)
    args.captures = [path.resolve(strict=True) for path in args.captures]
    args.workspace = args.workspace.resolve()
    args.workspace.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix='jasmin-cpu-replay-') as temp:
        result = asyncio.run(replay(args, Path(temp)))
    path = args.workspace / ('verification-' + result['audit_id'] + '.json')
    with path.open('x') as out:
        json.dump(result, out, indent=2, ensure_ascii=False)
    print(path)


if __name__ == '__main__':
    main()
