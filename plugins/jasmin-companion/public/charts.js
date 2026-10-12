// Shared, data-only adapters. No DOM and no inferred RAC identities.
export const finite = value => typeof value === 'number' && Number.isFinite(value);
export const valueAt = (row, path) => path.split('.').reduce((v, key) => v?.[key], row);
export function timestamp(value) {
  if (typeof value !== 'string') return null;
  const oracle = /^(\d{2})-([A-Za-z]{3})-(\d{2}|\d{4}) (\d{2}:\d{2}:\d{2})$/.exec(value);
  let normalized = value;
  if (oracle) {
    const month = ['jan','feb','mar','apr','may','jun','jul','aug','sep','oct','nov','dec'].indexOf(oracle[2].toLowerCase()) + 1;
    if (!month) return null;
    normalized = `${oracle[3].length === 2 ? '20' : ''}${oracle[3]}-${String(month).padStart(2,'0')}-${oracle[1]}T${oracle[4]}`;
  }
  // Treat database wall-clock times consistently; do not shift them to the browser timezone.
  if (/^\d{4}-\d\d-\d\d[ T]\d\d:\d\d(?::\d\d)?$/.test(normalized)) normalized = normalized.replace(' ', 'T') + 'Z';
  const result = Date.parse(normalized);
  return Number.isFinite(result) ? result : null;
}
export function identity(row) { return row.instance_name ?? row.instance_number ?? row.instance_id ?? row.inst_id ?? null; }
export function point(row, path) {
  return { label: row.begin_snap_time || `#${row.snap_id}`, end: row.end_snap_time, x: timestamp(row.begin_snap_time) ?? row.snap_id,
    value: finite(valueAt(row, path)) ? valueAt(row, path) : null, snap_id: row.snap_id,
    ...(identity(row) != null ? { instance: String(identity(row)) } : {}) };
}
export function timeline(rows, path, name, unit) {
  const groups = new Map();
  for (const row of rows) {
    const key = identity(row) == null ? 'Instance unspecified' : String(identity(row));
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(point(row, path));
  }
  const series = [...groups].map(([label, points]) => ({ name: label, points: points.sort((a,b) => a.x-b.x) }));
  const ambiguous = series.some(s => new Set(s.points.map(p => p.x)).size < s.points.length);
  return { type: 'line', title: name, unit, series, x_label: rows.some(r => timestamp(r.begin_snap_time) != null) ? 'Database time' : 'Snapshot',
    scatter: ambiguous, note: ambiguous ? 'Multiple records share a timestamp without distinct instance IDs. Points are shown separately; no RAC instance is inferred and no totals are added.' : 'Missing observations are gaps, not zero.' };
}
export function automaticPresentation(view) {
  const d = view.data.result ?? view.data;
  const source = view.data.evidence_id || view.tool;
  const panels = [];
  const add = p => panels.push({ classification: 'measured', source, ...p });
  const bars = (title, rows, field, unit, label = 'event') => ({type:'bar',title,unit,items:rows.map(r=>({label:String(r[label] ?? r.snap_id),value:finite(r[field])?r[field]:null}))});
  if (Array.isArray(d.timeline)) {
    if (view.tool === 'get_wait_event_timeline') {
      add(timeline(d.timeline, 'total_wait_time_s', d.event_name || 'Wait time', 'seconds'));
      add(timeline(d.timeline, 'avg_wait_ms', 'Average wait duration', 'milliseconds'));
    } else if (view.tool === 'get_sql_timeline') {
      for (const [path,label,unit] of [['elapsed.elapsed_time_s','SQL elapsed','seconds'],['cpu.cpu_time_s','SQL CPU','seconds'],['io.io_time_s','SQL I/O','seconds'],['gets.buffer_gets','Buffer gets','gets'],['reads.physical_reads','Physical reads','reads']]) {
        if (d.timeline.some(r=>finite(valueAt(r,path)))) add(timeline(d.timeline,path,`${d.sql_id} · ${label}`,unit));
      }
    }
  } else if (Array.isArray(d.series)) {
    const unit = d.field === 'pct_dbtime' ? '% DB Time' : d.field?.startsWith('pct_') ? '%' : d.kind === 'load_profile' && /db.?time|db.?cpu/i.test(d.name) ? 's/s · average active sessions' : d.kind === 'time_model' || d.field === 'total_wait_time_s' ? 'seconds' : d.field === 'per_second' ? 'per second' : d.field || 'value';
    add(timeline(d.series,'value',d.name || 'Metric timeline',unit));
  } else if (view.tool === 'get_database_load_summary') {
    if (Array.isArray(d.busiest_snapshots_by_db_time)) {
      add({type:'table',title:'Highest-load intervals',columns:['Snapshot','Database time','DB Time [s]','CPU [s]','Leading wait','DB Time [%]'],rows:d.busiest_snapshots_by_db_time.map(r=>[r.snap_id,r.begin_snap_time,r.db_time_s,r.db_cpu_s,r.top_fg_event?.event,r.top_fg_event?.pct_dbtime].map(v=>v??null)),note:'Ranked intervals, not a timeline. Records without instance identity are not assigned to RAC nodes.'});
    }
    if (Array.isArray(d.top_foreground_waits_by_total_time)) add({...bars('Foreground waits across the collection',d.top_foreground_waits_by_total_time,'total_wait_time_s','seconds'),note:'Collection totals identify workload priorities, not the cause of an incident.'});
  } else if (view.tool === 'top_wait_events_in_snapshot' && Array.isArray(d.items)) {
    const field=d.rank_by || 'pct_dbtime'; add(bars('Wait events',d.items,field,field==='pct_dbtime'?'% DB Time':field==='total_wait_time_s'?'seconds':field));
  } else if (view.tool === 'top_sqls_in_snapshot' && Array.isArray(d.items)) {
    const fields={elapsed_time:['elapsed_time_s','Elapsed [s]'],cpu_time:['cpu_time_s','CPU [s]'],io_time:['io_time_s','I/O [s]'],buffer_gets:['buffer_gets','Buffer gets'],physical_reads:['physical_reads','Physical reads']};
    const [field,label]=fields[d.metric] || fields.elapsed_time;
    add({type:'table',title:'SQL in focus',columns:['SQL ID',label,'Executions','Module'],rows:d.items.map(r=>[r.sql_id,r[field]??null,r.executions??null,r.sql_module||r.module||'—']),selections:d.items.map(r=>({snap_id:d.snap_id,sql_id:r.sql_id,...(identity(r)!=null?{instance:String(identity(r))}:{})}))});
  } else if (view.tool === 'get_sql_text' && typeof d.sql_text === 'string') {
    add({type:'sql',title:d.sql_id || 'SQL text',text:d.sql_text});
  } else if (view.tool === 'compare_snapshots') {
    for (const [key,title,columns,fields] of [
      ['load_profile_deltas','Workload: baseline vs incident',['Metric','Baseline /s','Incident /s','Delta /s'],['stat_name','a_per_sec','b_per_sec','delta']],
      ['fg_wait_event_deltas','Wait share: baseline vs incident',['Wait','Baseline [%]','Incident [%]','Delta [pp]'],['event','a_pct_dbtime','b_pct_dbtime','delta_pct_dbtime']],
      ['sql_elapsed_deltas','SQL elapsed: baseline vs incident',['SQL ID','Baseline [s]','Incident [s]','Delta [s]'],['sql_id','a_elapsed_s','b_elapsed_s','delta_elapsed_s']]
    ]) if (Array.isArray(d[key])) add({type:'table',title,columns,rows:d[key].map(r=>fields.map(f=>r[f]??null)),note:`Baseline #${d.snap_a?.snap_id ?? view.args?.snap_id_a}, incident #${d.snap_b?.snap_id ?? view.args?.snap_id_b}. Upstream deltas may substitute zero for absent TOP entries; verify original measurements before interpreting zeros or ratios. Snapshot-only lookups do not establish RAC instance identity.`});
  } else if (view.tool === 'get_snapshot_details') {
    if (Array.isArray(d.foreground_waits)) add(bars('Foreground waits in this report',d.foreground_waits.slice(0,10),'pct_dbtime','% DB Time'));
    if (Array.isArray(d.time_model)) add(bars('Time model',d.time_model,'time_s','seconds','stat_name'));
  }
  return {title:d.event_name || (d.sql_id ? `SQL ${d.sql_id}` : d.name) || ({get_database_load_summary:'Where should we look first?',get_snapshot_details:'Inside the selected interval',top_wait_events_in_snapshot:'What is the database waiting for?',top_sqls_in_snapshot:'SQL in focus'})[view.tool] || 'Evidence collected',panels:panels.slice(0,3)};
}

export const seriesPoints = data => (Array.isArray(data?.series)?data.series:[]).filter(p=>finite(p.value));
export const unitFor = data => automaticPresentation({tool:'get_metric_time_series',data:{...data,series:data.series || []}}).panels[0]?.unit || 'value';
const node = (tag,text,cls) => {const el=document.createElement(tag);if(text!==undefined)el.textContent=text;if(cls)el.className=cls;return el;};
const svgNode=(tag,attrs={},text)=>{const e=document.createElementNS('http://www.w3.org/2000/svg',tag);for(const[k,v]of Object.entries(attrs))e.setAttribute(k,v);if(text!==undefined)e.textContent=text;return e;};
const fmt=n=>finite(n)?n.toLocaleString('en-US',{maximumFractionDigits:2}):'—';
const colors=['#70d8be','#e8ba7b','#aab5ff','#ee9eb4','#8ecbf0','#d2ce95'];
function line(container,p,onSelect) {
  const all=p.series.flatMap(s=>s.points), measured=all.filter(v=>finite(v.value));
  if(!measured.length){container.append(node('p','No retained measurements. Missing does not mean zero.','empty'));return;}
  const controls=node('div',undefined,'chart-controls');
  const label=node('label','Window '), select=node('select');select.setAttribute('aria-label',`${p.title}: time window`);
  for(const [v,t]of [['all','All observations'],['peak','Around the peak']]){const option=node('option',t);option.value=v;select.append(option);}label.append(select);controls.append(label);
  const readout=node('p','Hover or focus a point to inspect it. Select it to set the conversation context.','chart-readout');
  const canvas=node('div');container.append(controls,canvas,readout);
  const peak=measured.reduce((a,b)=>a.value>b.value?a:b);
  function draw(){
    canvas.replaceChildren();
    const xs=all.map(v=>v.x).filter(finite);let lo=Math.min(...xs),hi=Math.max(...xs);
    if(select.value==='peak'){
      const unique=[...new Set(xs)].sort((a,b)=>a-b),idx=unique.indexOf(peak.x);
      lo=unique[Math.max(0,idx-5)];hi=unique[Math.min(unique.length-1,idx+5)];
    }
    const visible=p.series.map(s=>({...s,points:s.points.filter(v=>v.x>=lo&&v.x<=hi)}));
    const values=visible.flatMap(s=>s.points).filter(v=>finite(v.value));
    const min=Math.min(0,...values.map(v=>v.value)),max=Math.max(1,...values.map(v=>v.value))*1.1;
    const w=760,h=270,left=76,right=20,top=22,bottom=56;
    const x=v=>left+(w-left-right)*(v-lo)/(hi-lo||1),y=v=>top+(h-top-bottom)*(max-v)/(max-min);
    const svg=svgNode('svg',{viewBox:`0 0 ${w} ${h}`,role:'group','aria-label':`${p.title}, ${p.unit}`});
    for(let i=0;i<=4;i++){const v=min+(max-min)*i/4;svg.append(svgNode('line',{x1:left,y1:y(v),x2:w-right,y2:y(v),stroke:'#2b403b'}),svgNode('text',{x:left-10,y:y(v)+4,'text-anchor':'end'},fmt(v)));}
    const ticks=[...new Map(visible.flatMap(s=>s.points).map(v=>[v.x,v])).values()].sort((a,b)=>a.x-b.x);
    for(const idx of [...new Set([0,Math.floor((ticks.length-1)/2),ticks.length-1])]){
      const v=ticks[idx];if(!v)continue;
      const txt=v.label.length>23?v.label.slice(0,23):v.label;
      svg.append(svgNode('text',{x:x(v.x),y:h-29,'text-anchor':idx===0?'start':idx===ticks.length-1?'end':'middle'},txt));
    }
    svg.append(svgNode('text',{x:left,y:h-5},p.x_label));
    const legend=node('div',undefined,'legend');
    visible.forEach((s,index)=>{
      const color=colors[index%colors.length];let previous=null,path='';
      for(const v of s.points){
        if(!finite(v.value)){previous=null;continue;}
        const consecutive=previous && (v.snap_id==null || previous.snap_id==null || v.snap_id===previous.snap_id+1);
        path+=`${consecutive?'L':'M'}${x(v.x)},${y(v.value)} `;previous=v;
      }
      if(!p.scatter)svg.append(svgNode('path',{d:path,fill:'none',stroke:color,'stroke-width':2}));
      const item=node('span',s.name);item.style.color=color;legend.append(item);
      s.points.filter(v=>finite(v.value)).forEach(v=>{
        const circle=svgNode('circle',{cx:x(v.x),cy:y(v.value),r:all.length>150?2.8:4,class:'point',fill:color,tabindex:0,role:'button','aria-label':`${s.name}, ${v.label}, ${fmt(v.value)} ${p.unit}`,'data-snap-id':v.snap_id??'','data-instance':v.instance??''});
        const info=`${s.name} · ${v.label}${v.end?' → '+v.end:''} · ${fmt(v.value)} ${p.unit}${v.snap_id!=null?' · #'+v.snap_id:''}`;
        circle.append(svgNode('title',{},info));
        const inspect=()=>{readout.textContent=info;};circle.addEventListener('mouseenter',inspect);circle.addEventListener('focus',inspect);
        const choose=()=>{inspect();if(v.snap_id!=null)onSelect(v);};circle.addEventListener('click',choose);circle.addEventListener('keydown',e=>{if(e.key==='Enter'||e.key===' '){e.preventDefault();choose();}});svg.append(circle);
      });
    });
    canvas.append(legend,svg);
  }
  select.onchange=draw;draw();
  const missing=all.filter(v=>v.value===null).length;
  container.append(node('p',`${measured.length} measurements${missing?` · ${missing} missing`:''} · Peak: ${fmt(peak.value)} ${p.unit} at ${peak.label}`,'chart-note'));
}
export function renderPresentation(container,board,onSelect){
  container.replaceChildren();
  if(board.takeaway)container.append(node('p',board.takeaway,'takeaway'));
  if(!board.panels.length){container.append(node('p','Evidence collected. Ask for a focused comparison or explanation to bring the relevant findings into this panel.','empty'));return;}
  for(const p of board.panels){
    const card=node('section',undefined,'insight-card');
    const heading=node('div',undefined,'card-heading');heading.append(node('h3',p.title),node('span',p.classification,p.classification==='hypothesis'?'classification hypothesis':'classification'));card.append(heading);
    if(p.unit)card.append(node('p',p.unit,'unit'));
    if(p.type==='line')line(card,p,onSelect);
    else if(p.type==='bar'){
      const max=Math.max(1,...p.items.map(v=>finite(v.value)?v.value:0));
      for(const v of p.items){const row=node('div',undefined,'bar-row'),head=node('div',undefined,'bar-top');head.append(node('span',v.label),node('span',`${fmt(v.value)} ${p.unit}`));const track=node('div',undefined,'bar-track'),fill=node('div',undefined,'bar-fill');fill.style.width=`${finite(v.value)?Math.max(0,v.value)/max*100:0}%`;track.append(fill);row.append(head,track);card.append(row);}
      if(!p.items.length)card.append(node('p','No measurements returned.','empty'));
    }else if(p.type==='table'){
      const wrap=node('div',undefined,'table-wrap'),table=node('table'),thead=node('thead'),tr=node('tr');for(const c of p.columns)tr.append(node('th',c));thead.append(tr);table.append(thead);const body=node('tbody');
      p.rows.forEach((r,i)=>{const row=node('tr');r.forEach((v,j)=>{const cell=node('td');if(j===0&&p.selections?.[i]){const b=node('button',String(v??'—'));b.onclick=()=>onSelect(p.selections[i]);cell.append(b);}else cell.textContent=typeof v==='number'?fmt(v):v??'—';row.append(cell);});body.append(row);});table.append(body);wrap.append(table);card.append(wrap);
    }else if(p.type==='flow'){
      const flow=node('ol',undefined,'mechanism');for(const step of p.steps){const item=node('li');item.append(node('strong',step.title),node('p',step.detail));flow.append(item);}card.append(flow);
    }else if(p.type==='sql'){card.append(node('pre',p.text,'sql-text'));}
    if(p.note)card.append(node('p',p.note,'caveat'));
    card.append(node('p',`Source · ${p.source}`,'source'));container.append(card);
  }
}
export function renderChart(container,view,onSelect){renderPresentation(container,view.presentation || automaticPresentation(view),onSelect);}
