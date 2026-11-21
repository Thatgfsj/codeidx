// bb_phase2.mjs — Phase 2 黑盒验收（docs/ACCEPTANCE.md §3 用例 F2.1–F2.6）
// 用法：node scripts/bb_phase2.mjs <path-to-codeidx-mcp-binary>
// 前提：模型缓存 ~/.cache/codeidx-mcp（首次运行将触发下载 ~90MB，预算 240s）。
import { cpSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { McpStdioClient, expect, expectEq } from './bb_client.mjs';

const bin = process.argv[2];
if (!bin) { console.error('usage: node scripts/bb_phase2.mjs <bin>'); process.exit(2); }
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const fixture = join(repoRoot, 'tests/fixtures/sample-project');

const results = [];
let failed = 0;
async function run(id, name, fn) {
  try { await fn(); results.push(`PASS ${id} ${name}`); }
  catch (e) { failed++; results.push(`FAIL ${id} ${name} :: ${e.message}`); }
}

async function main() {
  // —— 默认进程：F2.1/F2.2/F2.3/F2.6 ——
  const work = mkdtempSync(join(tmpdir(), 'bb2-sample-'));
  cpSync(fixture, work, { recursive: true });
  const c = new McpStdioClient(bin);
  await c.initialize();
  let mountedRss = null;

  await run('F2.1', 'mount(enable_vector=true) 懒加载或降级', async () => {
    const r = await c.tool('mount_project', { path: work, enable_vector: true }, { timeoutMs: 240000 });
    expect(!r.isError, 'mount ok: ' + r.text);
    mountedRss = await c.vmRssKb();
    console.log(`  [info] mounted vector=${r.json.vector} rss=${mountedRss}kB`);
    if (r.json.vector) expect(mountedRss > 100 * 1024, 'model resident pushes rss over 100MB');
    expect(typeof r.json.elapsed_ms === 'number', 'elapsed_ms');
  });

  await run('F2.2', 'vector 语义命中目标函数', async () => {
    // 断言校准依据：teammate-g 真模型实测该查询下 unicorn_marker_fn 排
    // vector 路 top8 第 4（bge-small-zh 中文↔代码区分度边际，探针 margin
    // 0.06）。验收意图是"语义检索能命中目标"而非名次，故断言 top8 内含。
    const r = await c.tool('search_code', {
      path: work, query: '组合配置生成标记字符串的函数', mode: 'vector', top_k: 8,
    }, { timeoutMs: 120000 });
    expect(!r.isError, 'vector search ok: ' + r.text);
    const results = r.json.results;
    console.log('  [info] vector top rows: ' + results.split('\n').slice(2, 6).join(' | '));
    expect(results.includes('unicorn_marker_fn'), 'unicorn_marker_fn in top-8 vector results');
  });

  await run('F2.3', 'hybrid RRF 融合两路', async () => {
    const r = await c.tool('search_code', {
      path: work, query: 'unicorn_marker_fn', mode: 'hybrid', top_k: 8,
    }, { timeoutMs: 120000 });
    expect(!r.isError, 'hybrid ok: ' + r.text);
    expectEq(r.json.mode, 'hybrid', 'mode echoed');
    expect(r.json.results.includes('src/lib.rs'), 'definition chunk fused into results');
  });

  await run('F2.6', '内存画像记录', async () => {
    const c6 = new McpStdioClient(bin);
    await c6.initialize();
    const work6 = mkdtempSync(join(tmpdir(), 'bb2-novec-'));
    cpSync(fixture, work6, { recursive: true });
    await c6.tool('mount_project', { path: work6, enable_vector: false });
    const novecRss = await c6.vmRssKb();
    console.log(`  [info] rss no-vector=${novecRss}kB vs vector=${mountedRss}kB`);
    expect(novecRss < 60 * 1024, 'no-vector mount stays under 60MB');
    await c6.tool('unmount_project', { path: work6 });
    await c6.kill('SIGTERM');
    rmSync(work6, { recursive: true, force: true });
  });

  await run('stdout-clean-2', 'stdout 零污染', async () => {
    expectEq(c.stdoutPollution.length, 0, JSON.stringify(c.stdoutPollution.slice(0, 3)));
  });
  await c.kill('SIGTERM');
  rmSync(work, { recursive: true, force: true });

  // —— 禁用进程：F2.5 ——
  await run('F2.5', 'CODEIDX_DISABLE_VECTOR 强制降级', async () => {
    const c5 = new McpStdioClient(bin, { env: { CODEIDX_DISABLE_VECTOR: '1' } });
    await c5.initialize();
    const work5 = mkdtempSync(join(tmpdir(), 'bb2-dis-'));
    cpSync(fixture, work5, { recursive: true });
    const m = await c5.tool('mount_project', { path: work5, enable_vector: true });
    expect(!m.isError && m.json.vector === false, 'vector forced off: ' + m.text);
    const s = await c5.tool('search_code', { path: work5, query: 'unicorn_marker_fn', mode: 'hybrid' });
    expect(!s.isError, 'degraded hybrid ok');
    expectEq(s.json.mode, 'keyword', 'mode falls back to keyword');
    expect((s.json.results ?? []).length >= 1, 'keyword results present');
    await c5.kill('SIGTERM');
    rmSync(work5, { recursive: true, force: true });
  });

  console.log('\n==== Phase 2 black-box summary ====');
  for (const l of results) console.log(l);
  console.log(`${results.length - failed}/${results.length} passed`);
  process.exit(failed ? 1 : 0);
}

main().catch(e => { console.error('driver crash:', e); process.exit(2); });
