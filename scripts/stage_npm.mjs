// Usage: node scripts/stage_npm.mjs <platform> <native-bin-dir> <new-stage-dir>
import { chmodSync, copyFileSync, cpSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const [platform, binaries, dest] = process.argv.slice(2);
if (!['linux-x64', 'win32-x64'].includes(platform) || !binaries || !dest) {
  throw Error('usage: node scripts/stage_npm.mjs <linux-x64|win32-x64> <native-bin-dir> <new-stage-dir>');
}
const [os, cpu] = platform.split('-');
const release = JSON.parse(readFileSync(join(root, 'npm/launcher/package.json'), 'utf8'));
const target = '@thatgfsj/codeidx-' + platform;
if (release.optionalDependencies[target] !== release.version) throw Error('platform package version drift');
const out = resolve(dest);
mkdirSync(out);
const native = join(out, platform);
mkdirSync(join(native, 'bin'), { recursive: true });
const exe = os === 'win32' ? '.exe' : '';
for (const name of ['codeidx-mcp', 'codeidx-tui']) {
  const file = name + exe;
  copyFileSync(resolve(binaries, file), join(native, 'bin', file));
  if (!exe) chmodSync(join(native, 'bin', file), 0o755);
}
copyFileSync(join(root, 'LICENSE'), join(native, 'LICENSE'));
writeFileSync(join(native, 'README.md'), [
  '# Codeidx native binaries', '',
  'Install the [codeidx launcher](https://www.npmjs.com/package/@thatgfsj/codeidx) to use these executables.',
  'Source and build instructions: https://github.com/Thatgfsj/codeidx', '',
].join('\n'));
writeFileSync(join(native, 'package.json'), JSON.stringify({
  name: target, version: release.version, description: 'Codeidx native executables for ' + platform,
  license: release.license, repository: release.repository, os: [os], cpu: [cpu],
  ...(os === 'linux' ? { libc: ['glibc'] } : {}),
  files: ['bin'], publishConfig: { access: 'public', registry: 'https://registry.npmjs.org/' },
}, null, 2) + '\n');
const launcher = join(out, 'launcher');
cpSync(join(root, 'npm/launcher'), launcher, { recursive: true });
copyFileSync(join(root, 'LICENSE'), join(launcher, 'LICENSE'));
console.log(native);
console.log(launcher);
