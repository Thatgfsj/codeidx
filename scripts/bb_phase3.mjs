// bb_phase3.mjs — Phase 3 黑盒验收（docs/ACCEPTANCE.md §4 用例 F3.1–F3.5）
// 用法：node scripts/bb_phase3.mjs <path-to-codeidx-mcp-binary>
// 前提：本机安装 rust-analyzer；vtsls/pyright/gopls 等未安装（F3.5 用 Python 实测）。
import { cpSync, mkdtempSync, rmSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { McpStdioClient, expect, expectEq } from './bb_client.mjs';

const bin = process.argv[2];
if (!bin) { console.error('usage: node scripts/bb_phase3.mjs <bin>'); process.exit(2); }
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const fixture = join(repoRoot, 'tests/fixtures/sample-project');

const results = [];
let failed = 0;
async function run(id, name, fn) {
  try { await fn(); results.push(`PASS ${id} ${name}`); }
  catch (e) { failed++; results.push(`FAIL ${id} ${name} :: ${e.message}`); }
}

function pgrepRa() {
  try { return execFileSync('pgrep', ['-x', 'rust-analyzer'], { encoding: 'utf8' }).trim(); }
  catch { return ''; }
}

async function main() {
  const work = mkdtempSync(join(tmpdir(), 'bb3-sample-'));
  cpSync(fixture, work, { recursive: true });

  // —— 主会话：F3.1 / F3.2 / F3.5 ——
  const c = new McpStdioClient(bin);
  await c.initialize();
  await run('F3.0', 'mount fixture', async () => {
    const r = await c.tool('mount_project', { path: work });
    expect(!r.isError, 'mount ok: ' + r.text);
  });

  await run('F3.1', 'lsp_definition 跳转定义', async () => {
    const mainText = readFileSync(join(work, 'src/main.rs'), 'utf8').split('\n');
    let callLine = -1, callChar = -1;
    for (let i = 0; i < mainText.length; i++) {
      const col = mainText[i].indexOf('unicorn_marker_fn');
      if (col >= 0 && mainText[i].includes('(')) { callLine = i + 1; callChar = col; break; }
    }
    expect(callLine > 0, 'locate call site in src/main.rs');
    const r = await c.tool('lsp_definition', {
      path: work, file_path: 'src/main.rs', line: callLine, character: callChar,
    }, { timeoutMs: 90000 });
    expect(!r.isError, 'definition ok: ' + r.text);
    const locs = r.json.locations ?? [];
    expect(locs.some(l => l.file === 'src/lib.rs'), 'jumps to src/lib.rs: ' + JSON.stringify(locs));
  });

  await run('F3.2', 'lsp_references 引用集', async () => {
    const r = await c.tool('lsp_references', {
      path: work, file_path: 'src/utils.rs', line: 4, character: 7,
    }, { timeoutMs: 90000 });
    expect(!r.isError, 'references ok: ' + r.text);
    const lines = (r.json.locations ?? []).filter(l => l.file === 'src/lib.rs').map(l => l.line);
    for (const want of [28, 29, 30]) {
      expect(lines.includes(want), `reference line ${want} present, got ${JSON.stringify(lines)}`);
    }
  });

  await run('F3.5', '未装语言 LSP 优雅报错', async () => {
    const r = await c.tool('lsp_definition', {
      path: work, file_path: 'app.py', line: 1, character: 0,
    }, { timeoutMs: 30000 });
    expect(r.isError, 'isError expected');
    expect(/not found on PATH/.test(r.text), 'message: ' + r.text);
  });

  await run('F3.0-list', 'list 展示活跃 LSP', async () => {
    const r = await c.tool('list_mounted_projects', {});
    const p = (r.json.projects ?? []).find(x => x.path === work);
    expect(p && p.lsps.includes('rust-analyzer'), 'lsps shows rust-analyzer: ' + JSON.stringify(p?.lsps));
  });

  await run('F3.4', 'unmount kill 关联 LSP 无孤儿', async () => {
    expect(pgrepRa() !== '', 'rust-analyzer running before unmount');
    const r = await c.tool('unmount_project', { path: work });
    expectEq(r.json.unmounted, true, 'unmounted');
    expectEq(r.json.killed_lsps, 1, 'killed_lsps=1');
    let gone = false;
    for (let i = 0; i < 20; i++) {
      if (pgrepRa() === '') { gone = true; break; }
      await new Promise(res => setTimeout(res, 300));
    }
    expect(gone, 'rust-analyzer terminated after unmount');
  });

  await run('stdout-clean-3', 'stdout 零污染', async () => {
    expectEq(c.stdoutPollution.length, 0, JSON.stringify(c.stdoutPollution.slice(0, 3)));
  });
  await c.kill('SIGTERM');

  // —— 独立进程：F3.3 idle timeout ——
  await run('F3.3', 'idle timeout 自动 kill', async () => {
    const c2 = new McpStdioClient(bin, { env: { CODEIDX_LSP_IDLE_SECS: '8' } });
    await c2.initialize();
    const work2 = mkdtempSync(join(tmpdir(), 'bb3-idle-'));
    cpSync(fixture, work2, { recursive: true });
    await c2.tool('mount_project', { path: work2 });
    const mainText = readFileSync(join(work2, 'src/main.rs'), 'utf8').split('\n');
    let callLine = -1, callChar = -1;
    for (let i = 0; i < mainText.length; i++) {
      const col = mainText[i].indexOf('unicorn_marker_fn');
      if (col >= 0 && mainText[i].includes('(')) { callLine = i + 1; callChar = col; break; }
    }
    const d = await c2.tool('lsp_definition', {
      path: work2, file_path: 'src/main.rs', line: callLine, character: callChar,
    }, { timeoutMs: 90000 });
    expect(!d.isError && pgrepRa() !== '', 'rust-analyzer spawned and answered');
    await new Promise(res => setTimeout(res, 16000));
    expect(pgrepRa() === '', 'rust-analyzer killed by idle reaper after 16s (idle=8s)');
    await c2.tool('unmount_project', { path: work2 });
    await c2.kill('SIGTERM');
    rmSync(work2, { recursive: true, force: true });
  });

  rmSync(work, { recursive: true, force: true });
  console.log('\n==== Phase 3 black-box summary ====');
  for (const l of results) console.log(l);
  console.log(`${results.length - failed}/${results.length} passed`);
  process.exit(failed ? 1 : 0);
}

main().catch(e => { console.error('driver crash:', e); process.exit(2); });
