window.FlClashConfig = (function () {
  'use strict';

  const params = new URLSearchParams(window.location.search);
  const meta = document.querySelector('meta[name="flclash-api-base"]');
  const stored = window.localStorage.getItem('flclash.theme');
  const prefersDark = window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches;

  /* The API secret may arrive once via `?secret=` (LuCI jump link). Persist it
   * in sessionStorage and drop it from the address bar so it does not end up
   * in browser history, bookmarks or a reverse-proxy access log. */
  let secret = params.get('secret') || '';
  try {
    if (secret) {
      window.sessionStorage.setItem('flclash.secret', secret);
      params.delete('secret');
      const query = params.toString();
      window.history.replaceState(null, '',
        window.location.pathname + (query ? '?' + query : '') + window.location.hash);
    } else {
      secret = window.sessionStorage.getItem('flclash.secret') || '';
    }
  } catch (error) { /* private mode / storage disabled */ }
  if (!secret) secret = window.FLCLASH_SECRET || '';

  return {
    apiBase: params.get('api') || window.FLCLASH_API_BASE || (meta && meta.content) || '/flclash/api',
    secret: secret,
    pollInterval: Number(window.FLCLASH_POLL_INTERVAL) || 2000,
    timeout: Number(window.FLCLASH_TIMEOUT) || 12000,
    mock: params.has('mock') || window.FLCLASH_MOCK === true,
    theme: stored || (window.innerWidth < 860 ? 'dark' : (prefersDark ? 'dark' : 'light'))
  };
})();
