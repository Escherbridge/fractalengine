// Dashboard fixture: renders host-pushed node, readings and query deliveries into fixed probe elements.
const probe = (name) => document.querySelector(`[data-fe-probe="${name}"]`);
let targetNodeId = null;
let readingsSeen = 0;

fetch('https://example.invalid/canary', { mode: 'no-cors' })
  .then(() => { probe('csp').textContent = 'canary-loaded'; })
  .catch(() => { probe('csp').textContent = 'canary-blocked'; });

function render(delivery) {
  const node = delivery.data && (delivery.data.node || delivery.data);
  if (!node || node.name !== 'Probe Target') return;
  targetNodeId = node.id;
  probe('node-name').textContent = node.name;
  const position = node.position || [0, 0, 0];
  probe('node-position').textContent = position.map((v) => Number(v).toFixed(2)).join(',');
}

async function start() {
  if (!window.fe) { probe('status').textContent = 'api-absent'; return; }
  const hello = await window.fe.hello();
  probe('status').textContent = 'hello:' + hello.capabilities.join('+');
  window.fe.subscribe('node', { petal_id: hello.context.petal_id }, render);
  window.fe.subscribe('readings', { petal_id: hello.context.petal_id }, () => {
    readingsSeen += 1;
    probe('readings-count').textContent = String(readingsSeen);
  });
  window.fe.subscribe('query', { name: 'node_count' }, (delivery) => {
    const row = delivery.data && delivery.data.rows && delivery.data.rows[0];
    if (row) probe('query-node-count').textContent = String(row.n);
  });
  document.querySelector('[data-fe-slot="1"]').addEventListener('click', () => {
    if (targetNodeId) window.fe.intent({ focus_node: targetNodeId });
  });
}
start().catch((error) => { probe('status').textContent = 'error:' + error.message; });
