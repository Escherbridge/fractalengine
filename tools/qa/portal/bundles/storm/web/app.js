// Storm fixture (PJ7): slot 1 opens 64 subscriptions, floods ~500 msgs/s for 5 s, bursts 20 set_property in 1 s, then idles 60 s.
const probe = (name) => document.querySelector(`[data-fe-probe="${name}"]`);
const bump = (name) => { probe(name).textContent = String(Number(probe(name).textContent) + 1); };
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let hello = null;

async function runStorm() {
  probe('phase').textContent = 'subscribe';
  const kinds = ['node', 'readings', 'query'];
  for (let i = 0; i < 64; i += 1) {
    const kind = kinds[i % 3];
    // Valid params per stream (ipc-v1 section 5), so every refusal past the 16th is too_many_subscriptions, never invalid.
    const params = kind === 'query' ? { name: 'node_count' } : kind === 'node' ? { petal_id: hello.context.petal_id } : {};
    try { await window.fe.subscribe(kind, params, () => {}); bump('subs-ok'); } catch (error) { bump('subs-refused'); }
  }
  probe('phase').textContent = 'flood';
  const floodEnd = performance.now() + 5000;
  while (performance.now() < floodEnd) {
    for (let i = 0; i < 50; i += 1) window.fe.hello().catch(() => {});
    await sleep(100);
  }
  probe('phase').textContent = 'writes';
  const writes = [];
  for (let i = 0; i < 20; i += 1) {
    writes.push(window.fe.act({ op: 'set_property', node_id: hello.context.repository_node_id, key: 'qa_storm', value: i })
      .then(() => bump('writes-ok'), () => bump('writes-refused')));
    await sleep(50);
  }
  await Promise.all(writes);
  probe('phase').textContent = 'idle60';
  await sleep(60000);
  probe('phase').textContent = 'done';
}

async function start() {
  if (!window.fe) { probe('phase').textContent = 'api-absent'; return; }
  hello = await window.fe.hello();
  document.querySelector('[data-fe-slot="1"]').addEventListener('click', () => { runStorm(); });
}
start().catch((error) => { probe('phase').textContent = 'error:' + error.message; });
