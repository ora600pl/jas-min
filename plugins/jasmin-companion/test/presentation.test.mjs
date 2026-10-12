import test from 'node:test';
import assert from 'node:assert/strict';
import { Companion } from '../companion.mjs';
import { automaticPresentation, timestamp } from '../public/charts.js';
import { validatePresentation } from '../presentation.mjs';
const view=(tool,result)=>({tool,args:{},data:{evidence_id:'E-fixture',result}});
const board=()=>({title:'Why did contention spike?',takeaway:'The wait rose despite fewer hard parses. The trigger is unproven.',panels:[{type:'line',title:'Wait by instance',classification:'measured',source:'Fixture AWR',unit:'seconds',x_label:'Database time',series:[{name:'Instance 1',points:[{x:1,label:'Baseline',value:0,snap_id:1,instance:'1'},{x:2,label:'Missing',value:null,snap_id:2,instance:'1'},{x:3,label:'Incident',value:100,snap_id:3,instance:'1'}]}]}]});
test('actual timeline shapes: nested SQL values, missing TOP observations, RAC ambiguity and database timestamps',()=>{
  const result=automaticPresentation(view('get_sql_timeline',{sql_id:'fixture',timeline:[
    {snap_id:1,begin_snap_time:'09-Oct-26 06:00:18',elapsed:{elapsed_time_s:0},cpu:null},
    {snap_id:2,begin_snap_time:'09-Oct-26 07:00:18',elapsed:null,cpu:{cpu_time_s:2}},
    {snap_id:2,begin_snap_time:'09-Oct-26 07:00:18',elapsed:{elapsed_time_s:4},cpu:null}
  ]}));
  assert.equal(result.panels.length,2);
  assert.deepEqual(result.panels[0].series[0].points.map(p=>p.value),[0,null,4]);
  assert.equal(result.panels[0].scatter,true);
  assert.equal(result.panels[0].series.length,1); // No invented instance order.
  assert.equal(timestamp('09-Oct-26 06:00:18'),Date.UTC(2026,9,9,6,0,18));
  assert.equal(timestamp('2026-10-09 06:00:18'),Date.UTC(2026,9,9,6,0,18));
  const waits=automaticPresentation(view('get_wait_event_timeline',{event_name:'fixture wait',timeline:[{snap_id:1,instance_number:1,total_wait_time_s:10,avg_wait_ms:2},{snap_id:1,instance_number:2,total_wait_time_s:12,avg_wait_ms:4}]}));
  assert.equal(waits.panels[0].series.length,2);
  assert.equal(waits.panels[0].scatter,false);
  assert.deepEqual(waits.panels.map(p=>p.unit),['seconds','milliseconds']);
});
test('summary and empty metric produce visual models, unsupported metadata has no raw JSON card',()=>{
  const summary=automaticPresentation(view('get_database_load_summary',{busiest_snapshots_by_db_time:[{snap_id:7,db_time_s:120,db_cpu_s:3,top_fg_event:{event:'fixture',pct_dbtime:70}}],top_foreground_waits_by_total_time:[{event:'fixture',total_wait_time_s:90}]}));
  assert.deepEqual(summary.panels.map(p=>p.type),['table','bar']);
  assert.equal(summary.panels[0].rows[0][2],120);
  assert.equal(automaticPresentation(view('get_metric_time_series',{name:'DB Time(s)',kind:'load_profile',series:[]})).panels[0].series.length,0);
  assert.deepEqual(automaticPresentation(view('list_available_metrics',{metrics:['x']})).panels,[]);
});
test('presentation validation rejects ambiguous instances, nonfinite values, missing provenance and false measured mechanisms',()=>{
  assert.equal(validatePresentation(board()).panels[0].series[0].points[0].value,0);
  for(const mutate of [b=>b.panels[0].series[0].points[1].x=1,b=>b.panels[0].series[0].points[1].value=Infinity,b=>delete b.panels[0].source,b=>b.panels[0].type='html',b=>b.panels[0]={type:'flow',title:'Cause',source:'x',classification:'measured',steps:[{title:'x',detail:'y'},{title:'z',detail:'w'}]}]){const b=board();mutate(b);assert.throws(()=>validatePresentation(b));}
});
test('board survives collection and eviction; exact instance selection and project reset',async()=>{
  const c=new Companion();await c.open({mode:'demo'});await c.select('demo');
  const shown=c.present(board());c.note('A short English interpretation.');
  for(let i=0;i<24;i++)await c.query('get_metric_time_series',{kind:'load_profile',name:'DBTime'});
  assert.equal(c.state.current,shown.view_id);assert.equal(c.state.note,'A short English interpretation.');
  assert.ok(c.state.views.length<=20);assert.ok(c.state.views.find(v=>v.id===shown.view_id));
  assert.throws(()=>c.selectPoint({snap_id:3,instance:'2'}));
  c.selectPoint({snap_id:3,instance:'1'});assert.deepEqual(c.state.selection,{snap_id:3,instance:'1'});
  const evidence=c.state.views.at(-1);c.show(evidence.id);c.show(shown.view_id);assert.equal(c.state.note,'A short English interpretation.');
  c.present({title:'Two views',takeaway:'Measured demo data.',view_ids:[evidence.id]});
  await c.select('demo');assert.equal(c.state.current,null);assert.deepEqual(c.state.selection,{});await c.close();
});
