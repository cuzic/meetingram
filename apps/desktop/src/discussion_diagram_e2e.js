// Runs in the actual WebView while the synthetic recording is active.
// The fixture covers 30 items and the maximum depth and label length.
const tasks = [];
const observer = new PerformanceObserver(list => {
  tasks.push(...list.getEntries().map(entry => entry.duration));
});
observer.observe({ entryTypes: ['longtask'] });

async function trials(count, limit) {
  const root = { label: '議論の全体像', children: [] };
  const labels = ['記録の共有先を確認する', '代替案と反対理由を整理する', '確認事項😀 < > &'];
  const nodes = [root];
  for (let i = 1; i < count; i++) {
    const deep = count === 100 && i < 16;
    const label = i === 15 && deep ? '日本語の長い項目'.repeat(30) : labels[i % labels.length] + i;
    const node = { label, sub: i % 3 ? '反対 · 要確認' : '決定済み', children: [] };
    const parent = count === 100 ? (deep ? i - 1 : 0) : Math.floor((i - 1) / 3);
    nodes[parent].children.push(node);
    nodes.push(node);
  }
  let fast = 0;
  for (let i = 0; i < 10; i++) {
    const start = performance.now();
    const result = await globalThis.__meetingramDiagram({ version: 1, root });
    if (!result.svg) throw new Error('diagram_fixture_failed');
    const image = new Image();
    image.src = 'data:image/svg+xml,' + encodeURIComponent(result.svg);
    await image.decode();
    if (performance.now() - start <= limit) fast++;
    await new Promise(resolve => setTimeout(resolve, 0));
  }
  return fast;
}

try {
  const fast_30 = await trials(30, 1000);
  const fast_100 = await trials(100, 3000);
  const longest_task = Math.ceil(Math.max(0, ...tasks));
  return { fast_30, fast_100, longest_task };
} catch {
  return null;
} finally {
  observer.disconnect();
}
