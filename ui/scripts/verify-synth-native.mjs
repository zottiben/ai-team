// Offline, opt-in macOS acceptance. Never starts ait, Pi, a PTY or the installed app.
import { createServer } from 'vite';
import react from '@vitejs/plugin-react';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';

if (process.platform !== 'darwin') {
  console.log('SKIP: native WKWebView checks require macOS, swiftc and a genuinely visible desktop. This is not a native pass.');
  process.exit(0);
}
const root = fileURLToPath(new URL('../', import.meta.url));
const temporary = await mkdtemp(join(tmpdir(), 'ai-team-synth-native-'));
let server;
async function run(program, args, env, timeout) {
  return new Promise((resolve, reject) => {
    const child = spawn(program, args, { env, detached: true, stdio: ['ignore', 'inherit', 'inherit'] });
    const stop = reason => {
      try { if (child.pid) process.kill(-child.pid, 'SIGKILL'); } catch (error) { if (error.code !== 'ESRCH') { reject(error); return; } }
      reject(new Error(reason));
    };
    const interrupt = () => stop('Native verification interrupted');
    const timer = setTimeout(() => stop('Native verification timed out; no pass recorded'), timeout);
    const cleanup = () => { clearTimeout(timer); process.off('SIGINT', interrupt); process.off('SIGTERM', interrupt); };
    process.once('SIGINT', interrupt); process.once('SIGTERM', interrupt);
    child.once('error', error => { cleanup(); reject(error); });
    child.once('exit', (code, signal) => { cleanup(); code === 0 ? resolve() : reject(new Error(`${program} exited ${code ?? signal}`)); });
  });
}
try {
  server = await createServer({ root, configFile: false, cacheDir: join(temporary, 'vite-cache'), plugins: [react(), {
    name: 'raw-built-worker-fixture',
    configureServer(server) {
      // Exercise the actual build, not Vite's transformation of a built chunk.
      server.middlewares.use((request, response, next) => {
        const match = /^\/dist\/(diff-syntax-[a-zA-Z0-9._-]+\.js)(?:\?.*)?$/.exec(request.url ?? '');
        if (!match) return next();
        readFile(join(root, 'dist', match[1])).then(bytes => { response.setHeader('Content-Type', 'text/javascript'); response.end(bytes); }, next);
      });
    },
  }], server: { host: '127.0.0.1', port: 0, strictPort: true, hmr: false, watch: null } });
  await server.listen();
  const address = server.httpServer.address();
  if (!address || typeof address === 'string') throw new Error('Isolated fixture server has no TCP address');
  const binary = join(temporary, 'synth-native');
  await run('swiftc', ['-parse-as-library', join(root, 'scripts/synth-native.swift'), '-o', binary], process.env, 120000);
  await run(binary, [], { ...process.env, SYNTH_FIXTURE_URL: `http://127.0.0.1:${address.port}`, SYNTH_PROBE_START: '0' }, 250000);
  console.log('PASS: 26 visible WKWebView synthetic checks. Not physical input, installed-app, CPU/INP or release certification.');
} finally {
  await server?.close();
  await rm(temporary, { recursive: true, force: true });
}
