/* FlClash WebUI — API client. One method per backend contract endpoint. */
window.FlClashAPI = (function () {
  'use strict';

  const cfg = window.FlClashConfig;
  const mock = window.FlClashMock;

  function url(path) {
    return cfg.apiBase.replace(/\/$/, '') + path;
  }

  function withTimeout(ms) {
    const controller = new AbortController();
    const timer = window.setTimeout(function () { controller.abort(); }, ms);
    return { signal: controller.signal, done: function () { window.clearTimeout(timer); } };
  }

  async function request(path, options) {
    const init = Object.assign({
      method: 'GET',
      headers: { Accept: 'application/json' },
      credentials: 'same-origin',
      cache: 'no-store'
    }, options || {});

    init.headers = Object.assign({}, init.headers);
    if (cfg.secret) init.headers['X-FlClash-Secret'] = cfg.secret;
    if (init.body && typeof init.body !== 'string') {
      init.headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(init.body);
    }

    const guard = withTimeout(cfg.timeout);
    init.signal = guard.signal;
    let response;
    try {
      response = await fetch(url(path), init);
    } finally {
      guard.done();
    }

    if (!response.ok) {
      let detail = response.statusText;
      try {
        const payload = await response.json();
        detail = payload.error || payload.message || detail;
      } catch (error) { /* keep statusText */ }
      const failure = new Error(String(detail || '请求失败'));
      failure.status = response.status;
      throw failure;
    }
    if (response.status === 204) return { ok: true };
    const text = await response.text();
    if (!text) return { ok: true };
    try {
      return JSON.parse(text);
    } catch (error) {
      throw new Error('响应不是合法 JSON');
    }
  }

  function live(method, path, body) {
    if (cfg.mock && mock && typeof mock[method] === 'function') {
      return mock[method](body);
    }
    const options = { method: path.method || 'GET' };
    if (body !== undefined) options.body = body;
    return request(path.path || path, options);
  }

  return {
    url: url,
    request: request,
    status: function () { return live('status', '/status'); },
    memory: function () { return live('memory', '/memory'); },
    profiles: function () { return live('profiles', '/profiles'); },
    proxies: function () { return live('proxies', '/proxies'); },
    connections: function () { return live('connections', '/connections'); },
    logs: function () { return live('logs', '/logs'); },
    config: function () { return live('config', '/config'); },
    version: function () { return live('version', '/version'); },
    rules: function () { return live('rules', '/rules'); },
    providers: function () { return live('providers', '/providers/proxies'); },
    proxyStatus: function () { return live('proxyStatus', '/proxy'); },
    proxyToggle: function (action) {
      return live('proxyToggle', { path: '/proxy/' + encodeURIComponent(action), method: 'POST' });
    },
    dnsResolve: function (name, type) {
      if (cfg.mock && mock && typeof mock.dnsResolve === 'function') {
        return mock.dnsResolve(name, type);
      }
      return request('/dns/resolve?name=' + encodeURIComponent(name) + '&type=' + encodeURIComponent(type || 'A'));
    },

    systemProxy: function () {
      if (cfg.mock && mock && typeof mock.systemProxy === 'function') {
        return mock.systemProxy();
      }
      return live('systemProxy', '/system/proxy');
    },

    /* Core management lives on the same origin as the WebUI (flclash-api).
     * The rpcd ubus object remains for LuCI-side callers, but the WebUI must
     * not depend on a cross-origin /ubus that this origin does not serve. */
    coreInfo: function () {
      if (cfg.mock && mock && typeof mock.coreInfo === 'function') return mock.coreInfo();
      return live('coreInfo', '/core');
    },
    coreUpdate: function (core) {
      if (cfg.mock && mock && typeof mock.coreUpdate === 'function') return mock.coreUpdate(core);
      return live('coreUpdate', { path: '/core/update', method: 'POST' }, { core: core });
    },
    coreUpdateStatus: function () {
      if (cfg.mock && mock && typeof mock.coreUpdateStatus === 'function') return mock.coreUpdateStatus();
      return live('coreUpdateStatus', '/core/update/status');
    },
    coreSwitch: function (core) {
      if (cfg.mock && mock && typeof mock.coreSwitch === 'function') return mock.coreSwitch(core);
      return live('coreSwitch', { path: '/core/switch', method: 'POST' }, { core: core });
    },

    importProfile: function (profileUrl) {
      return live('importProfile', { path: '/profiles/import', method: 'POST' }, { url: profileUrl });
    },
    updateProfile: function (id) {
      return live('updateProfile', { path: '/profiles/' + encodeURIComponent(id) + '/update', method: 'POST' });
    },
    editProfile: function (id, patch) {
      return live('editProfile', { path: '/profiles/' + encodeURIComponent(id), method: 'PATCH' }, patch);
    },
    applyProfile: function (id) {
      return live('applyProfile', { path: '/profiles/' + encodeURIComponent(id) + '/apply', method: 'POST' });
    },
    deleteProfile: function (id) {
      return live('deleteProfile', { path: '/profiles/' + encodeURIComponent(id), method: 'DELETE' });
    },
    selectProxy: function (group, name, extra) {
      const body = Object.assign({ group: group, name: name }, extra || {});
      return live('selectProxy', { path: '/proxies/select', method: 'POST' }, body);
    },
    testDelay: function (name, group) {
      const body = group ? { name: name, group: group } : { name: name };
      return live('testDelay', { path: '/proxies/delay', method: 'POST' }, body);
    },
    testGroupDelay: function (group) {
      return live('testGroupDelay', { path: '/proxies/group-delay', method: 'POST' }, { group: group });
    },
    closeConnection: function (id) {
      return live('closeConnection', { path: '/connections/' + encodeURIComponent(id), method: 'DELETE' });
    },
    flushDns: function () { return live('flushDns', { path: '/dns/flush', method: 'POST' }); },
    saveConfig: function (patch) { return live('saveConfig', { path: '/config', method: 'PUT' }, patch); },
    service: function (action) {
      return live('service', { path: '/service/' + encodeURIComponent(action), method: 'POST' });
    },

    /* ---- mixin & config editor ---- */
    mixin: function () { return live('mixin', '/mixin'); },
    saveMixin: function (text) {
      return live('saveMixin', { path: '/mixin', method: 'PUT' }, { text: text });
    },
    validateConfig: function (payload) {
      return live('validateConfig', { path: '/config/validate', method: 'POST' }, payload || {});
    },
    profileRaw: function (id) {
      return live('profileRaw', { path: '/profiles/' + encodeURIComponent(id) + '/raw' });
    },
    saveProfileRaw: function (id, text) {
      return live('saveProfileRaw', { path: '/profiles/' + encodeURIComponent(id) + '/raw', method: 'PUT' }, { text: text });
    },

    /* ---- rule providers & geo data ---- */
    ruleProviders: function () { return live('ruleProviders', '/providers/rules'); },
    proxyProviders: function () { return live('providers', '/providers/proxies'); },
    updateProxyProvider: function (name) {
      return live('updateProxyProvider', { path: '/providers/proxies/' + encodeURIComponent(name) + '/update', method: 'POST' }, {});
    },
    updateRuleProvider: function (name) {
      return live('updateRuleProvider', { path: '/providers/rules/' + encodeURIComponent(name) + '/update', method: 'POST' }, {});
    },
    geoUpdate: function () { return live('geoUpdate', { path: '/system/geo', method: 'POST' }, {}); },

    /* ---- advanced DNS ---- */
    dnsConfig: function () { return live('dnsConfig', '/dns/config'); },

    /* ---- LAN ACL ---- */
    acl: function () { return live('acl', '/proxy/acl'); },
    saveAcl: function (payload) {
      return live('saveAcl', { path: '/proxy/acl', method: 'PUT' }, payload);
    },

    /* ---- backup & restore ---- */
    backupList: function () { return live('backupList', '/backup'); },
    backupCreate: function (label) {
      return live('backupCreate', { path: '/backup', method: 'POST' }, { label: label || 'manual' });
    },
    backupRestore: function (name) {
      return live('backupRestore', { path: '/backup/restore', method: 'POST' }, { name: name });
    },
    backupDelete: function (name) {
      return live('backupDelete', { path: '/backup/' + encodeURIComponent(name), method: 'DELETE' });
    },
    /* Raw gzip body — bypasses the JSON wrapper on purpose. */
    backupUpload: async function (bytes) {
      if (cfg.mock && mock && typeof mock.backupUpload === 'function') return mock.backupUpload(bytes);
      const headers = { 'Content-Type': 'application/gzip', Accept: 'application/json' };
      if (cfg.secret) headers['X-FlClash-Secret'] = cfg.secret;
      const response = await fetch(url('/backup/upload'), {
        method: 'POST', body: bytes, headers: headers, credentials: 'same-origin'
      });
      const payload = await response.json().catch(function () { return {}; });
      if (!response.ok) throw new Error(payload.error || '恢复失败');
      return payload;
    },
    // Authenticated blob download: REST auth accepts headers only, so the
    // secret never travels in a URL (no logs / history leakage).
    backupDownload: async function (name) {
      const headers = { Accept: 'application/gzip' };
      if (cfg.secret) headers['X-FlClash-Secret'] = cfg.secret;
      const response = await fetch(url('/backup/download/' + encodeURIComponent(name)), {
        headers: headers, credentials: 'same-origin', cache: 'no-store'
      });
      if (!response.ok) throw new Error('下载失败 (' + response.status + ')');
      return response.blob();
    }
  };
})();
