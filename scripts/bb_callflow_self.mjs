// Usage: node scripts/bb_callflow_self.mjs <codeidx-mcp>
import { once } from 'node:events';
import { realpathSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { McpStdioClient, expect, expectEq } from './bb_client.mjs';

if (!process.argv[2]) throw Error('pass the MCP binary path');
const path = realpathSync(fileURLToPath(new URL('../', import.meta.url)));
const c = new McpStdioClient(resolve(process.argv[2]));
const trace = async (symbol, file_path, direction = 'down', refine_lsp = false) => {
  const result = await c.tool('trace_calls', {
    path, symbol, file_path, direction, max_depth: 1, max_nodes: 128, budget: 3200, refine_lsp,
  });
  expect(!result.isError && result.json, symbol + ': ' + result.text);
  return result.json.roots[0].children;
};
const lspBridge = 'crates/codeidx-lsp/src/bridge.rs';
const rpcServer = 'crates/codeidx-mcp/src/server.rs';
const rpcTypes = 'crates/codeidx-protocol/src/jsonrpc.rs';
try {
  await c.initialize();
  const mount = await c.tool('mount_project', { path, auto_sync: false });
  expect(!mount.isError, mount.text);
  const bad = (await trace('from_env', lspBridge)).filter(n => n.location.symbol === 'ok');
  expect(bad.length >= 2, 'the two Result::ok calls are visible');
  expect(bad.every(n => n.location.file === lspBridge && !n.confidence),
    'Result methods must not point to Outgoing::ok: ' + JSON.stringify(bad));
  const real = await trace('dispatch_request', rpcServer);
  expect(real.some(n => n.location.file === rpcServer && n.location.symbol === 'handle_tools_call'),
    'normal direct call still resolves');
  expect(!real.some(n => n.location.symbol === 'request' && n.callback), 'a request variable is not a callback');
  const callers = await trace('ok', rpcTypes, 'up');
  expect(!callers.some(n => n.location.file === lspBridge && n.location.symbol === 'from_env'), 'no invented reverse edge');
  if (process.argv.includes('--lsp')) {
    const resolved = await trace('dispatch_request', rpcServer, 'down', true);
    expectEq(resolved.filter(n => n.location.file === rpcTypes && n.location.symbol === 'ok' && n.confidence === 'lsp').length,
      3, 'LSP confirms all three qualified cross-crate calls');
    const reverse = await trace('ok', rpcTypes, 'up', true);
    expect(reverse.some(n => n.location.file === rpcServer && n.location.symbol === 'dispatch_request'), 'real cross-crate reverse edge');
    expect(!reverse.some(n => n.location.file === lspBridge && n.location.symbol === 'from_env'), 'no invented semantic reverse edge');
  }
  expectEq(c.stdoutPollution.length, 0, 'stdio');
  const close = once(c.proc, 'close', { signal: AbortSignal.timeout(10000) });
  c.proc.stdin.end();
  expectEq((await close)[0], 0, 'EOF exit');
  console.log('PASS Codeidx self-index: receiver collisions, direct calls and reverse edges');
} finally {
  if (!c.exited) await c.kill();
}
