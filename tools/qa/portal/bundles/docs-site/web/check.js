// Reports whether the host page API exists, and whether the CSP blocks an undeclared fetch (canary).
const apiLabel = document.querySelector('[data-fe-probe="api"]');
apiLabel.textContent = typeof window.fe === 'undefined' ? 'api-absent' : 'api-present';
fetch('https://example.invalid/canary', { mode: 'no-cors' })
  .then(() => { apiLabel.dataset.csp = 'canary-loaded'; })
  .catch(() => { apiLabel.dataset.csp = 'canary-blocked'; });
