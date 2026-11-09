// Dashboard fixture: renders host-pushed node, readings and query deliveries into fixed probe elements.
const probe = (name) => document.querySelector(`[data-fe-probe="${name}"]`);
let targetNodeId = null;
let readingsSeen = 0;

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
// resync_required is followed by a fresh snapshot on this same subscription (ipc-v1 section 5.1): wait, never resubscribe.
function render(delivery) {
  const data = delivery.data || {};
  if (delivery.kind === 'resync_required' || delivery.kind === 'ended') return;
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

// Static path (in-app static sandbox or an export, static-export.md section 3): one code path, /__fe/feeds.json then fetch.
// The `feed` probe shows the first feed's HTTP status, or `expired` on 410 (PJ10).
async function staticFeeds() {
  probe('status').textContent = 'api-absent';
  try {
    const doc = await (await fetch('/__fe/feeds.json')).json();
    const first = Object.values((doc && doc.feeds) || {})[0];
    if (!first) { probe('feed').textContent = 'none'; return; }
    const response = await fetch(first.url);
    probe('feed').textContent = response.status === 410 ? 'expired' : String(response.status);
  } catch (error) { probe('feed').textContent = 'error'; }
}

async function start() {
  if (!window.fe) { await staticFeeds(); return; }
  const hello = await window.fe.hello();
  probe('status').textContent = 'hello:' + hello.capabilities.join('+');
  await window.fe.subscribe('node', { petal_id: hello.context.petal_id }, render);
  // readings params are {node_id?, metric?} only (ipc-v1 section 5.2); the stream is already confined to the binding petal.
  window.fe.subscribe('readings', {}, (delivery) => {
    if (delivery.kind !== 'delta') return;
    readingsSeen += ((delivery.data && delivery.data.readings) || []).length;
    probe('readings-count').textContent = String(readingsSeen);
  }).catch((error) => { probe('readings-count').textContent = 'error:' + error.code; });
  window.fe.subscribe('query', { name: 'node_count', args: {} }, (delivery) => {
    const row = delivery.data && delivery.data.data && delivery.data.data[0];
    if (row) probe('query-node-count').textContent = String(row.n);
  }).catch((error) => { probe('query-node-count').textContent = 'error:' + error.code; });
  document.querySelector('[data-fe-slot="1"]').addEventListener('click', () => {
    if (targetNodeId) window.fe.intent({ focus_node: targetNodeId });
  });
}
start().catch((error) => { probe('status').textContent = 'error:' + error.message; });
