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

  /* `?api=` exists for reverse-proxy setups that serve the UI and the API
   * from a different path/host of the SAME router. Anything pointing at
   * another host would silently forward every poll — and the X-FlClash-Secret
   * header and the WS auth frame — to that host, so a crafted link must not
   * be able to exfiltrate the secret: reject foreign hosts and fall back to
   * the default base. */
  function resolveApiBase(candidate) {
    const fallback = (meta && meta.content) || '/flclash/api';
    if (!candidate) return fallback;
    try {
      const url = new URL(candidate, window.location.href);
      if (url.host !== window.location.host) {
        console.warn('flclash: ignoring cross-host ?api= target ' + url.host);
        return fallback;
      }
      return candidate;
    } catch (error) {
      return fallback;
    }
  }

  return {
    apiBase: resolveApiBase(params.get('api') || window.FLCLASH_API_BASE),
    secret: secret,
    pollInterval: Number(window.FLCLASH_POLL_INTERVAL) || 2000,
    timeout: Number(window.FLCLASH_TIMEOUT) || 12000,
    mock: params.has('mock') || window.FLCLASH_MOCK === true,
    theme: stored || (window.innerWidth < 860 ? 'dark' : (prefersDark ? 'dark' : 'light'))
  };
})();
