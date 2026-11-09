// Actions fixture: slot 1 writes the declared key, slot 2 the key only v3 declares; localStorage proves origin continuity (PJ9).
const probe = (name) => document.querySelector(`[data-fe-probe="${name}"]`);
let counter = 0;
let repositoryNodeId = null;

fetch('build-id.txt').then((r) => r.text()).then((t) => { probe('build-id').textContent = t.trim(); }).catch(() => {});
try {
  localStorage.setItem('qa_origin_key', localStorage.getItem('qa_origin_key') || 'seeded');
  probe('storage').textContent = localStorage.getItem('qa_origin_key');
} catch (error) { probe('storage').textContent = 'storage-error'; }

// fe.act resolves {node_id, key} and rejects with FePortalError {code} (ipc-v1 section 4.2).
async function act(key, value) {
  try {
    const result = await window.fe.act({ op: 'set_property', node_id: repositoryNodeId, key, value });
    probe('last-reply').textContent = 'ok:' + result.key;
  } catch (error) { probe('last-reply').textContent = 'error:' + (error.code || error.message); }
}

async function start() {
  if (!window.fe) { probe('status').textContent = 'api-absent'; return; }
  const hello = await window.fe.hello();
  repositoryNodeId = hello.context.repository_node_id;
  probe('status').textContent = 'admitted';
  document.querySelector('[data-fe-slot="1"]').addEventListener('click', () => act('qa_counter', ++counter));
  document.querySelector('[data-fe-slot="2"]').addEventListener('click', () => act('qa_extra', ++counter));
}
start().catch((error) => { probe('status').textContent = 'error:' + error.message; });
