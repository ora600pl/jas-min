// Only send displayed fields to the core that owns this audit. It matches against
// the project's SQL and wait catalog and checkpoints the matched identifiers.
export function panelEntitiesByProject(board, activeProject, projects, noteProject) {
  const groups = new Map();
  function group(project) {
    if (!groups.has(project)) groups.set(project, { fields: new Set(), sql_ids: new Set(), wait_events: new Set() });
    return groups.get(project);
  }
  function add(project, ...values) {
    for (const value of values) if (typeof value === 'string' && value.trim()) group(project).fields.add(value);
  }
  const panels = board?.panels || [];
  const scopes = new Set(panels.map(panel => panel.project_id || activeProject));
  const boardProject = scopes.size === 1 ? [...scopes][0] : activeProject;
  add(boardProject, board?.title, board?.takeaway);
  add(noteProject || boardProject, board?.note);
  for (const panel of panels) {
    const project = panel.project_id || activeProject;
    add(project, panel.title, panel.note, panel.source, panel.unit, panel.x_label, panel.text, ...(panel.columns || []));
    const scopeColumn = panel.columns?.findIndex(name => /^(project([ _]id)?|projekt|instance|instancja|scope|zakres)$/i.test(name));
    const sqlColumn = panel.columns?.findIndex(name => /^sql[ _]?id$/i.test(name));
    const waitColumn = panel.columns?.findIndex(name => /^(wait( event)?( name)?|leading wait|zdarzenie oczekiwania|nazwa oczekiwania|oczekiwanie)$/i.test(name));
    for (const [i, row] of (panel.rows || []).entries()) {
      const label = String(row[scopeColumn] ?? '').trim().toLowerCase();
      const matches = label ? projects.filter(id => {
        const key = id.toLowerCase();
        return key === label || key.split(/[_-]awr[_-]/, 1)[0] === label;
      }) : [];
      const scope = panel.row_project_ids?.[i] || (matches.length === 1 ? matches[0] : project);
      add(scope, ...row, panel.selections?.[i]?.sql_id);
      const sql = row[sqlColumn]?.trim?.();
      if (sql && /^[0-9abcdfghjkmnpqrstuvwxyz]{13}$/i.test(sql)) group(scope).sql_ids.add(sql.toLowerCase());
      const wait = row[waitColumn]?.trim?.();
      if (wait && Buffer.byteLength(wait) <= 256 && !/[\u0000-\u001f\u007f]/.test(wait) &&
          !/^(?:[-–—]+|N\/A|unknown|brak|ON CPU)$/i.test(wait)) group(scope).wait_events.add(wait);
    }
    for (const item of panel.items || []) add(project, item.label);
    for (const step of panel.steps || []) add(project, step.title, step.detail);
    for (const series of panel.series || []) {
      add(project, series.name);
      for (const point of series.points) add(project, point.label, point.sql_id);
    }
  }
  return groups;
}

// Bound each request even for SQL text or a large timeline. Overlap within a
// single field keeps a name crossing a chunk boundary visible to the matcher.
export function* panelTextChunks(fields) {
  const limit = 32 * 1024;
  let chunk = '', size = 0;
  for (const field of fields) {
    const bytes = Buffer.byteLength(field);
    if (size && size + 1 + bytes > limit) {
      yield chunk;
      chunk = ''; size = 0;
    }
    if (bytes > limit) {
      // At most four UTF-8 bytes per code point.
      const chars = Array.from(field);
      for (let i = 0; i < chars.length; i += 8192 - 256) {
        yield chars.slice(i, i + 8192).join('');
        if (i + 8192 >= chars.length) break;
      }
    } else {
      chunk += (size ? '\n' : '') + field;
      size += (size ? 1 : 0) + bytes;
    }
  }
  if (size) yield chunk;
}
