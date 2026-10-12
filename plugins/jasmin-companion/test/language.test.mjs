import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from '../server.mjs';
import { automaticPresentation, localizedView, formatNumber } from '../public/charts.js';

test('dialogue language follows open/present/note; switches preserve evidence and cache authored translations', async t => {
  const app = await createServer(), other = await createServer();
  t.after(async () => { await app.close(); await other.close(); });
  const call = app.call, c = app.companion;
  await call('companion_open', { mode:'demo', language:'pl' });
  await call('companion_select_project', { project_id:'demo' });
  const evidence = await call('companion_query', { tool:'get_metric_time_series', arguments:{kind:'load_profile',name:'DBTime'} });
  const evidenceView = c.state.views[0], originalData = structuredClone(evidenceView.data);
  const result = await call('companion_present', { language:'pl', title:'Skąd wzrost obciążenia?', takeaway:'Szczyt wymaga dalszego sprawdzenia.', view_ids:[evidence.view_id], panels:[{
    type:'flow', title:'Możliwy mechanizm', classification:'hypothesis', source:'E-demo',
    steps:[{title:'Więcej pracy',detail:'Sesje wykonują zapytania.'},{title:'Kolejka',detail:'Sesje oczekują.'}]
  }] });
  await call('companion_note', { text:'Sprawdź zakres czasu.', language:'pl' });
  c.selectPoint({ snap_id:107 });
  const original = structuredClone(c.state.views.find(v => v.id === result.view_id));
  const analysis = c.analysis, selection = structuredClone(c.state.selection);
  const opened = await call('companion_open', { mode:'demo', language:'en' }); // same session
  assert.equal(opened.language,'en'); assert.ok(opened.pending.length);
  assert.equal(c.analysis,analysis); assert.deepEqual(c.state.selection,selection);
  assert.equal(c.state.current,result.view_id); assert.equal(c.state.views.length,2);
  assert.equal(other.companion.state.language,'en');
  const translations = {
    'presentation.title':'Why did load rise?', 'presentation.takeaway':'The peak needs further investigation.',
    'note':'Check the time window.', 'presentation.panels.1.title':'Possible mechanism',
    'presentation.panels.1.steps.0.title':'More work', 'presentation.panels.1.steps.0.detail':'Sessions execute queries.',
    'presentation.panels.1.steps.1.title':'Queue', 'presentation.panels.1.steps.1.detail':'Sessions wait.'
  };
  assert.deepEqual(new Set(opened.pending.map(f=>f.path)),new Set(Object.keys(translations)));
  const translated = await call('companion_set_language', { language:'en',view_id:result.view_id,translations:Object.entries(translations).map(([path,text])=>({path,text})) });
  assert.deepEqual(translated.pending,[]);
  const view = c.state.views.find(v=>v.id===result.view_id);
  assert.equal(localizedView(view,'en').presentation.title,'Why did load rise?');
  assert.equal(localizedView(view,'pl').presentation.title,original.presentation.title);
  assert.match(localizedView(view,'pl').presentation.panels[0].unit,/średnia/);
  assert.match(localizedView(view,'en').presentation.panels[0].unit,/average/);
  assert.deepEqual(view.presentation,original.presentation); // no mutation of original board
  assert.deepEqual(evidenceView.data,originalData); assert.deepEqual(c.state.selection,selection);
  for (const language of ['pl','en','pl']) assert.deepEqual((await call('companion_set_language',{language})).pending,[]);
  await call('companion_note',{text:'Now inspect SQL.',language:'en'});
  assert.equal((await call('companion_set_language',{language:'pl'})).pending.length,1); // old note cache invalidated
  await call('companion_show',{view_id:evidence.view_id});
  const restored = await call('companion_show',{view_id:result.view_id});
  assert.equal(restored.pending[0].path,'note');
  const state = await (await fetch(app.panel_url+'state')).json();
  assert.equal(state.language,'pl'); assert.equal(state.current,result.view_id);
  await call('companion_select_project',{project_id:'demo'});
  assert.equal(c.state.language,'pl'); assert.equal(c.state.views.length,0);
});

test('translations reject stale views, protected evidence and invalid patches atomically', async t => {
  const app=await createServer(); t.after(()=>app.close()); const call=app.call;
  await call('companion_open',{mode:'demo',language:'en'}); await call('companion_select_project',{project_id:'demo'});
  const evidence=await call('companion_query',{tool:'get_metric_time_series',arguments:{kind:'load_profile',name:'DBTime'}});
  const board=await call('companion_present',{title:'Load',takeaway:'Demo only.',view_ids:[evidence.view_id]});
  for (const patch of [
    {language:'de'},
    {language:'pl',view_id:evidence.view_id,translations:[{path:'presentation.title',text:'Obciążenie'}]},
    {language:'pl',view_id:board.view_id,translations:[{path:'presentation.title',text:'Obciążenie'},{path:'presentation.panels.0.series.0.points.0.value',text:'0'}]},
    {language:'pl',view_id:board.view_id,translations:[{path:'presentation.panels.0.source',text:'Fake'}]},
    {language:'pl',view_id:board.view_id,translations:[{path:'__proto__.polluted',text:'yes'}]},
    {language:'pl',view_id:board.view_id,translations:[{path:'presentation.title',text:'x'.repeat(181)}]}
  ]) {
    await assert.rejects(call('companion_set_language',patch));
    assert.equal(app.companion.state.language,'en');
    assert.equal(localizedView(app.companion.state.views.at(-1),'pl').presentation.title,'Load');
  }
  assert.equal({}.polluted,undefined);
});

test('automatic captions localize while SQL, Oracle terms, RAC IDs and numbers stay verbatim', () => {
  const sql = "select 'English or Polish?' from dual";
  const sqlView = {tool:'get_sql_text',data:{result:{sql_id:'xyz123',sql_text:sql},evidence_id:'E-001'}};
  assert.equal(automaticPresentation(sqlView,'pl').panels[0].text,sql);
  const wait = {tool:'get_wait_event_timeline',data:{result:{event_name:'cursor: pin S wait on X',timeline:[{snap_id:123,instance_name:'prod1',total_wait_time_s:12.34,avg_wait_ms:0}]},evidence_id:'E-002'}};
  const pl=automaticPresentation(wait,'pl'), en=automaticPresentation(wait,'en');
  assert.equal(pl.title,en.title); assert.equal(pl.panels[0].series[0].name,'prod1');
  assert.equal(pl.panels[0].unit,'sekundy'); assert.equal(pl.panels[1].title,'Średni czas oczekiwania');
  assert.deepEqual(pl.panels[0].series,en.panels[0].series);
  assert.equal(formatNumber(12.34,'pl'),'12,34'); assert.equal(formatNumber(12.34,'en'),'12.34');
});
