const { spawn } = require('node:child_process');
const { dirname, join } = require('node:path');
module.exports = function launch(name) {
  const platform = process.platform + '-' + process.arch;
  const fail = message => { console.error(name + ': ' + message); process.exitCode = 1; };
  if (!['linux-x64', 'win32-x64'].includes(platform)) {
    return fail('unsupported platform ' + platform + '; build from https://github.com/Thatgfsj/codeidx');
  }
  if (process.platform === 'linux' && !process.report.getReport().header.glibcVersionRuntime) {
    return fail('the Linux npm binary requires glibc; build from source for other environments');
  }
  const pkg = '@thatgfsj/codeidx-' + platform;
  let manifest;
  try { manifest = require.resolve(pkg + '/package.json'); }
  catch { return fail('missing ' + pkg + '; reinstall with optional dependencies enabled'); }
  const executable = join(dirname(manifest), 'bin', name + (process.platform === 'win32' ? '.exe' : ''));
  const child = spawn(executable, process.argv.slice(2), { stdio: 'inherit' });
  // In a terminal the foreground group already receives Ctrl+C; keep the
  // wrapper alive while the native executable handles it and restores the tty.
  const interrupt = () => {};
  const terminate = () => child.kill('SIGTERM');
  process.on('SIGINT', interrupt);
  if (process.platform !== 'win32') process.on('SIGTERM', terminate);
  child.once('error', error => fail(error.message));
  child.once('close', (code, signal) => {
    process.removeListener('SIGINT', interrupt);
    process.removeListener('SIGTERM', terminate);
    process.exitCode = code ?? (signal === 'SIGINT' ? 130 : signal === 'SIGTERM' ? 143 : 1);
  });
};
