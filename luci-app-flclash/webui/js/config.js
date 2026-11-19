window.FlClashConfig = (function () {
  'use strict';

  const params = new URLSearchParams(window.location.search);
  const meta = document.querySelector('meta[name="flclash-api-base"]');
  const stored = window.localStorage.getItem('flclash.theme');
  const prefersDark = window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches;

  return {
    apiBase: params.get('api') || window.FLCLASH_API_BASE || (meta && meta.content) || '/flclash/api',
    secret: params.get('secret') || window.FLCLASH_SECRET || '',
    pollInterval: Number(window.FLCLASH_POLL_INTERVAL) || 2000,
    timeout: Number(window.FLCLASH_TIMEOUT) || 12000,
    mock: params.has('mock') || window.FLCLASH_MOCK === true,
    theme: stored || (window.innerWidth < 860 ? 'dark' : (prefersDark ? 'dark' : 'light'))
  };
})();
