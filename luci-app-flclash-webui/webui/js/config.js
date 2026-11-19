window.FlClashConfig = (function () {
  'use strict';

  const params = new URLSearchParams(window.location.search);
  const meta = document.querySelector('meta[name="flclash-api-base"]');
  const stored = window.localStorage.getItem('flclash.theme');
  const prefersDark = window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches;

  return {
    apiBase: params.get('api') || window.FLCLASH_API_BASE || (meta && meta.content) || '/flclash/api',
    // Live read-only system proxy source (rpcd ubus). Devices without this
    // endpoint simply leave it unset and the UI falls back to the Go API.
    ubusBase: params.get('ubus') || window.FLCLASH_UBUS_BASE || '/ubus',
    secret: params.get('secret') || window.FLCLASH_SECRET || '',
    pollInterval: Number(window.FLCLASH_POLL_INTERVAL) || 2000,
    timeout: Number(window.FLCLASH_TIMEOUT) || 12000,
    mock: params.has('mock') || window.FLCLASH_MOCK === true,
    theme: stored || (window.innerWidth < 860 ? 'dark' : (prefersDark ? 'dark' : 'light'))
  };
})();
