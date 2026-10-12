import { finite } from './public/charts.js';
const str = (s, max=500) => typeof s==='string' && s.trim().length>0 && s.length<=max;
const fail = message => { throw new Error(`Invalid presentation: ${message}`); };
export function validatePresentation(board) {
  if (!board || !str(board.title,180) || !str(board.takeaway,1200)) fail('title and takeaway are required.');
  if (!Array.isArray(board.panels) || !board.panels.length || board.panels.length>6) fail('provide 1–6 panels.');
  if (JSON.stringify(board).length>700000) fail('payload exceeds 700 KB; narrow the window.');
  for (const p of board.panels) {
    if (!str(p.title,180) || !str(p.source,1000)) fail('each panel needs a title and a source/evidence reference.');
    if (!['measured','hypothesis','explanation'].includes(p.classification)) fail('classify each panel as measured, hypothesis or explanation.');
    if (p.note!==undefined && !str(p.note,1500)) fail('invalid panel note.');
    if (['line','bar'].includes(p.type) && !str(p.unit,100)) fail('charts require explicit units.');
    if (p.type==='line') {
      if (!Array.isArray(p.series) || !p.series.length || p.series.length>6) fail('line charts need 1–6 series.');
      if (!str(p.x_label,100)) fail('line charts require x_label.');
      for (const s of p.series) {
        if (!str(s.name,180) || !Array.isArray(s.points) || s.points.length>3000) fail('invalid series (max 3000 points).');
        let previous=-Infinity;
        for (const v of s.points) {
          if (!finite(v.x) || v.x<=previous || !(v.value===null || finite(v.value)) || !str(v.label,180)) fail('points need strictly increasing numeric x, label, and finite value or null. Separate RAC instances into series.');
          if (v.snap_id!==undefined && !Number.isInteger(v.snap_id)) fail('snap_id must be an integer.');
          if (v.instance!==undefined && !str(v.instance,100)) fail('instance must be a source-verified name.');
          previous=v.x;
        }
      }
    } else if (p.type==='bar') {
      if (!Array.isArray(p.items) || p.items.length>30 || p.items.some(v=>!str(v.label,180) || !(v.value===null || finite(v.value)) || (finite(v.value)&&v.value<0))) fail('bars need up to 30 nonnegative measurements or null.');
    } else if (p.type==='table') {
      if (!Array.isArray(p.columns) || !p.columns.length || p.columns.length>8 || p.columns.some(v=>!str(v,100)) || !Array.isArray(p.rows) || p.rows.length>100 || p.rows.some(r=>!Array.isArray(r)||r.length!==p.columns.length||r.some(v=>!(v===null||finite(v)||str(v,1000))))) fail('invalid table (max 8 columns, 100 rows).');
    } else if (p.type==='flow') {
      if (!Array.isArray(p.steps) || p.steps.length<2 || p.steps.length>8 || p.steps.some(s=>!str(s.title,100)||!str(s.detail,500))) fail('flow needs 2–8 titled steps with details.');
      if (p.classification==='measured') fail('mechanism diagrams must be labelled explanation or hypothesis.');
    } else fail('supported panel types: line, bar, table, flow.');
  }
  return structuredClone(board);
}
const text={type:'string'};
export const presentationProperties = {
  title:{type:'string',description:'Question or finding in the current dialogue language addressed by the board.'},
  takeaway:{type:'string',description:'Short conclusion in the current dialogue language; distinguish evidence from hypothesis.'},
  view_ids:{type:'array',items:text,description:'Optional existing view IDs to render together using automatic adapters. Up to 6 panels in total.'},
  panels:{type:'array',description:'Optional custom visual panels from verified evidence or a clearly labelled mechanism. No HTML or scripts.',items:{type:'object',properties:{
    type:{type:'string',enum:['line','bar','table','flow']},title:text,classification:{type:'string',enum:['measured','hypothesis','explanation']},source:{type:'string',description:'Evidence IDs or exact local artifact and extraction/derivation description. Required.'},note:text,unit:text,x_label:text,
    series:{type:'array',items:{type:'object',properties:{name:text,points:{type:'array',items:{type:'object',properties:{x:{type:'number'},label:text,value:{type:['number','null']},snap_id:{type:'integer'},instance:text},required:['x','label','value']}}},required:['name','points']}},
    items:{type:'array',items:{type:'object',properties:{label:text,value:{type:['number','null']}},required:['label','value']}},
    columns:{type:'array',items:text},rows:{type:'array',items:{type:'array',items:{type:['string','number','null']}}},
    steps:{type:'array',items:{type:'object',properties:{title:text,detail:text},required:['title','detail']}}
  },required:['type','title','classification','source']}}
};
