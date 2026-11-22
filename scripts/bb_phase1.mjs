// bb_phase1.mjs — Phase 1 黑盒验收（docs/ACCEPTANCE.md §2 用例 F1.1–F1.15）
// 用法：node scripts/bb_phase1.mjs <path-to-codeidx-mcp-binary>
// 退出码 0 = 全部通过。file_path 参数语义：挂载根内相对路径。
import { cpSync, mkdtempSync, rmSync, mkdirSync, writeFileSync, renameSync, utimesSync, rmSync as rmf, statSync } from 'node:fs';
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
    expectEq(caps.capabilities?.tools?.listChanged, false, 'tool catalog is static');
    expect(!Object.hasOwn(caps, 'instructions'), 'initialize has no agent instructions');
  });

  await runCase('F1.2', '挂载前 tools/list 已列出全部 11 个工具', async () => {
    const res = await c.call('tools/list');
    const names = res.result.tools.map(t => t.name).sort();
    expect(res.result.tools.every(t => t.inputSchema.properties.format.default === 'compact'), 'compact is default');
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
    const compact = await c.toolAs('list_mounted_projects', {});
    expect(!compact.isError && compact.text.includes(work), 'default project TSV: ' + compact.text);
  });

  await runCase('F1.8', '挂载后直接 read_symbol 返回完整实现体', async () => {
    const r = await c.tool('read_symbol', { path: work, file_path: 'src/lib.rs', symbol_name: 'unicorn_marker_fn' });
    expect(!r.isError, 'read_symbol ok: ' + r.text);
    expectEq(r.json.file, 'src/lib.rs', 'file');
    expectEq(r.json.name, 'unicorn_marker_fn', 'name');
    expect(r.json.code.includes('unicorn_marker_fn'), 'code contains signature');
    expect((r.json.code.match(/\n/g) ?? []).length >= 2, 'multi-line body returned, not snippet');
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
    expect(tiny.json?.truncated > 0 && tiny.json.shown + tiny.json.truncated === tiny.json.symbols, 'budget truncation marker: ' + tiny.text.slice(-120));
  });

  await runCase('F1.6', 'get_file_outline 符号与行号', async () => {
    const r = await c.tool('get_file_outline', { path: work, file_path: 'src/lib.rs' });
    expect(!r.isError, 'outline ok: ' + r.text);
    expectEq(r.json.file, 'src/lib.rs', 'outline file');
    expect(r.json.symbols.some(s => s.kind === 'fn' && s.name === 'unicorn_marker_fn' && s.start_line === 27), r.text);
    expect(r.json.symbols.some(s => s.kind === 'struct' && s.name === 'GoldConfig'), r.text);
  });

  await runCase('F1.7', 'search_code keyword 唯一函数命中', async () => {
    const r = await c.tool('search_code', { path: work, query: 'unicorn_marker_fn', mode: 'keyword', top_k: 8 });
    expect(!r.isError, 'search ok: ' + r.text);
    const results = r.json.results;
    expect(typeof results === 'string' && results.length > 0, 'results compact text: ' + r.text);
    const dataRows = results.split('\n').filter(l => l && !l.startsWith('#') && !l.startsWith('file	'));
    expect(dataRows.length >= 1, 'at least one hit row');
    const top = dataRows[0];
    expect(top.startsWith('src/lib.rs	'), 'hit in src/lib.rs, got ' + top);
    expect(top.includes('	unicorn_marker_fn	') || top.includes('	-	'), 'symbol field sane: ' + top);
    expect(/^src\/lib\.rs	[^	]*	[1-9][0-9]*/.test(top), 'start_line valid: ' + top);
  });

  await runCase('F1.11', '未挂载路径优雅报错', async () => {
    const r = await c.tool('get_repo_map', { path: '/tmp/bb-definitely-not-mounted-' + Date.now() });
    expect(r.isError, 'must be isError');
    expect(/not mounted/i.test(r.text), 'message mentions not mounted: ' + r.text);
  });

  await runCase('F1.9', 'sync 增量 + 变更生效', async () => {
    const mount = await c.tool('mount_project', { path: work, auto_sync: false });
    expect(!mount.isError, 'mount without automatic sync: ' + mount.text);
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
    expect(!s.isError && s.json.results.includes('src/extra.rs'), 'new file indexed after sync');
    const s2 = await c.tool('search_code', { path: work, query: 'edited_fn_added_later', mode: 'keyword' });
    expect(!s2.isError && s2.json.results.includes('edited_fn_added_later'), 'modified content indexed after sync');
  });

  await runCase('F1.10', 'unmount_project', async () => {
    const r = await c.tool('unmount_project', { path: work });
    expect(!r.isError, 'unmount ok: ' + r.text);
    expectEq(r.json.unmounted, true, 'unmounted');
    const l = await c.tool('list_mounted_projects', {});
    expect(!(l.json.projects ?? []).some(p => p.path === work), 'gone from list');
  });

  await runCase('auto-sync', '默认监听新增、原子替换和删除，卸载后可关闭', async () => {
    const call = async (name, args = {}) => {
      const result = await c.tool(name, { path: work, ...args });
      expect(!result.isError, name + ': ' + result.text);
      return result;
    };
    const has = async query => {
      const { json } = await call('search_code', { query, mode: 'keyword' });
      return json.results.split('\n').some(row => {
        const [file, symbol] = row.split('\t');
        return file === 'src/live.rs' && symbol === query;
      });
    };
    const eventually = async (condition, message) => {
      const deadline = Date.now() + 8000;
      while (Date.now() < deadline) {
        if (await condition()) return;
        await new Promise(resolve => setTimeout(resolve, 40));
      }
      throw new Error(message);
    };
    await call('mount_project');
    const live = join(work, 'src/live.rs');
    const temp = join(work, 'src/.live-save');
    const timestamp = 1700000000;
    // 通过重命名发布完整文件，避免监听读到中间写入状态。
    writeFileSync(temp, 'pub fn zz_watch_before() {}');
    utimesSync(temp, timestamp, timestamp);
    renameSync(temp, live);
    await eventually(() => has('zz_watch_before'), '新文件未进入索引');

    // 旧、新文件长度和 mtime 完全一致，必须由文件事件触发内容复核。
    const before = statSync(live);
    writeFileSync(temp, 'pub fn zz_watch_afterx() {}');
    utimesSync(temp, timestamp, timestamp);
    renameSync(temp, live);
    const after = statSync(live);
    expectEq(after.size, before.size, 'same file size');
    expectEq(after.mtimeMs, before.mtimeMs, 'same mtime');
    await eventually(() => has('zz_watch_afterx'), '原子替换未进入索引');
    expect(!(await has('zz_watch_before')), '旧符号已移除');

    rmf(live);
    await eventually(async () => !(await has('zz_watch_afterx')), '删除未清除索引');
    // 留下可能未消费的文件事件，随后卸载并用禁用模式挂载同一路径。
    writeFileSync(temp, 'pub fn zz_pending_event() {}');
    await call('unmount_project');
    const absent = await c.tool('search_code', { path: work, query: 'zz_pending_event' });
    expect(absent.isError && /not mounted/.test(absent.text), '卸载后不可查询');
    await call('mount_project', { auto_sync: false });
    renameSync(temp, live);
    await new Promise(resolve => setTimeout(resolve, 500));
    expect(!(await has('zz_pending_event')), '禁用或卸载后未自动更新');
    const synced = await call('sync_project');
    expectEq(synced.json.added, 1, 'manual sync added file');
    expect(await has('zz_pending_event'), '手动同步后可查');
    await call('unmount_project');
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
    const focus = r.json?.files.find(f => f.path === 'src/utils.rs');
    expect(focus?.symbols[0].rank <= 5, 'src/utils.rs promoted to top-5: ' + r.text);
    await c5.kill('SIGTERM');
  });

  rmSync(work, { recursive: true, force: true });
  console.log('\n==== Phase 1 black-box summary ====');
  for (const line of results) console.log(line);
  console.log(`${results.length - failed}/${results.length} passed`);
  process.exit(failed ? 1 : 0);
}

main().catch(e => { console.error('driver crash:', e); process.exit(2); });
