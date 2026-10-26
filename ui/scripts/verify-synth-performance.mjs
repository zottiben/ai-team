import { build } from 'vite';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join, resolve, extname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

// Optional local benchmark: use existing browser tooling, never install a runtime
// or use the operator's profile. This does not launch the installed application.
if (!process.env.SYNTH_BROWSER_MODULE || !process.env.CHROME_BIN) {
  throw Error('Set SYNTH_BROWSER_MODULE to an existing Puppeteer module and CHROME_BIN to an existing Chrome executable. No tooling is installed automatically.');
}
const imported = await import(process.env.SYNTH_BROWSER_MODULE);
const puppeteer = imported.puppeteer ?? imported.default;
if (typeof puppeteer?.launch !== 'function') throw Error('The supplied browser module has no Puppeteer launch API');
const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const temp = await mkdtemp(join(tmpdir(), 'synth-compiled-perf-'));
const reportPath = process.env.SYNTH_PERF_REPORT ?? join(tmpdir(), `synth-performance-${Date.now()}.json`);
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
let server, browser, cleaning;
function cleanup() {
  return cleaning ??= (async () => {
    try { if (browser) await browser.close(); }
    finally { if (server) await new Promise(resolve => server.close(resolve)); await rm(temp, { recursive: true, force: true }); }
  })();
}
for (const [signal, code] of [['SIGINT', 130], ['SIGTERM', 143]]) process.once(signal, () => {
  void cleanup().then(() => process.exit(code), error => { console.error(error); process.exit(code); });
});
try {
  const built = await build({ root, configFile: join(root, 'vite.config.ts'), build: {
    outDir: temp, emptyOutDir: true,
    rollupOptions: { input: join(root, 'src/synth/shell-preview.tsx'), preserveEntrySignatures: 'strict', output: { entryFileNames: 'src/synth/shell-preview.tsx' } },
  } });
  const entry = (Array.isArray(built) ? built[0] : built).output.find(item => item.type === 'chunk' && item.isEntry);
  if (!entry?.exports.includes('appendActivity') || !entry.viteMetadata?.importedCss?.size) throw Error('Compiled fixture exports or styles were not preserved');
  const links = [...(entry.viteMetadata?.importedCss ?? [])].map(path => `<link rel="stylesheet" href="/${path}">`).join('');
  const html = (await readFile(join(root, 'synth-shell-preview.html'), 'utf8')).replace('</head>', links + '</head>');
  await writeFile(join(temp, 'synth-shell-preview.html'), html);
  server = createServer(async (req, res) => {
    try {
      const path = new URL(req.url, 'http://localhost').pathname, file = resolve(temp, '.' + path);
      if (!file.startsWith(temp + '/') || path.startsWith('/api/')) { res.writeHead(404).end(); return; }
      const data = await readFile(file);
      res.setHeader('Content-Type', ({ '.html': 'text/html', '.js': 'text/javascript', '.tsx': 'text/javascript', '.css': 'text/css', '.woff2': 'font/woff2', '.svg': 'image/svg+xml' })[extname(file)] ?? 'application/octet-stream');
      res.end(data);
    } catch (error) {
      if (error.code !== 'ENOENT') console.error('Fixture asset unavailable', error);
      res.writeHead(error.code === 'ENOENT' ? 404 : 500).end();
    }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  browser = await puppeteer.launch({ executablePath: process.env.CHROME_BIN, headless: true,
    args: ['--disable-background-networking', '--disable-component-update', '--disable-sync', '--use-mock-keychain', '--password-store=basic'] });
  const report = { scope: 'Production-compiled isolated fixture; not whole-app CPU, installed-app, physical-input or release certification.',
    timing: 'Pane visibility plus two frame opportunities bounds presentation, not cold content readiness. EventTiming samples are not a whole-session INP certification.', views: [] };
  for (const scene of [
    { name: 'tracker-5000', query: 'activity=5000', tab: 'Overview', selector: '.synth-overview-rack', scroll: '.chat-transcript', rows: '.chat-transcript-row' },
    { name: 'diff-10000', query: 'review=true', tab: 'Review', selector: '[aria-label="Diff for src/large.ts"]', scroll: '[aria-label="Diff for src/large.ts"]', rows: '.synth-diff-row' },
    { name: 'board-100', query: 'board=true&board-input=true', tab: 'Board', selector: '.synth-plan-overview', rows: '[data-slice-key]' },
  ]) {
    const page = await browser.newPage(); let externalAttempts = 0;
    await page.setViewport({ width: 1280, height: 900 });
    await page.setRequestInterception(true);
    page.on('request', request => {
      const url = new URL(request.url());
      if (/^https?:$/.test(url.protocol) && url.origin !== base) { externalAttempts++; void request.abort(); }
      else void request.continue();
    });
    await page.evaluateOnNewDocument(() => {
      window.__eventTimings = [];
      new PerformanceObserver(list => {
        for (const event of list.getEntries()) if (event.interactionId && window.__eventTimings.length < 128)
          window.__eventTimings.push({ name: event.name, duration: event.duration, id: event.interactionId });
      }).observe({ type: 'event', durationThreshold: 16, buffered: true });
    });
    await page.goto(`${base}/synth-shell-preview.html?${scene.query}&theme=dark`);
    await page.waitForSelector('.chat-transcript-row'); await page.evaluate(() => document.fonts.ready); await sleep(250);
    const point = await page.evaluate(label => {
      const button = [...document.querySelectorAll('[role=tab]')].find(node => node.textContent.trim() === label);
      if (!button) throw Error('Tab missing');
      const box = button.getBoundingClientRect(); return { x: box.x + box.width / 2, y: box.y + box.height / 2, time: performance.now() };
    }, scene.tab);
    await page.mouse.click(point.x, point.y); await page.waitForSelector(`#chat-pane-${scene.tab.toLowerCase()}`, { visible: true });
    const pane = await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve(performance.now())))));
    await page.waitForSelector(scene.selector, { visible: true });
    const content = await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve(performance.now())))));
    await sleep(700);
    const client = await page.createCDPSession(); await client.send('Performance.enable');
    const before = await client.send('Performance.getMetrics'), wall = performance.now();
    await sleep(2000); const after = await client.send('Performance.getMetrics');
    const metric = result => result.metrics.find(item => item.name === 'TaskDuration')?.value;
    const idleRendererTaskCpuPercent = (metric(after) - metric(before)) / ((performance.now() - wall) / 1000) * 100;
    await page.evaluate(() => {
      window.__frameTimes = []; window.__frameStop = false;
      const tick = time => { if (window.__frameTimes.length < 4096) window.__frameTimes.push(time); if (!window.__frameStop) requestAnimationFrame(tick); };
      requestAnimationFrame(tick);
    });
    if (scene.scroll) {
      const position = await page.evaluate(selector => { const box = document.querySelector(selector).getBoundingClientRect(); return { x: box.x + box.width / 2, y: box.y + box.height / 2 }; }, scene.scroll);
      await page.mouse.move(position.x, position.y);
      for (let index = 0; index < 35; index++) { await page.mouse.wheel({ deltaY: scene.name === 'tracker-5000' ? -300 : 250 }); await sleep(20); }
    } else {
      await page.waitForSelector('[data-slice-drag]:not(:disabled)');
      const position = await page.evaluate(() => {
        const grip = [...document.querySelectorAll('[data-slice-drag]:not(:disabled)')].find(node => {
          const box = node.getBoundingClientRect();
          return box.width && box.height && box.top > 0 && box.bottom < innerHeight && document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2)?.closest('[data-slice-drag]') === node;
        });
        if (!grip) throw Error('No hit-testable grip'); const box = grip.getBoundingClientRect(); return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
      });
      await page.mouse.move(position.x, position.y); await page.mouse.down();
      let ghostLeft;
      for (let index = 0; index < 60; index++) {
        await page.mouse.move(position.x + 8 + Math.sin(index / 8) * 5, position.y + 8); await sleep(16);
        if (index === 10 || index === 40) {
          const left = await page.evaluate(() => document.querySelector('.synth-plan-drag-overlay:has(.synth-plan-ghost)')?.getBoundingClientRect().left);
          if (!Number.isFinite(left)) throw Error('Trusted movement did not create a drag ghost');
          if (index === 10) ghostLeft = left;
          else if (Math.abs(left - ghostLeft) < 2) throw Error('Owned drag feedback did not follow the trusted input');
        }
      }
      await page.keyboard.press('Escape'); await page.mouse.up();
      const requests = await page.evaluate(async () => {
        const entry = [...document.scripts].find(script => new URL(script.src || location.href).pathname === '/src/synth/shell-preview.tsx');
        if (!entry || document.querySelectorAll('#root > .chat-shell').length !== 1) throw Error('Expected one fixture owner');
        return (await import(entry.src)).boardState().requests.length;
      });
      if (requests !== 0) throw Error('Cancelled benchmark gesture submitted a command');
    }
    const frames = await page.evaluate(() => { window.__frameStop = true; return window.__frameTimes; }); await sleep(300);
    const state = await page.evaluate(scene => {
      const count = scene.name === 'tracker-5000' ? document.querySelector('.synth-pattern-heading')?.textContent.match(/of (\d+) mapped/)?.[1]
        : scene.name === 'diff-10000' ? document.querySelector(scene.scroll)?.previousElementSibling?.querySelector('span')?.textContent.split(' displayed')[0].replace(/\D/g, '')
        : document.querySelectorAll('[data-slice-key]').length;
      return { eventTimingSupported: PerformanceObserver.supportedEntryTypes.includes('event'), events: window.__eventTimings, logicalRows: Number(count),
        mountedRows: document.querySelectorAll(scene.rows).length, visibility: document.visibilityState,
        apiRequests: performance.getEntriesByType('resource').filter(entry => new URL(entry.name).pathname.startsWith('/api/')).length };
    }, scene);
    const deltas = frames.slice(1).map((time, index) => time - frames[index]).sort((a, b) => a - b);
    const frameIntervalP95Ms = deltas[Math.floor(deltas.length * .95)];
    const maximumSampledInteractionMs = state.events.length ? Math.max(...state.events.map(event => event.duration)) : null;
    const clickToPaneAndTwoFrameOpportunitiesMs = pane - point.time;
    const workload = scene.name === 'tracker-5000' ? state.logicalRows >= 5000 && state.mountedRows <= 200
      : scene.name === 'diff-10000' ? state.logicalRows >= 10000 && state.mountedRows <= 128 : state.logicalRows === 100;
    const pass = workload && state.visibility === 'visible' && state.eventTimingSupported && state.apiRequests === 0 && externalAttempts === 0
      && Number.isFinite(idleRendererTaskCpuPercent) && idleRendererTaskCpuPercent < 1 && deltas.length >= 30 && frameIntervalP95Ms <= 17
      && (maximumSampledInteractionMs === null || maximumSampledInteractionMs < 50) && clickToPaneAndTwoFrameOpportunitiesMs < 100;
    report.views.push({ name: scene.name, pass, clickToPaneAndTwoFrameOpportunitiesMs, coldContentReadyMs: content - point.time,
      idleRendererTaskCpuPercent, frameIntervalP95Ms, maximumSampledInteractionMs, sampledFrames: frames.length, externalAttempts, ...state });
    await page.close();
  }
  await writeFile(reportPath, JSON.stringify(report, null, 2)); console.log(JSON.stringify(report, null, 2)); console.log(`Report: ${reportPath}`);
  if (report.views.some(view => !view.pass)) throw Error('A scoped fixture budget failed; see the report.');
} finally { await cleanup(); }
