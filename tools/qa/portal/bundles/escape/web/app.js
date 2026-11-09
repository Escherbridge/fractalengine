// Escape fixture: each numbered slot is one escape attempt; slots 7 and 8 are script-driven, the rest are native navigations.
const probe = (name) => document.querySelector(`[data-fe-probe="${name}"]`);
document.querySelectorAll('[data-fe-slot]').forEach((element) => {
  element.addEventListener('click', () => { probe('last-slot').textContent = element.dataset.feSlot; });
});
document.querySelector('[data-fe-slot="7"]').addEventListener('click', () => {
  window.open('https://undeclared.example.net/', '_blank');
});
document.querySelector('[data-fe-slot="8"]').addEventListener('click', () => {
  const meta = document.createElement('meta');
  meta.httpEquiv = 'refresh';
  meta.content = '0;url=https://undeclared.example.net/';
  document.head.appendChild(meta);
});
probe('status').textContent = 'ready';
