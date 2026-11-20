#!/usr/bin/env python3
"""Replay a TX discussion using two external AWR JSON captures and two real process lifetimes."""
import argparse
import asyncio
from hashlib import sha256
import json
import math
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from uuid import uuid4
from zipfile import ZipFile

from pilot_replay import REPO, processes

EVENT = 'enq: TX - row lock contention'
BOUNDARY = ('Czas oczekiwania sumuje pracę oczekujących sesji. Do ustalenia, która '
            'transakcja trzymała blokadę, potrzebny jest łańcuch sesji powiązany z kluczem biznesowym.')
NEXT_STEP = ('DBA wraz z zespołem aplikacji porównuje dwa równoległe zgłoszenia tego samego klucza '
             'z próbą dla różnych kluczy. W obu próbach rejestruje sesję oczekującą i blokującą oraz granice transakcji.')
QUESTION = 'Która transakcja trzymała blokadę i jakiego klucza dotyczyła?'
OLD_CONCLUSION = 'Hipoteza robocza przypisuje blokowanie operacji widocznej po stronie oczekującej.'
TITLE = 'Ustal, która transakcja odpowiada za blokady w badanym oknie'
SUCCESS = 'Dla odtworzonego oczekiwania zespół potrafi wskazać klucz, obie sesje i moment zwolnienia blokady.'


def reference(path, first, last):
    capture = json.loads(path.read_bytes())
    rows = {}
    for snap in capture['awrs']:
        key = snap['snap_info']['begin_snap_id']
        if first <= key <= last:
            matches = [e for e in snap['foreground_wait_events'] if e['event'] == EVENT]
            assert len(matches) == 1 and key not in rows, 'Expected one foreground TX value per requested snapshot'
            rows[key] = matches[0]['total_wait_time_s']
    assert sorted(rows) == list(range(first, last + 1)), 'Use a contiguous window covered by both instances'
    return rows


async def collect(client, rows, first, last):
    result = await client.call('query', tool='get_wait_event_timeline',
                               arguments={'event_name': EVENT, 'kind': 'foreground'}, display=False)
    measured = {r['snap_id']: r['total_wait_time_s'] for r in result['result']['timeline'] if first <= r['snap_id'] <= last}
    assert measured == rows
    return result['evidence_id']


def find_brief(status, name):
    return next(b for b in status['briefs'] if b['brief_id'] == name)


async def replay(args, scratch):
    ranges = [reference(p, args.first, args.last) for p in (args.reference_capture, args.incident_capture)]
    totals = [sum(rows.values()) for rows in ranges]
    assert totals[1] > totals[0], 'Supply the reference instance first and the higher-wait instance second'
    marker = 'PRIVATE-' + uuid4().hex
    note = ('Audytor skorygował przypisanie roli blokującego. Po wznowieniu trzeba rozdzielić sesję czekającą od właściciela transakcji. ' + marker)
    corrected = (f'W badanym oknie sesje {args.incident_label} zgromadziły więcej czasu oczekiwania na blokady TX '
                 f'niż sesje {args.reference_label}. Pomiar wskazuje, gdzie szukać oczekujących sesji. '
                 'Identyfikacja transakcji blokującej wymaga odtworzenia łańcucha blokowania.')
    next_step = NEXT_STEP
    if args.business_key:
        next_step += f' W tej próbie kluczem biznesowym jest {args.business_key}.'
    ref_brief, main_brief = 'B-TX-REFERENCE', 'B-TX-INCIDENT'
    common = dict(chapter_id='wait_events', question=QUESTION, evidence_boundary=BOUNDARY,
                  next_step=next_step, open_questions=[QUESTION])
    async with processes(args, scratch, 1) as (c, before):
        projects = (await c.call('projects'))['projects']
        # The native numeric instance ID comes from the capture; the MCP project ID is advertised by the server.
        by_instance = {int(p['database']['instance_num']): p['project_id'] for p in projects}
        ref_project, incident_project = (by_instance[i] for i in (args.reference_instance, args.incident_instance))
        selected = await c.call('select_project', project_id=ref_project, project_ids=[ref_project, incident_project])
        analysis = selected['analysis_id']
        await c.call('enable_tx_pilot')
        await c.call('advance_audit', chapter_id='wait_events')
        ref = await collect(c, ranges[0], args.first, args.last)
        await c.call('record_brief', **common, brief_id=ref_brief, evidence_refs=[ref],
                     explanation='Punkt odniesienia obejmuje tę samą serię przedziałów godzinowych.',
                     conclusion=f'Czas oczekiwania na {args.reference_label} stanowi punkt odniesienia dla drugiej instancji.')
        await c.call('resume_audit', analysis_id=analysis, project_id=incident_project)
        incident = await collect(c, ranges[1], args.first, args.last)
        board = await c.call('present', title='Oczekiwanie na blokady transakcyjne', language='pl',
                             takeaway='Suma czasu sesji w badanych przedziałach.',
                             panels=[dict(type='table', title='Pomiar TX', classification='measured', source=incident,
                                          columns=['Instancja', 'Czas oczekiwania [s]'],
                                          rows=[[args.incident_label, totals[1]]])])
        main = dict(**common, brief_id=main_brief, evidence_refs=[incident], view_id=board['view_id'],
                    explanation='Oczekiwania TX zostały zsumowane dla kolejnych przedziałów godzinowych.')
        status = await c.call('record_brief', **main, conclusion=OLD_CONCLUSION)
        assert find_brief(status, main_brief)['revision'] == 1
        status = await c.call('record_brief', **main, conclusion=corrected, private_note=note,
                              operation_id=str(uuid4()), expected_revision=status['snapshot']['revision'])
        brief = find_brief(status, main_brief)
        assert brief['revision'] == 2 and brief['conclusion'] == corrected
        await c.call('advance_audit', chapter_id='sql', focus_brief_id=main_brief)
        saved = await c.call('advance_audit', chapter_id='wait_events', focus_brief_id=main_brief)
        audit_id = saved['audit_id']
        assert saved['snapshot']['persisted'] and saved['snapshot']['revision'] > status['snapshot']['revision']
        print(f'Correction and stage transitions saved. Stopping both processes at snapshot {saved["snapshot"]["revision"]}.', flush=True)
    async with processes(args, scratch, 2) as (c, after):
        assert set(before).isdisjoint(after)
        listed = await c.call('list_saved_audits')
        assert any(a['audit_id'] == audit_id for a in listed['audits'])
        restored = await c.call('resume_saved_audit', audit_id=audit_id, project_id=incident_project)
        assert restored['analysis_id'] != analysis and restored['snapshot']['revision'] == saved['snapshot']['revision']
        assert not restored['report_authorized'] and restored['current_chapter_id'] == 'wait_events'
        assert restored['resume_context']['last_private_note'] == note
        assert restored['resume_context']['last_correction']['brief_revision'] == 2
        restored_brief = find_brief(restored, main_brief)
        assert restored_brief == brief and restored_brief['open_questions'] == [QUESTION]
        view = await c.call('context', include_displayed_board=True)
        assert view['viewed_brief_id'] == main_brief and view['displayed_board']
        assert restored_brief['visual']['panels'][0]['rows'] == [[args.incident_label, totals[1]]]
        projections = [dict(instance_label=label, evidence_ref=ref_id, first_snap_id=args.first, last_snap_id=args.last)
                       for label, ref_id in [(args.reference_label, ref), (args.incident_label, incident)]]
        projections[0].update(source_brief_id=ref_brief, source_brief_revision=1)
        preview = await c.call('preview_tx_report', brief_id=main_brief, brief_revision=2, title=args.title,
                               owner='DBA', success_criterion=SUCCESS, evidence=projections)
        measurements = preview['measurements']
        assert all(math.isclose(row['total_wait_s'], value) for row, value in zip(measurements, totals, strict=True))
        assert all(row['observed_intervals'] == len(ranges[i]) for i, row in enumerate(measurements))
        preview_root = Path(preview['index_path']).parent
        # Read and verify the actual public copy before authorizing these files.
        html = ''.join(p.read_text() for p in sorted(preview_root.rglob('*.html')))
        assert all(s in html for s in [corrected, BOUNDARY, next_step, SUCCESS])
        assert marker not in html and OLD_CONCLUSION not in html
        published = await c.call('publish_tx_report', preview_id=preview['preview_id'],
                                 user_request='Przygotuj gotowy ZIP dla klienta z przyjętej wersji wniosku o blokadach TX.')
        zip_path = Path(published['zip_path'])
        with ZipFile(zip_path) as bundle:
            files = bundle.namelist()
            assert sorted(files) == ['evidence.html', 'index.html', 'report.css']
            assert bundle.testzip() is None
            for name in files:
                raw = bundle.read(name)
                assert raw == (preview_root / name).read_bytes()
                for private in (marker, analysis, restored['analysis_id'], audit_id, OLD_CONCLUSION, ref_brief, main_brief):
                    assert private.encode() not in raw and private not in name
            with TemporaryDirectory(dir=scratch) as tmp:
                bundle.extractall(tmp)
                readers = [name for name in files if name.endswith('.html')]
                validator = await asyncio.create_subprocess_exec(
                    sys.executable, REPO / 'scripts/validate-report-package.py', '--root', tmp, *readers, stdout=asyncio.subprocess.PIPE)
                output, _ = await validator.communicate()
                if validator.returncode:
                    raise AssertionError(output.decode()[:1000])
        checksum = sha256(zip_path.read_bytes()).hexdigest()
        assert published['zip_sha256'] == checksum
        assert Path(published['receipt_path']).is_file()
        return dict(audit_id=audit_id, snapshot=saved['snapshot'], analysis_before=analysis,
                    analysis_after=restored['analysis_id'], pids_before=before, pids_after=after,
                    brief_revision=2, private_note_recovered=True, board_recovered=True,
                    verified_measurements=measurements, public_files=files, zip_path=str(zip_path),
                    zip_sha256=checksum, release_receipt=published['receipt_path'], matches_preview=True,
                    private_markers_absent=True, offline_links_valid=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, default=REPO / 'target/release/jas-min')
    p.add_argument('--companion', type=Path, default=REPO / 'plugins/jasmin-companion/server.mjs',
                   help='Companion stdio entry point, optionally an installed copy')
    p.add_argument('--workspace', type=Path, required=True, help='Private workspace; creates a new isolated audit')
    p.add_argument('--title', default=TITLE, help='Reader-facing decision title')
    p.add_argument('--business-key', help='Optional public description of the key for the proposed test')
    for name in ('reference', 'incident'):
        p.add_argument(f'--{name}-capture', type=Path, required=True)
        p.add_argument(f'--{name}-instance', type=int, required=True)
        p.add_argument(f'--{name}-label', required=True, help='Public label to include in the report')
    for name in ('first', 'last'):
        p.add_argument(f'--{name}', required=True, type=int, help='Inclusive begin snapshot ID')
    args = p.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.companion = args.companion.resolve(strict=True)
    args.reference_capture = args.reference_capture.resolve(strict=True)
    args.incident_capture = args.incident_capture.resolve(strict=True)
    args.captures = [args.reference_capture, args.incident_capture]
    args.workspace = args.workspace.resolve()
    os.umask(0o077)
    args.workspace.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix='jasmin-tx-replay-') as tmp:
        result = asyncio.run(replay(args, Path(tmp)))
    path = args.workspace / ('verification-' + result['audit_id'] + '.json')
    with path.open('x') as out:
        json.dump(result, out, indent=2, ensure_ascii=False)
    print(path)


if __name__ == '__main__':
    main()
