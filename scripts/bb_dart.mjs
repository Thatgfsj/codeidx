import { cpSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { once } from 'node:events';
import { McpStdioClient, expect, expectEq } from './bb_client.mjs';

if (!process.argv[2]) throw new Error('usage: node scripts/bb_dart.mjs <binary>');
const path = mkdtempSync(join(tmpdir(), 'codeidx-dart-'));
let c;
try {
  cpSync(new URL('../tests/fixtures/dart-project', import.meta.url), path, { recursive: true });
  c = new McpStdioClient(resolve(process.argv[2]));
  await c.initialize();
  const m = await c.tool('mount_project', { path });
  expectEq(m.json?.files, 2, 'mount both Dart files');
  const map = await c.tool('get_repo_map', { path });
  expect(map.text.includes('dart_unique_marker_fn'), 'marker in RepoMap');
  const outline = await c.tool('get_file_outline', { path, file_path: 'lib.dart' });
  expect(outline.text.includes('dart_unique_marker_fn	32	'), 'marker definition on line 32');
  const search = await c.tool('search_code', { path, mode: 'keyword', top_k: 1, query: 'dart_unique_marker_fn' });
  expect(search.json?.results?.includes('lib.dart	dart_unique_marker_fn	32	'), 'first keyword hit is definition');
  const u = await c.tool('unmount_project', { path });
  expectEq(u.json?.unmounted, true, 'unmount fixture');
  const closed = once(c.proc, 'close', { signal: AbortSignal.timeout(10000) });
  c.proc.stdin.end();
  expectEq((await closed)[0], 0, 'exit on EOF');
  expectEq(c.stdoutPollution.length, 0, 'stdout remains JSON-RPC only');
  console.log('PASS Dart mount, RepoMap, outline line number and keyword search');
} finally {
  if (c && !c.exited) await c.kill();
  rmSync(path, { recursive: true, force: true });
}
