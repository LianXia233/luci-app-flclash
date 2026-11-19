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
      return request('/system/proxy').catch(function () {
        return request(cfg.ubusBase + '?auth=' + encodeURIComponent(cfg.secret || ''), {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            method: 'call',
            params: ['flclash', 'system_proxy', {}]
          })
        });
      });
    },

    /* ---- core management (rpcd ubus `flclash`, mock-backed in demo mode) ---- */
    ubus: function (method, params) {
      return request(cfg.ubusBase + '?auth=' + encodeURIComponent(cfg.secret || ''), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ method: 'call', params: ['flclash', method, params || {}] })
      }).then(function (reply) {
        if (reply && reply.code) {
          const err = new Error(reply.error || '操作失败');
          err.code = reply.code;
          throw err;
        }
        return reply;
      });
    },
    coreInfo: function () {
      if (cfg.mock && mock && typeof mock.coreInfo === 'function') return mock.coreInfo();
      return this.ubus('core_type', {});
    },
    coreUpdate: function (core) {
      if (cfg.mock && mock && typeof mock.coreUpdate === 'function') return mock.coreUpdate(core);
      return this.ubus('core_update', { core: core });
    },
    coreUpdateStatus: function () {
      if (cfg.mock && mock && typeof mock.coreUpdateStatus === 'function') return mock.coreUpdateStatus();
      return this.ubus('core_update_status', {});
    },
    coreSwitch: function (core) {
      if (cfg.mock && mock && typeof mock.coreSwitch === 'function') return mock.coreSwitch(core);
      return this.ubus('core_switch', { core: core });
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
    }
  };
})();
