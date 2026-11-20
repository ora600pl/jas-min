"""Local MCP and Companion processes for guided audit restart tests."""
import asyncio
from contextlib import asynccontextmanager
import json
import os
from pathlib import Path
import socket

REPO = Path(__file__).resolve().parents[1]


class Panel:
    def __init__(self, proc):
        self.proc = proc
        self.seq = 0

    async def rpc(self, method, params):
        self.seq += 1
        self.proc.stdin.write((json.dumps(dict(jsonrpc='2.0', id=self.seq, method=method, params=params)) + '\n').encode())
        await self.proc.stdin.drain()
        line = await asyncio.wait_for(self.proc.stdout.readline(), 180)
        if not line:
            raise RuntimeError('Companion closed its transport')
        result = json.loads(line)
        if result.get('error') or result.get('id') != self.seq:
            raise RuntimeError(str(result.get('error', 'Mismatched response')))
        return result['result']

    async def call(self, name, **args):
        result = await self.rpc('tools/call', {'name': 'companion_' + name, 'arguments': args})
        if result.get('isError'):
            raise RuntimeError(name + ': ' + result['content'][0]['text'][:1200])
        return result['structuredContent']


@asynccontextmanager
async def processes(args, scratch, run):
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    address = f'127.0.0.1:{port}'
    with (scratch / f'core-{run}.log').open('wb') as log, (scratch / f'panel-{run}.log').open('wb') as panel_log:
        core = await asyncio.create_subprocess_exec(
            args.binary, '--mcp', address + '/mcp', '-s', f'{args.first}-{args.last + 1}',
            *[v for capture in args.captures for v in ('-j', capture)], cwd=scratch, stdout=log, stderr=log,
            env={**os.environ, 'JASMIN_AUDIT_WORKSPACE': str(args.workspace)})
        panel = None
        try:
            async with asyncio.timeout(360):
                while True:
                    if core.returncode is not None:
                        detail = Path(log.name).read_text(errors='replace')[-800:]
                        raise RuntimeError(f'MCP exited {core.returncode}: {detail}')
                    try:
                        _, connection = await asyncio.open_connection('127.0.0.1', port)
                        connection.close()
                        await connection.wait_closed()
                        break
                    except OSError:
                        await asyncio.sleep(.25)
            panel = await asyncio.create_subprocess_exec(
                'node', args.companion, cwd=scratch,
                stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                stderr=panel_log, limit=64 * 1024 * 1024)
            client = Panel(panel)
            await client.rpc('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                                            'clientInfo': {'name': 'pilot-restart-test', 'version': '1'}})
            opened = await client.call('open', endpoint=f'http://{address}/mcp', language='pl')
            client.panel_url = opened['panel_url']
            yield client, [core.pid, panel.pid]
        finally:
            owned = [p for p in (panel, core) if p is not None]
            for proc in owned:
                if proc.returncode is None:
                    proc.kill()
            await asyncio.gather(*(proc.wait() for proc in owned))
