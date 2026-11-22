// bb_client.mjs — 黑盒 MCP stdio 驱动（仅开发期验收工具，不属于发布物）
// 用法见 bb_phase1.mjs。硬约束：本驱动逐行 JSON.parse 子进程 stdout，
// 任何无法解析的 stdout 行都会记为 pollution 并导致验收失败（ARCHITECTURE §5.1）。
import { spawn } from 'node:child_process';

export class McpStdioClient {
  constructor(binPath, { env = {}, cwd } = {}) {
    this.proc = spawn(binPath, [], {
      env: { ...process.env, ...env },
      cwd,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    this.nextId = 1;
    this.pending = new Map();
    this.stdoutPollution = [];
    this.stderrText = '';
    this.exited = null;
    this.proc.stderr.on('data', d => { this.stderrText += d.toString(); });
    this.proc.stdout.on('data', d => this._onStdout(d));
    this.proc.on('exit', (code, sig) => { this.exited = { code, sig }; });
  }

  _onStdout(buf) {
    const s = (this._buf ?? '') + buf.toString();
    const lines = s.split('\n');
    this._buf = lines.pop();
    for (const line of lines) {
      const t = line.trim();
      if (!t) continue;
      let msg;
      try { msg = JSON.parse(t); } catch {
        this.stdoutPollution.push(t.slice(0, 200));
        continue;
      }
      if (msg.id !== undefined && this.pending.has(msg.id)) {
        const { resolve } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        resolve(msg);
      }
    }
  }

  send(obj) {
    this.proc.stdin.write(JSON.stringify(obj) + '\n');
  }

  notify(method, params) {
    this.send({ jsonrpc: '2.0', method: method, params: params ?? {} });
  }

  call(method, params, { timeoutMs = 60000 } = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`timeout waiting response for ${method} id=${id}`));
      }, timeoutMs);
      this.pending.set(id, {
        resolve: m => { clearTimeout(timer); resolve(m); },
      });
      this.send({ jsonrpc: '2.0', id, method, params: params ?? {} });
    });
  }

  async initialize({ timeoutMs = 30000 } = {}) {
    const res = await this.call('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'bb-driver', version: '0.1.0' },
    }, { timeoutMs });
    if (res.error) throw new Error('initialize failed: ' + JSON.stringify(res.error));
    this.notify('notifications/initialized');
    return res.result;
  }

  // tools/call 便捷封装：返回 { isError, text, json }
  async tool(name, args, opts) {
    // Existing regression scripts exercise the machine-readable output.
    return this.toolAs(name, { format: 'json', ...args }, opts);
  }

  async toolAs(name, args, opts) {
    const res = await this.call('tools/call', { name, arguments: args }, opts);
    if (res.error) return { isError: true, text: JSON.stringify(res.error), json: null, rpcError: res.error };
    const r = res.result;
    const text = r?.content?.[0]?.text ?? '';
    let json = null;
    if (!r?.isError) { try { json = JSON.parse(text); } catch { /* 纯文本返回（如 repo_map） */ } }
    return { isError: !!r?.isError, text, json };
  }

  async kill(signal = 'SIGTERM', waitMs = 10000) {
    if (this.exited) return this.exited;
    this.proc.kill(signal);
    const start = Date.now();
    while (!this.exited && Date.now() - start < waitMs) {
      await new Promise(r => setTimeout(r, 50));
    }
    if (!this.exited) { this.proc.kill('SIGKILL'); return { code: null, sig: 'SIGKILL-forced' }; }
    return this.exited;
  }

  async vmRssKb() {
    const fs = await import('node:fs/promises');
    const s = await fs.readFile(`/proc/${this.proc.pid}/status`, 'utf8');
    const m = s.match(/VmRSS:\s*(\d+)\s*kB/);
    return m ? Number(m[1]) : null;
  }
}

export function expect(cond, msg) {
  if (!cond) throw new Error('ASSERT FAIL: ' + msg);
}

export function expectEq(a, b, msg) {
  if (a !== b) throw new Error(`ASSERT FAIL: ${msg}: got ${JSON.stringify(a)} want ${JSON.stringify(b)}`);
}
