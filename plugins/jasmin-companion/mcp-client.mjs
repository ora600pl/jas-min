import manifest from './package.json' with { type: 'json' };
// Minimal Streamable HTTP client: JSON and SSE, including stateful sessions.
export class McpClient {
  constructor(url) {
    this.url = url;
    this.sequence = 0;
    this.session = null;
    this.protocol = '2025-06-18';
  }
  async rpc(method, params = {}, notification = false) {
    // Checkpoints include retained evidence and are returned in both MCP content forms.
    const auditPayload = method === 'tools/call' && ['export_audit', 'get_audit_status', 'start_audit', 'record_audit_brief', 'advance_audit', 'authorize_report', 'restore_audit'].includes(params.name);
    const maxResponseBytes = (auditPayload ? 192 : 8) * 1024 * 1024;
    const id = notification ? undefined : ++this.sequence;
    const headers = { 'Content-Type': 'application/json', Accept: 'application/json, text/event-stream' };
    if (this.session) headers['Mcp-Session-Id'] = this.session;
    if (method !== 'initialize') headers['MCP-Protocol-Version'] = this.protocol;
    const response = await fetch(this.url, {
      method: 'POST', headers, redirect: 'error', signal: AbortSignal.timeout(method === 'tools/call' && params.name === 'restore_audit' ? 120000 : 45000),
      body: JSON.stringify({ jsonrpc: '2.0', ...(notification ? {} : { id }), method, params }),
    });
    if (!response.ok) throw new Error(`MCP HTTP ${response.status}. Check the server and session.`);
    this.session = response.headers.get('mcp-session-id') || this.session;
    if (notification) { await response.body?.cancel(); return; }
    let message;
    if (response.headers.get('content-type')?.includes('text/event-stream')) {
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = '', bytes = 0;
      try {
        outer: while (true) {
          const { value, done } = await reader.read();
          if (done) break;
          bytes += value.length;
          if (bytes > maxResponseBytes) throw new Error(`MCP response exceeded the ${maxResponseBytes / 1024 / 1024} MiB limit. Narrow the query or export a smaller audit.`);
          buffer += decoder.decode(value, { stream: true });
          let match;
          while ((match = /\r?\n\r?\n/.exec(buffer))) {
            const frame = buffer.slice(0, match.index);
            buffer = buffer.slice(match.index + match[0].length);
            const data = frame.split(/\r?\n/).filter(x => x.startsWith('data:')).map(x => x.slice(5).trimStart()).join('\n');
            if (!data) continue;
            const candidate = JSON.parse(data);
            if (candidate.id === id) { message = candidate; break outer; }
          }
        }
      } finally { await reader.cancel(); }
    } else {
      const text = await response.text();
      if (Buffer.byteLength(text, 'utf8') > maxResponseBytes) throw new Error('MCP response is too large.');
      message = JSON.parse(text);
    }
    if (!message || message.id !== id) throw new Error('No matching MCP response.');
    if (message.error) throw new Error(message.error.message || 'MCP error');
    return message.result;
  }
  async initialize() {
    const result = await this.rpc('initialize', {
      protocolVersion: this.protocol, capabilities: {}, clientInfo: { name: 'jasmin-companion', version: manifest.version },
    });
    this.protocol = result.protocolVersion;
    await this.rpc('notifications/initialized', {}, true);
  }
  async tools() {
    const tools = [];
    let cursor;
    do {
      const result = await this.rpc('tools/list', cursor ? { cursor } : {});
      tools.push(...result.tools);
      cursor = result.nextCursor;
    } while (cursor);
    return tools;
  }
  async call(name, args = {}) {
    const result = await this.rpc('tools/call', { name, arguments: args });
    const value = result.structuredContent ?? JSON.parse(result.content.find(x => x.type === 'text').text);
    if (result.isError || value.error) throw new Error(typeof value.error === 'string' ? value.error : JSON.stringify(value));
    return value;
  }
  async close() {
    if (!this.session) return;
    try {
      await fetch(this.url, { method: 'DELETE', redirect: 'error', signal: AbortSignal.timeout(3000),
        headers: { 'Mcp-Session-Id': this.session, 'MCP-Protocol-Version': this.protocol } });
    } finally { this.session = null; }
  }
}
