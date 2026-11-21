// Windows stdio/LSP smoke. Also runnable on Linux without signal-specific checks.
import { cpSync, mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { once } from 'node:events';
import { McpStdioClient, expect, expectEq } from './bb_client.mjs';

const bin = process.argv[2];
if (!bin) throw new Error('usage: node scripts/bb_windows.mjs <binary>');
const fixture = fileURLToPath(new URL('../tests/fixtures/sample-project', import.meta.url));
const root = realpathSync(mkdtempSync(join(tmpdir(), 'codeidx space 中文-')));
let client;
try {
  cpSync(fixture, root, { recursive: true });
  client = new McpStdioClient(resolve(bin));
  await client.initialize();
  const catalog = await client.call('tools/list', {});
  expect(catalog.result?.tools.some(entry => entry.name === 'lsp_definition'), 'lsp_definition is advertised');
  const mounted = await client.tool('mount_project', { path: root, enable_vector: false });
  expect(mounted.json?.mounted && mounted.json.files >= 6, 'mount fixture: ' + mounted.text);
  const found = await client.tool('search_code', { path: root, query: 'unicorn_marker_fn', mode: 'keyword', top_k: 1 });
  expect(found.json?.results?.includes('unicorn_marker_fn'), 'keyword result: ' + found.text);
  const definition = await client.tool('lsp_definition', {
    path: root, file_path: 'src/main.rs', line: 9, character: 19,
  }, { timeoutMs: 90000 });
  expect(definition.json?.locations.some(loc => loc.file === 'src/lib.rs' && loc.line === 27),
    'definition must use project-relative path and line: ' + definition.text);
  const unmounted = await client.tool('unmount_project', { path: root });
  expectEq(unmounted.json?.killed_lsps, 1, 'rust-analyzer retired');
  const closed = once(client.proc, 'close', { signal: AbortSignal.timeout(10000) });
  client.proc.stdin.end();
  const [code] = await closed;
  expectEq(code, 0, 'MCP exited cleanly on EOF');
  expectEq(client.stdoutPollution.length, 0, 'stdout contains only JSON-RPC');
  console.log('PASS MCP handshake, mount, keyword, Rust definition, unmount and EOF');
} finally {
  if (client && !client.exited) await client.kill();
  rmSync(root, { recursive: true, force: true });
}
