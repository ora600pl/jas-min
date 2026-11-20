#!/usr/bin/env python3
"""Replay two previously published pilot discussions into one durable audit and check the client edition."""
import argparse
import asyncio
from hashlib import sha256
from html import unescape
import json
import math
import os
from pathlib import Path
import re
import sys
from tempfile import TemporaryDirectory
from uuid import uuid4
from zipfile import ZipFile

from pilot_replay import REPO, processes

REPORT_SECTIONS = ('overview', 'tx', 'tx-evidence', 'cpu', 'cpu-assumptions', 'cpu-evidence', 'charts')
CHART_PAGE = re.compile(r'wykresy/(?P<kind>tx|cpu|sql)-chart-(?P<node>[1-9][0-9]*)(?:-(?P<slot>[1-9][0-9]*))?\.html')
TX_TOOL = 'get_wait_event_timeline'
CPU_TOOL = 'get_rac_cpu_scenario'


def read_json(path):
    return json.loads(path.read_bytes())


class PublishedPilot:
    def __init__(self, receipt, kind):
        issued = read_json(receipt)
        preview = receipt.parents[1]
        self.recipe = read_json(preview / 'recipe.json')
        self.recipe.pop('analysis_id')
        if kind == 'tx':
            self.recipe['evidence'] = [{k:v for k,v in selection.items() if v is not None}
                                       for selection in self.recipe['evidence']]
        self.audit_id = issued['audit_id']
        assert preview.name == issued['preview_id'] and preview.parent.name == self.audit_id
        private = preview.parents[2]
        directory = private / 'audits' / self.audit_id
        snapshot_file = max(directory.glob('[0-9]' * 20 + '.json'))
        source_zip, = (private.parent/'wydania'/issued['content_sha256']).glob('*.zip')
        assert sha256(source_zip.read_bytes()).hexdigest() == issued['zip_sha256']
        with ZipFile(source_zip) as z:
            assert all(z.read(f) == (preview/'public'/f).read_bytes() for f in z.namelist())
        snapshot = read_json(snapshot_file)
        assert snapshot['version'] == 1
        self.state = snapshot['state']
        encoded = json.dumps(self.state, ensure_ascii=False, separators=(',', ':')).encode()
        assert sha256(encoded).hexdigest() == snapshot['digest'], 'Source snapshot checksum differs'
        assert self.state.get('pilot_kind', 'tx') == kind and self.state['audit_id'] == self.audit_id
        self.provenance = dict(audit_id=self.audit_id, revision=self.state['revision'],
                               snapshot_sha256=sha256(snapshot_file.read_bytes()).hexdigest(), zip_sha256=issued['zip_sha256'])

        def object_data(digest):
            assert re.fullmatch(r'[0-9a-f]{64}', digest)
            data = (directory / 'objects' / digest).read_bytes()
            assert sha256(data).hexdigest() == digest
            return json.loads(data)
        checkpoint = object_data(self.state['checkpoint_digest'])
        self.audit = checkpoint['audit']
        self.records = {eid: object_data(key) for eid, key in self.state['evidence_objects'].items()}
        self.main = self.audit['briefs'][self.recipe['brief_id']]
        assert self.main['revision'] == self.recipe['brief_revision'], 'Select a publication of the current source brief'
        self.versions = sorted([*self.audit['history'], *self.audit['briefs'].values()], key=lambda b: b['updated_at_revision'])
        assert self.state['focus_brief_id'] == self.main['brief_id']


def same_measurements(a, b):
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(same_measurements(a[k], b[k]) for k in a)
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(same_measurements(i,j) for i,j in zip(a,b))
    if isinstance(a, (float,int)) and isinstance(b, (float,int)):
        return math.isclose(a, b, rel_tol=1e-12, abs_tol=1e-12)
    return a == b


def remap(value, refs):
    if isinstance(value, list):
        return [remap(v, refs) for v in value]
    if isinstance(value, dict):
        return {k: remap(v, refs) for k,v in value.items()}
    if isinstance(value, str):
        return re.sub(r'\bE-\d+\b', lambda m: refs[m[0]], value)
    return value


async def transfer(c, session_id, pilot, kind):
    refs = {}
    for version in pilot.versions:
        await c.call('resume_audit', analysis_id=session_id, project_id=version['project_id'])
        for eid in version['evidence_refs']:
            if eid in refs:
                continue
            source = pilot.records[eid]
            assert source['tool_name'] == (TX_TOOL if kind == 'tx' else CPU_TOOL)
            arguments = dict(source['arguments'])
            if kind == 'tx':
                assert source['project_id'] == version['project_id']
                selected = next(row for row in pilot.recipe['evidence'] if row['evidence_ref'] == eid)
                arguments.update(first_snap_id=selected['first_snap_id'],last_snap_id=selected['last_snap_id'])
            data = await c.call('query', tool=source['tool_name'], arguments=arguments, display=False)
            current, archived = data['result'], source['result']
            if kind == 'cpu':
                assert same_measurements(current, archived), 'RAC CPU scenario differs from the published source'
            else:
                lo, hi = selected['first_snap_id'], selected['last_snap_id']
                def window(result):
                    return [r for r in result['timeline'] if lo <= r['snap_id'] <= hi]
                rows = window(current)
                assert current['points'] == len(rows) == hi - lo + 1 and same_measurements(rows, window(archived)), 'TX source window changed'
            refs[eid] = data['evidence_id']
        args = {k: remap(v, refs) for k,v in version.items() if k not in {
            'revision','updated_at_revision','project_id','visual','topic'}}
        if visual := version.get('visual'):
            board = await c.call('present', **{k: remap(visual[k], refs) for k in ('title','takeaway','panels','language')})
            args['view_id'] = board['view_id']
        if version['brief_id'] == pilot.main['brief_id'] and version['revision'] == pilot.main['revision']:
            args['private_note'] = pilot.state['last_private_note']
        state = await c.call('record_brief', **args)
        imported = next(b for b in state['briefs'] if b['brief_id'] == version['brief_id'])
        assert imported['revision'] == version['revision'] and imported['conclusion'] == version['conclusion']
        if visual:
            assert imported['visual']['panels'] == remap(visual['panels'],refs)
    return remap(pilot.recipe, refs), refs


async def focus(c, topic, expected_brief):
    status = await c.call('switch_audit_topic', topic=topic)
    assert status['active_topic'] == topic
    assert status['resume_context']['focus_brief_id'] == expected_brief['brief_id']
    ctx = await c.call('context', include_displayed_board=True)
    assert ctx['project_id'] == expected_brief['project_id']
    assert ctx['displayed_board']['brief_id'] == expected_brief['brief_id']
    return status


async def reject(c, tool, **args):
    out = await c.rpc('tools/call', {'name':'companion_' + tool, 'arguments':args})
    assert out.get('isError'), f'Unexpected success: {tool}'
    return out['content'][0]['text']


def verify_zip(preview, release, forbidden, recipe, unpacked):
    zpath = Path(release['zip_path'])
    entries = Path(preview['index_path']).parent
    with ZipFile(zpath) as z:
        members = z.infolist()
        assert len(set(z.namelist())) == len(members)
        files = {m.filename for m in members if not m.is_dir()}
        assert files == set(preview['files'])
        assert {m.filename for m in members if m.is_dir()} <= {'wykresy/'}
        chart_pages = {name for name in files if CHART_PAGE.fullmatch(name)}
        expected_assets = {'index.html', 'wykresy/report.css', 'wykresy/report-nav.js', 'wykresy/ora-600.png'} | chart_pages
        if chart_pages:
            expected_assets.update({'wykresy/plotly.min.js', 'wykresy/jasmin-plot.js'})
        assert files == expected_assets, files ^ expected_assets
        report = z.read('index.html').decode()
        for section in REPORT_SECTIONS:
            assert report.count(f'id="{section}"') == 1, section
        for section in ('overview', 'tx', 'cpu', 'charts'):
            assert f'href="#{section}"' in report, section
        chapters, _, catalog = report.partition('id="charts"')
        labels = [label.strip() for _, label in sorted(recipe['cpu']['instance_labels'].items())]
        for name in chart_pages:
            assert f'href="{name}"' in catalog and f'href="{name}"' in chapters, name
            chart = CHART_PAGE.fullmatch(name)
            kind = chart['kind']
            label = labels[int(chart['node']) - 1]
            assert bool(chart['slot']) == (kind == 'sql'), name
            page = z.read(name).decode()
            assert 'id="jasmin-figure"' in page and 'src="plotly.min.js"' in page, name
            assert f'href="../index.html#charts-{kind}"' in page, name
            if kind == 'tx':
                topic, rows = 'tx-window', preview['tx']['measurements']
            else:
                topic, rows = 'cpu-node', preview['cpu']['cpu_model']['instances']
            anchors = [f'{topic}-{i}' for i, row in enumerate(rows, 1) if row['instance_label'] == label]
            assert anchors, (name, label)
            for anchor in anchors:
                assert f'href="../index.html#{anchor}"' in page and f'id="{anchor}"' in report, (name, anchor)
        assert z.testzip() is None and not z.comment
        for member in members:
            assert not member.comment
            if member.is_dir():
                continue
            data = z.read(member)
            assert data == (entries/member.filename).read_bytes()
            for value in forbidden:
                assert value.encode() not in data, f'Private content in {member.filename}'
        z.extractall(unpacked)
        assert {p.name for p in unpacked.iterdir()} == {'index.html', 'wykresy'}
    receipt = read_json(Path(release['receipt_path']))
    assert receipt['preview_id'] == preview['preview_id']
    assert sha256(zpath.read_bytes()).hexdigest() == release['zip_sha256'] == receipt['zip_sha256']
    return files


async def run(args, scratch):
    tx = PublishedPilot(args.tx_receipt, 'tx')
    cpu = PublishedPilot(args.cpu_receipt, 'cpu')
    assert tx.state['project_ids'] == cpu.state['project_ids']
    marker = 'PRIVATE-' + uuid4().hex
    async with processes(args, scratch, 1) as (c, before):
        available = {p['project_id'] for p in (await c.call('projects'))['projects']}
        ids = cpu.state['project_ids']
        assert available == set(ids)
        selected = await c.call('select_project', project_ids=ids, project_id=tx.main['project_id'])
        aid = selected['analysis_id']
        state = await c.call('enable_combined_pilot')
        assert state['pilot_kind'] == 'combined' and state['active_topic'] == 'tx'
        tx_recipe, txrefs = await transfer(c, aid, tx, 'tx')
        await c.call('switch_audit_topic', topic='cpu')
        cpu_recipe, cpurefs = await transfer(c, aid, cpu, 'cpu')
        recipe = dict(title=args.title, tx=tx_recipe, cpu=cpu_recipe)
        # The public preview and both source revisions are frozen together.
        old = await c.call('preview_combined_report', **recipe)
        status = await focus(c, 'tx', tx.main)
        assert status['resume_context']['last_private_note'] == tx.state['last_private_note']
        assert status['topics']['cpu']['last_private_note'] == cpu.state['last_private_note']
        values = old['tx']['measurements']
        tx_sum = sorted(values,key=lambda r:r['total_wait_s'],reverse=True)
        assert len(tx_sum) == 2 and tx_sum[0]['observed_intervals'] == tx_sum[1]['observed_intervals']
        def wait(value):
            return f'{value:,.0f}'.replace(',',' ')
        conclusion = (f'W {tx_sum[0]["observed_intervals"]} wspólnych przedziałach sesje {tx_sum[0]["instance_label"]} '
                      f'zgromadziły {wait(tx_sum[0]["total_wait_s"])} s oczekiwania na blokady transakcyjne TX, '
                      f'a sesje {tx_sum[1]["instance_label"]} {wait(tx_sum[1]["total_wait_s"])} s. '
                      'Pomiar lokalizuje sesje czekające. Aby wskazać właściciela blokady, trzeba uchwycić '
                      'łańcuch blokowania, transakcję sesji blokującej i sporny klucz biznesowy.')
        # Working copies are made from the source brief; private notes stay in durable snapshots.
        update = {k: remap(v, txrefs) for k,v in tx.main.items() if k not in {
            'revision','updated_at_revision','project_id','visual','topic','conclusion'}}
        visual = tx.main['visual']
        panel = dict(type='table',title='Czas oczekiwania TX w tym samym oknie',classification='measured',
                     source=' / '.join(row['evidence_ref'] for row in tx_recipe['evidence']),
                     columns=['Instancja','Oczekiwanie sesji [s]'], rows=[[r['instance_label'],r['total_wait_s']] for r in tx_sum])
        board = await c.call('present',language=visual['language'], title=visual['title'],takeaway=visual['takeaway'],panels=[panel])
        note = 'Wersja po porównaniu obu instancji. Kontynuuj od testu właściciela transakcji. ' + marker
        fixed = await c.call('record_brief', **update, conclusion=conclusion, private_note=note,
                             view_id=board['view_id'], expected_revision=status['snapshot']['revision'])
        main = next(b for b in fixed['briefs'] if b['brief_id'] == tx.main['brief_id'])
        assert main['revision'] == tx.main['revision']+1
        native_stale = await reject(c, 'preview_combined_report', **recipe)
        assert 'STALE' in native_stale.upper() or 'REVIS' in native_stale.upper(), native_stale
        await reject(c,'publish_combined_report',preview_id=old['preview_id'], user_request=args.user_request)
        recipe['tx']['brief_revision'] = main['revision']
        revisited_cpu = await focus(c,'cpu',cpu.main)
        assert revisited_cpu['resume_context']['last_private_note'] == cpu.state['last_private_note']
        saved = await focus(c,'tx',tx.main)
        assert saved['resume_context']['last_correction']['conclusion'] == conclusion
        assert saved['snapshot']['persisted'] and saved['snapshot']['revision'] > fixed['snapshot']['revision']
        audit_id = saved['audit_id']
        print(f'Both source histories and new correction saved. Restarting MCP and Companion after snapshot {saved["snapshot"]["revision"]}.',flush=True)
    async with processes(args, scratch, 2) as (c, after):
        assert set(before).isdisjoint(after)
        listed = (await c.call('list_saved_audits'))['audits']
        assert any(a['audit_id']==audit_id and a['pilot_kind']=='combined' for a in listed)
        resumed = await c.call('resume_saved_audit',audit_id=audit_id)
        assert resumed['analysis_id'] != aid and resumed['snapshot'] == saved['snapshot']
        assert resumed['topics'] == saved['topics'] and resumed['briefs'] == saved['briefs']
        assert resumed['resume_context']['last_private_note'] == note and not resumed['report_authorized']
        assert (await c.call('context',include_displayed_board=True))['displayed_board']['brief_revision'] == main['revision']
        cpu_state = await focus(c,'cpu',cpu.main)
        assert cpu_state['resume_context']['last_private_note'] == cpu.state['last_private_note']
        final = await focus(c,'tx',tx.main)
        preview = await c.call('preview_combined_report',**recipe)
        assert preview['tx']['measurements'] == values and preview['cpu']['cpu_model'] == old['cpu']['cpu_model']
        report = unescape(Path(preview['index_path']).read_text())
        assert conclusion in report and cpu.main['conclusion'] in report
        release = await c.call('publish_combined_report',preview_id=preview['preview_id'],user_request=args.user_request)
        forbidden = [marker,'PRIVATE-',tx.audit_id,cpu.audit_id,audit_id,aid,resumed['analysis_id'],args.user_request,
                     tx.main['brief_id'],cpu.main['brief_id'],tx.main['conclusion'],
                     *ids,*txrefs.values(),*cpurefs.values(), *(b['conclusion'] for p in (tx,cpu) for b in p.audit['history'])]
        unpacked = scratch / 'client-edition'
        public_files = verify_zip(preview,release,forbidden,recipe,unpacked)
        test = await asyncio.create_subprocess_exec(sys.executable,REPO/'scripts/validate-report-package.py',
                    '--root',unpacked,*sorted(f for f in public_files if f.endswith('.html')),stdout=asyncio.subprocess.PIPE)
        out,_ = await test.communicate()
        assert test.returncode == 0, out.decode()
        return dict(audit_id=audit_id, sources={'tx':tx.provenance,'cpu':cpu.provenance},
                    snapshot_before_restart=saved['snapshot'], snapshot_after_navigation=final['snapshot'],
                    pids_before=before,pids_after=after,analysis_before=aid,analysis_after=resumed['analysis_id'],
                    brief_revisions={b['brief_id']:b['revision'] for b in resumed['briefs']},
                    evidence_remap={'tx':txrefs,'cpu':cpurefs}, restored_briefs_and_boards=True,
                    stale_preview_rejected=True,private_notes_recovered=True,source_measurements_replayed=True,
                    tx_measurements=values,cpu_model=preview['cpu']['cpu_model'],index_path=preview['index_path'],
                    public_files=sorted(public_files),zip_path=release['zip_path'],zip_sha256=release['zip_sha256'],
                    receipt_path=release['receipt_path'], recipe=recipe,private_markers_absent=True,offline_links_valid=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, default=REPO/'target/release/jas-min')
    p.add_argument('--companion', type=Path, default=REPO/'plugins/jasmin-companion/server.mjs')
    for kind in ('tx','cpu'):
        p.add_argument(f'--{kind}-receipt',type=Path,required=True,help='Private publication receipt of the source pilot')
    p.add_argument('--captures',type=Path,nargs=2,required=True)
    p.add_argument('--first',type=int,required=True)
    p.add_argument('--last',type=int,required=True)
    p.add_argument('--workspace',type=Path,required=True)
    p.add_argument('--title',required=True)
    p.add_argument('--user-request',required=True,help='Actual request to issue the combined client ZIP')
    args = p.parse_args()
    os.umask(0o077)
    args.captures = [path.resolve(strict=True) for path in args.captures]
    args.workspace = args.workspace.resolve()
    args.workspace.mkdir(parents=True,exist_ok=True)
    with TemporaryDirectory(prefix='jasmin-combined-replay-') as scratch:
        result = asyncio.run(run(args,Path(scratch)))
    receipt = args.workspace / ('verification-'+result['audit_id']+'.json')
    with receipt.open('x') as out:
        json.dump(result,out,indent=2,ensure_ascii=False)
    print(receipt)


if __name__ == '__main__':
    main()
