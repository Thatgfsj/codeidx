// Windows stdio/LSP smoke. Also runnable on Linux without signal-specific checks.
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { once } from 'node:events';
import { McpStdioClient, expect, expectEq } from './bb_client.mjs';

const bin = process.argv[2];
if (!bin) throw new Error('usage: node scripts/bb_windows.mjs <binary>');
const fixture = fileURLToPath(new URL('../tests/fixtures/sample-project', import.meta.url));
const sources = ['app.py', 'index.ts', 'src/lib.rs', 'src/main.rs', 'src/utils.rs', 'utils.py'];
const unsupported = ['Cargo.toml', 'README.md'];
const temp = mkdtempSync(join(tmpdir(), 'codeidx space 中文-'));
let client;
try {
  const root = realpathSync(temp);
  // Node 22's unfiltered recursive cpSync can copy into a mojibake sibling on Windows.
  // Copy the smoke inputs individually and verify the exact files before launching MCP.
  for (const file of [...sources, ...unsupported]) {
    const from = join(fixture, file);
    const to = join(root, file);
    mkdirSync(dirname(to), { recursive: true });
    copyFileSync(from, to);
    expect(readFileSync(to).equals(readFileSync(from)), 'fixture bytes differ: ' + to);
  }
  client = new McpStdioClient(resolve(bin));
  await client.initialize();
  const catalog = await client.call('tools/list', {});
  expect(catalog.result?.tools.some(entry => entry.name === 'lsp_definition'), 'lsp_definition is advertised');
  const mounted = await client.tool('mount_project', { path: root, enable_vector: false });
  expect(!mounted.isError && mounted.json?.mounted && mounted.json.symbols > 0,
    'mount verified fixture: ' + mounted.text);
  expectEq(mounted.json.files, sources.length, 'indexed source files');
  expectEq(mounted.json.skipped_unsupported, unsupported.length, 'scanned non-source files');
  const found = await client.tool('search_code', { path: root, query: 'unicorn_marker_fn', mode: 'keyword', top_k: 1 });
  expect(found.json?.mode === 'keyword' && found.json.total_candidates > 0 &&
    found.json.results?.split('\n').some(row => row.startsWith('src/lib.rs\tunicorn_marker_fn\t')),
  'keyword result from indexed src/lib.rs: ' + found.text);
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
} catch (error) {
  if (client?.stderrText.trim()) console.error(client.stderrText.trim());
  throw error;
} finally {
  if (client && !client.exited) await client.kill();
  rmSync(temp, { recursive: true, force: true });
}
