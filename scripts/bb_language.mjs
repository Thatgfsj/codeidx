// MCP blackbox for newly registered source languages.
import { cpSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { once } from 'node:events';
import { McpStdioClient, expect, expectEq } from './bb_client.mjs';

const [bin, fixture, ...cases] = process.argv.slice(2);
if (!bin || !fixture || !cases.length) throw new Error('usage: bb_language.mjs <bin> <fixture-dir> <file:symbol:line>...');
const path = mkdtempSync(join(tmpdir(), 'codeidx-language-'));
let c;
try {
  cpSync(resolve(fixture), path, { recursive: true });
  c = new McpStdioClient(resolve(bin));
  await c.initialize();
  const m = await c.tool('mount_project', { path });
  expect(m.json?.mounted, 'mount: ' + m.text);
  const map = await c.tool('get_repo_map', { path });
  for (const entry of cases) {
    const [file_path, symbol, line] = entry.split(':');
    const outline = await c.tool('get_file_outline', { path, file_path });
    expect(outline.text.includes('	' + symbol + '	' + line + '	'), 'outline: ' + outline.text);
    expect(map.text.split(String.fromCharCode(10)).some(row => row.startsWith(file_path + ':' + line + ' ') && row.endsWith(' ' + symbol)), 'RepoMap: ' + entry);
    const search = await c.tool('search_code', { path, mode: 'keyword', top_k: 10, query: symbol });
    expect(search.json?.results?.includes(file_path + '	' + symbol + '	' + line + '	'), 'search: ' + search.text);
  }
  expectEq((await c.tool('unmount_project', { path })).json?.unmounted, true, 'unmount');
  const closed = once(c.proc, 'close', { signal: AbortSignal.timeout(10000) });
  c.proc.stdin.end();
  expectEq((await closed)[0], 0, 'exit on EOF');
  expectEq(c.stdoutPollution.length, 0, 'stdout must be JSON-RPC only');
  console.log('PASS mount, outline, RepoMap and keyword search: ' + cases.join(', '));
} finally {
  if (c && !c.exited) await c.kill();
  rmSync(path, { recursive: true, force: true });
}
