// Dashboard fixture: renders host-pushed node, readings and query deliveries into fixed probe elements.
const probe = (name) => document.querySelector(`[data-fe-probe="${name}"]`);
let targetNodeId = null;
let readingsSeen = 0;
let petalId = null;

fetch('https://example.invalid/canary', { mode: 'no-cors' })
  .then(() => { probe('csp').textContent = 'canary-loaded'; })
  .catch(() => { probe('csp').textContent = 'canary-blocked'; });

const TARGET_NAME = 'Probe Target';

function nodeKey(node) { return node.id || node.node_id; }

function showPosition(position) {
  probe('node-position').textContent = (position || [0, 0, 0]).map((v) => Number(v).toFixed(2)).join(',');
}

function showTarget(node) {
  targetNodeId = nodeKey(node);
  probe('node-name').textContent = node.name;
  showPosition(node.position);
}

// A node delivery is a /ws body: snapshot parts carry data.nodes[], deltas carry data.changes[] (SceneChange, tag "op").
function render(delivery) {
  const data = delivery.data || {};
  if (delivery.kind === 'resync_required') { window.fe.subscribe('node', { petal_id: petalId }, render).catch(() => {}); return; }
  if (delivery.kind === 'snapshot') {
    const match = (data.nodes || []).find((node) => node.name === TARGET_NAME);
    if (match) showTarget(match);
    return;
  }
  for (const change of data.changes || []) {
    if (change.op === 'node_added' && change.node && change.node.name === TARGET_NAME) showTarget(change.node);
    else if (change.node_id && change.node_id === targetNodeId && change.op === 'node_transform') showPosition(change.position);
    else if (change.node_id && change.node_id === targetNodeId && change.op === 'node_renamed') probe('node-name').textContent = change.new_name;
  }
}

async function start() {
  if (!window.fe) { probe('status').textContent = 'api-absent'; return; }
  const hello = await window.fe.hello();
  petalId = hello.context.petal_id;
  probe('status').textContent = 'hello:' + hello.capabilities.join('+');
  await window.fe.subscribe('node', { petal_id: hello.context.petal_id }, render);
  window.fe.subscribe('readings', { petal_id: hello.context.petal_id }, () => {
    readingsSeen += 1;
    probe('readings-count').textContent = String(readingsSeen);
  });
  window.fe.subscribe('query', { name: 'node_count', args: {} }, (delivery) => {
    const row = delivery.data && delivery.data.data && delivery.data.data[0];
    if (row) probe('query-node-count').textContent = String(row.n);
  });
  document.querySelector('[data-fe-slot="1"]').addEventListener('click', () => {
    if (targetNodeId) window.fe.intent({ focus_node: targetNodeId });
  });
}
start().catch((error) => { probe('status').textContent = 'error:' + error.message; });
