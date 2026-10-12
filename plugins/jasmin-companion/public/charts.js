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
export function timeline(rows, path, name, unit, language = 'en') {
  const t = text => translate(language, text);
  const groups = new Map();
  for (const row of rows) {
    const key = identity(row) == null ? t('Instance unspecified') : String(identity(row));
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(point(row, path));
  }
  const series = [...groups].map(([label, points]) => ({ name: label, points: points.sort((a,b) => a.x-b.x) }));
  const ambiguous = series.some(s => new Set(s.points.map(p => p.x)).size < s.points.length);
  return { type: 'line', title: name, unit, series, x_label: rows.some(r => timestamp(r.begin_snap_time) != null) ? t('Database time') : t('Snapshot'),
    scatter: ambiguous, note: ambiguous ? t('Multiple records share a timestamp without distinct instance IDs. Points are shown separately; no RAC instance is inferred and no totals are added.') : t('Missing observations are gaps, not zero.') };
}
export function automaticPresentation(view, language = 'en') {
  const t = text => translate(language, text);
  const lineModel = (rows, path, name, unit) => timeline(rows, path, name, t(unit), language);
  const d = view.data.result ?? view.data;
  const source = view.data.evidence_id || view.tool;
  const panels = [];
  const add = p => panels.push({ classification: 'measured', source, ...p });
  const bars = (title, rows, field, unit, label = 'event') => ({type:'bar',title:t(title),unit:t(unit),items:rows.map(r=>({label:String(r[label] ?? r.snap_id),value:finite(r[field])?r[field]:null}))});
  if (Array.isArray(d.timeline)) {
    if (view.tool === 'get_wait_event_timeline') {
      add(lineModel(d.timeline, 'total_wait_time_s', d.event_name || t('Wait time'), 'seconds'));
      add(lineModel(d.timeline, 'avg_wait_ms', t('Average wait duration'), 'milliseconds'));
    } else if (view.tool === 'get_sql_timeline') {
      for (const [path,label,unit] of [['elapsed.elapsed_time_s','SQL elapsed','seconds'],['cpu.cpu_time_s','SQL CPU','seconds'],['io.io_time_s','SQL I/O','seconds'],['gets.buffer_gets','Buffer gets','gets'],['reads.physical_reads','Physical reads','reads']]) {
        if (d.timeline.some(r=>finite(valueAt(r,path)))) add(lineModel(d.timeline,path,`${d.sql_id} · ${t(label)}`,unit));
      }
    }
  } else if (Array.isArray(d.series)) {
    const unit = d.field === 'pct_dbtime' ? '% DB Time' : d.field?.startsWith('pct_') ? '%' : d.kind === 'load_profile' && /db.?time|db.?cpu/i.test(d.name) ? 's/s · average active sessions' : d.kind === 'time_model' || d.field === 'total_wait_time_s' ? 'seconds' : d.field === 'per_second' ? 'per second' : d.field || 'value';
    add(lineModel(d.series,'value',d.name || t('Metric timeline'),unit));
  } else if (view.tool === 'get_database_load_summary') {
    if (Array.isArray(d.busiest_snapshots_by_db_time)) {
      add({type:'table',title:t('Highest-load intervals'),columns:[t('Snapshot'),t('Database time'),'DB Time [s]','CPU [s]',t('Leading wait'),'DB Time [%]'],rows:d.busiest_snapshots_by_db_time.map(r=>[r.snap_id,r.begin_snap_time,r.db_time_s,r.db_cpu_s,r.top_fg_event?.event,r.top_fg_event?.pct_dbtime].map(v=>v??null)),note:t('Ranked intervals, not a timeline. Records without instance identity are not assigned to RAC nodes.')});
    }
    if (Array.isArray(d.top_foreground_waits_by_total_time)) add({...bars('Foreground waits across the collection',d.top_foreground_waits_by_total_time,'total_wait_time_s','seconds'),note:t('Collection totals identify workload priorities, not the cause of an incident.')});
  } else if (view.tool === 'top_wait_events_in_snapshot' && Array.isArray(d.items)) {
    const field=d.rank_by || 'pct_dbtime'; add(bars('Wait events',d.items,field,field==='pct_dbtime'?'% DB Time':field==='total_wait_time_s'?'seconds':field));
  } else if (view.tool === 'top_sqls_in_snapshot' && Array.isArray(d.items)) {
    const fields={elapsed_time:['elapsed_time_s','Elapsed [s]'],cpu_time:['cpu_time_s','CPU [s]'],io_time:['io_time_s','I/O [s]'],buffer_gets:['buffer_gets','Buffer gets'],physical_reads:['physical_reads','Physical reads']};
    const [field,label]=fields[d.metric] || fields.elapsed_time;
    add({type:'table',title:t('SQL in focus'),columns:['SQL ID',t(label),t('Executions'),t('Module')],rows:d.items.map(r=>[r.sql_id,r[field]??null,r.executions??null,r.sql_module||r.module||'—']),selections:d.items.map(r=>({snap_id:d.snap_id,sql_id:r.sql_id,...(identity(r)!=null?{instance:String(identity(r))}:{})}))});
  } else if (view.tool === 'get_sql_text' && typeof d.sql_text === 'string') {
    add({type:'sql',title:d.sql_id || t('SQL text'),text:d.sql_text});
  } else if (view.tool === 'compare_snapshots') {
    for (const [key,title,columns,fields] of [
      ['load_profile_deltas','Workload: baseline vs incident',['Metric','Baseline /s','Incident /s','Delta /s'],['stat_name','a_per_sec','b_per_sec','delta']],
      ['fg_wait_event_deltas','Wait share: baseline vs incident',['Wait','Baseline [%]','Incident [%]','Delta [pp]'],['event','a_pct_dbtime','b_pct_dbtime','delta_pct_dbtime']],
      ['sql_elapsed_deltas','SQL elapsed: baseline vs incident',['SQL ID','Baseline [s]','Incident [s]','Delta [s]'],['sql_id','a_elapsed_s','b_elapsed_s','delta_elapsed_s']]
    ]) if (Array.isArray(d[key])) add({type:'table',title:t(title),columns:columns.map(t),rows:d[key].map(r=>fields.map(f=>r[f]??null)),note:`${t('Baseline')} #${d.snap_a?.snap_id ?? view.args?.snap_id_a}, ${t('incident')} #${d.snap_b?.snap_id ?? view.args?.snap_id_b}. ${t('Upstream deltas may substitute zero for absent TOP entries; verify original measurements before interpreting zeros or ratios. Snapshot-only lookups do not establish RAC instance identity.')}`});
  } else if (view.tool === 'get_snapshot_details') {
    if (Array.isArray(d.foreground_waits)) add(bars('Foreground waits in this report',d.foreground_waits.slice(0,10),'pct_dbtime','% DB Time'));
    if (Array.isArray(d.time_model)) add(bars('Time model',d.time_model,'time_s','seconds','stat_name'));
  }
  return {title:d.event_name || (d.sql_id ? `SQL ${d.sql_id}` : d.name) || t(({get_database_load_summary:'Where should we look first?',get_snapshot_details:'Inside the selected interval',top_wait_events_in_snapshot:'What is the database waiting for?',top_sqls_in_snapshot:t('SQL in focus')})[view.tool] || 'Evidence collected'),panels:panels.slice(0,3)};
}

export const seriesPoints = data => (Array.isArray(data?.series)?data.series:[]).filter(p=>finite(p.value));
export const unitFor = data => automaticPresentation({tool:'get_metric_time_series',data:{...data,series:data.series || []}}).panels[0]?.unit || 'value';
const node = (tag,text,cls) => {const el=document.createElement(tag);if(text!==undefined)el.textContent=text;if(cls)el.className=cls;return el;};
const svgNode=(tag,attrs={},text)=>{const e=document.createElementNS('http://www.w3.org/2000/svg',tag);for(const[k,v]of Object.entries(attrs))e.setAttribute(k,v);if(text!==undefined)e.textContent=text;return e;};
export const formatNumber = (n, language = 'en') => finite(n) ? n.toLocaleString(language === 'pl' ? 'pl-PL' : 'en-US', {maximumFractionDigits:2}) : '—';
const colors=['#70d8be','#e8ba7b','#aab5ff','#ee9eb4','#8ecbf0','#d2ce95'];
function line(container,p,onSelect,language) {
  const t = text => translate(language, text), fmt = n => formatNumber(n, language);
  const all=p.series.flatMap(s=>s.points), measured=all.filter(v=>finite(v.value));
  if(!measured.length){container.append(node('p',t('No retained measurements. Missing does not mean zero.'),'empty'));return;}
  const controls=node('div',undefined,'chart-controls');
  const label=node('label',t('Window ')), select=node('select');select.setAttribute('aria-label',`${p.title}: ${t('time window')}`);
  for(const [v,labelText]of [['all',t('All observations')],['peak',t('Around the peak')]]){const option=node('option',labelText);option.value=v;select.append(option);}label.append(select);controls.append(label);
  const readout=node('p',t('Hover or focus a point to inspect it. Select it to set the conversation context.'),'chart-readout');
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
  container.append(node('p',`${t('Measurements')}: ${fmt(measured.length)}${missing?` · ${t('Missing')}: ${fmt(missing)}`:''} · ${t('Peak')}: ${fmt(peak.value)} ${p.unit} · ${peak.label}`,'chart-note'));
}
export function renderPresentation(container,board,onSelect,language = 'en'){
  const t = text => translate(language, text), fmt = n => formatNumber(n, language);
  container.replaceChildren();
  if(board.takeaway)container.append(node('p',board.takeaway,'takeaway'));
  if(!board.panels.length){container.append(node('p',t('Evidence collected. Ask for a focused comparison or explanation to bring the relevant findings into this panel.'),'empty'));return;}
  for(const p of board.panels){
    const card=node('section',undefined,'insight-card');
    const heading=node('div',undefined,'card-heading');heading.append(node('h3',p.title),node('span',t(p.classification),p.classification==='hypothesis'?'classification hypothesis':'classification'));card.append(heading);
    if(p.unit)card.append(node('p',p.unit,'unit'));
    if(p.type==='line')line(card,p,onSelect,language);
    else if(p.type==='bar'){
      const max=Math.max(1,...p.items.map(v=>finite(v.value)?v.value:0));
      for(const v of p.items){const row=node('div',undefined,'bar-row'),head=node('div',undefined,'bar-top');head.append(node('span',v.label),node('span',`${fmt(v.value)} ${p.unit}`));const track=node('div',undefined,'bar-track'),fill=node('div',undefined,'bar-fill');fill.style.width=`${finite(v.value)?Math.max(0,v.value)/max*100:0}%`;track.append(fill);row.append(head,track);card.append(row);}
      if(!p.items.length)card.append(node('p',t('No measurements returned.'),'empty'));
    }else if(p.type==='table'){
      const wrap=node('div',undefined,'table-wrap'),table=node('table'),thead=node('thead'),tr=node('tr');for(const c of p.columns)tr.append(node('th',c));thead.append(tr);table.append(thead);const body=node('tbody');
      p.rows.forEach((r,i)=>{const row=node('tr');r.forEach((v,j)=>{const cell=node('td');if(j===0&&p.selections?.[i]){const b=node('button',String(v??'—'));b.onclick=()=>onSelect(p.selections[i]);cell.append(b);}else cell.textContent=typeof v==='number'?fmt(v):v??'—';row.append(cell);});body.append(row);});table.append(body);wrap.append(table);card.append(wrap);
    }else if(p.type==='flow'){
      const flow=node('ol',undefined,'mechanism');for(const step of p.steps){const item=node('li');item.append(node('strong',step.title),node('p',step.detail));flow.append(item);}card.append(flow);
    }else if(p.type==='sql'){card.append(node('pre',p.text,'sql-text'));}
    if(p.note)card.append(node('p',p.note,'caveat'));
    card.append(node('p',`${t('Source')} · ${p.source}`,'source'));container.append(card);
  }
}
export function renderChart(container,view,onSelect,language = 'en'){renderPresentation(container,localizedView(view, language).presentation || automaticPresentation(view, language),onSelect,language);}

// Only presentation copy is translatable. Measurements, table cells, SQL, point
// timestamps/labels, instance identities and provenance remain source data.
export function viewTexts(view) {
  const fields = [];
  const add = (path, text, maxLength, language = view.language || 'en') => {
    if (typeof text === 'string' && text) fields.push({ path, text, maxLength, language });
  };
  add('note', view.note, 2000, view.noteLanguage || view.language || 'en');
  const b = view.presentation;
  if (!b) return fields;
  add('presentation.title', b.title, 180); add('presentation.takeaway', b.takeaway, 1200);
  b.panels.forEach((p, i) => {
    const path = `presentation.panels.${i}`;
    for (const [key, max] of [['title',180],['note',1500],['unit',100],['x_label',100]]) add(`${path}.${key}`, p[key], max);
    p.columns?.forEach((text,j) => add(`${path}.columns.${j}`, text, 100));
    p.steps?.forEach((step,j) => { add(`${path}.steps.${j}.title`, step.title, 100); add(`${path}.steps.${j}.detail`, step.detail, 500); });
    p.series?.forEach((series,j) => add(`${path}.series.${j}.name`, series.name, 180));
    p.items?.forEach((item,j) => add(`${path}.items.${j}.label`, item.label, 180));
  });
  return fields;
}
export function localizedView(view, language = 'en') {
  const result = { ...view, ...(view.presentation ? { presentation: structuredClone(view.presentation) } : {}) };
  for (const {path} of viewTexts(view)) {
    const text = view.translations?.[language]?.[path];
    if (text === undefined) continue;
    const parts = path.split('.'), key = parts.pop();
    let target = result;
    for (const part of parts) target = target[part];
    target[key] = text;
  }
  return result;
}
export function translationStatus(view, language = 'en') {
  return { language, view_id: view?.id ?? null, pending: view ? viewTexts(view).filter(f =>
    f.language !== language && view.translations?.[language]?.[f.path] === undefined &&
    // Automatically generated panels are localized by adapters at presentation time.
    !view.automaticTextPaths?.includes(f.path)) : [] };
}
export const translate = (language, text) => language === 'pl' ? (POLISH[text] ?? text) : text;
const POLISH = {
  'Instance unspecified': 'Instancja nieokreślona', 'Database time': 'Czas bazy danych', 'Snapshot': 'Snapshot',
  'Multiple records share a timestamp without distinct instance IDs. Points are shown separately; no RAC instance is inferred and no totals are added.': 'Wiele rekordów ma ten sam czas bez odrębnych identyfikatorów instancji. Punkty pokazujemy osobno, bez przypisywania instancji RAC ani sumowania.',
  'Missing observations are gaps, not zero.': 'Brakujące obserwacje pozostają lukami — nie oznaczają zera.',
  'Wait time': 'Czas oczekiwania', 'Average wait duration': 'Średni czas oczekiwania',
  'seconds': 'sekundy', 'milliseconds': 'milisekundy', 'per second': 'na sekundę', 'value': 'wartość',
  's/s · average active sessions': 's/s · średnia liczba aktywnych sesji',
  'SQL elapsed': 'Czas wykonania SQL', 'SQL CPU': 'Czas CPU SQL', 'SQL I/O': 'Czas I/O SQL',
  'Buffer gets': 'Odczyty buforów', 'Physical reads': 'Odczyty fizyczne', 'gets': 'odczyty buforów', 'reads': 'odczyty',
  'Metric timeline': 'Przebieg metryki', 'Highest-load intervals': 'Przedziały największego obciążenia',
  'Leading wait': 'Główne oczekiwanie',
  'Ranked intervals, not a timeline. Records without instance identity are not assigned to RAC nodes.': 'Ranking przedziałów, nie oś czasu. Rekordy bez identyfikatora instancji nie są przypisywane do węzłów RAC.',
  'Foreground waits across the collection': 'Oczekiwania sesji użytkowników w całym zbiorze',
  'Collection totals identify workload priorities, not the cause of an incident.': 'Sumy dla całego zbioru wskazują priorytety analizy obciążenia, ale nie przyczynę incydentu.',
  'Wait events': 'Zdarzenia oczekiwania', 'Elapsed [s]': 'Czas wykonania [s]', 'SQL in focus': 'Analizowany SQL',
  'Executions': 'Wykonania', 'Module': 'Moduł', 'SQL text': 'Tekst SQL',
  'Workload: baseline vs incident': 'Obciążenie: okres odniesienia a incydent',
  'Wait share: baseline vs incident': 'Udział oczekiwań: okres odniesienia a incydent',
  'SQL elapsed: baseline vs incident': 'Czas wykonania SQL: okres odniesienia a incydent',
  'Metric': 'Metryka', 'Wait': 'Oczekiwanie', 'Baseline /s': 'Odniesienie /s', 'Incident /s': 'Incydent /s', 'Delta /s': 'Różnica /s',
  'Baseline [%]': 'Odniesienie [%]', 'Incident [%]': 'Incydent [%]', 'Delta [pp]': 'Różnica [pp]',
  'Baseline [s]': 'Odniesienie [s]', 'Incident [s]': 'Incydent [s]', 'Delta [s]': 'Różnica [s]',
  'Baseline': 'Okres odniesienia', 'incident': 'incydent',
  'Upstream deltas may substitute zero for absent TOP entries; verify original measurements before interpreting zeros or ratios. Snapshot-only lookups do not establish RAC instance identity.': 'Dane źródłowe mogą zastępować brak wpisu TOP zerem. Sprawdź oryginalne pomiary przed interpretacją zer i ilorazów. Sam numer snapshotu nie identyfikuje instancji RAC.',
  'Foreground waits in this report': 'Oczekiwania sesji użytkowników w tym raporcie', 'Time model': 'Model czasu',
  'Where should we look first?': 'Od czego zacząć analizę?', 'Inside the selected interval': 'Wybrany przedział',
  'What is the database waiting for?': 'Na co czeka baza danych?', 'Evidence collected': 'Zebrane dane',
  'No retained measurements. Missing does not mean zero.': 'Brak zachowanych pomiarów. Brak danych nie oznacza zera.',
  'Window ': 'Zakres ', 'time window': 'zakres czasu', 'All observations': 'Wszystkie obserwacje', 'Around the peak': 'Wokół maksimum',
  'Hover or focus a point to inspect it. Select it to set the conversation context.': 'Najedź na punkt lub przejdź do niego klawiaturą, aby zobaczyć szczegóły. Wybierz go, aby ustawić kontekst rozmowy.',
  'Measurements': 'Pomiary', 'Missing': 'Brakujące', 'Peak': 'Maksimum',
  'Evidence collected. Ask for a focused comparison or explanation to bring the relevant findings into this panel.': 'Dane zebrane. Poproś o konkretne porównanie lub wyjaśnienie, aby zobaczyć tutaj istotne wyniki.',
  'measured': 'pomiar', 'hypothesis': 'hipoteza', 'explanation': 'wyjaśnienie', 'No measurements returned.': 'Nie zwrócono pomiarów.', 'Source': 'Źródło',
  'Connecting…': 'Łączenie…', 'Hooded JAS-MIN guide': 'Przewodniczka JAS-MIN w kapturze',
  'YOUR GUIDE TO THE DATA': 'TWOJA PRZEWODNICZKA PO DANYCH', 'Let’s make sense of the evidence.': 'Zrozummy razem zebrane dane.',
  'Checking the evidence…': 'Sprawdzam dane…', 'Ready when you are': 'Gotowa do analizy',
  'Voice in Codex': 'Rozmowa głosowa w Codex', 'Start a voice conversation in this chat. The evidence we discuss will appear here.': 'Rozpocznij rozmowę głosową w tym czacie. Omawiane dane pojawią się tutaj.',
  'IN FOCUS': 'W CENTRUM UWAGI', 'Earlier views': 'Wcześniejsze widoki', 'Evidence history': 'Historia analizy',
  'Technical evidence · raw data': 'Dane techniczne · surowe wyniki', 'LOCAL PANEL · MCP EVIDENCE': 'LOKALNY PANEL · DANE Z MCP',
  'Energy brightens with tool activity': 'Energia rozświetla się podczas pracy narzędzi',
  'DEMO · SYNTHETIC DATA': 'DEMO · DANE SYNTETYCZNE', 'WAITING FOR CODEX': 'OCZEKIWANIE NA CODEX',
  'Select a point to focus the conversation': 'Wybierz punkt jako kontekst rozmowy', 'ANALYSIS BOARD': 'PLANSZA ANALIZY', 'MEASURED EVIDENCE': 'DANE POMIAROWE',
  'What should we investigate?': 'Co przeanalizujemy?', 'Useful comparisons and explanations will appear here as we investigate.': 'W trakcie analizy pojawią się tutaj przydatne porównania i wyjaśnienia.',
  'Panel unavailable. Reopen it with companion_open.': 'Panel niedostępny. Otwórz go ponownie przez companion_open.', 'Panel connection lost': 'Utracono połączenie z panelem',
  'The action could not be completed.': 'Nie udało się wykonać tej czynności.'
};
