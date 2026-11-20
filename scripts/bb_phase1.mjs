// bb_phase1.mjs — Phase 1 黑盒验收（docs/ACCEPTANCE.md §2 用例 F1.1–F1.15）
// 用法：node scripts/bb_phase1.mjs <path-to-codeidx-mcp-binary>
// 退出码 0 = 全部通过。file_path 参数语义：挂载根内相对路径。
import { cpSync, mkdtempSync, rmSync, mkdirSync, writeFileSync, rmSync as rmf, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { McpStdioClient, expect, expectEq } from './bb_client.mjs';

const bin = process.argv[2];
if (!bin) { console.error('usage: node scripts/bb_phase1.mjs <bin>'); process.exit(2); }
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const fixture = join(repoRoot, 'tests/fixtures/sample-project');

const results = [];
let failed = 0;
async function runCase(id, name, fn) {
  try {
    await fn();
    results.push(`PASS ${id} ${name}`);
  } catch (e) {
    failed++;
    results.push(`FAIL ${id} ${name} :: ${e.message}`);
  }
}

// 工作区副本（F1.9 需要改文件，不能污染仓库 fixture）
function freshCopy() {
  const dir = mkdtempSync(join(tmpdir(), 'bb-sample-'));
  cpSync(fixture, dir, { recursive: true });
  return dir;
}

async function main() {
  const work = freshCopy();

  // —— 长会话用例 F1.1–F1.11 ——
  const c = new McpStdioClient(bin);

  await runCase('F1.1', 'initialize 握手', async () => {
    const caps = await c.initialize();
    expect(!!caps, 'capabilities returned');
  });

  await runCase('F1.2', 'tools/list 11 工具齐全', async () => {
    const res = await c.call('tools/list');
    const names = (res.result?.tools ?? []).map(t => t.name).sort();
    expectEq(names.length, 11, 'tool count');
    for (const want of ['mount_project', 'unmount_project', 'list_mounted_projects', 'sync_project',
      'get_repo_map', 'search_code', 'get_file_outline', 'read_symbol',
      'lsp_definition', 'lsp_references', 'lsp_hover']) {
      expect(names.includes(want), `tools/list contains ${want} (got ${names.join(',')})`);
    }
  });

  await runCase('F1.3', 'mount_project 基础挂载', async () => {
    const r = await c.tool('mount_project', { path: work });
    expect(!r.isError, 'mount ok: ' + r.text);
    expectEq(r.json.mounted, true, 'mounted');
    expect(r.json.files > 5, 'files > 5, got ' + r.json.files);
    expect(r.json.symbols > 8, 'symbols > 8, got ' + r.json.symbols);
    expect(typeof r.json.elapsed_ms === 'number', 'elapsed_ms present');
  });

  await runCase('F1.4', 'list_mounted_projects', async () => {
    const r = await c.tool('list_mounted_projects', {});
    expect(!r.isError, 'list ok');
    const p = (r.json.projects ?? []).find(x => x.path === work);
    expect(!!p, 'mounted project listed');
    expect(Array.isArray(p.lsps), 'lsps array');
  });

  await runCase('F1.5', 'get_repo_map 骨架+截断', async () => {
    const full = await c.tool('get_repo_map', { path: work, token_budget: 3000 });
    expect(!full.isError, 'repo_map ok: ' + full.text);
    expect(full.text.includes('helper_spinner'), 'high-centrality helper_spinner ranked in');
    const tiny = await c.tool('get_repo_map', { path: work, token_budget: 80 });
    expect(!tiny.isError, 'tiny repo_map ok');
    expect(tiny.text.includes('truncated'), 'budget truncation marker: ' + tiny.text.slice(-120));
  });

  await runCase('F1.6', 'get_file_outline 符号与行号', async () => {
    const r = await c.tool('get_file_outline', { path: work, file_path: 'src/lib.rs' });
    expect(!r.isError, 'outline ok: ' + r.text);
    const sym = (r.json.symbols ?? []).find(s => s.name === 'unicorn_marker_fn');
    expect(!!sym, 'unicorn_marker_fn in outline');
    expectEq(sym.kind.toLowerCase(), 'fn', 'kind fn');
    expect(sym.start_line >= 1 && sym.end_line >= sym.start_line, 'line numbers sane');
    const cfg = (r.json.symbols ?? []).find(s => s.name === 'GoldConfig');
    expect(!!cfg, 'GoldConfig struct in outline');
  });

  await runCase('F1.7', 'search_code keyword 唯一函数命中', async () => {
    const r = await c.tool('search_code', { path: work, query: 'unicorn_marker_fn', mode: 'keyword', top_k: 8 });
    expect(!r.isError, 'search ok: ' + r.text);
    expect((r.json.results ?? []).length >= 1, 'at least one hit');
    const top = r.json.results[0];
    expect(top.file === 'src/lib.rs', 'hit in src/lib.rs, got ' + top.file);
    expect(top.symbol === 'unicorn_marker_fn' || top.symbol === null, 'symbol field sane');
    expect(top.start_line >= 1, 'start_line valid');
  });

  await runCase('F1.8', 'read_symbol 完整实现体', async () => {
    const r = await c.tool('read_symbol', { path: work, file_path: 'src/lib.rs', symbol_name: 'unicorn_marker_fn' });
    expect(!r.isError, 'read_symbol ok: ' + r.text);
    expectEq(r.json.file, 'src/lib.rs', 'file');
    expectEq(r.json.name, 'unicorn_marker_fn', 'name');
    expect(r.json.code.includes('unicorn_marker_fn'), 'code contains signature');
    expect((r.json.code.match(/\n/g) ?? []).length >= 2, 'multi-line body returned, not snippet');
  });

  await runCase('F1.11', '未挂载路径优雅报错', async () => {
    const r = await c.tool('get_repo_map', { path: '/tmp/bb-definitely-not-mounted-' + Date.now() });
    expect(r.isError, 'must be isError');
    expect(/not mounted/i.test(r.text), 'message mentions not mounted: ' + r.text);
  });

  await runCase('F1.9', 'sync 增量 + 变更生效', async () => {
    // 新增
    writeFileSync(join(work, 'src/extra.rs'), 'pub fn brand_new_fn() -> u32 { 42 }\n');
    // 修改
    const lib = join(work, 'src/lib.rs');
    const old = await import('node:fs').then(fs => fs.readFileSync(lib, 'utf8'));
    await import('node:fs').then(fs => fs.writeFileSync(lib, old + '\npub fn edited_fn_added_later() -> u32 { 7 }\n'));
    // 删除
    rmf(join(work, 'app.py'));
    const r = await c.tool('sync_project', { path: work });
    expect(!r.isError, 'sync ok: ' + r.text);
    expectEq(r.json.added, 1, 'added=1');
    expectEq(r.json.modified, 1, 'modified=1');
    expectEq(r.json.removed, 1, 'removed=1');
    const s = await c.tool('search_code', { path: work, query: 'brand_new_fn', mode: 'keyword' });
    expect(!s.isError && (s.json.results ?? []).some(h => h.file === 'src/extra.rs'), 'new file indexed after sync');
    const s2 = await c.tool('search_code', { path: work, query: 'edited_fn_added_later', mode: 'keyword' });
    expect(!s2.isError && (s2.json.results ?? []).length >= 1, 'modified content indexed after sync');
  });

  await runCase('F1.10', 'unmount_project', async () => {
    const r = await c.tool('unmount_project', { path: work });
    expect(!r.isError, 'unmount ok: ' + r.text);
    expectEq(r.json.unmounted, true, 'unmounted');
    const l = await c.tool('list_mounted_projects', {});
    expect(!(l.json.projects ?? []).some(p => p.path === work), 'gone from list');
  });

  await runCase('stdout-clean', 'stdout 全程无污染', async () => {
    expectEq(c.stdoutPollution.length, 0, 'stdout pollution lines: ' + JSON.stringify(c.stdoutPollution.slice(0, 3)));
  });

  await c.kill('SIGTERM');
  await runCase('exit-0', 'SIGTERM 后退出码 0', async () => {
    expectEq(c.exited?.code, 0, 'exit code');
  });

  // —— 独立进程用例 ——
  await runCase('F1.13', '空闲内存 < 15MB', async () => {
    const c2 = new McpStdioClient(bin);
    await c2.initialize();
    await new Promise(r => setTimeout(r, 2000));
    const rss = await c2.vmRssKb();
    expect(rss !== null && rss < 15 * 1024, `VmRSS=${rss}kB must be < ${15 * 1024}kB`);
    await c2.kill('SIGTERM');
    console.log(`  [info] idle VmRSS = ${rss} kB`);
  });

  await runCase('F1.12', '挂载后 SIGTERM 干净退出', async () => {
    const c3 = new McpStdioClient(bin);
    await c3.initialize();
    const m = await c3.tool('mount_project', { path: work });
    expect(!m.isError, 'mount ok');
    const ex = await c3.kill('SIGTERM');
    expectEq(ex.code, 0, 'exit code after SIGTERM with mounted project');
  });

  await runCase('F1.14', '挂载峰值内存记录', async () => {
    const c4 = new McpStdioClient(bin);
    await c4.initialize();
    const before = await c4.vmRssKb();
    await c4.tool('mount_project', { path: work });
    const after = await c4.vmRssKb();
    console.log(`  [info] rss idle=${before}kB mounted=${after}kB`);
    await c4.kill('SIGTERM');
  });

  await runCase('F1.15', 'get_repo_map focus_files 提权', async () => {
    const c5 = new McpStdioClient(bin);
    await c5.initialize();
    await c5.tool('mount_project', { path: work });
    const r = await c5.tool('get_repo_map', { path: work, focus_files: ['src/utils.rs'], token_budget: 3000 });
    expect(!r.isError, 'focused repo_map ok: ' + r.text);
    const lines = r.text.split('\n').filter(l => l.trim() && !l.includes('truncated'));
    expect(lines.length > 0, 'non-empty map');
    const utilsPos = lines.findIndex(l => l.startsWith('src/utils.rs'));
    expect(utilsPos >= 0 && utilsPos < 5, `src/utils.rs promoted to top-5 (pos=${utilsPos})`);
    await c5.kill('SIGTERM');
  });

  rmSync(work, { recursive: true, force: true });
  console.log('\n==== Phase 1 black-box summary ====');
  for (const line of results) console.log(line);
  console.log(`${results.length - failed}/${results.length} passed`);
  process.exit(failed ? 1 : 0);
}

main().catch(e => { console.error('driver crash:', e); process.exit(2); });
