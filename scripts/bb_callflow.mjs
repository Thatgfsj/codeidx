// Usage: node scripts/bb_callflow.mjs <codeidx-mcp>
import { cpSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { McpStdioClient, expect, expectEq } from './bb_client.mjs';

if (!process.argv[2]) throw Error('pass the MCP binary path');
const path = mkdtempSync(join(tmpdir(), 'codeidx-callflow-'));
const c = new McpStdioClient(resolve(process.argv[2]));
const trace = async args => {
  const r = await c.tool('trace_calls', { path, refine_lsp: false, ...args });
  expect(!r.isError && r.json, 'trace JSON: ' + r.text);
  return r.json;
};
const child = (node, name) => node?.children.find(n => n.location.symbol === name);
const root = (report, name) => report.roots.find(n => n.location.symbol === name);
const write = (name, text) => writeFileSync(join(path, 'src', name), text);
const sync = async () => {
  const r = await c.tool('sync_project', { path });
  expect(!r.isError, 'sync: ' + r.text);
};

try {
  cpSync(fileURLToPath(new URL('../tests/fixtures/sample-project/', import.meta.url)), path, { recursive: true });
  await c.initialize();
  const catalog = (await c.call('tools/list')).result.tools;
  expectEq(catalog.length, 12, 'catalog length');
  expectEq(catalog.find(t => t.name === 'trace_calls').inputSchema.properties.format.default, 'compact', 'default format');
  const missing = await c.toolAs('trace_calls', { path });
  expect(missing.isError && !missing.rpcError && missing.text.startsWith('error:'), 'unmounted tool error');
  const mounted = await c.tool('mount_project', { path, auto_sync: false });
  expect(!mounted.isError, 'mount: ' + mounted.text);

  const main = root(await trace({}), 'main');
  for (const name of ['forge_config', 'unicorn_marker_fn', 'helper_spinner']) {
    expect(child(main, name), 'main directly calls ' + name);
  }
  const from = child(main, 'unicorn_marker_fn').location;
  const subtree = await trace({ from });
  expectEq(subtree.roots.length, 1, 'one continuation root');
  expect(child(root(subtree, 'unicorn_marker_fn'), 'helper_spinner'), 'continuation');
  const reverse = await trace({ symbol: 'helper_spinner', direction: 'up' });
  expect(child(root(reverse, 'helper_spinner'), 'main'), 'main is a caller');
  const both = await trace({ from, direction: 'both' });
  expect(child(both.roots[0], 'main') && child(both.roots[0], 'helper_spinner'), 'both directions');

  const raw = async format => (await c.call('tools/call', { name: 'trace_calls', arguments: {
    path, from, refine_lsp: false, ...(format ? { format } : {}),
  } })).result;
  const compact = await raw();
  const json = await raw('json');
  const markdown = await raw('md');
  expect(compact.content[0].text.startsWith('flow '), 'default compact');
  expectEq(JSON.parse(json.content[0].text).roots[0].location.symbol, from.symbol, 'explicit JSON');
  for (const r of [compact, json, markdown]) {
    expect(!r.isError && !Object.hasOwn(r, 'structuredContent'), 'only chosen format');
    expectEq(r.content.length, 1, 'single body');
    expectEq(r.content[0].type, 'text', 'text body');
  }
  const fence = String.fromCharCode(96).repeat(3);
  expectEq(markdown.content[0].text, fence + 'text\n' + compact.content[0].text.trimEnd() + '\n' + fence + '\n', 'Markdown uses compact');

  write('cycle.rs', 'pub fn bb_affine() {\n    bb_cycle();\n}\npub fn bb_cycle() {\n    bb_affine();\n}\n');
  write('one.rs', 'pub fn bb_ambiguous() {}\n');
  write('caller.rs', 'pub fn bb_caller() {\n    bb_ambiguous();\n}\n');
  await sync();
  const cycle = root(await trace({ symbol: 'bb_affine' }), 'bb_affine');
  expect(child(child(cycle, 'bb_cycle'), 'bb_affine')?.cycle, 'recursive edge');
  const depth = await trace({ symbol: 'bb_affine', max_depth: 0 });
  expect(depth.truncated.depth > 0 && depth.roots[0].children.length === 0, 'depth truncation');
  const budget = await trace({ symbol: 'bb_affine', max_nodes: 1 });
  expectEq(budget.nodes, 1, 'max_nodes');
  expect(budget.truncated.nodes > 0, 'node truncation');
  const candidates = async () => root(await trace({ symbol: 'bb_caller' }), 'bb_caller').children;
  expectEq((await candidates()).filter(n => n.location.symbol === 'bb_ambiguous').length, 1, 'one original definition');
  write('two.rs', 'pub fn bb_ambiguous() {}\n');
  await sync();
  const ambiguous = (await candidates()).filter(n => n.location.symbol === 'bb_ambiguous');
  expect(ambiguous.some(n => n.confidence === 'ambiguous' && n.candidates === 2), 'other file changes old caller');
  rmSync(join(path, 'src', 'one.rs'));
  await sync();
  const after = (await candidates()).filter(n => n.location.symbol === 'bb_ambiguous');
  expectEq(after.length, 1, 'removed definition drops stale edge');
  expectEq(after[0].location.file, 'src/two.rs', 'surviving definition');
  expectEq(c.stdoutPollution.length, 0, 'stdout only JSON-RPC');
  console.log('PASS discovery, directions, continuation, formats, cycles, budgets and cross-file sync');
} finally {
  await c.kill();
  rmSync(path, { recursive: true, force: true });
}
